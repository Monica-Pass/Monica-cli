use std::time::Duration;

use base64::Engine;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroizing;

use crate::config::Connection;
use crate::error::{GatewayError, Result};
use crate::model::{Arguments, Provider};
use crate::vault::Credential;

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub fn client() -> Result<reqwest::Client> {
    client_builder()
        .build()
        .map_err(|_| GatewayError::StateUnavailable)
}

pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .pool_max_idle_per_host(0)
}

pub(crate) async fn execute(
    client: &reqwest::Client,
    binding: &Connection,
    credential: &Credential,
    arguments: &Arguments,
) -> Result<Value> {
    let url = request_url(binding, arguments)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("MonicaPass-CredentialGateway/0.2"),
    );
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    let mut authorization = match binding.provider {
        Provider::Github => {
            headers.insert(
                "x-github-api-version",
                HeaderValue::from_static("2022-11-28"),
            );
            let value = Zeroizing::new(format!("Bearer {}", credential.token.as_str()));
            HeaderValue::from_str(&value)
        }
        Provider::Gitlab => HeaderValue::from_str(&credential.token),
    }
    .map_err(|_| GatewayError::CredentialUnavailable)?;
    authorization.set_sensitive(true);
    headers.insert(
        if binding.provider == Provider::Github {
            AUTHORIZATION
        } else {
            reqwest::header::HeaderName::from_static("private-token")
        },
        authorization,
    );
    let request = match arguments {
        Arguments::Create(args) => {
            let body = match binding.provider {
                Provider::Github => json!({"title": args.title, "body": args.body}),
                Provider::Gitlab => json!({"title": args.title, "description": args.body}),
            };
            client.post(url).headers(headers).json(&body)
        }
        _ => client.get(url).headers(headers),
    };
    let result = read_response(request, binding, credential, arguments).await;
    // Once a write has been dispatched, a malformed/blocked response cannot prove
    // that the issue was not created. Every such outcome must remain non-replayable.
    if matches!(arguments, Arguments::Create(_)) {
        result.map_err(|_| GatewayError::WriteOutcomeUnknown)
    } else {
        result
    }
}

async fn read_response(
    request: reqwest::RequestBuilder,
    binding: &Connection,
    credential: &Credential,
    arguments: &Arguments,
) -> Result<Value> {
    // There is deliberately no retry layer, including for POST.
    let mut response = request.send().await.map_err(|_| {
        if matches!(arguments, Arguments::Create(_)) {
            GatewayError::WriteOutcomeUnknown
        } else {
            GatewayError::UpstreamUnavailable
        }
    })?;
    if response.status().is_redirection() {
        return Err(GatewayError::RedirectBlocked);
    }
    if !response.status().is_success() {
        return Err(GatewayError::UpstreamRejected);
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return Err(GatewayError::ResponseTooLarge);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        if matches!(arguments, Arguments::Create(_)) {
            GatewayError::WriteOutcomeUnknown
        } else {
            GatewayError::UpstreamUnavailable
        }
    })? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let raw: Value = serde_json::from_slice(&bytes).map_err(|_| GatewayError::ResponseBlocked)?;
    reject_secret(&raw, credential)?;
    project_response(raw, binding, arguments)
}

fn request_url(binding: &Connection, arguments: &Arguments) -> Result<Url> {
    let mut url = Url::parse(&binding.api_base).map_err(|_| GatewayError::InvalidConfig)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| GatewayError::InvalidConfig)?;
        segments.pop_if_empty();
        match binding.provider {
            Provider::Github => {
                segments.push("repos");
                for segment in arguments.repository().split('/') {
                    segments.push(segment);
                }
            }
            Provider::Gitlab => {
                segments.push("projects").push(arguments.repository());
            }
        }
        segments.push("issues");
        if let Arguments::Get(args) = arguments {
            segments.push(&args.number.to_string());
        }
    }
    if let Arguments::List(args) = arguments {
        url.query_pairs_mut()
            .append_pair("state", "all")
            .append_pair("page", &args.page.to_string())
            .append_pair("per_page", &args.per_page.to_string());
    }
    Ok(url)
}

/// Inspect values as well as keys, including JSON-unescaped strings. This is a final guard;
/// the normal response path already projects a small allowlist of service result fields.
pub(crate) fn reject_secret(value: &Value, credential: &Credential) -> Result<()> {
    reject_secret_value(value, &credential.token)
}

pub(crate) fn reject_secret_value(value: &Value, token: &str) -> Result<()> {
    let variants = Zeroizing::new(vec![
        token.to_owned(),
        base64::engine::general_purpose::STANDARD.encode(token),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(token),
        base64::engine::general_purpose::URL_SAFE.encode(token),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token),
        hex::encode(token),
        hex::encode_upper(token),
        url::form_urlencoded::byte_serialize(token.as_bytes()).collect(),
    ]);
    fn contains(value: &Value, variants: &[String]) -> bool {
        match value {
            Value::String(text) => variants.iter().any(|secret| text.contains(secret)),
            Value::Array(items) => items.iter().any(|item| contains(item, variants)),
            Value::Object(map) => map.iter().any(|(key, value)| {
                variants.iter().any(|secret| key.contains(secret)) || contains(value, variants)
            }),
            Value::Number(number) => {
                let text = number.to_string();
                variants.iter().any(|secret| text.contains(secret))
            }
            _ => false,
        }
    }
    if contains(value, &variants) {
        Err(GatewayError::ResponseBlocked)
    } else {
        Ok(())
    }
}

fn project_response(raw: Value, binding: &Connection, arguments: &Arguments) -> Result<Value> {
    let provider = binding.provider;
    match arguments {
        Arguments::List(args) => {
            let rows = raw.as_array().ok_or(GatewayError::ResponseBlocked)?;
            if rows.len() > args.per_page as usize {
                return Err(GatewayError::ResponseTooLarge);
            }
            let items = rows
                .iter()
                .filter(|row| provider != Provider::Github || row.get("pull_request").is_none())
                .map(|row| project_issue(row, binding, arguments.repository(), false))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"items": items, "page": args.page, "per_page": args.per_page}))
        }
        Arguments::Get(args) => {
            let issue = project_issue(&raw, binding, arguments.repository(), true)?;
            if issue["number"] != args.number {
                return Err(GatewayError::ResponseBlocked);
            }
            Ok(json!({"issue": issue}))
        }
        Arguments::Create(_) => {
            // Journal only the created resource identity, not its potentially private body.
            let issue = project_issue(&raw, binding, arguments.repository(), false)?;
            Ok(json!({"issue": {"number": issue["number"], "url": issue["url"]}}))
        }
    }
}

fn project_issue(
    raw: &Value,
    binding: &Connection,
    repository: &str,
    include_body: bool,
) -> Result<Value> {
    let (number_key, body_key) = match binding.provider {
        Provider::Github => ("number", "body"),
        Provider::Gitlab => ("iid", "description"),
    };
    let number = raw
        .get(number_key)
        .and_then(Value::as_u64)
        .filter(|number| *number > 0)
        .ok_or(GatewayError::ResponseBlocked)?;
    let title = bounded_string(raw, "title", 1024)?;
    let state = bounded_string(raw, "state", 32)?;
    // Resource links are built from the trusted connection and repository scope;
    // arbitrary links supplied by the service are never forwarded to the Agent.
    let mut link = Url::parse(&binding.api_base).map_err(|_| GatewayError::InvalidConfig)?;
    if binding.provider == Provider::Github && link.host_str() == Some("api.github.com") {
        link.set_host(Some("github.com"))
            .map_err(|_| GatewayError::InvalidConfig)?;
    }
    let suffix = if binding.provider == Provider::Gitlab {
        "-/issues"
    } else {
        "issues"
    };
    link.set_path(&format!("/{repository}/{suffix}/{number}"));
    let mut result =
        json!({"number": number, "title": title, "state": state, "url": link.as_str()});
    if include_body {
        let body = match raw.get(body_key) {
            None | Some(Value::Null) => "",
            Some(Value::String(text)) if text.len() <= 256 * 1024 => text,
            _ => return Err(GatewayError::ResponseTooLarge),
        };
        result["body"] = Value::String(body.to_owned());
    }
    Ok(result)
}

fn bounded_string<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| text.len() <= max)
        .ok_or(GatewayError::ResponseBlocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_rejects_decoded_and_numeric_secret_reflections() {
        let credential = Credential {
            token: Zeroizing::new("1234567890123456".to_owned()),
        };
        assert_eq!(
            reject_secret(&json!({"number":1234567890123456_u64}), &credential),
            Err(GatewayError::ResponseBlocked)
        );
        let credential = Credential {
            token: Zeroizing::new("synthetic-token-to-be-protected".to_owned()),
        };
        let escaped = credential
            .token
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect::<String>();
        let decoded: Value = serde_json::from_str(&format!("{{\"body\":\"{escaped}\"}}")).unwrap();
        assert_eq!(
            reject_secret(&decoded, &credential),
            Err(GatewayError::ResponseBlocked)
        );
        assert_eq!(
            reject_secret(
                &json!({"body":hex::encode_upper(credential.token.as_bytes())}),
                &credential
            ),
            Err(GatewayError::ResponseBlocked)
        );
    }
}
