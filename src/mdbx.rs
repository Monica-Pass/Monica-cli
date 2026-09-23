//! Read-only facts about the database file itself: the format header it stores and the
//! files that belong to it.
//!
//! Nothing here unlocks a vault, and nothing here takes the broker lock, so the answers stay
//! available while Monica for Android or a running broker holds the same file. The engine
//! opens its one read-only inspection entry with `SQLITE_OPEN_READ_ONLY`; every other public
//! entry point it offers is writable, because opening a vault normally upgrades its schema and
//! seeds the Android root collection. That would be a real cost here: a vault someone syncs
//! from a phone must not change because a person asked how big it is.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use serde::Serialize;

use crate::config::ConfigStore;
use crate::error::{GatewayError, Result};

/// The header of a vault file, and the file facts around it.
///
/// `format_version` and `schema_version` are what a client reports about the file, not about
/// this build: `target_*` says what this build would move the file to when something opens it
/// for writing.
#[derive(Debug, Serialize)]
pub struct Check {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified_unix: i64,
    pub initialized: bool,
    pub format_version: Option<String>,
    pub schema_version: Option<u32>,
    pub min_reader_version: Option<String>,
    pub min_writer_version: Option<String>,
    pub requires_upgrade: bool,
    pub unknown_critical_extensions: bool,
    pub target_format_version: String,
    pub target_schema_version: u32,
}

/// One file the vault is made of.
#[derive(Debug, Serialize)]
pub struct StoredFile {
    /// `vault`, `wal`, `shm`, `journal` or `blobs`. Stable for `--json`; the table translates it.
    pub role: &'static str,
    pub path: PathBuf,
    pub directory: bool,
    /// Bytes on disk. For the attachment store this is a recursive sum, and `None` when the
    /// walk hit its entry budget rather than guessing at a number.
    pub size_bytes: Option<u64>,
    pub modified_unix: i64,
}

/// Names SQLite and the engine's attachment store append to the vault file name.
const SIDECARS: &[(&str, &str)] = &[
    ("-wal", "wal"),
    ("-shm", "shm"),
    ("-journal", "journal"),
    (".blobs", "blobs"),
];

/// Attachment objects live in a two-level prefix tree, so counting them needs a walk. The
/// budget keeps a mistyped path from turning a read-only check into a long scan.
const MAX_WALK_ENTRIES: usize = 20_000;

pub fn check(store: &ConfigStore, given: Option<PathBuf>) -> Result<Check> {
    let path = vault_file(store, given)?;
    let (size_bytes, modified_unix) = facts(&path)?;
    let info = mdbx_storage::migration::inspect_migration_path(&path)
        .map_err(|_| GatewayError::VaultFileUnreadable)?;
    Ok(Check {
        path,
        size_bytes,
        modified_unix,
        initialized: info.initialized,
        format_version: info.format_version,
        schema_version: info.schema_version,
        min_reader_version: info.min_reader_version,
        min_writer_version: info.min_writer_version,
        requires_upgrade: info.requires_upgrade,
        unknown_critical_extensions: info.unknown_critical_extensions,
        target_format_version: info.target_format_version,
        target_schema_version: info.target_schema_version,
    })
}

/// The vault file plus whichever sidecars are actually there.
pub fn files(store: &ConfigStore, given: Option<PathBuf>) -> Result<Vec<StoredFile>> {
    let path = vault_file(store, given)?;
    let mut entries = Vec::with_capacity(SIDECARS.len() + 1);
    entries.push(stored_file("vault", &path, false)?.expect("checked above"));
    for (suffix, role) in SIDECARS {
        let sibling = sibling(&path, suffix);
        if let Some(entry) = stored_file(role, &sibling, true)? {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn vault_file(store: &ConfigStore, given: Option<PathBuf>) -> Result<PathBuf> {
    let path = absolute(&match given {
        Some(path) => path,
        None => store.load()?.vault,
    })?;
    if !path.is_file() {
        return Err(GatewayError::VaultFileMissing);
    }
    Ok(path)
}

fn stored_file(role: &'static str, path: &Path, missing_ok: bool) -> Result<Option<StoredFile>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if missing_ok && matches!(error.kind(), io::ErrorKind::NotFound) => {
            return Ok(None);
        }
        Err(_) => return Err(GatewayError::StateUnavailable),
    };
    let directory = metadata.is_dir();
    let size_bytes = if directory {
        directory_bytes(path)
    } else {
        Some(metadata.len())
    };
    Ok(Some(StoredFile {
        role,
        path: path.to_path_buf(),
        directory,
        size_bytes,
        modified_unix: unix(&metadata),
    }))
}

fn directory_bytes(root: &Path) -> Option<u64> {
    let mut pending = vec![root.to_path_buf()];
    let mut seen = 0usize;
    let mut total = 0u64;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_WALK_ENTRIES {
                return None;
            }
            // `file_type` does not follow links, so a loop outside this tree cannot trap the walk.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total = total.saturating_add(entry.metadata().map(|m| m.len()).unwrap_or(0));
            }
        }
    }
    Some(total)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn facts(path: &Path) -> Result<(u64, i64)> {
    let metadata = path
        .metadata()
        .map_err(|_| GatewayError::VaultFileMissing)?;
    Ok((metadata.len(), unix(&metadata)))
}

fn unix(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|_| GatewayError::InvalidConfig)
}

#[cfg(test)]
mod tests {
    use super::{check, files};
    use crate::admin::initialize;
    use crate::config::ConfigStore;
    use crate::error::GatewayError;

    fn store_with_vault(directory: &std::path::Path) -> ConfigStore {
        let store = ConfigStore::new(directory.join("gateway.json"));
        initialize(
            &store,
            &directory.join("vault.mdbx"),
            47851,
            "first",
            "first",
        )
        .unwrap();
        store
    }

    /// The whole point of this command group: asking about a vault must not change the database.
    /// Measured rather than assumed, and the measurement says the engine's read-only entry still
    /// leaves SQLite's empty write-ahead-log pair behind — which is why `files` words its note
    /// the way it does instead of treating a log as proof of a live client.
    #[test]
    fn checking_a_vault_leaves_the_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_vault(directory.path());
        let vault = directory.path().join("vault.mdbx");
        let before = snapshot(directory.path());
        assert_eq!(
            names(&before),
            ["vault.mdbx"],
            "the vault was not a single file to begin with, so the after-picture proves nothing"
        );
        let report = check(&store, None).unwrap();
        assert!(report.initialized);
        assert_eq!(report.path, vault);
        assert_eq!(report.format_version.as_deref(), Some("MDBX-2"));
        assert_eq!(report.target_format_version, "MDBX-2");
        assert_eq!(report.schema_version, Some(report.target_schema_version));
        assert!(!report.requires_upgrade);
        assert!(!report.unknown_critical_extensions);
        assert_eq!(report.size_bytes, vault.metadata().unwrap().len());
        assert!(report.modified_unix > 0);
        assert!(
            !report.min_reader_version.unwrap_or_default().is_empty(),
            "the compatibility floor the file declares went missing"
        );
        let after = snapshot(directory.path());
        // Size, last write and every byte, because a rewrite can land inside one second.
        assert_eq!(
            entry(&after, "vault.mdbx"),
            entry(&before, "vault.mdbx"),
            "the database file itself changed"
        );
        assert_eq!(
            names(&after),
            ["vault.mdbx", "vault.mdbx-shm", "vault.mdbx-wal"],
            "something other than SQLite's own log pair appeared beside the vault"
        );
    }

    /// An explicit path is inspected instead of the configured vault, and a name that is not
    /// there says so rather than reading as a corrupt database.
    #[test]
    fn a_given_path_overrides_the_current_database_and_missing_files_are_named() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_vault(directory.path());
        let other = directory.path().join("other.mdbx");
        drop(mdbx_storage::connection::VaultConnection::create(&other).unwrap());
        let report = check(&store, Some(other.clone())).unwrap();
        assert_eq!(report.path, other);
        assert!(
            !report.initialized,
            "a created-but-unseeded file has no vault header yet"
        );
        assert_eq!(report.format_version, None);
        assert_eq!(
            (
                report.min_reader_version.clone(),
                report.min_writer_version.clone()
            ),
            (None, None),
            "an unseeded file declares no compatibility floor"
        );
        assert_eq!(
            check(&store, Some(directory.path().join("nope.mdbx"))).unwrap_err(),
            GatewayError::VaultFileMissing
        );
    }

    /// A file that is not a database at all must not be reported as an empty vault.
    #[test]
    fn a_file_that_is_not_a_database_is_rejected_without_touching_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_vault(directory.path());
        let stray = directory.path().join("notes.txt");
        std::fs::write(&stray, b"not a database").unwrap();
        assert_eq!(
            check(&store, Some(stray.clone())).unwrap_err(),
            GatewayError::VaultFileUnreadable
        );
        assert_eq!(
            std::fs::read(&stray).unwrap(),
            b"not a database".to_vec(),
            "the refused file was rewritten"
        );
    }

    /// The attachment tree is what single-file sync refuses to carry and the log pair is what a
    /// writer leaves behind, so the listing has to show both with the sizes actually on disk.
    #[test]
    fn the_files_beside_a_vault_are_listed_with_their_own_sizes() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_vault(directory.path());
        let vault = directory.path().join("vault.mdbx");
        write(&vault.with_extension("mdbx-wal"), 1_024);
        write(&vault.with_extension("mdbx-shm"), 256);
        let blob = directory
            .path()
            .join("vault.mdbx.blobs")
            .join("aa")
            .join("bb")
            .join("aabbccddeeff");
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        write(&blob, 4_096);
        let before = snapshot(directory.path());
        let listed = files(&store, None).unwrap();
        // Listing is pure stat: it must not even create the WAL pair the check command leaves.
        assert_eq!(
            names(&snapshot(directory.path())),
            names(&before),
            "the listing brought files of its own into the directory"
        );
        let by_role: Vec<(&str, Option<u64>, bool)> = listed
            .iter()
            .map(|entry| (entry.role, entry.size_bytes, entry.directory))
            .collect();
        assert_eq!(
            by_role,
            vec![
                ("vault", Some(vault.metadata().unwrap().len()), false),
                ("wal", Some(1_024), false),
                ("shm", Some(256), false),
                ("blobs", Some(4_096), true),
            ],
            "the listing drifted from what is actually on disk"
        );
        // A sidecar row never points at another file's tree.
        for entry in &listed {
            let name = entry
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(name.starts_with("vault.mdbx"), "unrelated {name} listed");
        }
    }

    fn write(path: &std::path::Path, bytes: usize) {
        std::fs::write(path, vec![0x5a; bytes]).unwrap();
    }

    /// Content, size and timestamp of every vault file under a directory.
    fn snapshot(root: &std::path::Path) -> Vec<(String, u64, i64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let metadata = entry.metadata().unwrap();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if !name.starts_with("vault.mdbx") {
                    continue;
                }
                out.push((
                    name,
                    metadata.len(),
                    super::unix(&metadata),
                    std::fs::read(&path).unwrap_or_default(),
                ));
            }
        }
        out.sort();
        out
    }

    fn entry(snapshot: &[(String, u64, i64, Vec<u8>)], name: &str) -> (u64, i64, Vec<u8>) {
        snapshot
            .iter()
            .find(|(file, ..)| file == name)
            .map(|(_, size, modified, content)| (*size, *modified, content.clone()))
            .unwrap_or_else(|| panic!("{name} is missing from the snapshot"))
    }

    fn names(snapshot: &[(String, u64, i64, Vec<u8>)]) -> Vec<&str> {
        snapshot.iter().map(|(file, ..)| file.as_str()).collect()
    }
}
