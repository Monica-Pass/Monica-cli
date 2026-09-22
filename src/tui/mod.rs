//! The human terminal owns secrets and broker lifetime. MCP remains a separate
//! stdio entry point and never initializes this terminal or its input state.
mod actions;
mod browser;
mod form;
mod fuzzy;
mod home;
mod preview;
mod view;

use std::io::{IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use zeroize::Zeroizing;

use actions::{Action, Outcome};
use browser::{Filter, Focus, quick_command};
use form::{Form, FormEvent, Input, Kind};

use crate::admin::{self, BrokerSession};
use crate::config::{Config, ConfigStore, Grant, GrantState, write_json};
use crate::credstore;
use crate::error::{GatewayError, Result};
use crate::i18n::{self, Language, LanguageChoice, Message, Preferences};
use crate::tr;
use crate::webdav::{RemoteEntry, WebDavClient, WebDavProfile};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Connections,
    Grants,
    WebDav,
    Help,
}

impl Page {
    const ALL: [Self; 5] = [
        Self::Dashboard,
        Self::Connections,
        Self::Grants,
        Self::WebDav,
        Self::Help,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap_or(0)
    }
    fn label(self, lang: Language) -> &'static str {
        match self {
            Self::Dashboard => tr!(lang, PageOverview),
            Self::Connections => tr!(lang, PageConnections),
            Self::Grants => tr!(lang, PageGrants),
            Self::WebDav => "WebDAV",
            Self::Help => tr!(lang, PageCommands),
        }
    }
}

impl Page {
    fn short_label(self, lang: Language) -> &'static str {
        match self {
            Self::Dashboard => tr!(lang, ShortOverview),
            Self::Connections => tr!(lang, ShortConnections),
            Self::Grants => tr!(lang, ShortGrants),
            Self::WebDav => tr!(lang, ShortWebDav),
            Self::Help => tr!(lang, ShortCommands),
        }
    }
}

struct CommandHelp {
    command: &'static str,
    key: &'static str,
    description: Message,
    cli: Message,
}

const COMMANDS: &[CommandHelp] = &[
    CommandHelp {
        command: "add",
        key: "c",
        description: Message::CommandAddDescription,
        cli: Message::CommandAddCli,
    },
    CommandHelp {
        command: "note",
        key: "e",
        description: Message::CommandNoteDescription,
        cli: Message::CommandNoteCli,
    },
    CommandHelp {
        command: "init",
        key: "n",
        description: Message::CommandInitDescription,
        cli: Message::CommandInitCli,
    },
    CommandHelp {
        command: "open",
        key: "o",
        description: Message::CommandOpenDescription,
        cli: Message::CommandOpenCli,
    },
    CommandHelp {
        command: "connect",
        key: "C",
        description: Message::CommandConnectDescription,
        cli: Message::CommandConnectCli,
    },
    CommandHelp {
        command: "ssh",
        key: "S",
        description: Message::CommandSshDescription,
        cli: Message::CommandSshCli,
    },
    CommandHelp {
        command: "sshimport",
        key: "I",
        description: Message::CommandSshImportDescription,
        cli: Message::CommandSshImportCli,
    },
    CommandHelp {
        command: "gpg",
        key: "A",
        description: Message::CommandGpgDescription,
        cli: Message::CommandGpgCli,
    },
    CommandHelp {
        command: "grant",
        key: "a",
        description: Message::CommandGrantDescription,
        cli: Message::CommandGrantCli,
    },
    CommandHelp {
        command: "revoke",
        key: "x",
        description: Message::CommandRevokeDescription,
        cli: Message::CommandRevokeCli,
    },
    CommandHelp {
        command: "unlock",
        key: "u",
        description: Message::CommandUnlockDescription,
        cli: Message::CommandUnlockCli,
    },
    CommandHelp {
        command: "lock",
        key: "L",
        description: Message::CommandLockDescription,
        cli: Message::CommandLockCli,
    },
    CommandHelp {
        command: "login",
        key: "i",
        description: Message::CommandLoginDescription,
        cli: Message::CommandLoginCli,
    },
    CommandHelp {
        command: "logout",
        key: "",
        description: Message::CommandLogoutDescription,
        cli: Message::CommandLogoutCli,
    },
    CommandHelp {
        command: "webdav",
        key: "4",
        description: Message::CommandWebdavDescription,
        cli: Message::CommandWebdavCli,
    },
    CommandHelp {
        command: "publish",
        key: "P",
        description: Message::CommandPublishDescription,
        cli: Message::CommandPublishCli,
    },
    CommandHelp {
        command: "sync",
        key: "s",
        description: Message::CommandSyncDescription,
        cli: Message::CommandSyncCli,
    },
    CommandHelp {
        command: "mcp",
        key: "m",
        description: Message::CommandMcpDescription,
        cli: Message::CommandMcpCli,
    },
    CommandHelp {
        command: "check",
        key: "p",
        description: Message::CommandCheckDescription,
        cli: Message::CommandCheckCli,
    },
    CommandHelp {
        command: "refresh",
        key: "r",
        description: Message::CommandRefreshDescription,
        cli: Message::CommandRefreshCli,
    },
    CommandHelp {
        command: "help",
        key: "?",
        description: Message::CommandHelpDescription,
        cli: Message::CommandHelpCli,
    },
    CommandHelp {
        command: "commands",
        key: "5",
        description: Message::CommandCommandsDescription,
        cli: Message::CommandCommandsCli,
    },
    CommandHelp {
        command: "message",
        key: "!",
        description: Message::CommandMessageDescription,
        cli: Message::CommandMessageCli,
    },
    CommandHelp {
        command: "icons",
        key: "",
        description: Message::CommandIconsDescription,
        cli: Message::CommandIconsCli,
    },
    CommandHelp {
        command: "lang",
        key: "F2",
        description: Message::CommandLanguageDescription,
        cli: Message::CommandLanguageCli,
    },
    CommandHelp {
        command: "quit",
        key: "q",
        description: Message::CommandQuitDescription,
        cli: Message::CommandQuitCli,
    },
];

struct Popup {
    title: String,
    lines: Vec<String>,
    scroll: usize,
    max_scroll: usize,
    client: Option<PathBuf>,
}
enum Mode {
    Normal,
    Command(Input),
    Filter(Filter),
    Form(Form),
    Popup(Popup),
}

struct App {
    databases: Vec<crate::databases::Database>,
    home: bool,
    home_rail_selected: usize,
    home_rail_offset: usize,
    home_offset: usize,
    home_filter: Zeroizing<String>,
    library: Option<crate::library::Library>,
    keys: Vec<crate::vault::KeyEntrySummary>,
    library_loaded: Option<Instant>,
    category: Option<String>,
    home_selected: usize,
    home_restore: Option<(Option<String>, Option<String>)>,
    store: ConfigStore,
    usage: std::collections::BTreeMap<String, u32>,
    language: Language,
    config: Option<Config>,
    profile: Option<WebDavProfile>,
    webdav: Option<WebDavClient>,
    folder: String,
    entries: Vec<RemoteEntry>,
    broker: Option<BrokerSession>,
    external_busy: bool,
    page: Page,
    selected: [usize; 5],
    list_offsets: [usize; 5],
    filters: [Zeroizing<String>; 5],
    focus: Focus,
    preview_scroll: usize,
    preview_max_scroll: usize,
    page_size: usize,
    mode: Mode,
    pending: Option<tokio::task::JoinHandle<Result<Outcome>>>,
    pending_label: Message,
    message: String,
    message_at: Option<Instant>,
    nerd_font: bool,
    failed: bool,
    quitting: bool,
    last_refresh: Instant,
}

impl App {
    fn new(store: ConfigStore, language: Language) -> Self {
        let lang = language;
        let mut app = Self {
            databases: Vec::new(),
            home: true,
            home_rail_selected: 0,
            home_rail_offset: 0,
            home_offset: 0,
            home_filter: Zeroizing::new(String::new()),
            library: None,
            keys: Vec::new(),
            library_loaded: None,
            category: None,
            home_selected: 0,
            home_restore: None,
            store,
            usage: std::collections::BTreeMap::new(),
            language,
            config: None,
            profile: None,
            webdav: None,
            folder: String::new(),
            entries: Vec::new(),
            broker: None,
            external_busy: false,
            page: Page::Dashboard,
            selected: [0; 5],
            list_offsets: [0; 5],
            filters: std::array::from_fn(|_| Zeroizing::new(String::new())),
            focus: Focus::List,
            preview_scroll: 0,
            preview_max_scroll: 0,
            page_size: 10,
            mode: Mode::Normal,
            pending: None,
            pending_label: Message::Welcome,
            message: tr!(lang, Welcome).to_owned(),
            message_at: None,
            nerd_font: std::env::var("MONICA_TUI_ICONS").map_or(true, |value| value != "plain"),
            failed: false,
            quitting: false,
            last_refresh: Instant::now(),
        };
        app.reload();
        match crate::databases::list(&app.store) {
            Ok(databases) => app.databases = databases,
            Err(error) => app.error(error),
        }
        // Keep the landing page stable. The overview is the user's workspace;
        // management and AI authorization are entered deliberately from it.
        if app
            .config
            .as_ref()
            .is_some_and(|config| !config.connections.is_empty())
        {
            app.info(tr!(lang, ConnectionsLoaded));
        }
        match WebDavProfile::load(&app.store) {
            Ok(profile) => app.profile = profile,
            Err(error) => app.error(error),
        }
        // A password remembered on this computer lets the TUI start already logged
        // in. The vault master password is still asked for on every action.
        let profile = app.profile.clone();
        if let Some(profile) = profile
            && let Some(password) = credstore::load(&profile.base_url, &profile.username)
            && let Ok(client) = WebDavClient::new(profile, password)
        {
            app.webdav = Some(client);
        }
        if !app.failed {
            app.message_at = None;
        }
        app
    }

    fn reload(&mut self) {
        let selection = Page::ALL.map(|page| self.selection_key(page));
        if self.store.path.exists() {
            match self.store.load() {
                Ok(config) => self.config = Some(config),
                Err(error) => {
                    self.config = None;
                    self.error(error);
                }
            }
        } else {
            self.config = None;
        }
        // The usage ledger needs neither the master password nor the broker lock,
        // so grant budgets stay readable while the vault is closed. A failed read
        // keeps the previous values instead of blanking the panel for one tick.
        if let Ok(usage) = crate::gateway::load_call_usage(&self.store) {
            self.usage = usage;
        }
        for (page, key) in Page::ALL.into_iter().zip(selection) {
            self.restore_selection(page, key.as_deref());
        }
        self.last_refresh = Instant::now();
        self.external_busy = self.config.is_some()
            && self.broker.is_none()
            && self.pending.is_none()
            && matches!(
                self.store.acquire_broker_lock(),
                Err(GatewayError::BrokerAlreadyRunning)
            );
    }

    fn error(&mut self, error: GatewayError) {
        self.message = self.language.error(error).to_owned();
        self.message_at = Some(Instant::now());
        self.failed = true;
    }
    fn info(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.message_at = Some(Instant::now());
        self.failed = false;
    }
    /// A plain message that stays on the status line, the way an error does.
    fn warning(&mut self, message: impl Into<String>) {
        self.message = message.into();
        self.message_at = Some(Instant::now());
        self.failed = true;
    }
    fn selected_connection(&self) -> Option<String> {
        if self.page == Page::Grants {
            if let Some(grant) = self.selected_grant() {
                return Some(grant.connection.clone());
            }
            if !self.filters[Page::Grants.index()].is_empty() {
                return None;
            }
        }
        let config = self.config.as_ref()?;
        config
            .connections
            .keys()
            .nth(self.selected_index(Page::Connections)?)
            .cloned()
    }
    fn selected_grant(&self) -> Option<&Grant> {
        self.config
            .as_ref()?
            .grants
            .get(self.selected_index(Page::Grants)?)
    }
    /// One source of truth for "is this grant still serving the AI", shared with
    /// `admin::status` and the gateway so the terminal cannot advertise a refused
    /// authorization as usable.
    fn grant_state(&self, grant: &Grant) -> GrantState {
        crate::config::grant_state(grant, &self.usage, chrono::Utc::now().timestamp())
    }
    /// Authorizations currently serving the AI: not pending, not expired, budget unspent.
    fn grants_in_force(&self) -> usize {
        let Some(config) = self.config.as_ref() else {
            return 0;
        };
        config
            .grants
            .iter()
            .filter(|grant| {
                let state = self.grant_state(grant);
                !state.pending && !state.refresh_required()
            })
            .count()
    }
    fn grant_client(&self) -> Result<(String, PathBuf)> {
        let grant = self.selected_grant().ok_or(GatewayError::NotFound)?;
        let path = match &grant.client_file {
            Some(path) => path.clone(),
            None => admin::client_path(&self.store, &grant.name)?,
        };
        Ok((grant.name.clone(), path))
    }
    fn start(&mut self, action: Action) {
        if self.pending.is_some() {
            return;
        }
        if action.pauses_broker() {
            if matches!(
                action,
                Action::Connect { .. }
                    | Action::Token { .. }
                    | Action::Category { .. }
                    | Action::RenameCategory { .. }
                    | Action::RenameEntry { .. }
                    | Action::Move { .. }
            ) {
                let row = self
                    .home_rows()
                    .get(self.home_selected)
                    .map(|row| row.id().to_owned());
                self.home_restore = Some((self.category.clone(), row));
            } else {
                self.home_restore = None;
            }
            self.library = None;
            self.keys.clear();
            self.library_loaded = None;
        }
        self.pending_label = action.label();
        self.failed = false;
        let broker = if action.pauses_broker() {
            self.broker.take()
        } else {
            None
        };
        let store = self.store.clone();
        let webdav = self.webdav.clone();
        self.pending = Some(tokio::spawn(async move {
            if let Some(broker) = broker {
                broker.stop().await?;
            }
            actions::perform(store, action, webdav).await
        }));
    }

    fn change_language(&mut self, choice: LanguageChoice) {
        if let Err(error) = (Preferences { language: choice }).save(&self.store) {
            self.error(error);
            return;
        }
        let selections = Page::ALL.map(|page| self.selection_key(page));
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.language = choice.resolve(i18n::system_language());
        self.preview_scroll = 0;
        for (page, selected) in Page::ALL.into_iter().zip(selections) {
            self.restore_selection(page, selected.as_deref());
        }
        self.mode = match mode {
            Mode::Form(mut previous) => {
                let mut form = Form::new(previous.kind.clone(), self);
                // Move buffers, including Zeroizing secrets, rather than copying
                // user input into a translation template or reconstructing values.
                for (field, old) in form.fields.iter_mut().zip(&mut previous.fields) {
                    std::mem::swap(&mut field.input, &mut old.input);
                }
                form.selected = previous.selected;
                form.insert = previous.insert;
                form.authenticating = previous.authenticating;
                Mode::Form(form)
            }
            Mode::Popup(_) => Mode::Normal,
            mode => mode,
        };
        self.info(tr!(
            self.language,
            LanguageSaved,
            language = self.language.name()
        ));
    }

    fn show_form(&mut self, kind: Kind) {
        self.failed = false;
        self.message_at = None;
        self.mode = Mode::Form(Form::new(kind, self));
    }

    fn show_help(&mut self) {
        let lang = self.language;
        let mut lines = Vec::new();
        if self.home {
            lines.extend([
                tr!(lang, HelpHomeRail).to_owned(),
                tr!(lang, HelpHomeEnter).to_owned(),
                tr!(lang, HelpHomeSearch).to_owned(),
                tr!(lang, HelpHomeNew).to_owned(),
                tr!(lang, HelpHomeEdit).to_owned(),
                tr!(lang, HelpHomeKeys).to_owned(),
                tr!(lang, HelpHomeCopy).to_owned(),
            ]);
        }
        let commands: &[&str] = match self.page {
            Page::Dashboard => &["add", "open", "login", "grant", "unlock"],
            Page::Connections => &["add", "note", "grant", "connect", "unlock", "lock"],
            Page::Grants => &["grant", "mcp", "check", "revoke", "unlock", "lock"],
            Page::WebDav => &["login", "publish", "sync", "refresh", "logout"],
            Page::Help => &["commands", "message", "icons"],
        };
        if !self.home {
            for name in commands {
                if let Some(command) = COMMANDS.iter().find(|command| command.command == *name) {
                    let key = if command.key.is_empty() {
                        format!(":{}", command.command)
                    } else {
                        command.key.to_owned()
                    };
                    lines.push(format!("{key:<10} {}", lang.text(command.description)));
                }
            }
        }
        lines.extend([
            String::new(),
            if self.home {
                tr!(lang, HelpHomeManage).to_owned()
            } else {
                tr!(lang, HelpEnter).to_owned()
            },
            tr!(lang, HelpPanes).to_owned(),
            tr!(lang, HelpMove).to_owned(),
            tr!(lang, HelpPaging).to_owned(),
            tr!(lang, HelpTab).to_owned(),
            if self.home {
                tr!(lang, HelpHomeEscape).to_owned()
            } else {
                tr!(lang, HelpSections).to_owned()
            },
            tr!(lang, HelpSearch).to_owned(),
            tr!(lang, HelpCommand).to_owned(),
            tr!(lang, HelpMessage).to_owned(),
            tr!(lang, HelpQuit).to_owned(),
            String::new(),
            tr!(lang, HelpForm).to_owned(),
            tr!(lang, HelpFormEscape).to_owned(),
            tr!(lang, HelpIcons).to_owned(),
            tr!(lang, HelpLanguage).to_owned(),
        ]);
        self.mode = Mode::Popup(Popup {
            title: tr!(
                lang,
                HelpTitle,
                page = if self.home {
                    lang.text(Message::PageHome)
                } else {
                    self.page.label(lang)
                }
            )
            .to_owned(),
            lines,
            scroll: 0,
            max_scroll: 0,
            client: None,
        });
    }

    fn show_message(&mut self) {
        let lang = self.language;
        self.mode = Mode::Popup(Popup {
            title: if self.failed {
                tr!(lang, FailedTitle)
            } else {
                tr!(lang, ResultTitle)
            }
            .to_owned(),
            lines: self.message.lines().map(str::to_owned).collect(),
            scroll: 0,
            max_scroll: 0,
            client: None,
        });
    }

    fn invoke(&mut self, command: &str) {
        let lang = self.language;
        let parts: Vec<_> = command.split_whitespace().collect();
        if matches!(parts.first(), Some(&"lang" | &"language")) {
            match parts.as_slice() {
                [_] => self.change_language(lang.other().choice()),
                [_, value] => match LanguageChoice::parse(value) {
                    Some(choice) => self.change_language(choice),
                    None => self.info(tr!(lang, LanguageInvalid)),
                },
                _ => self.info(tr!(lang, LanguageInvalid)),
            }
            return;
        }
        if self.pending.is_some()
            && !matches!(
                command,
                "help" | "commands" | "message" | "icons" | "q" | "q!" | "quit"
            )
        {
            self.info(tr!(lang, OperationPending));
            return;
        }
        match command {
            "add" => self.show_form(Kind::Add {
                new_vault: self.config.is_none(),
            }),
            "init" => {
                if self.config.is_some() {
                    self.error(GatewayError::AlreadyExists);
                } else {
                    self.show_form(Kind::Init);
                }
            }
            "open" => self.show_form(Kind::OpenLocal),
            "connect" | "note" | "grant" | "unlock" | "publish" | "sync" | "ssh" | "sshimport"
            | "gpg"
                if self.config.is_none() =>
            {
                self.error(GatewayError::NotFound)
            }
            "ssh" | "sshimport" | "gpg" if self.library.is_none() => {
                self.info(tr!(lang, KeyNeedsOpenedDatabase));
                self.show_form(Kind::Library);
            }
            "connect" => self.show_form(Kind::Connect),
            "ssh" => self.show_form(Kind::AddSsh { generate: true }),
            "sshimport" => self.show_form(Kind::AddSsh { generate: false }),
            "gpg" => self.show_form(Kind::AddGpg),
            "note" if !matches!(self.page, Page::Connections | Page::Grants) => {
                self.set_page(Page::Connections);
                self.info(tr!(lang, SelectConnectionToEdit));
            }
            "revoke" | "mcp" | "check" if self.page != Page::Grants => {
                self.set_page(Page::Grants);
                self.info(tr!(lang, SelectGrantActions));
            }
            "note" | "grant" if self.selected_connection().is_none() => {
                self.info(tr!(lang, SelectConnectionFirst));
            }
            "note" => self.show_form(Kind::Note),
            "grant" => self.show_form(Kind::Grant),
            "revoke" if self.selected_grant().is_none() => {
                self.info(tr!(lang, SelectGrantFirst));
            }
            "revoke" => self.show_form(Kind::Revoke),
            "unlock" => self.show_form(Kind::Unlock),
            "lock" => self.start(Action::Lock),
            "login" => self.show_form(Kind::Login),
            "logout" => {
                self.webdav = None;
                self.entries.clear();
                self.info(tr!(lang, WebDavLoggedOut));
            }
            "publish" | "sync" if self.webdav.is_none() => {
                self.info(tr!(lang, WebDavLoginFirst));
                self.show_form(Kind::Login);
            }
            "publish" => self.show_form(Kind::Publish),
            "sync" => {
                if self
                    .config
                    .as_ref()
                    .is_some_and(|config| config.webdav.is_some())
                {
                    self.show_form(Kind::Sync);
                } else {
                    self.error(GatewayError::RemoteNotConfigured);
                }
            }
            "webdav" => self.set_page(Page::WebDav),
            "connections" => self.set_page(Page::Connections),
            "grants" => self.set_page(Page::Grants),
            "dashboard" | "status" => self.set_page(Page::Dashboard),
            "commands" => self.set_page(Page::Help),
            "help" => self.show_help(),
            "message" => self.show_message(),
            "icons" => {
                self.nerd_font = !self.nerd_font;
                self.info(if self.nerd_font {
                    tr!(lang, NerdIconsEnabled)
                } else {
                    tr!(lang, PlainIconsEnabled)
                });
            }
            "mcp" => match self.grant_client() {
                Ok((name, path)) => self.show_mcp(&name, path),
                Err(error) => self.error(error),
            },
            "check" => match self.grant_client() {
                Ok((_, path)) => self.start(Action::Probe(path)),
                Err(error) => self.error(error),
            },
            "refresh" => {
                self.reload();
                if self.page == Page::WebDav && self.webdav.is_some() {
                    self.start(Action::Browse(self.folder.clone()));
                }
            }
            "q" | "q!" | "quit" => self.quitting = true,
            _ => self.info(tr!(lang, UnknownCommand)),
        }
        self.clamp_selection();
    }

    fn enter(&mut self) {
        let lang = self.language;
        if self.page == Page::WebDav && self.webdav.is_none() {
            self.invoke("login");
            return;
        }
        let Some(index) = self.selected_index(self.page) else {
            self.info(tr!(lang, NoActionableRow));
            return;
        };
        match self.page {
            Page::Dashboard => {
                if let Some(command) = quick_command(index) {
                    self.invoke(command.command);
                }
            }
            Page::Connections => self.invoke("grant"),
            Page::Grants => self.invoke("mcp"),
            Page::Help => self.invoke(COMMANDS[index].command),
            Page::WebDav => {
                if let Some(entry) = self.entries.get(index) {
                    let path = entry.path.clone();
                    if entry.is_directory {
                        self.start(Action::Browse(path));
                    } else if path.to_ascii_lowercase().ends_with(".mdbx") {
                        self.show_form(Kind::OpenRemote(path));
                    } else {
                        self.info(tr!(lang, ChooseVaultOrFolder));
                    }
                }
            }
        }
    }

    fn show_mcp(&mut self, name: &str, path: PathBuf) {
        let lang = self.language;
        let result = (|| {
            let value = admin::mcp_settings(name, &path)?;
            let output = path.with_extension("mcp.json");
            write_json(&output, &value, true)?;
            let pretty =
                serde_json::to_string_pretty(&value).map_err(|_| GatewayError::StateUnavailable)?;
            let mut lines = vec![
                tr!(lang, McpCopySettings).to_owned(),
                tr!(lang, McpSaved, path = output.display()),
                if self.broker.is_some() {
                    tr!(lang, McpUnlockedHint).to_owned()
                } else {
                    tr!(lang, McpUnlockHint).to_owned()
                },
                String::new(),
            ];
            lines.extend(pretty.lines().map(str::to_owned));
            Ok(Popup {
                title: format!("MCP · {name}"),
                lines,
                scroll: 0,
                max_scroll: 0,
                client: Some(path),
            })
        })();
        match result {
            Ok(popup) => self.mode = Mode::Popup(popup),
            Err(error) => self.error(error),
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if key.code == KeyCode::F(2) && key.modifiers.is_empty() {
            self.change_language(self.language.other().choice());
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = Mode::Normal;
            self.quitting = true;
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Filter(mut filter) => match key.code {
                KeyCode::Esc => {
                    if self.home {
                        self.update_home_filter(&filter.previous);
                        self.restore_home_selection(filter.selection.as_deref());
                    } else {
                        self.update_filter(&filter.previous);
                        self.restore_selection(self.page, filter.selection.as_deref());
                    }
                }
                KeyCode::Enter => {}
                _ => {
                    filter.input.key(key);
                    if self.home {
                        self.update_home_filter(&filter.input.value);
                    } else {
                        self.update_filter(&filter.input.value);
                    }
                    self.mode = Mode::Filter(filter);
                }
            },
            Mode::Form(mut form) => match form.key(key) {
                FormEvent::Cancel => self.info(tr!(self.language, FormCancelled)),
                FormEvent::None => self.mode = Mode::Form(form),
                FormEvent::Submit => match form.action(self) {
                    Ok(action) => self.start(action),
                    Err(error) => {
                        self.error(error);
                        self.mode = Mode::Form(form);
                    }
                },
            },
            Mode::Command(mut input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.invoke(input.value.trim()),
                KeyCode::Char(ch) if ch.is_ascii_lowercase() || ch == '-' || ch == '!' => {
                    input.key(key);
                    self.mode = Mode::Command(input);
                }
                KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End => {
                    input.key(key);
                    self.mode = Mode::Command(input);
                }
                _ => self.mode = Mode::Command(input),
            },
            Mode::Popup(mut popup) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {}
                KeyCode::Char('j') | KeyCode::Down => {
                    popup.scroll = popup.scroll.saturating_add(1).min(popup.max_scroll);
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    popup.scroll = popup.scroll.saturating_sub(1);
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    popup.scroll = 0;
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('G') | KeyCode::End => {
                    popup.scroll = popup.max_scroll;
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::PageDown => {
                    popup.scroll = popup
                        .scroll
                        .saturating_add(self.page_size)
                        .min(popup.max_scroll);
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::PageUp => {
                    popup.scroll = popup.scroll.saturating_sub(self.page_size);
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    popup.scroll = popup
                        .scroll
                        .saturating_add((self.page_size / 2).max(1))
                        .min(popup.max_scroll);
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    popup.scroll = popup.scroll.saturating_sub((self.page_size / 2).max(1));
                    self.mode = Mode::Popup(popup);
                }
                KeyCode::Char('p') if popup.client.is_some() => {
                    if let Some(path) = popup.client {
                        self.start(Action::Probe(path));
                    }
                }
                KeyCode::Char('u') if popup.client.is_some() => self.invoke("unlock"),
                _ => self.mode = Mode::Popup(popup),
            },
            Mode::Normal => self.normal_key(key),
        }
    }

    fn normal_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::F(3) {
            self.home = !self.home;
            return;
        }
        if self.home {
            self.home_key(key);
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.move_selection(true, (self.page_size / 2).max(1)),
                KeyCode::Char('u') => self.move_selection(false, (self.page_size / 2).max(1)),
                _ => {}
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quitting = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help(),
            KeyCode::Char(':') => self.mode = Mode::Command(Input::new("", 32)),
            KeyCode::Char('/') => self.begin_filter(),
            KeyCode::Esc => {
                self.message_at = None;
                self.update_filter("");
                self.focus = Focus::List;
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(true, 1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(false, 1),
            KeyCode::PageDown => self.move_selection(true, self.page_size),
            KeyCode::PageUp => self.move_selection(false, self.page_size),
            KeyCode::Char('g') | KeyCode::Home => self.move_selection(false, usize::MAX),
            KeyCode::Char('G') | KeyCode::End => self.move_selection(true, usize::MAX),
            KeyCode::Char('h') | KeyCode::Left
                if self.focus == Focus::List
                    && self.page == Page::WebDav
                    && !self.folder.is_empty() =>
            {
                if self.pending.is_none() {
                    self.start(Action::Browse(
                        self.folder
                            .rsplit_once('/')
                            .map_or("", |(parent, _)| parent)
                            .to_owned(),
                    ));
                }
            }
            KeyCode::Char('l') | KeyCode::Right
                if self.focus == Focus::List
                    && self.page == Page::WebDav
                    && self
                        .selected_index(Page::WebDav)
                        .and_then(|index| self.entries.get(index))
                        .is_some_and(|entry| entry.is_directory) =>
            {
                self.enter()
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = match self.focus {
                    Focus::Preview => Focus::List,
                    _ => Focus::Navigation,
                };
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = match self.focus {
                    Focus::Navigation => Focus::List,
                    _ => Focus::Preview,
                };
            }
            KeyCode::Tab => self.shift_focus(true),
            KeyCode::BackTab => self.shift_focus(false),
            KeyCode::Char(ch @ '1'..='5') => self.set_page(Page::ALL[ch as usize - '1' as usize]),
            KeyCode::Enter if self.focus != Focus::List => self.focus = Focus::List,
            KeyCode::Enter if self.pending.is_none() => self.enter(),
            KeyCode::F(5) => self.invoke("refresh"),
            KeyCode::Char(ch) => {
                if let Some(command) = COMMANDS
                    .iter()
                    .find(|command| command.key == ch.to_string())
                {
                    self.invoke(command.command);
                }
            }
            _ => {}
        }
        self.clamp_selection();
    }

    fn apply(&mut self, outcome: Outcome) {
        let lang = self.language;
        self.reload();
        match crate::databases::list(&self.store) {
            Ok(databases) => self.databases = databases,
            Err(error) => self.error(error),
        }
        match outcome {
            Outcome::Library {
                library,
                keys,
                saved,
            } => {
                self.library = Some(library);
                self.keys = keys;
                self.library_loaded = Some(Instant::now());
                self.category = None;
                self.home_selected = 0;
                self.home_offset = 0;
                self.focus = Focus::List;
                self.preview_scroll = 0;
                if let Some(saved) = saved {
                    let (summary, created) = *saved;
                    self.home_restore = Some((Some(summary.collection_id), Some(summary.entry_id)));
                    let name = summary.title;
                    self.info(if created {
                        tr!(
                            lang,
                            CliKeyStored,
                            name = name,
                            fingerprint = summary.fingerprint
                        )
                    } else {
                        tr!(lang, CliKeyEdited, name = name)
                    });
                    return;
                }
                if let Some((category, row)) = self.home_restore.take() {
                    self.category = category.filter(|id| {
                        self.library
                            .as_ref()
                            .is_some_and(|l| l.categories.iter().any(|c| &c.id == id))
                    });
                    self.restore_home_selection(row.as_deref());
                }
                self.message_at = None;
            }
            Outcome::Added { name, path, broker } => {
                self.set_page(Page::Grants);
                self.update_filter("");
                self.restore_selection(Page::Grants, Some(&name));
                match broker {
                    Ok(broker) => {
                        self.broker = Some(broker);
                        self.external_busy = false;
                        self.info(tr!(lang, AddedReady));
                    }
                    Err(error) => {
                        self.info(tr!(lang, AddedBrokerFailed, error = lang.error(error)));
                        self.failed = true;
                    }
                }
                self.show_mcp(&name, path);
            }
            Outcome::Message(message) => self.info(lang.text(message)),
            Outcome::Revoked(name) => self.info(tr!(lang, RevokedHint, name = name)),
            Outcome::Opened(count) => self.info(tr!(lang, OpenedHint, count = count)),
            Outcome::Synced(outcome) => self.info(tr!(
                lang,
                SyncFinishedHint,
                result = lang.sync_message(&outcome)
            )),
            Outcome::Broker(broker) => {
                self.broker = Some(broker);
                self.external_busy = false;
                self.info(tr!(lang, BrokerReadyHint));
            }
            Outcome::Login {
                client,
                entries,
                password_saved,
            } => {
                self.profile = Some(client.profile.clone());
                self.webdav = Some(client);
                self.entries = entries;
                self.folder.clear();
                self.set_page(Page::WebDav);
                self.update_filter("");
                self.selected[3] = 0;
                self.info(if password_saved {
                    tr!(lang, WebDavLoginReady)
                } else {
                    tr!(lang, WebDavLoginPasswordNotSaved)
                });
            }
            Outcome::Browse { path, entries } => {
                let selection = self.selection_key(Page::WebDav);
                let same_folder = self.folder == path;
                self.folder = path;
                self.entries = entries;
                if same_folder {
                    self.restore_selection(Page::WebDav, selection.as_deref());
                } else {
                    self.filters[Page::WebDav.index()] = Zeroizing::new(String::new());
                    self.selected[3] = 0;
                }
                self.preview_scroll = 0;
                self.info(tr!(lang, RemoteRefreshed));
            }
            Outcome::Grant { name, path } => {
                self.set_page(Page::Grants);
                self.update_filter("");
                self.restore_selection(Page::Grants, Some(&name));
                self.info(tr!(lang, GrantCreatedHint));
                self.show_mcp(&name, path);
            }
            Outcome::Probe(tools) => {
                let mut lines = vec![
                    tr!(lang, ProbeCount, count = tools.len()),
                    tr!(lang, ProbeNoUpstream).to_owned(),
                    String::new(),
                ];
                lines.extend(tools);
                self.mode = Mode::Popup(Popup {
                    title: tr!(lang, ProbeTitle).to_owned(),
                    lines,
                    scroll: 0,
                    max_scroll: 0,
                    client: None,
                });
                self.info(tr!(lang, ProbeSuccess));
            }
        }
    }

    async fn poll(&mut self) {
        if self
            .library_loaded
            .is_some_and(|time| time.elapsed() >= Duration::from_secs(300))
        {
            self.library = None;
            self.keys.clear();
            self.library_loaded = None;
        }
        let lang = self.language;
        if self.pending.as_ref().is_some_and(|job| job.is_finished()) {
            if let Some(job) = self.pending.take() {
                match job.await {
                    Ok(Ok(outcome)) => self.apply(outcome),
                    Ok(Err(error)) => self.error(error),
                    Err(_) => self.error(GatewayError::StateUnavailable),
                }
            }
            self.reload();
        }
        if self.broker.as_ref().is_some_and(BrokerSession::is_finished)
            && let Some(broker) = self.broker.take()
        {
            match broker.stop().await {
                Ok(()) => self.info(tr!(lang, BrokerExpiredHint)),
                Err(error) => self.error(error),
            }
        }
        if self.last_refresh.elapsed() >= Duration::from_secs(1) {
            self.reload();
        }
        if self.quitting && self.pending.is_none() && self.broker.is_some() {
            self.start(Action::Lock);
        }
    }

    async fn finish(&mut self) -> Result<()> {
        if let Some(job) = self.pending.take()
            && let Ok(Ok(outcome)) = job.await
        {
            self.apply(outcome);
        }
        if let Some(broker) = self.broker.take() {
            broker.stop().await?;
        }
        self.webdav = None;
        self.mode = Mode::Normal;
        Ok(())
    }
}

struct ScreenGuard;
impl ScreenGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|_| GatewayError::HumanTerminalRequired)?;
        let guard = Self;
        execute!(
            std::io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            Hide
        )
        .map_err(|_| GatewayError::HumanTerminalRequired)?;
        Ok(guard)
    }
}
fn restore_screen() {
    let _ = disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        DisableBracketedPaste,
        Show,
        LeaveAlternateScreen
    );
}
impl Drop for ScreenGuard {
    fn drop(&mut self) {
        restore_screen();
    }
}

pub async fn run(store: ConfigStore, lang: Language) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(GatewayError::HumanTerminalRequired);
    }
    let _screen = ScreenGuard::enter()?;
    std::panic::set_hook(Box::new(move |_| {
        restore_screen();
        eprintln!("monica-pass: {}", tr!(lang, TerminalFailed));
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))
        .map_err(|_| GatewayError::HumanTerminalRequired)?;
    terminal
        .clear()
        .map_err(|_| GatewayError::HumanTerminalRequired)?;
    let mut app = App::new(store, lang);
    let result = event_loop(&mut terminal, &mut app).await;
    let shutdown = app.finish().await;
    result.and(shutdown)
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    loop {
        app.poll().await;
        terminal
            .draw(|frame| view::render(frame, app))
            .map_err(|_| GatewayError::HumanTerminalRequired)?;
        if app.quitting && app.pending.is_none() && app.broker.is_none() {
            return Ok(());
        }
        for _ in 0..32 {
            if !event::poll(Duration::ZERO).map_err(|_| GatewayError::HumanTerminalRequired)? {
                break;
            }
            match event::read().map_err(|_| GatewayError::HumanTerminalRequired)? {
                Event::Key(key) => app.key(key),
                Event::Paste(value) => {
                    let value = Zeroizing::new(value);
                    app.paste(&value);
                }
                _ => {}
            }
        }
        tokio::select! { _ = interval.tick() => {}, _ = tokio::signal::ctrl_c() => app.quitting = true }
    }
}

#[cfg(test)]
mod tests;
