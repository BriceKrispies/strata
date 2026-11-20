//! The file store of a tenant (components `file-store` and `upload-scanner`, WG-4993): uploads,
//! scanning, the readability of a file, signed addresses and delivery.
//!
//! **What a file is.** A field of the `file` type holds the text of a file id, and the file id is
//! the SHA-256 of the file's content as 64 lowercase hexadecimal digits (CMS-1591, D-6587: "the
//! file field type holds a content hash"). The same content uploaded twice is one file, with one
//! note per upload of who uploaded it and under what name.
//!
//! **Where it is kept** ([`storage`]): the metadata of a file (its id, the media type its content
//! confirmed, its size, its state, its uploads) is a document of the tenant database
//! (`Store::doc_put`, kind [`FILE_KIND`]); the bytes are a file on disk under the tenant's own
//! directory of the file root, named by their hash, written under a temporary name and renamed
//! once the hash of what was written is known. No byte is ever under a path the gateway serves,
//! and no stored file is executable.
//!
//! **States**: an upload that is not finished is `uploading` (a chunked upload, kind
//! [`UPLOAD_KIND`]); a finished upload is `scanning` until the scanner's job decides, then
//! `clean` or `rejected` ([`scan`]). Only a clean file is ever delivered (CMS-260).
//!
//! **Who may read a file** ([`access`]): whoever the kernel lets read a field of a record that
//! refers to it (record policies and the field's `read` rule both apply, D-6586, D-6587); a file
//! no record refers to is readable by its uploaders only. References are found through the
//! record index (`Need::ByField` on every field of the `file` type), so no count can drift from
//! the records. A file no record refers to is removed 24 hours after it was uploaded or last seen
//! referred to ([`purge`]).
//!
//! **Addresses and delivery** ([`address`], [`deliver`]): an address is signed with a key of the
//! tenant for the file, the principal (or `public`, when anyone may read the file) and an expiry
//! of at most an hour; delivery checks the signature, the expiry, the session and the right to
//! read again (D-6588).
//!
//! **Media types** ([`sniff`]): decided from the content against an allow-list (PNG, JPEG, GIF,
//! WebP, PDF), never from the declared type or the name (CMS-261). Markup (SVG, HTML, XML) is
//! refused at intake: CMS-258 allows a markup-capable file only from a file domain under a
//! sandboxed policy, and this platform has no file domain yet (WG-5224), so none is accepted.
//!
//! Implements: CMS-008, CMS-260, CMS-261, CMS-1591, CMS-1593, EDG-012, EDG-1592

pub mod access;
pub mod address;
pub mod deliver;
pub mod erasure;
pub mod routes;
pub mod scan;
pub mod sniff;
pub mod storage;
pub mod upload;

use crate::app::{ApiError, App, LOG};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use strata_kernel::{TenantId, Timestamp};
use strata_protocol::api::ErrorCode;
use strata_storage_api::StoreResult;

/// The largest file of the platform (D-6589). A tenant may lower it, and a field again.
pub const PLATFORM_MAX_BYTES: u64 = 104_857_600;

/// The largest file sent in one `upload-file` call; a larger one is sent in chunks (D-6589).
pub const SINGLE_MAX_BYTES: u64 = 1_048_576;

/// The size of every chunk of a chunked upload but the last (D-6589: at most 8388608 bytes).
pub const CHUNK_BYTES: u64 = 8_388_608;

/// How long an unfinished upload, and a file no record refers to, are kept (D-6587).
pub const UNATTACHED_MS: i64 = 24 * 3_600_000;

/// The longest expiry of a signed address (D-6588).
pub const ADDRESS_MAX_SECONDS: u32 = 3_600;

/// The largest decompressed image the platform accepts when the tenant states none (CMS-263):
/// 256 MiB, a 64-megapixel image at four bytes a pixel. The plan limit is not held anywhere yet.
pub const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = 268_435_456;

/// The document kind of the metadata of a stored file (`rec-file`), keyed by file id.
pub const FILE_KIND: &str = "file";

/// The document kind of an unfinished chunked upload, keyed by upload id.
pub const UPLOAD_KIND: &str = "file-upload";

/// The document kind of what an erasure removed of files, keyed by the erasure's correlation id.
pub const ERASURE_KIND: &str = "file-erasure";

/// The file service of a process: where its bytes live and the scanner it asks.
pub struct FileService {
    root: OnceLock<PathBuf>,
    scanner: OnceLock<Box<dyn scan::Scanner>>,
}

impl Default for FileService {
    fn default() -> Self {
        FileService::new()
    }
}

impl FileService {
    pub fn new() -> Self {
        FileService {
            root: OnceLock::new(),
            scanner: OnceLock::new(),
        }
    }

    /// Set the directory the bytes of every tenant live under. Only the first call (or first use)
    /// decides; returns whether this call did.
    pub fn set_root(&self, root: PathBuf) -> bool {
        self.root.set(root).is_ok()
    }

    /// The directory the bytes of every tenant live under: what [`set_root`](Self::set_root)
    /// gave, else `STRATA_FILES`, else `var/strata-files`. It is never inside the built frontend
    /// the gateway serves.
    pub fn root(&self) -> &Path {
        self.root.get_or_init(|| {
            std::env::var("STRATA_FILES")
                .ok()
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("var/strata-files"))
        })
    }

    /// Set the scanner. Only the first call (or first use) decides; returns whether this call did.
    pub fn set_scanner(&self, scanner: Box<dyn scan::Scanner>) -> bool {
        self.scanner.set(scanner).is_ok()
    }

    /// The scanner: what [`set_scanner`](Self::set_scanner) gave, else the development scanner,
    /// whose use is logged as a warning.
    pub fn scanner(&self) -> &dyn scan::Scanner {
        self.scanner
            .get_or_init(|| {
                LOG.warn(
                    "files.development_scanner",
                    "",
                    json!({ "note": "the development scanner rejects only the EICAR test file" }),
                );
                Box::new(scan::DevScanner)
            })
            .as_ref()
    }
}

/// The state of a stored file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileState {
    /// Stored and waiting for the scanner's verdict: not deliverable.
    Scanning,
    /// The scanner found nothing: deliverable to whoever may read it.
    Clean,
    /// The scanner rejected it: its bytes are removed and it is never delivered.
    Rejected,
}

impl FileState {
    pub fn as_str(self) -> &'static str {
        match self {
            FileState::Scanning => "scanning",
            FileState::Clean => "clean",
            FileState::Rejected => "rejected",
        }
    }
}

/// A field of the `file` type, named by its object type and field id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRef {
    pub object: String,
    pub field: String,
}

/// One upload of a file: who, under what name and declared type, and when. The name is tenant
/// text: it is kept as data and never used as a path or a header (CMS-257).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadNote {
    pub principal: String,
    pub name: String,
    pub declared_type: String,
    pub at: i64,
}

/// The metadata of a stored file (`rec-file`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    /// The file id: the SHA-256 of the content, in hexadecimal.
    pub file: String,
    /// The media type the content confirmed.
    pub media_type: String,
    pub size: u64,
    pub state: FileState,
    /// Every upload of this content, oldest first.
    pub uploads: Vec<UploadNote>,
    pub created_at: i64,
    /// When the janitor last saw a record refer to the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referenced_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<i64>,
    /// Why the scanner rejected it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// The field the first upload named, whose renditions the file is to get.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<FieldRef>,
}

impl FileMeta {
    /// Whether `principal` uploaded this content.
    pub fn uploaded_by(&self, principal: &str) -> bool {
        self.uploads.iter().any(|note| note.principal == principal)
    }
}

/// The file limits of a tenant, with the platform's where the tenant states none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_decompressed_bytes: u64,
    pub address_max_seconds: u32,
}

/// The file limits of a tenant (`tenant-settings.files`, D-6594), each at most the platform's.
///
/// The tenant settings are not stored anywhere yet (configure-tenant-settings is not built), so
/// every tenant has the platform limits; [`Limits::tightened`] is what applies a stored value
/// (WG-5225).
pub fn tenant_limits(_app: &App, _tenant: &TenantId) -> Limits {
    Limits::tightened(None)
}

impl Limits {
    /// The platform limits, lowered by what the tenant states: a value above the platform's is
    /// ignored, since a tenant may only tighten (D-6594).
    pub fn tightened(stated: Option<&strata_model::types::FileLimits>) -> Limits {
        let platform = Limits {
            max_bytes: PLATFORM_MAX_BYTES,
            max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
            address_max_seconds: ADDRESS_MAX_SECONDS,
        };
        let Some(stated) = stated else {
            return platform;
        };
        Limits {
            max_bytes: stated
                .max_bytes
                .map_or(platform.max_bytes, |n| n.min(platform.max_bytes)),
            max_decompressed_bytes: stated
                .max_decompressed_bytes
                .map_or(platform.max_decompressed_bytes, |n| {
                    n.min(platform.max_decompressed_bytes)
                }),
            address_max_seconds: stated
                .address_max_seconds
                .map_or(platform.address_max_seconds, |n| {
                    n.min(platform.address_max_seconds)
                }),
        }
    }
}

/// The refusal of anything about a file that does not exist for this tenant or that the caller
/// may not read: the two are not told apart.
pub(crate) fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "No such file.")
}

pub(crate) fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Invalid, message)
}

pub(crate) fn io_error(what: &str, error: std::io::Error) -> ApiError {
    ApiError::hidden(
        ErrorCode::Unavailable,
        format!("file store: {what}: {error}"),
    )
}

pub(crate) fn json_error(error: serde_json::Error) -> ApiError {
    ApiError::hidden(ErrorCode::Internal, format!("file store: {error}"))
}

/// One pass of the janitor over the files of every tenant (`ret-file-bytes`): unfinished uploads
/// older than 24 hours are removed with their chunks, and a file no record refers to is removed
/// with its bytes 24 hours after it was uploaded or last seen referred to. Returns how many
/// uploads and files were removed. A tenant that fails is logged and the pass goes on.
pub fn purge(app: &App, now: Timestamp) -> StoreResult<u64> {
    let mut removed = 0;
    for summary in app.store.tenants()? {
        match purge_tenant(app, &summary.id, now) {
            Ok(n) => removed += n,
            Err(error) => LOG.error(
                "files.purge_failed",
                "",
                json!({ "tenant": summary.id.as_str(), "error": error.describe() }),
            ),
        }
    }
    Ok(removed)
}

/// [`purge`] for one tenant.
pub fn purge_tenant(app: &App, tenant: &TenantId, now: Timestamp) -> Result<u64, ApiError> {
    let mut removed = upload::purge_uploads(app, tenant, now)?;
    let Some(state) = app.store.load(tenant, &[])?.tenant else {
        return Ok(removed);
    };
    for (key, text) in app.store.doc_list(tenant, FILE_KIND)? {
        let Ok(mut meta) = serde_json::from_str::<FileMeta>(&text) else {
            continue;
        };
        if meta.file != key || !storage::valid_file_id(&key) {
            continue;
        }
        let referenced = !access::references(app, tenant, &state.schema, &meta.file)?.is_empty();
        if referenced {
            // Written at most once an hour, so a pass does not rewrite every file.
            if meta.referenced_at.is_none_or(|at| now.0 - at >= 3_600_000) {
                meta.referenced_at = Some(now.0);
                storage::put_meta(app, tenant, &meta)?;
            }
            continue;
        }
        let last = meta.referenced_at.unwrap_or(0).max(meta.created_at);
        if now.0 - last >= UNATTACHED_MS {
            storage::delete_file(app, tenant, &meta.file)?;
            removed += 1;
            LOG.info(
                "files.purged",
                "",
                json!({ "tenant": tenant.as_str(), "file": meta.file }),
            );
        }
    }
    Ok(removed)
}
