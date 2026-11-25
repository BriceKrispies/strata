// Run history: the report of each gate run and each test run, kept as JSON under var/runs/<kind>/
// (var/ is gitignored). The newest runs of a kind are kept; older ones are deleted.
//
//   node scripts/runs.mjs [gate|tests] [--json]     print the latest run, with Δ seconds against the run before it
//
// Kinds: `gate` (one report per gate run, pass or fail, written even when the gate stops part way)
// and `tests` (one report per test run of the gate: every test binary, and each test where the run
// reported its time). `STRATA_RUNS_KEEP` sets how many runs of each kind are kept (default 10).
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { parseCargoLog } from './test-tiers.mjs';

export const DEFAULT_ROOT = resolve(import.meta.dirname, '..');
export const DEFAULT_KEEP = 10;
// A step or test whose time grew by more than this fraction against the previous run is a regression.
export const REGRESSION_THRESHOLD = 0.2;

const KIND = /^[a-z][a-z-]*$/;
// <UTC stamp>.json, or <UTC stamp>-<n>.json for a second run of the same kind in the same second.
const RUN_FILE = /^(\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}Z)(?:-(\d+))?\.json$/;

/** How many runs of each kind are kept: STRATA_RUNS_KEEP, a whole number of at least 1, else 10. */
export function keepCount(env = process.env) {
  const raw = env.STRATA_RUNS_KEEP;
  if (raw === undefined || raw.trim() === '') return DEFAULT_KEEP;
  if (!/^\d+$/.test(raw.trim()) || Number(raw) < 1) {
    throw new Error(`STRATA_RUNS_KEEP must be a whole number of at least 1, not ${JSON.stringify(raw)}`);
  }
  return Number(raw);
}

/** The directory of one kind of run, under `root`/var/runs. */
export function runsDir(kind, root = DEFAULT_ROOT) {
  if (!KIND.test(kind)) throw new Error(`a run kind is a lowercase word, not ${JSON.stringify(kind)}`);
  return join(root, 'var', 'runs', kind);
}

/** `2026-11-25T12-58-03Z` for a date: UTC, to the second, with no colons (valid in file names on Windows). */
export function stampOf(date) {
  return date.toISOString().replace(/\.\d{3}Z$/, 'Z').replace(/:/g, '-');
}

// Newest first: the stamp sorts as text; the sequence number orders runs within one second.
function newestFirst(a, b) {
  const [sa, na] = orderOf(a);
  const [sb, nb] = orderOf(b);
  if (sa !== sb) return sa < sb ? 1 : -1;
  return nb - na;
}

function orderOf(name) {
  const m = RUN_FILE.exec(basename(name));
  return [m[1], Number(m[2] ?? 0)];
}

/** The run files of a kind, newest first, as paths. Other files in the directory are not runs. */
export function list(kind, options = {}) {
  const dir = runsDir(kind, options.root ?? DEFAULT_ROOT);
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => RUN_FILE.test(name))
    .sort(newestFirst)
    .map((name) => join(dir, name));
}

/** A run file's report. */
export function readRun(path) {
  return JSON.parse(readFileSync(path, 'utf8'));
}

/** Writes `text` to `path` through a temporary file in the same directory, so a reader sees the old file or the new one, never half of it. */
export function writeAtomic(path, text) {
  const tmp = join(dirname(path), `.${basename(path)}.${process.pid}.tmp`);
  try {
    writeFileSync(tmp, text);
    renameSync(tmp, path);
  } catch (error) {
    rmSync(tmp, { force: true });
    throw error;
  }
}

/** Deletes every run of the kind except the newest `keep`. Returns the paths deleted. */
export function prune(kind, options = {}) {
  const keep = options.keep ?? keepCount(options.env ?? process.env);
  const removed = list(kind, options).slice(keep);
  for (const path of removed) rmSync(path, { force: true });
  return removed;
}

/**
 * Writes `report` as the newest run of `kind` (`{kind, recordedAt, ...report}`), then prunes the kind.
 * Returns the path written. `options`: `root`, `keep`, `env` and `now` (a Date), for tests.
 */
export function record(kind, report, options = {}) {
  const dir = runsDir(kind, options.root ?? DEFAULT_ROOT);
  mkdirSync(dir, { recursive: true });
  const now = options.now ?? new Date();
  const stamp = stampOf(now);
  let name = `${stamp}.json`;
  for (let n = 1; existsSync(join(dir, name)); n += 1) name = `${stamp}-${n}.json`;
  const path = join(dir, name);
  writeAtomic(path, `${JSON.stringify({ kind, recordedAt: now.toISOString(), ...report }, null, 2)}\n`);
  prune(kind, options);
  return path;
}

/** The commit checked out under `root` and whether its tree has changes, or nulls when git is not there. */
export function gitState(root = DEFAULT_ROOT) {
  const head = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  if (head.status !== 0) return { commit: null, dirty: null };
  const status = spawnSync('git', ['status', '--porcelain'], { cwd: root, encoding: 'utf8' });
  return { commit: head.stdout.trim(), dirty: status.status === 0 ? status.stdout.trim() !== '' : null };
}

/**
 * The `tests` report of one test run: `log` is the text of `cargo test` (its `Running` lines and results),
 * `graph` the workspace from readWorkspace (it names the package of each binary). Per binary: package, target
 * (`lib`, `bin` or the integration test name), label, seconds, status, passed, failed. Per test: package, binary,
 * name, seconds (null without --report-time), ok. `extra` is merged in (commit, flags, steps, ...).
 */
export function testsReport(log, graph, extra = {}) {
  const rows = parseCargoLog(log);
  const binaries = rows.map((row) => ({
    package: packageOf(row, graph),
    target: row.name,
    label: row.label,
    seconds: row.seconds,
    status: row.status,
    passed: row.passed,
    failed: row.failed,
  }));
  const cases = rows.flatMap((row) =>
    row.cases.map((c) => ({ package: packageOf(row, graph), binary: row.name, name: c.name, seconds: c.seconds, ok: c.ok })),
  );
  return { ...extra, binaries, cases, perTestTimes: cases.some((c) => c.seconds !== null) };
}

// The package of a parsed binary: unit tests by their crate name (the executable's), integration tests by
// the package that names the test target.
function packageOf(row, graph) {
  if (row.name === 'lib' || row.name === 'bin' || row.name === 'doc') {
    const crate = row.label.split(' ')[0];
    return graph.packages.find((p) => p.name.replace(/-/g, '_') === crate)?.name ?? null;
  }
  return graph.packages.find((p) => p.tests.includes(row.name))?.name ?? null;
}

/**
 * Each current row with its time in the previous run: `key` names a row, `seconds` reads its time.
 * `delta` is the change in seconds (null when the previous run has no such row); `regressed` is true when
 * the time grew by more than REGRESSION_THRESHOLD of the previous time.
 */
export function compareTimes(current, previous, { key, seconds, threshold = REGRESSION_THRESHOLD }) {
  const before = new Map(previous.map((row) => [key(row), seconds(row)]));
  return current.map((row) => {
    const now = seconds(row);
    const was = before.has(key(row)) ? before.get(key(row)) : null;
    const known = now !== null && was !== null;
    return {
      key: key(row),
      seconds: now,
      previous: was,
      delta: known ? round3(now - was) : null,
      regressed: known && was > 0 && (now - was) / was > threshold,
    };
  });
}

const round3 = (n) => Math.round(n * 1000) / 1000;

/**
 * The summary `runs.mjs` prints, for the newest run of `kind` and the one before it (`previous`, or null).
 * gate: the steps (name, seconds, checks) slowest first, and the total. tests: the slowest 20 binaries and
 * the slowest 20 tests (only those with a time, so a run without --report-time has none).
 */
export function summarize(kind, current, previous, top = 20) {
  if (kind === 'gate') {
    const steps = compareTimes(current.steps ?? [], previous?.steps ?? [], {
      key: (s) => s.name,
      seconds: (s) => (typeof s.seconds === 'number' ? s.seconds : null),
    }).map((row, i) => ({ name: row.key, seconds: row.seconds, previous: row.previous, delta: row.delta, regressed: row.regressed, ok: current.steps[i].ok, checks: current.steps[i].checks ?? [] }));
    steps.sort((a, b) => (b.seconds ?? 0) - (a.seconds ?? 0));
    const total = compareTimes([{ totalSeconds: current.totalSeconds }], previous ? [{ totalSeconds: previous.totalSeconds }] : [], {
      key: () => 'total',
      seconds: (r) => r.totalSeconds,
    })[0];
    return { kind, steps, total: { seconds: total.seconds, previous: total.previous, delta: total.delta, regressed: total.regressed } };
  }
  if (kind === 'tests') {
    const binaryKey = (b) => `${b.package ?? '?'}:${b.target}`;
    const binaries = compareTimes(current.binaries ?? [], previous?.binaries ?? [], { key: binaryKey, seconds: (b) => b.seconds })
      .sort((a, b) => (b.seconds ?? 0) - (a.seconds ?? 0))
      .slice(0, top);
    const caseKey = (c) => `${c.package ?? '?'}:${c.binary}::${c.name}`;
    const timed = (current.cases ?? []).filter((c) => c.seconds !== null);
    const cases = compareTimes(timed, previous?.cases ?? [], { key: caseKey, seconds: (c) => c.seconds })
      .sort((a, b) => (b.seconds ?? 0) - (a.seconds ?? 0))
      .slice(0, top);
    return { kind, perTestTimes: Boolean(current.perTestTimes), binaries, cases };
  }
  throw new Error(`no summary for the kind ${JSON.stringify(kind)}`);
}
