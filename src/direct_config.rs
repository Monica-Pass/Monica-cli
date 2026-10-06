//! Explicit, trusted local configuration of clients that hold the upstream key.
//! Never reachable through MCP; neither credentials nor rendered files are returned.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table, value};
use zeroize::{Zeroize, Zeroizing};

use crate::api_keys::{ApiProtocol, Authentication};
use crate::config::{ConfigStore, ensure_parent, private_file};
use crate::error::{GatewayError, Result};
use crate::model::{Provider, validate_api_base, validate_name};
use crate::upstream::reject_secret_value;
use crate::vault::Vault;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    Codex,
    Claude,
}

impl Client {
    fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn protocol(self) -> ApiProtocol {
        match self {
            Self::Codex => ApiProtocol::Openai,
            Self::Claude => ApiProtocol::Anthropic,
        }
    }

    fn authentication(self) -> Authentication {
        match self {
            Self::Codex => Authentication::Bearer,
            Self::Claude => Authentication::XApiKey,
        }
    }
}

#[derive(clap::Args)]
pub struct Options {
    /// Client whose native configuration is written; it will hold the raw upstream Key.
    #[arg(long, value_enum)]
    pub client: Client,
    /// Exact upstream model ID. Monica does not infer or translate model names.
    #[arg(long)]
    pub model: String,
    /// Explicit client configuration file (.toml for Codex, .json for Claude Code).
    #[arg(long, value_name = "FILE")]
    pub output: PathBuf,
    /// Merge into an existing file, preserving unrelated settings and making a private backup.
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// Write a manually supplied Key; hidden prompt or --secrets-stdin {"token":...}.
    Manual {
        #[command(flatten)]
        options: Options,
        /// HTTPS service root or API base, not a complete request endpoint.
        #[arg(long)]
        api_base: String,
        /// Defaults to Bearer for Codex and x-api-key for Claude Code.
        #[arg(long, value_enum)]
        auth: Option<Authentication>,
    },
    /// Copy a bound API Key into client settings after password and Tiga export authorization.
    Saved {
        #[arg(value_name = "CONNECTION")]
        name: String,
        #[command(flatten)]
        options: Options,
    },
}

pub fn manual(
    options: &Options,
    api_base: &str,
    auth: Option<Authentication>,
    token: &str,
) -> Result<Value> {
    write(
        options,
        api_base,
        auth.unwrap_or(options.client.authentication()),
        token,
    )
}

pub fn saved(
    store: &ConfigStore,
    name: &str,
    options: &Options,
    password: &(impl crate::credentials::VaultPassword + ?Sized),
) -> Result<Value> {
    validate_name(name)?;
    let _guard = store.acquire_broker_lock()?;
    let config = store.load()?;
    let binding = config.connections.get(name).ok_or(GatewayError::NotFound)?;
    let source = binding
        .api_key
        .as_ref()
        .ok_or(GatewayError::InvalidRequest)?;
    if source.protocol != options.client.protocol() {
        return Err(GatewayError::InvalidRequest);
    }
    let path = crate::admin::absolute(&options.output)?;
    if [store.path.as_path(), config.vault.as_path()]
        .iter()
        .any(|protected| same_file(&path, protected))
    {
        return Err(GatewayError::InvalidRequest);
    }
    reject_secret_value(
        &json!([path, options.model, binding.api_base]),
        password.as_ref(),
    )
    .map_err(|_| GatewayError::SensitiveMetadata)?;
    let vault = Vault::open(&config.vault, password)?;
    let credential = vault.export_bound_api_key(binding, chrono::Utc::now().timestamp());
    vault.lock()?;
    let credential = credential?;
    write(options, &binding.api_base, source.auth, &credential.token)
}

pub fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || fs::canonicalize(left)
            .ok()
            .zip(fs::canonicalize(right).ok())
            .is_some_and(|(a, b)| a == b)
}

fn client_base(client: Client, input: &str) -> Result<String> {
    if input.len() > 2048 || input.chars().any(char::is_control) || input.contains('\\') {
        return Err(GatewayError::InvalidConfig);
    }
    let mut url = validate_api_base(
        &format!("{}/", input.trim_end_matches('/')),
        Provider::ApiKey,
    )?;
    let path = url.path().trim_end_matches('/');
    if ["/responses", "/chat/completions", "/messages", "/models"]
        .iter()
        .any(|end| path.ends_with(end))
    {
        return Err(GatewayError::InvalidConfig);
    }
    let path = match client {
        Client::Codex if path.is_empty() => "/v1".to_owned(),
        Client::Claude => path.strip_suffix("/v1").unwrap_or(path).to_owned(),
        _ => path.to_owned(),
    };
    url.set_path(&path);
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

fn read_existing(path: &Path) -> Result<Option<Zeroizing<String>>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(GatewayError::StateUnavailable),
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > MAX_CONFIG_BYTES {
                return Err(GatewayError::InvalidConfig);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.nlink() != 1 {
                    return Err(GatewayError::InvalidConfig);
                }
            }
        }
    }
    let mut text = Zeroizing::new(String::new());
    File::open(path)
        .and_then(|file| file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut text))
        .map_err(|_| GatewayError::InvalidConfig)?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(GatewayError::InvalidConfig);
    }
    Ok(Some(text))
}

fn write(options: &Options, api_base: &str, auth: Authentication, token: &str) -> Result<Value> {
    if token.is_empty()
        || token.len() > 4096
        || !token.bytes().all(|b| (33..=126).contains(&b))
        || options.model.is_empty()
        || options.model.len() > 256
        || options.model.chars().any(char::is_control)
        || (options.client == Client::Codex && auth != Authentication::Bearer)
    {
        return Err(GatewayError::InvalidRequest);
    }
    let base = client_base(options.client, api_base)?;
    let path = crate::admin::absolute(&options.output)?;
    let extension = match options.client {
        Client::Codex => "toml",
        Client::Claude => "json",
    };
    if path.extension().and_then(|v| v.to_str()) != Some(extension) {
        return Err(GatewayError::InvalidRequest);
    }
    reject_secret_value(&json!([path, base, options.model]), token)
        .map_err(|_| GatewayError::SensitiveMetadata)?;
    let original = read_existing(&path)?;
    if original.is_some() && !options.force {
        return Err(GatewayError::AlreadyExists);
    }
    let rendered = match options.client {
        Client::Codex => render_codex(
            original.as_deref().map(String::as_str),
            &base,
            &options.model,
            token,
        )?,
        Client::Claude => render_claude(
            original.as_deref().map(String::as_str),
            &base,
            &options.model,
            auth,
            token,
        )?,
    };
    let changed = original.as_deref().map(String::as_str) != Some(rendered.as_str());
    if rendered.len() as u64 > MAX_CONFIG_BYTES {
        return Err(GatewayError::InvalidConfig);
    }
    let mut backup = None;
    if changed {
        ensure_parent(&path)?;
        // Never overwrite a concurrent edit observed after rendering.
        if read_existing(&path)? != original {
            return Err(GatewayError::ObjectChanged);
        }
        if let Some(original) = &original {
            let file_name = path
                .file_name()
                .ok_or(GatewayError::InvalidRequest)?
                .to_string_lossy();
            let copy =
                path.with_file_name(format!("{file_name}.monica-{}.bak", uuid::Uuid::new_v4()));
            persist(&copy, original.as_bytes(), false)?;
            backup = Some(copy);
        }
        persist(&path, rendered.as_bytes(), original.is_some())?;
    } else {
        private_file(&path)?;
    }
    Ok(
        json!({"mode":"direct", "client":options.client.name(), "base_url":base,
        "model":options.model, "config_file":path, "backup_file":backup, "changed":changed,
        "contains_upstream_key":true, "monica_limits_apply":false}),
    )
}

fn persist(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    let parent = path.parent().ok_or(GatewayError::InvalidRequest)?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| GatewayError::StateUnavailable)?;
    private_file(temp.path())?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
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
        .and_then(|file| file.sync_all())
        .map_err(|_| GatewayError::StateUnavailable)?;
    Ok(())
}

fn render_codex(
    original: Option<&str>,
    base: &str,
    model: &str,
    token: &str,
) -> Result<Zeroizing<String>> {
    let mut doc = original
        .unwrap_or("")
        .parse::<DocumentMut>()
        .map_err(|_| GatewayError::InvalidConfig)?;
    if !doc.contains_key("model_providers") {
        doc["model_providers"] = Item::Table(Table::new());
    }
    let providers = doc["model_providers"]
        .as_table_like_mut()
        .ok_or(GatewayError::InvalidConfig)?;
    if !providers.contains_key("monica_direct") {
        providers.insert("monica_direct", Item::Table(Table::new()));
    }
    let provider = providers
        .get_mut("monica_direct")
        .and_then(Item::as_table_like_mut)
        .ok_or(GatewayError::InvalidConfig)?;
    // An existing alternative credential/routing mechanism must be resolved by the owner.
    for key in [
        "env_key",
        "auth",
        "aws",
        "gateway_oauth",
        "http_headers",
        "env_http_headers",
        "query_params",
    ] {
        if provider.contains_key(key) {
            return Err(GatewayError::InvalidConfig);
        }
    }
    for (key, entry) in [
        ("name", value("Monica direct")),
        ("base_url", value(base)),
        ("wire_api", value("responses")),
        ("requires_openai_auth", value(false)),
        ("experimental_bearer_token", value(token)),
        ("supports_websockets", value(false)),
    ] {
        provider.insert(key, entry);
    }
    doc["model_provider"] = value("monica_direct");
    doc["model"] = value(model);
    if let Some(profile) = doc.get("profile") {
        let profile = profile
            .as_str()
            .ok_or(GatewayError::InvalidConfig)?
            .to_owned();
        let selected = doc
            .get_mut("profiles")
            .and_then(Item::as_table_like_mut)
            .and_then(|profiles| profiles.get_mut(&profile))
            .and_then(Item::as_table_like_mut)
            .ok_or(GatewayError::InvalidConfig)?;
        selected.insert("model_provider", value("monica_direct"));
        selected.insert("model", value(model));
    }
    let rendered = Zeroizing::new(doc.to_string());
    Ok(rendered)
}

fn render_claude(
    original: Option<&str>,
    base: &str,
    model: &str,
    auth: Authentication,
    token: &str,
) -> Result<Zeroizing<String>> {
    if let Some(text) = original {
        validate_json(text, 0)?;
    }
    let mut root: Value =
        serde_json::from_str(original.unwrap_or("{}")).map_err(|_| GatewayError::InvalidConfig)?;
    let object = root.as_object_mut().ok_or(GatewayError::InvalidConfig)?;
    object.entry("env").or_insert_with(|| json!({}));
    let env = object
        .get_mut("env")
        .and_then(Value::as_object_mut)
        .ok_or(GatewayError::InvalidConfig)?;
    if env.values().any(|v| !v.is_string()) {
        return Err(GatewayError::InvalidConfig);
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        env.remove(key);
    }
    let key = match auth {
        Authentication::Bearer => "ANTHROPIC_AUTH_TOKEN",
        Authentication::XApiKey => "ANTHROPIC_API_KEY",
    };
    env.insert(key.to_owned(), json!(token));
    env.insert("ANTHROPIC_BASE_URL".to_owned(), json!(base));
    object.remove("apiKeyHelper");
    object.insert("model".to_owned(), json!(model));
    let rendered = Zeroizing::new(
        serde_json::to_string_pretty(&root).map_err(|_| GatewayError::InvalidConfig)?,
    );
    zeroize_json(&mut root);
    Ok(rendered)
}

// Reject duplicate JSON fields before a merge can silently discard user settings.
fn validate_json(text: &str, depth: usize) -> Result<()> {
    use serde::{Deserialize, Deserializer};
    use serde_json::value::RawValue;
    struct Fields<'a>(Vec<&'a RawValue>);
    impl<'de> Deserialize<'de> for Fields<'de> {
        fn deserialize<D: Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Fields<'de>;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("unique JSON fields")
                }
                fn visit_map<M: serde::de::MapAccess<'de>>(
                    self,
                    mut map: M,
                ) -> std::result::Result<Self::Value, M::Error> {
                    let mut names = std::collections::BTreeSet::new();
                    let mut values = Vec::new();
                    while let Some((name, value)) = map.next_entry::<String, &'de RawValue>()? {
                        if !names.insert(name) {
                            return Err(serde::de::Error::custom("duplicate field"));
                        }
                        values.push(value);
                    }
                    Ok(Fields(values))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }
    if depth > 128 {
        return Err(GatewayError::InvalidConfig);
    }
    let text = text.trim_start();
    let values = match text.as_bytes().first() {
        Some(b'{') => serde_json::from_str::<Fields<'_>>(text).map(|f| f.0),
        Some(b'[') => serde_json::from_str::<Vec<&RawValue>>(text),
        _ => return Ok(()),
    }
    .map_err(|_| GatewayError::InvalidConfig)?;
    for value in values {
        validate_json(value.get(), depth + 1)?;
    }
    Ok(())
}

fn zeroize_json(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(zeroize_json),
        Value::Object(values) => values.values_mut().for_each(zeroize_json),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const KEY: &str = "synthetic-direct-upstream-credential";

    fn options(dir: &Path, client: Client) -> Options {
        Options {
            client,
            model: "test-model".into(),
            output: dir.join(match client {
                Client::Codex => "config.toml",
                Client::Claude => "settings.json",
            }),
            force: false,
        }
    }

    #[test]
    fn manual_writes_native_files_but_reports_only_public_metadata() {
        for client in [Client::Codex, Client::Claude] {
            let dir = tempfile::tempdir().unwrap();
            let opts = options(dir.path(), client);
            let outcome = manual(&opts, "https://models.example.test/v1/", None, KEY).unwrap();
            assert!(!outcome.to_string().contains(KEY));
            assert_eq!(outcome["contains_upstream_key"], true);
            assert_eq!(outcome["monica_limits_apply"], false);
            let text = fs::read_to_string(&opts.output).unwrap();
            match client {
                Client::Codex => {
                    let doc = text.parse::<DocumentMut>().unwrap();
                    assert_eq!(
                        doc["model_providers"]["monica_direct"]["base_url"].as_str(),
                        Some("https://models.example.test/v1")
                    );
                    assert_eq!(
                        doc["model_providers"]["monica_direct"]["experimental_bearer_token"]
                            .as_str(),
                        Some(KEY)
                    );
                }
                Client::Claude => {
                    let doc: Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(
                        doc["env"]["ANTHROPIC_BASE_URL"],
                        "https://models.example.test"
                    );
                    assert_eq!(doc["env"]["ANTHROPIC_API_KEY"], KEY);
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&opts.output).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }

    #[test]
    fn force_merges_backups_and_idempotence_preserve_unrelated_settings() {
        for (client, original) in [
            (
                Client::Codex,
                "# keep comment\napproval_policy = 'on-request'\n[mcp_servers.other]\ncommand = 'other'\n",
            ),
            (
                Client::Claude,
                r#"{"permissions":{"deny":["Bash"]},"env":{"KEEP":"yes","ANTHROPIC_AUTH_TOKEN":"old-token"}}"#,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path(), client);
            fs::write(&opts.output, original).unwrap();
            assert_eq!(
                manual(&opts, "https://models.example.test", None, KEY),
                Err(GatewayError::AlreadyExists)
            );
            assert_eq!(fs::read_to_string(&opts.output).unwrap(), original);
            opts.force = true;
            let outcome = manual(&opts, "https://models.example.test", None, KEY).unwrap();
            let backup = outcome["backup_file"].as_str().unwrap();
            assert_eq!(fs::read_to_string(backup).unwrap(), original);
            let written = fs::read_to_string(&opts.output).unwrap();
            assert!(written.contains(if client == Client::Codex {
                "# keep comment"
            } else {
                "permissions"
            }));
            let repeat = manual(&opts, "https://models.example.test", None, KEY).unwrap();
            assert_eq!(repeat["changed"], false);
            assert_eq!(repeat["backup_file"], Value::Null);
            assert_eq!(fs::read_to_string(&opts.output).unwrap(), written);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }

    #[test]
    fn rejects_ambiguous_settings_without_touching_them() {
        for (client, text) in [
            (Client::Claude, "[]"),
            (Client::Claude, r#"{"env":false}"#),
            (Client::Claude, r#"{"env":{"X":"a","X":"b"}}"#),
            (Client::Codex, "[[model_providers]]\nname='unexpected'"),
            (
                Client::Codex,
                "[model_providers.monica_direct]\nenv_key='OTHER_KEY'",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut opts = options(dir.path(), client);
            opts.force = true;
            fs::write(&opts.output, text).unwrap();
            assert!(manual(&opts, "https://models.example.test", None, KEY).is_err());
            assert_eq!(fs::read_to_string(&opts.output).unwrap(), text);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn rejects_bad_endpoints_header_keys_and_secret_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path(), Client::Codex);
        for base in [
            "http://example.test",
            "https://name:password@example.test",
            "https://example.test?q=x",
            "https://example.test/#fragment",
            "https://example.test/v1/responses",
        ] {
            assert!(manual(&opts, base, None, KEY).is_err());
        }
        assert!(
            manual(
                &opts,
                "https://example.test",
                Some(Authentication::XApiKey),
                KEY
            )
            .is_err()
        );
        assert!(manual(&opts, "https://example.test", None, "key\nheader").is_err());
        opts.model = KEY.into();
        assert_eq!(
            manual(&opts, "https://example.test", None, KEY),
            Err(GatewayError::SensitiveMetadata)
        );
        assert!(!opts.output.exists());
    }

    #[test]
    fn codex_active_profile_switches_provider_without_changing_permissions() {
        let text = "profile='work'\n[profiles.work]\nmodel_provider='old'\nmodel='old-model'\napproval_policy='on-request'\n[profiles.other]\nmodel='keep'\n";
        let rendered = render_codex(
            Some(text),
            "https://models.example.test/v1",
            "new-model",
            KEY,
        )
        .unwrap();
        let doc = rendered.parse::<DocumentMut>().unwrap();
        assert_eq!(
            doc["profiles"]["work"]["model_provider"].as_str(),
            Some("monica_direct")
        );
        assert_eq!(doc["profiles"]["work"]["model"].as_str(), Some("new-model"));
        assert_eq!(
            doc["profiles"]["work"]["approval_policy"].as_str(),
            Some("on-request")
        );
        assert_eq!(doc["profiles"]["other"]["model"].as_str(), Some("keep"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_and_hardlinks() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path(), Client::Claude);
        opts.force = true;
        let other = dir.path().join("other.json");
        fs::write(&other, "{}").unwrap();
        std::os::unix::fs::symlink(&other, &opts.output).unwrap();
        assert!(manual(&opts, "https://example.test", None, KEY).is_err());
        fs::remove_file(&opts.output).unwrap();
        fs::hard_link(&other, &opts.output).unwrap();
        assert!(manual(&opts, "https://example.test", None, KEY).is_err());
        assert_eq!(fs::read_to_string(other).unwrap(), "{}");
    }

    #[tokio::test]
    async fn saved_checks_protocol_password_export_policy_and_preserves_source() {
        use crate::api_keys::SourceFormat;
        use crate::test_support::{PASSWORD, TOKEN};
        use mdbx_core::tiga::TigaMode;
        for format in [SourceFormat::AndroidApiKey, SourceFormat::NativeApiToken] {
            let (fixture, binding, _) =
                crate::api_keys_tests::bound_fixture(ApiProtocol::Openai, format, vec![]).await;
            let opts = options(fixture._directory.path(), Client::Codex);
            let before = fixture.store.load().unwrap().connections[&binding.name].clone();
            assert!(saved(&fixture.store, &binding.name, &opts, "wrong-password").is_err());
            assert!(!opts.output.exists());
            let outcome = saved(&fixture.store, &binding.name, &opts, PASSWORD).unwrap();
            assert!(!outcome.to_string().contains(TOKEN));
            assert!(fs::read_to_string(&opts.output).unwrap().contains(TOKEN));
            assert_eq!(
                fixture.store.load().unwrap().connections[&binding.name].api_key,
                before.api_key
            );
            let mut incompatible = options(fixture._directory.path(), Client::Claude);
            assert!(saved(&fixture.store, &binding.name, &incompatible, PASSWORD).is_err());
            incompatible.client = Client::Codex;
            incompatible.output = fixture._directory.path().join("denied.toml");
            let config = fixture.store.load().unwrap();
            let vault = Vault::open(&config.vault, PASSWORD).unwrap();
            vault.set_tiga_policy(TigaMode::Power, None).unwrap();
            vault.lock().unwrap();
            assert!(saved(&fixture.store, &binding.name, &incompatible, PASSWORD).is_err());
            assert!(!incompatible.output.exists());
        }
    }
}
