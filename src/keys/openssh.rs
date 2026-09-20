//! OpenSSH key text in the shape Monica for Android writes it.
//!
//! Generation, import and fingerprinting mirror `utils/SshKeyGenerator.kt`: the PEM body is one
//! Base64 run of the container with no line breaks inside, wrapped at 70 characters for the
//! OpenSSH container and 64 for PKCS#1, with a newline after the END marker. The public line is
//! `<alg-name> <base64 wire blob>` plus the comment only when it is non-empty, and the
//! fingerprint is the unpadded Base64 SHA256 of that wire blob.
//!
//! An imported key keeps its own container bytes verbatim: the CLI never re-wraps material
//! another client produced, because that would change the secret the user is holding.
use base64::Engine as _;
use serde_json::Map;
use sha2::{Digest as _, Sha256};
use ssh_key::Algorithm;
use ssh_key::private::{Ed25519Keypair, KeypairData, PrivateKey, RsaKeypair};

use super::limits::MAX_KEY_INPUT_BYTES;
use super::payload::SshKeyData;
use crate::error::{GatewayError, Result};

/// `SshKeyData.schema`: records without it are read as v1 by both clients.
pub const SCHEMA_V1: &str = "monica.ssh-key.v1";
/// `SshKeyData.format`: labels the record, not the RSA container (see the handoff contract).
pub const FORMAT_OPENSSH: &str = "OPENSSH";
pub const ALGORITHM_ED25519: &str = "ED25519";
pub const ALGORITHM_RSA: &str = "RSA";

/// Android's `PEM_LINE_LEN_OPENSSH` / `PEM_LINE_LEN_RSA`.
const PEM_LINE_OPENSSH: usize = 70;
const PEM_LINE_RSA: usize = 64;
const HEADER_OPENSSH: &str = "OPENSSH PRIVATE KEY";
const HEADER_PKCS1: &str = "RSA PRIVATE KEY";
/// Android reports 256 for Ed25519, matching `ED25519_KEY_SIZE`.
const ED25519_KEY_SIZE: i64 = 256;

/// RSA sizes Android's generator UI offers.
pub const RSA_ALLOWED_KEY_SIZES: [usize; 3] = [2048, 3072, 4096];
pub const DEFAULT_RSA_KEY_SIZE: usize = 3072;

/// Key types the CLI can create. GPG material is import-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshAlgorithm {
    Ed25519,
    Rsa(usize),
}

impl SshAlgorithm {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "ed25519" => Ok(Self::Ed25519),
            "rsa" => Ok(Self::Rsa(DEFAULT_RSA_KEY_SIZE)),
            _ if let Some(bits) = value.strip_prefix("rsa") => match bits.parse::<usize>() {
                Ok(bits) => Ok(Self::Rsa(bits)),
                Err(_) => Err(GatewayError::InvalidRequest),
            },
            _ => Err(GatewayError::InvalidRequest),
        }
    }
}

/// A parsed or freshly generated key, reduced to the fields `SshKeyData` stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshKeyPair {
    pub algorithm: String,
    pub key_size: i64,
    pub public_key_openssh: String,
    pub private_key_pem: String,
    pub fingerprint_sha256: String,
    pub comment: String,
}

impl SshKeyPair {
    /// Reads an OpenSSH container or a PKCS#1 RSA PEM, keeping the input text verbatim.
    pub fn from_pem(text: &str) -> Result<Self> {
        if text.len() > MAX_KEY_INPUT_BYTES {
            return Err(GatewayError::KeyPayloadTooLarge);
        }
        let key = match PrivateKey::from_openssh(text) {
            Ok(key) => key,
            Err(_) => {
                let private = parse_pkcs1(text)?;
                let pair =
                    RsaKeypair::try_from(&private).map_err(|_| GatewayError::InvalidKeyMaterial)?;
                PrivateKey::from(pair)
            }
        };
        if key.key_data().is_encrypted() {
            return Err(GatewayError::InvalidKeyMaterial);
        }
        Self::from_key(&key, text.to_owned())
    }

    /// Creates a new key inside the process: the secret never travels through argv or a pipe.
    pub fn generate(algorithm: SshAlgorithm, comment: &str) -> Result<Self> {
        let mut rng = ssh_key::rand_core::OsRng;
        let comment = comment.trim();
        // The container written here is the one Android writes for the same algorithm: the
        // OpenSSH native form for Ed25519, PKCS#1 for RSA.
        match algorithm {
            SshAlgorithm::Ed25519 => {
                let key =
                    PrivateKey::new(KeypairData::from(Ed25519Keypair::random(&mut rng)), comment)
                        .map_err(|_| GatewayError::StateUnavailable)?;
                let pem = wrap_pem(
                    HEADER_OPENSSH,
                    &key.to_bytes().map_err(|_| GatewayError::StateUnavailable)?,
                    PEM_LINE_OPENSSH,
                );
                Self::from_key(&key, pem)
            }
            SshAlgorithm::Rsa(bits) => {
                if !RSA_ALLOWED_KEY_SIZES.contains(&bits) {
                    return Err(GatewayError::InvalidRequest);
                }
                // ssh-key cannot turn its own keypair back into an `rsa` private key (it feeds
                // `p` as both primes), so the rsa key is the source and ssh-key only reads it.
                use rsa::pkcs1::EncodeRsaPrivateKey as _;
                let private = rsa::RsaPrivateKey::new(&mut rng, bits)
                    .map_err(|_| GatewayError::InvalidKeyMaterial)?;
                let pair =
                    RsaKeypair::try_from(&private).map_err(|_| GatewayError::InvalidKeyMaterial)?;
                let key = PrivateKey::new(KeypairData::from(pair), comment)
                    .map_err(|_| GatewayError::StateUnavailable)?;
                let der = private
                    .to_pkcs1_der()
                    .map_err(|_| GatewayError::StateUnavailable)?;
                Self::from_key(&key, wrap_pem(HEADER_PKCS1, der.as_bytes(), PEM_LINE_RSA))
            }
        }
    }

    /// Replaces the comment, which is also the tail of the public line.
    pub fn with_comment(&self, comment: &str) -> Result<Self> {
        Ok(Self {
            public_key_openssh: public_line_with_comment(&self.public_key_openssh, comment)?,
            comment: comment.trim().to_owned(),
            ..self.clone()
        })
    }

    pub fn to_data(&self) -> SshKeyData {
        SshKeyData {
            algorithm: self.algorithm.clone(),
            key_size: self.key_size,
            public_key_openssh: self.public_key_openssh.clone(),
            private_key_openssh: self.private_key_pem.clone(),
            fingerprint_sha256: self.fingerprint_sha256.clone(),
            comment: self.comment.clone(),
            format: FORMAT_OPENSSH.to_owned(),
            schema: SCHEMA_V1.to_owned(),
            additional: Map::new(),
        }
    }

    /// The public wire blob, which is what `ssh-keygen -lf` hashes.
    pub fn public_blob(&self) -> Result<Vec<u8>> {
        public_blob(&self.public_key_openssh)
    }

    fn from_key(key: &PrivateKey, private_key_pem: String) -> Result<Self> {
        let algorithm = key.public_key().algorithm();
        let (name, key_size) = match algorithm {
            Algorithm::Ed25519 => (ALGORITHM_ED25519, ED25519_KEY_SIZE),
            Algorithm::Rsa { .. } => (ALGORITHM_RSA, rsa_key_size(key)?),
            _ => return Err(GatewayError::InvalidKeyMaterial),
        };
        let blob = key
            .public_key()
            .to_bytes()
            .map_err(|_| GatewayError::InvalidKeyMaterial)?;
        let comment = key.comment().trim().to_owned();
        Ok(Self {
            algorithm: name.to_owned(),
            key_size,
            public_key_openssh: public_line(algorithm.as_str(), &blob, &comment),
            private_key_pem,
            fingerprint_sha256: fingerprint(&blob),
            comment,
        })
    }
}

/// Recomputes the fingerprint of a stored public line, so a listing can flag a mismatch.
pub fn fingerprint_of_public_line(line: &str) -> Result<String> {
    Ok(fingerprint(&public_blob(line)?))
}

/// Swaps the comment tail of a stored public line. The algorithm name and the wire blob are
/// carried over untouched, so the fingerprint of an edited entry stays valid.
pub fn public_line_with_comment(line: &str, comment: &str) -> Result<String> {
    let mut parts = line.split_whitespace();
    let prefix = parts.next().ok_or(GatewayError::InvalidKeyMaterial)?;
    let blob = parts.next().ok_or(GatewayError::InvalidKeyMaterial)?;
    Ok(public_line(prefix, &decode_base64(blob)?, comment.trim()))
}

fn parse_pkcs1(text: &str) -> Result<rsa::RsaPrivateKey> {
    use rsa::pkcs1::DecodeRsaPrivateKey as _;
    let der = pem_body(text, HEADER_PKCS1)?;
    rsa::RsaPrivateKey::from_pkcs1_der(&der).map_err(|_| GatewayError::InvalidKeyMaterial)
}

fn rsa_key_size(key: &PrivateKey) -> Result<i64> {
    let KeypairData::Rsa(pair) = key.key_data() else {
        return Err(GatewayError::InvalidKeyMaterial);
    };
    let bytes = pair
        .public
        .n
        .as_positive_bytes()
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    Ok(bytes.len() as i64 * 8)
}

/// `formatOpenSshPublicKey`: prefix and blob, then the comment only when it is present.
fn public_line(prefix: &str, blob: &[u8], comment: &str) -> String {
    let head = format!("{prefix} {}", encode_base64(blob));
    if comment.is_empty() {
        head
    } else {
        format!("{head} {comment}")
    }
}

fn public_blob(line: &str) -> Result<Vec<u8>> {
    let blob = line
        .split_whitespace()
        .nth(1)
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    decode_base64(blob)
}

/// `formatPemPrivateKey`: one Base64 run, fixed-width lines, newline after the END marker.
fn wrap_pem(header: &str, body: &[u8], line_len: usize) -> String {
    let encoded = encode_base64(body);
    let mut text = format!("-----BEGIN {header}-----\n");
    for chunk in encoded.as_bytes().chunks(line_len) {
        text.push_str(core::str::from_utf8(chunk).unwrap_or_default());
        text.push('\n');
    }
    text.push_str(&format!("-----END {header}-----\n"));
    text
}

/// Extracts and decodes a PEM body, tolerating any line endings.
fn pem_body(text: &str, header: &str) -> Result<Vec<u8>> {
    let (_, rest) = text
        .split_once(&format!("-----BEGIN {header}-----"))
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    let (body, _) = rest
        .split_once(&format!("-----END {header}-----"))
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    decode_base64(
        &body
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
    )
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| GatewayError::InvalidKeyMaterial)
}

/// `fingerprintSha256`: `SHA256:` plus the unpadded Base64 of the digest.
fn fingerprint(blob: &[u8]) -> String {
    let digest = Sha256::digest(blob);
    format!(
        "SHA256:{}",
        encode_base64(digest.as_slice()).trim_end_matches('=')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_lines(pem: &str) -> Vec<String> {
        pem.lines()
            .filter(|line| !line.starts_with("-----"))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn ed25519_generation_matches_the_android_text_shape() {
        let key = SshKeyPair::generate(SshAlgorithm::Ed25519, "monica@test").unwrap();
        assert_eq!(key.algorithm, ALGORITHM_ED25519);
        assert_eq!(key.key_size, 256);
        assert!(key.public_key_openssh.starts_with("ssh-ed25519 "));
        assert!(key.public_key_openssh.ends_with(" monica@test"));
        assert!(
            key.private_key_pem
                .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n")
        );
        assert!(
            key.private_key_pem
                .ends_with("-----END OPENSSH PRIVATE KEY-----\n")
        );
        assert!(key.fingerprint_sha256.starts_with("SHA256:"));
        assert!(!key.fingerprint_sha256.contains('='));
        let body = body_lines(&key.private_key_pem);
        assert!(body.len() > 1);
        assert!(
            body[..body.len() - 1]
                .iter()
                .all(|line| line.chars().count() == PEM_LINE_OPENSSH)
        );
    }

    #[test]
    fn rsa_uses_the_pkcs1_container_and_64_character_lines() {
        let key = SshKeyPair::generate(SshAlgorithm::Rsa(2048), "rsa").unwrap();
        assert_eq!(key.algorithm, ALGORITHM_RSA);
        assert_eq!(key.key_size, 2048);
        assert!(key.public_key_openssh.starts_with("ssh-rsa "));
        assert!(
            key.private_key_pem
                .starts_with("-----BEGIN RSA PRIVATE KEY-----\n")
        );
        let body = body_lines(&key.private_key_pem);
        assert!(
            body[..body.len() - 1]
                .iter()
                .all(|line| line.chars().count() == PEM_LINE_RSA)
        );
    }

    #[test]
    fn private_key_text_round_trips_and_rechecks_its_fingerprint() {
        for algorithm in [SshAlgorithm::Ed25519, SshAlgorithm::Rsa(2048)] {
            let key = SshKeyPair::generate(algorithm, "").unwrap();
            assert_eq!(key.comment, "");
            let again = SshKeyPair::from_pem(&key.private_key_pem).unwrap();
            assert_eq!(again.algorithm, key.algorithm);
            assert_eq!(again.key_size, key.key_size);
            assert_eq!(again.public_key_openssh, key.public_key_openssh);
            assert_eq!(again.fingerprint_sha256, key.fingerprint_sha256);
            assert_eq!(again.private_key_pem, key.private_key_pem);
            assert_eq!(
                fingerprint_of_public_line(&key.public_key_openssh).unwrap(),
                key.fingerprint_sha256
            );
        }
    }

    #[test]
    fn an_imported_container_is_never_rewrapped() {
        let key = SshKeyPair::generate(SshAlgorithm::Rsa(2048), "keep").unwrap();
        // Android puts an OpenSSH native container under the `RSA PRIVATE KEY` header for RSA.
        // Whichever header and body arrive, they are stored back byte for byte.
        assert!(key.private_key_pem.contains("BEGIN RSA PRIVATE KEY"));
        assert_eq!(key.with_comment("keep").unwrap(), key);
        let mut renamed = key
            .private_key_pem
            .replace("RSA PRIVATE KEY", "OPENSSH PRIVATE KEY");
        renamed = renamed.replace("BEGIN OPENSSH", "BEGIN RSA");
        assert!(SshKeyPair::from_pem(&renamed).is_err());
    }

    #[test]
    fn comment_changes_the_public_line_and_nothing_else() {
        let key = SshKeyPair::generate(SshAlgorithm::Ed25519, "first").unwrap();
        let blob = key.public_blob().unwrap();
        let moved = key.with_comment("  second  ").unwrap();
        assert_eq!(moved.comment, "second");
        assert_eq!(
            moved.public_key_openssh,
            public_line("ssh-ed25519", &blob, "second")
        );
        assert_eq!(moved.fingerprint_sha256, key.fingerprint_sha256);
        assert_eq!(moved.private_key_pem, key.private_key_pem);
        assert!(
            !moved
                .with_comment("")
                .unwrap()
                .public_key_openssh
                .contains("second")
        );
    }

    #[test]
    fn unsupported_material_is_refused_not_guessed() {
        assert_eq!(
            SshKeyPair::from_pem("").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        assert_eq!(
            SshKeyPair::from_pem("not a key").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        // PKCS#8 sits outside the shared contract.
        assert_eq!(
            SshKeyPair::from_pem("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n")
                .unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        assert_eq!(
            pem_body("-----BEGIN RSA PRIVATE KEY-----\nzz\n", HEADER_PKCS1).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        let mut oversized = "x".repeat(MAX_KEY_INPUT_BYTES + 1);
        oversized.truncate(MAX_KEY_INPUT_BYTES + 1);
        assert_eq!(
            SshKeyPair::from_pem(&oversized).unwrap_err(),
            GatewayError::KeyPayloadTooLarge
        );
    }

    #[test]
    fn algorithm_selection_follows_androids_choices() {
        assert_eq!(
            SshAlgorithm::parse("ed25519").unwrap(),
            SshAlgorithm::Ed25519
        );
        assert_eq!(
            SshAlgorithm::parse(" RSA ").unwrap(),
            SshAlgorithm::Rsa(DEFAULT_RSA_KEY_SIZE)
        );
        assert_eq!(
            SshAlgorithm::parse("rsa4096").unwrap(),
            SshAlgorithm::Rsa(4096)
        );
        assert_eq!(
            SshAlgorithm::parse("rsa1024").unwrap(),
            SshAlgorithm::Rsa(1024)
        );
        // Generating stays inside the sizes the Android UI offers.
        assert_eq!(
            SshKeyPair::generate(SshAlgorithm::Rsa(1024), "").unwrap_err(),
            GatewayError::InvalidRequest
        );
        assert_eq!(
            SshAlgorithm::parse("ecdsa").unwrap_err(),
            GatewayError::InvalidRequest
        );
        assert_eq!(
            SshAlgorithm::parse("rsa-wide").unwrap_err(),
            GatewayError::InvalidRequest
        );
    }

    #[test]
    fn stored_record_keeps_the_schema_and_format_android_expects() {
        let key = SshKeyPair::generate(SshAlgorithm::Ed25519, "s").unwrap();
        let data = key.to_data();
        assert_eq!(data.schema, SCHEMA_V1);
        assert_eq!(data.format, FORMAT_OPENSSH);
        let raw = data.to_json_string();
        let back = SshKeyData::decode(&raw).unwrap().unwrap();
        assert_eq!(back, data);
        assert_eq!(back.private_key_openssh, key.private_key_pem);
    }
}
