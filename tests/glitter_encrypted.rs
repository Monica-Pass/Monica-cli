//! A real encrypted native Glitter fixture using an ordinary device context.
//! The native combined-factor contract is portable; no hardware assertion is synthesized.
use std::process::{Command, Stdio};

use mdbx_core::tiga::{DeviceAssurance, DeviceContext, TigaMode};
use mdbx_storage::connection::VaultConnection;
use mdbx_storage::init::{VaultInitParams, initialize_vault_with_device_context};
use mdbx_storage::unlock::UnlockService;
use monica_pass_cli::error::GatewayError;
use monica_pass_cli::vault::Vault;
use serde_json::Value;

#[test]
fn glitter_real_encrypted_fixture_is_recognized_but_client_refuses_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted-glitter.mdbx");
    let password = "synthetic-native-glitter-fixture-password";
    {
        let mut conn = VaultConnection::create(&path).unwrap();
        let standard_device = DeviceContext {
            device_id: Some("synthetic-standard-desktop".into()),
            assurance: DeviceAssurance::Standard,
            secure_clipboard_available: false,
            screen_capture_protection_available: false,
            secure_temp_files_available: false,
        };
        initialize_vault_with_device_context(
            &conn,
            &VaultInitParams {
                default_tiga_mode: "glitter".into(),
                ..Default::default()
            },
            &standard_device,
        )
        .unwrap();
        let method = UnlockService::setup_password_security_key_with_device_context(
            &mut conn,
            password,
            &[0x53; 32],
            TigaMode::Glitter,
            &standard_device,
        )
        .unwrap();
        let kdf = mdbx_core::model::KdfParams::from_json_bytes(&method.kdf_params_ct).unwrap();
        assert!(kdf.is_supported_glitter_v1());
        assert_eq!(kdf.mem_limit_kib, 524_288);
        assert_eq!(kdf.ops_limit, 10);
        assert_eq!(kdf.parallelism, 4);
        assert!(conn.keyring().is_some());
        assert!(conn.active_session().is_some());
        conn.clear_session();
    }
    let before = std::fs::read(&path).unwrap();
    let modified = path.metadata().unwrap().modified().unwrap();
    assert!(
        !before
            .windows(password.len())
            .any(|value| value == password.as_bytes())
    );
    let check = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_monica-pass"))
            .arg("--config")
            .arg(dir.path().join("gateway.json"))
            .arg("mdbx")
            .arg("check")
            .arg(&path)
            .args(args)
            .env_remove("MONICA_LANG")
            .env("LOCALAPPDATA", dir.path())
            .env("XDG_STATE_HOME", dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let output = check(&["--json"]);
    assert!(output.status.success());
    let data: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(data["data"]["declared_tiga_profile"], "glitter");
    assert_eq!(data["data"]["header_authenticated"], false);
    assert_eq!(data["data"]["terminal_support"], "unsupported");
    for (language, explanation) in [("en", "does not integrate"), ("zh-CN", "暂未接入")] {
        let output = check(&["--lang", language]);
        assert!(output.status.success());
        let shown = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(shown.contains(explanation));
        assert!(!shown.contains(password));
    }
    assert_eq!(
        Vault::open(&path, password).err(),
        Some(GatewayError::GlitterUnavailable)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
    assert!(!dir.path().join("gateway.json").exists());
}
