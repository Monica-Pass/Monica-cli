use std::path::PathBuf;

use clap::{Parser, Subcommand};
use monica_pass_cli::admin::{AddOptions, GrantOptions, RefreshOptions};
use monica_pass_cli::config::DEFAULT_PORT;
use monica_pass_cli::i18n::LanguageChoice;
use monica_pass_cli::model::Provider;

#[derive(Parser)]
#[command(
    name = "monica",
    visible_alias = "monica-pass",
    version,
    about = "Monica's local credential gateway for AI, backed by MDBX3"
)]
pub struct Cli {
    /// Local configuration; defaults to portable data/ or the private MonicaPass state directory.
    #[arg(short = 'C', long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Language for this run. Use the language command to save a preference.
    #[arg(short = 'l', long, global = true, value_enum, ignore_case = true)]
    pub lang: Option<LanguageChoice>,
    /// Emit stable JSON results and errors, and never prompt. Not used by MCP or TUI.
    #[arg(short = 'j', long, global = true)]
    pub json: bool,
    /// Fail if an operation needs input instead of opening a prompt.
    #[arg(long, visible_alias = "no-prompt", global = true)]
    pub non_interactive: bool,
    /// Read secret fields from one bounded JSON object on stdin, supplied by a trusted process.
    #[arg(long, global = true)]
    pub secrets_stdin: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// List the current and previously opened databases.
    #[command(visible_alias = "db")]
    Databases,
    /// Switch to a saved database ID. Requires its password; old grants stay revoked.
    Use { id: String },
    /// Replace a stored Token through secure input; revoke its old AI grants.
    Token { name: String },
    /// Rename a native MDBX category by its stable ID.
    RenameCategory { id: String, title: String },
    /// Rename an entry's display title (Chinese allowed) by its connection handle.
    RenameEntry { name: String, title: String },
    /// Manage SSH and GPG key entries in the vault; bare keys lists them.
    #[command(visible_alias = "k", hide = true)]
    Keys {
        #[command(subcommand)]
        command: Option<KeysCommand>,
    },
    /// Browse database categories and entry summaries after unlocking.
    #[command(visible_alias = "tree")]
    Library,
    /// Create a native MDBX category; optionally nest it under a category ID.
    #[command(visible_alias = "mkdir")]
    Category {
        title: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Move an entry or category into another category by ID.
    #[command(visible_alias = "mv")]
    Move { id: String, target: String },
    /// Delete a connection or entry by handle or ID; confirm by typing the target.
    #[command(visible_aliases = ["rm", "del"])]
    Delete {
        /// Connection name, or the entry ID shown by monica-pass library.
        target: String,
        /// Delete without the typed confirmation, after checking the target.
        #[arg(long = "force", id = "force_delete")]
        force: bool,
    },
    /// Remove an empty native category by ID. Never deletes what is inside it.
    #[command(visible_alias = "rmdir")]
    DeleteCategory {
        /// Existing category ID, as listed by monica-pass library.
        #[arg(id = "category_id")]
        id: String,
        /// Delete without the typed confirmation, after checking the target.
        #[arg(long = "force", id = "force_delete")]
        force: bool,
    },
    /// Show or save the interface language.
    #[command(visible_alias = "lang")]
    Language {
        #[arg(value_enum, ignore_case = true)]
        language: Option<LanguageChoice>,
    },
    /// Discover commands, aliases, parameters and required secret fields.
    #[command(visible_alias = "cmds")]
    Commands {
        /// Optional command path, such as add or webdav open.
        #[arg(value_name = "COMMAND")]
        topic: Vec<String>,
    },
    /// Open the Vim-style terminal manager (the default without a subcommand).
    #[command(visible_alias = "ui")]
    Tui,
    /// Create a named connection and scoped AI grant; create a vault on first use.
    #[command(visible_alias = "a")]
    Add {
        #[command(flatten)]
        options: AddOptions,
        /// Keep serving the broker after setup without asking for the password again.
        #[arg(short = 's', long)]
        serve: bool,
    },
    /// List connection names, services and public notes; never reveal tokens.
    #[command(visible_aliases = ["ls", "l"])]
    List,
    /// Show one named connection and its grants; never reveal tokens.
    #[command(visible_alias = "info")]
    Show { name: String },
    /// Edit a connection's public purpose note. Requires the vault password.
    #[command(visible_aliases = ["e", "edit"])]
    Note { name: String, note: String },
    /// Open an existing MDBX as a managed copy; preserve old vaults and reset grants.
    #[command(visible_alias = "o")]
    Open { vault: PathBuf },
    /// WebDAV login, browsing and encrypted vault synchronization.
    #[command(visible_alias = "dav")]
    Webdav {
        #[command(subcommand)]
        command: WebDavCommand,
    },
    /// Print and save MCP settings for an existing grant, selected by name.
    #[command(visible_aliases = ["m", "mcp-config"])]
    Settings {
        name: String,
        /// Write the entry into this AI client's own MCP configuration file.
        #[arg(long, value_name = "CLIENT", value_enum)]
        install: Option<monica_pass_cli::install::Client>,
    },
    /// Check authenticated MCP discovery using a grant name or a client file.
    #[command(visible_aliases = ["ck", "p"], group(clap::ArgGroup::new("target").required(true).args(["name", "client"])))]
    Check {
        /// Existing grant name, as shown by status.
        name: Option<String>,
        #[arg(short = 'c', long, value_name = "CLIENT_FILE")]
        client: Option<PathBuf>,
    },
    /// Create an encrypted MDBX3 vault. Requires a new master password.
    #[command(visible_alias = "n")]
    Init {
        #[arg(short = 'v', long, value_name = "NEW_FILE")]
        vault: Option<PathBuf>,
        #[arg(short = 'p', long, default_value_t = DEFAULT_PORT, value_parser = clap::value_parser!(u16).range(1024..))]
        port: u16,
    },
    /// Save a service connection. Requires a password and token through secure input.
    #[command(visible_alias = "c")]
    Connect {
        /// Native category ID; defaults to the connection collection.
        #[arg(long)]
        category: Option<String>,
        name: String,
        /// Optional human-facing display title (Chinese allowed). The name stays the AI handle.
        #[arg(long, default_value = "")]
        title: String,
        #[arg(short = 'p', long, value_enum, default_value = "github")]
        provider: Provider,
        /// HTTPS API root; omit for github.com or gitlab.com.
        #[arg(short = 'b', long)]
        api_base: Option<String>,
        /// Public purpose/context visible to authorized AI clients. Do not include secrets.
        #[arg(short = 'n', long, default_value = "")]
        note: String,
    },
    /// Authorize exact repositories and operations. Requires the vault password.
    #[command(visible_alias = "g")]
    Grant(GrantOptions),
    /// Re-authorize an existing grant with a fresh capability; the old one stops working.
    #[command(visible_alias = "rf")]
    Refresh(RefreshOptions),
    /// Execute a tool through an unlocked broker. Request file contains public ToolCall JSON.
    Call {
        /// Existing grant name.
        name: String,
        #[arg(long, value_name = "JSON_FILE")]
        request: PathBuf,
    },
    /// Revoke a grant. Subsequent calls using its capability will fail.
    #[command(visible_aliases = ["rv", "x"])]
    Revoke { name: String },
    /// Unlock and run until Ctrl+C, lock, or five-minute session expiry.
    #[command(visible_aliases = ["s", "u", "unlock"])]
    Serve,
    /// Lock the broker and wait for in-flight operations to drain.
    #[command(visible_aliases = ["lk", "L"])]
    Lock,
    /// Show connection, grant and broker metadata without revealing credentials.
    #[command(visible_alias = "st")]
    Status,
    /// Read the local gateway audit trail: which grant ran which operation, and how it ended.
    Audit {
        /// Restrict the trail to one grant name.
        #[arg(long, value_name = "GRANT")]
        grant: Option<String>,
        /// How many most recent events to return, newest first.
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=500))]
        limit: u16,
    },
    /// Start the MCP stdio bridge. Never prompts for upstream credentials.
    Mcp {
        #[arg(short = 'c', long, value_name = "CLIENT_FILE")]
        client: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum KeysCommand {
    /// Generate a new OpenSSH key or import a private PEM file as a key entry.
    #[command(group(clap::ArgGroup::new("material").required(true).args(["generate","private_key"])))]
    Ssh {
        #[arg(value_name = "NAME")]
        key_name: String,
        #[arg(long)]
        generate: Option<String>,
        #[arg(long, value_name = "FILE")]
        private_key: Option<PathBuf>,
        #[arg(long)]
        comment: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(short = 'n', long = "note", default_value = "")]
        purpose: String,
    },
    /// Import an OpenPGP certificate, and optionally the secret ring that belongs to it.
    #[command(group(clap::ArgGroup::new("armor").required(true).multiple(true).args(["public_key","private_key"])))]
    Gpg {
        #[arg(value_name = "NAME")]
        key_name: String,
        #[arg(long, value_name = "FILE")]
        public_key: Option<PathBuf>,
        #[arg(long, value_name = "FILE")]
        private_key: Option<PathBuf>,
        #[arg(long)]
        category: Option<String>,
        #[arg(short = 'n', long = "note", default_value = "")]
        purpose: String,
    },
    /// Rename a key entry or edit its comment and purpose note.
    #[command(group(clap::ArgGroup::new("field").required(true).multiple(true).args(["new_title","purpose","comment"])))]
    Edit {
        entry: String,
        #[arg(long = "title")]
        new_title: Option<String>,
        #[arg(short = 'n', long = "note")]
        purpose: Option<String>,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Delete a key entry. Its public text stays in any file already exported.
    Delete {
        entry: String,
        /// Delete without the typed confirmation, after checking the target.
        #[arg(long = "force", id = "force_delete")]
        force: bool,
    },
    /// Write the public or private half of a key entry to a file. Never prints key material.
    Export {
        entry: String,
        #[arg(short = 'o', long, value_name = "FILE")]
        output: PathBuf,
        #[arg(long)]
        private: bool,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum WebDavCommand {
    /// Verify login and save the URL/username. A typed password is kept on this computer only.
    #[command(visible_alias = "in")]
    Login {
        #[arg(short = 'u', long)]
        url: String,
        #[arg(short = 'n', long)]
        username: String,
    },
    /// List a folder relative to the saved WebDAV URL.
    #[command(visible_alias = "ls")]
    List {
        #[arg(default_value = "")]
        path: String,
    },
    /// Show the saved WebDAV profile and sync binding without logging in.
    #[command(visible_alias = "st")]
    Status,
    /// Delete the saved WebDAV password from this computer's credential manager.
    #[command(visible_alias = "forget")]
    ForgetPassword,
    /// Download and open an MDBX; preserve the old local vault and clear grants.
    #[command(visible_alias = "o")]
    Open { path: String },
    /// Upload the encrypted vault to a new remote name and connect sync.
    #[command(visible_aliases = ["p", "push"])]
    Publish { path: String },
    /// Synchronize without overwriting conflicting revisions.
    #[command(visible_alias = "s")]
    Sync,
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Call { .. } => "call",
            Self::Databases => "databases",
            Self::Use { .. } => "use",
            Self::Token { .. } => "token",
            Self::RenameCategory { .. } => "rename-category",
            Self::RenameEntry { .. } => "rename-entry",
            Self::Keys { command } => match command {
                None => "keys",
                Some(KeysCommand::Ssh { .. }) => "keys ssh",
                Some(KeysCommand::Gpg { .. }) => "keys gpg",
                Some(KeysCommand::Edit { .. }) => "keys edit",
                Some(KeysCommand::Delete { .. }) => "keys delete",
                Some(KeysCommand::Export { .. }) => "keys export",
            },
            Self::Library => "library",
            Self::Category { .. } => "category",
            Self::Move { .. } => "move",
            Self::Delete { .. } => "delete",
            Self::DeleteCategory { .. } => "delete-category",
            Self::Language { .. } => "language",
            Self::Commands { .. } => "commands",
            Self::Tui => "tui",
            Self::Add { .. } => "add",
            Self::List => "list",
            Self::Show { .. } => "show",
            Self::Note { .. } => "note",
            Self::Open { .. } => "open",
            Self::Webdav { command } => match command {
                WebDavCommand::Login { .. } => "webdav login",
                WebDavCommand::List { .. } => "webdav list",
                WebDavCommand::Status => "webdav status",
                WebDavCommand::ForgetPassword => "webdav forget-password",
                WebDavCommand::Open { .. } => "webdav open",
                WebDavCommand::Publish { .. } => "webdav publish",
                WebDavCommand::Sync => "webdav sync",
            },
            Self::Settings { .. } => "settings",
            Self::Check { .. } => "check",
            Self::Init { .. } => "init",
            Self::Connect { .. } => "connect",
            Self::Grant(_) => "grant",
            Self::Refresh(_) => "refresh",
            Self::Revoke { .. } => "revoke",
            Self::Serve => "serve",
            Self::Lock => "lock",
            Self::Status => "status",
            Self::Audit { .. } => "audit",
            Self::Mcp { .. } => "mcp",
        }
    }
}
