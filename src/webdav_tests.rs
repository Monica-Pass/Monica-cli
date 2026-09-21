use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::GatewayError;
use crate::test_support::{FakeUpstream, Reply};
use crate::webdav::{WebDavClient, WebDavProfile, WriteCondition};

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
    pub next_write: Arc<Mutex<Option<WriteFault>>>,
}

pub(crate) enum WriteFault {
    Race(Vec<u8>),
    LoseResponse,
}

impl FakeWebDav {
    pub async fn new(files: BTreeMap<String, Vec<u8>>) -> Self {
        let files = Arc::new(Mutex::new(files));
        let shared = files.clone();
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
            match request.method.as_str() {
                "PROPFIND" => {
                    if path != "/dav/" && !files.keys().any(|key| key.starts_with(&path)) {
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
                    for (name, bytes) in files.iter().filter(|(key, _)| key.starts_with(&path)) {
                        let etag = etag(bytes).replace('"', "&quot;");
                        xml.push_str(&format!(r#"<d:response><d:href>{name}</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>{}</d:getcontentlength><d:getetag>{etag}</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"#, bytes.len()));
                    }
                    xml.push_str("</d:multistatus>");
                    response(207, xml.into_bytes(), None)
                }
                "GET" => match files.get(&path) {
                    Some(bytes) => response(200, bytes.clone(), Some(etag(bytes))),
                    None => response(404, Vec::new(), None),
                },
                "PUT" => {
                    let failure = fault.lock().unwrap().take();
                    let lost_response = matches!(failure, Some(WriteFault::LoseResponse));
                    if let Some(WriteFault::Race(bytes)) = failure { files.insert(path.clone(), bytes); }
                    let existing = files.get(&path);
                    let create = request.headers.get("if-none-match").is_some_and(|value| value == "*");
                    let replace = existing.is_some_and(|bytes| request.headers.get("if-match") == Some(&etag(bytes)));
                    if !(create && existing.is_none() || !create && replace) {
                        return response(412, Vec::new(), None);
                    }
                    files.insert(path, request.body.clone());
                    let mut reply = response(201, Vec::new(), Some(etag(&request.body)));
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
            next_write,
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
