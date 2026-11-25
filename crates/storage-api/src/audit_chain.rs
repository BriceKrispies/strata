//! The hash chain of a tenant's audit trail (PRV-359, PRV-360, OPS-329).
//!
//! Each line is stored with a chain value: the hash of the chain value of the line before it and
//! of the line itself in canonical form, as stored. Members that name a person are sealed under
//! the person's subject key before the line is stored ([`crate::sealed`]), so the chain covers the
//! sealed form: destroying the key at an erasure changes no stored byte, and the chain verifies
//! as before. A line that was altered, or a line removed from the middle, breaks the chain at the
//! next line and is found by [`verify_audit_chain`].
//!
//! The removal of the newest lines leaves a shorter chain that is still whole, so the head is
//! anchored outside the tenant database too ([`AuditAnchor`], D-6208, D-8423, OBS-3129): at each
//! anchor interval the store writes the sequence number and chain value of the newest line to the
//! control side, which the tenant's own database does not hold. A chain whose head is older than
//! its last anchor, or whose line at the anchored number is not the one anchored, is refused with
//! `AnchorMismatch`, naming the first sequence number the chain has lost.
//!
//! What the chain does not show: the alteration of a line written before the chain existed, which
//! has no chain value, and the removal of lines written after the last anchor.

use crate::AuditEntry;
use serde::{Deserialize, Serialize};
use strata_kernel::canon;

/// One line of the audit trail as stored, with what places it in the chain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainedAuditEntry {
    /// The line's sequence number in its tenant's trail: increasing by one, never reused.
    pub seq: u64,
    pub entry: AuditEntry,
    /// The chain value stored with the line; `None` for a line written before the chain existed.
    pub chain: Option<String>,
}

/// How often the head of a tenant's chain is anchored, in milliseconds: fifteen minutes (D-6208).
pub const ANCHOR_INTERVAL_MS: i64 = 15 * 60 * 1000;

/// The code of a chain that stops short of its anchor, or whose anchored line is not the one
/// anchored (OBS-3129).
pub const ANCHOR_MISMATCH: &str = "AnchorMismatch";

/// The head of a tenant's chain as it was at an anchor: held outside the tenant database, and
/// only ever moved forward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditAnchor {
    /// The sequence number of the newest chained line at the anchor.
    pub seq: u64,
    /// The chain value of that line.
    pub chain: String,
    /// When the anchor was written, in milliseconds.
    pub at: i64,
}

/// The chain value of `entry` written after a line whose chain value is `previous` (the empty
/// string for the first chained line of a tenant).
pub fn audit_link(previous: &str, entry: &AuditEntry) -> String {
    canon::hash_bytes(
        format!(
            "strata/audit-chain/v1\0{previous}\0{}",
            canon::canonical_json(entry)
        )
        .as_bytes(),
    )
}

/// What a check of the chain found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainReport {
    /// Lines read.
    pub entries: u64,
    /// Lines written before the chain existed, at the start of the trail.
    pub unchained: u64,
    /// Chained lines whose chain value is the one computed from the line before them.
    pub verified: u64,
    /// The sequence number of the first line where the chain breaks, and why. For a chain cut
    /// short of its anchor it is the first sequence number the chain has lost.
    pub first_break: Option<(u64, String)>,
    /// The code of the break when it has one: [`ANCHOR_MISMATCH`].
    pub code: Option<String>,
    /// No break was found.
    pub ok: bool,
}

/// Check a tenant's trail, read in sequence order from its first line (`Store::audit_chain` from
/// 0): every chained line must follow the one before it by one sequence number and carry the
/// chain value computed from it. With the last anchor of the tenant (`anchor`), the chain must also
/// reach it: its newest line is the anchored one or later, and the line at the anchored number is
/// the anchored line.
pub fn verify_audit_chain(
    lines: &[ChainedAuditEntry],
    anchor: Option<&AuditAnchor>,
) -> AuditChainReport {
    let mut report = AuditChainReport {
        entries: lines.len() as u64,
        ..AuditChainReport::default()
    };
    let mut previous: Option<(u64, &str)> = None;
    for line in lines {
        let problem = match (&line.chain, previous) {
            (None, None) => {
                report.unchained += 1;
                None
            }
            (None, Some(_)) => {
                Some("a line without a chain value follows chained lines".to_string())
            }
            (Some(chain), before) => {
                let (expected_seq, prior) = match before {
                    Some((seq, value)) => (Some(seq + 1), value),
                    None => (None, ""),
                };
                if expected_seq.is_some_and(|seq| seq != line.seq) {
                    Some(format!(
                        "lines {} to {} are missing",
                        expected_seq.unwrap_or(line.seq),
                        line.seq.saturating_sub(1)
                    ))
                } else if audit_link(prior, &line.entry) != *chain {
                    Some(
                        "the line does not hash to its chain value: it, or the line before it, was altered or removed"
                            .to_string(),
                    )
                } else {
                    report.verified += 1;
                    previous = Some((line.seq, chain.as_str()));
                    None
                }
            }
        };
        if let Some(problem) = problem {
            report.first_break = Some((line.seq, problem));
            break;
        }
    }
    if report.first_break.is_none() {
        if let Some(anchor) = anchor {
            if let Some((seq, reason)) = anchor_break(lines, anchor) {
                report.first_break = Some((seq, reason));
                report.code = Some(ANCHOR_MISMATCH.to_string());
            }
        }
    }
    report.ok = report.first_break.is_none();
    report
}

/// How `lines`, a chain that is whole, fails to reach `anchor`.
fn anchor_break(lines: &[ChainedAuditEntry], anchor: &AuditAnchor) -> Option<(u64, String)> {
    let newest = lines
        .iter()
        .filter(|line| line.chain.is_some())
        .map(|line| line.seq)
        .max()
        .unwrap_or(0);
    if newest < anchor.seq {
        return Some((
            newest + 1,
            format!(
                "{ANCHOR_MISMATCH}: the chain ends at line {newest}, behind the anchored head at line {}; lines {} to {} are lost",
                anchor.seq,
                newest + 1,
                anchor.seq
            ),
        ));
    }
    match lines.iter().find(|line| line.seq == anchor.seq) {
        Some(line) if line.chain.as_deref() == Some(anchor.chain.as_str()) => None,
        _ => Some((
            anchor.seq,
            format!(
                "{ANCHOR_MISMATCH}: line {} is not the line that was anchored",
                anchor.seq
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: u64, data: &str, previous: &str) -> ChainedAuditEntry {
        let entry = AuditEntry {
            at: seq as i64,
            kind: "audit".into(),
            event: "role.changed".into(),
            correlation: format!("c-{seq}"),
            data: data.into(),
        };
        let chain = audit_link(previous, &entry);
        ChainedAuditEntry {
            seq,
            entry,
            chain: Some(chain),
        }
    }

    fn trail() -> Vec<ChainedAuditEntry> {
        let mut lines = Vec::new();
        let mut previous = String::new();
        for seq in 1..=4 {
            let next = line(seq, &format!("{{\"n\":{seq}}}"), &previous);
            previous = next.chain.clone().unwrap_or_default();
            lines.push(next);
        }
        lines
    }

    #[test]
    fn a_whole_chain_verifies() {
        let report = verify_audit_chain(&trail(), None);
        assert!(report.ok, "{report:?}");
        assert_eq!(report.verified, 4);
    }

    #[test]
    fn an_altered_line_breaks_the_chain() {
        let mut lines = trail();
        lines[1].entry.data = "{\"n\":99}".into();
        let report = verify_audit_chain(&lines, None);
        assert_eq!(report.first_break.map(|(seq, _)| seq), Some(2));
    }

    #[test]
    fn a_removed_line_breaks_the_chain() {
        let mut lines = trail();
        lines.remove(2);
        let report = verify_audit_chain(&lines, None);
        assert_eq!(report.first_break.map(|(seq, _)| seq), Some(4));
    }

    #[test]
    fn lines_before_the_chain_are_counted_and_not_judged() {
        let mut lines = trail();
        lines.insert(
            0,
            ChainedAuditEntry {
                seq: 0,
                entry: lines[0].entry.clone(),
                chain: None,
            },
        );
        let report = verify_audit_chain(&lines, None);
        assert!(report.ok, "{report:?}");
        assert_eq!(report.unchained, 1);
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::*;

    fn lines(n: u64) -> Vec<ChainedAuditEntry> {
        let mut previous = String::new();
        (1..=n)
            .map(|seq| {
                let entry = AuditEntry {
                    at: seq as i64,
                    kind: "audit".into(),
                    event: "session.ended".into(),
                    correlation: format!("c-{seq}"),
                    data: "{}".into(),
                };
                let chain = audit_link(&previous, &entry);
                previous = chain.clone();
                ChainedAuditEntry {
                    seq,
                    entry,
                    chain: Some(chain),
                }
            })
            .collect()
    }

    fn anchor_at(lines: &[ChainedAuditEntry], seq: u64) -> AuditAnchor {
        AuditAnchor {
            seq,
            chain: lines[seq as usize - 1].chain.clone().unwrap_or_default(),
            at: 0,
        }
    }

    #[test]
    fn a_chain_that_stops_short_of_its_anchor_names_the_first_lost_line() {
        let held = lines(5);
        let anchor = anchor_at(&held, 5);
        assert!(verify_audit_chain(&held, Some(&anchor)).ok);
        let report = verify_audit_chain(&held[..3], Some(&anchor));
        assert_eq!(report.code.as_deref(), Some(ANCHOR_MISMATCH));
        assert_eq!(report.first_break.map(|(seq, _)| seq), Some(4));
    }

    #[test]
    fn a_line_other_than_the_anchored_one_is_refused() {
        let mut held = lines(4);
        let anchor = anchor_at(&held, 4);
        // The line is rewritten and its chain value forged to match: the chain is whole, and
        // only the anchor refuses it.
        held[3].entry.data = "{\"changed\":true}".into();
        held[3].chain = Some(audit_link(
            held[2].chain.as_deref().unwrap_or(""),
            &held[3].entry,
        ));
        let report = verify_audit_chain(&held, Some(&anchor));
        assert_eq!(report.code.as_deref(), Some(ANCHOR_MISMATCH), "{report:?}");
    }

    #[test]
    fn reordered_lines_break_the_chain() {
        let mut held = lines(4);
        held.swap(1, 2);
        let report = verify_audit_chain(&held, None);
        assert!(!report.ok);
    }
}
