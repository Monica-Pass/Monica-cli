//! Read-only header hints and fail-closed missing-factor preflight; headers are not authentication.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use mdbx_core::tiga::TigaMode;
use mdbx_storage::connection::VaultConnection;
use mdbx_storage::init::{VaultInitParams, initialize_vault};
use monica_pass_cli::error::GatewayError;
use monica_pass_cli::vault::Vault;
use serde_json::{Value, json};

const PASSWORD: &str = "synthetic-glitter-compatibility-password";

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_monica-pass"))
        .arg("--config")
        .arg(root.join("gateway.json"))
        .args(args)
        .env_remove("MONICA_LANG")
        .env("LOCALAPPDATA", root)
        .env("XDG_STATE_HOME", root)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn failure(output: Output, code: &str) -> Value {
    assert!(!output.status.success());
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains(PASSWORD));
    }
    let data: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["error"]["code"], code, "{data}");
    assert_eq!(data["error"]["recovery"]["automatic_retry"], false);
    data
}

fn declared_glitter(path: &Path) {
    let conn = VaultConnection::create(path).unwrap();
    initialize_vault(&conn, &VaultInitParams::default()).unwrap();
    conn.inner()
        .execute(
            "UPDATE vault_meta SET default_tiga_mode='glitter', tiga_policy_version=3,
         critical_extensions='[\"tiga-glitter-v1\"]'",
            [],
        )
        .unwrap();
}

#[test]
fn glitter_create_refuses_before_secret_input_or_state_changes() {
    let directory = tempfile::tempdir().unwrap();
    for lang in ["en", "zh-CN"] {
        failure(
            run(
                directory.path(),
                &[
                    "init",
                    "--tiga",
                    "glitter",
                    "--vault",
                    "missing/new.mdbx",
                    "--json",
                    "--lang",
                    lang,
                ],
            ),
            "glitter_unavailable",
        );
        failure(
            run(
                directory.path(),
                &[
                    "init",
                    "--tiga",
                    "glitter",
                    "--json",
                    "--lang",
                    lang,
                    "--secrets-stdin",
                ],
            ),
            "glitter_unavailable",
        );
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn glitter_direct_create_is_unsupported_before_pending_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("glitter.mdbx");
    assert_eq!(
        Vault::create(&path, PASSWORD, TigaMode::Glitter).err(),
        Some(GatewayError::GlitterUnavailable)
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn glitter_file_check_is_read_only_and_password_only_open_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("glitter.mdbx");
    declared_glitter(&path);
    let before = std::fs::read(&path).unwrap();
    let output = run(
        directory.path(),
        &["mdbx", "check", "glitter.mdbx", "--json"],
    );
    assert!(output.status.success(), "{output:?}");
    let check: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check["data"]["declared_tiga_profile"], "glitter");
    assert_eq!(check["data"]["terminal_support"], "unsupported");
    assert_eq!(check["data"]["header_authenticated"], false);
    failure(
        run(directory.path(), &["open", "glitter.mdbx", "--json"]),
        "glitter_unavailable",
    );
    assert_eq!(
        Vault::open(&path, PASSWORD).err(),
        Some(GatewayError::GlitterUnavailable)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!directory.path().join("gateway.json").exists());
}

#[test]
fn glitter_admin_refusal_preserves_existing_configuration() {
    use monica_pass_cli::admin::{NewVault, TigaLevel, initialize_with};
    use monica_pass_cli::config::ConfigStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("active.mdbx");
    let store = ConfigStore::new(dir.path().join("gateway.json"));
    initialize_with(
        &store,
        &path,
        47839,
        PASSWORD,
        PASSWORD,
        &NewVault::default(),
    )
    .unwrap();
    let before = std::fs::read(&store.path).unwrap();
    let target = dir.path().join("not-created/new.mdbx");
    assert_eq!(
        initialize_with(
            &store,
            &target,
            47839,
            PASSWORD,
            PASSWORD,
            &NewVault {
                name: Some("Glitter"),
                tiga: TigaLevel::Glitter
            }
        )
        .unwrap_err(),
        GatewayError::GlitterUnavailable
    );
    assert_eq!(
        monica_pass_cli::tiga::set(&store, PASSWORD, TigaLevel::Glitter, None).unwrap_err(),
        GatewayError::GlitterUnavailable
    );
    assert_eq!(std::fs::read(&store.path).unwrap(), before);
    assert!(!dir.path().join("not-created").exists());
}

#[test]
fn glitter_is_not_advertised_as_an_available_creation_or_policy_choice() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec!["commands", "init", "--json"],
        vec!["commands", "tiga", "set", "--json"],
    ] {
        let output = run(directory.path(), &args);
        assert!(output.status.success());
        let data: Value = serde_json::from_slice(&output.stdout).unwrap();
        let argument = data["data"]["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|arg| arg["id"] == "tiga" || arg["id"] == "level")
            .unwrap();
        assert_eq!(argument["choices"], json!(["sky", "multi", "power"]));
        assert!(
            argument["choice_details"]
                .as_array()
                .unwrap()
                .iter()
                .all(|choice| choice["name"] != "glitter")
        );
        assert_eq!(data["data"]["secret_input"]["key_file"]["persisted"], false);
    }
}
