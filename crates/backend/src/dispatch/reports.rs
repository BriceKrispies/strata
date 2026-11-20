//! Delivery reports (EML-017, EML-1567, D-6569): what the dispatcher does with a report the
//! provider's adapter has read and authenticated.
//!
//! The specification states no format and no signature scheme of the provider's reports, so the
//! contract receive-delivery-report is not served yet: the adapter that reads and verifies the
//! provider's reports is WG-5215, and it calls [`apply`]. Everything that does not depend on
//! the provider is here: the five-minute window around the receipt time, the mapping to the
//! tenant and the message by the provider reference recorded at submission
//! ([`super::store::provider_reference`]), an unknown reference recorded as unmatched and
//! changing nothing else, and an event id seen within seven days changing nothing.

use super::store::{self, Suppression, SuppressionReason, ALL, ALL_PROMOTIONAL};
use super::{MessageState, Purpose};
use crate::app::{App, LOG};
use serde_json::json;
use strata_kernel::{TenantId, Timestamp};
use strata_storage_api::StoreResult;

/// The most a report's timestamp may differ from its receipt (D-6569).
pub const REPORT_WINDOW_MS: i64 = 5 * 60_000;

/// What a report says happened to a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportKind {
    Delivered,
    /// A bounce; only a permanent one changes anything.
    Bounce {
        permanent: bool,
    },
    Complaint,
}

/// A delivery report, authenticated by the provider's adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryReport {
    /// The provider's event identifier.
    pub event: String,
    /// The provider reference the message was submitted with.
    pub reference: String,
    pub kind: ReportKind,
    /// The timestamp the signature covers.
    pub at: Timestamp,
}

/// What became of a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportOutcome {
    Applied,
    /// The event was seen within seven days: nothing changed.
    Seen,
    /// No message has the reference: recorded as unmatched, nothing changed.
    Unmatched,
    /// The timestamp is more than five minutes from the receipt: refused (error stale-report).
    Stale,
}

/// Apply a report received at `received` (EML-017, EML-1567): a permanent bounce suppresses the
/// whole address and marks the message bounced; a complaint suppresses the category of the
/// message, and for a promotional category every promotional category, and marks it complained;
/// a delivery marks a sent message delivered.
pub fn apply(
    app: &App,
    report: &DeliveryReport,
    received: Timestamp,
) -> StoreResult<ReportOutcome> {
    if (report.at.0 - received.0).abs() > REPORT_WINDOW_MS {
        return Ok(ReportOutcome::Stale);
    }
    let unmatched = || -> StoreResult<ReportOutcome> {
        LOG.warn(
            "mail.report_unmatched",
            "",
            json!({ "event": report.event }),
        );
        Ok(ReportOutcome::Unmatched)
    };
    let Some((tenant, message)) = report.reference.split_once(':') else {
        return unmatched();
    };
    let Ok(tenant) = TenantId::new(tenant) else {
        return unmatched();
    };
    let Some(mut record) = store::message(app, &tenant, message)? else {
        return unmatched();
    };
    if !store::first_sighting(app, &tenant, &report.event, received)? {
        return Ok(ReportOutcome::Seen);
    }
    let hash = record.address_hash.clone();
    match report.kind {
        ReportKind::Delivered => {
            if record.state == MessageState::Sent {
                record.state = MessageState::Delivered;
            }
        }
        ReportKind::Bounce { permanent: false } => {}
        ReportKind::Bounce { permanent: true } => {
            record.state = MessageState::Bounced;
            if let Some(hash) = &hash {
                suppress(app, &tenant, hash, ALL, SuppressionReason::Bounce, received)?;
            }
        }
        ReportKind::Complaint => {
            record.state = MessageState::Complained;
            if let (Some(hash), Some(category)) = (&hash, &record.category) {
                suppress(
                    app,
                    &tenant,
                    hash,
                    category,
                    SuppressionReason::Complaint,
                    received,
                )?;
                let promotional = store::category(app, &tenant, category)?
                    .is_some_and(|c| c.purpose == Purpose::Promotional);
                if promotional {
                    suppress(
                        app,
                        &tenant,
                        hash,
                        ALL_PROMOTIONAL,
                        SuppressionReason::Complaint,
                        received,
                    )?;
                }
            }
        }
    }
    record.updated_at = received.0;
    store::put_message(app, &tenant, &record)?;
    Ok(ReportOutcome::Applied)
}

fn suppress(
    app: &App,
    tenant: &TenantId,
    hash: &str,
    category: &str,
    reason: SuppressionReason,
    at: Timestamp,
) -> StoreResult<()> {
    store::add_suppression(
        app,
        tenant,
        &Suppression {
            address_hash: hash.to_string(),
            category: category.to_string(),
            reason,
            recorded_at: at.0,
        },
    )
}
