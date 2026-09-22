//! Local-only storage for the WebDAV app password, backed by the Windows
//! credential manager. Nothing here touches the vault, `gateway.json` or the
//! network: the secret stays under the current Windows account and can be
//! inspected or deleted in the Credential Manager control panel.
//!
//! A password injected by a trusted producer through `--secrets-stdin` is never
//! written here. Only one a person typed at the hidden prompt is remembered.
use zeroize::Zeroizing;

/// The credential manager caps a generic credential's blob at 512 bytes.
pub const MAX_STORED_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    TooLarge { bytes: usize },
    Os { code: u32 },
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
            Self::Unsupported => "this platform has no supported credential store".to_string(),
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
    use super::StoreError;
    use zeroize::Zeroizing;

    pub fn load(_target: &str) -> Option<Zeroizing<String>> {
        None
    }

    pub fn save(_target: &str, _username: &str, _password: &str) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    pub fn forget(_target: &str) -> Result<bool, StoreError> {
        Ok(false)
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
}
