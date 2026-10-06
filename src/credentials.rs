//! Explicit per-operation vault factors. No global state, persisted key bytes or hardware claims.
use std::fs::File;
use std::io::Read;
use std::path::Path;

use zeroize::Zeroizing;

use crate::error::{GatewayError, Result};

pub const MAX_KEY_FILE_BYTES: usize = 1024 * 1024;

/// Existing password-only callers remain valid for legacy modes. A second factor is never
/// silently discarded: the native engine must authenticate the supplied combination.
pub trait VaultPassword: AsRef<str> + Sync {
    fn security_key(&self) -> Option<&[u8]> {
        None
    }
}

impl VaultPassword for str {}
impl VaultPassword for String {}
impl VaultPassword for Zeroizing<String> {}

/// Secret-bearing input deliberately has no Debug or Serialize implementation.
#[derive(Clone)]
pub struct VaultCredentials {
    password: Zeroizing<String>,
    security_key: Option<Zeroizing<Vec<u8>>>,
}

impl From<Zeroizing<String>> for VaultCredentials {
    fn from(password: Zeroizing<String>) -> Self {
        Self {
            password,
            security_key: None,
        }
    }
}

impl AsRef<str> for VaultCredentials {
    fn as_ref(&self) -> &str {
        &self.password
    }
}

impl std::ops::Deref for VaultCredentials {
    type Target = str;
    fn deref(&self) -> &str {
        &self.password
    }
}

impl VaultPassword for VaultCredentials {
    fn security_key(&self) -> Option<&[u8]> {
        self.security_key.as_deref().map(Vec::as_slice)
    }
}

impl VaultCredentials {
    pub fn from_key_file(password: Zeroizing<String>, path: Option<&Path>) -> Result<Self> {
        let security_key = path.map(read_key_file).transpose()?;
        Ok(Self {
            password,
            security_key,
        })
    }

    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}

fn read_key_file(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    // Refuse devices and pipes before open; check the opened handle too. Never use metadata
    // length as the only bound, because the file may grow while it is being read.
    let metadata = std::fs::metadata(path).map_err(|_| GatewayError::InvalidKeyFile)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_KEY_FILE_BYTES as u64 {
        return Err(GatewayError::InvalidKeyFile);
    }
    let file = File::open(path).map_err(|_| GatewayError::InvalidKeyFile)?;
    if !file
        .metadata()
        .map_err(|_| GatewayError::InvalidKeyFile)?
        .is_file()
    {
        return Err(GatewayError::InvalidKeyFile);
    }
    // A single bounded allocation also avoids leaving earlier allocations containing key
    // bytes behind when read_to_end grows its buffer.
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_KEY_FILE_BYTES + 1));
    file.take((MAX_KEY_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| GatewayError::InvalidKeyFile)?;
    if bytes.is_empty() || bytes.len() > MAX_KEY_FILE_BYTES {
        return Err(GatewayError::InvalidKeyFile);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_file_input_is_binary_bounded_and_never_optional_when_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.key");
        for bytes in [vec![], vec![0x55; MAX_KEY_FILE_BYTES + 1]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(matches!(
                read_key_file(&path),
                Err(GatewayError::InvalidKeyFile)
            ));
        }
        assert!(read_key_file(dir.path()).is_err());
        assert!(read_key_file(&dir.path().join("missing.key")).is_err());
        std::fs::write(&path, [0, 255, 10, 13]).unwrap();
        let credentials = VaultCredentials::from_key_file(
            Zeroizing::new("synthetic-password".into()),
            Some(&path),
        )
        .unwrap();
        assert_eq!(
            credentials.security_key(),
            Some([0, 255, 10, 13].as_slice())
        );
    }
}
