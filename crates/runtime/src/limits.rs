//! Small, bounded admission tools shared by the services: a cap on how many of something one key
//! may hold at once, and a cap on how often one key may do something.
//!
//! A key table never grows without bound. The cap refuses a new key when the table is full of
//! held keys. The rate limiter, when full, forgets the key that has been quiet the longest, so a
//! spray of new sources cannot lock every other source out.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};

/// The most keys either tool tracks at once.
pub const MAX_KEYS: usize = 20_000;

/// The key a source address counts under. An IPv6 address counts as its /64 network, since one
/// subscriber holds a whole /64 and could otherwise use a new address for every request; an IPv4
/// address, or text that is not an address, counts as itself.
pub fn source_key(source: &str) -> String {
    let bare = source.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => {
            let [a, b, c, d, ..] = v6.segments();
            format!("v6:{a:x}:{b:x}:{c:x}:{d:x}")
        }
        _ => source.to_string(),
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// At most `limit` holders per key at once. A holder is a [`Permit`]; dropping it frees the place.
#[derive(Clone, Debug)]
pub struct Cap {
    limit: usize,
    held: Arc<Mutex<BTreeMap<String, usize>>>,
}

impl Cap {
    pub fn new(limit: usize) -> Self {
        Cap {
            limit,
            held: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// A place for `key`, or `None` when the key already holds `limit` places (or the table of
    /// keys is full).
    pub fn acquire(&self, key: &str) -> Option<Permit> {
        let mut held = locked(&self.held);
        let known = held.contains_key(key);
        if !known && held.len() >= MAX_KEYS {
            return None;
        }
        let count = held.entry(key.to_string()).or_insert(0);
        if *count >= self.limit {
            return None;
        }
        *count += 1;
        Some(Permit {
            held: self.held.clone(),
            key: key.to_string(),
        })
    }

    /// How many places `key` holds now.
    pub fn held_by(&self, key: &str) -> usize {
        locked(&self.held).get(key).copied().unwrap_or(0)
    }
}

/// One place held under a [`Cap`], freed on drop.
#[derive(Debug)]
pub struct Permit {
    held: Arc<Mutex<BTreeMap<String, usize>>>,
    key: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut held = locked(&self.held);
        if let Some(count) = held.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

/// At most `max` events per key in any window of `window_ms` (a sliding window).
#[derive(Debug)]
pub struct Rate {
    max: usize,
    window_ms: i64,
    table: Mutex<RateTable>,
}

#[derive(Debug, Default)]
struct RateTable {
    hits: BTreeMap<String, VecDeque<i64>>,
    /// Each key with the time of its latest hit, oldest first: the key to forget when full.
    latest: BTreeSet<(i64, String)>,
}

impl Rate {
    pub fn new(max: usize, window_ms: i64) -> Self {
        Rate {
            max,
            window_ms,
            table: Mutex::new(RateTable::default()),
        }
    }

    /// Count an event for `key` at `now_ms` and say whether it is within the limit. A refused
    /// event is not counted. When the table is full, the key that was last heard from longest
    /// ago is forgotten to make room.
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        if self.max == 0 {
            return false;
        }
        let mut guard = locked(&self.table);
        let RateTable { hits, latest } = &mut *guard;
        let horizon = now_ms - self.window_ms;
        if !hits.contains_key(key) && hits.len() >= MAX_KEYS {
            if let Some((_, oldest)) = latest.pop_first() {
                hits.remove(&oldest);
            }
        }
        let times = hits.entry(key.to_string()).or_default();
        let previous = times.back().copied();
        while times.front().is_some_and(|at| *at <= horizon) {
            times.pop_front();
        }
        if times.len() >= self.max {
            return false;
        }
        times.push_back(now_ms);
        if let Some(previous) = previous {
            latest.remove(&(previous, key.to_string()));
        }
        latest.insert((now_ms, key.to_string()));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cap_holds_each_key_to_its_limit_and_frees_places_on_drop() {
        let cap = Cap::new(2);
        let first = cap.acquire("a").expect("first");
        let _second = cap.acquire("a").expect("second");
        assert!(cap.acquire("a").is_none(), "a third place for the same key");
        assert!(cap.acquire("b").is_some(), "another key is unaffected");
        assert_eq!(cap.held_by("a"), 2);
        drop(first);
        assert_eq!(cap.held_by("a"), 1);
        assert!(
            cap.acquire("a").is_some(),
            "a freed place can be taken again"
        );
    }

    #[test]
    fn a_cap_forgets_keys_that_hold_nothing() {
        let cap = Cap::new(1);
        drop(cap.acquire("gone"));
        assert_eq!(cap.held_by("gone"), 0);
        assert!(cap.held.lock().unwrap().is_empty());
    }

    #[test]
    fn a_rate_allows_its_maximum_per_window_and_recovers_after_it() {
        let rate = Rate::new(3, 1_000);
        assert!(rate.allow("k", 0));
        assert!(rate.allow("k", 100));
        assert!(rate.allow("k", 200));
        assert!(!rate.allow("k", 300), "the fourth in a window");
        assert!(
            !rate.allow("k", 999),
            "a refusal is not counted, the window has not moved"
        );
        assert!(rate.allow("other", 300), "another key has its own window");
        assert!(rate.allow("k", 1_001), "the first hit has left the window");
    }

    #[test]
    fn a_full_rate_table_forgets_its_quietest_key_instead_of_refusing_new_ones() {
        let rate = Rate::new(1, 1_000_000);
        for n in 0..MAX_KEYS {
            assert!(rate.allow(&format!("k{n}"), n as i64));
        }
        assert!(rate.allow("new", MAX_KEYS as i64), "a new key when full");
        assert!(!rate.allow("new", MAX_KEYS as i64 + 1), "and it is limited");
        assert!(
            rate.allow("k0", MAX_KEYS as i64 + 2),
            "the quietest key was forgotten"
        );
        assert!(
            !rate.allow("k5", MAX_KEYS as i64 + 3),
            "a recent key is kept"
        );
    }

    #[test]
    fn an_ipv6_source_counts_as_its_64_network() {
        assert_eq!(
            source_key("2001:db8:1:2:aaaa:bbbb:cccc:dddd"),
            source_key("2001:db8:1:2::1")
        );
        assert_ne!(source_key("2001:db8:1:2::1"), source_key("2001:db8:1:3::1"));
        assert_eq!(source_key("203.0.113.7"), "203.0.113.7");
        assert_eq!(source_key("unknown"), "unknown");
    }
}
