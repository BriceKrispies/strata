//! Differential random testing at scale, in the pattern Cedar uses between its Lean model and its
//! Rust implementation.
//!
//! [`run`] draws scenarios from [`ScenarioStream`], the generator behind the committed fixtures,
//! and writes them a batch at a time. Each batch goes to the Lean model in one `lake exe` process
//! and through the Rust kernel in process, and the two are compared scenario by scenario. A
//! scenario starts from the model's initial world, so a disagreement belongs to one scenario.
//!
//! The first disagreement is minimised by [`minimise`] (delta debugging over whole scenarios, then
//! over the operations of each) and written as a hand-written scenario named `h-drt-<seed>-<n>`,
//! ready to append to `lean/fixtures/scenarios.txt`. Each candidate the minimiser tries is one more
//! Lean process, and that happens only once there is a disagreement to shrink.

use crate::lean::{build_model, compare_outputs, kernel_output, run_model, ScenarioStream};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// One scenario: its name, and its operation lines as they appear between `scenario` and `end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub ops: Vec<String>,
}

/// The scenarios of a scenario file. Only `scenario` and `op` lines are read: the `end` lines are
/// implied by the next `scenario` line or the end of the file, and anything else is dropped.
pub fn parse_cases(text: &str) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("scenario ") {
            cases.push(Case {
                name: name.to_string(),
                ops: Vec::new(),
            });
        } else if line.starts_with("op ") {
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
}

/// Generate `count` scenarios from `seed` in batches of `batch` scenarios, and compare the Lean
/// model with the Rust kernel on each. Every batch is written to `out_dir` as `batch-<k>.txt`
/// and run through one `lake exe strata_model` process. Scenarios are compared one at a time,
/// and the first disagreement is minimised (see [`minimise`]); later ones are only counted.
pub fn run(
    lean_dir: &Path,
    out_dir: &Path,
    seed: u64,
    count: usize,
    batch: usize,
) -> Result<Totals, String> {
    if batch == 0 {
        return Err("the batch size must be at least 1".into());
    }
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    build_model(lean_dir)?;
    let scratch = out_dir.join("minimise-scratch.txt");
    let mut stream = ScenarioStream::new(seed);
    let mut totals = Totals::default();
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
