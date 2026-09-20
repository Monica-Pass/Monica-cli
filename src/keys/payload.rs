//! The shared payload contract Monica for Android uses for key entries.
//!
//! Every rule here mirrors a specific Android source; the `ssh-gpg-android-cli-compatibility.md`
//! handoff documents the same contract from the other side. Payload edits are read-modify-write
//! on a `serde_json::Value`, because Android re-saves a login payload from its own columns and
//! the CLI must not be the side that wipes a field it does not model.
use serde_json::{Map, Value, json};

use super::limits::{GPG_PUBLIC_CHUNK_CHARS, MAX_GPG_IMPORT_BYTES, MAX_KEY_ENTRY_PAYLOAD_BYTES};
use crate::error::{GatewayError, Result};

/// Native entry type of every key entry: a plain login, discriminated by `login_type`.
pub const LOGIN_ENTRY_TYPE: &str = "login";
pub const LOGIN_TYPE_SSH: &str = "SSH_KEY";
pub const LOGIN_TYPE_GPG: &str = "GPG_KEY";

/// Android's `MdbxKeyPayload` prefers `ssh_key_data` and tolerates the camelCase alias on import.
pub const SSH_KEY_FIELD: &str = "ssh_key_data";
pub const SSH_KEY_FIELD_ALIAS: &str = "sshKeyData";

/// `GpgEntryFields`: public material only. The private armor lives in `password_plain`.
pub const GPG_MARKER: &str = "monica_gpg_type";
pub const GPG_FINGERPRINT: &str = "monica_gpg_fingerprint";
pub const GPG_USER_ID: &str = "monica_gpg_user_id";
pub const GPG_ENCODING: &str = "monica_gpg_encoding";
pub const GPG_PUBLIC_PREFIX: &str = "monica_gpg_public_";
pub const GPG_ENCODING_BASE64: &str = "base64";
pub const GPG_TYPE_VALUE: &str = LOGIN_TYPE_GPG;

/// A Room password row carries these ids; foreign entries have no Room row.
const ROOM_ID: i64 = 0;
const SORT_ORDER: i64 = 0;

/// The key set Android always writes for a login payload.
///
/// Anything Android models as nullable is left absent instead of `null`, which is what
/// `org.json.JSONObject.put(name, null)` does.
pub fn new_login_payload(logical_entry_id: &str, login_type: &str) -> Value {
    json!({
        "kind": "password",
        "monica_entry_id": logical_entry_id,
        "room_id": ROOM_ID,
        "website": "",
        "username": "",
        "app_package_name": "",
        "app_name": "",
        "password_plain": "",
        "notes": "",
        "sort_order": SORT_ORDER,
        "login_type": login_type,
        "ssh_key_data": "",
        "authenticator_key": "",
        "passkey_bindings": "",
        "custom_fields": [],
        "bitwarden_mode": false,
        "keepass_mode": false,
    })
}

/// `optString` semantics: absent or JSON null reads as an empty string.
pub fn read_string(payload: &Value, key: &str) -> String {
    match payload.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => String::new(),
        Some(_) => String::new(),
    }
}

pub fn read_login_type(payload: &Value) -> String {
    let value = read_string(payload, "login_type");
    if value.is_empty() {
        "PASSWORD".to_owned()
    } else {
        value
    }
}

/// Writes the native folder pointer. Root, blank and the literal `"root"` all mean "no key",
/// matching `JSONObject.optMdbxFolderId` and the root fallback in `Mdbx2Repository`.
pub fn set_folder(payload: &mut Value, folder_id: Option<&str>) {
    let Some(map) = payload.as_object_mut() else {
        return;
    };
    match folder_id
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.eq_ignore_ascii_case("root"))
    {
        Some(id) => {
            map.insert("mdbx_folder_id".to_owned(), Value::String(id.to_owned()));
        }
        None => {
            map.remove("mdbx_folder_id");
            map.remove("category_id");
        }
    }
}

/// Serializes a payload only if the whole entry still fits the key budget.
pub fn serialize(payload: &Value) -> Result<String> {
    let text = serde_json::to_string(payload).map_err(|_| GatewayError::StateUnavailable)?;
    if text.len() > MAX_KEY_ENTRY_PAYLOAD_BYTES {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    Ok(text)
}

/// Port of `readMdbxSshKeyData`: snake_case wins, the camelCase alias is accepted, and a
/// missing key means "keep what is already stored" rather than "clear it".
pub fn read_ssh_key_field(payload: &Value, existing: &str) -> Result<String> {
    let chosen = [SSH_KEY_FIELD, SSH_KEY_FIELD_ALIAS]
        .into_iter()
        .find(|key| matches!(payload.get(*key), Some(value) if !value.is_null()));
    let Some(key) = chosen else {
        return Ok(existing.to_owned());
    };
    match payload.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        Some(Value::Object(value)) => {
            serde_json::to_string(value).map_err(|_| GatewayError::InvalidKeyMaterial)
        }
        _ => Err(GatewayError::InvalidKeyMaterial),
    }
}

/// Writes `ssh_key_data` and drops the alias so the two never disagree later.
pub fn set_ssh_key_data(payload: &mut Value, raw: &str) {
    if let Some(map) = payload.as_object_mut() {
        map.insert(SSH_KEY_FIELD.to_owned(), Value::String(raw.to_owned()));
        map.remove(SSH_KEY_FIELD_ALIAS);
    }
}

/// The eight keys `SshKeyData` models, plus everything else the producer carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshKeyData {
    pub algorithm: String,
    pub key_size: i64,
    pub public_key_openssh: String,
    pub private_key_openssh: String,
    pub fingerprint_sha256: String,
    pub comment: String,
    pub format: String,
    pub schema: String,
    pub additional: Map<String, Value>,
}

impl Default for SshKeyData {
    fn default() -> Self {
        Self {
            algorithm: String::new(),
            key_size: 0,
            public_key_openssh: String::new(),
            private_key_openssh: String::new(),
            fingerprint_sha256: String::new(),
            comment: String::new(),
            format: super::openssh::FORMAT_OPENSSH.to_owned(),
            schema: super::openssh::SCHEMA_V1.to_owned(),
            additional: Map::new(),
        }
    }
}

const KNOWN_SSH_KEYS: [&str; 8] = [
    "algorithm",
    "keySize",
    "publicKeyOpenSsh",
    "privateKeyOpenSsh",
    "fingerprintSha256",
    "comment",
    "format",
    "schema",
];

impl SshKeyData {
    /// Android treats a record with no algorithm and no key text as "no SSH key".
    pub fn is_empty(&self) -> bool {
        self.algorithm.trim().is_empty()
            && self.public_key_openssh.trim().is_empty()
            && self.private_key_openssh.trim().is_empty()
            && self.fingerprint_sha256.trim().is_empty()
    }

    pub fn to_value(&self) -> Value {
        let mut map = self.additional.clone();
        map.insert("algorithm".to_owned(), json!(self.algorithm));
        map.insert("keySize".to_owned(), json!(self.key_size));
        map.insert(
            "publicKeyOpenSsh".to_owned(),
            json!(self.public_key_openssh),
        );
        map.insert(
            "privateKeyOpenSsh".to_owned(),
            json!(self.private_key_openssh),
        );
        map.insert(
            "fingerprintSha256".to_owned(),
            json!(self.fingerprint_sha256),
        );
        map.insert("comment".to_owned(), json!(self.comment));
        map.insert("format".to_owned(), json!(self.format));
        map.insert("schema".to_owned(), json!(self.schema));
        Value::Object(map)
    }

    pub fn to_json_string(&self) -> String {
        // `encodeDefaults` in Kotlin: all eight keys are always present on the wire.
        serde_json::to_string(&self.to_value()).unwrap_or_default()
    }

    pub fn from_value(value: &Value) -> Result<Self> {
        let map = value.as_object().ok_or(GatewayError::InvalidKeyMaterial)?;
        let text = |key: &str, fallback: &str| -> Result<String> {
            match map.get(key) {
                None | Some(Value::Null) => Ok(fallback.to_owned()),
                Some(Value::String(value)) => Ok(value.clone()),
                Some(_) => Err(GatewayError::InvalidKeyMaterial),
            }
        };
        let key_size = match map.get("keySize") {
            None | Some(Value::Null) => 0,
            Some(Value::Number(number)) => {
                number.as_i64().ok_or(GatewayError::InvalidKeyMaterial)?
            }
            Some(_) => return Err(GatewayError::InvalidKeyMaterial),
        };
        let additional = map
            .iter()
            .filter(|(key, _)| !KNOWN_SSH_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(Self {
            algorithm: text("algorithm", "")?,
            key_size,
            public_key_openssh: text("publicKeyOpenSsh", "")?,
            private_key_openssh: text("privateKeyOpenSsh", "")?,
            fingerprint_sha256: text("fingerprintSha256", "")?,
            comment: text("comment", "")?,
            format: text("format", super::openssh::FORMAT_OPENSSH)?,
            // Old records carry no schema and are read as v1.
            schema: text("schema", super::openssh::SCHEMA_V1)?,
            additional,
        })
    }

    /// Unlike Android, a malformed inner record is refused rather than silently turned into
    /// an empty field, which is how a decode error would otherwise destroy the stored key.
    pub fn decode(raw: &str) -> Result<Option<Self>> {
        if raw.trim().is_empty() {
            return Ok(None);
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|_| GatewayError::InvalidKeyMaterial)?;
        let data = Self::from_value(&value)?;
        Ok((!data.is_empty()).then_some(data))
    }

    pub fn encode(data: Option<&Self>) -> String {
        data.filter(|key| !key.is_empty())
            .map(SshKeyData::to_json_string)
            .unwrap_or_default()
    }
}

/// Public half of a GPG entry: the certificate plus the metadata Android shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpgCertificate {
    pub fingerprint: String,
    pub user_id: String,
    pub public_armor: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CustomField {
    title: String,
    value: String,
    is_protected: bool,
}

fn custom_fields(payload: &Value) -> Vec<CustomField> {
    payload
        .get("custom_fields")
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|item| {
                    let item = item.as_object()?;
                    let title = item.get("title")?.as_str()?.to_owned();
                    if title.trim().is_empty() {
                        return None; // Android skips blank titles on import.
                    }
                    let value = item
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let is_protected = item
                        .get("is_protected")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(CustomField {
                        title,
                        value,
                        is_protected,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_custom_fields(payload: &mut Value, fields: Vec<CustomField>) {
    let array = fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            json!({
                "title": field.title,
                "value": field.value,
                "is_protected": field.is_protected,
                "sort_order": index,
            })
        })
        .collect::<Vec<_>>();
    if let Some(map) = payload.as_object_mut() {
        map.insert("custom_fields".to_owned(), Value::Array(array));
    }
}

fn chunk_title(index: usize) -> String {
    format!("{GPG_PUBLIC_PREFIX}{index:04}")
}

/// Encodes a certificate into the marker, metadata and contiguous Base64 chunks.
fn encode_gpg_fields(certificate: &GpgCertificate) -> Result<Vec<CustomField>> {
    let armor = certificate.public_armor.as_bytes();
    if armor.is_empty() || armor.len() > MAX_GPG_IMPORT_BYTES {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    let mut fields = vec![
        CustomField {
            title: GPG_MARKER.to_owned(),
            value: GPG_TYPE_VALUE.to_owned(),
            is_protected: false,
        },
        CustomField {
            title: GPG_FINGERPRINT.to_owned(),
            value: certificate.fingerprint.clone(),
            is_protected: false,
        },
        CustomField {
            title: GPG_USER_ID.to_owned(),
            value: certificate.user_id.clone(),
            is_protected: false,
        },
        CustomField {
            title: GPG_ENCODING.to_owned(),
            value: GPG_ENCODING_BASE64.to_owned(),
            is_protected: false,
        },
    ];
    // One Base64 pass over the whole armor, then fixed-size chunks: chunk-local encoding would
    // not survive concatenation on the read side.
    let encoded = base64_encode(armor);
    for (index, part) in encoded
        .as_bytes()
        .chunks(GPG_PUBLIC_CHUNK_CHARS)
        .enumerate()
    {
        fields.push(CustomField {
            title: chunk_title(index),
            value: String::from_utf8_lossy(part).into_owned(),
            is_protected: false,
        });
    }
    Ok(fields)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(text: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| GatewayError::InvalidKeyMaterial)
}

/// Reads back what `encode_gpg_fields` wrote, including the older unencoded chunk form.
fn decode_gpg_fields(fields: &[CustomField]) -> Result<Option<GpgCertificate>> {
    if !fields
        .iter()
        .any(|field| field.title == GPG_MARKER && field.value.eq_ignore_ascii_case(GPG_TYPE_VALUE))
    {
        return Ok(None);
    }
    let get = |title: &str| {
        fields
            .iter()
            .rev()
            .find(|field| field.title == title)
            .map(|field| field.value.clone())
            .unwrap_or_default()
    };
    let mut chunks = fields
        .iter()
        .filter(|field| field.title.starts_with(GPG_PUBLIC_PREFIX))
        .map(|field| (field.title.clone(), field.value.clone()))
        .collect::<Vec<_>>();
    chunks.sort_by(|left, right| left.0.cmp(&right.0));
    if chunks.is_empty() {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    for (index, (title, _)) in chunks.iter().enumerate() {
        if *title != chunk_title(index) {
            return Err(GatewayError::InvalidKeyMaterial); // Gaps would silently truncate the key.
        }
    }
    let stored = chunks
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<String>();
    let encoding = get(GPG_ENCODING);
    let bytes = match encoding.as_str() {
        GPG_ENCODING_BASE64 => base64_decode(stored.trim())?,
        // Earlier Android builds stored the armor text in chunks with no encoding marker.
        "" => stored.into_bytes(),
        _ => return Err(GatewayError::InvalidKeyMaterial),
    };
    if bytes.is_empty() || bytes.len() > MAX_GPG_IMPORT_BYTES {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    let public_armor = String::from_utf8(bytes).map_err(|_| GatewayError::InvalidKeyMaterial)?;
    Ok(Some(GpgCertificate {
        fingerprint: get(GPG_FINGERPRINT),
        user_id: get(GPG_USER_ID),
        public_armor,
    }))
}

/// Replaces the certificate, dropping every stale chunk so a shorter key cannot leave the
/// tail of a longer one behind. Unrelated custom fields keep their values and relative order.
pub fn apply_gpg_certificate(payload: &mut Value, certificate: &GpgCertificate) -> Result<()> {
    let current = custom_fields(payload);
    let gpg_owns = |title: &str| {
        title == GPG_MARKER
            || title == GPG_FINGERPRINT
            || title == GPG_USER_ID
            || title == GPG_ENCODING
            || title.starts_with(GPG_PUBLIC_PREFIX)
    };
    let mut kept = current
        .into_iter()
        .filter(|field| !gpg_owns(&field.title))
        .collect::<Vec<_>>();
    let mut next = Vec::with_capacity(kept.len() + 4);
    next.extend(encode_gpg_fields(certificate)?);
    next.append(&mut kept);
    write_custom_fields(payload, next);
    Ok(())
}

pub fn read_gpg_certificate(payload: &Value) -> Result<Option<GpgCertificate>> {
    decode_gpg_fields(&custom_fields(payload))
}

/// How many public chunks a stored certificate occupies, without decoding them.
pub fn gpg_chunk_count(payload: &Value) -> usize {
    custom_fields(payload)
        .iter()
        .filter(|field| field.title.starts_with(GPG_PUBLIC_PREFIX))
        .count()
}

/// Removes the GPG projection, used when an entry stops being a GPG record.
pub fn clear_gpg_certificate(payload: &mut Value) {
    let current = custom_fields(payload);
    let gpg_owns = |title: &str| {
        title == GPG_MARKER
            || title == GPG_FINGERPRINT
            || title == GPG_USER_ID
            || title == GPG_ENCODING
            || title.starts_with(GPG_PUBLIC_PREFIX)
    };
    let kept = current
        .into_iter()
        .filter(|field| !gpg_owns(&field.title))
        .collect::<Vec<_>>();
    write_custom_fields(payload, kept);
}

pub fn set_password_plain(payload: &mut Value, value: &str) {
    if let Some(map) = payload.as_object_mut() {
        map.insert("password_plain".to_owned(), Value::String(value.to_owned()));
    }
}

pub fn set_notes(payload: &mut Value, value: &str) {
    if let Some(map) = payload.as_object_mut() {
        map.insert("notes".to_owned(), Value::String(value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> Value {
        new_login_payload(
            "password:22222222-2222-2222-2222-222222222222",
            LOGIN_TYPE_SSH,
        )
    }

    #[test]
    fn login_payload_carries_the_android_key_set() {
        let value = payload();
        for key in [
            "kind",
            "monica_entry_id",
            "room_id",
            "website",
            "username",
            "app_package_name",
            "app_name",
            "password_plain",
            "notes",
            "sort_order",
            "login_type",
            "ssh_key_data",
            "authenticator_key",
            "passkey_bindings",
            "custom_fields",
            "bitwarden_mode",
            "keepass_mode",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert_eq!(value["kind"], "password");
        assert_eq!(value["room_id"], 0);
        assert_eq!(value["login_type"], LOGIN_TYPE_SSH);
        assert!(value.get("category_id").is_none());
        assert!(value.get("mdbx_folder_id").is_none());
    }

    #[test]
    fn root_folder_writes_no_pointer() {
        let mut value = payload();
        set_folder(&mut value, Some("  "));
        assert!(value.get("mdbx_folder_id").is_none());
        set_folder(&mut value, Some("ROOT"));
        assert!(value.get("mdbx_folder_id").is_none());
        set_folder(&mut value, Some("aaaaaaaa-1111-1111-1111-111111111111"));
        assert_eq!(
            value["mdbx_folder_id"],
            "aaaaaaaa-1111-1111-1111-111111111111"
        );
        set_folder(&mut value, None);
        assert!(value.get("mdbx_folder_id").is_none());
    }

    #[test]
    fn ssh_field_alias_rules_match_android() {
        let mut value = payload();
        // Absent keys keep the stored value instead of clearing it.
        value.as_object_mut().unwrap().remove(SSH_KEY_FIELD);
        assert_eq!(read_ssh_key_field(&value, "keep").unwrap(), "keep");
        // Explicit empty string clears.
        value[SSH_KEY_FIELD] = json!("");
        assert_eq!(read_ssh_key_field(&value, "keep").unwrap(), "");
        // snake_case wins when both are present.
        value[SSH_KEY_FIELD_ALIAS] = json!("alias");
        value[SSH_KEY_FIELD] = json!("preferred");
        assert_eq!(read_ssh_key_field(&value, "keep").unwrap(), "preferred");
        // An object value is accepted and stringified.
        value.as_object_mut().unwrap().remove(SSH_KEY_FIELD);
        value[SSH_KEY_FIELD_ALIAS] = json!({ "b": 2, "a": 1 });
        assert_eq!(
            read_ssh_key_field(&value, "keep").unwrap(),
            "{\"a\":1,\"b\":2}"
        );
        // Any other type rejects the import.
        value[SSH_KEY_FIELD_ALIAS] = json!(7);
        assert_eq!(
            read_ssh_key_field(&value, "keep").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        // A null snake_case key falls through to the alias, not to an error.
        value[SSH_KEY_FIELD] = Value::Null;
        value[SSH_KEY_FIELD_ALIAS] = json!("alias-only");
        assert_eq!(read_ssh_key_field(&value, "keep").unwrap(), "alias-only");
    }

    #[test]
    fn writing_ssh_data_leaves_no_alias_behind() {
        let mut value = payload();
        value[SSH_KEY_FIELD_ALIAS] = json!("stale");
        set_ssh_key_data(&mut value, "text");
        assert_eq!(value[SSH_KEY_FIELD], "text");
        assert!(value.get(SSH_KEY_FIELD_ALIAS).is_none());
    }

    #[test]
    fn unknown_inner_ssh_properties_survive_a_round_trip() {
        let raw = r#"{"algorithm":"ED25519","keySize":256,"publicKeyOpenSsh":"ssh-ed25519 AAAA",
            "privateKeyOpenSsh":"-----BEGIN OPENSSH PRIVATE KEY-----\n","fingerprintSha256":"SHA256:x",
            "comment":"a@b","format":"OPENSSH","schema":"monica.ssh-key.v1",
            "customExtension":{"kept":true},"futureField":"yes"}"#;
        let key = SshKeyData::decode(raw)
            .unwrap()
            .expect("a populated record");
        assert_eq!(key.comment, "a@b");
        let again = SshKeyData::decode(&key.to_json_string())
            .unwrap()
            .expect("still populated");
        assert_eq!(key, again);
        assert_eq!(again.additional["futureField"], json!("yes"));
        assert_eq!(again.additional["customExtension"]["kept"], json!(true));
        // Android re-emits all eight known keys even when the source omitted them.
        let sparse = SshKeyData::decode(r#"{"algorithm":"RSA","keySize":2048,"privateKeyOpenSsh":"p","fingerprintSha256":"SHA256:y","publicKeyOpenSsh":"ssh-rsa z"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(sparse.schema, super::super::openssh::SCHEMA_V1);
        assert_eq!(sparse.format, super::super::openssh::FORMAT_OPENSSH);
        assert_eq!(
            serde_json::from_str::<Value>(&sparse.to_json_string())
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            8
        );
    }

    #[test]
    fn empty_and_malformed_ssh_records() {
        assert!(SshKeyData::decode("").unwrap().is_none());
        assert!(
            SshKeyData::decode(r#"{"comment":"only a note"}"#)
                .unwrap()
                .is_none()
        );
        assert_eq!(SshKeyData::encode(None), "");
        assert!(SshKeyData::decode("not json").is_err());
        assert!(SshKeyData::decode(r#"{"algorithm":"ED25519","keySize":"wide"}"#).is_err());
        assert_eq!(
            SshKeyData::decode("[1,2]").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
    }

    #[test]
    fn gpg_chunks_are_contiguous_and_rejoinable() {
        let armor = format!(
            "-----BEGIN PGP PUBLIC KEY BLOCK-----\n{}\n-----END PGP PUBLIC KEY BLOCK-----\n",
            "A".repeat(4_500)
        );
        let certificate = GpgCertificate {
            fingerprint: "A".repeat(40),
            user_id: "Monica <monica@example.com>".to_owned(),
            public_armor: armor.clone(),
        };
        let mut value = new_login_payload("password:1", LOGIN_TYPE_GPG);
        apply_gpg_certificate(&mut value, &certificate).unwrap();
        let fields = custom_fields(&value);
        let chunks = fields
            .iter()
            .filter(|field| field.title.starts_with(GPG_PUBLIC_PREFIX))
            .count();
        assert_eq!(chunks, 4); // ceil(4500+ / 2000) on the Base64 length, not the armor length.
        assert_eq!(
            read_gpg_certificate(&value).unwrap().unwrap().public_armor,
            armor
        );
        assert_eq!(fields[0].title, GPG_MARKER);
        assert_eq!(fields[3].title, GPG_ENCODING);
        for (index, field) in fields.iter().enumerate() {
            assert!(!field.value.contains('\n'), "{}", field.title);
            assert_eq!(
                value["custom_fields"][index]["sort_order"].as_i64(),
                Some(index as i64)
            );
        }
    }

    #[test]
    fn shorter_certificate_removes_stale_chunks() {
        let long = GpgCertificate {
            fingerprint: "B".repeat(40),
            user_id: "long".to_owned(),
            public_armor: "L".repeat(6_000),
        };
        let short = GpgCertificate {
            fingerprint: "C".repeat(40),
            user_id: "short".to_owned(),
            public_armor: "S".repeat(100),
        };
        let mut value = new_login_payload("password:1", LOGIN_TYPE_GPG);
        apply_gpg_certificate(&mut value, &long).unwrap();
        let unrelated = CustomField {
            title: "url".to_owned(),
            value: "https://example.com".to_owned(),
            is_protected: false,
        };
        let mut fields = custom_fields(&value);
        fields.push(unrelated.clone());
        write_custom_fields(&mut value, fields);
        apply_gpg_certificate(&mut value, &short).unwrap();
        let after = custom_fields(&value);
        assert_eq!(
            after
                .iter()
                .filter(|field| field.title.starts_with(GPG_PUBLIC_PREFIX))
                .count(),
            1
        );
        assert_eq!(after.last().unwrap().title, unrelated.title);
        assert_eq!(
            read_gpg_certificate(&value).unwrap().unwrap().public_armor,
            "S".repeat(100)
        );
        assert_eq!(
            read_gpg_certificate(&value).unwrap().unwrap().fingerprint,
            "C".repeat(40)
        );
    }

    #[test]
    fn gpg_field_gaps_and_unknown_encodings_are_refused() {
        let mut value = new_login_payload("password:1", LOGIN_TYPE_GPG);
        apply_gpg_certificate(
            &mut value,
            &GpgCertificate {
                fingerprint: "D".repeat(40),
                user_id: "u".to_owned(),
                public_armor: "armored".to_owned(),
            },
        )
        .unwrap();
        let mut fields = custom_fields(&value);
        fields.retain(|field| field.title != chunk_title(0));
        assert_eq!(
            decode_gpg_fields(&fields).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        let mut fields = custom_fields(&value);
        fields.retain(|field| field.title != GPG_MARKER);
        assert_eq!(decode_gpg_fields(&fields).unwrap(), None);
        let mut fields = custom_fields(&value);
        for field in &mut fields {
            if field.title == GPG_ENCODING {
                field.value = "hex".to_owned();
            }
        }
        assert_eq!(
            decode_gpg_fields(&fields).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
    }

    #[test]
    fn legacy_unencoded_chunks_still_read_back() {
        let mut fields = vec![
            CustomField {
                title: GPG_MARKER.to_owned(),
                value: GPG_TYPE_VALUE.to_owned(),
                is_protected: false,
            },
            CustomField {
                title: chunk_title(0),
                value: "-----BEGIN PGP".to_owned(),
                is_protected: false,
            },
            CustomField {
                title: chunk_title(1),
                value: " PUBLIC KEY BLOCK-----".to_owned(),
                is_protected: false,
            },
        ];
        let certificate = decode_gpg_fields(&fields).unwrap().unwrap();
        assert_eq!(
            certificate.public_armor,
            "-----BEGIN PGP PUBLIC KEY BLOCK-----"
        );
        let legacy = fields
            .iter_mut()
            .find(|field| field.title == chunk_title(0))
            .unwrap();
        legacy.value = "!!!not base64!!!".to_owned();
        fields.push(CustomField {
            title: GPG_ENCODING.to_owned(),
            value: GPG_ENCODING_BASE64.to_owned(),
            is_protected: false,
        });
        assert_eq!(
            decode_gpg_fields(&fields).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
    }

    #[test]
    fn clearing_gpg_fields_keeps_unrelated_metadata() {
        let mut value = new_login_payload("password:1", LOGIN_TYPE_GPG);
        apply_gpg_certificate(
            &mut value,
            &GpgCertificate {
                fingerprint: "E".repeat(40),
                user_id: "u".to_owned(),
                public_armor: "armor".to_owned(),
            },
        )
        .unwrap();
        let mut fields = custom_fields(&value);
        fields.push(CustomField {
            title: "keep".to_owned(),
            value: "yes".to_owned(),
            is_protected: true,
        });
        write_custom_fields(&mut value, fields);
        clear_gpg_certificate(&mut value);
        let after = custom_fields(&value);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].title, "keep");
        assert!(after[0].is_protected);
        assert!(read_gpg_certificate(&value).unwrap().is_none());
    }

    #[test]
    fn oversized_payload_is_rejected_before_the_write() {
        let mut value = payload();
        set_ssh_key_data(&mut value, &"x".repeat(MAX_KEY_ENTRY_PAYLOAD_BYTES));
        assert_eq!(
            serialize(&value).unwrap_err(),
            GatewayError::KeyPayloadTooLarge
        );
    }

    #[test]
    fn plain_login_reads_as_password() {
        let mut value = payload();
        value["login_type"] = json!("");
        assert_eq!(read_login_type(&value), "PASSWORD");
        value.as_object_mut().unwrap().remove("login_type");
        assert_eq!(read_login_type(&value), "PASSWORD");
        assert_eq!(read_string(&value, "website"), "");
    }
}
