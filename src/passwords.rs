//! Android login adapter. Secret documents never implement Debug or Serialize;
//! local management returns metadata, and MCP has no route into this module.
use mdbx_core::model::ObjectSummary;
use mdbx_storage::repo::{
    CommitContext, OperationCoordinator, WriteCommand, WriteOperationRequest,
};
use serde::Serialize;
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    config::ConfigStore,
    error::{GatewayError, Result},
    keys::id::{physical_entry_id, root_collection_id},
    vault::{RevealPurpose, Vault},
};

pub(crate) const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

/// Wipe owned JSON strings, including unknown field names and nested fields.
pub(crate) struct Document {
    pub summary: ObjectSummary,
    pub value: Value,
    pub android_identity: bool,
}

fn wipe(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(wipe),
        Value::Object(fields) => {
            for (mut key, mut value) in std::mem::take(fields) {
                key.zeroize();
                wipe(&mut value);
            }
        }
        _ => {}
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        wipe(&mut self.value);
    }
}

/// Native metadata only. No usernames, notes, OTP, custom fields or passwords.
#[derive(Serialize)]
pub struct Summary {
    pub id: String,
    pub logical_id: Option<String>,
    pub category: String,
    pub title: String,
    pub kind: String,
    pub payload_schema_version: u32,
    pub head_commit_id: String,
    pub android_roundtrip_identity: bool,
}

impl Document {
    fn metadata(&self) -> Summary {
        Summary {
            id: self.summary.object_id.clone(),
            // A malformed logical identifier is not public metadata.
            logical_id: self
                .android_identity
                .then(|| self.value["monica_entry_id"].as_str().unwrap().to_owned()),
            category: self.summary.collection_id.clone(),
            title: String::from_utf8_lossy(self.summary.title.as_deref().unwrap_or_default())
                .into_owned(),
            kind: "login".into(),
            payload_schema_version: self.summary.payload_schema_version,
            head_commit_id: self.summary.head_commit_id.clone(),
            android_roundtrip_identity: self.android_identity,
        }
    }
}

fn parse(text: &str) -> Result<Value> {
    if text.len() > MAX_PAYLOAD_BYTES {
        return Err(GatewayError::ObjectPayloadTooLarge);
    }
    let value = mdbx_core::json::from_str(text).map_err(|_| GatewayError::InvalidRequest)?;
    if !value.is_object() {
        return Err(GatewayError::InvalidRequest);
    }
    Ok(value)
}

fn encode(value: &Value) -> Result<Zeroizing<String>> {
    let text =
        Zeroizing::new(serde_json::to_string(value).map_err(|_| GatewayError::InvalidRequest)?);
    if text.len() > MAX_PAYLOAD_BYTES {
        return Err(GatewayError::ObjectPayloadTooLarge);
    }
    Ok(text)
}

/// Fields are replacements, not a JSON Merge Patch: null is kept as null and
/// absence means leave the existing field byte semantics unchanged.
fn merge(value: &mut Value, text: &str) -> Result<()> {
    let mut patch = parse(text)?;
    let result = (|| {
        for (key, field) in patch.as_object().unwrap() {
            let valid = match key.as_str() {
                "website" | "username" | "password_plain" | "notes" | "authenticator_key"
                | "passkey_bindings" | "ssh_key_data" => field.is_string(),
                "app_package_name"
                | "app_name"
                | "password_group_id"
                | "bound_note_entry_id"
                | "email"
                | "phone"
                | "address_line"
                | "city"
                | "state"
                | "zip_code"
                | "country"
                | "credit_card_number_plain"
                | "credit_card_holder"
                | "credit_card_expiry"
                | "credit_card_cvv_plain" => field.is_string() || field.is_null(),
                "wifi_metadata" => field.is_string() || field.is_object() || field.is_null(),
                "sort_order" => field.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
                "custom_fields" => field.as_array().is_some_and(|fields| {
                    fields.iter().all(|f| {
                        f.is_object()
                            && f["title"].is_string()
                            && f["value"].is_string()
                            && f["is_protected"].is_boolean()
                            && f["sort_order"]
                                .as_i64()
                                .is_some_and(|n| i32::try_from(n).is_ok())
                    })
                }),
                _ => false,
            };
            if !valid {
                return Err(GatewayError::InvalidRequest);
            }
        }
        if patch.get("password_plain").is_some() {
            value["monica_password_encoding"] = json!("plaintext-v1");
        }
        for (key, field) in patch.as_object().unwrap() {
            if let Some(old) = value.as_object_mut().unwrap().get_mut(key) {
                wipe(old);
            }
            value[key] = field.clone();
        }
        Ok(())
    })();
    wipe(&mut patch);
    result
}

impl Vault {
    pub(crate) fn password_document(&self, id: &str) -> Result<Document> {
        let disclosed = self.reveal(
            id,
            RevealPurpose::PasswordAdmin,
            chrono::Utc::now().timestamp(),
        )?;
        let text =
            std::str::from_utf8(&disclosed.payload).map_err(|_| GatewayError::ObjectReadOnly)?;
        let mut value = parse(text).map_err(|_| GatewayError::ObjectReadOnly)?;
        let login_type = value
            .get("login_type")
            .map(Value::as_str)
            .unwrap_or(Some("PASSWORD"));
        let recognized =
            value["kind"] == "password" && matches!(login_type, Some("PASSWORD" | "WIFI" | "SSO"));
        if !recognized {
            wipe(&mut value);
            return Err(GatewayError::ObjectReadOnly);
        }
        let vault_id = self.vault_id()?;
        let android_identity = value["monica_entry_id"].as_str().is_some_and(|logical| {
            logical
                .strip_prefix("password:")
                .is_some_and(|suffix| !suffix.is_empty())
                && physical_entry_id(&vault_id, logical).to_string() == id
        });
        Ok(Document {
            summary: disclosed.snapshot,
            value,
            android_identity,
        })
    }

    pub fn password_summary(&self, id: &str) -> Result<Summary> {
        Ok(self.password_document(id)?.metadata())
    }

    /// Caller supplies a UUID once; retrying it uses the same logical and native
    /// IDs and the engine's durable operation receipt, even after reopening.
    pub fn create_password(
        &self,
        id: uuid::Uuid,
        category: Option<&str>,
        title: &str,
        fields: &str,
    ) -> Result<Summary> {
        crate::model::validate_title(title)?;
        let vault_id = self.vault_id()?;
        let logical = format!("password:{id}");
        let physical = physical_entry_id(&vault_id, &logical).to_string();
        let root = root_collection_id(&vault_id).to_string();
        let category = category.unwrap_or(&root);
        if !self.library()?.categories.iter().any(|c| c.id == category) {
            return Err(GatewayError::NotFound);
        }
        let mut value = json!({"kind":"password", "monica_entry_id":logical, "login_type":"PASSWORD",
            "monica_password_encoding":"plaintext-v1", "sort_order":0, "custom_fields":[],
            "bitwarden_mode":false, "keepass_mode":false});
        for key in [
            "website",
            "username",
            "password_plain",
            "notes",
            "app_package_name",
            "app_name",
            "ssh_key_data",
            "authenticator_key",
            "passkey_bindings",
            "email",
            "phone",
            "address_line",
            "city",
            "state",
            "zip_code",
            "country",
            "credit_card_number_plain",
            "credit_card_holder",
            "credit_card_expiry",
            "credit_card_cvv_plain",
            "wifi_metadata",
        ] {
            value[key] = json!("");
        }
        let result = (|| {
            merge(&mut value, fields)?;
            if category != root {
                value["mdbx_folder_id"] = json!(category);
            }
            let payload = encode(&value)?;
            let conn = self
                .runtime
                .write()
                .map_err(|_| GatewayError::StateUnavailable)?;
            OperationCoordinator::execute(
                &conn,
                &CommitContext::new("monica-pass-admin".into()),
                WriteOperationRequest::new(
                    format!("password-create:{id}"),
                    "password-create",
                    vec![WriteCommand::CreateEntry {
                        entry_id: physical.clone(),
                        project_id: category.into(),
                        entry_type: "login".into(),
                        title: title.into(),
                        payload_json: payload.to_string(),
                    }],
                ),
            )
            .map_err(|_| GatewayError::ObjectChanged)?;
            Ok(())
        })();
        wipe(&mut value);
        result?;
        self.password_summary(&physical)
    }

    /// Read/merge/write keeps every field outside this explicit patch. A native
    /// head supplied by the caller prevents a delayed edit overwriting new work.
    pub fn edit_password(
        &self,
        id: &str,
        expected_head: &str,
        title: Option<&str>,
        fields: &str,
    ) -> Result<Summary> {
        if let Some(title) = title {
            crate::model::validate_title(title)?;
        }
        let mut document = self.password_document(id)?;
        if !document.android_identity {
            return Err(GatewayError::ObjectReadOnly);
        }
        if document.summary.head_commit_id != expected_head {
            return Err(GatewayError::ObjectChanged);
        }
        merge(&mut document.value, fields)?;
        let payload = encode(&document.value)?;
        let old_title =
            String::from_utf8_lossy(document.summary.title.as_deref().unwrap_or_default());
        self.write_object(
            &document.summary,
            "password-edit",
            WriteCommand::UpdateEntry {
                entry_id: id.into(),
                project_id: document.summary.collection_id.clone(),
                entry_type: "login".into(),
                title: title.unwrap_or(&old_title).into(),
                payload_json: payload.to_string(),
            },
        )?;
        self.password_summary(id)
    }

    /// Both folder representations change in the same engine transaction, with
    /// a stale-head check. Attached entries stay put until the native engine
    /// supports moving attachment ownership together with their entry.
    pub(crate) fn move_password(&self, mut document: Document, target: &str) -> Result<()> {
        if !document.android_identity {
            return Err(GatewayError::ObjectReadOnly);
        }
        if self.is_android_root(target) {
            document
                .value
                .as_object_mut()
                .unwrap()
                .remove("mdbx_folder_id");
        } else {
            document.value["mdbx_folder_id"] = json!(target);
        }
        let payload = encode(&document.value)?;
        let original = &document.summary;
        let mut commands = vec![WriteCommand::UpdateEntry {
            entry_id: original.object_id.clone(),
            project_id: original.collection_id.clone(),
            entry_type: "login".into(),
            title: String::from_utf8_lossy(original.title.as_deref().unwrap_or_default())
                .into_owned(),
            payload_json: payload.to_string(),
        }];
        if original.collection_id != target {
            commands.push(WriteCommand::MoveEntry {
                entry_id: original.object_id.clone(),
                project_id: original.collection_id.clone(),
                target_project_id: target.into(),
            });
        }
        self.write_objects(original, "password-move", commands)
    }
}

pub fn with_vault<T>(
    store: &ConfigStore,
    password: &str,
    action: impl FnOnce(&Vault) -> Result<T>,
) -> Result<T> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = action(&vault);
    vault.lock()?;
    result
}

#[cfg(test)]
mod tests;
