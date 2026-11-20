//! The upload scanner (component `upload-scanner`, contract scan-upload, CMS-260): every finished
//! upload is scanned by a job of the automation worker, and a file is deliverable only once the
//! scanner has called it clean.
//!
//! The scanner is a trait, so a production scanner (an antivirus daemon, a sandboxed service) is
//! one adapter away. The development scanner rejects the EICAR test file and accepts everything
//! else: enough for the state machine to be real and tested.
//!
//! Renditions (CMS-009) are declared on the file field and would be produced by a job each. No
//! image library is a dependency of the backend, so the job kind exists and fails with a clear
//! error instead of producing anything; the `rendition-ready` event therefore never follows.

use super::{storage, FileMeta, FileState};
use crate::app::{App, LOG};
use crate::worker::steps::Outcome;
use serde_json::json;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use strata_kernel::{FieldId, FieldType, ObjectTypeId, TenantId, Timestamp};
use strata_storage_api::jobs::{JobReport, JobWork, NewJob, RetryPolicy, WorkerWrites};

/// What a scanner decided about a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// Rejected, and why.
    Rejected(String),
}

/// A scanner of uploaded files. It reads the stored bytes and holds no credential of the tenant.
pub trait Scanner: Send + Sync {
    /// The scanner's name, for the log.
    fn name(&self) -> &str;
    /// Scan the file at `path`. An error means the scanner could not decide; the job is retried.
    fn scan(&self, path: &Path) -> Result<Verdict, String>;
}

/// The development scanner: rejects a file that holds the EICAR test string anywhere, accepts
/// everything else.
pub struct DevScanner;

/// The EICAR anti-virus test string, assembled at run time so that no source or binary of this
/// crate holds it whole and is itself taken for the test file.
pub fn eicar() -> Vec<u8> {
    [
        "X5O!P%@AP[4\\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
}

impl Scanner for DevScanner {
    fn name(&self) -> &str {
        "development"
    }

    fn scan(&self, path: &Path) -> Result<Verdict, String> {
        let needle = eicar();
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut window: Vec<u8> = Vec::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                return Ok(Verdict::Clean);
            }
            window.extend_from_slice(&buffer[..read]);
            if window.windows(needle.len()).any(|part| part == needle) {
                return Ok(Verdict::Rejected("the EICAR test file".into()));
            }
            // Keep the tail that a match could still start in.
            let keep = needle.len().saturating_sub(1).min(window.len());
            window.drain(..window.len() - keep);
        }
    }
}

/// The key of the scan job of a file: one job per file of a tenant.
fn scan_job_key(file: &str) -> String {
    format!("file-scan:{file}")
}

/// The scan job of a newly stored file, due now.
pub fn scan_job(file: &str, now: Timestamp) -> NewJob {
    NewJob {
        key: scan_job_key(file),
        work: JobWork::ScanFile {
            file: file.to_string(),
        },
        due_at: now,
        policy: RetryPolicy::default(),
    }
}

/// The rendition jobs of a clean file: one for each rendition the field named at upload declares.
fn rendition_jobs(app: &App, tenant: &TenantId, meta: &FileMeta, now: Timestamp) -> Vec<NewJob> {
    let Some(target) = &meta.field else {
        return Vec::new();
    };
    let (Ok(object), Ok(field)) = (
        ObjectTypeId::new(target.object.as_str()),
        FieldId::new(target.field.as_str()),
    ) else {
        return Vec::new();
    };
    let Ok(Some(state)) = app.store.load(tenant, &[]).map(|s| s.tenant) else {
        return Vec::new();
    };
    let ty = state
        .schema
        .objects
        .get(&object)
        .and_then(|def| def.fields.get(&field))
        .map(|def| def.ty.clone());
    let renditions = match ty {
        Some(FieldType::File { renditions, .. }) => renditions,
        Some(FieldType::List(inner)) => match *inner {
            FieldType::File { renditions, .. } => renditions,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    renditions
        .into_iter()
        .map(|rendition| NewJob {
            key: format!("file-rendition:{}:{}", meta.file, rendition.name),
            work: JobWork::Rendition {
                file: meta.file.clone(),
                rendition: rendition.name,
            },
            due_at: now,
            policy: RetryPolicy {
                attempts: 1,
                ..RetryPolicy::default()
            },
        })
        .collect()
}

fn failed(error: impl Into<String>, retry: bool) -> Outcome {
    Outcome {
        report: JobReport::Failed {
            error: error.into(),
            retry,
        },
        writes: WorkerWrites::default(),
    }
}

/// Run the scan job of a file (contract scan-upload): record the scanner's verdict, remove the
/// bytes of a rejected file, and create the rendition jobs of a clean one. A file that is gone or
/// already decided is left as it is. `exhausted` says the last allowed attempt's lease ran out.
///
/// Implements: CT-204
pub fn run_scan_job(
    app: &App,
    tenant: &TenantId,
    file: &str,
    exhausted: bool,
    now: Timestamp,
) -> Outcome {
    let done = |writes| Outcome {
        report: JobReport::Succeeded,
        writes,
    };
    let meta = match storage::get_meta(app, tenant, file) {
        Ok(Some(meta)) => meta,
        Ok(None) => return done(WorkerWrites::default()),
        Err(error) => return failed(error.describe(), true),
    };
    if meta.state != FileState::Scanning {
        return done(WorkerWrites::default());
    }
    if exhausted {
        return failed("the scanner did not finish within its lease", false);
    }
    let path = match storage::bytes_path(app, tenant, file) {
        Ok(path) => path,
        Err(error) => return failed(error.describe(), false),
    };
    let scanner = app.files.scanner();
    let verdict = match scanner.scan(&path) {
        Ok(verdict) => verdict,
        Err(error) => return failed(format!("the scanner failed: {error}"), true),
    };
    let mut meta = meta;
    meta.scanned_at = Some(now.0);
    let clean = verdict == Verdict::Clean;
    match verdict {
        Verdict::Clean => meta.state = FileState::Clean,
        Verdict::Rejected(why) => {
            meta.state = FileState::Rejected;
            meta.verdict = Some(why);
            if let Err(error) = storage::delete_bytes(app, tenant, file) {
                return failed(error.describe(), true);
            }
        }
    }
    if let Err(error) = storage::put_meta(app, tenant, &meta) {
        return failed(error.describe(), true);
    }
    // The domain event file-scanned is logged here; the kernel's event body for it is not built
    // (WG-5223).
    LOG.info(
        "file.scanned",
        "",
        json!({
            "tenant": tenant.as_str(),
            "file": file,
            "clean": clean,
            "scanner": scanner.name(),
        }),
    );
    if !clean {
        app.audit(
            "file.rejected",
            "",
            json!({ "tenant": tenant.as_str(), "file": file, "verdict": meta.verdict }),
        );
        return done(WorkerWrites::default());
    }
    done(WorkerWrites {
        jobs: rendition_jobs(app, tenant, &meta, now),
        ..WorkerWrites::default()
    })
}

/// Run a rendition job. No image library is a dependency of the backend, so a rendition is never
/// produced: the job fails for good with an error that says so, and no `rendition-ready` event
/// follows (CMS-009, CMS-262).
pub fn run_rendition_job(tenant: &TenantId, file: &str, rendition: &str) -> Outcome {
    LOG.warn(
        "file.rendition_not_built",
        "",
        json!({ "tenant": tenant.as_str(), "file": file, "rendition": rendition }),
    );
    failed(
        format!(
            "rendition {rendition} of file {file} was not produced: renditions are not built, because no image library is a dependency of the backend (WG-5227)"
        ),
        false,
    )
}
