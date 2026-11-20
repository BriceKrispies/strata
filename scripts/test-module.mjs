// Checks scoped to one module of the specification (WG-4991).
//
//   node scripts/test-module.mjs <module> [--dry-run] [--budget <seconds>] [--list]
//   npm run test:module -- <module>
//
// A module is a component of the specification (`components`, for example `kernel`, `identity-service`,
// `mail-dispatcher`), named by its id or its name. For that module and nothing else it runs:
//
//   1. Exegra verification: the module's claims, evaluated in one Exegra process after the
//      specification validates (scripts/exegra-module.mjs). A violated claim fails the step.
//   2. Generation: the specc freshness checks (`check`, `check-web`, `check-e2e`), which fail when
//      the generated model, web modules or journeys no longer match the specification.
//   3. The machine differential: the system host against Exegra's trace, for the scenarios that drive
//      the module's behaviours only (crates/system-differential/tests/exegra.rs reads the scenario
//      ids from STRATA_MODULE_SCENARIOS_FILE and skips its whole-specification allow-list and floor).
//   4. The module's generated journeys: the Playwright files of the uiScenarios that verify a claim of
//      the module (tests/e2e/generated/<uiScenario id>.spec.js).
//
// What belongs to the module is read from the specification:
//   claims     a claim whose boundary lists a contract the module implements, or that a behaviour of
//              the model of a claim of the module (`models`), or that evaluates a scenario of the module;
//   scenarios  a scenario that runs a behaviour whose `subject` is the module;
//   journeys   a uiScenario whose `verifies` names a claim of the module.
//
// Every step runs, a failing step does not stop the later ones, and the command exits 1 when any step
// failed, was killed, or when the whole run took longer than the budget (default 120 s, measured with
// CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2, which this command sets). It records the time of every step
// and the peak of the machine's used memory while it ran (sampled every 250 ms; the children are
// cargo, rustc and test processes, so the machine's figure is the one that shows a low-memory kill),
// in var/module-runs/<module>.json. `--dry-run` prints the plan and runs nothing. `--list` prints
// the module names.
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { freemem, totalmem } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseJsonc } from './jsonc.mjs';
import { productArgs, semanticHashDetail } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
export const DEFAULT_BUDGET_SECONDS = 120;

/** The options of an argument list: `{module, dryRun, list, budget}`. */
export function parseArgs(argv) {
  const out = { module: null, dryRun: false, list: false, budget: DEFAULT_BUDGET_SECONDS };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--dry-run') out.dryRun = true;
    else if (a === '--list') out.list = true;
    else if (a === '--budget') {
      out.budget = Number(argv[++i]);
      if (!Number.isFinite(out.budget) || out.budget <= 0) throw new Error('--budget wants a positive number of seconds');
    } else if (a.startsWith('--')) throw new Error(`unknown argument ${a}`);
    else if (out.module === null) out.module = a;
    else throw new Error(`one module at a time: ${out.module} and ${a}`);
  }
  if (out.module === null && !out.list) throw new Error('give a module name (a component id of the specification), or --list');
  return out;
}

/** The component a name denotes: its id, or its name compared without case. Throws, listing the ids, when none does. */
export function resolveModule(spec, name) {
  const components = spec.components ?? [];
  const wanted = String(name).toLowerCase();
  const found = components.find((c) => c.id === name) ?? components.find((c) => String(c.name ?? '').toLowerCase() === wanted || String(c.id).toLowerCase() === wanted);
  if (!found) throw new Error(`no module "${name}" in the specification; the modules are: ${components.map((c) => c.id).join(', ')}`);
  return found;
}

/** The scenario ids a claim's evaluation names (`scenario` or `scenarios`, and its negatives'). */
function scenariosOf(claim) {
  const e = claim.evaluate ?? {};
  return [e.scenario, ...(e.scenarios ?? []), ...(e.negatives ?? []).map((n) => n.scenario)].filter((x) => typeof x === 'string');
}

/**
 * What the specification says belongs to a module: `{module, claims, scenarios, journeys}`, each list
 * of ids in declaration order. See the header for the rules.
 */
export function moduleScope(spec, name) {
  const module = resolveModule(spec, name).id;
  const contractsOf = new Set((spec.contracts ?? []).filter((c) => c.implementedBy === module).map((c) => c.id));
  const boundaries = new Set((spec.boundaries ?? []).filter((b) => (b.contracts ?? []).some((c) => contractsOf.has(c))).map((b) => b.id));
  const behaviours = new Set((spec.behaviors ?? []).filter((b) => b.subject === module).map((b) => b.id));
  const scenarios = (spec.scenarios ?? []).filter((s) => (s.behaviors ?? []).some((b) => behaviours.has(b))).map((s) => s.id);
  const scenarioSet = new Set(scenarios);
  const byBoundary = new Set((spec.claims ?? []).filter((c) => c.boundary && boundaries.has(c.boundary)).map((c) => c.id));
  const claims = (spec.claims ?? [])
    .filter((c) => byBoundary.has(c.id) || (typeof c.models === 'string' && byBoundary.has(c.models)) || scenariosOf(c).some((id) => scenarioSet.has(id)))
    .map((c) => c.id);
  const claimSet = new Set(claims);
  const journeys = (spec.uiScenarios ?? []).filter((u) => (u.verifies ?? []).some((c) => claimSet.has(c))).map((u) => u.id);
  return { module, claims, scenarios, journeys };
}

/**
 * The steps of a run, in order: `{id, label, command, args, env, files, skip}`. `files` are
 * `{path, content}` written before the step (relative to `root`); `skip` is a reason when the module
 * has nothing for the step. `home` is the Exegra checkout; `hash` the specification's semantic hash
 * (null when Exegra gave none, which skips the hash arguments as test-changed does).
 */
export function planSteps(scope, { root: base = root, home, hash = null, products = [], node = 'node', jobs = '2' } = {}) {
  const dir = `var/module-runs/${scope.module}`;
  const env = { CARGO_BUILD_JOBS: jobs, RUST_TEST_THREADS: jobs };
  const hashArgs = hash ? ['--spec-hash', hash] : [];
  const specc = (name, extra) => ['cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--', name, '--spec', 'specifications/spec.jsonc', ...extra, ...products]];
  const generation = [
    ['check', ['--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime', '--core-out', 'crates/model-core', '--core-crate-name', 'strata-model-core', ...hashArgs]],
    ['check-web', ['--out', 'web/src/generated', ...hashArgs]],
    ['check-e2e', ['--out', 'tests/e2e/generated', '--origin', 'http://127.0.0.1:4200']],
  ];
  const steps = [
    {
      id: 'exegra',
      label: `Exegra verification of ${scope.claims.length} claim(s)`,
      command: node,
      args: ['scripts/exegra-module.mjs', '--exegra', home, '--spec', 'specifications/spec.jsonc', '--claims', `${dir}/claims.json`],
      env: {},
      files: [{ path: `${dir}/claims.json`, content: JSON.stringify(scope.claims) }],
      skip: scope.claims.length === 0 ? `no claim of ${scope.module} is found through its contracts or behaviours` : null,
    },
    ...generation.map(([name, extra]) => {
      const [command, args] = specc(name, extra);
      return { id: `specc-${name}`, label: `generation: specc ${name}`, command, args, env, files: [], skip: null };
    }),
    {
      id: 'differential',
      label: `machine differential of ${scope.scenarios.length} scenario(s)`,
      command: 'cargo',
      args: ['test', '-p', 'strata-system-differential', '--test', 'exegra', '--', '--nocapture'],
      env: { ...env, EXEGRA_HOME: home, STRATA_MODULE_SCENARIOS_FILE: join(base, dir, 'scenarios.json') },
      files: [{ path: `${dir}/scenarios.json`, content: JSON.stringify(scope.scenarios) }],
      skip: scope.scenarios.length === 0 ? `no scenario runs a behaviour of ${scope.module}` : null,
    },
    {
      id: 'journeys',
      label: `generated journeys: ${scope.journeys.length}`,
      command: node,
      args: [join('node_modules', '@playwright', 'test', 'cli.js'), 'test', '-c', 'tests/e2e/generated/playwright.generated.config.js', ...scope.journeys.map((j) => `${j}.spec.js`)],
      env,
      files: [],
      skip: scope.journeys.length === 0 ? `no journey verifies a claim of ${scope.module}` : null,
    },
  ];
  return steps;
}

/**
 * Runs the steps through `exec(step) -> Promise<{status, signal}>` and reports
 * `{steps: [{id, label, outcome, seconds}], seconds, ok, overBudget, peakUsedBytes, peakRiseBytes}`.
 * An outcome is `pass`, `FAIL`, `killed` (a signal, or no exit status) or `skipped`. `used()` gives the
 * machine's used memory in bytes and `now()` a time in milliseconds; `sampleMs` is how often memory is read.
 */
export async function runSteps(steps, { exec, now = Date.now, used = () => totalmem() - freemem(), budgetSeconds = DEFAULT_BUDGET_SECONDS, sampleMs = 250, setTimer = setInterval, clearTimer = clearInterval }) {
  const baseline = used();
  let peak = baseline;
  const timer = setTimer(() => {
    peak = Math.max(peak, used());
  }, sampleMs);
  timer?.unref?.();
  const begun = now();
  const rows = [];
  for (const step of steps) {
    if (step.skip) {
      rows.push({ id: step.id, label: step.label, outcome: 'skipped', seconds: 0, why: step.skip });
      continue;
    }
    const t0 = now();
    let done;
    try {
      done = await exec(step);
    } catch (e) {
      done = { status: null, signal: null, error: e?.message ?? String(e) };
    }
    peak = Math.max(peak, used());
    const outcome = done.signal || done.status === null || done.status === undefined ? 'killed' : done.status === 0 ? 'pass' : 'FAIL';
    rows.push({ id: step.id, label: step.label, outcome, seconds: (now() - t0) / 1000, ...(done.error ? { why: done.error } : {}) });
  }
  clearTimer(timer);
  const seconds = (now() - begun) / 1000;
  const overBudget = seconds > budgetSeconds;
  return {
    steps: rows,
    seconds,
    budgetSeconds,
    overBudget,
    ok: rows.every((r) => r.outcome === 'pass' || r.outcome === 'skipped') && !overBudget,
    peakUsedBytes: peak,
    peakRiseBytes: Math.max(0, peak - baseline),
  };
}

function execStep(step) {
  return new Promise((done) => {
    for (const f of step.files) {
      const target = join(root, f.path);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, f.content);
    }
    console.log(`\n== ${step.label}: ${step.command} ${step.args.join(' ')}`);
    const child = spawn(step.command, step.args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...step.env }, shell: false });
    child.on('error', (e) => done({ status: null, signal: null, error: e.message }));
    child.on('close', (status, signal) => done({ status, signal }));
  });
}

export function describePlan(scope, steps) {
  const lines = [`module ${scope.module}: ${scope.claims.length} claim(s), ${scope.scenarios.length} scenario(s), ${scope.journeys.length} journey(s)`];
  for (const s of steps) lines.push(s.skip ? `  skip   ${s.label}: ${s.skip}` : `  run    ${s.label}: ${s.command} ${s.args.join(' ')}`);
  return lines;
}

async function main(argv) {
  const options = parseArgs(argv);
  const spec = parseJsonc(readFileSync(join(root, 'specifications', 'spec.jsonc'), 'utf8'));
  if (options.list) {
    for (const c of spec.components) console.log(`${c.id}\t${c.name}`);
    return 0;
  }
  const scope = moduleScope(spec, options.module);
  const home = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
  const hash = !options.dryRun && existsSync(join(home, 'cli/exegra.js')) ? semanticHashDetail(home, process.env.EXEGRA_PROJECT ?? 'strata').hash : null;
  const steps = planSteps(scope, { home, hash, products: productArgs(root) });
  for (const line of describePlan(scope, steps)) console.log(line);
  if (options.dryRun) return 0;
  const report = await runSteps(steps, { exec: execStep, budgetSeconds: options.budget });
  console.log('');
  for (const r of report.steps) console.log(`${r.outcome.padEnd(7)} ${r.seconds.toFixed(1).padStart(7)} s  ${r.label}${r.why ? `  (${r.why})` : ''}`);
  const mb = (b) => `${(b / 1048576).toFixed(0)} MB`;
  console.log(`total ${report.seconds.toFixed(1)} s of ${report.budgetSeconds} s${report.overBudget ? ' (OVER BUDGET)' : ''}; machine memory peak ${mb(report.peakUsedBytes)} used, ${mb(report.peakRiseBytes)} above the start`);
  const out = join(root, 'var', 'module-runs', `${scope.module}.json`);
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify({ module: scope.module, ...report }, null, 2)}\n`);
  return report.ok ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then((code) => { process.exitCode = code; }, (e) => { console.error(`test-module: ${e.message}`); process.exitCode = 2; });
}
