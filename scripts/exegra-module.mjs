// Exegra verification of a list of claims from one process (WG-4991, scripts/test-module.mjs).
//
//   node scripts/exegra-module.mjs --exegra <checkout> --spec <spec.jsonc> --claims <ids.json>
//
// Builds the specification once with Exegra's own modules (as scripts/exegra-traces.mjs does), fails
// when it has validation errors, then evaluates each claim of the JSON array of ids with
// `evaluateClaim` (the call `exegra evaluate <claim>` makes) and fails when any is violated or cannot
// be evaluated. Prints one JSON line: {"ok", "evaluated", "byStatus", "problems": [{"id", "status"|"error"}]}.
// Exegra is only read.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { readSpec } from './spec-parts.mjs';

/** The ids that fail: a result whose status is `violated`, or an id that is not a claim. */
export function problemsOf(results) {
  return results.filter((r) => r.status === 'violated' || r.error).map((r) => (r.error ? { id: r.id, error: r.error } : { id: r.id, status: r.status }));
}

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--') || argv[i + 1] === undefined) throw new Error(`bad argument ${argv[i]}`);
    out[argv[i].slice(2)] = argv[i + 1];
  }
  return out;
}

async function main() {
  const a = args(process.argv.slice(2));
  for (const k of ['exegra', 'spec', 'claims']) if (!a[k]) throw new Error(`--${k} is required`);
  const load = (rel) => import(pathToFileURL(path.join(a.exegra, rel)).href);
  const { Project } = await load('src/core/project.js');
  const { evaluateClaim } = await load('src/core/claims.js');
  const ids = JSON.parse(fs.readFileSync(a.claims, 'utf8'));
  const project = new Project({ name: null, specText: readSpec(a.spec).text, layoutText: null, layers: [] });
  if (!project.model) throw new Error('the specification has syntax errors');
  const errors = project.diagnostics.filter((d) => d.severity === 'error');
  if (errors.length) throw new Error(`the specification has ${errors.length} validation error(s): ${errors[0].code} ${errors[0].entityId ?? ''}`);
  const results = ids.map((id) => {
    try {
      return { id, status: evaluateClaim(project.model, id, {}).status };
    } catch (e) {
      return { id, error: e?.message ?? String(e) };
    }
  });
  const byStatus = {};
  for (const r of results) byStatus[r.status ?? 'error'] = (byStatus[r.status ?? 'error'] ?? 0) + 1;
  const problems = problemsOf(results);
  process.stdout.write(`${JSON.stringify({ ok: problems.length === 0, evaluated: results.length, byStatus, problems })}\n`);
  process.exitCode = problems.length === 0 ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((e) => {
    process.stdout.write(`${JSON.stringify({ ok: false, evaluated: 0, byStatus: {}, problems: [], error: e?.message ?? String(e) })}\n`);
    process.exitCode = 1;
  });
}
