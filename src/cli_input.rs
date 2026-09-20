//! Explicit, bounded secret input for trusted local automation. No secret values
//! are accepted in argv, environment variables, or machine responses.
use std::io::{IsTerminal, Read};

use monica_pass_cli::error::{GatewayError, Result};
use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// Shared by execution and command discovery, so the documented input contract
/// is the one enforced by the parser. Confirmation is only needed when typing.
pub fn required_fields(command: &str) -> &'static [SecretField] {
    use SecretField::*;
    match command {
        "add" | "connect" | "token" => &[Password, Token],
        "init" | "note" | "open" | "grant" | "refresh" | "serve" | "library" | "category"
        | "move" | "rename-category" | "rename-entry" | "use" | "keys" | "keys ssh"
        | "keys gpg" | "keys edit" | "keys export" => &[Password],
        "webdav login" | "webdav list" => &[WebDavPassword],
        "webdav open" | "webdav publish" | "webdav sync" => &[Password, WebDavPassword],
        _ => &[],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretField {
    Password,
    Token,
    WebDavPassword,
}

impl SecretField {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Token => "token",
            Self::WebDavPassword => "webdav_password",
        }
    }
}

// Deliberately no Debug or Serialize. Partial deserialization failures also
// drop and zeroize fields that have already been parsed.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Secrets {
    #[serde(default, deserialize_with = "secret")]
    password: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "secret")]
    token: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "secret")]
    webdav_password: Option<Zeroizing<String>>,
}

fn secret<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Zeroizing<String>>, D::Error> {
    String::deserialize(deserializer).map(|value| Some(Zeroizing::new(value)))
}

impl Secrets {
    fn field(&mut self, field: SecretField) -> &mut Option<Zeroizing<String>> {
        match field {
            SecretField::Password => &mut self.password,
            SecretField::Token => &mut self.token,
            SecretField::WebDavPassword => &mut self.webdav_password,
        }
    }

    fn read(reader: impl Read, required: &[SecretField]) -> Result<Self> {
        let mut bytes = Zeroizing::new(Vec::new());
        reader
            .take((MAX_SECRET_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| GatewayError::InvalidSecretInput)?;
        if bytes.len() > MAX_SECRET_BYTES
            || bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{')
        {
            return Err(GatewayError::InvalidSecretInput);
        }
        let mut secrets: Self =
            serde_json::from_slice(&bytes).map_err(|_| GatewayError::InvalidSecretInput)?;
        for field in [
            SecretField::Password,
            SecretField::Token,
            SecretField::WebDavPassword,
        ] {
            let supplied = secrets.field(field);
            if required.contains(&field) {
                if supplied.as_ref().is_none_or(|value| value.is_empty()) {
                    return Err(GatewayError::SecretInputRequired);
                }
            } else if supplied.is_some() {
                return Err(GatewayError::InvalidSecretInput);
            }
        }
        Ok(secrets)
    }
}

pub struct SecretInput {
    pipe: Option<Secrets>,
    non_interactive: bool,
}

impl SecretInput {
    pub fn new(stdin: bool, non_interactive: bool, required: &[SecretField]) -> Result<Self> {
        let pipe = if stdin {
            if required.is_empty() {
                return Err(GatewayError::InvalidRequest);
            }
            if std::io::stdin().is_terminal() {
                return Err(GatewayError::SecretInputRequired);
            }
            Some(Secrets::read(std::io::stdin().lock(), required)?)
        } else {
            if non_interactive && !required.is_empty() {
                return Err(GatewayError::SecretInputRequired);
            }
            None
        };
        Ok(Self {
            pipe,
            non_interactive,
        })
    }

    pub fn take(&mut self, field: SecretField, prompt: &str) -> Result<Zeroizing<String>> {
        if let Some(pipe) = &mut self.pipe {
            return pipe
                .field(field)
                .take()
                .ok_or(GatewayError::SecretInputRequired);
        }
        self.prompt(prompt)
    }

    pub fn confirm(&self, password: &str, prompt: &str) -> Result<Zeroizing<String>> {
        if self.pipe.is_some() {
            // Confirmation prevents typing mistakes in the human UI. A trusted
            // producer supplies one exact password; it is not echoed back.
            Ok(Zeroizing::new(password.to_owned()))
        } else {
            self.prompt(prompt)
        }
    }

    fn prompt(&self, prompt: &str) -> Result<Zeroizing<String>> {
        if self.non_interactive {
            return Err(GatewayError::SecretInputRequired);
        }
        if !std::io::stdin().is_terminal() {
            return Err(GatewayError::HumanTerminalRequired);
        }
        rpassword::prompt_password(prompt)
            .map(Zeroizing::new)
            .map_err(|_| GatewayError::HumanTerminalRequired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_input_preserves_unicode_spaces_and_requires_exact_fields() {
        let bytes = br#"{"password":"  \u5bc6\u7801  ","token":"a-token"}"#;
        let mut secrets = Secrets::read(
            bytes.as_slice(),
            &[SecretField::Password, SecretField::Token],
        )
        .unwrap();
        assert_eq!(secrets.password.take().unwrap().as_str(), "  密码  ");
        assert_eq!(secrets.token.take().unwrap().as_str(), "a-token");
        assert!(matches!(
            Secrets::read(bytes.as_slice(), &[SecretField::Password]),
            Err(GatewayError::InvalidSecretInput)
        ));
    }

    #[test]
    fn reauthorizing_an_ai_grant_needs_the_master_password_only() {
        for command in ["grant", "refresh"] {
            assert_eq!(
                required_fields(command),
                &[SecretField::Password],
                "{command} must never ask for the stored token"
            );
            let password_only = br#"{"password":"vault-passphrase"}"#;
            Secrets::read(password_only.as_slice(), required_fields(command)).unwrap();
            let with_token = br#"{"password":"vault-passphrase","token":"a-token"}"#;
            assert!(matches!(
                Secrets::read(with_token.as_slice(), required_fields(command)),
                Err(GatewayError::InvalidSecretInput)
            ));
        }
    }
}
