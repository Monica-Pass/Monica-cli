//! Writing one MCP entry into an AI client's own configuration file.
//!
//! These files belong to other programs, so a write is always a merge: the
//! entry for this grant is added or replaced and everything else in the file
//! survives untouched. A file that cannot be read back in the format its
//! client expects is refused before anything is written, and a file that is
//! about to change is copied aside first.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::config::{ensure_parent, private_file, read_json, write_json};
use crate::error::{GatewayError, Result};

/// A client this tool knows how to write into. Claude Desktop is deliberately
/// absent: its file sits in a per-platform application-support directory that
/// was never verified here, so it keeps the paste-the-JSON path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    #[value(name = "claude")]
    Claude,
    #[value(name = "cursor")]
    Cursor,
    #[value(name = "codex")]
    Codex,
    #[value(name = "vscode")]
    VsCode,
}

/// A client file of this shape is tens of KiB. Anything larger is a state dump
/// this command has no business rewriting, so it is refused rather than read.
const MAX_CLIENT_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// What one call to [`install`] did, so the caller can say where the entry went
/// and which copy to put back if the client turns out to dislike it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    /// False when the file already carried exactly this entry.
    pub changed: bool,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::VsCode => "vscode",
        }
    }

    /// Where this client keeps its server list, under a person's home directory.
    pub fn config_path(self, home: &Path) -> PathBuf {
        match self {
            Self::Claude => home.join(".claude.json"),
            Self::Cursor => home.join(".cursor").join("mcp.json"),
            Self::Codex => home.join(".codex").join("config.toml"),
            Self::VsCode => home.join(".vscode").join("mcp.json"),
        }
    }

    /// The JSON object the servers sit under. Codex keeps TOML, which has no key.
    fn servers_key(self) -> Option<&'static str> {
        match self {
            Self::Claude | Self::Cursor => Some("mcpServers"),
            Self::VsCode => Some("servers"),
            Self::Codex => None,
        }
    }
}

/// The home directory the client paths are resolved against, or none when the
/// environment does not name one. Nothing is guessed from the current
/// directory: writing into the wrong tree is worse than declining.
pub fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home = home.or_else(|| std::env::var_os("HOME"))?;
    (!home.is_empty()).then(|| PathBuf::from(home))
}

/// Add or replace exactly one server entry in `client`'s own file under `home`.
///
/// `entry` is the single-key object `{"<grant name>": {"command":…, "args":…}}`
/// that [`crate::admin::mcp_settings`] already prints, so what a person reads on
/// screen and what lands in the file cannot drift apart.
pub fn install(client: Client, home: &Path, entry: &Value) -> Result<Outcome> {
    let (name, body) = single_entry(entry)?;
    match client.servers_key() {
        Some(key) => merge_json(&client.config_path(home), key, &name, body),
        None => merge_toml(&client.config_path(home), &name, &body),
    }
}

/// The one entry this command is allowed to write, validated so a client never
/// receives half an MCP definition.
fn single_entry(entry: &Value) -> Result<(String, Value)> {
    let (name, body) = entry
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.iter().next())
        .ok_or(GatewayError::InvalidConfig)?;
    let body = body
        .as_object()
        .filter(|body| {
            body.get("command").is_some_and(Value::is_string)
                && body
                    .get("args")
                    .and_then(Value::as_array)
                    .is_some_and(|args| args.iter().all(Value::is_string))
        })
        .ok_or(GatewayError::InvalidConfig)?;
    Ok((name.clone(), Value::Object(body.clone())))
}

fn merge_json(path: &Path, key: &str, name: &str, body: Value) -> Result<Outcome> {
    let decoded = read_json::<Box<serde_json::value::RawValue>>(path, MAX_CLIENT_FILE_BYTES)
        .and_then(|raw| {
            mdbx_core::json::from_str(raw.get()).map_err(|_| GatewayError::InvalidConfig)
        });
    let mut root = match decoded {
        Ok(root) if root.is_object() => root,
        // A file that is not there yet is the easy case: it starts empty.
        Err(GatewayError::StateUnavailable) if !path.exists() => json!({}),
        Ok(_) | Err(_) => return Err(GatewayError::ClientConfigUnusable),
    };
    let before = root.clone();
    let object = root
        .as_object_mut()
        .ok_or(GatewayError::ClientConfigUnusable)?;
    match object.get_mut(key) {
        Some(Value::Object(servers)) => match servers.get_mut(name) {
            // Only the fields this command generates are replaced, so a block a
            // person added to the entry by hand, such as `env`, survives a second
            // install. Codex behaves the same way: its `env` is its own table.
            Some(Value::Object(existing)) => {
                if let Some(fields) = body.as_object() {
                    for (field, value) in fields {
                        existing.insert(field.clone(), value.clone());
                    }
                }
            }
            _ => {
                servers.insert(name.to_owned(), body);
            }
        },
        Some(_) => return Err(GatewayError::ClientConfigUnusable),
        None => {
            object.insert(key.to_owned(), json!({ name: body }));
        }
    }
    if root == before {
        return Ok(Outcome {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }
    let backup = back_up(path)?;
    write_json(path, &root, true)?;
    Ok(Outcome {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

/// This grant's server in Codex's `config.toml`, updated in place or added.
///
/// The file is parsed rather than scanned line by line, because TOML spells one
/// server many ways: `[mcp_servers.work]`, `[mcp_servers.'work']`,
/// `[ mcp_servers.work ]`, `work = { … }` under `[mcp_servers]`, or dotted keys.
/// A scan that misses one of them appends a second definition, and Codex then
/// cannot load the file at all. The parser keeps comments and layout, only
/// `command` and `args` of this entry change, and sub-tables that belong to the
/// entry, such as its `env`, stay where they are.
fn merge_toml(path: &Path, name: &str, body: &Value) -> Result<Outcome> {
    let command = body
        .get("command")
        .and_then(Value::as_str)
        .ok_or(GatewayError::InvalidConfig)?;
    let args = body
        .get("args")
        .and_then(Value::as_array)
        .ok_or(GatewayError::InvalidConfig)?
        .iter()
        .map(|arg| arg.as_str().ok_or(GatewayError::InvalidConfig))
        .collect::<Result<Vec<_>>>()?;
    let existing = match read_text(path) {
        Ok(text) => Some(text),
        Err(GatewayError::StateUnavailable) if !path.exists() => None,
        Err(_) => return Err(GatewayError::ClientConfigUnusable),
    };
    let mut document = match existing.as_deref() {
        // A table or key defined twice does not parse, so a file that already
        // defines one server twice is refused here, before anything is written.
        Some(text) => text
            .parse::<DocumentMut>()
            .map_err(|_| GatewayError::ClientConfigUnusable)?,
        None => DocumentMut::new(),
    };
    set_server(&mut document, name, command, &args)?;
    let merged = document.to_string();
    if Some(merged.as_str()) == existing.as_deref() {
        return Ok(Outcome {
            path: path.to_path_buf(),
            backup: None,
            changed: false,
        });
    }
    // Whatever is written has to be a file Codex can load again.
    merged
        .parse::<DocumentMut>()
        .map_err(|_| GatewayError::ClientConfigUnusable)?;
    let backup = back_up(path)?;
    if existing.is_none() {
        ensure_parent(path)?;
    }
    write_text(path, merged.as_bytes())?;
    Ok(Outcome {
        path: path.to_path_buf(),
        backup,
        changed: true,
    })
}

fn set_server(document: &mut DocumentMut, name: &str, command: &str, args: &[&str]) -> Result<()> {
    let root = document.as_table_mut();
    if !root.contains_key("mcp_servers") {
        let mut servers = Table::new();
        // No bare `[mcp_servers]` header: the entry's own header names the path.
        servers.set_implicit(true);
        root.insert("mcp_servers", Item::Table(servers));
    }
    // `[[mcp_servers]]` or a scalar is not a server list this can add to, and
    // picking one element of an array would silently lose the others.
    let servers = root
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .ok_or(GatewayError::ClientConfigUnusable)?;
    match servers.get_mut(name) {
        Some(Item::Table(entry)) => {
            // Only `[mcp_servers.work.env]` was there, so the entry itself was
            // implicit; now it carries values and needs its own header.
            entry.set_implicit(false);
        }
        Some(item) if item.is_table_like() => {}
        // `[[mcp_servers.work]]` defines this server more than once.
        Some(Item::ArrayOfTables(_)) => return Err(GatewayError::ClientConfigUnusable),
        // A placeholder value under our own name is replaced, as in the JSON clients.
        Some(_) | None => {
            servers.insert(name, Item::Table(Table::new()));
        }
    }
    let entry = servers
        .get_mut(name)
        .and_then(Item::as_table_like_mut)
        .ok_or(GatewayError::ClientConfigUnusable)?;
    // Unchanged fields are left exactly as written, so a second run of this
    // command leaves the file byte-identical.
    if entry.get("command").and_then(Item::as_str) != Some(command) {
        entry.insert("command", value(command));
    }
    let current = entry
        .get("args")
        .and_then(Item::as_array)
        .and_then(|array| {
            array
                .iter()
                .map(|arg| arg.as_str())
                .collect::<Option<Vec<_>>>()
        });
    if current.as_deref() != Some(args) {
        entry.insert("args", value(args.iter().copied().collect::<Array>()));
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<String> {
    if fs::metadata(path)
        .map_err(|_| GatewayError::StateUnavailable)?
        .len()
        > MAX_CLIENT_FILE_BYTES
    {
        return Err(GatewayError::ClientConfigUnusable);
    }
    // Non-UTF-8 is not a text file this command may rewrite.
    fs::read_to_string(path).map_err(|_| GatewayError::ClientConfigUnusable)
}

/// Write `bytes` through a private temporary file in the target's own directory.
///
/// The temporary is owner-only because a client file can carry another client's
/// credentials in its `env` blocks, which means the merged result lands with the
/// same restriction. A person who wants their previous permissions back can copy
/// the backup file over it.
fn write_text(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure_parent(path)?;
    let parent = path.parent().ok_or(GatewayError::StateUnavailable)?;
    let temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| GatewayError::StateUnavailable)?;
    private_file(temp.path())?;
    fs::write(temp.path(), bytes).map_err(|_| GatewayError::StateUnavailable)?;
    temp.persist(path)
        .map_err(|_| GatewayError::StateUnavailable)?;
    Ok(())
}

/// Copy the client's file aside before it changes, so a person who finds their
/// client unhappy can move the old file back by hand.
fn back_up(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let name = path
        .file_name()
        .ok_or(GatewayError::StateUnavailable)?
        .to_os_string();
    for attempt in 0..16u32 {
        let mut candidate = name.clone();
        candidate.push(OsString::from(format!(".monica-{stamp}-{attempt}")));
        let target = path.with_file_name(candidate);
        if target.exists() {
            // Two runs in the same second must not overwrite the first backup.
            continue;
        }
        fs::copy(path, &target).map_err(|_| GatewayError::StateUnavailable)?;
        private_file(&target)?;
        return Ok(Some(target));
    }
    Err(GatewayError::StateUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, client_file: &str) -> Value {
        json!({
            name: {"command": "/opt/monica/monica-pass", "args": ["mcp", "--client", client_file]}
        })
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn merging_leaves_every_other_part_of_the_client_file_alone() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Claude.config_path(home.path());
        let original = serde_json::to_vec_pretty(&json!({
            "theme": "dark",
            "numStartups": 41,
            "literal": {"$serde_json::private::Number": "123"},
            "raw": {"$serde_json::private::RawValue": "null"},
            "mcpServers": {"other": {"command": "other-server", "args": []}},
            "projects": {"/srv/app": {"allowedTools": ["Bash"]}}
        }))
        .unwrap();
        fs::write(&path, &original).unwrap();

        let outcome = install(
            Client::Claude,
            home.path(),
            &entry("work", "clients/work.client.mcp.json"),
        )
        .unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.path, path);
        let backup = outcome.backup.expect("a changed file is copied aside");
        assert_eq!(read(&backup), String::from_utf8(original).unwrap());
        assert!(
            backup
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".claude.json.monica-")
        );
        let root: Value = mdbx_core::json::from_str(&read(&path)).unwrap();
        assert_eq!(root["literal"]["$serde_json::private::Number"], "123");
        assert_eq!(root["raw"]["$serde_json::private::RawValue"], "null");
        assert_eq!(root["theme"], "dark");
        assert_eq!(root["numStartups"], 41);
        assert_eq!(root["projects"]["/srv/app"]["allowedTools"][0], "Bash");
        assert_eq!(root["mcpServers"]["other"]["command"], "other-server");
        assert_eq!(
            root["mcpServers"]["work"]["args"],
            json!(["mcp", "--client", "clients/work.client.mcp.json"])
        );
    }

    #[test]
    fn a_hand_added_block_on_the_entry_survives_the_next_install() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::VsCode.config_path(home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "servers": {
                    "work": {
                        "command": "/old/monica-pass",
                        "args": ["mcp"],
                        "env": {"RUST_LOG": "info"},
                        "disabled": false
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        install(Client::VsCode, home.path(), &entry("work", "w.json")).unwrap();

        let root: Value = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(root["servers"]["work"]["env"]["RUST_LOG"], "info");
        assert_eq!(root["servers"]["work"]["disabled"], false);
        assert_eq!(
            root["servers"]["work"]["command"],
            "/opt/monica/monica-pass"
        );
        assert_eq!(root["servers"]["work"]["args"][2], "w.json");
    }

    #[test]
    fn a_placeholder_under_our_own_name_is_replaced() {
        // A stub is not a server definition, so this name is ours to overwrite;
        // everything else in the file still has to survive.
        let home = tempfile::tempdir().unwrap();
        let path = Client::Claude.config_path(home.path());
        fs::write(
            &path,
            serde_json::to_vec(&json!({"mcpServers": {"work": "off"}})).unwrap(),
        )
        .unwrap();

        install(Client::Claude, home.path(), &entry("work", "w.json")).unwrap();

        let root: Value = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(
            root["mcpServers"]["work"]["command"],
            "/opt/monica/monica-pass"
        );
    }

    #[test]
    fn a_missing_client_file_is_created_with_its_directory() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Cursor.config_path(home.path());
        assert!(!path.exists());

        let outcome = install(Client::Cursor, home.path(), &entry("work", "w.json")).unwrap();

        assert!(outcome.changed);
        assert_eq!(outcome.backup, None);
        let root: Value = serde_json::from_str(&read(&path)).unwrap();
        assert_eq!(
            root["mcpServers"]["work"]["command"],
            "/opt/monica/monica-pass"
        );
    }

    #[test]
    fn installing_the_same_entry_twice_leaves_the_file_untouched() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Claude.config_path(home.path());
        let first = install(Client::Claude, home.path(), &entry("work", "w.json")).unwrap();
        assert!(first.changed);
        assert_eq!(first.backup, None);
        let first = read(&path);

        let outcome = install(Client::Claude, home.path(), &entry("work", "w.json")).unwrap();

        assert!(!outcome.changed);
        assert_eq!(outcome.backup, None);
        assert_eq!(read(&path), first);
    }

    #[test]
    fn a_client_file_this_tool_cannot_read_back_is_refused_untouched() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not JSON at all", b"oops {".to_vec()),
            ("a JSON array, not an object", b"[1, 2, 3]".to_vec()),
            (
                "a servers key of the wrong shape",
                serde_json::to_vec(&json!({"mcpServers": "work"})).unwrap(),
            ),
        ];
        for (label, bytes) in cases {
            let home = tempfile::tempdir().unwrap();
            let path = Client::Claude.config_path(home.path());
            fs::write(&path, &bytes).unwrap();

            let error = install(Client::Claude, home.path(), &entry("work", "w.json"))
                .err()
                .unwrap_or_else(|| panic!("accepted a client file that is {label}"));

            assert_eq!(error, GatewayError::ClientConfigUnusable);
            assert_eq!(fs::read(&path).unwrap(), bytes, "{label} was rewritten");
            let strays: Vec<_> = fs::read_dir(home.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry.path() != path)
                .collect();
            assert!(strays.is_empty(), "{label} left files behind: {strays:?}");
        }
    }

    #[test]
    fn an_entry_that_is_not_one_complete_server_is_refused() {
        let home = tempfile::tempdir().unwrap();
        for bad in [
            json!({}),
            json!({"work": {"command": "/x"}, "other": {"command": "/y"}}),
            json!({"work": {"args": ["mcp"]}}),
            json!({"work": {"command": "/x", "args": [123]}}),
            json!({"work": "not an object"}),
        ] {
            assert_eq!(
                install(Client::Claude, home.path(), &bad).err(),
                Some(GatewayError::InvalidConfig),
                "accepted {bad}"
            );
        }
        assert!(!Client::Claude.config_path(home.path()).exists());
    }

    #[test]
    fn toml_replace_keeps_the_env_table_and_unrelated_sections() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Codex.config_path(home.path());
        let original = "\
model = \"gpt-5\"

[mcp_servers.work]
command = \"/old/monica-pass\"
args = [\"mcp\"]

[mcp_servers.work.env]
RUST_LOG = \"info\"

[mcp_servers.other]
command = \"other\"

[profile]
mode = \"default\"
";
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, original).unwrap();

        install(Client::Codex, home.path(), &entry("work", "w.json")).unwrap();

        let merged = read(&path);
        assert!(merged.contains("model = \"gpt-5\""));
        assert!(merged.contains("command = \"/opt/monica/monica-pass\""));
        assert!(merged.contains("args = [\"mcp\", \"--client\", \"w.json\"]"));
        assert!(!merged.contains("/old/monica-pass"));
        assert_eq!(
            merged.matches("[mcp_servers.work]").count(),
            1,
            "the block was duplicated: {merged}"
        );
        // The entry's own sub-table stays where it was, right after it.
        assert!(
            merged.contains("[mcp_servers.work]\ncommand = \"/opt/monica/monica-pass\"\nargs = [\"mcp\", \"--client\", \"w.json\"]\n\n[mcp_servers.work.env]\nRUST_LOG = \"info\"\n")
        );
        assert!(merged.contains("[mcp_servers.other]\ncommand = \"other\"\n"));
        assert!(merged.contains("[profile]\nmode = \"default\"\n"));
    }

    #[test]
    fn toml_append_and_reinstall_are_byte_identical() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Codex.config_path(home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "model = \"gpt-5\"\n").unwrap();

        let first = entry("work", "w.json");
        install(Client::Codex, home.path(), &first).unwrap();
        let after_first = read(&path);
        assert!(after_first.starts_with("model = \"gpt-5\"\n\n[mcp_servers.work]\n"));
        assert!(after_first.ends_with("w.json\"]\n"));

        let outcome = install(Client::Codex, home.path(), &first).unwrap();
        assert!(!outcome.changed);
        assert_eq!(read(&path), after_first);
    }

    #[test]
    fn toml_refuses_shapes_where_one_server_could_be_defined_twice() {
        for text in [
            "[[mcp_servers]]\ncommand = \"other\"\n",
            "[[mcp_servers.work]]\ncommand = \"other\"\n",
            "[mcp_servers.work]\ncommand = \"first\"\n\n[mcp_servers.work]\ncommand = \"second\"\n",
            "[mcp_servers.\"work\"]\ncommand = \"first\"\n\n[mcp_servers.work]\ncommand = \"second\"\n",
            "[mcp_servers.'work']\ncommand = \"first\"\n\n[mcp_servers.work]\ncommand = \"second\"\n",
            "[mcp_servers]\nwork = { command = \"first\" }\n\n[mcp_servers.work]\ncommand = \"second\"\n",
            "mcp_servers = \"none\"\n",
            "model = \"unterminated\n",
        ] {
            let home = tempfile::tempdir().unwrap();
            let path = Client::Codex.config_path(home.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();

            let error = install(Client::Codex, home.path(), &entry("work", "w.json"))
                .err()
                .unwrap_or_else(|| panic!("accepted {text:?}"));
            assert_eq!(error, GatewayError::ClientConfigUnusable);
            assert_eq!(read(&path), text);
        }
    }

    #[test]
    fn a_toml_key_that_needs_quoting_is_still_found_next_time() {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Codex.config_path(home.path());
        let one = entry("work laptop", "w.json");

        install(Client::Codex, home.path(), &one).unwrap();
        let after_first = read(&path);
        assert!(after_first.contains("[mcp_servers.\"work laptop\"]\n"));

        let outcome = install(Client::Codex, home.path(), &one).unwrap();
        assert!(!outcome.changed, "{after_first}");
        assert_eq!(read(&path), after_first);
        assert_eq!(after_first.matches("work laptop").count(), 1);
    }

    fn codex_file(text: &str) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let path = Client::Codex.config_path(home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        (home, path)
    }

    /// What Codex would read back: the one server's command and args.
    fn codex_server(text: &str, name: &str) -> (String, Vec<String>) {
        let document = text.parse::<DocumentMut>().unwrap_or_else(|error| {
            panic!("the merged file is not TOML Codex can load: {error}\n{text}")
        });
        let server = document["mcp_servers"][name].as_table_like().unwrap();
        let command = server.get("command").and_then(Item::as_str).unwrap();
        let args = server
            .get("args")
            .and_then(Item::as_array)
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect();
        (command.to_owned(), args)
    }

    #[test]
    fn every_toml_spelling_of_the_server_is_updated_instead_of_duplicated() {
        for (label, text) in [
            (
                "a plain header",
                "[mcp_servers.work]\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "a header with a trailing comment",
                "[mcp_servers.work] # Monica\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "an indented header",
                "  [mcp_servers.work]\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "spaces inside the brackets",
                "[ mcp_servers.work ]\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "a basic-quoted key",
                "[mcp_servers.\"work\"]\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "a literal-quoted key",
                "[mcp_servers.'work']\ncommand = \"old\"\nargs = []\n",
            ),
            (
                "an inline table",
                "[mcp_servers]\nwork = { command = \"old\", args = [] }\n",
            ),
            (
                "dotted keys",
                "mcp_servers.work.command = \"old\"\nmcp_servers.work.args = []\n",
            ),
            ("a placeholder value", "[mcp_servers]\nwork = \"off\"\n"),
        ] {
            let original = format!("model = \"gpt-5\"\n\n{text}");
            let (home, path) = codex_file(&original);

            let outcome = install(Client::Codex, home.path(), &entry("work", "w.json"))
                .unwrap_or_else(|error| panic!("{label}: {error:?}"));

            assert!(outcome.changed, "{label}");
            let merged = read(&path);
            assert_eq!(
                codex_server(&merged, "work"),
                (
                    "/opt/monica/monica-pass".to_owned(),
                    vec!["mcp".to_owned(), "--client".to_owned(), "w.json".to_owned()]
                ),
                "{label}: {merged}"
            );
            assert!(!merged.contains("\"old\""), "{label}: {merged}");
            assert!(
                merged.starts_with("model = \"gpt-5\"\n"),
                "{label}: {merged}"
            );
            if label == "a header with a trailing comment" {
                assert!(merged.contains("# Monica"), "{merged}");
            }

            let again = install(Client::Codex, home.path(), &entry("work", "w.json")).unwrap();
            assert!(!again.changed, "{label}: a second run rewrote {merged}");
            assert_eq!(read(&path), merged, "{label}");
        }
    }

    #[test]
    fn an_entry_known_only_through_its_env_table_gets_its_own_header() {
        let (home, path) = codex_file("[mcp_servers.work.env]\nRUST_LOG = \"info\"\n");

        install(Client::Codex, home.path(), &entry("work", "w.json")).unwrap();

        let merged = read(&path);
        assert_eq!(codex_server(&merged, "work").0, "/opt/monica/monica-pass");
        let document = merged.parse::<DocumentMut>().unwrap();
        assert_eq!(
            document["mcp_servers"]["work"]["env"]["RUST_LOG"].as_str(),
            Some("info")
        );
    }

    #[test]
    fn a_windows_command_path_reads_back_exactly() {
        // The real command is the binary's own path, which on Windows is full of
        // backslashes and may hold a space or an apostrophe. Whatever quoting the
        // writer picks, Codex has to read back the exact same path.
        for directory in [
            r"C:\Users\someone\Apps\Monica CLI",
            r"C:\Users\O'Brien\Apps\Monica CLI",
        ] {
            let home = tempfile::tempdir().unwrap();
            let path = Client::Codex.config_path(home.path());
            let command = format!(r"{directory}\monica-pass.exe");
            let client = format!(r"{directory}\data\clients\work.client.json");
            let one = json!({"work": {"command": command, "args": ["mcp", "--client", client]}});

            install(Client::Codex, home.path(), &one).unwrap();

            let merged = read(&path);
            assert_eq!(
                codex_server(&merged, "work"),
                (
                    command.clone(),
                    vec!["mcp".to_owned(), "--client".to_owned(), client.clone()]
                ),
                "{merged}"
            );
            assert!(!install(Client::Codex, home.path(), &one).unwrap().changed);
        }
    }

    #[test]
    fn each_client_writes_the_key_its_own_app_reads() {
        let home = tempfile::tempdir().unwrap();
        for client in [
            Client::Claude,
            Client::Cursor,
            Client::VsCode,
            Client::Codex,
        ] {
            install(client, home.path(), &entry(client.name(), "w.json")).unwrap();
        }
        let claude: Value =
            serde_json::from_str(&read(&Client::Claude.config_path(home.path()))).unwrap();
        assert!(claude["mcpServers"].get("claude").is_some());
        let vscode: Value =
            serde_json::from_str(&read(&Client::VsCode.config_path(home.path()))).unwrap();
        assert!(vscode["servers"].get("vscode").is_some());
        let codex = read(&Client::Codex.config_path(home.path()));
        assert!(codex.contains("[mcp_servers.codex]"), "{codex}");
    }
}
