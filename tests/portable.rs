use std::path::Path;
use std::process::{Command, Output};

fn cli(executable: &Path, cwd: &Path, user_state: &Path, args: &[&str]) -> Output {
    Command::new(executable)
        .current_dir(cwd)
        .env_remove("MONICA_LANG")
        .env("LOCALAPPDATA", user_state)
        .env("XDG_STATE_HOME", user_state)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn portable_state_follows_the_executable_and_explicit_config_takes_precedence() {
    let directory = tempfile::tempdir().unwrap();
    let install = directory.path().join("Monica 中文 portable");
    let cwd = directory.path().join("working");
    let user_state = directory.path().join("user-state");
    std::fs::create_dir(&install).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    let executable = install.join(if cfg!(windows) {
        "monica-pass.exe"
    } else {
        "monica-pass"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_monica-pass"), &executable).unwrap();
    let marker = install.join("monica-pass.portable");
    std::fs::write(&marker, []).unwrap();

    let result = cli(&executable, &cwd, &user_state, &["language", "zh-CN", "-j"]);
    assert!(result.status.success(), "{result:?}");
    let preferences = install.join("data/gateway.preferences.json");
    assert!(preferences.exists());
    assert!(!user_state.exists());
    assert_eq!(std::fs::read_dir(&cwd).unwrap().count(), 0);
    let help = cli(&executable, &cwd, &user_state, &["--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("用法："));

    let override_path = cwd.join("other/gateway.json");
    let result = cli(
        &executable,
        &cwd,
        &user_state,
        &[
            "-C",
            override_path.to_str().unwrap(),
            "language",
            "en",
            "-j",
        ],
    );
    assert!(result.status.success(), "{result:?}");
    assert!(override_path.with_extension("preferences.json").exists());
    assert!(
        std::fs::read_to_string(&preferences)
            .unwrap()
            .contains("zh-CN")
    );

    std::fs::write(&marker, b"invalid marker").unwrap();
    let result = cli(&executable, &cwd, &user_state, &["status", "-j"]);
    assert!(!result.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.stdout).unwrap()["error"]["code"],
        "invalid_config"
    );
    assert!(!user_state.exists());

    std::fs::remove_file(marker).unwrap();
    let result = cli(&executable, &cwd, &user_state, &["language", "en", "-j"]);
    assert!(result.status.success(), "{result:?}");
    assert!(
        user_state
            .join(if cfg!(windows) {
                "MonicaPass/gateway.preferences.json"
            } else {
                "monica-pass/gateway.preferences.json"
            })
            .exists()
    );
}
