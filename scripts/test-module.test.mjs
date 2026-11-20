// Tests for scripts/test-module.mjs and scripts/exegra-module.mjs. Run with: node --test scripts/test-module.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { problemsOf } from './exegra-module.mjs';
import { describePlan, moduleScope, parseArgs, planSteps, resolveModule, runSteps } from './test-module.mjs';

const SPEC = {
  components: [{ id: 'mail-dispatcher', name: 'Mail dispatcher' }, { id: 'kernel', name: 'Kernel' }, { id: 'empty', name: 'Empty' }],
  contracts: [{ id: 'send-mail', implementedBy: 'mail-dispatcher' }, { id: 'decide', implementedBy: 'kernel' }],
  boundaries: [{ id: 'b-1', contracts: ['decide'] }, { id: 'b-2', contracts: ['send-mail'] }],
  behaviors: [{ id: 'send-flow', subject: 'mail-dispatcher' }, { id: 'kernel-flow', subject: 'kernel' }],
  scenarios: [
    { id: 'sc-send', behaviors: ['send-flow'] },
    { id: 'sc-kernel', behaviors: ['kernel-flow'] },
  ],
  claims: [
    { id: 'mal-001', kind: 'requirement', boundary: 'b-2', evaluate: { method: 'scenario' } },
    { id: 'krn-001', kind: 'requirement', boundary: 'b-1', evaluate: { method: 'property' } },
    { id: 'm-send-once', kind: 'invariant', models: 'mal-001', evaluate: { method: 'state-invariant', scenarios: ['sc-send'] } },
    { id: 'm-by-scenario', kind: 'invariant', evaluate: { method: 'never', scenarios: ['sc-send'] } },
    { id: 'm-kernel', kind: 'invariant', models: 'krn-001', evaluate: { method: 'never', scenarios: ['sc-kernel'] } },
  ],
  uiScenarios: [
    { id: 'ui-send', verifies: ['mal-001'] },
    { id: 'ui-kernel', verifies: ['krn-001'] },
  ],
};

test('arguments: a module, the flags, and a refusal of anything else', () => {
  assert.deepEqual(parseArgs(['kernel', '--dry-run']), { module: 'kernel', dryRun: true, list: false, budget: 120 });
  assert.equal(parseArgs(['kernel', '--budget', '90']).budget, 90);
  assert.equal(parseArgs(['--list']).list, true);
  assert.throws(() => parseArgs([]), /give a module/);
  assert.throws(() => parseArgs(['a', 'b']), /one module at a time/);
  assert.throws(() => parseArgs(['a', '--budget', '0']), /positive/);
  assert.throws(() => parseArgs(['a', '--nope']), /unknown argument/);
});

test('a module is a component named by id or by name, and an unknown one lists the known', () => {
  assert.equal(resolveModule(SPEC, 'kernel').id, 'kernel');
  assert.equal(resolveModule(SPEC, 'Mail Dispatcher').id, 'mail-dispatcher');
  assert.throws(() => resolveModule(SPEC, 'nope'), /no module "nope".*kernel/);
});

test('the scope of a module is its claims, scenarios and journeys, and nothing of another module', () => {
  assert.deepEqual(moduleScope(SPEC, 'mail-dispatcher'), {
    module: 'mail-dispatcher',
    claims: ['mal-001', 'm-send-once', 'm-by-scenario'],
    scenarios: ['sc-send'],
    journeys: ['ui-send'],
  });
  assert.deepEqual(moduleScope(SPEC, 'kernel').claims, ['krn-001', 'm-kernel']);
  assert.deepEqual(moduleScope(SPEC, 'empty'), { module: 'empty', claims: [], scenarios: [], journeys: [] });
});

test('the plan runs Exegra, generation, the differential and the journeys for that module only', () => {
  const scope = moduleScope(SPEC, 'mail-dispatcher');
  const steps = planSteps(scope, { home: 'C:/exegra', hash: 'abc', products: ['--product', 'p.jsonc'] });
  assert.deepEqual(steps.map((s) => s.id), ['exegra', 'specc-check', 'specc-check-web', 'specc-check-e2e', 'differential', 'journeys']);
  assert.ok(steps.every((s) => s.skip === null));
  const by = Object.fromEntries(steps.map((s) => [s.id, s]));
  assert.deepEqual(JSON.parse(by.exegra.files[0].content), ['mal-001', 'm-send-once', 'm-by-scenario']);
  assert.deepEqual(JSON.parse(by.differential.files[0].content), ['sc-send']);
  assert.ok(by.differential.env.STRATA_MODULE_SCENARIOS_FILE.endsWith('scenarios.json'));
  assert.equal(by.differential.env.CARGO_BUILD_JOBS, '2');
  assert.equal(by.differential.env.RUST_TEST_THREADS, '2');
  assert.deepEqual(by.journeys.args.slice(-1), ['ui-send.spec.js']);
  assert.ok(!by.journeys.args.includes('ui-kernel.spec.js'));
  assert.ok(by['specc-check'].args.includes('abc'));
  assert.ok(by['specc-check'].args.includes('p.jsonc'));
});

test('a module with nothing for a step skips that step with a reason, and the dry run says so', () => {
  const scope = moduleScope(SPEC, 'empty');
  const steps = planSteps(scope, { home: 'C:/exegra' });
  const skipped = steps.filter((s) => s.skip).map((s) => s.id);
  assert.deepEqual(skipped, ['exegra', 'differential', 'journeys']);
  const text = describePlan(scope, steps).join('\n');
  assert.match(text, /skip .*Exegra verification/);
  assert.match(text, /run .*specc check-web/);
});

const stepsFor = () => planSteps(moduleScope(SPEC, 'mail-dispatcher'), { home: 'C:/exegra' });
const clock = () => {
  let t = 0;
  return { now: () => t, advance: (ms) => { t += ms; } };
};
const quiet = { setTimer: () => 0, clearTimer: () => {} };
const pass = { status: 0, signal: null };

test('a run where every step passes passes, with each step timed', async () => {
  const c = clock();
  const report = await runSteps(stepsFor(), { ...quiet, now: c.now, used: () => 1000, exec: async () => { c.advance(10_000); return pass; } });
  assert.equal(report.ok, true);
  assert.equal(report.steps.length, 6);
  assert.ok(report.steps.every((r) => r.outcome === 'pass' && r.seconds === 10));
  assert.equal(report.seconds, 60);
});

for (const [broken, id] of [['a broken claim', 'exegra'], ['a broken machine refinement', 'differential'], ['a broken journey', 'journeys']]) {
  test(`${broken} makes the command fail, and the other steps still run`, async () => {
    const seen = [];
    const report = await runSteps(stepsFor(), { ...quiet, used: () => 1, exec: async (s) => { seen.push(s.id); return { status: s.id === id ? 1 : 0, signal: null }; } });
    assert.equal(report.ok, false);
    assert.deepEqual(report.steps.filter((r) => r.outcome === 'FAIL').map((r) => r.id), [id]);
    assert.equal(seen.length, 6);
  });
}

test('a step killed by a signal, or one that did not start, fails the run', async () => {
  const killed = await runSteps(stepsFor(), { ...quiet, used: () => 1, exec: async (s) => (s.id === 'differential' ? { status: null, signal: 'SIGKILL' } : pass) });
  assert.equal(killed.ok, false);
  assert.equal(killed.steps.find((r) => r.id === 'differential').outcome, 'killed');
  const threw = await runSteps(stepsFor(), { ...quiet, used: () => 1, exec: async (s) => { if (s.id === 'exegra') throw new Error('spawn failed'); return pass; } });
  assert.equal(threw.steps[0].outcome, 'killed');
  assert.equal(threw.steps[0].why, 'spawn failed');
});

test('a run over its budget fails even when every step passed', async () => {
  const c = clock();
  const report = await runSteps(stepsFor(), { ...quiet, now: c.now, used: () => 1, budgetSeconds: 120, exec: async () => { c.advance(25_000); return pass; } });
  assert.equal(report.seconds, 150);
  assert.equal(report.overBudget, true);
  assert.equal(report.ok, false);
});

test('the peak of used memory is recorded, from the samples and from after each step', async () => {
  const readings = [1000, 4000, 9000];
  let i = 0;
  const report = await runSteps(stepsFor().slice(0, 2), { ...quiet, used: () => readings[Math.min(i++, readings.length - 1)], exec: async () => pass });
  assert.equal(report.peakUsedBytes, 9000);
  assert.equal(report.peakRiseBytes, 8000);
});

test('Exegra verification fails on a violated claim and on an id that is not a claim', () => {
  assert.deepEqual(problemsOf([{ id: 'a', status: 'verified' }, { id: 'b', status: 'inconclusive' }]), []);
  assert.deepEqual(problemsOf([{ id: 'a', status: 'violated' }, { id: 'c', error: 'Unknown claim "c".' }]), [{ id: 'a', status: 'violated' }, { id: 'c', error: 'Unknown claim "c".' }]);
});
