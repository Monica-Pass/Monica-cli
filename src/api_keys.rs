//! Trusted binding of existing Android credentials to a bounded service proxy.
//! Source objects are read through Tiga and never converted, copied or rewritten.
use std::collections::BTreeMap;

use mdbx_core::model::{ObjectSummary, ObjectTypeId};
use mdbx_core::tiga::{DeviceAssurance, DeviceContext};
use mdbx_storage::object_disclosure::{ObjectDisclosureLimits, ObjectDisclosureService};
use mdbx_storage::repo::ObjectSummaryRepo;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use zeroize::Zeroizing;

use crate::config::{ConfigStore, Connection};
use crate::error::{GatewayError, Result};
use crate::model::{Provider, validate_api_base, validate_name, validate_note};
use crate::upstream::reject_secret_value;
use crate::vault::{Credential, Vault};

const MAX_PAYLOAD_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ApiProtocol {
    #[default]
    Openai,
    Anthropic,
}
impl ApiProtocol {
    pub fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Authentication {
    Bearer,
    XApiKey,
}

impl Authentication {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x-api-key",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    AndroidApiKey,
    NativeApiToken,
}

impl SourceFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::AndroidApiKey => "android_api_key",
            Self::NativeApiToken => "native_api_token",
        }
    }
    fn native_type(self) -> ObjectTypeId {
        match self {
            Self::AndroidApiKey => ObjectTypeId::Login,
            Self::NativeApiToken => ObjectTypeId::ApiToken,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyBinding {
    pub format: SourceFormat,
    pub protocol: ApiProtocol,
    pub auth: Authentication,
    pub head_commit_id: String,
}

impl ApiKeyBinding {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.head_commit_id.is_empty()
            || self.head_commit_id.len() > 256
            || self.head_commit_id.chars().any(char::is_control)
        {
            return Err(GatewayError::InvalidConfig);
        }
        Ok(())
    }
    pub(crate) fn matches(&self, summary: &ObjectSummary) -> bool {
        !summary.deleted
            && summary.payload_schema_version == 1
            && summary.object_type_id == self.format.native_type()
            && summary.head_commit_id == self.head_commit_id
    }
}

#[derive(clap::Args)]
pub struct BindOptions {
    /// Local connection name used by the AI; does not rename the Android entry.
    #[arg(value_name = "CONNECTION")]
    pub name: String,
    /// Existing native entry UUID from monica library.
    #[arg(long, value_name = "ENTRY_ID")]
    pub entry: String,
    /// How Monica injects the stored key. The AI cannot override authentication.
    #[arg(long, value_enum)]
    pub auth: Option<Authentication>,
    /// Native wire protocol used by the client and upstream; no protocol conversion.
    #[arg(long, value_enum, default_value = "openai")]
    pub protocol: ApiProtocol,
    /// HTTPS API root, required only when the stored API endpoint is empty.
    #[arg(long)]
    pub api_base: Option<String>,
    /// Explicit public purpose visible to the AI; Android notes stay private.
    #[arg(long, default_value = "")]
    pub note: String,
    /// Rebind an existing API-key connection and revoke all its previous grants.
    #[arg(long)]
    pub replace: bool,
}

/// Borrow all JSON values from the zeroized disclosure buffer. Reject duplicate
/// keys rather than interpreting routing/type/secret fields differently by client.
struct RawObject<'a>(BTreeMap<String, &'a RawValue>);
impl<'de> Deserialize<'de> for RawObject<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = RawObject<'de>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object with unique fields")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    if fields.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate field"));
                    }
                }
                Ok(RawObject(fields))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
impl RawObject<'_> {
    fn text(&self, key: &str) -> Result<Zeroizing<String>> {
        self.0
            .get(key)
            .map(|raw| {
                serde_json::from_str::<String>(raw.get())
                    .map(Zeroizing::new)
                    .map_err(|_| GatewayError::CredentialUnavailable)
            })
            .unwrap_or_else(|| Ok(Zeroizing::new(String::new())))
    }
}

// No Serialize or Debug: even endpoints may accidentally contain a secret.
struct Source {
    summary: ObjectSummary,
    format: SourceFormat,
    key: Zeroizing<String>,
    endpoint: Zeroizing<String>,
}

fn parse_payload(
    raw: &[u8],
    format: SourceFormat,
) -> Result<(Zeroizing<String>, Zeroizing<String>)> {
    let fields: RawObject<'_> =
        serde_json::from_slice(raw).map_err(|_| GatewayError::CredentialUnavailable)?;
    let (key, endpoint) = match format {
        SourceFormat::AndroidApiKey => {
            if fields.text("kind")?.as_str() != "password"
                || fields.text("login_type")?.as_str() != "API_KEY"
            {
                return Err(GatewayError::ObjectReadOnly);
            }
            // Explicit empty or null password_plain never falls back to an old password.
            let key = fields.text(if fields.0.contains_key("password_plain") {
                "password_plain"
            } else {
                "password"
            })?;
            let mut marker = None;
            let mut endpoint = None;
            if let Some(raw_fields) = fields.0.get("custom_fields") {
                let custom: Vec<&RawValue> = serde_json::from_str(raw_fields.get())
                    .map_err(|_| GatewayError::CredentialUnavailable)?;
                if custom.len() > 256 {
                    return Err(GatewayError::CredentialUnavailable);
                }
                for raw_field in custom {
                    let field: RawObject<'_> = serde_json::from_str(raw_field.get())
                        .map_err(|_| GatewayError::CredentialUnavailable)?;
                    let slot = match field.text("title")?.as_str() {
                        "monica_api_key_type" => &mut marker,
                        "monica_api_key_url" => &mut endpoint,
                        _ => continue,
                    };
                    if slot.replace(field.text("value")?).is_some() {
                        return Err(GatewayError::CredentialUnavailable);
                    }
                }
            }
            if marker
                .as_ref()
                .is_some_and(|value| value.as_str() != "API_KEY")
            {
                return Err(GatewayError::ObjectReadOnly);
            }
            (key, endpoint.unwrap_or_default())
        }
        SourceFormat::NativeApiToken => {
            if !matches!(
                fields.text("schema")?.as_str(),
                "monica.api-token.v1" | "monica.gateway.credential.v1"
            ) {
                return Err(GatewayError::ObjectReadOnly);
            }
            if fields.text("provider")?.trim().is_empty() {
                return Err(GatewayError::CredentialUnavailable);
            }
            (fields.text("token")?, fields.text("api_base")?)
        }
    };
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|c| (33..=126).contains(&c)) {
        return Err(GatewayError::CredentialUnavailable);
    }
    Ok((key, endpoint))
}

fn normalized_base(value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(char::is_control) || value.contains('\\') {
        return Err(GatewayError::InvalidConfig);
    }
    let value = if value.ends_with('/') {
        value.to_owned()
    } else {
        format!("{value}/")
    };
    Ok(validate_api_base(&value, Provider::ApiKey)?.to_string())
}

impl Vault {
    fn api_key_source(
        &self,
        id: &str,
        expected: Option<&ApiKeyBinding>,
        now: i64,
    ) -> Result<Source> {
        let mut conn = self
            .runtime
            .write()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if conn.keyring().is_none() || conn.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let summary = ObjectSummaryRepo::get(&conn, id)
            .map_err(|_| GatewayError::StateUnavailable)?
            .filter(|s| !s.deleted)
            .ok_or(GatewayError::NotFound)?;
        if summary.payload_schema_version != 1 {
            return Err(GatewayError::ObjectReadOnly);
        }
        if expected.is_some_and(|source| !source.matches(&summary)) {
            return Err(GatewayError::ObjectChanged);
        }
        let format = match summary.object_type_id {
            ObjectTypeId::Login => SourceFormat::AndroidApiKey,
            ObjectTypeId::ApiToken => SourceFormat::NativeApiToken,
            _ => return Err(GatewayError::ObjectReadOnly),
        };
        let disclosed = ObjectDisclosureService::reveal_with_active_session_and_limits(
            &mut conn,
            id,
            &DeviceContext {
                device_id: Some("monica-api-key-broker".to_owned()),
                assurance: DeviceAssurance::Standard,
                ..Default::default()
            },
            now,
            ObjectDisclosureLimits::new(MAX_PAYLOAD_BYTES)
                .map_err(|_| GatewayError::StateUnavailable)?,
        )
        .map_err(|error| match error {
            mdbx_storage::error::StorageError::ResourceLimit { .. } => {
                GatewayError::ObjectPayloadTooLarge
            }
            _ => GatewayError::UnlockRequired,
        })?;
        let raw = Zeroizing::new(disclosed.object.payload_ct);
        if disclosed.object.head_commit_id != summary.head_commit_id
            || disclosed.object.project_id != summary.collection_id
            || disclosed.object.entry_type != summary.object_type_id
            || disclosed.object.payload_schema_version != summary.payload_schema_version
        {
            return Err(GatewayError::ObjectChanged);
        }
        let (key, endpoint) = parse_payload(&raw, format)?;
        Ok(Source {
            summary,
            format,
            key,
            endpoint,
        })
    }

    pub(crate) fn bound_api_key(&self, binding: &Connection, now: i64) -> Result<Credential> {
        let expected = binding
            .api_key
            .as_ref()
            .ok_or(GatewayError::InvalidConfig)?;
        let source = self.api_key_source(&binding.credential_id, Some(expected), now)?;
        if !source.endpoint.is_empty() && normalized_base(&source.endpoint)? != binding.api_base {
            return Err(GatewayError::ObjectChanged);
        }
        reject_secret_value(
            &serde_json::json!([binding.api_base, binding.note]),
            &source.key,
        )
        .map_err(|_| GatewayError::SensitiveMetadata)?;
        Ok(Credential { token: source.key })
    }
}

/// Local setup only. Existing Android payloads and labels remain untouched.
pub fn bind(
    store: &ConfigStore,
    options: &BindOptions,
    password: &str,
) -> Result<(Connection, usize)> {
    validate_name(&options.name)?;
    validate_note(&options.note)?;
    uuid::Uuid::parse_str(&options.entry).map_err(|_| GatewayError::InvalidRequest)?;
    let _guard = store.acquire_broker_lock()?;
    store.update(|previous| {
        let mut config = previous.ok_or(GatewayError::NotFound)?;
        if let Some(existing) = config.connections.get(&options.name) {
            if !options.replace || existing.api_key.is_none() {
                return Err(GatewayError::AlreadyExists);
            }
        } else if options.replace {
            return Err(GatewayError::NotFound);
        }
        let vault = Vault::open(&config.vault, password)?;
        let source = vault.api_key_source(&options.entry, None, chrono::Utc::now().timestamp());
        vault.lock()?;
        let source = source?;
        let stored_base = (!source.endpoint.is_empty())
            .then(|| normalized_base(&source.endpoint))
            .transpose()?;
        let supplied = options
            .api_base
            .as_deref()
            .map(normalized_base)
            .transpose()?;
        if stored_base
            .as_ref()
            .zip(supplied.as_ref())
            .is_some_and(|(a, b)| a != b)
        {
            return Err(GatewayError::InvalidConfig);
        }
        let base = stored_base
            .or(supplied)
            .ok_or(GatewayError::InvalidConfig)?;
        let public = serde_json::json!([options.name, options.entry, options.note, base]);
        for secret in [password, source.key.as_str()] {
            reject_secret_value(&public, secret).map_err(|_| GatewayError::SensitiveMetadata)?;
        }
        let binding = Connection {
            provider: Provider::ApiKey,
            credential_id: options.entry.clone(),
            api_base: base,
            note: options.note.clone(),
            api_key: Some(ApiKeyBinding {
                format: source.format,
                protocol: options.protocol,
                auth: options.auth.unwrap_or(match options.protocol {
                    ApiProtocol::Openai => Authentication::Bearer,
                    ApiProtocol::Anthropic => Authentication::XApiKey,
                }),
                head_commit_id: source.summary.head_commit_id,
            }),
        };
        binding
            .api_key
            .as_ref()
            .ok_or(GatewayError::InvalidConfig)?
            .validate()?;
        let before = config.grants.len();
        config
            .grants
            .retain(|grant| grant.connection != options.name);
        let revoked = before - config.grants.len();
        config
            .connections
            .insert(options.name.clone(), binding.clone());
        Ok((config, (binding, revoked)))
    })
}

/// Remove only the local binding and its grants, including a stale/deleted source.
pub fn unbind(store: &ConfigStore, name: &str, password: &str) -> Result<usize> {
    validate_name(name)?;
    let _guard = store.acquire_broker_lock()?;
    store.update(|previous| {
        let mut config = previous.ok_or(GatewayError::NotFound)?;
        let binding = config.connections.get(name).ok_or(GatewayError::NotFound)?;
        if binding.api_key.is_none() {
            return Err(GatewayError::InvalidRequest);
        }
        let vault = Vault::open(&config.vault, password)?;
        vault.lock()?;
        config.connections.remove(name);
        let before = config.grants.len();
        config.grants.retain(|grant| grant.connection != name);
        let revoked = before - config.grants.len();
        Ok((config, revoked))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ordinary_logins_ambiguous_fields_and_future_token_schemas() {
        for raw in [
            r#"{"kind":"password","login_type":"PASSWORD","password_plain":"synthetic-key"}"#,
            r#"{"kind":"password","login_type":"API_KEY","password_plain":"","password":"old-secret"}"#,
            r#"{"kind":"password","login_type":"API_KEY","password_plain":null,"password":"old-secret"}"#,
            r#"{"kind":"password","login_type":"API_KEY","password_plain":"key-one","password_plain":"key-two"}"#,
            r#"{"kind":"password","login_type":"API_KEY","password_plain":"key","custom_fields":[{"title":"monica_api_key_url","value":"https://one.test"},{"title":"monica_api_key_url","value":"https://two.test"}]}"#,
            r#"{"kind":"password","login_type":"API_KEY","password_plain":"key","custom_fields":[{"title":"monica_api_key_type","value":"PASSWORD"}]}"#,
        ] {
            assert!(parse_payload(raw.as_bytes(), SourceFormat::AndroidApiKey).is_err());
        }
        for raw in [
            r#"{"schema":"monica.api-token.v2","provider":"openai","token":"key"}"#,
            r#"{"schema":"monica.api-token.v1","provider":"","token":"key"}"#,
            r#"{"schema":"monica.api-token.v1","provider":"openai","token":"key\r\nInjected: value"}"#,
        ] {
            assert!(parse_payload(raw.as_bytes(), SourceFormat::NativeApiToken).is_err());
        }
        let (key, endpoint) = parse_payload(
            br#"{"kind":"password","login_type":"API_KEY","password":"old-secret"}"#,
            SourceFormat::AndroidApiKey,
        )
        .unwrap();
        assert_eq!(key.as_str(), "old-secret");
        assert!(endpoint.is_empty());
    }

    #[test]
    fn endpoints_require_https_and_cannot_embed_auth_query_or_fragment() {
        for base in [
            "http://example.test",
            "https://user:password@example.test",
            "https://example.test/?key=secret",
            "https://example.test/#secret",
            "https://example.test/\\evil",
            "",
        ] {
            assert!(normalized_base(base).is_err());
        }
        assert_eq!(
            normalized_base("https://example.test/v1").unwrap(),
            "https://example.test/v1/"
        );
    }
}
