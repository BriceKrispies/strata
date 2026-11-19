//! Counterfactual projection: run a schedule as it is and again with a proposed change, and
//! say exactly how the two executions differ.
//!
//! The comparison is structural. Outcomes are compared step by step, final states record by
//! record and field by field, query results per observer, events by what they were about.
//! Every finding carries its evidence class; nothing is scored.

use crate::engine::{
    run_with, Cancelled, EventSummary, Injection, Outcome, Scenario, Scheduled, Trace,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::ext::*;
use strata_kernel::rules::Lookup;
use strata_kernel::{
    plan_migration, Check, Context, Coverage, ErrorKind, ExistingData, Intent, Principal,
    ReadRequest, Record, SchemaChange, TenantId, TenantState, Timestamp, Value, World,
};
use strata_verification::evidence::{unmodelled, Class};

/// The proposed change of a projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub tenant: TenantId,
    /// Who proposes it. Must be allowed to change the schema in the branch.
    pub principal: Principal,
    pub changes: Vec<SchemaChange>,
    pub existing: ExistingData,
    /// The change is applied immediately before this step of the schedule.
    pub before_step: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeChange {
    pub step: usize,
    pub at: i64,
    pub label: String,
    pub principal: String,
    pub baseline: String,
    pub branch: String,
    /// The checks that failed on whichever side rejected.
    pub because: Vec<Check>,
    pub class: Class,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    pub baseline: Value,
    pub branch: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordChange {
    pub object: String,
    pub id: u64,
    /// `only_in_baseline`, `only_in_branch` or `modified`.
    pub kind: String,
    pub title: String,
    pub fields: Vec<FieldChange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryChange {
    pub query: String,
    pub observer: String,
    pub baseline: Vec<u64>,
    pub branch: Vec<u64>,
    pub missing_in_branch: Vec<u64>,
    pub new_in_branch: Vec<u64>,
    pub reordered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantChange {
    pub id: String,
    pub statement: String,
    pub baseline_holds: bool,
    pub branch_holds: bool,
    pub branch_violations: Vec<String>,
    pub branch_violation_count: usize,
}

/// An existing record that would not conform to the proposed schema.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Affected {
    pub object: String,
    pub id: u64,
    pub title: String,
    pub rule: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub class: Class,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub identical: bool,
    /// The first schedule step whose outcome differs.
    pub first_divergence: Option<usize>,
    pub outcome_changes: Vec<OutcomeChange>,
    /// Failed rule to how many more times it failed in the branch than in the baseline.
    pub new_failures: BTreeMap<String, u64>,
    pub record_changes: Vec<RecordChange>,
    pub query_changes: Vec<QueryChange>,
    pub queries_compared: usize,
    pub events_only_in_baseline: Vec<EventSummary>,
    pub events_only_in_branch: Vec<EventSummary>,
    pub other_tenants_untouched: bool,
    pub invariants: Vec<InvariantChange>,
}

/// The smallest schedule found that still shows the first divergence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterexample {
    /// Original step numbers kept, in order. The last one is the step that diverges.
    pub steps: Vec<usize>,
    pub labels: Vec<String>,
    pub baseline: String,
    pub branch: String,
    pub because: Vec<Check>,
    pub runs_tried: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    pub proposal: Proposal,
    /// How the kernel answered the proposed change itself, in the branch.
    pub change_outcome: Outcome,
    /// Existing records that do not conform to the proposed schema, found by the kernel's
    /// migration planning on the snapshot.
    pub affected_existing: Vec<Affected>,
    pub baseline: Trace,
    pub branch: Trace,
    pub baseline_hash: String,
    pub branch_hash: String,
    pub comparison: Comparison,
    pub counterexample: Option<Counterexample>,
    pub findings: Vec<Finding>,
    pub unmodelled: Vec<String>,
}

fn principal_name(principal: &Principal) -> String {
    match principal {
        Principal::User(id) => id.as_str().to_string(),
        Principal::Anonymous => "an anonymous visitor".to_string(),
        Principal::System => "the system".to_string(),
    }
}

/// A human name for a record: its `title` or `name` field if it has one.
fn title_of(record: &Record) -> String {
    ["title", "name", "slug"]
        .iter()
        .filter_map(|name| strata_kernel::FieldId::new(*name).ok())
        .find_map(|field| record.get(&field).as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{} {}", record.object, record.id))
}

fn failed_checks(outcome: &Outcome) -> Vec<Check> {
    match outcome {
        Outcome::Rejected { failed, .. } => failed.clone(),
        Outcome::Accepted { .. } => Vec::new(),
    }
}

fn diff_records(tenant: &TenantId, baseline: &World, branch: &World) -> Vec<RecordChange> {
    let empty = BTreeMap::new();
    let records = |world: &World| -> BTreeMap<(String, u64), std::sync::Arc<Record>> {
        world
            .tenants
            .get(tenant)
            .map_or(&empty, |t| &t.records)
            .iter()
            .flat_map(|(object, records)| {
                records
                    .values()
                    .map(move |r| ((object.to_string(), r.id.0), r.clone()))
            })
            .collect()
    };
    let (a, b) = (records(baseline), records(branch));
    let keys: BTreeSet<&(String, u64)> = a.keys().chain(b.keys()).collect();
    let mut changes = Vec::new();
    for key in keys {
        let change = match (a.get(key), b.get(key)) {
            (Some(left), Some(right)) => {
                let fields: BTreeSet<_> = left.fields.keys().chain(right.fields.keys()).collect();
                let differing: Vec<FieldChange> = fields
                    .into_iter()
                    .filter(|field| left.get(field) != right.get(field))
                    .map(|field| FieldChange {
                        field: field.as_str().to_string(),
                        baseline: left.get(field).clone(),
                        branch: right.get(field).clone(),
                    })
                    .collect();
                if differing.is_empty() {
                    continue;
                }
                RecordChange {
                    object: key.0.clone(),
                    id: key.1,
                    kind: "modified".into(),
                    title: title_of(right),
                    fields: differing,
                }
            }
            (Some(left), None) => RecordChange {
                object: key.0.clone(),
                id: key.1,
                kind: "only_in_baseline".into(),
                title: title_of(left),
                fields: vec![],
            },
            (None, Some(right)) => RecordChange {
                object: key.0.clone(),
                id: key.1,
                kind: "only_in_branch".into(),
                title: title_of(right),
                fields: vec![],
            },
            (None, None) => continue,
        };
        changes.push(change);
    }
    changes
}

/// Run every parameterless named query as every kind of observer on both final worlds.
fn diff_queries(
    tenant: &TenantId,
    baseline: &World,
    branch: &World,
    at: Timestamp,
) -> (Vec<QueryChange>, usize) {
    let Some(state) = baseline.tenants.get(tenant) else {
        return (Vec::new(), 0);
    };
    let mut observers = vec![Principal::Anonymous];
    observers.extend(state.members.keys().cloned().map(Principal::User));
    let mut changes = Vec::new();
    let mut compared = 0;
    for (id, query) in state
        .schema
        .queries
        .iter()
        .filter(|(_, q)| q.params.is_empty())
    {
        // A query the branch's schema no longer has, or has changed, is a schema difference, not
        // a difference in results; only queries present on both sides are compared.
        if branch
            .tenants
            .get(tenant)
            .is_none_or(|t| !t.schema.queries.contains_key(id))
        {
            continue;
        }
        for observer in &observers {
            let ctx = Context {
                tenant: tenant.clone(),
                principal: observer.clone(),
                session: None,
                now: at,
                correlation: "compare".into(),
                verified_address: None,
            };
            let request = ReadRequest::Named {
                query: query.id.clone(),
                params: BTreeMap::new(),
            };
            let ids = |world: &World| -> Vec<u64> {
                world
                    .read(&request, &ctx)
                    .map(|result| result.rows.iter().map(|r| r.id.0).collect())
                    .unwrap_or_default()
            };
            let (a, b) = (ids(baseline), ids(branch));
            compared += 1;
            if a == b {
                continue;
            }
            let (set_a, set_b): (BTreeSet<u64>, BTreeSet<u64>) =
                (a.iter().copied().collect(), b.iter().copied().collect());
            changes.push(QueryChange {
                query: id.as_str().to_string(),
                observer: principal_name(observer),
                missing_in_branch: set_a.difference(&set_b).copied().collect(),
                new_in_branch: set_b.difference(&set_a).copied().collect(),
                reordered: set_a == set_b,
                baseline: a,
                branch: b,
            });
        }
    }
    (changes, compared)
}

fn events_of(trace: &Trace) -> Vec<EventSummary> {
    trace
        .steps
        .iter()
        .filter(|step| step.index.is_some())
        .flat_map(|step| match &step.outcome {
            Outcome::Accepted { events, .. } => events.clone(),
            Outcome::Rejected { .. } => Vec::new(),
        })
        .collect()
}

/// Compare two executions of the same schedule.
pub fn compare(
    tenant: &TenantId,
    baseline: (&Trace, &World),
    branch: (&Trace, &World),
) -> Comparison {
    let by_index = |trace: &Trace| -> BTreeMap<usize, crate::engine::Step> {
        trace
            .steps
            .iter()
            .filter_map(|step| step.index.map(|i| (i, step.clone())))
            .collect()
    };
    let (a, b) = (by_index(baseline.0), by_index(branch.0));
    let mut outcome_changes = Vec::new();
    let mut new_failures: BTreeMap<String, i64> = BTreeMap::new();
    for (index, left) in &a {
        let Some(right) = b.get(index) else {
            continue;
        };
        for check in failed_checks(&left.outcome) {
            *new_failures.entry(check.rule).or_default() -= 1;
        }
        for check in failed_checks(&right.outcome) {
            *new_failures.entry(check.rule).or_default() += 1;
        }
        let same = match (&left.outcome, &right.outcome) {
            (Outcome::Accepted { output: x, .. }, Outcome::Accepted { output: y, .. }) => x == y,
            (Outcome::Rejected { kind: x, .. }, Outcome::Rejected { kind: y, .. }) => x == y,
            _ => false,
        };
        if !same {
            let rejecting = if right.outcome.accepted() {
                &left.outcome
            } else {
                &right.outcome
            };
            outcome_changes.push(OutcomeChange {
                step: *index,
                at: left.at,
                label: left.label.clone(),
                principal: principal_name(&left.context.principal),
                baseline: left.outcome.verdict(),
                branch: right.outcome.verdict(),
                because: failed_checks(rejecting),
                class: Class::Reproduced,
            });
        }
    }
    let (events_a, events_b) = (events_of(baseline.0), events_of(branch.0));
    // Events are matched by what they were about; sequence numbers shift once the branch has an
    // extra schema event or loses a write.
    let key = |e: &EventSummary| (e.kind.clone(), e.object.clone(), e.id);
    let mut counts: BTreeMap<_, i64> = BTreeMap::new();
    events_a
        .iter()
        .for_each(|e| *counts.entry(key(e)).or_default() += 1);
    events_b
        .iter()
        .for_each(|e| *counts.entry(key(e)).or_default() -= 1);
    let surplus = |events: &[EventSummary], sign: i64| -> Vec<EventSummary> {
        let mut remaining = counts.clone();
        events
            .iter()
            .filter(|e| {
                let slot = remaining.entry(key(e)).or_default();
                if *slot * sign > 0 {
                    *slot -= sign;
                    true
                } else {
                    false
                }
            })
            .cloned()
            .collect()
    };
    let end = Timestamp(baseline.0.steps.last().map_or(0, |s| s.at) + 1);
    let (query_changes, queries_compared) = diff_queries(tenant, baseline.1, branch.1, end);
    let record_changes = diff_records(tenant, baseline.1, branch.1);
    let other_tenants_untouched = baseline
        .1
        .tenants
        .keys()
        .chain(branch.1.tenants.keys())
        .filter(|id| *id != tenant)
        .all(|id| baseline.1.tenant_hash(id) == branch.1.tenant_hash(id));
    let invariants = baseline
        .0
        .invariants
        .iter()
        .map(|left| {
            let right = branch.0.invariants.iter().find(|i| i.id == left.id);
            InvariantChange {
                id: left.id.clone(),
                statement: left.statement.clone(),
                baseline_holds: left.holds,
                branch_holds: right.is_some_and(|r| r.holds),
                branch_violations: right.map(|r| r.violations.clone()).unwrap_or_default(),
                branch_violation_count: right.map_or(0, |r| r.violation_count),
            }
        })
        .collect::<Vec<_>>();
    let events_only_in_baseline = surplus(&events_a, 1);
    let events_only_in_branch = surplus(&events_b, -1);
    Comparison {
        identical: outcome_changes.is_empty()
            && record_changes.is_empty()
            && query_changes.is_empty()
            && events_only_in_baseline.is_empty()
            && events_only_in_branch.is_empty()
            && baseline.1.hash() == branch.1.hash(),
        first_divergence: outcome_changes.first().map(|change| change.step),
        outcome_changes,
        new_failures: new_failures
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(rule, n)| (rule, n as u64))
            .collect(),
        record_changes,
        query_changes,
        queries_compared,
        events_only_in_baseline,
        events_only_in_branch,
        other_tenants_untouched,
        invariants,
    }
}

fn injection(scenario: &Scenario, proposal: &Proposal) -> Injection {
    let at = scenario
        .schedule
        .get(proposal.before_step)
        .map(|s| s.context.now.0)
        .or_else(|| scenario.schedule.last().map(|s| s.context.now.0 + 1))
        .unwrap_or(0);
    Injection {
        before: proposal.before_step,
        scheduled: Scheduled {
            label: "apply the proposed change".into(),
            context: Context {
                tenant: proposal.tenant.clone(),
                principal: proposal.principal.clone(),
                session: None,
                now: Timestamp(at),
                correlation: "sim-proposal".into(),
                verified_address: None,
            },
            intent: Intent::ChangeSchema {
                changes: proposal.changes.clone(),
                expected_version: None,
                existing: proposal.existing,
            },
        },
    }
}

/// Records of the snapshot that would not conform to the proposed schema. Exact for the
/// snapshot: it is the kernel's own migration planning.
pub fn affected_existing(
    tenant: &TenantState,
    proposal: &Proposal,
    now: Timestamp,
) -> Vec<Affected> {
    let lookup = Lookup {
        tenant,
        coverage: &Coverage::Complete,
    };
    let Ok(migration) = plan_migration(lookup, &proposal.changes, now) else {
        return Vec::new();
    };
    migration
        .violations
        .into_iter()
        .map(|violation| Affected {
            title: tenant
                .record(&violation.object, violation.id)
                .map_or_else(String::new, |r| title_of(r)),
            object: violation.object.as_str().to_string(),
            id: violation.id.0,
            rule: violation.check.rule,
            detail: violation.check.detail,
        })
        .collect()
}

/// Find a small sub-schedule that still makes the diverging step diverge the same way.
///
/// Greedy one-at-a-time removal: sound (what it returns does diverge) but not guaranteed to be
/// the smallest possible.
fn minimise(
    scenario: &Scenario,
    proposal: &Proposal,
    diverging: usize,
    expected: (&str, &str),
) -> Option<Counterexample> {
    let shows = |kept: &[usize]| -> Option<(String, String, Vec<Check>)> {
        let position = kept.iter().position(|i| *i == diverging)?;
        let sub = Scenario {
            schedule: kept.iter().map(|i| scenario.schedule[*i].clone()).collect(),
            faults: Default::default(),
            ..scenario.clone()
        };
        let before = kept.iter().filter(|i| **i < proposal.before_step).count();
        let sub_proposal = Proposal {
            before_step: before,
            ..proposal.clone()
        };
        let (base, _) = run_with(&sub, None, &mut |_, _| true).ok()?;
        let (branch, _) = run_with(&sub, Some(&injection(&sub, &sub_proposal)), &mut |_, _| {
            true
        })
        .ok()?;
        let find = |trace: &Trace| {
            trace
                .steps
                .iter()
                .find(|s| s.index == Some(position))
                .map(|s| s.outcome.clone())
        };
        let (a, b) = (find(&base)?, find(&branch)?);
        (a.verdict() == expected.0 && b.verdict() == expected.1).then(|| {
            let because = failed_checks(if b.accepted() { &a } else { &b });
            (a.verdict(), b.verdict(), because)
        })
    };
    let mut kept: Vec<usize> = (0..=diverging).collect();
    let mut runs = 1;
    shows(&kept)?;
    let mut position = 0;
    while position < kept.len() {
        if kept[position] == diverging {
            position += 1;
            continue;
        }
        let mut trial = kept.clone();
        trial.remove(position);
        runs += 1;
        if shows(&trial).is_some() {
            kept = trial;
        } else {
            position += 1;
        }
    }
    let (baseline, branch, because) = shows(&kept)?;
    Some(Counterexample {
        labels: kept
            .iter()
            .map(|i| scenario.schedule[*i].label.clone())
            .collect(),
        steps: kept,
        baseline,
        branch,
        because,
        runs_tried: runs + 1,
    })
}

fn findings(projection: &Projection) -> Vec<Finding> {
    let c = &projection.comparison;
    let n = projection.baseline.steps.len();
    let mut out = Vec::new();
    let both_hold: Vec<&str> = c
        .invariants
        .iter()
        .filter(|i| i.baseline_holds && i.branch_holds)
        .map(|i| i.id.as_str())
        .collect();
    out.push(Finding {
        class: Class::Proven,
        title: "Tenant isolation, authorization and determinism of a step".into(),
        detail: "For the abstract model in lean/Strata these hold for every state and every intent, so they hold for this change too. See the verification report for the theorems and their assumptions.".into(),
    });
    match &projection.change_outcome {
        Outcome::Accepted { warnings, .. } => out.push(Finding {
            class: Class::Reproduced,
            title: "The kernel accepts the proposed change".into(),
            detail: if warnings.is_empty() {
                "Every existing record already conforms to the proposed schema.".into()
            } else {
                format!("It is accepted with {} failed check(s) on existing records, which stay as they are until they are next written.", warnings.len())
            },
        }),
        Outcome::Rejected { message, .. } => out.push(Finding {
            class: Class::Reproduced,
            title: "The kernel rejects the proposed change".into(),
            detail: format!("{message}. The branch therefore behaves exactly like the baseline."),
        }),
    }
    out.push(Finding {
        class: Class::Reproduced,
        title: if c.identical {
            format!("All {n} scheduled operations behave identically with and without the change")
        } else {
            format!("{} of {n} scheduled operations end differently", c.outcome_changes.len())
        },
        detail: format!(
            "Baseline trace {} and branch trace {}. Running the projection again gives the same two hashes. Only the first divergence is certainly caused by the change itself: later operations name records by id, and a refused create shifts every id after it, so some later differences are knock-on effects.",
            &projection.baseline_hash[..12],
            &projection.branch_hash[..12]
        ),
    });
    if !projection.affected_existing.is_empty() {
        let records: BTreeSet<(&str, u64)> = projection
            .affected_existing
            .iter()
            .map(|a| (a.object.as_str(), a.id))
            .collect();
        out.push(Finding {
            class: Class::SoundApproximation,
            title: format!("{} existing record(s) could be refused on their next write", records.len()),
            detail: "These records do not conform to the proposed schema. Each write to one of them will be rejected unless the same write also brings it into line. Records that are never written again are never affected, so this is an upper bound.".into(),
        });
    }
    out.push(Finding {
        class: Class::Empirical,
        title: format!(
            "{} of {n} operations accepted in the baseline, {} with the change",
            projection.baseline.accepted(),
            projection.branch.accepted()
        ),
        detail: "Counted over this schedule only. A generated schedule is a sample drawn from a seed; a recorded one is what happened once. Neither is a forecast of future traffic.".into(),
    });
    out.push(Finding {
        class: Class::Reproduced,
        title: format!("{} invariant(s) hold in both executions", both_hold.len()),
        detail: if c.invariants.iter().any(|i| i.baseline_holds && !i.branch_holds) {
            "At least one invariant that holds in the baseline fails with the change; see the invariant table.".into()
        } else {
            "No invariant that holds in the baseline fails with the change.".into()
        },
    });
    out.push(Finding {
        class: Class::Unknown,
        title: "Performance, concurrency and everything outside the kernel".into(),
        detail: "The simulator executes domain logic only. It makes no claim about latency, load, interleavings it was not given, or the behaviour of browsers and sessions.".into(),
    });
    out
}

/// Run the baseline and the branch and compare them.
///
/// `progress` receives `(phase, done, total)` with phase 0 for the baseline, 1 for the branch
/// and 2 for minimisation; returning `false` cancels.
pub fn project(
    scenario: &Scenario,
    proposal: &Proposal,
    progress: &mut dyn FnMut(u8, usize, usize) -> bool,
) -> Result<Projection, Cancelled> {
    let (baseline, baseline_world) =
        run_with(scenario, None, &mut |done, total| progress(0, done, total))?;
    let inject = injection(scenario, proposal);
    let (branch, branch_world) = run_with(scenario, Some(&inject), &mut |done, total| {
        progress(1, done, total)
    })?;
    let change_outcome = branch
        .steps
        .iter()
        .find(|step| step.index.is_none())
        .map(|step| step.outcome.clone())
        .unwrap_or(Outcome::Rejected {
            kind: ErrorKind::InvalidIntent,
            message: "the change was never applied".into(),
            failed: vec![],
        });
    let comparison = compare(
        &proposal.tenant,
        (&baseline, &baseline_world),
        (&branch, &branch_world),
    );
    if !progress(2, 0, 1) {
        return Err(Cancelled);
    }
    let counterexample = comparison.outcome_changes.first().and_then(|first| {
        minimise(
            scenario,
            proposal,
            first.step,
            (&first.baseline, &first.branch),
        )
    });
    progress(2, 1, 1);
    let affected = scenario
        .initial
        .tenants
        .get(&proposal.tenant)
        .map(|tenant| affected_existing(tenant, proposal, inject.scheduled.context.now))
        .unwrap_or_default();
    let mut projection = Projection {
        proposal: proposal.clone(),
        change_outcome,
        affected_existing: affected,
        baseline_hash: baseline.hash(),
        branch_hash: branch.hash(),
        baseline,
        branch,
        comparison,
        counterexample,
        findings: Vec::new(),
        unmodelled: unmodelled().into_iter().map(str::to_string).collect(),
    };
    projection.findings = findings(&projection);
    Ok(projection)
}
