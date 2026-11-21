// The test tiers: what a change is checked with before it is stacked (routine) and what the stack
// is checked with (full). The list is `scripts/test-tiers.json`; this script reads it.
//
//   node scripts/test-tiers.mjs check            the tier list covers every test binary, nothing stale (no build), and
//                                                 the recorded full-suite measurement fits its budget
//   node scripts/test-tiers.mjs routine          run the routine tier (cargo test on the routine binaries, then the
//                                                 web and script tests) and fail when its test time passes the budget
//   node scripts/test-tiers.mjs times <log> [--write] [--steps <json>] [--machine <note>]
//                                                the measured time of every test binary in a `cargo test` log (with its
//                                                 `Running` lines), ranked, and the routine total; `--write` records the
//                                                 times and a `measured` record (total, steps, binaries) in the tier list.
//                                                 `--steps` is a JSON array of {name, seconds} for the whole test phase;
//                                                 without it the total is the binaries' sum
//   node scripts/test-tiers.mjs list             the tier list as a table
//
// The full tier is every test binary: `npm run spec` (the gate) runs `cargo test --workspace`, which
// includes the routine ones. A test binary that is in neither tier fails `check`, which the gate runs.
//
// Unit tests (the `lib` and `bin` targets of every package) are always in the routine tier; their
// measured sum is `unitSeconds` in the list. Doctests are in the full tier (`doctestSeconds`).
//
// The full test phase has its own budget: `fullBudgetSeconds` for the whole phase and `binaryCapSeconds`
// for any one test binary's run. The gate measures both from the run it makes and fails over either;
// `measured` in the tier list is the last recorded run (`times --write`), which `check` judges too.
import { spawnSync } from 'node:child_process';
import { arch, cpus, platform } from 'node:os';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { readWorkspace } from './test-select.mjs';

const root = resolve(import.meta.dirname, '..');
export const TIERS_FILE = resolve(import.meta.dirname, 'test-tiers.json');

export function readTiers(path = TIERS_FILE) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

/**
 * What is wrong with the tier list against the workspace, as sentences (empty when sound):
 * a test binary in neither tier or in a tier twice, an entry for a binary that does not exist, an
 * excluded binary without a reason or a measured time, a test name that is routine in one package
 * and full in another (cargo selects `--test` by name), and a routine tier over its budget.
 */
export function tierProblems(graph, tiers) {
  const problems = [];
  const exists = new Map(graph.packages.flatMap((p) => p.tests.map((t) => [`${p.name}:${t}`, p])));
  const seen = new Map();
  for (const e of tiers.tests) {
    const key = `${e.package}:${e.target}`;
    if (!['routine', 'full'].includes(e.tier)) problems.push(`${key}: tier must be "routine" or "full", not ${JSON.stringify(e.tier)}`);
    if (seen.has(key)) problems.push(`${key} is listed twice in scripts/test-tiers.json`);
    seen.set(key, e);
    if (!exists.has(key)) problems.push(`${key} is in scripts/test-tiers.json but no such test binary exists: remove the entry`);
    if (e.tier === 'full') {
      if (!String(e.reason ?? '').trim()) problems.push(`${key} is excluded from the routine tier without a reason`);
      if (e.seconds === null && !e.unmeasured) problems.push(`${key} is excluded from the routine tier without a measured time (or "unmeasured": true)`);
    }
    if (e.tier === 'routine' && e.slow) problems.push(`${key} is marked slow but is in the routine tier`);
  }
  for (const key of exists.keys()) {
    if (!seen.has(key)) problems.push(`test binary ${key} is in neither tier: add it to scripts/test-tiers.json with its tier (routine when it runs in seconds, full with a reason when it does not)`);
  }
  const byName = new Map();
  for (const e of tiers.tests) {
    const tiersOfName = byName.get(e.target) ?? new Set();
    tiersOfName.add(e.tier);
    byName.set(e.target, tiersOfName);
  }
  for (const [name, set] of byName) {
    if (set.size > 1) problems.push(`test name ${name} is routine in one package and full in another; cargo selects --test by name, so give them one tier`);
  }
  const total = routineSeconds(tiers);
  if (total > tiers.budgetSeconds) problems.push(`the routine tier measures ${total.toFixed(1)} s, over its budget of ${tiers.budgetSeconds} s: move the slowest routine binaries to the full tier`);
  return problems;
}

/** Measured seconds of the routine tier: the listed routine binaries and the unit tests. */
export function routineSeconds(tiers) {
  return tiers.unitSeconds + tiers.tests.filter((e) => e.tier === 'routine').reduce((s, e) => s + (e.seconds ?? 0), 0);
}

/** The cargo arguments of the routine tier: unit tests of the workspace and the routine binaries by name. */
export function routineArgs(tiers) {
  const names = [...new Set(tiers.tests.filter((e) => e.tier === 'routine').map((e) => e.target))].sort();
  return ['test', '--workspace', '--lib', '--bins', ...names.flatMap((n) => ['--test', n]), '--no-fail-fast', '--features', 'strata-backend/development'];
}

// `fast-2.exe` and `strata_kernel-1f.exe` name their binary `fast` and `strata_kernel`.
function exeName(file) {
  return file.split(/[\\/]/).pop().replace(/\.exe$/, '').replace(/-[0-9a-f]+$/, '');
}

/**
 * Every test binary a `cargo test` log reports: `{name, label, tests, status, seconds}` from the
 * `Running tests\x.rs (...)` line and the `test result: ... finished in Ns` line after it. `name` is
 * the integration test name, `lib` or `bin` for unit tests, `doc` for doctests; `label` adds the
 * crate and source path. The log must hold the `Running` lines, which cargo writes to stderr.
 */
export function parseCargoLog(text) {
  const rows = [];
  let current = null;
  for (const line of text.split(/\r?\n/)) {
    let m = /^\s+Running (?:unittests )?(.+?) \(/.exec(line);
    if (m) {
      const path = m[1].replace(/\\/g, '/');
      const test = /(?:^|\/)tests\/([^/]+?)(?:\.rs|\/main\.rs)$/.exec(path);
      const exe = exeName(/\(([^()]*)\)\s*$/.exec(line)?.[1] ?? '');
      current = { name: test ? test[1] : path.endsWith('main.rs') ? 'bin' : 'lib', label: `${exe} ${path}`.trim() };
      continue;
    }
    m = /^\s+Doc-tests (\S+)/.exec(line);
    if (m) {
      current = { name: 'doc', label: `${m[1]} (doc tests)` };
      continue;
    }
    m = /^test result: (\w+)\. (\d+) passed; (\d+) failed;.*finished in ([\d.]+)s/.exec(line);
    if (m && current) {
      rows.push({ ...current, status: m[1], tests: Number(m[2]) + Number(m[3]), seconds: Number(m[4]) });
      current = null;
    }
  }
  return rows;
}

/** Record measured times in the tier list: per test name the largest time among binaries of that name. */
export function applyTimes(tiers, rows) {
  const byName = new Map();
  for (const r of rows) byName.set(r.name, Math.max(byName.get(r.name) ?? 0, r.seconds));
  const out = structuredClone(tiers);
  for (const e of out.tests) {
    if (byName.has(e.target)) {
      e.seconds = byName.get(e.target);
      delete e.unmeasured;
      e.slow = e.tier === 'full' && e.seconds >= out.budgetSeconds ? true : undefined;
      if (e.slow === undefined) delete e.slow;
    }
  }
  const sum = (name) => rows.filter((r) => r.name === name).reduce((s, r) => s + r.seconds, 0);
  out.unitSeconds = Number((sum('lib') + sum('bin')).toFixed(2));
  out.doctestSeconds = Number(sum('doc').toFixed(2));
  return out;
}

/** Which machine a measurement was taken on, without naming the person or the host. */
export function machineNote() {
  return `${platform()} ${arch()}, ${cpus().length} logical cores`;
}

const seconds3 = (n) => Math.round(n * 1000) / 1000;

/**
 * The `measured` record of one run of the full test phase: `steps` are the wall-clock seconds of each
 * step (cargo build, cargo run, lake, web, scripts, tsc), `binaries` the parsed cargo rows. The total
 * is the sum of the steps.
 */
export function measuredRecord({ steps, binaries, date = new Date().toISOString(), machine = machineNote() }) {
  const stepRows = steps.map((s) => ({ name: s.name, seconds: seconds3(s.seconds) }));
  return {
    date,
    machine,
    totalSeconds: seconds3(stepRows.reduce((sum, s) => sum + s.seconds, 0)),
    steps: stepRows,
    binaries: binaries.map((b) => ({ label: b.label ?? b.name, name: b.name, seconds: seconds3(b.seconds), status: b.status, tests: b.tests })),
  };
}

/**
 * The full-suite verdict on a measurement: the total against `fullBudgetSeconds`, and every test
 * binary against `binaryCapSeconds`. When the total is over, every step is listed with its time; each
 * binary over the cap is listed with its time. `ok` is false when there is no measurement.
 */
export function suiteVerdict(tiers, measured) {
  const budget = tiers.fullBudgetSeconds;
  const cap = tiers.binaryCapSeconds;
  if (!Number.isFinite(budget) || !Number.isFinite(cap)) {
    return { ok: false, detail: 'scripts/test-tiers.json has no fullBudgetSeconds and binaryCapSeconds', lines: [] };
  }
  if (!measured || !Number.isFinite(measured.totalSeconds) || !Array.isArray(measured.binaries)) {
    return { ok: false, detail: 'no measurement of the test steps and binaries was recorded', lines: [] };
  }
  if (measured.binaries.length === 0) {
    return { ok: false, detail: 'no test binary was timed (the test run did not report any)', lines: [] };
  }
  const lines = [];
  const total = measured.totalSeconds;
  const overTotal = total > budget;
  if (overTotal) {
    lines.push(`the suite total ${total} s is over the ${budget} s budget; steps, slowest first:`);
    for (const s of [...(measured.steps ?? [])].sort((a, b) => b.seconds - a.seconds)) lines.push(`  ${s.name}  ${s.seconds} s`);
  }
  const overBinaries = measured.binaries.filter((b) => b.seconds > cap).sort((a, b) => b.seconds - a.seconds);
  for (const b of overBinaries) lines.push(`binary over the ${cap} s cap: ${b.label ?? b.name}  ${b.seconds} s`);
  const slowest = Math.max(0, ...measured.binaries.map((b) => b.seconds));
  const ok = !overTotal && overBinaries.length === 0;
  const detail = ok
    ? `suite ${total} s of ${budget} s; slowest binary ${slowest} s, cap ${cap} s`
    : [
        overTotal ? `suite ${total} s of ${budget} s` : null,
        overBinaries.length ? `${overBinaries.length} binar${overBinaries.length === 1 ? 'y' : 'ies'} over the ${cap} s cap` : null,
      ].filter(Boolean).join('; ');
  return { ok, detail, lines };
}

function run(command, args, env = {}) {
  const done = spawnSync(command, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env }, shell: false });
  return done.status === 0;
}

function table(tiers) {
  const rows = [...tiers.tests].sort((a, b) => (b.seconds ?? -1) - (a.seconds ?? -1));
  for (const e of rows) {
    console.log(`${e.tier.padEnd(8)} ${(e.seconds === null || e.seconds === undefined ? 'unmeasured' : `${e.seconds.toFixed(2)} s`).padStart(11)}  ${e.package}:${e.target}${e.reason ? `  (${e.reason})` : ''}`);
  }
  console.log(`routine tier: ${routineSeconds(tiers).toFixed(1)} s measured of ${tiers.budgetSeconds} s (unit tests ${tiers.unitSeconds} s)`);
}

// Prints the suite verdict of the tier list's recorded measurement; returns whether it fits.
function reportSuite(tiers, measured) {
  if (!measured) {
    console.log('note  no full-suite measurement is recorded in scripts/test-tiers.json yet (the gate measures its own run)');
    return true;
  }
  const verdict = suiteVerdict(tiers, measured);
  if (verdict.ok) console.log(`pass  the test suite fits its budget: ${verdict.detail}`);
  else console.log(`FAIL  the test suite fits its budget: ${verdict.detail}`);
  for (const line of verdict.lines) console.log(`      ${line}`);
  return verdict.ok;
}

/** The `check` command and the `times` command read the tier list at `tiersPath` (the default is the committed one). */
export function main(argv, tiersPath = TIERS_FILE) {
  const [command, ...rest] = argv;
  const tiers = readTiers(tiersPath);
  if (command === 'check') {
    const problems = tierProblems(readWorkspace(root), tiers);
    for (const p of problems) console.log(`FAIL  ${p}`);
    if (problems.length === 0) console.log(`pass  every test binary is in a tier; routine tier ${routineSeconds(tiers).toFixed(1)} s of ${tiers.budgetSeconds} s`);
    const suiteOk = reportSuite(tiers, tiers.measured);
    return problems.length === 0 && suiteOk ? 0 : 1;
  }
  if (command === 'list') {
    table(tiers);
    return 0;
  }
  if (command === 'times') {
    const log = rest[0];
    if (!log || log.startsWith('--')) throw new Error('times needs a cargo test log');
    const flag = (name) => {
      const i = rest.indexOf(name);
      if (i < 0) return undefined;
      if (!rest[i + 1] || rest[i + 1].startsWith('--')) throw new Error(`${name} needs a value`);
      return rest[i + 1];
    };
    const rows = parseCargoLog(readFileSync(log, 'utf8'));
    const updated = applyTimes(tiers, rows);
    for (const r of [...rows].sort((a, b) => b.seconds - a.seconds)) console.log(`${r.seconds.toFixed(2).padStart(9)} s  ${String(r.tests).padStart(5)} tests  ${r.status.padEnd(6)} ${r.label}`);
    console.log(`routine tier from this log: ${routineSeconds(updated).toFixed(1)} s of ${updated.budgetSeconds} s`);
    const stepsFile = flag('--steps');
    const steps = stepsFile ? JSON.parse(readFileSync(stepsFile, 'utf8')) : [{ name: 'cargo test run', seconds: rows.reduce((s, r) => s + r.seconds, 0) }];
    const measured = measuredRecord({ steps, binaries: rows, machine: flag('--machine') });
    reportSuite(tiers, measured);
    if (rest.includes('--write')) {
      updated.measured = measured;
      writeFileSync(tiersPath, `${JSON.stringify(updated, null, 2)}\n`);
      console.log(`recorded in ${tiersPath}`);
    }
    return 0;
  }
  if (command === 'routine') {
    const problems = tierProblems(readWorkspace(root), tiers);
    for (const p of problems) console.log(`FAIL  ${p}`);
    if (problems.length) return 1;
    const env = { CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? '2', RUST_TEST_THREADS: process.env.RUST_TEST_THREADS ?? '2' };
    const started = Date.now();
    const done = spawnSync('cargo', routineArgs(tiers), { cwd: root, encoding: 'utf8', maxBuffer: 1 << 28, env: { ...process.env, ...env } });
    process.stdout.write(done.stdout ?? '');
    process.stderr.write(done.stderr ?? '');
    const testSeconds = parseCargoLog(done.stdout ?? '').reduce((sum, r) => sum + r.seconds, 0);
    const web = run(process.execPath, ['--test', 'web/test/kernel.test.js', 'web/test/client.test.js', 'web/test/tenant-ui.test.js']);
    const over = testSeconds > tiers.budgetSeconds;
    console.log(`routine tier: ${testSeconds.toFixed(1)} s of test time (budget ${tiers.budgetSeconds} s, compilation excluded), ${((Date.now() - started) / 1000).toFixed(0)} s wall with compilation`);
    if (over) console.log('FAIL  the routine tier is over its budget: record the new times (node scripts/test-tiers.mjs times <log> --write) and move the slowest binaries to the full tier');
    return done.status === 0 && web && !over ? 0 : 1;
  }
  console.error('usage: node scripts/test-tiers.mjs check | routine | list | times <log> [--write] [--steps <json>] [--machine <note>]');
  return 2;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exitCode = main(process.argv.slice(2));
