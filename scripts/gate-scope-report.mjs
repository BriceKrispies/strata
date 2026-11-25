// The scope report: how many requirements each scope holds, and how many of them have a passing proof.
//
// A scope is a set of requirements held by the gate by ref prefix and ref rather than by milestone. Two files list them:
// gate.json (`startedScopes`, the scopes the gate holds now) and docs/plinth/gate-scopes.json (`scopes`, proposed and not
// started). Each scope is judged by the evidence module (scripts/gate-evidence.mjs), against the last recorded test
// results in var/test-results: the same proof index and the same rules the gate uses.
//
//   node scripts/gate-scope-report.mjs [--scope NAME] [--json] [--root <dir>] [--results-dir <dir>]
//
// For each scope the report prints the matched, proven and unproven counts. Unproven requirements are grouped by area
// and section, and each group names the test files that already carry `Verifies:` lines for refs of the same prefix:
// the places a backfilling agent should add tests. Without recorded results every matched requirement is unproven, and
// the report says so.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collectEvidence, collectRefs, evaluateEvidence, normaliseScopes, readEvidenceResults, readGateConfig, scopeMatches, scopeProblems } from './gate-evidence.mjs';
import { readSpecValue } from './spec-parts.mjs';

/** The prefix of a ref: `LDG-` for `LDG-001`, `A-B-` for `A-B-12.3`. */
export function refPrefix(ref) {
  return /^(.*?-)\d/.exec(String(ref).toUpperCase())?.[1] ?? String(ref).toUpperCase();
}

/**
 * The report of every scope: the scopes of gate.json (origin `started`) and of docs/plinth/gate-scopes.json (origin
 * `proposed`), read under `root`. `only` limits it to the scope of that name. Returns
 * `{resultsRecorded, resultsDir, scopes: [{name, origin, problems, matched, proven, unproven, groups}]}`, where each
 * group is `{area, section, refs: [{ref, id, milestone, reason}], prefixes: {prefix: [test files]}}`.
 */
export function buildScopeReport({ root, resultsDir = join(root, 'var/test-results'), only = null }) {
  const spec = readSpecValue(join(root, 'specifications/spec.jsonc'));
  const started = readGateConfig(root).startedScopes ?? [];
  const proposedPath = join(root, 'docs/plinth/gate-scopes.json');
  const proposed = existsSync(proposedPath) ? JSON.parse(readFileSync(proposedPath, 'utf8')).scopes ?? [] : [];
  const verdictsPath = join(root, 'var/exegra-verdicts.json');
  const verdicts = existsSync(verdictsPath) ? JSON.parse(readFileSync(verdictsPath, 'utf8')).results ?? null : null;

  const claims = spec.claims ?? [];
  const requirements = claims.filter((c) => c.kind === 'requirement');
  const documented = new Set(requirements.filter((c) => c.check === 'documentation').map((c) => c.id));
  const byId = new Map(requirements.map((c) => [c.id, c]));
  const evidence = collectEvidence(root);
  const results = readEvidenceResults(resultsDir);
  const filesByPrefix = new Map();
  for (const site of [...evidence.rustTests, ...evidence.leanTheorems]) {
    for (const ref of site.refs) {
      const prefix = refPrefix(ref);
      if (!filesByPrefix.has(prefix)) filesByPrefix.set(prefix, new Set());
      filesByPrefix.get(prefix).add(site.file);
    }
  }

  const declared = [
    ...started.map((scope) => ({ scope, origin: 'started' })),
    ...proposed.map((scope) => ({ scope, origin: 'proposed' })),
  ].filter(({ scope }) => only === null || scope?.name === only);

  const scopes = declared.map(({ scope, origin }) => {
    const problems = scopeProblems([scope], requirements);
    const normal = normaliseScopes([scope])[0];
    const matched = normal ? requirements.filter((r) => !documented.has(r.id) && scopeMatches(normal, r)) : [];
    const matchedIds = new Set(matched.map((r) => r.id));
    const judged = evaluateEvidence({
      requirements,
      knownRefs: collectRefs(spec),
      modelClaims: claims.filter((c) => String(c.id).startsWith('m-')),
      verdicts,
      rustTests: evidence.rustTests,
      rustResults: results.rustResults,
      leanTheorems: evidence.leanTheorems,
      lake: results.lake,
      started: [],
      scopes: [scope],
    });
    const unproven = judged.unproven.filter((u) => matchedIds.has(u.id));
    const groups = new Map();
    for (const u of unproven) {
      const claim = byId.get(u.id);
      const key = `${claim?.area ?? '-'}\u0000${claim?.section ?? '-'}`;
      if (!groups.has(key)) groups.set(key, { area: claim?.area ?? '-', section: claim?.section ?? '-', refs: [], prefixes: {} });
      const group = groups.get(key);
      group.refs.push({ ref: u.ref, id: u.id, milestone: u.milestone, reason: u.reason });
      const prefix = refPrefix(u.ref ?? u.id);
      group.prefixes[prefix] = [...(filesByPrefix.get(prefix) ?? [])].sort();
    }
    return {
      name: scope?.name ?? '(unnamed)',
      origin,
      problems,
      matched: matched.length,
      proven: matched.filter((r) => judged.proven.some((p) => p.id === r.id)).length,
      unproven: unproven.length,
      groups: [...groups.values()],
    };
  });
  return { resultsRecorded: results.cargoText !== null || results.lakeText !== null, resultsDir, scopes };
}

/** The report as lines for a terminal. */
export function formatScopeReport(report) {
  const lines = [];
  if (!report.resultsRecorded) {
    lines.push(`no test results in ${report.resultsDir} (cargo-test.txt and lake-build.txt are missing): nothing is proven, every matched requirement is unproven`);
  }
  if (report.scopes.length === 0) lines.push('no scopes to report');
  for (const s of report.scopes) {
    lines.push(`scope ${s.name} (${s.origin}): ${s.matched} matched, ${s.proven} proven, ${s.unproven} unproven`);
    for (const p of s.problems) lines.push(`  scope problem: ${p}`);
    for (const g of s.groups) {
      lines.push(`  ${g.area} / ${g.section}: ${g.refs.length} unproven`);
      lines.push(`    refs: ${g.refs.map((r) => r.ref ?? r.id).join(', ')}`);
      for (const [prefix, files] of Object.entries(g.prefixes)) {
        lines.push(`    tests with Verifies lines for ${prefix}: ${files.length > 0 ? files.join(', ') : 'none yet'}`);
      }
    }
  }
  return lines;
}

// ---------------------------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------------------------
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const flag = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
  const root = resolve(flag('--root', resolve(dirname(fileURLToPath(import.meta.url)), '..')));
  const resultsDir = resolve(flag('--results-dir', join(root, 'var/test-results')));
  const only = args.includes('--scope') ? flag('--scope', null) : null;
  const report = buildScopeReport({ root, resultsDir, only });
  if (only !== null && report.scopes.length === 0) {
    console.error(`no scope named ${only} in gate.json or docs/plinth/gate-scopes.json`);
    process.exit(1);
  }
  if (args.includes('--json')) console.log(JSON.stringify(report, null, 2));
  else for (const line of formatScopeReport(report)) console.log(line);
}
