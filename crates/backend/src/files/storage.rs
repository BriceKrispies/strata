//! Where the files of a tenant are kept: metadata in the tenant database, bytes on disk.
//!
//! The bytes of a tenant live under `<root>/tenant-<tenant id>/` and nowhere else (CMS-008):
//!
//! - `bytes/<first two digits>/<file id>`: the content, named by its SHA-256;
//! - `tmp/<random>.part`: content being written; it is renamed to its name under `bytes/` only
//!   once the hash of what was written is known, so a name under `bytes/` always holds the content
//!   it names;
//! - `uploads/<upload id>/<index>.chunk`: the chunks of an unfinished upload.
//!
//! Every name is built from a value this module checked first: a file id is 64 lowercase
//! hexadecimal digits, an upload id 32, a chunk index a number, and a tenant id is an identifier
//! the kernel checked. No text from a request is ever a path. Files are created without any
//! execute permission.

use super::{io_error, json_error, not_found, FileMeta, FILE_KIND};
use crate::app::{ApiError, App};
use crate::auth::random_hex;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use strata_kernel::TenantId;

/// Whether `text` is a file id: 64 lowercase hexadecimal digits.
pub fn valid_file_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `text` is an upload id: 32 lowercase hexadecimal digits.
pub fn valid_upload_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Hexadecimal text of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The directory of one tenant. The prefix keeps a tenant id from ever being a name the
/// operating system reserves.
pub fn tenant_dir(app: &App, tenant: &TenantId) -> PathBuf {
    app.files.root().join(format!("tenant-{}", tenant.as_str()))
}

/// Where the bytes of a file are. `file` must be a checked file id.
pub fn bytes_path(app: &App, tenant: &TenantId, file: &str) -> Result<PathBuf, ApiError> {
    if !valid_file_id(file) {
        return Err(not_found());
    }
    Ok(tenant_dir(app, tenant)
        .join("bytes")
        .join(&file[..2])
        .join(file))
}

/// The directory of an unfinished upload. `upload` must be a checked upload id.
pub fn upload_dir(app: &App, tenant: &TenantId, upload: &str) -> Result<PathBuf, ApiError> {
    if !valid_upload_id(upload) {
        return Err(not_found());
    }
    Ok(tenant_dir(app, tenant).join("uploads").join(upload))
}

/// Where chunk `index` of an upload is.
pub fn chunk_path(
    app: &App,
    tenant: &TenantId,
    upload: &str,
    index: u32,
) -> Result<PathBuf, ApiError> {
    Ok(upload_dir(app, tenant, upload)?.join(format!("{index}.chunk")))
}

/// A new file, readable and writable by this process's user only and never executable.
fn create_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// A file being written under a temporary name, hashed as it is written.
pub struct Pending {
    path: PathBuf,
    file: Option<File>,
    hasher: Sha256,
    written: u64,
}

impl Pending {
    /// Start a temporary file in `dir`.
    pub fn in_dir(dir: &Path) -> Result<Pending, ApiError> {
        fs::create_dir_all(dir).map_err(|e| io_error("create a directory", e))?;
        let name = format!("{}.part", random_hex(16)?);
        let path = dir.join(name);
        let file = create_new(&path).map_err(|e| io_error("create a temporary file", e))?;
        Ok(Pending {
            path,
            file: Some(file),
            hasher: Sha256::new(),
            written: 0,
        })
    }

    /// Start a temporary file of a tenant.
    pub fn of_tenant(app: &App, tenant: &TenantId) -> Result<Pending, ApiError> {
        Pending::in_dir(&tenant_dir(app, tenant).join("tmp"))
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        let Some(file) = self.file.as_mut() else {
            return Err(io_error(
                "write",
                std::io::Error::other("the file was closed"),
            ));
        };
        file.write_all(bytes)
            .map_err(|e| io_error("write a temporary file", e))?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Flush the content to disk and say its hash. The file stays under its temporary name.
    pub fn seal(&mut self) -> Result<String, ApiError> {
        if let Some(file) = self.file.take() {
            file.sync_all()
                .map_err(|e| io_error("flush a temporary file", e))?;
        }
        Ok(hex(&self.hasher.clone().finalize()))
    }

    /// Move the sealed content to `target`, replacing nothing: when `target` exists already it
    /// holds the same content (its name is its hash), and the temporary file is dropped.
    pub fn rename_to(mut self, target: &Path) -> Result<(), ApiError> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error("create a directory", e))?;
        }
        if target.exists() {
            let _ = fs::remove_file(&self.path);
        } else {
            fs::rename(&self.path, target).map_err(|e| io_error("rename a temporary file", e))?;
        }
        self.path = PathBuf::new();
        Ok(())
    }
}

impl Drop for Pending {
    /// A temporary file that was not renamed is removed.
    fn drop(&mut self) {
        self.file = None;
        if !self.path.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The first `limit` bytes of a file.
pub fn read_head(path: &Path, limit: usize) -> Result<Vec<u8>, ApiError> {
    let file = File::open(path).map_err(|e| io_error("open a file", e))?;
    let mut head = Vec::with_capacity(limit.min(1 << 20));
    file.take(limit as u64)
        .read_to_end(&mut head)
        .map_err(|e| io_error("read a file", e))?;
    Ok(head)
}

/// Bytes `start..start + len` of a stored file.
pub fn read_range(path: &Path, start: u64, len: u64) -> Result<Vec<u8>, ApiError> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).map_err(|e| io_error("open a file", e))?;
    file.seek(SeekFrom::Start(start))
        .map_err(|e| io_error("seek in a file", e))?;
    let mut out = Vec::with_capacity(usize::try_from(len).unwrap_or(0).min(1 << 24));
    file.take(len)
        .read_to_end(&mut out)
        .map_err(|e| io_error("read a file", e))?;
    Ok(out)
}

/// The metadata of a file of a tenant, when it exists.
pub fn get_meta(app: &App, tenant: &TenantId, file: &str) -> Result<Option<FileMeta>, ApiError> {
    if !valid_file_id(file) {
        return Ok(None);
    }
    match app.store.doc_get(tenant, FILE_KIND, file)? {
        Some(text) => Ok(Some(serde_json::from_str(&text).map_err(json_error)?)),
        None => Ok(None),
    }
}

/// Keep the metadata of a file, replacing what was there.
pub fn put_meta(app: &App, tenant: &TenantId, meta: &FileMeta) -> Result<(), ApiError> {
    let text = serde_json::to_string(meta).map_err(json_error)?;
    app.store
        .doc_put(tenant, FILE_KIND, &meta.file, &text, false)?;
    Ok(())
}

/// Keep the metadata of a new file only when the tenant has none for it yet. Returns whether it
/// was kept.
pub fn create_meta(app: &App, tenant: &TenantId, meta: &FileMeta) -> Result<bool, ApiError> {
    let text = serde_json::to_string(meta).map_err(json_error)?;
    Ok(app
        .store
        .doc_put(tenant, FILE_KIND, &meta.file, &text, true)?)
}

/// Store content in the namespace of its tenant under its content hash, and say the hash
/// (contract put-file, CMS-008). The bytes only: the metadata is the caller's.
///
/// Implements: CT-141
pub fn put_file(app: &App, tenant: &TenantId, content: &[u8]) -> Result<String, ApiError> {
    let mut pending = Pending::of_tenant(app, tenant)?;
    pending.write(content)?;
    let hash = pending.seal()?;
    let target = bytes_path(app, tenant, &hash)?;
    pending.rename_to(&target)?;
    Ok(hash)
}

/// The content and media type of a stored file of a tenant (contract get-file). A file id held
/// by another tenant is not found: every path is under the tenant's own directory.
///
/// Implements: CT-142
pub fn get_file(app: &App, tenant: &TenantId, file: &str) -> Result<(Vec<u8>, String), ApiError> {
    let meta = get_meta(app, tenant, file)?.ok_or_else(not_found)?;
    let path = bytes_path(app, tenant, file)?;
    let content = fs::read(&path).map_err(|_| not_found())?;
    Ok((content, meta.media_type))
}

/// Remove a stored file of a tenant: its bytes and its metadata (contract delete-file).
/// Removing a file that is not there does nothing.
///
/// Implements: CT-143
pub fn delete_file(app: &App, tenant: &TenantId, file: &str) -> Result<(), ApiError> {
    delete_bytes(app, tenant, file)?;
    if valid_file_id(file) {
        app.store.doc_remove(tenant, FILE_KIND, file)?;
    }
    Ok(())
}

/// Remove the bytes of a stored file and keep its metadata.
pub fn delete_bytes(app: &App, tenant: &TenantId, file: &str) -> Result<(), ApiError> {
    let path = bytes_path(app, tenant, file)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("remove a file", error)),
    }
}
