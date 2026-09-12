use std::collections::BTreeMap;
use std::path::Path;

use mdbx_core::model::ObjectTypeId;
use mdbx_core::tiga::{DeviceAssurance, DeviceContext, TigaMode};
use mdbx_storage::connection::{PendingVaultCreation, VaultConnection};
use mdbx_storage::error::StorageError;
use mdbx_storage::init::{VaultInitParams, initialize_vault};
use mdbx_storage::object_disclosure::{ObjectDisclosureLimits, ObjectDisclosureService};
use mdbx_storage::repo::{
    CollectionSummaryRepo, CommitContext, ObjectSummaryRepo, OperationCoordinator, WriteCommand,
    WriteOperationRequest,
};
use mdbx_storage::runtime::VaultRuntime;
use mdbx_storage::unlock::UnlockService;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::config::{Connection, private_file};
use crate::error::{GatewayError, Result};
use crate::model::{Provider, validate_api_base, validate_name, validate_note};
use crate::upstream::reject_secret_value;

const CREDENTIAL_SCHEMA: &str = "monica.gateway.credential.v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredential {
    schema: String,
    provider: Provider,
    api_base: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
    token: String,
}

impl Drop for StoredCredential {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// Intentionally has no Serialize or Debug implementation.
pub(crate) struct Credential {
    pub token: Zeroizing<String>,
}

#[derive(Clone)]
pub struct Vault {
    pub(crate) runtime: VaultRuntime,
}

/// A human import can recover gateway bindings without returning any payloads.
pub(crate) struct GatewayInventory {
    pub vault_id: String,
    pub collection_id: Option<String>,
    pub connections: BTreeMap<String, Connection>,
}

impl Vault {
    pub fn create(path: &Path, password: &str, mode: TigaMode) -> Result<Self> {
        if password.is_empty() || path.exists() {
            return Err(GatewayError::InvalidRequest);
        }
        let mut pending =
            PendingVaultCreation::begin(path).map_err(|_| GatewayError::StateUnavailable)?;
        private_file(path)?;
        initialize_vault(
            pending.connection(),
            &VaultInitParams {
                default_tiga_mode: mode.to_string(),
                ..Default::default()
            },
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        UnlockService::setup_password_with_mode(pending.connection_mut(), password, mode)
            .map_err(|_| GatewayError::UnlockRequired)?;
        Ok(Self {
            runtime: VaultRuntime::from_connection(pending.commit()),
        })
    }

    pub fn open(path: &Path, password: &str) -> Result<Self> {
        if !path.is_file() || password.is_empty() {
            return Err(GatewayError::UnlockRequired);
        }
        let mut connection = VaultConnection::open(path).map_err(|error| match error {
            // The early Android MDBX-1 variant stores PBKDF2/AES metadata in
            // vault_meta instead of the native unlock table. Never synthesize
            // an empty table or treat that connection as unlocked.
            StorageError::Database(error)
                if error.to_string() == "no such table: unlock_methods" =>
            {
                GatewayError::VaultSchemaUnsupported
            }
            StorageError::Validation(_) => GatewayError::InvalidVault,
            _ => GatewayError::StateUnavailable,
        })?;
        UnlockService::unlock_with_password(&mut connection, password)
            .map_err(|_| GatewayError::UnlockRequired)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        Ok(Self {
            runtime: VaultRuntime::from_connection(connection),
        })
    }

    /// Local management only. The broker/MCP protocol has no route to this method.
    pub fn store_credential(
        &self,
        collection_id: Option<&str>,
        name: &str,
        provider: Provider,
        api_base: &str,
        note: &str,
        token: Zeroizing<String>,
    ) -> Result<(String, Connection)> {
        validate_name(name)?;
        let api_base = validate_api_base(api_base, provider)?.to_string();
        validate_token(&token)?;
        validate_note(note)?;
        reject_secret_value(&serde_json::json!([name, &api_base, note]), &token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        let collection = collection_id
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let credential_id = uuid::Uuid::new_v4().to_string();
        let stored = StoredCredential {
            schema: CREDENTIAL_SCHEMA.to_owned(),
            provider,
            api_base: api_base.clone(),
            note: note.to_owned(),
            token: token.to_string(),
        };
        let payload =
            serde_json::to_string(&stored).map_err(|_| GatewayError::CredentialUnavailable)?;
        let mut commands = Vec::new();
        if collection_id.is_none() {
            commands.push(WriteCommand::CreateProject {
                project_id: collection.clone(),
                title: "Monica Credential Gateway".to_owned(),
            });
        }
        commands.push(WriteCommand::CreateEntry {
            entry_id: credential_id.clone(),
            project_id: collection.clone(),
            entry_type: ObjectTypeId::ApiToken.to_string(),
            title: name.to_owned(),
            payload_json: payload,
        });
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        OperationCoordinator::execute(
            &connection,
            &CommitContext::new("monica-pass-admin".to_owned()),
            WriteOperationRequest::new(
                uuid::Uuid::new_v4().to_string(),
                "gateway-add-credential",
                commands,
            ),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        Ok((
            collection,
            Connection {
                provider,
                credential_id,
                api_base,
                note: note.to_owned(),
            },
        ))
    }

    /// Called only after the gateway grant and repository have been checked.
    pub(crate) fn credential(&self, binding: &Connection, now: i64) -> Result<Credential> {
        let (mut stored, _) = self.reveal_stored(binding, now)?;
        Ok(Credential {
            token: Zeroizing::new(std::mem::take(&mut stored.token)),
        })
    }

    fn reveal_stored(&self, binding: &Connection, now: i64) -> Result<(StoredCredential, String)> {
        let mut connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let device = DeviceContext {
            device_id: Some("monica-pass-gateway".to_owned()),
            assurance: DeviceAssurance::Standard,
            ..Default::default()
        };
        let limits =
            ObjectDisclosureLimits::new(16 * 1024).map_err(|_| GatewayError::StateUnavailable)?;
        let disclosed = ObjectDisclosureService::reveal_with_active_session_and_limits(
            &mut connection,
            &binding.credential_id,
            &device,
            now,
            limits,
        )
        .map_err(|_| GatewayError::UnlockRequired)?;
        if disclosed.object.entry_type != ObjectTypeId::ApiToken {
            return Err(GatewayError::CredentialUnavailable);
        }
        let bytes = Zeroizing::new(disclosed.object.payload_ct);
        let stored: StoredCredential =
            serde_json::from_slice(&bytes).map_err(|_| GatewayError::CredentialUnavailable)?;
        if stored.schema != CREDENTIAL_SCHEMA
            || stored.provider != binding.provider
            || stored.api_base != binding.api_base
            || stored.note != binding.note
        {
            return Err(GatewayError::CredentialUnavailable);
        }
        validate_token(&stored.token)?;
        validate_note(&stored.note)?;
        Ok((stored, disclosed.object.project_id))
    }

    /// Edit only public context, retaining the encrypted token and credential identity.
    pub(crate) fn update_note(
        &self,
        name: &str,
        binding: &Connection,
        note: &str,
    ) -> Result<Connection> {
        self.edit_credential(name, binding, note, None)
    }

    pub(crate) fn edit_credential(
        &self,
        name: &str,
        binding: &Connection,
        note: &str,
        token: Option<Zeroizing<String>>,
    ) -> Result<Connection> {
        validate_name(name)?;
        validate_note(note)?;
        let (mut stored, project_id) =
            self.reveal_stored(binding, chrono::Utc::now().timestamp())?;
        reject_secret_value(&serde_json::json!([name, note]), &stored.token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        stored.note = note.to_owned();
        if let Some(token) = token {
            validate_token(&token)?;
            reject_secret_value(&serde_json::json!([name, note, &binding.api_base]), &token)
                .map_err(|_| GatewayError::SensitiveMetadata)?;
            stored.token.zeroize();
            stored.token = token.to_string();
        }
        let payload_json =
            serde_json::to_string(&stored).map_err(|_| GatewayError::StateUnavailable)?;
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        OperationCoordinator::execute(
            &connection,
            &CommitContext::new("monica-pass-admin".to_owned()),
            WriteOperationRequest::new(
                uuid::Uuid::new_v4().to_string(),
                "gateway-edit-note",
                vec![WriteCommand::UpdateEntry {
                    entry_id: binding.credential_id.clone(),
                    project_id,
                    entry_type: ObjectTypeId::ApiToken.to_string(),
                    title: name.to_owned(),
                    payload_json,
                }],
            ),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        let mut updated = binding.clone();
        updated.note = note.to_owned();
        Ok(updated)
    }

    pub fn lock(&self) -> Result<()> {
        self.runtime
            .with_write(|connection| {
                connection.clear_session();
                Ok(())
            })
            .map_err(|_| GatewayError::StateUnavailable)
    }

    pub(crate) fn gateway_inventory(&self) -> Result<GatewayInventory> {
        let mut connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let counts = connection
            .diagnostics_summary()
            .map_err(|_| GatewayError::InvalidVault)?;
        if counts.external_attachment_count > 0 {
            return Err(GatewayError::ExternalBlobsUnsupported);
        }
        if counts.project_count > 2048 {
            return Err(GatewayError::VaultConnectionsInvalid);
        }
        let mut result = GatewayInventory {
            vault_id: connection
                .vault_id()
                .map_err(|_| GatewayError::InvalidVault)?,
            collection_id: None,
            connections: BTreeMap::new(),
        };
        let device = DeviceContext {
            device_id: Some("monica-pass-admin".to_owned()),
            assurance: DeviceAssurance::Standard,
            ..Default::default()
        };
        let limits =
            ObjectDisclosureLimits::new(16 * 1024).map_err(|_| GatewayError::StateUnavailable)?;
        let mut cursor = None;
        let mut inspected = 0;
        loop {
            let collections =
                CollectionSummaryRepo::list_active(&connection, 100, cursor.as_deref())
                    .map_err(|_| GatewayError::InvalidVault)?;
            for collection in collections.items {
                // Unrelated passwords and application records are never disclosed.
                // Native categories can be renamed or nested by either client.
                // Only explicitly typed API tokens with our schema are imported.
                let mut object_cursor = None;
                loop {
                    let objects = ObjectSummaryRepo::list(
                        &connection,
                        &collection.collection_id,
                        Some(&ObjectTypeId::ApiToken),
                        100,
                        object_cursor.as_deref(),
                    )
                    .map_err(|_| GatewayError::InvalidVault)?;
                    for object in objects.items {
                        inspected += 1;
                        if inspected > 256 {
                            return Err(GatewayError::VaultConnectionsInvalid);
                        }
                        let disclosed =
                            ObjectDisclosureService::reveal_with_active_session_and_limits(
                                &mut connection,
                                &object.object_id,
                                &device,
                                chrono::Utc::now().timestamp(),
                                limits,
                            )
                            .map_err(|_| GatewayError::UnlockRequired)?;
                        let payload = Zeroizing::new(disclosed.object.payload_ct);
                        let Ok(stored) = serde_json::from_slice::<StoredCredential>(&payload)
                        else {
                            continue;
                        };
                        if stored.schema != CREDENTIAL_SCHEMA {
                            continue;
                        }
                        let name = String::from_utf8(object.title.unwrap_or_default())
                            .map_err(|_| GatewayError::VaultConnectionsInvalid)?;
                        validate_name(&name).map_err(|_| GatewayError::VaultConnectionsInvalid)?;
                        validate_api_base(&stored.api_base, stored.provider)?;
                        validate_token(&stored.token)?;
                        validate_note(&stored.note)?;
                        reject_secret_value(
                            &serde_json::json!([&name, &stored.note, &stored.api_base]),
                            &stored.token,
                        )
                        .map_err(|_| GatewayError::SensitiveMetadata)?;
                        let binding = Connection {
                            provider: stored.provider,
                            api_base: stored.api_base.clone(),
                            credential_id: object.object_id,
                            note: stored.note.clone(),
                        };
                        if result.connections.insert(name, binding).is_some()
                            || result.connections.len() > 64
                        {
                            return Err(GatewayError::VaultConnectionsInvalid);
                        }
                        result
                            .collection_id
                            .get_or_insert(collection.collection_id.clone());
                    }
                    object_cursor = objects.next_cursor;
                    if object_cursor.is_none() {
                        break;
                    }
                }
            }
            cursor = collections.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(result)
    }
}

pub(crate) fn validate_token(token: &str) -> Result<()> {
    if !(16..=4096).contains(&token.len()) || !token.bytes().all(|byte| (33..=126).contains(&byte))
    {
        return Err(GatewayError::CredentialUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "A correct and lengthy test passphrase 938!";
    const TOKEN: &str = "test-upstream-secret-32-characters";

    #[test]
    fn vault_credential_roundtrip_is_encrypted_and_lock_revokes_access() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "work",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        let now = chrono::Utc::now().timestamp();
        assert_eq!(
            vault.credential(&binding, now).unwrap().token.as_str(),
            TOKEN
        );
        let mut changed = binding.clone();
        changed.api_base = "https://another-host.example/".to_owned();
        assert!(matches!(
            vault.credential(&changed, now),
            Err(GatewayError::CredentialUnavailable)
        ));
        vault.lock().unwrap();
        assert!(matches!(
            vault.credential(&binding, now),
            Err(GatewayError::UnlockRequired)
        ));
        drop(vault);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(TOKEN.len())
                .any(|window| window == TOKEN.as_bytes())
        );
        assert!(matches!(
            Vault::open(&path, "wrong"),
            Err(GatewayError::UnlockRequired)
        ));
        let reopened = Vault::open(&path, PASSWORD).unwrap();
        let now = chrono::Utc::now().timestamp();
        assert_eq!(
            reopened.credential(&binding, now).unwrap().token.as_str(),
            TOKEN
        );
        assert!(matches!(
            reopened.credential(&binding, now + 301),
            Err(GatewayError::UnlockRequired)
        ));
    }

    #[test]
    fn vault_failed_creation_does_not_leave_a_usable_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        assert!(Vault::create(&path, "", TigaMode::Multi).is_err());
        assert!(!path.exists());
    }
}
