// A requirement's realisation label, derived from the evidence present (WG-5029, from audit finding 25
// of reports/drift.md: 784 claims were labelled `checked` by hand and nothing looked at the evidence).
//
// The label of a requirement (`construction`, `checked`, `operational`) is a statement about what
// stands behind it. This module reads what stands behind it and derives the label from that:
//
//   operational   the requirement is evaluated by a budget (`evaluate.method` is `budget`): a number
//                 measured on a running deployment;
//   checked       a test or a journey names it: a `Verifies: <REF>` comment in the source of a test
//                 (the ref of the requirement, in any case), or a uiScenario whose `verifies` lists it;
//   construction  no test names it, and either a claim models it (`models`) or it names constructs
//                 in `realisedBy`: what guarantees it is the model and the generated code;
//   (none)        no evidence at all: the label is a declaration nothing supports.
//
// The report counts the declared labels against the derived ones and names every disagreement. Two
// kinds of disagreement fail the gate, because the evidence contradicts the label outright: a
// requirement declared `construction` with no model claim, construct or test behind it, and one
// declared anything but `operational` that a budget measures. The rest is reported, not failed:
// whether a requirement that a model claim restates but no test names is `construction` or
// `checked` is the owner's call (a declared `checked` with only a model behind it is listed as
// "declared checked, derived construction").
//
// Property claims (`evaluate.method` is `property`) are reported on a second count: those no test
// names, and those whose named tests use no generator (a property over one fixed input is an
// example). A test file is taken to use a generator when it holds a marker of one (proptest, a
// seeded random generator, a generator function).
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

export const LABELS = ['construction', 'checked', 'operational'];

// What marks a generator of inputs in a test file.
export const GENERATOR_MARKER = /\b(?:proptest!?|prop_compose!?|quickcheck|fast-check|fc\.(?:assert|property)|StdRng|SmallRng|seed_from_u64|xorshift|splitmix|Rng\s*\(|fn\s+(?:arb|gen|generate)_\w*|fixed-seed|seeded)/i;

/** The refs a source names in `Verifies:` comments, as a Map of lower-case ref to the paths that name it. */
export function verifiedRefs(sources) {
  const out = new Map();
  for (const { path, text } of sources) {
    for (const m of text.matchAll(/Verifies:\s*([A-Za-z0-9_, -]+)/g)) {
      for (const ref of m[1].split(/[,\s]+/).filter(Boolean)) {
        const key = ref.toLowerCase();
        if (!out.has(key)) out.set(key, new Set());
        out.get(key).add(path);
      }
    }
  }
  return out;
}

/** The evidence for one requirement: `{tests: [paths], journeys: [ids], models: [claim ids], realisedBy: [ids], budget}`. */
export function evidenceOf(claim, { refs, uiScenarios, claims }) {
  const key = String(claim.ref ?? '').toLowerCase();
  const tests = [...(refs.get(key) ?? []), ...(refs.get(String(claim.id).toLowerCase()) ?? [])];
  return {
    tests: [...new Set(tests)],
    journeys: uiScenarios.filter((u) => (u.verifies ?? []).includes(claim.id)).map((u) => u.id),
    models: claims.filter((c) => c.models === claim.id).map((c) => c.id),
    realisedBy: Array.isArray(claim.realisedBy) ? claim.realisedBy : [],
    budget: claim.evaluate?.method === 'budget',
  };
}

/** The label the evidence supports, or null when there is none. */
export function deriveLabel(evidence) {
  if (evidence.budget) return 'operational';
  if (evidence.tests.length > 0 || evidence.journeys.length > 0) return 'checked';
  if (evidence.models.length > 0 || evidence.realisedBy.length > 0) return 'construction';
  return null;
}

/**
 * The realisation report of a specification: for every requirement the declared and derived label, the
 * disagreements, the ones the evidence contradicts, and the property claims without a test or a
 * generator. `sources` are `{path, text}` of the files that can hold a `Verifies:` comment.
 */
export function realisationReport({ claims, uiScenarios = [], sources }) {
  const refs = verifiedRefs(sources);
  const textOf = new Map(sources.map((s) => [s.path, s.text]));
  const requirements = claims.filter((c) => c.kind === 'requirement');
  const declared = { construction: 0, checked: 0, operational: 0, missing: 0 };
  const derived = { construction: 0, checked: 0, operational: 0, none: 0 };
  const differ = [];
  const contradicted = [];
  for (const claim of requirements) {
    const evidence = evidenceOf(claim, { refs, uiScenarios, claims });
    const label = deriveLabel(evidence);
    declared[LABELS.includes(claim.realisation) ? claim.realisation : 'missing'] += 1;
    derived[label ?? 'none'] += 1;
    if (!LABELS.includes(claim.realisation)) {
      contradicted.push(`${claim.id} has no realisation label (derived: ${label ?? 'none'})`);
    } else if (claim.realisation === 'construction' && label === null) {
      contradicted.push(`${claim.id} is declared construction and no model claim, construct or test stands behind it`);
    } else if (evidence.budget && claim.realisation !== 'operational') {
      contradicted.push(`${claim.id} is declared ${claim.realisation} but a budget measures it (derived: operational)`);
    }
    if (label !== claim.realisation) differ.push({ id: claim.id, declared: claim.realisation ?? null, derived: label });
  }

  const properties = claims.filter((c) => c.evaluate?.method === 'property');
  const untested = [];
  const withoutGenerator = [];
  for (const claim of properties) {
    const { tests } = evidenceOf(claim, { refs, uiScenarios, claims });
    if (tests.length === 0) untested.push(claim.id);
    else if (!tests.some((path) => GENERATOR_MARKER.test(textOf.get(path) ?? ''))) withoutGenerator.push(claim.id);
  }
  return { requirements: requirements.length, declared, derived, differ, contradicted, properties: { total: properties.length, untested, withoutGenerator } };
}

function sample(ids, n = 8) {
  return ids.slice(0, n).join(', ') + (ids.length > n ? `, and ${ids.length - n} more` : '');
}

/** The gate verdict `{name, ok, detail, lines}` of a report. */
export function realisationVerdict(report) {
  const { declared, derived, differ, contradicted, properties } = report;
  const lines = [
    `declared: construction ${declared.construction}, checked ${declared.checked}, operational ${declared.operational}, missing ${declared.missing}`,
    `derived from evidence: construction ${derived.construction}, checked ${derived.checked}, operational ${derived.operational}, none ${derived.none}`,
  ];
  const groups = new Map();
  for (const d of differ) {
    const key = `declared ${d.declared}, derived ${d.derived ?? 'none (no evidence)'}`;
    groups.set(key, [...(groups.get(key) ?? []), d.id]);
  }
  for (const [key, ids] of [...groups].sort((a, b) => b[1].length - a[1].length)) lines.push(`${ids.length} requirement(s) ${key}: ${sample(ids)}`);
  lines.push(`property claims: ${properties.total}; ${properties.untested.length} no test names; ${properties.withoutGenerator.length} named only by tests with no generator: ${sample(properties.withoutGenerator)}`);
  for (const c of contradicted) lines.push(`contradicted: ${c}`);
  return {
    name: 'every requirement realisation label is supported by the evidence (derived from tests, models and budgets)',
    ok: contradicted.length === 0,
    detail: contradicted.length === 0 ? `${report.requirements} requirements, ${report.requirements - differ.length} agree with the derived label, ${differ.length} reported` : `${contradicted.length} contradicted by the evidence`,
    lines,
  };
}

const SKIP = new Set(['node_modules', 'target', '.git', 'generated', 'step-checks', 'var', 'dist']);

function walk(dir, out) {
  for (const entry of readdirSync(dir)) {
    if (SKIP.has(entry)) continue;
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, out);
    else if (/\.(rs|js|mjs)$/.test(entry)) out.push(path);
  }
}

/** The files that can hold a `Verifies:` comment: the Rust crates, the web and browser tests, the scripts' tests. */
export function evidenceSources(root) {
  const files = [];
  for (const dir of ['crates', 'web/test', 'tests/e2e']) if (existsSync(join(root, dir))) walk(join(root, dir), files);
  return files.map((file) => ({ path: file.slice(root.length + 1).split('\\').join('/'), text: readFileSync(file, 'utf8') }));
}
