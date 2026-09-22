use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};

use rand::RngCore;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::error::{GatewayError, Result};
use crate::model::{
    ApprovalPolicy, Operation, Provider, validate_api_base, validate_name, validate_note,
    validate_repository,
};

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
pub const DEFAULT_PORT: u16 = 47831;
pub const MAX_GRANT_CALLS: u32 = 100_000;
/// A grant that asks for "no expiry" still gets this bounded lifetime, so an AI
/// authorization always has to be re-requested by a human eventually.
pub const DEFAULT_GRANT_TTL_MINUTES: u32 = 240;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub provider: Provider,
    pub credential_id: String,
    pub api_base: String,
    /// Human-designated public context; safe for the authorized AI to see.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub name: String,
    pub capability_hash: String,
    pub connection: String,
    pub connection_fingerprint: String,
    pub repositories: BTreeSet<String>,
    pub operations: BTreeSet<Operation>,
    pub issued_at: i64,
    pub expires_at: i64,
    pub requests_per_minute: u32,
    /// Upstream calls allowed before a human must re-authorize this grant. 0 means uncapped.
    #[serde(default)]
    pub max_calls: u32,
    /// Whether a human approves each call. Absent in files written before this
    /// existed, which is the same as the gate being off.
    #[serde(default)]
    pub approval: ApprovalPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_file: Option<PathBuf>,
}

/// When a grant stops serving the AI. Expiry alone is not enough: an exhausted
/// call budget blocks the grant just as hard, and a frontend that ignores it
/// would advertise a refused authorization as usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantState {
    pub used: u32,
    pub pending: bool,
    pub expired: bool,
    pub exhausted: bool,
}

impl GrantState {
    pub fn refresh_required(&self) -> bool {
        self.expired || self.exhausted
    }
}

pub fn grant_state(grant: &Grant, usage: &BTreeMap<String, u32>, now: i64) -> GrantState {
    let used = usage.get(&grant.capability_hash).copied().unwrap_or(0);
    GrantState {
        used,
        pending: now < grant.issued_at,
        expired: grant.expires_at != 0 && now >= grant.expires_at,
        exhausted: grant.max_calls != 0 && used >= grant.max_calls,
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    pub version: u32,
    pub vault: PathBuf,
    pub collection_id: Option<String>,
    pub listen: SocketAddrV4,
    pub connections: BTreeMap<String, Connection>,
    pub grants: Vec<Grant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav: Option<crate::sync::RemoteBinding>,
    /// Identity this device appends its segment stream under. Generated once and
    /// never reused, because peers locate this device's history by that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_device_id: Option<String>,
}

impl Config {
    pub fn new(vault: PathBuf) -> Self {
        Self {
            database_name: None,
            version: 1,
            vault,
            collection_id: None,
            listen: SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_PORT),
            connections: BTreeMap::new(),
            grants: Vec::new(),
            webdav: None,
            webdav_device_id: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self
            .database_name
            .as_ref()
            .is_some_and(|name| name.len() > 1024 || name.chars().any(char::is_control))
        {
            return Err(GatewayError::InvalidConfig);
        }
        if let Some(remote) = &self.webdav {
            remote.validate()?;
        }
        // The value names a remote directory, so it has to survive path
        // normalisation, and the engine rejects device IDs outside this shape.
        if self.webdav_device_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 256
                || !id
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || "-.".contains(ch))
        }) {
            return Err(GatewayError::InvalidConfig);
        }
        if self.version != 1
            || !self.vault.is_absolute()
            || *self.listen.ip() != Ipv4Addr::LOCALHOST
            || self.listen.port() < 1024
            || self.connections.len() > 64
            || self.grants.len() > 128
            || self
                .collection_id
                .as_ref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err(GatewayError::InvalidConfig);
        }
        for (name, connection) in &self.connections {
            validate_name(name)?;
            uuid::Uuid::parse_str(&connection.credential_id)
                .map_err(|_| GatewayError::InvalidConfig)?;
            validate_api_base(&connection.api_base, connection.provider)?;
            validate_note(&connection.note)?;
        }
        let mut names = BTreeSet::new();
        let mut hashes = BTreeSet::new();
        for grant in &self.grants {
            validate_name(&grant.name)?;
            let connection = self
                .connections
                .get(&grant.connection)
                .ok_or(GatewayError::InvalidConfig)?;
            if !names.insert(&grant.name)
                || !hashes.insert(&grant.capability_hash)
                || grant.capability_hash.len() != 64
                || grant.connection_fingerprint.len() != 64
                || hex::decode(&grant.connection_fingerprint).is_err()
                || hex::decode(&grant.capability_hash).is_err()
                || grant.repositories.is_empty()
                || grant.repositories.len() > 128
                || grant.operations.is_empty()
                || !(1..=600).contains(&grant.requests_per_minute)
                || grant.max_calls > MAX_GRANT_CALLS
                || grant
                    .client_file
                    .as_ref()
                    .is_some_and(|path| !path.is_absolute())
                || (grant.expires_at != 0
                    && (grant.expires_at <= grant.issued_at
                        || grant.expires_at.saturating_sub(grant.issued_at) > 24 * 60 * 60))
            {
                return Err(GatewayError::InvalidConfig);
            }
            if grant.operations.iter().any(|op| op.is_api()) {
                if grant.repositories.len() != 1
                    || !grant.repositories.contains("*")
                    || grant.operations.iter().any(|op| !op.is_api())
                {
                    return Err(GatewayError::InvalidConfig);
                }
                continue;
            }
            for repository in &grant.repositories {
                validate_repository(repository, connection.provider)
                    .map_err(|_| GatewayError::InvalidConfig)?;
            }
        }
        Ok(())
    }

    pub fn authenticate(&self, capability: &str, now: i64) -> Result<&Grant> {
        if capability.len() != 64 || !capability.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GatewayError::Unauthorized);
        }
        let hash = capability_hash(capability);
        let grant = self
            .grants
            .iter()
            .find(|grant| bool::from(grant.capability_hash.as_bytes().ct_eq(hash.as_bytes())))
            .ok_or(GatewayError::Unauthorized)?;
        if now < grant.issued_at {
            return Err(GatewayError::Unauthorized);
        }
        if grant.expires_at != 0 && now >= grant.expires_at {
            return Err(GatewayError::ReauthorizationRequired);
        }
        Ok(grant)
    }
}

/// Contains only a limited gateway capability, never an upstream token or vault password.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConfig {
    pub version: u32,
    pub endpoint: String,
    pub capability: String,
}

impl ClientConfig {
    pub fn validate(&self) -> Result<()> {
        let url = url::Url::parse(&self.endpoint).map_err(|_| GatewayError::InvalidConfig)?;
        if self.version != 1
            || url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.port().is_none_or(|port| port < 1024)
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || self.capability.len() != 64
            || !self.capability.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(GatewayError::InvalidConfig);
        }
        Ok(())
    }
}

impl Drop for ClientConfig {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.capability.zeroize();
    }
}

#[derive(Clone)]
pub struct ConfigStore {
    pub path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Config> {
        let config: Config = read_json(&self.path, MAX_CONFIG_BYTES)?;
        config.validate()?;
        Ok(config)
    }

    /// Serializes human mutations so grant creation cannot overwrite a concurrent revocation.
    pub fn update<T>(
        &self,
        action: impl FnOnce(Option<Config>) -> Result<(Config, T)>,
    ) -> Result<T> {
        ensure_parent(&self.path)?;
        let lock_path = self.path.with_extension("config-lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|_| GatewayError::StateUnavailable)?;
        private_file(&lock_path)?;
        lock.lock().map_err(|_| GatewayError::StateUnavailable)?;
        let previous = if self.path.exists() {
            Some(self.load()?)
        } else {
            None
        };
        let (config, value) = action(previous)?;
        config.validate()?;
        if serde_json::to_vec_pretty(&config)
            .map_err(|_| GatewayError::InvalidConfig)?
            .len()
            > MAX_CONFIG_BYTES as usize
        {
            return Err(GatewayError::InvalidConfig);
        }
        write_json(&self.path, &config, true)?;
        Ok(value)
    }

    /// Held for the lifetime of the unlocked broker, including shutdown.
    pub fn acquire_broker_lock(&self) -> Result<File> {
        ensure_parent(&self.path)?;
        let path = self.path.with_extension("broker-lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|_| GatewayError::StateUnavailable)?;
        private_file(&path)?;
        lock.try_lock()
            .map_err(|_| GatewayError::BrokerAlreadyRunning)?;
        Ok(lock)
    }

    pub fn lock_marker(&self) -> PathBuf {
        self.path.with_extension("locked")
    }

    /// Advisory status without creating or modifying any state files.
    pub fn broker_running(&self) -> Result<bool> {
        let path = self.path.with_extension("broker-lock");
        let lock = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(GatewayError::StateUnavailable),
        };
        match lock.try_lock() {
            Ok(()) => Ok(false),
            Err(std::fs::TryLockError::WouldBlock) => Ok(true),
            Err(_) => Err(GatewayError::StateUnavailable),
        }
    }

    pub fn audit_path(&self) -> PathBuf {
        self.path.with_extension("audit.jsonl")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.path.with_extension("operations.json")
    }

    /// Spent-call counters that must survive the broker locking and restarting.
    pub fn usage_path(&self) -> PathBuf {
        self.path.with_extension("usage.json")
    }

    pub fn request_lock(&self) -> Result<()> {
        write_json(
            &self.lock_marker(),
            &serde_json::json!({"locked": true}),
            true,
        )
    }
}

pub fn capability_hash(capability: &str) -> String {
    hex::encode(Sha256::digest(capability.as_bytes()))
}

pub fn connection_fingerprint(connection: &Connection) -> String {
    let bytes = format!(
        "{}\n{}\n{}",
        connection.provider.prefix(),
        connection.credential_id,
        connection.api_base
    );
    hex::encode(Sha256::digest(bytes.as_bytes()))
}

pub fn new_capability() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    rand::rngs::OsRng.fill_bytes(bytes.as_mut());
    Zeroizing::new(hex::encode(*bytes))
}

pub fn read_json<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<T> {
    let file = File::open(path).map_err(|_| GatewayError::StateUnavailable)?;
    if file
        .metadata()
        .map_err(|_| GatewayError::StateUnavailable)?
        .len()
        > limit
    {
        return Err(GatewayError::InvalidConfig);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GatewayError::StateUnavailable)?;
    if bytes.len() as u64 > limit {
        return Err(GatewayError::InvalidConfig);
    }
    serde_json::from_slice(&bytes).map_err(|_| GatewayError::InvalidConfig)
}

pub fn write_json(path: &Path, value: &impl Serialize, replace: bool) -> Result<()> {
    ensure_parent(path)?;
    let parent = path.parent().ok_or(GatewayError::StateUnavailable)?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| GatewayError::StateUnavailable)?;
    private_file(temp.path())?;
    let bytes = Zeroizing::new(
        serde_json::to_vec_pretty(value).map_err(|_| GatewayError::StateUnavailable)?,
    );
    temp.write_all(&bytes)
        .map_err(|_| GatewayError::StateUnavailable)?;
    temp.as_file()
        .sync_all()
        .map_err(|_| GatewayError::StateUnavailable)?;
    if replace {
        temp.persist(path)
            .map_err(|_| GatewayError::StateUnavailable)?;
    } else {
        temp.persist_noclobber(path)
            .map_err(|_| GatewayError::StateUnavailable)?;
    }
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| GatewayError::StateUnavailable)?;
    Ok(())
}

pub fn ensure_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or(GatewayError::StateUnavailable)?;
    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|_| GatewayError::StateUnavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| GatewayError::StateUnavailable)?;
        }
        #[cfg(windows)]
        private_file(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if path.is_dir() { 0o700 } else { 0o600 }),
    )
    .map_err(|_| GatewayError::StateUnavailable)
}

#[cfg(windows)]
fn owner_sid(path: &Path) -> Result<String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `name` is NUL terminated and outlives both calls; Windows allocates the
    // descriptor and the SID text, which are freed exactly once on every path below.
    unsafe {
        let status = GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 || owner.is_null() {
            LocalFree(descriptor);
            return Err(GatewayError::StateUnavailable);
        }
        let mut text: *mut u16 = ptr::null_mut();
        if ConvertSidToStringSidW(owner, &mut text) == 0 {
            LocalFree(descriptor);
            return Err(GatewayError::StateUnavailable);
        }
        let end = (0..2048).find(|i| *text.add(*i) == 0).unwrap_or(0);
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, end));
        LocalFree(text.cast());
        LocalFree(descriptor);
        if sid.starts_with("S-1-") {
            Ok(sid)
        } else {
            Err(GatewayError::StateUnavailable)
        }
    }
}

/// Grants access to the owning account and SYSTEM only. The SDDL `OW` trustee is the
/// `OWNER RIGHTS` special identity rather than the owning account, and Windows OpenSSH
/// refuses to load a private key whose DACL names it, so the resolved owner SID is used.
#[cfg(windows)]
pub fn private_file(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };
    let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let owner = owner_sid(path)?;
    // Protected DACL: the owner and SYSTEM can access the file; inherited access is removed.
    let descriptor = if path.is_dir() {
        format!("D:P(A;OICI;FA;;;{owner})(A;OICI;FA;;;SY)")
    } else {
        format!("D:P(A;;FA;;;{owner})(A;;FA;;;SY)")
    };
    let descriptor: Vec<u16> = descriptor.encode_utf16().chain(Some(0)).collect();
    let mut security = ptr::null_mut();
    // SAFETY: both strings are NUL terminated; Windows allocates the descriptor, which is
    // freed exactly once after SetFileSecurityW completes. No pointer escapes this block.
    let ok = unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut security,
            ptr::null_mut(),
        ) == 0
        {
            return Err(GatewayError::StateUnavailable);
        }
        let ok = SetFileSecurityW(
            name.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            security,
        );
        LocalFree(security);
        ok
    };
    if ok == 0 {
        Err(GatewayError::StateUnavailable)
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
pub fn private_file(_path: &Path) -> Result<()> {
    Err(GatewayError::StateUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(directory: &Path) -> (Config, String) {
        let capability = new_capability().to_string();
        let connection = Connection {
            provider: Provider::Github,
            credential_id: uuid::Uuid::new_v4().to_string(),
            api_base: Provider::Github.default_api_base().to_owned(),
            note: String::new(),
        };
        let mut config = Config::new(directory.join("vault.mdbx"));
        config.grants.push(Grant {
            name: "agent".to_owned(),
            capability_hash: capability_hash(&capability),
            connection: "work".to_owned(),
            connection_fingerprint: connection_fingerprint(&connection),
            repositories: ["org/repo".to_owned()].into(),
            operations: [Operation::ListIssues, Operation::GetIssue].into(),
            issued_at: 1000,
            expires_at: 2000,
            requests_per_minute: 60,
            max_calls: 0,
            approval: ApprovalPolicy::Off,
            client_file: None,
        });
        config.connections.insert("work".to_owned(), connection);
        (config, capability)
    }

    #[test]
    fn grant_state_treats_a_spent_budget_exactly_like_expiry() {
        let directory = tempfile::tempdir().unwrap();
        let (config, _) = fixture(directory.path());
        let grant = &config.grants[0];
        let used: BTreeMap<String, u32> =
            [(grant.capability_hash.clone(), 4)].into_iter().collect();

        assert!(grant_state(grant, &BTreeMap::new(), 999).pending);
        let live = grant_state(grant, &BTreeMap::new(), 1500);
        assert!(!live.pending && !live.refresh_required());
        let uncapped = grant_state(grant, &used, 1500);
        assert_eq!(uncapped.used, 4);
        assert!(
            !uncapped.refresh_required(),
            "a grant without a call budget cannot run out"
        );

        let mut capped = grant.clone();
        capped.max_calls = 4;
        let spent = grant_state(&capped, &used, 1500);
        assert!(spent.exhausted && !spent.expired);
        assert!(spent.refresh_required());
        assert!(grant_state(&capped, &BTreeMap::new(), 2000).expired);
    }

    #[test]
    fn perpetual_grants_survive_time_and_serialization_but_remain_revocable() {
        let directory = tempfile::tempdir().unwrap();
        let (mut config, capability) = fixture(directory.path());
        config.grants[0].expires_at = 0;
        config.validate().unwrap();
        let serialized = serde_json::to_vec(&config).unwrap();
        let mut restored: Config = serde_json::from_slice(&serialized).unwrap();
        restored.validate().unwrap();
        assert!(restored.authenticate(&capability, 999).is_err());
        assert!(restored.authenticate(&capability, 1000).is_ok());
        assert!(
            restored
                .authenticate(&capability, 1000 + 20 * 365 * 86400)
                .is_ok()
        );
        restored.grants.clear();
        assert!(restored.authenticate(&capability, 1001).is_err());
    }

    #[test]
    fn a_gateway_file_from_before_the_call_budget_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        let (config, capability) = fixture(directory.path());
        let mut legacy = serde_json::to_value(&config).unwrap();
        let grants = legacy["grants"].as_array_mut().unwrap();
        for grant in grants.iter_mut() {
            grant.as_object_mut().unwrap().remove("max_calls");
        }
        let restored: Config = serde_json::from_value(legacy).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.grants[0].max_calls, 0);
        assert!(restored.authenticate(&capability, 1500).is_ok());
    }

    #[test]
    fn config_capabilities_enforce_lifetime_and_revocation_without_storing_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let (mut config, capability) = fixture(directory.path());
        config.validate().unwrap();
        assert!(config.authenticate(&capability, 1000).is_ok());
        assert!(config.authenticate(&capability, 999).is_err());
        assert!(config.authenticate(&capability, 2000).is_err());
        assert!(config.authenticate(&new_capability(), 1500).is_err());
        assert!(
            !serde_json::to_string(&config)
                .unwrap()
                .contains(&capability)
        );
        let original = config.grants[0].connection_fingerprint.clone();
        config.connections.get_mut("work").unwrap().api_base =
            "https://enterprise.example/api/v3/".to_owned();
        assert_ne!(
            connection_fingerprint(&config.connections["work"]),
            original
        );
        config.grants.clear();
        assert!(config.authenticate(&capability, 1500).is_err());
    }

    #[test]
    fn config_rejects_invalid_grants_and_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let (config, _) = fixture(directory.path());
        for variant in 0..6 {
            let mut bad = config.clone();
            match variant {
                0 => bad.grants[0].expires_at = 1000 + 86401,
                1 => bad.grants[0].requests_per_minute = 0,
                2 => {
                    bad.grants[0].repositories = ["org/../secret".to_owned()].into();
                }
                3 => {
                    bad.grants.push(bad.grants[0].clone());
                }
                4 => bad.listen.set_ip(Ipv4Addr::UNSPECIFIED),
                _ => bad.grants[0].operations.clear(),
            }
            assert!(bad.validate().is_err(), "variant {variant}");
        }
        let mut json = serde_json::to_value(&config).unwrap();
        json["token"] = "unsupported".into();
        assert!(serde_json::from_value::<Config>(json).is_err());
    }

    #[test]
    fn config_client_can_only_contact_an_explicit_ipv4_loopback_port() {
        let capability = new_capability();
        for endpoint in [
            "http://127.0.0.1:47831/",
            "http://localhost:47831/",
            "https://127.0.0.1:47831/",
            "http://example.com:47831/",
            "http://127.0.0.1/",
            "http://127.0.0.1:47831/path",
            "http://user@127.0.0.1:47831/",
            "http://127.0.0.1:47831/?redirect=1",
        ] {
            let client = ClientConfig {
                version: 1,
                endpoint: endpoint.to_owned(),
                capability: capability.to_string(),
            };
            assert_eq!(
                client.validate().is_ok(),
                endpoint == "http://127.0.0.1:47831/",
                "{endpoint}"
            );
        }
    }

    #[test]
    fn config_updates_are_atomic_serialized_and_broker_is_singleton() {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let (initial, _) = fixture(directory.path());
        store.update(|_| Ok((initial.clone(), ()))).unwrap();
        let original_bytes = fs::read(&store.path).unwrap();
        let result: Result<()> = store.update(|_| Err(GatewayError::InvalidRequest));
        assert!(result.is_err());
        assert_eq!(fs::read(&store.path).unwrap(), original_bytes);
        assert!(write_json(&store.path, &initial, false).is_err());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|number| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .update(|config| {
                            let mut config = config.unwrap();
                            config.connections.insert(
                                format!("work-{number}"),
                                config.connections["work"].clone(),
                            );
                            Ok((config, ()))
                        })
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(store.load().unwrap().connections.len(), 9);
        let lock = store.acquire_broker_lock().unwrap();
        assert!(matches!(
            store.acquire_broker_lock(),
            Err(GatewayError::BrokerAlreadyRunning)
        ));
        drop(lock);
        assert!(store.acquire_broker_lock().is_ok());
    }
}
