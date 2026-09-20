/// Entry identity shared with Monica for Android.
///
/// Android derives native ids with `UUID.nameUUIDFromBytes`, which is MD5 over the raw
/// bytes with no namespace prefix. Standard UUID-v3 helpers prepend a namespace and would
/// produce a different id, so the derivation lives here and is pinned by test vectors.
use md5::{Digest, Md5};
use uuid::Uuid;

/// Java `UUID.nameUUIDFromBytes`: MD5 digest, version nibble 3, RFC 4122 variant.
pub fn name_uuid_from_bytes(input: &[u8]) -> Uuid {
    let mut bytes = <[u8; 16]>::from(Md5::digest(input));
    bytes[6] = (bytes[6] & 0x0F) | 0x30;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Logical id Android keeps in the payload as `monica_entry_id`.
pub fn new_logical_entry_id() -> String {
    format!("password:{}", Uuid::new_v4())
}

/// Native entry id for a logical id inside one vault.
pub fn physical_entry_id(vault_id: &str, logical_entry_id: &str) -> Uuid {
    name_uuid_from_bytes(format!("monica-entry:{vault_id}:{logical_entry_id}").as_bytes())
}

/// Native collection id of Android's root folder.
pub fn root_collection_id(vault_id: &str) -> Uuid {
    name_uuid_from_bytes(format!("monica-root:{vault_id}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_id_matches_the_android_vector() {
        let vault = "11111111-1111-1111-1111-111111111111";
        let logical = "password:22222222-2222-2222-2222-222222222222";
        assert_eq!(
            physical_entry_id(vault, logical).to_string(),
            "48f94648-e2a0-3d3d-91b8-77ff9895b136"
        );
    }

    #[test]
    fn derived_ids_are_version_3_and_stable() {
        let vault = "11111111-1111-1111-1111-111111111111";
        let root = root_collection_id(vault);
        assert_eq!(root.get_version(), Some(uuid::Version::Md5));
        assert_eq!(root.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(
            root,
            name_uuid_from_bytes(format!("monica-root:{vault}").as_bytes())
        );
    }
}
