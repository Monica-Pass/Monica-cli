use super::*;
use crate::model::{Operation, Provider};
use crate::test_support::Fixture;
use mdbx_core::{model::ObjectTypeId, tiga::TigaMode};
use mdbx_storage::{
    backup::BackupService,
    repo::{AttachmentCreateRequest, AttachmentRepo, EntryRepo, ObjectSummaryRepo},
};

const PASSWORD: &str = "Synthetic password alignment 20261005!";
const ENTRY_SECRET: &str = "  C2|literal-user-password\n密码 e\u{301}  ";

fn fixture() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(
        &dir.path().join("passwords.mdbx"),
        PASSWORD,
        TigaMode::Multi,
    )
    .unwrap();
    (dir, vault)
}

fn create(vault: &Vault) -> Summary {
    vault.create_password(uuid::Uuid::new_v4(), None, "Synthetic account", &json!({
        "password_plain":ENTRY_SECRET, "username":"alice", "website":"https://example.invalid",
        "authenticator_key":"otpauth://hotp/test?secret=JBSWY3DPEHPK3PXP&algorithm=SHA512&digits=8&counter=42",
        "passkey_bindings":r#"[{"credentialId":"synthetic","rpId":"example.invalid"}]"#,
        "password_group_id":"synthetic-group", "app_package_name":"com.example.one|com.example.two",
        "custom_fields":[{"title":"问题","value":"hidden-answer","is_protected":true,"sort_order":0}]
    }).to_string()).unwrap()
}

fn native_payload(vault: &Vault, id: &str, value: Value) {
    let original = vault.password_document(id).unwrap();
    vault
        .write_object(
            &original.summary,
            "synthetic-android-writer",
            WriteCommand::UpdateEntry {
                entry_id: id.into(),
                project_id: original.summary.collection_id.clone(),
                entry_type: "login".into(),
                title: "Synthetic account".into(),
                payload_json: encode(&value).unwrap().to_string(),
            },
        )
        .unwrap();
}

#[test]
fn create_edit_portable_reopen_preserves_android_identity_and_secret_bytes() {
    let (dir, vault) = fixture();
    let first = create(&vault);
    let before = vault.password_document(&first.id).unwrap();
    let mut rich = before.value.clone();
    rich["future"] = mdbx_core::json::from_str(r#"{"integer":123456789012345678901234567890,"decimal":1.2345678901234567890123456789,"literal":{"$serde_json::private::Number":"123"},"array":[null,false,""]}"#).unwrap();
    rich["wifi_metadata"] =
        json!(r#"{"security":"future","extra":123456789012345678901234567890}"#);
    rich["ssh_key_data"] = json!(r#"{"schema":"monica.ssh-key.v1","future":{"x":true}}"#);
    rich["customFields"] =
        json!([{"label":"legacy","value":"keep","isProtected":true,"sortOrder":9}]);
    native_payload(&vault, &first.id, rich.clone());
    let head = vault.password_summary(&first.id).unwrap().head_commit_id;
    let edited = vault
        .edit_password(
            &first.id,
            &head,
            Some("Edited account"),
            r#"{"notes":"第一行\nsecond line","email":null,"password_group_id":null}"#,
        )
        .unwrap();
    let after = vault.password_document(&first.id).unwrap();
    for key in [
        "future",
        "wifi_metadata",
        "ssh_key_data",
        "custom_fields",
        "customFields",
        "password_plain",
        "authenticator_key",
        "passkey_bindings",
        "app_package_name",
    ] {
        assert_eq!(after.value[key], rich[key], "preserved {key}");
    }
    assert_eq!(after.value["password_plain"], ENTRY_SECRET);
    assert!(after.value["email"].is_null());
    assert!(after.value["password_group_id"].is_null());
    assert_eq!(edited.logical_id, first.logical_id);
    assert_eq!(edited.id, first.id);
    let public = serde_json::to_string(&edited).unwrap();
    for secret in [ENTRY_SECRET, "hidden-answer", "JBSWY3DPEHPK3PXP", "alice"] {
        assert!(!public.contains(secret));
    }
    assert!(matches!(
        vault.edit_password(&first.id, &head, None, r#"{"username":"stale"}"#),
        Err(GatewayError::ObjectChanged)
    ));
    let expected = after.value.clone();
    vault.lock().unwrap();
    drop(vault);
    let portable = dir.path().join("portable.mdbx");
    BackupService::create_portable_copy_path(&dir.path().join("passwords.mdbx"), &portable)
        .unwrap();
    let reopened = Vault::open(&portable, PASSWORD).unwrap();
    assert_eq!(
        reopened.password_document(&first.id).unwrap().value,
        expected
    );
    assert_eq!(reopened.library().unwrap().entries.len(), 1);
    reopened.lock().unwrap();
}

#[test]
fn moves_are_atomic_and_both_folder_representations_return_to_root() {
    let (_dir, vault) = fixture();
    let first = create(&vault);
    let parent = vault.create_category("Parent", None).unwrap();
    let child = vault.create_category("Child", Some(&parent)).unwrap();
    let original = vault.password_document(&first.id).unwrap();
    let original_head = original.summary.head_commit_id.clone();
    // Update succeeds first, then MoveEntry fails. The entire transaction must roll back.
    assert!(
        vault
            .move_password(original, &uuid::Uuid::new_v4().to_string())
            .is_err()
    );
    assert_eq!(
        vault.password_summary(&first.id).unwrap().head_commit_id,
        original_head
    );
    assert!(
        vault
            .password_document(&first.id)
            .unwrap()
            .value
            .get("mdbx_folder_id")
            .is_none()
    );
    vault.move_library_item(&first.id, &child).unwrap();
    let moved = vault.password_document(&first.id).unwrap();
    assert_eq!(moved.summary.collection_id, child);
    assert_eq!(moved.value["mdbx_folder_id"], child);
    assert_eq!(moved.value["password_plain"], ENTRY_SECRET);
    vault.move_library_item(&first.id, &first.category).unwrap();
    let root = vault.password_document(&first.id).unwrap();
    assert_eq!(root.summary.collection_id, first.category);
    assert!(root.value.get("mdbx_folder_id").is_none());
    vault.delete_entry(&first.id).unwrap();
    assert!(vault.library().unwrap().entries.is_empty());
    assert!(vault.password_document(&first.id).is_err());
    // Recovery is a native restore, never an isDeleted payload patch.
    vault.key_write_for_password_test(vec![WriteCommand::RestoreEntry {
        entry_id: first.id.clone(),
        project_id: first.category.clone(),
    }]);
    assert_eq!(
        vault.password_document(&first.id).unwrap().value["password_plain"],
        ENTRY_SECRET
    );
    vault.lock().unwrap();
}

#[test]
fn attached_passwords_remain_editable_but_moves_cannot_strand_attachment_ownership() {
    let (_dir, vault) = fixture();
    let first = create(&vault);
    let target = vault.create_category("Target", None).unwrap();
    let attachment = {
        let conn = vault.runtime.read().unwrap();
        let ctx = CommitContext::new("synthetic-password-attachment".into());
        let attachment = AttachmentRepo::add_with_request(
            &conn,
            &ctx,
            AttachmentCreateRequest {
                project_id: &first.category,
                entry_id: Some(&first.id),
                file_name: "合成附件.txt",
                media_type: Some("text/plain"),
                content_hash: "",
                original_size: 0,
            },
        )
        .unwrap();
        AttachmentRepo::write_inline_content(
            &conn,
            &ctx,
            &attachment.attachment_id,
            b"synthetic attachment",
        )
        .unwrap();
        attachment.attachment_id
    };
    vault
        .edit_password(
            &first.id,
            &first.head_commit_id,
            None,
            r#"{"notes":"updated"}"#,
        )
        .unwrap();
    let before = vault.password_document(&first.id).unwrap();
    for deleted in [false, true] {
        if deleted {
            AttachmentRepo::soft_delete(
                &vault.runtime.read().unwrap(),
                &CommitContext::new("synthetic-delete".into()),
                &attachment,
            )
            .unwrap();
        }
        assert!(matches!(
            vault.move_library_item(&first.id, &target),
            Err(GatewayError::AttachmentMoveUnsupported)
        ));
        let after = vault.password_document(&first.id).unwrap();
        assert_eq!(before.value, after.value);
        assert_eq!(before.summary, after.summary);
        let conn = vault.runtime.read().unwrap();
        let attached = AttachmentRepo::get_by_id(&conn, &attachment)
            .unwrap()
            .unwrap();
        assert_eq!(attached.project_id, first.category);
        assert_eq!(attached.entry_id.as_deref(), Some(first.id.as_str()));
        if !deleted {
            assert_eq!(
                AttachmentRepo::read_content(&conn, &attachment).unwrap(),
                b"synthetic attachment"
            );
        }
    }
    vault.lock().unwrap();
}

#[test]
fn legacy_password_bytes_and_wifi_sso_extensions_survive_unrelated_edits() {
    let (_dir, vault) = fixture();
    let first = create(&vault);
    for login_type in ["PASSWORD", "WIFI", "SSO"] {
        let mut value = vault.password_document(&first.id).unwrap().value.clone();
        value.as_object_mut().unwrap().remove("password_plain");
        value
            .as_object_mut()
            .unwrap()
            .remove("monica_password_encoding");
        value["password"] = json!("C2|synthetic-legacy-local-ciphertext");
        value["login_type"] = json!(login_type);
        value["wifi_metadata"] = json!({"future":true,"ssid":"合成 Wi-Fi"});
        value["ssoProvider"] = json!("synthetic-provider");
        native_payload(&vault, &first.id, value);
        let head = vault.password_summary(&first.id).unwrap().head_commit_id;
        vault
            .edit_password(&first.id, &head, None, r#"{"notes":"changed"}"#)
            .unwrap();
        let preserved = vault.password_document(&first.id).unwrap();
        assert!(preserved.value.get("password_plain").is_none());
        assert!(preserved.value.get("monica_password_encoding").is_none());
        assert_eq!(
            preserved.value["password"],
            "C2|synthetic-legacy-local-ciphertext"
        );
        assert_eq!(preserved.value["login_type"], login_type);
        assert_eq!(preserved.value["ssoProvider"], "synthetic-provider");
        assert_eq!(preserved.value["wifi_metadata"]["future"], true);
    }
    let head = vault.password_summary(&first.id).unwrap().head_commit_id;
    vault
        .edit_password(
            &first.id,
            &head,
            None,
            r#"{"password_plain":"C2|literal-replacement"}"#,
        )
        .unwrap();
    let replaced = vault.password_document(&first.id).unwrap();
    assert_eq!(replaced.value["password_plain"], "C2|literal-replacement");
    assert_eq!(replaced.value["monica_password_encoding"], "plaintext-v1");
    let mut future = replaced.value.clone();
    future["login_type"] = json!("FUTURE_LOGIN_KIND");
    native_payload(&vault, &first.id, future);
    assert!(matches!(
        vault.editable_object(&first.id),
        Err(GatewayError::ObjectReadOnly)
    ));
    assert!(matches!(
        vault.delete_entry(&first.id),
        Err(GatewayError::ObjectReadOnly)
    ));
    vault.lock().unwrap();
}

impl Vault {
    fn key_write_for_password_test(&self, commands: Vec<WriteCommand>) {
        OperationCoordinator::execute(
            &self.runtime.read().unwrap(),
            &CommitContext::new("synthetic-restore".into()),
            WriteOperationRequest::new(uuid::Uuid::new_v4().to_string(), "restore", commands),
        )
        .unwrap();
    }
}

#[test]
fn fixed_create_id_retries_without_duplicates_and_groups_do_not_define_identity() {
    let (dir, vault) = fixture();
    let id = uuid::Uuid::new_v4();
    let fields = r#"{"password_plain":"first","password_group_id":"same-group"}"#;
    let first = vault
        .create_password(id, None, "Same title", fields)
        .unwrap();
    let retry = vault
        .create_password(id, None, "Same title", fields)
        .unwrap();
    assert_eq!(first.head_commit_id, retry.head_commit_id);
    assert!(
        vault
            .create_password(id, None, "Different", fields)
            .is_err()
    );
    vault
        .create_password(uuid::Uuid::new_v4(), None, "Same title", fields)
        .unwrap();
    assert_eq!(vault.library().unwrap().entries.len(), 2);
    vault.lock().unwrap();
    drop(vault);
    let reopened = Vault::open(&dir.path().join("passwords.mdbx"), PASSWORD).unwrap();
    assert_eq!(
        reopened
            .create_password(id, None, "Same title", fields)
            .unwrap()
            .head_commit_id,
        first.head_commit_id
    );
    let (_other_dir, other) = fixture();
    let second_vault = other
        .create_password(id, None, "Same title", fields)
        .unwrap();
    assert_ne!(second_vault.id, first.id);
    other.lock().unwrap();
    reopened.lock().unwrap();
}

#[test]
fn malformed_unknown_and_non_android_logins_never_gain_an_edit_adapter() {
    let (_dir, vault) = fixture();
    let category = root_collection_id(&vault.vault_id().unwrap()).to_string();
    let conn = vault.runtime.read().unwrap();
    let native = EntryRepo::create(
        &conn,
        &CommitContext::new("synthetic-native".into()),
        &category,
        ObjectTypeId::Login,
        Some("Native"),
        &json!({"kind":"password","password_plain":"hidden"}),
    )
    .unwrap();
    drop(conn);
    assert!(
        !vault
            .password_summary(&native.entry_id)
            .unwrap()
            .android_roundtrip_identity
    );
    assert!(matches!(
        vault.editable_object(&native.entry_id),
        Err(GatewayError::ObjectReadOnly)
    ));
    let first = create(&vault);
    let doc = vault.password_document(&first.id).unwrap();
    for fields in [
        r#"{"monica_entry_id":"password:replacement"}"#,
        r#"{"mdbx_folder_id":"root"}"#,
        r#"{"login_type":"GPG_KEY"}"#,
        r#"{"password_plain":null}"#,
        r#"{"custom_fields":"[]"}"#,
        r#"{"sort_order":2147483648}"#,
    ] {
        assert!(matches!(
            vault.edit_password(&first.id, &doc.summary.head_commit_id, None, fields),
            Err(GatewayError::InvalidRequest)
        ));
    }
    assert_eq!(
        vault.password_summary(&first.id).unwrap().head_commit_id,
        doc.summary.head_commit_id
    );
    vault
        .edit_password(
            &first.id,
            &doc.summary.head_commit_id,
            None,
            r#"{"password_plain":"","custom_fields":[]}"#,
        )
        .unwrap();
    let cleared = vault.password_document(&first.id).unwrap();
    assert_eq!(cleared.value["password_plain"], "");
    assert_eq!(cleared.value["custom_fields"], json!([]));
    vault.lock().unwrap();
}

#[tokio::test]
async fn gateway_cannot_use_an_android_password_as_a_service_token() {
    let fixture = Fixture::new(Provider::Github, &[Operation::ListIssues], vec![]).await;
    let password = create(&fixture.vault);
    fixture
        .store
        .update(|config| {
            let mut config = config.unwrap();
            let name = config.grants[0].connection.clone();
            config.connections.get_mut(&name).unwrap().credential_id = password.id.clone();
            config.grants[0].connection_fingerprint =
                crate::config::connection_fingerprint(&config.connections[&name]);
            Ok((config, ()))
        })
        .unwrap();
    let result = fixture
        .gateway
        .call(
            &fixture.capability,
            fixture.call(
                Operation::ListIssues,
                json!({"repository":crate::test_support::REPOSITORY}),
            ),
        )
        .await;
    assert!(matches!(result, Err(GatewayError::CredentialUnavailable)));
    assert!(fixture.upstream.requests().is_empty());
    assert!(
        fixture
            .gateway
            .discovery_access(&fixture.capability)
            .await
            .is_err()
    );
    assert!(
        ObjectSummaryRepo::get(&fixture.vault.runtime.read().unwrap(), &password.id)
            .unwrap()
            .is_some()
    );
}

#[test]
#[ignore = "exports synthetic fixture for Android password adapter instrumentation"]
fn export_android_password_fixture() {
    let root = std::path::PathBuf::from(std::env::var_os("MONICA_PASSWORD_FIXTURE_DIR").unwrap());
    assert!(!root.exists());
    std::fs::create_dir_all(&root).unwrap();
    let vault = Vault::create(&root.join("fixture.mdbx"), PASSWORD, TigaMode::Multi).unwrap();
    let first = create(&vault);
    let second = create(&vault);
    let parent = vault.create_category("Password parent", None).unwrap();
    let child = vault
        .create_category("Password child", Some(&parent))
        .unwrap();
    vault.move_library_item(&first.id, &child).unwrap();
    let first_doc = vault.password_document(&first.id).unwrap();
    let second_doc = vault.password_document(&second.id).unwrap();
    let android_logical = format!("password:{}", uuid::Uuid::new_v4());
    let manifest = json!({"password":PASSWORD,"vault_id":vault.vault_id().unwrap(),"parent":parent,"child":child,
        "objects":[{"id":first.id,"logical_id":first.logical_id,"category":child,"payload":first_doc.value},
            {"id":second.id,"logical_id":second.logical_id,"category":second.category,"payload":second_doc.value}],
        "android_logical":android_logical,"android_id":physical_entry_id(&vault.vault_id().unwrap(),&android_logical).to_string()});
    std::fs::write(
        root.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    vault.lock().unwrap();
}

#[test]
#[ignore = "requires independently returned Android password adapter fixture"]
fn verify_android_password_return() {
    let root = std::path::PathBuf::from(std::env::var_os("MONICA_PASSWORD_RETURN_DIR").unwrap());
    let manifest =
        mdbx_core::json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["password"], PASSWORD);
    let path = root.join("android-return.mdbx");
    let vault = Vault::open(&path, PASSWORD).unwrap();
    assert_eq!(vault.vault_id().unwrap(), manifest["vault_id"]);
    assert_eq!(vault.library().unwrap().entries.len(), 3);
    for (index, expected) in manifest["objects"].as_array().unwrap().iter().enumerate() {
        let doc = vault
            .password_document(expected["id"].as_str().unwrap())
            .unwrap();
        assert!(doc.android_identity);
        assert_eq!(doc.summary.collection_id, expected["category"]);
        for (key, value) in expected["payload"].as_object().unwrap() {
            let expected_value = if index == 0 && key == "username" {
                json!("android-edited-user")
            } else if index == 0 && key == "notes" {
                json!("Android 第一行\n第二行")
            } else {
                value.clone()
            };
            assert_eq!(
                doc.value[key], expected_value,
                "Android roundtrip field {key}"
            );
        }
    }
    let id = manifest["android_id"].as_str().unwrap();
    let original = vault.password_document(id).unwrap();
    assert_eq!(
        original.value["password_plain"],
        "synthetic-android-created-password"
    );
    assert_eq!(
        original.value["monica_entry_id"],
        manifest["android_logical"]
    );
    let head = original.summary.head_commit_id.clone();
    vault
        .edit_password(
            id,
            &head,
            None,
            r#"{"username":"cli-edited-android-user","notes":"CLI 原样回写\n下一行"}"#,
        )
        .unwrap();
    assert_eq!(vault.library().unwrap().entries.len(), 3);
    vault.lock().unwrap();
    drop(vault);
    BackupService::create_portable_copy_path(&path, &root.join("cli-return.mdbx")).unwrap();
}
