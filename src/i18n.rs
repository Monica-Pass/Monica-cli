//! Human-facing language selection. Locale is passed explicitly; broker/MCP
//! identifiers, schemas, JSON and error codes never depend on a UI preference.
mod messages;

use std::fmt::{Display, Write};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigStore, read_json, write_json};
use crate::error::{GatewayError, Result};

pub use messages::Message;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    En,
    ZhCn,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum LanguageChoice {
    #[default]
    #[serde(rename = "auto")]
    #[value(name = "auto")]
    Auto,
    #[serde(rename = "en")]
    #[value(name = "en", alias = "en-US", alias = "en_US")]
    En,
    #[serde(rename = "zh-CN")]
    #[value(name = "zh-CN", alias = "zh", alias = "zh_CN")]
    ZhCn,
}

impl LanguageChoice {
    pub fn parse(value: &str) -> Option<Self> {
        use clap::ValueEnum;
        Self::from_str(value, true).ok()
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }

    pub fn resolve(self, system: Language) -> Language {
        match self {
            Self::Auto => system,
            Self::En => Language::En,
            Self::ZhCn => Language::ZhCn,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub language: LanguageChoice,
}

impl Preferences {
    // Separate from vault configuration, so changing language neither creates
    // a vault nor writes connection/grant state while a broker is running.
    pub fn load(store: &ConfigStore) -> Result<Self> {
        let path = store.path.with_extension("preferences.json");
        if !path
            .try_exists()
            .map_err(|_| GatewayError::StateUnavailable)?
        {
            return Ok(Self::default());
        }
        read_json(&path, 4096)
    }

    pub fn save(&self, store: &ConfigStore) -> Result<()> {
        write_json(&store.path.with_extension("preferences.json"), self, true)
    }
}

impl Language {
    pub const fn name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
        }
    }

    pub const fn choice(self) -> LanguageChoice {
        match self {
            Self::En => LanguageChoice::En,
            Self::ZhCn => LanguageChoice::ZhCn,
        }
    }

    pub const fn other(self) -> Self {
        match self {
            Self::En => Self::ZhCn,
            Self::ZhCn => Self::En,
        }
    }

    pub fn from_locale(value: &str) -> Option<Self> {
        let value = value.split(['.', '@']).next()?.replace('_', "-");
        match value.split('-').next()?.to_ascii_lowercase().as_str() {
            "zh" => Some(Self::ZhCn),
            "en" | "c" | "posix" => Some(Self::En),
            _ => None,
        }
    }

    /// Interpolate catalog placeholders once. User values are display data,
    /// never recursively interpreted as another template or translated.
    pub fn format(self, message: Message, args: &[(&str, &dyn Display)]) -> String {
        let mut rest = self.text(message);
        let mut output = String::with_capacity(rest.len());
        while let Some(start) = rest.find('{') {
            output.push_str(&rest[..start]);
            let Some(end) = rest[start + 1..].find('}') else {
                output.push_str(&rest[start..]);
                return output;
            };
            let end = start + 1 + end;
            let key = &rest[start + 1..end];
            if let Some((_, value)) = args.iter().find(|(name, _)| *name == key) {
                let _ = write!(output, "{value}");
            } else {
                // Keep a missing placeholder visible rather than drop content.
                output.push_str(&rest[start..=end]);
            }
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
        output
    }
}

pub fn resolve(
    explicit: Option<LanguageChoice>,
    environment: Option<LanguageChoice>,
    saved: LanguageChoice,
    system: Language,
) -> Language {
    explicit.or(environment).unwrap_or(saved).resolve(system)
}

pub fn environment_choice() -> Option<LanguageChoice> {
    std::env::var("MONICA_LANG")
        .ok()
        .as_deref()
        .and_then(LanguageChoice::parse)
}

pub fn system_language() -> Language {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Language::from_locale(&value).unwrap_or(Language::En);
        }
    }
    native_locale()
        .as_deref()
        .and_then(Language::from_locale)
        .unwrap_or(Language::En)
}

#[cfg(windows)]
fn native_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buffer = [0_u16; 85]; // LOCALE_NAME_MAX_LENGTH, including the NUL.
    // SAFETY: Windows receives a writable buffer and its actual element count.
    // No pointers escape; a successful length includes the terminating NUL.
    let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

#[cfg(not(windows))]
fn native_locale() -> Option<String> {
    None
}

/// Catalog keys are checked by the compiler; named arguments stay explicit.
#[macro_export]
macro_rules! tr {
    ($language:expr, $message:ident $(,)?) => {
        $language.text($crate::i18n::Message::$message)
    };
    ($language:expr, $message:ident, $($name:ident = $value:expr),+ $(,)?) => {
        $language.format(
            $crate::i18n::Message::$message,
            &[$((stringify!($name), &$value as &dyn std::fmt::Display)),+],
        )
    };
}

impl Language {
    pub fn error(self, error: GatewayError) -> &'static str {
        self.text(match error {
            GatewayError::InvalidRequest => Message::ErrorInvalidRequest,
            GatewayError::InvalidNote => Message::ErrorInvalidNote,
            GatewayError::SensitiveMetadata => Message::ErrorSensitiveMetadata,
            GatewayError::RepositoryRequired => Message::ErrorRepositoryRequired,
            GatewayError::InvalidConfig => Message::ErrorInvalidConfig,
            GatewayError::AlreadyExists => Message::ErrorAlreadyExists,
            GatewayError::NotFound => Message::ErrorNotFound,
            GatewayError::PasswordRequirements => Message::ErrorPasswordRequirements,
            GatewayError::ListenUnavailable => Message::ErrorListenUnavailable,
            GatewayError::BrokerAlreadyRunning => Message::ErrorBrokerAlreadyRunning,
            GatewayError::BrokerUnavailable => Message::ErrorBrokerUnavailable,
            GatewayError::HumanTerminalRequired => Message::ErrorHumanTerminalRequired,
            GatewayError::SecretInputRequired => Message::ErrorSecretInputRequired,
            GatewayError::InvalidSecretInput => Message::ErrorInvalidSecretInput,
            GatewayError::Unauthorized => Message::ErrorUnauthorized,
            GatewayError::ReauthorizationRequired => Message::ErrorReauthorizationRequired,
            GatewayError::PermissionDenied => Message::ErrorPermissionDenied,
            GatewayError::UnlockRequired => Message::ErrorUnlockRequired,
            GatewayError::CredentialUnavailable => Message::ErrorCredentialUnavailable,
            GatewayError::StateUnavailable => Message::ErrorStateUnavailable,
            GatewayError::RateLimited => Message::ErrorRateLimited,
            GatewayError::UpstreamUnavailable => Message::ErrorUpstreamUnavailable,
            GatewayError::UpstreamRejected => Message::ErrorUpstreamRejected,
            GatewayError::RedirectBlocked => Message::ErrorRedirectBlocked,
            GatewayError::ResponseTooLarge => Message::ErrorResponseTooLarge,
            GatewayError::ResponseBlocked => Message::ErrorResponseBlocked,
            GatewayError::WriteOutcomeUnknown => Message::ErrorWriteOutcomeUnknown,
            GatewayError::RequestIdConflict => Message::ErrorRequestIdConflict,
            GatewayError::JournalFull => Message::ErrorJournalFull,
            GatewayError::InvalidWebDav => Message::ErrorInvalidWebDav,
            GatewayError::WebDavUnauthorized => Message::ErrorWebDavUnauthorized,
            GatewayError::WebDavUnavailable => Message::ErrorWebDavUnavailable,
            GatewayError::InvalidWebDavResponse => Message::ErrorInvalidWebDavResponse,
            GatewayError::RemoteNotFound => Message::ErrorRemoteNotFound,
            GatewayError::SyncConflict => Message::ErrorSyncConflict,
            GatewayError::RemoteVersionRequired => Message::ErrorRemoteVersionRequired,
            GatewayError::RemoteProtocolUnsupported => Message::ErrorRemoteProtocolUnsupported,
            GatewayError::SyncOutcomeUnknown => Message::ErrorSyncOutcomeUnknown,
            GatewayError::InvalidVault => Message::ErrorInvalidVault,
            GatewayError::VaultSchemaUnsupported => Message::ErrorVaultSchemaUnsupported,
            GatewayError::ExternalBlobsUnsupported => Message::ErrorExternalBlobsUnsupported,
            GatewayError::VaultConnectionsInvalid => Message::ErrorVaultConnectionsInvalid,
            GatewayError::RemoteNotConfigured => Message::ErrorRemoteNotConfigured,
            GatewayError::KeyPayloadTooLarge => Message::ErrorKeyPayloadTooLarge,
            GatewayError::KeyEntryTypeMismatch => Message::ErrorKeyEntryTypeMismatch,
            GatewayError::InvalidKeyMaterial => Message::ErrorInvalidKeyMaterial,
            GatewayError::KeySecretMissing => Message::ErrorKeySecretMissing,
        })
    }

    pub fn sync_result(self, result: crate::sync::SyncResult) -> &'static str {
        use crate::sync::SyncResult;
        self.text(match result {
            SyncResult::UpToDate => Message::SyncUpToDate,
            SyncResult::Uploaded => Message::SyncUploaded,
            SyncResult::Downloaded => Message::SyncDownloaded,
            SyncResult::Published => Message::SyncPublished,
        })
    }
}

#[cfg(test)]
mod tests;
