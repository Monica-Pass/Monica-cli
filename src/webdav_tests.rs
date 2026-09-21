use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::GatewayError;
use crate::test_support::{FakeUpstream, Reply};
use crate::webdav::{WebDavClient, WebDavProfile, WriteCondition, normalize_path};

pub(crate) const DAV_PASSWORD: &str = "synthetic-webdav-password-only";

#[tokio::test]
async fn webdav_public_metadata_cannot_disclose_the_session_password() {
    let profile = WebDavProfile::new("https://example.invalid/dav/", DAV_PASSWORD).unwrap();
    assert!(matches!(
        WebDavClient::new(profile, zeroize::Zeroizing::new(DAV_PASSWORD.to_owned())),
        Err(GatewayError::SensitiveMetadata)
    ));
    let remote = FakeWebDav::new(
        [(
            format!("/dav/{DAV_PASSWORD}.mdbx"),
            b"encrypted fixture".to_vec(),
        )]
        .into(),
    )
    .await;
    assert_eq!(
        remote.client.list("").await.unwrap_err(),
        GatewayError::ResponseBlocked
    );
    let count = remote.server.requests().len();
    assert_eq!(
        remote.client.list(DAV_PASSWORD).await.unwrap_err(),
        GatewayError::SensitiveMetadata
    );
    assert_eq!(remote.server.requests().len(), count);

    let server = FakeUpstream::start(vec![response(
        200,
        b"encrypted fixture".to_vec(),
        Some(format!("\"{DAV_PASSWORD}\"")),
    )])
    .await;
    let profile =
        WebDavProfile::new(&format!("https://127.0.0.1:{}/dav/", server.port), "human").unwrap();
    let client = WebDavClient::for_test(profile, DAV_PASSWORD, server.client.clone());
    let mut target = tempfile::NamedTempFile::new().unwrap();
    assert!(matches!(
        client.download("vault.mdbx", &mut target).await,
        Err(GatewayError::ResponseBlocked)
    ));
    assert_eq!(target.as_file().metadata().unwrap().len(), 0);
}

pub(crate) struct FakeWebDav {
    pub server: FakeUpstream,
    pub client: WebDavClient,
    pub files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    pub collections: Arc<Mutex<BTreeSet<String>>>,
    pub next_write: Arc<Mutex<Option<WriteFault>>>,
}

pub(crate) enum WriteFault {
    Race(Vec<u8>),
    LoseResponse,
    /// Stores bytes other than those sent while replying as if the write had succeeded.
    Corrupt(Vec<u8>),
    /// Answers a conditional create with success even though the name is already taken.
    IgnoreConditions,
}

impl FakeWebDav {
    pub async fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
        let files = Arc::new(Mutex::new(files));
        let shared = files.clone();
        let collections = Arc::new(Mutex::new(BTreeSet::<String>::new()));
        let listed = collections.clone();
        let next_write = Arc::new(Mutex::new(None));
        let fault = next_write.clone();
        let expected_auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("human:{DAV_PASSWORD}"))
        );
        let server = FakeUpstream::start_handler(move |request| {
            if request.headers.get("authorization") != Some(&expected_auth) {
                return response(401, Vec::new(), None);
            }
            let path = request.target.clone();
            let mut files = shared.lock().unwrap();
            let mut collections = listed.lock().unwrap();
            match request.method.as_str() {
                "PROPFIND" => {
                    let directory = path.trim_end_matches('/');
                    let known = directory == "/dav"
                        || collections.contains(directory)
                        || files.keys().any(|key| key.starts_with(&path));
                    if !known {
                        return response(404, Vec::new(), None);
                    }
                    let mut xml = format!(r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>{path}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#);
                    for name in files.keys().filter(|key| key.starts_with(&path)) {
                        let mut ancestor = path.clone();
                        for part in name[path.len()..].split('/') {
                            ancestor.push_str(part);
                            if ancestor != *name {
                                xml.push_str(&format!(r#"<d:response><d:href>{ancestor}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#));
                            }
                            ancestor.push('/');
                        }
                    }
                    for name in collections
                        .iter()
                        .filter(|key| key.starts_with(&path) && **key != path)
                    {
                        xml.push_str(&format!(r#"<d:response><d:href>{name}/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#));
                    }
                    for (name, bytes) in files.iter().filter(|(key, _)| key.starts_with(&path)) {
                        let etag = etag(bytes).replace('"', "&quot;");
                        xml.push_str(&format!(r#"<d:response><d:href>{name}</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>{}</d:getcontentlength><d:getetag>{etag}</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#, bytes.len()));
                    }
                    xml.push_str("</d:multistatus>");
                    response(207, xml.into_bytes(), None)
                }
                "MKCOL" => {
                    let directory = path.trim_end_matches('/').to_owned();
                    if collections.contains(&directory) {
                        return response(405, Vec::new(), None);
                    }
                    let parent = directory.rsplit_once('/').map_or("/dav", |(parent, _)| parent);
                    if parent != "/dav" && !collections.contains(&parent.to_owned()) {
                        return response(409, Vec::new(), None);
                    }
                    collections.insert(directory);
                    response(201, Vec::new(), None)
                }
                "GET" => match files.get(&path) {
                    Some(bytes) => response(200, bytes.clone(), Some(etag(bytes))),
                    None => response(404, Vec::new(), None),
                },
                "PUT" => {
                    let failure = fault.lock().unwrap().take();
                    let lost_response = matches!(failure, Some(WriteFault::LoseResponse));
                    let ignore_conditions = matches!(failure, Some(WriteFault::IgnoreConditions));
                    let raced = matches!(failure, Some(WriteFault::Race(_)));
                    let stored = match failure {
                        Some(WriteFault::Race(bytes)) | Some(WriteFault::Corrupt(bytes)) => bytes,
                        _ => request.body.clone(),
                    };
                    if raced {
                        files.insert(path.clone(), stored.clone());
                    }
                    let existing = files.get(&path);
                    let create = request.headers.get("if-none-match").is_some_and(|value| value == "*");
                    let replace = existing.is_some_and(|bytes| request.headers.get("if-match") == Some(&etag(bytes)));
                    if !(create && existing.is_none() || !create && replace)
                        && !ignore_conditions
                    {
                        return response(412, Vec::new(), None);
                    }
                    files.insert(path, stored.clone());
                    let mut reply = response(201, Vec::new(), Some(etag(&stored)));
                    reply.disconnect = lost_response;
                    reply
                }
                _ => response(405, Vec::new(), None),
            }
        }).await;
        let profile =
            WebDavProfile::new(&format!("https://127.0.0.1:{}/dav/", server.port), "human")
                .unwrap();
        let client = WebDavClient::for_test(profile, DAV_PASSWORD, server.client.clone());
        Self {
            server,
            client,
            files,
            collections,
            next_write,
        }
    }

    /// Registers the collection chain a remote object path needs, the way a
    /// directory created earlier by any client would already exist on the server.
    pub fn seed_collection(&self, path: &str) {
        let mut collections = self.collections.lock().unwrap();
        let mut ancestor = String::new();
        for part in path.trim_matches('/').split('/') {
            ancestor.push('/');
            ancestor.push_str(part);
            collections.insert(format!("/dav{ancestor}"));
        }
    }
}

fn etag(bytes: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Sha256::digest(bytes)))
}

fn response(status: u16, body: Vec<u8>, etag: Option<String>) -> Reply {
    Reply {
        status,
        body,
        headers: etag
            .into_iter()
            .map(|etag| ("ETag".to_owned(), etag))
            .collect(),
        ..Reply::json(Value::Null)
    }
}

#[tokio::test]
async fn webdav_login_list_download_and_conditional_upload_use_real_https() {
    let remote =
        FakeWebDav::new([("/dav/vault.mdbx".to_owned(), b"encrypted fixture".to_vec())].into())
            .await;
    let entries = remote.client.list("").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "vault.mdbx");
    let mut downloaded = tempfile::NamedTempFile::new().unwrap();
    let revision = remote
        .client
        .download("vault.mdbx", &mut downloaded)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(downloaded.path()).unwrap(),
        b"encrypted fixture"
    );
    std::fs::write(downloaded.path(), b"new encrypted fixture").unwrap();
    assert_eq!(
        remote
            .client
            .upload("vault.mdbx", downloaded.path(), WriteCondition::Create)
            .await,
        Err(GatewayError::SyncConflict)
    );
    remote
        .client
        .upload(
            "vault.mdbx",
            downloaded.path(),
            WriteCondition::Match(revision.etag.as_deref().unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(
        remote
            .client
            .upload(
                "vault.mdbx",
                downloaded.path(),
                WriteCondition::Match(revision.etag.as_deref().unwrap())
            )
            .await,
        Err(GatewayError::SyncConflict)
    );
    let requests = remote.server.requests();
    assert_eq!(requests[0].method, "PROPFIND");
    assert_eq!(requests[0].headers["depth"], "1");
    assert_eq!(requests[2].headers["if-none-match"], "*");
    assert_eq!(requests[3].headers["if-match"], revision.etag.unwrap());
    assert!(
        requests
            .iter()
            .all(|request| !request.target.contains(DAV_PASSWORD))
    );
    let wrong = WebDavClient::for_test(
        remote.client.profile.clone(),
        "wrong",
        remote.server.client.clone(),
    );
    assert_eq!(
        wrong.list("").await.unwrap_err(),
        GatewayError::WebDavUnauthorized
    );
    let config = serde_json::to_string(&remote.client.profile).unwrap();
    assert!(!config.contains(DAV_PASSWORD));
}

#[tokio::test]
async fn webdav_redirects_oversized_bodies_and_weak_versions_are_rejected() {
    let server = FakeUpstream::start(vec![
        Reply {
            status: 302,
            location: Some("https://outside.test/".to_owned()),
            ..Reply::json(Value::Null)
        },
        Reply {
            headers: vec![(
                "Content-Length".to_owned(),
                (65_u64 * 1024 * 1024).to_string(),
            )],
            omit_length: true,
            ..Reply::json(Value::Null)
        },
    ])
    .await;
    let profile =
        WebDavProfile::new(&format!("https://127.0.0.1:{}/dav/", server.port), "human").unwrap();
    let client = WebDavClient::for_test(profile, DAV_PASSWORD, server.client.clone());
    assert_eq!(
        client.list("").await.unwrap_err(),
        GatewayError::RedirectBlocked
    );
    let mut file = tempfile::NamedTempFile::new().unwrap();
    assert!(matches!(
        client.download("vault.mdbx", &mut file).await,
        Err(GatewayError::ResponseTooLarge)
    ));
    std::fs::write(file.path(), b"encrypted").unwrap();
    assert_eq!(
        client
            .upload(
                "vault.mdbx",
                file.path(),
                WriteCondition::Match("W/\"weak\"")
            )
            .await,
        Err(GatewayError::RemoteVersionRequired)
    );
    assert_eq!(server.requests().len(), 2);
}

fn ancestors(path: &str) -> Vec<String> {
    let mut current = String::new();
    path.split('/')
        .map(|part| {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            current.clone()
        })
        .collect()
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn webdav_segment_collections_build_top_down_and_repeats_are_confirmed() {
    let remote = FakeWebDav::new(BTreeMap::new()).await;
    let segment = format!(
        "vault.mdbx.sync/streams/device-1/bootstrap/segments/0000000003-{}.mdbxsync",
        "a".repeat(64)
    );
    let chain = ancestors(segment.rsplit_once('/').unwrap().0);

    assert_eq!(
        remote.client.create_collection(&chain[3]).await,
        Err(GatewayError::SyncConflict)
    );
    for ancestor in &chain {
        remote.client.create_collection(ancestor).await.unwrap();
    }
    let first = remote.server.requests();
    assert_eq!(
        first
            .iter()
            .filter(|request| request.method == "MKCOL")
            .count(),
        chain.len() + 1
    );
    // A second sync run against the tree Android already owns must not fail on
    // the servers that answer MKCOL with 405, 409 or 501 instead of 201.
    for ancestor in &chain {
        remote.client.create_collection(ancestor).await.unwrap();
    }
    assert!(
        remote
            .server
            .requests()
            .iter()
            .skip(first.len())
            .all(|request| request.method == "MKCOL" || request.method == "PROPFIND")
    );
    assert!(remote.client.create_collection("").await.is_err());

    let payload = b"aad-segment-payload".to_vec();
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), &payload).unwrap();
    assert!(
        remote
            .client
            .create_immutable(&segment, file.path())
            .await
            .unwrap()
    );
    assert!(
        !remote
            .client
            .create_immutable(&segment, file.path())
            .await
            .unwrap()
    );
    assert_eq!(
        remote.files.lock().unwrap()[&format!("/dav/{segment}")],
        payload
    );

    // A server that answers 201 while storing other bytes is only caught by
    // reading the object back, so a successful reply is never proof.
    let written = b"second-segment".to_vec();
    let second = segment.replace("0000000003", "0000000004");
    std::fs::write(file.path(), &written).unwrap();
    *remote.next_write.lock().unwrap() = Some(WriteFault::Corrupt(b"torn".to_vec()));
    assert!(
        remote
            .client
            .create_immutable(&second, file.path())
            .await
            .unwrap()
    );
    let mut read_back = tempfile::NamedTempFile::new().unwrap();
    assert_eq!(
        remote
            .client
            .download(&second, &mut read_back)
            .await
            .unwrap()
            .sha256,
        digest(b"torn")
    );
    assert_ne!(digest(b"torn"), digest(&written));

    // A server that answers a conditional create with success even though the name
    // was taken is indistinguishable from an honest create by reply alone. Nothing in
    // the response saves a later reader; only the digest travelling in the name does,
    // which is why the caller reads the object back and re-checks it on every download.
    let taken = digest(&remote.files.lock().unwrap()[&format!("/dav/{segment}")]);
    std::fs::write(file.path(), b"rewritten under a taken name").unwrap();
    *remote.next_write.lock().unwrap() = Some(WriteFault::IgnoreConditions);
    assert!(
        remote
            .client
            .create_immutable(&segment, file.path())
            .await
            .unwrap()
    );
    let served = remote.files.lock().unwrap()[&format!("/dav/{segment}")].clone();
    assert_ne!(digest(&served), taken);
    assert_ne!(digest(&served), "a".repeat(64));

    let wrong = WebDavClient::for_test(
        remote.client.profile.clone(),
        "wrong",
        remote.server.client.clone(),
    );
    let count = remote.server.requests().len();
    assert_eq!(
        wrong
            .create_collection("vault.mdbx.sync/streams/device-2")
            .await,
        Err(GatewayError::WebDavUnauthorized)
    );
    assert_eq!(remote.server.requests().len(), count + 1);
}

#[test]
fn webdav_accepts_android_object_names_and_rejects_encoded_shapes() {
    for path in [
        "vault.mdbx.sync/streams/device-1/01HQXYZ/bootstrap/segments/0000000000-{}.mdbxsync"
            .replace("{}", &"f".repeat(64)),
        format!("vault.mdbx.sync/blobs/ab/cd/{}", "0".repeat(64)),
    ] {
        assert_eq!(normalize_path(&path).unwrap(), path);
    }
    for path in [
        "vault.mdbx.sync/blobs/%2e%2e",
        "vault.mdbx.sync/#frag",
        "vault.mdbx.sync/?q=1",
        "vault.mdbx.sync/../escape",
        "/absolute",
        "trailing/",
    ] {
        assert_eq!(
            normalize_path(path).unwrap_err(),
            GatewayError::InvalidWebDav
        );
    }
    assert_eq!(normalize_path("").unwrap(), "");
}
