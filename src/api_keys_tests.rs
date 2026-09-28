use mdbx_storage::repo::{
    CommitContext, ObjectSummaryRepo, OperationCoordinator, WriteCommand, WriteOperationRequest,
};
use serde_json::{Value, json};
use std::time::Duration;

use crate::api_keys::{self, ApiProtocol, BindOptions, SourceFormat};
use crate::config::{ClientConfig, connection_fingerprint, write_json};
use crate::error::GatewayError;
use crate::model::{CONNECTION_CATALOG_TOOL, Operation, Provider, ToolCall};
use crate::test_support::{Fixture, PASSWORD, Reply, TOKEN};

pub(crate) async fn bound_fixture(
    protocol: ApiProtocol,
    format: SourceFormat,
    replies: Vec<Reply>,
) -> (Fixture, BindOptions, String) {
    let mut fixture = Fixture::new(
        Provider::Github,
        &[Operation::ApiRead, Operation::ApiWrite],
        replies,
    )
    .await;
    let config = fixture.store.load().unwrap();
    let collection = config.collection_id.unwrap();
    let entry = uuid::Uuid::new_v4().to_string();
    let endpoint = format!("https://127.0.0.1:{}/v1", fixture.upstream.port);
    let payload = match format {
        SourceFormat::AndroidApiKey => json!({"kind":"password", "login_type":"API_KEY", "password_plain":TOKEN,
            "website":"https://private-console.example.test", "notes":"PRIVATE_ANDROID_NOTE",
            "custom_fields":[{"title":"monica_api_key_type","value":"API_KEY"},
                {"title":"monica_api_key_url","value":endpoint}, {"title":"future","value":"PRIVATE_CUSTOM_FIELD","protected":true}],
            "extension":{"values":[null,false,"","中文"]}}),
        SourceFormat::NativeApiToken => json!({"schema":"monica.api-token.v1", "provider":"自建模型", "api_base":endpoint,
            "token":TOKEN,"note":"PRIVATE_ANDROID_NOTE","extension":{"fields":[null,false]}}),
    }.to_string();
    {
        let conn = fixture.vault.runtime.read().unwrap();
        OperationCoordinator::execute(
            &conn,
            &CommitContext::new("android-fixture".to_owned()),
            WriteOperationRequest::new(
                uuid::Uuid::new_v4().to_string(),
                "android-api-key",
                vec![WriteCommand::CreateEntry {
                    entry_id: entry.clone(),
                    project_id: collection,
                    entry_type: match format {
                        SourceFormat::AndroidApiKey => "login",
                        SourceFormat::NativeApiToken => "api-token",
                    }
                    .to_owned(),
                    title: "Android key".to_owned(),
                    payload_json: payload.clone(),
                }],
            ),
        )
        .unwrap();
    }
    let options = BindOptions {
        name: "android".to_owned(),
        entry,
        auth: None,
        protocol,
        api_base: None,
        note: "Explicit AI purpose".to_owned(),
        replace: false,
    };
    let (binding, _) = api_keys::bind(&fixture.store, &options, PASSWORD).unwrap();
    let file = fixture._directory.path().join("test-agent.client.json");
    write_json(
        &file,
        &ClientConfig {
            version: 1,
            endpoint: format!("http://{}/", config.listen),
            capability: fixture.capability.to_string(),
        },
        false,
    )
    .unwrap();
    fixture
        .store
        .update(|config| {
            let mut config = config.unwrap();
            config.grants[0].connection = "android".to_owned();
            config.grants[0].operations = [Operation::ModelList, Operation::ModelInvoke].into();
            config.grants[0].connection_fingerprint = connection_fingerprint(&binding);
            config.grants[0].client_file = Some(file);
            Ok((config, ()))
        })
        .unwrap();
    fixture.gateway = fixture.rebuild();
    (fixture, options, payload)
}

fn start(
    fixture: &mut Fixture,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<crate::error::Result<()>>,
) {
    let listener = fixture.broker_listener.take().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let gateway = fixture.gateway.clone();
    let task = tokio::spawn(crate::protocol::serve_broker(gateway, listener, async {
        let _ = stopped.await;
    }));
    (stop, task)
}

fn local_url(fixture: &Fixture, path: &str) -> String {
    format!("http://{}{}", fixture.store.load().unwrap().listen, path)
}
fn local_key(fixture: &Fixture) -> String {
    format!("monica-{}", fixture.capability.as_str())
}

fn approve_proxy(fixture: &mut Fixture, seconds: u32) {
    std::sync::Arc::get_mut(&mut fixture.gateway)
        .unwrap()
        .authorize_proxy_grants(
            &["test-agent".to_owned()],
            seconds,
            chrono::Utc::now().timestamp(),
        )
        .unwrap();
}

#[tokio::test]
async fn explicit_proxy_sessions_cross_fresh_auth_window_for_both_protocols() {
    for protocol in [ApiProtocol::Openai, ApiProtocol::Anthropic] {
        let (mut fixture, _, _) = bound_fixture(
            protocol,
            SourceFormat::AndroidApiKey,
            vec![Reply::json(json!({"id":"beyond-freshness"}))],
        )
        .await;
        // Start close to the boundary, then let real wall time cross it without
        // replacing the authenticated session (which correctly revokes leases).
        {
            let mut conn = fixture.vault.runtime.write().unwrap();
            let mut session = conn.active_session().unwrap().clone();
            session.assurance.authenticated_at_unix_secs = chrono::Utc::now().timestamp() - 299;
            session.assurance.last_activity_at_unix_secs =
                session.assurance.authenticated_at_unix_secs;
            conn.attach_session(session);
        }
        assert_eq!(fixture.gateway.proxy_session_info(), json!([]));
        approve_proxy(&mut fixture, 3600);
        tokio::time::sleep(Duration::from_millis(2200)).await;
        let binding = fixture.store.load().unwrap().connections["android"].clone();
        assert!(
            fixture
                .vault
                .credential(&binding, chrono::Utc::now().timestamp())
                .is_err()
        );
        assert!(
            fixture
                .gateway
                .call(
                    &fixture.capability,
                    ToolCall {
                        tool: CONNECTION_CATALOG_TOOL.to_owned(),
                        arguments: json!({}),
                    }
                )
                .await
                .is_err()
        );
        let (stop, task) = start(&mut fixture);
        let path = if protocol == ApiProtocol::Openai {
            "/v1/responses"
        } else {
            "/v1/messages"
        };
        let response = reqwest::Client::new()
            .post(local_url(&fixture, path))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture","messages":[],"max_tokens":8}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap()["id"],
            "beyond-freshness"
        );
        assert_eq!(fixture.upstream.requests().len(), 1);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn explicit_proxy_session_expiry_interrupts_stream_and_cannot_fall_back() {
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![Reply {
            body: b"data: {\"id\":\"first\"}\n\n".to_vec(),
            headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
            stream_chunks: vec![(Duration::from_secs(5), b"data: [DONE]\n\n".to_vec())],
            ..Reply::json(Value::Null)
        }],
    )
    .await;
    approve_proxy(&mut fixture, 2);
    let (stop, task) = start(&mut fixture);
    let http = reqwest::Client::new();
    let mut response = http
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture","stream":true}))
        .send()
        .await
        .unwrap();
    assert!(response.chunk().await.unwrap().is_some());
    assert!(
        tokio::time::timeout(Duration::from_secs(3), response.text())
            .await
            .unwrap()
            .is_err()
    );
    let response = http
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture"}))
        .send()
        .await
        .unwrap();
    assert_ne!(response.status(), 200);
    assert_eq!(fixture.upstream.requests().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn proxy_session_admission_rejects_invalid_grants_and_reports_actual_deadline() {
    let (mut fixture, _, _) =
        bound_fixture(ApiProtocol::Openai, SourceFormat::AndroidApiKey, vec![]).await;
    let now = chrono::Utc::now().timestamp();
    let original = fixture.store.load().unwrap();
    for invalid in [
        "missing",
        "future",
        "expired",
        "unbounded",
        "rest",
        "duplicate",
    ] {
        let mut config = original.clone();
        match invalid {
            "future" => config.grants[0].issued_at = now + 100,
            "expired" => config.grants[0].expires_at = now - 1,
            "unbounded" => config.grants[0].expires_at = 0,
            "rest" => config.grants[0].operations = [Operation::ApiRead].into(),
            _ => {}
        }
        fixture.store.update(|_| Ok((config, ()))).unwrap();
        fixture.gateway = fixture.rebuild();
        let names = match invalid {
            "missing" => vec!["missing".into()],
            "duplicate" => vec!["test-agent".into(), "test-agent".into()],
            _ => vec!["test-agent".into()],
        };
        assert!(
            std::sync::Arc::get_mut(&mut fixture.gateway)
                .unwrap()
                .authorize_proxy_grants(&names, 3600, now)
                .is_err()
        );
        assert_eq!(fixture.gateway.proxy_session_info(), json!([]));
    }
    fixture.store.update(|_| Ok((original, ()))).unwrap();
    fixture.gateway = fixture.rebuild();
    approve_proxy(&mut fixture, 86400);
    let expires = fixture.gateway.proxy_session_info()[0]["expires_at_unix"]
        .as_i64()
        .unwrap();
    assert!(expires <= now + 7200);
    assert!(expires > now + 600);
    assert_eq!(fixture.rebuild().proxy_session_info(), json!([]));
    assert!(fixture.upstream.requests().is_empty());
}

#[tokio::test]
async fn proxy_sessions_preserve_call_budgets_across_broker_rebuilds() {
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![Reply::json(json!({"id":"one-call"}))],
    )
    .await;
    fixture
        .store
        .update(|config| {
            let mut config = config.unwrap();
            config.grants[0].max_calls = 1;
            Ok((config, ()))
        })
        .unwrap();
    fixture.gateway = fixture.rebuild();
    approve_proxy(&mut fixture, 3600);
    let (stop, task) = start(&mut fixture);
    for expected in [200, 401] {
        let response = reqwest::Client::new()
            .post(local_url(&fixture, "/v1/responses"))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        let _ = response.text().await.unwrap();
    }
    assert_eq!(fixture.upstream.requests().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    fixture.gateway = fixture.rebuild();
    assert!(matches!(
        std::sync::Arc::get_mut(&mut fixture.gateway)
            .unwrap()
            .authorize_proxy_grants(&["test-agent".into()], 3600, chrono::Utc::now().timestamp()),
        Err(GatewayError::ReauthorizationRequired)
    ));
}

#[tokio::test]
async fn upstream_receives_only_the_inspected_json_when_fields_are_duplicated() {
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![Reply::json(json!({"id":"fixture"}))],
    )
    .await;
    let (stop, task) = start(&mut fixture);
    let raw = format!(
        r#"{{"model":"fixture","input":"{}","input":"safe","background":true,"background":false,"extension":123456789012345678901234567890}}"#,
        local_key(&fixture)
    );
    let response = reqwest::Client::new()
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .header("content-type", "application/json")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let sent = fixture.upstream.requests();
    let text = String::from_utf8(sent[0].body.clone()).unwrap();
    assert!(!text.contains(fixture.capability.as_str()));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["background"], false);
    assert_eq!(value["input"], "safe");
    assert_eq!(
        value["extension"].to_string(),
        "123456789012345678901234567890"
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn local_api_key_provider_does_not_expand_the_legacy_gateway_edit_adapter() {
    let (fixture, options, payload) =
        bound_fixture(ApiProtocol::Openai, SourceFormat::NativeApiToken, vec![]).await;
    let original = ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &options.entry)
        .unwrap()
        .unwrap();
    let mut payload: Value = serde_json::from_str(&payload).unwrap();
    payload["schema"] = json!("monica.gateway.credential.v1");
    payload["provider"] = json!("api-key");
    fixture
        .vault
        .write_object(
            &original,
            "future-provider",
            WriteCommand::UpdateEntry {
                entry_id: options.entry.clone(),
                project_id: original.collection_id.clone(),
                entry_type: "api-token".to_owned(),
                title: "foreign-token".to_owned(),
                payload_json: payload.to_string(),
            },
        )
        .unwrap();
    let inventory = fixture.vault.gateway_inventory().unwrap();
    assert!(
        inventory
            .connections
            .values()
            .all(|c| c.credential_id != options.entry)
    );
    assert!(matches!(
        fixture.vault.delete_entry(&options.entry),
        Err(GatewayError::ObjectReadOnly)
    ));
    let after = ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &options.entry)
        .unwrap()
        .unwrap();
    assert!(!after.deleted);
}

#[tokio::test]
async fn client_disconnect_releases_proxy_capacity_and_truncated_sse_is_an_error() {
    let first = b"data: {\"delta\":\"hello\"}\n\n";
    let slow = Reply {
        body: first.to_vec(),
        headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
        stream_chunks: vec![(Duration::from_secs(5), b"data: [DONE]\n\n".to_vec())],
        ..Reply::json(Value::Null)
    };
    let truncated = Reply {
        body: first.to_vec(),
        headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
        ..Reply::json(Value::Null)
    };
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![slow, truncated],
    )
    .await;
    let occupied: Vec<_> = (0..3)
        .map(|_| fixture.gateway.proxy_slot().unwrap())
        .collect();
    let (stop, task) = start(&mut fixture);
    let http = reqwest::Client::new();
    let mut response = http
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture","stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), first);
    assert!(fixture.gateway.proxy_slot().is_err());
    drop(response);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(permit) = fixture.gateway.proxy_slot() {
                drop(permit);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("client cancellation did not release capacity");
    drop(occupied);
    let response = http
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture","stream":true}))
        .send()
        .await;
    if let Ok(response) = response {
        assert!(response.bytes().await.is_err());
    }
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn model_grants_cannot_call_generic_tools_and_endpoint_overrides_fail_closed() {
    let (fixture, mut options, _) =
        bound_fixture(ApiProtocol::Openai, SourceFormat::AndroidApiKey, vec![]).await;
    for tool in [
        "service_api_read",
        "service_api_write",
        "service_model_invoke",
        "github_list_issues",
    ] {
        assert!(
            fixture
                .gateway
                .call(
                    &fixture.capability,
                    ToolCall {
                        tool: tool.to_owned(),
                        arguments: json!({"method":"GET", "path":"models"})
                    }
                )
                .await
                .is_err()
        );
    }
    options.name = "other".to_owned();
    options.api_base = Some("https://different.example.test/v1".to_owned());
    assert!(matches!(
        api_keys::bind(&fixture.store, &options, PASSWORD),
        Err(GatewayError::InvalidConfig)
    ));
    assert!(
        !fixture
            .store
            .load()
            .unwrap()
            .connections
            .contains_key("other")
    );
    assert!(fixture.upstream.requests().is_empty());
}

#[tokio::test]
async fn expired_grants_and_revoked_streams_stop_without_completion() {
    for expire in [true, false] {
        let first = b"data: {\"delta\":\"hello\"}\n\n";
        let reply = Reply {
            body: first.to_vec(),
            headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
            stream_chunks: vec![(Duration::from_secs(5), b"data: [DONE]\n\n".to_vec())],
            ..Reply::json(Value::Null)
        };
        let (mut fixture, _, _) = bound_fixture(
            ApiProtocol::Openai,
            SourceFormat::AndroidApiKey,
            vec![reply],
        )
        .await;
        approve_proxy(&mut fixture, 3600);
        let (stop, task) = start(&mut fixture);
        let http = reqwest::Client::new();
        let mut response = http
            .post(local_url(&fixture, "/v1/responses"))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture","stream":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), first);
        if expire {
            fixture
                .store
                .update(|c| {
                    let mut c = c.unwrap();
                    c.grants[0].issued_at = chrono::Utc::now().timestamp() - 120;
                    c.grants[0].expires_at = chrono::Utc::now().timestamp() - 1;
                    Ok((c, ()))
                })
                .unwrap();
        } else {
            crate::admin::revoke(&fixture.store, "test-agent").unwrap();
        }
        let rest = tokio::time::timeout(Duration::from_secs(2), response.text())
            .await
            .expect("revoked stream kept running");
        assert!(
            rest.is_err(),
            "interrupted stream must report a transport failure"
        );
        assert_eq!(
            http.get(local_url(&fixture, "/v1/models"))
                .bearer_auth(local_key(&fixture))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(fixture.upstream.requests().len(), 1);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn redirects_reflections_disconnects_and_error_statuses_are_not_retried() {
    let replies = vec![
        Reply {
            status: 302,
            location: Some("https://elsewhere.test".to_owned()),
            ..Reply::json(json!({}))
        },
        Reply::json(json!({"reflected":TOKEN})),
        Reply::disconnected(),
        Reply {
            status: 429,
            headers: vec![("retry-after".to_owned(), "10".to_owned())],
            ..Reply::json(json!({"error":{"type":"rate_limit_error","message":"synthetic limit"}}))
        },
    ];
    let (mut fixture, _, _) =
        bound_fixture(ApiProtocol::Openai, SourceFormat::NativeApiToken, replies).await;
    let (stop, task) = start(&mut fixture);
    let http = reqwest::Client::new();
    for expected in [502, 502, 502, 429] {
        let response = http
            .post(local_url(&fixture, "/v1/responses"))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == 429 {
            assert_eq!(response.headers()["retry-after"], "10");
        }
        let body = response.text().await.unwrap();
        assert!(!body.contains(TOKEN));
        assert!(!body.contains(fixture.capability.as_str()));
    }
    assert_eq!(fixture.upstream.requests().len(), 4);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn tiga_expiry_interrupts_an_already_authorized_stream() {
    let first = b"data: {\"delta\":\"hello\"}\n\n";
    let reply = Reply {
        body: first.to_vec(),
        headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
        stream_chunks: vec![(Duration::from_secs(5), b"data: [DONE]\n\n".to_vec())],
        ..Reply::json(Value::Null)
    };
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![reply],
    )
    .await;
    let (stop, task) = start(&mut fixture);
    let mut response = reqwest::Client::new()
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture","stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap().as_ref(), first);
    {
        let mut conn = fixture.vault.runtime.write().unwrap();
        let mut session = conn.active_session().unwrap().clone();
        session.assurance.authenticated_at_unix_secs -= 86400;
        session.assurance.last_activity_at_unix_secs -= 86400;
        conn.attach_session(session);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), response.text())
            .await
            .unwrap()
            .is_err()
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn binding_and_unbinding_preserve_android_payload_identity_and_private_notes() {
    for format in [SourceFormat::AndroidApiKey, SourceFormat::NativeApiToken] {
        let (fixture, options, payload) = bound_fixture(ApiProtocol::Openai, format, vec![]).await;
        let before = ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &options.entry)
            .unwrap()
            .unwrap();
        let binding = fixture.store.load().unwrap().connections["android"].clone();
        assert_eq!(binding.credential_id, options.entry);
        assert_eq!(
            binding.api_key.as_ref().unwrap().head_commit_id,
            before.head_commit_id
        );
        assert!(
            fixture
                .vault
                .credential(&binding, chrono::Utc::now().timestamp())
                .is_ok()
        );
        let catalog = fixture
            .gateway
            .call(
                &fixture.capability,
                ToolCall {
                    tool: CONNECTION_CATALOG_TOOL.to_owned(),
                    arguments: json!({}),
                },
            )
            .await
            .unwrap();
        for data in [
            std::fs::read_to_string(&fixture.store.path).unwrap(),
            catalog.to_string(),
        ] {
            for secret in [TOKEN, "PRIVATE_ANDROID_NOTE", "PRIVATE_CUSTOM_FIELD"] {
                assert!(!data.contains(secret));
            }
        }
        assert!(matches!(
            crate::admin::delete_connection(&fixture.store, "android", PASSWORD),
            Err(GatewayError::ObjectReadOnly)
        ));
        assert_eq!(
            api_keys::unbind(&fixture.store, "android", PASSWORD).unwrap(),
            1
        );
        assert!(
            !fixture
                .store
                .load()
                .unwrap()
                .connections
                .contains_key("android")
        );
        let after = ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &options.entry)
            .unwrap()
            .unwrap();
        assert_eq!(before, after);
        let document = fixture.vault.inspect_object(&options.entry).unwrap();
        let fields: serde_json::Map<String, Value> = document
            .fields
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::from_str(v).unwrap()))
            .collect();
        assert_eq!(
            Value::Object(fields),
            serde_json::from_str::<Value>(&payload).unwrap()
        );
    }
}

#[tokio::test]
async fn local_openai_and_anthropic_requests_use_scoped_local_keys_and_inject_only_upstream_auth() {
    for protocol in [ApiProtocol::Openai, ApiProtocol::Anthropic] {
        let (mut fixture, _, _) = bound_fixture(
            protocol,
            SourceFormat::AndroidApiKey,
            vec![Reply::json(json!({"id":"message-fixture","content":[]}))],
        )
        .await;
        let path = if protocol == ApiProtocol::Openai {
            "/v1/responses"
        } else {
            "/v1/messages?beta=true"
        };
        let (stop, task) = start(&mut fixture);
        let http = reqwest::Client::new();
        let mut request = http
            .post(local_url(&fixture, path))
            .json(&json!({"model":"synthetic-model","max_tokens":20,"stream":false,"messages":[]}));
        request = if protocol == ApiProtocol::Openai {
            request.bearer_auth(local_key(&fixture))
        } else {
            request.header("x-api-key", local_key(&fixture))
        };
        let response = request
            .header("cookie", "do-not-forward")
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["id"], "message-fixture");
        let sent = fixture.upstream.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].target, path);
        assert!(!sent[0].headers.contains_key("cookie"));
        assert!(!format!("{:?}", sent[0]).contains(fixture.capability.as_str()));
        match protocol {
            ApiProtocol::Openai => {
                assert_eq!(sent[0].headers["authorization"], format!("Bearer {TOKEN}"));
                assert!(!sent[0].headers.contains_key("x-api-key"));
            }
            ApiProtocol::Anthropic => {
                assert_eq!(sent[0].headers["x-api-key"], TOKEN);
                assert!(!sent[0].headers.contains_key("authorization"));
            }
        }
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn proxy_configuration_contains_only_the_revocable_local_key() {
    for protocol in [ApiProtocol::Openai, ApiProtocol::Anthropic] {
        let (fixture, _, _) = bound_fixture(protocol, SourceFormat::NativeApiToken, vec![]).await;
        let path = fixture._directory.path().join("model-client.json");
        let result =
            crate::ai_proxy::write_client_config(&fixture.store, "test-agent", &path, false)
                .unwrap();
        let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["api_key"], local_key(&fixture));
        assert!(!config.to_string().contains(TOKEN));
        assert!(!result.to_string().contains(fixture.capability.as_str()));
        assert_eq!(
            config["base_url"],
            local_url(
                &fixture,
                if protocol == ApiProtocol::Openai {
                    "/v1"
                } else {
                    ""
                }
            )
        );
        assert!(
            crate::ai_proxy::write_client_config(&fixture.store, "test-agent", &path, false)
                .is_err()
        );
        assert!(
            crate::ai_proxy::write_client_config(
                &fixture.store,
                "test-agent",
                &fixture.store.path,
                true
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn proxy_refuses_wrong_protocol_ambiguous_auth_and_exhausted_budgets_before_network() {
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::NativeApiToken,
        vec![Reply::json(json!({"data":[]}))],
    )
    .await;
    fixture
        .store
        .update(|config| {
            let mut c = config.unwrap();
            c.grants[0].max_calls = 1;
            Ok((c, ()))
        })
        .unwrap();
    let (stop, task) = start(&mut fixture);
    let http = reqwest::Client::new();
    let invalid = http
        .post(local_url(&fixture, "/v1/messages"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 403);
    let invalid = http
        .get(local_url(&fixture, "/v1/models"))
        .bearer_auth(local_key(&fixture))
        .header("x-api-key", local_key(&fixture))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 401);
    let invalid = http
        .get(local_url(&fixture, "/v1/models"))
        .bearer_auth(local_key(&fixture))
        .header("origin", "https://outside.test")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 403);
    assert!(fixture.upstream.requests().is_empty());
    assert_eq!(
        http.get(local_url(&fixture, "/v1/models"))
            .bearer_auth(local_key(&fixture))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let capped = http
        .get(local_url(&fixture, "/v1/models"))
        .bearer_auth(local_key(&fixture))
        .send()
        .await
        .unwrap();
    assert_eq!(capped.status(), 401);
    assert_eq!(fixture.upstream.requests().len(), 1);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn source_edits_stop_old_keys_and_rebinding_revokes_their_grants() {
    let (fixture, mut options, payload) =
        bound_fixture(ApiProtocol::Openai, SourceFormat::AndroidApiKey, vec![]).await;
    let original = ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &options.entry)
        .unwrap()
        .unwrap();
    let mut changed: Value = serde_json::from_str(&payload).unwrap();
    changed["password_plain"] = json!("synthetic-replaced-upstream-key");
    fixture
        .vault
        .write_object(
            &original,
            "replace-android-key",
            WriteCommand::UpdateEntry {
                entry_id: options.entry.clone(),
                project_id: original.collection_id.clone(),
                entry_type: "login".to_owned(),
                title: "Android key".to_owned(),
                payload_json: changed.to_string(),
            },
        )
        .unwrap();
    let binding = fixture.store.load().unwrap().connections["android"].clone();
    assert!(matches!(
        fixture
            .vault
            .credential(&binding, chrono::Utc::now().timestamp()),
        Err(GatewayError::ObjectChanged)
    ));
    options.replace = true;
    let (new, revoked) = api_keys::bind(&fixture.store, &options, PASSWORD).unwrap();
    assert_eq!(revoked, 1);
    assert_ne!(
        connection_fingerprint(&new),
        connection_fingerprint(&binding)
    );
    assert!(fixture.store.load().unwrap().grants.is_empty());
    assert!(fixture.upstream.requests().is_empty());
}

#[tokio::test]
async fn streaming_delivers_first_event_before_completion_and_shutdown_cancels_inflight_request() {
    let first = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n";
    let reply = Reply {
        body: first.to_vec(),
        headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
        stream_chunks: vec![(Duration::from_secs(5), b"data: [DONE]\n\n".to_vec())],
        ..Reply::json(Value::Null)
    };
    let (mut fixture, _, _) = bound_fixture(
        ApiProtocol::Openai,
        SourceFormat::AndroidApiKey,
        vec![reply],
    )
    .await;
    approve_proxy(&mut fixture, 3600);
    let (stop, task) = start(&mut fixture);
    let mut response = reqwest::Client::new()
        .post(local_url(&fixture, "/v1/responses"))
        .bearer_auth(local_key(&fixture))
        .json(&json!({"model":"fixture","stream":true,"input":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let chunk = tokio::time::timeout(Duration::from_secs(2), response.chunk())
        .await
        .expect("first event was buffered until completion")
        .unwrap()
        .unwrap();
    assert_eq!(chunk.as_ref(), first);
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("shutdown did not cancel the stream")
        .unwrap()
        .unwrap();
    assert!(!response.text().await.unwrap_or_default().contains("[DONE]"));
}

#[tokio::test]
async fn streaming_blocks_keys_split_across_events_and_preserves_complete_normal_sse() {
    for protocol in [ApiProtocol::Openai, ApiProtocol::Anthropic] {
        let (head, tail) = TOKEN.split_at(17);
        let event = |text: &str| match protocol {
            ApiProtocol::Openai => format!(
                "data: {}\n\n",
                json!({"choices":[{"delta":{"content":text}}]})
            ),
            ApiProtocol::Anthropic => format!(
                "event: content_block_delta\ndata: {}\n\n",
                json!({"type":"content_block_delta","delta":{"type":"text_delta","text":text}})
            ),
        };
        let reflected = format!("{}{}data: [DONE]\n\n", event(head), event(tail));
        let reply = Reply {
            body: reflected.into_bytes(),
            headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
            ..Reply::json(Value::Null)
        };
        let normal = format!("{}data: [DONE]\n\n", event("normal answer"));
        let ok = Reply {
            body: normal.clone().into_bytes(),
            headers: vec![("Content-Type".to_owned(), "text/event-stream".to_owned())],
            ..Reply::json(Value::Null)
        };
        let (mut fixture, _, _) =
            bound_fixture(protocol, SourceFormat::AndroidApiKey, vec![reply, ok]).await;
        let (stop, task) = start(&mut fixture);
        let path = if protocol == ApiProtocol::Openai {
            "/v1/chat/completions"
        } else {
            "/v1/messages"
        };
        let http = reqwest::Client::new();
        let response = http
            .post(local_url(&fixture, path))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture","stream":true}))
            .send()
            .await
            .unwrap();
        let bytes = response.bytes().await.unwrap_or_default();
        assert!(!String::from_utf8_lossy(&bytes).contains(head));
        let response = http
            .post(local_url(&fixture, path))
            .bearer_auth(local_key(&fixture))
            .json(&json!({"model":"fixture","stream":true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), normal);
        stop.send(()).unwrap();
        task.await.unwrap().unwrap();
    }
}
