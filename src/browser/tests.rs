use super::*;
use crate::test_support::{PASSWORD, TOKEN};

fn fixture() -> (tempfile::TempDir, BrowserHost) {
    let temp = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(temp.path().join("gateway.json"));
    admin::initialize(
        &store,
        &temp.path().join("test.mdbx"),
        17321,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    (temp, BrowserHost::new(store))
}
fn parse(value: Value) -> Request {
    serde_json::from_value(value).unwrap()
}
async fn mutation(host: &mut BrowserHost, id: uuid::Uuid, mut request: Value) -> Result<Value> {
    request["id"] = json!(1);
    request["protocol"] = json!(PROTOCOL);
    request["operation_id"] = json!(id);
    host.execute(serde_json::from_value(request).unwrap()).await
}

#[tokio::test]
async fn durable_receipts_deduplicate_across_restart_without_request_payloads() {
    let (temp, mut host) = fixture();
    unlock(&mut host).await;
    let id = uuid::Uuid::new_v4();
    let request = json!({"op":"create_category","title":"receipt private title","parent":null});
    let first = mutation(&mut host, id, request.clone()).await.unwrap();
    assert_eq!(first["receipt"]["state"], "succeeded");
    let store = host.store.clone();
    drop(host);
    let mut host = BrowserHost::new(store);
    unlock(&mut host).await;
    let duplicate = mutation(&mut host, id, request).await.unwrap();
    assert_eq!(duplicate["receipt"]["state"], "succeeded");
    assert_eq!(
        host.snapshot().unwrap()["categories"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["title"] == "receipt private title")
            .count(),
        1
    );
    let journal =
        std::fs::read_to_string(temp.path().join("gateway.browser-receipts.json")).unwrap();
    for secret in [PASSWORD, "receipt private title", "capability"] {
        assert!(!journal.contains(secret));
    }
    assert!(
        mutation(
            &mut host,
            id,
            json!({"op":"delete_category","entry":"different"})
        )
        .await
        .is_err()
    );
    host.lock();
    assert!(
        host.dispatch(parse(json!({"op":"operation_status","operation":id})))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn interrupted_intents_stay_unconfirmed_and_never_execute_again() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    let vault = host.with_vault(|v| v.vault_id()).unwrap();
    let id = uuid::Uuid::new_v4();
    {
        let mut journal = receipts::Receipts::open(&host.store).unwrap();
        journal.begin(id, &vault, "create_category").unwrap();
    }
    // Models a killed host after intent fsync, both before and after native commit.
    let reply = mutation(
        &mut host,
        id,
        json!({"op":"create_category","title":"must not be retried","parent":null}),
    )
    .await
    .unwrap();
    assert_eq!(reply["saved"], false);
    assert_eq!(reply["receipt"]["state"], "unconfirmed");
    assert!(
        !host.snapshot().unwrap()["categories"]
            .to_string()
            .contains("must not be retried")
    );
    let status = host
        .dispatch(parse(json!({"op":"operation_status","operation":id})))
        .await
        .unwrap();
    assert_eq!(status["state"], "unconfirmed");
    assert_eq!(
        host.dispatch(parse(
            json!({"op":"operation_status","operation":uuid::Uuid::new_v4()})
        ))
        .await
        .unwrap()["state"],
        "not_recorded"
    );
}

#[tokio::test]
async fn protocol_and_operation_ids_are_required_before_mutation() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    for command in [
        json!({"id":1,"op":"create_category","title":"old client","parent":null}),
        json!({"id":1,"protocol":PROTOCOL,"op":"create_category","title":"missing identity","parent":null}),
    ] {
        assert!(
            host.execute(serde_json::from_value(command).unwrap())
                .await
                .is_err()
        );
    }
    assert_eq!(
        host.dispatch(Request::Hello {}).await.unwrap()["protocol"],
        PROTOCOL
    );
    assert!(
        !host.snapshot().unwrap()["categories"]
            .to_string()
            .contains("old client")
    );
}

#[tokio::test]
async fn human_key_and_grant_management_require_fresh_factors_and_hide_secrets() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    let generate = |password: &str| json!({"op":"generate_ssh","title":"browser-key","category":null,"algorithm":"ed25519","auth":{"password":password}});
    assert!(host.dispatch(parse(generate("wrong"))).await.is_err());
    host.dispatch(parse(generate(PASSWORD))).await.unwrap();
    let public = host.dispatch(parse(json!({"op":"keys"}))).await.unwrap();
    assert_eq!(public["keys"].as_array().unwrap().len(), 1);
    assert!(!public.to_string().contains("PRIVATE"));
    assert!(!public.to_string().contains("public_key"));
    host.dispatch(parse(json!({"op":"add_connection","name":"work","title":"Work","provider":"github","category":null,"token":TOKEN}))).await.unwrap();
    let grant = |password: &str, ttl: u32| json!({"op":"issue_grant","name":"browser-agent","connection":"work","repositories":["example/project"],"operations":["get_issue"],"ttl_minutes":ttl,"requests_per_minute":60,"max_calls":3,"approval":"write","auth":{"password":password}});
    assert!(host.dispatch(parse(grant("wrong", 240))).await.is_err());
    assert!(host.dispatch(parse(grant(PASSWORD, 0))).await.is_err());
    host.dispatch(parse(grant(PASSWORD, 240))).await.unwrap();
    let previous = host.store.load().unwrap().grants[0].clone();
    assert!(
        host.dispatch(parse(
            json!({"op":"renew_grant","name":"browser-agent","auth":{"password":"wrong"}})
        ))
        .await
        .is_err()
    );
    host.dispatch(parse(
        json!({"op":"renew_grant","name":"browser-agent","auth":{"password":PASSWORD}}),
    ))
    .await
    .unwrap();
    let next = host.store.load().unwrap().grants[0].clone();
    assert_ne!(previous.capability_hash, next.capability_hash);
    assert_eq!(next.max_calls, 3);
    assert_eq!(next.approval, previous.approval);
    let metadata = host.snapshot().unwrap().to_string();
    for secret in [PASSWORD, TOKEN, &next.capability_hash] {
        assert!(!metadata.contains(secret));
    }
}

#[tokio::test]
async fn browser_key_file_bytes_authenticate_without_host_path_or_secret_output() {
    let (temp, mut host) = fixture();
    let bytes = Zeroizing::new(vec![7_u8; 64]);
    let factors =
        VaultCredentials::from_key_bytes(Zeroizing::new(PASSWORD.into()), Some(bytes.clone()))
            .unwrap();
    let path = temp.path().join("key-file.mdbx");
    admin::initialize_with(
        &host.store,
        &path,
        17321,
        &factors,
        PASSWORD,
        &admin::NewVault {
            name: None,
            tiga: admin::TigaLevel::Multi,
        },
    )
    .unwrap();
    assert!(
        host.dispatch(parse(json!({"op":"unlock","password":PASSWORD})))
            .await
            .is_err()
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let reply = host
        .dispatch(parse(
            json!({"op":"unlock","password":PASSWORD,"key_file":encoded}),
        ))
        .await
        .unwrap();
    assert!(!reply.to_string().contains(&encoded));
    // Fresh authentication must carry the same second factor, not just a password.
    let generate = |key_file: Option<&str>| json!({"op":"generate_ssh","title":"factor-key","category":null,"algorithm":"ed25519","auth":{"password":PASSWORD,"key_file":key_file}});
    assert!(host.dispatch(parse(generate(None))).await.is_err());
    host.dispatch(parse(generate(Some(&encoded))))
        .await
        .unwrap();
    // Browser sessions pin the live path; portable validation copies must keep
    // the native identity and key factor while allowing their temporary path.
    let remote = crate::webdav_tests::FakeWebDav::new(Default::default()).await;
    crate::sync::publish(&host.store, &remote.client, "vault.mdbx", &factors)
        .await
        .unwrap();
    let session = host.session.as_ref().unwrap();
    crate::sync::synchronize(&host.store, &remote.client, session)
        .await
        .unwrap();
    let requests_before = remote.server.requests().len();
    let saved = host.store.load().unwrap().vault;
    host.store
        .update(|config| {
            let mut config = config.unwrap();
            config.vault = temp.path().join("foreign.mdbx");
            Ok((config, ()))
        })
        .unwrap();
    assert!(matches!(
        crate::sync::synchronize(&host.store, &remote.client, session).await,
        Err(GatewayError::ObjectChanged)
    ));
    assert_eq!(remote.server.requests().len(), requests_before);
    host.store
        .update(|config| {
            let mut config = config.unwrap();
            config.vault = saved;
            Ok((config, ()))
        })
        .unwrap();
    assert!(
        serde_json::from_value::<Request>(
            json!({"op":"unlock","password":PASSWORD,"key_file_path":"C:/arbitrary"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn browser_gpg_import_uses_native_adapter_and_returns_only_fixed_metadata() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    let armor = include_str!("../../tests/fixtures/gpg/ed25519-secret.asc");
    let request = json!({"op":"import_key","title":"browser-gpg","category":null,"format":"openpgp","text":armor,"auth":{"password":PASSWORD}});
    let reply = mutation(&mut host, uuid::Uuid::new_v4(), request)
        .await
        .unwrap();
    assert_eq!(reply["receipt"]["state"], "succeeded");
    let listing = host.dispatch(parse(json!({"op":"keys"}))).await.unwrap();
    assert_eq!(listing["keys"][0]["kind"], "GPG_KEY");
    for forbidden in [
        armor,
        "PRIVATE",
        "secret_armor",
        "public_key",
        "cli-fixture@example.com",
        PASSWORD,
    ] {
        assert!(!listing.to_string().contains(forbidden));
    }
    assert!(serde_json::from_value::<Request>(json!({"op":"sync","path":"https://arbitrary.example/","webdav_password":"x","auth":{"password":PASSWORD}})).is_err());
}
async fn unlock(host: &mut BrowserHost) -> Value {
    host.dispatch(parse(json!({"op":"unlock","password":PASSWORD})))
        .await
        .unwrap()
}
#[test]
fn browser_origin_and_request_grammar_are_closed() {
    for bytes in [vec![], vec![1_u8; 32769]] {
        let factors: Factors = serde_json::from_value(json!({"password":PASSWORD,"key_file":base64::engine::general_purpose::STANDARD.encode(bytes)})).unwrap();
        assert!(matches!(
            factors.credentials(),
            Err(GatewayError::InvalidKeyFile)
        ));
    }
    assert!(valid_origin(
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/"
    ));
    for origin in [
        "https://example.test/",
        "null",
        "chrome-extension://abc/",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/path",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/?x",
    ] {
        assert!(!valid_origin(origin));
    }
    for request in [
        json!({"op":"reveal","entry":"x"}),
        json!({"op":"snapshot","path":"C:/other"}),
        json!({"op":"unlock","password":"x","command":"shell"}),
    ] {
        assert!(serde_json::from_value::<Request>(request).is_err());
    }
    let envelope: Envelope = serde_json::from_value(json!({"id":7,"op":"hello"})).unwrap();
    assert_eq!(envelope.id, 7);
    assert!(
        serde_json::from_value::<Envelope>(json!({"id":7,"op":"hello","unexpected":true})).is_err()
    );
}
#[tokio::test]
async fn locked_wrong_password_expiry_and_foreign_switch_fail_closed() {
    let (_temp, mut host) = fixture();
    assert!(matches!(
        host.dispatch(Request::Snapshot {}).await,
        Err(GatewayError::UnlockRequired)
    ));
    assert!(
        host.dispatch(parse(json!({"op":"unlock","password":"wrong"})))
            .await
            .is_err()
    );
    unlock(&mut host).await;
    host.session.as_mut().unwrap().used = Instant::now() - IDLE;
    assert!(host.dispatch(Request::Snapshot {}).await.is_err());
    assert!(host.session.is_none());
    unlock(&mut host).await;
    host.session.as_mut().unwrap().started = Instant::now() - LIFETIME;
    assert!(host.dispatch(Request::Snapshot {}).await.is_err());
    unlock(&mut host).await;
    host.session.as_mut().unwrap().vault_id = "foreign".into();
    assert!(matches!(
        host.dispatch(Request::Snapshot {}).await,
        Err(GatewayError::ObjectChanged)
    ));
    host.lock();
    assert!(host.session.is_none());
}
#[tokio::test]
async fn browser_password_edits_preserve_unknown_fields_and_reject_stale_heads() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    let folder = host
        .dispatch(parse(
            json!({"op":"create_category","title":"工作","parent":null}),
        ))
        .await
        .unwrap();
    assert_eq!(folder["saved"], true);
    let state = host.snapshot().unwrap();
    let category = state["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["title"] == "工作")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let id = uuid::Uuid::new_v4();
    host.dispatch(parse(json!({"op":"create_password","identity":id,"title":"合成密码","category":category,"fields":json!({"password_plain":"secret-sentinel","username":"private-user","notes":"private-note"}).to_string()}))).await.unwrap();
    let state = host.snapshot().unwrap();
    let serialized = state.to_string();
    for secret in [PASSWORD, "secret-sentinel", "private-user", "private-note"] {
        assert!(!serialized.contains(secret));
    }
    let entry = state["entries"][0]["id"].as_str().unwrap();
    // Simulate a future Android writer without teaching the browser an export
    // operation. Its unknown precise-number fields must survive our patch.
    host.with_vault(|vault| {
        let document=vault.password_document(entry)?;
        let mut value=document.value.clone();
        value["future"]=mdbx_core::json::from_str(r#"{"integer":123456789012345678901234567890,"literal":{"$serde_json::private::Number":"123"},"values":[null,false,""]}"#).unwrap();
        vault.write_object(&document.summary,"synthetic-future-writer",mdbx_storage::repo::WriteCommand::UpdateEntry {
            entry_id:entry.into(),project_id:document.summary.collection_id.clone(),entry_type:"login".into(),
            title:"Future fields".into(),payload_json:serde_json::to_string(&value).unwrap(),
        })
    }).unwrap();
    let info = host
        .dispatch(parse(json!({"op":"password_info","entry":entry})))
        .await
        .unwrap();
    let command = json!({"op":"edit_password","entry":entry,"head":info["head_commit_id"],"title":"Changed","fields":json!({"website":"https://example.test"}).to_string()});
    host.dispatch(parse(command.clone())).await.unwrap();
    assert!(matches!(
        host.dispatch(parse(command)).await,
        Err(GatewayError::ObjectChanged)
    ));
    host.with_vault(|vault| {
        let document = vault.password_document(entry)?;
        assert_eq!(document.value["password_plain"], "secret-sentinel");
        assert_eq!(document.value["notes"], "private-note");
        assert_eq!(document.value["future"],mdbx_core::json::from_str(r#"{"integer":123456789012345678901234567890,"literal":{"$serde_json::private::Number":"123"},"values":[null,false,""]}"#).unwrap());
        Ok(())
    })
    .unwrap();
    host.dispatch(parse(json!({"op":"delete","entry":entry})))
        .await
        .unwrap();
    assert!(
        host.snapshot().unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn native_mutation_checks_the_pinned_vault_inside_the_shared_open() {
    let (temp, mut host) = fixture();
    unlock(&mut host).await;
    // Same password, different database: a caller must not borrow the browser's
    // authenticated session after another local manager switches configuration.
    admin::initialize(
        &host.store,
        &temp.path().join("other.mdbx"),
        17321,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    assert!(matches!(
        host.dispatch(parse(
            json!({"op":"create_category","title":"must not appear","parent":null})
        ))
        .await,
        Err(GatewayError::ObjectChanged)
    ));
    let tree = library::read(&host.store, PASSWORD).unwrap();
    assert!(!tree.categories.iter().any(|c| c.title == "must not appear"));
}

#[tokio::test]
async fn switching_saved_databases_requires_target_password_and_locks_the_session() {
    let (temp, mut host) = fixture();
    let first = host.store.load().unwrap().vault;
    let second = temp.path().join("second.mdbx");
    admin::initialize(
        &host.store,
        &second,
        17321,
        "second-password",
        "second-password",
    )
    .unwrap();
    let saved = databases::list(&host.store)
        .unwrap()
        .into_iter()
        .find(|d| !d.current)
        .unwrap();
    assert!(
        host.dispatch(parse(
            json!({"op":"switch_database","database":saved.id,"password":"wrong"})
        ))
        .await
        .is_err()
    );
    assert_eq!(host.store.load().unwrap().vault, second);
    host.dispatch(parse(
        json!({"op":"switch_database","database":saved.id,"password":PASSWORD}),
    ))
    .await
    .unwrap();
    assert_eq!(host.store.load().unwrap().vault, first);
    assert!(host.session.is_none());
    assert!(first.is_file() && second.is_file());
}
#[tokio::test]
async fn protected_root_nonempty_folders_and_cycles_stay_protected() {
    let (_temp, mut host) = fixture();
    let state = unlock(&mut host).await;
    let root = state["categories"][0]["id"].as_str().unwrap();
    assert!(matches!(
        host.dispatch(parse(json!({"op":"delete_category","entry":root})))
            .await,
        Err(GatewayError::ProtectedCollection)
    ));
    host.dispatch(parse(
        json!({"op":"create_category","title":"child","parent":root}),
    ))
    .await
    .unwrap();
    let state = host.snapshot().unwrap();
    let child = state["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["title"] == "child")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    assert!(
        host.dispatch(parse(json!({"op":"move","entry":child,"target":child})))
            .await
            .is_err()
    );
    host.dispatch(parse(json!({"op":"delete_category","entry":child})))
        .await
        .unwrap();
}
#[tokio::test]
async fn token_rotation_revokes_grants_and_never_returns_credentials() {
    let (_temp, mut host) = fixture();
    unlock(&mut host).await;
    host.dispatch(parse(json!({"op":"add_connection","name":"work","title":"Work","provider":"github","category":null,"token":TOKEN}))).await.unwrap();
    let grant = admin::GrantOptions {
        name: "browser-test".into(),
        connection: "work".into(),
        repositories: vec!["example/project".into()],
        operations: vec![crate::model::Operation::GetIssue],
        ttl_minutes: 240,
        requests_per_minute: 60,
        max_calls: 0,
        approval: crate::model::ApprovalPolicy::Off,
        out: None,
    };
    admin::issue_grant(&host.store, &grant, PASSWORD).unwrap();
    let state = host.snapshot().unwrap();
    assert_eq!(state["grants"].as_array().unwrap().len(), 1);
    for secret in [PASSWORD, TOKEN, "capability_hash"] {
        assert!(!state.to_string().contains(secret));
    }
    host.dispatch(parse(
        json!({"op":"replace_token","name":"work","token":"new-synthetic-token"}),
    ))
    .await
    .unwrap();
    assert!(
        host.snapshot().unwrap()["grants"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(host.store.load().unwrap().connections.len(), 1);
}
#[tokio::test]
async fn framing_accepts_fragmentation_and_rejects_oversize_and_invalid_input() {
    let (_temp, mut host) = fixture();
    let (mut client, server) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(server);
    let task = tokio::spawn(async move { serve(&mut host, input, output).await });
    let bytes = serde_json::to_vec(&json!({"id":42,"op":"hello"})).unwrap();
    for byte in (bytes.len() as u32).to_le_bytes().into_iter().chain(bytes) {
        client.write_all(&[byte]).await.unwrap();
    }
    let size = client.read_u32_le().await.unwrap() as usize;
    let mut reply = vec![0; size];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&reply).unwrap()["id"], 42);
    client
        .write_all(&((MAX_FRAME + 1) as u32).to_le_bytes())
        .await
        .unwrap();
    assert!(task.await.unwrap().is_err());
}
