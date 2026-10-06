//! Deny-only client compatibility checks. An unauthenticated header is never authorization.
//! The native engine owns all policy decisions, key handling and unlock validation.
use std::path::Path;

use mdbx_core::tiga::{GLITTER_POLICY_VERSION, TigaMode};
use mdbx_storage::migration::GLITTER_EXTENSION;
use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::error::{GatewayError, Result};

#[derive(Default)]
pub(crate) struct HeaderHint {
    pub declared_profile: Option<String>,
    pub is_glitter: bool,
}

pub(crate) fn require_terminal_mode(mode: TigaMode) -> Result<()> {
    if mode == TigaMode::Glitter {
        Err(GatewayError::GlitterUnavailable)
    } else {
        Ok(())
    }
}

pub(crate) fn require_terminal_file(path: &Path) -> Result<()> {
    if inspect_header(path)?.is_glitter {
        Err(GatewayError::GlitterUnavailable)
    } else {
        Ok(())
    }
}

pub(crate) fn require_remote_file(path: &Path) -> Result<()> {
    // Preserve the transport's existing error for malformed downloaded database bytes.
    if inspect_header(path)
        .map_err(|_| GatewayError::InvalidVault)?
        .is_glitter
    {
        Err(GatewayError::GlitterUnavailable)
    } else {
        Ok(())
    }
}

/// Read only public header declarations, before any writable engine connection is opened.
/// Legacy headers missing these columns remain the engine's responsibility. A single Glitter
/// marker is sufficient to refuse: inconsistent/malicious headers cannot make us attempt KDF.
/// No raw header text reaches output, and SQL bounds text before Rust materializes it.
pub(crate) fn inspect_header(path: &Path) -> Result<HeaderHint> {
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| GatewayError::VaultFileUnreadable)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| GatewayError::VaultFileUnreadable)?;
    let tx = conn
        .transaction()
        .map_err(|_| GatewayError::VaultFileUnreadable)?;
    let has_column = |name: &str| -> Result<bool> {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('vault_meta') WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(|_| GatewayError::VaultFileUnreadable)
    };
    let has_mode = has_column("default_tiga_mode")?;
    let has_extensions = has_column("critical_extensions")?;
    let has_version = has_column("tiga_policy_version")?;
    if !has_mode && !has_extensions && !has_version {
        return Ok(HeaderHint::default());
    }
    // Only these fixed SQL fragments are interpolated, never identifiers or file contents.
    let mode = if has_mode {
        "CASE default_tiga_mode WHEN 'sky' THEN 'sky' WHEN 'multi' THEN 'multi' \
         WHEN 'power' THEN 'power' WHEN 'glitter' THEN 'glitter' ELSE NULL END"
    } else {
        "NULL"
    };
    let extensions = if has_extensions {
        "CASE WHEN length(CAST(critical_extensions AS BLOB)) <= 4096 \
         THEN critical_extensions ELSE NULL END"
    } else {
        "'[]'"
    };
    let version = if has_version {
        "tiga_policy_version"
    } else {
        "NULL"
    };
    let row: Option<(Option<String>, Option<String>, Option<i64>)> = tx
        .query_row(
            &format!("SELECT {mode}, {extensions}, {version} FROM vault_meta LIMIT 1"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| GatewayError::VaultFileUnreadable)?;
    let Some((declared_profile, extensions, version)) = row else {
        return Ok(HeaderHint::default());
    };
    let declared_glitter = declared_profile.as_deref() == Some("glitter")
        || version == Some(i64::from(GLITTER_POLICY_VERSION));
    let extensions = extensions.ok_or(GatewayError::VaultFileUnreadable)?;
    // Match the native legacy encoding too: empty text and comma-separated names predate
    // JSON arrays. Unknown names remain for the engine to reject, never to authorize here.
    let extensions = extensions.trim();
    let extensions: Vec<String> = if extensions.is_empty() {
        Vec::new()
    } else if extensions.starts_with('[') {
        serde_json::from_str(extensions).map_err(|_| GatewayError::VaultFileUnreadable)?
    } else {
        extensions.split(',').map(str::to_owned).collect()
    };
    if extensions.iter().any(|value| value.trim().is_empty()) {
        return Err(GatewayError::VaultFileUnreadable);
    }
    Ok(HeaderHint {
        declared_profile,
        is_glitter: declared_glitter
            || extensions
                .iter()
                .any(|value| value.trim() == GLITTER_EXTENSION),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdbx_storage::connection::VaultConnection;
    use mdbx_storage::init::{VaultInitParams, initialize_vault};

    fn fixture(root: &Path, sql: &str) -> std::path::PathBuf {
        let path = root.join("fixture.mdbx");
        let conn = VaultConnection::create(&path).unwrap();
        initialize_vault(&conn, &VaultInitParams::default()).unwrap();
        conn.inner().execute_batch(sql).unwrap();
        path
    }

    #[test]
    fn glitter_partial_markers_refuse_without_migrating_or_modifying_header() {
        for sql in [
            "UPDATE vault_meta SET default_tiga_mode='glitter', schema_version=1",
            "UPDATE vault_meta SET tiga_policy_version=3, schema_version=1",
            "UPDATE vault_meta SET critical_extensions='[\"tiga-glitter-v1\"]', schema_version=1",
            "UPDATE vault_meta SET critical_extensions=' field-key-epochs-v1, tiga-glitter-v1 ', schema_version=1",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = fixture(dir.path(), sql);
            let before = std::fs::read(&path).unwrap();
            assert_eq!(
                require_terminal_file(&path),
                Err(GatewayError::GlitterUnavailable)
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn glitter_header_read_is_bounded_and_never_reflects_untrusted_values() {
        for extension in [" ".repeat(4097), "[invalid-json]".into(), "[\"\"]".into()] {
            let dir = tempfile::tempdir().unwrap();
            let path = fixture(dir.path(), "");
            {
                let conn = Connection::open(&path).unwrap();
                conn.execute("UPDATE vault_meta SET critical_extensions=?1", [&extension])
                    .unwrap();
            }
            let before = std::fs::read(&path).unwrap();
            assert!(matches!(
                inspect_header(&path),
                Err(GatewayError::VaultFileUnreadable)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let dir = tempfile::tempdir().unwrap();
        let path = fixture(
            dir.path(),
            "UPDATE vault_meta SET default_tiga_mode='untrusted-secret-sentinel'",
        );
        let hint = inspect_header(&path).unwrap();
        assert_eq!(hint.declared_profile, None);
        assert!(!hint.is_glitter);
    }

    #[test]
    fn glitter_header_without_new_columns_remains_an_engine_decision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.mdbx");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE vault_meta(vault_id TEXT); INSERT INTO vault_meta VALUES ('legacy')",
            )
            .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let hint = inspect_header(&path).unwrap();
        assert_eq!(hint.declared_profile, None);
        assert!(!hint.is_glitter);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(inspect_header(&dir.path().join("missing.mdbx")).is_err());
        assert!(!dir.path().join("missing.mdbx").exists());
    }
}
