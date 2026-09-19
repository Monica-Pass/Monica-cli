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
    use monica_pass_cli::model::Operation;
    use monica_pass_cli::model::Provider;

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
}
