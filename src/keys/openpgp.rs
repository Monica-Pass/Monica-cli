//! OpenPGP v4 ASCII-armor reader used to validate imports and derive the public certificate.
//!
//! Monica for Android keeps the armored text itself: the private armor goes to `password_plain`
//! and the public armor is stored in Base64 chunks. This module therefore only reads far enough
//! to (a) prove the text really is one key ring, (b) reproduce the 40-hex primary fingerprint
//! Android shows, and (c) recover the public packets when the user supplies only a secret key.
//! Nothing is rewritten: a public ring is stored exactly as it arrived.
//!
//! Deliberate limits, all reported as `InvalidKeyMaterial`: v4 packets only (a v6 fingerprint
//! needs SHA3-256, which this crate does not carry), no passphrase-protected secret material, and
//! no compressed wrapper.
use base64::Engine as _;
use sha1::{Digest as _, Sha1};

use super::limits::MAX_GPG_IMPORT_BYTES;
pub use super::payload::GpgCertificate;
use crate::error::{GatewayError, Result};

const PKT_PUBLIC_KEY: u8 = 6;
const PKT_SECRET_KEY: u8 = 5;
const PKT_SECRET_SUBKEY: u8 = 7;
const PKT_COMPRESSED: u8 = 8;
const PKT_USER_ID: u8 = 13;
const PKT_PUBLIC_SUBKEY: u8 = 14;
const PKT_USER_ATTR: u8 = 17;
const PUBLIC_LABEL: &str = "PGP PUBLIC KEY BLOCK";
const SECRET_LABEL: &str = "PGP PRIVATE KEY BLOCK";
const ARMOR_LINE: usize = 64;
const CRC_POLYNOMIAL: u32 = 0x1864CFB;

/// A key ring read out of armor: identity plus the two text forms the vault stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpgKey {
    pub fingerprint: String,
    pub user_id: String,
    pub algorithm: String,
    pub bits: Option<i64>,
    pub has_secret: bool,
    pub public_armor: String,
    /// The armor exactly as supplied, when it carried secret material.
    pub secret_armor: Option<String>,
}

impl GpgKey {
    pub fn certificate(&self) -> GpgCertificate {
        GpgCertificate {
            fingerprint: self.fingerprint.clone(),
            user_id: self.user_id.clone(),
            public_armor: self.public_armor.clone(),
        }
    }
}

/// Reads one armor block, deriving the public ring from a secret ring when needed.
pub fn read(armor: &str) -> Result<GpgKey> {
    if armor.len() > MAX_GPG_IMPORT_BYTES {
        return Err(GatewayError::KeyPayloadTooLarge);
    }
    let (label, body) = dearmor(armor)?;
    let has_secret = match label.as_str() {
        PUBLIC_LABEL => false,
        SECRET_LABEL => true,
        _ => return Err(GatewayError::InvalidKeyMaterial),
    };
    let packets = packets(&body)?;
    let (tag, primary) = packets.first().ok_or(GatewayError::InvalidKeyMaterial)?;
    let expected = if has_secret {
        PKT_SECRET_KEY
    } else {
        PKT_PUBLIC_KEY
    };
    if *tag != expected || packets.iter().any(|(tag, _)| *tag == PKT_COMPRESSED) {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    let public_len = public_body_len(primary)?;
    if has_secret && primary.get(public_len) != Some(&0) {
        // S2K usage 96/97/253/254 means the private MPIs are encrypted under a passphrase.
        return Err(GatewayError::InvalidKeyMaterial);
    }
    let (algorithm, bits) = describe(&primary[..public_len])?;
    let digest = Sha1::digest(v4_hash(&primary[..public_len]));
    Ok(GpgKey {
        fingerprint: upper_hex(&digest),
        user_id: packets
            .iter()
            .find(|(tag, _)| *tag == PKT_USER_ID)
            .map(|(_, body)| String::from_utf8_lossy(body).into_owned())
            .unwrap_or_default(),
        algorithm,
        bits,
        has_secret,
        public_armor: if has_secret {
            armor_of(PUBLIC_LABEL, &public_ring(&packets)?)
        } else {
            armor.to_owned()
        },
        secret_armor: has_secret.then(|| armor.to_owned()),
    })
}

/// The bytes a v4 fingerprint is taken over: `0x99`, the body length, then the body.
fn v4_hash(public_body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(public_body.len() + 3);
    data.push(0x99);
    data.extend_from_slice(&(public_body.len() as u16).to_be_bytes());
    data.extend_from_slice(public_body);
    data
}

/// Rebuilds a public ring from a secret ring: key packets lose their private tail.
fn public_ring(packets: &[(u8, &[u8])]) -> Result<Vec<u8>> {
    let mut public = Vec::with_capacity(packets.len());
    for (tag, body) in packets {
        match *tag {
            PKT_SECRET_KEY | PKT_SECRET_SUBKEY => {
                let end = public_body_len(body)?;
                let target = if *tag == PKT_SECRET_KEY {
                    PKT_PUBLIC_KEY
                } else {
                    PKT_PUBLIC_SUBKEY
                };
                public.push((target, &body[..end]));
            }
            PKT_PUBLIC_KEY | PKT_PUBLIC_SUBKEY | PKT_USER_ID | PKT_USER_ATTR | 2 => {
                public.push((*tag, body));
            }
            _ => {} // markers, PKESK and SKESK are not part of the certificate
        }
    }
    if public.first().map(|(tag, _)| *tag) != Some(PKT_PUBLIC_KEY) {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    Ok(encode_packets(&public))
}

fn encode_packets(packets: &[(u8, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (tag, body) in packets {
        out.push(0xC0 | tag);
        let len = body.len();
        if len < 192 {
            out.push(len as u8);
        } else if len < 8384 {
            let shifted = len - 192;
            out.push((192 + (shifted >> 8)) as u8);
            out.push((shifted & 0xFF) as u8);
        } else {
            out.push(0xFF);
            out.extend_from_slice(&(len as u32).to_be_bytes());
        }
        out.extend_from_slice(body);
    }
    out
}

/// Bytes the public key fields occupy inside a v4 key or secret key body.
fn public_body_len(body: &[u8]) -> Result<usize> {
    if body.len() < 7 || body[0] != 4 {
        return Err(GatewayError::InvalidKeyMaterial); // v6 fingerprints use SHA3-256.
    }
    let after = match body[5] {
        1 => skip_mpi(&body[6..], 2),
        16 | 21 => skip_mpi(&body[6..], 3),
        17 => skip_mpi(&body[6..], 4),
        // Elliptic-curve keys lead with a length-prefixed OID, not an MPI, and ECDH
        // appends a length-prefixed KDF field after the point (RFC 9580 §5.5.5.6).
        18 => skip_oid(&body[6..])
            .and_then(|rest| skip_mpi(rest, 1))
            .and_then(skip_oid),
        19 | 22 => skip_oid(&body[6..]).and_then(|rest| skip_mpi(rest, 1)),
        _ => Err(GatewayError::InvalidKeyMaterial),
    }?;
    Ok(body.len() - after.len())
}

/// Skips a one-octet length followed by that many bytes: the curve OID, and the ECDH
/// KDF field, which share the encoding.
fn skip_oid(data: &[u8]) -> Result<&[u8]> {
    let len = usize::from(*data.first().ok_or(GatewayError::InvalidKeyMaterial)?);
    data.get(1 + len..).ok_or(GatewayError::InvalidKeyMaterial)
}

fn skip_mpi(data: &[u8], count: usize) -> Result<&[u8]> {
    let mut rest = data;
    for _ in 0..count {
        let bits = u16::from_be_bytes(two(rest)?);
        let bytes = usize::from(bits).div_ceil(8);
        rest = rest
            .get(2 + bytes..)
            .ok_or(GatewayError::InvalidKeyMaterial)?;
    }
    Ok(rest)
}

fn two(data: &[u8]) -> Result<[u8; 2]> {
    data.get(..2)
        .and_then(|b| b.try_into().ok())
        .ok_or(GatewayError::InvalidKeyMaterial)
}

fn describe(public_body: &[u8]) -> Result<(String, Option<i64>)> {
    let algorithm = match public_body[5] {
        1 => "RSA",
        16 => "ELGAMAL",
        17 => "DSA",
        18 => "ECDH",
        19 => "ECDSA",
        21 => "DH",
        22 => "EDDSA",
        _ => return Err(GatewayError::InvalidKeyMaterial),
    };
    let bits = if algorithm == "RSA" {
        Some(i64::from(u16::from_be_bytes(two(&public_body[6..])?)))
    } else {
        None
    };
    Ok((algorithm.to_owned(), bits))
}

/// Splits an armor block into its label and decoded bytes, checking the CRC24 trailer.
fn dearmor(text: &str) -> Result<(String, Vec<u8>)> {
    let (_, after) = text
        .split_once("-----BEGIN ")
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    let (label, rest) = after
        .split_once("-----")
        .ok_or(GatewayError::InvalidKeyMaterial)?;
    let label = label.trim().to_owned();
    let Some(end) = rest.find("-----END ") else {
        return Err(GatewayError::InvalidKeyMaterial);
    };
    let (payload, tail) = rest.split_at(end);
    if !tail.contains(&format!("END {label}")) || label.is_empty() {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    let mut lines = payload.lines().map(str::trim).peekable();
    // Armor headers (`Version:`, `Comment:`, …) precede the blank separator line.
    while lines
        .peek()
        .is_some_and(|line| !line.is_empty() && line.contains(": "))
    {
        lines.next();
    }
    let mut checksum = None;
    let mut encoded = String::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        match line.strip_prefix('=') {
            // A body line never starts with '=', so this is the CRC24 trailer.
            Some(digits) => checksum = Some(digits.to_owned()),
            None => encoded.push_str(line),
        }
    }
    let bytes = decode_base64(&encoded)?;
    let expected = checksum.as_deref().map(decode_base64).transpose()?;
    let crc = crc24(&bytes).to_be_bytes();
    if expected
        .as_deref()
        .is_some_and(|expected| expected != &crc[1..])
    {
        return Err(GatewayError::InvalidKeyMaterial);
    }
    Ok((label, bytes))
}

/// Radix-64 with the OpenPGP CRC24 trailer, line-wrapped at 64 characters.
fn armor_of(label: &str, body: &[u8]) -> String {
    let encoded = encode_base64(body);
    let checksum = encode_base64(&crc24(body).to_be_bytes()[1..]);
    let mut text = format!("-----BEGIN {label}-----\n\n");
    for chunk in encoded.as_bytes().chunks(ARMOR_LINE) {
        text.push_str(core::str::from_utf8(chunk).unwrap_or_default());
        text.push('\n');
    }
    text.push('=');
    text.push_str(&checksum);
    text.push('\n');
    text.push_str(&format!("-----END {label}-----\n"));
    text
}

fn crc24(data: &[u8]) -> u32 {
    let mut crc: u32 = 0x00B7_04CE;
    for byte in data {
        crc ^= u32::from(*byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x1_00_00_00 != 0 {
                crc ^= CRC_POLYNOMIAL;
            }
        }
    }
    crc & 0xFF_FF_FF
}

/// Walks the packet stream, tolerating old and new packet headers.
fn packets(data: &[u8]) -> Result<Vec<(u8, &[u8])>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < data.len() {
        let header = data[cursor];
        if header & 0x80 == 0 {
            return Err(GatewayError::InvalidKeyMaterial);
        }
        cursor += 1;
        let (tag, len) = if header & 0x40 == 0 {
            let tag = (header >> 2) & 0x0F;
            let (len, octets) = match header & 0x03 {
                0 => (usize::from(*at(data, cursor, 1)?), 1),
                1 => (
                    usize::from(u16::from_be_bytes([
                        *at(data, cursor, 1)?,
                        *at(data, cursor, 2)?,
                    ])),
                    2,
                ),
                2 => (
                    usize::try_from(u32::from_be_bytes([
                        *at(data, cursor, 1)?,
                        *at(data, cursor, 2)?,
                        *at(data, cursor, 3)?,
                        *at(data, cursor, 4)?,
                    ]))
                    .map_err(|_| GatewayError::InvalidKeyMaterial)?,
                    4,
                ),
                _ => (data.len() - cursor, 0),
            };
            cursor += octets;
            (tag, len)
        } else {
            let tag = header & 0x3F;
            let mut total = 0usize;
            loop {
                let first = *at(data, cursor, 1)?;
                cursor += 1;
                if first < 192 {
                    total += usize::from(first);
                    break;
                }
                if first < 224 {
                    let second = usize::from(*at(data, cursor, 1)?);
                    cursor += 1;
                    total += ((usize::from(first) - 192) << 8) + second + 192;
                    break;
                }
                if first == 255 {
                    let bytes: [u8; 4] = data
                        .get(cursor..cursor + 4)
                        .and_then(|b| b.try_into().ok())
                        .ok_or(GatewayError::InvalidKeyMaterial)?;
                    cursor += 4;
                    total += usize::try_from(u32::from_be_bytes(bytes))
                        .map_err(|_| GatewayError::InvalidKeyMaterial)?;
                    break;
                }
                total += 1usize << (first & 0x1F); // partial length: another chunk follows
            }
            (tag, total)
        };
        let end = cursor
            .checked_add(len)
            .filter(|end| *end <= data.len())
            .ok_or(GatewayError::InvalidKeyMaterial)?;
        out.push((tag, &data[cursor..end]));
        cursor = end;
    }
    Ok(out)
}

fn at(data: &[u8], index: usize, offset: usize) -> Result<&u8> {
    index
        .checked_add(offset - 1)
        .and_then(|index| data.get(index))
        .ok_or(GatewayError::InvalidKeyMaterial)
}

fn upper_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .map_err(|_| GatewayError::InvalidKeyMaterial)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real GnuPG 2.4.7 RSA-2048 + RSA-2048-subkey certificate, public material only.
    const PUBLIC: &str = include_str!("../../tests/fixtures/gpg/rsa2048-public.asc");
    const FINGERPRINT: &str = "FED3E5D44B8C5C711E0B30004B591683D08EC385";

    /// A real GnuPG 2.4.7 Ed25519 certificate pair. Its curve OID travels as an MPI, so
    /// the public key fields are two MPIs rather than an OID and a point.
    const ED25519_PUBLIC: &str = include_str!("../../tests/fixtures/gpg/ed25519-public.asc");
    const ED25519_SECRET: &str = include_str!("../../tests/fixtures/gpg/ed25519-secret.asc");
    const ED25519_FINGERPRINT: &str = "837A7DFF0F73A85EAE2D04E6B0B4C3983F688DA1";

    /// The public packet of `PUBLIC`, as the bytes the fingerprint covers.
    fn public_packet() -> Vec<u8> {
        let (_, body) = dearmor(PUBLIC).unwrap();
        packets(&body).unwrap()[0].1.to_vec()
    }

    /// Wraps a public key packet into a secret key packet: S2K usage 0, dummy private MPIs.
    fn secret_armor_from_public() -> String {
        let mut secret = public_packet();
        secret.push(0); // no passphrase protection
        for bits in [2043u16, 1024, 1024, 1023] {
            secret.extend_from_slice(&bits.to_be_bytes());
            secret.extend_from_slice(&vec![0xAA; usize::from(bits).div_ceil(8)]);
        }
        secret.extend_from_slice(&[0x3E, 0xAC]); // the checksum gpg reports for this key
        let mut ring = encode_old_format(PKT_SECRET_KEY, &secret);
        // Keep the user ID packet so the derived ring still carries an identity.
        let (_, all) = dearmor(PUBLIC).unwrap();
        let packets = packets(&all).unwrap();
        for (tag, body) in &packets[1..] {
            if *tag == PKT_USER_ID {
                ring.push(0xCD);
                ring.push(body.len() as u8);
                ring.extend_from_slice(body);
            }
        }
        armor_of(SECRET_LABEL, &ring)
    }

    #[test]
    fn a_public_ring_reads_with_gnupgs_fingerprint() {
        let key = read(PUBLIC).unwrap();
        assert_eq!(key.fingerprint, FINGERPRINT);
        assert_eq!(key.user_id, "Monica Test <monica@example.com>");
        assert_eq!(key.algorithm, "RSA");
        assert_eq!(key.bits, Some(2048));
        assert!(!key.has_secret);
        assert!(key.secret_armor.is_none());
        // The armor a public ring arrives in is the armor that gets stored.
        assert_eq!(key.public_armor, PUBLIC);
    }

    #[test]
    fn a_secret_ring_yields_the_same_certificate_and_keeps_its_text() {
        let armor = secret_armor_from_public();
        let key = read(&armor).unwrap();
        assert!(key.has_secret);
        assert_eq!(key.fingerprint, FINGERPRINT);
        assert_eq!(key.user_id, "Monica Test <monica@example.com>");
        assert_eq!(key.secret_armor.as_deref(), Some(armor.as_str()));
        assert_ne!(key.public_armor, armor);
        // The derived certificate re-reads as a plain public ring with the same identity.
        let again = read(&key.public_armor).unwrap();
        assert!(!again.has_secret);
        assert_eq!(again.fingerprint, key.fingerprint);
        assert_eq!(again.public_armor, key.public_armor);
        assert_eq!(again.certificate(), key.certificate());
    }

    #[test]
    fn an_eddsa_ring_reads_with_gnupgs_fingerprint() {
        let public = read(ED25519_PUBLIC).unwrap();
        assert_eq!(public.fingerprint, ED25519_FINGERPRINT);
        assert_eq!(public.algorithm, "EDDSA");
        assert_eq!(public.bits, None);
        assert_eq!(public.user_id, "Cli Fixture <cli-fixture@example.com>");

        let secret = read(ED25519_SECRET).unwrap();
        assert!(secret.has_secret);
        assert_eq!(secret.fingerprint, ED25519_FINGERPRINT);
        assert_eq!(secret.secret_armor.as_deref(), Some(ED25519_SECRET));
        // The certificate rebuilt from the secret ring re-reads as gpg's own export.
        let derived = read(&secret.public_armor).unwrap();
        assert!(!derived.has_secret);
        assert_eq!(derived.fingerprint, public.fingerprint);
        assert_eq!(derived.user_id, public.user_id);
        assert_eq!(derived.algorithm, public.algorithm);
    }

    #[test]
    fn armor_checksum_and_labels_are_enforced() {
        // Changing one body character leaves valid Base64 but a stale CRC24 trailer.
        let lines: Vec<&str> = PUBLIC.lines().collect();
        let body = lines
            .iter()
            .position(|line| {
                !line.is_empty()
                    && !line.starts_with("-----")
                    && !line.starts_with('=')
                    && !line.contains(": ")
            })
            .expect("an armor body line");
        let head = if lines[body].starts_with('Q') {
            'R'
        } else {
            'Q'
        };
        let broken = format!(
            "{}\n{head}{}\n{}",
            lines[..body].join("\n"),
            &lines[body][1..],
            lines[body + 1..].join("\n")
        );
        assert_ne!(broken, PUBLIC);
        assert_eq!(read(&broken).unwrap_err(), GatewayError::InvalidKeyMaterial);
        let wrong_end = PUBLIC.replace("END PGP PUBLIC KEY", "END PGP PRIVATE KEY");
        assert_eq!(
            read(&wrong_end).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        assert_eq!(
            read("-----BEGIN PGP MESSAGE-----\n\nAAAA\n-----END PGP MESSAGE-----\n").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        assert_eq!(
            read("not armor").unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        assert_eq!(
            read(PUBLIC.split_once('\n').unwrap().0).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
    }

    #[test]
    fn crc24_matches_the_openpgp_test_vector() {
        // The 18-bit checksum of "123456789" is the standard check value 0x21CF02.
        assert_eq!(crc24(b"123456789"), 0x21_CF_02);
        // Re-armed bytes read back as the same key, so the writer matches the reader.
        let (_, bytes) = dearmor(PUBLIC).unwrap();
        let again = read(&armor_of(PUBLIC_LABEL, &bytes)).unwrap();
        assert_eq!(again.fingerprint, FINGERPRINT);
        assert_eq!(again.user_id, read(PUBLIC).unwrap().user_id);
    }

    #[test]
    fn derived_armor_reuses_the_reader_and_respects_line_width() {
        let armor = secret_armor_from_public();
        let body = armor
            .lines()
            .filter(|line| !line.starts_with("-----") && !line.is_empty() && !line.starts_with('='))
            .map(str::len)
            .collect::<Vec<_>>();
        assert!(body.len() > 2);
        assert!(body[..body.len() - 1].iter().all(|len| *len == ARMOR_LINE));
        let (_, bytes) = dearmor(&armor).unwrap();
        assert!(bytes.len() > 300);
    }

    #[test]
    fn protected_and_unsupported_rings_are_refused() {
        let mut secret = public_packet();
        secret.push(254); // AEAD-protected secret MPIs
        secret.extend_from_slice(&[0u8; 40]);
        let protected = armor_of(SECRET_LABEL, &encode_old_format(PKT_SECRET_KEY, &secret));
        assert_eq!(
            read(&protected).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        // A v6 key packet would need a SHA3-256 fingerprint.
        let mut v6 = public_packet();
        v6[0] = 6;
        let ring = encode_old_format(PKT_PUBLIC_KEY, &v6);
        assert_eq!(
            read(&armor_of(PUBLIC_LABEL, &ring)).unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
        // Unknown public-key algorithms cannot be measured, so they are not accepted.
        let mut unknown = public_packet();
        unknown[5] = 99;
        assert_eq!(
            read(&armor_of(
                PUBLIC_LABEL,
                &encode_old_format(PKT_PUBLIC_KEY, &unknown)
            ))
            .unwrap_err(),
            GatewayError::InvalidKeyMaterial
        );
    }

    fn encode_old_format(tag: u8, body: &[u8]) -> Vec<u8> {
        // Length type 01: tag plus two length octets, the form GnuPG uses for key packets.
        let mut out = vec![
            0x80 | (tag << 2) | 0x01,
            (body.len() >> 8) as u8,
            body.len() as u8,
        ];
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn oversized_armor_is_refused_before_decoding() {
        let text = "A".repeat(MAX_GPG_IMPORT_BYTES + 1);
        assert_eq!(read(&text).unwrap_err(), GatewayError::KeyPayloadTooLarge);
    }

    #[test]
    fn packet_headers_of_both_formats_are_walked() {
        let (_, bytes) = dearmor(PUBLIC).unwrap();
        let walked = packets(&bytes).unwrap();
        assert_eq!(walked[0].0, PKT_PUBLIC_KEY);
        assert_eq!(walked[1].0, PKT_USER_ID);
        let new_format = encode_packets(&walked.iter().map(|(t, b)| (*t, *b)).collect::<Vec<_>>());
        let reread = packets(&new_format).unwrap();
        assert_eq!(reread.len(), walked.len());
        assert!(
            reread
                .iter()
                .zip(&walked)
                .all(|((tag, body), (other, other_body))| tag == other && body == other_body)
        );
        // Truncated streams fail closed instead of yielding a shorter ring.
        assert!(packets(&new_format[..new_format.len() - 5]).is_err());
        assert!(packets(&[0x00]).is_err());
    }
}
