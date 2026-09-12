use std::time::Duration;

use rmcp::model::{CallToolRequestParams, ProtocolVersion};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::config::{ClientConfig, new_capability};
use crate::model::{CONNECTION_CATALOG_TOOL, Operation, Provider};
use crate::protocol::{MAX_REQUEST_BYTES, serve_broker, serve_mcp_io};
use crate::test_support::{Fixture, REPOSITORY, Reply, TOKEN, issue};

#[tokio::test]
async fn protocol_real_mcp_initialize_discover_list_call_and_revoke() {
    for (provider, lifecycle) in [
        (Provider::Github, ClientLifecycleMode::Initialize),
        (
            Provider::Gitlab,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    ] {
        let mut fixture = Fixture::new(
            provider,
            &[Operation::GetIssue, Operation::ListIssues],
            vec![Reply::json(issue(provider, 42))],
        )
        .await;
        let listener = fixture.broker_listener.take().unwrap();
        // A hand-written client file may omit the trailing slash.
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let broker = tokio::spawn(serve_broker(fixture.gateway.clone(), listener, async {
            let _ = stopped.await;
        }));
        let config = ClientConfig {
            version: 1,
            endpoint,
            capability: fixture.capability.to_string(),
        };
        let (server_io, client_io) = tokio::io::duplex(128 * 1024);
        let (read, write) = tokio::io::split(server_io);
        let bridge = tokio::spawn(serve_mcp_io(config, read, write));
        let client = tokio::time::timeout(
            Duration::from_secs(10),
            ().serve_with_lifecycle(client_io, lifecycle),
        )
        .await
        .unwrap()
        .unwrap();
        let info = client.peer_info().unwrap();
        assert_eq!(info.server_info.as_ref().unwrap().name, "monica-pass");
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!(tools.tools.len(), 3);
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| tool.name.starts_with(provider.prefix())
                    || tool.name == CONNECTION_CATALOG_TOOL)
        );
        assert!(
            tools
                .tools
                .iter()
                .all(|tool| tool.annotations.as_ref().unwrap().read_only_hint == Some(true))
        );
        assert_eq!(
            tools.tools[0].input_schema["properties"]["repository"]["enum"],
            json!([REPOSITORY])
        );
        assert_eq!(
            tools.tools[0].input_schema["properties"]["connection"]["enum"],
            json!(["work"])
        );
        assert!(
            !tools.tools[0]
                .input_schema
                .get("required")
                .and_then(|value| value.as_array())
                .is_some_and(|required| required.contains(&json!("repository")))
        );
        let catalog = client
            .call_tool(CallToolRequestParams::new(CONNECTION_CATALOG_TOOL))
            .await
            .unwrap();
        assert_eq!(
            catalog.structured_content.as_ref().unwrap()["connections"][0]["name"],
            "work"
        );
        assert_eq!(
            catalog.structured_content.as_ref().unwrap()["connections"][0]["note"],
            "用于项目 Issue 跟踪"
        );
        let read = client
            .call_tool(
                CallToolRequestParams::new(Operation::GetIssue.tool_name(provider)).with_arguments(
                    json!({"connection":"work", "number":42})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(read.is_error, Some(false));
        assert_eq!(
            read.structured_content.as_ref().unwrap()["issue"]["number"],
            42
        );
        let denied = client.call_tool(CallToolRequestParams::new(Operation::CreateIssue.tool_name(provider))
            .with_arguments(json!({"repository":REPOSITORY, "title":"Denied", "request_id":new_capability().as_str()}).as_object().unwrap().clone())).await.unwrap();
        assert_eq!(denied.is_error, Some(true));
        assert_eq!(
            denied.structured_content.as_ref().unwrap()["error"]["code"],
            "permission_denied"
        );
        fixture
            .store
            .update(|config| {
                let mut config = config.unwrap();
                config.grants.clear();
                Ok((config, ()))
            })
            .unwrap();
        let revoked = client
            .call_tool(
                CallToolRequestParams::new(Operation::GetIssue.tool_name(provider)).with_arguments(
                    json!({"repository":REPOSITORY,"number":42})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(revoked.is_error, Some(true));
        assert_eq!(
            revoked.structured_content.as_ref().unwrap()["error"]["code"],
            "unauthorized"
        );
        assert!(client.list_tools(None).await.is_err());
        let transcript = format!(
            "{}{}{}{}{}{}",
            serde_json::to_string(info.as_ref()).unwrap(),
            serde_json::to_string(&tools).unwrap(),
            serde_json::to_string(&read).unwrap(),
            serde_json::to_string(&denied).unwrap(),
            serde_json::to_string(&revoked).unwrap(),
            serde_json::to_string(&catalog).unwrap()
        );
        assert!(!transcript.contains(TOKEN));
        assert!(!transcript.contains(fixture.capability.as_str()));
        assert_eq!(fixture.upstream.requests().len(), 1);
        client.cancel().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), broker)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn protocol_broker_rejects_browsers_wrong_host_missing_auth_and_bad_bodies() {
    let mut fixture = Fixture::new(Provider::Github, &[Operation::ListIssues], vec![]).await;
    let listener = fixture.broker_listener.take().unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let broker = tokio::spawn(serve_broker(fixture.gateway.clone(), listener, async {
        let _ = stopped.await;
    }));
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = http.get(format!("{base}/v1/tools")).send().await.unwrap();
    assert_eq!(response.status(), 401);
    let response = http
        .get(format!("{base}/v1/tools"))
        .bearer_auth(fixture.capability.as_str())
        .header("Origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = http
        .get(format!("{base}/v1/tools"))
        .bearer_auth(fixture.capability.as_str())
        .header("Host", "rebinding.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = http
        .get(format!("{base}/v1/tools"))
        .bearer_auth(fixture.capability.as_str())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    for bad in [
        json!({"tool":"github_list_issues", "arguments":{"repository":REPOSITORY}, "credential":"must not echo"}),
        json!({"tool":"github_list_issues", "arguments":{"repository":REPOSITORY,"url":"https://untrusted.example"}}),
    ] {
        let response = http
            .post(format!("{base}/v1/call"))
            .bearer_auth(fixture.capability.as_str())
            .json(&bad)
            .send()
            .await
            .unwrap();
        let error: crate::error::Result<serde_json::Value> = response.json().await.unwrap();
        assert_eq!(
            error.unwrap_err(),
            crate::error::GatewayError::InvalidRequest
        );
    }
    let response = http
        .post(format!("{base}/v1/call"))
        .bearer_auth(fixture.capability.as_str())
        .header("Content-Type", "application/json")
        .body(" ".repeat(MAX_REQUEST_BYTES + 1))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    assert!(fixture.upstream.requests().is_empty());
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), broker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn protocol_mcp_codec_closes_on_oversized_frames() {
    let config = ClientConfig {
        version: 1,
        endpoint: "http://127.0.0.1:47831/".to_owned(),
        capability: new_capability().to_string(),
    };
    let (server_io, mut client_io) = tokio::io::duplex(2 * MAX_REQUEST_BYTES);
    let (read, write) = tokio::io::split(server_io);
    let bridge = tokio::spawn(serve_mcp_io(config, read, write));
    client_io
        .write_all(&vec![b'x'; MAX_REQUEST_BYTES + 1])
        .await
        .unwrap();
    client_io.write_all(b"\n").await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}
