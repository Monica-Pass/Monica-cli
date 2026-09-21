//! Segment-stream synchronization, interoperable with the `<vault>.mdbx.sync` tree
//! Monica Android maintains. Each device appends immutable, content-addressed
//! segments to its own stream and the engine merges commits, so no device ever
//! replaces a file another device is tracking.
//!
//! Transport cursors live in `gateway.sync.json` next to the configuration. Every
//! merge decision is delegated to `PeerSyncService`; this module only decides
//! which segment to fetch or hand over next, and remembers where it stopped.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use mdbx_storage::error::StorageError;
use mdbx_storage::peer_sync::{PeerSyncSegmentOptions, PeerSyncService};
use mdbx_storage::sync_apply::ApplyBatchResult;
use mdbx_sync::{
    IncrementalBundleCheckpoint, IncrementalBundleResume, IncrementalSyncBundle, SyncBundleFile,
    SyncError, bundle_file_from_bytes_authenticated, incremental_bundle_payload_sha256,
    incremental_bundle_to_bytes_authenticated,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::config::{ConfigStore, ensure_parent, private_file, read_json, write_json};
use crate::error::{GatewayError, Result};
use crate::sync::{RemoteBinding, download_file};
use crate::vault::Vault;
use crate::webdav::{WebDavClient, normalize_path};

/// Android names each generation a random UUID, so the listing order of a cold
/// connect is not the order the segments apply in. Applicability is discovered by
/// retrying, and the bytes stay in memory so a retry never costs another download.
/// The bound only stops a cycle or a permanently missing parent from spinning.
const MAX_RECEIVE_ROUNDS: usize = 64;
/// Memory guard for the retries above, not a batch size: past it a segment is
/// simply re-downloaded on the round that can use it.
const MAX_CACHED_SEGMENTS: usize = 64;
/// A runaway guard, not a batch size: a vault with more pending history simply
/// continues on the next run.
const MAX_SEGMENTS_PER_SYNC: usize = 10_000;
const SEGMENT_PAGE_SIZE: usize = 128;
const SEGMENT_SUFFIX: &str = "mdbxsync";
const MAX_CURSOR_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PATH_PART_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Report {
    pub downloaded_segments: usize,
    pub uploaded_segments: usize,
    pub applied_commits: u32,
    pub skipped_commits: u32,
    pub conflicts: u32,
    pub blocked_streams: usize,
}

impl Report {
    /// Nothing crossed the wire, so the caller can report the same quiet result a
    /// single-file sync reports when both copies already agree.
    pub fn is_quiet(&self) -> bool {
        self.downloaded_segments == 0 && self.uploaded_segments == 0
    }
}

/// Where this device's export stands. Kept as a named state rather than the raw
/// checkpoint strings because those are opaque engine identifiers and the useful
/// fact is only how much the next run will owe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportBase {
    /// No base chosen yet, so the next run re-exports this vault's whole history.
    #[default]
    Unset,
    /// Anchored on the start of history, the state a remote install is in.
    Bootstrap,
    /// Anchored on a checkpoint the engine reported once the previous run drained.
    Anchored,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    /// False when no segment run has left a cursor for this vault yet.
    pub tracked: bool,
    pub export_base: ExportBase,
    /// True while the next segment continues a generation already opened.
    pub generation_open: bool,
    /// A segment written but not yet confirmed uploaded; the next run retries these
    /// exact bytes instead of exporting a second digest for one sequence number.
    pub pending_upload: Option<PendingUpload>,
    pub streams: usize,
    pub complete_streams: usize,
    /// Streams left short of the end of a peer's history. The reason is one of the
    /// `WAITING_*` tokens below, so `--json` stays a stable key and the human renderer
    /// can translate it.
    pub waiting: Vec<Waiting>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PendingUpload {
    pub remote: String,
    pub size: u64,
}

/// A peer published segment N+1 while this device still holds N.
pub const WAITING_EARLIER_SEGMENT: &str = "waiting_for_earlier_segment";
/// The segment needs a commit that a different peer has not published yet.
pub const WAITING_PARENT_COMMIT: &str = "waiting_for_parent_commit";
/// A segment arrived for a generation already marked complete.
pub const WAITING_AFTER_COMPLETION: &str = "segment_after_completion";
/// The bytes under a path do not describe the segment that path names.
pub const WAITING_PATH_MISMATCH: &str = "segment_path_mismatch";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub stream: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Cursor {
    vault_id: String,
    /// `None` means this device has not chosen a starting point yet.
    export_base: Option<IncrementalBundleCheckpoint>,
    /// Continuation inside the current generation. `None` means the next exported
    /// segment opens a new generation directory.
    export_resume: Option<IncrementalBundleResume>,
    /// Persisted before the upload and cleared after it is confirmed, so a restart
    /// retries the exact same bytes instead of exporting a different digest for one
    /// sequence number.
    pending: Option<Pending>,
    streams: BTreeMap<String, Stream>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    file: PathBuf,
    remote: String,
    digest: String,
    size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Stream {
    next_sequence: u32,
    /// Dropped once a generation is complete. Its segments are immutable, so only
    /// the position in the listing is still needed to skip them.
    checkpoint: Option<IncrementalBundleCheckpoint>,
    resume: Option<IncrementalBundleResume>,
    complete: bool,
    blocked: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    sequence: u32,
    digest: String,
    path: String,
}

pub fn sync_root(binding: &RemoteBinding) -> Result<String> {
    normalize_path(&format!("{}.sync", binding.path))
}

/// Publishes everything this device still owes, then applies whatever peers have
/// published. The order matches Android so a local edit is visible to the pull.
///
/// The returned inventory is `Some` only when a peer stream actually landed in
/// this vault: that is the one case where the connection list cached in the
/// configuration went stale, and reading it costs a disclosure of every token.
pub(crate) async fn synchronize(
    store: &ConfigStore,
    client: &WebDavClient,
    binding: &RemoteBinding,
    device_id: &str,
    password: &str,
) -> Result<(Report, Option<crate::vault::GatewayInventory>)> {
    let root = sync_root(binding)?;
    let vault_path = store.load()?.vault;
    let vault = open_vault(&vault_path, binding, password)?;
    let mut cursor = load_cursor(store, &binding.vault_id)?.unwrap_or(Cursor {
        vault_id: binding.vault_id.clone(),
        ..Cursor::default()
    });
    let (report, drained) = settle(store, client, &root, &vault, device_id, &mut cursor).await?;
    let merged = if report.downloaded_segments > 0 || report.conflicts > 0 {
        Some(vault.gateway_inventory()?)
    } else {
        None
    };
    reanchor(store, &vault, &mut cursor, drained)?;
    vault.lock()?;
    Ok((report, merged))
}

/// Applies the peer streams to a freshly installed copy of the remote bootstrap.
/// That copy *is* the starting point the streams were cut from, so the export
/// cursor is anchored on its own checkpoint instead of falling back to the
/// "cursor lost, re-export everything" path a diverged local vault needs.
///
/// `path` is passed in rather than read from the configuration so a caller can
/// replay a copy it has installed but not yet switched to: if the replay fails,
/// the configuration still describes the vault the user had before.
pub(crate) async fn bootstrap(
    store: &ConfigStore,
    client: &WebDavClient,
    path: &Path,
    binding: &RemoteBinding,
    device_id: &str,
    password: &str,
) -> Result<(Report, crate::vault::GatewayInventory)> {
    let root = sync_root(binding)?;
    let vault = open_vault(path, binding, password)?;
    let mut cursor = Cursor {
        vault_id: binding.vault_id.clone(),
        export_base: Some(current_checkpoint(&vault)?),
        ..Cursor::default()
    };
    // Written before the first request so a failed replay cannot leave the cursor a
    // previous session had for this vault ID.
    save_cursor(store, &cursor)?;
    let (report, drained) = settle(store, client, &root, &vault, device_id, &mut cursor).await?;
    let merged = vault.gateway_inventory()?;
    reanchor(store, &vault, &mut cursor, drained)?;
    vault.lock()?;
    Ok((report, merged))
}

fn open_vault(path: &Path, binding: &RemoteBinding, password: &str) -> Result<Vault> {
    let vault = Vault::open(path, password)?;
    // Also refuses a vault whose attachments live outside the database.
    if vault.gateway_binding()? != binding.vault_id {
        return Err(GatewayError::SyncConflict);
    }
    Ok(vault)
}

async fn settle(
    store: &ConfigStore,
    client: &WebDavClient,
    root: &str,
    vault: &Vault,
    device_id: &str,
    cursor: &mut Cursor,
) -> Result<(Report, bool)> {
    let mut report = Report::default();
    let drained = publish(vault, store, client, root, device_id, cursor, &mut report).await?;
    receive(vault, store, client, root, device_id, cursor, &mut report).await?;
    Ok((report, drained && report.conflicts == 0))
}

/// Moves the export base to where this device stands once a run is over.
///
/// The push runs before the pull, so by the time the pull is done every batch
/// still owed here is state the engine derived locally: the mirror of the
/// auxiliary deltas it just applied, and the audit rows a disclosure writes. Those
/// are not this device's work. Every peer reads every stream, so nothing is lost
/// by not forwarding what was received, while publishing them would leave every
/// client owing a fresh segment after every command that ever touches the vault.
fn reanchor(store: &ConfigStore, vault: &Vault, cursor: &mut Cursor, drained: bool) -> Result<()> {
    if !drained {
        return Ok(());
    }
    cursor.export_base = Some(current_checkpoint(vault)?);
    cursor.export_resume = None;
    save_cursor(store, cursor)
}

/// `monica-cli-<uuid>` is generated once and survives vault switches: the remote
/// stream name is how peers recognise this device, and losing it would strand the
/// published history under an orphan stream.
pub fn device_id(store: &ConfigStore) -> Result<String> {
    let existing = store.load()?.webdav_device_id;
    if let Some(device_id) = existing {
        return Ok(device_id);
    }
    let generated = format!("monica-cli-{}", uuid::Uuid::new_v4());
    let mut stored = generated.clone();
    store.update(|current| {
        let mut current = current.ok_or(GatewayError::NotFound)?;
        if let Some(existing) = current.webdav_device_id.clone() {
            stored = existing;
        } else {
            current.webdav_device_id = Some(generated.clone());
        }
        Ok((current, ()))
    })?;
    Ok(stored)
}

/// Projects the transport cursor for `webdav status`. Only the cursor file is read,
/// so this answers without a vault password and without a request that would list a
/// remote tree the user may not want to pay for.
pub fn status(store: &ConfigStore, vault_id: &str) -> Result<Status> {
    let Some(cursor) = load_cursor(store, vault_id)? else {
        return Ok(Status::default());
    };
    let export_base = match &cursor.export_base {
        None => ExportBase::Unset,
        Some(base) if base.commit_inventory.is_none() && base.delta_inventory.is_none() => {
            ExportBase::Bootstrap
        }
        Some(_) => ExportBase::Anchored,
    };
    Ok(Status {
        tracked: true,
        export_base,
        generation_open: cursor.export_resume.is_some(),
        pending_upload: cursor.pending.as_ref().map(|pending| PendingUpload {
            remote: pending.remote.clone(),
            size: pending.size,
        }),
        streams: cursor.streams.len(),
        complete_streams: cursor
            .streams
            .values()
            .filter(|stream| stream.complete)
            .count(),
        waiting: cursor
            .streams
            .iter()
            .filter_map(|(key, stream)| {
                stream.blocked.as_ref().map(|reason| Waiting {
                    stream: key.clone(),
                    reason: reason.clone(),
                })
            })
            .collect(),
    })
}

fn load_cursor(store: &ConfigStore, vault_id: &str) -> Result<Option<Cursor>> {
    let path = cursor_path(store);
    if !path.is_file() {
        return Ok(None);
    }
    let cursor: Cursor = match read_json(&path, MAX_CURSOR_BYTES) {
        Ok(cursor) => cursor,
        // A cursor this module cannot parse is still this module's state, and a
        // fresh one is always safe to build: the remote segments are immutable.
        Err(_) => return Ok(None),
    };
    if cursor.vault_id != vault_id || cursor.streams.len() > MAX_SEGMENTS_PER_SYNC {
        return Ok(None);
    }
    Ok(Some(cursor))
}

fn cursor_path(store: &ConfigStore) -> PathBuf {
    store.path.with_extension("sync.json")
}

fn pending_directory(store: &ConfigStore) -> PathBuf {
    store.path.with_extension("sync")
}

/// `ensure_parent` only goes one level up, so a directory this module owns has to
/// be created before its access rights can be read and rewritten.
fn private_directory(path: &Path) -> Result<()> {
    ensure_parent(path)?;
    std::fs::create_dir_all(path).map_err(|_| GatewayError::StateUnavailable)?;
    private_file(path)
}

fn save_cursor(store: &ConfigStore, cursor: &Cursor) -> Result<()> {
    write_json(&cursor_path(store), cursor, true)
}

// ---------------------------------------------------------------------------
// publish
// ---------------------------------------------------------------------------

/// Uploads every segment this device owes. `Ok(true)` means the debt is gone: the
/// export came back empty, so the caller may treat whatever the pull then writes
/// locally as this device's own state rather than unpublished work.
async fn publish(
    vault: &Vault,
    store: &ConfigStore,
    client: &WebDavClient,
    root: &str,
    device_id: &str,
    cursor: &mut Cursor,
    report: &mut Report,
) -> Result<bool> {
    if cursor.export_base.is_none() {
        cursor.export_base = Some(bootstrapped());
        save_cursor(store, cursor)?;
    }
    let directory = pending_directory(store);
    private_directory(&directory)?;
    for _ in 0..MAX_SEGMENTS_PER_SYNC {
        let base = cursor
            .export_base
            .clone()
            .ok_or(GatewayError::StateUnavailable)?;
        let (bundle, bytes) = match &cursor.pending {
            Some(pending) => {
                let bytes =
                    std::fs::read(&pending.file).map_err(|_| GatewayError::SyncStateMissing)?;
                if bytes.len() as u64 != pending.size {
                    return Err(GatewayError::SyncSegmentCorrupt);
                }
                let bundle = parse_segment(vault, &bytes, &pending.digest)?;
                if bundle.manifest.base != base {
                    // The remote moved under the cursor; the stored bytes are for a
                    // different starting point and must not be published.
                    return Err(GatewayError::SyncStateMissing);
                }
                (bundle, bytes)
            }
            None => {
                let (bundle, bytes) =
                    export_segment(vault, device_id, &base, cursor.export_resume.as_ref())?;
                if is_empty(&bundle) {
                    return Ok(true);
                }
                let digest = payload_digest(&bundle)?;
                let remote = segment_path(
                    root,
                    device_id,
                    &bundle.manifest.transfer_id,
                    bundle.manifest.segment_index,
                    &digest,
                );
                let file = directory.join(format!(
                    "segment-{}.{}",
                    uuid::Uuid::new_v4(),
                    SEGMENT_SUFFIX
                ));
                write_new_private(&file, &bytes)?;
                cursor.pending = Some(Pending {
                    file: file.clone(),
                    remote: remote.clone(),
                    digest,
                    size: bytes.len() as u64,
                });
                save_cursor(store, cursor)?;
                (bundle, bytes)
            }
        };
        let pending = cursor
            .pending
            .take()
            .ok_or(GatewayError::StateUnavailable)?;
        upload_segment(client, store, &pending.remote, &pending.file, &bytes).await?;
        report.uploaded_segments += 1;
        cursor.export_base = Some(bundle.manifest.result.clone());
        cursor.export_resume = next_resume(&bundle)?;
        save_cursor(store, cursor)?;
        let _ = std::fs::remove_file(&pending.file);
    }
    Ok(false)
}

/// The paired-empty marker the engine reads as "start of history".
///
/// With no cursor this device cannot tell the remote what it already has, and any
/// smaller base silently keeps the local commits made before the first segment run
/// (a CLI that published the single-file bootstrap and then edited the vault is
/// exactly that case). Re-exporting from genesis costs one bounded pass and peers
/// skip commits they already hold, so the duplication is cheap and the loss is not.
fn bootstrapped() -> IncrementalBundleCheckpoint {
    IncrementalBundleCheckpoint {
        commit_inventory: None,
        delta_inventory: None,
    }
}

fn is_empty(bundle: &IncrementalSyncBundle) -> bool {
    bundle.commits.is_empty() && bundle.manifest.delta_inventory.is_empty()
}

fn segments_directory(root: &str, device_id: &str, generation: &str) -> String {
    format!("{root}/streams/{device_id}/{generation}/segments")
}

fn segment_path(
    root: &str,
    device_id: &str,
    generation: &str,
    sequence: u32,
    digest: &str,
) -> String {
    format!(
        "{}/{}",
        segments_directory(root, device_id, generation),
        segment_name(sequence, digest)
    )
}

fn segment_name(sequence: u32, digest: &str) -> String {
    format!("{sequence:010}-{digest}.{SEGMENT_SUFFIX}")
}

fn parse_segment_name(name: &str) -> Option<(u32, String)> {
    let (sequence, rest) = name.split_once('-')?;
    let (digest, suffix) = rest.rsplit_once('.')?;
    if sequence.len() != 10
        || !sequence.bytes().all(|byte| byte.is_ascii_digit())
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || suffix != SEGMENT_SUFFIX
    {
        return None;
    }
    Some((sequence.parse().ok()?, digest.to_ascii_lowercase()))
}

/// Directory and file names on a peer's side of the tree are untrusted input; they
/// only have to be usable as a single path component.
fn safe_part(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PATH_PART_BYTES
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
}

fn payload_digest(bundle: &IncrementalSyncBundle) -> Result<String> {
    incremental_bundle_payload_sha256(bundle)
        .map(hex::encode)
        .map_err(bundle_error)
}

fn next_resume(bundle: &IncrementalSyncBundle) -> Result<Option<IncrementalBundleResume>> {
    PeerSyncService::next_resume(bundle).map_err(engine_error)
}

fn write_new_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| GatewayError::StateUnavailable)?;
    writer
        .write_all(bytes)
        .and_then(|()| writer.sync_all())
        .map_err(|_| GatewayError::StateUnavailable)?;
    drop(writer);
    private_file(path)
}

async fn upload_segment(
    client: &WebDavClient,
    store: &ConfigStore,
    remote: &str,
    file: &Path,
    bytes: &[u8],
) -> Result<()> {
    ensure_collections(client, remote).await?;
    client.create_immutable(remote, file).await?;
    // A reply is never proof: this deployment has no strong ETags, so the stored
    // bytes are compared with the ones that were sent.
    let mut incoming = download_file(store)?;
    let revision = client
        .download(remote, &mut incoming.file)
        .await
        .map_err(|_| GatewayError::SyncOutcomeUnknown)?;
    if revision.sha256 != sha256_hex(bytes) {
        return Err(GatewayError::SyncSegmentCorrupt);
    }
    Ok(())
}

/// Creates each collection top-down once per run; a server that refuses a repeated
/// MKCOL is confirmed with a PROPFIND instead of failing the whole segment.
async fn ensure_collections(client: &WebDavClient, remote: &str) -> Result<()> {
    let mut path = String::new();
    for part in remote
        .split('/')
        .take_while(|part| !part.ends_with(SEGMENT_SUFFIX))
    {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        client.create_collection(&path).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// receive
// ---------------------------------------------------------------------------

/// A segment that is not applicable yet, kept so the next round does not pay for
/// another download.
struct Fetched {
    sequence: u32,
    bundle: IncrementalSyncBundle,
}

struct Receive<'a> {
    vault: &'a Vault,
    store: &'a ConfigStore,
    client: &'a WebDavClient,
    device_id: &'a str,
    cursor: &'a mut Cursor,
    report: &'a mut Report,
    /// Segments that are not applicable yet, so a retry never pays for another
    /// download.
    fetched: BTreeMap<String, Fetched>,
}

async fn receive(
    vault: &Vault,
    store: &ConfigStore,
    client: &WebDavClient,
    root: &str,
    device_id: &str,
    cursor: &mut Cursor,
    report: &mut Report,
) -> Result<()> {
    let streams = list_streams(client, root, device_id).await?;
    let mut state = Receive {
        vault,
        store,
        client,
        device_id,
        cursor,
        report,
        fetched: BTreeMap::new(),
    };
    for _ in 0..MAX_RECEIVE_ROUNDS {
        let mut progressed = false;
        for (key, segments) in &streams {
            progressed |= state.advance(key, segments).await?;
        }
        if !progressed {
            break;
        }
    }
    state.report.blocked_streams = streams
        .iter()
        .filter(|(key, _)| {
            state
                .cursor
                .streams
                .get(key.as_str())
                .is_some_and(|stream| stream.blocked.is_some())
        })
        .count();
    save_cursor(state.store, state.cursor)?;
    Ok(())
}

impl Receive<'_> {
    /// Applies one stream as far as causality allows and returns whether the cursor
    /// moved, so a stalled stream cannot spin the round loop.
    async fn advance(&mut self, key: &str, segments: &[Segment]) -> Result<bool> {
        let mut progressed = false;
        let (source, generation) = key.split_once('/').ok_or(GatewayError::InvalidWebDav)?;
        for segment in segments {
            let stream = self.cursor.streams.get(key).cloned().unwrap_or_default();
            if segment.sequence < stream.next_sequence {
                continue;
            }
            if segment.sequence > stream.next_sequence {
                self.stall(key, WAITING_EARLIER_SEGMENT)?;
                return Ok(progressed);
            }
            if stream.complete {
                self.stall(key, WAITING_AFTER_COMPLETION)?;
                return Ok(progressed);
            }
            let cached = self
                .fetched
                .get(key)
                .filter(|entry| entry.sequence == segment.sequence)
                .map(|entry| entry.bundle.clone());
            let bundle = match cached {
                Some(bundle) => bundle,
                None => {
                    self.fetched.remove(key);
                    let bytes = download_segment(self.client, self.store, segment).await?;
                    self.report.downloaded_segments += 1;
                    let bundle = parse_segment(self.vault, &bytes, &segment.digest)?;
                    if bundle.manifest.vault_id != self.cursor.vault_id
                        || bundle.manifest.source_device_id != source
                        || bundle.manifest.transfer_id != generation
                        || bundle.manifest.segment_index != segment.sequence
                    {
                        self.stall(key, WAITING_PATH_MISMATCH)?;
                        return Err(GatewayError::SyncSegmentCorrupt);
                    }
                    if self.fetched.len() < MAX_CACHED_SEGMENTS {
                        self.fetched.insert(
                            key.to_owned(),
                            Fetched {
                                sequence: segment.sequence,
                                bundle: bundle.clone(),
                            },
                        );
                    }
                    bundle
                }
            };
            // A stream never seen before is anchored on the base the segment itself
            // declares, which is also what a replayed bootstrap carries.
            let base = stream
                .checkpoint
                .unwrap_or_else(|| bundle.manifest.base.clone());
            let applied = match apply_segment(
                self.vault,
                self.device_id,
                &bundle,
                &base,
                stream.resume.as_ref(),
            ) {
                Ok(applied) => applied,
                Err(ApplyFailure::MissingParent) => {
                    // Another peer's generation has to land first. The round loop
                    // retries this stream; nothing is lost by waiting.
                    self.stall(key, WAITING_PARENT_COMMIT)?;
                    return Ok(progressed);
                }
                Err(ApplyFailure::Failed(error)) => return Err(error),
            };
            self.report.applied_commits += applied.result.applied_commits;
            self.report.skipped_commits += applied.result.skipped_commits;
            self.report.conflicts += applied.result.conflict_count;
            self.fetched.remove(key);
            let complete = bundle.manifest.is_last;
            self.cursor.streams.insert(
                key.to_owned(),
                Stream {
                    next_sequence: segment.sequence + 1,
                    checkpoint: if complete {
                        None
                    } else {
                        Some(bundle.manifest.result.clone())
                    },
                    resume: if complete {
                        None
                    } else {
                        next_resume(&bundle)?
                    },
                    complete,
                    blocked: None,
                },
            );
            // Re-pointing our own export base means the applied commits are already on
            // the remote; the open generation started before them, so it has to close.
            if self.cursor.export_base.as_ref() == Some(&applied.before) {
                self.cursor.export_base = Some(applied.after);
                self.cursor.export_resume = None;
            }
            save_cursor(self.store, self.cursor)?;
            progressed = true;
        }
        Ok(progressed)
    }

    /// Records why a stream cannot move yet, before handing control back: the reason
    /// is what `webdav status` shows while a peer still owes the missing piece.
    fn stall(&mut self, key: &str, reason: &str) -> Result<()> {
        block(self.cursor, key, reason);
        save_cursor(self.store, self.cursor)
    }
}

async fn list_streams(
    client: &WebDavClient,
    root: &str,
    device_id: &str,
) -> Result<Vec<(String, Vec<Segment>)>> {
    let mut streams = Vec::new();
    let devices = format!("{root}/streams");
    for device in client.list(&devices).await? {
        if !device.is_directory || device.name() == device_id || !safe_part(device.name()) {
            continue;
        }
        let device_path = format!("{devices}/{}", device.name());
        for generation in client.list(&device_path).await? {
            if !generation.is_directory || !safe_part(generation.name()) {
                continue;
            }
            let directory = format!("{device_path}/{}/segments", generation.name());
            let key = format!("{}/{}", device.name(), generation.name());
            let mut segments = Vec::new();
            for entry in client.list(&directory).await? {
                if entry.is_directory || !safe_part(entry.name()) {
                    continue;
                }
                let Some((sequence, digest)) = parse_segment_name(entry.name()) else {
                    continue;
                };
                if segments
                    .iter()
                    .any(|seen: &Segment| seen.sequence == sequence)
                {
                    // Two names for one sequence means somebody rewrote a segment
                    // that is supposed to be immutable. Nothing here is applicable.
                    return Err(GatewayError::SyncSegmentCorrupt);
                }
                segments.push(Segment {
                    sequence,
                    digest,
                    path: format!("{directory}/{}", entry.name()),
                });
            }
            segments.sort_by_key(|segment| segment.sequence);
            if !segments.is_empty() {
                streams.push((key, segments));
            }
        }
    }
    Ok(streams)
}

fn block(cursor: &mut Cursor, key: &str, reason: &str) {
    let stream = cursor.streams.entry(key.to_owned()).or_default();
    stream.blocked = Some(reason.to_owned());
}

async fn download_segment(
    client: &WebDavClient,
    store: &ConfigStore,
    segment: &Segment,
) -> Result<Vec<u8>> {
    let mut incoming = download_file(store)?;
    client.download(&segment.path, &mut incoming.file).await?;
    std::fs::read(incoming.path()).map_err(|_| GatewayError::StateUnavailable)
}

fn parse_segment(vault: &Vault, bytes: &[u8], digest: &str) -> Result<IncrementalSyncBundle> {
    let key = integrity_key(vault)?;
    let bundle = match bundle_file_from_bytes_authenticated(bytes, &key) {
        Ok(SyncBundleFile::Incremental(bundle)) => *bundle,
        Ok(SyncBundleFile::Complete(_)) => {
            // A complete bundle is a whole-vault snapshot; merging it blind would
            // discard local commits, so it is refused rather than applied.
            return Err(GatewayError::RemoteProtocolUnsupported);
        }
        Err(error) => return Err(bundle_error(error)),
    };
    if payload_digest(&bundle)? != digest {
        return Err(GatewayError::SyncSegmentCorrupt);
    }
    Ok(bundle)
}

fn integrity_key(vault: &Vault) -> Result<Zeroizing<Vec<u8>>> {
    vault
        .runtime
        .with_read(|conn| {
            conn.keyring()
                .map(|keyring| keyring.integrity_subkey.clone())
                .ok_or_else(|| {
                    StorageError::Validation(
                        "peer synchronization requires an unlocked vault".to_owned(),
                    )
                })
        })
        .map_err(engine_error)
}

fn current_checkpoint(vault: &Vault) -> Result<IncrementalBundleCheckpoint> {
    vault
        .runtime
        .with_read(PeerSyncService::current_checkpoint)
        .map_err(engine_error)
}

fn export_segment(
    vault: &Vault,
    device_id: &str,
    base: &IncrementalBundleCheckpoint,
    resume: Option<&IncrementalBundleResume>,
) -> Result<(IncrementalSyncBundle, Vec<u8>)> {
    let key = integrity_key(vault)?;
    let bundle = vault
        .runtime
        .with_read(|conn| {
            PeerSyncService::export_incremental_segment(
                conn,
                device_id,
                base,
                resume,
                PeerSyncSegmentOptions {
                    page_size: SEGMENT_PAGE_SIZE,
                },
            )
        })
        .map_err(engine_error)?;
    let bytes = incremental_bundle_to_bytes_authenticated(&bundle, &key).map_err(bundle_error)?;
    Ok((bundle, bytes))
}

struct Applied {
    result: ApplyBatchResult,
    before: IncrementalBundleCheckpoint,
    after: IncrementalBundleCheckpoint,
}

enum ApplyFailure {
    /// The engine rolled the segment back because a parent commit is still on a
    /// peer's side of the tree. Waiting is the correct response, not a conflict.
    MissingParent,
    Failed(GatewayError),
}

fn apply_segment(
    vault: &Vault,
    device_id: &str,
    bundle: &IncrementalSyncBundle,
    base: &IncrementalBundleCheckpoint,
    resume: Option<&IncrementalBundleResume>,
) -> std::result::Result<Applied, ApplyFailure> {
    vault
        .runtime
        .with_write(|conn| {
            let before = PeerSyncService::current_checkpoint(conn)?;
            let result =
                PeerSyncService::apply_incremental_segment(conn, device_id, bundle, base, resume)?;
            let after = PeerSyncService::current_checkpoint(conn)?;
            Ok(Applied {
                result,
                before,
                after,
            })
        })
        .map_err(|error| {
            // `apply_incremental_batch_mut` reports the refusal with this exact
            // wording and zeroes the counter on every path that succeeds, so the
            // result struct can never signal it.
            if matches!(&error, StorageError::Validation(message)
                if message.contains("commit parent"))
            {
                ApplyFailure::MissingParent
            } else {
                ApplyFailure::Failed(engine_error(error))
            }
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `Database` carries the missing-table failures of an early Android vault; a
/// constraint or validation message means the cursor and the remote disagree, which
/// is a conflict a person has to resolve rather than transport damage.
fn engine_error(error: StorageError) -> GatewayError {
    match &error {
        StorageError::Database(_) if error.to_string().contains("no such table") => {
            GatewayError::VaultSchemaUnsupported
        }
        StorageError::Validation(message) if message.contains("unlocked") => {
            GatewayError::UnlockRequired
        }
        StorageError::Io(_) | StorageError::SchemaCreation(_) => GatewayError::StateUnavailable,
        StorageError::ResourceLimit { .. } => GatewayError::ResponseTooLarge,
        _ => GatewayError::SyncConflict,
    }
}

fn bundle_error(error: SyncError) -> GatewayError {
    match error {
        SyncError::BundleIntegrity(_) | SyncError::IoError(_) => GatewayError::SyncSegmentCorrupt,
        SyncError::ResourceLimit { .. } => GatewayError::ResponseTooLarge,
        SyncError::BundleFormat(_)
        | SyncError::UnsupportedFeature(_)
        | SyncError::VersionMismatch { .. } => GatewayError::VaultSchemaUnsupported,
        SyncError::Protocol(_)
        | SyncError::Serialization(_)
        | SyncError::InvalidMessage(_)
        | SyncError::Connection(_)
        | SyncError::Conflict(_) => GatewayError::SyncConflict,
    }
}
