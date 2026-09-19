//! Resolve language before Clap renders help, without scanning arbitrary user
//! values for flags or changing command/argument identifiers.
use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Arg, ArgAction, Command, CommandFactory, FromArgMatches, error::ErrorKind};
use monica_pass_cli::admin::{absolute, default_config};
use monica_pass_cli::config::ConfigStore;
use monica_pass_cli::error::GatewayError;
use monica_pass_cli::i18n::{self, Language, LanguageChoice, Message, Preferences};
use monica_pass_cli::tr;

use super::Cli;

pub(super) fn parse() -> Result<(Cli, Language), ExitCode> {
    let args: Vec<_> = std::env::args_os().collect();
    let language = language_for_args(&args);
    let mode = bootstrap_matches(&args);
    let json = json_requested(&args, mode.as_ref());
    let protocol = mode
        .as_ref()
        .is_some_and(|matches| matches.subcommand_name() == Some("mcp"));
    let result = localized_command(language)
        .try_get_matches_from(args)
        .and_then(|matches| Cli::from_arg_matches(&matches));
    match result {
        Ok(cli) => Ok((cli, language)),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                return Err(if error.print().is_ok() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                });
            }
            if json && !protocol {
                let _ =
                    crate::cli_output::print_json(&GatewayError::InvalidRequest.response(), false);
                return Err(ExitCode::from(2));
            }
            let message = match error.kind() {
                ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
                    Message::CliParseUnknown
                }
                ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => {
                    Message::CliParseMissing
                }
                ErrorKind::InvalidValue
                | ErrorKind::ValueValidation
                | ErrorKind::TooManyValues
                | ErrorKind::TooFewValues
                | ErrorKind::WrongNumberOfValues => Message::CliParseInvalid,
                ErrorKind::ArgumentConflict => Message::CliParseConflict,
                _ => Message::CliParseError,
            };
            // Rejected argv can contain an accidentally pasted secret. Only
            // fixed diagnostics may be echoed into terminal logs.
            eprintln!("monica-pass: {}", language.text(message));
            Err(ExitCode::from(2))
        }
    }
}

fn localized_command(language: Language) -> Command {
    // build() first, so global flags and the built-in help/version arguments
    // exist and have propagated into every subcommand before they are translated.
    let mut command = Cli::command();
    command.build();
    localize(command, language)
}

fn bootstrap(command: Command) -> Command {
    command
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .ignore_errors(true)
        .mut_subcommands(bootstrap)
}

fn json_requested(args: &[OsString], matches: Option<&clap::ArgMatches>) -> bool {
    // Clap's recovery pass stops at the first invalid argument. These flags
    // take no value, and no public argument accepts bare hyphenated values.
    // Respect `--`: after it, even the exact text "--json" is ordinary data.
    matches.is_some_and(|matches| matches.get_flag("json"))
        || args
            .iter()
            .skip(1)
            .take_while(|arg| *arg != "--")
            .any(|arg| arg == "--json" || arg == "-j")
}

fn bootstrap_matches(args: &[OsString]) -> Option<clap::ArgMatches> {
    // Use the actual grammar. Making help/version ordinary flags in this
    // read-only pass also handles add --help --lang en correctly.
    bootstrap(Cli::command())
        .arg(
            Arg::new("bootstrap_help")
                .long("help")
                .short('h')
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("bootstrap_version")
                .long("version")
                .short('V')
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .subcommand(Command::new("help").arg(Arg::new("topics").num_args(0..)))
        .try_get_matches_from(args)
        .ok()
}

fn language_for_args(args: &[OsString]) -> Language {
    let matches = bootstrap_matches(args);
    let explicit = matches
        .as_ref()
        .and_then(|m| m.get_one::<LanguageChoice>("lang"))
        .copied();
    let machine = matches
        .as_ref()
        .is_some_and(|m| matches!(m.subcommand_name(), Some("mcp" | "check")))
        || json_requested(args, matches.as_ref());
    let saved = if machine {
        LanguageChoice::Auto
    } else {
        let path = matches
            .as_ref()
            .and_then(|m| m.get_one::<std::path::PathBuf>("config"))
            .cloned()
            .or_else(|| default_config().ok());
        path.and_then(|path| absolute(&path).ok())
            .and_then(|path| Preferences::load(&ConfigStore::new(path)).ok())
            .unwrap_or_default()
            .language
    };
    i18n::resolve(
        explicit,
        i18n::environment_choice(),
        saved,
        i18n::system_language(),
    )
}

pub(super) fn localize(mut command: Command, language: Language) -> Command {
    use Message::*;
    let about = match command.get_name() {
        "monica-pass" => CliAbout,
        "tui" => CliTuiHelp,
        "add" => CliAddHelp,
        "list" => CliListHelp,
        "show" => CliShowHelp,
        "settings" => CliSettingsHelp,
        "commands" => CliCommandsHelp,
        "note" => CliNoteHelp,
        "open" => CliOpenHelp,
        "webdav" => CliWebdavHelp,
        "check" => CliCheckHelp,
        "init" => CliInitHelp,
        "connect" => CliConnectHelp,
        "grant" => CliGrantHelp,
        "call" => CliCallHelp,
        "revoke" => CliRevokeHelp,
        "serve" => CliServeHelp,
        "lock" => CliLockHelp,
        "status" => CliStatusHelp,
        "mcp" => CliMcpHelp,
        "language" => CliLanguageHelp,
        "login" => CliLoginHelp,
        "publish" => CliDavPublishHelp,
        "sync" => CliDavSyncHelp,
        "help" => CliHelpCommandHelp,
        _ => CliAbout,
    };
    let webdav = command.get_name() == "webdav";
    command = command
        .about(language.text(about))
        .subcommand_help_heading(tr!(language, CliCommandsHeading))
        .help_template(format!(
            "{{before-help}}{{about-with-newline}}\n{} {{usage}}\n\n{{all-args}}{{after-help}}",
            tr!(language, CliUsage)
        ));
    // mut_args keeps declaration order; mut_arg would move each rewritten
    // argument to the end of the list and desync the positional slots that
    // build() already froze, so values land on the wrong fields.
    command = command.mut_args(|arg| {
        let message = argument_message(arg.get_id().as_str());
        let positional = arg.is_positional();
        let Some(message) = message else {
            return arg;
        };
        let heading = if positional {
            CliArgumentsHeading
        } else {
            CliOptionsHeading
        };
        arg.help(language.text(message))
            .help_heading(language.text(heading))
            .hide_default_value(language == Language::ZhCn)
            .hide_possible_values(language == Language::ZhCn)
    });
    command.mut_subcommands(|child| {
        let about = match child.get_name() {
            "list" if webdav => Some(CliDavListHelp),
            "open" if webdav => Some(CliDavOpenHelp),
            "status" if webdav => Some(CliDavStatusHelp),
            _ => None,
        };
        let child = localize(child, language);
        match about {
            Some(message) => child.about(language.text(message)),
            None => child,
        }
    })
}

fn argument_message(id: &str) -> Option<Message> {
    use Message::*;
    Some(match id {
        "config" => CliConfigHelp,
        "lang" => CliLangHelp,
        "json" => CliJsonHelp,
        "non_interactive" => CliNonInteractiveHelp,
        "secrets_stdin" => CliSecretsStdinHelp,
        "topic" => CliCommandTopicHelp,
        "language" => CliLanguageValueHelp,
        "name" => CliNameHelp,
        "note" => CliNoteValueHelp,
        "serve" => CliServeAfterHelp,
        "vault" => CliVaultHelp,
        "client" => CliClientHelp,
        "port" => CliPortHelp,
        "provider" => CliProviderHelp,
        "api_base" => CliApiBaseHelp,
        "connection" => CliConnectionHelp,
        "repositories" => CliRepositoriesHelp,
        "operations" => CliOperationsHelp,
        "ttl_minutes" => CliTtlHelp,
        "requests_per_minute" => CliRateHelp,
        "out" => CliOutHelp,
        "allow_write" => CliAllowWriteHelp,
        "url" => CliUrlHelp,
        "username" => CliUsernameHelp,
        "path" => CliRemotePathHelp,
        "help" => CliHelpHelp,
        "version" => CliVersionHelp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Command as CliCommand;

    #[test]
    fn bootstrap_does_not_treat_note_text_as_language_arguments() {
        let args = [
            "monica-pass",
            "--lang",
            "en",
            "note",
            "work",
            "--lang zh-CN",
        ]
        .map(OsString::from);
        assert_eq!(language_for_args(&args), Language::En);
    }

    #[test]
    fn json_mode_recovers_after_errors_but_respects_literal_values() {
        let args = ["monica-pass", "add", "--unsupported", "--json"].map(OsString::from);
        assert!(json_requested(&args, bootstrap_matches(&args).as_ref()));
        let args = ["monica-pass", "note", "work", "--", "--json"].map(OsString::from);
        assert!(!json_requested(&args, bootstrap_matches(&args).as_ref()));
    }

    fn bind(argv: &[&str], language: Language) -> CliCommand {
        let matches = localized_command(language)
            .try_get_matches_from(argv.iter().map(OsString::from))
            .unwrap_or_else(|error| panic!("{argv:?} rejected: {error}"));
        Cli::from_arg_matches(&matches)
            .expect("bind")
            .command
            .expect("subcommand")
    }

    #[test]
    fn translated_grammar_keeps_call_fields_aligned() {
        for language in [Language::En, Language::ZhCn] {
            let command = bind(
                &["monica", "call", "gitlab-api", "--request", "request.json"],
                language,
            );
            assert!(
                matches!(command,
                    CliCommand::Call { ref name, ref request }
                    if name == "gitlab-api"
                        && request == std::path::Path::new("request.json")),
                "{language:?} bound the wrong fields"
            );
        }
    }

    #[test]
    fn translated_grammar_keeps_option_values_off_positionals() {
        let command = bind(
            &[
                "monica",
                "connect",
                "gitlab",
                "-b",
                "https://example.test/api/v4/",
                "-n",
                "public note",
            ],
            Language::ZhCn,
        );
        assert!(matches!(command,
                CliCommand::Connect { ref name, ref api_base, ref note, .. }
                if name == "gitlab"
                    && api_base.as_deref() == Some("https://example.test/api/v4/")
                    && note == "public note"));
    }

    #[test]
    fn translation_rewrites_arguments_without_reordering_them() {
        let command = localized_command(Language::ZhCn);
        let call = command.find_subcommand("call").expect("call subcommand");
        let ids: Vec<&str> = call
            .get_arguments()
            .map(|arg| arg.get_id().as_str())
            .collect();
        assert_eq!(ids[..2], ["name", "request"], "{ids:?}");
        let help = |id: &str| {
            call.get_arguments()
                .find(|arg| arg.get_id().as_str() == id)
                .and_then(|arg| arg.get_help())
                .map(|help| help.to_string())
        };
        for (id, message) in [
            ("name", Message::CliNameHelp),
            ("config", Message::CliConfigHelp),
        ] {
            assert_eq!(
                help(id),
                Some(Language::ZhCn.text(message).to_string()),
                "{id} lost its translated help"
            );
        }
        assert!(
            help("help").is_some(),
            "built-in arguments are out of reach of the translation pass"
        );
    }
}
