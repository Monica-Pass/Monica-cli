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
        &["add", "connect", "list", "show", "note", "token", "delete"],
    ),
    (
        "grants",
        Message::HelpGroupGrants,
        &[
            "grant", "settings", "check", "refresh", "revoke", "audit", "call", "mcp",
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
const STEPS: [&str; 5] = ["vault", "connection", "grant", "broker", "client"];

/// Reads configuration, grant and broker metadata only. It never opens the
/// vault, asks for a password or reads an AI client's own files, so the last
/// step is advice rather than a finding.
pub(super) fn next(store: &ConfigStore) -> Result<Value> {
    let status = match admin::status(store) {
        Ok(status) => Some(status),
        Err(GatewayError::SetupRequired) => None,
        Err(error) => return Err(error),
    };
    let names = |key: &str| -> Vec<String> {
        status
            .as_ref()
            .and_then(|status| status[key].as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let connections = names("connections");
    let grants: Vec<&Value> = status
        .as_ref()
        .and_then(|status| status["grants"].as_array())
        .map(|grants| grants.iter().collect())
        .unwrap_or_default();
    let usable = grants.iter().find(|grant| {
        grant["expired"] == Value::Bool(false) && grant["refresh_required"] == Value::Bool(false)
    });
    let running = status.as_ref().is_some_and(|status| {
        status["broker_running"] == Value::Bool(true)
            && status["lock_requested"] != Value::Bool(true)
    });
    let grant_name = |grant: &Value| grant["name"].as_str().unwrap_or("<grant>").to_owned();

    let (stage, commands, renew) = if status.is_none() {
        (
            "vault",
            vec!["add <name> --repo <owner/repo>".to_owned()],
            false,
        )
    } else if connections.is_empty() {
        (
            "connection",
            vec!["add <name> --repo <owner/repo>".to_owned()],
            false,
        )
    } else if let Some(grant) = usable {
        if running {
            let name = grant_name(grant);
            (
                "client",
                vec![
                    format!("settings {name} --install <claude|codex|cursor|vscode>"),
                    format!("check {name}"),
                ],
                false,
            )
        } else {
            ("broker", vec!["serve".to_owned()], false)
        }
    } else if let Some(grant) = grants.first() {
        (
            "grant",
            vec![format!("refresh {}", grant_name(grant))],
            true,
        )
    } else {
        (
            "grant",
            vec![format!(
                "grant <grant> -c {} -r <owner/repo>",
                connections[0]
            )],
            false,
        )
    };
    let reached = STEPS.iter().position(|step| *step == stage).unwrap_or(0);
    let steps: Vec<_> = STEPS
        .iter()
        .enumerate()
        .map(|(index, id)| json!({"id": id, "done": index < reached}))
        .collect();
    Ok(json!({
        "stage": stage,
        "renew": renew,
        "steps": steps,
        "commands": commands.iter().map(|command| format!("monica {command}")).collect::<Vec<_>>(),
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
        ("grant", true) => Message::NextWhyRenew,
        ("grant", false) => Message::NextWhyGrant,
        ("broker", _) => Message::NextWhyBroker,
        _ => Message::NextWhyClient,
    };
    out.push_str(&language.format(why, &[("bin", &bin)]));
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
