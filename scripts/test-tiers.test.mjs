// Tests for scripts/test-tiers.mjs. Run with: node --test scripts/test-tiers.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { join, resolve } from 'node:path';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { applyTimes, gateWallVerdict, main, measuredRecord, parseCargoLog, readTiers, routineArgs, routineSeconds, suiteVerdict, tierProblems } from './test-tiers.mjs';
import { readWorkspace } from './test-select.mjs';

const GRAPH = {
  packages: [
    { name: 'a', dir: 'crates/a', deps: [], tests: ['fast', 'slow', 'same'] },
    { name: 'b', dir: 'crates/b', deps: [], tests: ['same'] },
  ],
};
const tiers = (tests, extra = {}) => ({ budgetSeconds: 120, unitSeconds: 1, doctestSeconds: 0, tests, ...extra });
const routine = (target, seconds = 1, pkg = 'a') => ({ package: pkg, target, tier: 'routine', seconds });
const full = (target, pkg = 'a', extra = {}) => ({ package: pkg, target, tier: 'full', seconds: 500, reason: 'replays every scenario', ...extra });

test('a sound list has no problems', () => {
  assert.deepEqual(tierProblems(GRAPH, tiers([routine('fast'), full('slow'), routine('same'), routine('same', 1, 'b')])), []);
});

test('a test binary in neither tier is a problem naming it', () => {
  const problems = tierProblems(GRAPH, tiers([routine('fast'), routine('same'), routine('same', 1, 'b')]));
  assert.equal(problems.length, 1);
  assert.match(problems[0], /a:slow is in neither tier/);
});

test('an entry for a binary that does not exist, and a binary listed twice, are problems', () => {
  const base = [routine('fast'), routine('slow'), routine('same'), routine('same', 1, 'b')];
  assert.match(tierProblems(GRAPH, tiers([...base, routine('gone')])).join('\n'), /a:gone .* no such test binary/);
  assert.match(tierProblems(GRAPH, tiers([...base, routine('fast')])).join('\n'), /a:fast is listed twice/);
});

test('an excluded binary needs a reason and a measured time', () => {
  const base = [routine('fast'), routine('same'), routine('same', 1, 'b')];
  assert.match(tierProblems(GRAPH, tiers([...base, full('slow', 'a', { reason: ' ' })])).join('\n'), /without a reason/);
  assert.match(tierProblems(GRAPH, tiers([...base, full('slow', 'a', { seconds: null })])).join('\n'), /without a measured time/);
  assert.deepEqual(tierProblems(GRAPH, tiers([...base, full('slow', 'a', { seconds: null, unmeasured: true })])), []);
});

test('a test name that is routine in one package and full in another is a problem', () => {
  const problems = tierProblems(GRAPH, tiers([routine('fast'), routine('slow'), routine('same'), full('same', 'b')]));
  assert.match(problems.join('\n'), /test name same is routine in one package and full in another/);
});

test('a routine tier over its budget is a problem', () => {
  const problems = tierProblems(GRAPH, tiers([routine('fast', 100), routine('slow', 30), routine('same'), routine('same', 1, 'b')]));
  assert.match(problems.join('\n'), /routine tier measures 133\.0 s, over its budget of 120 s/);
});

test('the routine arguments run the unit tests and the routine binaries by name, with the backend feature', () => {
  const args = routineArgs(tiers([routine('fast'), full('slow'), routine('same'), routine('same', 1, 'b')]));
  assert.deepEqual(args, ['test', '--workspace', '--lib', '--bins', '--test', 'fast', '--test', 'same', '--no-fail-fast', '--features', 'strata-backend/development']);
  assert.equal(routineSeconds(tiers([routine('fast', 2), full('slow')])), 3);
});

const LOG = [
  '     Running unittests src\\lib.rs (target\\debug\\deps\\strata_kernel-1.exe)',
  'test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s',
  '     Running tests\\fast.rs (target\\debug\\deps\\fast-2.exe)',
  'test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.50s',
  '     Running tests\\same.rs (target\\debug\\deps\\same-3.exe)',
  'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.40s',
  '     Running tests\\same.rs (target\\debug\\deps\\same-4.exe)',
  'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.90s',
  '     Running tests\\slow.rs (target\\debug\\deps\\slow-5.exe)',
  'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 700.00s',
  '   Doc-tests strata_kernel',
  'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.77s',
].join('\n');

test('a cargo log gives each binary its name, status, tests and time', () => {
  const rows = parseCargoLog(LOG);
  assert.deepEqual(rows.map((r) => [r.name, r.status, r.tests, r.seconds]), [
    ['lib', 'ok', 3, 0.2],
    ['fast', 'FAILED', 5, 1.5],
    ['same', 'ok', 1, 0.4],
    ['same', 'ok', 1, 0.9],
    ['slow', 'ok', 1, 700],
    ['doc', 'ok', 2, 0.77],
  ]);
});

test('measured times are recorded per name (the largest of binaries sharing it) and very slow ones are marked', () => {
  const updated = applyTimes(tiers([routine('fast', 9), full('slow', 'a', { seconds: null, unmeasured: true }), routine('same'), routine('same', 1, 'b')]), parseCargoLog(LOG));
  const by = (pkg, target) => updated.tests.find((t) => t.package === pkg && t.target === target);
  assert.equal(by('a', 'fast').seconds, 1.5);
  assert.equal(by('a', 'same').seconds, 0.9);
  assert.equal(by('a', 'slow').seconds, 700);
  assert.equal(by('a', 'slow').slow, true);
  assert.equal(by('a', 'slow').unmeasured, undefined);
  assert.equal(updated.unitSeconds, 0.2);
  assert.equal(updated.doctestSeconds, 0.77);
});

test('the committed tier list covers every test binary of this workspace and fits its budget', () => {
  const problems = tierProblems(readWorkspace(resolve(import.meta.dirname, '..')), readTiers());
  assert.deepEqual(problems, []);
});

const BUDGETS = { budgetSeconds: 120, fullBudgetSeconds: 600, binaryCapSeconds: 90 };
const steps = (total) => [
  { name: 'cargo build of the test binaries', seconds: 40 },
  { name: 'cargo test run', seconds: total - 40 },
];
const fixtureRecord = (total, binaries) => measuredRecord({
  steps: steps(total),
  binaries: binaries.map(([label, seconds]) => ({ label, name: label, seconds, status: 'ok', tests: 1 })),
  date: '2026-11-21T00:00:00.000Z',
  machine: 'fixture',
});

test('a suite over its total budget, or a binary over its cap, fails with the offenders named', () => {
  const overTotal = suiteVerdict(BUDGETS, fixtureRecord(700, [['fast', 1]]));
  assert.equal(overTotal.ok, false);
  assert.equal(overTotal.detail, 'suite 700 s of 600 s');
  assert.ok(overTotal.lines.includes('  cargo test run  660 s'));

  const overCap = suiteVerdict(BUDGETS, fixtureRecord(300, [['spec_compiler_slow', 91]]));
  assert.equal(overCap.ok, false);
  assert.equal(overCap.detail, '1 binary over the 90 s cap');
  assert.deepEqual(overCap.lines, ['binary over the 90 s cap: spec_compiler_slow  91 s']);

  assert.equal(suiteVerdict(BUDGETS, fixtureRecord(300, [['fast', 90]])).ok, true);
  assert.equal(suiteVerdict(BUDGETS, null).ok, false);
  assert.equal(suiteVerdict(BUDGETS, fixtureRecord(300, [])).ok, false);
});

// A tier file with the given `measured` record, written where `check` can read it.
function tierFile(measured) {
  const dir = mkdtempSync(join(tmpdir(), 'tiers-'));
  const path = join(dir, 'test-tiers.json');
  writeFileSync(path, JSON.stringify({ ...readTiers(), measured }, null, 2));
  return path;
}

function checkOutput(path) {
  const lines = [];
  const log = console.log;
  console.log = (...args) => lines.push(args.join(' '));
  try {
    const code = main(['check'], path);
    return { code, text: lines.join('\n') };
  } finally {
    console.log = log;
  }
}

test('check refuses a tier file whose recorded suite total is over the full budget', () => {
  const { code, text } = checkOutput(tierFile(fixtureRecord(400, [['fast', 1]])));
  assert.equal(code, 1);
  assert.match(text, /FAIL {2}the test suite fits its budget: suite 400 s of 300 s/);
});

test('check refuses a tier file with a recorded binary over the per-binary cap', () => {
  const { code, text } = checkOutput(tierFile(fixtureRecord(200, [['spec_compiler_slow', 91]])));
  assert.equal(code, 1);
  assert.match(text, /binary over the 90 s cap: spec_compiler_slow  91 s/);
});

test('check passes a tier file whose recorded suite fits both budgets', () => {
  const { code, text } = checkOutput(tierFile(fixtureRecord(250, [['fast', 30], ['slow', 89]])));
  assert.equal(code, 0);
  assert.match(text, /pass {2}the test suite fits its budget: suite 250 s of 300 s/);
});

test('times --write records the measured run, and its steps and total, in the tier file', () => {
  const dir = mkdtempSync(join(tmpdir(), 'tiers-'));
  const path = join(dir, 'test-tiers.json');
  const log = join(dir, 'cargo.txt');
  writeFileSync(path, JSON.stringify(readTiers(), null, 2));
  writeFileSync(log, LOG);
  const stepsFile = join(dir, 'steps.json');
  writeFileSync(stepsFile, JSON.stringify([{ name: 'cargo test run', seconds: 710 }, { name: 'lake build', seconds: 5 }]));
  assert.equal(main(['times', log, '--write', '--steps', stepsFile, '--machine', 'fixture'], path), 0);
  const written = JSON.parse(readFileSync(path, 'utf8'));
  assert.equal(written.measured.totalSeconds, 715);
  assert.equal(written.measured.machine, 'fixture');
  assert.equal(written.measured.binaries.find((b) => b.name === 'slow').seconds, 700);
  assert.equal(checkOutput(path).code, 1, 'a 700 s binary is over the 90 s cap, so the recorded run is refused');
});

test('the gate run is judged on wall time, with the browser suites left out, against fullBudgetSeconds of the committed list', () => {
  assert.equal(readTiers().fullBudgetSeconds, 300);
  const budget = { fullBudgetSeconds: 300 };
  assert.equal(gateWallVerdict(budget, 299.4).ok, true);
  const over = gateWallVerdict(budget, 301);
  assert.equal(over.ok, false);
  assert.match(over.detail, /301 s of 300 s/);
  const withBrowser = gateWallVerdict(budget, 900, 650);
  assert.equal(withBrowser.ok, true);
  assert.match(withBrowser.detail, /browser suites 650 s left out/);
  assert.equal(gateWallVerdict({}, 10).ok, false);
});

test('a cargo log without a Doc-tests section reads the same', () => {
  const rows = parseCargoLog(LOG.slice(0, LOG.indexOf('   Doc-tests')));
  assert.equal(rows.some((r) => r.name === 'doc'), false);
  assert.equal(applyTimes(tiers([routine('fast')]), rows).doctestSeconds, 0);
});
