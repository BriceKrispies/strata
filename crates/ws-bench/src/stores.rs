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
    /// A populated SQLite store opened as a private copy, so a run never changes the original.
    SqliteReuse(PathBuf),
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
            StoreSpec::SqliteReuse(path) => format!("sqlite-reuse:{}", path.display()),
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
            StoreSpec::Sqlite(_) | StoreSpec::SqliteReuse(_) => {
                let path = match self {
                    StoreSpec::Sqlite(path) => path.as_ref(),
                    _ => None,
                };
                let (control, private_dir) = match (self, path) {
                    (StoreSpec::SqliteReuse(source), _) => {
                        let dir = private_dir(salt)?;
                        let control = copy_store(source, &dir).map_err(|e| {
                            StoreError::Unavailable(format!("{}: {e}", source.display()))
                        })?;
                        (control, Some(dir))
                    }
                    (_, Some(path)) => (path.clone(), None),
                    (_, None) => {
                        let dir = private_dir(salt)?;
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
            StoreSpec::Sqlite(_) | StoreSpec::SqliteReuse(_) => {
                Some(strata_storage_sqlite::schema_decodes())
            }
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
        if let (StoreSpec::Sqlite(_) | StoreSpec::SqliteReuse(_), Some(control)) =
            (&self.spec, &self.control)
        {
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

/// A fresh private directory for a run.
fn private_dir(salt: u64) -> Result<PathBuf, StoreError> {
    let dir = std::env::temp_dir().join(format!("strata-ws-bench-{}-{salt:x}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| StoreError::Unavailable(format!("{}: {e}", dir.display())))?;
    Ok(dir)
}

/// Copy a control database (with its write-ahead files) and its tenants directory into `into`,
/// keeping the names the adapter derives the tenants directory from. Returns the copied control
/// database.
fn copy_store(control: &std::path::Path, into: &std::path::Path) -> std::io::Result<PathBuf> {
    let name = control
        .file_name()
        .ok_or_else(|| std::io::Error::other("the store path names no file"))?;
    let copy = into.join(name);
    std::fs::copy(control, &copy)?;
    for suffix in ["-wal", "-shm"] {
        let mut side = control.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.exists() {
            let mut to = copy.as_os_str().to_owned();
            to.push(suffix);
            std::fs::copy(&side, PathBuf::from(to))?;
        }
    }
    let tenants = strata_storage_sqlite::default_tenants_dir(control);
    if tenants.is_dir() {
        copy_tree(&tenants, &strata_storage_sqlite::default_tenants_dir(&copy))?;
    }
    Ok(copy)
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
