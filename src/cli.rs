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
    Settings { name: String },
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
    /// Start the MCP stdio bridge. Never prompts for upstream credentials.
    Mcp {
        #[arg(short = 'c', long, value_name = "CLIENT_FILE")]
        client: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum WebDavCommand {
    /// Verify login and save the URL/username. The password is not saved.
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
            Self::Library => "library",
            Self::Category { .. } => "category",
            Self::Move { .. } => "move",
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
            Self::Mcp { .. } => "mcp",
        }
    }
}
