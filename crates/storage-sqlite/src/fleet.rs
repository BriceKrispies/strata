//! Bringing every tenant file of a fleet up to this binary's layout in one pass, before the
//! process serves, instead of at each tenant's first touch.
//!
//! A tenant is opened one after another. The run reports each tenant's status and stops when the
//! tenants that failed exceed a stated share of the fleet, leaving the rest untouched so that a
//! faulty step does not run against every file. A tenant file whose ledger is newer than the
//! binary fails like any other (the migration runner refuses it); it is never rewound.

use crate::SqliteStore;
use strata_kernel::TenantId;
use strata_storage_api::StoreResult;

/// What happened to one tenant file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TenantStatus {
    /// The file was opened and its ledger is complete for this binary.
    Ready,
    /// The file could not be migrated; the text says why. Its ledger is unchanged by the step
    /// that failed.
    Failed(String),
    /// The run stopped before reaching this tenant.
    NotAttempted,
}

/// The outcome of a fleet migration, one entry per tenant in the order they were visited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetReport {
    pub tenants: Vec<(TenantId, TenantStatus)>,
    /// The share of the fleet, in percent, that may fail before the run stops.
    pub max_failure_percent: u8,
    /// Whether the run stopped early because failures passed that share.
    pub stopped: bool,
}

impl FleetReport {
    pub fn failed(&self) -> usize {
        self.count(|status| matches!(status, TenantStatus::Failed(_)))
    }

    pub fn ready(&self) -> usize {
        self.count(|status| matches!(status, TenantStatus::Ready))
    }

    pub fn not_attempted(&self) -> usize {
        self.count(|status| matches!(status, TenantStatus::NotAttempted))
    }

    /// Whether every tenant is ready.
    pub fn complete(&self) -> bool {
        self.ready() == self.tenants.len()
    }

    fn count(&self, pick: impl Fn(&TenantStatus) -> bool) -> usize {
        self.tenants.iter().filter(|(_, s)| pick(s)).count()
    }

    /// One line per tenant, then a summary line.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (tenant, status) in &self.tenants {
            let line = match status {
                TenantStatus::Ready => "ready".to_string(),
                TenantStatus::Failed(why) => format!("failed: {why}"),
                TenantStatus::NotAttempted => "not attempted".to_string(),
            };
            out.push_str(&format!("{tenant}: {line}\n"));
        }
        out.push_str(&format!(
            "{} ready, {} failed, {} not attempted of {} tenants; stop above {}% failed{}\n",
            self.ready(),
            self.failed(),
            self.not_attempted(),
            self.tenants.len(),
            self.max_failure_percent,
            if self.stopped { " (stopped)" } else { "" }
        ));
        out
    }
}

/// Visit `tenants` in order, calling `migrate` on each. After a failure, if the failed tenants
/// are more than `max_failure_percent` of all `tenants`, the run stops and the tenants not yet
/// visited are `NotAttempted`. A share of 0 stops at the first failure; 100 never stops.
pub fn migrate_in_order<F>(
    tenants: Vec<TenantId>,
    max_failure_percent: u8,
    mut migrate: F,
) -> FleetReport
where
    F: FnMut(&TenantId) -> Result<(), String>,
{
    let total = tenants.len();
    let mut failed = 0usize;
    let mut stopped = false;
    let mut out = Vec::with_capacity(total);
    for tenant in tenants {
        if stopped {
            out.push((tenant, TenantStatus::NotAttempted));
            continue;
        }
        match migrate(&tenant) {
            Ok(()) => out.push((tenant, TenantStatus::Ready)),
            Err(why) => {
                failed += 1;
                out.push((tenant, TenantStatus::Failed(why)));
                if failed * 100 > total * usize::from(max_failure_percent) {
                    stopped = true;
                }
            }
        }
    }
    FleetReport {
        tenants: out,
        max_failure_percent,
        stopped,
    }
}

impl SqliteStore {
    /// Migrate every active tenant file now, as [`migrate_in_order`] says. The files are opened
    /// and closed again; nothing is kept open.
    pub fn migrate_fleet(&self, max_failure_percent: u8) -> StoreResult<FleetReport> {
        let tenants = self.directory()?;
        Ok(migrate_in_order(tenants, max_failure_percent, |tenant| {
            let file = self
                .directory_file(tenant, "active")
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "the tenant is no longer active".to_string())?;
            self.open_file(&file).map(|_| ()).map_err(|e| e.to_string())
        }))
    }
}
