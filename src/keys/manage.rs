//! Key management for a person at a terminal: one vault unlock per command, plus the file
//! transport for imports and the single explicit export.
//!
//! Key text never travels through argv, the environment or a pipe. It is either generated inside
//! this process or read from a file the person named, and it leaves only through `export`.
use std::io::Write as _;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use super::limits::MAX_KEY_INPUT_BYTES;
use super::openpgp;
use super::openssh::{SshAlgorithm, SshKeyPair};
use crate::config::{ConfigStore, private_file};
use crate::error::{GatewayError, Result};
use crate::vault::{KeyEntrySummary, NewKeyEntry, Vault};

/// Where the generated or imported key goes and what the caller may show afterwards.
pub struct ExportedKey {
    pub summary: KeyEntrySummary,
    pub path: PathBuf,
    pub bytes: usize,
    pub private: bool,
}

pub fn list(store: &ConfigStore, password: &str) -> Result<Vec<KeyEntrySummary>> {
    with_vault(store, password, |vault| vault.key_entries())
}

pub fn find(store: &ConfigStore, password: &str, name: &str) -> Result<KeyEntrySummary> {
    with_vault(store, password, |vault| vault.key_entry_by_title(name))
}

/// Creates a key entry from a freshly generated pair. RSA sizes are the ones Android offers.
pub fn generate_ssh(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    algorithm: &str,
    comment: &str,
) -> Result<KeyEntrySummary> {
    let pair = SshKeyPair::generate(SshAlgorithm::parse(algorithm)?, comment)?;
    add_ssh(store, password, title, note, category, pair)
}

/// Imports an OpenSSH or PKCS#1 PEM file as a new key entry.
pub fn import_ssh(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    comment: &str,
    path: &Path,
) -> Result<KeyEntrySummary> {
    import_ssh_text(
        store,
        password,
        title,
        note,
        category,
        comment,
        &read_key_text(path)?,
    )
}

/// Imports key text a person pasted at the terminal. Same bytes, minus the file.
pub fn import_ssh_text(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    comment: &str,
    text: &str,
) -> Result<KeyEntrySummary> {
    let pair = SshKeyPair::from_pem(text)?;
    let pair = if comment.trim().is_empty() {
        pair
    } else {
        pair.with_comment(comment)?
    };
    add_ssh(store, password, title, note, category, pair)
}

fn add_ssh(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    pair: SshKeyPair,
) -> Result<KeyEntrySummary> {
    let data = pair.to_data();
    with_vault(store, password, |vault| {
        vault.add_key_entry(category, title, note, NewKeyEntry::Ssh { data: &data })
    })
}

/// Imports an OpenPGP ring. Either armor alone is enough; with both, the two must be one key.
pub fn import_gpg(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    public: Option<&Path>,
    secret: Option<&Path>,
) -> Result<KeyEntrySummary> {
    let secret_key = match secret {
        Some(path) => Some(openpgp::read(&read_key_text(path)?)?),
        None => None,
    };
    let public_key = match public {
        Some(path) => Some(openpgp::read(&read_key_text(path)?)?),
        None => None,
    };
    let certificate = match (public_key, secret_key) {
        (None, None) => return Err(GatewayError::InvalidRequest),
        (Some(public), None) => (public.certificate(), String::new()),
        (None, Some(secret)) => {
            let armor = secret
                .secret_armor
                .clone()
                .ok_or(GatewayError::InvalidKeyMaterial)?;
            (secret.certificate(), armor)
        }
        (Some(public), Some(secret)) => {
            if public.fingerprint != secret.fingerprint {
                // Two rings under one entry would make the private half unusable for the certificate.
                return Err(GatewayError::InvalidKeyMaterial);
            }
            let armor = secret
                .secret_armor
                .clone()
                .ok_or(GatewayError::InvalidKeyMaterial)?;
            (public.certificate(), armor)
        }
    };
    let (certificate, secret_armor) = certificate;
    add_gpg(
        store,
        password,
        title,
        note,
        category,
        &certificate,
        &secret_armor,
    )
}

/// Imports pasted OpenPGP armor: a secret ring keeps both halves, a public one its certificate.
pub fn import_gpg_text(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    armor: &str,
) -> Result<KeyEntrySummary> {
    if armor.len() > MAX_KEY_INPUT_BYTES {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    let key = openpgp::read(armor)?;
    let secret = key.secret_armor.clone().unwrap_or_default();
    add_gpg(
        store,
        password,
        title,
        note,
        category,
        &key.certificate(),
        &secret,
    )
}

fn add_gpg(
    store: &ConfigStore,
    password: &str,
    title: &str,
    note: &str,
    category: Option<&str>,
    certificate: &openpgp::GpgCertificate,
    secret_armor: &str,
) -> Result<KeyEntrySummary> {
    with_vault(store, password, |vault| {
        vault.add_key_entry(
            category,
            title,
            note,
            NewKeyEntry::Gpg {
                certificate,
                secret_armor,
            },
        )
    })
}

/// Renames, re-notes or re-comments one key entry, addressed by its title.
pub fn edit(
    store: &ConfigStore,
    password: &str,
    name: &str,
    title: Option<&str>,
    note: Option<&str>,
    comment: Option<&str>,
) -> Result<KeyEntrySummary> {
    with_vault(store, password, |vault| {
        let found = vault.key_entry_by_title(name)?;
        vault.edit_key_entry(&found.entry_id, title, note, comment)
    })
}

/// The same edit addressed by entry id: the terminal already points at a row, and its title
/// may be the very thing being changed.
pub fn edit_entry(
    store: &ConfigStore,
    password: &str,
    entry_id: &str,
    title: Option<&str>,
    note: Option<&str>,
    comment: Option<&str>,
) -> Result<KeyEntrySummary> {
    with_vault(store, password, |vault| {
        vault.edit_key_entry(entry_id, title, note, comment)
    })
}

/// Tombstones one key entry addressed by its title, and hands back what was removed.
/// Text a person already exported keeps living on disk; only the vault stops carrying it.
pub fn delete(store: &ConfigStore, password: &str, name: &str) -> Result<KeyEntrySummary> {
    with_vault(store, password, |vault| {
        let found = vault.key_entry_by_title(name)?;
        vault.delete_entry(&found.entry_id)?;
        Ok(found)
    })
}

/// Writes the half a person asked for to the file they named. The only path out of the vault.
pub fn export(
    store: &ConfigStore,
    password: &str,
    name: &str,
    output: &Path,
    with_private: bool,
    force: bool,
) -> Result<ExportedKey> {
    let text = with_vault(store, password, |vault| {
        let found = vault.key_entry_by_title(name)?;
        vault.key_export_text(&found.entry_id)
    })?;
    let secret: Option<Zeroizing<String>> = if with_private {
        Some(text.private_text.ok_or(GatewayError::KeySecretMissing)?)
    } else {
        None
    };
    let body: &[u8] = secret
        .as_ref()
        .map(|private| private.as_bytes())
        .unwrap_or(text.public_text.as_bytes());
    let bytes = body.len();
    write_key_file(output, body, force)?;
    drop(secret);
    Ok(ExportedKey {
        summary: text.summary,
        path: output.to_path_buf(),
        bytes,
        private: with_private,
    })
}

/// Reads a key file the person pointed at, bounded before any of its bytes are decoded.
fn read_key_text(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path).map_err(|_| GatewayError::StateUnavailable)?;
    if !metadata.is_file() || metadata.len() > MAX_KEY_INPUT_BYTES as u64 {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    let text = std::fs::read_to_string(path).map_err(|_| GatewayError::StateUnavailable)?;
    if text.trim().is_empty() {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    Ok(text)
}

/// Writes the output, tightening it while it is still empty. `force` supersedes an existing
/// file and still creates one that is missing.
fn write_key_file(path: &Path, bytes: &[u8], force: bool) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::AlreadyExists => GatewayError::AlreadyExists,
        _ => GatewayError::StateUnavailable,
    })?;
    private_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| GatewayError::StateUnavailable)
}

fn with_vault<T>(
    store: &ConfigStore,
    password: &str,
    act: impl FnOnce(&Vault) -> Result<T>,
) -> Result<T> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = act(&vault);
    vault.lock()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keys::openssh;
    use crate::test_support::PASSWORD;
    use mdbx_core::tiga::TigaMode;

    const SECRET_RING: &str = include_str!("../../tests/fixtures/gpg/rsa2048-secret.asc");
    const PUBLIC_RING: &str = include_str!("../../tests/fixtures/gpg/rsa2048-secret-pub.asc");

    fn fixture() -> (tempfile::TempDir, ConfigStore) {
        let directory = tempfile::tempdir().unwrap();
        let vault_path = directory.path().join("keys.mdbx");
        Vault::create(&vault_path, PASSWORD, TigaMode::Multi)
            .unwrap()
            .lock()
            .unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        store.update(|_| Ok((Config::new(vault_path), ()))).unwrap();
        (directory, store)
    }

    fn generated(store: &ConfigStore, title: &str) -> KeyEntrySummary {
        generate_ssh(
            store,
            PASSWORD,
            title,
            "用于登录代码托管",
            None,
            "ed25519",
            "cli@test",
        )
        .unwrap()
    }

    #[test]
    fn a_generated_key_comes_back_as_the_same_pair_through_a_file() {
        let (directory, store) = fixture();
        let created = generated(&store, "laptop");
        assert_eq!(created.algorithm, openssh::ALGORITHM_ED25519);
        assert_eq!(created.comment, "cli@test");
        assert_eq!(created.login_type, "SSH_KEY");
        assert!(created.has_secret);
        assert_eq!(list(&store, PASSWORD).unwrap(), vec![created.clone()]);
        assert_eq!(
            find(&store, PASSWORD, " laptop ").unwrap().entry_id,
            created.entry_id
        );
        assert!(matches!(
            find(&store, PASSWORD, "absent"),
            Err(GatewayError::NotFound)
        ));

        let path = directory.path().join("id_ed25519");
        export(&store, PASSWORD, "laptop", &path, true, false).unwrap();
        let reimported =
            import_ssh(&store, PASSWORD, "copy", "", None, "imported@test", &path).unwrap();
        assert_eq!(reimported.fingerprint, created.fingerprint);
        assert_ne!(reimported.public_key, created.public_key);
        assert!(reimported.public_key.ends_with(" imported@test"));
        assert_eq!(reimported.comment, "imported@test");
        assert_eq!(list(&store, PASSWORD).unwrap().len(), 2);
    }

    #[test]
    fn a_deleted_key_leaves_the_vault_but_not_the_files_already_written() {
        let (directory, store) = fixture();
        let created = generated(&store, "laptop");
        let exported = directory.path().join("id_ed25519_copy");
        export(&store, PASSWORD, "laptop", &exported, true, false).unwrap();

        assert_eq!(
            delete(&store, PASSWORD, "laptop").unwrap().entry_id,
            created.entry_id
        );
        assert!(list(&store, PASSWORD).unwrap().is_empty());
        assert!(matches!(
            find(&store, PASSWORD, "laptop"),
            Err(GatewayError::NotFound)
        ));
        assert!(matches!(
            export(
                &store,
                PASSWORD,
                "laptop",
                &directory.path().join("again"),
                false,
                false
            ),
            Err(GatewayError::NotFound)
        ));
        assert!(matches!(
            delete(&store, PASSWORD, "laptop"),
            Err(GatewayError::NotFound)
        ));
        // A tombstone is not a shredder: text already taken out of the vault stays put.
        assert!(
            std::fs::read_to_string(&exported)
                .unwrap()
                .contains("BEGIN OPENSSH PRIVATE KEY")
        );
    }

    #[test]
    fn only_a_key_file_the_person_can_read_is_accepted() {
        let (directory, store) = fixture();
        let missing = directory.path().join("missing");
        assert!(matches!(
            import_ssh(&store, PASSWORD, "gone", "", None, "", &missing),
            Err(GatewayError::StateUnavailable)
        ));
        assert!(matches!(
            read_key_text(directory.path()),
            Err(GatewayError::KeyPayloadTooLarge)
        ));

        let created = generated(&store, "laptop");
        let public_line = directory.path().join("id_ed25519.pub");
        std::fs::write(&public_line, created.public_key.clone() + "\n").unwrap();
        assert!(matches!(
            import_ssh(&store, PASSWORD, "pub", "", None, "", &public_line),
            Err(GatewayError::InvalidKeyMaterial)
        ));
        let blank = directory.path().join("blank");
        std::fs::write(&blank, "  \n\t\n").unwrap();
        assert!(matches!(
            import_ssh(&store, PASSWORD, "blank", "", None, "", &blank),
            Err(GatewayError::InvalidKeyMaterial)
        ));
        let huge = directory.path().join("huge");
        std::fs::write(&huge, "a".repeat(MAX_KEY_INPUT_BYTES + 1)).unwrap();
        assert!(matches!(
            import_gpg(&store, PASSWORD, "huge", "", None, Some(&huge), None),
            Err(GatewayError::KeyPayloadTooLarge)
        ));
        assert!(matches!(
            generate_ssh(&store, PASSWORD, "weak", "", None, "rsa512", ""),
            Err(GatewayError::InvalidRequest)
        ));
        assert_eq!(list(&store, PASSWORD).unwrap(), vec![created]);
    }

    #[test]
    fn export_writes_one_half_at_a_time_and_never_clobbers_silently() {
        let (directory, store) = fixture();
        let created = generated(&store, "laptop");
        let private = directory.path().join("id_ed25519");
        let exported = export(&store, PASSWORD, "laptop", &private, true, false).unwrap();
        assert!(exported.private);
        assert_eq!(exported.summary, created);
        let text = std::fs::read_to_string(&private).unwrap();
        assert_eq!(exported.bytes, text.len());
        assert!(
            text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n")
                && text.ends_with("-----END OPENSSH PRIVATE KEY-----\n")
        );
        assert!(matches!(
            export(&store, PASSWORD, "laptop", &private, true, false),
            Err(GatewayError::AlreadyExists)
        ));

        let public = directory.path().join("id_ed25519.pub");
        export(&store, PASSWORD, "laptop", &public, false, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&public).unwrap(),
            format!("{}\n", created.public_key)
        );
        let renamed = edit(
            &store,
            PASSWORD,
            "laptop",
            Some("work"),
            None,
            Some("work@laptop"),
        )
        .unwrap();
        assert_eq!(renamed.title, "work");
        assert!(matches!(
            find(&store, PASSWORD, "laptop"),
            Err(GatewayError::NotFound)
        ));
        // A stale export is a snapshot until the person asks for the new text.
        assert_eq!(
            std::fs::read_to_string(&public).unwrap(),
            format!("{}\n", created.public_key)
        );
        let refreshed = export(&store, PASSWORD, "work", &public, false, true).unwrap();
        assert!(!refreshed.private);
        assert!(
            std::fs::read_to_string(&public)
                .unwrap()
                .ends_with("work@laptop\n")
        );
        assert_eq!(refreshed.summary.fingerprint, created.fingerprint);
        // Forcing an output that is not there yet still creates it.
        let fresh = directory.path().join("fresh.pub");
        export(&store, PASSWORD, "work", &fresh, false, true).unwrap();
        assert!(fresh.is_file());
    }

    #[test]
    fn a_ring_stores_both_halves_verbatim_and_reports_what_is_missing() {
        let (directory, store) = fixture();
        assert!(matches!(
            import_gpg(&store, PASSWORD, "ring", "", None, None, None),
            Err(GatewayError::InvalidRequest)
        ));

        let public = directory.path().join("public.asc");
        let secret = directory.path().join("secret.asc");
        std::fs::write(&public, PUBLIC_RING).unwrap();
        std::fs::write(&secret, SECRET_RING).unwrap();

        let certificate_only =
            import_gpg(&store, PASSWORD, "cert", "", None, Some(&public), None).unwrap();
        assert_eq!(certificate_only.login_type, "GPG_KEY");
        assert_eq!(certificate_only.algorithm, "RSA");
        assert_eq!(certificate_only.key_size, Some(2048));
        assert!(!certificate_only.has_secret);
        assert!(certificate_only.chunk_count > 0);
        let output = directory.path().join("out.asc");
        assert!(matches!(
            export(&store, PASSWORD, "cert", &output, true, false),
            Err(GatewayError::KeySecretMissing)
        ));
        assert!(!output.exists());
        export(&store, PASSWORD, "cert", &output, false, false).unwrap();
        assert_eq!(std::fs::read_to_string(&output).unwrap(), PUBLIC_RING);

        // A secret ring alone must still carry a certificate, derived from its own public part.
        let derived =
            import_gpg(&store, PASSWORD, "secret", "", None, None, Some(&secret)).unwrap();
        assert!(derived.has_secret);
        assert_eq!(derived.fingerprint, certificate_only.fingerprint);
        assert_eq!(
            derived.comment,
            "Monica CLI Fixture <cli-fixture@example.com>"
        );
        let private = directory.path().join("private.asc");
        export(&store, PASSWORD, "secret", &private, true, false).unwrap();
        assert_eq!(std::fs::read_to_string(&private).unwrap(), SECRET_RING);

        // An OpenSSH private key handed to the GPG import is not a ring and must be refused.
        let not_armor = directory.path().join("id_ed25519");
        std::fs::write(
            &not_armor,
            SshKeyPair::generate(SshAlgorithm::Ed25519, "cli@test")
                .unwrap()
                .private_key_pem,
        )
        .unwrap();
        assert!(matches!(
            import_gpg(
                &store,
                PASSWORD,
                "mixed",
                "",
                None,
                Some(&public),
                Some(&not_armor),
            ),
            Err(GatewayError::InvalidKeyMaterial)
        ));
        assert_eq!(list(&store, PASSWORD).unwrap().len(), 2);
    }
}
