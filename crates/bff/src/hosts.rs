//! Learning the sites this process serves from the backend, so that a tenant created while the
//! process runs needs no restart.
//!
//! `STRATA_SITES` and `STRATA_ORIGINS` stay what they were: a static override, consulted first,
//! so a host they name is never moved to another tenant by what the backend says. Beside them the
//! process keeps a [`Directory`] it fills from the backend's internal route
//! `/internal/platform/hosts`, which is reached with the service token and lists every host a
//! tenant is bound to with the origins its pages are served from.
//!
//! - **Bounded age.** The list is fetched again when it is older than `max_age_ms`. A list older
//!   than `max_stale_ms` (the backend has not answered for that long) is not believed at all, so a
//!   host the backend dropped does not stay served for ever.
//! - **A miss refreshes, once in a while.** A request for a host no list names asks the backend
//!   again, but no more often than once per `miss_interval_ms` however many unknown hosts ask, so
//!   a scan of made-up host names costs the backend at most one call per interval. Attempts that
//!   fail count too.
//! - **One call at a time.** Concurrent requests that find the list due share one fetch.

use crate::{Bff, LOG};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::RwLock;
use std::time::Duration;
use strata_kernel::TenantId;
use strata_protocol::api::HEADER_SERVICE;
use strata_protocol::log::now_ms;

/// How the learned list is kept fresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refresh {
    /// The list is fetched again once it is this old (`STRATA_HOSTS_MAX_AGE_SECS`, default 60).
    pub max_age_ms: i64,
    /// A list this old is no longer believed (ten times `max_age_ms`).
    pub max_stale_ms: i64,
    /// The least time between two calls to the backend, whatever asked for them
    /// (`STRATA_HOSTS_MISS_INTERVAL_SECS`, default 5).
    pub miss_interval_ms: i64,
}

impl Default for Refresh {
    fn default() -> Self {
        Refresh {
            max_age_ms: 60_000,
            max_stale_ms: 600_000,
            miss_interval_ms: 5_000,
        }
    }
}

impl Refresh {
    /// The refresh policy for a maximum age and a miss interval, in milliseconds.
    pub fn new(max_age_ms: i64, miss_interval_ms: i64) -> Self {
        Refresh {
            max_age_ms,
            max_stale_ms: max_age_ms.saturating_mul(10),
            miss_interval_ms,
        }
    }
}

/// One host of the backend's list.
#[derive(Debug, Deserialize)]
struct Binding {
    host: String,
    tenant: String,
    #[serde(default)]
    origins: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Listing {
    hosts: Vec<Binding>,
}

#[derive(Default)]
struct Learned {
    sites: BTreeMap<String, TenantId>,
    origins: Vec<String>,
    /// When the list was fetched; `None` before the first answer.
    fetched_at: Option<i64>,
}

/// The hosts and origins learned from the backend.
pub struct Directory {
    refresh: Refresh,
    learned: RwLock<Learned>,
    /// When the backend was last asked, answered or not.
    last_attempt: AtomicI64,
    /// Held while a fetch is running.
    fetching: tokio::sync::Mutex<()>,
}

impl Directory {
    pub fn new(refresh: Refresh) -> Self {
        Directory {
            refresh,
            learned: RwLock::new(Learned::default()),
            last_attempt: AtomicI64::new(i64::MIN / 2),
            fetching: tokio::sync::Mutex::new(()),
        }
    }

    /// The learned list when it is still believed.
    fn believed<T>(&self, read: impl FnOnce(&Learned) -> T) -> Option<T> {
        let learned = self.learned.read().ok()?;
        let fetched = learned.fetched_at?;
        (now_ms() - fetched <= self.refresh.max_stale_ms).then(|| read(&learned))
    }

    /// The tenant a learned host serves.
    pub fn site(&self, host: &str) -> Option<TenantId> {
        self.believed(|learned| learned.sites.get(host).cloned())
            .flatten()
    }

    /// Every learned origin.
    pub fn origins(&self) -> Vec<String> {
        self.believed(|learned| learned.origins.clone())
            .unwrap_or_default()
    }

    /// Replace the list with what the backend said. Entries that are not a well-formed host and
    /// tenant, and origins that are not `http(s)://` ones, are dropped.
    fn replace(&self, listing: Listing, at: i64) {
        let mut learned = Learned {
            fetched_at: Some(at),
            ..Learned::default()
        };
        for binding in listing.hosts {
            let host = binding.host.to_lowercase();
            let Ok(tenant) = TenantId::new(binding.tenant.as_str()) else {
                continue;
            };
            if host.is_empty() || !host.bytes().all(is_host_byte) {
                continue;
            }
            learned.sites.insert(host, tenant);
            learned.origins.extend(
                binding
                    .origins
                    .into_iter()
                    .filter(|o| o.starts_with("http://") || o.starts_with("https://")),
            );
        }
        learned.origins.sort();
        learned.origins.dedup();
        if let Ok(mut held) = self.learned.write() {
            *held = learned;
        }
    }

    /// Whether the backend may be asked now: not within `miss_interval_ms` of the last ask, and
    /// either a host is unknown (`miss`) or the list is older than `max_age_ms`.
    fn due(&self, miss: bool, now: i64) -> bool {
        if now.saturating_sub(self.last_attempt.load(Ordering::Relaxed))
            < self.refresh.miss_interval_ms
        {
            return false;
        }
        let aged = match self.learned.read().ok().and_then(|l| l.fetched_at) {
            Some(fetched) => now - fetched >= self.refresh.max_age_ms,
            None => true,
        };
        miss || aged
    }
}

fn is_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'[' | b']' | b':')
}

/// Ask the backend for its hosts when the list is due (see the module description), and keep what
/// it says. `miss` is true when the request that prompted this named a host nobody knows.
/// A backend that cannot be reached leaves the list as it was.
pub async fn refresh_if_due(bff: &Bff, miss: bool) {
    if !bff.directory.due(miss, now_ms()) {
        return;
    }
    let _one_at_a_time = bff.directory.fetching.lock().await;
    // Another request may have fetched while this one waited.
    if !bff.directory.due(miss, now_ms()) {
        return;
    }
    bff.directory
        .last_attempt
        .store(now_ms(), Ordering::Relaxed);
    match fetch(bff).await {
        Ok(listing) => {
            let hosts = listing.hosts.len();
            bff.directory.replace(listing, now_ms());
            LOG.info("hosts.refreshed", "", json!({ "hosts": hosts }));
        }
        Err(why) => LOG.warn("hosts.refresh_failed", "", json!({ "error": why })),
    }
}

async fn fetch(bff: &Bff) -> Result<Listing, String> {
    let response = bff
        .http
        .post(format!("{}/internal/platform/hosts", bff.cfg.backend))
        .header(HEADER_SERVICE, &bff.cfg.service_token)
        .timeout(Duration::from_secs(3))
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("the backend answered {}", response.status()));
    }
    response.json::<Listing>().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(entries: &[(&str, &str, &[&str])]) -> Listing {
        Listing {
            hosts: entries
                .iter()
                .map(|(host, tenant, origins)| Binding {
                    host: host.to_string(),
                    tenant: tenant.to_string(),
                    origins: origins.iter().map(|o| o.to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_list_is_believed_only_while_it_is_fresh_enough_and_malformed_entries_are_dropped() {
        let directory = Directory::new(Refresh::new(1_000, 0));
        assert_eq!(directory.site("a.example"), None, "nothing fetched yet");
        directory.replace(
            listing(&[
                ("A.example", "blog", &["https://a.example", "javascript:x"]),
                ("bad host", "blog", &[]),
                ("b.example", "Not A Tenant!", &[]),
            ]),
            now_ms(),
        );
        assert_eq!(
            directory.site("a.example").map(|t| t.to_string()),
            Some("blog".into())
        );
        assert_eq!(directory.site("bad host"), None);
        assert_eq!(directory.site("b.example"), None);
        assert_eq!(directory.origins(), vec!["https://a.example".to_string()]);
        // Older than ten maximum ages: not believed.
        directory.replace(listing(&[("a.example", "blog", &[])]), now_ms() - 10_001);
        assert_eq!(directory.site("a.example"), None);
    }

    #[test]
    fn unknown_hosts_cannot_make_the_backend_be_asked_more_than_once_per_interval() {
        let directory = Directory::new(Refresh::new(60_000, 5_000));
        assert!(directory.due(true, 1_000_000), "the first miss asks");
        directory.last_attempt.store(1_000_000, Ordering::Relaxed);
        for later in [1_000_001, 1_002_000, 1_004_999] {
            assert!(!directory.due(true, later), "{later}: inside the interval");
        }
        assert!(directory.due(true, 1_005_000), "the interval has passed");
        // A fresh list is not fetched again for a host that is known.
        directory.replace(listing(&[]), 1_005_000);
        directory.last_attempt.store(1_005_000, Ordering::Relaxed);
        assert!(!directory.due(false, 1_030_000));
        assert!(directory.due(false, 1_065_000), "the list has aged out");
    }
}
