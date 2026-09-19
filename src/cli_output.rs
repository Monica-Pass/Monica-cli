use std::io::Write;

use monica_pass_cli::error::{GatewayError, Result};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub struct Output {
    pub json: bool,
}

impl Output {
    pub fn note(self, message: impl std::fmt::Display) {
        if !self.json {
            eprintln!("{message}");
        }
    }

    pub fn result(self, command: &str, data: Value, human_json: Option<&Value>) -> Result<()> {
        if self.json {
            print_json(
                &json!({"ok": true, "command": command, "data": data}),
                false,
            )
        } else if let Some(value) = human_json {
            print_json(value, true)
        } else {
            Ok(())
        }
    }

    /// JSON mode emits the same envelope as [`Output::result`]; human mode prints a
    /// pre-rendered table so read-only commands stay friendly without changing `--json`.
    pub fn result_text(self, command: &str, data: Value, human: Option<String>) -> Result<()> {
        if self.json {
            print_json(
                &json!({"ok": true, "command": command, "data": data}),
                false,
            )
        } else if let Some(text) = human {
            let mut stdout = std::io::stdout().lock();
            writeln!(&mut stdout, "{text}").map_err(|_| GatewayError::StateUnavailable)?;
            stdout.flush().map_err(|_| GatewayError::StateUnavailable)
        } else {
            Ok(())
        }
    }

    pub fn event(self, command: &str, event: &str, data: Value) -> Result<()> {
        if self.json {
            print_json(
                &json!({"ok": true, "command": command, "event": event, "data": data}),
                false,
            )
        } else {
            Ok(())
        }
    }
}

pub fn print_json(value: &Value, pretty: bool) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    let result = if pretty {
        serde_json::to_writer_pretty(&mut stdout, value)
    } else {
        serde_json::to_writer(&mut stdout, value)
    };
    result.map_err(|_| GatewayError::StateUnavailable)?;
    writeln!(&mut stdout).map_err(|_| GatewayError::StateUnavailable)?;
    stdout.flush().map_err(|_| GatewayError::StateUnavailable)
}
