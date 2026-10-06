//! Native Glitter still works; this client opts out without modifying encrypted files.
use mdbx_core::tiga::TigaMode;
use mdbx_storage::connection::VaultConnection;
use mdbx_storage::init::{VaultInitParams, initialize_vault};
use mdbx_storage::unlock::UnlockService;
use monica_pass_cli::admin::TigaLevel;
use monica_pass_cli::credentials::VaultCredentials;
use monica_pass_cli::error::GatewayError;
use monica_pass_cli::vault::Vault;
use zeroize::Zeroizing;

const PASSWORD: &str = "synthetic-client-opt-out-password";

fn native_fixture(path: &std::path::Path) {
    let mut connection = VaultConnection::create(path).unwrap();
    initialize_vault(
        &connection,
        &VaultInitParams {
            default_tiga_mode: "glitter".into(),
            ..Default::default()
        },
    )
    .unwrap();
    UnlockService::setup_password_security_key(
        &mut connection,
        PASSWORD,
        &[0x75; 32],
        TigaMode::Glitter,
    )
    .unwrap();
    assert!(connection.keyring().is_some());
    connection.clear_session();
}

#[test]
fn native_glitter_remains_known_but_cli_does_not_admit_it() {
    assert_eq!(TigaLevel::Glitter.mode(), TigaMode::Glitter);
    assert_eq!(
        TigaLevel::Glitter.require_terminal_support(),
        Err(GatewayError::GlitterUnavailable)
    );
}

#[test]
fn glitter_creation_is_refused_even_with_both_factors_before_pending_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("must-not-exist.mdbx");
    let key = dir.path().join("synthetic.key");
    for length in [31, 32] {
        std::fs::write(&key, vec![0x75; length]).unwrap();
        let credentials =
            VaultCredentials::from_key_file(Zeroizing::new(PASSWORD.into()), Some(&key)).unwrap();
        assert_eq!(
            Vault::create(&path, &credentials, TigaMode::Glitter).err(),
            Some(GatewayError::GlitterUnavailable)
        );
        assert!(!path.exists());
        assert!(!path.with_extension("mdbx-wal").exists());
    }
}

#[test]
fn legacy_profiles_keep_combined_credentials_without_silent_password_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("legacy.key");
    let wrong_key = dir.path().join("wrong.key");
    std::fs::write(&key, [0x35; 32]).unwrap();
    std::fs::write(&wrong_key, [0x36; 32]).unwrap();
    let credentials =
        VaultCredentials::from_key_file(Zeroizing::new(PASSWORD.into()), Some(&key)).unwrap();
    let wrong_credentials =
        VaultCredentials::from_key_file(Zeroizing::new(PASSWORD.into()), Some(&wrong_key)).unwrap();
    for mode in [TigaMode::Sky, TigaMode::Multi, TigaMode::Power] {
        let path = dir.path().join(format!("legacy-{mode}.mdbx"));
        let vault = Vault::create(&path, &credentials, mode).unwrap();
        assert_eq!(vault.tiga_default().unwrap(), mode);
        vault.lock().unwrap();
        drop(vault);
        assert_eq!(
            Vault::open(&path, PASSWORD).err(),
            Some(GatewayError::UnlockRequired)
        );
        assert_eq!(
            Vault::open(&path, &wrong_credentials).err(),
            Some(GatewayError::UnlockRequired)
        );
        let vault = Vault::open(&path, &credentials).unwrap();
        assert_eq!(vault.tiga_default().unwrap(), mode);
        vault.lock().unwrap();
    }
}

#[test]
fn native_glitter_and_manual_backup_still_work_but_cli_rejects_correct_factors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.mdbx");
    native_fixture(&path);
    let key = dir.path().join("synthetic.key");
    std::fs::write(&key, [0x75; 32]).unwrap();
    let credentials =
        VaultCredentials::from_key_file(Zeroizing::new(PASSWORD.into()), Some(&key)).unwrap();
    let before = std::fs::read(&path).unwrap();
    let modified = path.metadata().unwrap().modified().unwrap();
    assert_eq!(
        Vault::open(&path, PASSWORD).err(),
        Some(GatewayError::GlitterUnavailable)
    );
    assert_eq!(
        Vault::open(&path, &credentials).err(),
        Some(GatewayError::GlitterUnavailable)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    let backup = dir.path().join("native-backup.mdbx");
    mdbx_storage::backup::BackupService::create_portable_copy_path(&path, &backup).unwrap();
    let mut native = VaultConnection::open(&backup).unwrap();
    UnlockService::unlock_with_password_security_key(&mut native, PASSWORD, &[0x75; 32]).unwrap();
    assert!(native.keyring().is_some());
    native.clear_session();
    drop(native);
    assert_eq!(
        Vault::open(&backup, &credentials).err(),
        Some(GatewayError::GlitterUnavailable)
    );
}

#[test]
fn cli_rejects_glitter_create_open_and_management_without_secret_or_file_changes() {
    use monica_pass_cli::config::{Config, ConfigStore};
    use serde_json::{Value, json};
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("native.mdbx");
    native_fixture(&path);
    let key = dir.path().join("synthetic.key");
    std::fs::write(&key, [0x75; 32]).unwrap();
    let store = ConfigStore::new(dir.path().join("gateway.json"));
    let entry_id = uuid::Uuid::new_v4().to_string();
    store
        .update(|_| {
            use monica_pass_cli::api_keys::{
                ApiKeyBinding, ApiProtocol, Authentication, SourceFormat,
            };
            use monica_pass_cli::config::Connection;
            use monica_pass_cli::model::Provider;
            let mut config = Config::new(path.clone());
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            config
                .listen
                .set_port(listener.local_addr().unwrap().port());
            config.connections.insert(
                "synthetic-model".into(),
                Connection {
                    provider: Provider::ApiKey,
                    credential_id: entry_id.clone(),
                    api_base: "https://models.example.test/v1/".into(),
                    note: String::new(),
                    api_key: Some(ApiKeyBinding {
                        format: SourceFormat::AndroidApiKey,
                        protocol: ApiProtocol::Anthropic,
                        auth: Authentication::XApiKey,
                        head_commit_id: "synthetic-head".into(),
                    }),
                },
            );
            Ok((config, ()))
        })
        .unwrap();
    let config_before = std::fs::read(&store.path).unwrap();
    let file_before = std::fs::read(&path).unwrap();
    for args in [
        vec![
            "init",
            "--tiga",
            "glitter",
            "--vault",
            "not-created/new.mdbx",
        ],
        vec!["open", "native.mdbx"],
        vec!["library"],
        vec!["bind", "new-binding", "--entry", &entry_id],
        vec!["unbind", "synthetic-model"],
        vec![
            "direct-config",
            "saved",
            "synthetic-model",
            "--client",
            "claude",
            "--model",
            "synthetic-model",
            "--output",
            "must-not-configure.json",
        ],
        vec![
            "serve",
            "--proxy-grant",
            "synthetic-grant",
            "--session-minutes",
            "1",
        ],
        vec!["tiga", "show"],
        vec!["tiga", "set", "multi", "--reason", "must not downgrade"],
        vec![
            "keys",
            "export",
            "unused",
            "--private",
            "--output",
            "must-not-export.key",
        ],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_monica-pass"))
            .arg("--config")
            .arg(&store.path)
            .arg("--key-file")
            .arg(&key)
            .args(["--json", "--secrets-stdin"])
            .args(args)
            .current_dir(dir.path())
            .env("LOCALAPPDATA", dir.path())
            .env("XDG_STATE_HOME", dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // A rejection may close stdin before consuming it; no secret is required to opt out.
        let _ = child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&json!({"password": PASSWORD})).unwrap());
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains(PASSWORD));
        }
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], "glitter_unavailable", "{value}");
        assert_eq!(value["error"]["recovery"]["automatic_retry"], false);
        assert_eq!(std::fs::read(&store.path).unwrap(), config_before);
        assert_eq!(std::fs::read(&path).unwrap(), file_before);
    }
    assert!(!dir.path().join("not-created").exists());
    assert!(!dir.path().join("must-not-export.key").exists());
    assert!(!dir.path().join("must-not-configure.json").exists());
}
