//! Size guards for key material carried inside a vault entry payload.
//!
//! The gateway path keeps its own tighter limit; widening these must never leak into it.

/// Largest single PEM or ASCII-armor text accepted from a file or the TUI.
pub const MAX_KEY_INPUT_BYTES: usize = 64 * 1024;

/// Largest serialized payload JSON a key entry may hold.
pub const MAX_KEY_ENTRY_PAYLOAD_BYTES: usize = 96 * 1024;

/// Disclosure limit used while a human manages a key entry. It has to hold the private
/// armor, the public armor and its Base64 chunks at the same time.
pub const KEY_DISCLOSURE_LIMIT_BYTES: usize = 128 * 1024;

/// Largest OpenPGP certificate Monica accepts, mirroring Android's import cap.
pub const MAX_GPG_IMPORT_BYTES: usize = 1024 * 1024;

/// Characters per `monica_gpg_public_%04d` custom field.
pub const GPG_PUBLIC_CHUNK_CHARS: usize = 2000;

const _: () = assert!(KEY_DISCLOSURE_LIMIT_BYTES > MAX_KEY_ENTRY_PAYLOAD_BYTES);
const _: () = assert!(MAX_KEY_ENTRY_PAYLOAD_BYTES > MAX_KEY_INPUT_BYTES);
