//! Cross-client object invariants shared by local adapters and human inspection.
use mdbx_core::model::ObjectSummary;
use mdbx_storage::error::StorageError;
use mdbx_storage::repo::{
    CommitContext, CommitOperation, ObjectSummaryRepo, OperationCoordinator, WriteCommand,
    WriteOperationRequest,
};
use sha2::{Digest, Sha256};

use crate::error::{GatewayError, Result};
use crate::vault::Vault;
use mdbx_core::tiga::{DeviceAssurance, DeviceContext};
use mdbx_storage::object_disclosure::{ObjectDisclosureLimits, ObjectDisclosureService};
use zeroize::Zeroizing;

const INSPECTION_LIMIT: u64 = 4 * 1024 * 1024;

/// Deliberately neither serializable nor printable. Only the human terminal owns it.
pub(crate) struct Inspection {
    pub summary: ObjectSummary,
    pub editable: bool,
    pub fields: Vec<(Zeroizing<String>, Zeroizing<String>)>,
    pub opened: std::time::Instant,
}

impl Vault {
    pub(crate) fn inspect_object(&self, id: &str) -> Result<Inspection> {
        let editable = self.editable_object(id).is_ok();
        let mut conn = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if conn.keyring().is_none() || conn.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let summary = ObjectSummaryRepo::get(&conn, id)
            .map_err(|_| GatewayError::StateUnavailable)?
            .filter(|object| !object.deleted)
            .ok_or(GatewayError::NotFound)?;
        let disclosed = ObjectDisclosureService::reveal_with_active_session_and_limits(
            &mut conn,
            id,
            &DeviceContext {
                device_id: Some("monica-pass-admin".to_owned()),
                assurance: DeviceAssurance::Standard,
                ..Default::default()
            },
            chrono::Utc::now().timestamp(),
            ObjectDisclosureLimits::new(INSPECTION_LIMIT)
                .map_err(|_| GatewayError::StateUnavailable)?,
        )
        .map_err(|error| match error {
            StorageError::ResourceLimit { .. } => GatewayError::ObjectPayloadTooLarge,
            StorageError::NotFound(_) => GatewayError::NotFound,
            _ => GatewayError::UnlockRequired,
        })?;
        if disclosed.object.head_commit_id != summary.head_commit_id
            || disclosed.object.project_id != summary.collection_id
            || disclosed.object.entry_type != summary.object_type_id
            || disclosed.object.payload_schema_version != summary.payload_schema_version
        {
            return Err(GatewayError::ObjectChanged);
        }
        let raw = Zeroizing::new(disclosed.object.payload_ct);
        let text = std::str::from_utf8(&raw).map_err(|_| GatewayError::InvalidVault)?;
        // RawValue keeps numeric lexemes, string-vs-object representations and
        // complete nested values. Never pass numbers through floating point.
        let fields = match serde_json::from_str::<
            std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>,
        >(text)
        {
            Ok(map) if !map.is_empty() => map
                .into_iter()
                .map(|(key, value)| (Zeroizing::new(key), Zeroizing::new(value.get().to_owned())))
                .collect(),
            _ => vec![(
                Zeroizing::new("(raw payload)".to_owned()),
                Zeroizing::new(text.to_owned()),
            )],
        };
        Ok(Inspection {
            summary,
            editable,
            fields,
            opened: std::time::Instant::now(),
        })
    }
}

pub(crate) fn inspect(
    store: &crate::config::ConfigStore,
    password: &str,
    id: &str,
) -> Result<Inspection> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.inspect_object(id);
    vault.lock()?;
    result
}

impl Vault {
    /// The expected head, collection, type and version are checked inside the same
    /// engine transaction as the write. Another process cannot win between them.
    pub(crate) fn write_object(
        &self,
        original: &ObjectSummary,
        intent: &str,
        command: WriteCommand,
    ) -> Result<()> {
        if original.payload_schema_version != 1 {
            return Err(GatewayError::ObjectReadOnly);
        }
        let commit_kind = match &command {
            WriteCommand::MoveEntry { .. } => "move",
            WriteCommand::UpdateEntry { .. } | WriteCommand::DeleteEntry { .. } => "change",
            _ => return Err(GatewayError::InvalidRequest),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let prepared =
            OperationCoordinator::prepare(WriteOperationRequest::new(&id, intent, vec![command]))
                .map_err(|_| GatewayError::StateUnavailable)?;
        let mut hash = Sha256::new();
        hash.update(prepared.intent_hash());
        hash.update(serde_json::to_vec(original).map_err(|_| GatewayError::StateUnavailable)?);
        let operation = CommitOperation::new(
            id,
            intent,
            "main",
            commit_kind,
            prepared.change_scope(),
            prepared.changed_objects().to_vec(),
        )
        .with_intent_hash(hash.finalize().to_vec());
        let conn = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let mut stale = false;
        let result = CommitContext::new("monica-pass-admin".to_owned()).run_operation(
            &conn,
            operation,
            |ctx| {
                let current = ObjectSummaryRepo::get(&conn, &original.object_id)?;
                if current.as_ref() != Some(original) || original.deleted {
                    stale = true;
                    return Err(StorageError::ConstraintViolation(
                        "stale local object edit".to_owned(),
                    ));
                }
                prepared.apply(&conn, ctx)
            },
        );
        result.map(|_| ()).map_err(|_| {
            if stale {
                GatewayError::ObjectChanged
            } else {
                GatewayError::StateUnavailable
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdbx_core::{model::ObjectTypeId, tiga::TigaMode};
    use mdbx_storage::repo::EntryRepo;
    use serde_json::{Value, json};

    fn fixture(kind: &str, version: u32, payload: &Value) -> (tempfile::TempDir, Vault, String) {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("fixture.mdbx"),
            "synthetic-password",
            TigaMode::Multi,
        )
        .unwrap();
        let folder = vault.create_category("fixture", None).unwrap();
        let object = EntryRepo::create_with_payload_schema_version(
            &vault.runtime.read().unwrap(),
            &CommitContext::new("fixture".into()),
            &folder,
            kind.parse::<ObjectTypeId>().unwrap(),
            Some("fixture"),
            payload,
            version,
        )
        .unwrap();
        (directory, vault, object.entry_id)
    }

    #[test]
    fn unknown_and_future_objects_are_discoverable_readable_and_mutation_protected() {
        let original: Value = serde_json::from_str(r#"{"array":[false,0,"",null,"示例二"],"nested":{"large":1234567890123456789012345678901234567890,"precise":1.2345678901234567890123456789}}"#).unwrap();
        for (kind, version) in [
            ("com.example.recovery-kit", 1),
            ("api-token", 9),
            ("login", 9),
        ] {
            let (directory, vault, id) = fixture(kind, version, &original);
            let inspection = vault.inspect_object(&id).unwrap();
            assert!(!inspection.editable);
            assert_eq!(inspection.summary.object_type_id.as_str(), kind);
            assert_eq!(inspection.summary.payload_schema_version, version);
            assert_eq!(inspection.fields.len(), 2);
            let values: serde_json::Map<String, Value> = inspection
                .fields
                .iter()
                .map(|(key, value)| (key.to_string(), serde_json::from_str(value).unwrap()))
                .collect();
            assert_eq!(Value::Object(values), original);
            assert!(
                vault
                    .library()
                    .unwrap()
                    .entries
                    .iter()
                    .any(|object| object.id == id)
            );
            let target = vault.create_category("target", None).unwrap();
            assert!(matches!(
                vault.move_library_item(&id, &target),
                Err(GatewayError::ObjectReadOnly)
            ));
            assert!(matches!(
                vault.delete_entry(&id),
                Err(GatewayError::ObjectReadOnly)
            ));
            assert_eq!(
                vault.inspect_object(&id).unwrap().summary,
                inspection.summary
            );
            vault.lock().unwrap();
            assert!(matches!(
                vault.inspect_object(&id),
                Err(GatewayError::UnlockRequired)
            ));
            drop(vault);
            let reopened =
                Vault::open(&directory.path().join("fixture.mdbx"), "synthetic-password").unwrap();
            assert_eq!(
                reopened.inspect_object(&id).unwrap().summary,
                inspection.summary
            );
        }
    }

    #[test]
    fn gateway_edits_preserve_extensions_absence_and_precise_numbers_and_stale_edits_fail() {
        let original: Value = serde_json::from_str(r#"{"schema":"monica.gateway.credential.v1","provider":"github","api_base":"https://api.github.com","token":"synthetic-token-1234567890","future":{"array":[null,false,"",{"id":"stable","n":123456789012345678901234567890}]}}"#).unwrap();
        let (directory, vault, id) = fixture("api-token", 1, &original);
        let binding = crate::config::Connection {
            api_key: None,
            provider: crate::model::Provider::Github,
            credential_id: id.clone(),
            api_base: "https://api.github.com".into(),
            note: "".into(),
        };
        let before = vault.inspect_object(&id).unwrap().summary;
        vault.rename_entry(&binding, "renamed").unwrap();
        let renamed = vault.inspect_object(&id).unwrap();
        assert!(
            !renamed
                .fields
                .iter()
                .any(|(key, _)| key.as_str() == "note" || key.as_str() == "name")
        );
        let updated = vault
            .edit_credential("fixture", &binding, "changed", None)
            .unwrap();
        let document = vault.inspect_object(&id).unwrap();
        assert_eq!(
            document
                .fields
                .iter()
                .find(|(key, _)| key.as_str() == "future")
                .unwrap()
                .1
                .as_str(),
            serde_json::to_string(&original["future"]).unwrap()
        );
        let stale = vault.write_object(
            &before,
            "stale",
            WriteCommand::UpdateEntry {
                entry_id: id.clone(),
                project_id: before.collection_id.clone(),
                entry_type: "api-token".into(),
                title: "stale".into(),
                payload_json: serde_json::to_string(&original).unwrap(),
            },
        );
        assert!(matches!(stale, Err(GatewayError::ObjectChanged)));
        assert_eq!(vault.inspect_object(&id).unwrap().summary, document.summary);
        vault.lock().unwrap();
        drop(vault);
        let reopened =
            Vault::open(&directory.path().join("fixture.mdbx"), "synthetic-password").unwrap();
        assert_eq!(
            reopened
                .credential(&updated, chrono::Utc::now().timestamp())
                .unwrap()
                .token
                .as_str(),
            original["token"].as_str().unwrap()
        );
    }

    #[test]
    fn generic_reader_preserves_non_object_payloads() {
        let raw = json!([null, "", "控制\u{1b}", {"value":false}]);
        let (_directory, vault, id) = fixture("com.example.raw", 1, &raw);
        let inspected = vault.inspect_object(&id).unwrap();
        assert_eq!(inspected.fields.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&inspected.fields[0].1).unwrap(),
            raw
        );
    }

    #[test]
    fn generic_reader_obeys_disclosure_policy_and_resource_limits() {
        let value = json!({"large": "x".repeat(INSPECTION_LIMIT as usize)});
        let (_directory, vault, id) = fixture("com.example.large", 1, &value);
        assert!(matches!(
            vault.inspect_object(&id),
            Err(GatewayError::ObjectPayloadTooLarge)
        ));
        vault.set_tiga_policy(TigaMode::Power, None).unwrap();
        // Policy is checked before the resource-limit or payload read.
        assert!(matches!(
            vault.inspect_object(&id),
            Err(GatewayError::UnlockRequired)
        ));
    }

    #[test]
    fn large_foreign_api_tokens_stay_visible_without_becoming_gateway_bindings() {
        let (_directory, vault, id) = fixture(
            "api-token",
            1,
            &json!({"schema":"another-app.v1","data":"x".repeat(20000)}),
        );
        assert!(vault.gateway_inventory().unwrap().connections.is_empty());
        let inspected = vault.inspect_object(&id).unwrap();
        assert!(!inspected.editable);
        assert_eq!(
            inspected
                .fields
                .iter()
                .find(|(key, _)| key.as_str() == "data")
                .unwrap()
                .1
                .len(),
            20002
        );
    }
}
