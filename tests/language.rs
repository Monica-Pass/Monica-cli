use std::path::Path;
use std::process::{Command, Output};

fn cli(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_monica-pass"))
        .current_dir(directory)
        .env_remove("MONICA_LANG")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LOCALAPPDATA", directory)
        .env("XDG_STATE_HOME", directory)
        .arg("--config")
        .arg(directory.join("gateway.json"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn localized_help_accepts_language_flags_before_and_after_subcommands() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec!["--lang", "en", "--help"],
        vec!["--help", "--lang", "en"],
        vec!["add", "--help", "--lang", "en"],
        vec!["webdav", "login", "--lang", "en", "--help"],
    ] {
        let output = cli(directory.path(), &args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("Usage:"), "{help}");
        assert!(help.contains("--lang"), "{help}");
    }
    for args in [
        vec!["--lang", "zh-CN", "--help"],
        vec!["add", "--help", "--lang", "zh-CN"],
        vec!["webdav", "login", "--lang", "zh-CN", "--help"],
        vec!["cmds", "dav", "ls", "--lang", "zh-CN"],
    ] {
        let output = cli(directory.path(), &args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("用法："), "{help}");
        assert!(help.contains("--lang"), "{help}");
        assert!(!help.contains("Usage:"), "{help}");
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn language_preference_works_before_vault_setup_and_overrides_are_temporary() {
    let directory = tempfile::tempdir().unwrap();
    let saved = cli(directory.path(), &["language", "zh-CN"]);
    assert!(saved.status.success(), "{:?}", saved.stderr);
    assert!(!directory.path().join("gateway.json").exists());
    assert!(!directory.path().join("gateway.mdbx").exists());
    let help = cli(directory.path(), &["--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("用法："));
    let help = cli(directory.path(), &["--lang", "en", "--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("Usage:"));
    let help = cli(directory.path(), &["--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("用法："));
    assert!(
        cli(directory.path(), &["language", "auto"])
            .status
            .success()
    );
    let help = cli(directory.path(), &["--help"]);
    assert!(String::from_utf8(help.stdout).unwrap().contains("Usage:"));
}

#[test]
fn human_errors_are_localized_without_changing_machine_output() {
    let directory = tempfile::tempdir().unwrap();
    for (language, expected) in [("en", "human terminal"), ("zh-CN", "人工终端")] {
        let output = cli(directory.path(), &["--lang", language, "serve"]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected), "{error}");
    }
    let store = monica_pass_cli::config::ConfigStore::new(directory.path().join("gateway.json"));
    let config = monica_pass_cli::config::Config::new(directory.path().join("unused.mdbx"));
    store.update(|_| Ok((config, ()))).unwrap();
    let mut outputs = Vec::new();
    for language in ["en", "zh-CN"] {
        let output = cli(directory.path(), &["--lang", language, "status", "--json"]);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        outputs.push(serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);
    for (language, expected) in [("en", "No AI grants yet."), ("zh-CN", "尚无 AI 授权。")] {
        let output = cli(directory.path(), &["--lang", language, "status"]);
        assert!(output.status.success(), "{:?}", output.stderr);
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(!report.starts_with('{'), "{report}");
        assert!(report.contains(expected), "{report}");
    }
}
