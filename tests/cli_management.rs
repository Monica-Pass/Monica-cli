use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use monica_pass_cli::config::ConfigStore;
use monica_pass_cli::model::Operation;
use serde_json::{Value, json};

const PASSWORD: &str = "Synthetic CLI test password 8392!";
const TOKEN: &str = "synthetic-cli-service-token-42";

/// Any private half showing up in a stream is the same class of leak as a password, and
/// `success`/`failure` check every command's stdout and stderr for these markers.
const KEY_MATERIAL: &[&str] = &["PRIVATE KEY", "Proc-Type: 4,ENCRYPTED"];

fn command(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_monica-pass"));
    command
        .current_dir(directory)
        .env_remove("MONICA_LANG")
        .env("LC_ALL", "en_US.UTF-8")
        .env("LOCALAPPDATA", directory)
        .env("XDG_STATE_HOME", directory)
        .arg("--config")
        .arg(directory.join("gateway.json"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn cli(directory: &Path, args: &[&str], secrets: Option<&[u8]>) -> Output {
    let mut child = command(directory, args).spawn().unwrap();
    if let Some(secrets) = secrets {
        child.stdin.take().unwrap().write_all(secrets).unwrap();
    } else {
        drop(child.stdin.take());
    }
    child.wait_with_output().unwrap()
}

fn no_secrets(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(stream);
        for secret in [PASSWORD, TOKEN] {
            assert!(!text.contains(secret), "secret in command output");
        }
        for marker in KEY_MATERIAL {
            assert!(!text.contains(marker), "key material in command output");
        }
    }
}

fn success(output: Output) -> Value {
    no_secrets(&output);
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true, "{value}");
    value["data"].clone()
}

fn failure(output: Output, code: &str) {
    no_secrets(&output);
    assert!(!output.status.success());
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], code);
}

fn password() -> Vec<u8> {
    serde_json::to_vec(&json!({"password": PASSWORD})).unwrap()
}

fn credentials() -> Vec<u8> {
    serde_json::to_vec(&json!({"password": PASSWORD, "token": TOKEN})).unwrap()
}

#[test]
fn token_edit_uses_secret_stdin_and_revokes_previous_grants() {
    let directory = tempfile::tempdir().unwrap();
    success(cli(
        directory.path(),
        &[
            "add",
            "work",
            "--repo",
            "org/repo",
            "--json",
            "--secrets-stdin",
        ],
        Some(&credentials()),
    ));
    let new_token = "synthetic-replacement-token-523";
    let secret = serde_json::to_vec(&json!({"password":PASSWORD,"token":new_token})).unwrap();
    let output = cli(
        directory.path(),
        &["token", "work", "--json", "--secrets-stdin"],
        Some(&secret),
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(new_token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(new_token));
    let data = success(output);
    assert_eq!(data["token_updated"], true);
    assert_eq!(data["previous_grants_revoked"], true);
    let status = success(cli(directory.path(), &["status", "--json"], None));
    assert!(status["grants"].as_array().unwrap().is_empty());
    assert_eq!(status["connections"].as_array().unwrap().len(), 1);
    let discovery = success(cli(
        directory.path(),
        &["commands", "token", "--json"],
        None,
    ));
    assert_eq!(
        discovery["secret_input"]["required"],
        json!(["password", "token"])
    );
}

#[test]
fn short_passwords_work_for_new_vaults_and_quick_add_without_special_flags() {
    const SHORT_PASSWORD: &str = "735941";
    let directory = tempfile::tempdir().unwrap();
    let secret = serde_json::to_vec(&json!({"password":SHORT_PASSWORD})).unwrap();
    let credentials =
        serde_json::to_vec(&json!({"password":SHORT_PASSWORD,"token":TOKEN})).unwrap();
    let created = success(cli(
        directory.path(),
        &["n", "-j", "--secrets-stdin"],
        Some(&secret),
    ));
    assert!(Path::new(created["vault"].as_str().unwrap()).is_file());
    let connected = cli(
        directory.path(),
        &["c", "gitlab", "-p", "gitlab", "-j", "--secrets-stdin"],
        Some(&credentials),
    );
    assert!(!String::from_utf8_lossy(&connected.stdout).contains(SHORT_PASSWORD));
    success(connected);

    let quick = tempfile::tempdir().unwrap();
    success(cli(
        quick.path(),
        &[
            "a",
            "work",
            "-r",
            "example/project",
            "-j",
            "--secrets-stdin",
        ],
        Some(&credentials),
    ));
    let store = ConfigStore::new(quick.path().join("gateway.json"));
    assert_eq!(store.load().unwrap().connections.len(), 1);
    let wrong = serde_json::to_vec(&json!({"password":"735942"})).unwrap();
    failure(
        cli(
            quick.path(),
            &["e", "work", "changed", "-j", "--secrets-stdin"],
            Some(&wrong),
        ),
        "unlock_required",
    );
    success(cli(
        quick.path(),
        &["e", "work", "changed", "-j", "--secrets-stdin"],
        Some(&secret),
    ));

    let blank = tempfile::tempdir().unwrap();
    for command in ["n", "a"] {
        let (args, input) = if command == "n" {
            (
                vec!["n", "-j", "--secrets-stdin"],
                json!({"password":" \u{2003} "}),
            )
        } else {
            (
                vec![
                    "a",
                    "work",
                    "-r",
                    "example/project",
                    "-j",
                    "--secrets-stdin",
                ],
                json!({"password":" \u{2003} ", "token":TOKEN}),
            )
        };
        failure(
            cli(
                blank.path(),
                &args,
                Some(&serde_json::to_vec(&input).unwrap()),
            ),
            "password_requirements",
        );
    }
    assert_eq!(std::fs::read_dir(blank.path()).unwrap().count(), 0);
}

fn add(directory: &Path) -> Value {
    success(cli(
        directory,
        &[
            "a",
            "work",
            "-r",
            "example/project",
            "-n",
            "项目反馈",
            "-j",
            "--secrets-stdin",
        ],
        Some(&credentials()),
    ))
}

#[test]
fn command_discovery_uses_the_real_grammar_without_a_vault() {
    let directory = tempfile::tempdir().unwrap();
    let root = success(cli(directory.path(), &["cmds", "-j"], None));
    let check = root["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == "check")
        .unwrap();
    assert_eq!(
        check["argument_groups"][0]["required_one_of"],
        json!(["name", "client"])
    );
    let english = success(cli(
        directory.path(),
        &["cmds", "a", "--json", "--lang", "en"],
        None,
    ));
    let chinese = success(cli(
        directory.path(),
        &["commands", "add", "--json", "--lang", "zh-CN"],
        None,
    ));
    assert_eq!(english, chinese);
    assert_eq!(english["name"], "add");
    assert!(english["aliases"].as_array().unwrap().contains(&json!("a")));
    let repo = english["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|arg| arg["long"] == "repo")
        .unwrap();
    assert_eq!(repo["short"], "r");
    assert_eq!(repo["required"], true);
    assert_eq!(
        english["secret_input"]["required"],
        json!(["password", "token"])
    );
    let dav = success(cli(directory.path(), &["cmds", "dav", "o", "-j"], None));
    assert_eq!(
        dav["secret_input"]["required"],
        json!(["password", "webdav_password"])
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn aliases_and_piped_secrets_complete_the_management_workflow() {
    let directory = tempfile::tempdir().unwrap();
    let created = add(directory.path());
    assert_eq!(created["name"], "work");
    assert!(created["mcp"]["mcpServers"]["work"]["command"].is_string());
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let config = store.load().unwrap();
    assert_eq!(config.connections["work"].note, "项目反馈");
    assert_eq!(
        config.grants[0].repositories,
        ["example/project".to_owned()].into()
    );
    assert_eq!(
        config.grants[0].operations,
        [Operation::ListIssues, Operation::GetIssue].into()
    );
    for file in [&store.path, &config.vault] {
        let bytes = std::fs::read(file).unwrap();
        for secret in [PASSWORD, TOKEN] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
    }

    success(cli(
        directory.path(),
        &["e", "work", "新的用途说明", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    let shown = success(cli(directory.path(), &["show", "work", "-j"], None));
    assert_eq!(shown["name"], "work");
    assert_eq!(shown["note"], "新的用途说明");
    assert_eq!(
        shown["grants"][0]["operations"],
        json!(["list_issues", "get_issue"])
    );
    failure(
        cli(directory.path(), &["show", "missing", "-j"], None),
        "not_found",
    );

    let granted = success(cli(
        directory.path(),
        &[
            "g",
            "writer",
            "-c",
            "work",
            "-r",
            "example/project",
            "--op",
            "create-issue",
            "-t",
            "15",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(granted["name"], "writer");
    let settings = success(cli(directory.path(), &["m", "writer", "-j"], None));
    assert_eq!(settings["mcp"], granted["mcp"]);
    assert!(Path::new(settings["settings_file"].as_str().unwrap()).is_file());
    let before = store.load().unwrap();
    assert_eq!(before.grants[1].operations, [Operation::CreateIssue].into());
    success(cli(directory.path(), &["rv", "writer", "-j"], None));
    assert_eq!(store.load().unwrap().grants.len(), 1);
    failure(
        cli(directory.path(), &["m", "writer", "-j"], None),
        "not_found",
    );
    let listed = success(cli(directory.path(), &["ls", "-j"], None));
    assert_eq!(listed["connections"].as_array().unwrap().len(), 1);
}

#[test]
fn missing_and_invalid_secret_input_fails_without_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    failure(
        cli(
            directory.path(),
            &["add", "work", "-r", "example/project", "-j"],
            None,
        ),
        "secret_input_required",
    );
    for input in [b"{}".as_slice(), b"{\"password\":\"x\"}".as_slice()] {
        failure(
            cli(
                directory.path(),
                &[
                    "add",
                    "work",
                    "-r",
                    "example/project",
                    "-j",
                    "--secrets-stdin",
                ],
                Some(input),
            ),
            "secret_input_required",
        );
    }
    let invalid = [
        serde_json::to_string(&json!([PASSWORD, TOKEN])).unwrap(),
        format!("{{\"password\":\"{PASSWORD}\",\"token\":\"{TOKEN}\",\"extra\":true}}"),
        format!(
            "{{\"password\":\"{PASSWORD}\",\"password\":\"{PASSWORD}\",\"token\":\"{TOKEN}\"}}"
        ),
        format!("{{\"password\":\"{PASSWORD}\",\"token\":null}}"),
        format!(
            "{{\"password\":\"{PASSWORD}\",\"token\":\"{}\"}}",
            "x".repeat(17000)
        ),
        format!("not-json {TOKEN}"),
    ];
    for input in invalid {
        failure(
            cli(
                directory.path(),
                &[
                    "a",
                    "work",
                    "-r",
                    "example/project",
                    "-j",
                    "--secrets-stdin",
                ],
                Some(input.as_bytes()),
            ),
            "invalid_secret_input",
        );
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn parser_and_runtime_errors_are_json_without_echoing_rejected_values() {
    let directory = tempfile::tempdir().unwrap();
    for args in [
        vec![
            "--json",
            "add",
            "work",
            "--repo",
            "example/project",
            "--token",
            TOKEN,
        ],
        vec![
            "a",
            "work",
            "-r",
            "example/project",
            "-j",
            "--password",
            PASSWORD,
        ],
        vec![
            "a",
            "work",
            "-r",
            "example/project",
            "--token",
            TOKEN,
            "--json",
        ],
        vec!["--json", "--lang", "zh-CN", "add"],
    ] {
        let output = cli(directory.path(), &args, None);
        assert_eq!(output.status.code(), Some(2));
        failure(output, "invalid_request");
    }
    failure(
        cli(
            directory.path(),
            &[
                "a",
                "work",
                "-r",
                "example/project",
                "-n",
                TOKEN,
                "-j",
                "--secrets-stdin",
            ],
            Some(&credentials()),
        ),
        "sensitive_metadata",
    );
    assert!(!directory.path().join("gateway.json").exists());
    let output = cli(
        directory.path(),
        &["mcp", "--client", "missing.json", "--json"],
        None,
    );
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "MCP stdout must contain only protocol messages"
    );
}

#[test]
fn separate_setup_and_local_vault_switch_are_available_without_a_terminal() {
    let directory = tempfile::tempdir().unwrap();
    success(cli(
        directory.path(),
        &["n", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    success(cli(
        directory.path(),
        &[
            "c",
            "team",
            "-p",
            "gitlab",
            "-n",
            "团队问题跟踪",
            "-j",
            "--secrets-stdin",
        ],
        Some(&credentials()),
    ));
    success(cli(
        directory.path(),
        &[
            "g",
            "reader",
            "-c",
            "team",
            "-r",
            "group/sub/project",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let before = store.load().unwrap();
    assert_eq!(before.grants.len(), 1);
    assert_eq!(before.connections.len(), 1);
    let wrong = serde_json::to_vec(&json!({"password":"a wrong synthetic password"})).unwrap();
    failure(
        cli(
            directory.path(),
            &["e", "team", "Must not be saved", "-j", "--secrets-stdin"],
            Some(&wrong),
        ),
        "unlock_required",
    );
    assert_eq!(
        store.load().unwrap().connections["team"].note,
        "团队问题跟踪"
    );

    let source = before.vault.to_str().unwrap();
    let opened = success(cli(
        directory.path(),
        &["o", source, "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert_eq!(opened["grants_reset"], true);
    assert_eq!(opened["connections"], 1);
    let after = store.load().unwrap();
    assert!(after.grants.is_empty());
    assert_eq!(after.connections["team"].note, "团队问题跟踪");
    assert!(before.vault.is_file());
    assert_ne!(before.vault, after.vault);

    let dav = success(cli(directory.path(), &["dav", "st", "-j"], None));
    assert_eq!(dav["password_saved"], false);
    assert!(dav["profile"].is_null());
    let missing = cli(
        directory.path(),
        &[
            "dav",
            "in",
            "-u",
            "https://example.invalid/",
            "-n",
            "tester",
            "-j",
        ],
        None,
    );
    failure(missing, "secret_input_required");
    let dav_password =
        serde_json::to_vec(&json!({"webdav_password":"synthetic-webdav-password"})).unwrap();
    failure(
        cli(
            directory.path(),
            &[
                "dav",
                "in",
                "-u",
                "http://example.invalid/",
                "-n",
                "tester",
                "-j",
                "--secrets-stdin",
            ],
            Some(&dav_password),
        ),
        "invalid_web_dav",
    );
    assert!(!directory.path().join("gateway.webdav.json").exists());
}

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_cli_management_command_drains_a_running_broker_before_editing() {
    let directory = tempfile::tempdir().unwrap();
    add(directory.path());
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    store
        .update(|config| {
            let mut config = config.unwrap();
            config.listen.set_port(port);
            Ok((config, ()))
        })
        .unwrap();
    drop(listener);
    // The generated capability uses the original port. Keep this test client
    // consistent with the temporary broker address.
    let client_path = monica_pass_cli::admin::client_path(&store, "work").unwrap();
    let mut client: Value = serde_json::from_slice(&std::fs::read(&client_path).unwrap()).unwrap();
    client["endpoint"] = json!(format!("http://127.0.0.1:{port}/"));
    monica_pass_cli::config::write_json(&client_path, &client, true).unwrap();

    let mut broker = Broker(
        command(directory.path(), &["u", "-j", "--secrets-stdin"])
            .spawn()
            .unwrap(),
    );
    broker
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(&password())
        .unwrap();
    let mut reader = BufReader::new(broker.0.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let ready: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(ready["event"], "ready");
    assert!(!line.contains(PASSWORD));
    let checked = success(cli(directory.path(), &["ck", "work", "-j"], None));
    assert_eq!(checked["tools"].as_array().unwrap().len(), 3);
    let status = success(cli(directory.path(), &["st", "-j"], None));
    assert_eq!(status["broker_running"], true);

    success(cli(
        directory.path(),
        &[
            "e",
            "work",
            "Changed while broker was running",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let status = success(cli(directory.path(), &["st", "-j"], None));
    assert_eq!(status["broker_running"], false);
    assert_eq!(
        store.load().unwrap().connections["work"].note,
        "Changed while broker was running"
    );
    assert!(broker.0.wait().unwrap().success());
}

#[test]
fn service_api_grant_requires_explicit_wide_scope_and_call_is_discoverable() {
    let directory = tempfile::tempdir().unwrap();
    success(cli(
        directory.path(),
        &[
            "add",
            "work",
            "--repo",
            "org/repo",
            "--json",
            "--secrets-stdin",
        ],
        Some(&credentials()),
    ));
    let invalid = cli(
        directory.path(),
        &[
            "grant",
            "invalid",
            "--connection",
            "work",
            "--repo",
            "org/repo",
            "--operation",
            "api-write",
            "--json",
            "--secrets-stdin",
        ],
        Some(&password()),
    );
    failure(invalid, "invalid_request");
    success(cli(
        directory.path(),
        &[
            "grant",
            "service",
            "--connection",
            "work",
            "--repo",
            "*",
            "--operation",
            "api-read",
            "--operation",
            "api-write",
            "--json",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let config = ConfigStore::new(directory.path().join("gateway.json"))
        .load()
        .unwrap();
    let grant = config.grants.iter().find(|g| g.name == "service").unwrap();
    assert_eq!(
        grant.operations,
        [Operation::ApiRead, Operation::ApiWrite].into()
    );
    assert_eq!(grant.repositories, ["*".to_owned()].into());
    let command = success(cli(directory.path(), &["commands", "call", "--json"], None));
    assert_eq!(command["secret_input"]["required"], json!([]));
}

#[test]
fn key_entries_generate_import_edit_export_and_stay_out_of_the_gateway() {
    let directory = tempfile::tempdir().unwrap();
    add(directory.path());
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gpg");
    let secret_ring = fixtures.join("ed25519-secret.asc");
    let secret_armor = std::fs::read_to_string(&secret_ring).unwrap();

    let ssh = success(cli(
        directory.path(),
        &[
            "keys",
            "ssh",
            "laptop",
            "--generate",
            "ed25519",
            "--comment",
            "cli-fixture",
            "-n",
            "work laptop",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let generated = &ssh["key"];
    assert_eq!(generated["login_type"], "SSH_KEY");
    assert_eq!(generated["algorithm"], "ED25519");
    assert_eq!(generated["key_size"], 256);
    assert_eq!(generated["comment"], "cli-fixture");
    assert_eq!(generated["notes"], "work laptop");
    assert_eq!(generated["has_secret"], true);
    assert_eq!(generated["chunk_count"], 0);
    assert!(
        generated["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("SHA256:")
    );
    assert!(
        generated["public_key"]
            .as_str()
            .unwrap()
            .starts_with("ssh-ed25519 AAAA")
    );
    let logical_id = generated["logical_id"].as_str().unwrap();
    assert!(logical_id.starts_with("password:"), "{logical_id}");
    assert_eq!(logical_id.len(), "password:".len() + 36);
    // Android derives the physical id with UUID.nameUUIDFromBytes, which is a v3 UUID.
    assert_eq!(
        generated["entry_id"].as_str().unwrap().chars().nth(14),
        Some('3')
    );

    let gpg = success(cli(
        directory.path(),
        &[
            "keys",
            "gpg",
            "signing",
            "--private-key",
            secret_ring.to_str().unwrap(),
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let imported = &gpg["key"];
    assert_eq!(imported["login_type"], "GPG_KEY");
    assert_eq!(imported["algorithm"], "EDDSA");
    assert_eq!(
        imported["fingerprint"],
        "837A7DFF0F73A85EAE2D04E6B0B4C3983F688DA1"
    );
    assert_eq!(imported["comment"], "Cli Fixture <cli-fixture@example.com>");
    assert_eq!(imported["has_secret"], true);
    assert_eq!(imported["chunk_count"], 1);
    assert_eq!(
        imported["entry_id"].as_str().unwrap().chars().nth(14),
        Some('3')
    );

    let list = success(cli(
        directory.path(),
        &["keys", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert_eq!(
        list["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["title"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["laptop", "signing"]
    );

    let private_path = directory.path().join("id_ed25519");
    let exported = success(cli(
        directory.path(),
        &[
            "keys",
            "export",
            "laptop",
            "--private",
            "-o",
            private_path.to_str().unwrap(),
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(exported["name"], "laptop");
    assert_eq!(exported["private"], true);
    assert_eq!(
        exported["path"].as_str().unwrap(),
        private_path.display().to_string()
    );
    let text = std::fs::read_to_string(&private_path).unwrap();
    assert_eq!(
        text.len() as u64,
        exported["bytes"].as_u64().unwrap(),
        "reported size differs from the file"
    );
    assert!(text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
    // ssh-keygen refuses a truncated PEM, and Android compares the text byte for byte.
    assert!(text.ends_with('\n'));
    failure(
        cli(
            directory.path(),
            &[
                "keys",
                "export",
                "laptop",
                "--private",
                "-o",
                private_path.to_str().unwrap(),
                "-j",
                "--secrets-stdin",
            ],
            Some(&password()),
        ),
        "already_exists",
    );

    let ring_path = directory.path().join("signing.asc");
    success(cli(
        directory.path(),
        &[
            "keys",
            "export",
            "signing",
            "--private",
            "-o",
            ring_path.to_str().unwrap(),
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(std::fs::read_to_string(&ring_path).unwrap(), secret_armor);

    let public_path = directory.path().join("id_ed25519.pub");
    let public = success(cli(
        directory.path(),
        &[
            "keys",
            "export",
            "laptop",
            "-o",
            public_path.to_str().unwrap(),
            "--force",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(public["private"], false);
    assert_eq!(
        std::fs::read_to_string(&public_path).unwrap(),
        format!("{}\n", generated["public_key"].as_str().unwrap())
    );

    let edited = success(cli(
        directory.path(),
        &[
            "keys",
            "edit",
            "laptop",
            "--title",
            "work-laptop",
            "--comment",
            "cli-fixture-renamed",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(edited["key"]["title"], "work-laptop");
    assert_eq!(edited["key"]["comment"], "cli-fixture-renamed");
    assert_eq!(edited["key"]["entry_id"], generated["entry_id"]);
    assert_eq!(edited["key"]["fingerprint"], generated["fingerprint"]);
    assert_eq!(edited["key"]["notes"], generated["notes"]);

    let library = success(cli(
        directory.path(),
        &["library", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    let rows: Vec<&Value> = library["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == "login")
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|entry| entry["category"] == generated["collection_id"])
    );

    // The AI-facing credential surface only ever lists api tokens, so a key entry cannot be
    // disclosed, bound to a grant, or decrypted through it.
    let credentials = success(cli(directory.path(), &["list", "-j"], None));
    let text = serde_json::to_string(&credentials).unwrap();
    for marker in ["work-laptop", "signing", "SHA256:", "EDDSA"] {
        assert!(!text.contains(marker), "key entry in the credential list");
    }
}

#[test]
fn key_administration_is_documented_without_private_export() {
    let directory = tempfile::tempdir().unwrap();
    let root = success(cli(directory.path(), &["cmds", "-j"], None));
    let text = serde_json::to_string(&root).unwrap();
    for marker in ["keys", "ssh", "gpg", "generate", "private_key"] {
        assert!(
            text.contains(marker),
            "missing key grammar in discovery output: {marker}"
        );
    }
    assert!(
        !text.contains("keys export"),
        "private export leaked into discovery output"
    );
    let keys = success(cli(directory.path(), &["cmds", "keys", "-j"], None));
    let keys_text = serde_json::to_string(&keys).unwrap();
    assert!(keys_text.contains("\"ssh\""));
    assert!(!keys_text.contains("keys export"));
}

#[test]
fn a_delete_tombstones_only_what_a_person_confirmed_or_forced() {
    let directory = tempfile::tempdir().unwrap();
    let dir = directory.path();
    add(dir);
    let library = success(cli(
        dir,
        &["library", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    let entries = library["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "quick add stores exactly one credential");
    let bound = entries[0]["id"].as_str().unwrap().to_owned();
    let collection = entries[0]["category"].as_str().unwrap().to_owned();
    assert_eq!(
        success(cli(dir, &["status", "-j"], None))["grants"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A trusted producer may pipe a password; it may never pipe consent.
    failure(
        cli(
            dir,
            &["delete", &bound, "-j", "--secrets-stdin"],
            Some(&password()),
        ),
        "confirmation_required",
    );
    // A row a connection still binds is never taken down as a bare entry, and neither is a
    // category that still holds contents.
    failure(
        cli(
            dir,
            &["delete", &bound, "--force", "-j", "--secrets-stdin"],
            Some(&password()),
        ),
        "invalid_request",
    );
    failure(
        cli(
            dir,
            &[
                "delete-category",
                &collection,
                "--force",
                "-j",
                "--secrets-stdin",
            ],
            Some(&password()),
        ),
        "invalid_request",
    );
    let library = success(cli(
        dir,
        &["library", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert_eq!(library["entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        success(cli(dir, &["status", "-j"], None))["grants"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a refused delete must leave the grant in force"
    );

    let deleted = success(cli(
        dir,
        &["delete", "work", "--force", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert_eq!(deleted["kind"], "connection");
    assert_eq!(deleted["credential_removed"], true);
    assert_eq!(deleted["grants_revoked"], 1);
    assert_eq!(deleted["tombstone"], true);
    assert!(
        success(cli(dir, &["status", "-j"], None))["grants"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let library = success(cli(
        dir,
        &["library", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert!(library["entries"].as_array().unwrap().is_empty());
    // The same flags now succeed because the category really is empty.
    let category = success(cli(
        dir,
        &[
            "delete-category",
            &collection,
            "--force",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(category["kind"], "category");
    assert_eq!(category["tombstone"], true);
    let library = success(cli(
        dir,
        &["library", "-j", "--secrets-stdin"],
        Some(&password()),
    ));
    assert!(
        library["categories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != collection),
        "a tombstone must not read back as a live row"
    );
    failure(
        cli(
            dir,
            &["delete", "work", "--force", "-j", "--secrets-stdin"],
            Some(&password()),
        ),
        "not_found",
    );

    success(cli(
        dir,
        &[
            "keys",
            "ssh",
            "laptop",
            "--generate",
            "ed25519",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    failure(
        cli(
            dir,
            &["keys", "delete", "laptop", "-j", "--secrets-stdin"],
            Some(&password()),
        ),
        "confirmation_required",
    );
    let private_path = dir.join("id_ed25519");
    success(cli(
        dir,
        &[
            "keys",
            "export",
            "laptop",
            "--private",
            "-o",
            private_path.to_str().unwrap(),
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    let removed = success(cli(
        dir,
        &[
            "keys",
            "delete",
            "laptop",
            "--force",
            "-j",
            "--secrets-stdin",
        ],
        Some(&password()),
    ));
    assert_eq!(removed["name"], "laptop");
    assert_eq!(removed["kind"], "key");
    assert_eq!(removed["tombstone"], true);
    assert!(
        success(cli(
            dir,
            &["keys", "-j", "--secrets-stdin"],
            Some(&password())
        ))["keys"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    failure(
        cli(
            dir,
            &[
                "keys",
                "delete",
                "laptop",
                "--force",
                "-j",
                "--secrets-stdin",
            ],
            Some(&password()),
        ),
        "not_found",
    );
    assert!(
        private_path.is_file(),
        "text a person already exported is never retracted"
    );
}
