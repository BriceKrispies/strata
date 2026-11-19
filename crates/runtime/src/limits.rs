//! Small, bounded admission tools shared by the services: a cap on how many of something one key
//! may hold at once, and a cap on how often one key may do something.
//!
//! Both fail closed. A key table never grows without bound: the rate limiter drops keys that
//! have gone quiet, and refuses a new key outright when the table is still full after that.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

/// The most keys either tool tracks at once.
pub const MAX_KEYS: usize = 20_000;

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
    hits: Mutex<BTreeMap<String, VecDeque<i64>>>,
}

impl Rate {
    pub fn new(max: usize, window_ms: i64) -> Self {
        Rate {
            max,
            window_ms,
            hits: Mutex::new(BTreeMap::new()),
        }
    }

    /// Count an event for `key` at `now_ms` and say whether it is within the limit. A refused
    /// event is not counted.
    pub fn allow(&self, key: &str, now_ms: i64) -> bool {
        let mut hits = locked(&self.hits);
        let horizon = now_ms - self.window_ms;
        if !hits.contains_key(key) && hits.len() >= MAX_KEYS {
            hits.retain(|_, times| {
                while times.front().is_some_and(|at| *at <= horizon) {
                    times.pop_front();
                }
                !times.is_empty()
            });
            if hits.len() >= MAX_KEYS {
                return false;
            }
        }
        let times = hits.entry(key.to_string()).or_default();
        while times.front().is_some_and(|at| *at <= horizon) {
            times.pop_front();
        }
        if times.len() >= self.max {
            return false;
        }
        times.push_back(now_ms);
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
}
