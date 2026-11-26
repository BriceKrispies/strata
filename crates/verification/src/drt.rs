//! Differential random testing at scale, in the pattern Cedar uses between its Lean model and its
//! Rust implementation.
//!
//! [`run`] draws scenarios from [`ScenarioStream`] (by default the full generator, which emits every
//! intent and value form of the scenario format; the fixture generator is the other choice) and
//! writes them a batch at a time. Each batch goes to the Lean model in one `lake exe` process
//! and through the Rust kernel in process, and the two are compared scenario by scenario. A
//! scenario starts from the model's initial world, so a disagreement belongs to one scenario.
//!
//! The first disagreement is minimised by [`minimise`] (delta debugging over whole scenarios, then
//! over the operations of each) and written as a hand-written scenario named `h-drt-<seed>-<n>`,
//! ready to append to `lean/fixtures/scenarios.txt`. Each candidate the minimiser tries is one more
//! Lean process, and that happens only once there is a disagreement to shrink.

use crate::lean::{
    build_model, compare_outputs, kernel_output, op_outcomes, read_shapes, run_model, Generator,
    ScenarioStream,
};
use crate::lean_diff::lean_error_names;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// One scenario: its name, and its operation lines as they appear between `scenario` and `end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub ops: Vec<String>,
}

/// The scenarios of a scenario file. Only `scenario`, `op` and `read` lines are read: the `end` lines are
/// implied by the next `scenario` line or the end of the file, and anything else is dropped.
pub fn parse_cases(text: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("scenario ") {
            cases.push(Case {
                name: name.to_string(),
                ops: Vec::new(),
            });
        } else if line.starts_with("op ") || line.starts_with("read ") {
            if let Some(case) = cases.last_mut() {
                case.ops.push(line.to_string());
            }
        }
    }
    cases
}

/// The scenario file for `cases`: each one as `scenario <name>`, its operations, then `end`.
pub fn render_cases(cases: &[Case]) -> String {
    let mut out = String::new();
    for case in cases {
        out.push_str(&format!("scenario {}\n", case.name));
        for op in &case.ops {
            out.push_str(op);
            out.push('\n');
        }
        out.push_str("end\n");
    }
    out
}

fn operations(cases: &[Case]) -> usize {
    cases.iter().map(|c| c.ops.len()).sum()
}

/// Delta debugging (Zeller's `ddmin`) on a list: the smallest sublist, in its original order, that
/// `still_fails` still accepts, found by removing chunks of halving size. `still_fails` must hold
/// for `items`; when it does not, `items` comes back unchanged. An `Err` from `still_fails` stops
/// the search and is returned.
pub fn ddmin<T: Clone, E>(
    items: &[T],
    still_fails: &mut impl FnMut(&[T]) -> Result<bool, E>,
) -> Result<Vec<T>, E> {
    if !still_fails(items)? {
        return Ok(items.to_vec());
    }
    let mut current = items.to_vec();
    let mut granularity = 2;
    while current.len() >= 2 {
        let size = current.len().div_ceil(granularity);
        let chunks: Vec<Range<usize>> = (0..current.len())
            .step_by(size)
            .map(|start| start..(start + size).min(current.len()))
            .collect();
        // First, each chunk on its own: a chunk that still fails restarts the search from it.
        let mut next = None;
        for chunk in &chunks {
            let candidate = current[chunk.clone()].to_vec();
            if still_fails(&candidate)? {
                next = Some((candidate, 2));
                break;
            }
        }
        // Then, each complement: removing one chunk and keeping the rest.
        if next.is_none() {
            for chunk in &chunks {
                let candidate: Vec<T> = current[..chunk.start]
                    .iter()
                    .chain(&current[chunk.end..])
                    .cloned()
                    .collect();
                if still_fails(&candidate)? {
                    next = Some((candidate, (granularity - 1).max(2)));
                    break;
                }
            }
        }
        match next {
            Some((candidate, next_granularity)) => {
                current = candidate;
                granularity = next_granularity;
            }
            None if granularity >= current.len() => break,
            None => granularity = (granularity * 2).min(current.len()),
        }
    }
    Ok(current)
}

/// Shrink a failing list of scenarios: first whole scenarios (`ddmin` over the list), then the
/// operations of each scenario, repeated until a round removes nothing. `still_fails` must hold for
/// `cases`. On return it still holds, and removing any one scenario, or any one operation of a
/// scenario that has two or more, makes it stop holding.
pub fn minimise<E>(
    cases: &[Case],
    still_fails: &mut impl FnMut(&[Case]) -> Result<bool, E>,
) -> Result<Vec<Case>, E> {
    let mut current = cases.to_vec();
    loop {
        let before = (current.len(), operations(&current));
        current = ddmin(&current, &mut *still_fails)?;
        for i in 0..current.len() {
            let base = current.clone();
            let ops = ddmin(&base[i].ops, &mut |ops: &[String]| {
                let mut candidate = base.clone();
                candidate[i].ops = ops.to_vec();
                still_fails(&candidate)
            })?;
            current[i].ops = ops;
        }
        if (current.len(), operations(&current)) == before {
            return Ok(current);
        }
    }
}

/// The output of each scenario, from its `scenario` line up to the next one.
fn outputs_by_scenario(output: &str) -> Vec<Vec<&str>> {
    let mut parts: Vec<Vec<&str>> = Vec::new();
    for line in output.lines() {
        if line.starts_with("scenario ") {
            parts.push(Vec::new());
        }
        if let Some(part) = parts.last_mut() {
            part.push(line);
        }
    }
    parts
}

/// The first line on which the model and the kernel print different text for `cases`, as
/// `(line, lean, kernel)`, or `None` when they agree. Runs one Lean process.
fn check(
    lean_dir: &Path,
    scratch: &Path,
    cases: &[Case],
) -> Result<Option<(usize, String, String)>, String> {
    let text = render_cases(cases);
    std::fs::write(scratch, &text)
        .map_err(|e| format!("cannot write {}: {e}", scratch.display()))?;
    let lean = run_model(lean_dir, &scratch.to_string_lossy())?;
    Ok(compare_outputs(&text, &lean, &kernel_output(&text)).first_difference)
}

/// A disagreement, minimised and written out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Minimised {
    /// Index of the scenario in the run (counted from 0 across all batches).
    pub scenario: usize,
    /// The hand-written scenario, named `h-drt-<seed>-<scenario>`, as a scenario file.
    pub path: PathBuf,
    pub operations_before: usize,
    pub operations_after: usize,
    /// Whether the scenario, run on its own, still disagrees. When it does not, nothing is shrunk:
    /// the file holds the scenario as the batch ran it.
    pub reproduced: bool,
    /// The first difference of the written scenario, when run on its own.
    pub first_difference: Option<(usize, String, String)>,
}

fn minimise_disagreement(
    lean_dir: &Path,
    out_dir: &Path,
    scratch: &Path,
    seed: u64,
    index: usize,
    case: &Case,
) -> Result<Minimised, String> {
    let original = Case {
        name: format!("h-drt-{seed}-{index}"),
        ops: case.ops.clone(),
    };
    let reproduced = check(lean_dir, scratch, std::slice::from_ref(&original))?.is_some();
    let shrunk = if reproduced {
        let mut still_differs =
            |cases: &[Case]| check(lean_dir, scratch, cases).map(|d| d.is_some());
        minimise(std::slice::from_ref(&original), &mut still_differs)?
    } else {
        vec![original.clone()]
    };
    let first_difference = check(lean_dir, scratch, &shrunk)?;
    let path = out_dir.join(format!("h-drt-{seed}-{index}.txt"));
    std::fs::write(&path, render_cases(&shrunk))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(Minimised {
        scenario: index,
        path,
        operations_before: original.ops.len(),
        operations_after: operations(&shrunk),
        reproduced,
        first_difference,
    })
}

/// The accepted outcome kinds, one per intent kind of the scenario format: the intent's name
/// followed by `ok`. `require` and `unique` are split by their mode, as the model's strict and lax
/// schema outcomes differ.
pub const ACCEPTED_KINDS: [&str; 23] = [
    "create ok",
    "update ok",
    "patch ok",
    "delete ok",
    "require-strict ok",
    "require-lax ok",
    "unique-strict ok",
    "unique-lax ok",
    "composite ok",
    "schema ok",
    "create-tenant ok",
    "assign ok",
    "revoke ok",
    "team ok",
    "team-assign ok",
    "team-revoke ok",
    "team-add ok",
    "team-remove ok",
    "invite ok",
    "invite-revoke ok",
    "redeem ok",
    "read-records ok",
    "read-query ok",
];

/// The outcome kinds of reads that a run must also reach: the shapes of an accepted read (with rows and
/// without, a next cursor, a cursor asked for) and the refusals of each kind of read. Reading a record
/// the principal may not read and reading one that is missing print alike, so they are one outcome. The
/// scan bound of a query (10,000 records) is not reachable: no tenant of the fixture holds that many. The
/// search and aggregate outcomes (slice 10, second half) follow: a search with hits, without, with a next
/// cursor, and its refusals; an aggregate ungrouped, grouped, with top rows, and its refusals. Not listed,
/// because the fixture cannot reach them: an aggregate's unknown field (the kernel's definition checks refuse
/// a declared field the object type lacks), its sum overflow (no fixture integer is near the limit) and its
/// group bound (no field of the fixture has a thousand values).
pub const READ_KINDS: [&str; 23] = [
    "read-records ok rows>0",
    "read-records ok rows=0",
    "read-records err limit_exceeded",
    "read-records err unknown_object",
    "read-query ok rows>0",
    "read-query ok rows=0",
    "read-query ok next",
    "read-query ok cursor",
    "read-query err unknown_query",
    "read-query err invalid_intent",
    "read-query err limit_exceeded",
    "read-search ok hits>0",
    "read-search ok hits=0",
    "read-search ok next",
    "read-search err invalid_intent",
    "read-search err unknown_object",
    "read-search err no_tenant",
    "read-aggregate ok ungrouped",
    "read-aggregate ok grouped",
    "read-aggregate ok top",
    "read-aggregate err unknown_query",
    "read-aggregate err invalid_intent",
    "read-aggregate err limit_exceeded scan-bound-exceeded",
];

/// Outcome kinds the model can print that no scenario can reach, each with the reason. They are
/// left out of the coverage requirement; anything else that never occurs is a coverage gap.
pub const UNREACHABLE: [(&str, &str); 1] = [(
    "err evaluation",
    "the kernel's expr::check refuses ill-typed constraints and defaults when they are defined, so      an accepted schema never fails to evaluate, and the generator never retypes or removes a field a constraint reads (the slice 11 review note says why); reachable again once the model has the kernel's type check of expressions",
)];

/// What a run found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub scenarios: usize,
    pub operations: usize,
    pub batches: usize,
    /// Scenarios on which the model and the kernel print different text.
    pub disagreements: usize,
    /// The first disagreement, minimised; `None` when every scenario agreed.
    pub minimised: Option<Minimised>,
    /// How often the model's output showed each outcome kind: `err <kind>` for each error
    /// constructor, `<intent> ok` for each accepted kind (see [`ACCEPTED_KINDS`]), and
    /// `unparsed` for a line the model could not read.
    pub coverage: BTreeMap<String, usize>,
    /// The outcome kinds a run must reach: every error the model can print, then
    /// [`ACCEPTED_KINDS`].
    pub required: Vec<String>,
}

impl Totals {
    /// The required outcome kinds the run never showed.
    pub fn uncovered(&self) -> Vec<String> {
        self.required
            .iter()
            .filter(|kind| self.coverage.get(kind.as_str()).copied().unwrap_or(0) == 0)
            .cloned()
            .collect()
    }
}

/// The intent of an `op` line, as the coverage names it: `create`, `update`, `patch`, `delete`,
/// `require-strict`, `require-lax`, `unique-strict`, `unique-lax`, `composite`, `schema` (slice 11), or one of the membership
/// intents of slice 7 (`create-tenant`, `assign`, `revoke`, `team`, `team-assign`, `team-revoke`,
/// `team-add`, `team-remove`, `invite`, `invite-revoke`, `redeem`).
fn intent_of(op: &str) -> &'static str {
    let mut tokens: Vec<&str> = op.split_whitespace().collect();
    // `read <tenant> <principal> records|query ..` is a read.
    if tokens.first().copied() == Some("read") {
        return match tokens.get(3).copied() {
            Some("records") => "read-records",
            Some("query") => "read-query",
            _ => "unknown",
        };
    }
    // `op <tenant> <principal> type <n> <intent> ..` names an object type; the intent follows it.
    if tokens.get(3).copied() == Some("type") {
        tokens.drain(3..5.min(tokens.len()));
    }
    match tokens.get(3).copied() {
        Some("create") => "create",
        Some("update") => "update",
        Some("patch") => "patch",
        Some("delete") => "delete",
        Some("require") if tokens.get(5).copied() == Some("strict") => "require-strict",
        Some("require") => "require-lax",
        Some("unique") if tokens.get(5).copied() == Some("strict") => "unique-strict",
        Some("unique") => "unique-lax",
        Some("composite") => "composite",
        Some("schema") => "schema",
        Some("create-tenant") => "create-tenant",
        Some("assign") => "assign",
        Some("revoke") => "revoke",
        Some("team") => "team",
        Some("team-assign") => "team-assign",
        Some("team-revoke") => "team-revoke",
        Some("team-add") => "team-add",
        Some("team-remove") => "team-remove",
        Some("invite") => "invite",
        Some("invite-revoke") => "invite-revoke",
        Some("redeem") => "redeem",
        _ => "unknown",
    }
}

/// Count the model's outcome of each operation of each scenario into `coverage`.
fn count_outcomes(
    coverage: &mut BTreeMap<String, usize>,
    cases: &[Case],
    lean_parts: &[Vec<&str>],
) {
    for (case, part) in cases.iter().zip(lean_parts) {
        // Line 0 is `scenario <name>`; then the lines of each operation, the first of which is its outcome.
        for (op, group) in case.ops.iter().zip(op_outcomes(part)) {
            let line = &group[0];
            let kind = if *line == "ok" {
                format!("{} ok", intent_of(op))
            } else if let Some(error) = line.strip_prefix("err ") {
                format!("err {error}")
            } else {
                "unparsed".to_string()
            };
            *coverage.entry(kind).or_insert(0) += 1;
            // The shapes of a read's answer, besides its outcome.
            for shape in read_shapes(op, group) {
                if !shape.ends_with(" ok") {
                    *coverage.entry(shape).or_insert(0) += 1;
                }
            }
        }
    }
}

/// Generate `count` scenarios from `seed` with `generator` in batches of `batch` scenarios, and
/// compare the Lean model with the Rust kernel on each. Every batch is written to `out_dir` as
/// `batch-<k>.txt` and run through one `lake exe strata_model` process. Scenarios are compared one
/// at a time, and the first disagreement is minimised (see [`minimise`]); later ones are only
/// counted. The model's outcomes are counted into [`Totals::coverage`].
pub fn run(
    lean_dir: &Path,
    out_dir: &Path,
    seed: u64,
    count: usize,
    batch: usize,
    generator: Generator,
) -> Result<Totals, String> {
    if batch == 0 {
        return Err("the batch size must be at least 1".into());
    }
    let main = std::fs::read_to_string(lean_dir.join("Main.lean"))
        .map_err(|e| format!("cannot read Main.lean: {e}"))?;
    let mut required: Vec<String> = lean_error_names(&main)?
        .into_iter()
        .map(|name| format!("err {name}"))
        .filter(|kind| !UNREACHABLE.iter().any(|(k, _)| k == kind))
        .collect();
    required.extend(ACCEPTED_KINDS.iter().map(|kind| kind.to_string()));
    required.extend(READ_KINDS.iter().map(|kind| kind.to_string()));
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    // `lake exe` runs in `lean_dir`, so a relative batch path would be read from the wrong place.
    let out_dir = &std::path::absolute(out_dir)
        .map_err(|e| format!("cannot resolve {}: {e}", out_dir.display()))?;
    build_model(lean_dir)?;
    let scratch = out_dir.join("minimise-scratch.txt");
    let mut stream = match generator {
        Generator::Fixture => ScenarioStream::new(seed),
        Generator::Full => ScenarioStream::full(seed),
    };
    let mut totals = Totals {
        required,
        ..Totals::default()
    };
    while totals.scenarios < count {
        let size = batch.min(count - totals.scenarios);
        let first = totals.scenarios;
        let text = stream.batch(size);
        let path = out_dir.join(format!("batch-{:05}.txt", totals.batches));
        std::fs::write(&path, &text)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let lean = run_model(lean_dir, &path.to_string_lossy())?;
        let kernel = kernel_output(&text);
        let cases = parse_cases(&text);
        let (lean_parts, kernel_parts) = (outputs_by_scenario(&lean), outputs_by_scenario(&kernel));
        if lean_parts.len() != cases.len() || kernel_parts.len() != cases.len() {
            return Err(format!(
                "batch {} has {} scenarios, but Lean printed {} and the kernel {}",
                path.display(),
                cases.len(),
                lean_parts.len(),
                kernel_parts.len()
            ));
        }
        totals.batches += 1;
        totals.scenarios += cases.len();
        totals.operations += operations(&cases);
        count_outcomes(&mut totals.coverage, &cases, &lean_parts);
        for (k, case) in cases.iter().enumerate() {
            if lean_parts[k] == kernel_parts[k] {
                continue;
            }
            totals.disagreements += 1;
            if totals.minimised.is_none() {
                totals.minimised = Some(minimise_disagreement(
                    lean_dir,
                    out_dir,
                    &scratch,
                    seed,
                    first + k,
                    case,
                )?);
            }
        }
    }
    Ok(totals)
}
