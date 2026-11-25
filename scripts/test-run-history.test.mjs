// Tests for scripts/run-history.mjs and scripts/runs.mjs. Run with: node --test scripts/test-run-history.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { compareTimes, keepCount, list, prune, readRun, record, stampOf, summarize, testsReport, writeAtomic } from './run-history.mjs';
import { main as runsMain } from './runs.mjs';

const tempRoot = () => mkdtempSync(join(tmpdir(), 'strata-runs-test-'));
const at = (seconds) => new Date(Date.UTC(2026, 10, 25, 12, 58, seconds));
const names = (root, kind) => list(kind, { root }).map((p) => p.split(/[\\/]/).pop());

test('the keep count is 10 by default and STRATA_RUNS_KEEP overrides it', () => {
  assert.equal(keepCount({}), 10);
  assert.equal(keepCount({ STRATA_RUNS_KEEP: '' }), 10);
  assert.equal(keepCount({ STRATA_RUNS_KEEP: '3' }), 3);
  assert.throws(() => keepCount({ STRATA_RUNS_KEEP: '0' }), /whole number of at least 1/);
  assert.throws(() => keepCount({ STRATA_RUNS_KEEP: 'many' }), /whole number/);
});

test('a run is named by its UTC second, and a second run in the same second gets a sequence number', () => {
  assert.equal(stampOf(at(3)), '2026-11-25T12-58-03Z');
  const root = tempRoot();
  record('gate', { n: 1 }, { root, now: at(3), keep: 10 });
  record('gate', { n: 2 }, { root, now: at(3), keep: 10 });
  assert.deepEqual(names(root, 'gate'), ['2026-11-25T12-58-03Z-1.json', '2026-11-25T12-58-03Z.json']);
  assert.equal(readRun(list('gate', { root })[0]).n, 2);
  rmSync(root, { recursive: true, force: true });
});

test('recording prunes a kind to its newest runs and leaves the other kinds alone', () => {
  const root = tempRoot();
  for (let s = 0; s < 12; s += 1) record('gate', { s }, { root, now: at(s), keep: 10 });
  record('tests', { s: 0 }, { root, now: at(0), keep: 10 });
  assert.equal(list('gate', { root }).length, 10);
  assert.equal(readRun(list('gate', { root })[0]).s, 11, 'the newest run is kept');
  assert.equal(readRun(list('gate', { root })[9]).s, 2, 'the tenth newest is the oldest kept');
  assert.equal(list('tests', { root }).length, 1);
  rmSync(root, { recursive: true, force: true });
});

test('STRATA_RUNS_KEEP sets how many runs survive a record', () => {
  const root = tempRoot();
  for (let s = 0; s < 4; s += 1) record('gate', { s }, { root, now: at(s), env: { STRATA_RUNS_KEEP: '2' } });
  assert.deepEqual(list('gate', { root }).map((p) => readRun(p).s), [3, 2]);
  assert.deepEqual(prune('gate', { root, keep: 1 }).length, 1);
  assert.equal(list('gate', { root }).length, 1);
  rmSync(root, { recursive: true, force: true });
});

test('a write is atomic: the record is complete and no temporary file is left behind', () => {
  const root = tempRoot();
  const path = record('gate', { big: 'x'.repeat(100_000) }, { root, now: at(5) });
  assert.equal(readRun(path).big.length, 100_000);
  assert.deepEqual(readdirSync(join(root, 'var', 'runs', 'gate')).filter((n) => !n.endsWith('.json')), []);
  writeAtomic(path, '{"replaced":true}\n');
  assert.deepEqual(JSON.parse(readFileSync(path, 'utf8')), { replaced: true });
  rmSync(root, { recursive: true, force: true });
});

test('a write that fails leaves no temporary file behind', () => {
  const root = tempRoot();
  const target = join(root, 'blocked.json');
  mkdirSync(target); // a directory where the file should go: the rename cannot replace it
  assert.throws(() => writeAtomic(target, '{}'));
  assert.deepEqual(readdirSync(root).filter((n) => n.endsWith('.tmp')), []);
  rmSync(root, { recursive: true, force: true });
});

test('list takes only run files of the kind, newest first', () => {
  const root = tempRoot();
  record('gate', {}, { root, now: at(1) });
  record('gate', {}, { root, now: at(2) });
  writeFileSync(join(root, 'var', 'runs', 'gate', 'notes.txt'), 'not a run');
  writeFileSync(join(root, 'var', 'runs', 'gate', '.x.json.123.tmp'), '{');
  assert.deepEqual(names(root, 'gate'), ['2026-11-25T12-58-02Z.json', '2026-11-25T12-58-01Z.json']);
  assert.deepEqual(list('tests', { root }), []);
  rmSync(root, { recursive: true, force: true });
});

// A cargo test log, as cargo writes it: Running lines on stderr, results on stdout, one binary without
// the per-test times, and one with them (-Z unstable-options --report-time).
const LOG_PLAIN = [
  '     Running unittests src\\lib.rs (target\\debug\\deps\\strata_a-1111.exe)',
  'running 2 tests',
  'test tests_one ... ok',
  'test tests_two ... FAILED',
  '',
  'test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s',
  '',
  '     Running tests\\fast.rs (target\\debug\\deps\\fast-2222.exe)',
  'running 2 tests',
  'test quick ... ok',
  'test skipped ... ignored',
  '',
  'test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.02s',
].join('\n');

const LOG_TIMED = [
  '     Running unittests src\\lib.rs (target\\debug\\deps\\strata_a-1111.exe)',
  'running 2 tests',
  'test tests_one ... ok <0.120s>',
  'test tests_two ... FAILED <0.030s>',
  '',
  'test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s',
  '',
  '     Running tests\\fast.rs (target\\debug\\deps\\fast-2222.exe)',
  'running 2 tests',
  'test quick ... ok <0.012s>',
  'test skipped ... ignored',
  '',
  'test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.02s',
].join('\n');

const GRAPH = {
  packages: [
    { name: 'strata-a', dir: 'crates/a', deps: [], tests: ['fast'] },
    { name: 'strata-b', dir: 'crates/b', deps: [], tests: ['fast'] },
  ],
};

test('a test run report has each binary with its package, times and counts', () => {
  const report = testsReport(LOG_PLAIN, GRAPH, { ok: false });
  assert.equal(report.ok, false);
  assert.deepEqual(report.binaries.map((b) => [b.package, b.target, b.status, b.passed, b.failed]), [
    ['strata-a', 'lib', 'FAILED', 1, 1],
    ['strata-a', 'fast', 'ok', 1, 0],
  ]);
});

test('a binary is attributed to its package by its crate name, or by the package that names the test target', () => {
  // Both packages name the target `fast`; the unit tests of strata-b are told apart by their crate.
  const report = testsReport(LOG_PLAIN.replace('strata_a-1111', 'strata_b-1111'), GRAPH);
  assert.equal(report.binaries[0].package, 'strata-b');
  assert.equal(report.binaries[1].package, 'strata-a', 'the first package naming the target');
});

test('per-test cases are read from the log; without --report-time they carry no time and perTestTimes is false', () => {
  const report = testsReport(LOG_PLAIN, GRAPH);
  assert.deepEqual(report.cases.map((c) => [c.binary, c.name, c.seconds, c.ok]), [
    ['lib', 'tests_one', null, true],
    ['lib', 'tests_two', null, false],
    ['fast', 'quick', null, true],
  ]);
  assert.equal(report.perTestTimes, false);
});

test('with --report-time the per-test seconds are read, and ignored tests are not cases', () => {
  const report = testsReport(LOG_TIMED, GRAPH);
  assert.deepEqual(report.cases.map((c) => [c.name, c.seconds, c.ok]), [
    ['tests_one', 0.12, true],
    ['tests_two', 0.03, false],
    ['quick', 0.012, true],
  ]);
  assert.equal(report.perTestTimes, true);
  assert.equal(report.cases.some((c) => c.name === 'skipped'), false);
});

test('a log without Running lines reports no binaries', () => {
  assert.deepEqual(testsReport('error: could not compile', GRAPH).binaries, []);
});

test('comparison gives the change in seconds, and flags a time that grew by more than 20%', () => {
  const key = (r) => r.name;
  const seconds = (r) => r.s;
  const rows = compareTimes(
    [{ name: 'a', s: 12.5 }, { name: 'b', s: 1.2 }, { name: 'c', s: 3 }, { name: 'new', s: 1 }],
    [{ name: 'a', s: 10 }, { name: 'b', s: 1 }, { name: 'c', s: 2 }],
    { key, seconds },
  );
  assert.deepEqual(rows.map((r) => [r.key, r.delta, r.regressed]), [
    ['a', 2.5, true],
    ['b', 0.2, false],
    ['c', 1, true],
    ['new', null, false],
  ]);
  assert.equal(rows[3].previous, null);
});

test('a time exactly 20% over its previous time is not a regression', () => {
  const rows = compareTimes([{ name: 'a', s: 1.2 }], [{ name: 'a', s: 1 }], { key: (r) => r.name, seconds: (r) => r.s });
  assert.equal(rows[0].regressed, false);
});

test('the gate summary lists steps slowest first, with the change against the run before', () => {
  const current = {
    totalSeconds: 130,
    steps: [
      { name: 'npm audit', seconds: 4, ok: true, checks: [{ name: 'npm audit', ok: true }] },
      { name: 'cargo test', seconds: 90, ok: false, checks: [{ name: 'cargo test --workspace', ok: false }] },
    ],
  };
  const previous = { totalSeconds: 100, steps: [{ name: 'cargo test', seconds: 60, ok: true, checks: [] }] };
  const summary = summarize('gate', current, previous);
  assert.deepEqual(summary.steps.map((s) => [s.name, s.delta, s.regressed]), [
    ['cargo test', 30, true],
    ['npm audit', null, false],
  ]);
  assert.equal(summary.total.delta, 30);
  assert.equal(summary.total.regressed, true);
  assert.equal(summary.steps[0].checks[0].ok, false);
});

test('the tests summary lists the slowest binaries and tests, and compares them by name', () => {
  const current = testsReport(LOG_TIMED, GRAPH);
  const previous = testsReport(LOG_TIMED.replace('<0.120s>', '<0.050s>'), GRAPH);
  const summary = summarize('tests', current, previous);
  assert.equal(summary.binaries[0].key, 'strata-a:lib');
  assert.equal(summary.cases[0].key, 'strata-a:lib::tests_one');
  assert.equal(summary.cases[0].delta, 0.07);
  assert.equal(summary.cases[0].regressed, true);
});

test('runs.mjs prints the latest gate run against the one before it, and --json is the summary', () => {
  const root = tempRoot();
  const older = { startedAt: '2026-11-24T10:00:00.000Z', totalSeconds: 100, verdict: 'passed', commit: 'abc', dirty: false, flags: [], steps: [{ name: 'cargo test', command: 'cargo test', seconds: 60, ok: true, checks: [] }] };
  const newer = { startedAt: '2026-11-25T10:00:00.000Z', totalSeconds: 130, verdict: 'failed', commit: 'def', dirty: true, flags: ['--browser'], failures: ['cargo test --workspace'], steps: [{ name: 'cargo test', command: 'cargo test', seconds: 90, ok: false, checks: [{ name: 'cargo test --workspace', ok: false }] }] };
  record('gate', older, { root, now: at(1) });
  record('gate', newer, { root, now: at(2) });
  const lines = [];
  assert.equal(runsMain(['gate'], { root, out: (l) => lines.push(l), err: (l) => lines.push(l) }), 0);
  const text = lines.join('\n');
  assert.match(text, /verdict failed/);
  assert.match(text, /REGRESSED/);
  assert.match(text, /\+30\.0 s/);
  assert.match(text, /cargo test --workspace/);
  const json = [];
  assert.equal(runsMain(['gate', '--json'], { root, out: (l) => json.push(l) }), 0);
  assert.equal(JSON.parse(json.join('\n')).steps[0].delta, 30);
  rmSync(root, { recursive: true, force: true });
});

test('runs.mjs exits 1 with no run recorded, and 2 for a bad argument', () => {
  const root = tempRoot();
  const err = [];
  assert.equal(runsMain(['tests'], { root, out: () => {}, err: (l) => err.push(l) }), 1);
  assert.equal(runsMain(['bogus'], { root, out: () => {}, err: (l) => err.push(l) }), 2);
  assert.match(err.join('\n'), /usage/);
  rmSync(root, { recursive: true, force: true });
});
