//! Browser-only stdio. Deliberately separate from CLI discovery and MCP.
#![cfg_attr(windows, windows_subsystem = "windows")]
use clap::Parser;
use monica_pass_cli::{admin, browser, config::ConfigStore};
use std::path::PathBuf;

const FIREFOX_EXTENSION_ID: &str = "monica@monica-pass.github.io";

#[derive(Parser)]
#[command(version, about = "Monica browser Native Messaging host")]
struct Args {
    #[arg(long)]
    config: Option<PathBuf>,
    /// Chromium extension origin, or Firefox's absolute native manifest path.
    #[arg(required_unless_present = "diagnose")]
    origin: Option<String>,
    /// Firefox passes its add-on ID after the native manifest path.
    firefox_extension_id: Option<String>,
    /// Read-only installation metadata; does not connect, unlock or change files.
    #[arg(long)]
    diagnose: bool,
    /// Chromium on Windows appends its parent window handle.
    #[arg(long, hide = true)]
    parent_window: Option<String>,
}

impl Args {
    fn valid_launch(&self) -> bool {
        let Some(first) = self.origin.as_deref() else {
            return false;
        };
        match self.firefox_extension_id.as_deref() {
            None => browser::valid_origin(first),
            Some(id) => {
                id == FIREFOX_EXTENSION_ID
                    && std::path::Path::new(first).is_absolute()
                    && self.parent_window.is_none()
            }
        }
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();
    if !args.diagnose && !args.valid_launch() {
        return std::process::ExitCode::FAILURE;
    }
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
            "firefox_extension_id":FIREFOX_EXTENSION_ID,
            "config":config,"state":state,"secret_reading":false})
        );
        return std::process::ExitCode::SUCCESS;
    }
    let mut host = browser::BrowserHost::new(ConfigStore::new(config));
    match browser::serve(&mut host, tokio::io::stdin(), tokio::io::stdout()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_launch_keeps_exact_origin_rules_and_window_argument() {
        let args = Args::try_parse_from([
            "host",
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/",
            "--parent-window=1234",
        ])
        .unwrap();
        assert!(args.valid_launch());
        for origin in [
            "https://example.com/",
            "moz-extension://random/",
            "chrome-extension://bad/",
        ] {
            assert!(
                !Args::try_parse_from(["host", origin])
                    .unwrap()
                    .valid_launch()
            );
        }
    }

    #[test]
    fn firefox_launch_accepts_only_fixed_id_after_absolute_manifest() {
        let manifest = std::env::current_dir()
            .unwrap()
            .join("Firefox manifest with spaces.json");
        let manifest = manifest.to_str().unwrap();
        assert!(
            Args::try_parse_from(["host", manifest, FIREFOX_EXTENSION_ID])
                .unwrap()
                .valid_launch()
        );
        for id in [
            "other@example.com",
            "Monica@monica-pass.github.io",
            "monica@monica-pass.github.io/",
            "",
        ] {
            assert!(
                !Args::try_parse_from(["host", manifest, id])
                    .unwrap()
                    .valid_launch()
            );
        }
        assert!(
            !Args::try_parse_from(["host", manifest])
                .unwrap()
                .valid_launch()
        );
        assert!(
            !Args::try_parse_from(["host", "relative.json", FIREFOX_EXTENSION_ID])
                .unwrap()
                .valid_launch()
        );
        assert!(
            !Args::try_parse_from([
                "host",
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/",
                FIREFOX_EXTENSION_ID
            ])
            .unwrap()
            .valid_launch()
        );
        assert!(
            !Args::try_parse_from([
                "host",
                manifest,
                FIREFOX_EXTENSION_ID,
                "--parent-window=1234"
            ])
            .unwrap()
            .valid_launch()
        );
        assert!(Args::try_parse_from(["host", manifest, FIREFOX_EXTENSION_ID, "extra"]).is_err());
    }
}
