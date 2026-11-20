//! The copies of a tenant that outlive an erasure for a stated time, named one by one in the
//! receipt with the expiry of each (PRV-353, D-351, D-7727, WG-5109).
//!
//! - **Backup images.** A snapshot of tenant state is an image of the kind `copy-tenant-backup`
//!   (its description says so). Each one the tenant holds is listed with when it was taken and when
//!   it is removed, 35 days after (D-351). Its personal fields are sealed under the subject key
//!   and the key is not part of the snapshot, so the destruction that ends every erasure makes
//!   them unreadable in it; the line says what the image keeps until its expiry.
//! - **Exports of the person's data.** The results and parts of `export-subject-data` hold the
//!   person's values as they were read. The erasure removes each one before its expiry and the
//!   receipt names it with the expiry it had.
//!
//! What this build writes none of, and so cannot name, is said in the receipt's not-reached lines
//! (`erasure::not_reached`): images of the tenant file taken outside the platform and tenant
//! exports (`copy-tenant-export`, TEN-010).

use crate::app::{ApiError, App};
use crate::subject_export::{remove_documents, ExportRequest, REQUEST_KIND};
use strata_kernel::{PrincipalId, TenantId, Timestamp};

/// How long a backup image is kept after it was taken: 35 days (D-351), in milliseconds.
pub const BACKUP_RETENTION_MS: i64 = 35 * 86_400_000;

/// The receipt lines for the copies held now, for the erasure of `subject` at `now`. The exports
/// of the subject's data are removed by this call.
pub fn of_subject(
    app: &App,
    tenant: &TenantId,
    subject: &PrincipalId,
    now: Timestamp,
) -> Result<Vec<String>, ApiError> {
    let mut lines = Vec::new();
    for image in app.store.snapshots(tenant)? {
        let expires = image.created_at.0 + BACKUP_RETENTION_MS;
        lines.push(format!(
            "backup image (copy-tenant-backup) snapshot {} \"{}\", taken at {} (milliseconds), revision {}: its personal fields are in their sealed form and the subject key is not part of the image, so they cannot be opened after the destruction; it keeps the members, teams and invitations as they were until it is removed; expires at {expires} (milliseconds), 35 days after it was taken (D-351){}",
            image.id,
            image.label,
            image.created_at.0,
            image.revision.0,
            if expires <= now.0 { ", which has passed: the janitor removes it" } else { "" },
        ));
    }
    let mut exports = 0;
    for (key, text) in app.store.doc_list(tenant, REQUEST_KIND)? {
        let Ok(request) = serde_json::from_str::<ExportRequest>(&text) else {
            continue;
        };
        if request.subject != subject.as_str() {
            continue;
        }
        remove_documents(app, tenant, &key)?;
        exports += 1;
        lines.push(format!(
            "export of the subject's data (export-subject-data) {}, requested at {} (milliseconds), expiry {} (milliseconds): removed with its parts by this erasure, before its expiry",
            request.id, request.requested_at, request.expires_at,
        ));
    }
    if exports == 0 {
        lines
            .push("exports of the subject's data (export-subject-data): none was held".to_string());
    }
    Ok(lines)
}
