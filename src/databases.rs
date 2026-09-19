//! Reopen saved database configurations without restoring old AI grants.
use crate::config::{Config, ConfigStore, read_json};
use crate::error::{GatewayError, Result};
use crate::vault::Vault;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Serialize)]
pub struct Database {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub current: bool,
}
fn entry(id: String, config: &Config, current: bool) -> Database {
    let source = config
        .webdav
        .as_ref()
        .map(|binding| std::path::Path::new(&binding.path))
        .unwrap_or(&config.vault);
    Database {
        id,
        name: config.database_name.clone().unwrap_or_else(|| {
            source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
        path: config.vault.clone(),
        current,
    }
}

pub fn list(store: &ConfigStore) -> Result<Vec<Database>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    if store.path.exists() {
        let current = store.load()?;
        seen.insert(current.vault.clone());
        result.push(entry("current".into(), &current, true));
    }
    let history = store.path.with_extension("history");
    if !history.exists() {
        return Ok(result);
    }
    let mut files = Vec::new();
    for file in std::fs::read_dir(history).map_err(|_| GatewayError::StateUnavailable)? {
        let file = file.map_err(|_| GatewayError::StateUnavailable)?;
        if files.len() >= 2048 {
            return Err(GatewayError::ResponseTooLarge);
        }
        let path = file.path();
        if !file
            .file_type()
            .map_err(|_| GatewayError::StateUnavailable)?
            .is_file()
            || path.extension().is_none_or(|e| e != "json")
        {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if uuid::Uuid::parse_str(id).is_err() {
            continue;
        }
        let modified = file
            .metadata()
            .and_then(|m| m.modified())
            .map_err(|_| GatewayError::StateUnavailable)?;
        files.push((modified, path));
    }
    files.sort_by(|a, b| b.cmp(a));
    for (_, path) in files {
        let saved: Config = read_json(&path, 256 * 1024)?;
        saved.validate()?;
        if !seen.insert(saved.vault.clone()) {
            continue;
        }
        result.push(entry(
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            &saved,
            false,
        ));
    }
    Ok(result)
}

pub fn switch(store: &ConfigStore, id: &str, password: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err(GatewayError::InvalidRequest);
    }
    let _guard = store.acquire_broker_lock()?;
    let saved: Config = read_json(
        &store
            .path
            .with_extension("history")
            .join(format!("{id}.json")),
        256 * 1024,
    )?;
    saved.validate()?;
    let vault = Vault::open(&saved.vault, password)?;
    let inventory = vault.gateway_inventory();
    vault.lock()?;
    let inventory = inventory?;
    let mut next = saved;
    next.connections = inventory.connections;
    next.collection_id = inventory.collection_id;
    next.grants.clear();
    store.update(|previous| {
        if let Some(previous) = previous {
            crate::sync::remember_previous(store, &previous)?;
            next.listen = previous.listen;
        }
        Ok((next, ()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creating_another_database_preserves_previous_and_rejects_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path().join("gateway.json"));
        let first = temp.path().join("first.mdbx");
        let second = temp.path().join("second.mdbx");
        crate::admin::initialize(&store, &first, 17321, "first", "first").unwrap();
        crate::admin::initialize(&store, &second, 17321, "second", "second").unwrap();
        assert_eq!(store.load().unwrap().vault, second);
        let databases = list(&store).unwrap();
        assert_eq!(databases.len(), 2);
        assert!(crate::admin::initialize(&store, &first, 17321, "new", "new").is_err());
        assert_eq!(store.load().unwrap().vault, second);
        switch(&store, &databases[1].id, "first").unwrap();
        assert_eq!(store.load().unwrap().vault, first);
        assert!(second.is_file());
    }

    #[test]
    fn database_switch_preserves_files_and_never_restores_old_grants() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path().join("gateway.json"));
        crate::admin::quick_add(
            &store,
            &crate::admin::AddOptions {
                name: "work".into(),
                title: String::new(),
                provider: crate::model::Provider::Github,
                api_base: None,
                note: String::new(),
                repositories: vec!["org/repo".into()],
                allow_write: false,
                ttl_minutes: 0,
            },
            "first-password",
            Some("first-password"),
            zeroize::Zeroizing::new("synthetic-database-token".into()),
        )
        .unwrap();
        let first = store.load().unwrap().vault;
        assert_eq!(store.load().unwrap().grants.len(), 1);
        let second = temp.path().join("second.mdbx");
        let vault =
            Vault::create(&second, "second-password", mdbx_core::tiga::TigaMode::Multi).unwrap();
        vault.lock().unwrap();
        drop(vault);
        crate::sync::open_local(&store, &second, "second-password").unwrap();
        let current = store.load().unwrap().vault;
        let databases = list(&store).unwrap();
        assert_eq!(databases.len(), 2);
        assert_eq!(databases[0].name, "second");
        let old = databases.iter().find(|db| db.path == first).unwrap();
        assert!(switch(&store, &old.id, "wrong").is_err());
        assert_eq!(store.load().unwrap().vault, current);
        switch(&store, &old.id, "first-password").unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.vault, first);
        assert!(restored.grants.is_empty());
        assert!(restored.connections.contains_key("work"));
        assert!(current.is_file() && first.is_file() && second.is_file());
        assert_eq!(list(&store).unwrap().len(), 2);
        assert!(switch(&store, "../outside", "first-password").is_err());
    }
}
