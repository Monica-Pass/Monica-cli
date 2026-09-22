//! Clipboard bridge for the TUI. Only fields the vault already published as public
//! metadata reach this module: an SSH public key line, a key fingerprint, a GPG user
//! id. Private key material and gateway tokens have no code path here, and nothing in
//! this module reads the vault or decrypts a payload.
//!
//! Windows talks to the clipboard through the same win32 bindings the rest of the binary
//! uses. macOS and a Linux desktop go through that platform's own helper program
//! (`pbcopy`, `wl-copy`, `xclip` or `xsel`); the text always travels on the child's
//! stdin, never in its arguments.
use thiserror::Error;

/// Longest text accepted. A public key line, a fingerprint and a user id all stay far
/// below this, so a longer value means a whole key ring was handed over by mistake.
pub const MAX_CLIPBOARD_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ClipboardError {
    #[error("another window holds the clipboard, try again in a moment")]
    Busy,
    #[error("nothing to copy")]
    Empty,
    #[error("the text is too long for the clipboard ({characters} characters)")]
    TooLarge { characters: usize },
    #[error("clipboard rejected the text (os error {code})")]
    Os { code: u32 },
    #[error("clipboard helper {program} refused the text (exit {code})")]
    Helper { program: &'static str, code: i32 },
    #[error("no clipboard helper is available on this machine")]
    Unsupported,
}

/// Replaces the clipboard text. Callers pass an explicit public field.
pub fn copy_text(text: &str) -> Result<(), ClipboardError> {
    let characters = text.chars().count();
    if characters == 0 {
        return Err(ClipboardError::Empty);
    }
    if characters > MAX_CLIPBOARD_CHARS {
        return Err(ClipboardError::TooLarge { characters });
    }
    copy(text)
}

#[cfg(windows)]
fn copy(text: &str) -> Result<(), ClipboardError> {
    use std::ptr;
    use std::thread::sleep;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BUSY, GetLastError, HWND, WIN32_ERROR,
    };
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};

    // A process that is already holding the clipboard answers with one of these two;
    // anything else is a real failure and should not be retried.
    const CONTENTION: [WIN32_ERROR; 2] = [ERROR_ACCESS_DENIED, ERROR_BUSY];
    const RETRIES: usize = 8;
    const RETRY_WAIT: Duration = Duration::from_millis(5);

    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    for _ in 0..RETRIES {
        // SAFETY: a null window handle means the clipboard is owned by this thread
        // rather than a window, which is the documented shape for a console process.
        let opened = unsafe { OpenClipboard(ptr::null_mut::<std::ffi::c_void>() as HWND) };
        if opened != 0 {
            let result = write_text(&units);
            // SAFETY: every path that gets here opened the clipboard exactly once and
            // has not closed it yet.
            unsafe { CloseClipboard() };
            return result;
        }
        // SAFETY: reads this thread's last error code; no memory is dereferenced.
        let code = unsafe { GetLastError() };
        if !CONTENTION.contains(&code) {
            return Err(ClipboardError::Os { code });
        }
        sleep(RETRY_WAIT);
    }
    Err(ClipboardError::Busy)
}

#[cfg(windows)]
fn write_text(units: &[u16]) -> Result<(), ClipboardError> {
    use std::mem::size_of_val;
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, GlobalFree};
    use windows_sys::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    // CF_UNICODETEXT from winuser.h. windows-sys 0.61 only publishes the constant behind
    // the Win32_System_Ole feature, which would pull the whole ole module into the binary.
    const CF_UNICODETEXT: u32 = 13;

    let size = size_of_val(units);
    // SAFETY: the clipboard is open on this thread. `units` stays alive for the whole
    // block and `size` bytes are copied into it. The allocated block is transferred to
    // the clipboard on success and freed here on every failure path, never both.
    unsafe {
        if EmptyClipboard() == 0 {
            return Err(ClipboardError::Os {
                code: GetLastError(),
            });
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, size);
        if memory.is_null() {
            return Err(ClipboardError::Os {
                code: GetLastError(),
            });
        }
        let destination = GlobalLock(memory);
        if destination.is_null() {
            let code = GetLastError();
            GlobalFree(memory);
            return Err(ClipboardError::Os { code });
        }
        ptr::copy_nonoverlapping(units.as_ptr() as *const u8, destination as *mut u8, size);
        GlobalUnlock(memory);
        if SetClipboardData(CF_UNICODETEXT, memory).is_null() {
            let code = GetLastError();
            GlobalFree(memory);
            return Err(ClipboardError::Os { code });
        }
        // On success the clipboard owns the block: freeing it here would corrupt it.
        Ok(())
    }
}

#[cfg(not(windows))]
fn copy(text: &str) -> Result<(), ClipboardError> {
    unix::copy(text)
}

/// The external-helper half of this module: which program, which options, and where the
/// text goes. It uses only standard library calls and compiles on every platform under
/// `cargo test`, so the argument lists and the pipe plumbing are checked even where no
/// clipboard exists. No call below has been made to a real `pbcopy`, `wl-copy`, `xclip`
/// or `xsel` from this repository.
#[cfg(any(unix, test))]
mod unix {
    use super::ClipboardError;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// Tried in order. A desktop session has exactly one of these, so the first one that
    /// answers wins and the rest cost a failed spawn each.
    pub const MACOS: [(&str, &[&str]); 1] = [("pbcopy", &[])];
    pub const LINUX: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];

    pub fn helpers() -> &'static [(&'static str, &'static [&'static str])] {
        if cfg!(target_os = "macos") {
            &MACOS
        } else if cfg!(target_os = "linux") {
            &LINUX
        } else {
            &[]
        }
    }

    /// Why a helper did not take the text. A missing program is not a refusal: the next
    /// candidate still has a chance, and none of the answer is the user's problem to read.
    #[derive(Debug, PartialEq, Eq)]
    pub enum Failure {
        Missing,
        Refused(ClipboardError),
    }

    pub fn write(program: &'static str, args: &[&str], text: &str) -> Result<(), Failure> {
        let mut child = Command::new(program)
            .args(args)
            // The TUI owns this terminal: a helper must never read from it, and its own
            // messages must never reach it.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Failure::Missing)?;
        // Dropping the handle closes stdin, which is how a helper knows the text ended.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin
                .write_all(text.as_bytes())
                .and_then(|()| stdin.flush());
        }
        let status = child.wait().map_err(|_| Failure::Missing)?;
        if status.success() {
            return Ok(());
        }
        Err(Failure::Refused(ClipboardError::Helper {
            program,
            code: status.code().unwrap_or(-1),
        }))
    }

    pub fn copy(text: &str) -> Result<(), ClipboardError> {
        let mut refused = None;
        for (program, args) in helpers() {
            match write(program, args, text) {
                Ok(()) => return Ok(()),
                Err(Failure::Missing) => continue,
                Err(Failure::Refused(error)) => refused = Some(error),
            }
        }
        // A helper that ran and said no is worth reporting; a machine with none of them
        // installed gets the plain answer.
        Err(refused.unwrap_or(ClipboardError::Unsupported))
    }
}

#[cfg(test)]
mod tests {
    use super::ClipboardError;
    use super::unix::{Failure, LINUX, MACOS, copy, helpers, write};

    /// The text a helper would be handed, used only to check it never becomes an argument.
    const SAMPLE: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAJoey@example";

    #[test]
    fn each_platform_gets_the_session_clipboard_of_its_own_desktop() {
        // Pinned as data: on the wrong platform a helper is never spawned, so this is the
        // only place the option lists can be checked.
        assert_eq!(MACOS, [("pbcopy", &[][..])]);
        assert_eq!(
            LINUX,
            [
                ("wl-copy", &[][..]),
                ("xclip", &["-selection", "clipboard"][..]),
                ("xsel", &["--clipboard", "--input"][..]),
            ]
        );
        for (program, args) in MACOS.iter().chain(LINUX.iter()) {
            assert!(
                !program.is_empty(),
                "an empty program name cannot be spawned"
            );
            assert!(
                args.iter().all(|arg| !arg.contains(SAMPLE)),
                "the copied text must never be an argument: {program} {args:?}"
            );
        }
        assert_eq!(
            helpers().is_empty(),
            cfg!(not(any(target_os = "macos", target_os = "linux")))
        );
        if helpers().is_empty() {
            // Only checked where this machine has none of them: a test must not overwrite
            // the developer's clipboard on a Mac or a Linux desktop.
            assert_eq!(copy(SAMPLE), Err(ClipboardError::Unsupported));
        }
    }

    #[test]
    fn the_plumbing_feeds_stdin_and_reads_the_exit_status() {
        // `git` is a deterministic stand-in for a clipboard helper: it reads what it is
        // given and fails loudly on a missing file, so both outcomes are observable
        // without a desktop session.
        if write("git", &["--version"], "probe").is_err() {
            eprintln!("skip: no git available to act as a child process");
            return;
        }
        assert_eq!(
            write("git", &["hash-object", "-t", "blob", "--stdin"], "abc\n"),
            Ok(())
        );
        assert_eq!(
            write("git", &["hash-object", "no-such-file-9e2c"], "abc\n"),
            Err(Failure::Refused(ClipboardError::Helper {
                program: "git",
                code: 128
            })),
            "a helper that ran and refused must not look like one that is missing"
        );
        assert_eq!(
            write("no-such-helper-9e2c", &[], "abc\n"),
            Err(Failure::Missing)
        );
    }
}
