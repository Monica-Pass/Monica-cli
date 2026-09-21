//! Build documentation from the parser used to execute commands. The JSON form
//! is locale-independent and contains public parameter metadata only.
use clap::{Command, CommandFactory};
use monica_pass_cli::error::{GatewayError, Result};
use monica_pass_cli::i18n::Language;
use serde_json::{Value, json};

use crate::cli::Cli;
use crate::cli_input::{MAX_SECRET_BYTES, required_fields};
use crate::cli_output::Output;

pub fn run(topic: &[String], language: Language, output: Output) -> Result<()> {
    let mut command = Cli::command();
    command.build();
    command =
        crate::cli_language::localize(command, if output.json { Language::En } else { language });
    let mut path = Vec::new();
    for name in topic {
        let child = command
            .get_subcommands()
            .filter(|child| discoverable(child, &path))
            .find(|child| {
                child.get_name() == name || child.get_all_aliases().any(|alias| alias == name)
            })
            .cloned()
            .ok_or(GatewayError::InvalidRequest)?;
        command = child;
        path.push(command.get_name().to_owned());
    }
    if output.json {
        let data = describe(&command, &path);
        output.result("commands", data, None)
    } else {
        command
            .print_long_help()
            .map_err(|_| GatewayError::StateUnavailable)?;
        println!();
        Ok(())
    }
}

fn describe(command: &Command, path: &[String]) -> Value {
    let name = path.join(" ");
    let arguments: Vec<_> = command.get_arguments().filter(|arg| !arg.is_hide_set()).map(|arg| {
        let values: Vec<_> = arg.get_possible_values().into_iter()
            .filter(|value| !value.is_hide_set()).map(|value| value.get_name().to_owned()).collect();
        let defaults: Vec<_> = arg.get_default_values().iter()
            .map(|value| value.to_string_lossy().into_owned()).collect();
        json!({
            "id": arg.get_id().as_str(),
            "long": arg.get_long(),
            "short": arg.get_short().map(|short| short.to_string()),
            "aliases": arg.get_visible_aliases().unwrap_or_default(),
            "positional": arg.is_positional(),
            "required": arg.is_required_set(),
            "global": arg.is_global_set(),
            "takes_value": arg.get_action().takes_values(),
            "repeatable": matches!(arg.get_action(), clap::ArgAction::Append | clap::ArgAction::Count),
            "value_names": arg.get_value_names().unwrap_or_default().iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            "choices": values,
            "defaults": defaults,
            "help": arg.get_help().map(ToString::to_string),
            "conflicts_with": command.get_arg_conflicts_with(arg).iter().map(|other| other.get_id().as_str()).collect::<Vec<_>>(),
        })
    }).collect();
    let groups: Vec<_> = command.get_groups().filter(|group| group.is_required_set()).map(|group| {
        json!({"required_one_of": group.get_args().map(|id| id.as_str()).collect::<Vec<_>>()})
    }).collect();
    let children: Vec<_> = command
        .get_subcommands()
        .filter(|child| child.get_name() != "help" && discoverable(child, path))
        .map(|child| {
            let mut path = path.to_vec();
            path.push(child.get_name().to_owned());
            describe(child, &path)
        })
        .collect();
    let secret_fields: Vec<_> = required_fields(&name)
        .iter()
        .map(|field| field.name())
        .collect();
    json!({
        "schema_version": 1,
        "name": command.get_name(),
        "path": path,
        "aliases": command.get_visible_aliases().collect::<Vec<_>>(),
        "summary": command.get_about().map(ToString::to_string),
        "arguments": arguments,
        "argument_groups": groups,
        "secret_input": {"transport": "stdin_json", "flag": "--secrets-stdin", "required": secret_fields, "max_bytes": MAX_SECRET_BYTES},
        "json_supported": !matches!(name.as_str(), "mcp" | "tui"),
        "long_running": name == "serve" || name == "mcp" || name == "tui",
        "commands": children,
    })
}

/// Key management is intentionally hidden from the ordinary top-level help because it is a
/// human-only surface, but its public metadata is still needed by trusted local executors.
/// Private key export remains outside the AI discovery contract.
fn discoverable(command: &Command, parent_path: &[String]) -> bool {
    if command.get_name() == "export" && parent_path.last().is_some_and(|name| name == "keys") {
        return false;
    }
    !command.is_hide_set() || (parent_path.is_empty() && command.get_name() == "keys")
}
