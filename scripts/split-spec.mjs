// Splits specifications/spec.jsonc into a manifest and part files by the tool that consumes each
// section (WG-5386; the table is in specifications/README.md), and an area part for each area that
// AREA_PARTS names: the items of every section whose id has the area's prefix (WG-5415).
//
//   node scripts/split-spec.mjs                     split specifications/spec.jsonc in place
//   node scripts/split-spec.mjs --spec <file>       split another file (its parts go beside it)
//   node scripts/split-spec.mjs --check <monolith>  confirm the parts on disk reassemble to the
//                                                   monolithic file, section by section
//   node scripts/split-spec.mjs --verify            exit 1, writing nothing, when an item is not in the
//                                                   part the split places it in (an area item edited
//                                                   into a consumer part by Exegra's apply, say)
//
// The input may be a monolithic spec.jsonc or a manifest: a manifest is resolved first, so the script
// can be run again on its own output (the files come out identical) and again on a monolithic file
// that another branch has edited. Item text is copied verbatim; the file's opening whole-line `//`
// comments stay in the manifest.
//
// Order. The semantic hash (Exegra) ignores the order of entities within a section, and specc's
// output does not depend on it either, so splitting `claims` in two parts changes nothing they
// compute. The check below still requires each part to keep the monolith's relative order.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MANIFEST_KEY, PLACEMENT_KEY, SpecPartsError, assembleParts, isManifest, readSpec, scanTopLevel } from './spec-parts.mjs';
import { parseJsonc } from './jsonc.mjs';

/** The parts, in manifest order, with what each is for. */
export const PARTS = [
  ['intent.jsonc', 'what the system is and why: the white paper, decisions, roadmap, glossary and document layout (people, and Concordance rendering the documents)'],
  ['boundary.jsonc', 'the model specc generates from: types, records, contracts, capabilities, quotas, components and the interface'],
  ['behaviour.jsonc', 'what Exegra runs: events, behaviours, faults, scenarios, failure models and the claims it evaluates itself (m-...)'],
  ['requirements.jsonc', 'the requirements the gate reconciles with the code and the recorded tests, and their case tables'],
  ['functions.jsonc', 'the transcribed function layer and its example fixtures'],
  ['frontend.jsonc', 'the browser client: its decisions, the copies it keeps, and the requirements its model is held to (an area part: items whose id starts fe-, d-fe- or copy-browser-)'],
  ['design-system.jsonc', 'the design system and agentic composition: its decisions and the requirements its models are held to (an area part: items whose id is ds- and a number, or starts d-ds-; docs/design-system.md)'],
];

const intent = 'intent.jsonc';
const boundary = 'boundary.jsonc';
const behaviour = 'behaviour.jsonc';
const requirements = 'requirements.jsonc';
const functions = 'functions.jsonc';

/** The part each top-level section is in. `claims` is split by the rule in `partOfItem`. */
export const SECTION_PART = {
  system: intent, exegra: intent, vocabulary: intent, goals: intent, nonGoals: intent, boundaries: intent,
  decisions: intent, constraints: intent, openQuestions: intent, assumptions: intent, releaseStages: intent,
  milestones: intent, terms: intent, documents: intent, sections: intent, views: intent, environments: intent,

  types: boundary, records: boundary, copies: boundary, contracts: boundary, capabilities: boundary, roles: boundary,
  quotas: boundary, retentions: boundary, plans: boundary, grants: boundary, delegations: boundary,
  tokenScopes: boundary, featureFlags: boundary, validations: boundary, languages: boundary, components: boundary,
  relationships: boundary, screens: boundary, forms: boundary, actions: boundary, liveBindings: boundary,
  errorMessages: boundary,

  events: behaviour, behaviors: behaviour, faults: behaviour, scenarios: behaviour, uiScenarios: behaviour,
  failureModes: behaviour, faultModels: behaviour,

  caseTables: requirements,

  functions, exampleFixtures: functions,
};

/**
 * The area parts: an item of any array section whose id matches goes to the area's part, whatever
 * its section. An area part reads as one place for the area; the tools read the resolved manifest,
 * so it changes nothing they compute. Exegra's `apply` places a new item by `specSections`, that is
 * by consumer; move it with this script afterwards.
 */
export const AREA_PARTS = [
  ['frontend.jsonc', /^(fe-|d-fe-|copy-browser-)/],
  ['design-system.jsonc', /^(ds-\d|d-ds-)/],
];

/** The area part an item belongs to, or null. */
export function areaPart(item) {
  const id = item?.id;
  if (typeof id !== 'string') return null;
  const hit = AREA_PARTS.find(([, pattern]) => pattern.test(id));
  return hit ? hit[0] : null;
}

/** The part of one item of an array section: its area part, else the part of its section (by kind, for `claims`). */
export function itemPart(key, item) {
  return areaPart(item) ?? (key === 'claims' ? claimPart(item) : SECTION_PART[key]);
}

/** Where `claims` go, in the manifest's placement table: by the item's `kind`, as `claimPart` decides. */
export const CLAIMS_PLACEMENT = { by: 'kind', values: { requirement: requirements }, default: behaviour };

/** The part of one item of `claims`: a requirement goes to requirements, every other claim (m-...) to behaviour. */
export function claimPart(claim) {
  return claim?.kind === 'requirement' ? requirements : behaviour;
}

const PART_HEADER = (name, what) =>
  `// ${name}: ${what}.\n// A part of the Strata specification; spec.jsonc lists the parts. Generated by scripts/split-spec.mjs\n// the first time, edited in place after that. See specifications/README.md.\n`;

/**
 * Splits resolved specification text. Returns the manifest text and each part's text.
 * @returns {{manifest: string, parts: Map<string, string>}}
 */
export function splitText(text, header) {
  const top = scanTopLevel(text, 'the specification');
  /** @type {Map<string, string[]>} */
  const bySection = new Map(PARTS.map(([name]) => [name, []]));
  for (const m of top.members) {
    const body = top.src;
    if (m.key === MANIFEST_KEY) throw new SpecPartsError('nested-manifest', 'the input is itself a manifest after resolving');
    const render = (items) => (items.length ? `  ${JSON.stringify(m.key)}: [\n${items.map((t) => `    ${t}`).join(',\n')}\n  ]` : `  ${JSON.stringify(m.key)}: []`);
    const part = m.key === 'claims' ? null : SECTION_PART[m.key];
    if (m.key !== 'claims' && !part) {
      throw new SpecPartsError('unassigned-section', `the section "${m.key}" is in no part; add it to SECTION_PART in scripts/split-spec.mjs and to specifications/README.md`);
    }
    if (m.items === null) {
      bySection.get(part).push(`  ${JSON.stringify(m.key)}: ${body.slice(m.start, m.end)}`);
      continue;
    }
    // An array section: each item to its part. The section's own part keeps it even when empty.
    const items = new Map(PARTS.map(([name]) => [name, []]));
    for (const item of m.items) {
      const t = body.slice(item.start, item.end);
      items.get(itemPart(m.key, parseJsonc(t))).push(t);
    }
    for (const [name, list] of items) if (list.length || name === part) bySection.get(name).push(render(list));
  }
  const parts = new Map();
  for (const [name, what] of PARTS) {
    parts.set(name, `${PART_HEADER(name, what)}{\n${bySection.get(name).join(',\n')}\n}\n`);
  }
  const list = PARTS.map(([name]) => `    ${JSON.stringify(name)}`).join(',\n');
  const placement = [...Object.entries(SECTION_PART), ['claims', CLAIMS_PLACEMENT]]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([section, to]) => `    ${JSON.stringify(section)}: ${JSON.stringify(to)}`)
    .join(',\n');
  return {
    manifest: `${header}{\n  ${JSON.stringify(MANIFEST_KEY)}: [\n${list}\n  ],\n  ${JSON.stringify(PLACEMENT_KEY)}: {\n${placement}\n  }\n}\n`,
    parts,
  };
}

/** The `//` comment lines that open a file, as written. */
export function openingComments(text) {
  const lines = text.replace(/^﻿/, '').split('\n');
  const out = [];
  for (const line of lines) {
    if (!/^\s*\/\//.test(line)) break;
    out.push(line);
  }
  return out.length ? `${out.join('\n')}\n` : '';
}

/** Splits the file at `specPath` in place. @returns {{written: string[]}} */
export function splitFile(specPath, { dryRun = false } = {}) {
  const path = resolve(specPath);
  const own = readFileSync(path, 'utf8');
  const { text } = readSpec(path);
  const { manifest, parts } = splitText(text, openingComments(own));
  const dir = dirname(path);
  const written = [];
  const put = (file, content) => {
    const prior = existsSync(file) ? readFileSync(file, 'utf8') : null;
    if (prior === content) return;
    written.push(file);
    if (dryRun) return;
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, content);
  };
  for (const [name, content] of parts) put(join(dir, name), content);
  put(path, manifest);
  return { written };
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Compares the specification at `specPath` (resolved) with a monolithic file. Equal when both have the
 * same sections, and an array section has the same items with each part's items in the monolith's
 * relative order (a section is in several parts when it holds claims of both kinds or items of an area).
 * @returns {string[]} the differences, none when equal
 */
export function checkAgainst(specPath, monolithPath) {
  const problems = [];
  const resolved = parseJsonc(readSpec(specPath).text);
  const original = parseJsonc(readFileSync(resolve(monolithPath), 'utf8'));
  const a = Object.keys(resolved).sort();
  const b = Object.keys(original).sort();
  if (!same(a, b)) problems.push(`sections differ: only in parts [${a.filter((k) => !b.includes(k))}], only in the monolith [${b.filter((k) => !a.includes(k))}]`);
  for (const key of b.filter((k) => a.includes(k))) {
    const x = resolved[key];
    const y = original[key];
    if (Array.isArray(x) && Array.isArray(y)) {
      const sorted = (list) => list.map((v) => JSON.stringify(v)).sort();
      if (!same(sorted(x), sorted(y))) problems.push(`${key}: the items differ`);
      for (const [name] of PARTS) {
        const sel = (list) => list.filter((c) => itemPart(key, c) === name).map((v) => JSON.stringify(v));
        if (!same(sel(x), sel(y))) problems.push(`${key}: the order of the items in ${name} differs`);
      }
    } else if (!same(x, y)) {
      problems.push(`${key}: differs`);
    }
  }
  return problems;
}

function main(argv) {
  const flag = (name) => (argv.includes(name) ? argv[argv.indexOf(name) + 1] : undefined);
  const spec = resolve(flag('--spec') ?? join(dirname(fileURLToPath(import.meta.url)), '..', 'specifications', 'spec.jsonc'));
  const monolith = flag('--check');
  if (monolith) {
    if (!isManifest(readFileSync(spec, 'utf8'))) throw new Error(`${spec} is not a manifest`);
    const problems = checkAgainst(spec, monolith);
    for (const p of problems) console.log(`FAIL  ${p}`);
    console.log(problems.length ? `split-spec: ${problems.length} difference(s)` : 'split-spec: the parts reassemble to the monolithic file');
    return problems.length ? 1 : 0;
  }
  if (argv.includes('--verify')) {
    const { written } = splitFile(spec, { dryRun: true });
    for (const file of written) console.log(`FAIL  ${file} is not where the split places it; run node scripts/split-spec.mjs`);
    console.log(written.length ? `split-spec: ${written.length} file(s) would change` : 'split-spec: every item is in its part');
    return written.length ? 1 : 0;
  }
  const { written } = splitFile(spec);
  console.log(written.length ? `split-spec: wrote ${written.join(', ')}` : 'split-spec: nothing to change');
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (e) {
    console.error(`split-spec: ${e.message}`);
    process.exitCode = 2;
  }
}
