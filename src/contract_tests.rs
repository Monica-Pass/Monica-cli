//! Synthetic fixtures shared with the independently packaged Android runtime.
use crate::vault::Vault;
use mdbx_core::json::{from_slice, from_str};
use mdbx_core::tiga::TigaMode;
use mdbx_storage::{
    backup::BackupService,
    repo::attachment::{AttachmentCreateRequest, AttachmentWriteOptions},
    repo::{AttachmentRepo, CommitContext, EntryRepo},
};
use serde_json::{Value, json};

const PASSWORD: &str = "Synthetic cross-client contract 20260928!";

/// Run explicitly after the independent Android instrumentation has returned its
/// portable files. Only public fixture paths come from the environment.
#[test]
#[ignore = "requires independent Android synthetic output; see compatibility validation guide"]
fn android_returned_contract() {
    let root = std::path::PathBuf::from(
        std::env::var_os("MONICA_CONTRACT_RETURN_DIR").expect("synthetic return directory"),
    );
    let manifest: Value = from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["password"], PASSWORD);
    for name in ["android-fixture.mdbx", "android-portable.mdbx"] {
        let source = root.join(name);
        let original_bytes = std::fs::read(&source).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let working = temporary.path().join("return.mdbx");
        BackupService::create_portable_copy_path(&source, &working).unwrap();
        crate::blobs::copy_all(&source, &working).unwrap();
        let vault = Vault::open(&working, PASSWORD).unwrap();
        let library = vault.library().unwrap();
        assert_eq!(library.entries.len(), 5);
        assert!(library.categories.iter().any(|category| {
            category.id == manifest["child"].as_str().unwrap()
                && category.parent.as_deref() == manifest["parent"].as_str()
        }));
        let expectations = expected_objects(&manifest);
        for expected in &expectations {
            assert!(
                entry_matches(&vault, expected),
                "returned object differs from independent contract: {}",
                expected.id
            );
        }
        let deleted = mdbx_storage::repo::ObjectSummaryRepo::get(
            &vault.runtime.read().unwrap(),
            manifest["android"]["deleted"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert!(deleted.deleted);
        assert_eq!(deleted.object_type_id.as_str(), "login");
        assert_eq!(deleted.collection_id, manifest["child"].as_str().unwrap());
        crate::blobs::verify(&vault).unwrap();
        let expected_bytes: Vec<u8> = manifest["attachment"]["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect();
        let conn = vault.runtime.read().unwrap();
        let mut actual = Vec::new();
        AttachmentRepo::read_content_to_writer_with_blob_store(
            &conn,
            manifest["attachment"]["id"].as_str().unwrap(),
            &conn.external_blob_store().unwrap(),
            &mut actual,
        )
        .unwrap();
        assert_eq!(actual, expected_bytes);
        drop(conn);
        // Corrupt only this disposable working copy. The same verifier must
        // reject lost fields, wrong versions and wrong collections.
        let editable = expectations
            .iter()
            .find(|item| item.title == "CLI fixture edited by Android")
            .unwrap();
        let ctx = CommitContext::new("negative-contract-control".into());
        let conn = vault.runtime.read().unwrap();
        let original = EntryRepo::get_by_id(&conn, &editable.id).unwrap().unwrap();
        let mut changed = original.clone();
        let mut payload = from_slice(&changed.payload_ct).unwrap();
        payload.as_object_mut().unwrap().remove("extra");
        changed.payload_ct = serde_json::to_vec(&payload).unwrap();
        EntryRepo::update(&conn, &ctx, &changed).unwrap();
        drop(conn);
        assert!(
            !entry_matches(&vault, editable),
            "lost extension was accepted"
        );
        let conn = vault.runtime.read().unwrap();
        changed = original.clone();
        changed.payload_schema_version = 8;
        EntryRepo::update(&conn, &ctx, &changed).unwrap();
        drop(conn);
        assert!(
            !entry_matches(&vault, editable),
            "wrong version was accepted"
        );
        let conn = vault.runtime.read().unwrap();
        EntryRepo::update(&conn, &ctx, &original).unwrap();
        EntryRepo::move_to_project(
            &conn,
            &ctx,
            &editable.id,
            manifest["parent"].as_str().unwrap(),
        )
        .unwrap();
        drop(conn);
        assert!(
            !entry_matches(&vault, editable),
            "wrong collection was accepted"
        );
        vault.lock().unwrap();
        assert_eq!(std::fs::read(source).unwrap(), original_bytes);
    }
}

#[test]
fn synthetic_cross_client_fixture_and_portable_copy() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("original.mdbx");
    let vault = Vault::create(&source, PASSWORD, TigaMode::Multi).unwrap();
    let parent = vault.create_category("父分类", None).unwrap();
    let child = vault.create_category("子分类", Some(&parent)).unwrap();
    let future: Value = from_str(r#"{"schema":"com.example.recovery-kit.v9","codes":["synthetic-one","二",null],"metadata":{"literal":{"$serde_json::private::Number":"123"},"raw":{"$serde_json::private::RawValue":"null"},"enabled":false,"counter":123456789012345678901234567890,"fraction":1.2345678901234567890123456789,"empty":""},"empty":{},"null":null}"#).unwrap();
    let token = json!({"schema":"monica.gateway.credential.v1","provider":"github","api_base":"https://api.github.com","token":"synthetic-no-service-access-token","extra":future});
    let mut ids = Vec::new();
    for (kind, version, payload, title) in [
        ("com.example.recovery-kit", 1, &future, "未知类型"),
        ("api-token", 7, &token, "未来令牌"),
        ("api-token", 1, &token, "可编辑令牌"),
        ("login", 9, &future, "未来登录"),
    ] {
        let entry = EntryRepo::create_with_payload_schema_version(
            &vault.runtime.read().unwrap(),
            &CommitContext::new("cli-fixture".into()),
            &child,
            kind.parse().unwrap(),
            Some(title),
            payload,
            version,
        )
        .unwrap();
        ids.push(entry.entry_id);
    }
    let binding = crate::config::Connection {
        api_key: None,
        provider: crate::model::Provider::Github,
        credential_id: ids[2].clone(),
        api_base: "https://api.github.com".into(),
        note: String::new(),
    };
    vault
        .edit_credential("fixture", &binding, "CLI edited", None)
        .unwrap();
    vault.move_library_item(&ids[2], &parent).unwrap();
    let attachment = {
        let conn = vault.runtime.read().unwrap();
        let ctx = CommitContext::new("cli-fixture".into());
        let bytes = "synthetic 附件\0内容".repeat(60).into_bytes();
        let attachment = AttachmentRepo::add_with_request(
            &conn,
            &ctx,
            AttachmentCreateRequest {
                project_id: &child,
                entry_id: Some(&ids[0]),
                file_name: "跨端附件.bin",
                media_type: Some("application/octet-stream"),
                content_hash: "",
                original_size: bytes.len() as u64,
            },
        )
        .unwrap();
        AttachmentRepo::write_external_content_from_reader_with_options(
            &conn,
            &ctx,
            &attachment.attachment_id,
            &mut std::io::Cursor::new(&bytes),
            AttachmentWriteOptions::exact(256, bytes.len() as u64),
            &conn.external_blob_store().unwrap(),
        )
        .unwrap();
        json!({"id":attachment.attachment_id,"bytes":bytes,"collection":child,"object":ids[0]})
    };
    let objects: Vec<_> = ids.iter().map(|id| {
        let document = vault.inspect_object(id).unwrap();
        let payload: serde_json::Map<String, Value> = document.fields.iter().map(|(key, value)|
            (key.to_string(), from_str(value).unwrap())).collect();
        json!({"id":id,"type":document.summary.object_type_id.as_str(),"version":document.summary.payload_schema_version,
            "collection":document.summary.collection_id,"title":String::from_utf8(document.summary.title.unwrap()).unwrap(),
            "payload":serde_json::to_string(&payload).unwrap()})
    }).collect();
    let manifest = json!({"password":PASSWORD,"objects":objects,"attachment":attachment,"parent":parent,"child":child,"android":{"id":uuid::Uuid::new_v4().to_string(),"deleted":uuid::Uuid::new_v4().to_string()}});
    vault.lock().unwrap();
    drop(vault);
    let portable = temporary.path().join("portable.mdbx");
    BackupService::create_portable_copy_path(&source, &portable).unwrap();
    crate::blobs::copy_all(&source, &portable).unwrap();
    let independent = Vault::open(&portable, PASSWORD).unwrap();
    crate::blobs::verify(&independent).unwrap();
    assert_eq!(independent.library().unwrap().entries.len(), 4);
    independent.lock().unwrap();
    drop(independent);
    // The path is non-secret test configuration, never a production reveal option.
    if let Some(output) = std::env::var_os("MONICA_CONTRACT_FIXTURE_DIR") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        let fixture = output.join("fixture.mdbx");
        assert!(!fixture.exists(), "use a new fixture output directory");
        BackupService::create_portable_copy_path(&source, &fixture).unwrap();
        crate::blobs::copy_all(&source, &fixture).unwrap();
        std::fs::write(
            output.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let copied = output.join("portable.mdbx");
        BackupService::create_portable_copy_path(&source, &copied).unwrap();
        crate::blobs::copy_all(&source, &copied).unwrap();
    }
}

const ANDROID_PAYLOAD: &str = r#"{"schema":"monica.gateway.credential.v1","name":"android-fixture","provider":"github","api_base":"https://api.github.com","token":"synthetic-android-no-access-token","note":"Android edited","androidExtension":{"null":null,"bits":[0,false],"literal":{"$serde_json::private::Number":"123"},"decimal":1.2345678901234567890123456789}}"#;

#[derive(Clone, Debug, PartialEq)]
struct ExpectedObject {
    id: String,
    kind: String,
    version: u32,
    collection: String,
    title: String,
    payload: Value,
}

fn expected_objects(manifest: &Value) -> Vec<ExpectedObject> {
    let mut objects: Vec<_> = manifest["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|original| {
            let editable = original["type"] == "api-token" && original["version"] == 1;
            let mut payload = from_str(original["payload"].as_str().unwrap()).unwrap();
            if editable {
                payload["note"] = json!("Android roundtrip");
            }
            ExpectedObject {
                id: original["id"].as_str().unwrap().into(),
                kind: original["type"].as_str().unwrap().into(),
                version: original["version"].as_u64().unwrap() as u32,
                collection: if editable {
                    &manifest["child"]
                } else {
                    &original["collection"]
                }
                .as_str()
                .unwrap()
                .into(),
                title: if editable {
                    "CLI fixture edited by Android"
                } else {
                    original["title"].as_str().unwrap()
                }
                .into(),
                payload,
            }
        })
        .collect();
    objects.push(ExpectedObject {
        id: manifest["android"]["id"].as_str().unwrap().into(),
        kind: "api-token".into(),
        version: 1,
        collection: manifest["parent"].as_str().unwrap().into(),
        title: "android-fixture-edited".into(),
        payload: from_str(ANDROID_PAYLOAD).unwrap(),
    });
    objects
}

fn entry_matches(vault: &Vault, expected: &ExpectedObject) -> bool {
    let Ok(actual) = vault.inspect_object(&expected.id) else {
        return false;
    };
    let fields: serde_json::Map<String, Value> = actual
        .fields
        .iter()
        .map(|(key, value)| (key.to_string(), from_str(value).unwrap()))
        .collect();
    actual.summary.object_id == expected.id
        && actual.summary.object_type_id.as_str() == expected.kind
        && actual.summary.payload_schema_version == expected.version
        && actual.summary.collection_id == expected.collection
        && actual.summary.title.as_deref() == Some(expected.title.as_bytes())
        && Value::Object(fields) == expected.payload
}
