use std::path::Path;

use zeroize::Zeroizing;

use crate::admin;
use crate::config::{ConfigStore, DEFAULT_PORT};
use crate::error::GatewayError;
use crate::model::Provider;
use crate::segment::{self, ExportBase};
use crate::sync::{self, SyncResult};
use crate::test_support::{PASSWORD, TOKEN};
use crate::webdav_tests::{DAV_PASSWORD, FakeWebDav, WriteFault};

fn initialized(parent: &Path, name: &str) -> ConfigStore {
    let store = ConfigStore::new(parent.join(name).join("gateway.json"));
    admin::initialize(
        &store,
        &store.path.with_extension("mdbx"),
        DEFAULT_PORT,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    add(&store, "work");
    store
}

fn add(store: &ConfigStore, name: &str) {
    admin::add_connection(
        store,
        name,
        Provider::Github,
        Provider::Github.default_api_base(),
        "用于项目 Issue 跟踪",
        PASSWORD,
        Zeroizing::new(TOKEN.to_owned()),
    )
    .unwrap();
}

fn get_requests(remote: &FakeWebDav) -> usize {
    remote
        .server
        .requests()
        .iter()
        .filter(|request| request.method == "GET")
        .count()
}

#[tokio::test]
async fn sync_real_mdbx_roundtrip_preserves_old_copies_and_detects_divergence() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    assert_eq!(
        sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap(),
        SyncResult::Published
    );
    assert_eq!(
        sync::synchronize(&writer, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate
    );
    let reader = ConfigStore::new(directory.path().join("reader/gateway.json"));
    assert_eq!(
        sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap(),
        1
    );
    let original_reader = reader.load().unwrap().vault;
    assert_eq!(
        reader.load().unwrap().connections["work"].note,
        "用于项目 Issue 跟踪"
    );
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate
    );
    add(&writer, "second");
    admin::update_note(&writer, "work", "同步后的项目用途备注", PASSWORD).unwrap();
    assert_eq!(
        sync::synchronize(&writer, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::Uploaded
    );
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::Downloaded
    );
    assert_ne!(reader.load().unwrap().vault, original_reader);
    assert!(original_reader.is_file());
    assert_eq!(reader.load().unwrap().connections.len(), 2);
    assert_eq!(
        reader.load().unwrap().connections["work"].note,
        "同步后的项目用途备注"
    );
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate
    );
    add(&writer, "writer-change");
    add(&reader, "reader-change");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();
    let prior_config = std::fs::read(&reader.path).unwrap();
    let prior_remote = remote.files.lock().unwrap()["/dav/vault.mdbx"].clone();
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap_err(),
        GatewayError::SyncConflict
    );
    assert_eq!(std::fs::read(&reader.path).unwrap(), prior_config);
    assert_eq!(
        remote.files.lock().unwrap()["/dav/vault.mdbx"],
        prior_remote
    );
    assert!(reader.path.with_extension("history").is_dir());
    for secret in [TOKEN, PASSWORD, DAV_PASSWORD] {
        assert!(
            !prior_config
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_bytes())
        );
        assert!(
            !prior_remote
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_bytes())
        );
    }
}

#[tokio::test]
async fn android_segment_layout_is_joined_without_replacing_the_bootstrap() {
    let directory = tempfile::tempdir().unwrap();
    let store = initialized(directory.path(), "writer");
    // The name Android would never use is still a plausible neighbour in a tree the
    // phone owns: it has to be skipped, not parsed and not fatal.
    let remote = FakeWebDav::new(
        [(
            "/dav/Mdbx/vault.mdbx.sync/streams/android-device/generation-1/segments/0000000000-integrity.mdbxsync".to_owned(),
            b"encrypted android segment".to_vec(),
        )]
        .into(),
    )
    .await;
    sync::publish(&store, &remote.client, "Mdbx/vault.mdbx", PASSWORD)
        .await
        .unwrap();
    add(&store, "changed-after-android");
    let bootstrap = remote.files.lock().unwrap()["/dav/Mdbx/vault.mdbx"].clone();
    let requests = remote.server.requests().len();

    let outcome = sync::synchronize(&store, &remote.client, PASSWORD)
        .await
        .unwrap();
    assert_eq!(outcome.result, SyncResult::Merged);
    let report = outcome.segments.expect("a segment run reports what moved");
    assert_eq!(report.uploaded_segments, 1);
    assert_eq!(report.downloaded_segments, 0);
    assert_eq!(report.conflicts, 0);
    assert_eq!(report.blocked_streams, 0);
    assert_eq!(
        remote.files.lock().unwrap()["/dav/Mdbx/vault.mdbx"],
        bootstrap,
        "the bootstrap every other device tracks must stay byte-identical"
    );
    let device_id = store.load().unwrap().webdav_device_id.clone().unwrap();
    assert!(device_id.starts_with("monica-cli-"), "{device_id}");
    let sent = remote.server.requests();
    let writes = sent[requests..]
        .iter()
        .filter(|request| request.method == "PUT")
        .collect::<Vec<_>>();
    assert!(!writes.is_empty());
    for write in writes {
        assert!(
            write
                .target
                .starts_with(&format!("/dav/Mdbx/vault.mdbx.sync/streams/{device_id}/")),
            "{}",
            write.target
        );
    }

    let again = sync::synchronize(&store, &remote.client, PASSWORD)
        .await
        .unwrap();
    assert_eq!(again.result, SyncResult::UpToDate);
    assert!(again.segments.is_none());
}

#[tokio::test]
async fn segment_streams_converge_two_devices_without_replacing_any_file() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    // The sidecar tree a phone would have created by now: joining it has to beat
    // falling back to replacing the single file.
    remote.seed_collection("vault.mdbx.sync");
    add(&writer, "writer-only");
    let pushed = sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();
    assert_eq!(pushed.result, SyncResult::Merged);
    assert_eq!(pushed.segments.unwrap().uploaded_segments, 1);

    let reader = initialized(directory.path(), "reader");
    assert_eq!(
        sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap(),
        2,
        "opening the remote vault has to include the published segments"
    );
    assert!(
        reader
            .load()
            .unwrap()
            .connections
            .contains_key("writer-only")
    );
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate,
        "a cold connect owes the remote nothing it was just told"
    );

    add(&reader, "reader-only");
    let reply = sync::synchronize(&reader, &remote.client, PASSWORD)
        .await
        .unwrap()
        .segments
        .expect("the reply published a segment");
    assert_eq!(reply.uploaded_segments, 1);
    assert_eq!(reply.downloaded_segments, 0);
    let pull = sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap()
        .segments
        .expect("the writer read the reply back");
    assert_eq!(pull.uploaded_segments, 0);
    assert_eq!(pull.downloaded_segments, 1);
    assert!(pull.applied_commits > 0);
    assert!(
        writer
            .load()
            .unwrap()
            .connections
            .contains_key("reader-only")
    );
    assert!(
        reader
            .load()
            .unwrap()
            .connections
            .contains_key("writer-only")
    );
    assert_eq!(
        sync::synchronize(&writer, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate,
        "a received commit must not be published back as one's own"
    );
    let view = segment::status(&writer, &writer.load().unwrap().webdav.unwrap().vault_id).unwrap();
    assert!(
        view.tracked && view.export_base == ExportBase::Anchored,
        "a drained run leaves the export anchored, not owing a full re-export: {view:?}"
    );
    assert!(view.pending_upload.is_none(), "{view:?}");
    assert_eq!(
        view.streams, 1,
        "only the peer stream is received: {view:?}"
    );
    assert!(view.waiting.is_empty(), "{view:?}");
    let held: Vec<usize> = {
        let files = remote.files.lock().unwrap();
        files
            .iter()
            .filter(|(name, _)| {
                name.starts_with("/dav/vault.mdbx.sync/") && name.ends_with(".mdbxsync")
            })
            .map(|(_, bytes)| bytes.len())
            .collect()
    };
    let usage = view.usage.expect("the receive walk measured the tree");
    assert_eq!(
        usage.segments,
        held.len(),
        "every segment file in the tree counts, this device's own uploads included"
    );
    assert_eq!(
        usage.bytes as usize,
        held.iter().sum::<usize>(),
        "the reported total is what the server actually holds"
    );
    assert_eq!(usage.unmeasured, 0, "{usage:?}");
    for secret in [TOKEN, PASSWORD, DAV_PASSWORD] {
        for bytes in remote.files.lock().unwrap().values() {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|chunk| chunk == secret.as_bytes()),
                "{secret} reached the remote"
            );
        }
    }
}

#[tokio::test]
async fn segment_gap_in_a_peer_stream_waits_and_the_cursor_says_why() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    remote.seed_collection("vault.mdbx.sync");
    add(&writer, "writer-only");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();

    // A peer whose upload was cut short leaves a hole no later segment can cover:
    // applying segment 9 without 1..8 would drop the commits in between silently.
    let parent = remote
        .files
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.ends_with(".mdbxsync"))
        .map(|key| key.rsplit_once('/').unwrap().0.to_owned())
        .expect("the writer published a segment");
    let stream = parent
        .trim_start_matches("/dav/vault.mdbx.sync/streams/")
        .trim_end_matches("/segments")
        .to_owned();
    remote.files.lock().unwrap().insert(
        format!("{parent}/0000000009-{}.mdbxsync", "c".repeat(64)),
        b"unreachable-without-the-segments-before-it".to_vec(),
    );

    let reader = initialized(directory.path(), "reader");
    assert_eq!(
        sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap(),
        2,
        "the segments up to the hole still land"
    );
    let report = sync::synchronize(&reader, &remote.client, PASSWORD)
        .await
        .unwrap()
        .segments
        .expect("a stalled stream is reported");
    assert_eq!(report.blocked_streams, 1);
    assert_eq!(report.conflicts, 0);
    assert_eq!(report.uploaded_segments, 0);
    assert_eq!(
        report.downloaded_segments, 0,
        "the segment past the hole is never even fetched"
    );
    assert_eq!(report.applied_commits, 0);
    assert!(
        reader
            .load()
            .unwrap()
            .connections
            .contains_key("writer-only")
    );

    let binding = reader.load().unwrap().webdav.unwrap();
    let view = segment::status(&reader, &binding.vault_id).unwrap();
    assert_eq!(view.streams, 1);
    assert_eq!(
        view.complete_streams, 1,
        "the segment before the hole did apply; the one past it did not"
    );
    assert_eq!(
        view.waiting,
        vec![segment::Waiting {
            stream,
            reason: segment::WAITING_EARLIER_SEGMENT.to_owned(),
        }],
        "why a stream stopped is the one thing a status line has to carry"
    );
}

#[tokio::test]
async fn a_sync_folder_that_holds_no_segment_yet_reads_as_emptiness() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    // Monica Android creates the folder when it sets sync up, before it has any
    // history to cut, so the first client to connect reads an empty tree.
    remote.seed_collection("vault.mdbx.sync");
    let reader = initialized(directory.path(), "reader");
    assert_eq!(
        sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap(),
        1
    );
    let quiet = sync::synchronize(&reader, &remote.client, PASSWORD)
        .await
        .unwrap();
    assert_eq!(quiet.result, SyncResult::UpToDate);
    assert_eq!(quiet.segments, None, "nothing crossed the wire");

    add(&reader, "first");
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap()
            .segments
            .expect("this device now owes a segment")
            .uploaded_segments,
        1
    );
}

#[tokio::test]
async fn a_cancelled_replay_stops_between_segments_and_resumes_without_loss() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    remote.seed_collection("vault.mdbx.sync");
    let reader = initialized(directory.path(), "reader");
    sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    // Two peer generations published after the reader joined. Each run closes its
    // generation, so the listing order is a UUID draw: one of them needs the other
    // first and has to be fetched to find that out. What the stop owes is therefore
    // "no read after it", not "exactly one read in total".
    add(&writer, "peer-two");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();
    add(&writer, "peer-three");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();

    let cancel = segment::Cancel::default();
    let mut seen: Vec<segment::Event> = Vec::new();
    let mut fetched_at_stop: Option<usize> = None;
    let mut sink = |event: &segment::Event| {
        seen.push(event.clone());
        if matches!(event, segment::Event::Applied { .. }) && fetched_at_stop.is_none() {
            cancel.trigger();
            fetched_at_stop = Some(get_requests(&remote));
        }
    };
    let stopped =
        sync::synchronize_with_progress(&reader, &remote.client, PASSWORD, &mut sink, &cancel)
            .await
            .unwrap();
    let report = stopped.segments.expect("an interrupted run is reported");
    assert!(report.cancelled);
    assert_eq!(
        get_requests(&remote),
        fetched_at_stop.expect("the replay reached a segment it could apply"),
        "the segment past the stop was never fetched: {report:?}"
    );
    assert!(
        seen.iter()
            .any(|event| matches!(event, segment::Event::Applied { sequence: 0, .. })),
        "a replay that shows nothing per segment is what this fixes: {seen:?}"
    );
    let names = reader.load().unwrap().connections;
    assert!(names.contains_key("peer-two"));
    assert!(!names.contains_key("peer-three"));

    let resumed = sync::synchronize(&reader, &remote.client, PASSWORD)
        .await
        .unwrap()
        .segments
        .expect("the rest of the history is still owed");
    assert!(!resumed.cancelled);
    assert_eq!(resumed.downloaded_segments, 1);
    assert_eq!(resumed.conflicts, 0);
    assert!(
        reader
            .load()
            .unwrap()
            .connections
            .contains_key("peer-three")
    );
}

#[tokio::test]
async fn segment_replay_after_losing_the_cursor_skips_commits_it_already_has() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    remote.seed_collection("vault.mdbx.sync");
    add(&writer, "writer-only");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();

    let reader = initialized(directory.path(), "reader");
    sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    let before = reader
        .load()
        .unwrap()
        .connections
        .into_keys()
        .collect::<Vec<_>>();
    assert!(before.contains(&"writer-only".to_owned()));

    // Local state is not part of the guarantee: the tree on the remote is. Losing
    // the cursor has to cost one re-read, never a second copy of a commit.
    std::fs::remove_file(reader.path.with_extension("sync.json")).unwrap();
    let again = sync::synchronize(&reader, &remote.client, PASSWORD)
        .await
        .unwrap();
    let report = again.segments.expect("a replayed stream is reported");
    assert!(
        report.skipped_commits > 0,
        "the engine saw its own history again: {:?}",
        report
    );
    assert_eq!(report.conflicts, 0);
    assert_eq!(
        reader
            .load()
            .unwrap()
            .connections
            .into_keys()
            .collect::<Vec<_>>(),
        before
    );
}

#[tokio::test]
async fn segment_stream_bytes_that_do_not_hash_to_their_name_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let writer = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&writer, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    remote.seed_collection("vault.mdbx.sync");
    add(&writer, "writer-only");
    sync::synchronize(&writer, &remote.client, PASSWORD)
        .await
        .unwrap();

    // The bytes stay exactly as this device signed them; only the name is re-planted
    // under another digest, which is what a lying or editing server produces.
    let segment = remote
        .files
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.ends_with(".mdbxsync"))
        .cloned()
        .expect("the writer published a segment");
    {
        let (parent, name) = segment.rsplit_once('/').unwrap();
        let sequence = name.split_once('-').unwrap().0;
        let mut files = remote.files.lock().unwrap();
        let bytes = files.remove(&segment).unwrap();
        files.insert(
            format!("{parent}/{sequence}-{}.mdbxsync", "b".repeat(64)),
            bytes,
        );
    }

    let reader = initialized(directory.path(), "reader");
    assert_eq!(
        sync::open_remote(&reader, &remote.client, "vault.mdbx", PASSWORD)
            .await
            .unwrap_err(),
        GatewayError::SyncSegmentCorrupt,
        "a segment that does not hash to its own name never reaches the engine"
    );
    let settled = reader.load().unwrap();
    assert!(
        settled.webdav.is_none(),
        "a refused replay must not leave a remote configured"
    );
    assert!(!settled.connections.contains_key("writer-only"));
}

#[tokio::test]
async fn sync_etag_race_and_lost_upload_response_do_not_overwrite_or_repeat_writes() {
    let directory = tempfile::tempdir().unwrap();
    let store = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(Default::default()).await;
    sync::publish(&store, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    add(&store, "added");
    let previous_config = std::fs::read(&store.path).unwrap();
    let prior_remote = remote.files.lock().unwrap()["/dav/vault.mdbx"].clone();
    *remote.next_write.lock().unwrap() = Some(WriteFault::Race(b"racing remote revision".to_vec()));
    assert_eq!(
        sync::synchronize(&store, &remote.client, PASSWORD)
            .await
            .unwrap_err(),
        GatewayError::SyncConflict
    );
    assert_eq!(
        remote.files.lock().unwrap()["/dav/vault.mdbx"],
        b"racing remote revision"
    );
    assert_eq!(std::fs::read(&store.path).unwrap(), previous_config);
    remote
        .files
        .lock()
        .unwrap()
        .insert("/dav/vault.mdbx".to_owned(), prior_remote);
    *remote.next_write.lock().unwrap() = Some(WriteFault::LoseResponse);
    assert_eq!(
        sync::synchronize(&store, &remote.client, PASSWORD)
            .await
            .unwrap_err(),
        GatewayError::SyncOutcomeUnknown
    );
    assert_eq!(std::fs::read(&store.path).unwrap(), previous_config);
    let put_count = remote
        .server
        .requests()
        .iter()
        .filter(|request| request.method == "PUT")
        .count();
    assert_eq!(
        sync::synchronize(&store, &remote.client, PASSWORD)
            .await
            .unwrap()
            .result,
        SyncResult::UpToDate
    );
    assert_eq!(
        remote
            .server
            .requests()
            .iter()
            .filter(|request| request.method == "PUT")
            .count(),
        put_count
    );
}

#[tokio::test]
async fn sync_invalid_vault_password_and_busy_broker_preserve_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let store = initialized(directory.path(), "writer");
    let remote = FakeWebDav::new(
        [(
            "/dav/invalid.mdbx".to_owned(),
            b"not an MDBX vault".to_vec(),
        )]
        .into(),
    )
    .await;
    sync::publish(&store, &remote.client, "vault.mdbx", PASSWORD)
        .await
        .unwrap();
    let config = std::fs::read(&store.path).unwrap();
    assert!(matches!(
        sync::open_remote(&store, &remote.client, "invalid.mdbx", PASSWORD).await,
        Err(GatewayError::InvalidVault)
    ));
    assert!(matches!(
        sync::open_remote(&store, &remote.client, "vault.mdbx", "wrong").await,
        Err(GatewayError::UnlockRequired)
    ));
    assert_eq!(std::fs::read(&store.path).unwrap(), config);
    let guard = store.acquire_broker_lock().unwrap();
    let request_count = remote.server.requests().len();
    assert_eq!(
        sync::synchronize(&store, &remote.client, PASSWORD)
            .await
            .unwrap_err(),
        GatewayError::BrokerAlreadyRunning
    );
    assert_eq!(remote.server.requests().len(), request_count);
    assert_eq!(std::fs::read(&store.path).unwrap(), config);
    drop(guard);
}
