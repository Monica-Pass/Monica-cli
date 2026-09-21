use std::path::Path;

use zeroize::Zeroizing;

use crate::admin;
use crate::config::{ConfigStore, DEFAULT_PORT};
use crate::error::GatewayError;
use crate::model::Provider;
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
            .unwrap(),
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
            .unwrap(),
        SyncResult::UpToDate
    );
    add(&writer, "second");
    admin::update_note(&writer, "work", "同步后的项目用途备注", PASSWORD).unwrap();
    assert_eq!(
        sync::synchronize(&writer, &remote.client, PASSWORD)
            .await
            .unwrap(),
        SyncResult::Uploaded
    );
    assert_eq!(
        sync::synchronize(&reader, &remote.client, PASSWORD)
            .await
            .unwrap(),
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
            .unwrap(),
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
        sync::synchronize(&reader, &remote.client, PASSWORD).await,
        Err(GatewayError::SyncConflict)
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
async fn android_segment_layout_is_refused_before_the_bootstrap_is_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let store = initialized(directory.path(), "writer");
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
    let local_config = std::fs::read(&store.path).unwrap();
    let bootstrap = remote.files.lock().unwrap()["/dav/Mdbx/vault.mdbx"].clone();
    let requests = remote.server.requests().len();

    let error = sync::synchronize(&store, &remote.client, PASSWORD)
        .await
        .unwrap_err();
    assert_eq!(error, GatewayError::RemoteProtocolUnsupported);
    assert!(error.to_string().contains(".sync"), "{error}");
    assert_eq!(std::fs::read(&store.path).unwrap(), local_config);
    assert_eq!(
        remote.files.lock().unwrap()["/dav/Mdbx/vault.mdbx"],
        bootstrap
    );
    assert!(
        remote.server.requests()[requests..]
            .iter()
            .all(|request| request.method != "PUT"),
        "a detected segment vault must never be written to"
    );
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
        sync::synchronize(&store, &remote.client, PASSWORD).await,
        Err(GatewayError::SyncConflict)
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
        sync::synchronize(&store, &remote.client, PASSWORD).await,
        Err(GatewayError::SyncOutcomeUnknown)
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
            .unwrap(),
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
        sync::synchronize(&store, &remote.client, PASSWORD).await,
        Err(GatewayError::BrokerAlreadyRunning)
    );
    assert_eq!(remote.server.requests().len(), request_count);
    assert_eq!(std::fs::read(&store.path).unwrap(), config);
    drop(guard);
}
