//! The hash chain of a ledger journal and the checks that rest on it (LDG-2921 to LDG-2924,
//! D-8431), and the transaction of the double-entry journal (LDG-2903 to LDG-2917, D-8430).
//!
//! Each entry of an account carries the hash of the entry before it (the genesis value of the
//! account for the first) and a hash over its own content, so that a changed, removed or
//! reordered entry is found from the journal alone ([`check_journal`]). The head of an account
//! journal is the hash of its last entry, held in the header of the account (`head-hash`) and
//! anchored in the audit log by the commit that changes it (`storage_api::ledger_audit`). A
//! restore is compared with the journal it would replace ([`restore_refusal`]) and with the
//! anchors ([`anchor_refusal`]).
//!
//! Nothing here reads a clock or draws a random value: every hash is a function of the content.

use crate::canon;
use crate::types::{LedgerAccount, LedgerAccountId, LedgerEntry, LedgerState};

/// The code of a journal whose chain does not hold (LDG-2922).
pub const CHAIN_BROKEN: &str = "chain-broken";
/// The code of a journal whose stored balance is not the sum of its entries (LDG-020).
pub const BALANCE_DRIFT: &str = "balance-drift";
/// The code of a restore that would drop an entry the journal holds (LDG-2911).
pub const RESTORE_DROPS_ENTRIES: &str = "restore-drops-entries";
/// The code of a restore or an import whose head is older than the last anchor (LDG-2924).
pub const HEAD_BEHIND_ANCHOR: &str = "head-behind-anchor";

/// The genesis value of the journal of an account: the hash the first entry names as its
/// predecessor. The account id derives from the tenant, so the value is the tenant's and the
/// account's alone.
pub fn genesis(account: &LedgerAccountId) -> String {
    canon::hash_of(&("strata-ledger-genesis", account))
}

/// The hash of an entry that follows the entry hashed `previous`: SHA-256 over the canonical form
/// of the account, number, amount, key, reason, time, correction and transaction of the entry
/// and `previous`. `caused_by` is not in it, because an erasure clears it and the chain must
/// verify before and after (LDG-027).
pub fn entry_hash(entry: &LedgerEntry, previous: &str) -> String {
    canon::hash_of(&(
        "strata-ledger-entry",
        &entry.account,
        entry.sequence,
        entry.amount,
        &entry.idempotency_key,
        &entry.reason,
        entry.at,
        entry.corrects,
        &entry.transaction,
        previous,
    ))
}

/// The hash an entry written after `header` names as its predecessor.
pub fn previous_of(header: Option<&LedgerAccount>, account: &LedgerAccountId) -> String {
    header
        .and_then(|h| h.head_hash.clone())
        .unwrap_or_else(|| genesis(account))
}

/// The entry with its chain fields set: it follows the journal whose head is `previous`.
pub fn seal(mut entry: LedgerEntry, previous: &str) -> LedgerEntry {
    entry.previous_hash = Some(previous.to_string());
    entry.hash = Some(entry_hash(&entry, previous));
    entry
}

/// Where a journal first fails its check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Break {
    pub account: LedgerAccountId,
    /// The number of the first entry that is changed, removed or out of order.
    pub sequence: u64,
    /// [`CHAIN_BROKEN`] or [`BALANCE_DRIFT`].
    pub code: &'static str,
}

/// Check the journal of one account (LDG-2921, LDG-2922): the numbers run from one without a
/// gap, each chained entry names the hash of the one before it and hashes to its own, an entry
/// made before the chain existed may precede the chained ones and none may follow them, the
/// header names the last number and the head, and the balance is the sum of the entries.
/// `entries` is the whole journal in order. The first failure is the answer.
pub fn check_journal(
    account: &LedgerAccountId,
    header: Option<&LedgerAccount>,
    entries: &[LedgerEntry],
) -> Option<Break> {
    let broken = |sequence: u64| {
        Some(Break {
            account: *account,
            sequence,
            code: CHAIN_BROKEN,
        })
    };
    let mut previous = genesis(account);
    let mut chained = false;
    let mut sum: i64 = 0;
    for (index, entry) in entries.iter().enumerate() {
        let expected = index as u64 + 1;
        if entry.sequence != expected || entry.account != *account {
            return broken(expected);
        }
        match (&entry.hash, &entry.previous_hash) {
            (Some(hash), Some(named)) => {
                if *named != previous || *hash != entry_hash(entry, &previous) {
                    return broken(expected);
                }
                previous = hash.clone();
                chained = true;
            }
            (None, None) if !chained => {}
            _ => return broken(expected),
        }
        sum = match sum.checked_add(entry.amount.0) {
            Some(sum) => sum,
            None => return broken(expected),
        };
    }
    let Some(header) = header else {
        return if entries.is_empty() { None } else { broken(1) };
    };
    let held = entries.len() as u64;
    if header.last_sequence != held {
        return broken(held.min(header.last_sequence) + 1);
    }
    if chained && header.head_hash.as_deref() != Some(previous.as_str()) {
        return broken(held);
    }
    if !chained && header.head_hash.is_some() {
        return broken(1);
    }
    if sum != header.balance.0 {
        return Some(Break {
            account: *account,
            sequence: held,
            code: BALANCE_DRIFT,
        });
    }
    None
}

/// Check every account of a complete ledger state, in the order of the account ids; the first
/// failure is the answer. Returns the number of entries read when none fails.
pub fn check_state(ledger: &LedgerState) -> Result<u64, Break> {
    let mut read = 0;
    for (key, header) in &ledger.accounts {
        let entries = ledger.entries.get(key).map_or(&[][..], Vec::as_slice);
        if let Some(found) = check_journal(&header.account, Some(header), entries) {
            return Err(found);
        }
        read += entries.len() as u64;
    }
    for (key, entries) in &ledger.entries {
        if !ledger.accounts.contains_key(key) {
            if let Some(first) = entries.first() {
                return Err(Break {
                    account: first.account,
                    sequence: 1,
                    code: CHAIN_BROKEN,
                });
            }
        }
    }
    Ok(read)
}

/// A restore refused, with its code and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

/// Whether restoring `snapshot` over `current` would drop or change an entry (LDG-2911): the
/// snapshot must hold every account of the journal with at least its last number, and an account
/// with the same last number must have the same head. A snapshot of a journal that is a prefix
/// of the journal it replaces would silently drop rows, so it is refused.
pub fn restore_refusal(
    current: &std::collections::BTreeMap<String, (u64, Option<String>)>,
    snapshot: &LedgerState,
) -> Option<Refusal> {
    for (key, (last, head)) in current {
        let kept = snapshot.accounts.get(key);
        let kept_last = kept.map_or(0, |a| a.last_sequence);
        if kept_last < *last {
            return Some(Refusal {
                code: RESTORE_DROPS_ENTRIES,
                message: format!(
                    "the journal of account {key} holds {last} entries and the snapshot {kept_last}, and entries are never removed"
                ),
            });
        }
        if kept_last == *last && kept.and_then(|a| a.head_hash.clone()) != *head {
            return Some(Refusal {
                code: RESTORE_DROPS_ENTRIES,
                message: format!(
                    "the journal of account {key} differs from the snapshot at its last entry"
                ),
            });
        }
    }
    None
}

/// The heads of the journals a state holds: the number of the last entry and the head hash of
/// each account, by the text of its id.
pub fn heads(
    ledger: Option<&LedgerState>,
) -> std::collections::BTreeMap<String, (u64, Option<String>)> {
    ledger
        .into_iter()
        .flat_map(|l| l.accounts.iter())
        .map(|(key, a)| (key.clone(), (a.last_sequence, a.head_hash.clone())))
        .collect()
}

/// Whether the journal head of a restore or an import is older than the last anchor of the audit
/// log (LDG-2924): an anchor is the number and hash of the last entry an account had at a commit.
/// The snapshot must reach that number, and when it holds the entry it must hold the one that was
/// anchored. `anchors` maps the text of an account id to its newest anchor.
pub fn anchor_refusal(
    anchors: &std::collections::BTreeMap<String, (u64, String)>,
    snapshot: &LedgerState,
) -> Option<Refusal> {
    for (key, (sequence, hash)) in anchors {
        let kept = snapshot.accounts.get(key).map_or(0, |a| a.last_sequence);
        if kept < *sequence {
            return Some(Refusal {
                code: HEAD_BEHIND_ANCHOR,
                message: format!(
                    "the journal head of account {key} is entry {kept}, behind the anchored entry {sequence}"
                ),
            });
        }
        let at_anchor = snapshot
            .entries
            .get(key)
            .and_then(|entries| entries.iter().find(|e| e.sequence == *sequence));
        if let Some(entry) = at_anchor {
            if entry.hash.as_deref() != Some(hash.as_str()) {
                return Some(Refusal {
                    code: HEAD_BEHIND_ANCHOR,
                    message: format!(
                        "entry {sequence} of account {key} is not the entry that was anchored"
                    ),
                });
            }
        } else if kept == *sequence
            && snapshot
                .accounts
                .get(key)
                .and_then(|a| a.head_hash.as_deref())
                != Some(hash.as_str())
        {
            return Some(Refusal {
                code: HEAD_BEHIND_ANCHOR,
                message: format!(
                    "the head of account {key} is not the head that was anchored at entry {sequence}"
                ),
            });
        }
    }
    None
}
