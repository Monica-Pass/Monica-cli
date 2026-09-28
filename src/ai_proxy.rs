//! OpenAI/Anthropic loopback wire endpoints. No management or upstream keys in responses.
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::api_keys::ApiProtocol;
use crate::config::{ConfigStore, Connection};
use crate::error::{GatewayError, Result};
use crate::gateway::Gateway;
use crate::model::Operation;

pub(crate) const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct ProxyState {
    gateway: Arc<Gateway>,
    host: String,
}

pub(crate) fn router(gateway: Arc<Gateway>, address: std::net::SocketAddrV4) -> Router {
    Router::new()
        .route("/v1/models", any(handle))
        .route("/v1/responses", any(handle))
        .route("/v1/chat/completions", any(handle))
        .route("/v1/messages", any(handle))
        .route("/v1/messages/count_tokens", any(handle))
        .with_state(ProxyState {
            gateway,
            host: address.to_string(),
        })
}

fn local_capability(headers: &HeaderMap) -> Result<Zeroizing<String>> {
    let bearer = headers.get_all("authorization").iter().collect::<Vec<_>>();
    let key = headers.get_all("x-api-key").iter().collect::<Vec<_>>();
    let value = match (bearer.as_slice(), key.as_slice()) {
        ([value], []) => value.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")),
        ([], [value]) => value.to_str().ok(),
        _ => None,
    }
    .and_then(|s| s.strip_prefix("monica-"))
    .ok_or(GatewayError::Unauthorized)?;
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(GatewayError::Unauthorized);
    }
    Ok(Zeroizing::new(value.to_owned()))
}

async fn handle(State(state): State<ProxyState>, request: Request) -> Response {
    let result = async {
        if request.headers().contains_key("origin")
            || request.headers().get_all("host").iter().count() != 1
            || request.headers().get("host").and_then(|v| v.to_str().ok())
                != Some(state.host.as_str())
        {
            return Err(GatewayError::PermissionDenied);
        }
        let capability = local_capability(request.headers())?;
        // Authenticate and select the protocol before accepting a potentially large body.
        let (_, binding) = state.gateway.access(&capability)?;
        let source = binding
            .api_key
            .as_ref()
            .ok_or(GatewayError::PermissionDenied)?;
        let permit = state.gateway.proxy_slot()?;
        let (parts, body) = request.into_parts();
        let path = route(source.protocol, &parts.method, parts.uri.path())?;
        let query = match parts.uri.query() {
            None => None,
            Some("beta=true") if source.protocol == ApiProtocol::Anthropic => Some("beta=true"),
            Some(_) => return Err(GatewayError::InvalidRequest),
        };
        let bytes =
            tokio::time::timeout(Duration::from_secs(30), to_bytes(body, MAX_REQUEST_BYTES))
                .await
                .map_err(|_| GatewayError::InvalidRequest)?
                .map_err(|_| GatewayError::ResponseTooLarge)?;
        let body: Option<Value> = if parts.method == Method::GET {
            if !bytes.is_empty() {
                return Err(GatewayError::InvalidRequest);
            }
            None
        } else {
            if parts.headers.get_all("content-type").iter().count() != 1
                || !parts
                    .headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.split(';').next() == Some("application/json"))
            {
                return Err(GatewayError::InvalidRequest);
            }
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| GatewayError::InvalidRequest)?;
            if !value.is_object()
                || value
                    .get("model")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s.is_empty() || s.len() > 256)
                || value.get("stream").is_some_and(|s| !s.is_boolean())
                || value
                    .get("background")
                    .is_some_and(|s| s != &Value::Bool(false))
            {
                return Err(GatewayError::InvalidRequest);
            }
            Some(value)
        };
        let mut headers = HeaderMap::new();
        // Local credentials, Host, cookies, transport headers and arbitrary routing
        // headers never reach the upstream. SDK version/beta headers may pass.
        for name in ["anthropic-version", "anthropic-beta", "openai-beta"] {
            if parts.headers.get_all(name).iter().count() > 1 {
                return Err(GatewayError::InvalidRequest);
            }
            if let Some(value) = parts.headers.get(name) {
                if value.as_bytes().len() > 2048 {
                    return Err(GatewayError::InvalidRequest);
                }
                headers.insert(name, value.clone());
            }
        }
        let call = ProxyRequest {
            protocol: source.protocol,
            method: parts.method,
            path,
            query,
            headers,
            parsed: body,
        };
        state.gateway.proxy_request(capability, call, permit).await
    }
    .await;
    result.unwrap_or_else(failure)
}

pub(crate) fn route(protocol: ApiProtocol, method: &Method, path: &str) -> Result<&'static str> {
    match (protocol, method.as_str(), path) {
        (_, "GET", "/v1/models") => Ok("models"),
        (ApiProtocol::Openai, "POST", "/v1/responses") => Ok("responses"),
        (ApiProtocol::Openai, "POST", "/v1/chat/completions") => Ok("chat/completions"),
        (ApiProtocol::Anthropic, "POST", "/v1/messages") => Ok("messages"),
        (ApiProtocol::Anthropic, "POST", "/v1/messages/count_tokens") => {
            Ok("messages/count_tokens")
        }
        _ => Err(GatewayError::PermissionDenied),
    }
}

pub(crate) struct ProxyRequest {
    pub protocol: ApiProtocol,
    pub method: Method,
    pub path: &'static str,
    pub query: Option<&'static str>,
    pub headers: HeaderMap,
    pub parsed: Option<Value>,
}
impl ProxyRequest {
    pub fn operation(&self) -> Operation {
        if self.method == Method::GET {
            Operation::ModelList
        } else {
            Operation::ModelInvoke
        }
    }
}

pub(crate) fn failure(error: GatewayError) -> Response {
    let (status, kind) = match error {
        GatewayError::Unauthorized | GatewayError::ReauthorizationRequired => {
            (StatusCode::UNAUTHORIZED, "authentication_error")
        }
        GatewayError::PermissionDenied
        | GatewayError::UnlockRequired
        | GatewayError::ApprovalDenied => (StatusCode::FORBIDDEN, "permission_error"),
        GatewayError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error"),
        GatewayError::BrokerBusy | GatewayError::ApprovalTimeout => {
            (StatusCode::SERVICE_UNAVAILABLE, "overloaded_error")
        }
        GatewayError::InvalidRequest | GatewayError::InvalidConfig => {
            (StatusCode::BAD_REQUEST, "invalid_request_error")
        }
        GatewayError::ObjectChanged | GatewayError::ObjectReadOnly | GatewayError::NotFound => {
            (StatusCode::CONFLICT, "invalid_request_error")
        }
        GatewayError::ResponseTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error"),
        _ => (StatusCode::BAD_GATEWAY, "api_error"),
    };
    let mut response = (
        status,
        Json(
            json!({"type":"error", "error":{"type":kind,"code":error,"message":error.to_string()}}),
        ),
    )
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}

/// Write a client-ready local key to a private file, never stdout or client config.
pub fn write_client_config(
    store: &ConfigStore,
    name: &str,
    output: &std::path::Path,
    force: bool,
) -> Result<Value> {
    let client_path = crate::admin::grant_client(store, name)?;
    let client: crate::config::ClientConfig = crate::config::read_json(&client_path, 16 * 1024)?;
    client.validate()?;
    let config = store.load()?;
    let grant = config.authenticate(&client.capability, chrono::Utc::now().timestamp())?;
    if grant.name != name
        || !grant.operations.contains(&Operation::ModelInvoke)
        || !grant.repositories.contains("*")
    {
        return Err(GatewayError::PermissionDenied);
    }
    let binding = config
        .connections
        .get(&grant.connection)
        .ok_or(GatewayError::NotFound)?;
    if grant.connection_fingerprint != crate::config::connection_fingerprint(binding) {
        return Err(GatewayError::Unauthorized);
    }
    let source = binding
        .api_key
        .as_ref()
        .ok_or(GatewayError::InvalidRequest)?;
    let root = format!("http://{}", config.listen);
    let base = match source.protocol {
        ApiProtocol::Openai => format!("{root}/v1"),
        ApiProtocol::Anthropic => root,
    };
    let output = crate::admin::absolute(output)?;
    if [
        store.path.as_path(),
        config.vault.as_path(),
        client_path.as_path(),
    ]
    .contains(&output.as_path())
    {
        return Err(GatewayError::InvalidRequest);
    }
    crate::config::write_json(
        &output,
        &json!({"protocol":source.protocol, "base_url":base,
        "api_key":format!("monica-{}", client.capability), "grant":name}),
        force,
    )?;
    Ok(
        json!({"grant":name,"protocol":source.protocol,"base_url":base,"config_file":output,"expires_at":grant.expires_at}),
    )
}

pub(crate) struct Delivery {
    pub gateway: Arc<Gateway>,
    pub capability: Zeroizing<String>,
    pub binding: Connection,
    pub operation: Operation,
    pub secrets: Zeroizing<Vec<String>>,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Delivery {
    fn alive(&self) -> Result<()> {
        self.gateway
            .proxy_recheck(&self.capability, &self.binding, self.operation)
    }
    fn scan(&self, value: &Value) -> Result<()> {
        for key in self.secrets.iter() {
            crate::upstream::reject_secret_value(value, key)?;
        }
        Ok(())
    }
}

/// Keep complete SSE events until inspected. Pending token prefixes across text
/// deltas are held until resolved, without buffering an ordinary whole response.
struct EventFilter {
    variants: Zeroizing<Vec<String>>,
    tail: Zeroizing<String>,
    held: Zeroizing<Vec<u8>>,
    terminal: bool,
}
impl EventFilter {
    fn new(secrets: &[String]) -> Self {
        use base64::Engine;
        let mut variants = Vec::new();
        for secret in secrets {
            variants.extend([
                secret.clone(),
                hex::encode(secret),
                hex::encode_upper(secret),
                base64::engine::general_purpose::STANDARD.encode(secret),
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
                url::form_urlencoded::byte_serialize(secret.as_bytes()).collect(),
            ]);
        }
        Self {
            variants: Zeroizing::new(variants),
            tail: Zeroizing::new(String::new()),
            held: Zeroizing::new(Vec::new()),
            terminal: false,
        }
    }
    fn event(&mut self, event: &[u8], delivery: &Delivery) -> Result<Option<Vec<u8>>> {
        let text = std::str::from_utf8(event).map_err(|_| GatewayError::ResponseBlocked)?;
        delivery.scan(&json!(text))?;
        let data = Zeroizing::new(
            text.lines()
                .filter_map(|line| {
                    line.strip_prefix("data:")
                        .map(|s| s.strip_prefix(' ').unwrap_or(s))
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if !data.is_empty() && data.as_str() != "[DONE]" {
            let value: Value =
                serde_json::from_str(&data).map_err(|_| GatewayError::ResponseBlocked)?;
            delivery.scan(&value)?;
            self.terminal |= matches!(
                value["type"].as_str(),
                Some(
                    "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "message_stop"
                        | "error"
                )
            );
            let mut append = |value: &Value| {
                if let Some(s) = value.as_str() {
                    self.tail.push_str(s);
                }
            };
            append(&value["delta"]);
            append(&value["delta"]["text"]);
            append(&value["delta"]["partial_json"]);
            append(&value["delta"]["thinking"]);
            append(&value["content_block"]["text"]);
            append(&value["completion"]);
            if let Some(choices) = value["choices"].as_array() {
                for choice in choices {
                    append(&choice["delta"]["content"]);
                    if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                        for call in calls {
                            append(&call["function"]["arguments"]);
                        }
                    }
                }
            }
            if self
                .variants
                .iter()
                .any(|key| self.tail.contains(key.as_str()))
            {
                return Err(GatewayError::ResponseBlocked);
            }
        }
        self.terminal |= data.as_str() == "[DONE]";
        let hold = self
            .variants
            .iter()
            .flat_map(|key| {
                (1..key.len())
                    .filter(|n| key.is_char_boundary(*n))
                    .filter(|n| self.tail.ends_with(&key[..*n]))
            })
            .max()
            .unwrap_or(0);
        let keep = self.tail.len().saturating_sub(hold);
        self.tail.drain(..keep);
        if self.held.len().saturating_add(event.len()) > 2 * MAX_EVENT_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        self.held.extend_from_slice(event);
        if hold == 0 || data.as_str() == "[DONE]" {
            Ok(Some(std::mem::take(&mut *self.held)))
        } else {
            Ok(None)
        }
    }
}

fn event_end(buffer: &[u8]) -> Option<usize> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4);
    lf.into_iter().chain(crlf).min()
}

pub(crate) async fn response(
    mut upstream: reqwest::Response,
    delivery: Delivery,
) -> Result<Response> {
    if upstream.status().is_redirection() {
        return Err(GatewayError::RedirectBlocked);
    }
    if upstream.headers().get_all("content-type").iter().count() > 1 {
        return Err(GatewayError::ResponseBlocked);
    }
    let status = upstream.status();
    if upstream
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(GatewayError::ResponseTooLarge);
    }
    let mut headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if matches!(
            name.as_str(),
            "content-type" | "request-id" | "x-request-id" | "retry-after" | "openai-processing-ms"
        ) || name.as_str().starts_with("x-ratelimit-")
            || name.as_str().starts_with("anthropic-ratelimit-")
        {
            delivery.scan(&json!(
                value.to_str().map_err(|_| GatewayError::ResponseBlocked)?
            ))?;
            headers.insert(name.clone(), value.clone());
        }
    }
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    let streaming = upstream
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next() == Some("text/event-stream"));
    if !streaming {
        let mut bytes = Zeroizing::new(Vec::new());
        loop {
            let chunk = checked_chunk(&mut upstream, &delivery).await?;
            let Some(chunk) = chunk else {
                break;
            };
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(GatewayError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        delivery.scan(&json!(String::from_utf8_lossy(&bytes)))?;
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => delivery.scan(&value)?,
            Err(_) if status.is_success() => return Err(GatewayError::ResponseBlocked),
            Err(_) => {}
        }
        delivery.alive()?;
        let mut response = Response::new(Body::from(std::mem::take(&mut *bytes)));
        *response.status_mut() = status;
        *response.headers_mut() = headers;
        return Ok(response);
    }
    // A bounded queue propagates backpressure; dropping the client cancels its
    // upstream socket and releases the concurrency permit.
    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(1);
    tokio::spawn(async move {
        let result = pump(&mut upstream, &delivery, &tx).await;
        if result.is_err() {
            // Drop the upstream and its secrets even when a client stops reading.
            drop(upstream);
            drop(delivery);
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                tx.send(Err(std::io::Error::other(
                    "Monica proxy stream interrupted",
                ))),
            )
            .await;
        }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

async fn checked_chunk(
    upstream: &mut reqwest::Response,
    delivery: &Delivery,
) -> Result<Option<Bytes>> {
    let chunk = upstream.chunk();
    tokio::pin!(chunk);
    loop {
        delivery.alive()?;
        tokio::select! {
            result = &mut chunk => return result.map_err(|_| GatewayError::UpstreamUnavailable),
            _ = delivery.gateway.stopped.cancelled() => return Err(GatewayError::UnlockRequired),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
    }
}

async fn send(
    tx: &tokio::sync::mpsc::Sender<std::result::Result<Bytes, std::io::Error>>,
    delivery: &Delivery,
    bytes: Vec<u8>,
) -> Result<()> {
    loop {
        delivery.alive()?;
        tokio::select! {
            permit = tx.reserve() => { permit.map_err(|_| GatewayError::BrokerUnavailable)?.send(Ok(Bytes::from(bytes))); return Ok(()); },
            _ = delivery.gateway.stopped.cancelled() => return Err(GatewayError::UnlockRequired),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {},
        }
    }
}

async fn pump(
    upstream: &mut reqwest::Response,
    delivery: &Delivery,
    tx: &tokio::sync::mpsc::Sender<std::result::Result<Bytes, std::io::Error>>,
) -> Result<()> {
    let mut buffer = Zeroizing::new(Vec::new());
    let mut filter = EventFilter::new(&delivery.secrets);
    let mut total = 0usize;
    loop {
        let chunk = tokio::select! {
            chunk = checked_chunk(upstream, delivery) => chunk?,
            _ = tx.closed() => return Ok(()),
        };
        let Some(chunk) = chunk else {
            break;
        };
        total = total.saturating_add(chunk.len());
        if total > MAX_RESPONSE_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        buffer.extend_from_slice(&chunk);
        while let Some(end) = event_end(&buffer) {
            if end > MAX_EVENT_BYTES {
                return Err(GatewayError::ResponseTooLarge);
            }
            if let Some(bytes) = filter.event(&buffer[..end], delivery)? {
                send(tx, delivery, bytes).await?;
            }
            buffer.drain(..end);
        }
        if buffer.len() > MAX_EVENT_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
    }
    // An incomplete frame is a truncated stream, never a successful completion.
    if !buffer.is_empty() || !filter.terminal {
        return Err(GatewayError::UpstreamUnavailable);
    }
    if !filter.held.is_empty() {
        send(tx, delivery, std::mem::take(&mut *filter.held)).await?;
    }
    Ok(())
}
