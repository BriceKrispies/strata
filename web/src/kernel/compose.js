// The interpreter of a tenant screen (docs/design-system.md, "Screens as compositions", "The
// interpreter"): `compose(part, model, ctx)` builds a page model, a tree of pattern nodes with every
// value already formatted by its display type (display.js), from the screen's part, the shapes of
// its sections and the view reply with its `people` table. The main thread draws each node with
// the renderer of its pattern (web/src/views/patterns/*.js) and decides nothing.
//
// A screen today declares only its kind (list, detail, editor, settings) and the sections of its
// view, so the pattern of each section is chosen from what the definition says:
//
// - an aggregate section is `aggregate-bars` (grouped) or `stat-tile` (one value), never JSON;
// - a list whose records have an unread/read state is a `notification-list`;
// - any other list is a `record-list` of cards, or a `data-table` when its records have many short
//   fields and no body;
// - a single section is a `record-detail`;
// - a form is a `form` with labelled controls from its fields and its operation's input shape.
//
// Every data pattern has the states `loading` (a skeleton with a fixed number of rows, which says
// nothing about how many records there are), `empty`, `error` and `ready`.
//
// A state is said in words, only where it departs from normal (D-DS-31, no pills): a status whose
// mark is normal is not on the page at all; any other becomes the record's `state` (a lead word and,
// when the presentation declares one, a sentence) and its `edge`, the mark the renderer draws as a
// rule on the record's leading edge. An unread notice has the edge `unread`.
//
// The rules this module keeps by construction: no text of the page model is a record id drawn as
// a number, an epoch, a principal or JSON. Values go through display.js, which draws none of
// those; labels are humanised identifiers; the one place a record's number is shown is the
// self-reference a product gives its records so that forms can ask for them (showcase gap 6), drawn
// as "No. 12".
// @ts-check

import { fallbackDisplayType, formatValue } from './display.js';

/** How many rows a loading skeleton shows, whatever the section will hold. */
export const SKELETON_ROWS = 3;
/** How many groups an aggregate draws as bars; the rest are counted in one line. */
export const BAR_LIMIT = 8;
/** A data table is chosen when records show at least this many short fields and no body. */
const TABLE_FIELDS = 4;
/** The quantised widths of a bar (0 to 20, in steps of 5%), so no style is written inline. */
const FILL_STEPS = 20;

const ANON = 'A member';
const PRINCIPAL = /^(user|team|sealed|automation|service|agent|system|org|svc|role|group):/;
// Field ids that name a record to the machine, never to a reader.
const IDENTIFIERS = new Set(['key', 'handle', 'slug', 'principal', 'recipient', 'holder', 'reactor', 'owner']);
const STATUS_IDS = ['status', 'state'];
// Short words that name a record when it has no title field (a reaction is its kind).
const TITLE_FALLBACKS = ['kind', 'type', 'category', 'label'];
// The times that make a list a feed: when a record was posted, not when its subject joined or is due.
const FEED_TIMES = ['posted_at', 'written_at', 'published_at', 'sent_at', 'at', 'created_at'];
const BODY_IDS = ['body', 'content', 'text', 'description', 'bio', 'summary', 'reason', 'detail', 'how_to_earn', 'purpose', 'about', 'message'];
const TIME_PREFERENCE = ['posted_at', 'written_at', 'published_at', 'awarded_at', 'at', 'sent_at', 'due_at', 'created_at', 'joined_at'];
const LONG_TEXT = 60;
const CHOICE_WORD = /^[a-z][a-z0-9]*(?:[ _-][a-z0-9]+){0,3}$/;

/**
 * @typedef {import('./display.js').DisplayNode} DisplayNode
 * @typedef {{ lang?: string, timeZone?: string, now?: number, people?: Record<string, any>, screens?: { id: string, route?: string }[], route?: string, forms?: Record<string, any>, moreLoading?: string | null, rowActions?: any[], affordances?: Record<string, any>, confirm?: string | null }} ComposeContext
 * @typedef {{ id: string, type: string, target: string | null, displayType: string | null, props: Record<string, any> | null, required: boolean, label: string }} Field
 * @typedef {{ name: string, label: string, single: boolean, aggregate: boolean, object: string | null, title: string | null, fields: Field[], marks: Record<string, Lookup>, joins: Record<string, Lookup> }} Section
 * @typedef {{ object: string | null, aggregate: string, key: string | null, second: string | null }} Lookup
 * @typedef {{ key: string | null, value: number | string | null }} JoinGroup
 * @typedef {{ id: string, label: string, value: DisplayNode }} Fact
 * @typedef {{ key: string, title: string | null, href: string | null, subtitle: string | null, avatar: { initials: string, src: string | null } | null, state: DisplayNode | null, edge: string | null, person: DisplayNode | null, time: DisplayNode | null, timeLabel: string, body: { text?: string, blocks?: any[] } | null, meta: Fact[], number: string | null, marks: Record<string, boolean>, joins: Record<string, JoinGroup[]> }} RecordModel
 */

// ---- words --------------------------------------------------------------------------------

/**
 * A label from an identifier: the part after the product's prefix, with underscores as spaces and
 * a capital first letter (`showcase_community__my_badges` is "My badges").
 * @param {unknown} id
 */
export function humanise(id) {
  const local = String(id ?? '').split('__').pop() ?? '';
  const spaced = local.replace(/[_-]+/g, ' ').replace(/\s+/g, ' ').trim();
  return spaced ? spaced.charAt(0).toUpperCase() + spaced.slice(1) : '';
}

/** The label of a field from its id: `posted_at` is "Posted", `at` is "When". @param {string} id */
function fieldLabel(id) {
  if (id === 'at') return 'When';
  return humanise(id.replace(/_at$/, '')) || humanise(id);
}

/** The local name of an object type: `showcase_community__message` is `message`. @param {string | null} object */
function localName(object) {
  return String(object ?? '').split('__').pop() ?? '';
}

/** A field that names a record to the machine (`key`, `badge_key`, a handle, a principal). @param {string} id */
function isIdentifier(id) {
  return IDENTIFIERS.has(id) || id.endsWith('_key');
}

/** @param {unknown} v @returns {v is Record<string, any>} */
function isRecord(v) {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** @param {any} value */
function isAggregateValue(value) {
  return isRecord(value) && Array.isArray(value.groups);
}

/** The text a display node shows, for titles, notices and tests. @param {DisplayNode | null | undefined} node @returns {string} */
export function nodeText(node) {
  if (!node) return '';
  if (node.kind === 'empty') return '';
  if (node.kind === 'person') return String(node.name ?? '');
  if (node.kind === 'state') return String(node.label ?? '');
  if (node.kind === 'rich') return String(node.markdown ?? '');
  return typeof node.text === 'string' ? node.text : '';
}

// ---- shapes ---------------------------------------------------------------------------------

/** `ref<post>` is `ref` with target `post`; `list<str>` is `list`. @param {unknown} type */
function splitType(type) {
  const text = String(type ?? 'str').toLowerCase();
  const m = /^(\w+)<([^>]*)>$/.exec(text);
  return m ? { type: m[1], target: m[1] === 'ref' ? String(type).slice(4, -1) : null } : { type: text, target: null };
}

/** @param {any} f @returns {Field} */
function fieldOf(f) {
  const { type, target } = splitType(f?.type);
  const id = String(f?.id ?? '');
  return {
    id,
    type,
    target,
    displayType: typeof f?.displayType === 'string' ? f.displayType : null,
    props: isRecord(f?.props) ? f.props : null,
    required: f?.required === true,
    label: typeof f?.label === 'string' && f.label !== '' ? f.label : fieldLabel(id),
  };
}

/**
 * The fields of a section with no declared shape, read from its rows: the type each value has.
 * A number in a field named like a time (`*_at`, `at`) is a time; any other number an integer.
 * @param {any} value @returns {Field[]}
 */
function inferFields(value) {
  const rows = Array.isArray(value) ? value : isRecord(value) ? [value] : [];
  /** @type {Map<string, any>} */
  const seen = new Map();
  for (const row of rows) {
    for (const [id, v] of Object.entries(row?.fields ?? {})) if (!seen.has(id) || seen.get(id) === null) seen.set(id, v);
  }
  return [...seen.keys()].sort().map((id) => {
    const v = seen.get(id);
    let type = 'str';
    /** @type {string | null} */
    let displayType = null;
    if (typeof v === 'boolean') type = 'bool';
    else if (typeof v === 'number') type = id === 'at' || id.endsWith('_at') ? 'time' : 'int';
    else if (Array.isArray(v)) type = 'list';
    else if (isRecord(v) && 'ref' in v) type = 'ref';
    else if (isRecord(v) && 'person' in v) displayType = 'person';
    return { id, type, target: null, displayType, props: null, required: false, label: fieldLabel(id) };
  });
}

/**
 * The section a widget binds to: its label, whether it is single or an aggregate, its object type
 * and fields, from the part's shape when it has one and from the rows otherwise.
 * @param {any} part @param {string} name @param {any} value @returns {Section}
 */
export function sectionOf(part, name, value) {
  const declared = part?.sections?.[name];
  const shape = isRecord(declared?.shape) ? declared.shape : null;
  const aggregate = typeof shape?.aggregate === 'string' || isAggregateValue(value);
  const single = typeof declared?.single === 'boolean' ? declared.single : !aggregate && value !== undefined && !Array.isArray(value);
  const fields = Array.isArray(shape?.fields) ? shape.fields.map(fieldOf) : inferFields(value);
  const firstRow = Array.isArray(value) ? value[0] : value;
  return {
    name,
    label: typeof declared?.label === 'string' && declared.label !== '' ? declared.label : humanise(name),
    single,
    aggregate,
    object: typeof shape?.object === 'string' ? shape.object : typeof firstRow?.object === 'string' ? firstRow.object : null,
    title: typeof shape?.title === 'string' ? shape.title : null,
    marks: lookupsOf(shape?.marks),
    joins: lookupsOf(shape?.joins),
    fields,
  };
}

// ---- roles of fields --------------------------------------------------------------------------

/**
 * What each field does on a card: which is the title, the status, the person, the time, the body,
 * and which are shown as facts. Decided once per section from its shape and its rows.
 * @param {Section} section @param {any[]} rows
 */
function rolesOf(section, rows) {
  const byId = new Map(section.fields.map((f) => [f.id, f]));
  const values = (/** @type {string} */ id) => rows.map((r) => r?.fields?.[id]).filter((v) => v !== null && v !== undefined && v !== '');
  const isStr = (/** @type {string} */ id) => byId.get(id)?.type === 'str';
  const holdsPrincipals = (/** @type {string} */ id) => values(id).some((v) => typeof v === 'string' && PRINCIPAL.test(v));
  const isPerson = (/** @type {Field} */ f) => f.displayType === 'person' || values(f.id).some((v) => isRecord(v) && 'person' in v);
  // The record's own number: a reference to its own object type (showcase gap 6).
  const numberField = section.fields.find((f) => f.type === 'ref' && ((section.object !== null && f.target === section.object) || (f.target === null && f.id === 'number'))) ?? null;
  const usable = (/** @type {Field} */ f) => isStr(f.id) && !isIdentifier(f.id) && !holdsPrincipals(f.id) && f.displayType !== 'person';

  let title = section.title && byId.has(section.title) ? section.title : null;
  if (!title) title = ['display_name', 'name', 'title'].find((id) => byId.has(id) && usable(/** @type {Field} */ (byId.get(id)))) ?? null;
  if (!title) title = section.fields.find((f) => usable(f) && (f.id.endsWith('_name') || f.id.endsWith('_title')))?.id ?? null;
  if (!title) title = TITLE_FALLBACKS.find((id) => byId.has(id) && usable(/** @type {Field} */ (byId.get(id)))) ?? null;
  const subtitle = title && title !== 'title' && byId.has('title') && usable(/** @type {Field} */ (byId.get('title'))) ? 'title' : null;
  const status = section.fields.find((f) => f.displayType === 'status')?.id ?? STATUS_IDS.find((id) => isStr(id) && id !== title) ?? null;
  const persons = section.fields.filter(isPerson).map((f) => f.id);
  const timeFields = section.fields.filter((f) => f.type === 'time' && (f.displayType === null || f.displayType === 'datetime' || f.displayType === 'date'));
  const time = TIME_PREFERENCE.find((id) => timeFields.some((f) => f.id === id)) ?? timeFields[0]?.id ?? null;
  const taken = new Set([title, subtitle, status, time, numberField?.id].filter(Boolean));
  const averageLength = (/** @type {string} */ id) => {
    const texts = values(id).filter((v) => typeof v === 'string');
    return texts.length ? texts.reduce((n, t) => n + t.length, 0) / texts.length : 0;
  };
  const marked = rows.some((r) => isRecord(r?.blocks)) ? Object.keys(rows.find((r) => isRecord(r?.blocks))?.blocks ?? {}) : [];
  const bodyCandidates = section.fields.filter((f) => !taken.has(f.id) && usable(f));
  const body = section.fields.find((f) => f.displayType === 'rich-text' && !taken.has(f.id))?.id
    ?? marked.find((id) => !taken.has(id))
    ?? BODY_IDS.find((id) => bodyCandidates.some((f) => f.id === id))
    ?? bodyCandidates.find((f) => averageLength(f.id) >= LONG_TEXT)?.id
    ?? null;
  if (body) taken.add(body);
  const shown = section.fields.filter((f) => {
    if (taken.has(f.id) || persons.includes(f.id)) return false;
    if (isIdentifier(f.id)) return false;
    if (f.type === 'str' && holdsPrincipals(f.id) && f.displayType !== 'person') return false;
    return true;
  });
  const bodyLength = body ? averageLength(body) : 0;
  return { title, subtitle, status, persons, time, body, bodyLength, number: numberField?.id ?? null, shown, avatar: title === 'display_name' || ['profile', 'person', 'member', 'user'].includes(localName(section.object)) };
}

/** The display type a value of a field is drawn by. @param {Field} field @param {any} value */
function displayOf(field, value) {
  if (isRecord(value) && 'person' in value) return 'person';
  if (field.displayType) return field.displayType;
  if (field.type === 'str' && STATUS_IDS.includes(field.id)) return 'status';
  // A short lowercase word in a text field is a choice: "profile rule" is drawn "Profile rule".
  if (field.type === 'str' && typeof value === 'string' && value.length <= 24 && CHOICE_WORD.test(value)) return 'choice';
  return fallbackDisplayType(field.type);
}

/** @param {Field} field @param {any} value @param {ComposeContext} ctx @returns {DisplayNode} */
function format(field, value, ctx) {
  return formatValue({ value, fieldType: field.type, displayType: displayOf(field, value), props: field.props, ctx: displayCtx(ctx) });
}

/**
 * The state a status node says, or null when there is nothing to say: a normal value is not drawn.
 * @param {DisplayNode | null} node @returns {DisplayNode | null}
 */
function departure(node) {
  return node && node.kind === 'state' && node.mark !== 'normal' ? node : null;
}

/** @param {ComposeContext} ctx */
function displayCtx(ctx) {
  return { lang: ctx.lang, timeZone: ctx.timeZone, now: ctx.now, people: ctx.people };
}

// ---- links ------------------------------------------------------------------------------------

/**
 * The address of the screen that shows a record of this list, when the tenant has one: a screen
 * whose route continues the current screen's route and ends in one parameter the record has (a
 * field, or `id`), or a route named after the object type (`/messages/:id` for a message).
 * @param {Section} section @param {any} row @param {ComposeContext} ctx
 */
function recordHref(section, row, ctx) {
  const here = ctx.route ?? '';
  const object = localName(section.object);
  for (const screen of ctx.screens ?? []) {
    const route = screen.route ?? '';
    const segments = route.split('/').filter(Boolean);
    const params = segments.filter((s) => s.startsWith(':'));
    if (params.length !== 1 || !segments[segments.length - 1].startsWith(':')) continue;
    const param = params[0].slice(1);
    const value = param === 'id' ? row?.id : row?.fields?.[param];
    if (typeof value !== 'string' && typeof value !== 'number') continue;
    const literal = segments[segments.length - 2] ?? '';
    const continues = here !== '' && here !== '/' && route.startsWith(`${here}/`) && segments.length - here.split('/').filter(Boolean).length <= 2;
    const named = object !== '' && (literal === object || literal === `${object}s`);
    if (!continues && !named) continue;
    return `/${segments.map((s) => (s.startsWith(':') ? encodeURIComponent(String(value)) : s)).join('/')}`;
  }
  return null;
}

// ---- records ----------------------------------------------------------------------------------

/** @param {string} name */
function initialsOf(name) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => Array.from(w)[0].toUpperCase()).join('');
}

/**
 * A row mark or row join of a section, as its screen part's shape declares it (WG-5599): the object
 * type its query reads, the aggregate, the reference field the rows are keyed by, and for a join the
 * second field its groups are made by.
 * @param {any} value @returns {Lookup}
 */
function lookupOf(value) {
  return {
    object: typeof value?.object === 'string' ? value.object : null,
    aggregate: typeof value?.aggregate === 'string' ? value.aggregate : '',
    key: typeof value?.key === 'string' ? value.key : null,
    second: typeof value?.second === 'string' ? value.second : null,
  };
}

/** The declared marks or joins of a section shape, by name. @param {any} declared @returns {Record<string, Lookup>} */
function lookupsOf(declared) {
  if (!isRecord(declared)) return {};
  return Object.fromEntries(Object.entries(declared).map(([name, value]) => [name, lookupOf(value)]));
}

/**
 * The marks of a row: each declared mark by name, true when the reader's records hold the row.
 * @param {any} row @returns {Record<string, boolean>}
 */
function rowMarksOf(row) {
  if (!isRecord(row?.marks)) return {};
  return Object.fromEntries(Object.entries(row.marks).map(([name, held]) => [name, held === true]));
}

/**
 * The joins of a row: each declared join by name, the groups the row holds (a key and a value each).
 * @param {any} row @returns {Record<string, JoinGroup[]>}
 */
function rowJoinsOf(row) {
  if (!isRecord(row?.joins)) return {};
  return Object.fromEntries(
    Object.entries(row.joins).map(([name, groups]) => [
      name,
      (Array.isArray(groups) ? groups : []).map((/** @type {any} */ g) => ({
        key: typeof g?.key === 'string' ? g.key : null,
        value: typeof g?.value === 'number' || typeof g?.value === 'string' ? g.value : null,
      })),
    ]),
  );
}

/**
 * One record as a card or a detail shows it.
 * @param {Section} section @param {ReturnType<typeof rolesOf>} roles @param {any} row @param {ComposeContext} ctx
 * @returns {RecordModel}
 */
function recordOf(section, roles, row, ctx) {
  const values = row?.fields ?? {};
  const byId = new Map(section.fields.map((f) => [f.id, f]));
  const textOf = (/** @type {string | null} */ id) => {
    if (!id) return null;
    const field = byId.get(id);
    if (!field) return null;
    const text = nodeText(format(field, values[id], ctx));
    return text === '' ? null : text;
  };
  const nodeOf = (/** @type {string | null} */ id) => {
    if (!id) return null;
    const field = byId.get(id);
    if (!field) return null;
    const node = format(field, values[id], ctx);
    return node.kind === 'empty' ? null : node;
  };
  // The person: the first person field, else the author when the reply names them.
  let person = roles.persons.map((id) => nodeOf(id)).find(Boolean) ?? null;
  if (!person && isRecord(row?.created_by)) {
    const author = formatValue({ value: row.created_by, fieldType: 'created_by', ctx: displayCtx(ctx) });
    if (author.kind === 'person' && author.name !== ANON) person = author;
  }
  /** @type {{ text?: string, blocks?: any[] } | null} */
  let body = null;
  if (roles.body) {
    const blocks = row?.blocks?.[roles.body];
    if (Array.isArray(blocks) && blocks.length) body = { blocks };
    else {
      const text = textOf(roles.body);
      if (text) body = { text };
    }
  }
  const title = textOf(roles.title);
  const numberValue = roles.number ? values[roles.number] : null;
  const numberId = isRecord(numberValue) && isRecord(numberValue.ref) ? numberValue.ref.id : null;
  const avatar = roles.avatar && title ? { initials: initialsOf(title), src: null } : null;
  /** @type {Fact[]} */
  const meta = roles.shown.map((f) => {
    const value = format(f, values[f.id], ctx);
    // A state that is normal is said nowhere, in the facts either.
    return { id: f.id, label: f.label, value: value.kind === 'state' && value.mark === 'normal' ? { kind: 'empty', text: '—' } : value };
  });
  const state = departure(nodeOf(roles.status));
  const timeField = roles.time ? byId.get(roles.time) : null;
  return {
    key: String(row?.id ?? title ?? ''),
    title,
    href: recordHref(section, row, ctx),
    subtitle: textOf(roles.subtitle),
    avatar,
    state,
    edge: state ? state.mark : null,
    person,
    time: nodeOf(roles.time),
    timeLabel: timeField ? timeField.label : '',
    body,
    meta,
    number: typeof numberId === 'number' || (typeof numberId === 'string' && /^\d{1,8}$/.test(numberId)) ? `No. ${numberId}` : null,
    marks: rowMarksOf(row),
    joins: rowJoinsOf(row),
  };
}

// ---- patterns ---------------------------------------------------------------------------------

/** The fixed skeleton of a pattern: the same number of rows for every section. @param {string} pattern */
function skeleton(pattern) {
  return { rows: pattern === 'stat-tile' ? 1 : SKELETON_ROWS };
}

/**
 * The state every data pattern carries, and its words for the empty and error states.
 * @param {Section} section @param {'loading' | 'empty' | 'error' | 'ready'} state @param {string} pattern
 */
function base(section, state, pattern) {
  return {
    pattern,
    id: `section:${section.name}`,
    section: section.name,
    label: section.label,
    state,
    skeleton: state === 'loading' ? skeleton(pattern) : null,
    empty: state === 'empty' ? emptyWords(section, pattern) : null,
    error: state === 'error' ? 'This part of the page could not be loaded. Try again in a moment.' : null,
  };
}

/** @param {Section} section @param {string} pattern */
function emptyWords(section, pattern) {
  if (pattern === 'notification-list') return { title: 'You are all caught up', text: 'New notifications will appear here.' };
  if (pattern === 'aggregate-bars' || pattern === 'stat-tile') return { title: 'Nothing to count yet', text: `${section.label} will fill in as activity comes in.` };
  if (pattern === 'record-detail') return { title: 'Not available', text: 'There is nothing to show here, or you may not see it.' };
  return { title: `No ${section.label.toLowerCase()} yet`, text: 'When there is something to show, it will appear here.' };
}

/** Whether a list section holds notices: records with an unread or read state. @param {Section} section @param {any[]} rows */
function isNotices(section, rows) {
  if (/notification|notice|inbox/.test(localName(section.object))) return true;
  const state = section.fields.find((f) => STATUS_IDS.includes(f.id) || f.id === 'read');
  if (!state) return false;
  return rows.some((r) => {
    const v = r?.fields?.[state.id];
    return v === 'unread' || v === 'read' || (state.id === 'read' && typeof v === 'boolean');
  });
}

/** The pattern a section is drawn by, before its data is known. @param {Section} section */
function patternGuess(section) {
  if (section.aggregate) return 'aggregate-bars';
  if (section.single) return 'record-detail';
  if (/notification|notice|inbox/.test(localName(section.object))) return 'notification-list';
  return 'record-list';
}

/** @param {Section} section @param {any} value @param {ComposeContext} ctx */
function aggregateNode(section, value, ctx) {
  const groups = Array.isArray(value?.groups) ? value.groups : [];
  if (groups.length === 1 && (groups[0]?.key === null || groups[0]?.key === undefined)) {
    const node = base(section, 'ready', 'stat-tile');
    return { ...node, value: nodeText(formatValue({ value: groups[0]?.value, displayType: 'count', fieldType: 'int', ctx: displayCtx(ctx) })) || '0' };
  }
  if (!groups.length) return base(section, 'empty', 'aggregate-bars');
  const counted = groups.map((/** @type {any} */ g, /** @type {number} */ i) => {
    const n = typeof g?.value === 'number' && Number.isFinite(g.value) ? g.value : typeof g?.value === 'string' && /^-?\d+$/.test(g.value) ? Number(g.value) : 0;
    return { key: `group-${i}`, label: groupLabel(g?.key, ctx), value: n, text: nodeText(formatValue({ value: g?.value, displayType: 'count', fieldType: 'int', ctx: displayCtx(ctx) })) || '0' };
  });
  const max = Math.max(1, ...counted.map((/** @type {any} */ c) => c.value));
  const total = counted.reduce((/** @type {number} */ n, /** @type {any} */ c) => n + c.value, 0);
  const shown = counted.slice(0, BAR_LIMIT).map((/** @type {any} */ c) => ({ ...c, fill: Math.max(c.value > 0 ? 1 : 0, Math.round((c.value / max) * FILL_STEPS)) }));
  const rest = counted.length - shown.length;
  return {
    ...base(section, 'ready', 'aggregate-bars'),
    bars: shown.map(({ value: _v, ...bar }) => bar),
    total: `${nodeText(formatValue({ value: total, displayType: 'count', fieldType: 'int', ctx: displayCtx(ctx) }))} in all`,
    rest: rest > 0 ? `and ${rest} more` : null,
  };
}

/**
 * The label of an aggregate group. A group of a person is their name (or "A member"), never the
 * principal; a group without a value is "Not set".
 * @param {any} key @param {ComposeContext} ctx
 */
function groupLabel(key, ctx) {
  if (key === null || key === undefined || key === '') return 'Not set';
  if (isRecord(key) && 'person' in key) return nodeText(formatValue({ value: key, fieldType: 'created_by', ctx: displayCtx(ctx) }));
  if (isRecord(key) && 'ref' in key) return nodeText(formatValue({ value: key, fieldType: 'ref', ctx: displayCtx(ctx) }));
  if (typeof key === 'boolean') return key ? 'Yes' : 'No';
  if (typeof key === 'number') return nodeText(formatValue({ value: key, displayType: 'count', fieldType: 'int', ctx: displayCtx(ctx) })) || 'Not set';
  if (typeof key !== 'string') return 'Other';
  if (PRINCIPAL.test(key)) return ANON;
  if (/^\d{9,}$/.test(key)) return 'Other';
  return CHOICE_WORD.test(key) ? humanise(key) : key;
}

/** @param {Section} section @param {any[]} rows @param {ComposeContext} ctx */
function noticeNode(section, rows, ctx) {
  if (!rows.length) return base(section, 'empty', 'notification-list');
  const byId = new Map(section.fields.map((f) => [f.id, f]));
  const stateField = section.fields.find((f) => STATUS_IDS.includes(f.id) || f.id === 'read');
  const textField = ['text', 'title', 'body', 'message', 'summary'].find((id) => byId.has(id)) ?? null;
  const kindField = byId.has('kind') ? 'kind' : null;
  const roles = rolesOf(section, rows);
  const items = rows.map((row) => {
    const values = row?.fields ?? {};
    const raw = stateField ? values[stateField.id] : null;
    const unread = raw === 'unread' || (stateField?.id === 'read' && raw === false);
    const text = textField ? nodeText(format(/** @type {Field} */ (byId.get(textField)), values[textField], ctx)) : '';
    const kind = kindField ? nodeText(format(/** @type {Field} */ (byId.get(kindField)), values[kindField], ctx)) : '';
    const link = typeof values.link === 'string' && /^\/(?!\/)\S*$/.test(values.link) ? values.link : null;
    const record = recordOf(section, roles, row, ctx);
    return { key: String(row?.id ?? text), text: text || 'Notification', kind: kind || null, unread, edge: unread ? 'unread' : null, time: record.time, href: link, number: record.number };
  });
  const unread = items.filter((i) => i.unread).length;
  // The count is said in the heading line, in words: "8 unread".
  return { ...base(section, 'ready', 'notification-list'), unread, count: unread > 0 ? `${unread} unread` : null, items };
}

/**
 * The row actions a section's row shows (WG-5466, D-DS-32): each action of the screen whose row is
 * this section, with the answer preflight gave this row. A refused action is removed or disabled as
 * its when-refused says, and says why in words; an unknown or allowed one is drawn enabled. An
 * action with a confirmation is armed on the row it was first pressed on.
 * @param {ComposeContext} ctx @param {string} section @param {any} row
 */
export function rowActionsOf(ctx, section, row) {
  const rowKey = String(row?.id ?? '');
  const answers = ctx.affordances?.[section]?.[rowKey] ?? {};
  return (ctx.rowActions ?? [])
    .filter((/** @type {any} */ action) => action.row === section)
    .flatMap((/** @type {any} */ action) => {
      const answer = answers[action.id] ?? null;
      const refused = answer?.state === 'refused';
      if (refused && action.when_refused === 'hide') return [];
      const armed = ctx.confirm === `${action.id}#${rowKey}`;
      return [{
        id: action.id,
        label: armed && action.confirm ? action.confirm : action.label,
        section,
        row: rowKey,
        disabled: refused,
        note: refused ? (answer.detail || 'Not available for this record.') : null,
        armed,
      }];
    });
}

/** @param {Section} section @param {any[]} rows @param {ComposeContext} ctx @param {'main' | 'aside'} region @param {string | null} more */
function listNode(section, rows, ctx, region, more) {
  const roles = rolesOf(section, rows);
  const short = roles.shown.filter((f) => f.type !== 'list');
  const table = !roles.body && short.length >= TABLE_FIELDS && region === 'main';
  const pattern = table ? 'data-table' : 'record-list';
  const moreNode = more ? { section: section.name, busy: ctx.moreLoading === section.name } : null;
  if (!rows.length) return { ...base(section, 'empty', pattern), count: 0, more: null };
  const records = rows.map((row) => ({ ...recordOf(section, roles, row, ctx), actions: rowActionsOf(ctx, section.name, row) }));
  const count = `${rows.length}${more ? '+' : ''}`;
  if (table) {
    /** @type {{ id: string, label: string, numeric: boolean }[]} */
    const columns = [];
    if (roles.number) columns.push({ id: '_number', label: 'No.', numeric: false });
    if (roles.title) columns.push({ id: '_title', label: fieldLabel(roles.title === 'display_name' ? 'name' : roles.title), numeric: false });
    if (roles.subtitle) columns.push({ id: '_subtitle', label: fieldLabel(roles.subtitle), numeric: false });
    // A state column only when some row departs from normal; a normal row says nothing in it.
    if (roles.status && records.some((r) => r.state)) columns.push({ id: '_status', label: fieldLabel(roles.status), numeric: false });
    if (roles.persons.length) columns.push({ id: '_person', label: fieldLabel(roles.persons[0]), numeric: false });
    for (const f of roles.shown) columns.push({ id: f.id, label: f.label, numeric: f.type === 'int' });
    if (roles.time) columns.push({ id: '_time', label: fieldLabel(roles.time), numeric: false });
    const rowsOut = records.map((r) => {
      /** @type {Record<string, DisplayNode>} */
      const cells = {};
      const text = (/** @type {string | null} */ t) => (t ? { kind: 'text', text: t } : { kind: 'empty', text: '—' });
      cells._number = text(r.number);
      cells._title = r.title ? { kind: 'title', text: r.title, href: r.href, avatar: r.avatar } : { kind: 'empty', text: '—' };
      cells._subtitle = text(r.subtitle);
      cells._status = r.state ?? { kind: 'text', text: '' };
      cells._person = r.person ?? { kind: 'empty', text: '—' };
      cells._time = r.time ?? { kind: 'empty', text: '—' };
      for (const m of r.meta) cells[m.id] = m.value;
      return { key: r.key, edge: r.edge, cells: columns.map((c) => ({ column: c.id, label: c.label, numeric: c.numeric, value: cells[c.id] ?? { kind: 'empty', text: '—' } })) };
    });
    return { ...base(section, 'ready', 'data-table'), count, columns, rows: rowsOut, more: moreNode };
  }
  // A feed (records with a time and a body) reads down one column; anything else is a grid of cards.
  const feed = region === 'aside' || (roles.time !== null && FEED_TIMES.includes(roles.time) && roles.body !== null) || roles.bodyLength > 400;
  return {
    ...base(section, 'ready', 'record-list'),
    count,
    layout: feed ? 'list' : 'grid',
    compact: region === 'aside',
    // Cards side by side keep an even height: their bodies are cut to a few lines.
    clamp: region === 'aside' || !feed,
    items: records.map((r) => ({ ...r, meta: r.meta.filter((m) => m.value.kind !== 'empty') })),
    more: moreNode,
  };
}

/** @param {Section} section @param {any} row @param {ComposeContext} ctx */
function detailNode(section, row, ctx) {
  if (!isRecord(row)) return base(section, 'empty', 'record-detail');
  const roles = rolesOf(section, [row]);
  const record = recordOf(section, roles, row, ctx);
  const facts = record.meta.filter((m) => m.value.kind !== 'empty');
  if (record.time) facts.push({ id: '_time', label: record.timeLabel, value: record.time });
  const { meta: _meta, ...rest } = record;
  return { ...base(section, 'ready', 'record-detail'), record: { ...rest, href: null }, facts };
}

// ---- forms ------------------------------------------------------------------------------------

/**
 * The state of a form of a tenant screen the page keeps, as tenant-ui.js holds it.
 * @typedef {{ values: Record<string, string | boolean>, busy: boolean, error: string, notice?: string, errors: Record<string, string> }} HeldForm
 */

/**
 * A form: its title (the operation it invokes), its controls with their labels, values and errors,
 * whether each input is required, and its primary submit.
 * @param {string} id @param {any} form @param {HeldForm | undefined} held @param {{ collapsible: boolean, open: boolean }} options
 */
export function formNode(id, form, held, options) {
  const inputs = new Map((Array.isArray(form?.shape?.inputs) ? form.shape.inputs : []).map((/** @type {any} */ i) => [i?.name, i]));
  const values = held?.values ?? {};
  const errors = held?.errors ?? {};
  const fields = (form?.fields ?? []).map((/** @type {any} */ field) => {
    const input = inputs.get(field.input);
    return {
      name: field.name,
      label: field.label,
      control: field.control,
      options: field.options ?? [],
      value: values[field.name] ?? (field.control === 'checkbox' ? false : ''),
      error: errors[field.name] ?? '',
      required: input ? input.required === true : null,
    };
  });
  const error = held?.error ?? '';
  return {
    pattern: 'form',
    id: `form:${id}`,
    form: id,
    title: humanise(form?.shape?.operation ?? form?.operation ?? id),
    submitLabel: form?.submit_label ?? 'Send',
    busy: held?.busy === true,
    error,
    notice: held?.notice ?? '',
    collapsible: options.collapsible,
    open: !options.collapsible || options.open || error !== '' || fields.some((/** @type {any} */ f) => f.error !== ''),
    fields,
  };
}

// ---- the page ---------------------------------------------------------------------------------

/**
 * The page model of a tenant screen. `model` null draws every section in its loading state, from
 * the part's section shapes alone.
 * @param {any} part @param {any} model @param {ComposeContext & { layout?: string }} ctx
 */
export function compose(part, model, ctx) {
  const layout = ctx.layout ?? part?.screen?.kind ?? 'list';
  const loading = !model;
  const data = isRecord(model?.data) ? model.data : {};
  // The row actions of the screen and the answers preflight gave its rows (WG-5466).
  const rowActions = Object.entries(isRecord(part?.actions) ? part.actions : {})
    .filter(([, action]) => isRecord(action) && typeof action.row === 'string')
    .map(([id, action]) => ({ id, ...action }));
  const context = {
    ...ctx,
    people: isRecord(model?.people) ? model.people : ctx.people ?? {},
    rowActions,
    affordances: isRecord(model?.affordances) ? model.affordances : {},
  };
  const declared = Object.keys(isRecord(part?.sections) ? part.sections : {});
  const names = declared.length ? [...declared, ...Object.keys(data).filter((n) => !declared.includes(n))] : Object.keys(data).sort();
  const sections = names.map((name) => sectionOf(part, name, data[name]));
  const hasSingle = sections.some((s) => s.single && !s.aggregate);
  const lists = sections.filter((s) => !s.single && !s.aggregate);

  /** @type {any[]} */
  const main = [];
  /** @type {any[]} */
  const aside = [];
  /** @type {any[]} */
  const leading = [];

  for (const section of sections) {
    const value = data[section.name];
    if (loading) {
      const guess = patternGuess(section);
      (guess === 'aggregate-bars' || guess === 'notification-list' ? aside : main).push(base(section, 'loading', guess));
      continue;
    }
    if (!(section.name in data)) {
      (section.aggregate ? aside : main).push(base(section, 'error', patternGuess(section)));
      continue;
    }
    if (section.aggregate) {
      aside.push(aggregateNode(section, value, context));
      continue;
    }
    if (section.single) {
      leading.push(detailNode(section, value, context));
      continue;
    }
    const rows = Array.isArray(value) ? value : [];
    const more = typeof model?.next?.[section.name] === 'string' ? model.next[section.name] : null;
    if (isNotices(section, rows) || /notification|notice|inbox/.test(localName(section.object))) {
      const node = noticeNode(section, rows, context);
      (lists.length > 1 || hasSingle ? aside : main).push(node);
      continue;
    }
    // A short list beside a record (a few fields, no body) sits in the side column.
    const roles = rolesOf(section, rows);
    const small = !roles.body && roles.shown.length <= 3;
    const secondary = (hasSingle && (layout === 'editor' || small)) || (small && lists.length > 1 && rows.length > 0 && main.length > 0);
    const region = secondary ? 'aside' : 'main';
    (region === 'aside' ? aside : main).push(listNode(section, rows, context, region, more));
  }
  // A record shown in detail leads the page.
  main.unshift(...leading);

  const formIds = Object.keys(isRecord(part?.forms) ? part.forms : {}).sort();
  const collapsible = formIds.length > 2 && layout !== 'editor';
  const forms = formIds.map((id, i) => formNode(id, part.forms[id], ctx.forms?.[id], { collapsible, open: i === 0 }));
  if (layout === 'editor' || layout === 'detail') main.push(...forms);
  else if (layout === 'settings' || main.length > 0) aside.unshift(...forms);
  else main.push(...forms);

  return {
    template: aside.length && main.length ? 'split' : 'single',
    state: loading ? 'loading' : 'ready',
    main: main.length ? main : aside,
    aside: main.length ? aside : [],
  };
}

/** Every pattern node of a page model, main column first. @param {{ main: any[], aside: any[] }} page */
export function nodesOf(page) {
  return [...(page?.main ?? []), ...(page?.aside ?? [])];
}
