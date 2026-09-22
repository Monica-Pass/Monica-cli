//! Human-owned WebDAV transport. Paths never come from MCP requests, and
//! authenticated requests cannot leave the configured HTTPS origin and folder.
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use reqwest::header::{self, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::config::{ConfigStore, private_file, read_json, write_json};
use crate::error::{GatewayError, Result};

pub const MAX_VAULT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LIST_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 1024;
const PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getetag/></d:prop></d:propfind>"#;

/// Only these non-secret preferences are persisted. The password belongs to
/// `crate::credstore`: this computer's credential manager, never a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebDavProfile {
    pub base_url: String,
    pub username: String,
}

impl WebDavProfile {
    pub fn new(base_url: &str, username: &str) -> Result<Self> {
        let value = Self {
            base_url: validate_base(base_url)?.to_string(),
            username: username.to_owned(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        let base = validate_base(&self.base_url)?;
        if base.as_str() != self.base_url
            || self.username.is_empty()
            || self.username.len() > 256
            || self.username.chars().any(|ch| ch.is_control() || ch == ':')
        {
            return Err(GatewayError::InvalidWebDav);
        }
        Ok(())
    }

    pub fn load(store: &ConfigStore) -> Result<Option<Self>> {
        let path = store.path.with_extension("webdav.json");
        if !path.exists() {
            return Ok(None);
        }
        let profile: Self = read_json(&path, 8192)?;
        profile.validate()?;
        Ok(Some(profile))
    }

    pub fn save(&self, store: &ConfigStore) -> Result<()> {
        self.validate()?;
        write_json(&store.path.with_extension("webdav.json"), self, true)
    }
}

pub fn validate_base(value: &str) -> Result<Url> {
    if value.len() > 2048
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(GatewayError::InvalidWebDav);
    }
    let mut url = Url::parse(value).map_err(|_| GatewayError::InvalidWebDav)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(GatewayError::InvalidWebDav);
    }
    let path = percent_encoding::percent_decode_str(url.path())
        .decode_utf8()
        .map_err(|_| GatewayError::InvalidWebDav)?;
    normalize_path(path.trim_matches('/'))?;
    if !url.path().ends_with('/') {
        url.path_segments_mut()
            .map_err(|_| GatewayError::InvalidWebDav)?
            .push("");
    }
    Ok(url)
}

pub fn normalize_path(path: &str) -> Result<String> {
    if path.len() > 2048
        || path.starts_with('/')
        || path.ends_with('/')
        || path
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '%' | '?' | '#' | ':'))
        || (!path.is_empty()
            && path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > 255))
    {
        return Err(GatewayError::InvalidWebDav);
    }
    Ok(path.to_owned())
}

pub fn strong_etag(value: &str) -> Option<String> {
    (value.len() >= 2
        && value.len() <= 256
        && value.starts_with('"')
        && value.ends_with('"')
        && value[1..value.len() - 1]
            .bytes()
            .all(|byte| byte >= 33 && byte != b'"' && byte != 127))
    .then(|| value.to_owned())
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RemoteEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
    pub etag: Option<String>,
}

impl RemoteEntry {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

pub struct Download {
    pub etag: Option<String>,
    pub sha256: String,
    pub size: u64,
}

pub enum WriteCondition<'a> {
    Create,
    Match(&'a str),
}

/// Deliberately has no Debug/Serialize implementation. Dropping the last client
/// clears the human password; each HTTP Authorization header is marked sensitive.
#[derive(Clone)]
pub struct WebDavClient {
    pub profile: WebDavProfile,
    password: Arc<Zeroizing<String>>,
    http: reqwest::Client,
}

impl WebDavClient {
    pub fn new(profile: WebDavProfile, password: Zeroizing<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .user_agent("MonicaPass-WebDAV/0.2")
            .build()
            .map_err(|_| GatewayError::WebDavUnavailable)?;
        Self::build(profile, password, http)
    }

    fn build(
        profile: WebDavProfile,
        password: Zeroizing<String>,
        http: reqwest::Client,
    ) -> Result<Self> {
        profile.validate()?;
        if password.is_empty() || password.len() > 4096 || password.chars().any(char::is_control) {
            return Err(GatewayError::InvalidWebDav);
        }
        crate::upstream::reject_secret_value(
            &json!([&profile.base_url, &profile.username]),
            &password,
        )
        .map_err(|_| GatewayError::SensitiveMetadata)?;
        Ok(Self {
            profile,
            password: Arc::new(password),
            http,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(profile: WebDavProfile, password: &str, http: reqwest::Client) -> Self {
        Self::build(profile, Zeroizing::new(password.to_owned()), http).unwrap()
    }

    fn url(&self, path: &str, directory: bool) -> Result<Url> {
        let path = normalize_path(path)?;
        crate::upstream::reject_secret_value(&json!(&path), &self.password)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        let mut url = validate_base(&self.profile.base_url)?;
        if !path.is_empty() {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| GatewayError::InvalidWebDav)?;
            segments.pop_if_empty().extend(path.split('/'));
            if directory {
                segments.push("");
            }
        }
        Ok(url)
    }

    fn request(&self, method: reqwest::Method, url: Url) -> Result<reqwest::RequestBuilder> {
        let raw = Zeroizing::new(format!(
            "{}:{}",
            self.profile.username,
            self.password.as_str()
        ));
        let encoded =
            Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(raw.as_bytes()));
        let value = Zeroizing::new(format!("Basic {}", encoded.as_str()));
        let mut authorization =
            HeaderValue::from_str(&value).map_err(|_| GatewayError::InvalidWebDav)?;
        authorization.set_sensitive(true);
        Ok(self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, authorization))
    }

    /// A successful depth-one PROPFIND verifies both authentication and the folder.
    pub async fn list(&self, path: &str) -> Result<Vec<RemoteEntry>> {
        let path = normalize_path(path)?;
        let response = self
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                self.url(&path, true)?,
            )?
            .header("Depth", "1")
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(PROPFIND)
            .send()
            .await
            .map_err(|_| GatewayError::WebDavUnavailable)?;
        check_status(response.status())?;
        let mut bytes = Vec::new();
        if response
            .content_length()
            .is_some_and(|length| length > MAX_LIST_BYTES as u64)
        {
            return Err(GatewayError::ResponseTooLarge);
        }
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| GatewayError::WebDavUnavailable)?;
            if bytes.len() + chunk.len() > MAX_LIST_BYTES {
                return Err(GatewayError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let entries = parse_listing(&bytes, &validate_base(&self.profile.base_url)?, &path)?;
        crate::upstream::reject_secret_value(&json!(&entries), &self.password)?;
        Ok(entries)
    }

    /// Streams only encrypted bytes to a private temporary file owned by the caller.
    pub async fn download(
        &self,
        path: &str,
        target: &mut tempfile::NamedTempFile,
    ) -> Result<Download> {
        if path.is_empty() {
            return Err(GatewayError::InvalidWebDav);
        }
        private_file(target.path())?;
        let response = self
            .request(reqwest::Method::GET, self.url(path, false)?)?
            .send()
            .await
            .map_err(|_| GatewayError::WebDavUnavailable)?;
        check_status(response.status())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(GatewayError::InvalidWebDavResponse);
        }
        let declared = response.content_length();
        if declared.is_some_and(|length| length > MAX_VAULT_BYTES) {
            return Err(GatewayError::ResponseTooLarge);
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .and_then(strong_etag);
        crate::upstream::reject_secret_value(&json!(&etag), &self.password)?;
        let mut size = 0;
        let mut hash = Sha256::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| GatewayError::WebDavUnavailable)?;
            size += chunk.len() as u64;
            if size > MAX_VAULT_BYTES {
                return Err(GatewayError::ResponseTooLarge);
            }
            hash.update(&chunk);
            target
                .write_all(&chunk)
                .map_err(|_| GatewayError::StateUnavailable)?;
        }
        if size == 0 || declared.is_some_and(|length| length != size) {
            return Err(GatewayError::InvalidWebDavResponse);
        }
        target
            .as_file()
            .sync_all()
            .map_err(|_| GatewayError::StateUnavailable)?;
        Ok(Download {
            etag,
            sha256: hex::encode(hash.finalize()),
            size,
        })
    }

    /// There is intentionally no unconditional replacement operation.
    pub async fn upload(
        &self,
        path: &str,
        source: &Path,
        condition: WriteCondition<'_>,
    ) -> Result<()> {
        let status = self.put(path, source, condition).await?;
        check_status(status)?;
        if !matches!(status.as_u16(), 200 | 201 | 204) {
            return Err(GatewayError::SyncOutcomeUnknown);
        }
        Ok(())
    }

    /// Creates one immutable, content-addressed object. `Ok(false)` means the server already
    /// holds that name, which is only benign once the caller has compared the stored bytes.
    pub async fn create_immutable(&self, path: &str, source: &Path) -> Result<bool> {
        let status = self.put(path, source, WriteCondition::Create).await?;
        match status.as_u16() {
            200 | 201 | 204 => Ok(true),
            412 => Ok(false),
            _ => {
                check_status(status)?;
                Err(GatewayError::SyncOutcomeUnknown)
            }
        }
    }

    async fn put(
        &self,
        path: &str,
        source: &Path,
        condition: WriteCondition<'_>,
    ) -> Result<reqwest::StatusCode> {
        if path.is_empty() {
            return Err(GatewayError::InvalidWebDav);
        }
        let file = tokio::fs::File::open(source)
            .await
            .map_err(|_| GatewayError::StateUnavailable)?;
        let size = file
            .metadata()
            .await
            .map_err(|_| GatewayError::StateUnavailable)?
            .len();
        if size == 0 || size > MAX_VAULT_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        let mut request = self
            .request(reqwest::Method::PUT, self.url(path, false)?)?
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CONTENT_LENGTH, size);
        request = match condition {
            WriteCondition::Create => request.header(header::IF_NONE_MATCH, "*"),
            WriteCondition::Match(etag) => request.header(
                header::IF_MATCH,
                strong_etag(etag).ok_or(GatewayError::RemoteVersionRequired)?,
            ),
        };
        let response = request
            .body(reqwest::Body::wrap_stream(
                tokio_util::io::ReaderStream::new(file),
            ))
            .send()
            .await
            .map_err(|_| GatewayError::SyncOutcomeUnknown)?;
        if response.status().is_server_error() {
            return Err(GatewayError::SyncOutcomeUnknown);
        }
        Ok(response.status())
    }

    /// Creates a collection. Servers reject a repeated MKCOL with 405, 409, 412 or 501 rather
    /// than agreeing on one answer, so a failed reply is confirmed with a Depth:1 PROPFIND.
    pub async fn create_collection(&self, path: &str) -> Result<()> {
        let path = normalize_path(path)?;
        if path.is_empty() {
            return Err(GatewayError::InvalidWebDav);
        }
        let response = self
            .request(
                reqwest::Method::from_bytes(b"MKCOL").unwrap(),
                self.url(&path, true)?,
            )?
            .send()
            .await
            .map_err(|_| GatewayError::WebDavUnavailable)?;
        let status = response.status();
        drop(response);
        if status.is_success() {
            return Ok(());
        }
        let error = match check_status(status) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if error == GatewayError::WebDavUnauthorized {
            return Err(error);
        }
        if self.list(&path).await.is_ok() {
            return Ok(());
        }
        Err(error)
    }
}

fn check_status(status: reqwest::StatusCode) -> Result<()> {
    match status.as_u16() {
        200..=299 => Ok(()),
        300..=399 => Err(GatewayError::RedirectBlocked),
        401 | 403 => Err(GatewayError::WebDavUnauthorized),
        404 => Err(GatewayError::RemoteNotFound),
        409 | 412 => Err(GatewayError::SyncConflict),
        _ => Err(GatewayError::WebDavUnavailable),
    }
}

fn parse_listing(bytes: &[u8], base: &Url, directory: &str) -> Result<Vec<RemoteEntry>> {
    let xml = std::str::from_utf8(bytes).map_err(|_| GatewayError::InvalidWebDavResponse)?;
    let document = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 30_000,
            ..Default::default()
        },
    )
    .map_err(|_| GatewayError::InvalidWebDavResponse)?;
    let root = document.root_element();
    if !root.has_tag_name(("DAV:", "multistatus")) {
        return Err(GatewayError::InvalidWebDavResponse);
    }
    let mut entries = std::collections::BTreeMap::new();
    let mut folder_found = false;
    for response in root
        .children()
        .filter(|node| node.has_tag_name(("DAV:", "response")))
    {
        let Some(href) = response
            .children()
            .find(|node| node.has_tag_name(("DAV:", "href")))
            .and_then(|node| node.text())
        else {
            continue;
        };
        let Some(path) = href_path(base, href) else {
            continue;
        };
        let properties: Vec<_> = response
            .children()
            .filter(|node| node.has_tag_name(("DAV:", "propstat")))
            .filter(|node| {
                node.children()
                    .find(|child| child.has_tag_name(("DAV:", "status")))
                    .and_then(|node| node.text())
                    .and_then(|text| text.split_whitespace().nth(1))
                    == Some("200")
            })
            .filter_map(|node| {
                node.children()
                    .find(|child| child.has_tag_name(("DAV:", "prop")))
            })
            .collect();
        if properties.is_empty() {
            continue;
        }
        let is_directory = properties
            .iter()
            .flat_map(|property| property.children())
            .find(|node| node.has_tag_name(("DAV:", "resourcetype")))
            .is_some_and(|node| {
                node.children()
                    .any(|child| child.has_tag_name(("DAV:", "collection")))
            });
        if path == directory {
            folder_found |= is_directory;
            continue;
        }
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        if parent != directory {
            continue;
        }
        let size = properties
            .iter()
            .flat_map(|property| property.children())
            .find(|node| node.has_tag_name(("DAV:", "getcontentlength")))
            .and_then(|node| node.text())
            .and_then(|text| text.parse().ok());
        let etag = properties
            .iter()
            .flat_map(|property| property.children())
            .find(|node| node.has_tag_name(("DAV:", "getetag")))
            .and_then(|node| node.text())
            .and_then(strong_etag);
        entries.insert(
            path.clone(),
            RemoteEntry {
                path,
                is_directory,
                size,
                etag,
            },
        );
        if entries.len() > MAX_ENTRIES {
            return Err(GatewayError::ResponseTooLarge);
        }
    }
    if !folder_found {
        return Err(GatewayError::InvalidWebDavResponse);
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

fn href_path(base: &Url, href: &str) -> Option<String> {
    if href.chars().any(|ch| ch.is_control() || ch == '\\') {
        return None;
    }
    let target = base.join(href).ok()?;
    if target.origin() != base.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || target.query().is_some()
        || target.fragment().is_some()
    {
        return None;
    }
    if target.path().trim_end_matches('/') == base.path().trim_end_matches('/') {
        return Some(String::new());
    }
    let encoded = target
        .path()
        .strip_prefix(base.path())?
        .trim_end_matches('/');
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .ok()?;
    normalize_path(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webdav_listing_accepts_collection_hrefs_without_slashes_and_split_properties() {
        let xml = br#"<multistatus xmlns="DAV:">
          <response><href>/dav</href><propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
          <response><href>/dav/vault.mdbx</href>
            <propstat><prop><resourcetype/></prop><status>HTTP/1.1 200 OK</status></propstat>
            <propstat><prop><getetag>&quot;revision&quot;</getetag><getcontentlength>2048</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat>
          </response>
        </multistatus>"#;
        let entries = parse_listing(
            xml,
            &validate_base("https://example.test/dav/").unwrap(),
            "",
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].etag.as_deref(), Some("\"revision\""));
        assert_eq!(entries[0].size, Some(2048));
    }

    #[test]
    fn webdav_paths_and_xml_cannot_escape_the_configured_folder() {
        let base = validate_base("https://example.test/dav").unwrap();
        assert_eq!(base.as_str(), "https://example.test/dav/");
        for value in [
            "http://example.test/",
            "https://u:p@example.test/",
            "https://example.test/?token=x",
        ] {
            assert!(validate_base(value).is_err());
        }
        for value in [
            "../other",
            "/outside",
            "folder/../other",
            "%2e%2e/secret",
            "folder\\other",
            "folder\u{001b}other",
        ] {
            assert!(normalize_path(value).is_err());
        }
        for href in [
            "https://other.test/dav/vault.mdbx",
            "/outside/vault.mdbx",
            "/dav/%252e%252e/vault.mdbx",
            "/dav/%2e%2e/vault.mdbx",
        ] {
            assert!(href_path(&base, href).is_none());
        }
        assert_eq!(
            href_path(&base, "/dav/%E4%BF%9D%E9%99%A9%E5%BA%93%20one.mdbx").as_deref(),
            Some("保险库 one.mdbx")
        );
        assert!(strong_etag("W/\"weak\"").is_none());
        assert!(strong_etag("\"good\"\r\n").is_none());
        assert!(parse_listing(b"<!DOCTYPE x [<!ENTITY x 'bad'>]><x/>", &base, "").is_err());
    }
}
