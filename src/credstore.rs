//! Local-only storage for the WebDAV app password, backed by whatever keychain this
//! machine already has: the Windows credential manager, the macOS keychain through
//! `/usr/bin/security`, or a Secret Service provider through `secret-tool`. Nothing here
//! touches the vault, `gateway.json` or the network, and a remembered password is never
//! an argument to a spawned program — it only ever travels on that program's stdin.
//!
//! A password injected by a trusted producer through `--secrets-stdin` is never written
//! here. Only one a person typed at the hidden prompt is remembered.
use zeroize::Zeroizing;

/// Windows caps a generic credential's blob at 512 bytes. The same ceiling is applied on
/// every platform so a remembered password never depends on which machine stored it.
pub const MAX_STORED_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    TooLarge {
        bytes: usize,
    },
    Os {
        code: u32,
    },
    /// The unix helpers take the secret as one line on stdin, so a value containing a
    /// newline would be stored truncated rather than refused.
    Multiline {
        bytes: usize,
    },
    /// A keychain helper ran and refused the request. Only its name and exit status are
    /// reported: its own message can quote the item label we passed it.
    Helper {
        program: &'static str,
        code: i32,
    },
    /// No supported keychain helper could be started on this machine.
    Unsupported,
}

impl StoreError {
    /// A short technical detail for a note. Never contains the secret itself.
    pub fn reason(self) -> String {
        match self {
            Self::TooLarge { bytes } => {
                format!("secret is {bytes} bytes, limit is {MAX_STORED_BYTES}")
            }
            Self::Os { code } => format!("credential manager rejected the write (error {code})"),
            Self::Multiline { bytes } => {
                format!("secret is {bytes} bytes across more than one line")
            }
            Self::Helper { program, code } => {
                format!("{program} refused the request (exit {code})")
            }
            Self::Unsupported => "no supported credential store is available".to_string(),
        }
    }
}

/// Stable, human-readable credential target for one WebDAV account.
pub fn target_name(base_url: &str, username: &str) -> String {
    let host = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or(base_url);
    format!("Monica CLI/webdav/{host}/{username}")
}

pub fn load(base_url: &str, username: &str) -> Option<Zeroizing<String>> {
    platform::load(&target_name(base_url, username))
}

pub fn present(base_url: &str, username: &str) -> bool {
    load(base_url, username).is_some()
}

pub fn save(base_url: &str, username: &str, password: &str) -> Result<(), StoreError> {
    if password.len() > MAX_STORED_BYTES {
        return Err(StoreError::TooLarge {
            bytes: password.len(),
        });
    }
    platform::save(&target_name(base_url, username), username, password)
}

/// Removes the stored password. `false` means there was nothing to remove.
pub fn forget(base_url: &str, username: &str) -> Result<bool, StoreError> {
    platform::forget(&target_name(base_url, username))
}

#[cfg(windows)]
mod platform {
    use super::StoreError;
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use zeroize::{Zeroize, Zeroizing};

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    pub fn load(target: &str) -> Option<Zeroizing<String>> {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if ok == 0 || credential.is_null() {
            return None;
        }
        let stored = unsafe {
            let credential = &*credential;
            let blob = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            std::str::from_utf8(blob)
                .map(|text| Zeroizing::new(text.to_owned()))
                .ok()
        };
        unsafe {
            let credential = &mut *credential;
            let length = credential.CredentialBlobSize as usize;
            if length > 0 && !credential.CredentialBlob.is_null() {
                let blob = std::slice::from_raw_parts_mut(credential.CredentialBlob, length);
                blob.zeroize();
            }
            credential.CredentialBlobSize = 0;
            CredFree(credential as *mut CREDENTIALW as *const c_void);
        }
        stored
    }

    pub fn save(target: &str, username: &str, password: &str) -> Result<(), StoreError> {
        let mut target_name = wide(target);
        let mut user_name = wide(username);
        let mut blob = Zeroizing::new(password.as_bytes().to_vec());
        let ok = unsafe {
            let credential = CREDENTIALW {
                Type: CRED_TYPE_GENERIC,
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                TargetName: target_name.as_mut_ptr(),
                UserName: user_name.as_mut_ptr(),
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                ..Default::default()
            };
            CredWriteW(&credential, 0)
        };
        if ok == 0 {
            return Err(StoreError::Os {
                code: unsafe { GetLastError() } as u32,
            });
        }
        Ok(())
    }

    pub fn forget(target: &str) -> Result<bool, StoreError> {
        let target_name = wide(target);
        let ok = unsafe { CredDeleteW(target_name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            let code = unsafe { GetLastError() } as u32;
            if code == ERROR_NOT_FOUND {
                return Ok(false);
            }
            return Err(StoreError::Os { code });
        }
        Ok(true)
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{StoreError, unix};
    use zeroize::Zeroizing;

    pub fn load(target: &str) -> Option<Zeroizing<String>> {
        unix::load(target)
    }

    pub fn save(target: &str, username: &str, password: &str) -> Result<(), StoreError> {
        unix::save(target, username, password)
    }

    pub fn forget(target: &str) -> Result<bool, StoreError> {
        unix::forget(target)
    }
}

/// The external-command half of this module: which program, which options, and where the
/// password goes. It is written with plain standard library calls and compiled on every
/// platform under `cargo test`, so the argument lists and the pipe plumbing are checked
/// even where no keychain exists. The dispatch below only exists on a Mac or a Linux
/// desktop, and a real `secret-tool` has been reached from it on two Linux machines —
/// one without the tool, one with a provider-less install. No `/usr/bin/security` call
/// has ever run: this workspace has no macOS host.
#[cfg(any(unix, test))]
mod unix {
    use super::StoreError;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use zeroize::Zeroizing;

    /// Attribute namespace for the Secret Service on Linux. Items are keyed by the account
    /// target as well, so two WebDAV servers never share one entry.
    const SERVICE: &str = "monica-pass-cli";

    /// One keychain call. `secret` is the only place a password is held, and it goes to
    /// the child's stdin, never into `args`.
    pub struct Invocation {
        pub program: &'static str,
        pub args: Vec<String>,
        pub secret: Option<Zeroizing<Vec<u8>>>,
    }

    // Diagnostics print an invocation, so the password has to stay out of it.
    impl std::fmt::Debug for Invocation {
        fn fmt(&self, form: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            form.debug_struct("Invocation")
                .field("program", &self.program)
                .field("args", &self.args)
                .field(
                    "secret",
                    &self.secret.as_ref().map_or("none", |_| "<stdin>"),
                )
                .finish()
        }
    }

    impl Invocation {
        pub(super) fn new(program: &'static str, args: &[&str]) -> Self {
            Self {
                program,
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                secret: None,
            }
        }

        pub(super) fn secret(mut self, secret: &str) -> Self {
            self.secret = Some(Zeroizing::new(secret.as_bytes().to_vec()));
            self
        }

        /// Runs the child with stdout captured and stderr dropped: a helper's own message
        /// can quote the item label, and none of it is ours to repeat.
        pub fn run(&self) -> Result<Zeroizing<Vec<u8>>, StoreError> {
            let mut child = Command::new(self.program)
                .args(&self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| StoreError::Unsupported)?;
            // Dropping the handle closes stdin, which is what tells a line-reading helper
            // that the secret has ended. A child that quits before reading anything is
            // reported by its exit status below, not by the write error.
            if let (Some(mut stdin), Some(secret)) = (child.stdin.take(), &self.secret) {
                let _ = stdin.write_all(secret).and_then(|()| stdin.flush());
            }
            let output = child
                .wait_with_output()
                .map_err(|_| StoreError::Unsupported)?;
            if !output.status.success() {
                return Err(StoreError::Helper {
                    program: self.program,
                    code: output.status.code().unwrap_or(-1),
                });
            }
            Ok(Zeroizing::new(output.stdout))
        }
    }

    /// The keychain helper this system is expected to have. Everything else reports the
    /// same honest "nothing can remember the password here" as a missing helper.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Helper {
        Keychain,
        SecretService,
    }

    pub fn helper() -> Option<Helper> {
        if cfg!(target_os = "macos") {
            Some(Helper::Keychain)
        } else if cfg!(target_os = "linux") {
            Some(Helper::SecretService)
        } else {
            None
        }
    }

    impl Helper {
        pub(super) fn load(self, target: &str) -> Invocation {
            match self {
                // A trailing `-w` prints only the password attribute.
                Self::Keychain => Invocation::new(
                    "/usr/bin/security",
                    &["find-generic-password", "-s", target, "-w"],
                ),
                Self::SecretService => Invocation::new(
                    "secret-tool",
                    &["lookup", "service", SERVICE, "target", target],
                ),
            }
        }

        pub(super) fn store(self, target: &str, username: &str, secret: &str) -> Invocation {
            match self {
                // `-U` replaces an existing item and `-w` stays last, so the password can
                // never be read back as the value of another option.
                Self::Keychain => Invocation::new(
                    "/usr/bin/security",
                    &[
                        "add-generic-password",
                        "-a",
                        username,
                        "-s",
                        target,
                        "-U",
                        "-w",
                    ],
                )
                .secret(secret),
                Self::SecretService => Invocation::new(
                    "secret-tool",
                    &[
                        "store",
                        &format!("--label={target}"),
                        "service",
                        SERVICE,
                        "target",
                        target,
                    ],
                )
                .secret(secret),
            }
        }

        pub(super) fn delete(self, target: &str) -> Invocation {
            match self {
                Self::Keychain => Invocation::new(
                    "/usr/bin/security",
                    &["delete-generic-password", "-s", target],
                ),
                Self::SecretService => Invocation::new(
                    "secret-tool",
                    &["clear", "service", SERVICE, "target", target],
                ),
            }
        }
    }

    pub fn load(target: &str) -> Option<Zeroizing<String>> {
        let stdout = helper()?.load(target).run().ok()?;
        one_line(&stdout)
    }

    pub fn save(target: &str, username: &str, secret: &str) -> Result<(), StoreError> {
        if secret.contains(['\n', '\r']) {
            return Err(StoreError::Multiline {
                bytes: secret.len(),
            });
        }
        let helper = helper().ok_or(StoreError::Unsupported)?;
        helper.store(target, username, secret).run()?;
        Ok(())
    }

    /// A helper answers a missing item with a non-zero exit, so absence is established by
    /// reading the entry first rather than by guessing at an exit code.
    pub fn forget(target: &str) -> Result<bool, StoreError> {
        let Some(helper) = helper() else {
            return Ok(false);
        };
        if load(target).is_none() {
            return Ok(false);
        }
        helper.delete(target).run()?;
        Ok(true)
    }

    /// Helpers print the secret followed by one newline. Anything else — nothing, a second
    /// line, bytes that are not text — is not a password this program stored.
    pub(super) fn one_line(stdout: &[u8]) -> Option<Zeroizing<String>> {
        let end = stdout.len() - usize::from(stdout.last() == Some(&b'\n'));
        let body = stdout.get(..end)?;
        if body.is_empty() || body.contains(&b'\n') {
            return None;
        }
        Some(Zeroizing::new(String::from_utf8(body.to_vec()).ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_stay_under_the_credential_limit_and_name_the_account() {
        let target = target_name("https://dav.jianguoyun.com/dav/", "me@example.com");
        assert_eq!(
            target,
            "Monica CLI/webdav/dav.jianguoyun.com/me@example.com"
        );
        assert!(target.chars().count() < 512);
        // The path below the host must not leak into the target name.
        assert!(!target.contains("/dav/"));
    }

    #[test]
    fn an_oversized_secret_is_refused_before_any_write() {
        let password = "x".repeat(MAX_STORED_BYTES + 1);
        assert_eq!(
            save("https://dav.example/dav/", "someone", &password),
            Err(StoreError::TooLarge {
                bytes: MAX_STORED_BYTES + 1
            })
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_stored_secret_round_trips_and_forgets_under_a_unique_target() {
        let _turn = crate::test_support::credential_turn()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let username = format!("roundtrip-{}", uuid::Uuid::new_v4());
        let base = "https://credential-test.invalid/dav/";
        assert!(load(base, &username).is_none());
        save(base, &username, "app-password-1").expect("store the test secret");
        assert_eq!(
            load(base, &username)
                .map(|secret| secret.to_string())
                .as_deref(),
            Some("app-password-1")
        );
        assert!(present(base, &username));
        save(base, &username, "app-password-2").expect("replace the test secret");
        assert_eq!(
            load(base, &username)
                .map(|secret| secret.to_string())
                .as_deref(),
            Some("app-password-2")
        );
        assert!(forget(base, &username).expect("remove the test secret"));
        assert!(!forget(base, &username).expect("a second removal is not an error"));
        assert!(!present(base, &username));
    }

    #[cfg(any(unix, test))]
    mod unix_backends {
        use super::super::unix::{Helper, Invocation, helper, one_line};
        use super::*;

        const TARGET: &str = "Monica CLI/webdav/dav.example.org/me@example.com";
        const USERNAME: &str = "me@example.com";
        const SECRET: &str = "remembered-app-password";

        fn store(helper: Helper) -> Invocation {
            helper.store(TARGET, USERNAME, SECRET)
        }

        #[test]
        fn a_remembered_secret_never_reaches_an_argument_list() {
            for helper in [Helper::Keychain, Helper::SecretService] {
                let invocation = store(helper);
                assert!(
                    invocation.args.iter().all(|arg| !arg.contains(SECRET)),
                    "{invocation:?}"
                );
                assert_eq!(
                    invocation.secret.as_ref().map(|bytes| bytes.to_vec()),
                    Some(SECRET.as_bytes().to_vec()),
                    "the password has to travel on stdin instead: {invocation:?}"
                );
                // Debug output is what a note or a test failure would print.
                assert!(!format!("{invocation:?}").contains(SECRET));
            }
        }

        #[test]
        fn the_keychain_options_end_with_the_bare_write_flag() {
            let invocation = store(Helper::Keychain);
            assert_eq!(invocation.program, "/usr/bin/security");
            assert_eq!(
                invocation.args.first().map(String::as_str),
                Some("add-generic-password")
            );
            assert_eq!(
                invocation.args.last().map(String::as_str),
                Some("-w"),
                "a value after -w would be an argument, and an argument is readable"
            );
            assert!(
                invocation.args.contains(&"-U".to_string()),
                "{invocation:?}"
            );
            assert!(invocation.args.contains(&TARGET.to_string()));
            assert!(invocation.args.contains(&USERNAME.to_string()));
        }

        #[test]
        fn store_lookup_and_clear_describe_the_same_secret_service_item() {
            let invocation = store(Helper::SecretService);
            assert_eq!(invocation.program, "secret-tool");
            assert_eq!(invocation.args.first().map(String::as_str), Some("store"));
            assert!(
                invocation
                    .args
                    .iter()
                    .any(|arg| arg.starts_with("--label=") && arg.contains(TARGET))
            );
            let keys = |command: &Invocation| {
                command
                    .args
                    .windows(2)
                    .filter(|pair| pair[0] == "service" || pair[0] == "target")
                    .map(|pair| format!("{}={}", pair[0], pair[1]))
                    .collect::<Vec<_>>()
            };
            let stored = keys(&invocation);
            assert_eq!(
                stored,
                vec!["service=monica-pass-cli", &format!("target={TARGET}")],
                "{invocation:?}"
            );
            for command in [
                Helper::SecretService.load(TARGET),
                Helper::SecretService.delete(TARGET),
            ] {
                assert_eq!(keys(&command), stored, "{command:?}");
            }
        }

        #[test]
        fn only_a_single_line_answer_reads_back_as_a_password() {
            assert_eq!(
                one_line(format!("{SECRET}\n").as_bytes()).map(|secret| secret.to_string()),
                Some(SECRET.to_string())
            );
            // A helper that prints nothing, or more than the one line it was given, has not
            // answered with a stored password.
            let noise: Vec<Vec<u8>> = vec![
                Vec::new(),
                b"\n".to_vec(),
                format!("{SECRET}\n{SECRET}\n").into_bytes(),
                vec![0xff, b'x', b'\n'],
            ];
            for bytes in noise {
                assert!(one_line(&bytes).is_none(), "{bytes:?}");
            }
            assert_eq!(
                one_line(SECRET.as_bytes()).map(|secret| secret.to_string()),
                Some(SECRET.to_string()),
                "a helper that prints no trailing newline is still understood"
            );
        }

        #[test]
        fn a_multiline_secret_is_refused_rather_than_stored_truncated() {
            assert_eq!(
                super::super::unix::save(TARGET, USERNAME, "one\ntwo"),
                Err(StoreError::Multiline { bytes: 7 })
            );
        }

        #[test]
        fn a_refusal_names_the_helper_and_never_the_secret() {
            assert_eq!(
                StoreError::Helper {
                    program: "secret-tool",
                    code: 1
                }
                .reason(),
                "secret-tool refused the request (exit 1)"
            );
            assert!(
                !StoreError::Multiline { bytes: 40 }
                    .reason()
                    .contains(SECRET)
            );
            assert_eq!(
                StoreError::Unsupported.reason(),
                "no supported credential store is available"
            );
        }

        #[test]
        fn the_plumbing_feeds_stdin_and_reads_the_exit_status() {
            // `git hash-object --stdin` is a deterministic stand-in for a keychain helper:
            // it hashes exactly the bytes it reads, so the digest proves the text arrived
            // on stdin rather than in the argument list.
            if Invocation::new("git", &["--version"]).run().is_err() {
                eprintln!("skip: no git available to act as a child process");
                return;
            }
            let digest = Invocation::new("git", &["hash-object", "--stdin"])
                .secret("abc")
                .run()
                .expect("hash the stdin supplied to the child");
            assert_eq!(
                std::str::from_utf8(&digest).expect("git prints a hex digest"),
                "f2ba8f84ab5c1bce84a7b441cb1959cfc7093b7f\n"
            );
            assert_eq!(
                Invocation::new("git", &["hash-object", "--stdin", "no-such-file-9e2c"])
                    .run()
                    .err(),
                Some(StoreError::Helper {
                    program: "git",
                    code: 128
                }),
                "a helper that ran and refused has to look different from one missing"
            );
            assert_eq!(
                Invocation::new("no-such-helper-9e2c", &[]).run().err(),
                Some(StoreError::Unsupported)
            );
        }

        #[test]
        fn a_machine_without_a_helper_says_so_instead_of_inventing_one() {
            assert_eq!(
                helper().is_some(),
                cfg!(any(target_os = "macos", target_os = "linux"))
            );
            if helper().is_some() {
                // A real keychain is not this test's to write into.
                return;
            }
            assert_eq!(
                super::super::unix::load(TARGET).map(|secret| secret.to_string()),
                None
            );
            assert_eq!(super::super::unix::forget(TARGET), Ok(false));
            assert_eq!(
                super::super::unix::save(TARGET, USERNAME, SECRET),
                Err(StoreError::Unsupported)
            );
        }
    }
}
