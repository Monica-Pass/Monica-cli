use std::collections::BTreeMap;
use std::path::Path;

use mdbx_core::model::{ObjectSummary, ObjectTypeId};
use mdbx_core::tiga::{
    DeviceAssurance, DeviceContext, PolicyException, ResolvedTigaPolicy, TigaMode,
    TigaPolicyOverride, TigaScope,
};
use mdbx_storage::connection::{PendingVaultCreation, VaultConnection};
use mdbx_storage::error::StorageError;
use mdbx_storage::init::{VaultInitParams, initialize_vault};
use mdbx_storage::object_disclosure::{ObjectDisclosureLimits, ObjectDisclosureService};
use mdbx_storage::repo::{
    CollectionSummaryRepo, CommitContext, ObjectSummaryRepo, OperationCoordinator, ProjectRepo,
    WriteCommand, WriteOperationRequest,
};
use mdbx_storage::runtime::VaultRuntime;
use mdbx_storage::tiga::TigaService;
use mdbx_storage::tiga_policy::TigaAuthorizationContext;
use mdbx_storage::unlock::UnlockService;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::config::{Connection, private_file};
use crate::error::{GatewayError, Result};
use crate::keys::id::{new_logical_entry_id, physical_entry_id, root_collection_id};
use crate::keys::limits::KEY_DISCLOSURE_LIMIT_BYTES;
use crate::keys::openpgp;
use crate::keys::openssh;
use crate::keys::payload::{
    self, GpgCertificate, LOGIN_ENTRY_TYPE, LOGIN_TYPE_GPG, LOGIN_TYPE_SSH, SshKeyData,
};
use crate::model::{Provider, validate_api_base, validate_name, validate_note, validate_title};
use crate::upstream::reject_secret_value;

const CREDENTIAL_SCHEMA: &str = "monica.gateway.credential.v1";

/// Gateway tokens are small JSON records; a bigger payload is not a credential we wrote.
const GATEWAY_PAYLOAD_LIMIT_BYTES: u64 = 16 * 1024;
/// Folder created for key entries when the caller does not name one. English and fixed, like the
/// gateway collection, because Android users can move the entries out of it afterwards.
const KEY_FOLDER_TITLE: &str = "Monica Keys";
/// Label of the collection Android itself writes new entries into. Only the id matters for
/// interoperability — Android looks the row up by id and shows its own heading — so this is what
/// this CLI's own listings call the folder.
const ANDROID_ROOT_TITLE: &str = "Monica";
/// Upper bound on login rows a single key listing will decrypt.
const MAX_KEY_SCAN_ENTRIES: usize = 4096;

#[derive(Serialize, Deserialize)]
struct StoredCredential {
    schema: String,
    /// ASCII connection handle, stored so a Chinese display title never breaks vault re-import.
    /// Empty in pre-title vaults, where the entry title still equals the handle.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
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

/// Why a payload is being decrypted. The two purposes never share a size limit or a type check,
/// so key material cannot be read through the gateway path and a gateway token cannot be read
/// through the wider key limit. Which `login_type` a key record may carry is a separate question
/// and is settled by the caller that reads the payload.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RevealPurpose {
    GatewayToken,
    KeyAdmin,
    PasswordAdmin,
}

pub(crate) struct Disclosed {
    pub(crate) payload: Zeroizing<Vec<u8>>,
    pub(crate) project_id: String,
    pub(crate) snapshot: ObjectSummary,
}

/// What a person may see about one key entry. Carries public metadata only, never key text.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeyEntrySummary {
    pub entry_id: String,
    pub logical_id: String,
    pub collection_id: String,
    pub title: String,
    pub login_type: String,
    pub algorithm: String,
    pub key_size: Option<i64>,
    pub fingerprint: String,
    pub comment: String,
    pub public_key: String,
    pub notes: String,
    pub has_secret: bool,
    pub chunk_count: usize,
}

/// The material a new key entry carries, already reduced to the Android field shape.
pub enum NewKeyEntry<'a> {
    Ssh {
        data: &'a SshKeyData,
    },
    Gpg {
        certificate: &'a GpgCertificate,
        /// Private armor, or an empty string for a public-only certificate.
        secret_armor: &'a str,
    },
}

/// Text the single human-only export command writes to a file the person names.
pub struct KeyExportText {
    pub summary: KeyEntrySummary,
    pub private_text: Option<Zeroizing<String>>,
    pub public_text: String,
}

impl Vault {
    /// Read-only client compatibility preflight, independent of supplied unlock factors.
    /// Success is not authentication and must not authorize any credential disclosure.
    pub fn check_terminal_support(path: &Path) -> Result<()> {
        crate::glitter::require_terminal_file(path)
    }

    pub fn create(
        path: &Path,
        password: &(impl crate::credentials::VaultPassword + ?Sized),
        mode: TigaMode,
    ) -> Result<Self> {
        crate::glitter::require_terminal_mode(mode)?;
        if password.as_ref().is_empty() || path.exists() {
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
        if let Some(key) = password.security_key() {
            UnlockService::setup_password_security_key(
                pending.connection_mut(),
                password.as_ref(),
                key,
                mode,
            )
        } else {
            UnlockService::setup_password_with_mode(
                pending.connection_mut(),
                password.as_ref(),
                mode,
            )
        }
        .map_err(|_| GatewayError::UnlockRequired)?;
        let vault = Self {
            runtime: VaultRuntime::from_connection(pending.commit()),
        };
        vault.ensure_android_root();
        Ok(vault)
    }

    pub fn open(
        path: &Path,
        password: &(impl crate::credentials::VaultPassword + ?Sized),
    ) -> Result<Self> {
        if !path.is_file() || password.as_ref().is_empty() {
            return Err(GatewayError::UnlockRequired);
        }
        crate::glitter::require_terminal_file(path)?;
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
        // Recheck after open; a public header hint is never authentication.
        if UnlockService::is_glitter(&connection).map_err(|_| GatewayError::InvalidVault)? {
            return Err(GatewayError::GlitterUnavailable);
        }
        if let Some(key) = password.security_key() {
            UnlockService::unlock_with_password_security_key(
                &mut connection,
                password.as_ref(),
                key,
            )
        } else {
            UnlockService::unlock_with_password(&mut connection, password.as_ref())
        }
        .map_err(|_| GatewayError::UnlockRequired)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let vault = Self {
            runtime: VaultRuntime::from_connection(connection),
        };
        vault.ensure_android_root();
        Ok(vault)
    }

    /// The security profile this vault starts every new entry on.
    pub fn tiga_default(&self) -> Result<TigaMode> {
        self.runtime
            .with_read(TigaService::get_global_default)
            .map_err(|_| GatewayError::StateUnavailable)
    }

    /// The profile as it actually applies, with any override the vault already
    /// carries — from this CLI or from Monica for Android — folded in.
    pub fn tiga_policy(&self) -> Result<ResolvedTigaPolicy> {
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        TigaService::resolve_vault_policy(&connection).map_err(map_tiga_error)
    }

    /// Move the vault's default profile.
    ///
    /// Raising it is a plain change. Lowering it is a recorded exception: the
    /// vault keeps the reason a person typed beside the audit event, with no
    /// deadline, so `tiga show` keeps reporting the vault as running under an
    /// exception until someone raises it back. This mirrors the path the Android
    /// app takes through the same engine, so both clients read the same state.
    pub fn set_tiga_policy(&self, target: TigaMode, reason: Option<&str>) -> Result<()> {
        crate::glitter::require_terminal_mode(target)?;
        let connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let current = TigaService::get_global_default(&connection).map_err(map_tiga_error)?;
        let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());
        let exception = match (target < current, reason) {
            (true, None) => return Err(GatewayError::TigaReasonRequired),
            (false, Some(_)) => {
                // A reason that goes nowhere would read later like it was recorded.
                return Err(GatewayError::TigaReasonNotApplicable);
            }
            (true, Some(reason)) => Some(PolicyException {
                exception_id: uuid::Uuid::new_v4().to_string(),
                target: TigaScope::Vault,
                approved_override: TigaPolicyOverride::for_vault_profile(target),
                reason: reason.to_owned(),
                expires_at_unix_secs: None,
            }),
            (false, None) => None,
        };
        let session = connection.active_session().cloned();
        let device = DeviceContext {
            device_id: Some("monica-pass-admin".to_owned()),
            // What this CLI honestly has: a normal unlocked process, no hardware
            // attestation and no screen-capture guarantee to claim.
            assurance: DeviceAssurance::Standard,
            secure_clipboard_available: false,
            screen_capture_protection_available: false,
            secure_temp_files_available: true,
        };
        TigaService::set_vault_profile_authorized(
            &connection,
            &CommitContext::new("monica-pass-admin".to_owned()),
            target,
            exception.as_ref(),
            TigaAuthorizationContext {
                session: session.as_ref(),
                device: &device,
                now_unix_secs: chrono::Utc::now().timestamp(),
            },
        )
        .map_err(map_tiga_error)?;
        Ok(())
    }

    /// Make the vault writable by Monica for Android, and repair the vaults that were not.
    ///
    /// Android names the folder it saves new entries into after the vault itself —
    /// `monica-root:{vault_id}` as a version-3 UUID — while every id minted here is version 4, so
    /// a database created by this CLI simply has no row for Android to write to. The read path
    /// never asks for one, which is why such a vault browses and opens but rejects every save.
    ///
    /// Deliberately best effort: a vault that cannot be seeded yet still opens, and the next
    /// unlock tries again. Nothing here can turn a working open into a failing one.
    pub(crate) fn ensure_android_root(&self) {
        let Ok(vault_id) = self.vault_id() else {
            return;
        };
        let root_id = root_collection_id(&vault_id).to_string();
        let existing = {
            let Ok(connection) = self.runtime.read() else {
                return;
            };
            match ProjectRepo::get_by_id(&connection, &root_id) {
                Ok(project) => project.map(|project| project.deleted),
                Err(_) => return,
            }
        };
        let command = match existing {
            None => WriteCommand::CreateProject {
                project_id: root_id,
                title: ANDROID_ROOT_TITLE.to_owned(),
            },
            Some(true) => WriteCommand::RestoreProject {
                project_id: root_id,
                parent_project_id: None,
            },
            Some(false) => return,
        };
        let _ = self.key_write("android-root-seed", vec![command]);
    }

    /// True for the folder Android saves into. Deleting or nesting it would make the vault
    /// read-only in Monica again, so the editing commands refuse to touch it as a target.
    pub(crate) fn is_android_root(&self, id: &str) -> bool {
        self.vault_id()
            .is_ok_and(|vault_id| root_collection_id(&vault_id).to_string() == id)
    }

    /// Local management only. The broker/MCP protocol has no route to this method.
    #[allow(clippy::too_many_arguments)]
    pub fn store_credential(
        &self,
        collection_id: Option<&str>,
        name: &str,
        title: &str,
        provider: Provider,
        api_base: &str,
        note: &str,
        token: Zeroizing<String>,
    ) -> Result<(String, Connection)> {
        validate_name(name)?;
        let title = if title.trim().is_empty() {
            name.to_owned()
        } else {
            validate_title(title)?;
            title.trim().to_owned()
        };
        let api_base = validate_api_base(api_base, provider)?.to_string();
        validate_token(&token)?;
        validate_note(note)?;
        reject_secret_value(&serde_json::json!([name, &title, &api_base, note]), &token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        let collection = collection_id
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let credential_id = uuid::Uuid::new_v4().to_string();
        let stored = StoredCredential {
            schema: CREDENTIAL_SCHEMA.to_owned(),
            name: name.to_owned(),
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
            title,
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
        let (stored, disclosed) = self.credential_document(binding, now)?;
        Ok((stored, disclosed.project_id))
    }

    fn credential_document(
        &self,
        binding: &Connection,
        now: i64,
    ) -> Result<(StoredCredential, Disclosed)> {
        let disclosed = self.reveal(&binding.credential_id, RevealPurpose::GatewayToken, now)?;
        let stored: StoredCredential = serde_json::from_slice(&disclosed.payload)
            .map_err(|_| GatewayError::CredentialUnavailable)?;
        if stored.schema != CREDENTIAL_SCHEMA {
            return Err(GatewayError::ObjectReadOnly);
        }
        if stored.provider != binding.provider
            || stored.api_base != binding.api_base
            || stored.note != binding.note
        {
            return Err(GatewayError::CredentialUnavailable);
        }
        validate_token(&stored.token)?;
        validate_note(&stored.note)?;
        Ok((stored, disclosed))
    }

    /// The only path that turns stored ciphertext into plaintext. Every caller must state which
    /// kind of record it expects, and a record of another kind is refused before any bytes of it
    /// leave this function.
    pub(crate) fn reveal(
        &self,
        entry_id: &str,
        purpose: RevealPurpose,
        now: i64,
    ) -> Result<Disclosed> {
        let (device_id, limit, expected_type) = match purpose {
            RevealPurpose::GatewayToken => (
                "monica-pass-gateway",
                GATEWAY_PAYLOAD_LIMIT_BYTES,
                ObjectTypeId::ApiToken,
            ),
            RevealPurpose::KeyAdmin => (
                "monica-pass-admin",
                KEY_DISCLOSURE_LIMIT_BYTES as u64,
                ObjectTypeId::Login,
            ),
            RevealPurpose::PasswordAdmin => (
                "monica-pass-admin",
                crate::passwords::MAX_PAYLOAD_BYTES as u64,
                ObjectTypeId::Login,
            ),
        };
        let mut connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let snapshot = ObjectSummaryRepo::get(&connection, entry_id)
            .map_err(|_| GatewayError::StateUnavailable)?
            .ok_or(GatewayError::NotFound)?;
        if snapshot.payload_schema_version != 1 {
            return Err(GatewayError::ObjectReadOnly);
        }
        let device = DeviceContext {
            device_id: Some(device_id.to_owned()),
            assurance: DeviceAssurance::Standard,
            ..Default::default()
        };
        let limits =
            ObjectDisclosureLimits::new(limit).map_err(|_| GatewayError::StateUnavailable)?;
        let disclosed = ObjectDisclosureService::reveal_with_active_session_and_limits(
            &mut connection,
            entry_id,
            &device,
            now,
            limits,
        )
        .map_err(|error| match (&error, purpose) {
            (StorageError::NotFound(_), _) => GatewayError::NotFound,
            (StorageError::ResourceLimit { .. }, RevealPurpose::KeyAdmin) => {
                GatewayError::KeyPayloadTooLarge
            }
            (StorageError::ResourceLimit { .. }, RevealPurpose::GatewayToken) => {
                GatewayError::CredentialUnavailable
            }
            (StorageError::ResourceLimit { .. }, RevealPurpose::PasswordAdmin) => {
                GatewayError::ObjectPayloadTooLarge
            }
            _ => GatewayError::UnlockRequired,
        })?;
        if disclosed.object.entry_type != expected_type {
            return Err(match purpose {
                RevealPurpose::GatewayToken => GatewayError::CredentialUnavailable,
                RevealPurpose::KeyAdmin => GatewayError::KeyEntryTypeMismatch,
                RevealPurpose::PasswordAdmin => GatewayError::ObjectReadOnly,
            });
        }
        if disclosed.object.payload_schema_version != 1
            || disclosed.object.head_commit_id != snapshot.head_commit_id
            || disclosed.object.project_id != snapshot.collection_id
        {
            return Err(GatewayError::ObjectChanged);
        }
        Ok(Disclosed {
            payload: Zeroizing::new(disclosed.object.payload_ct),
            project_id: disclosed.object.project_id,
            snapshot,
        })
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
        let (stored, disclosed) =
            self.credential_document(binding, chrono::Utc::now().timestamp())?;
        let mut document: Value = mdbx_core::json::from_slice(&disclosed.payload)
            .map_err(|_| GatewayError::CredentialUnavailable)?;
        reject_secret_value(&serde_json::json!([name, note]), &stored.token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        if stored.note != note {
            document["note"] = Value::String(note.to_owned());
        }
        if let Some(token) = token {
            validate_token(&token)?;
            reject_secret_value(&serde_json::json!([name, note, &binding.api_base]), &token)
                .map_err(|_| GatewayError::SensitiveMetadata)?;
            document["token"] = Value::String(token.to_string());
        }
        let payload_json =
            serde_json::to_string(&document).map_err(|_| GatewayError::StateUnavailable)?;
        self.write_object(
            &disclosed.snapshot,
            "gateway-edit-note",
            WriteCommand::UpdateEntry {
                entry_id: binding.credential_id.clone(),
                project_id: disclosed.project_id,
                entry_type: ObjectTypeId::ApiToken.to_string(),
                title: object_title(&disclosed.snapshot)?,
                payload_json,
            },
        )?;
        let mut updated = binding.clone();
        updated.note = note.to_owned();
        Ok(updated)
    }

    /// Change only the display title, retaining the encrypted payload and identity.
    pub(crate) fn rename_entry(&self, binding: &Connection, title: &str) -> Result<()> {
        validate_title(title)?;
        let title = title.trim();
        let (stored, disclosed) =
            self.credential_document(binding, chrono::Utc::now().timestamp())?;
        reject_secret_value(&serde_json::json!([title]), &stored.token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
        let payload_json = String::from_utf8(disclosed.payload.to_vec())
            .map_err(|_| GatewayError::CredentialUnavailable)?;
        self.write_object(
            &disclosed.snapshot,
            "gateway-rename-entry",
            WriteCommand::UpdateEntry {
                entry_id: binding.credential_id.clone(),
                project_id: disclosed.project_id,
                entry_type: ObjectTypeId::ApiToken.to_string(),
                title: title.to_owned(),
                payload_json,
            },
        )?;
        Ok(())
    }

    /// Local management of SSH and GPG entries. Nothing here is reachable from the broker: the
    /// gateway inventory only ever discloses API tokens.
    pub(crate) fn vault_id(&self) -> Result<String> {
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        connection
            .vault_id()
            .map_err(|_| GatewayError::InvalidVault)
    }

    /// The folder key entries land in when the caller does not name one. Reuses the existing
    /// folder so repeated adds do not scatter records across the tree.
    fn keys_folder(&self) -> Result<String> {
        let library = self.library()?;
        if let Some(category) = library
            .categories
            .iter()
            .find(|category| category.title == KEY_FOLDER_TITLE)
        {
            return Ok(category.id.clone());
        }
        let project_id = uuid::Uuid::new_v4().to_string();
        self.key_write(
            "keys-folder",
            vec![WriteCommand::CreateProject {
                project_id: project_id.clone(),
                title: KEY_FOLDER_TITLE.to_owned(),
            }],
        )?;
        Ok(project_id)
    }

    fn key_write(&self, intent: &str, commands: Vec<WriteCommand>) -> Result<()> {
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
            WriteOperationRequest::new(uuid::Uuid::new_v4().to_string(), intent, commands),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        Ok(())
    }

    /// Discloses a key payload and checks it really is one. An ordinary password login is
    /// refused here, which is what keeps the wider key limit out of reach of normal secrets.
    fn reveal_key(&self, entry_id: &str, login_type: Option<&str>) -> Result<(Value, String)> {
        let (value, disclosed) = self.key_document(entry_id, login_type)?;
        Ok((value, disclosed.project_id))
    }

    fn key_document(&self, entry_id: &str, login_type: Option<&str>) -> Result<(Value, Disclosed)> {
        let disclosed = self.reveal(
            entry_id,
            RevealPurpose::KeyAdmin,
            chrono::Utc::now().timestamp(),
        )?;
        let text = std::str::from_utf8(&disclosed.payload)
            .map_err(|_| GatewayError::KeyEntryTypeMismatch)?;
        let value: Value =
            mdbx_core::json::from_str(text).map_err(|_| GatewayError::KeyEntryTypeMismatch)?;
        let stored = payload::read_login_type(&value);
        let known = matches!(stored.as_str(), LOGIN_TYPE_SSH | LOGIN_TYPE_GPG);
        if !known || login_type.is_some_and(|wanted| wanted != stored) {
            return Err(GatewayError::KeyEntryTypeMismatch);
        }
        if stored == LOGIN_TYPE_SSH {
            let raw = payload::read_ssh_key_field(&value, "")?;
            if SshKeyData::decode(&raw)?.is_some_and(|data| data.schema != openssh::SCHEMA_V1) {
                return Err(GatewayError::ObjectReadOnly);
            }
        }
        Ok((value, disclosed))
    }

    /// Ordinary mutations require an adapter which understands both the native
    /// version and the inner schema. Merely recognizing the native type is insufficient.
    pub(crate) fn editable_object(&self, entry_id: &str) -> Result<ObjectSummary> {
        let summary = {
            let connection = self
                .runtime
                .read()
                .map_err(|_| GatewayError::StateUnavailable)?;
            ObjectSummaryRepo::get(&connection, entry_id)
                .map_err(|_| GatewayError::StateUnavailable)?
                .filter(|object| !object.deleted)
                .ok_or(GatewayError::NotFound)?
        };
        match summary.object_type_id {
            ObjectTypeId::ApiToken => {
                let disclosed = self.reveal(
                    entry_id,
                    RevealPurpose::GatewayToken,
                    chrono::Utc::now().timestamp(),
                )?;
                let stored: StoredCredential = serde_json::from_slice(&disclosed.payload)
                    .map_err(|_| GatewayError::ObjectReadOnly)?;
                if stored.schema != CREDENTIAL_SCHEMA {
                    return Err(GatewayError::ObjectReadOnly);
                }
                Ok(disclosed.snapshot)
            }
            ObjectTypeId::Login => match self.password_document(entry_id) {
                Ok(document) if document.android_identity => Ok(document.summary.clone()),
                Ok(_) => Err(GatewayError::ObjectReadOnly),
                Err(GatewayError::ObjectReadOnly) => self
                    .key_document(entry_id, None)
                    .map(|(_, document)| document.snapshot)
                    .map_err(|error| {
                        if matches!(
                            error,
                            GatewayError::KeyEntryTypeMismatch | GatewayError::InvalidKeyMaterial
                        ) {
                            GatewayError::ObjectReadOnly
                        } else {
                            error
                        }
                    }),
                Err(error) => Err(error),
            },
            _ => Err(GatewayError::ObjectReadOnly),
        }
    }

    /// `(payload, collection, title)` for a key entry the person can act on.
    fn load_key(
        &self,
        entry_id: &str,
        login_type: Option<&str>,
    ) -> Result<(Value, String, String)> {
        let (value, collection_id) = self.reveal_key(entry_id, login_type)?;
        let library = self.library()?;
        let title = library
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .map(|entry| entry.title.clone())
            .ok_or(GatewayError::NotFound)?;
        Ok((value, collection_id, title))
    }

    /// Projects a payload onto the public fields a listing or preview may show.
    fn summarize_key(
        entry_id: &str,
        collection_id: &str,
        title: &str,
        value: &Value,
    ) -> Result<KeyEntrySummary> {
        let login_type = payload::read_login_type(value);
        let mut summary = KeyEntrySummary {
            entry_id: entry_id.to_owned(),
            logical_id: payload::read_string(value, "monica_entry_id"),
            collection_id: collection_id.to_owned(),
            title: title.to_owned(),
            algorithm: String::new(),
            key_size: None,
            fingerprint: String::new(),
            comment: String::new(),
            public_key: String::new(),
            notes: payload::read_string(value, "notes"),
            has_secret: false,
            chunk_count: 0,
            login_type,
        };
        match summary.login_type.as_str() {
            LOGIN_TYPE_SSH => {
                let raw = payload::read_ssh_key_field(value, "")?;
                if let Some(data) = SshKeyData::decode(&raw)? {
                    summary.algorithm = data.algorithm;
                    summary.key_size = (data.key_size > 0).then_some(data.key_size);
                    summary.fingerprint = data.fingerprint_sha256;
                    summary.comment = data.comment;
                    summary.public_key = data.public_key_openssh;
                    summary.has_secret = !data.private_key_openssh.trim().is_empty();
                }
            }
            LOGIN_TYPE_GPG => {
                summary.chunk_count = payload::gpg_chunk_count(value);
                summary.has_secret = !payload::read_string(value, "password_plain")
                    .trim()
                    .is_empty();
                if let Some(certificate) = payload::read_gpg_certificate(value)? {
                    summary.fingerprint = certificate.fingerprint;
                    summary.comment = certificate.user_id;
                    // The Android field set stores no algorithm column, so the certificate the
                    // payload already carries is the only source for these two cells.
                    if let Ok(key) = openpgp::read(&certificate.public_armor) {
                        summary.algorithm = key.algorithm;
                        summary.key_size = key.bits;
                    }
                }
            }
            _ => return Err(GatewayError::KeyEntryTypeMismatch),
        }
        Ok(summary)
    }

    /// Creates an SSH or GPG entry exactly as Monica for Android writes one: a `login` record
    /// whose native id is derived from the vault id and its logical id, so an Android import
    /// updates this row instead of adding a second one.
    pub fn add_key_entry(
        &self,
        collection_id: Option<&str>,
        title: &str,
        note: &str,
        key: NewKeyEntry<'_>,
    ) -> Result<KeyEntrySummary> {
        validate_title(title)?;
        validate_note(note)?;
        let title = title.trim().to_owned();
        let login_type = match key {
            NewKeyEntry::Ssh { .. } => LOGIN_TYPE_SSH,
            NewKeyEntry::Gpg { .. } => LOGIN_TYPE_GPG,
        };
        if self.key_entries()?.iter().any(|entry| entry.title == title) {
            return Err(GatewayError::AlreadyExists);
        }
        let collection_id = match collection_id {
            Some(id) => {
                if !self.library()?.categories.iter().any(|c| c.id == id) {
                    return Err(GatewayError::NotFound);
                }
                id.to_owned()
            }
            None => self.keys_folder()?,
        };
        let logical_id = new_logical_entry_id();
        let entry_id = physical_entry_id(&self.vault_id()?, &logical_id).to_string();
        let mut value = payload::new_login_payload(&logical_id, login_type);
        payload::set_folder(&mut value, Some(&collection_id));
        payload::set_notes(&mut value, note);
        match key {
            NewKeyEntry::Ssh { data } => {
                payload::set_ssh_key_data(&mut value, &data.to_json_string());
            }
            NewKeyEntry::Gpg {
                certificate,
                secret_armor,
            } => {
                payload::apply_gpg_certificate(&mut value, certificate)?;
                payload::set_password_plain(&mut value, secret_armor);
            }
        }
        let payload_json = payload::serialize(&value)?;
        self.key_write(
            "keys-add",
            vec![WriteCommand::CreateEntry {
                entry_id: entry_id.clone(),
                project_id: collection_id.clone(),
                entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                title: title.clone(),
                payload_json,
            }],
        )?;
        Self::summarize_key(&entry_id, &collection_id, &title, &value)
    }

    /// Edits the public half of a key entry. Key material itself is replaced by importing again,
    /// because a partial edit of a secret is never what a person means.
    pub fn edit_key_entry(
        &self,
        entry_id: &str,
        title: Option<&str>,
        note: Option<&str>,
        comment: Option<&str>,
    ) -> Result<KeyEntrySummary> {
        let (mut value, disclosed) = self.key_document(entry_id, None)?;
        let collection_id = disclosed.project_id;
        let current_title = object_title(&disclosed.snapshot)?;
        let login_type = payload::read_login_type(&value);
        let title = match title {
            Some(title) => {
                validate_title(title)?;
                title.trim().to_owned()
            }
            None => current_title,
        };
        if self
            .key_entries()?
            .iter()
            .any(|entry| entry.entry_id != entry_id && entry.title == title)
        {
            // Titles are the only handle a key entry has, so a rename may not create a second match.
            return Err(GatewayError::AlreadyExists);
        }
        if let Some(note) = note {
            validate_note(note)?;
            payload::set_notes(&mut value, note);
        }
        if let Some(comment) = comment {
            if login_type != LOGIN_TYPE_SSH {
                return Err(GatewayError::KeyEntryTypeMismatch);
            }
            payload::edit_ssh_comment(&mut value, comment)?;
        }
        let payload_json = payload::serialize(&value)?;
        self.write_object(
            &disclosed.snapshot,
            "keys-edit",
            WriteCommand::UpdateEntry {
                entry_id: entry_id.to_owned(),
                project_id: collection_id.clone(),
                entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                title: title.clone(),
                payload_json,
            },
        )?;
        Self::summarize_key(entry_id, &collection_id, &title, &value)
    }

    /// Every SSH and GPG entry in the vault. Unrelated logins are decrypted only far enough to
    /// prove they are not keys, and never leave this function.
    pub fn key_entries(&self) -> Result<Vec<KeyEntrySummary>> {
        let library = self.library()?;
        let mut entries = Vec::new();
        let mut scanned = 0usize;
        for entry in library
            .entries
            .iter()
            .filter(|entry| entry.kind == LOGIN_ENTRY_TYPE)
        {
            scanned += 1;
            if scanned > MAX_KEY_SCAN_ENTRIES {
                return Err(GatewayError::InvalidVault);
            }
            let Ok((value, _)) = self.reveal_key(&entry.id, None) else {
                continue;
            };
            entries.push(Self::summarize_key(
                &entry.id,
                &entry.category,
                &entry.title,
                &value,
            )?);
        }
        entries.sort_by(|left, right| {
            left.title
                .to_lowercase()
                .cmp(&right.title.to_lowercase())
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        Ok(entries)
    }

    pub fn key_entry(&self, entry_id: &str) -> Result<KeyEntrySummary> {
        let (value, collection_id, title) = self.load_key(entry_id, None)?;
        Self::summarize_key(entry_id, &collection_id, &title, &value)
    }

    /// Resolves the entry a person named in a command. Titles are the only handle key entries
    /// have, so an ambiguous one is an error rather than a guess.
    pub fn key_entry_by_title(&self, title: &str) -> Result<KeyEntrySummary> {
        let wanted = title.trim();
        let matches = self
            .key_entries()?
            .into_iter()
            .filter(|entry| entry.title == wanted)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Err(GatewayError::NotFound),
            [one] => Ok(one.clone()),
            _ => Err(GatewayError::AlreadyExists),
        }
    }

    /// The private text a person asked to write to a file they named. Reached only by the export
    /// command, which is outside the broker, the MCP tool list and command discovery.
    pub fn key_export_text(&self, entry_id: &str) -> Result<KeyExportText> {
        self.require_key_export_allowed()?;
        let (value, collection_id, title) = self.load_key(entry_id, None)?;
        let summary = Self::summarize_key(entry_id, &collection_id, &title, &value)?;
        let (private_text, mut public_text) = match summary.login_type.as_str() {
            LOGIN_TYPE_SSH => {
                let raw = payload::read_ssh_key_field(&value, "")?;
                let data = SshKeyData::decode(&raw)?.ok_or(GatewayError::InvalidKeyMaterial)?;
                let private = (!data.private_key_openssh.is_empty())
                    .then(|| Zeroizing::new(data.private_key_openssh));
                (private, summary.public_key.clone())
            }
            LOGIN_TYPE_GPG => {
                let certificate = payload::read_gpg_certificate(&value)?
                    .ok_or(GatewayError::InvalidKeyMaterial)?;
                let armor = payload::read_string(&value, "password_plain");
                let private = (!armor.is_empty()).then(|| Zeroizing::new(armor));
                (private, certificate.public_armor)
            }
            _ => return Err(GatewayError::KeyEntryTypeMismatch),
        };
        // The OpenSSH public field stores one unwrapped line; a `.pub` file a person appends to
        // `authorized_keys` has to end with a newline, like `ssh-keygen -y` output.
        if !public_text.ends_with('\n') {
            public_text.push('\n');
        }
        Ok(KeyExportText {
            summary,
            private_text,
            public_text,
        })
    }

    pub(crate) fn require_key_export_allowed(&self) -> Result<()> {
        // Human-only access is not permission to bypass Glitter's egress policy. Keep
        // the existing legacy-mode path unchanged even if a future client admits Glitter.
        let policy = self.tiga_policy()?.policy;
        if policy.profile == TigaMode::Glitter && !policy.egress.export_allowed {
            return Err(GatewayError::PermissionDenied);
        }
        Ok(())
    }

    pub(crate) fn require_remote_sync_allowed(&self) -> Result<()> {
        // Resolve the native policy, never the mutable application configuration.
        if self.tiga_policy()?.policy.profile == TigaMode::Glitter {
            return Err(GatewayError::GlitterUnavailable);
        }
        Ok(())
    }

    pub fn lock(&self) -> Result<()> {
        match self.runtime.write() {
            Ok(mut connection) => {
                connection.clear_session();
                Ok(())
            }
            // The native runtime clears all keyrings before reporting this state. Lock is
            // idempotent during broker drain, expiry, and the final shutdown cleanup.
            Err(mdbx_storage::runtime::RuntimeLockPoisoned::AuthenticationRequired) => Ok(()),
            Err(mdbx_storage::runtime::RuntimeLockPoisoned::Poisoned) => {
                Err(GatewayError::StateUnavailable)
            }
        }
    }

    /// Which vault this file is, without disclosing a single secret.
    ///
    /// `gateway_inventory` reveals every token it lists and each reveal writes a
    /// security-audit row, which the schema triggers turn into a pending sync delta
    /// this device then owes its peers. A read that only has to prove identity has
    /// to stay out of that path.
    pub(crate) fn gateway_binding(&self) -> Result<String> {
        let connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        connection
            .vault_id()
            .map_err(|_| GatewayError::InvalidVault)
    }

    pub(crate) fn gateway_inventory(&self) -> Result<GatewayInventory> {
        let mut connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let counts = connection
            .diagnostics_summary()
            .map_err(|_| GatewayError::InvalidVault)?;
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
                        if object.payload_schema_version != 1 {
                            continue;
                        }
                        inspected += 1;
                        if inspected > 256 {
                            return Err(GatewayError::VaultConnectionsInvalid);
                        }
                        let disclosed =
                            match ObjectDisclosureService::reveal_with_active_session_and_limits(
                                &mut connection,
                                &object.object_id,
                                &device,
                                chrono::Utc::now().timestamp(),
                                limits,
                            ) {
                                Ok(disclosed) => disclosed,
                                Err(StorageError::ResourceLimit { .. }) => continue,
                                Err(_) => return Err(GatewayError::UnlockRequired),
                            };
                        if disclosed.object.entry_type != ObjectTypeId::ApiToken
                            || disclosed.object.payload_schema_version != 1
                        {
                            continue;
                        }
                        let payload = Zeroizing::new(disclosed.object.payload_ct);
                        let Ok(stored) = serde_json::from_slice::<StoredCredential>(&payload)
                        else {
                            continue;
                        };
                        if stored.schema != CREDENTIAL_SCHEMA {
                            continue;
                        }
                        let name = if stored.name.is_empty() {
                            // Legacy vaults stored no handle; their title is still the handle.
                            String::from_utf8(object.title.unwrap_or_default())
                                .map_err(|_| GatewayError::VaultConnectionsInvalid)?
                        } else {
                            stored.name.clone()
                        };
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

/// Turn an engine refusal into the one error that tells a person what to do next.
///
/// The TIGA code reports a denied change and a missing unlock through the same handful of
/// variants, and the difference matters: one asks for Monica, the other for a password.
fn map_tiga_error(error: StorageError) -> GatewayError {
    match error {
        StorageError::Authorization(_) => GatewayError::TigaChangeDenied,
        StorageError::Validation(message) | StorageError::ConstraintViolation(message) => {
            let message = message.to_lowercase();
            if message.contains("version") {
                GatewayError::VaultSchemaUnsupported
            } else if message.contains("session") || message.contains("unlock") {
                GatewayError::UnlockRequired
            } else {
                GatewayError::TigaChangeDenied
            }
        }
        StorageError::NotFound(_) => GatewayError::NotFound,
        _ => GatewayError::StateUnavailable,
    }
}

fn object_title(snapshot: &ObjectSummary) -> Result<String> {
    String::from_utf8(snapshot.title.clone().ok_or(GatewayError::ObjectReadOnly)?)
        .map_err(|_| GatewayError::ObjectReadOnly)
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

    /// The engine treats a timestamp older than the session's last activity as an
    /// expired session, and every disclosure moves that activity forward. A test
    /// that reused one captured timestamp across disclosures therefore failed
    /// whenever a second boundary fell between them, so each disclosure reads the
    /// clock itself, as the product code does.
    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

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
                "",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert_eq!(
            vault.credential(&binding, now()).unwrap().token.as_str(),
            TOKEN
        );
        let mut changed = binding.clone();
        changed.api_base = "https://another-host.example/".to_owned();
        assert!(matches!(
            vault.credential(&changed, now()),
            Err(GatewayError::CredentialUnavailable)
        ));
        vault.lock().unwrap();
        assert!(matches!(
            vault.credential(&binding, now()),
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
        assert_eq!(
            reopened.credential(&binding, now()).unwrap().token.as_str(),
            TOKEN
        );
        assert!(matches!(
            reopened.credential(&binding, now() + 301),
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

    fn entry_title(vault: &Vault, credential_id: &str) -> Option<String> {
        vault
            .library()
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.id == credential_id)
            .map(|entry| entry.title)
    }

    #[test]
    fn store_credential_display_title_is_utf8_and_blank_falls_back_to_name() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let (_, custom) = vault
            .store_credential(
                None,
                "work",
                "微信令牌",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert_eq!(
            entry_title(&vault, &custom.credential_id).as_deref(),
            Some("微信令牌")
        );
        let (_, blank) = vault
            .store_credential(
                None,
                "slack",
                "   ",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert_eq!(
            entry_title(&vault, &blank.credential_id).as_deref(),
            Some("slack")
        );
    }

    #[test]
    fn editing_note_or_replacing_token_preserves_the_display_title() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "work",
                "GitHub 工作",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert_eq!(
            entry_title(&vault, &binding.credential_id).as_deref(),
            Some("GitHub 工作")
        );
        let after_note = vault.update_note("work", &binding, "更新后的用途").unwrap();
        assert_eq!(
            entry_title(&vault, &binding.credential_id).as_deref(),
            Some("GitHub 工作")
        );
        let after_token = vault
            .edit_credential(
                "work",
                &after_note,
                "更新后的用途",
                Some(Zeroizing::new(
                    "replacement-upstream-secret-32-chars".to_owned(),
                )),
            )
            .unwrap();
        assert_eq!(
            entry_title(&vault, &binding.credential_id).as_deref(),
            Some("GitHub 工作")
        );
        assert_eq!(
            vault
                .credential(&after_token, now())
                .unwrap()
                .token
                .as_str(),
            "replacement-upstream-secret-32-chars"
        );
    }

    #[test]
    fn rename_entry_retitles_and_survives_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "oldhandle",
                "",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert_eq!(
            entry_title(&vault, &binding.credential_id).as_deref(),
            Some("oldhandle")
        );
        vault.rename_entry(&binding, "旧条目改名").unwrap();
        assert_eq!(
            entry_title(&vault, &binding.credential_id).as_deref(),
            Some("旧条目改名")
        );
        vault.lock().unwrap();
        drop(vault);
        let reopened = Vault::open(&path, PASSWORD).unwrap();
        assert_eq!(
            entry_title(&reopened, &binding.credential_id).as_deref(),
            Some("旧条目改名")
        );
        assert_eq!(
            reopened.credential(&binding, now()).unwrap().token.as_str(),
            TOKEN
        );
    }

    #[test]
    fn display_title_cannot_leak_the_token() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        assert!(matches!(
            vault.store_credential(
                None,
                "work",
                TOKEN,
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            ),
            Err(GatewayError::SensitiveMetadata)
        ));
        let (_, binding) = vault
            .store_credential(
                None,
                "work",
                "ok",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert!(matches!(
            vault.rename_entry(&binding, TOKEN),
            Err(GatewayError::SensitiveMetadata)
        ));
    }

    #[test]
    fn gateway_inventory_uses_the_handle_not_the_display_title_when_importing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "work",
                "微信令牌",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        let inventory = vault.gateway_inventory().unwrap();
        assert!(inventory.connections.contains_key("work"));
        assert!(!inventory.connections.contains_key("微信令牌"));
        assert_eq!(
            inventory.connections["work"].credential_id,
            binding.credential_id
        );
        vault.lock().unwrap();
        drop(vault);
        let reopened = Vault::open(&path, PASSWORD).unwrap();
        let again = reopened.gateway_inventory().unwrap();
        assert_eq!(
            again.connections["work"].credential_id,
            binding.credential_id
        );
        assert_eq!(
            entry_title(&reopened, &binding.credential_id).as_deref(),
            Some("微信令牌")
        );
    }

    #[test]
    fn gateway_inventory_falls_back_to_the_title_for_legacy_vaults_without_a_handle() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let (_, binding) = vault
            .store_credential(
                None,
                "legacyhandle",
                "",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        // Simulate a pre-change vault: rewrite the encrypted payload so it carries no handle.
        let (mut stored, project_id) = vault.reveal_stored(&binding, now()).unwrap();
        stored.name = String::new();
        let payload_json = serde_json::to_string(&stored).unwrap();
        assert!(
            !payload_json.contains("\"name\""),
            "legacy payload must omit the stored handle: {payload_json}"
        );
        let connection = vault.runtime.read().unwrap();
        OperationCoordinator::execute(
            &connection,
            &CommitContext::new("monica-pass-test".to_owned()),
            WriteOperationRequest::new(
                uuid::Uuid::new_v4().to_string(),
                "gateway-legacy-simulation",
                vec![WriteCommand::UpdateEntry {
                    entry_id: binding.credential_id.clone(),
                    project_id,
                    entry_type: ObjectTypeId::ApiToken.to_string(),
                    title: "legacyhandle".to_owned(),
                    payload_json,
                }],
            ),
        )
        .unwrap();
        drop(connection);
        // With no stored handle, the ASCII title is recovered as the connection handle.
        let inventory = vault.gateway_inventory().unwrap();
        assert!(inventory.connections.contains_key("legacyhandle"));
        assert_eq!(
            inventory.connections["legacyhandle"].credential_id,
            binding.credential_id
        );
        vault.lock().unwrap();
    }

    /// Text standing in for a private ring: the vault stores it verbatim and never parses it.
    const PRIVATE_ARMOR: &str = "-----BEGIN PGP PRIVATE KEY BLOCK-----\n\nodN6hB\n=d8gQ\n-----END PGP PRIVATE KEY BLOCK-----\n";

    fn ssh_data() -> SshKeyData {
        openssh::SshKeyPair::generate(openssh::SshAlgorithm::Ed25519, "monica@cli")
            .unwrap()
            .to_data()
    }

    fn gpg_key() -> openpgp::GpgKey {
        openpgp::read(include_str!("../tests/fixtures/gpg/rsa2048-public.asc")).unwrap()
    }

    #[test]
    fn key_entry_is_written_with_the_android_identity_and_payload_shape() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let data = ssh_data();
        let pem = data.private_key_openssh.clone();
        assert!(pem.ends_with("-----END OPENSSH PRIVATE KEY-----\n"));
        let summary = vault
            .add_key_entry(
                None,
                "laptop key",
                "ssh login key",
                NewKeyEntry::Ssh { data: &data },
            )
            .unwrap();
        assert_eq!(summary.login_type, LOGIN_TYPE_SSH);
        assert_eq!(summary.algorithm, openssh::ALGORITHM_ED25519);
        assert_eq!(summary.key_size, Some(256));
        assert_eq!(summary.fingerprint, data.fingerprint_sha256);
        assert_eq!(summary.notes, "ssh login key");
        assert!(summary.has_secret);
        assert_eq!(summary.chunk_count, 0);
        assert!(summary.logical_id.starts_with("password:"));
        // The native id is the Java derivation of vault id plus logical id, so an Android import
        // of the same snapshot updates this row instead of adding a second one.
        assert_eq!(
            summary.entry_id,
            physical_entry_id(&vault.vault_id().unwrap(), &summary.logical_id).to_string()
        );
        let library = vault.library().unwrap();
        let entry = library
            .entries
            .iter()
            .find(|entry| entry.id == summary.entry_id)
            .unwrap();
        assert_eq!(entry.kind, LOGIN_ENTRY_TYPE);
        assert_eq!(entry.title, "laptop key");
        assert_eq!(
            library
                .categories
                .iter()
                .filter(|category| category.title == KEY_FOLDER_TITLE)
                .count(),
            1
        );
        let (value, collection_id) = vault
            .reveal_key(&summary.entry_id, Some(LOGIN_TYPE_SSH))
            .unwrap();
        assert_eq!(collection_id, summary.collection_id);
        assert_eq!(value["room_id"], 0);
        assert_eq!(value["login_type"], LOGIN_TYPE_SSH);
        assert_eq!(value["monica_entry_id"], summary.logical_id.as_str());
        assert_eq!(value["mdbx_folder_id"], collection_id.as_str());
        assert_eq!(value["password_plain"], "");
        assert!(value["ssh_key_data"].is_string());
        let inner: Value =
            serde_json::from_str(value["ssh_key_data"].as_str().unwrap_or_default()).unwrap();
        assert_eq!(inner["privateKeyOpenSsh"], pem.as_str());
        assert_eq!(inner["format"], openssh::FORMAT_OPENSSH);
        assert_eq!(inner["schema"], openssh::SCHEMA_V1);
        // A second add reuses the folder rather than scattering records across the tree.
        let second = vault
            .add_key_entry(None, "server key", "", NewKeyEntry::Ssh { data: &data })
            .unwrap();
        assert_eq!(second.collection_id, summary.collection_id);
        assert_eq!(vault.key_entries().unwrap().len(), 2);
        drop(vault);
        // No second copy of the secret exists: the file carries only engine ciphertext.
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(pem.len()).any(|w| w == pem.as_bytes()));
    }

    #[test]
    fn both_key_texts_and_the_chunked_certificate_survive_a_reopen_byte_for_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let key = gpg_key();
        let certificate = key.certificate();
        let ssh = ssh_data();
        let ssh_summary = vault
            .add_key_entry(None, "ssh", "", NewKeyEntry::Ssh { data: &ssh })
            .unwrap();
        let gpg_summary = vault
            .add_key_entry(
                Some(&ssh_summary.collection_id),
                "gpg",
                "public plus private",
                NewKeyEntry::Gpg {
                    certificate: &certificate,
                    secret_armor: PRIVATE_ARMOR,
                },
            )
            .unwrap();
        assert_eq!(gpg_summary.login_type, LOGIN_TYPE_GPG);
        assert_eq!(gpg_summary.fingerprint, key.fingerprint);
        // Android shows the user id where a login would show a comment.
        assert_eq!(gpg_summary.comment, key.user_id);
        assert_eq!(gpg_summary.algorithm, "RSA");
        assert_eq!(gpg_summary.key_size, Some(2048));
        assert!(gpg_summary.has_secret);
        assert!(gpg_summary.chunk_count >= 2);
        let (value, _) = vault
            .reveal_key(&gpg_summary.entry_id, Some(LOGIN_TYPE_GPG))
            .unwrap();
        let titles = value["custom_fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["title"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            &titles[..4],
            [
                payload::GPG_MARKER,
                payload::GPG_FINGERPRINT,
                payload::GPG_USER_ID,
                payload::GPG_ENCODING
            ]
        );
        // Chunks are numbered from 0000 with no gap; a gap would silently truncate the ring.
        for index in 0..gpg_summary.chunk_count {
            assert!(
                titles
                    .iter()
                    .any(|title| *title == format!("{}{index:04}", payload::GPG_PUBLIC_PREFIX)),
                "chunk {index:04} is missing from {titles:?}"
            );
        }
        vault.lock().unwrap();
        drop(vault);
        let reopened = Vault::open(&path, PASSWORD).unwrap();
        // The public armor only exists on disk as Base64 chunks, so this is the round trip.
        let export = reopened.key_export_text(&gpg_summary.entry_id).unwrap();
        assert_eq!(export.public_text, key.public_armor);
        assert_eq!(
            export.private_text.as_deref().map(String::as_str),
            Some(PRIVATE_ARMOR)
        );
        assert_eq!(export.summary, gpg_summary);
        let export = reopened.key_export_text(&ssh_summary.entry_id).unwrap();
        assert_eq!(
            export.private_text.as_deref().map(String::as_str),
            Some(ssh.private_key_openssh.as_str())
        );
        assert_eq!(export.public_text, format!("{}\n", ssh.public_key_openssh));
        assert_eq!(
            reopened
                .key_entries()
                .unwrap()
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["gpg", "ssh"]
        );
        reopened.lock().unwrap();
    }

    #[test]
    fn the_wider_key_limit_never_opens_a_login_and_the_gateway_never_opens_a_key() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let data = ssh_data();
        let key = vault
            .add_key_entry(None, "laptop", "", NewKeyEntry::Ssh { data: &data })
            .unwrap();
        assert!(matches!(
            vault.reveal_key(&key.entry_id, Some(LOGIN_TYPE_GPG)),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        assert!(matches!(
            vault.reveal(&key.entry_id, RevealPurpose::GatewayToken, now()),
            Err(GatewayError::CredentialUnavailable)
        ));
        let binding = Connection {
            provider: Provider::Github,
            api_base: Provider::Github.default_api_base().to_owned(),
            credential_id: key.entry_id.clone(),
            note: String::new(),
        };
        assert!(matches!(
            vault.credential(&binding, now()),
            Err(GatewayError::CredentialUnavailable)
        ));
        // An ordinary password login shares the native type but is refused by login_type, which
        // is what keeps the 128 KiB limit out of reach of normal secrets.
        let folder = vault.keys_folder().unwrap();
        let mut plain = payload::new_login_payload(&new_logical_entry_id(), "PASSWORD");
        payload::set_folder(&mut plain, Some(&folder));
        payload::set_password_plain(&mut plain, "someone-elses-password");
        let login_id = uuid::Uuid::new_v4().to_string();
        vault
            .key_write(
                "keys-plain-login-simulation",
                vec![WriteCommand::CreateEntry {
                    entry_id: login_id.clone(),
                    project_id: folder.clone(),
                    entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                    title: "plain login".to_owned(),
                    payload_json: payload::serialize(&plain).unwrap(),
                }],
            )
            .unwrap();
        assert!(matches!(
            vault.reveal_key(&login_id, None),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        assert_eq!(
            vault
                .key_entries()
                .unwrap()
                .iter()
                .map(|entry| entry.entry_id.as_str())
                .collect::<Vec<_>>(),
            [key.entry_id.as_str()]
        );
        // Gateway tokens are never key entries either.
        let (_, token) = vault
            .store_credential(
                None,
                "work",
                "",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new(TOKEN.to_owned()),
            )
            .unwrap();
        assert!(matches!(
            vault.reveal_key(&token.credential_id, None),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        let inventory = vault.gateway_inventory().unwrap();
        assert_eq!(inventory.connections.len(), 1);
        assert_eq!(
            inventory.connections["work"].credential_id,
            token.credential_id
        );
        vault.lock().unwrap();
    }

    #[test]
    fn editing_a_key_entry_keeps_every_field_the_cli_does_not_model() {
        let directory = tempfile::tempdir().unwrap();
        use serde_json::json;
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let data = ssh_data();
        let first = vault
            .add_key_entry(None, "laptop", "", NewKeyEntry::Ssh { data: &data })
            .unwrap();
        let (mut value, collection_id) = vault.reveal_key(&first.entry_id, None).unwrap();
        let mut inner: Value =
            serde_json::from_str(&payload::read_ssh_key_field(&value, "").unwrap()).unwrap();
        inner["futureField"] = json!("keep me");
        payload::set_ssh_key_data(&mut value, &serde_json::to_string(&inner).unwrap());
        value["androidExtra"] = json!({"kept": true});
        let payload_json = payload::serialize(&value).unwrap();
        vault
            .key_write(
                "keys-android-simulation",
                vec![WriteCommand::UpdateEntry {
                    entry_id: first.entry_id.clone(),
                    project_id: collection_id,
                    entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                    title: "laptop".to_owned(),
                    payload_json,
                }],
            )
            .unwrap();
        let edited = vault
            .edit_key_entry(
                &first.entry_id,
                Some("renamed"),
                Some("rotated comment"),
                Some("laptop@home"),
            )
            .unwrap();
        assert_eq!(edited.title, "renamed");
        assert_eq!(edited.notes, "rotated comment");
        assert_eq!(edited.comment, "laptop@home");
        assert!(edited.public_key.ends_with(" laptop@home"));
        // Only the comment tail moved: the wire blob and therefore the fingerprint are intact.
        assert_eq!(edited.fingerprint, data.fingerprint_sha256);
        let (value, _, title) = vault.load_key(&first.entry_id, None).unwrap();
        assert_eq!(title, "renamed");
        assert_eq!(value["androidExtra"]["kept"], true);
        let inner: Value =
            serde_json::from_str(value["ssh_key_data"].as_str().unwrap_or_default()).unwrap();
        assert_eq!(inner["futureField"], "keep me");
        assert_eq!(
            inner["privateKeyOpenSsh"],
            data.private_key_openssh.as_str()
        );
        assert_eq!(inner["publicKeyOpenSsh"], edited.public_key.as_str());
        assert_eq!(
            entry_title(&vault, &first.entry_id).as_deref(),
            Some("renamed")
        );
        // A rename may not create a second entry under one name, and a GPG entry has no comment.
        vault
            .add_key_entry(None, "phone", "", NewKeyEntry::Ssh { data: &ssh_data() })
            .unwrap();
        assert!(matches!(
            vault.edit_key_entry(&first.entry_id, Some("phone"), None, None),
            Err(GatewayError::AlreadyExists)
        ));
        let certificate = gpg_key().certificate();
        let gpg = vault
            .add_key_entry(
                None,
                "ring",
                "",
                NewKeyEntry::Gpg {
                    certificate: &certificate,
                    secret_armor: "",
                },
            )
            .unwrap();
        assert!(!gpg.has_secret);
        assert!(matches!(
            vault.edit_key_entry(&gpg.entry_id, None, None, Some("ignored")),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        vault.lock().unwrap();
    }

    #[test]
    fn an_oversized_key_payload_is_refused_before_the_write() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let certificate = GpgCertificate {
            fingerprint: "F".repeat(40),
            user_id: String::new(),
            public_armor: "a".repeat(200 * 1024),
        };
        assert!(matches!(
            vault.add_key_entry(
                None,
                "oversized",
                "",
                NewKeyEntry::Gpg {
                    certificate: &certificate,
                    secret_armor: "",
                },
            ),
            Err(GatewayError::KeyPayloadTooLarge)
        ));
        assert!(vault.key_entries().unwrap().is_empty());
        vault.lock().unwrap();
    }

    #[test]
    fn titles_are_the_only_handle_so_an_ambiguous_name_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::create(
            &directory.path().join("vault.mdbx"),
            PASSWORD,
            TigaMode::Multi,
        )
        .unwrap();
        let data = ssh_data();
        let first = vault
            .add_key_entry(None, "same", "", NewKeyEntry::Ssh { data: &data })
            .unwrap();
        assert!(matches!(
            vault.add_key_entry(None, "same", "", NewKeyEntry::Ssh { data: &data }),
            Err(GatewayError::AlreadyExists)
        ));
        assert!(matches!(
            vault.add_key_entry(
                Some("missing-collection"),
                "elsewhere",
                "",
                NewKeyEntry::Ssh { data: &data }
            ),
            Err(GatewayError::NotFound)
        ));
        // An older vault or a hand-made snapshot can still hold two entries under one title.
        vault
            .key_write(
                "keys-duplicate-simulation",
                vec![WriteCommand::CreateEntry {
                    entry_id: uuid::Uuid::new_v4().to_string(),
                    project_id: first.collection_id.clone(),
                    entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                    title: "same".to_owned(),
                    payload_json: payload::serialize(&payload::new_login_payload(
                        &new_logical_entry_id(),
                        LOGIN_TYPE_SSH,
                    ))
                    .unwrap(),
                }],
            )
            .unwrap();
        assert_eq!(vault.key_entries().unwrap().len(), 2);
        assert!(matches!(
            vault.key_entry_by_title("same"),
            Err(GatewayError::AlreadyExists)
        ));
        assert!(matches!(
            vault.key_entry_by_title("nope"),
            Err(GatewayError::NotFound)
        ));
        assert!(matches!(
            vault.key_entry("00000000-0000-0000-0000-000000000000"),
            Err(GatewayError::NotFound)
        ));
        vault.lock().unwrap();
    }

    /// HarmonyOS Monica writes every non-password item as a `com.monica.harmony.{itemType}`
    /// object in the Android root folder, wrapping its whole record in the payload. This CLI
    /// never parses those payloads: the listing passes the object type through as the kind, the
    /// keys path only ever scans `login` rows, and every reveal of a foreign type must be a
    /// clean error. Pinned here so a synced phone database can never poison the vault.
    #[test]
    fn harmony_object_rows_are_absorbed_and_never_reach_the_credential_paths() {
        use serde_json::json;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let vault_id = vault.vault_id().unwrap();
        let root = root_collection_id(&vault_id).to_string();
        // The item types Monica-for-harmony's MdbxEntryCodec actually writes.
        let mut harmony: Vec<(String, String, String)> = Vec::new();
        for (item_type, title) in [
            ("note", "会议记录"),
            ("bank_card", "工资卡"),
            ("send", "临时分享"),
            ("authenticator", "GitHub OTP"),
        ] {
            let logical = format!("{item_type}:{}", uuid::Uuid::new_v4());
            let record = json!({
                "recordVersion": 1,
                "id": logical,
                "itemType": item_type,
                "title": title,
                "subtitle": "",
                "notes": "",
                "favorite": false,
                "source": {"type": "local", "provider": "harmony", "externalId": logical},
                "payloadJson": "{\"secret\":\"inside the record\"}",
            });
            // The codec nests the record as a JSON object; an earlier reading of the contract
            // embedded it as a JSON string. Both must absorb the same way, because nothing
            // here ever opens the wrapper.
            let wrapper = if item_type == "send" {
                json!({
                    "kind": "harmony",
                    "monica_entry_id": logical,
                    "monica_harmony_record": serde_json::to_string(&record).unwrap(),
                })
            } else {
                json!({
                    "kind": "harmony",
                    "monica_entry_id": logical,
                    "monica_harmony_record": record,
                })
            };
            // Harmony derives its native id the same Java way Android does, from vault and
            // logical id, so the CLI must absorb that id unchanged.
            let entry_id = physical_entry_id(&vault_id, &logical).to_string();
            vault
                .key_write(
                    "harmony-import-simulation",
                    vec![WriteCommand::CreateEntry {
                        entry_id: entry_id.clone(),
                        project_id: root.clone(),
                        entry_type: format!("com.monica.harmony.{item_type}"),
                        title: title.to_owned(),
                        payload_json: serde_json::to_string(&wrapper).unwrap(),
                    }],
                )
                .unwrap();
            harmony.push((
                entry_id,
                format!("com.monica.harmony.{item_type}"),
                title.to_owned(),
            ));
        }

        let library = vault.library().unwrap();
        for (entry_id, kind, title) in &harmony {
            let entry = library
                .entries
                .iter()
                .find(|entry| entry.id == *entry_id)
                .unwrap_or_else(|| panic!("{title} vanished from the listing"));
            // object_type_id passes through as the kind, verbatim.
            assert_eq!(&entry.kind, kind);
            assert_eq!(&entry.title, title);
            assert_eq!(&entry.category, &root);
        }
        // None of these are `login` rows, so the keys path never even scans them.
        assert!(vault.key_entries().unwrap().is_empty());
        assert!(vault.gateway_inventory().unwrap().connections.is_empty());

        // Reveals refuse by type — never a panic, whatever purpose asks.
        let note_id = &harmony[0].0;
        assert!(matches!(
            vault.reveal(note_id, RevealPurpose::KeyAdmin, now()),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        assert!(matches!(
            vault.reveal(note_id, RevealPurpose::GatewayToken, now()),
            Err(GatewayError::CredentialUnavailable)
        ));
        assert!(matches!(
            vault.reveal_key(note_id, None),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        // And an entry that is not there at all says NotFound.
        assert!(matches!(
            vault.reveal(
                &uuid::Uuid::new_v4().to_string(),
                RevealPurpose::KeyAdmin,
                now()
            ),
            Err(GatewayError::NotFound)
        ));

        // Counterexample pinned against the engine rule: an object type id with capitals fails
        // validation — which is exactly why the all-lowercase harmony ids absorb. The CLI
        // collapses every coordinator refusal into StateUnavailable; no entry lands.
        assert!(matches!(
            vault.key_write(
                "harmony-uppercase-simulation",
                vec![WriteCommand::CreateEntry {
                    entry_id: uuid::Uuid::new_v4().to_string(),
                    project_id: root.clone(),
                    entry_type: "com.monica.Harmony.Note".to_owned(),
                    title: "大写类型".to_owned(),
                    payload_json: "{}".to_owned(),
                }],
            ),
            Err(GatewayError::StateUnavailable)
        ));
        assert_eq!(vault.library().unwrap().entries.len(), harmony.len());

        vault.lock().unwrap();
        drop(vault);
        // Reopen: the rows survive with their kinds verbatim, and the Android root folder this
        // CLI seeds is still exactly one row, still protected — harmony writes changed nothing.
        let reopened = Vault::open(&path, PASSWORD).unwrap();
        let library = reopened.library().unwrap();
        for (entry_id, kind, title) in &harmony {
            let entry = library
                .entries
                .iter()
                .find(|entry| entry.id == *entry_id)
                .unwrap_or_else(|| panic!("{title} lost across reopen"));
            assert_eq!(&entry.kind, kind);
        }
        assert_eq!(
            library
                .categories
                .iter()
                .filter(|category| category.id == root)
                .count(),
            1,
            "reopening must not seed a second root row beside harmony's writes"
        );
        assert!(matches!(
            reopened.delete_category(&root),
            Err(crate::library::DeleteBlocked::Protected(_))
        ));
        reopened.lock().unwrap();
    }

    /// Harmony password items land as Android-wire `login` rows, and every field may be
    /// missing — the side writes with `optString` semantics. The keys path scans exactly those
    /// two rows, decrypts both without error, and never surfaces a plain password as a key.
    #[test]
    fn harmony_login_rows_absorb_and_stay_out_of_the_key_listing() {
        use serde_json::json;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        let vault_id = vault.vault_id().unwrap();
        let root = root_collection_id(&vault_id).to_string();

        // The sparse case from the contract: only website and password_plain present.
        let partial_logical = new_logical_entry_id();
        let partial_id = physical_entry_id(&vault_id, &partial_logical).to_string();
        let partial_payload = json!({
            "website": "https://harmony.example",
            "password_plain": "a sparse harmony secret",
        });
        // A complete Android login shares the same row shape.
        let full_logical = new_logical_entry_id();
        let full_id = physical_entry_id(&vault_id, &full_logical).to_string();
        let mut full = payload::new_login_payload(&full_logical, "PASSWORD");
        full["website"] = json!("https://complete.example");
        full["username"] = json!("someone");
        payload::set_password_plain(&mut full, "a complete harmony secret");
        vault
            .key_write(
                "harmony-login-simulation",
                vec![
                    WriteCommand::CreateEntry {
                        entry_id: partial_id.clone(),
                        project_id: root.clone(),
                        entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                        title: "鸿蒙缺字段".to_owned(),
                        payload_json: serde_json::to_string(&partial_payload).unwrap(),
                    },
                    WriteCommand::CreateEntry {
                        entry_id: full_id.clone(),
                        project_id: root.clone(),
                        entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                        title: "鸿蒙完整条目".to_owned(),
                        payload_json: payload::serialize(&full).unwrap(),
                    },
                ],
            )
            .unwrap();

        // The keys scan is offered exactly the two login rows…
        let library = vault.library().unwrap();
        assert_eq!(
            library
                .entries
                .iter()
                .filter(|entry| entry.kind == LOGIN_ENTRY_TYPE)
                .count(),
            2
        );
        // …and decrypting them yields no keys: a PASSWORD login is skipped, not an error.
        assert!(vault.key_entries().unwrap().is_empty());
        assert!(matches!(
            vault.key_entry(&partial_id),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));

        // Absorption semantics: absent fields read as empty strings and an absent login_type
        // reads as PASSWORD, exactly like Android's optString path.
        let disclosed = vault
            .reveal(&partial_id, RevealPurpose::KeyAdmin, now())
            .unwrap();
        assert_eq!(disclosed.project_id, root);
        let value: Value = serde_json::from_slice(&disclosed.payload).unwrap();
        assert_eq!(
            payload::read_string(&value, "website"),
            "https://harmony.example"
        );
        assert_eq!(
            payload::read_string(&value, "password_plain"),
            "a sparse harmony secret"
        );
        assert_eq!(payload::read_string(&value, "username"), "");
        assert_eq!(payload::read_string(&value, "notes"), "");
        assert_eq!(payload::read_string(&value, "monica_entry_id"), "");
        assert_eq!(payload::read_login_type(&value), "PASSWORD");
        assert!(matches!(
            vault.reveal_key(&partial_id, None),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        let disclosed = vault
            .reveal(&full_id, RevealPurpose::KeyAdmin, now())
            .unwrap();
        let value: Value = serde_json::from_slice(&disclosed.payload).unwrap();
        assert_eq!(value["kind"], "password");
        assert_eq!(value["monica_entry_id"], full_logical.as_str());
        assert_eq!(payload::read_login_type(&value), "PASSWORD");
        // Neither purpose that discloses secrets reaches a plain password login either.
        assert!(matches!(
            vault.reveal_key(&full_id, None),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        assert!(matches!(
            vault.reveal(&full_id, RevealPurpose::GatewayToken, now()),
            Err(GatewayError::CredentialUnavailable)
        ));

        vault.lock().unwrap();
        // The synced plaintext never exists unencrypted beside the vault.
        let secret = "a sparse harmony secret";
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
        drop(directory);
    }
}
