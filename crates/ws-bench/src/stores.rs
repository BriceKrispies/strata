//! The storage a run is over: two handles on the same data, one for the store under test and one
//! for the reference the coherence check compares with (and the foreign writes go through).
//!
//! - `memory`: one `MemoryStore`, both handles the same object.
//! - `sqlite[:path]`: a control database at `path` (a private temporary directory when none is
//!   given) with its tenant files beside it; the reference is a second `SqliteStore` opened on the
//!   same files, so a write through it is a write the first handle's caches know nothing of.
//! - `sqlcipher`: refused. This build links SQLite as `rusqlite` with `bundled` and no SQLCipher;
//!   the encrypted adapter does not exist here, and the harness will not pretend it does.

use std::path::PathBuf;
use std::sync::Arc;
use strata_kernel::Timestamp;
use strata_storage_api::memory::MemoryStore;
use strata_storage_api::{Store, StoreError};
use strata_storage_sqlite::SqliteStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreSpec {
    Memory,
    Sqlite(Option<PathBuf>),
}

impl StoreSpec {
    pub fn parse(text: &str) -> Result<StoreSpec, String> {
        match text {
            "memory" => Ok(StoreSpec::Memory),
            "sqlite" => Ok(StoreSpec::Sqlite(None)),
            "sqlcipher" => Err(
                "sqlcipher is not available in this build: storage-sqlite links bundled SQLite \
                 without SQLCipher"
                    .to_string(),
            ),
            other => match other.strip_prefix("sqlite:") {
                Some(path) if !path.is_empty() => Ok(StoreSpec::Sqlite(Some(PathBuf::from(path)))),
                _ => Err(format!(
                    "unknown store {other:?}: memory, sqlite or sqlite:<path>"
                )),
            },
        }
    }

    pub fn name(&self) -> String {
        match self {
            StoreSpec::Memory => "memory".to_string(),
            StoreSpec::Sqlite(None) => "sqlite".to_string(),
            StoreSpec::Sqlite(Some(path)) => format!("sqlite:{}", path.display()),
        }
    }

    /// Open the storage. `salt` names the private directory of a pathless sqlite run.
    pub fn open(&self, salt: u64) -> Result<Opened, StoreError> {
        match self {
            StoreSpec::Memory => {
                let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
                Ok(Opened {
                    spec: self.clone(),
                    primary: store.clone(),
                    reference: store,
                    private_dir: None,
                    control: None,
                })
            }
            StoreSpec::Sqlite(path) => {
                let (control, private_dir) = match path {
                    Some(path) => (path.clone(), None),
                    None => {
                        let dir = std::env::temp_dir()
                            .join(format!("strata-ws-bench-{}-{salt:x}", std::process::id()));
                        std::fs::create_dir_all(&dir).map_err(|e| {
                            StoreError::Unavailable(format!("{}: {e}", dir.display()))
                        })?;
                        (dir.join("control.db"), Some(dir))
                    }
                };
                let primary: Arc<dyn Store> = Arc::new(SqliteStore::open(&control, Timestamp(0))?);
                let reference: Arc<dyn Store> =
                    Arc::new(SqliteStore::open(&control, Timestamp(0))?);
                Ok(Opened {
                    spec: self.clone(),
                    primary,
                    reference,
                    private_dir,
                    control: Some(control),
                })
            }
        }
    }

    /// The count of schema decodes so far in this process, where the adapter keeps one.
    pub fn schema_decodes(&self) -> Option<u64> {
        match self {
            StoreSpec::Memory => None,
            StoreSpec::Sqlite(_) => Some(strata_storage_sqlite::schema_decodes()),
        }
    }
}

/// Two handles on one storage.
pub struct Opened {
    spec: StoreSpec,
    /// The handle the store under test is built over.
    pub primary: Arc<dyn Store>,
    /// The unwrapped second handle: the reference, and where foreign writes go.
    pub reference: Arc<dyn Store>,
    private_dir: Option<PathBuf>,
    control: Option<PathBuf>,
}

impl Opened {
    /// A fresh primary handle on the same data, with whatever the old one had cached gone: a
    /// process restart. The memory store has nothing to reopen and keeps its handle.
    pub fn reopen_primary(&mut self) -> Result<(), StoreError> {
        if let (StoreSpec::Sqlite(_), Some(control)) = (&self.spec, &self.control) {
            self.primary = Arc::new(SqliteStore::open(control, Timestamp(0))?);
        }
        Ok(())
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        if let Some(dir) = &self.private_dir {
            // Close the database files first (an open file cannot be removed on Windows).
            let closed: Arc<dyn Store> = Arc::new(MemoryStore::new());
            self.primary = closed.clone();
            self.reference = closed;
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
