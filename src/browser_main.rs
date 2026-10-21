//! Browser-only stdio. Deliberately separate from CLI discovery and MCP.
#![cfg_attr(windows, windows_subsystem = "windows")]
use clap::Parser;
use monica_pass_cli::{admin, browser, config::ConfigStore};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Monica browser Native Messaging host")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    /// Supplied by Chromium, never a web origin.
    #[arg(required_unless_present = "diagnose")]
    origin: Option<String>,
    /// Read-only installation metadata; does not connect, unlock or change files.
    #[arg(long)]
    diagnose: bool,
    /// Chromium on Windows appends its parent window handle.
    #[arg(long, hide = true)]
    parent_window: Option<String>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let config = match args.config.map(Ok).unwrap_or_else(admin::default_config) {
        Ok(path) => path,
        Err(_) => return std::process::ExitCode::FAILURE,
    };
    if args.diagnose {
        let store = ConfigStore::new(config.clone());
        let state = match store.load() {
            Ok(_) => "configured",
            Err(monica_pass_cli::error::GatewayError::SetupRequired) => "setup_required",
            Err(_) => "invalid_or_unreadable_config",
        };
        println!(
            "{}",
            serde_json::json!({"host":"app.monica.cli","protocol":browser::PROTOCOL,
            "version":env!("CARGO_PKG_VERSION"),"integration_revision":2,"capabilities":browser::CAPABILITIES,
            "config":config,"state":state,"secret_reading":false})
        );
        return std::process::ExitCode::SUCCESS;
    }
    if !args.origin.as_deref().is_some_and(browser::valid_origin) {
        return std::process::ExitCode::FAILURE;
    }
    let mut host = browser::BrowserHost::new(ConfigStore::new(config));
    match browser::serve(&mut host, tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
