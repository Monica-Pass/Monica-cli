use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, GatewayError>;

/// Errors crossing either broker or MCP boundaries contain only fixed messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum GatewayError {
    #[error("The request is invalid or contains unsupported fields.")]
    InvalidRequest,
    #[error(
        "The public note must be plain text of at most 1024 UTF-8 bytes, without control characters."
    )]
    InvalidNote,
    #[error(
        "AI-visible fields contain a credential or vault password. Remove the secret from the name, note or other public fields."
    )]
    SensitiveMetadata,
    #[error(
        "This grant allows multiple repositories. Specify an exact repository from the connection catalog."
    )]
    RepositoryRequired,
    #[error("The configuration is invalid. Check the local configuration file.")]
    InvalidConfig,
    #[error(
        "The configuration, vault, connection, grant or output file already exists. Use a new name."
    )]
    AlreadyExists,
    #[error("The requested local configuration, connection or grant does not exist.")]
    NotFound,
    #[error("The new password must not be empty or whitespace-only, and both entries must match.")]
    PasswordRequirements,
    #[error("The configured loopback port is unavailable. Check for another running broker.")]
    ListenUnavailable,
    #[error(
        "The vault is busy. Lock the broker and wait for it to stop before managing or syncing the vault."
    )]
    BrokerAlreadyRunning,
    #[error("Start and unlock the broker in the TUI or a trusted local CLI process.")]
    BrokerUnavailable,
    #[error(
        "This command requires a human terminal for hidden input, or --secrets-stdin with a trusted producer."
    )]
    HumanTerminalRequired,
    #[error(
        "Secret input is required. Use --secrets-stdin with a trusted producer; commands --json documents the required fields."
    )]
    SecretInputRequired,
    #[error(
        "Secret input must be one UTF-8 JSON object of at most 16384 bytes with exactly the required string fields. Values are never echoed."
    )]
    InvalidSecretInput,
    #[error("The gateway capability is invalid, expired or revoked.")]
    Unauthorized,
    #[error(
        "This AI authorization has reached its time limit or call limit. A person must run `monica refresh <grant>` locally with the vault password, then restart the MCP server."
    )]
    ReauthorizationRequired,
    #[error("This operation or repository is not permitted by the grant.")]
    PermissionDenied,
    #[error("The vault requires a fresh unlock. Use the TUI or the local CLI with secure input.")]
    UnlockRequired,
    #[error("The credential is unavailable or does not match the configured service.")]
    CredentialUnavailable,
    #[error("Local state could not be read or safely written.")]
    StateUnavailable,
    #[error("The grant has reached its request limit. Try again later.")]
    RateLimited,
    #[error("The upstream service could not be reached.")]
    UpstreamUnavailable,
    #[error("The upstream service rejected the request. Check the account permissions.")]
    UpstreamRejected,
    #[error("An upstream redirect was blocked.")]
    RedirectBlocked,
    #[error("The upstream response exceeded the supported limit.")]
    ResponseTooLarge,
    #[error("The response failed the credential disclosure check.")]
    ResponseBlocked,
    #[error("The write outcome is unknown. Inspect the repository before making a new request.")]
    WriteOutcomeUnknown,
    #[error("The request ID was already used with different arguments.")]
    RequestIdConflict,
    #[error("The operation journal is full. Rotate it while the broker is stopped.")]
    JournalFull,
    #[error(
        "The WebDAV address or path is invalid. Use HTTPS and a path inside the configured folder."
    )]
    InvalidWebDav,
    #[error("WebDAV authentication failed. Check the username and app password.")]
    WebDavUnauthorized,
    #[error("WebDAV could not complete the request. Check the URL, network and TLS certificate.")]
    WebDavUnavailable,
    #[error("The WebDAV server returned an invalid or unsupported response.")]
    InvalidWebDavResponse,
    #[error("The remote file or directory does not exist.")]
    RemoteNotFound,
    #[error(
        "The remote revision changed or both copies have changes. Both copies are preserved; resolve the conflict before syncing."
    )]
    SyncConflict,
    #[error(
        "The remote server did not provide a strong ETag. Read or publish to a new filename; safe replacement requires an ETag."
    )]
    RemoteVersionRequired,
    #[error(
        "The upload outcome is unknown. Compare both copies before retrying: sync an existing connection, or open the remote file after an initial publish."
    )]
    SyncOutcomeUnknown,
    #[error("The downloaded file is not a supported MDBX vault or its integrity check failed.")]
    InvalidVault,
    #[error(
        "This vault uses an incompatible unlock schema, such as legacy Android MDBX-1. Keep the original file and use a native MDBX3 vault."
    )]
    VaultSchemaUnsupported,
    #[error(
        "This vault needs external attachment files. Single-file WebDAV sync cannot transfer those files."
    )]
    ExternalBlobsUnsupported,
    #[error(
        "This vault has too many or ambiguous gateway connections. Resolve them in Monica before importing."
    )]
    VaultConnectionsInvalid,
    #[error(
        "No WebDAV vault is connected. Open a remote MDBX file or publish the local vault first."
    )]
    RemoteNotConfigured,
    #[error(
        "The key entry payload exceeds the supported size limit. Re-export a smaller key certificate."
    )]
    KeyPayloadTooLarge,
    #[error("The entry is not a key entry of the requested kind, or it was not created as one.")]
    KeyEntryTypeMismatch,
    #[error(
        "The key material could not be read. Supported: OpenSSH Ed25519 or RSA PEM, and OpenPGP v4 ASCII armor."
    )]
    InvalidKeyMaterial,
    #[error(
        "This key entry stores no private material. Import the secret ring or the private PEM first."
    )]
    KeySecretMissing,
}

impl GatewayError {
    pub fn response(self) -> serde_json::Value {
        serde_json::json!({"ok": false, "error": {"code": self, "message": self.to_string()}})
    }
}
