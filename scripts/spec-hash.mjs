// The specification's semantic hash and the saved Exegra traces that carry it.
//
// A generated file records the semantic hash of the specification it came from in its header, and a
// saved trace records it as `semanticHash` beside Exegra's own fields, so neither can go stale
// unnoticed: the gate (scripts/spec-gate.mjs) compares both with what Exegra says now.
//
//   node scripts/record-trace.mjs <scenario> --project <id> --specs <dir> --out <file>
//
// writes a saved trace. This module is shared by that script, `regenerate.mjs` and the gate.
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';

/**
 * The product specifications layered on the platform specification (WG-4989): every *.jsonc in
 * specifications/products, in name order, as specc arguments (`--product <file>` each, relative to
 * the root). specc checks them against the platform and generates nothing from them, so the files
 * it writes stay the platform's alone. None while the directory does not exist.
 */
export function productArgs(root) {
  const dir = join(root, 'specifications', 'products');
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith('.jsonc'))
    .sort()
    .flatMap((name) => ['--product', `specifications/products/${name}`]);
}

/** The longest an Exegra call may run before it is killed and counted as failed. */
export const EXEGRA_TIMEOUT_MS = Number(process.env.STRATA_EXEGRA_TIMEOUT_MS ?? 180_000);

/**
 * Runs the Exegra CLI. `envelope` is its parsed JSON output or null when it printed none;
 * `problem` says why there is none (did not start, timed out, killed, or its own last line).
 */
export function exegraRun(exegraHome, args) {
  const cli = resolve(exegraHome, 'cli/exegra.js');
  const done = spawnSync(process.execPath, [cli, ...args], {
    cwd: exegraHome,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
    timeout: EXEGRA_TIMEOUT_MS,
  });
  let problem = null;
  if (done.error?.code === 'ETIMEDOUT') problem = `timed out after ${EXEGRA_TIMEOUT_MS / 1000}s`;
  else if (done.error) problem = `did not start: ${done.error.message}`;
  else if (done.signal) problem = `killed by ${done.signal}`;
  let envelope = null;
  try {
    envelope = JSON.parse(done.stdout);
  } catch {
    envelope = null;
    problem ??= `printed no JSON (exit ${done.status}): ${(done.stderr ?? '').trim().split('\n').pop() ?? ''}`;
  }
  return { envelope, problem };
}

/** Runs the Exegra CLI and returns its parsed JSON envelope, or null when it printed none or timed out. */
export function exegraJson(exegraHome, args) {
  return exegraRun(exegraHome, args).envelope;
}

/** Up to five of the validation errors (or the error message) of an Exegra envelope, for a message. */
export function envelopeProblems(envelope) {
  const lines = [];
  if (envelope?.error) lines.push(typeof envelope.error === 'string' ? envelope.error : JSON.stringify(envelope.error));
  const result = envelope?.result;
  const errors = Array.isArray(result?.errors) ? result.errors : [];
  const diagnostics = Array.isArray(result?.diagnostics) ? result.diagnostics.filter((d) => d.severity === 'error') : [];
  for (const d of (errors.length ? errors : diagnostics).slice(0, 5)) {
    lines.push(typeof d === 'string' ? d : `${d.code ?? ''} ${d.entityId ?? ''} ${d.message ?? JSON.stringify(d)}`.trim());
  }
  return lines;
}

/**
 * The semantic hash `exegra validate` reports for a project, and when there is none, why not:
 * the validation errors Exegra printed, or what went wrong running it.
 * @returns {{ hash: string | null, reason: string | null }}
 */
export function semanticHashDetail(exegraHome, project, specs) {
  const args = ['validate', '--project', project, ...(specs ? ['--specs', specs] : [])];
  const { envelope, problem } = exegraRun(exegraHome, args);
  const hash = envelope?.semanticHash;
  if (typeof hash === 'string' && hash.length > 0) return { hash, reason: null };
  const lines = envelopeProblems(envelope);
  const reason = lines.length ? `Exegra validation failed: ${lines.join('; ')}` : problem ?? 'Exegra reported no semanticHash (set EXEGRA_HOME and EXEGRA_PROJECT)';
  return { hash: null, reason };
}

/** The semantic hash `exegra validate` reports for a project, or null. */
export function semanticHash(exegraHome, project, specs) {
  return semanticHashDetail(exegraHome, project, specs).hash;
}

/** Sorts object keys recursively, the form Exegra writes saved traces in. */
export function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map((k) => [k, sorted(value[k])]));
  return value;
}

/** Runs a scenario and returns the saved form of its trace: Exegra's `result` plus `semanticHash`. */
export function recordedTrace(exegraHome, scenario, project, specs) {
  const envelope = exegraJson(exegraHome, ['trace', scenario, '--project', project, '--specs', specs]);
  if (!envelope?.ok || typeof envelope.semanticHash !== 'string') {
    throw new Error(`exegra trace ${scenario} (project ${project}) gave no trace: ${JSON.stringify(envelope?.error ?? envelope)}`);
  }
  return sorted({ ...envelope.result, semanticHash: envelope.semanticHash });
}

/** Every saved trace under the crates (`*.trace.json`), as absolute paths. */
export function savedTraces(root) {
  const found = [];
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      if (name === 'node_modules' || name === 'target') continue;
      const path = join(dir, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (name.endsWith('.trace.json')) found.push(path);
    }
  };
  if (existsSync(join(root, 'crates'))) walk(join(root, 'crates'));
  return found.sort();
}

/**
 * Whether a saved trace is current. Its project is the directory it sits in and its specs
 * directory the parent of that (the layout of tests/fixtures/<project>/). It is current when it
 * names the semantic hash Exegra reports now and equals the trace Exegra runs now.
 * @returns {{ ok: boolean, detail: string }}
 */
export function traceFreshness(exegraHome, root, file) {
  const saved = JSON.parse(readFileSync(file, 'utf8'));
  const project = basename(dirname(file));
  const specs = dirname(dirname(file));
  const name = relative(root, file).split(sep).join('/');
  if (typeof saved.semanticHash !== 'string') return { ok: false, detail: `${name} records no semanticHash; re-record it (node scripts/record-trace.mjs)` };
  let now;
  try {
    now = recordedTrace(exegraHome, saved.scenarioId, project, specs);
  } catch (error) {
    return { ok: false, detail: `${name}: ${error.message}` };
  }
  if (now.semanticHash !== saved.semanticHash) {
    return { ok: false, detail: `${name} was recorded at specification ${saved.semanticHash}, Exegra reports ${now.semanticHash}; re-record it` };
  }
  if (JSON.stringify(now) !== JSON.stringify(sorted(saved))) {
    return { ok: false, detail: `${name} differs from the trace Exegra runs now at the same hash (the simulator changed); re-record it` };
  }
  return { ok: true, detail: `${name} at ${saved.semanticHash}` };
}
