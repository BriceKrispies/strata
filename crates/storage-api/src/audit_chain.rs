//! The hash chain of a tenant's audit trail (PRV-359, PRV-360, OPS-329).
//!
//! Each line is stored with a chain value: the hash of the chain value of the line before it and
//! of the line itself in canonical form, as stored. Members that name a person are sealed under
//! the person's subject key before the line is stored ([`crate::sealed`]), so the chain covers the
//! sealed form: destroying the key at an erasure changes no stored byte, and the chain verifies
//! as before. A line that was altered, or a line removed from the middle, breaks the chain at the
//! next line and is found by [`verify_audit_chain`].
//!
//! What the chain does not show: the removal of the newest lines, which leaves a shorter chain
//! that is still whole (the head is not anchored outside the tenant database), and the alteration
//! of a line written before the chain existed, which has no chain value.

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
    /// The sequence number of the first line where the chain breaks, and why.
    pub first_break: Option<(u64, String)>,
    /// No break was found.
    pub ok: bool,
}

/// Check a tenant's trail, read in sequence order from its first line (`Store::audit_chain` from
/// 0): every chained line must follow the one before it by one sequence number and carry the
/// chain value computed from it.
pub fn verify_audit_chain(lines: &[ChainedAuditEntry]) -> AuditChainReport {
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
    report.ok = report.first_break.is_none();
    report
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
        let report = verify_audit_chain(&trail());
        assert!(report.ok, "{report:?}");
        assert_eq!(report.verified, 4);
    }

    #[test]
    fn an_altered_line_breaks_the_chain() {
        let mut lines = trail();
        lines[1].entry.data = "{\"n\":99}".into();
        let report = verify_audit_chain(&lines);
        assert_eq!(report.first_break.map(|(seq, _)| seq), Some(2));
    }

    #[test]
    fn a_removed_line_breaks_the_chain() {
        let mut lines = trail();
        lines.remove(2);
        let report = verify_audit_chain(&lines);
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
        let report = verify_audit_chain(&lines);
        assert!(report.ok, "{report:?}");
        assert_eq!(report.unchained, 1);
    }
}
