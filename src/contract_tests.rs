//! Synthetic fixtures shared with the independently packaged Android runtime.
use crate::vault::Vault;
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
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["password"], PASSWORD);
    for name in ["android-fixture.mdbx", "android-portable.mdbx"] {
        let returned: Value =
            serde_json::from_slice(&std::fs::read(root.join(format!("{name}.json"))).unwrap())
                .unwrap();
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
        for expected in manifest["objects"].as_array().unwrap() {
            let id = expected["id"].as_str().unwrap();
            let document = vault.inspect_object(id).unwrap();
            assert_eq!(document.summary.object_type_id.as_str(), expected["type"]);
            assert_eq!(
                document.summary.payload_schema_version as u64,
                expected["version"].as_u64().unwrap()
            );
            let edited = returned["cliId"] == id;
            let expected_payload = if edited {
                &returned["cliPayload"]
            } else {
                &expected["payload"]
            };
            let expected_collection = if edited {
                &returned["cliCollection"]
            } else {
                &expected["collection"]
            };
            assert_eq!(
                document.summary.collection_id,
                expected_collection.as_str().unwrap()
            );
            let fields: serde_json::Map<String, Value> = document
                .fields
                .iter()
                .map(|(key, value)| (key.to_string(), serde_json::from_str(value).unwrap()))
                .collect();
            assert_eq!(
                Value::Object(fields),
                serde_json::from_str::<Value>(expected_payload.as_str().unwrap()).unwrap()
            );
        }
        let android = vault
            .inspect_object(returned["id"].as_str().unwrap())
            .unwrap();
        assert_eq!(android.summary.collection_id, returned["collection"]);
        assert_eq!(android.summary.object_type_id.as_str(), "api-token");
        let fields: serde_json::Map<String, Value> = android
            .fields
            .iter()
            .map(|(key, value)| (key.to_string(), serde_json::from_str(value).unwrap()))
            .collect();
        assert_eq!(
            Value::Object(fields),
            serde_json::from_str::<Value>(returned["payload"].as_str().unwrap()).unwrap()
        );
        let deleted = mdbx_storage::repo::ObjectSummaryRepo::get(
            &vault.runtime.read().unwrap(),
            returned["deleted"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
        assert!(deleted.deleted);
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
    let future: Value = serde_json::from_str(r#"{"schema":"com.example.recovery-kit.v9","codes":["synthetic-one","二",null],"metadata":{"enabled":false,"counter":123456789012345678901234567890,"fraction":1.2345678901234567890123456789,"empty":""},"empty":{},"null":null}"#).unwrap();
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
            (key.to_string(), serde_json::from_str(value).unwrap())).collect();
        json!({"id":id,"type":document.summary.object_type_id.as_str(),"version":document.summary.payload_schema_version,
            "collection":document.summary.collection_id,"title":String::from_utf8(document.summary.title.unwrap()).unwrap(),
            "payload":serde_json::to_string(&payload).unwrap()})
    }).collect();
    let manifest = json!({"password":PASSWORD,"objects":objects,"attachment":attachment,"parent":parent,"child":child});
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
