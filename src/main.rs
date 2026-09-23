mod cli;
mod cli_discovery;
mod cli_input;
mod cli_language;
mod cli_output;
mod cli_run;
mod cli_table;

use cli::{Cli, Command};
use monica_pass_cli::error::GatewayError;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let (cli, language) = match cli_language::parse() {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let protocol = matches!(cli.command, Some(Command::Mcp { .. }));
    let json = cli.json && !protocol;
    let command =
        cli.command
            .as_ref()
            .map(Command::name)
            .unwrap_or(if cli.json || cli.non_interactive {
                "status"
            } else {
                "tui"
            });
    match cli_run::run(cli, language).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if json {
                let mut value = error.response();
                value["command"] = command.into();
                if error == GatewayError::SecretInputRequired {
                    value["error"]["required"] = serde_json::json!(
                        cli_input::required_fields(command)
                            .iter()
                            .map(|field| field.name())
                            .collect::<Vec<_>>()
                    );
                }
                let _ = cli_output::print_json(&value, false);
            } else {
                eprintln!("monica-pass: {}", language.error(error));
            }
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use cli::KeysCommand;
    use monica_pass_cli::model::Operation;
    use monica_pass_cli::model::Provider;
    use std::path::Path;

    #[test]
    fn cli_quick_add_is_read_only_by_default_with_explicit_write_opt_in() {
        let cli = Cli::try_parse_from([
            "monica-pass",
            "add",
            "work",
            "--repo",
            "example/project",
            "--note",
            "项目 Issue 跟踪",
            "--serve",
        ])
        .unwrap();
        let Some(Command::Add { options, serve }) = cli.command else {
            panic!("expected add")
        };
        assert_eq!(options.name, "work");
        assert_eq!(options.provider, Provider::Github);
        assert_eq!(options.repositories, ["example/project"]);
        assert_eq!(options.note, "项目 Issue 跟踪");
        assert!(!options.allow_write);
        assert!(serve);
        let cli = Cli::try_parse_from([
            "monica-pass",
            "add",
            "gitlab",
            "--provider",
            "gitlab",
            "--repo",
            "group/sub/project",
            "--allow-write",
        ])
        .unwrap();
        let Some(Command::Add { options, serve }) = cli.command else {
            panic!("expected add")
        };
        assert!(options.allow_write);
        assert!(!serve);
        assert_eq!(options.provider, Provider::Gitlab);
        assert!(options.validate().is_ok());
    }

    #[test]
    fn cli_setup_does_not_accept_secrets_or_unscoped_grants_in_arguments() {
        assert!(Cli::try_parse_from(["monica-pass", "add", "work"]).is_err());
        for secret_flag in ["--token", "--password", "--master-password"] {
            assert!(
                Cli::try_parse_from([
                    "monica-pass",
                    "add",
                    "work",
                    "--repo",
                    "example/project",
                    secret_flag,
                    "not-accepted"
                ])
                .is_err()
            );
        }
        let cli =
            Cli::try_parse_from(["monica-pass", "note", "work", "updated public context"]).unwrap();
        assert!(
            matches!(cli.command, Some(Command::Note { name, note }) if name == "work" && note == "updated public context")
        );
    }

    #[test]
    fn cli_requires_explicit_write_authorization() {
        let cli = Cli::try_parse_from([
            "monica-pass",
            "grant",
            "agent",
            "--connection",
            "work",
            "--repo",
            "org/repo",
        ])
        .unwrap();
        let Some(Command::Grant(options)) = cli.command else {
            panic!("expected grant")
        };
        assert_eq!(
            options.operations,
            [Operation::ListIssues, Operation::GetIssue]
        );
        let cli = Cli::try_parse_from([
            "monica-pass",
            "grant",
            "agent",
            "--connection",
            "work",
            "--repo",
            "org/repo",
            "--operation",
            "create-issue",
        ])
        .unwrap();
        let Some(Command::Grant(options)) = cli.command else {
            panic!("expected grant")
        };
        assert_eq!(options.operations, [Operation::CreateIssue]);
    }

    #[test]
    fn key_commands_carry_names_and_file_paths_never_key_text() {
        for bare in ["keys", "k"] {
            let cli = Cli::try_parse_from(["monica-pass", bare]).unwrap();
            assert!(matches!(cli.command, Some(Command::Keys { command: None })));
            assert_eq!(cli.command.as_ref().unwrap().name(), "keys");
        }

        let cli = Cli::try_parse_from([
            "monica-pass",
            "keys",
            "ssh",
            "笔记本",
            "--generate",
            "rsa3072",
            "--comment",
            "cli@test",
            "--category",
            "33333333-3333-3333-3333-333333333333",
            "-n",
            "登录代码托管",
        ])
        .unwrap();
        let Some(Command::Keys {
            command:
                Some(KeysCommand::Ssh {
                    key_name,
                    generate,
                    private_key,
                    comment,
                    category,
                    purpose,
                }),
        }) = &cli.command
        else {
            panic!("expected keys ssh")
        };
        assert_eq!(key_name, "笔记本");
        assert_eq!(generate.as_deref(), Some("rsa3072"));
        assert!(private_key.is_none());
        assert_eq!(comment.as_deref(), Some("cli@test"));
        assert_eq!(
            category.as_deref(),
            Some("33333333-3333-3333-3333-333333333333")
        );
        assert_eq!(purpose, "登录代码托管");
        assert_eq!(cli.command.as_ref().unwrap().name(), "keys ssh");
        let cli = Cli::try_parse_from([
            "monica-pass",
            "keys",
            "ssh",
            "x",
            "--private-key",
            "id_ed25519",
        ])
        .unwrap();
        let Some(Command::Keys {
            command: Some(KeysCommand::Ssh { purpose, .. }),
        }) = &cli.command
        else {
            panic!("expected keys ssh")
        };
        assert_eq!(purpose, "");

        // Exactly one source of key material, and never a value that could be the material.
        assert!(Cli::try_parse_from(["monica-pass", "keys", "ssh", "x"]).is_err());
        assert!(
            Cli::try_parse_from([
                "monica-pass",
                "keys",
                "ssh",
                "x",
                "--generate",
                "ed25519",
                "--private-key",
                "id_ed25519",
            ])
            .is_err()
        );

        let cli = Cli::try_parse_from([
            "monica-pass",
            "keys",
            "gpg",
            "ring",
            "--private-key",
            "secring.asc",
        ])
        .unwrap();
        let Some(Command::Keys {
            command:
                Some(KeysCommand::Gpg {
                    key_name,
                    public_key,
                    private_key,
                    ..
                }),
        }) = &cli.command
        else {
            panic!("expected keys gpg")
        };
        assert_eq!(key_name, "ring");
        assert!(public_key.is_none());
        assert_eq!(private_key.as_deref(), Some(Path::new("secring.asc")));
        assert_eq!(cli.command.as_ref().unwrap().name(), "keys gpg");
        assert!(Cli::try_parse_from(["monica-pass", "keys", "gpg", "ring"]).is_err());

        let cli = Cli::try_parse_from(["monica-pass", "keys", "edit", "ring", "--title", "旧ring"])
            .unwrap();
        let Some(Command::Keys {
            command:
                Some(KeysCommand::Edit {
                    entry,
                    new_title,
                    purpose,
                    comment,
                }),
        }) = &cli.command
        else {
            panic!("expected keys edit")
        };
        assert_eq!(entry, "ring");
        assert_eq!(new_title.as_deref(), Some("旧ring"));
        assert!(purpose.is_none() && comment.is_none());
        assert_eq!(cli.command.as_ref().unwrap().name(), "keys edit");
        assert!(Cli::try_parse_from(["monica-pass", "keys", "edit", "ring"]).is_err());
        let cli = Cli::try_parse_from([
            "monica-pass",
            "keys",
            "edit",
            "ring",
            "--title",
            "work",
            "-n",
            "签名用",
            "--comment",
            "work@laptop",
        ])
        .unwrap();
        let Some(Command::Keys {
            command:
                Some(KeysCommand::Edit {
                    new_title,
                    purpose,
                    comment,
                    ..
                }),
        }) = &cli.command
        else {
            panic!("expected keys edit")
        };
        assert_eq!(
            (new_title.as_deref(), purpose.as_deref(), comment.as_deref()),
            (Some("work"), Some("签名用"), Some("work@laptop"))
        );

        let cli = Cli::try_parse_from([
            "monica-pass",
            "keys",
            "export",
            "ring",
            "-o",
            "out.asc",
            "--private",
            "--force",
        ])
        .unwrap();
        let Some(Command::Keys {
            command:
                Some(KeysCommand::Export {
                    entry,
                    output,
                    private,
                    force,
                }),
        }) = &cli.command
        else {
            panic!("expected keys export")
        };
        assert_eq!(entry, "ring");
        assert_eq!(output, Path::new("out.asc"));
        assert!(*private && *force);
        assert_eq!(cli.command.as_ref().unwrap().name(), "keys export");
        assert!(Cli::try_parse_from(["monica-pass", "keys", "export", "ring"]).is_err());
    }

    /// The teaching site under docs/reference/ is generated from this binary's
    /// grammar, and a sample line the parser rejects is worse than no sample.
    /// Node is not available in `cargo test`, so the shipped data.js is checked
    /// against the same clap tree here instead.
    #[test]
    fn teaching_site_data_matches_the_live_grammar() {
        use clap::CommandFactory;
        use monica_pass_cli::i18n::Language;
        use serde_json::Value;
        use std::collections::BTreeMap;

        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("reference")
            .join("data.js");
        let raw = std::fs::read_to_string(&file).expect("docs/reference/data.js is missing");
        let json = &raw[raw.find('{').expect("data.js carries no JSON payload")
            ..=raw.rfind('}').expect("data.js carries no JSON payload")];
        let site: Value = serde_json::from_str(json).expect("data.js is not valid JSON");

        let mut command = Cli::command();
        command.build();
        let command = cli_language::localize(command, Language::En);
        let grammar = cli_discovery::describe(&command, &[]);

        let mut flags: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut constraints: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut globals: Vec<String> = Vec::new();
        collect_grammar(&grammar, &mut flags, &mut constraints, &mut globals);

        let site_flags: BTreeMap<String, Vec<String>> = site["commands"]
            .as_array()
            .expect("data.js has no commands")
            .iter()
            .map(|entry| {
                (
                    entry["key"].as_str().unwrap().to_owned(),
                    shape(entry["args"].as_array().unwrap()),
                )
            })
            .collect();

        let site_constraints: BTreeMap<String, Vec<String>> = site["commands"]
            .as_array()
            .expect("data.js has no commands")
            .iter()
            .map(|entry| {
                (
                    entry["key"].as_str().unwrap().to_owned(),
                    constraint_tokens(
                        entry["args"].as_array().unwrap(),
                        &entry["argGroups"],
                        &entry["subcommandRequired"],
                    ),
                )
            })
            .collect();

        let hand_authored: Vec<&str> = site["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["handAuthored"] == Value::Bool(true))
            .map(|entry| entry["key"].as_str().unwrap())
            .collect();

        for key in flags.keys() {
            assert!(
                site_flags.contains_key(key.as_str()),
                "`{key}` is in the grammar but not in docs/reference/data.js; re-run `node docs/reference/build.mjs`"
            );
        }
        for key in site_flags.keys() {
            assert!(
                flags.contains_key(key.as_str()) || hand_authored.contains(&key.as_str()),
                "data.js teaches `{key}`, which the grammar no longer has; re-run build.mjs"
            );
        }
        for (key, expected) in &flags {
            if let Some(actual) = site_flags.get(key) {
                assert_eq!(
                    actual, expected,
                    "the flags taught for `{key}` differ from the grammar; re-run build.mjs"
                );
            }
        }
        for (key, expected) in &constraints {
            if let Some(actual) = site_constraints.get(key) {
                assert_eq!(
                    actual, expected,
                    "the constraints taught for `{key}` differ from the grammar; re-run build.mjs"
                );
            }
        }
        assert_eq!(
            site["meta"]["commandCount"].as_i64().unwrap() as usize,
            flags.len(),
            "data.js counts a different number of grammar commands"
        );
        let site_globals: Vec<String> = site["meta"]["globals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            site_globals, globals,
            "the global flags taught by data.js differ from the grammar"
        );
    }

    fn collect_grammar(
        node: &serde_json::Value,
        flags: &mut std::collections::BTreeMap<String, Vec<String>>,
        constraints: &mut std::collections::BTreeMap<String, Vec<String>>,
        globals: &mut Vec<String>,
    ) {
        use serde_json::Value;
        let path: Vec<&str> = node["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect();
        let arguments = node["arguments"].as_array().unwrap();
        for arg in arguments {
            if arg["global"] == Value::Bool(true)
                && !globals.contains(&arg["id"].as_str().unwrap().to_owned())
            {
                globals.push(arg["id"].as_str().unwrap().to_owned());
            }
        }
        if !path.is_empty() {
            let own = arguments
                .iter()
                .filter(|arg| arg["global"] != Value::Bool(true))
                .cloned()
                .collect::<Vec<_>>();
            let key = path.join(" ");
            flags.insert(key.clone(), shape(&own));
            constraints.insert(
                key,
                constraint_tokens(&own, &node["argument_groups"], &node["subcommand_required"]),
            );
        }
        for child in node["commands"].as_array().unwrap() {
            collect_grammar(child, flags, constraints, globals);
        }
    }

    /// Reads a field under either spelling so one comparison covers the grammar
    /// (snake_case, straight out of clap) and data.js (camelCase, what build.mjs
    /// writes for the browser).
    fn field<'a>(value: &'a serde_json::Value, snake: &str, camel: &str) -> &'a serde_json::Value {
        value
            .get(snake)
            .or_else(|| value.get(camel))
            .unwrap_or(&serde_json::Value::Null)
    }

    /// The facts the practice grammar judges a line by beyond argument names:
    /// which values are allowed, whether case folds, which arguments form a
    /// group, and whether a group of them is required. Those are exactly the
    /// fields a stale data.js would silently get wrong.
    fn constraint_tokens(
        args: &[serde_json::Value],
        groups: &serde_json::Value,
        subcommand_required: &serde_json::Value,
    ) -> Vec<String> {
        fn list(value: &serde_json::Value) -> String {
            value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default()
        }
        fn truthy(value: &serde_json::Value) -> bool {
            *value == serde_json::Value::Bool(true)
        }
        let mut tokens = vec![format!(
            "subcommand_required {}",
            truthy(field(
                subcommand_required,
                "subcommand_required",
                "subcommandRequired"
            ))
        )];
        for arg in args {
            let id = arg["id"].as_str().unwrap_or("?");
            tokens.push(format!(
                "choices {id} {}",
                list(field(arg, "choices", "choices"))
            ));
            tokens.push(format!(
                "aliases {id} {}",
                list(field(arg, "choice_aliases", "choiceAliases"))
            ));
            tokens.push(format!(
                "ignore_case {id} {}",
                truthy(field(arg, "ignore_case", "ignoreCase"))
            ));
        }
        if let serde_json::Value::Array(items) = groups {
            for group in items {
                tokens.push(format!(
                    "group {} {}",
                    list(field(group, "required_one_of", "oneOf")),
                    truthy(field(group, "multiple", "multiple"))
                ));
            }
        }
        tokens.sort();
        tokens
    }

    /// Same projection build.mjs --check compares: a flag's long name, or the
    /// argument id for positionals, sorted so ordering never matters.
    fn shape(args: &[serde_json::Value]) -> Vec<String> {
        let mut names: Vec<String> = args
            .iter()
            .map(|arg| match arg["long"].as_str() {
                Some(long) => long.to_owned(),
                None => arg["id"].as_str().unwrap().to_owned(),
            })
            .collect();
        names.sort();
        names
    }
}
