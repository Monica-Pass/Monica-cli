//! Explicit, conservative single-file WebDAV synchronization. The engine makes
//! portable snapshots; this module compares revisions and never merges bytes.
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use mdbx_storage::backup::BackupService;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{
    Config, ConfigStore, connection_fingerprint, ensure_parent, private_file, write_json,
};
use crate::error::{GatewayError, Result};
use crate::vault::{GatewayInventory, Vault};
use crate::webdav::{
    Download, MAX_VAULT_BYTES, WebDavClient, WebDavProfile, WriteCondition, normalize_path,
    strong_etag,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteBinding {
    pub profile: WebDavProfile,
    pub path: String,
    pub vault_id: String,
    pub etag: Option<String>,
    pub remote_sha256: String,
    pub local_sha256: String,
    pub last_sync: i64,
}

impl RemoteBinding {
    pub fn validate(&self) -> Result<()> {
        self.profile.validate()?;
        normalize_path(&self.path)?;
        if self.path.is_empty()
            || uuid::Uuid::parse_str(&self.vault_id).is_err()
            || !valid_hash(&self.remote_sha256)
            || !valid_hash(&self.local_sha256)
            || self
                .etag
                .as_deref()
                .is_some_and(|etag| strong_etag(etag).is_none())
        {
            return Err(GatewayError::InvalidConfig);
        }
        Ok(())
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hex::decode(hash).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncResult {
    UpToDate,
    Uploaded,
    Downloaded,
    Published,
    Merged,
}

/// What one `webdav sync` run achieved. `segments` is present only for a remote
/// whose revisions live in the `.sync` tree, so the single-file payload an AI
/// client has been reading stays exactly as it was.
#[derive(Debug)]
pub struct SyncOutcome {
    pub result: SyncResult,
    pub segments: Option<crate::segment::Report>,
}

impl From<SyncResult> for SyncOutcome {
    fn from(result: SyncResult) -> Self {
        Self {
            result,
            segments: None,
        }
    }
}

impl std::fmt::Display for SyncResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UpToDate => "Both copies are up to date.",
            Self::Uploaded => "Uploaded a consistent encrypted snapshot.",
            Self::Downloaded => {
                "Opened the new remote revision; the previous local copy is preserved."
            }
            Self::Published => "Published the encrypted vault and connected WebDAV sync.",
            Self::Merged => "Merged commit-level segments with the remote stream.",
        })
    }
}

/// A completed snapshot has no required WAL/SHM sidecars. Validation happens on
/// a separate copy so opening/migrating/auditing it cannot change upload bytes.
struct Snapshot {
    directory: tempfile::TempDir,
    path: PathBuf,
    sha256: String,
}

impl Snapshot {
    fn new(store: &ConfigStore, source: &Path) -> Result<Self> {
        ensure_parent(&store.path)?;
        let parent = store.path.parent().ok_or(GatewayError::InvalidConfig)?;
        let directory = tempfile::Builder::new()
            .prefix(".monica-snapshot-")
            .tempdir_in(parent)
            .map_err(|_| GatewayError::StateUnavailable)?;
        private_file(directory.path())?;
        if std::fs::metadata(source)
            .map_err(|_| GatewayError::StateUnavailable)?
            .len()
            > MAX_VAULT_BYTES
        {
            return Err(GatewayError::ResponseTooLarge);
        }
        let path = directory.path().join("portable.mdbx");
        let info = BackupService::create_portable_copy_path(source, &path)
            .map_err(|_| GatewayError::InvalidVault)?;
        private_file(&path)?;
        if info.file_size_bytes > MAX_VAULT_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        let sha256 = hash_file(&path)?;
        Ok(Self {
            directory,
            path,
            sha256,
        })
    }

    fn inspect(&self, password: &str) -> Result<GatewayInventory> {
        let validation = self
            .directory
            .path()
            .join(format!("verify-{}.mdbx", uuid::Uuid::new_v4()));
        // Only an already verified portable snapshot is copied here, never a live main file.
        BackupService::create_portable_copy_path(&self.path, &validation)
            .map_err(|_| GatewayError::InvalidVault)?;
        private_file(&validation)?;
        let vault = Vault::open(&validation, password)?;
        let inventory = vault.gateway_inventory()?;
        vault.lock()?;
        Ok(inventory)
    }

    fn install(&self, store: &ConfigStore) -> Result<(PathBuf, String)> {
        let destination = store
            .path
            .with_extension("vaults")
            .join(format!("{}.mdbx", uuid::Uuid::new_v4()));
        ensure_parent(&destination)?;
        BackupService::create_portable_copy_path(&self.path, &destination)
            .map_err(|_| GatewayError::StateUnavailable)?;
        private_file(&destination)?;
        // Compare exactly the representation that future source snapshots use.
        let baseline = Self::new(store, &destination)?.sha256;
        Ok((destination, baseline))
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|_| GatewayError::StateUnavailable)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut length = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| GatewayError::StateUnavailable)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > MAX_VAULT_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) struct DownloadFile {
    // Drop the open file before removing its directory and SQLite sidecars.
    pub(crate) file: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}

impl DownloadFile {
    pub(crate) fn path(&self) -> &Path {
        self.file.path()
    }
}

pub(crate) fn download_file(store: &ConfigStore) -> Result<DownloadFile> {
    ensure_parent(&store.path)?;
    let directory = tempfile::Builder::new()
        .prefix(".monica-download-")
        .tempdir_in(store.path.parent().ok_or(GatewayError::InvalidConfig)?)
        .map_err(|_| GatewayError::StateUnavailable)?;
    private_file(directory.path())?;
    let file = tempfile::NamedTempFile::new_in(directory.path())
        .map_err(|_| GatewayError::StateUnavailable)?;
    private_file(file.path())?;
    Ok(DownloadFile {
        file,
        _directory: directory,
    })
}

pub(crate) fn remember_previous(store: &ConfigStore, previous: &Config) -> Result<()> {
    let history = store
        .path
        .with_extension("history")
        .join(format!("{}.json", uuid::Uuid::new_v4()));
    write_json(&history, previous, false)
}

fn replace_vault(
    store: &ConfigStore,
    path: PathBuf,
    inventory: GatewayInventory,
    remote: Option<RemoteBinding>,
    name: Option<String>,
) -> Result<usize> {
    let count = inventory.connections.len();
    store.update(|previous| {
        let mut config = Config::new(path);
        config.database_name = name;
        if let Some(previous) = previous {
            remember_previous(store, &previous)?;
            config.listen = previous.listen;
            config.webdav_device_id = previous.webdav_device_id;
        }
        // Switching vaults requires fresh explicit grants, even for the same vault ID.
        config.connections = inventory.connections;
        config.collection_id = inventory.collection_id;
        config.webdav = remote;
        Ok((config, count))
    })
}

/// Copies an existing vault into managed local storage; the original stays untouched.
pub fn open_local(store: &ConfigStore, source: &Path, password: &str) -> Result<usize> {
    let _guard = store.acquire_broker_lock()?;
    let snapshot = Snapshot::new(store, source)?;
    let inventory = snapshot.inspect(password)?;
    let (path, _) = snapshot.install(store)?;
    replace_vault(
        store,
        path,
        inventory,
        None,
        source
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_owned),
    )
}

/// Human selection of a remote MDBX. Existing local state and configuration are
/// preserved in history; grants are not carried across a vault switch.
pub async fn open_remote(
    store: &ConfigStore,
    client: &WebDavClient,
    path: &str,
    password: &str,
) -> Result<usize> {
    let _guard = store.acquire_broker_lock()?;
    let mut download = download_file(store)?;
    let revision = client.download(path, &mut download.file).await?;
    let snapshot = Snapshot::new(store, download.path())?;
    let inventory = snapshot.inspect(password)?;
    let (local, local_sha256) = snapshot.install(store)?;
    let mut binding = RemoteBinding {
        profile: client.profile.clone(),
        path: normalize_path(path)?,
        vault_id: inventory.vault_id.clone(),
        etag: revision.etag,
        remote_sha256: revision.sha256,
        local_sha256,
        last_sync: chrono::Utc::now().timestamp(),
    };
    let inventory = if segment_sync_managed(client, &binding.path).await {
        // The bootstrap is written once, so on its own it is a stale snapshot: the
        // revisions that matter live in the `.sync` tree. Replay them into the
        // installed copy before the configuration starts using it. The replay
        // report is not surfaced here; `webdav sync` prints it from then on.
        let device_id = crate::segment::device_id(store)?;
        let (_, inventory) =
            crate::segment::bootstrap(store, client, &local, &binding, &device_id, password)
                .await?;
        // `local_sha256` still means "the digest of the configured vault".
        binding.local_sha256 = Snapshot::new(store, &local)?.sha256;
        inventory
    } else {
        inventory
    };
    let name = std::path::Path::new(path)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_owned);
    replace_vault(store, local, inventory, Some(binding), name)
}

/// Publishes to a new remote name only. It also provides a safe way to keep a
/// divergent local branch without overwriting the existing remote vault.
pub async fn publish(
    store: &ConfigStore,
    client: &WebDavClient,
    path: &str,
    password: &str,
) -> Result<SyncResult> {
    let _guard = store.acquire_broker_lock()?;
    let config = store.load()?;
    let snapshot = Snapshot::new(store, &config.vault)?;
    let inventory = snapshot.inspect(password)?;
    client
        .upload(path, &snapshot.path, WriteCondition::Create)
        .await?;
    let revision = verify_upload(store, client, path, &snapshot.sha256).await?;
    let binding = RemoteBinding {
        profile: client.profile.clone(),
        path: normalize_path(path)?,
        vault_id: inventory.vault_id,
        etag: revision.etag,
        remote_sha256: revision.sha256,
        local_sha256: snapshot.sha256,
        last_sync: chrono::Utc::now().timestamp(),
    };
    update_binding(store, &config, binding)?;
    Ok(SyncResult::Published)
}

async fn verify_upload(
    store: &ConfigStore,
    client: &WebDavClient,
    path: &str,
    expected: &str,
) -> Result<Download> {
    let mut file = download_file(store)?;
    let revision = client
        .download(path, &mut file.file)
        .await
        .map_err(|_| GatewayError::SyncOutcomeUnknown)?;
    if revision.sha256 != expected {
        return Err(GatewayError::SyncOutcomeUnknown);
    }
    Ok(revision)
}

fn update_binding(store: &ConfigStore, previous: &Config, binding: RemoteBinding) -> Result<()> {
    store.update(|current| {
        let mut current = current.ok_or(GatewayError::NotFound)?;
        ensure_same_vault(&current, previous)?;
        current.webdav = Some(binding);
        Ok((current, ()))
    })
}

fn ensure_same_vault(current: &Config, previous: &Config) -> Result<()> {
    if current.vault != previous.vault || current.webdav != previous.webdav {
        return Err(GatewayError::SyncConflict);
    }
    Ok(())
}

/// Points the configuration at `vault` and re-imports what the file now holds.
/// A segment merge can add, change or delete connections inside the same vault,
/// so a grant only survives when its connection is still there with the same
/// key material; anything else has to be authorized again by hand.
fn apply_remote_vault(
    store: &ConfigStore,
    previous: &Config,
    vault: PathBuf,
    binding: RemoteBinding,
    inventory: Option<GatewayInventory>,
) -> Result<()> {
    store.update(|current| {
        let mut current = current.ok_or(GatewayError::NotFound)?;
        ensure_same_vault(&current, previous)?;
        let Some(inventory) = inventory else {
            // Nothing of this vault's contents changed, so the cached connection list
            // and the grants derived from it are still exactly right.
            current.webdav = Some(binding);
            return Ok((current, ()));
        };
        if current.vault != vault {
            remember_previous(store, &current)?;
            current.vault = vault;
        }
        current.collection_id = inventory.collection_id;
        current.connections = inventory.connections;
        current.grants.retain(|grant| {
            current
                .connections
                .get(&grant.connection)
                .is_some_and(|connection| {
                    connection_fingerprint(connection) == grant.connection_fingerprint
                })
        });
        current.webdav = Some(binding);
        Ok((current, ()))
    })
}

/// Android publishes `<name>.mdbx` once as an immutable bootstrap and keeps every
/// later revision in a `<name>.mdbx.sync` folder. Replacing the bootstrap would
/// strand each device tracking it, and comparing it reports a stale vault as
/// up to date, so neither outcome is safe to compute from a single file.
async fn segment_sync_managed(client: &WebDavClient, path: &str) -> bool {
    let marker = format!("{path}.sync");
    let nested = format!("{marker}/");
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    client.list(parent).await.is_ok_and(|entries| {
        entries
            .into_iter()
            .any(|entry| entry.path == marker || entry.path.starts_with(&nested))
    })
}

pub async fn synchronize(
    store: &ConfigStore,
    client: &WebDavClient,
    password: &str,
) -> Result<SyncOutcome> {
    // This OS lock excludes broker startup, grants, connections and other syncs.
    // Revocation remains allowed; every final config update preserves it.
    let _guard = store.acquire_broker_lock()?;
    let config = store.load()?;
    let mut binding = config
        .webdav
        .clone()
        .ok_or(GatewayError::RemoteNotConfigured)?;
    if client.profile != binding.profile {
        return Err(GatewayError::InvalidWebDav);
    }
    if segment_sync_managed(client, &binding.path).await {
        let device_id = crate::segment::device_id(store)?;
        let (report, inventory) =
            crate::segment::synchronize(store, client, &binding, &device_id, password).await?;
        // The merge rewrote commits inside the configured vault, so the local
        // digest the single-file protocol compares no longer describes it; only
        // `last_sync` is meaningful here, and a segment remote never reads the
        // pair back.
        binding.last_sync = chrono::Utc::now().timestamp();
        apply_remote_vault(store, &config, config.vault.clone(), binding, inventory)?;
        let active = !report.is_quiet() || report.conflicts > 0 || report.blocked_streams > 0;
        return Ok(SyncOutcome {
            result: if active {
                SyncResult::Merged
            } else {
                SyncResult::UpToDate
            },
            segments: active.then_some(report),
        });
    }
    let local = Snapshot::new(store, &config.vault)?;
    let local_inventory = local.inspect(password)?;
    if local_inventory.vault_id != binding.vault_id {
        return Err(GatewayError::SyncConflict);
    }
    let local_changed = local.sha256 != binding.local_sha256;
    let mut incoming = download_file(store)?;
    let remote = client.download(&binding.path, &mut incoming.file).await?;
    let remote_changed = remote.sha256 != binding.remote_sha256;
    binding.etag = remote.etag.clone();
    binding.last_sync = chrono::Utc::now().timestamp();
    let settled = SyncOutcome::from;

    // Also recovers an acknowledged/unknown upload whose config save did not finish.
    if remote.sha256 == local.sha256 || !local_changed && !remote_changed {
        binding.remote_sha256 = remote.sha256;
        binding.local_sha256 = local.sha256;
        update_binding(store, &config, binding)?;
        return Ok(settled(SyncResult::UpToDate));
    }
    if local_changed && remote_changed {
        return Err(GatewayError::SyncConflict);
    }
    if local_changed {
        let etag = remote
            .etag
            .as_deref()
            .ok_or(GatewayError::RemoteVersionRequired)?;
        client
            .upload(&binding.path, &local.path, WriteCondition::Match(etag))
            .await?;
        let confirmed = verify_upload(store, client, &binding.path, &local.sha256).await?;
        binding.remote_sha256 = confirmed.sha256;
        binding.etag = confirmed.etag;
        binding.local_sha256 = local.sha256;
        update_binding(store, &config, binding)?;
        return Ok(settled(SyncResult::Uploaded));
    }

    let incoming = Snapshot::new(store, incoming.path())?;
    let inventory = incoming.inspect(password)?;
    if inventory.vault_id != binding.vault_id {
        return Err(GatewayError::SyncConflict);
    }
    let (path, baseline) = incoming.install(store)?;
    binding.local_sha256 = baseline;
    binding.remote_sha256 = remote.sha256;
    apply_remote_vault(store, &config, path, binding, Some(inventory))?;
    Ok(settled(SyncResult::Downloaded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::PASSWORD;

    #[tokio::test]
    async fn incompatible_remote_preserves_both_vaults_and_cleans_download_sidecars() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("android-shaped.mdbx");
        let vault = Vault::create(&source, PASSWORD, mdbx_core::tiga::TigaMode::Multi).unwrap();
        vault.lock().unwrap();
        drop(vault);
        let connection = mdbx_storage::connection::VaultConnection::open(&source).unwrap();
        connection
            .inner()
            .execute_batch(
                "UPDATE vault_meta SET format_version = 'MDBX-1', schema_version = 1;
             DROP TABLE unlock_methods;",
            )
            .unwrap();
        drop(connection);
        let bytes = std::fs::read(&source).unwrap();
        // A remote SQLite main file may retain the WAL header even when its
        // transaction is fully checkpointed. Opening it creates sidecars.
        assert_eq!(&bytes[18..20], &[2, 2]);
        let remote = crate::webdav_tests::FakeWebDav::new(
            [("/dav/legacy.mdbx".to_owned(), bytes.clone())].into(),
        )
        .await;
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let previous = directory.path().join("existing.mdbx");
        crate::admin::initialize(
            &store,
            &previous,
            crate::config::DEFAULT_PORT,
            PASSWORD,
            PASSWORD,
        )
        .unwrap();
        let config_before = std::fs::read(&store.path).unwrap();
        let vault_before = std::fs::read(&previous).unwrap();

        assert_eq!(
            open_remote(&store, &remote.client, "legacy.mdbx", PASSWORD)
                .await
                .unwrap_err(),
            GatewayError::VaultSchemaUnsupported
        );
        assert_eq!(std::fs::read(&store.path).unwrap(), config_before);
        assert_eq!(std::fs::read(&previous).unwrap(), vault_before);
        assert_eq!(remote.files.lock().unwrap()["/dav/legacy.mdbx"], bytes);
        assert!(
            remote
                .server
                .requests()
                .iter()
                .all(|request| request.method == "GET")
        );
        assert!(!store.path.with_extension("vaults").exists());
        for entry in std::fs::read_dir(directory.path()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(".tmp") && !name.starts_with(".monica-"),
                "{name}"
            );
        }
    }

    #[test]
    fn sync_snapshot_includes_wal_and_has_stable_hash_without_mutating_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.mdbx");
        let vault = Vault::create(&path, PASSWORD, mdbx_core::tiga::TigaMode::Multi).unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "work",
                "",
                crate::model::Provider::Github,
                crate::model::Provider::Github.default_api_base(),
                "",
                zeroize::Zeroizing::new(crate::test_support::TOKEN.to_owned()),
            )
            .unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let first = Snapshot::new(&store, &path).unwrap();
        let second = Snapshot::new(&store, &path).unwrap();
        assert_eq!(first.sha256, second.sha256);
        let inventory = first.inspect(PASSWORD).unwrap();
        assert!(inventory.connections["work"] == binding);
        assert_eq!(hash_file(&first.path).unwrap(), first.sha256);
        assert_eq!(Snapshot::new(&store, &path).unwrap().sha256, first.sha256);
        let (installed, baseline) = first.install(&store).unwrap();
        assert_eq!(Snapshot::new(&store, &installed).unwrap().sha256, baseline);
        vault.lock().unwrap();
    }
}
