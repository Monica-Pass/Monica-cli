//! Local, unlocked database browsing. Never exposed by MCP or cached on disk.
use crate::config::{ConfigStore, Connection};
use crate::error::{GatewayError, Result};
use crate::vault::Vault;
use mdbx_storage::repo::{
    CollectionSummaryRepo, CommitContext, ObjectSummaryRepo, OperationCoordinator, WriteCommand,
    WriteOperationRequest,
};
use serde::Serialize;

#[cfg(test)]
mod tests {
    use super::*;
    use mdbx_core::tiga::TigaMode;
    #[test]
    fn nested_categories_and_moves_survive_reopen_and_reject_cycles() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.mdbx");
        let vault = Vault::create(&path, "test-password", TigaMode::Multi).unwrap();
        let root = vault.create_category("Work", None).unwrap();
        let child = vault.create_category("Projects", Some(&root)).unwrap();
        let other = vault.create_category("Archive", None).unwrap();
        assert!(vault.move_library_item(&root, &child).is_err());
        vault.move_library_item(&child, &other).unwrap();
        vault.rename_category(&child, "Renamed").unwrap();
        vault.lock().unwrap();
        assert!(vault.library().is_err());
        drop(vault);
        let reopened = Vault::open(&path, "test-password").unwrap();
        let library = reopened.library().unwrap();
        assert_eq!(library.categories.len(), 3);
        assert_eq!(
            library
                .categories
                .iter()
                .find(|c| c.id == child)
                .unwrap()
                .title,
            "Renamed"
        );
        let tree = library.category_tree();
        assert_eq!(
            tree.iter()
                .map(|(c, _)| c.title.as_str())
                .collect::<Vec<_>>(),
            ["Archive", "Renamed", "Work"]
        );
        assert_eq!(tree[1].1, 1);
        assert_eq!(
            library
                .categories
                .iter()
                .find(|c| c.id == child)
                .unwrap()
                .parent
                .as_deref(),
            Some(other.as_str())
        );
        assert!(
            reopened
                .create_category("Invalid", Some("missing"))
                .is_err()
        );
        reopened.lock().unwrap();
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Library {
    pub categories: Vec<Category>,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Serialize)]
pub struct Category {
    pub id: String,
    pub parent: Option<String>,
    pub title: String,
}
#[derive(Clone, Serialize)]
pub struct Entry {
    pub id: String,
    pub category: String,
    pub title: String,
    pub kind: String,
}

impl Library {
    /// Parent-first display order, bounded even for malformed imported cycles.
    pub fn category_tree(&self) -> Vec<(&Category, usize)> {
        let mut result = Vec::new();
        let mut visited = std::collections::BTreeSet::new();
        let mut stack: Vec<_> = self
            .categories
            .iter()
            .rev()
            .filter(|c| {
                c.parent
                    .as_ref()
                    .is_none_or(|id| !self.categories.iter().any(|p| &p.id == id))
            })
            .map(|c| (c, 0))
            .collect();
        loop {
            while let Some((category, depth)) = stack.pop() {
                if !visited.insert(category.id.as_str()) {
                    continue;
                }
                result.push((category, depth));
                stack.extend(
                    self.categories
                        .iter()
                        .rev()
                        .filter(|c| c.parent.as_deref() == Some(category.id.as_str()))
                        .map(|c| (c, depth + 1)),
                );
            }
            match self
                .categories
                .iter()
                .find(|c| !visited.contains(c.id.as_str()))
            {
                Some(category) => stack.push((category, 0)),
                None => break,
            }
        }
        result
    }
}
impl Vault {
    pub fn rename_category(&self, id: &str, title: &str) -> Result<()> {
        if title.trim().is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
            return Err(GatewayError::InvalidRequest);
        }
        if !self.library()?.categories.iter().any(|c| c.id == id) {
            return Err(GatewayError::NotFound);
        }
        self.library_write(WriteCommand::RenameProject {
            project_id: id.to_owned(),
            title: title.to_owned(),
        })
    }
    pub fn library(&self) -> Result<Library> {
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        if connection.keyring().is_none() || connection.active_session().is_none() {
            return Err(GatewayError::UnlockRequired);
        }
        let mut library = Library::default();
        let mut cursor = None;
        loop {
            let page = CollectionSummaryRepo::list_active(&connection, 100, cursor.as_deref())
                .map_err(|_| GatewayError::InvalidVault)?;
            for category in page.items {
                if library.categories.len() >= 2048 {
                    return Err(GatewayError::InvalidVault);
                }
                library.categories.push(Category {
                    id: category.collection_id.clone(),
                    parent: category.group_id,
                    title: String::from_utf8_lossy(&category.title).into_owned(),
                });
                let mut entries_cursor = None;
                loop {
                    let entries = ObjectSummaryRepo::list(
                        &connection,
                        &category.collection_id,
                        None,
                        100,
                        entries_cursor.as_deref(),
                    )
                    .map_err(|_| GatewayError::InvalidVault)?;
                    for entry in entries.items {
                        if library.entries.len() >= 20000 {
                            return Err(GatewayError::InvalidVault);
                        }
                        library.entries.push(Entry {
                            id: entry.object_id,
                            category: category.collection_id.clone(),
                            title: String::from_utf8_lossy(&entry.title.unwrap_or_default())
                                .into_owned(),
                            kind: entry.object_type_id.to_string(),
                        });
                    }
                    entries_cursor = entries.next_cursor;
                    if entries_cursor.is_none() {
                        break;
                    }
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        library.categories.sort_by_key(|c| c.title.to_lowercase());
        library.entries.sort_by_key(|e| e.title.to_lowercase());
        Ok(library)
    }
    pub fn create_category(&self, title: &str, parent: Option<&str>) -> Result<String> {
        if title.trim().is_empty() || title.len() > 256 || title.chars().any(char::is_control) {
            return Err(GatewayError::InvalidRequest);
        }
        let inventory = self.library()?;
        if parent.is_some_and(|id| !inventory.categories.iter().any(|c| c.id == id)) {
            return Err(GatewayError::NotFound);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.library_write(WriteCommand::CreateProjectWithParent {
            project_id: id.clone(),
            title: title.to_owned(),
            parent_project_id: parent.map(str::to_owned),
        })?;
        Ok(id)
    }
    pub fn move_library_item(&self, id: &str, target: &str) -> Result<()> {
        let inventory = self.library()?;
        if !inventory.categories.iter().any(|c| c.id == target) {
            return Err(GatewayError::NotFound);
        }
        let command = if let Some(entry) = inventory.entries.iter().find(|e| e.id == id) {
            WriteCommand::MoveEntry {
                entry_id: id.to_owned(),
                project_id: entry.category.clone(),
                target_project_id: target.to_owned(),
            }
        } else if inventory.categories.iter().any(|c| c.id == id) {
            let mut parent = Some(target);
            for _ in 0..=inventory.categories.len() {
                match parent {
                    Some(p) if p == id => return Err(GatewayError::InvalidRequest),
                    Some(p) => {
                        parent = inventory
                            .categories
                            .iter()
                            .find(|c| c.id == p)
                            .and_then(|c| c.parent.as_deref())
                    }
                    None => break,
                }
            }
            if parent.is_some() {
                return Err(GatewayError::InvalidRequest);
            }
            WriteCommand::MoveProject {
                project_id: id.to_owned(),
                parent_project_id: Some(target.to_owned()),
            }
        } else {
            return Err(GatewayError::NotFound);
        };
        self.library_write(command)
    }
    fn library_write(&self, command: WriteCommand) -> Result<()> {
        let connection = self
            .runtime
            .read()
            .map_err(|_| GatewayError::StateUnavailable)?;
        OperationCoordinator::execute(
            &connection,
            &CommitContext::new("monica-library".to_owned()),
            WriteOperationRequest::new(
                uuid::Uuid::new_v4().to_string(),
                "library-edit",
                vec![command],
            ),
        )
        .map_err(|_| GatewayError::StateUnavailable)?;
        Ok(())
    }
}

pub fn read(store: &ConfigStore, password: &str) -> Result<Library> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.library();
    vault.lock()?;
    result
}

/// The tree plus the key rows, from one unlock. `login_type` lives inside the encrypted
/// payload, so a `login` row cannot be told apart from a key entry without this second pass.
pub fn read_with_keys(
    store: &ConfigStore,
    password: &str,
) -> Result<(Library, Vec<crate::vault::KeyEntrySummary>)> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = match vault.library() {
        Ok(library) => vault.key_entries().map(|keys| (library, keys)),
        Err(error) => Err(error),
    };
    vault.lock()?;
    result
}

pub fn rename_category(store: &ConfigStore, password: &str, id: &str, title: &str) -> Result<()> {
    crate::upstream::reject_secret_value(&serde_json::json!([title]), password)
        .map_err(|_| GatewayError::SensitiveMetadata)?;
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.rename_category(id, title);
    vault.lock()?;
    result
}

pub fn create_category(
    store: &ConfigStore,
    password: &str,
    title: &str,
    parent: Option<&str>,
) -> Result<String> {
    crate::upstream::reject_secret_value(&serde_json::json!([title]), password)
        .map_err(|_| GatewayError::SensitiveMetadata)?;
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.create_category(title, parent);
    vault.lock()?;
    result
}

pub fn move_item(store: &ConfigStore, password: &str, id: &str, target: &str) -> Result<()> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.move_library_item(id, target);
    vault.lock()?;
    result
}

pub fn entry_connection<'a>(
    entry: &Entry,
    connections: &'a std::collections::BTreeMap<String, Connection>,
) -> Option<(&'a str, &'a Connection)> {
    connections
        .iter()
        .find(|(_, c)| c.credential_id == entry.id)
        .map(|(name, connection)| (name.as_str(), connection))
}
