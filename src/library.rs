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
        // The folder Monica for Android writes into is seeded beside the user's own categories.
        assert_eq!(
            library
                .categories
                .iter()
                .filter(|category| !reopened.is_android_root(&category.id))
                .count(),
            3
        );
        assert_eq!(
            library
                .categories
                .iter()
                .filter(|category| reopened.is_android_root(&category.id))
                .count(),
            1,
            "exactly one folder may be the Android write target"
        );
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
                .filter(|(c, _)| !reopened.is_android_root(&c.id))
                .map(|(c, _)| c.title.as_str())
                .collect::<Vec<_>>(),
            ["Archive", "Renamed", "Work"]
        );
        assert_eq!(
            tree.iter().find(|(c, _)| c.title == "Renamed").unwrap().1,
            1
        );
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

    #[test]
    fn a_deleted_row_hides_behind_its_tombstone_and_a_category_empties_first() {
        use crate::model::Provider;
        use zeroize::Zeroizing;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.mdbx");
        let vault = Vault::create(&path, "test-password", TigaMode::Multi).unwrap();
        let work = vault.create_category("Work", None).unwrap();
        let (collection, stored) = vault
            .store_credential(
                Some(&work),
                "work-gh",
                "",
                Provider::Github,
                Provider::Github.default_api_base(),
                "",
                Zeroizing::new("test-upstream-secret-32-characters".to_owned()),
            )
            .unwrap();
        assert_eq!(collection, work);
        let credential = stored.credential_id;
        // Contents are never taken down with their category.
        assert!(matches!(
            vault.delete_category(&work),
            Err(DeleteBlocked::NotEmpty {
                entries: 1,
                children: 0,
                ..
            })
        ));
        vault.delete_entry(&credential).unwrap();
        let library = vault.library().unwrap();
        assert!(library.entries.is_empty());
        // A tombstone is not a row any reader is offered, so there is nothing left to delete.
        assert!(matches!(
            vault.delete_entry(&credential),
            Err(GatewayError::NotFound)
        ));
        assert!(matches!(
            vault.delete_category("missing"),
            Err(DeleteBlocked::Absent)
        ));
        assert_eq!(vault.delete_category(&work).unwrap().title, "Work");
        assert!(
            vault
                .library()
                .unwrap()
                .categories
                .iter()
                .all(|category| category.id != work)
        );
        vault.lock().unwrap();
        drop(vault);
        let reopened = Vault::open(&path, "test-password").unwrap();
        let library = reopened.library().unwrap();
        assert!(library.entries.is_empty());
        assert!(
            library
                .categories
                .iter()
                .all(|category| category.id != work),
            "a tombstone must not read back as a live row"
        );
        reopened.lock().unwrap();
    }

    /// The folder named after the vault id is what Monica for Android saves into, so opening a
    /// vault repairs it and no edit command may take it away again.
    #[test]
    fn the_android_root_folder_is_seeded_repaired_and_protected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("library.mdbx");
        let vault = Vault::create(&path, "test-password", TigaMode::Multi).unwrap();
        let root = vault
            .library()
            .unwrap()
            .categories
            .iter()
            .find(|category| vault.is_android_root(&category.id))
            .expect("a new vault offers Android its write folder")
            .id
            .clone();
        let other = vault.create_category("Work", None).unwrap();
        // Simulate a vault that lost the folder: the engine's own soft delete, which the CLI
        // refuses to issue for this id.
        vault
            .library_write(WriteCommand::DeleteProject {
                project_id: root.clone(),
            })
            .unwrap();
        assert!(
            !vault
                .library()
                .unwrap()
                .categories
                .iter()
                .any(|category| category.id == root),
            "a deleted folder is hidden while it is still a row"
        );
        vault.lock().unwrap();
        drop(vault);

        let reopened = Vault::open(&path, "test-password").unwrap();
        let library = reopened.library().unwrap();
        let matches = library
            .categories
            .iter()
            .filter(|category| category.id == root)
            .count();
        assert_eq!(
            matches, 1,
            "opening a vault restores the folder Android writes into"
        );
        assert!(matches!(
            reopened.delete_category(&root),
            Err(DeleteBlocked::Protected(_))
        ));
        assert!(matches!(
            reopened.move_library_item(&root, &other),
            Err(GatewayError::ProtectedCollection)
        ));
        // Entries may still be filed into it, and it can be renamed.
        reopened
            .move_library_item(&other, &root)
            .expect("the protected folder stays a usable destination");
        reopened.lock().unwrap();
        drop(reopened);
        // Reopening a healthy vault must not seed a second copy.
        let again = Vault::open(&path, "test-password").unwrap();
        assert_eq!(
            again
                .library()
                .unwrap()
                .categories
                .iter()
                .filter(|category| category.id == root)
                .count(),
            1
        );
        again.lock().unwrap();
    }
}

#[derive(Clone, Default, Serialize)]
pub struct Library {
    pub categories: Vec<Category>,
    pub entries: Vec<Entry>,
}
#[derive(Clone, Debug, Serialize)]
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

/// Why one category delete stopped before the engine was asked. The stable code is what
/// `--json` reports; the counts are what a person needs to hear to move things out.
#[derive(Debug)]
pub enum DeleteBlocked {
    Absent,
    NotEmpty {
        category: Category,
        entries: usize,
        children: usize,
    },
    /// Monica for Android saves the entries it creates into the collection named after the vault
    /// id. Deleting it would make the vault read-only on the phone again.
    Protected(Category),
    /// Unlocking, foreign vault, engine refusal: the vault already had a code for this.
    Failed(GatewayError),
}

impl DeleteBlocked {
    pub fn error(&self) -> GatewayError {
        match self {
            Self::Absent => GatewayError::NotFound,
            Self::NotEmpty { .. } => GatewayError::InvalidRequest,
            Self::Protected(_) => GatewayError::ProtectedCollection,
            Self::Failed(error) => *error,
        }
    }
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
        if self.is_android_root(id) {
            return Err(GatewayError::ProtectedCollection);
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

    /// Tombstones one entry. The engine hides the row from every listing and rides the delete
    /// to the other devices with the next segment; the encrypted bytes stay in the vault file
    /// until it gains a purge path, which neither the engine nor Android has yet.
    pub fn delete_entry(&self, id: &str) -> Result<()> {
        let project_id = self
            .library()?
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.category.clone())
            .ok_or(GatewayError::NotFound)?;
        self.library_write(WriteCommand::DeleteEntry {
            entry_id: id.to_owned(),
            project_id,
        })
    }

    /// Removes one category. Only an empty one: the engine refuses to orphan children, and a
    /// recursive delete from here would be the kind of surprise a person did not ask for.
    pub fn delete_category(&self, id: &str) -> std::result::Result<Category, DeleteBlocked> {
        let library = self.library().map_err(DeleteBlocked::Failed)?;
        let category = library
            .categories
            .iter()
            .find(|category| category.id == id)
            .ok_or(DeleteBlocked::Absent)?
            .clone();
        if self.is_android_root(&category.id) {
            return Err(DeleteBlocked::Protected(category));
        }
        let entries = library
            .entries
            .iter()
            .filter(|entry| entry.category == id)
            .count();
        let children = library
            .categories
            .iter()
            .filter(|category| category.parent.as_deref() == Some(id))
            .count();
        if entries > 0 || children > 0 {
            return Err(DeleteBlocked::NotEmpty {
                category,
                entries,
                children,
            });
        }
        self.library_write(WriteCommand::DeleteProject {
            project_id: id.to_owned(),
        })
        .map_err(DeleteBlocked::Failed)?;
        Ok(category)
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

pub fn delete_entry(store: &ConfigStore, password: &str, id: &str) -> Result<()> {
    let _guard = store.acquire_broker_lock()?;
    let vault = Vault::open(&store.load()?.vault, password)?;
    let result = vault.delete_entry(id);
    vault.lock()?;
    result
}

pub fn delete_category(
    store: &ConfigStore,
    password: &str,
    id: &str,
) -> std::result::Result<Category, DeleteBlocked> {
    let _guard = store.acquire_broker_lock().map_err(DeleteBlocked::Failed)?;
    let config = store.load().map_err(DeleteBlocked::Failed)?;
    let vault = Vault::open(&config.vault, password).map_err(DeleteBlocked::Failed)?;
    let result = vault.delete_category(id);
    vault.lock().map_err(DeleteBlocked::Failed)?;
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
