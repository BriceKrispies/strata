//! Differential random testing of the Rust kernel against the Lean model, in the pattern Cedar
//! uses between its Lean model and its Rust implementation.
//!
//! A seeded generator produces operation sequences ([`Scenario`]) over the two-tenant world the
//! Lean model starts from, using only the intents `lean/Main.lean` executes. Each sequence is
//! rendered to the scenario text both sides read (protocol in [`crate::lean`]). All sequences of a
//! run go into one scenario file, so one Lean process runs the whole batch. Every outcome line and
//! the final state of each scenario are compared; a difference not explained by an entry of
//! [`BY_DESIGN`] fails, with the seed and a sequence shrunk by delta reduction.
//!
//! Shrinking removes whole operations and single field assignments. Each round runs every
//! candidate in one Lean process and keeps the first that still differs, until none does.
//!
//! [`lean_intents`] reads the intent kinds `Main.lean` dispatches so a test can fail when the
//! model gains an intent that [`GENERATED_INTENTS`] does not cover.

use crate::lean::{build_model, kernel_output, run_model};
use std::path::Path;

/// The intent kinds the generator produces; each is the word after the principal in an `op` line.
pub const GENERATED_INTENTS: [&str; 4] = ["create", "update", "delete", "require"];

/// Known differences between the model and the kernel that are intended, each with its reason.
/// A differing pair of lines is accepted only if an entry explains it. Empty means the two agree
/// everywhere the generator reaches.
pub const BY_DESIGN: &[ByDesign] = &[];

/// One accepted difference.
pub struct ByDesign {
    pub name: &'static str,
    pub reason: &'static str,
    /// Whether this entry explains why `lean` and `rust` differ for the same position of `ops`.
    pub explains: fn(ops: &[Op], lean: &str, rust: &str) -> bool,
}

/// One generated operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub tenant: u64,
    pub principal: u64,
    pub kind: OpKind,
}

/// The intents, with fields as `(field number, value token)` (`s:oak`, `i:3`, `b:1`, `n`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpKind {
    Create(Vec<(u64, String)>),
    Update(u64, Vec<(u64, String)>),
    Delete(u64),
    Require(u64, bool),
}

pub type Scenario = Vec<Op>;

fn fields_text(fields: &[(u64, String)]) -> String {
    fields
        .iter()
        .map(|(f, v)| format!(" {f}={v}"))
        .collect::<String>()
}

impl Op {
    pub fn render(&self) -> String {
        let head = format!("op {} {}", self.tenant, self.principal);
        match &self.kind {
            OpKind::Create(f) => format!("{head} create{}", fields_text(f)),
            OpKind::Update(id, f) => format!("{head} update {id}{}", fields_text(f)),
            OpKind::Delete(id) => format!("{head} delete {id}"),
            OpKind::Require(f, strict) => {
                format!(
                    "{head} require {f} {}",
                    if *strict { "strict" } else { "lax" }
                )
            }
        }
    }
}

/// Render scenarios named `<prefix>0`, `<prefix>1`, ... into one scenario file.
pub fn render(prefix: &str, scenarios: &[Scenario]) -> String {
    let mut out = String::new();
    for (n, ops) in scenarios.iter().enumerate() {
        out.push_str(&format!("scenario {prefix}{n}\n"));
        for op in ops {
            out.push_str(&op.render());
            out.push('\n');
        }
        out.push_str("end\n");
    }
    out
}

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

fn value_token(rng: &mut Lcg, field: u64) -> String {
    // Mostly the right type for the field, sometimes the wrong one, sometimes null.
    let kind = match (rng.below(10), field) {
        (0, _) => 3,
        (1, _) => rng.below(3),
        (_, 1) => 0,
        (_, 2) => 1,
        (_, 3) => 2,
        _ => 4,
    };
    match kind {
        0 => format!("s:{}", rng.pick(&["oak", "ash", "elm", "fir", "yew"])),
        1 => format!("i:{}", rng.below(40) as i64 - 5),
        2 => format!("b:{}", rng.below(2)),
        // A reference: mostly to one of the first few records (which exist, or were deleted, so
        // chains form and both delete behaviours fire), sometimes to an id never issued.
        4 => {
            if rng.below(8) == 0 {
                format!("r:{}", 90 + rng.below(5))
            } else {
                format!("r:{}", 1 + rng.below(6))
            }
        }
        _ => "n".to_string(),
    }
}

fn field_list(rng: &mut Lcg) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    for field in [1u64, 2, 3] {
        // Field 1 is required, so it is usually given.
        let include = if field == 1 {
            rng.below(8) != 0
        } else {
            rng.below(2) == 0
        };
        if include {
            out.push((field, value_token(rng, field)));
        }
    }
    for (field, odds) in [(4u64, 3), (5, 4)] {
        if rng.below(odds) == 0 {
            out.push((field, value_token(rng, field)));
        }
    }
    if rng.below(25) == 0 {
        out.push((9, "i:1".to_string()));
    }
    out
}

/// One sequence, deterministic in `seed`.
pub fn generate(seed: u64) -> Scenario {
    let mut rng = Lcg(seed ^ 0x9e37_79b9_7f4a_7c15);
    // Warm the generator: nearby seeds would otherwise start alike.
    for _ in 0..4 {
        rng.below(2);
    }
    let mut ops = Vec::new();
    for _ in 0..(8 + rng.below(20)) {
        let tenant = match rng.below(12) {
            0 => 3,
            1 | 2 => 2,
            _ => 1,
        };
        let home = if tenant == 2 { 200 } else { 100 };
        let principal = match rng.below(14) {
            0 => 999,
            1 => {
                if home == 100 {
                    201
                } else {
                    101
                }
            }
            2 | 3 => home,
            4 | 5 => home + 1,
            6..=9 => home + 2,
            _ => home + 3,
        };
        let kind = match rng.below(20) {
            0..=7 => OpKind::Create(field_list(&mut rng)),
            8..=13 => OpKind::Update(1 + rng.below(6), field_list(&mut rng)),
            14..=16 => OpKind::Delete(1 + rng.below(6)),
            17 => OpKind::Require(1 + rng.below(5), true),
            _ => OpKind::Require(1 + rng.below(5), false),
        };
        // Schema changes are mostly attempted by the owner, so both strict outcomes occur.
        let principal = if matches!(kind, OpKind::Require(..)) && rng.below(4) != 0 {
            home
        } else {
            principal
        };
        ops.push(Op {
            tenant,
            principal,
            kind,
        });
    }
    ops
}

/// The seed of case `index` in a run with base seed `base`.
pub fn case_seed(base: u64, index: usize) -> u64 {
    base.wrapping_add((index as u64).wrapping_mul(0x2545_f491_4f6c_dd1d))
}

/// The intent kinds `Main.lean`'s `parseOp` dispatches: the first word of each arm of the
/// `match rest with` that follows the `op <tenant> <principal>` prefix. Errors when an arm cannot
/// be read, so a new arm in an unfamiliar form cannot slip past unexercised.
pub fn lean_intents(main_source: &str) -> Result<Vec<String>, String> {
    let start = main_source
        .find("def parseOp")
        .ok_or("Main.lean has no parseOp")?;
    let body = &main_source[start..];
    let body = &body[body
        .find("match rest with")
        .ok_or("parseOp has no `match rest with`")?..];
    let mut kinds: Vec<String> = Vec::new();
    for line in body.lines().skip(1) {
        let line = line.trim();
        if line.starts_with("def ") {
            break;
        }
        let Some(arm) = line.strip_prefix('|') else {
            continue;
        };
        let arm = arm.trim_start();
        if arm.starts_with('_') {
            break;
        }
        let word = arm
            .strip_prefix('[')
            .unwrap_or(arm)
            .strip_prefix('"')
            .and_then(|rest| rest.split_once('"'))
            .map(|(word, _)| word.to_string())
            .ok_or_else(|| format!("cannot read the intent of this parseOp arm: {line}"))?;
        if !kinds.contains(&word) {
            kinds.push(word);
        }
    }
    if kinds.is_empty() {
        return Err("no intent arms found in parseOp".into());
    }
    Ok(kinds)
}

/// One scenario's output per entry, from its `scenario` line to its `end` line.
fn chunks(output: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    for line in output.lines() {
        if line.starts_with("scenario ") {
            out.push(Vec::new());
        }
        if let Some(last) = out.last_mut() {
            last.push(line.to_string());
        }
    }
    out
}

/// A difference that no `BY_DESIGN` entry explains.
#[derive(Clone, Debug)]
pub struct Difference {
    /// Index into the scenario list the batch was run on.
    pub scenario: usize,
    /// First differing line: `(line within the scenario's output, lean, rust)`.
    pub line: (usize, String, String),
    pub lean: Vec<String>,
    pub rust: Vec<String>,
}

fn unexplained(ops: &[Op], lean: &[String], rust: &[String]) -> Option<(usize, String, String)> {
    for i in 0..lean.len().max(rust.len()) {
        let (a, b) = (
            lean.get(i).map_or("<missing>", String::as_str),
            rust.get(i).map_or("<missing>", String::as_str),
        );
        if a != b && !BY_DESIGN.iter().any(|d| (d.explains)(ops, a, b)) {
            return Some((i, a.to_string(), b.to_string()));
        }
    }
    None
}

/// Run every scenario through Lean (one process) and the kernel; return the unexplained
/// differences, one per differing scenario.
pub fn diff_batch(lean_dir: &Path, scenarios: &[Scenario]) -> Result<Vec<Difference>, String> {
    let text = render("c", scenarios);
    let path = std::env::temp_dir().join(format!(
        "strata-lean-diff-{}-{}.txt",
        std::process::id(),
        text.len()
    ));
    std::fs::write(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let lean = run_model(lean_dir, &path.to_string_lossy());
    let _ = std::fs::remove_file(&path);
    let lean = chunks(&lean?);
    let rust = chunks(&kernel_output(&text));
    if lean.len() != scenarios.len() || rust.len() != scenarios.len() {
        return Err(format!(
            "expected {} scenarios, Lean printed {} and the kernel {}",
            scenarios.len(),
            lean.len(),
            rust.len()
        ));
    }
    Ok(scenarios
        .iter()
        .enumerate()
        .filter_map(|(n, ops)| {
            unexplained(ops, &lean[n], &rust[n]).map(|line| Difference {
                scenario: n,
                line,
                lean: lean[n].clone(),
                rust: rust[n].clone(),
            })
        })
        .collect())
}

/// Every sequence one step smaller than `ops`: one operation removed, or one field removed.
fn reductions(ops: &Scenario) -> Vec<Scenario> {
    let mut out = Vec::new();
    for i in 0..ops.len() {
        let mut fewer = ops.clone();
        fewer.remove(i);
        out.push(fewer);
    }
    for (i, op) in ops.iter().enumerate() {
        let count = match &op.kind {
            OpKind::Create(f) | OpKind::Update(_, f) => f.len(),
            _ => continue,
        };
        for j in 0..count {
            let mut smaller = ops.clone();
            if let OpKind::Create(f) | OpKind::Update(_, f) = &mut smaller[i].kind {
                f.remove(j);
            }
            out.push(smaller);
        }
    }
    out
}

/// Delta-reduce a failing sequence: one Lean process per round, keep the first candidate that
/// still differs, stop when no candidate does.
pub fn shrink(lean_dir: &Path, failing: Scenario) -> Result<Scenario, String> {
    let mut current = failing;
    for _ in 0..500 {
        let candidates = reductions(&current);
        if candidates.is_empty() {
            break;
        }
        let diffs = diff_batch(lean_dir, &candidates)?;
        match diffs.first() {
            Some(d) => current = candidates[d.scenario].clone(),
            None => break,
        }
    }
    Ok(current)
}

/// Run `cases` generated sequences from `base_seed`. `Err` carries the full failure report: the
/// seeds, the shrunk sequence and both sides' output.
pub fn run(lean_dir: &Path, base_seed: u64, cases: usize) -> Result<(), String> {
    build_model(lean_dir)?;
    let scenarios: Vec<Scenario> = (0..cases)
        .map(|i| generate(case_seed(base_seed, i)))
        .collect();
    let diffs = diff_batch(lean_dir, &scenarios)?;
    let Some(first) = diffs.first() else {
        return Ok(());
    };
    let seed = case_seed(base_seed, first.scenario);
    let small = shrink(lean_dir, scenarios[first.scenario].clone())?;
    let after = diff_batch(lean_dir, std::slice::from_ref(&small))?;
    let shown = after.first().unwrap_or(first);
    Err(format!(
        "the Lean model and the kernel differ in {} of {cases} cases (base seed {base_seed}).\n\
         First failing case {} has seed {seed}; reproduce with STRATA_LEAN_DIFF_SEED={base_seed}.\n\
         Shrunk from {} to {} operations:\n{}\n\
         First difference at output line {}: lean `{}`, kernel `{}`\n\
         Lean:\n{}\nKernel:\n{}\n\
         If the difference is intended, add it to BY_DESIGN in crates/verification/src/lean_diff.rs with its reason.",
        diffs.len(),
        first.scenario,
        scenarios[first.scenario].len(),
        small.len(),
        render("shrunk", std::slice::from_ref(&small)),
        shown.line.0 + 1,
        shown.line.1,
        shown.line.2,
        shown.lean.join("\n"),
        shown.rust.join("\n"),
    ))
}
