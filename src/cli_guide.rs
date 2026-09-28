//! The two places the command line teaches itself: the grouped root `--help`
//! and `monica next`, which reads only public metadata to say which setup step
//! is still missing.
use std::fmt::Write as _;

use clap::Command;
use monica_pass_cli::admin;
use monica_pass_cli::config::ConfigStore;
use monica_pass_cli::error::{GatewayError, Result};
use monica_pass_cli::i18n::{Language, Message};
use monica_pass_cli::tr;
use serde_json::{Value, json};
use unicode_width::UnicodeWidthStr;

/// Where the practice book lives; `--help` and `next` both point at it.
pub(super) const PRACTICE_URL: &str = "https://monica-pass.github.io/Monica-cli/reference/";

/// The same seven groups, in the same order, as the practice book
/// (`docs/reference/examples.json`); a test keeps the two in step and requires
/// every visible command to sit in exactly one of them.
pub(super) const GROUPS: &[(&str, Message, &[&str])] = &[
    (
        "start",
        Message::HelpGroupStart,
        &[
            "next",
            "init",
            "status",
            "serve",
            "lock",
            "databases",
            "use",
            "open",
            "tui",
            "language",
            "commands",
            "help",
        ],
    ),
    ("mdbx", Message::HelpGroupMdbx, &["mdbx"]),
    ("vault", Message::HelpGroupVault, &["tiga"]),
    (
        "connections",
        Message::HelpGroupConnections,
        &[
            "add",
            "connect",
            "connections",
            "show",
            "note",
            "token",
            "delete",
        ],
    ),
    (
        "grants",
        Message::HelpGroupGrants,
        &[
            "grant",
            "mcp-config",
            "check",
            "renew",
            "revoke",
            "audit",
            "call",
            "mcp",
        ],
    ),
    (
        "content",
        Message::HelpGroupContent,
        &[
            "library",
            "category",
            "rename-category",
            "rename-entry",
            "move",
            "delete-category",
        ],
    ),
    ("sync", Message::HelpGroupSync, &["webdav"]),
];

/// The name this binary was started under, so a person who only has
/// `monica-pass` is never told to type `monica`. JSON always says `monica`.
pub(super) fn program() -> String {
    std::env::args_os()
        .next()
        .as_deref()
        .map(std::path::Path::new)
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .filter(|stem| stem.to_ascii_lowercase().starts_with("monica"))
        .unwrap_or("monica")
        .to_owned()
}

/// The root help body between the usage line and the options: a three-step
/// quick start, then every visible command under its group heading.
pub(super) fn root_help(command: &Command, language: Language) -> String {
    let bin = program();
    let mut out = tr!(language, HelpQuickStart, bin = bin, url = PRACTICE_URL);
    out.push_str("\n\n");
    let rows: Vec<_> = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .collect();
    let column = rows
        .iter()
        .map(|child| child.get_name().width())
        .max()
        .unwrap_or(0);
    for (_, heading, names) in GROUPS {
        let _ = writeln!(out, "{}:", language.text(*heading));
        for name in *names {
            let Some(child) = rows.iter().find(|child| child.get_name() == *name) else {
                continue;
            };
            let about = child
                .get_about()
                .map(ToString::to_string)
                .unwrap_or_default();
            let aliases: Vec<_> = child.get_visible_aliases().collect();
            let aliases = match aliases.len() {
                0 => String::new(),
                1 => format!(" [alias: {}]", aliases[0]),
                _ => format!(" [aliases: {}]", aliases.join(", ")),
            };
            let pad = " ".repeat(column - name.width());
            let _ = writeln!(out, "  {name}{pad}  {about}{aliases}");
        }
        out.push('\n');
    }
    out
}

/// One setup step, in the order a first run meets them.
const STEPS: [&str; 5] = ["vault", "connection", "grant", "client", "broker"];

/// Reads public metadata only. Client integration remains unverified: no AI
/// configuration files are opened and no state is created by this guide.
pub(super) fn next(store: &ConfigStore, selected: Option<&str>) -> Result<Value> {
    if let Some(name) = selected {
        monica_pass_cli::model::validate_name(name)?;
    }
    let status = match admin::status(store) {
        Ok(status) => Some(status),
        Err(GatewayError::SetupRequired) => None,
        Err(error) => return Err(error),
    };
    next_from_status(status.as_ref(), selected)
}

fn next_from_status(status: Option<&Value>, selected: Option<&str>) -> Result<Value> {
    let items = |key: &str| -> Vec<&Value> {
        status
            .and_then(|s| s[key].as_array())
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    };
    let connections = items("connections");
    let grants = items("grants");
    let chosen = if let Some(selected) = selected {
        Some(
            *grants
                .iter()
                .find(|g| g["name"] == selected)
                .ok_or(GatewayError::NotFound)?,
        )
    } else if grants.len() == 1 {
        Some(grants[0])
    } else {
        None
    };
    let usable = chosen.is_some_and(|g| g["expired"] == false && g["refresh_required"] == false);
    let running =
        status.is_some_and(|s| s["broker_running"] == true && s["lock_requested"] != true);
    let name = chosen.and_then(|g| g["name"].as_str()).unwrap_or("<GRANT>");
    let (stage, commands, renew) = if status.is_none() {
        (
            "vault",
            vec!["add <CONNECTION> --repo <owner/repo>".to_owned()],
            false,
        )
    } else if connections.is_empty() {
        (
            "connection",
            vec!["add <CONNECTION> --repo <owner/repo>".to_owned()],
            false,
        )
    } else if chosen.is_none() && grants.len() > 1 {
        (
            "grant_selection",
            vec!["status".to_owned(), "next --grant <GRANT>".to_owned()],
            false,
        )
    } else if usable {
        let mut commands = vec![format!(
            "mcp-config {name} --install <claude|codex|cursor|vscode>"
        )];
        if !running {
            commands.push("serve".to_owned());
        }
        commands.push(format!("check {name}"));
        ("client", commands, false)
    } else if chosen.is_some() {
        ("grant", vec![format!("renew {name}")], true)
    } else {
        let connection = if connections.len() == 1 {
            connections[0]["name"].as_str().unwrap_or("<CONNECTION>")
        } else {
            "<CONNECTION>"
        };
        (
            "grant",
            vec![format!(
                "grant <GRANT> --connection {connection} --repo <owner/repo>"
            )],
            false,
        )
    };
    let steps: Vec<_> = STEPS
        .iter()
        .map(|id| {
            let done = match *id {
                "vault" => status.is_some(),
                "connection" => !connections.is_empty(),
                "grant" => usable,
                "broker" => running,
                _ => false,
            };
            let state = if *id == "client" {
                "unverified"
            } else if done {
                "ready"
            } else {
                "missing"
            };
            json!({"id":id,"done":done,"state":state})
        })
        .collect();
    let commands: Vec<_> = commands.iter().map(|c| format!("monica {c}")).collect();
    let actions: Vec<_> = commands.iter().map(|command| json!({
        "command": command,
        "terminal": if command.contains(" check ") {"another"} else {"current"},
        "condition": if command.contains(" mcp-config ") {"if_client_not_configured"} else {"next_step"},
    })).collect();
    Ok(json!({
        "stage":stage,"renew":renew,"steps":steps,"commands":commands,"actions":actions,
        "selected_grant":chosen.map(|g| &g["name"]),
        "available_grants":grants.iter().map(|g| &g["name"]).collect::<Vec<_>>(),
        "client_integration":"unverified",
        "broker_running":running,
    }))
}

pub(super) fn render_next(data: &Value, language: Language) -> String {
    let bin = program();
    let mut out = String::new();
    let stage = data["stage"].as_str().unwrap_or_default();
    let mut line = format!("{} ", language.text(Message::NextProgress));
    for step in data["steps"].as_array().into_iter().flatten() {
        let id = step["id"].as_str().unwrap_or_default();
        let mark = if step["done"] == Value::Bool(true) {
            "✓"
        } else if id == stage {
            "→"
        } else {
            "·"
        };
        let label = match id {
            "vault" => Message::NextStepVault,
            "connection" => Message::NextStepConnection,
            "grant" => Message::NextStepGrant,
            "broker" => Message::NextStepBroker,
            _ => Message::NextStepClient,
        };
        let _ = write!(line, " {mark} {}", language.text(label));
    }
    out.push_str(&line);
    out.push_str("\n\n");
    let why = match (stage, data["renew"] == Value::Bool(true)) {
        ("vault", _) => Message::NextWhyVault,
        ("connection", _) => Message::NextWhyConnection,
        ("grant_selection", _) => Message::NextChooseGrant,
        ("grant", true) => Message::NextWhyRenew,
        ("grant", false) => Message::NextWhyGrant,
        ("broker", _) => Message::NextWhyBroker,
        _ => Message::NextWhyClient,
    };
    out.push_str(&language.format(why, &[("bin", &bin)]));
    if stage == "client" {
        out.push('\n');
        out.push_str(language.text(Message::NextTerminalOrder));
    }
    out.push('\n');
    for command in data["commands"].as_array().into_iter().flatten() {
        let command = command.as_str().unwrap_or_default();
        let command = command.strip_prefix("monica ").unwrap_or(command);
        let _ = writeln!(out, "  {bin} {command}");
    }
    out.push('\n');
    out.push_str(&tr!(language, NextPractice, url = PRACTICE_URL));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_readiness_does_not_claim_client_integration_and_multiple_grants_need_selection() {
        let mut status = json!({"connections":[{"name":"work-github"}],"grants":[
            {"name":"reader", "expired":false, "refresh_required":false},
            {"name":"writer", "expired":false, "refresh_required":false}
        ],"broker_running":true,"lock_requested":false});
        let choice = next_from_status(Some(&status), None).unwrap();
        assert_eq!(choice["stage"], "grant_selection");
        assert_eq!(choice["selected_grant"], Value::Null);
        assert_eq!(
            next_from_status(Some(&status), Some("missing")),
            Err(GatewayError::NotFound)
        );
        let selected = next_from_status(Some(&status), Some("writer")).unwrap();
        assert_eq!(selected["selected_grant"], "writer");
        assert_eq!(
            selected["commands"],
            json!([
                "monica mcp-config writer --install <claude|codex|cursor|vscode>",
                "monica check writer"
            ])
        );
        assert_eq!(
            selected["steps"][3],
            json!({"id":"client","done":false,"state":"unverified"})
        );
        assert_eq!(selected["steps"][4]["done"], true);
        status["lock_requested"] = true.into();
        let stopped = next_from_status(Some(&status), Some("reader")).unwrap();
        assert_eq!(stopped["commands"][1], "monica serve");
        assert_eq!(stopped["actions"][2]["terminal"], "another");
        assert_eq!(stopped["steps"][4]["done"], false);
        status["grants"][0]["refresh_required"] = true.into();
        let expired = next_from_status(Some(&status), Some("reader")).unwrap();
        assert_eq!(expired["commands"], json!(["monica renew reader"]));
        assert_eq!(expired["steps"][2]["done"], false);
    }
}
