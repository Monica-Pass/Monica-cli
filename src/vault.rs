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
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::config::{Connection, private_file};
use crate::error::{GatewayError, Result};
use crate::keys::id::{new_logical_entry_id, physical_entry_id};
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
/// Upper bound on login rows a single key listing will decrypt.
const MAX_KEY_SCAN_ENTRIES: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
}

struct Disclosed {
    payload: Zeroizing<Vec<u8>>,
    project_id: String,
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
        let disclosed = self.reveal(&binding.credential_id, RevealPurpose::GatewayToken, now)?;
        let stored: StoredCredential = serde_json::from_slice(&disclosed.payload)
            .map_err(|_| GatewayError::CredentialUnavailable)?;
        if stored.schema != CREDENTIAL_SCHEMA
            || stored.provider != binding.provider
            || stored.api_base != binding.api_base
            || stored.note != binding.note
        {
            return Err(GatewayError::CredentialUnavailable);
        }
        validate_token(&stored.token)?;
        validate_note(&stored.note)?;
        Ok((stored, disclosed.project_id))
    }

    /// The only path that turns stored ciphertext into plaintext. Every caller must state which
    /// kind of record it expects, and a record of another kind is refused before any bytes of it
    /// leave this function.
    fn reveal(&self, entry_id: &str, purpose: RevealPurpose, now: i64) -> Result<Disclosed> {
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
        };
        let mut connection = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
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
            _ => GatewayError::UnlockRequired,
        })?;
        if disclosed.object.entry_type != expected_type {
            return Err(match purpose {
                RevealPurpose::GatewayToken => GatewayError::CredentialUnavailable,
                RevealPurpose::KeyAdmin => GatewayError::KeyEntryTypeMismatch,
            });
        }
        Ok(Disclosed {
            payload: Zeroizing::new(disclosed.object.payload_ct),
            project_id: disclosed.object.project_id,
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
        let title = self.current_entry_title(&binding.credential_id, name);
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
                    title,
                    payload_json,
                }],
            ),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        let mut updated = binding.clone();
        updated.note = note.to_owned();
        Ok(updated)
    }

    /// Change only the display title, retaining the encrypted payload and identity.
    pub(crate) fn rename_entry(&self, binding: &Connection, title: &str) -> Result<()> {
        validate_title(title)?;
        let title = title.trim();
        let (stored, project_id) = self.reveal_stored(binding, chrono::Utc::now().timestamp())?;
        reject_secret_value(&serde_json::json!([title]), &stored.token)
            .map_err(|_| GatewayError::SensitiveMetadata)?;
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
                "gateway-rename-entry",
                vec![WriteCommand::UpdateEntry {
                    entry_id: binding.credential_id.clone(),
                    project_id,
                    entry_type: ObjectTypeId::ApiToken.to_string(),
                    title: title.to_owned(),
                    payload_json,
                }],
            ),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        Ok(())
    }

    /// Resolve the entry's stored display title, falling back to the handle when the entry is
    /// not yet visible in the local library. Editing note/token must never clobber a title.
    fn current_entry_title(&self, credential_id: &str, fallback: &str) -> String {
        self.library()
            .ok()
            .and_then(|library| {
                library
                    .entries
                    .into_iter()
                    .find(|entry| entry.id == credential_id)
                    .map(|entry| entry.title)
            })
            .unwrap_or_else(|| fallback.to_owned())
    }

    /// Local management of SSH and GPG entries. Nothing here is reachable from the broker: the
    /// gateway inventory only ever discloses API tokens.
    fn vault_id(&self) -> Result<String> {
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
        let disclosed = self.reveal(
            entry_id,
            RevealPurpose::KeyAdmin,
            chrono::Utc::now().timestamp(),
        )?;
        let text = std::str::from_utf8(&disclosed.payload)
            .map_err(|_| GatewayError::KeyEntryTypeMismatch)?;
        let value: Value =
            serde_json::from_str(text).map_err(|_| GatewayError::KeyEntryTypeMismatch)?;
        let stored = payload::read_login_type(&value);
        let known = matches!(stored.as_str(), LOGIN_TYPE_SSH | LOGIN_TYPE_GPG);
        if !known || login_type.is_some_and(|wanted| wanted != stored) {
            return Err(GatewayError::KeyEntryTypeMismatch);
        }
        Ok((value, disclosed.project_id))
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
        let (mut value, collection_id, current_title) = self.load_key(entry_id, None)?;
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
            let raw = payload::read_ssh_key_field(&value, "")?;
            let mut data = SshKeyData::decode(&raw)?.ok_or(GatewayError::InvalidKeyMaterial)?;
            data.public_key_openssh =
                openssh::public_line_with_comment(&data.public_key_openssh, comment)?;
            data.comment = comment.trim().to_owned();
            payload::set_ssh_key_data(&mut value, &data.to_json_string());
        }
        let payload_json = payload::serialize(&value)?;
        self.key_write(
            "keys-edit",
            vec![WriteCommand::UpdateEntry {
                entry_id: entry_id.to_owned(),
                project_id: collection_id.clone(),
                entry_type: LOGIN_ENTRY_TYPE.to_owned(),
                title: title.clone(),
                payload_json,
            }],
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
                "",
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
        let now = chrono::Utc::now().timestamp();
        assert_eq!(
            vault.credential(&after_token, now).unwrap().token.as_str(),
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
        let now = chrono::Utc::now().timestamp();
        assert_eq!(
            reopened.credential(&binding, now).unwrap().token.as_str(),
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
        let now = chrono::Utc::now().timestamp();
        let (mut stored, project_id) = vault.reveal_stored(&binding, now).unwrap();
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
        let now = chrono::Utc::now().timestamp();
        let data = ssh_data();
        let key = vault
            .add_key_entry(None, "laptop", "", NewKeyEntry::Ssh { data: &data })
            .unwrap();
        assert!(matches!(
            vault.reveal_key(&key.entry_id, Some(LOGIN_TYPE_GPG)),
            Err(GatewayError::KeyEntryTypeMismatch)
        ));
        assert!(matches!(
            vault.reveal(&key.entry_id, RevealPurpose::GatewayToken, now),
            Err(GatewayError::CredentialUnavailable)
        ));
        let binding = Connection {
            provider: Provider::Github,
            api_base: Provider::Github.default_api_base().to_owned(),
            credential_id: key.entry_id.clone(),
            note: String::new(),
        };
        assert!(matches!(
            vault.credential(&binding, now),
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
}
