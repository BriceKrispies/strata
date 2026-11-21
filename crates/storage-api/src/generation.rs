//! The tenant generation: what a guarded commit compares, beside the revision.
//!
//! The revision is not safe to key on. A restored snapshot sets it back, a tenant removed and
//! created again starts over at 1, and a tenant file can be restored from outside the cell. The
//! generation changes, to a value never used before, with every change to what a load returns
//! (`docs/working-set.md`, "The tenant generation").
//!
//! It lives in a row of its own per tenant, so that reading it is a constant-cost read that does
//! not touch the schema or member documents. It is a fact about the store, and not about the
//! kernel's state: [`Loaded`] pairs a kernel [`State`] with the generation read in the same
//! transaction as its rows, and the kernel's `State` is unchanged.
//!
//! - The `counter` is raised by one in every write transaction that changes what [`Store::load`]
//!   or [`Store::load_complete`] returns for the tenant, and by nothing else. Never reset.
//! - The `nonce` is drawn from a [`NonceSource`] at the events the control plane controls: creating
//!   the tenant, [`Store::restore_snapshot`], an import, and acquiring a lease
//!   ([`Store::rotate_nonce`]). Opening a handle draws nothing. A rotation leaves the counter as it
//!   is.
//! - The generation is never part of `delta_hash`, `events_hash`, a [`crate::JournalEntry`] or the
//!   idempotency request hash.
//!
//! [`Store`]: crate::Store
//! [`Store::load`]: crate::Store::load
//! [`Store::load_complete`]: crate::Store::load_complete
//! [`Store::restore_snapshot`]: crate::Store::restore_snapshot
//! [`Store::rotate_nonce`]: crate::Store::rotate_nonce

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use strata_kernel::{canon, State, TenantId};

/// Changes, to a value never used before, with every change to what a load returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Generation {
    /// Random, 128 bits. Drawn only at events the control plane controls. Written as 32
    /// lower-case hex digits wherever it is serialised, so that no JSON reader loses a bit.
    #[serde(with = "nonce_text")]
    pub nonce: u128,
    /// Raised by one in every write transaction that changes what a load returns. Never reset: a
    /// nonce rotation leaves it as it is.
    pub counter: u64,
}

impl Generation {
    /// The first generation of a tenant: a freshly drawn nonce and counter 1.
    pub fn first(nonce: u128) -> Generation {
        Generation { nonce, counter: 1 }
    }

    /// This generation with the counter raised by one: what a write transaction that changes a
    /// load result leaves.
    pub fn raised(self) -> Generation {
        Generation {
            nonce: self.nonce,
            counter: self.counter + 1,
        }
    }

    /// This generation under a new nonce, with the counter as it is.
    pub fn rotated(self, nonce: u128) -> Generation {
        Generation {
            nonce,
            counter: self.counter,
        }
    }

    /// The nonce as the 32 lower-case hex digits a store keeps.
    pub fn nonce_hex(&self) -> String {
        format!("{:032x}", self.nonce)
    }

    /// The nonce of a stored text, or `None` when it is not 32 hex digits.
    pub fn parse_nonce(text: &str) -> Option<u128> {
        if text.len() != 32 {
            return None;
        }
        u128::from_str_radix(text, 16).ok()
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}:{}", self.nonce, self.counter)
    }
}

mod nonce_text {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(nonce: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{nonce:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        let text = String::deserialize(deserializer)?;
        super::Generation::parse_nonce(&text)
            .ok_or_else(|| serde::de::Error::custom("a nonce is 32 hex digits"))
    }
}

/// What [`Store::load_at`](crate::Store::load_at) and
/// [`Store::load_complete_at`](crate::Store::load_complete_at) return: the state, and the
/// generation read in the same read transaction as its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub state: State,
    /// `None` when the tenant does not exist.
    pub generation: Option<Generation>,
}

/// Where the nonce of a new generation comes from. A store takes one at construction.
///
/// In deployment it is the operating system's random source ([`random_nonces`]). In tests and in
/// the simulator it is seeded ([`SeededNonces`]): a pure function of the seed, the tenant and the
/// number of draws for that tenant so far, so that a run is reproduced from its seed.
pub trait NonceSource: Send + Sync {
    /// A nonce for `tenant`, not drawn before for it.
    fn draw(&self, tenant: &TenantId) -> u128;
}

/// The operating system's random source.
#[derive(Clone, Copy, Debug, Default)]
pub struct RandomNonces;

impl NonceSource for RandomNonces {
    fn draw(&self, _tenant: &TenantId) -> u128 {
        let mut bytes = [0u8; 16];
        // No source of randomness means no safe nonce: refuse rather than repeat one.
        getrandom::getrandom(&mut bytes).expect("the operating system's random source");
        u128::from_be_bytes(bytes)
    }
}

/// The source a store uses when none is given: the operating system's.
pub fn random_nonces() -> Arc<dyn NonceSource> {
    Arc::new(RandomNonces)
}

/// A seeded source: the nonce is the first 128 bits of the SHA-256 of the seed, the tenant and the
/// number of draws for that tenant so far (in the manner of D19). Two runs of the same schedule
/// under one seed draw the same nonces.
#[derive(Debug)]
pub struct SeededNonces {
    seed: u64,
    draws: Mutex<BTreeMap<TenantId, u64>>,
}

impl SeededNonces {
    pub fn new(seed: u64) -> Self {
        SeededNonces {
            seed,
            draws: Mutex::new(BTreeMap::new()),
        }
    }

    /// A shared source for a store's constructor.
    pub fn shared(seed: u64) -> Arc<dyn NonceSource> {
        Arc::new(Self::new(seed))
    }
}

impl NonceSource for SeededNonces {
    fn draw(&self, tenant: &TenantId) -> u128 {
        let nth = {
            let mut draws = self
                .draws
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let count = draws.entry(tenant.clone()).or_insert(0);
            *count += 1;
            *count
        };
        let hash = canon::hash_of(&("strata-nonce", self.seed, tenant.as_str(), nth));
        // 64 hex digits: the first 32 are the nonce.
        u128::from_str_radix(&hash[..32], 16).expect("a hash is hex")
    }
}
