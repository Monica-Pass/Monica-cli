//! Service-wide, explicitly authorized API proxy. No arbitrary origin or credentials.
use crate::{
    error::{GatewayError, Result},
    model::Operation,
};
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiSurface {
    #[default]
    Rest,
    Graphql,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiArgs {
    /// REST by default. GraphQL uses the provider's fixed GraphQL endpoint and requires api_write.
    #[serde(default)]
    pub api: ApiSurface,
    /// Must be *: this tool requires a service-wide grant, never an issue-only grant.
    pub repository: String,
    /// GET, HEAD or OPTIONS for api_read; POST, PUT, PATCH or DELETE for api_write.
    pub method: String,
    /// Relative API path, e.g. projects/123/merge_requests. No host, query or fragment.
    pub path: String,
    /// Query pairs; repeated keys are supported.
    #[serde(default)]
    pub query: Vec<(String, String)>,
    /// Additional service headers. Authentication and transport headers are forbidden.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// JSON request body; mutually exclusive with body_base64.
    #[serde(default)]
    pub body: Option<Value>,
    /// Base64 bytes for non-JSON requests; specify Content-Type in headers.
    #[serde(default)]
    pub body_base64: Option<String>,
    /// Required UUID for every write. Reuse for the same intended request only.
    #[serde(default)]
    pub request_id: Option<String>,
}
impl ApiArgs {
    pub fn parse(operation: Operation, value: Value) -> Result<Self> {
        let mut args: Self =
            serde_json::from_value(value).map_err(|_| GatewayError::InvalidRequest)?;
        args.method.make_ascii_uppercase();
        let write = matches!(args.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE");
        if args.api == ApiSurface::Graphql && (args.method != "POST" || !args.path.is_empty()) {
            return Err(GatewayError::InvalidRequest);
        }
        if args.repository != "*"
            || operation.is_write() != write
            || (!write && !matches!(args.method.as_str(), "GET" | "HEAD" | "OPTIONS"))
            || args.path.len() > 4096
            || args.query.len() > 128
            || args.headers.len() > 32
            || (args.body.is_some() && args.body_base64.is_some())
            || (!write
                && (args.body.is_some() || args.body_base64.is_some() || args.request_id.is_some()))
        {
            return Err(GatewayError::InvalidRequest);
        }
        if write {
            args.request_id = Some(
                uuid::Uuid::parse_str(
                    args.request_id
                        .as_deref()
                        .ok_or(GatewayError::InvalidRequest)?,
                )
                .map_err(|_| GatewayError::InvalidRequest)?
                .to_string(),
            );
        }
        // Validate independently of the actual configured host, before credential access.
        args.url("https://service.example.test/api/v4/")?;
        args.extra_headers()?;
        if let Some(body) = &args.body_base64 {
            base64::engine::general_purpose::STANDARD
                .decode(body)
                .map_err(|_| GatewayError::InvalidRequest)?;
        }
        if serde_json::to_vec(&args)
            .map_err(|_| GatewayError::InvalidRequest)?
            .len()
            > 192 * 1024
        {
            return Err(GatewayError::ResponseTooLarge);
        }
        Ok(args)
    }

    pub(crate) fn url(&self, base: &str) -> Result<Url> {
        // Reject traversal, including nested %-encoding. Encoded slashes in project IDs
        // remain useful, but must not conceal a dot segment, backslash or control byte.
        let mut decoded = self.path.clone();
        for _ in 0..8 {
            if decoded.starts_with('/')
                || decoded.contains(['\\', '?', '#', ':'])
                || decoded.chars().any(char::is_control)
                || decoded.split('/').any(|p| p == "." || p == "..")
            {
                return Err(GatewayError::InvalidRequest);
            }
            if !decoded.contains('%') {
                break;
            }
            let bytes = decoded.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    let pair = bytes
                        .get(i + 1..i + 3)
                        .ok_or(GatewayError::InvalidRequest)?;
                    let pair =
                        std::str::from_utf8(pair).map_err(|_| GatewayError::InvalidRequest)?;
                    out.push(
                        u8::from_str_radix(pair, 16).map_err(|_| GatewayError::InvalidRequest)?,
                    );
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            decoded = String::from_utf8(out).map_err(|_| GatewayError::InvalidRequest)?;
        }
        if decoded.contains('%')
            || decoded.starts_with('/')
            || decoded.contains(['\\', '?', '#', ':'])
            || decoded.chars().any(char::is_control)
            || decoded.split('/').any(|p| p == "." || p == "..")
        {
            return Err(GatewayError::InvalidRequest);
        }
        let base = Url::parse(base).map_err(|_| GatewayError::InvalidConfig)?;
        if self.api == ApiSurface::Graphql {
            let mut url = base.clone();
            url.set_path(if base.path() == "/" {
                "/graphql"
            } else {
                "/api/graphql"
            });
            if !self.query.is_empty() {
                url.query_pairs_mut()
                    .extend_pairs(self.query.iter().map(|(k, v)| (k, v)));
            }
            return Ok(url);
        }
        let mut url = base
            .join(&self.path)
            .map_err(|_| GatewayError::InvalidRequest)?;
        if url.origin() != base.origin()
            || !url.path().starts_with(base.path())
            || url.fragment().is_some()
        {
            return Err(GatewayError::InvalidRequest);
        }
        if !self.query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(self.query.iter().map(|(k, v)| (k, v)));
        }
        Ok(url)
    }

    pub(crate) fn extra_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (key, value) in &self.headers {
            let name =
                HeaderName::from_bytes(key.as_bytes()).map_err(|_| GatewayError::InvalidRequest)?;
            let lower = name.as_str();
            if matches!(
                lower,
                "authorization"
                    | "private-token"
                    | "job-token"
                    | "deploy-token"
                    | "cookie"
                    | "host"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "upgrade"
                    | "te"
                    | "trailer"
                    | "expect"
                    | "forwarded"
            ) || lower.starts_with("proxy-")
                || lower.starts_with("x-forwarded-")
                || lower.starts_with("x-http-method")
                || lower == "x-method-override"
                || lower.starts_with("x-original-")
                || lower.starts_with("x-rewrite-")
            {
                return Err(GatewayError::InvalidRequest);
            }
            headers.insert(
                name,
                HeaderValue::from_str(value).map_err(|_| GatewayError::InvalidRequest)?,
            );
        }
        Ok(headers)
    }
}

pub(crate) async fn response(request: reqwest::RequestBuilder, token: &str) -> Result<Value> {
    let mut response = request
        .send()
        .await
        .map_err(|_| GatewayError::UpstreamUnavailable)?;
    if response.status().is_redirection() {
        return Err(GatewayError::RedirectBlocked);
    }
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|n| n > crate::upstream::MAX_RESPONSE_BYTES as u64)
    {
        return Err(GatewayError::ResponseTooLarge);
    }
    let mut headers = BTreeMap::new();
    for name in [
        "content-type",
        "x-page",
        "x-next-page",
        "x-prev-page",
        "x-per-page",
        "x-total",
        "x-total-pages",
        "retry-after",
        "etag",
        "link",
        "location",
        "last-modified",
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ] {
        if let Some(value) = response.headers().get(name) {
            headers.insert(
                name,
                value
                    .to_str()
                    .map_err(|_| GatewayError::ResponseBlocked)?
                    .to_owned(),
            );
        }
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| GatewayError::UpstreamUnavailable)?
    {
        if bytes.len() + chunk.len() > crate::upstream::MAX_RESPONSE_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    // Scan text before encoding binary responses, so encoding cannot hide the token.
    crate::upstream::reject_secret_value(&json!(String::from_utf8_lossy(&bytes)), token)?;
    let mut result = json!({"status":status, "headers":headers});
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(body) => result["body"] = body,
        Err(_) => {
            result["body_base64"] = json!(base64::engine::general_purpose::STANDARD.encode(&*bytes))
        }
    }
    crate::upstream::reject_secret_value(&result, token)?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::Provider,
        test_support::{Fixture, Reply, TOKEN},
    };

    #[test]
    fn api_scope_methods_headers_and_encoded_traversal_are_validated() {
        let read = |path: &str| {
            ApiArgs::parse(
                Operation::ApiRead,
                json!({"repository":"*","method":"GET","path":path}),
            )
        };
        for path in [
            "https://outside.test/",
            "//outside.test",
            "/user",
            "../user",
            "%2e%2e/user",
            "%252e%252e/user",
            "projects/a%2F..%2F..%2Fuser",
            "user?x=1",
            "user#x",
            "a\\b",
            "%0auser",
        ] {
            assert!(read(path).is_err(), "accepted {path}");
        }
        let args = read("projects/team%2Frepo/merge_requests").unwrap();
        assert_eq!(
            args.url("https://gitlab.example.test/api/v4/")
                .unwrap()
                .as_str(),
            "https://gitlab.example.test/api/v4/projects/team%2Frepo/merge_requests"
        );
        for method in ["POST", "DELETE", "TRACE", "CONNECT"] {
            assert!(
                ApiArgs::parse(
                    Operation::ApiRead,
                    json!({"repository":"*","method":method,"path":"user"})
                )
                .is_err()
            );
        }
        for header in [
            "Authorization",
            "PRIVATE-TOKEN",
            "Host",
            "Cookie",
            "X-HTTP-Method-Override",
            "X-Original-URL",
            "Proxy-Authorization",
        ] {
            assert!(
                ApiArgs::parse(
                    Operation::ApiRead,
                    json!({"repository":"*","method":"GET","path":"user","headers":{header:"bad"}})
                )
                .is_err()
            );
        }
        assert!(
            ApiArgs::parse(
                Operation::ApiWrite,
                json!({"repository":"*","method":"POST","path":"user"})
            )
            .is_err()
        );
        assert!(
            crate::model::validate_grant_scope(
                &["org/repo".into()],
                &[Operation::ApiWrite],
                Provider::Gitlab
            )
            .is_err()
        );
        assert!(
            crate::model::validate_grant_scope(
                &["*".into()],
                &[Operation::ListIssues],
                Provider::Gitlab
            )
            .is_err()
        );
        assert!(
            crate::model::validate_grant_scope(
                &["*".into()],
                &[Operation::ApiRead, Operation::ApiWrite],
                Provider::Gitlab
            )
            .is_ok()
        );
    }

    #[test]
    fn graphql_uses_fixed_provider_endpoint_and_write_authorization() {
        let value = json!({"api":"graphql","repository":"*","method":"POST","path":"", "body":{"query":"query { currentUser { username } }"},"request_id":uuid::Uuid::new_v4()});
        assert!(ApiArgs::parse(Operation::ApiRead, value.clone()).is_err());
        let args = ApiArgs::parse(Operation::ApiWrite, value).unwrap();
        assert_eq!(
            args.url("https://gitlab.example.test/api/v4/")
                .unwrap()
                .as_str(),
            "https://gitlab.example.test/api/graphql"
        );
        assert_eq!(
            args.url("https://api.github.com/").unwrap().as_str(),
            "https://api.github.com/graphql"
        );
        assert_eq!(
            args.url("https://github.example.test/api/v3/")
                .unwrap()
                .as_str(),
            "https://github.example.test/api/graphql"
        );
    }

    #[tokio::test]
    async fn api_creates_branch_single_commit_and_mr_and_reads_feedback() {
        let fixture = Fixture::new(
            Provider::Gitlab,
            &[Operation::ApiRead, Operation::ApiWrite],
            vec![
                Reply::json(json!({"name":"release-311"})),
                Reply::json(json!({"id":"commit-one"})),
                Reply::json(json!({"iid":311})),
                Reply::json(json!([{"body":"Please fix metadata"}])),
            ],
        )
        .await;
        let writes = [
            (
                "projects/123/repository/branches",
                json!({"branch":"release-311","ref":"master"}),
            ),
            (
                "projects/123/repository/commits",
                json!({"branch":"release-311","commit_message":"Update release metadata","actions":[{"action":"update","file_path":"metadata/app.yml","content":"synthetic metadata"},{"action":"create","file_path":"notes.txt","content":"synthetic notes"}]}),
            ),
            (
                "projects/123/merge_requests",
                json!({"source_branch":"release-311","target_branch":"master","title":"Release 311","squash":true}),
            ),
        ];
        for (path, body) in writes {
            let call = fixture.call(
                Operation::ApiWrite,
                json!({"method":"POST","path":path,"body":body,"request_id":uuid::Uuid::new_v4()}),
            );
            let result = fixture
                .gateway
                .call(&fixture.capability, call.clone())
                .await
                .unwrap();
            assert_eq!(result["status"], 200);
            assert!(!result.to_string().contains(TOKEN));
            fixture
                .gateway
                .call(&fixture.capability, call)
                .await
                .unwrap();
        }
        let result = fixture.gateway.call(&fixture.capability, fixture.call(Operation::ApiRead, json!({"method":"GET","path":"projects/123/merge_requests/311/notes","query":[["page","2"],["per_page","20"]]}))).await.unwrap();
        assert_eq!(result["body"][0]["body"], "Please fix metadata");
        let requests = fixture.upstream.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[0].target,
            "/api/v4/projects/123/repository/branches"
        );
        assert_eq!(requests[1].method, "POST");
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[1].body).unwrap()["actions"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(requests[3].target.ends_with("?page=2&per_page=20"));
        for request in &requests {
            assert_eq!(request.headers["private-token"], TOKEN);
        }
        fixture.vault.lock().unwrap();
        assert!(
            fixture
                .gateway
                .call(
                    &fixture.capability,
                    fixture.call(Operation::ApiRead, json!({"method":"GET","path":"user"}))
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.upstream.requests().len(), 4);
    }

    #[tokio::test]
    async fn api_does_not_elevate_old_grants_or_retry_uncertain_writes() {
        let old = Fixture::new(Provider::Gitlab, &[Operation::ListIssues], vec![]).await;
        assert_eq!(
            old.gateway
                .call(
                    &old.capability,
                    old.call(Operation::ApiRead, json!({"method":"GET","path":"user"}))
                )
                .await,
            Err(GatewayError::PermissionDenied)
        );
        assert!(old.upstream.requests().is_empty());
        let fixture = Fixture::new(
            Provider::Gitlab,
            &[Operation::ApiWrite],
            vec![Reply::disconnected()],
        )
        .await;
        let call=fixture.call(Operation::ApiWrite,json!({"method":"DELETE","path":"projects/123/repository/branches/obsolete","request_id":uuid::Uuid::new_v4()}));
        for _ in 0..2 {
            assert_eq!(
                fixture
                    .gateway
                    .call(&fixture.capability, call.clone())
                    .await,
                Err(GatewayError::WriteOutcomeUnknown)
            );
        }
        assert_eq!(fixture.upstream.requests().len(), 1);
    }

    #[tokio::test]
    async fn api_binary_payload_custom_accept_and_receipt_privacy() {
        let private_response = "synthetic-private-new-resource";
        let mut reply = Reply::json(Value::Null);
        reply.body = private_response.as_bytes().to_vec();
        reply
            .headers
            .push(("content-type".into(), "application/octet-stream".into()));
        let fixture = Fixture::new(Provider::Gitlab, &[Operation::ApiWrite], vec![reply]).await;
        let call = fixture.call(Operation::ApiWrite, json!({
            "method":"PUT", "path":"projects/123/repository/files/file.txt",
            "headers":{"Accept":"application/octet-stream", "Content-Type":"text/plain"},
            "body_base64":base64::engine::general_purpose::STANDARD.encode("synthetic file body"),
            "request_id":uuid::Uuid::new_v4()
        }));
        let first = fixture
            .gateway
            .call(&fixture.capability, call.clone())
            .await
            .unwrap();
        assert_eq!(
            first["body_base64"],
            base64::engine::general_purpose::STANDARD.encode(private_response)
        );
        let repeated = fixture
            .gateway
            .call(&fixture.capability, call)
            .await
            .unwrap();
        assert_eq!(repeated["replayed"], true);
        assert!(repeated.get("body_base64").is_none());
        let journal = std::fs::read_to_string(fixture.store.journal_path()).unwrap();
        assert!(!journal.contains(private_response));
        assert!(!journal.contains("synthetic file body"));
        assert!(!journal.contains(first["body_base64"].as_str().unwrap()));
        let requests = fixture.upstream.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body, b"synthetic file body");
        assert_eq!(requests[0].headers["accept"], "application/octet-stream");
    }

    #[tokio::test]
    async fn api_preserves_service_errors_and_blocks_reflected_credentials() {
        let mut rejected = Reply::json(json!({"message":"branch exists"}));
        rejected.status = 400;
        let fixture = Fixture::new(
            Provider::Github,
            &[Operation::ApiRead],
            vec![rejected, Reply::json(json!({"secret":TOKEN}))],
        )
        .await;
        let call = fixture.call(Operation::ApiRead, json!({"method":"GET","path":"user"}));
        let result = fixture
            .gateway
            .call(&fixture.capability, call.clone())
            .await
            .unwrap();
        assert_eq!(result["status"], 400);
        assert_eq!(result["body"]["message"], "branch exists");
        assert_eq!(
            fixture.gateway.call(&fixture.capability, call).await,
            Err(GatewayError::ResponseBlocked)
        );
        assert_eq!(
            fixture.upstream.requests()[0].headers["authorization"],
            format!("Bearer {TOKEN}")
        );
    }
}
