// Check Plinth port inventories against docs/plinth/inventory.schema.json.
//
//   node docs/plinth/check-inventory.mjs [file.json ...]   (default: every docs/plinth/inventory/*.json)
//
// Implements the JSON Schema keywords the inventory schema uses (type, required, properties,
// additionalProperties false, items, enum, pattern, minItems, $ref into #/$defs), then the
// cross-checks the schema cannot state: every F- id an item names is defined in the file's
// fundamentals, every defined fundamental is used, ids are unique, and a needs-fundamental or
// partly item names at least one fundamental. Exit status 1 on any finding.
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const schema = JSON.parse(readFileSync(join(here, 'inventory.schema.json'), 'utf8'));

function typeOf(v) {
  if (Array.isArray(v)) return 'array';
  if (v === null) return 'null';
  if (Number.isInteger(v)) return 'integer';
  return typeof v;
}

function validate(node, value, path, out) {
  if (node.$ref) node = node.$ref.split('/').slice(1).reduce((s, k) => s[k], schema);
  if (node.enum && !node.enum.includes(value)) out.push(`${path}: ${JSON.stringify(value)} is not one of ${node.enum.join(', ')}`);
  if (node.type) {
    const t = typeOf(value);
    const ok = node.type === t || (node.type === 'number' && t === 'integer');
    if (!ok) { out.push(`${path}: expected ${node.type}, found ${t}`); return; }
  }
  if (node.pattern && typeof value === 'string' && !new RegExp(node.pattern).test(value)) out.push(`${path}: "${value}" does not match ${node.pattern}`);
  if (Array.isArray(value)) {
    if (node.minItems && value.length < node.minItems) out.push(`${path}: needs at least ${node.minItems} item(s)`);
    if (node.items) value.forEach((v, i) => validate(node.items, v, `${path}[${i}]`, out));
  } else if (value && typeof value === 'object') {
    for (const key of node.required ?? []) if (!(key in value)) out.push(`${path}: missing "${key}"`);
    for (const [key, v] of Object.entries(value)) {
      if (node.properties?.[key]) validate(node.properties[key], v, `${path}.${key}`, out);
      else if (node.additionalProperties === false) out.push(`${path}: unknown key "${key}"`);
    }
  }
}

function crossCheck(doc, out) {
  const defined = new Map();
  for (const f of doc.fundamentals ?? []) {
    if (defined.has(f.id)) out.push(`fundamentals: ${f.id} is defined twice`);
    defined.set(f.id, 0);
  }
  const walk = (v, path) => {
    if (Array.isArray(v)) return v.forEach((x, i) => walk(x, `${path}[${i}]`));
    if (!v || typeof v !== 'object') return;
    if (v.strata && typeof v.strata === 'object') {
      const s = v.strata;
      for (const id of s.fundamentals ?? []) {
        if (!defined.has(id)) out.push(`${path}: names ${id}, which fundamentals does not define`);
        else defined.set(id, defined.get(id) + 1);
      }
      if ((s.status === 'needs-fundamental' || s.status === 'partly') && !(s.fundamentals?.length)) out.push(`${path}: is ${s.status} but names no fundamental`);
      if ((s.status === 'expressible' || s.status === 'partly') && !s.how) out.push(`${path}: is ${s.status} but does not say how`);
      if (!(s.evidence?.length) && s.status !== 'out-of-scope') out.push(`${path}: has no evidence`);
    }
    for (const [k, x] of Object.entries(v)) if (k !== 'strata') walk(x, `${path}.${k}`);
  };
  walk(doc.modules, 'modules');
  for (const [id, n] of defined) if (n === 0) out.push(`fundamentals: ${id} is defined but no item names it`);
  for (const m of doc.modules ?? []) {
    for (const part of ['entities', 'operations', 'queries', 'events', 'invariants']) {
      const seen = new Set();
      for (const it of m[part] ?? []) {
        if (seen.has(it.id)) out.push(`modules.${m.id}.${part}: id ${it.id} repeats`);
        seen.add(it.id);
      }
    }
  }
}

const files = process.argv.slice(2).length
  ? process.argv.slice(2)
  : readdirSync(join(here, 'inventory')).filter((f) => f.endsWith('.json')).map((f) => join(here, 'inventory', f));
let failed = false;
for (const file of files) {
  const out = [];
  let doc;
  try { doc = JSON.parse(readFileSync(file, 'utf8')); } catch (e) { console.log(`${file}: not JSON: ${e.message}`); failed = true; continue; }
  validate(schema, doc, '$', out);
  crossCheck(doc, out);
  const counts = (doc.modules ?? []).map((m) => `${m.id}: ${['entities', 'operations', 'queries', 'events', 'screens', 'invariants'].map((k) => `${(m[k] ?? []).length} ${k}`).join(', ')}`);
  console.log(`${file}: ${out.length ? `${out.length} finding(s)` : 'valid'}; ${(doc.fundamentals ?? []).length} fundamentals`);
  for (const c of counts) console.log(`  ${c}`);
  for (const o of out.slice(0, 200)) console.log(`  - ${o}`);
  if (out.length) failed = true;
}
process.exit(failed ? 1 : 0);
