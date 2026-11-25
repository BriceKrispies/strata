// Prints the latest run of a kind (scripts/run-history.mjs) with each time's change against the run before it.
//
//   node scripts/runs.mjs [gate|tests] [--json]
//
// gate: the steps of the latest gate run, slowest first, and the whole run's seconds.
// tests: the slowest 20 test binaries and the slowest 20 tests (a test has a time only when the run
// used --report-time; otherwise the table says so).
// Δ is the change in seconds against the run before; "REGRESSED" marks a time that grew by more than 20%.
import { pathToFileURL } from 'node:url';
import { DEFAULT_ROOT, list, readRun, REGRESSION_THRESHOLD, summarize } from './run-history.mjs';

const KINDS = ['gate', 'tests'];

const fmtSeconds = (n) => (n === null || n === undefined ? '-' : `${n.toFixed(1)} s`);
const fmtDelta = (d) => (d === null || d === undefined ? '-' : `${d >= 0 ? '+' : ''}${d.toFixed(1)} s`);
const flag = (row) => (row.regressed ? 'REGRESSED' : '');

// Columns: `right` aligns a column to the right; the last column is not padded.
function table(rows, columns) {
  const widths = columns.map((c) => Math.max(c.title.length, ...rows.map((r) => String(c.value(r)).length)));
  const line = (cells) => cells.map((cell, i) => (columns[i].right ? String(cell).padStart(widths[i]) : String(cell).padEnd(widths[i]))).join('  ').trimEnd();
  return [line(columns.map((c) => c.title)), ...rows.map((r) => line(columns.map((c) => c.value(r))))];
}

function printGate(summary, current, previousPath, file, out) {
  out(`gate run ${current.startedAt}  verdict ${current.verdict}  commit ${current.commit ? current.commit.slice(0, 12) : 'unknown'}${current.dirty ? ' (uncommitted changes)' : ''}`);
  out(`flags: ${(current.flags ?? []).join(' ') || 'none'}`);
  if (current.failures?.length) out(`failed: ${current.failures.join('; ')}`);
  out(previousPath ? `compared with the run before it (${previousPath})` : 'no earlier gate run to compare with');
  out(`file: ${file}`);
  out('');
  for (const line of table(summary.steps, [
    { title: 'seconds', right: true, value: (r) => fmtSeconds(r.seconds) },
    { title: 'Δ', right: true, value: (r) => fmtDelta(r.delta) },
    { title: 'step', value: (r) => r.name },
    { title: 'ok', value: (r) => (r.ok === null ? '-' : r.ok ? 'ok' : 'FAILED') },
    { title: '', value: flag },
  ])) out(line);
  out('');
  const t = summary.total;
  out(`total ${fmtSeconds(t.seconds)}  Δ ${fmtDelta(t.delta)}${t.regressed ? `  REGRESSED (over ${REGRESSION_THRESHOLD * 100}%)` : ''}`);
  const failing = summary.steps.flatMap((s) => (s.checks ?? []).filter((c) => !c.ok).map((c) => `${s.name}: ${c.name}`));
  if (failing.length) out(`failed checks: ${failing.length}\n  ${failing.join('\n  ')}`);
}

function printTests(summary, previousPath, file, out) {
  out(previousPath ? `compared with the run before it (${previousPath})` : 'no earlier test run to compare with');
  out(`file: ${file}`);
  out('');
  out('slowest test binaries');
  for (const line of table(summary.binaries, [
    { title: 'seconds', right: true, value: (r) => fmtSeconds(r.seconds) },
    { title: 'Δ', right: true, value: (r) => fmtDelta(r.delta) },
    { title: 'binary', value: (r) => r.key },
    { title: '', value: flag },
  ])) out(line);
  out('');
  if (!summary.perTestTimes) {
    out('slowest tests: none timed; this run recorded no per-test times (cargo test without --report-time)');
    return;
  }
  out('slowest tests');
  for (const line of table(summary.cases, [
    { title: 'seconds', right: true, value: (r) => fmtSeconds(r.seconds) },
    { title: 'Δ', right: true, value: (r) => fmtDelta(r.delta) },
    { title: 'test', value: (r) => r.key },
    { title: '', value: flag },
  ])) out(line);
}

/** `root` and `out` are for tests; the command uses the checkout and the console. Returns the exit code. */
export function main(argv, { root = DEFAULT_ROOT, out = console.log, err = console.error } = {}) {
  const json = argv.includes('--json');
  const positional = argv.filter((a) => !a.startsWith('--'));
  const kind = positional[0] ?? 'gate';
  if (positional.length > 1 || argv.some((a) => a.startsWith('--') && a !== '--json') || !KINDS.includes(kind)) {
    err('usage: node scripts/runs.mjs [gate|tests] [--json]');
    return 2;
  }
  const files = list(kind, { root });
  if (files.length === 0) {
    err(`no ${kind} run is recorded yet under var/runs/${kind}`);
    return 1;
  }
  const current = readRun(files[0]);
  const previousPath = files[1] ?? null;
  const previous = previousPath ? readRun(previousPath) : null;
  const summary = summarize(kind, current, previous);
  if (json) {
    out(JSON.stringify({ file: files[0], previous: previousPath, ...summary }, null, 2));
  } else if (kind === 'gate') {
    printGate(summary, current, previousPath, files[0], out);
  } else {
    printTests(summary, previousPath, files[0], out);
  }
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exitCode = main(process.argv.slice(2));
