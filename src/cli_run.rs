use std::path::PathBuf;

use monica_pass_cli::admin::{self, BrokerSession, absolute, default_config};
use monica_pass_cli::config::{ClientConfig, ConfigStore, read_json};
use monica_pass_cli::credstore;
use monica_pass_cli::error::{GatewayError, Result};
use monica_pass_cli::i18n::{self, Language, Message, Preferences};
use monica_pass_cli::model::{validate_api_base, validate_name, validate_note};
use monica_pass_cli::protocol::{McpBridge, serve_mcp};
use monica_pass_cli::segment;
use monica_pass_cli::tr;
use monica_pass_cli::webdav::{WebDavClient, WebDavProfile};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::cli::{Cli, Command, KeysCommand, WebDavCommand};
use crate::cli_input::{SecretField, SecretInput, required_fields};
use crate::cli_output::Output;
use crate::cli_table;

pub async fn run(cli: Cli, lang: Language) -> Result<()> {
    let output = Output { json: cli.json };
    let command = cli.command.unwrap_or(if cli.json || cli.non_interactive {
        Command::Status
    } else {
        Command::Tui
    });
    // Stdio belongs exclusively to MCP in this branch. It must never read
    // management secrets, print CLI JSON, or initialize the terminal manager.
    if let Command::Mcp { client } = &command {
        if cli.json || cli.secrets_stdin {
            return Err(GatewayError::InvalidRequest);
        }
        return serve_mcp(&absolute(client)?).await;
    }
    if matches!(command, Command::Tui) && (cli.json || cli.non_interactive || cli.secrets_stdin) {
        return Err(GatewayError::InvalidRequest);
    }
    let mut input = SecretInput::new(
        cli.secrets_stdin,
        cli.non_interactive || cli.json,
        required_fields(command.name()),
    )?;
    if let Command::Commands { topic } = &command {
        return crate::cli_discovery::run(topic, lang, output);
    }
    if let Command::Check {
        client: Some(client),
        ..
    } = &command
    {
        return check(absolute(client)?, output).await;
    }
    let path = cli.config.map(Ok).unwrap_or_else(default_config)?;
    let store = ConfigStore::new(absolute(&path)?);
    match command {
        Command::Call { name, request } => {
            let client = admin::grant_client(&store, &name)?;
            let call = read_json(
                &request,
                monica_pass_cli::protocol::MAX_REQUEST_BYTES as u64,
            )?;
            let result = McpBridge::new(read_json(&client, 16 * 1024)?)?
                .execute(call)
                .await?;
            output.result("call", result.clone(), Some(&result))?;
        }
        Command::Databases => {
            let data = json!({"databases":monica_pass_cli::databases::list(&store)?});
            let human = cli_table::render_databases(&data["databases"], lang);
            output.result_text("databases", data, Some(human))?;
        }
        Command::Use { id } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            monica_pass_cli::databases::switch(&store, &id, &password)?;
            let data = json!({"switched":true,"grants_reset":true});
            output.note(tr!(lang, CliSwitchedDatabase, id = id));
            output.result("use", data, None)?;
        }
        Command::Token { name } => {
            validate_name(&name)?;
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            let token = input.take(SecretField::Token, tr!(lang, PromptToken))?;
            admin::lock_broker(&store).await?;
            admin::update_token(&store, &name, &password, token)?;
            let data = json!({"name":name,"token_updated":true,"previous_grants_revoked":true});
            output.note(tr!(lang, CliTokenRotated, name = name));
            output.result("token", data, None)?;
        }
        Command::RenameCategory { id, title } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            monica_pass_cli::library::rename_category(&store, &password, &id, &title)?;
            let data = json!({"id":id,"title":title});
            output.note(tr!(lang, CliCategoryRenamed, id = id, title = title));
            output.result("rename-category", data, None)?;
        }
        Command::RenameEntry { name, title } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            admin::rename_entry(&store, &name, &title, &password)?;
            let data = json!({"name":name,"title":title});
            output.note(tr!(lang, CliEntryRenamed, name = name, title = title));
            output.result("rename-entry", data, None)?;
        }
        Command::Library => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            let library = monica_pass_cli::library::read(&store, &password)?;
            let data = json!(library);
            let human = cli_table::render_library(&data, lang);
            output.result_text("library", data, Some(human))?;
        }
        Command::Category { title, parent } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            let id = monica_pass_cli::library::create_category(
                &store,
                &password,
                &title,
                parent.as_deref(),
            )?;
            let data = json!({"id":id,"title":title,"parent":parent});
            output.note(tr!(lang, CliCategoryCreated, title = title, id = id));
            output.result("category", data, None)?;
        }
        Command::Move { id, target } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            monica_pass_cli::library::move_item(&store, &password, &id, &target)?;
            let data = json!({"id":id,"target":target});
            output.note(tr!(lang, CliMoved, id = id, target = target));
            output.result("move", data, None)?;
        }
        Command::Delete { target, force } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            if !force {
                confirm_deletion(&input, lang, output, &target)?;
            }
            admin::lock_broker(&store).await?;
            let data = delete_target(&store, &target, &password, lang, output)?;
            output.note(tr!(lang, CliDeletedTombstone, target = target));
            output.result("delete", data, None)?;
        }
        Command::DeleteCategory { id, force } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            if !force {
                confirm_deletion(&input, lang, output, &id)?;
            }
            admin::lock_broker(&store).await?;
            let category = match monica_pass_cli::library::delete_category(&store, &password, &id) {
                Ok(category) => category,
                Err(monica_pass_cli::library::DeleteBlocked::NotEmpty {
                    category,
                    entries,
                    children,
                }) => {
                    output.note(tr!(
                        lang,
                        CliDeleteCategoryNotEmpty,
                        title = category.title,
                        entries = entries,
                        children = children
                    ));
                    return Err(GatewayError::InvalidRequest);
                }
                Err(blocked) => return Err(blocked.error()),
            };
            let data = json!({"id":id,"title":category.title,"kind":"category","tombstone":true});
            output.note(tr!(lang, CliDeletedTombstone, target = id));
            output.result("delete-category", data, None)?;
        }
        Command::Keys { command } => {
            return keys_command(store, command, lang, &mut input, output).await;
        }
        Command::Tui => return monica_pass_cli::tui::run(store, lang).await,
        Command::Language { language } => {
            let saved = if let Some(choice) = language {
                Preferences { language: choice }.save(&store)?;
                choice
            } else {
                Preferences::load(&store)?.language
            };
            let effective = language
                .map(|choice| choice.resolve(i18n::system_language()))
                .unwrap_or(lang);
            output.result(
                "language",
                json!({"saved": saved, "effective": effective.choice()}),
                None,
            )?;
            if !output.json {
                println!(
                    "{}",
                    if language.is_some() {
                        tr!(effective, CliLanguageSaved, language = effective.name())
                    } else {
                        tr!(
                            lang,
                            LanguageCurrent,
                            language = lang.name(),
                            choice = saved.code()
                        )
                    }
                );
            }
        }
        Command::Add { options, serve } => {
            options.validate()?;
            let creating = !store.path.exists();
            let password = input.take(
                SecretField::Password,
                if creating {
                    tr!(lang, PromptNewPassword)
                } else {
                    tr!(lang, PromptPassword)
                },
            )?;
            let confirmation = if creating {
                Some(input.confirm(&password, tr!(lang, PromptConfirmPassword))?)
            } else {
                None
            };
            if let Some(confirmation) = &confirmation {
                admin::validate_new_password(&password, confirmation)?;
            }
            let token = input.take(SecretField::Token, tr!(lang, PromptTokenForAi))?;
            admin::lock_broker(&store).await?;
            let path = admin::quick_add(
                &store,
                &options,
                &password,
                confirmation.as_deref().map(String::as_str),
                token,
            )?;
            drop(confirmation);
            let settings = admin::mcp_settings(&options.name, &path)?;
            output.result("add", json!({"name":options.name, "client_file":path, "settings_file":path.with_extension("mcp.json"), "mcp":settings, "created_vault":creating}), Some(&settings))?;
            output.note(tr!(
                lang,
                CliCreated,
                name = options.name,
                repositories = format!("{:?}", options.repositories),
                access = if options.allow_write {
                    tr!(lang, CliWriteAllowed)
                } else {
                    tr!(lang, CliReadOnly)
                },
                path = path.with_extension("mcp.json").display()
            ));
            if serve {
                return run_broker(store, password, lang, output).await;
            }
            output.note(tr!(lang, CliNextServe));
        }
        Command::List => {
            let data = admin::status(&store)?;
            let connections = data["connections"].clone();
            let human = (!output.json).then(|| cli_table::render_connections(&connections, lang));
            output.result_text("list", json!({"connections": connections}), human)?;
        }
        Command::Show { name } => {
            let data = admin::show_connection(&store, &name)?;
            let human = (!output.json).then(|| cli_table::render_connection_detail(&data, lang));
            output.result_text("show", data, human)?;
        }
        Command::Note { name, note } => {
            validate_name(&name)?;
            validate_note(&note)?;
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            admin::update_note(&store, &name, &note, &password)?;
            output.result("note", json!({"name":name, "note":note}), None)?;
            output.note(tr!(lang, CliNoteUpdated, name = name));
        }
        Command::Open { vault } => {
            output.note(tr!(lang, CliOpeningLocal));
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            let count = monica_pass_cli::sync::open_local(&store, &absolute(&vault)?, &password)?;
            output.result(
                "open",
                json!({"connections":count, "vault":store.load()?.vault, "grants_reset":true}),
                None,
            )?;
            output.note(tr!(lang, CliOpenedLocal, count = count));
        }
        Command::Webdav { command } => {
            return webdav_command(store, command, lang, &mut input, output).await;
        }
        Command::Settings { name } => {
            let data = admin::settings_for_grant(&store, &name)?;
            output.result("settings", data.clone(), Some(&data["mcp"]))?;
            output.note(tr!(
                lang,
                CliSettingsSaved,
                path = data["settings_file"].as_str().unwrap_or_default()
            ));
        }
        Command::Check {
            name: Some(name),
            client: None,
        } => return check(admin::grant_client(&store, &name)?, output).await,
        Command::Init { vault, port } => {
            let path = match vault {
                Some(path) => absolute(&path)?,
                None => store.path.with_file_name("gateway.mdbx"),
            };
            if path.exists() {
                return Err(GatewayError::AlreadyExists);
            }
            let password = input.take(SecretField::Password, tr!(lang, PromptNewPassword))?;
            let confirmation = input.confirm(&password, tr!(lang, PromptConfirmPassword))?;
            admin::initialize(&store, &path, port, &password, &confirmation)?;
            output.result("init", json!({"vault":path, "config":store.path}), None)?;
            output.note(tr!(
                lang,
                CliVaultCreated,
                vault = path.display(),
                config = store.path.display()
            ));
        }
        Command::Connect {
            category,
            name,
            title,
            provider,
            api_base,
            note,
        } => {
            validate_name(&name)?;
            validate_note(&note)?;
            let base = validate_api_base(
                api_base.as_deref().unwrap_or(provider.default_api_base()),
                provider,
            )?
            .to_string();
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            let token = input.take(SecretField::Token, tr!(lang, PromptToken))?;
            admin::lock_broker(&store).await?;
            admin::add_connection_in_category(
                &store,
                admin::NewConnection {
                    name: &name,
                    title: &title,
                    provider,
                    base: &base,
                    note: &note,
                    category: category.as_deref(),
                },
                &password,
                token,
            )?;
            output.result(
                "connect",
                json!({"name":name, "provider":provider, "api_base":base, "note":note}),
                None,
            )?;
            output.note(tr!(lang, CliConnectionStored, name = name));
        }
        Command::Grant(options) => {
            validate_name(&options.name)?;
            let config = store.load()?;
            let connection = config
                .connections
                .get(&options.connection)
                .ok_or(GatewayError::NotFound)?;
            monica_pass_cli::model::validate_grant_scope(
                &options.repositories,
                &options.operations,
                connection.provider,
            )?;
            let password = input.take(SecretField::Password, tr!(lang, PromptGrantPassword))?;
            admin::lock_broker(&store).await?;
            let path = admin::issue_grant(&store, &options, &password)?;
            drop(password);
            let settings = admin::mcp_settings(&options.name, &path)?;
            output.result(
                "grant",
                json!({"name":options.name, "client_file":path, "mcp":settings}),
                Some(&settings),
            )?;
            output.note(tr!(lang, CliClientSaved, path = path.display()));
        }
        Command::Refresh(options) => {
            validate_name(&options.name)?;
            let password = input.take(SecretField::Password, tr!(lang, PromptGrantPassword))?;
            admin::lock_broker(&store).await?;
            let path = admin::refresh_grant(&store, &options, &password)?;
            drop(password);
            let settings = admin::mcp_settings(&options.name, &path)?;
            output.result(
                "refresh",
                json!({"name":options.name, "client_file":path, "mcp":settings}),
                Some(&settings),
            )?;
            output.note(tr!(lang, CliGrantRefreshed, name = options.name));
        }
        Command::Revoke { name } => {
            admin::revoke(&store, &name)?;
            output.result("revoke", json!({"name":name, "revoked":true}), None)?;
            output.note(tr!(lang, CliGrantRevoked, name = name));
        }
        Command::Serve => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            return run_broker(store, password, lang, output).await;
        }
        Command::Lock => {
            if !store.path.is_file() {
                return Err(GatewayError::NotFound);
            }
            admin::lock_broker(&store).await?;
            output.result("lock", json!({"locked":true}), None)?;
            output.note(tr!(lang, BrokerLockedHint));
        }
        Command::Status => {
            let data = admin::status(&store)?;
            let human = (!output.json).then(|| cli_table::render_status(&data, lang));
            output.result_text("status", data, human)?;
        }
        Command::Audit { grant, limit } => {
            let data = admin::read_audit(&store, grant.as_deref(), limit as usize)?;
            let human = (!output.json).then(|| cli_table::render_audit(&data, lang));
            output.result_text("audit", data, human)?;
        }
        Command::Mcp { .. } | Command::Check { .. } | Command::Commands { .. } => {
            return Err(GatewayError::InvalidRequest);
        }
    }
    Ok(())
}

async fn check(path: PathBuf, output: Output) -> Result<()> {
    let config: ClientConfig = read_json(&path, 16 * 1024)?;
    let tools = McpBridge::new(config)?.discover().await?;
    output.result(
        "check",
        json!({"tools":tools}),
        Some(&json!({"ok":true,"tools":tools})),
    )
}

async fn run_broker(
    store: ConfigStore,
    password: Zeroizing<String>,
    lang: Language,
    output: Output,
) -> Result<()> {
    let mut session = BrokerSession::start(store.clone(), password).await?;
    let address = store.load()?.listen;
    output.event(
        "serve",
        "ready",
        json!({"listen":address, "session_seconds":300}),
    )?;
    output.note(tr!(lang, CliBrokerReady, address = address));
    output.note(tr!(lang, CliSessionLifetime));
    tokio::select! {
        result = session.wait() => result?,
        _ = tokio::signal::ctrl_c() => session.stop().await?,
    }
    output.event("serve", "stopped", json!({"locked":true}))?;
    output.note(tr!(lang, CliBrokerStopped));
    Ok(())
}

async fn webdav_command(
    store: ConfigStore,
    command: WebDavCommand,
    lang: Language,
    input: &mut SecretInput,
    output: Output,
) -> Result<()> {
    if matches!(command, WebDavCommand::Status) {
        let profile = WebDavProfile::load(&store)?;
        let binding = if store.path.exists() {
            store.load()?.webdav
        } else {
            None
        };
        let password_saved = profile
            .as_ref()
            .is_some_and(|profile| credstore::present(&profile.base_url, &profile.username));
        let safe_remote_replace = binding.as_ref().map(|binding| binding.etag.is_some());
        let segments = binding
            .as_ref()
            .map(|binding| monica_pass_cli::segment::status(&store, &binding.vault_id))
            .transpose()?;
        let data = json!({"profile":profile, "sync":binding, "password_saved":password_saved, "safe_remote_replace":safe_remote_replace, "segments":segments});
        let human = (!output.json).then(|| cli_table::render_webdav_status(&data, lang));
        return output.result_text("webdav status", data, human);
    }
    if matches!(command, WebDavCommand::ForgetPassword) {
        let profile = WebDavProfile::load(&store)?.ok_or(GatewayError::InvalidWebDav)?;
        let removed =
            credstore::forget(&profile.base_url, &profile.username).map_err(map_store_error)?;
        output.result("webdav forget-password", json!({"removed":removed}), None)?;
        output.note(lang.text(if removed {
            Message::CliWebDavPasswordForgotten
        } else {
            Message::CliWebDavPasswordAbsent
        }));
        return Ok(());
    }
    let profile = match &command {
        WebDavCommand::Login { url, username } => WebDavProfile::new(url, username)?,
        WebDavCommand::Sync => {
            store
                .load()?
                .webdav
                .ok_or(GatewayError::RemoteNotConfigured)?
                .profile
        }
        _ => WebDavProfile::load(&store)?.ok_or(GatewayError::InvalidWebDav)?,
    };
    let account = (profile.base_url.clone(), profile.username.clone());
    // Only a password a person types is worth remembering. One supplied by a
    // trusted producer through `--secrets-stdin` stays in that process.
    let mut typed: Option<Zeroizing<String>> = None;
    let password = match credstore::load(&account.0, &account.1) {
        Some(secret) => secret,
        None => {
            let secret =
                input.take(SecretField::WebDavPassword, tr!(lang, PromptWebDavPassword))?;
            if !input.injected() {
                typed = Some(secret.clone());
            }
            secret
        }
    };
    let client = WebDavClient::new(profile, password)?;
    match command {
        WebDavCommand::Login { .. } => {
            client.list("").await?;
            client.profile.save(&store)?;
            let saved = remember_password(&account.0, &account.1, &mut typed, &output, lang);
            output.result(
                "webdav login",
                json!({"profile":client.profile,"password_saved":saved}),
                None,
            )?;
            output.note(tr!(lang, CliLoginVerified));
        }
        WebDavCommand::List { path } => {
            let entries = client.list(&path).await?;
            let data = json!({"path":path,"entries":entries});
            let human = cli_table::render_webdav_list(&data["entries"], lang);
            output.result_text("webdav list", data, Some(human))?;
        }
        WebDavCommand::Open { path } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptRemotePassword))?;
            admin::lock_broker(&store).await?;
            let cancel = segment::Cancel::default();
            let mut sink = |event: &segment::Event| output.note(lang.segment_progress(event));
            let count = until_cancelled(
                &cancel,
                monica_pass_cli::sync::open_remote_with_progress(
                    &store, &client, &path, &password, &mut sink, &cancel,
                ),
            )
            .await?;
            output.result(
                "webdav open",
                json!({"connections":count,"vault":store.load()?.vault,"grants_reset":true,
                       "cancelled":cancel.cancelled()}),
                None,
            )?;
            output.note(tr!(lang, CliOpenedRemote, count = count));
            if cancel.cancelled() {
                output.note(tr!(lang, CliOpenPartiallyReplayed));
            }
        }
        WebDavCommand::Publish { path } => {
            let password = input.take(SecretField::Password, tr!(lang, PromptLocalPassword))?;
            admin::lock_broker(&store).await?;
            let result = monica_pass_cli::sync::publish(&store, &client, &path, &password).await?;
            output.result("webdav publish", json!({"result":result}), None)?;
            if !output.json {
                println!("{}", lang.sync_result(result));
            }
        }
        WebDavCommand::Sync => {
            let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
            admin::lock_broker(&store).await?;
            let cancel = segment::Cancel::default();
            let mut sink = |event: &segment::Event| output.note(lang.segment_progress(event));
            let outcome = until_cancelled(
                &cancel,
                monica_pass_cli::sync::synchronize_with_progress(
                    &store, &client, &password, &mut sink, &cancel,
                ),
            )
            .await?;
            let data = match &outcome.segments {
                Some(report) => json!({"result":outcome.result,"segments":report}),
                None => json!({"result":outcome.result}),
            };
            output.result("webdav sync", data, None)?;
            if !output.json {
                println!("{}", lang.sync_message(&outcome));
            }
        }
        WebDavCommand::Status | WebDavCommand::ForgetPassword => {
            unreachable!("status and forget-password do not need a login")
        }
    }
    remember_password(&account.0, &account.1, &mut typed, &output, lang);
    Ok(())
}

/// Runs a long remote operation under Ctrl+C. The first press asks the segment run
/// to stop at its next boundary, which keeps the saved cursor exact and lets the
/// result still be printed; a second press ends the process at once, because a user
/// who presses twice wants out now rather than after one more segment.
async fn until_cancelled<T>(
    cancel: &segment::Cancel,
    operation: impl std::future::Future<Output = T>,
) -> T {
    let armed = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            cancel.trigger();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(130);
            }
        })
    };
    let value = operation.await;
    armed.abort();
    value
}

/// Stores a password a person just typed in this computer's credential manager.
/// Called once the request that used it has succeeded, so a wrong password is
/// never remembered. Returns whether a password is available locally afterwards.
fn remember_password(
    base_url: &str,
    username: &str,
    typed: &mut Option<Zeroizing<String>>,
    output: &Output,
    lang: Language,
) -> bool {
    let Some(secret) = typed.take() else {
        return credstore::present(base_url, username);
    };
    match credstore::save(base_url, username, &secret) {
        Ok(()) => {
            output.note(tr!(lang, CliWebDavPasswordRemembered));
            true
        }
        Err(err) => {
            output.note(tr!(
                lang,
                CliWebDavPasswordNotRemembered,
                reason = err.reason()
            ));
            false
        }
    }
}

fn map_store_error(_: credstore::StoreError) -> GatewayError {
    GatewayError::StateUnavailable
}

/// Nothing is removed until a person types the target back. `--force` is the explicit
/// opt-out for a target already checked, and where no prompt is possible the command
/// refuses rather than reading silence as consent.
fn confirm_deletion(
    input: &SecretInput,
    lang: Language,
    output: Output,
    target: &str,
) -> Result<()> {
    if input.typed(&tr!(lang, PromptConfirmDelete, target = target))? != target {
        output.note(tr!(lang, CliDeleteUnconfirmed, target = target));
        return Err(GatewayError::InvalidRequest);
    }
    Ok(())
}

/// One target, two meanings: a saved connection name wins, and anything else is a native
/// entry ID as shown by `library`. An entry holding a connection's credential is refused,
/// because deleting the row alone would leave the binding pointing at a deleted secret.
fn delete_target(
    store: &ConfigStore,
    target: &str,
    password: &str,
    lang: Language,
    output: Output,
) -> Result<Value> {
    let config = store.load()?;
    if config.connections.contains_key(target) {
        let (entry, grants) = admin::delete_connection(store, target, password)?;
        return Ok(json!({
            "target": target,
            "kind": "connection",
            "credential_removed": matches!(entry, admin::EntryDelete::Deleted),
            "grants_revoked": grants,
            "tombstone": true,
        }));
    }
    if let Some((name, _)) = config
        .connections
        .iter()
        .find(|(_, binding)| binding.credential_id == target)
    {
        output.note(tr!(
            lang,
            CliDeleteBoundCredential,
            target = target,
            name = name
        ));
        return Err(GatewayError::InvalidRequest);
    }
    monica_pass_cli::library::delete_entry(store, password, target)?;
    Ok(json!({"target":target,"kind":"entry","tombstone":true}))
}

/// SSH and GPG entries are managed locally: the vault is unlocked, the work is done through
/// `keys::manage`, and only public projections or export bookkeeping reach the response.
async fn keys_command(
    store: ConfigStore,
    command: Option<KeysCommand>,
    lang: Language,
    input: &mut SecretInput,
    output: Output,
) -> Result<()> {
    use monica_pass_cli::keys::manage;
    let password = input.take(SecretField::Password, tr!(lang, PromptPassword))?;
    // Confirmed before the broker is stopped: declining a delete should not cost a session.
    if let Some(KeysCommand::Delete {
        entry,
        force: false,
    }) = &command
    {
        confirm_deletion(input, lang, output, entry)?;
    }
    admin::lock_broker(&store).await?;
    let Some(command) = command else {
        let entries = manage::list(&store, &password)?;
        let human = (!output.json).then(|| cli_table::render_keys(&entries, lang));
        return output.result_text("keys", json!({"keys": entries}), human);
    };
    match command {
        KeysCommand::Ssh {
            key_name,
            generate,
            private_key,
            comment,
            category,
            purpose,
        } => {
            let saved = match (generate, private_key) {
                (Some(algorithm), None) => manage::generate_ssh(
                    &store,
                    &password,
                    &key_name,
                    &purpose,
                    category.as_deref(),
                    &algorithm,
                    comment.as_deref().unwrap_or_default(),
                )?,
                (None, Some(path)) => manage::import_ssh(
                    &store,
                    &password,
                    &key_name,
                    &purpose,
                    category.as_deref(),
                    comment.as_deref().unwrap_or_default(),
                    &absolute(&path)?,
                )?,
                _ => unreachable!("the material group accepts exactly one source"),
            };
            output.result("keys ssh", json!({"key": &saved}), None)?;
            output.note(tr!(
                lang,
                CliKeyStored,
                name = saved.title,
                fingerprint = saved.fingerprint
            ));
        }
        KeysCommand::Gpg {
            key_name,
            public_key,
            private_key,
            category,
            purpose,
        } => {
            let public = public_key.as_ref().map(|path| absolute(path)).transpose()?;
            let secret = private_key
                .as_ref()
                .map(|path| absolute(path))
                .transpose()?;
            let saved = manage::import_gpg(
                &store,
                &password,
                &key_name,
                &purpose,
                category.as_deref(),
                public.as_deref(),
                secret.as_deref(),
            )?;
            output.result("keys gpg", json!({"key": &saved}), None)?;
            output.note(tr!(
                lang,
                CliKeyStored,
                name = saved.title,
                fingerprint = saved.fingerprint
            ));
        }
        KeysCommand::Edit {
            entry,
            new_title,
            purpose,
            comment,
        } => {
            let saved = manage::edit(
                &store,
                &password,
                &entry,
                new_title.as_deref(),
                purpose.as_deref(),
                comment.as_deref(),
            )?;
            output.result("keys edit", json!({"key": saved}), None)?;
            output.note(tr!(lang, CliKeyEdited, name = saved.title));
        }
        KeysCommand::Delete { entry, .. } => {
            let removed = manage::delete(&store, &password, &entry)?;
            output.result(
                "keys delete",
                json!({
                    "name": removed.title,
                    "entry_id": removed.entry_id,
                    "algorithm": removed.algorithm,
                    "kind": "key",
                    "tombstone": true,
                }),
                None,
            )?;
            output.note(tr!(lang, CliDeletedTombstone, target = removed.title));
        }
        KeysCommand::Export {
            entry,
            output: destination,
            private,
            force,
        } => {
            let exported = manage::export(
                &store,
                &password,
                &entry,
                &absolute(&destination)?,
                private,
                force,
            )?;
            output.result(
                "keys export",
                json!({
                    "name": exported.summary.title,
                    "path": exported.path,
                    "bytes": exported.bytes,
                    "private": exported.private,
                }),
                None,
            )?;
            output.note(if exported.private {
                tr!(
                    lang,
                    CliKeyExportedPrivate,
                    bytes = exported.bytes,
                    path = exported.path.display()
                )
            } else {
                tr!(
                    lang,
                    CliKeyExportedPublic,
                    bytes = exported.bytes,
                    path = exported.path.display()
                )
            });
        }
    }
    Ok(())
}
