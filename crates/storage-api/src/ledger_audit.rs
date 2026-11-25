//! The audit line of a ledger commit and the anchors it carries (LDG-2923, LDG-2925, LDG-2926,
//! D-8431).
//!
//! [`Commit::of`](crate::Commit::of) derives one audit line for each transaction a decision
//! posts to, and one for each entry that is not part of a transaction. The line names the
//! transaction, its header (the actor, the principal acted for, the operation, the idempotency
//! key, the correlation, the reason and the time), the entries and the head hash each account
//! had at the end of the transaction. A store writes the lines in the transaction of the commit,
//! so a commit that cannot append its audit line changes nothing (LDG-2926).
//!
//! The head of each account is an anchor: [`anchors`] reads the newest one of each account from
//! the audit log, which a restore or an import is compared with (LDG-2924).

use crate::AuditEntry;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use strata_kernel::{
    Context, DeltaOp, Intent, IntentExt, LedgerDelta, LedgerEntry, Principal, StateDelta,
};

/// The audit `kind` of a ledger line.
pub const KIND: &str = "audit";
/// The `event` of a posting.
pub const POSTING: &str = "ledger.posting";
/// The `event` of a reversal.
pub const REVERSAL: &str = "ledger.reversal";
/// The `event` of the expiry of credit.
pub const EXPIRY: &str = "ledger.expiry";
/// The `event` of a verification of a journal (LDG-2928).
pub const VERIFICATION: &str = "ledger.verification";

/// The header of a transaction as the intent that made it states it: the operation, the
/// idempotency key and the principal acted for.
struct Header {
    operation: String,
    key: Option<String>,
    acted_for: Option<String>,
}

fn header_of(intent: &Intent, ctx: &Context) -> Header {
    match intent {
        Intent::PostLedgerTransaction { transaction } => Header {
            operation: transaction.operation.clone(),
            key: Some(transaction.idempotency_key.clone()),
            acted_for: transaction.on_behalf_of.as_ref().map(|p| p.to_string()),
        },
        Intent::InvokeOperation {
            operation,
            idempotency_key,
            ..
        } => Header {
            operation: format!("operation:{operation}"),
            key: Some(idempotency_key.clone()),
            acted_for: None,
        },
        other => Header {
            operation: other.name().to_string(),
            key: ctx.idempotency_key.clone(),
            acted_for: None,
        },
    }
}

/// The audit lines of the ledger entries a delta appends, in the order of their first entry.
pub fn lines(intent: &Intent, ctx: &Context, delta: &StateDelta) -> Vec<AuditEntry> {
    let entries: Vec<&LedgerEntry> = delta
        .ops
        .iter()
        .filter_map(|op| match op {
            DeltaOp::Ledger(LedgerDelta::AppendEntry(entry)) => Some(entry),
            _ => None,
        })
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    // One group for each transaction, one for each entry that names none.
    let mut groups: Vec<(Option<&str>, Vec<&LedgerEntry>)> = Vec::new();
    for entry in entries {
        let transaction = entry.transaction.as_deref();
        match groups
            .iter_mut()
            .find(|(t, _)| transaction.is_some() && *t == transaction)
        {
            Some((_, members)) => members.push(entry),
            None => groups.push((transaction, vec![entry])),
        }
    }
    let header = header_of(intent, ctx);
    let actor = match &ctx.principal {
        Principal::User(id) => Some(id.to_string()),
        _ => None,
    };
    groups
        .into_iter()
        .map(|(transaction, members)| {
            let event = if members.iter().all(|e| e.corrects.is_some()) {
                REVERSAL
            } else if members.iter().all(|e| e.reason == "expiry") {
                EXPIRY
            } else {
                POSTING
            };
            let mut heads: BTreeMap<String, (u64, String)> = BTreeMap::new();
            for entry in &members {
                if let Some(hash) = &entry.hash {
                    heads.insert(entry.account.to_string(), (entry.sequence, hash.clone()));
                }
            }
            let mut data = json!({
                "transaction": transaction,
                "operation": header.operation,
                "idempotency_key": header.key,
                "correlation": ctx.correlation,
                "reason": members[0].reason,
                "at": ctx.now.0,
                "revision": delta.new_revision.0,
                "entries": members.iter().map(|e| json!({
                    "account": e.account.to_string(),
                    "sequence": e.sequence,
                    "amount": e.amount.0,
                    "hash": e.hash,
                })).collect::<Vec<_>>(),
                "anchors": heads.iter().map(|(account, (sequence, hash))| json!({
                    "account": account,
                    "sequence": sequence,
                    "hash": hash,
                })).collect::<Vec<_>>(),
            });
            // The members that name a person are the ones a store seals (PRV).
            if let Some(object) = data.as_object_mut() {
                if let Some(actor) = &actor {
                    object.insert("actor".into(), Value::String(actor.clone()));
                }
                if let Some(acted_for) = &header.acted_for {
                    object.insert("principal".into(), Value::String(acted_for.clone()));
                }
            }
            AuditEntry {
                at: ctx.now.0,
                kind: KIND.to_string(),
                event: event.to_string(),
                correlation: ctx.correlation.clone(),
                data: data.to_string(),
            }
        })
        .collect()
}

/// The newest anchor of each account the lines name: the number and hash of its last entry at the
/// newest commit that changed it. `lines` are in the order they were written.
pub fn anchors<'a>(
    lines: impl IntoIterator<Item = &'a AuditEntry>,
) -> BTreeMap<String, (u64, String)> {
    let mut found: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for line in lines {
        if line.kind != KIND || !line.event.starts_with("ledger.") {
            continue;
        }
        let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&line.data) else {
            continue;
        };
        let Some(Value::Array(list)) = data.get("anchors") else {
            continue;
        };
        for anchor in list {
            let (Some(account), Some(sequence), Some(hash)) = (
                anchor.get("account").and_then(Value::as_str),
                anchor.get("sequence").and_then(Value::as_u64),
                anchor.get("hash").and_then(Value::as_str),
            ) else {
                continue;
            };
            let newer = found
                .get(account)
                .map_or(true, |(held, _)| sequence >= *held);
            if newer {
                found.insert(account.to_string(), (sequence, hash.to_string()));
            }
        }
    }
    found
}

/// The line that records a verification of a journal and its result (LDG-2928).
pub fn verification_line(
    ctx: &Context,
    scope: &str,
    accounts: u64,
    entries: u64,
    first_break: Option<(&str, u64, &str)>,
) -> AuditEntry {
    let mut data = json!({
        "scope": scope,
        "accounts": accounts,
        "entries": entries,
        "ok": first_break.is_none(),
        "correlation": ctx.correlation,
        "at": ctx.now.0,
    });
    if let (Some(object), Some((account, sequence, code))) = (data.as_object_mut(), first_break) {
        object.insert(
            "first_break".into(),
            json!({ "account": account, "sequence": sequence, "code": code }),
        );
    }
    if let (Some(object), Principal::User(id)) = (data.as_object_mut(), &ctx.principal) {
        object.insert("actor".into(), Value::String(id.to_string()));
    }
    AuditEntry {
        at: ctx.now.0,
        kind: KIND.to_string(),
        event: VERIFICATION.to_string(),
        correlation: ctx.correlation.clone(),
        data: data.to_string(),
    }
}

/// Whether a snapshot may be restored over the journal a tenant holds (LDG-2911, LDG-2924).
///
/// `heads` are the heads of the journals now held ([`strata_kernel::ledger_chain::heads`]) and
/// `audit` the lines of the audit log, which a restore never rolls back. The snapshot is refused
/// with `restore-drops-entries` when it lacks an entry the journal holds, and with
/// `head-behind-anchor` when its head is older than the last anchor. Each refusal is
/// [`StoreError::Invalid`] whose message begins with the code and changes nothing.
pub fn check_restore<'a>(
    heads: &BTreeMap<String, (u64, Option<String>)>,
    audit: impl IntoIterator<Item = &'a AuditEntry>,
    snapshot: Option<&strata_kernel::LedgerState>,
) -> crate::StoreResult<()> {
    let empty = strata_kernel::ledger::empty_state();
    let snapshot = snapshot.unwrap_or(&empty);
    let refusal = strata_kernel::ledger_chain::restore_refusal(heads, snapshot)
        .or_else(|| strata_kernel::ledger_chain::anchor_refusal(&anchors(audit), snapshot));
    match refusal {
        Some(refusal) => Err(crate::StoreError::Invalid(format!(
            "{}: {}",
            refusal.code, refusal.message
        ))),
        None => Ok(()),
    }
}
