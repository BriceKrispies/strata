// Tests for scripts/test-tiers.mjs. Run with: node --test scripts/test-tiers.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resolve } from 'node:path';
import { applyTimes, parseCargoLog, readTiers, routineArgs, routineSeconds, tierProblems } from './test-tiers.mjs';
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
