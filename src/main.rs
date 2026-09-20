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
}
