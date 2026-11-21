// Tests for scripts/gate-evidence.mjs. Run with: node --test scripts/gate-evidence.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { collectRefs, evaluateEvidence, indexLean, indexRust, isRequirementRef, leanOutcome, parseCargo, parseLake, reportLines, rustOutcome, verifiesFromDoc } from './gate-evidence.mjs';

test('refs have the shape of a requirement id, and Verifies lines name them in any case', () => {
  for (const ok of ['KRN-001', 'EML-1562', 'MIG-602', 'A-B-12.3']) assert.ok(isRequirementRef(ok), ok);
  for (const bad of ['krn-001', 'KRN', 'KRN-ABC', 'the', 'KRN-12345']) assert.ok(!isRequirementRef(bad), bad);
  assert.deepEqual(verifiesFromDoc(' Verifies: KRN-001, krn-002.\n prose\n Verifies KRN-003; (`KRN-004`)\n Verifies: the thing'), ['KRN-001', 'KRN-002', 'KRN-003', 'KRN-004']);
});

const RUST = `
//! Verifies: NOT-001
mod inner {
    /// Verifies: KRN-001
    #[test]
    fn a_doc_line() {}

    // Verifies: KRN-002
    #[tokio::test]
    async fn a_plain_comment_line() {}

    /// Verifies: KRN-003
    fn not_a_test() { let s = "#[test] fn x() {}"; }

    #[test]
    fn no_line() {}
}
#[cfg(test)]
mod tests {
    #[test]
    #[should_panic]
    /// Verifies: KRN-004
    fn after_attributes() {}
}
`;

test('Rust tests with a Verifies line are indexed by the path cargo prints, in a unit file and an aggregator module', () => {
  const crateInfo = { kernel: { name: 'strata-kernel', mounts: { 'tests/limits.rs': [{ binary: 'kernel_it', mod: 'limits' }] } } };
  const unit = indexRust('crates/kernel/src/canon.rs', RUST, crateInfo);
  assert.deepEqual(unit.map((t) => [t.name, t.refs]), [
    ['canon::inner::a_doc_line', ['KRN-001']],
    ['canon::inner::a_plain_comment_line', ['KRN-002']],
    ['canon::tests::after_attributes', ['KRN-004']],
  ]);
  assert.deepEqual(unit[0].names, [{ name: 'canon::inner::a_doc_line', binaries: ['strata_kernel'] }]);
  const mounted = indexRust('crates/kernel/tests/limits.rs', '// Verifies: KRN-9\n#[test]\nfn t() {}', crateInfo);
  assert.deepEqual(mounted[0].names, [
    { name: 't', binaries: ['limits'] },
    { name: 'limits::t', binaries: ['kernel_it'] },
  ]);
});

const CARGO = [
  'running 2 tests',
  'test canon::inner::a_doc_line ... ok',
  'test canon::tests::after_attributes ... FAILED',
  'test result: FAILED. 1 passed; 1 failed',
  '',
  'running 2 tests',
  'test canon::inner::a_doc_line ... ok',
  'test t ... ignored, slow',
  'test result: ok. 1 passed; 0 failed; 1 ignored',
  String.raw`   Running unittests src\lib.rs (target\debug\deps\strata_kernel-10b4fa49682a4a5f.exe)`,
  String.raw`   Running tests\other.rs (target\debug\deps\other-8644bc85652f68d2.exe)`,
].join('\n');

test('cargo output is read per test and binary, the headers at its end naming each block', () => {
  const results = parseCargo(CARGO);
  assert.deepEqual(results.map((r) => [r.name, r.outcome, r.binary]), [
    ['canon::inner::a_doc_line', 'passed', 'strata_kernel'],
    ['canon::tests::after_attributes', 'failed', 'strata_kernel'],
    ['canon::inner::a_doc_line', 'passed', 'other'],
    ['t', 'ignored', 'other'],
  ]);
  const site = (name, binaries) => ({ names: [{ name, binaries }] });
  assert.equal(rustOutcome(site('canon::inner::a_doc_line', ['strata_kernel']), results), 'passed');
  assert.equal(rustOutcome(site('canon::tests::after_attributes', ['strata_kernel']), results), 'failed');
  assert.equal(rustOutcome(site('t', ['strata_kernel']), results), null, 'the result of another binary is not this test');
  assert.equal(rustOutcome(site('t', ['other']), results), 'ignored');
  // headers that do not match the blocks: the binary is unknown and the name alone decides
  assert.deepEqual(parseCargo(CARGO.split('\n').slice(0, -1).join('\n')).map((r) => r.binary), [null, null, null, null]);
});

test('a result that a test printed over is read from the start of a later line', () => {
  const text = [
    'running 2 tests',
    'test edge::cookie ... {"event":"bff.request","status":200}',
    '{"event":"bff.request","status":403}',
    'ok',
    'test edge::origin ... FAILED',
    'test result: FAILED. 1 passed; 1 failed',
  ].join('\n');
  assert.deepEqual(parseCargo(text).map((r) => [r.name, r.outcome]), [
    ['edge::cookie', 'passed'],
    ['edge::origin', 'failed'],
  ]);
});

const LEAN = `import Strata.Model

/-- Statement.

    Verifies: MIG-1. -/
theorem good (a : Nat) : a + 0 = a := by
  simp

/-- Verifies: MIG-2 -/
@[simp] theorem hollow : True := trivial

/-- Verifies: MIG-3 -/
theorem unfinished (a : Nat) : a = a := by
  sorry

-- Verifies: MIG-4
theorem not_a_doc_comment : 1 = 1 := rfl
`;

test('Lean theorems with a Verifies doc comment are indexed with their statement and range', () => {
  const sites = indexLean('lean/Strata/Evolution/Lens.lean', LEAN);
  assert.deepEqual(sites.map((s) => [s.name, s.refs, s.module, s.trivial, s.sorry]), [
    ['good', ['MIG-1'], 'Strata.Evolution.Lens', false, false],
    ['hollow', ['MIG-2'], 'Strata.Evolution.Lens', true, false],
    ['unfinished', ['MIG-3'], 'Strata.Evolution.Lens', false, true],
  ]);
  assert.equal(sites[0].startLine, 6);
  assert.equal(sites[0].endLine, 9, 'up to the line before the next command, its doc comment included');
});

const LAKE = [
  '✔ [1/4] Built Strata.Model',
  'ℹ [2/4] Replayed Strata.Evolution.Lens',
  '⚠ [3/4] Replayed Strata.Evolution.Lemmas',
  'warning: Strata/Evolution/Lemmas.lean:12:2: declaration uses \'sorry\'',
  '✖ [4/4] Building Strata.Broken',
  'trace: .> lean Strata/Broken.lean',
  'error: Strata/Broken.lean:3:0: unknown identifier',
].join('\n');

test('a Lean theorem passes only when its module built with no sorry or error inside it', () => {
  const lake = parseLake(LAKE);
  assert.deepEqual([...lake.built].sort(), ['Strata.Evolution.Lemmas', 'Strata.Evolution.Lens', 'Strata.Model']);
  assert.deepEqual([...lake.failed], ['Strata.Broken']);
  const site = (file, startLine, endLine) => ({ file, module: file.replace(/^lean\//, '').replace(/\.lean$/, '').split('/').join('.'), startLine, endLine, sorry: false });
  assert.equal(leanOutcome(site('lean/Strata/Evolution/Lens.lean', 1, 10), lake), 'passed');
  assert.equal(leanOutcome(site('lean/Strata/Evolution/Lemmas.lean', 10, 14), lake), 'failed', 'a sorry inside the theorem');
  assert.equal(leanOutcome(site('lean/Strata/Evolution/Lemmas.lean', 20, 30), lake), 'passed', 'a sorry elsewhere in the module is another theorem');
  assert.equal(leanOutcome(site('lean/Strata/Broken.lean', 1, 5), lake), 'failed');
  assert.equal(leanOutcome(site('lean/Strata/Elsewhere.lean', 1, 5), lake), null, 'a module the log never mentions');
});

const requirement = (id, ref, milestone, realisation = 'checked') => ({ id, ref, milestone, kind: 'requirement', realisation });

test('a started requirement needs a passing proof; refs of contracts are known, others are reported', () => {
  const requirements = [
    requirement('a-1', 'AAA-001', 1),
    requirement('a-2', 'AAA-002', 1),
    requirement('a-3', 'AAA-003', 1),
    requirement('a-4', 'AAA-004', 1),
    requirement('a-5', 'AAA-005', 1, 'construction'),
    requirement('a-6', 'AAA-006', 1, 'construction'),
    requirement('b-1', 'BBB-001', 2),
  ];
  const site = (name, refs) => ({ file: 'crates/x/tests/t.rs', line: 1, name, names: [{ name, binaries: ['t'] }], refs });
  const report = evaluateEvidence({
    requirements,
    knownRefs: collectRefs({ contracts: [{ ref: 'CT-001' }], claims: requirements }),
    modelClaims: [{ id: 'm-5', models: 'a-5' }, { id: 'm-6', models: 'a-6' }],
    verdicts: [{ claimId: 'm-5', status: 'verified' }, { claimId: 'm-6', status: 'violated' }],
    rustTests: [site('passes', ['AAA-001']), site('fails', ['AAA-002']), site('ignored', ['AAA-003']), site('contract', ['CT-001']), site('stray', ['ZZZ-009']), site('planned', ['BBB-001'])],
    rustResults: [
      { name: 'passes', outcome: 'passed', binary: 't' },
      { name: 'fails', outcome: 'failed', binary: 't' },
      { name: 'ignored', outcome: 'ignored', binary: 't' },
      { name: 'planned', outcome: 'passed', binary: 't' },
    ],
    started: [1],
  });
  assert.equal(report.ok, false);
  assert.deepEqual(report.unproven.map((u) => u.ref), ['AAA-002', 'AAA-003', 'AAA-004', 'AAA-006']);
  assert.match(report.unproven[3].reason, /m-6 violated/);
  assert.deepEqual(report.unknownRefs.map((u) => u.ref), ['ZZZ-009']);
  assert.deepEqual(report.counts, { requirements: 7, proven: 3, started: 6, startedProven: 2, planned: 1, sites: { rust: 6, rustPassed: 2, lean: 0, leanPassed: 0 } });
  assert.ok(reportLines(report)[0].startsWith('2 of 6 started requirements proven'));
});

test('a theorem that states True is reported and does not prove the requirement', () => {
  const lean = { file: 'lean/A.lean', module: 'A', name: 'hollow', startLine: 1, endLine: 3, refs: ['AAA-001'], trivial: true, sorry: false };
  const base = { requirements: [requirement('a-1', 'AAA-001', 1)], lake: parseLake('✔ [1/1] Built A\n'), started: [1] };
  const report = evaluateEvidence({ ...base, leanTheorems: [lean] });
  assert.equal(report.ok, false);
  assert.deepEqual(report.trivial, ['lean/A.lean:1 hollow']);
  assert.match(report.unproven[0].reason, /trivial/);
  assert.equal(evaluateEvidence({ ...base, leanTheorems: [{ ...lean, trivial: false }] }).ok, true);
});
