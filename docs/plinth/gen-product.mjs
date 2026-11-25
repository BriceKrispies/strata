#!/usr/bin/env node
// Generates a Strata product skeleton for one Plinth module from the inventory.
//
//   node docs/plinth/gen-product.mjs M10      writes docs/plinth/products/m10.jsonc
//   node docs/plinth/gen-product.mjs --all    writes every module and prints the ranking
//
// Every item of the inventory is classed as one of:
//   live     emitted as a Strata declaration (specc bundle must accept it)
//   planned  emitted as a commented declaration, blocked on a named fundamental:
//            // PLANNED F-<id> (<item id>): <what it needs>
//   manual   emitted as a commented declaration that needs a human decision:
//            // MANUAL (<item id>): <reasons>
//   out      out-of-scope items, counted and not emitted
//
// Output is deterministic. A products file whose first line contains HAND-FINISHED is kept as
// it is: a hand-finished product is never overwritten by a regeneration (see PORTING.md).
// No npm dependencies.

import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const INV_DIR = join(HERE, 'inventory');
const OUT_DIR = join(HERE, 'products');
const GROUPS = ['g1-personal', 'g2-rewards', 'g3-engage', 'g4-governance'];
const IMPORT_SPEC = '../../../specifications/spec.jsonc';

// Short product slugs. A module not listed here uses its lower-cased id.
const SLUGS = {
  M0: 'shell', M1: 'messages', M2: 'notifications', M3: 'points', M4: 'store', M5: 'badges',
  M6: 'promotions', M7: 'automation', M8: 'content', M9: 'people', M10: 'tasks', M11: 'approvals',
  M12: 'audit', M13: 'admin', M14: 'home', M15: 'search', M16: 'calendar', M17: 'catalogue',
  M18: 'pages', REPORTING: 'reporting', MCP: 'mcp', KERNEL: 'kernel', COND: 'condition', AUTHZ: 'authz',
};

// Plinth type -> Strata field type. The table the generator applies to entity fields and operation
// inputs. "approx" marks a type with no direct Strata equivalent: the field is still declared, with
// the nearest type, and the declaration carries a MANUAL note.
const TYPE_TABLE = [
  ['text, string, str', 'Str', false, 'text to Str'],
  ['int, integer, bigint, smallint, int identity', 'Int', false, 'integers to Int (i64 in Strata)'],
  ['numeric(p,s), decimal, money', 'Int', true, 'exact decimals: Strata Int holds whole units only, scale the value or use a ledger unit (see F-double-entry-journal)'],
  ['bool, boolean', 'Bool', false, 'booleans to Bool'],
  ['timestamp, timestamptz', 'Time', false, 'instants to Time'],
  ['uuid (identifier, as Str)', 'Str', false, 'uuid to Str'],
  ['uuid ref -> T', 'Ref T', false, 'references to a Ref in the same product; cross-product references are approx (F-cross-product-refs)'],
  ['enum(a,b,...)', 'Str + constraint', false, 'enums to Str with a required-field constraint'],
  ['X[]', '{List: X}', false, 'arrays to List'],
  ['jsonb, bytea, hex, subject, set<...>, {...}', 'Str', true, 'structured values stored as text'],
];

// ---------------------------------------------------------------- loading

const fundamentals = JSON.parse(readFileSync(join(HERE, 'fundamentals.json'), 'utf8')).fundamentals;
const FUND = new Map();
const FUND_TITLE = new Map();
for (const f of fundamentals) {
  FUND.set(f.id, f.id);
  FUND_TITLE.set(f.id, f.title);
  for (const alias of f.namedAs ?? []) FUND.set(alias, f.id);
}

const MODULES = new Map();
const AUTHZ_ROLES = new Map(); // permission -> roles, from the AUTHZ matrix
for (const g of GROUPS) {
  const doc = JSON.parse(readFileSync(join(INV_DIR, `${g}.json`), 'utf8'));
  for (const m of doc.modules) MODULES.set(m.id.toUpperCase(), { group: g, m });
  const az = doc.modules.find((m) => m.id === 'AUTHZ');
  if (az) {
    for (const op of az.operations) {
      const perm = op.authz?.permission;
      if (perm && op.authz.roles) AUTHZ_ROLES.set(perm, op.authz.roles);
    }
  }
}

const UNKNOWN_F = new Set();
function canon(id) {
  if (!id) return null;
  const c = FUND.get(id);
  if (!c) UNKNOWN_F.add(id);
  return c ?? id;
}

// ---------------------------------------------------------------- helpers

const san = (s) => String(s).toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '');
const titleCase = (s) => {
  const t = String(s).replace(/[_.-]+/g, ' ').trim();
  return t.charAt(0).toUpperCase() + t.slice(1);
};
const oneLine = (s) => String(s).replace(/\s+/g, ' ').trim();
const q = (s) => `'${String(s).replace(/'/g, "\\'")}'`;

function slugOf(id) {
  return SLUGS[id] ?? id.toLowerCase();
}

function fundFirst(item, fallback) {
  const list = (item.strata?.fundamentals ?? []).map(canon);
  return list[0] ?? fallback ?? 'F-unassigned';
}

// The status the inventory gives an item, reduced to what the generator acts on.
// needs-fundamental, and partly with a named fundamental, are blocked.
function blockedBy(item) {
  const st = item.strata?.status;
  if (st === 'needs-fundamental') return fundFirst(item);
  if (st === 'partly' && (item.strata?.fundamentals ?? []).length > 0) return fundFirst(item);
  return null;
}

// ---------------------------------------------------------------- types

function mapType(raw, moduleEntityIds, ns = '') {
  let t = oneLine(raw ?? 'text');
  let isList = false;
  if (/\[\]$/.test(t)) {
    isList = true;
    t = t.replace(/\[\]$/, '').trim();
  }
  const notes = [];
  let approx = false;
  let enumValues = null;
  let refTarget = null;
  const en = /^enum\s*\(([^)]*)\)/i.exec(t);
  const ref = /^uuid ref\s*->\s*([\w.]+)/i.exec(t);
  let base;
  if (en) {
    enumValues = en[1].split(',').map((s) => s.trim()).filter(Boolean);
    base = 'Str';
  } else if (ref) {
    const target = san(ref[1]);
    if (moduleEntityIds.has(target)) {
      refTarget = ns + target;
      base = 'Ref';
    } else {
      approx = true;
      notes.push(`reference to ${ref[1]} is in another product (F-cross-product-refs); held as text`);
      base = 'Str';
    }
  } else {
    const bare = t.toLowerCase().replace(/\(.*?\)/g, '').replace(/,?\s*nullable/g, '').replace(/\s+identity/, '').trim();
    if (/^(text|string|str)$/.test(bare)) base = 'Str';
    else if (/^(uuid|uuid \(pk\))$/.test(bare)) base = 'Str';
    else if (/^(big|small)?int(eger)?\b|^i64$|^u32$/.test(bare) || /^positive/.test(bare)) base = 'Int';
    else if (/^(bool|boolean)\b/.test(bare)) base = 'Bool';
    else if (/^timestamp(tz)?\b/.test(bare)) base = 'Time';
    else if (/^(numeric|decimal|money)/.test(bare)) {
      base = 'Int';
      approx = true;
      notes.push(`"${raw}" is an exact decimal; Strata Int holds whole units (scale or use a ledger unit)`);
    } else {
      base = 'Str';
      approx = true;
      notes.push(`Plinth type "${raw}" has no direct Strata type; held as text`);
    }
  }
  let ty;
  if (refTarget) ty = { Ref: { target: refTarget, 'on-delete': 'Restrict' } };
  else ty = base;
  if (isList) ty = { List: typeof ty === 'string' ? ty : 'Str' };
  return { ty, approx, notes, enumValues, refTarget };
}

// ---------------------------------------------------------------- declarations

function fieldDecls(entity, moduleEntityIds, ns) {
  const fields = {};
  const notes = [];
  const enums = [];
  for (const f of entity.fields ?? []) {
    const name = san(f.name);
    const mt = mapType(f.type, moduleEntityIds, ns);
    const ty = mt.ty;
    for (const n of mt.notes) notes.push(`${f.name}: ${n}`);
    if (f.personal) notes.push(`${f.name}: personal data; declare its personal kind and subject`);
    fields[name] = { id: name, label: titleCase(f.name), ty, required: !!f.required, unique: !!f.unique };
    if (mt.enumValues) enums.push({ name, values: mt.enumValues, required: !!f.required });
  }
  return { fields, notes, enums };
}

function workflowOf(entity, objectId, fieldsDecl) {
  const pairs = [];
  for (const s of entity.states ?? []) {
    const m = /^\s*([\w-]+)\s*->\s*([\w-]+)/.exec(s);
    if (m) pairs.push([m[1], m[2]]);
  }
  if (pairs.length === 0) return null;
  const targets = new Set(pairs.map((p) => p[1]));
  const sources = pairs.map((p) => p[0]);
  const initial = [...new Set(sources.filter((s) => !targets.has(s)))];
  const seen = new Set();
  const transitions = [];
  for (const [from, to] of pairs) {
    const name = `${san(from)}_to_${san(to)}`;
    if (seen.has(name)) continue;
    seen.add(name);
    transitions.push({ name, from: san(from), to: san(to), set: {} });
  }
  const field = ['status', 'state'].find((n) => fieldsDecl[n]) ?? 'status';
  return {
    id: `${objectId}_lifecycle`,
    object: objectId,
    field,
    initial: (initial.length ? initial : [pairs[0][0]]).map(san),
    transitions,
  };
}

function scalarOf(ty) {
  return typeof ty === 'string' ? ty : ty?.Ref ? 'Ref' : ty?.List ? 'List' : 'Str';
}

// ---------------------------------------------------------------- module emission

function emitModule(id, ctx) {
  const { m } = MODULES.get(id);
  const slug = slugOf(id);
  const us = slug.replace(/-/g, '_');
  const P = `plinth_${us}__`;
  // Only entities that will be declared live can be named by another declaration in this product;
  // a reference or a create or query naming a planned entity would be an undeclared name.
  const entityIds = new Set((m.entities ?? [])
    .filter((e) => e.kind !== 'view' && e.kind !== 'append-only-log' && e.strata?.status !== 'out-of-scope')
    .filter((e) => !blockedBy(e))
    .map((e) => san(e.id)));
  const counts = { live: 0, planned: 0, manual: 0, out: 0 };
  const planned = []; // { fundamental, item, why }
  const sections = { roles: [], objects: [], workflows: [], policies: [], presentations: [], operations: [], queries: [], views: [], screens: [], settings: [], checks: [] };
  const liveObjects = new Set();
  const liveQueries = new Map(); // inventory query id -> query id
  const liveOps = new Set();
  const opRoles = new Set();

  function record(kind, item, fund, decl, reasons, out) {
    // kind: live | planned | manual | out
    counts[kind] = (counts[kind] ?? 0) + 1;
    if (kind === 'out') return;
    if (kind === 'planned') {
      const f = fund ?? 'F-unassigned';
      planned.push({ fundamental: f, item: item.id ?? item.route, why: reasons });
      sections[out].push({ kind: 'comment', lines: plannedLines(f, item, reasons, decl) });
    } else if (kind === 'manual') {
      sections[out].push({ kind: 'comment', lines: manualLines(item, reasons, decl) });
    }
  }

  // Roles: the roles every operation of the module names in the AUTHZ matrix (agent excluded).
  const roleNames = new Set();
  for (const op of m.operations ?? []) {
    for (const r of rolesOf(op)) if (r !== 'agent') roleNames.add(r);
  }
  for (const r of [...roleNames].sort()) {
    sections.roles.push({
      kind: 'live',
      obj: {
        id: `${P}role__${san(r)}`,
        name: titleCase(r),
        description: `The ${r} role of the Plinth AUTHZ matrix.`,
        level: 'tenant',
        holders: ['person'],
        capabilities: ['tenant-operation-invoke'],
      },
    });
  }

  // Objects and workflows.
  for (const e of m.entities ?? []) {
    const oid = `${P}${san(e.id)}`;
    const st = e.strata?.status;
    if (st === 'out-of-scope') { counts.out++; continue; }
    if (e.kind === 'view') {
      // A read model, not stored product data: a catalogue or schema description.
      record('manual', e, null, null, [`kind view: a read model, not a stored object (${e.strata?.how ? oneLine(e.strata.how).slice(0, 160) : 'see inventory'})`], 'objects');
      continue;
    }
    const fd = fieldDecls(e, entityIds, P);
    const decl = { id: oid, label: titleCase(e.id), fields: fd.fields, constraints: {} };
    for (const en of fd.enums) {
      if (!en.required) { fd.notes.push(`${en.name}: nullable enum; constraint not generated`); continue; }
      decl.constraints[`${en.name}_values`] = {
        id: `${en.name}_values`,
        expr: `record.${en.name} in [${en.values.map(q).join(', ')}]`,
        message: `${en.name} is one of ${en.values.join(', ')}.`,
      };
    }
    const wf = workflowOf(e, oid, fd.fields);
    if (wf && !fd.fields[wf.field]) {
      decl.fields[wf.field] = { id: wf.field, label: titleCase(wf.field), ty: 'Str', required: true, unique: false };
      fd.notes.push(`${wf.field}: field added for the lifecycle; check the stored values`);
    }
    if (e.kind === 'append-only-log') {
      record('planned', e, canon('F-append-only-records'), decl, ['append-only log: no update or delete path in Strata yet'], 'objects');
      continue;
    }
    const block = blockedBy(e);
    if (block) {
      record('planned', e, block, decl, fd.notes, 'objects');
      continue;
    }
    counts.live++;
    sections.objects.push({ kind: 'live', obj: decl });
    liveObjects.add(e.id);
    if (fd.notes.length) sections.objects.push({ kind: 'comment', lines: fd.notes.map((n) => `MANUAL (${e.id}): ${n}`) });
    if (wf) sections.workflows.push({ kind: 'live', obj: wf });
    const titleField = Object.keys(decl.fields).find((k) => ['name', 'title', 'label', 'subject'].includes(k));
    if (titleField) {
      sections.presentations.push({ kind: 'live', obj: { id: `${oid}_presentation`, object: oid, title: titleField, fields: {} } });
    } else {
      sections.presentations.push({ kind: 'comment', lines: [`MANUAL (${e.id}): no name-like field; choose the presentation title`] });
    }
  }

  // Operations.
  for (const op of m.operations ?? []) {
    const oid = `${P}${san(op.id)}`;
    if (op.strata?.status === 'out-of-scope') { counts.out++; continue; }
    const decl = buildOperation(op, oid, m, P, entityIds);
    const planReasons = [];
    const manualReasons = [];
    const block = blockedBy(op);
    if (block) planReasons.push({ f: block, text: op.strata?.note ?? 'inventory status' });
    const a = op.authz ?? {};
    if (a.approval) planReasons.push({ f: canon('F-held-operations'), text: `approval: ${a.approval}` });
    if (a.separationOfDuties) planReasons.push({ f: canon('F-separation-of-duties'), text: `separation of duties: ${a.separationOfDuties}` });
    if (a.rowPredicate || a.ownerOnly) planReasons.push({ f: canon('F-visibility'), text: `row scope: ${a.rowPredicate ?? 'owner only'}` });
    if ((op.preconditions ?? []).length) manualReasons.push(`${op.preconditions.length} precondition(s) are prose; write them as operation conditions: ${op.preconditions.map((p) => oneLine(p).slice(0, 90)).join(' | ')}`);
    if (decl.unmapped.length) manualReasons.push(`effects need steps: ${decl.unmapped.map((u) => oneLine(u).slice(0, 90)).join(' | ')}`);
    if (decl.invokersMissing) manualReasons.push(`no role in the AUTHZ matrix for ${a.permission ?? 'this operation'}`);
    if (a.stepUp) manualReasons.push('step-up required: declare it on the invoker or the intent');
    if (planReasons.length) {
      record('planned', op, planReasons[0].f, decl.decl, planReasons.map((r) => `${r.f}: ${r.text}`), 'operations');
    } else if (manualReasons.length) {
      record('manual', op, null, decl.decl, manualReasons, 'operations');
    } else {
      counts.live++;
      liveOps.add(op.id);
      sections.operations.push({ kind: 'live', obj: decl.decl });
    }
  }

  // Queries.
  for (const qy of m.queries ?? []) {
    const qid = `${P}${san(qy.id)}`;
    const st = qy.strata?.status;
    if (st === 'out-of-scope') { counts.out++; continue; }
    const target = findQueryObject(qy, m, entityIds, P);
    const decl = { id: qid, object: target, filter: null, params: [], order: [], limit: 200 };
    const block = blockedBy(qy);
    if (block) { record('planned', qy, block, decl, [`${block}: ${qy.strata?.note ?? 'inventory status'}`], 'queries'); continue; }
    const reasons = [];
    if (!target) reasons.push('no single object is named by the query');
    if (qy.filter) reasons.push(`filter is prose: ${oneLine(qy.filter).slice(0, 100)}`);
    if (qy.sort) reasons.push(`sort is prose: ${oneLine(qy.sort).slice(0, 100)}`);
    if ((qy.aggregates ?? []).length) reasons.push('aggregates need a Strata aggregate declaration');
    if (reasons.length) { record('manual', qy, null, decl, reasons, 'queries'); continue; }
    counts.live++;
    liveQueries.set(qy.id, qid);
    delete decl.filter;
    sections.queries.push({ kind: 'live', obj: decl });
  }

  // Events and invariants: checks and notices, not declarations. Blocked ones are planned.
  for (const ev of m.events ?? []) {
    const block = blockedBy(ev);
    const decl = { event: ev.id, audience: ev.audience, trigger: ev.trigger ?? null };
    if (ev.strata?.status === 'out-of-scope') { counts.out++; continue; }
    if (block) record('planned', ev, block, decl, [`${block}: ${ev.strata?.note ?? 'inventory status'}`], 'checks');
    else record('manual', ev, null, decl, ['event or notice: declare as a trigger or notice template once the event kind is chosen'], 'checks');
  }
  for (const inv of m.invariants ?? []) {
    const block = blockedBy(inv);
    const decl = { invariant: inv.id, statement: inv.statement };
    if (inv.strata?.status === 'out-of-scope') { counts.out++; continue; }
    if (block) record('planned', inv, block, decl, [`${block}: ${inv.strata?.note ?? 'inventory status'}`], 'checks');
    else record('manual', inv, null, decl, ['invariant: cover with a test (PORTING step 9)'], 'checks');
  }

  // Settings.
  for (const s of m.settings ?? []) {
    const decl = { id: `${P}${san(s.id)}`, label: titleCase(s.id), type: s.type ?? 'string', levels: ['person'], default: s.default ?? null };
    const block = blockedBy(s);
    if (block) { record('planned', s, block, decl, [`${block}: ${s.strata?.note ?? 'setting'}`], 'settings'); continue; }
    const reasons = [];
    if (!/^(string|text)$/i.test(s.type ?? '')) reasons.push(`setting type "${s.type}" not mapped (only string)`);
    if (!/^"[^"]*"$/.test(String(s.default ?? ''))) reasons.push(`default "${s.default ?? ''}" is not a plain quoted string`);
    if (reasons.length) { record('manual', s, null, decl, reasons, 'settings'); continue; }
    counts.live++;
    sections.settings.push({ kind: 'live', obj: { id: decl.id, label: decl.label, type: 'string', default: { Str: String(s.default).slice(1, -1) }, levels: ['person'] } });
  }

  // Screens: one view and one screen per route. Widgets are sections where they name a live query.
  const screenRoutes = new Map();
  for (const sc of m.screens ?? []) {
    if (!screenRoutes.has(sc.route)) screenRoutes.set(sc.route, []);
    screenRoutes.get(sc.route).push(sc);
  }
  for (const [route, screens] of screenRoutes) {
    const routeText = screens[0].route;
    const sid = `${P}${san(routeText === '/' ? 'home' : routeText) || 'root'}`;
    const screenTitle = routeText === '/' ? 'Home' : titleCase(routeText.split('/').filter((s) => s && !s.startsWith(':')).join(' ') || routeText);
    const sections2 = {};
    for (const sc of screens) {
      for (const w of sc.widgets ?? []) {
        const wid = san(w.id);
        const st = w.strata?.status;
        if (st === 'out-of-scope') { counts.out++; continue; }
        const block = blockedBy(w);
        const data = String(w.data ?? '');
        const hit = [...liveQueries.entries()].find(([qid]) => data.includes(qid) || data.split(/[\s,()]+/).includes(qid));
        if (block) { record('planned', w, block, { widget: w.id }, [`${block}: ${oneLine(w.purpose).slice(0, 110)}`], 'screens'); continue; }
        if (!routeText.startsWith('/') || !hit) {
          const why = !routeText.startsWith('/') ? 'shell chrome, not a route; platform-owned' : `widget data "${data.slice(0, 70)}" names no live query`;
          record('manual', w, null, { widget: w.id, purpose: oneLine(w.purpose).slice(0, 140) }, [why], 'screens');
          continue;
        }
        counts.live++;
        sections2[wid] = { query: hit[1], args: {}, include: [], markdown: [], single: false };
      }
    }
    if (Object.keys(sections2).length && routeText.startsWith('/')) {
      const vid = `${sid}_view`;
      sections.views.push({ kind: 'live', obj: { id: vid, route: routeText, title: screenTitle, sections: sections2 } });
      sections.screens.push({ kind: 'live', obj: { id: sid, name: screenTitle, route: routeText, viewModel: vid, kind: 'plinth.screen', auth: 'session', title: screenTitle, description: `Plinth route ${routeText}.` } });
    }
  }

  // Policies: derived from the AUTHZ permission each role holds. Row scope is not derivable, so
  // every derived policy is planned on F-visibility, with its actions recorded in the block.
  const policyActions = new Map();
  for (const op of m.operations ?? []) {
    const perm = op.authz?.permission ?? '';
    for (const r of rolesOf(op)) {
      if (r === 'agent') continue;
      const key = `${r}`;
      if (!policyActions.has(key)) policyActions.set(key, new Set());
      policyActions.get(key).add(perm);
    }
  }
  for (const [r, perms] of [...policyActions].sort()) {
    for (const e of m.entities ?? []) {
      if (!liveObjects.has(e.id)) continue;
      sections.policies.push({ kind: 'comment', lines: [`PLANNED F-visibility (policy ${P}policy_${san(r)}_${san(e.id)}): role ${r} on ${e.id} for ${[...perms].join(', ')}; row scope is not derivable (derived, not counted)`] });
    }
  }

  const totals = counts;
  const header = [
    `// ${id}: ${m.title}. Generated by docs/plinth/gen-product.mjs from docs/plinth/inventory/${MODULES.get(id).group}.json.`,
    `// Live ${totals.live}, planned ${totals.planned}, manual ${totals.manual}, out-of-scope ${totals.out}.`,
    '// Regenerate with: node docs/plinth/gen-product.mjs ' + id,
    '// PLANNED blocks are blocked on a fundamental (see docs/plinth/fundamentals.json); MANUAL blocks need a human decision.',
    '// Policies are derived from the AUTHZ matrix and are planned until row scope is declared (not counted in the totals).',
  ];
  const productJson = JSON.stringify({ id: `plinth-${slug}`, name: `Plinth ${m.title}`, description: `Plinth module ${id} (${m.title}) as a Strata product: generated skeleton.`, version: '0.1.0', platform: { min: '0.2' }, imports: { spec: IMPORT_SPEC, entities: ['tenant-operation-invoke', 'product-module'] } }, null, 2);
  const parts = [];
  parts.push(`  "product": ${productJson.split('\n').map((l, i) => (i === 0 ? l : '  ' + l)).join('\n')}`);
  const order = ['roles', 'objects', 'workflows', 'policies', 'presentations', 'operations', 'queries', 'views', 'screens', 'settings'];
  for (const name of order) parts.push(renderSection(name, sections[name]));
  const checkLines = sections.checks.flatMap((e) => e.lines.map((l) => `  // ${oneLine(l)}`));
  const checkBlock = checkLines.length ? ['  // CHECKS: invariants and events are tests and notices, not declarations', ...checkLines] : [];
  const text = ['{', ...header.map((h) => `  ${h}`), parts.join(',\n'), ...checkBlock, '}', ''].join('\n');
  return { text, counts, planned, slug, id, m };
}

function renderSection(name, entries) {
  if (entries.length === 0) return `  "${name}": []`;
  const live = entries.filter((e) => e.kind === 'live');
  const out = [`  "${name}": [`];
  let k = 0;
  for (const e of entries) {
    if (e.kind === 'comment') {
      for (const l of e.lines) out.push(`    // ${oneLine(l)}`);
      continue;
    }
    k++;
    const json = JSON.stringify(e.obj, null, 2).split('\n').map((l) => '    ' + l).join('\n');
    out.push(json + (k < live.length ? ',' : ''));
  }
  out.push('  ]');
  return out.join('\n');
}

function plannedLines(f, item, reasons, decl) {
  const title = FUND_TITLE.get(f) ?? '';
  const why = Array.isArray(reasons) ? reasons.join('; ') : String(reasons);
  const lines = [`PLANNED ${f} (${item.id ?? item.route}): ${title}${why ? '. ' + why : ''}`];
  if (decl) for (const l of JSON.stringify(decl, null, 2).split('\n')) lines.push(`  ${l}`);
  return lines;
}

function manualLines(item, reasons, decl) {
  const lines = [`MANUAL (${item.id ?? item.route}): ${Array.isArray(reasons) ? reasons.join('; ') : reasons}`];
  if (decl) for (const l of JSON.stringify(decl, null, 2).split('\n')) lines.push(`  ${l}`);
  return lines;
}

function rolesOf(op) {
  const perm = op.authz?.permission;
  if (perm === 'authenticated') return [];
  return op.authz?.roles ?? (perm && AUTHZ_ROLES.get(perm)) ?? [];
}

function findQueryObject(qy, m, entityIds, P) {
  const tokens = new Set(`${qy.id} ${qy.description ?? ''}`.toLowerCase().split(/[^a-z0-9]+/));
  const hits = (m.entities ?? []).filter((e) => tokens.has(san(e.id)) && entityIds.has(san(e.id)));
  return hits.length === 1 ? `${P}${san(hits[0].id)}` : null;
}

function buildOperation(op, oid, m, P, entityIds) {
  const a = op.authz ?? {};
  const decl = { id: oid, label: oneLine(op.id).replace(/\./g, ' ') };
  const roles = rolesOf(op).filter((r) => r !== 'agent');
  const invokers = a.permission === 'authenticated'
    ? { 'signed-in': true, roles: [] }
    : { 'signed-in': false, roles: roles.map((r) => `${P}role__${san(r)}`) };
  decl.invokers = invokers;
  decl.inputs = (op.inputs ?? []).map((i) => {
    const mt = mapType(i.type, entityIds, P);
    return { name: san(i.name), label: titleCase(i.name), ty: mt.ty, required: !!i.required };
  });
  decl.conditions = [];
  decl.steps = [];
  const unmapped = [];
  let stepN = 0;
  for (const e of op.effects ?? []) {
    const hit = createTarget(e, m, entityIds);
    if (hit) {
      stepN++;
      const entity = hit;
      const fields = {};
      const inputNames = new Set((op.inputs ?? []).map((i) => san(i.name)));
      for (const f of entity.fields ?? []) {
        const name = san(f.name);
        if (inputNames.has(name)) fields[name] = `param.${name}`;
      }
      const missing = (entity.fields ?? []).filter((f) => f.required && !fields[san(f.name)]).map((f) => f.name);
      if (missing.length) unmapped.push(`${e} [create ${entity.id}: fields ${missing.join(', ')} need expressions]`);
      decl.steps.push({ CreateRecord: { id: `step_${stepN}`, object: `${P}${san(entity.id)}`, fields } });
    } else {
      unmapped.push(e);
    }
  }
  const invokersMissing = !invokers['signed-in'] && invokers.roles.length === 0;
  return { decl, unmapped, invokersMissing };
}

// An effect that creates one record of a module entity, named as "insert <entity> ..." or
// "create <entity> ...". Returns the entity, or null.
function createTarget(effect, m, entityIds) {
  // entityIds: the entities this product declares live
  const re = /^\s*(?:insert(?:s)?|create(?:s)?|add(?:s)?)\s+(?:a |an |one |new )?(?:row (?:in|into) |row in )?([a-z][a-z0-9_.]*)\b/i.exec(effect);
  if (!re) return null;
  const want = san(re[1]);
  if (!entityIds.has(want)) return null;
  const e = (m.entities ?? []).find((x) => san(x.id) === want);
  return e ?? null;
}

// ---------------------------------------------------------------- run

function writeModule(id) {
  const res = emitModule(id);
  mkdirSync(OUT_DIR, { recursive: true });
  const file = join(OUT_DIR, `${id.toLowerCase()}.jsonc`);
  if (existsSync(file)) {
    const first = readFileSync(file, 'utf8').split('\n', 1)[0];
    if (first.includes('HAND-FINISHED')) return { ...res, file, kept: true };
  }
  writeFileSync(file, res.text);
  return { ...res, file, kept: false };
}

function report(res) {
  const { counts, planned, id, m } = res;
  const byF = new Map();
  for (const p of planned) byF.set(p.fundamental, (byF.get(p.fundamental) ?? 0) + 1);
  const top = [...byF].sort((a, b) => b[1] - a[1]).slice(0, 6).map(([f, n]) => `${f} x${n}`).join(', ');
  console.log(`${id} ${m.title}: live ${counts.live}, planned ${counts.planned}, manual ${counts.manual}, out ${counts.out}`);
  if (top) console.log(`  blocked by: ${top}`);
}

function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) {
    console.error('usage: node docs/plinth/gen-product.mjs <module-id> | --all');
    process.exit(2);
  }
  if (args[0] === '--all') {
    const rows = [];
    for (const id of MODULES.keys()) {
      const r = writeModule(id);
      rows.push(r);
    }
    rows.sort((a, b) => a.counts.planned - b.counts.planned || a.id.localeCompare(b.id));
    console.log('| rank | module | title | live | planned | manual | out | blocked by (top) |');
    console.log('|---:|---|---|---:|---:|---:|---:|---|');
    rows.forEach((r, i) => {
      const byF = new Map();
      for (const p of r.planned) byF.set(p.fundamental, (byF.get(p.fundamental) ?? 0) + 1);
      const top = [...byF].sort((a, b) => b[1] - a[1]).slice(0, 3).map(([f, n]) => `${f} ${n}`).join(', ') || '-';
      console.log(`| ${i + 1} | ${r.id} | ${r.m.title} | ${r.counts.live} | ${r.counts.planned} | ${r.counts.manual} | ${r.counts.out} | ${top} |`);
    });
    const unknown = [...UNKNOWN_F];
    if (unknown.length) console.log(`unknown fundamentals referenced: ${unknown.join(', ')}`);
    return;
  }
  const id = args[0].toUpperCase();
  if (!MODULES.has(id)) {
    console.error(`unknown module ${args[0]}`);
    process.exit(2);
  }
  const res = writeModule(id);
  report(res);
  if (res.kept) console.log(`kept hand-finished ${res.file}`);
  else console.log(`wrote ${res.file}`);
}

main();
