//! SSH and GPG key entries stored in the vault in Monica's shared on-disk format.
//!
//! The key text lives in the entry payload exactly as Monica for Android writes it, so the
//! engine encrypts it and no second copy exists on disk. Nothing in this module is reachable
//! from the gateway or the MCP surface: keys are managed by a human only.
pub mod id;
pub mod limits;
pub mod manage;
pub mod openpgp;
pub mod openssh;
pub mod payload;
