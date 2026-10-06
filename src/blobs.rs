//! Encrypted Blob preservation using the engine's provider, reference and transfer APIs.
use crate::{
    config::{ConfigStore, private_file},
    error::{GatewayError, Result},
    sync::download_file,
    vault::Vault,
    webdav::{MAX_VAULT_BYTES, WebDavClient},
};
use mdbx_storage::{
    blob_lifecycle::{BlobLifecycleLimits, collect_external_blob_references},
    blob_store::{
        EncryptedBlobStore, FileSystemBlobStore, ManageableEncryptedBlobStore, validate_blob_id,
    },
    blob_transfer::{BlobTransferLimits, BlobTransferService},
};
use std::{collections::BTreeMap, io::Write, path::Path};

const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

// The engine filesystem-provider namespace, also used by Android FFI.
// Resolving it does not open or migrate the source database.
fn provider(path: &Path) -> FileSystemBlobStore {
    FileSystemBlobStore::new(format!("{}.blobs", path.display()))
}

pub(crate) fn has_blobs(path: &Path) -> Result<bool> {
    Ok(!provider(path)
        .list(None, 1)
        .map_err(|_| GatewayError::BlobUnavailable)?
        .blobs
        .is_empty())
}

pub(crate) fn copy_all(source: &Path, destination: &Path) -> Result<()> {
    let source = provider(source);
    let destination = provider(destination);
    let mut cursor = None;
    let mut count = 0;
    let mut total = 0_u64;
    loop {
        let page = source
            .list(cursor.as_deref(), 100)
            .map_err(|_| GatewayError::BlobUnavailable)?;
        for metadata in page.blobs {
            count += 1;
            total = total
                .checked_add(metadata.stored_size)
                .ok_or(GatewayError::ResponseTooLarge)?;
            if count > 100_000 || metadata.stored_size > MAX_VAULT_BYTES || total > MAX_TOTAL_BYTES
            {
                return Err(GatewayError::ResponseTooLarge);
            }
            std::fs::create_dir_all(destination.root())
                .map_err(|_| GatewayError::StateUnavailable)?;
            private_file(destination.root())?;
            let result = BlobTransferService::transfer(
                &source,
                &destination,
                &metadata.blob_id,
                metadata.stored_size,
                &format!("monica-copy-{}", uuid::Uuid::new_v4()),
                None,
                BlobTransferLimits {
                    max_blob_bytes: MAX_VAULT_BYTES,
                    ..Default::default()
                },
            )
            .map_err(|_| GatewayError::BlobUnavailable)?;
            if !result.completed {
                return Err(GatewayError::BlobUnavailable);
            }
        }
        if page.next_cursor.is_none() {
            break;
        }
        if page.next_cursor == cursor {
            return Err(GatewayError::BlobUnavailable);
        }
        cursor = page.next_cursor;
    }
    Ok(())
}

fn references(vault: &Vault) -> Result<(FileSystemBlobStore, BTreeMap<String, usize>)> {
    let conn = vault
        .runtime
        .read()
        .map_err(|_| GatewayError::StateUnavailable)?;
    let provider = conn
        .external_blob_store()
        .map_err(|_| GatewayError::BlobUnavailable)?;
    let references = collect_external_blob_references(&conn, BlobLifecycleLimits::default())
        .map_err(|_| GatewayError::BlobUnavailable)?;
    Ok((provider, references.blobs))
}

pub(crate) fn verify(vault: &Vault) -> Result<()> {
    let (provider, references) = references(vault)?;
    let mut total = 0_u64;
    for (id, limit) in references {
        let bytes = provider
            .get(&id, limit.min(MAX_VAULT_BYTES as usize))
            .map_err(|_| GatewayError::BlobUnavailable)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
    }
    Ok(())
}

fn remote_path(root: &str, id: &str) -> Result<String> {
    validate_blob_id(id).map_err(|_| GatewayError::BlobUnavailable)?;
    Ok(format!("{root}/blobs/{}/{}/{id}", &id[..2], &id[2..4]))
}

pub(crate) async fn publish(
    vault: &Vault,
    store: &ConfigStore,
    client: &WebDavClient,
    root: &str,
) -> Result<()> {
    vault.require_remote_sync_allowed()?;
    let (provider, references) = references(vault)?;
    let mut total = 0_u64;
    for (id, limit) in references {
        let bytes = provider
            .get(&id, limit.min(MAX_VAULT_BYTES as usize))
            .map_err(|_| GatewayError::BlobUnavailable)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
        let mut outgoing = download_file(store)?;
        outgoing
            .file
            .write_all(&bytes)
            .map_err(|_| GatewayError::StateUnavailable)?;
        outgoing
            .file
            .as_file()
            .sync_all()
            .map_err(|_| GatewayError::StateUnavailable)?;
        let path = remote_path(root, &id)?;
        crate::segment::ensure_collections(client, &path).await?;
        // Both a repeated create and an acknowledged write must refer to the
        // exact ciphertext named by the content digest.
        client.create_immutable(&path, outgoing.path()).await?;
        let mut verification = download_file(store)?;
        let received = client.download(&path, &mut verification.file).await?;
        if received.sha256 != id || received.size != bytes.len() as u64 {
            return Err(GatewayError::BlobUnavailable);
        }
    }
    Ok(())
}

pub(crate) async fn receive(
    vault: &Vault,
    store: &ConfigStore,
    client: &WebDavClient,
    root: &str,
) -> Result<()> {
    vault.require_remote_sync_allowed()?;
    let (provider, references) = references(vault)?;
    let mut total = 0_u64;
    for (id, limit) in references {
        let path = provider
            .blob_path(&id)
            .map_err(|_| GatewayError::BlobUnavailable)?;
        if path.exists() {
            let bytes = provider
                .get(&id, limit.min(MAX_VAULT_BYTES as usize))
                .map_err(|_| GatewayError::BlobUnavailable)?;
            total += bytes.len() as u64;
        } else {
            let mut incoming = download_file(store)?;
            let remote = client
                .download(&remote_path(root, &id)?, &mut incoming.file)
                .await?;
            if remote.sha256 != id || remote.size > limit as u64 {
                return Err(GatewayError::BlobUnavailable);
            }
            total += remote.size;
            if total > MAX_TOTAL_BYTES {
                return Err(GatewayError::ResponseTooLarge);
            }
            let bytes =
                std::fs::read(incoming.path()).map_err(|_| GatewayError::StateUnavailable)?;
            std::fs::create_dir_all(provider.root()).map_err(|_| GatewayError::StateUnavailable)?;
            private_file(provider.root())?;
            provider
                .put(&id, &bytes)
                .map_err(|_| GatewayError::BlobUnavailable)?;
        }
        if total > MAX_TOTAL_BYTES {
            return Err(GatewayError::ResponseTooLarge);
        }
    }
    Ok(())
}
