//! At-most-once human mutations. Intent is synced before execution. An unfinished
//! intent is never replayed, even after a process crash. No request payload,
//! credential, user title, client capability or upstream response is persisted.
use crate::config::{ConfigStore, ensure_parent, private_file, read_json, write_json};
use crate::error::{GatewayError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    path::PathBuf,
};
use uuid::Uuid;

const LIMIT: usize = 4096;
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    records: BTreeMap<Uuid, Receipt>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    vault: String,
    operation: String,
    completed: bool,
    error: Option<GatewayError>,
}
pub(super) struct Receipts {
    _lock: File,
    path: PathBuf,
    journal: Journal,
}
impl Receipts {
    pub fn open(store: &ConfigStore) -> Result<Self> {
        let path = store.path.with_extension("browser-receipts.json");
        ensure_parent(&path)?;
        let lock_path = store.path.with_extension("browser-receipts-lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|_| GatewayError::StateUnavailable)?;
        private_file(&lock_path)?;
        lock.try_lock().map_err(|_| GatewayError::BrokerBusy)?;
        let journal: Journal = if path.exists() {
            read_json(&path, 2 * 1024 * 1024)?
        } else {
            Journal::default()
        };
        if journal.records.len() > LIMIT {
            return Err(GatewayError::InvalidConfig);
        }
        Ok(Self {
            _lock: lock,
            path,
            journal,
        })
    }
    pub fn status(&self, id: Uuid, vault: &str) -> Result<Value> {
        match self.journal.records.get(&id) {
            Some(r) if r.vault != vault => Err(GatewayError::ObjectChanged),
            Some(r) => Ok(json!({"operation_id":id,"operation":r.operation,
                "state":if r.completed {"succeeded"} else {"unconfirmed"},"error":r.error})),
            None => Ok(json!({"operation_id":id,"state":"not_recorded"})),
        }
    }
    pub fn begin(&mut self, id: Uuid, vault: &str, operation: &str) -> Result<Option<Value>> {
        if let Some(r) = self.journal.records.get(&id) {
            if r.vault != vault || r.operation != operation {
                return Err(GatewayError::ObjectChanged);
            }
            return self.status(id, vault).map(Some);
        }
        // Never evict a consumed ID and thereby make an old retry executable.
        if self.journal.records.len() >= LIMIT {
            return Err(GatewayError::RateLimited);
        }
        self.journal.records.insert(
            id,
            Receipt {
                vault: vault.into(),
                operation: operation.into(),
                completed: false,
                error: None,
            },
        );
        self.save()?;
        Ok(None)
    }
    pub fn finish(&mut self, id: Uuid, outcome: &Result<Value>) -> Result<()> {
        let receipt = self
            .journal
            .records
            .get_mut(&id)
            .ok_or(GatewayError::InvalidRequest)?;
        receipt.completed = outcome.is_ok();
        receipt.error = outcome.as_ref().err().copied();
        self.save()
    }
    fn save(&self) -> Result<()> {
        write_json(&self.path, &self.journal, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_lock_scope_and_capacity_never_make_consumed_ids_executable() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(temp.path().join("gateway.json"));
        let mut first = Receipts::open(&store).unwrap();
        assert!(matches!(
            Receipts::open(&store),
            Err(GatewayError::BrokerBusy)
        ));
        let id = Uuid::new_v4();
        first.begin(id, "vault-a", "create_category").unwrap();
        assert!(matches!(
            first.status(id, "vault-b"),
            Err(GatewayError::ObjectChanged)
        ));
        for _ in 1..LIMIT {
            first.journal.records.insert(
                Uuid::new_v4(),
                Receipt {
                    vault: "vault-a".into(),
                    operation: "delete".into(),
                    completed: false,
                    error: None,
                },
            );
        }
        first.save().unwrap();
        drop(first);
        let mut reopened = Receipts::open(&store).unwrap();
        assert!(
            reopened
                .begin(id, "vault-a", "create_category")
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            reopened.begin(Uuid::new_v4(), "vault-a", "delete"),
            Err(GatewayError::RateLimited)
        ));
        assert_eq!(reopened.journal.records.len(), LIMIT);
    }
}
