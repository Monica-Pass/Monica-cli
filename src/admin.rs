//! Trusted local management shared by the CLI and TUI. None of these operations
//! is reachable from the broker or MCP transport.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdbx_core::tiga::TigaMode;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::config::{
    ClientConfig, Config, ConfigStore, Connection, DEFAULT_GRANT_TTL_MINUTES, Grant,
    MAX_GRANT_CALLS, capability_hash, connection_fingerprint, ensure_parent, new_capability,
    write_json,
};
use crate::error::{GatewayError, Result};
use crate::gateway::Gateway;
use crate::model::{
    Operation, Provider, validate_api_base, validate_name, validate_note, validate_repository,
    validate_title,
};
use crate::protocol::serve_broker;
use crate::upstream::reject_secret_value;
use crate::vault::Vault;

#[derive(clap::Args)]
pub struct GrantOptions {
    pub name: String,
    #[arg(short = 'c', long)]
    pub connection: String,
    /// Exact repository paths, or * for an explicit service-wide API grant.
    #[arg(short = 'r', long = "repo", required = true)]
    pub repositories: Vec<String>,
    /// Read-only Issue tools by default. Explicit api-read/api-write grants enable service API access.
    #[arg(long = "operation", visible_alias = "op", value_enum, default_values = ["list-issues", "get-issue"])]
    pub operations: Vec<Operation>,
    #[arg(short = 't', long, visible_alias = "ttl", default_value_t = DEFAULT_GRANT_TTL_MINUTES, value_parser = clap::value_parser!(u32).range(0..=1440))]
    pub ttl_minutes: u32,
    #[arg(long, visible_alias = "rpm", default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=600))]
    pub requests_per_minute: u32,
    /// Upstream calls this grant may make before a person refreshes it; 0 is uncapped.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=MAX_GRANT_CALLS as i64))]
    pub max_calls: u32,
    /// A new capability file. Existing files are never overwritten.
    #[arg(short = 'o', long, value_name = "NEW_CLIENT_FILE")]
    pub out: Option<PathBuf>,
}

#[derive(Clone, clap::Args)]
pub struct RefreshOptions {
    /// Existing grant to re-authorize with a fresh capability.
    pub name: String,
    /// New window in minutes. Omit to reuse the window this grant was issued with.
    #[arg(short = 't', long = "ttl-minutes", visible_alias = "ttl", value_parser = clap::value_parser!(u32).range(1..=1440))]
    pub window: Option<u32>,
    /// New call cap. Omit to keep the cap this grant was issued with.
    #[arg(long = "max-calls", value_parser = clap::value_parser!(u32).range(0..=MAX_GRANT_CALLS as i64))]
    pub call_cap: Option<u32>,
}

#[derive(Clone, clap::Args)]
pub struct AddOptions {
    /// A short name the AI will use, such as work-github.
    pub name: String,
    /// Optional human-facing display title (Chinese allowed). The name stays the ASCII handle.
    #[arg(long, default_value = "")]
    pub title: String,
    #[arg(short = 'p', long, value_enum, default_value = "github")]
    pub provider: Provider,
    /// Optional HTTPS API root for a self-hosted service.
    #[arg(short = 'b', long)]
    pub api_base: Option<String>,
    /// Public purpose/context visible to AI. Never put a secret here.
    #[arg(short = 'n', long, default_value = "")]
    pub note: String,
    /// Exact repository scope; repeat for multiple repositories.
    #[arg(short = 'r', long = "repo", required = true)]
    pub repositories: Vec<String>,
    /// Also authorize creating Issues. Omit for read-only access.
    #[arg(short = 'w', long)]
    pub allow_write: bool,
    #[arg(short = 't', long, visible_alias = "ttl", default_value_t = DEFAULT_GRANT_TTL_MINUTES, value_parser = clap::value_parser!(u32).range(0..=1440))]
    pub ttl_minutes: u32,
}

impl AddOptions {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        if !self.title.trim().is_empty() {
            validate_title(&self.title)?;
        }
        validate_note(&self.note)?;
        validate_api_base(
            self.api_base
                .as_deref()
                .unwrap_or(self.provider.default_api_base()),
            self.provider,
        )?;
        if self.repositories.is_empty()
            || self.repositories.len() > 128
            || !(0..=1440).contains(&self.ttl_minutes)
        {
            return Err(GatewayError::InvalidRequest);
        }
        for repository in &self.repositories {
            validate_repository(repository, self.provider)?;
        }
        Ok(())
    }

    fn grant_options(&self) -> GrantOptions {
        let mut operations = vec![Operation::ListIssues, Operation::GetIssue];
        if self.allow_write {
            operations.push(Operation::CreateIssue);
        }
        GrantOptions {
            name: self.name.clone(),
            connection: self.name.clone(),
            repositories: self.repositories.clone(),
            operations,
            ttl_minutes: self.ttl_minutes,
            requests_per_minute: 60,
            max_calls: 0,
            out: None,
        }
    }
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|_| GatewayError::InvalidConfig)
}

pub fn default_config() -> Result<PathBuf> {
    let executable = std::env::current_exe().map_err(|_| GatewayError::InvalidConfig)?;
    let directory = executable.parent().ok_or(GatewayError::InvalidConfig)?;
    match std::fs::symlink_metadata(directory.join("monica-pass.portable")) {
        Ok(marker) if marker.file_type().is_file() && marker.len() == 0 => {
            return Ok(directory.join("data").join("gateway.json"));
        }
        Ok(_) => return Err(GatewayError::InvalidConfig),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(GatewayError::StateUnavailable),
    }
    #[cfg(windows)]
    let directory = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|directory| directory.join("MonicaPass"));
    #[cfg(not(windows))]
    let directory = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|directory| directory.join("monica-pass"));
    directory
        .map(|directory| directory.join("gateway.json"))
        .ok_or(GatewayError::InvalidConfig)
}

/// Match the engine's password rules without imposing a client-only minimum.
/// Whitespace around a nonempty password remains part of the credential.
pub fn validate_new_password(password: &str, confirmation: &str) -> Result<()> {
    if password.trim().is_empty() || password != confirmation {
        return Err(GatewayError::PasswordRequirements);
    }
    Ok(())
}

pub fn initialize(
    store: &ConfigStore,
    path: &Path,
    port: u16,
    password: &str,
    confirmation: &str,
) -> Result<()> {
    validate_new_password(password, confirmation)?;
    let path = absolute(path)?;
    if path == store.path || path.exists() {
        return Err(GatewayError::AlreadyExists);
    }
    let mut config = Config::new(path.clone());
    config.listen.set_port(port);
    config.validate()?;
    let _guard = store.acquire_broker_lock()?;
    store.update(|previous| {
        if let Some(previous) = &previous {
            crate::sync::remember_previous(store, previous)?;
        }
        ensure_parent(&path)?;
        let vault = Vault::create(&path, password, TigaMode::Multi)?;
        vault.lock()?;
        Ok((config, ()))
    })
}

pub struct NewConnection<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub provider: Provider,
    pub base: &'a str,
    pub note: &'a str,
    pub category: Option<&'a str>,
}

pub fn add_connection(
    store: &ConfigStore,
    name: &str,
    provider: Provider,
    base: &str,
    note: &str,
    password: &str,
    token: Zeroizing<String>,
) -> Result<()> {
    add_connection_in_category(
        store,
        NewConnection {
            name,
            title: "",
            provider,
            base,
            note,
            category: None,
        },
        password,
        token,
    )
}

pub fn add_connection_in_category(
    store: &ConfigStore,
    options: NewConnection<'_>,
    password: &str,
    token: Zeroizing<String>,
) -> Result<()> {
    let NewConnection {
        name,
        title,
        provider,
        base,
        note,
        category,
    } = options;
    validate_name(name)?;
    if !title.trim().is_empty() {
        validate_title(title)?;
    }
    validate_note(note)?;
    let base = validate_api_base(base, provider)?.to_string();
    check_public(&json!([name, title, &base, note]), &[password, &token])?;
    let _guard = store.acquire_broker_lock()?;
    store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        if config.connections.contains_key(name) {
            return Err(GatewayError::AlreadyExists);
        }
        let vault = Vault::open(&config.vault, password)?;
        if category.is_some_and(|id| uuid::Uuid::parse_str(id).is_err()) {
            vault.lock()?;
            return Err(GatewayError::InvalidRequest);
        }
        if let Some(id) = category
            && !vault.library()?.categories.iter().any(|c| c.id == id)
        {
            vault.lock()?;
            return Err(GatewayError::NotFound);
        }
        let (collection, connection) = vault.store_credential(
            category.or(config.collection_id.as_deref()),
            name,
            title,
            provider,
            &base,
            note,
            token,
        )?;
        vault.lock()?;
        if category.is_none() {
            config.collection_id = Some(collection);
        }
        config.connections.insert(name.to_owned(), connection);
        Ok((config, ()))
    })
}

fn check_public(value: &Value, secrets: &[&str]) -> Result<()> {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        reject_secret_value(value, secret).map_err(|_| GatewayError::SensitiveMetadata)?;
    }
    Ok(())
}

/// Shared one-action human setup. All public input is validated before vault creation.
pub fn quick_add(
    store: &ConfigStore,
    options: &AddOptions,
    password: &str,
    confirmation: Option<&str>,
    token: Zeroizing<String>,
) -> Result<PathBuf> {
    options.validate()?;
    crate::vault::validate_token(&token)?;
    let base = validate_api_base(
        options
            .api_base
            .as_deref()
            .unwrap_or(options.provider.default_api_base()),
        options.provider,
    )?
    .to_string();
    check_public(
        &json!([
            &options.name,
            &options.title,
            &options.note,
            &base,
            &options.repositories
        ]),
        &[password, &token],
    )?;
    let output = client_path(store, &options.name)?;
    let settings_path = output.with_extension("mcp.json");
    if output.exists() || settings_path.exists() {
        return Err(GatewayError::AlreadyExists);
    }
    let settings = mcp_settings(&options.name, &output)?;
    let _guard = store.acquire_broker_lock()?;
    let capability = new_capability();
    let mut client_written = false;
    let mut settings_written = false;
    let result = store.update(|previous| {
        let creating = previous.is_none();
        if creating {
            validate_new_password(
                password,
                confirmation.ok_or(GatewayError::PasswordRequirements)?,
            )?;
        }
        let mut config =
            previous.unwrap_or_else(|| Config::new(store.path.with_file_name("gateway.mdbx")));
        if config.connections.contains_key(&options.name)
            || config.grants.iter().any(|grant| grant.name == options.name)
            || config.connections.len() >= 64
            || config.grants.len() >= 128
            || (creating && config.vault.exists())
        {
            return Err(GatewayError::AlreadyExists);
        }
        let vault = if creating {
            Vault::create(&config.vault, password, TigaMode::Multi)?
        } else {
            Vault::open(&config.vault, password)?
        };
        let stored = vault.store_credential(
            config.collection_id.as_deref(),
            &options.name,
            &options.title,
            options.provider,
            &base,
            &options.note,
            token,
        );
        vault.lock()?;
        let (collection, connection) = stored?;
        config.collection_id = Some(collection);
        config.connections.insert(options.name.clone(), connection);
        let client = prepare_grant(&mut config, &options.grant_options(), &capability, &output)?;
        write_json(&output, &client, false)?;
        client_written = true;
        write_json(&settings_path, &settings, false)?;
        settings_written = true;
        Ok((config, ()))
    });
    if result.is_err() {
        // These files were created by this call and have no committed grant.
        if client_written {
            let _ = std::fs::remove_file(&output);
        }
        if settings_written {
            let _ = std::fs::remove_file(&settings_path);
        }
    }
    result?;
    Ok(output)
}

pub fn update_note(store: &ConfigStore, name: &str, note: &str, password: &str) -> Result<()> {
    validate_name(name)?;
    validate_note(note)?;
    check_public(&json!([name, note]), &[password])?;
    let _guard = store.acquire_broker_lock()?;
    store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        let binding = config.connections.get(name).ok_or(GatewayError::NotFound)?;
        let vault = Vault::open(&config.vault, password)?;
        let updated = vault.update_note(name, binding, note);
        vault.lock()?;
        config.connections.insert(name.to_owned(), updated?);
        Ok((config, ()))
    })
}

/// Retitle an existing entry's display title (Chinese allowed). The connection handle and the
/// encrypted payload stay unchanged; the title lives only in the shared MDBX entry.
pub fn rename_entry(store: &ConfigStore, name: &str, title: &str, password: &str) -> Result<()> {
    validate_name(name)?;
    validate_title(title)?;
    check_public(&json!([name, title]), &[password])?;
    let _guard = store.acquire_broker_lock()?;
    let config = store.load()?;
    let binding = config.connections.get(name).ok_or(GatewayError::NotFound)?;
    let vault = Vault::open(&config.vault, password)?;
    let result = vault.rename_entry(binding, title);
    vault.lock()?;
    result
}

/// Replace an encrypted token in place and require new authorization for it.
pub fn update_token(
    store: &ConfigStore,
    name: &str,
    password: &str,
    token: Zeroizing<String>,
) -> Result<()> {
    validate_name(name)?;
    crate::vault::validate_token(&token)?;
    let _guard = store.acquire_broker_lock()?;
    let (vault, binding) = store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        let binding = config.connections.get(name).ok_or(GatewayError::NotFound)?;
        check_public(
            &json!([name, &binding.note, &binding.api_base]),
            &[password, &token],
        )?;
        let vault = Vault::open(&config.vault, password)?;
        // Persist revocation before replacing the encrypted payload. A failed
        // config write must never leave old grants bound to a new Token.
        let binding = binding.clone();
        config.grants.retain(|grant| grant.connection != name);
        Ok((config, (vault, binding)))
    })?;
    let result = vault.edit_credential(name, &binding, &binding.note, Some(token));
    vault.lock()?;
    result.map(|_| ())
}

/// "No expiry" is not offered to an AI anymore: an unset window falls back to a
/// bounded one so every grant eventually has to be re-requested by a person.
fn window_minutes(ttl_minutes: u32) -> u32 {
    if ttl_minutes == 0 {
        DEFAULT_GRANT_TTL_MINUTES
    } else {
        ttl_minutes
    }
}

/// Proves a person holding the master password authorized this scope, and that
/// no AI-visible field carries that password or the upstream token.
fn confirm_scope<R: serde::Serialize>(
    config: &Config,
    binding: &Connection,
    name: &str,
    connection: &str,
    repositories: &R,
    password: &str,
) -> Result<()> {
    let vault = Vault::open(&config.vault, password)?;
    // A human must prove that this binding is an available credential.
    let credential = vault.credential(binding, chrono::Utc::now().timestamp())?;
    check_public(
        &json!([name, connection, repositories]),
        &[password, &credential.token],
    )?;
    drop(credential);
    vault.lock()
}

fn prepare_grant(
    config: &mut Config,
    options: &GrantOptions,
    capability: &str,
    output: &Path,
) -> Result<ClientConfig> {
    if config.grants.iter().any(|grant| grant.name == options.name) {
        return Err(GatewayError::AlreadyExists);
    }
    let binding = config
        .connections
        .get(&options.connection)
        .ok_or(GatewayError::NotFound)?;
    let now = chrono::Utc::now().timestamp();
    config.grants.push(Grant {
        name: options.name.clone(),
        capability_hash: capability_hash(capability),
        connection: options.connection.clone(),
        connection_fingerprint: connection_fingerprint(binding),
        repositories: options
            .repositories
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        operations: options.operations.iter().copied().collect(),
        issued_at: now,
        expires_at: now + i64::from(window_minutes(options.ttl_minutes)) * 60,
        requests_per_minute: options.requests_per_minute,
        max_calls: options.max_calls,
        client_file: Some(output.to_owned()),
    });
    config.validate()?;
    let client = ClientConfig {
        version: 1,
        endpoint: format!("http://{}/", config.listen),
        capability: capability.to_owned(),
    };
    client.validate()?;
    Ok(client)
}

pub fn issue_grant(store: &ConfigStore, options: &GrantOptions, password: &str) -> Result<PathBuf> {
    validate_name(&options.name)?;
    if !(0..=1440).contains(&options.ttl_minutes)
        || !(1..=600).contains(&options.requests_per_minute)
        || options.max_calls > MAX_GRANT_CALLS
        || options.operations.is_empty()
        || options.repositories.is_empty()
    {
        return Err(GatewayError::InvalidRequest);
    }
    let output = match &options.out {
        Some(path) => absolute(path)?,
        None => client_path(store, &options.name)?,
    };
    if output.exists() {
        return Err(GatewayError::AlreadyExists);
    }
    let _guard = store.acquire_broker_lock()?;
    let capability = new_capability();
    let mut client_written = false;
    let result = store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        if config.grants.iter().any(|grant| grant.name == options.name) {
            return Err(GatewayError::AlreadyExists);
        }
        let binding = config
            .connections
            .get(&options.connection)
            .ok_or(GatewayError::NotFound)?;
        crate::model::validate_grant_scope(
            &options.repositories,
            &options.operations,
            binding.provider,
        )?;
        confirm_scope(
            &config,
            binding,
            &options.name,
            &options.connection,
            &options.repositories,
            password,
        )?;
        let client = prepare_grant(&mut config, options, &capability, &output)?;
        write_json(&output, &client, false)?;
        client_written = true;
        Ok((config, ()))
    });
    if result.is_err() && client_written {
        let _ = std::fs::remove_file(&output);
    }
    result?;
    Ok(output)
}

/// Re-authorize an existing grant with a brand-new capability. The previous
/// bearer stops authenticating immediately, so a closed AI session has to be
/// re-requested instead of staying usable; the name and client file stay put.
pub fn refresh_grant(
    store: &ConfigStore,
    options: &RefreshOptions,
    password: &str,
) -> Result<PathBuf> {
    validate_name(&options.name)?;
    if options.window.is_some_and(|ttl| !(1..=1440).contains(&ttl))
        || options
            .call_cap
            .is_some_and(|calls| calls > MAX_GRANT_CALLS)
    {
        return Err(GatewayError::InvalidRequest);
    }
    let _guard = store.acquire_broker_lock()?;
    let capability = new_capability();
    store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        let position = config
            .grants
            .iter()
            .position(|grant| grant.name == options.name)
            .ok_or(GatewayError::NotFound)?;
        let previous = config.grants[position].clone();
        let binding = config
            .connections
            .get(&previous.connection)
            .ok_or(GatewayError::NotFound)?
            .clone();
        if previous.connection_fingerprint != connection_fingerprint(&binding) {
            return Err(GatewayError::CredentialUnavailable);
        }
        confirm_scope(
            &config,
            &binding,
            &previous.name,
            &previous.connection,
            &previous.repositories,
            password,
        )?;
        // Absent choices reuse the previous session, so refreshing a grant that
        // was created with a cap and a window does not silently widen either.
        let minutes = options.window.unwrap_or_else(|| {
            if previous.expires_at == 0 {
                DEFAULT_GRANT_TTL_MINUTES
            } else {
                ((previous.expires_at - previous.issued_at) / 60).clamp(1, 1440) as u32
            }
        });
        let output = match previous.client_file.clone() {
            Some(path) => path,
            None => client_path(store, &previous.name)?,
        };
        let now = chrono::Utc::now().timestamp();
        config.grants[position] = Grant {
            capability_hash: capability_hash(&capability),
            issued_at: now,
            expires_at: now + i64::from(window_minutes(minutes)) * 60,
            max_calls: options.call_cap.unwrap_or(previous.max_calls),
            ..previous
        };
        config.validate()?;
        let client = ClientConfig {
            version: 1,
            endpoint: format!("http://{}/", config.listen),
            capability: capability.as_str().to_owned(),
        };
        client.validate()?;
        write_json(&output, &client, true)?;
        Ok((config, output))
    })
}

pub fn client_path(store: &ConfigStore, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(store
        .path
        .parent()
        .ok_or(GatewayError::InvalidConfig)?
        .join("clients")
        .join(format!("{name}.client.json")))
}

pub fn mcp_settings(name: &str, client: &Path) -> Result<Value> {
    validate_name(name)?;
    let executable = std::env::current_exe().map_err(|_| GatewayError::StateUnavailable)?;
    Ok(json!({"mcpServers": {(name): {
        "command": executable, "args": ["mcp", "--client", client]
    }}}))
}

/// Resolve only an exact grant name; never silently select a different grant.
pub fn grant_client(store: &ConfigStore, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let config = store.load()?;
    let grant = config
        .grants
        .iter()
        .find(|grant| grant.name == name)
        .ok_or(GatewayError::NotFound)?;
    let path = grant
        .client_file
        .clone()
        .map(Ok)
        .unwrap_or_else(|| client_path(store, name))?;
    let client: ClientConfig = crate::config::read_json(&path, 16 * 1024)?;
    client.validate()?;
    if capability_hash(&client.capability) != grant.capability_hash
        || client.endpoint != format!("http://{}/", config.listen)
    {
        return Err(GatewayError::InvalidConfig);
    }
    Ok(path)
}

pub fn settings_for_grant(store: &ConfigStore, name: &str) -> Result<Value> {
    let client = grant_client(store, name)?;
    let settings = mcp_settings(name, &client)?;
    let path = client.with_extension("mcp.json");
    write_json(&path, &settings, true)?;
    Ok(json!({"name": name, "client_file": client, "settings_file": path, "mcp": settings}))
}

pub fn show_connection(store: &ConfigStore, name: &str) -> Result<Value> {
    validate_name(name)?;
    let status = status(store)?;
    let mut connection = status["connections"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == name))
        .cloned()
        .ok_or(GatewayError::NotFound)?;
    connection["grants"] = Value::Array(
        status["grants"]
            .as_array()
            .ok_or(GatewayError::InvalidConfig)?
            .iter()
            .filter(|grant| grant["connection"] == name)
            .cloned()
            .collect(),
    );
    Ok(connection)
}

/// Lock and drain a broker started by either frontend. Management functions
/// still acquire the OS lock themselves, so a concurrent restart fails safely.
pub async fn lock_broker(store: &ConfigStore) -> Result<()> {
    if !store
        .path
        .try_exists()
        .map_err(|_| GatewayError::StateUnavailable)?
    {
        return Ok(());
    }
    store.request_lock()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(50);
    loop {
        match store.acquire_broker_lock() {
            Ok(_guard) => return Ok(()),
            Err(GatewayError::BrokerAlreadyRunning) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

pub fn revoke(store: &ConfigStore, name: &str) -> Result<()> {
    validate_name(name)?;
    store.update(|config| {
        let mut config = config.ok_or(GatewayError::NotFound)?;
        let count = config.grants.len();
        config.grants.retain(|grant| grant.name != name);
        if count == config.grants.len() {
            return Err(GatewayError::NotFound);
        }
        Ok((config, ()))
    })
}

pub fn status(store: &ConfigStore) -> Result<Value> {
    let config = store.load()?;
    let connections: Vec<_> = config
        .connections
        .iter()
        .map(|(name, connection)| {
            json!({
                "name": name, "provider": connection.provider, "api_base": connection.api_base, "note": connection.note,
            })
        })
        .collect();
    let usage = crate::gateway::load_call_usage(store)?;
    let now = chrono::Utc::now().timestamp();
    let grants: Vec<_> = config
        .grants
        .iter()
        .map(|grant| {
            let state = crate::config::grant_state(grant, &usage, now);
            json!({
                "name": grant.name, "connection": grant.connection, "repositories": grant.repositories,
                "operations": grant.operations, "expires_at_unix": grant.expires_at,
                "expired": state.expired, "max_calls": grant.max_calls, "calls_used": state.used,
                "refresh_required": state.refresh_required(),
            })
        })
        .collect();
    Ok(
        json!({"config":store.path, "vault":config.vault, "listen":config.listen, "webdav":config.webdav,
        "safe_remote_replace":config.webdav.as_ref().map(|binding| binding.etag.is_some()),
        "lock_requested":store.lock_marker().exists(), "broker_running":store.broker_running()?,
        "connections":connections, "grants":grants}),
    )
}

/// Owns the broker lock until in-flight operations have drained. Dropping this
/// handle requests a lock; normal TUI/CLI exits additionally await shutdown.
pub struct BrokerSession {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
    deadline: Instant,
}

impl BrokerSession {
    pub async fn start(store: ConfigStore, password: Zeroizing<String>) -> Result<Self> {
        Self::start_using(store, password, Gateway::new).await
    }

    #[cfg(test)]
    pub(crate) async fn start_for_test(
        store: ConfigStore,
        password: Zeroizing<String>,
        http: reqwest::Client,
    ) -> Result<Self> {
        Self::start_using(store, password, move |store, vault| {
            Gateway::with_client(store, vault, http)
        })
        .await
    }

    async fn start_using(
        store: ConfigStore,
        password: Zeroizing<String>,
        build: impl FnOnce(ConfigStore, Vault) -> Result<Gateway> + Send + 'static,
    ) -> Result<Self> {
        let guard = store.acquire_broker_lock()?;
        let config = store.load()?;
        match std::fs::remove_file(store.lock_marker()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(GatewayError::StateUnavailable),
        }
        let listener = tokio::net::TcpListener::bind(config.listen)
            .await
            .map_err(|_| GatewayError::ListenUnavailable)?;
        let gateway = tokio::task::spawn_blocking(move || {
            let vault = Vault::open(&config.vault, &password)?;
            build(store, vault).map(Arc::new)
        })
        .await
        .map_err(|_| GatewayError::StateUnavailable)??;
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let lifetime = Duration::from_secs(300);
        let task = tokio::spawn(async move {
            let _guard = guard;
            serve_broker(gateway, listener, async move {
                tokio::select! {
                    _ = receiver => {}
                    _ = tokio::time::sleep(lifetime) => {}
                }
            })
            .await
        });
        Ok(Self {
            stop: Some(stop),
            task: Some(task),
            deadline: Instant::now() + lifetime,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished())
    }

    pub fn remaining_seconds(&self) -> u64 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
    }

    pub async fn wait(&mut self) -> Result<()> {
        let result = if let Some(task) = self.task.as_mut() {
            task.await.map_err(|_| GatewayError::StateUnavailable)?
        } else {
            Ok(())
        };
        self.task = None;
        result
    }

    pub async fn stop(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.wait().await
    }
}

impl Drop for BrokerSession {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn quick_options(name: &str) -> AddOptions {
        AddOptions {
            name: name.to_owned(),
            title: String::new(),
            provider: Provider::Github,
            api_base: None,
            note: "处理 Monica 的问题反馈；备注不是权限指令。".to_owned(),
            repositories: vec!["example/project".to_owned()],
            allow_write: false,
            ttl_minutes: 60,
        }
    }

    #[test]
    fn category_token_creation_and_move_recover_after_portable_sync() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let path = directory.path().join("source.mdbx");
        initialize(&store, &path, 47831, PASSWORD, PASSWORD).unwrap();
        let root = crate::library::create_category(&store, PASSWORD, "Projects", None).unwrap();
        let child =
            crate::library::create_category(&store, PASSWORD, "GitLab", Some(&root)).unwrap();
        add_connection_in_category(
            &store,
            NewConnection {
                name: "work",
                title: "",
                provider: Provider::Gitlab,
                base: Provider::Gitlab.default_api_base(),
                note: "Work issues",
                category: Some(&child),
            },
            PASSWORD,
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        let config = store.load().unwrap();
        let id = config.connections["work"].credential_id.clone();
        let library = crate::library::read(&store, PASSWORD).unwrap();
        assert_eq!(
            library
                .entries
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .category,
            child
        );
        crate::library::move_item(&store, PASSWORD, &id, &root).unwrap();
        crate::library::rename_category(&store, PASSWORD, &root, "Renamed Projects").unwrap();
        let copy = directory.path().join("portable.mdbx");
        mdbx_storage::backup::BackupService::create_portable_copy_path(&path, &copy).unwrap();
        let vault = Vault::open(&copy, PASSWORD).unwrap();
        let recovered = vault.gateway_inventory().unwrap();
        assert_eq!(recovered.connections["work"].credential_id, id);
        assert_eq!(
            vault
                .library()
                .unwrap()
                .entries
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .category,
            root
        );
        assert_eq!(
            vault
                .credential(
                    &recovered.connections["work"],
                    chrono::Utc::now().timestamp()
                )
                .unwrap()
                .token
                .as_str(),
            TOKEN
        );
        vault.lock().unwrap();
    }

    #[test]
    fn rename_entry_retitles_ascii_handle_to_chinese_and_rejects_secret() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        quick_add(
            &store,
            &quick_options("work"),
            PASSWORD,
            Some(PASSWORD),
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        let id = store.load().unwrap().connections["work"]
            .credential_id
            .clone();
        // A blank display title falls back to the ASCII handle on creation.
        assert_eq!(
            crate::library::read(&store, PASSWORD)
                .unwrap()
                .entries
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .title,
            "work"
        );
        rename_entry(&store, "work", "微信令牌", PASSWORD).unwrap();
        assert_eq!(
            crate::library::read(&store, PASSWORD)
                .unwrap()
                .entries
                .iter()
                .find(|e| e.id == id)
                .unwrap()
                .title,
            "微信令牌"
        );
        // The handle keeps resolving and the credential is intact after a title-only rename.
        assert!(store.load().unwrap().connections.contains_key("work"));
        // A display title that would leak the master password is rejected.
        assert!(rename_entry(&store, "work", PASSWORD, PASSWORD).is_err());
    }

    #[test]
    fn token_replacement_preserves_entry_and_requires_new_authorization() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        quick_add(
            &store,
            &quick_options("work"),
            PASSWORD,
            Some(PASSWORD),
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        let before = store.load().unwrap();
        assert!(!before.grants.is_empty());
        assert!(
            update_token(
                &store,
                "work",
                "wrong",
                Zeroizing::new("replacement-synthetic-token".into())
            )
            .is_err()
        );
        assert!(store.load().unwrap().grants == before.grants);
        let replacement = "replacement-synthetic-token";
        update_token(&store, "work", PASSWORD, Zeroizing::new(replacement.into())).unwrap();
        let after = store.load().unwrap();
        assert_eq!(
            after.connections["work"].credential_id,
            before.connections["work"].credential_id
        );
        assert!(after.grants.is_empty());
        let vault = Vault::open(&after.vault, PASSWORD).unwrap();
        assert_eq!(
            vault
                .credential(&after.connections["work"], chrono::Utc::now().timestamp())
                .unwrap()
                .token
                .as_str(),
            replacement
        );
        vault.lock().unwrap();
        drop(vault);
        let bytes = std::fs::read(&after.vault).unwrap();
        assert!(
            !bytes
                .windows(replacement.len())
                .any(|w| w == replacement.as_bytes())
        );
    }

    #[test]
    fn quick_add_creates_vault_grant_and_settings_and_preserves_explicit_permissions() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let options = quick_options("work");
        let path = quick_add(
            &store,
            &options,
            PASSWORD,
            Some(PASSWORD),
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        let config = store.load().unwrap();
        assert_eq!(config.connections["work"].note, options.note);
        assert_eq!(
            config.grants[0].operations,
            [Operation::ListIssues, Operation::GetIssue].into()
        );
        assert_eq!(config.grants[0].connection, "work");
        assert_eq!(
            config.grants[0].repositories,
            ["example/project".to_owned()].into()
        );
        let client: ClientConfig = crate::config::read_json(&path, 16 * 1024).unwrap();
        assert!(
            config
                .authenticate(&client.capability, chrono::Utc::now().timestamp())
                .is_ok()
        );
        for file in [
            &store.path,
            &path,
            &path.with_extension("mcp.json"),
            &config.vault,
        ] {
            let bytes = std::fs::read(file).unwrap();
            for secret in [PASSWORD, TOKEN] {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes())
                );
            }
        }
        let mut writer = quick_options("writer");
        writer.allow_write = true;
        let before = std::fs::read(&store.path).unwrap();
        assert_eq!(
            quick_add(
                &store,
                &writer,
                "incorrect password",
                None,
                Zeroizing::new(TOKEN.to_owned())
            ),
            Err(GatewayError::UnlockRequired)
        );
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
        assert!(!client_path(&store, "writer").unwrap().exists());
        quick_add(
            &store,
            &writer,
            PASSWORD,
            None,
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        assert_eq!(store.load().unwrap().grants[1].operations.len(), 3);
        assert_eq!(
            quick_add(
                &store,
                &options,
                PASSWORD,
                None,
                Zeroizing::new(TOKEN.to_owned())
            ),
            Err(GatewayError::AlreadyExists)
        );
        assert_eq!(
            crate::config::read_json::<ClientConfig>(&path, 16 * 1024)
                .unwrap()
                .capability,
            client.capability
        );
    }

    #[test]
    fn public_secrets_and_invalid_notes_are_rejected_before_quick_creation() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        for (index, secret) in [
            TOKEN.to_owned(),
            PASSWORD.to_owned(),
            base64::engine::general_purpose::STANDARD.encode(TOKEN),
            hex::encode(TOKEN),
        ]
        .into_iter()
        .enumerate()
        {
            let store =
                ConfigStore::new(directory.path().join(format!("case-{index}/gateway.json")));
            let mut options = quick_options("work");
            options.note = format!("Public context: {secret}");
            assert_eq!(
                quick_add(
                    &store,
                    &options,
                    PASSWORD,
                    Some(PASSWORD),
                    Zeroizing::new(TOKEN.to_owned())
                ),
                Err(GatewayError::SensitiveMetadata)
            );
            assert!(!store.path.exists());
            assert!(!store.path.with_file_name("gateway.mdbx").exists());
        }
        let store = ConfigStore::new(directory.path().join("invalid/gateway.json"));
        for note in [
            "\u{1b}[31m".to_owned(),
            "\u{202e}misleading".to_owned(),
            "x".repeat(1025),
        ] {
            let mut options = quick_options("work");
            options.note = note;
            assert_eq!(
                quick_add(
                    &store,
                    &options,
                    PASSWORD,
                    Some(PASSWORD),
                    Zeroizing::new(TOKEN.to_owned())
                ),
                Err(GatewayError::InvalidNote)
            );
        }
        let mut options = quick_options(TOKEN);
        assert_eq!(
            quick_add(
                &store,
                &options,
                PASSWORD,
                Some(PASSWORD),
                Zeroizing::new(TOKEN.to_owned())
            ),
            Err(GatewayError::SensitiveMetadata)
        );
        options.name = "work".to_owned();
        options.repositories = vec![format!("example/{TOKEN}")];
        assert_eq!(
            quick_add(
                &store,
                &options,
                PASSWORD,
                Some(PASSWORD),
                Zeroizing::new(TOKEN.to_owned())
            ),
            Err(GatewayError::SensitiveMetadata)
        );
        options.repositories = vec!["example/project".to_owned()];
        assert_eq!(
            quick_add(
                &store,
                &options,
                PASSWORD,
                Some("mismatch"),
                Zeroizing::new(TOKEN.to_owned())
            ),
            Err(GatewayError::PasswordRequirements)
        );
        assert!(!store.path.exists());
        assert!(!store.path.with_file_name("gateway.mdbx").exists());
    }

    #[test]
    fn editing_public_note_keeps_credential_identity_and_grant_scope() {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        let options = quick_options("work");
        quick_add(
            &store,
            &options,
            PASSWORD,
            Some(PASSWORD),
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        let before = store.load().unwrap();
        assert_eq!(
            update_note(&store, "work", TOKEN, PASSWORD),
            Err(GatewayError::SensitiveMetadata)
        );
        assert_eq!(
            update_note(&store, "work", PASSWORD, PASSWORD),
            Err(GatewayError::SensitiveMetadata)
        );
        assert_eq!(
            update_note(&store, "work", "a public note", "wrong"),
            Err(GatewayError::UnlockRequired)
        );
        update_note(
            &store,
            "work",
            "供 AI 查看项目反馈，创建仍需单独授权。",
            PASSWORD,
        )
        .unwrap();
        let after = store.load().unwrap();
        assert_eq!(
            before.grants[0].capability_hash,
            after.grants[0].capability_hash
        );
        assert_eq!(
            before.grants[0].connection_fingerprint,
            after.grants[0].connection_fingerprint
        );
        assert_eq!(
            before.connections["work"].credential_id,
            after.connections["work"].credential_id
        );
        assert_eq!(before.grants[0].operations, after.grants[0].operations);
        let vault = Vault::open(&after.vault, PASSWORD).unwrap();
        assert_eq!(
            vault
                .credential(&after.connections["work"], chrono::Utc::now().timestamp())
                .unwrap()
                .token
                .as_str(),
            TOKEN
        );
        assert_eq!(
            vault.gateway_inventory().unwrap().connections["work"].note,
            after.connections["work"].note
        );
        vault.lock().unwrap();
    }

    #[test]
    fn human_setup_encrypts_tokens_and_grant_creation_requires_password() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let password = "Synthetic human setup password 1938!";
        let vault = Vault::create(&path, password, TigaMode::Multi).unwrap();
        vault.lock().unwrap();
        drop(vault);
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        store
            .update(|_| Ok((Config::new(path.clone()), ())))
            .unwrap();
        let token = "synthetic-admin-upstream-token-only";
        add_connection(
            &store,
            "work",
            Provider::Github,
            Provider::Github.default_api_base(),
            "",
            password,
            Zeroizing::new(token.to_owned()),
        )
        .unwrap();
        let options = GrantOptions {
            name: "test-agent".to_owned(),
            connection: "work".to_owned(),
            repositories: vec!["example/project".to_owned()],
            operations: vec![Operation::GetIssue],
            ttl_minutes: 30,
            requests_per_minute: 10,
            max_calls: 0,
            out: None,
        };
        assert!(matches!(
            issue_grant(&store, &options, "wrong password"),
            Err(GatewayError::UnlockRequired)
        ));
        assert!(store.load().unwrap().grants.is_empty());
        let output = issue_grant(&store, &options, password).unwrap();
        let client: ClientConfig = crate::config::read_json(&output, 16 * 1024).unwrap();
        client.validate().unwrap();
        assert!(
            store
                .load()
                .unwrap()
                .authenticate(&client.capability, chrono::Utc::now().timestamp())
                .is_ok()
        );
        assert!(matches!(
            issue_grant(&store, &options, password),
            Err(GatewayError::AlreadyExists)
        ));
        for file in [path, store.path.clone(), output] {
            let bytes = std::fs::read(file).unwrap();
            assert!(
                !bytes
                    .windows(token.len())
                    .any(|window| window == token.as_bytes())
            );
            assert!(
                !bytes
                    .windows(password.len())
                    .any(|window| window == password.as_bytes())
            );
        }
    }

    fn grant_store() -> (tempfile::TempDir, ConfigStore) {
        use crate::test_support::{PASSWORD, TOKEN};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("vault.mdbx");
        let vault = Vault::create(&path, PASSWORD, TigaMode::Multi).unwrap();
        vault.lock().unwrap();
        drop(vault);
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        store.update(|_| Ok((Config::new(path), ()))).unwrap();
        add_connection(
            &store,
            "work",
            Provider::Github,
            Provider::Github.default_api_base(),
            "",
            PASSWORD,
            Zeroizing::new(TOKEN.to_owned()),
        )
        .unwrap();
        (directory, store)
    }

    #[test]
    fn issued_grants_always_close_and_a_refresh_rotates_the_bearer() {
        use crate::test_support::PASSWORD;
        let (_directory, store) = grant_store();
        let output = issue_grant(
            &store,
            &GrantOptions {
                name: "agent".to_owned(),
                connection: "work".to_owned(),
                repositories: vec!["example/project".to_owned()],
                operations: vec![Operation::GetIssue],
                ttl_minutes: 0,
                requests_per_minute: 10,
                max_calls: 3,
                out: None,
            },
            PASSWORD,
        )
        .unwrap();
        let config = store.load().unwrap();
        let grant = &config.grants[0];
        let client: ClientConfig = crate::config::read_json(&output, 16 * 1024).unwrap();
        assert_eq!(capability_hash(&client.capability), grant.capability_hash);
        assert_ne!(grant.expires_at, 0, "no grant may be perpetual");
        assert_eq!(
            (grant.expires_at - grant.issued_at) / 60,
            i64::from(DEFAULT_GRANT_TTL_MINUTES)
        );
        assert!(matches!(
            config.authenticate(&client.capability, grant.expires_at),
            Err(GatewayError::ReauthorizationRequired)
        ));

        let refreshed = refresh_grant(
            &store,
            &RefreshOptions {
                name: "agent".to_owned(),
                window: Some(15),
                call_cap: None,
            },
            PASSWORD,
        )
        .unwrap();
        assert_eq!(refreshed, output, "the MCP client path must stay put");
        let config = store.load().unwrap();
        let grant = &config.grants[0];
        let rotated: ClientConfig = crate::config::read_json(&output, 16 * 1024).unwrap();
        assert_ne!(rotated.capability, client.capability);
        assert_eq!(capability_hash(&rotated.capability), grant.capability_hash);
        assert_eq!(
            (grant.expires_at - grant.issued_at) / 60,
            15,
            "an explicit window replaces the default"
        );
        assert_eq!(grant.max_calls, 3, "a refresh must not widen the call cap");
        let now = chrono::Utc::now().timestamp();
        assert!(matches!(
            config.authenticate(&client.capability, now),
            Err(GatewayError::Unauthorized)
        ));
        assert_eq!(
            config.authenticate(&rotated.capability, now).unwrap().name,
            "agent"
        );
        assert_eq!(config.grants.len(), 1);
        assert_eq!(config.grants[0].connection, "work");
        assert_eq!(config.grants[0].repositories.len(), 1);
        assert!(config.grants[0].repositories.contains("example/project"));
        assert_eq!(config.grants[0].operations.len(), 1);
        assert!(config.grants[0].operations.contains(&Operation::GetIssue));

        refresh_grant(
            &store,
            &RefreshOptions {
                name: "agent".to_owned(),
                window: None,
                call_cap: None,
            },
            PASSWORD,
        )
        .unwrap();
        let grant = &store.load().unwrap().grants[0];
        assert_eq!(
            (grant.expires_at - grant.issued_at) / 60,
            15,
            "an omitted window reuses the previous session instead of widening it"
        );
    }
}
