// Moves repeated function-example arguments into shared example fixtures (WG-4963).
//
//   node scripts/example-fixtures.mjs [--spec <spec.jsonc>] [--ops <out.json>] [--min-bytes <n>]
//
// Exegra's `exampleFixtures` section (docs/model-and-format.md, "Example fixtures") holds a value
// once; an example argument refers to it as `{"fixture": id}` or `{"fixture": id, "with": {path: value}}`.
// This script reads the specification, finds the example arguments of at least `--min-bytes` bytes (default
// 200) and:
//
//   1. makes a fixture of every value that two or more arguments of the same parameter type repeat;
//   2. makes an argument that differs from a fixture of its type by a few values a reference with
//      overrides, when the reference is under 60 percent of the argument's size;
//   3. leaves every other argument written out.
//
// It writes the changes as an operations file (never the specification): a `CreateEntity` of kind
// `exampleFixture` per fixture and one `SetProperty` per changed argument, at `examples.<index>.args.<param>`
// of its function, so that it does not replace the lists of examples other changes add to. It prints the
// size of the function examples before and after, and the number of fixtures and references.
// Apply the file with fast-apply (see C:/dev/audit-wave/RULES.md).
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseJsonc } from './jsonc.mjs';

export const MIN_BYTES = 200;
const REF_OVERHEAD = 40;

const isObject = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/** JSON text with object keys sorted, so two equal values have one text. */
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (isObject(value)) return `{${Object.keys(value).sort().map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`;
  return JSON.stringify(value);
}

/**
 * The overrides that turn `base` into `value`, as `[[dotted path, value]]`, or null when they cannot be
 * written: a path step is a struct field, a map key, a list index or a variant name, a key with a dot
 * cannot be named (the parent is replaced whole instead), and a key the base has that the value lacks
 * cannot be removed (its object is replaced whole). The root cannot be replaced.
 */
export function overrides(base, value, path = []) {
  if (canonical(base) === canonical(value)) return [];
  const whole = () => (path.length === 0 ? null : [[path.join('.'), value]]);
  if (isObject(base) && isObject(value) && Object.keys(base).every((k) => k in value)) {
    if (Object.keys(value).some((k) => k === '' || k.includes('.'))) return whole();
    const out = [];
    for (const k of Object.keys(value)) {
      if (!(k in base)) out.push([[...path, k].join('.'), value[k]]);
      else {
        const inner = overrides(base[k], value[k], [...path, k]);
        if (inner === null) return null;
        out.push(...inner);
      }
    }
    return out;
  }
  if (Array.isArray(base) && Array.isArray(value) && base.length === value.length) {
    const out = [];
    for (let i = 0; i < value.length; i += 1) {
      const inner = overrides(base[i], value[i], [...path, String(i)]);
      if (inner === null) return null;
      out.push(...inner);
    }
    return out;
  }
  return whole();
}

/** Every example argument of every function: `{fn, index, param, type, value, size, key}`. */
export function exampleArgs(spec) {
  const out = [];
  for (const fn of spec.functions ?? []) {
    const types = Object.fromEntries((fn.params ?? []).map((p) => [p.name, p.type]));
    (fn.examples ?? []).forEach((example, index) => {
      for (const [param, value] of Object.entries(example.args ?? {})) {
        if (isObject(value) && 'fixture' in value) continue; // already a reference
        const text = canonical(value);
        out.push({ fn: fn.id, index, param, type: types[param], value, size: JSON.stringify(value).length, key: text });
      }
    });
  }
  return out;
}

/** The bytes of the function examples of a specification, and of its `exampleFixtures`. */
export function exampleBytes(spec) {
  const examples = (spec.functions ?? []).reduce((s, f) => s + JSON.stringify(f.examples ?? []).length, 0);
  const fixtures = JSON.stringify(spec.exampleFixtures ?? []).length;
  return { examples, fixtures, total: examples + fixtures };
}

const slug = (type) => String(type).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

/**
 * The plan: `{fixtures: [{id, type, value, description, uses}], refs: [{fn, index, param, ref}], before, after}`.
 * Deterministic: fixtures are numbered per type in the order of the first argument that uses them.
 */
export function planFixtures(spec, { minBytes = MIN_BYTES } = {}) {
  const args = exampleArgs(spec).filter((a) => a.type && a.size >= minBytes);
  const groups = new Map();
  for (const a of args) {
    const k = `${a.type}\u0000${a.key}`;
    groups.set(k, [...(groups.get(k) ?? []), a]);
  }
  const fixtures = [];
  const counters = new Map();
  const byType = new Map();
  const refs = [];
  const take = (a, fixture, ref) => refs.push({ fn: a.fn, index: a.index, param: a.param, ref });
  // 1. Values repeated exactly.
  for (const a of args) {
    const members = groups.get(`${a.type}\u0000${a.key}`);
    if (members.length < 2) continue;
    let fixture = byType.get(a.type)?.find((f) => f.key === a.key);
    if (!fixture) {
      const n = (counters.get(a.type) ?? 0) + 1;
      counters.set(a.type, n);
      fixture = { id: `fx-${slug(a.type)}-${n}`, type: a.type, value: a.value, key: a.key, uses: 0, size: a.size };
      fixtures.push(fixture);
      byType.set(a.type, [...(byType.get(a.type) ?? []), fixture]);
    }
    fixture.uses += 1;
    take(a, fixture, { fixture: fixture.id });
  }
  // 2. Values that differ from a fixture of the type by a few overrides.
  for (const a of args) {
    if (groups.get(`${a.type}\u0000${a.key}`).length >= 2) continue;
    let best = null;
    for (const f of byType.get(a.type) ?? []) {
      const diff = overrides(f.value, a.value);
      if (diff === null) continue;
      const withObject = Object.fromEntries(diff);
      const size = JSON.stringify({ fixture: f.id, ...(diff.length ? { with: withObject } : {}) }).length;
      if (!best || size < best.size) best = { fixture: f, with: withObject, size, count: diff.length };
    }
    if (best && best.size + REF_OVERHEAD < a.size * 0.6) {
      best.fixture.uses += 1;
      take(a, best.fixture, best.count ? { fixture: best.fixture.id, with: best.with } : { fixture: best.fixture.id });
    }
  }
  const used = fixtures.map(({ key, size, ...f }) => ({ ...f, description: `Example value of type ${f.type}, shared by ${f.uses} example argument(s) (WG-4963).` }));
  const before = exampleBytes(spec);
  const saved = refs.reduce((s, r) => {
    const original = args.find((a) => a.fn === r.fn && a.index === r.index && a.param === r.param);
    return s + original.size - JSON.stringify(r.ref).length;
  }, 0);
  const fixturesAfter = JSON.stringify([...(spec.exampleFixtures ?? []), ...used]).length;
  const after = { examples: before.examples - saved, fixtures: fixturesAfter, total: before.examples - saved + fixturesAfter };
  return { fixtures: used, refs, before, after };
}

/** The operations of a plan. */
export function operations(plan) {
  return [
    ...plan.fixtures.map((f) => ({ type: 'CreateEntity', kind: 'exampleFixture', data: { id: f.id, type: f.type, value: f.value, description: f.description } })),
    ...plan.refs.map((r) => ({ type: 'SetProperty', id: r.fn, property: `examples.${r.index}.args.${r.param}`, value: r.ref })),
  ];
}

/** The report lines of a plan: the repeated values and the sizes before and after. */
export function report(plan) {
  const mb = (n) => `${(n / 1048576).toFixed(2)} MB`;
  const lines = [`${plan.fixtures.length} fixture(s), ${plan.refs.length} example argument(s) now references`];
  for (const f of [...plan.fixtures].sort((a, b) => b.uses - a.uses).slice(0, 10)) lines.push(`  ${f.id}: ${f.uses} use(s), ${JSON.stringify(f.value).length} bytes`);
  lines.push(`function examples before: ${plan.before.examples} bytes (${mb(plan.before.examples)}); fixtures ${plan.before.fixtures}`);
  lines.push(`function examples after:  ${plan.after.examples} bytes (${mb(plan.after.examples)}); fixtures ${plan.after.fixtures}; together ${mb(plan.after.total)} against ${mb(plan.before.total)}`);
  return lines;
}

function main(argv) {
  const opts = { spec: 'specifications/spec.jsonc', ops: null, minBytes: MIN_BYTES };
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === '--spec') opts.spec = argv[i + 1];
    else if (argv[i] === '--ops') opts.ops = argv[i + 1];
    else if (argv[i] === '--min-bytes') opts.minBytes = Number(argv[i + 1]);
    else throw new Error(`unknown argument ${argv[i]}`);
  }
  const spec = parseJsonc(readFileSync(resolve(opts.spec), 'utf8'));
  const plan = planFixtures(spec, { minBytes: opts.minBytes });
  for (const line of report(plan)) console.log(line);
  if (opts.ops) {
    writeFileSync(resolve(opts.ops), `${JSON.stringify(operations(plan))}\n`);
    console.log(`operations written to ${opts.ops}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
