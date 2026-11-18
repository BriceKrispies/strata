//! Replaying what production actually committed.
//!
//! The store journals every accepted transition: the intent, the exact context (tenant,
//! principal, timestamp, correlation) and the hashes of the delta and events the kernel
//! produced. Replaying the journal from an empty state runs those inputs through the kernel
//! again and compares the hashes, entry by entry, and the final state with the live one.
//!
//! What this establishes: the kernel is deterministic on real inputs, and the live state is
//! exactly what the journalled decisions produce. What it does not: rejected requests are not
//! journalled, so the replay says nothing about them; nor does it re-enact sessions, network
//! timing, lost races that were retried, or the delivery of events.

use serde::{Deserialize, Serialize};
use strata_kernel::{
    canon, Context, Intent, StateRevision, TenantId, TenantState, World, KERNEL_VERSION,
};

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
    let mut world = World::default();
    let mut agreed = 0;
    let mut first_mismatch = None;
    let mut other_versions = Vec::new();
    for entry in entries {
        if entry.kernel_version != KERNEL_VERSION && !other_versions.contains(&entry.kernel_version)
        {
            other_versions.push(entry.kernel_version.clone());
        }
        let problem = match world.step(&entry.intent, &entry.context) {
            Ok((next, transition)) => {
                world = next;
                if canon::hash_of(&transition.delta) != entry.delta_hash {
                    Some("the replayed state delta differs from the recorded one".to_string())
                } else if canon::hash_of(&transition.events) != entry.events_hash {
                    Some("the replayed events differ from the recorded ones".to_string())
                } else if transition.delta.new_revision != entry.revision {
                    Some("the replay reached a different revision".to_string())
                } else {
                    None
                }
            }
            Err(error) => Some(format!("the kernel now rejects it: {error}")),
        };
        match problem {
            None => agreed += 1,
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
        matches_live: replayed_hash == live_hash,
        replayed_hash,
        live_hash,
        other_kernel_versions: other_versions,
        replayed: world,
    }
}
