//! Replaying what production actually committed.
//!
//! The store journals every accepted transition: the intent, the exact context (tenant,
//! principal, timestamp, correlation) and the hashes of the delta and events the kernel
//! produced. Replaying the journal from an empty state runs those inputs through the kernel
//! again and compares the hashes, entry by entry, and the final state with the live one.
//!
//! What this establishes: the kernel is deterministic on real inputs, and the live state is
//! exactly what the journalled decisions produce.
//!
//! A store that seals personal fields (D-350) records the hashes of the sealed delta and events,
//! and keeps the intents' personal values sealed. [`replay_journal_with`] takes the hashing of
//! that store: an entry agrees when its replayed delta and events hash as recorded either plainly
//! (written before sealing, or with no personal field) or sealed. An entry whose intent still
//! holds a sealed value after it was opened (its subject was erased) and that does not agree is
//! reported as redacted, with its revision, and not as a mismatch (PRV-361). What it does not: rejected requests are not
//! journalled, so the replay says nothing about them; nor does it re-enact sessions, network
//! timing, lost races that were retried, or the delivery of events.

use serde::{Deserialize, Serialize};
use strata_kernel::{
    canon, Context, DomainErrorExt, Intent, StateRevision, TenantId, TenantSchema, TenantState,
    Transition, World, KERNEL_VERSION,
};

/// The start of a sealed value (`strata_storage_api::sealed::TOKEN_PREFIX`, which this crate
/// does not depend on).
const SEALED: &str = "sealed:v1:";

/// The part of a journal entry a replay needs. Mirrors `strata_storage_api::JournalEntry`
/// without depending on the storage crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    pub revision: StateRevision,
    pub kernel_version: String,
    pub intent: Intent,
    pub context: Context,
    pub delta_hash: String,
    pub events_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalReplay {
    pub tenant: TenantId,
    pub entries: usize,
    /// Entries whose replayed delta and events hash exactly as recorded.
    pub agreed: usize,
    /// The revision of the first entry that did not agree, with what went wrong.
    pub first_mismatch: Option<(u64, String)>,
    /// Entries that could not be decided again because a value they hold stays sealed (its
    /// subject was erased): the revision and what was found. Entries after the first of them
    /// replay over a state that may differ from the one recorded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted: Vec<(u64, String)>,
    pub replayed_hash: String,
    pub live_hash: String,
    /// The replayed final state is byte-for-byte the live state.
    pub matches_live: bool,
    /// Kernel versions found in the journal, other than the running one.
    pub other_kernel_versions: Vec<String>,
    #[serde(skip)]
    pub replayed: World,
}

/// Replay one tenant's journal from nothing and compare with the live state.
pub fn replay_journal(
    tenant: &TenantId,
    entries: &[Recorded],
    live: Option<&TenantState>,
) -> JournalReplay {
    replay_journal_with(tenant, entries, live, &|_, _| None)
}

/// [`replay_journal`] for a journal written by a store that seals: `sealed` gives the hashes of a
/// replayed transition's delta and events in the store's form, from the schema the transition was
/// decided over, or `None` when they cannot be made again (a key is gone).
pub fn replay_journal_with(
    tenant: &TenantId,
    entries: &[Recorded],
    live: Option<&TenantState>,
    sealed: &dyn Fn(Option<&TenantSchema>, &Transition) -> Option<(String, String)>,
) -> JournalReplay {
    let mut world = World::default();
    let mut agreed = 0;
    let mut first_mismatch = None;
    let mut redacted = Vec::new();
    let mut other_versions = Vec::new();
    for entry in entries {
        if entry.kernel_version != KERNEL_VERSION && !other_versions.contains(&entry.kernel_version)
        {
            other_versions.push(entry.kernel_version.clone());
        }
        let before = world.tenants.get(tenant).map(|state| state.schema.clone());
        let problem = match world.step(&entry.intent, &entry.context) {
            Ok((next, transition)) => {
                world = next;
                let hashes = sealed(before.as_deref(), &transition);
                let (sealed_delta, sealed_events) = match &hashes {
                    Some((delta, events)) => (Some(delta.as_str()), Some(events.as_str())),
                    None => (None, None),
                };
                if canon::hash_of(&transition.delta) != entry.delta_hash
                    && sealed_delta != Some(entry.delta_hash.as_str())
                {
                    Some("the replayed state delta differs from the recorded one".to_string())
                } else if canon::hash_of(&transition.events) != entry.events_hash
                    && sealed_events != Some(entry.events_hash.as_str())
                {
                    Some("the replayed events differ from the recorded ones".to_string())
                } else if transition.delta.new_revision != entry.revision {
                    Some("the replay reached a different revision".to_string())
                } else {
                    None
                }
            }
            Err(error) => Some(format!("the kernel now rejects it: {}", error.describe())),
        };
        match problem {
            None => agreed += 1,
            Some(problem) if canon::canonical_json(&entry.intent).contains(SEALED) => {
                redacted.push((
                    entry.revision.0,
                    format!("it holds a value sealed under a destroyed key: {problem}"),
                ));
            }
            Some(problem) if first_mismatch.is_none() => {
                first_mismatch = Some((entry.revision.0, problem))
            }
            Some(_) => {}
        }
    }
    let replayed_hash = world.tenant_hash(tenant);
    let live_hash = canon::hash_of(&live);
    JournalReplay {
        tenant: tenant.clone(),
        entries: entries.len(),
        agreed,
        first_mismatch,
        redacted,
        matches_live: replayed_hash == live_hash,
        replayed_hash,
        live_hash,
        other_kernel_versions: other_versions,
        replayed: world,
    }
}
