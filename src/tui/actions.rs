use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::admin::{self, AddOptions, BrokerSession, GrantOptions};
use crate::config::{ClientConfig, ConfigStore, read_json};
use crate::error::{GatewayError, Result};
use crate::i18n::Message;
use crate::model::Provider;
use crate::protocol::McpBridge;
use crate::sync;
use crate::webdav::{RemoteEntry, WebDavClient, WebDavProfile};

pub(super) enum Action {
    SwitchDatabase {
        id: String,
        password: Zeroizing<String>,
    },
    Token {
        name: String,
        password: Zeroizing<String>,
        token: Zeroizing<String>,
    },
    RenameCategory {
        id: String,
        title: String,
        password: Zeroizing<String>,
    },
    Category {
        title: String,
        parent: Option<String>,
        password: Zeroizing<String>,
    },
    Move {
        id: String,
        target: String,
        password: Zeroizing<String>,
    },
    Library(Zeroizing<String>),
    Add {
        options: AddOptions,
        password: Zeroizing<String>,
        confirmation: Option<Zeroizing<String>>,
        token: Zeroizing<String>,
    },
    Note {
        name: String,
        note: String,
        password: Zeroizing<String>,
    },
    Init {
        path: PathBuf,
        port: u16,
        password: Zeroizing<String>,
        confirmation: Zeroizing<String>,
    },
    OpenLocal {
        path: PathBuf,
        password: Zeroizing<String>,
    },
    Connect {
        category: Option<String>,
        name: String,
        provider: Provider,
        base: String,
        note: String,
        token: Zeroizing<String>,
        password: Zeroizing<String>,
    },
    Grant {
        options: GrantOptions,
        password: Zeroizing<String>,
    },
    Revoke(String),
    Unlock(Zeroizing<String>),
    Lock,
    Login {
        profile: WebDavProfile,
        password: Zeroizing<String>,
    },
    Browse(String),
    OpenRemote {
        path: String,
        password: Zeroizing<String>,
    },
    Publish {
        path: String,
        password: Zeroizing<String>,
    },
    Sync(Zeroizing<String>),
    Probe(PathBuf),
}

impl Action {
    pub fn pauses_broker(&self) -> bool {
        matches!(
            self,
            Self::SwitchDatabase { .. }
                | Self::Token { .. }
                | Self::RenameCategory { .. }
                | Self::Category { .. }
                | Self::Move { .. }
                | Self::Library(_)
                | Self::Add { .. }
                | Self::Note { .. }
                | Self::Init { .. }
                | Self::OpenLocal { .. }
                | Self::Connect { .. }
                | Self::Grant { .. }
                | Self::Unlock(_)
                | Self::Lock
                | Self::OpenRemote { .. }
                | Self::Publish { .. }
                | Self::Sync(_)
        )
    }

    pub fn label(&self) -> Message {
        match self {
            Self::SwitchDatabase { .. } => Message::PendingOpen,
            Self::Token { .. } => Message::PendingConnect,
            Self::RenameCategory { .. } | Self::Category { .. } | Self::Move { .. } => {
                Message::PendingConnect
            }
            Self::Library(_) => Message::PendingUnlock,
            Self::Add { .. } => Message::PendingAdd,
            Self::Note { .. } => Message::PendingNote,
            Self::Init { .. } => Message::PendingInit,
            Self::OpenLocal { .. } | Self::OpenRemote { .. } => Message::PendingOpen,
            Self::Connect { .. } => Message::PendingConnect,
            Self::Grant { .. } => Message::PendingGrant,
            Self::Revoke(_) => Message::PendingRevoke,
            Self::Unlock(_) => Message::PendingUnlock,
            Self::Lock => Message::PendingLock,
            Self::Login { .. } => Message::PendingLogin,
            Self::Browse(_) => Message::PendingBrowse,
            Self::Publish { .. } => Message::PendingPublish,
            Self::Sync(_) => Message::PendingSync,
            Self::Probe(_) => Message::PendingProbe,
        }
    }
}

pub(super) enum Outcome {
    Library(crate::library::Library),
    Added {
        name: String,
        path: PathBuf,
        broker: Result<BrokerSession>,
    },
    Message(Message),
    Revoked(String),
    Opened(usize),
    Synced(sync::SyncResult),
    Broker(BrokerSession),
    Login {
        client: WebDavClient,
        entries: Vec<RemoteEntry>,
    },
    Browse {
        path: String,
        entries: Vec<RemoteEntry>,
    },
    Grant {
        name: String,
        path: PathBuf,
    },
    Probe(Vec<String>),
}

pub(super) async fn login(store: &ConfigStore, client: WebDavClient) -> Result<Outcome> {
    let entries = client.list("").await?;
    client.profile.save(store)?;
    Ok(Outcome::Login { client, entries })
}

pub(super) async fn perform(
    store: ConfigStore,
    action: Action,
    webdav: Option<WebDavClient>,
) -> Result<Outcome> {
    if action.pauses_broker() {
        admin::lock_broker(&store).await?;
    }
    match action {
        Action::SwitchDatabase { id, password } => {
            let library = tokio::task::spawn_blocking(move || {
                crate::databases::switch(&store, &id, &password)?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Token {
            name,
            password,
            token,
        } => {
            let library = tokio::task::spawn_blocking(move || {
                admin::update_token(&store, &name, &password, token)?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::RenameCategory {
            id,
            title,
            password,
        } => {
            let library = tokio::task::spawn_blocking(move || {
                crate::library::rename_category(&store, &password, &id, &title)?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Category {
            title,
            parent,
            password,
        } => {
            let library = tokio::task::spawn_blocking(move || {
                crate::library::create_category(&store, &password, &title, parent.as_deref())?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Move {
            id,
            target,
            password,
        } => {
            let library = tokio::task::spawn_blocking(move || {
                crate::library::move_item(&store, &password, &id, &target)?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Library(password) => {
            let library =
                tokio::task::spawn_blocking(move || crate::library::read(&store, &password))
                    .await
                    .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Add {
            options,
            password,
            confirmation,
            token,
        } => {
            let name = options.name.clone();
            let target = store.clone();
            let setup_password = password.clone();
            let path = tokio::task::spawn_blocking(move || {
                admin::quick_add(
                    &target,
                    &options,
                    &setup_password,
                    confirmation.as_deref().map(String::as_str),
                    token,
                )
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            // A bind/unlock failure must not hide that setup already succeeded.
            let broker = BrokerSession::start(store, password).await;
            Ok(Outcome::Added { name, path, broker })
        }
        Action::Note {
            name,
            note,
            password,
        } => {
            tokio::task::spawn_blocking(move || {
                admin::update_note(&store, &name, &note, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Message(Message::NoteUpdatedHint))
        }
        Action::Init {
            path,
            port,
            password,
            confirmation,
        } => {
            tokio::task::spawn_blocking(move || {
                admin::initialize(&store, &path, port, &password, &confirmation)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Message(Message::VaultCreatedHint))
        }
        Action::OpenLocal { path, password } => {
            let count =
                tokio::task::spawn_blocking(move || sync::open_local(&store, &path, &password))
                    .await
                    .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(opened(count))
        }
        Action::Connect {
            category,
            name,
            provider,
            base,
            note,
            token,
            password,
        } => {
            let library = tokio::task::spawn_blocking(move || {
                admin::add_connection_in_category(
                    &store,
                    admin::NewConnection {
                        name: &name,
                        provider,
                        base: &base,
                        note: &note,
                        category: category.as_deref(),
                    },
                    &password,
                    token,
                )?;
                crate::library::read(&store, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Library(library))
        }
        Action::Grant { options, password } => {
            let name = options.name.clone();
            let path = tokio::task::spawn_blocking(move || {
                admin::issue_grant(&store, &options, &password)
            })
            .await
            .map_err(|_| GatewayError::StateUnavailable)??;
            Ok(Outcome::Grant { name, path })
        }
        Action::Revoke(name) => {
            admin::revoke(&store, &name)?;
            Ok(Outcome::Revoked(name))
        }
        Action::Unlock(password) => BrokerSession::start(store, password)
            .await
            .map(Outcome::Broker),
        Action::Lock => Ok(Outcome::Message(Message::BrokerLockedHint)),
        Action::Login { profile, password } => {
            login(&store, WebDavClient::new(profile, password)?).await
        }
        Action::Browse(path) => {
            let entries = webdav
                .ok_or(GatewayError::WebDavUnauthorized)?
                .list(&path)
                .await?;
            Ok(Outcome::Browse { path, entries })
        }
        Action::OpenRemote { path, password } => {
            let client = webdav.ok_or(GatewayError::WebDavUnauthorized)?;
            Ok(opened(
                sync::open_remote(&store, &client, &path, &password).await?,
            ))
        }
        Action::Publish { path, password } => {
            let client = webdav.ok_or(GatewayError::WebDavUnauthorized)?;
            let result = sync::publish(&store, &client, &path, &password).await?;
            Ok(Outcome::Synced(result))
        }
        Action::Sync(password) => {
            let client = webdav.ok_or(GatewayError::WebDavUnauthorized)?;
            let result = sync::synchronize(&store, &client, &password).await?;
            Ok(Outcome::Synced(result))
        }
        Action::Probe(path) => {
            let config: ClientConfig = read_json(&path, 16 * 1024)?;
            let tools = McpBridge::new(config)?.discover().await?;
            Ok(Outcome::Probe(
                tools
                    .into_iter()
                    .map(|tool| {
                        let repositories = tool
                            .input_schema
                            .get("properties")
                            .and_then(|value| value.get("repository"))
                            .and_then(|value| value.get("enum"))
                            .map_or_else(String::new, ToString::to_string);
                        format!("{}  {}", tool.name, repositories)
                    })
                    .collect(),
            ))
        }
    }
}

fn opened(count: usize) -> Outcome {
    Outcome::Opened(count)
}
