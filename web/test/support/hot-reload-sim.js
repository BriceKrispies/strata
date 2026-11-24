// A deterministic simulator of several browser tabs running the real client kernel against a fake
// gateway, for the live update of a tenant's interface definition (WG-5344, WG-5346).
//
// - `Tab` holds the kernel state of one tab, steps it with `update`, routes the effects it returns
//   to the gateway, draws with `view` and checks the invariants I1-I7 after every step.
// - `FakeGateway` is the server: the manifest and the parts by hash, records per object, view
//   models built from them, operations and the notices they emit.
// - `Scheduler` keeps every response and notice in flight in a queue per tab; a scenario decides
//   what is delivered, in which order, and what is lost.
// - `Simulator` ties them together and adds `quiesce()`, which delivers everything.
//
// Nothing here changes the client: it only calls `initialState`, `update` and `view`.
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { initialState, update } from '../../src/kernel/update.js';
import { view } from '../../src/kernel/view.js';
import { matchTenantRoute, tenantScreenOfView } from '../../src/kernel/tenant-ui.js';
import { nodeText } from '../../src/kernel/compose.js';

/** @typedef {{ hash: string, schema_version: number, parts: any[] }} Manifest */
/** @typedef {{ manifest: Manifest, parts: Record<string, any> }} Version */

/** The session of a signed-in member, as the gateway reports it. */
export const MEMBER = { authenticated: true, tenant: 'acme', tenant_name: 'Acme', principal: 'user:ines', display_name: 'Ines', roles: ['member'], may: {}, may_manage_schema: false, csrf_token: 'tok' };

/** Raised when an invariant fails. Names the invariant, the tab, the step and the message. */
export class InvariantError extends Error {
  /** @param {string} invariant @param {string} tab @param {number} step @param {string} msg @param {string} detail */
  constructor(invariant, tab, step, msg, detail) {
    super(`${invariant} violated on tab ${tab}, step ${step}, message ${msg}: ${detail}`);
    this.name = 'InvariantError';
    this.invariant = invariant;
    this.tab = tab;
    this.step = step;
    this.msg = msg;
    this.detail = detail;
  }
}

/** A small seeded generator (mulberry32), for interleavings that are the same on every run. @param {number} seed */
export function seeded(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const clone = (value) => structuredClone(value);
const sha = (text) => createHash('sha256').update(text).digest('hex');

/** Read a version from a directory holding `manifest.json` and `parts/<hash>.json`. @param {string} dir @returns {Version} */
export function loadVersionDir(dir) {
  const manifest = JSON.parse(readFileSync(join(dir, 'manifest.json'), 'utf8'));
  /** @type {Record<string, any>} */
  const parts = {};
  for (const file of readdirSync(join(dir, 'parts'))) {
    if (file.endsWith('.json')) parts[file.slice(0, -5)] = JSON.parse(readFileSync(join(dir, 'parts', file), 'utf8'));
  }
  return { manifest, parts };
}

const PART_PATH = '/api/ui/parts/';

/** What the request of an effect is for, from its path. @param {{ path: string }} effect */
function requestKind(effect) {
  const path = effect.path.split('?')[0];
  if (path.startsWith(PART_PATH)) return 'ui.part';
  if (path === '/api/ui') return 'ui.manifest';
  if (path === '/api/view') return 'view';
  if (path === '/api/session') return 'session';
  if (path === '/api/operations/invoke') return 'operation';
  return 'other';
}

/** The hashes of the parts a tab needs from its manifest: the errors part and the shown screen's. */
function neededHashes(state) {
  const manifest = state.ui?.manifest;
  if (!manifest) return [];
  const shown = tenantScreenOfView(state.ui, state.view?.model?.view)?.id;
  const routed = matchTenantRoute(state.ui, state.route.path, state.route.query)?.screen;
  return manifest.parts.filter((e) => e.kind === 'errors' || (e.kind === 'screen' && (e.id === shown || e.id === routed))).map((e) => e.hash);
}

// ---------------------------------------------------------------------------------------------
// The gateway

/** The server side: what the tenant has installed, its records and its operations. */
export class FakeGateway {
  /** @param {{ version?: Version }} [options] */
  constructor(options = {}) {
    /** @type {Manifest} */
    this.manifest = { hash: sha('empty'), schema_version: 1, parts: [] };
    /** @type {Map<string, any>} Parts by hash; the parts of earlier versions stay, as on the real gateway. */
    this.parts = new Map();
    /** @type {Record<string, Version>} */
    this.versions = {};
    /** Each install or upgrade is an epoch, with the parts it changed. @type {{ version: string, changed: Set<string> }[]} */
    this.epochs = [{ version: '(initial)', changed: new Set() }];
    /** @type {Record<string, Map<number, any>>} */
    this.records = {};
    /** @type {Record<string, number>} */
    this.nextId = {};
    /** @type {Record<string, any>} */
    this.views = {};
    /** @type {Record<string, (inputs: any, gateway: FakeGateway) => { status: number, body: any }>} */
    this.operations = {};
    /** @type {Map<string, { status: number, body: any }>} */
    this.keys = new Map();
    this.head = 40;
    /** @type {{ seq: number, notice: any }[]} */
    this.log = [];
    /** @type {Map<string, any>} The open connection of each tab, by tab name. */
    this.connections = new Map();
    /** @type {Set<string>} Tabs whose next resume is answered with a resync frame. */
    this.resyncNext = new Set();
    if (options.version) this.install(options.version, { silent: true });
  }

  /** The epoch the gateway is in: the number of installs and upgrades so far. */
  get epoch() {
    return this.epochs.length - 1;
  }

  /** Register a version under a name: inline `{ manifest, parts }` or `{ dir }`. @param {string} name @param {Version | { dir: string }} spec */
  addVersion(name, spec) {
    this.versions[name] = 'dir' in spec ? loadVersionDir(spec.dir) : spec;
    return this;
  }

  /**
   * Install a version: swap the manifest, bump the schema version and tell every connected tab.
   * @param {string | Version} version A registered name or an inline version.
   * @param {{ silent?: boolean, schemaVersion?: number }} [options]
   */
  install(version, options = {}) {
    const found = typeof version === 'string' ? this.versions[version] : version;
    if (!found) throw new Error(`no version ${String(version)}`);
    const manifest = clone(found.manifest);
    const wanted = options.schemaVersion ?? (manifest.schema_version > this.manifest.schema_version ? manifest.schema_version : this.manifest.schema_version + 1);
    manifest.schema_version = wanted;
    manifest.hash ??= sha(JSON.stringify(manifest.parts));
    const before = new Set(this.manifest.parts.map((e) => e.hash));
    for (const [hash, part] of Object.entries(found.parts ?? {})) this.parts.set(hash, clone(part));
    this.manifest = manifest;
    // A silent install is the state the scenario starts from: no notice, and not an upgrade for I6.
    if (!options.silent) {
      this.epochs.push({ version: typeof version === 'string' ? version : '(inline)', changed: new Set(manifest.parts.map((e) => e.hash).filter((h) => !before.has(h))) });
      this.emit({ kind: 'schema', version: wanted });
    }
    return this;
  }

  /** The same as `install`: an upgrade is an install over an earlier one. @param {string | Version} version @param {{ silent?: boolean, schemaVersion?: number }} [options] */
  upgrade(version, options) {
    return this.install(version, options);
  }

  /** Remove the product: an empty manifest at a higher schema version. */
  uninstall() {
    return this.install({ manifest: { hash: sha(`none-${this.head}-${this.epochs.length}`), schema_version: 0, parts: [] }, parts: {} });
  }

  /** The gateway no longer serves this part: a request for it answers 404. @param {string} hash */
  dropPart(hash) {
    this.parts.delete(hash);
  }

  /** Declare a view: `{ title?, dependsOn: string[], sections: { name: { object, where? } } }`. @param {string} name @param {any} spec */
  defineView(name, spec) {
    this.views[name] = spec;
    return this;
  }

  /** Declare an operation by its handler. @param {string} name @param {(inputs: any, gateway: FakeGateway) => { status: number, body: any }} handler */
  defineOperation(name, handler) {
    this.operations[name] = handler;
    return this;
  }

  /**
   * An operation that creates a record of `object` from its inputs, refusing (422) when a required
   * input is empty, as the substrate does with `required:<object>.<field>`.
   * @param {string} name @param {string} object @param {{ required?: string[] }} [options]
   */
  defineCreate(name, object, options = {}) {
    return this.defineOperation(name, (inputs, gateway) => {
      const failed = (options.required ?? []).filter((f) => String(inputs[f] ?? '').trim() === '').map((f) => ({ rule: `required:${object}.${f}`, detail: `${f} is required`, passed: false }));
      if (failed.length) return { status: 422, body: { code: 'invalid', message: 'The request was refused.', failed } };
      const record = gateway.createRecord(object, inputs);
      return { status: 200, body: { result: { id: record.id }, event_seq: gateway.head, checks_passed: 1, dry_run: false } };
    });
  }

  /** @param {string} object @param {Record<string, any>} fields */
  createRecord(object, fields) {
    const table = (this.records[object] ??= new Map());
    const id = (this.nextId[object] = (this.nextId[object] ?? 0) + 1);
    const record = { id, version: 1, fields: { ...fields }, refs: {} };
    table.set(id, record);
    this.emit({ kind: 'upsert', object, id, version: 1 });
    return record;
  }

  /** @param {string} object @param {number} id @param {Record<string, any>} fields */
  updateRecord(object, id, fields) {
    const record = this.records[object]?.get(id);
    if (!record) throw new Error(`no ${object} ${id}`);
    record.fields = { ...record.fields, ...fields };
    record.version += 1;
    this.emit({ kind: 'upsert', object, id, version: record.version });
    return record;
  }

  /** Record a notice and send it to every subscribed connection. @param {any} notice */
  emit(notice) {
    const entry = { seq: (this.head += 1), notice };
    this.log.push(entry);
    for (const conn of this.connections.values()) if (conn.subscribed) this.pushEvent(conn, entry);
    return entry.seq;
  }

  /** @param {any} conn @param {{ seq: number, notice: any }} entry */
  pushEvent(conn, entry) {
    conn.send({ type: 'event', sub: 'site', n: (conn.n += 1), seq: entry.seq, correlation: '', notice: entry.notice });
  }

  /** Open a connection for a tab; `send(frame)` queues a frame for it. @param {string} name @param {(frame: any) => void} send */
  connect(name, send) {
    const conn = { name, send, n: 0, subscribed: false, resync: this.resyncNext.delete(name) };
    this.connections.set(name, conn);
    send({ type: 'welcome', v: 1, conn: `conn-${name}-${this.head}`, head: this.head, session: 'active', heartbeat_ms: 10000 });
    return conn;
  }

  /** @param {string} name */
  disconnect(name) {
    this.connections.delete(name);
  }

  /** A `subscribe` of a connection: resume after a sequence, else start at the head. @param {any} conn @param {number | undefined} after */
  subscribe(conn, after) {
    conn.subscribed = true;
    if (after === undefined) return;
    if (conn.resync) {
      conn.send({ type: 'resync', reason: 'too far behind to replay', head: this.head });
      return;
    }
    for (const entry of this.log) if (entry.seq > after) this.pushEvent(conn, entry);
  }

  /** The view model of a view as the gateway projects it. @param {string} name @param {string} path @param {Record<string, string>} params */
  buildModel(name, path, params) {
    const spec = this.views[name] ?? { dependsOn: [], sections: {} };
    /** @type {Record<string, any[]>} */
    const data = {};
    for (const [section, def] of Object.entries(spec.sections ?? {})) {
      const rows = [...(this.records[def.object]?.values() ?? [])].filter((r) => (def.where ? def.where(r) : true));
      data[section] = rows.map((r) => ({ id: r.id, version: r.version, fields: { ...r.fields }, refs: {} }));
    }
    return { view: name, title: spec.title ?? name, path, params, depends_on: spec.dependsOn ?? [], revision: this.head, schema_version: this.manifest.schema_version, theme: { accent: 'moss', paper: 'linen' }, data, next: {} };
  }

  /**
   * Answer a request of a tab, now: the answer is a snapshot of the gateway at this moment.
   * @param {{ method?: string, path: string, body?: any, idempotencyKey?: string }} request
   * @param {{ session?: any }} [who]
   * @returns {{ status: number, body: any }}
   */
  handle(request, who = {}) {
    const url = new URL(request.path, 'http://gateway');
    const path = url.pathname;
    if (path === '/api/ui') return { status: 200, body: clone(this.manifest) };
    if (path.startsWith(PART_PATH)) {
      const part = this.parts.get(path.slice(PART_PATH.length));
      return part ? { status: 200, body: clone(part) } : { status: 404, body: { code: 'not_found', message: 'No such part.' } };
    }
    if (path === '/api/session') return { status: 200, body: { session: who.session ?? MEMBER } };
    if (path === '/api/view') {
      const asked = url.searchParams.get('path') ?? '/';
      const query = Object.fromEntries([...url.searchParams].filter(([k]) => k !== 'path'));
      const hit = matchTenantRoute({ manifest: this.manifest, parts: {}, asked: {} }, asked, query);
      if (!hit?.entry.view) return { status: 404, body: { code: 'not_found', message: 'No such page.' } };
      return { status: 200, body: this.buildModel(hit.entry.view, asked, hit.params) };
    }
    if (path === '/api/operations/invoke' && request.method === 'POST') {
      const key = request.idempotencyKey;
      if (key && this.keys.has(key)) return clone(this.keys.get(key));
      const handler = this.operations[request.body?.operation];
      const answer = handler ? handler(request.body.inputs ?? {}, this) : { status: 404, body: { code: 'not_found', message: 'No such operation.' } };
      if (key && answer.status < 400) this.keys.set(key, clone(answer));
      return answer;
    }
    return { status: 404, body: { code: 'not_found', message: 'No such endpoint.' } };
  }
}

// ---------------------------------------------------------------------------------------------
// The scheduler

/**
 * What is in flight to a tab: a response (`http`) or a frame of the socket (`ws`, where `frameType`
 * is the frame's `type`, or `connect` for the socket opening).
 * @typedef {{ id: number, kind: 'http' | 'ws', epoch: number, path?: string, method?: string, status?: number, frame?: any, frameType?: string, notice?: any, hold: number, deliver: () => void, failWith?: () => void, discarded?: boolean }} Item
 */

/** The queues of what is in flight to each tab, and the means to deliver, reorder or lose it. */
export class Scheduler {
  /** @param {Simulator} sim */
  constructor(sim) {
    this.sim = sim;
    /** @type {Map<string, Item[]>} */
    this.queues = new Map();
    this.nextItem = 1;
  }

  /** @param {Tab | string} tab @returns {Tab} */
  tab(tab) {
    return typeof tab === 'string' ? this.sim.tab(tab) : tab;
  }

  /** The items in flight to the tab, in queue order. @param {Tab | string} tab @returns {Item[]} */
  queue(tab) {
    const name = this.tab(tab).name;
    if (!this.queues.has(name)) this.queues.set(name, []);
    return /** @type {Item[]} */ (this.queues.get(name));
  }

  /** @param {Tab} tab @param {Omit<Item, 'id' | 'hold'>} item */
  enqueue(tab, item) {
    const queued = { ...item, id: this.nextItem++, hold: 0 };
    this.queue(tab).push(queued);
    return queued;
  }

  /** Deliver the first item the tab has that is not held back. Returns it, or null when none is in flight. @param {Tab | string} tab */
  deliverNext(tab) {
    const t = this.tab(tab);
    const queue = this.queue(t);
    const item = queue.find((i) => i.hold <= t.delivered) ?? queue[0];
    return item ? this.#deliverItem(t, item) : null;
  }

  /** Deliver the first item matching a predicate; throws when none does. @param {Tab | string} tab @param {(item: Item) => boolean} predicate */
  deliver(tab, predicate) {
    const t = this.tab(tab);
    const item = this.queue(t).find(predicate);
    if (!item) throw new Error(`nothing in flight to tab ${t.name} matches; in flight: ${this.describe(t)}`);
    return this.#deliverItem(t, item);
  }

  /** Put the tab's queue in another order: `order` receives the items and returns them rearranged. @param {Tab | string} tab @param {(items: Item[]) => Item[]} order */
  reorder(tab, order) {
    const queue = this.queue(tab);
    const next = order([...queue]);
    if (next.length !== queue.length || next.some((i) => !queue.includes(i))) throw new Error('reorder must return the same items');
    queue.splice(0, queue.length, ...next);
  }

  /** Lose the matching items: the tab never hears of them. Returns how many. @param {Tab | string} tab @param {(item: Item) => boolean} predicate */
  drop(tab, predicate) {
    const queue = this.queue(tab);
    const kept = queue.filter((i) => !predicate(i));
    const lost = queue.length - kept.length;
    queue.splice(0, queue.length, ...kept);
    return lost;
  }

  /** Make matching responses fail as a dead network would: the tab is told status 0. @param {Tab | string} tab @param {(item: Item) => boolean} predicate */
  fail(tab, predicate) {
    for (const item of this.queue(tab).filter((i) => i.kind === 'http' && i.failWith && predicate(i))) {
      item.status = 0;
      item.deliver = /** @type {() => void} */ (item.failWith);
    }
  }

  /** Hold the matching items back until the tab has been delivered `deliveries` other items (a release happens when only held items remain). @param {Tab | string} tab @param {(item: Item) => boolean} predicate @param {number} [deliveries] */
  delay(tab, predicate, deliveries = 1) {
    const t = this.tab(tab);
    for (const item of this.queue(t).filter(predicate)) item.hold = t.delivered + deliveries;
  }

  /** The items in flight to a tab, as text. @param {Tab | string} tab */
  describe(tab) {
    return this.queue(tab).map((i) => (i.kind === 'http' ? `http ${i.method} ${i.path} -> ${i.status}` : `ws ${i.frameType}${i.notice ? `:${i.notice.kind}` : ''}`)).join(' | ') || '(nothing)';
  }

  /** @param {Tab} tab @param {Item} item */
  #deliverItem(tab, item) {
    const queue = this.queue(tab);
    queue.splice(queue.indexOf(item), 1);
    tab.delivered += 1;
    // A frame of a socket the tab has since lost is never heard.
    if (item.kind === 'ws' && item.epoch !== tab.epoch) return { ...item, discarded: true };
    item.deliver();
    return item;
  }
}

// ---------------------------------------------------------------------------------------------
// The tab

/** The default bound on the effects one message may produce (I7). */
const MAX_EFFECTS = 12;

/**
 * One tab: the client's kernel state, stepped by `update`, drawn by `view`.
 *
 * `options.kernel` replaces the kernel's `update` or `view` (a test of an invariant feeds a broken
 * one); `options.maxEffects` is the bound of I7.
 */
export class Tab {
  /**
   * @param {Simulator} sim @param {string} name
   * @param {{ path?: string, session?: any, kernel?: { update?: typeof update, view?: typeof view }, maxEffects?: number }} [options]
   */
  constructor(sim, name, options = {}) {
    this.sim = sim;
    this.name = name;
    this.path = options.path ?? '/';
    this.session = options.session ?? MEMBER;
    this.update = options.kernel?.update ?? update;
    this.view = options.kernel?.view ?? view;
    this.maxEffects = options.maxEffects ?? MAX_EFFECTS;
    this.state = initialState();
    this.stepNo = 0;
    /** How many items have been delivered to the tab. */
    this.delivered = 0;
    /** The socket's generation: a frame of an earlier one is never heard. */
    this.epoch = 0;
    this.connected = false;
    /** Every request the tab issued. @type {{ step: number, id: number, method: string, path: string, kind: string, epoch: number, hash?: string }[]} */
    this.requests = [];
    /** The token of the latest timer of each name. @type {Record<string, number>} */
    this.timers = {};
    /** @type {{ step: number, msg: string }[]} */
    this.history = [];
    /** What a scenario typed that has not been sent. @type {{ screen: string, form: string, name: string, value: string }[]} */
    this.drafts = [];
    /** The operations the tab submitted, by request id. @type {Map<number, { owner: string, kind: string, step: number, resolved: boolean }>} */
    this.ops = new Map();
    /** The changed parts of each upgrade that the tab needed. @type {Record<number, Set<string>>} */
    this.needed = {};
    this.maxVersion = -Infinity;
    /** Whether the client closed its own socket and its reconnect timer should be allowed to fire. */
    this.autoReconnect = false;
  }

  /** The drawn page: the kernel's `view` of the state. */
  draw() {
    return this.view(this.state);
  }

  /** Part fetches issued, optionally of one hash. @param {string} [hash] */
  partFetches(hash) {
    return this.requests.filter((r) => r.kind === 'ui.part' && (hash === undefined || r.hash === hash));
  }

  /** Open the page at its path, with the session the gateway would put in the bootstrap. */
  open() {
    return this.step({ type: 'init', path: this.path, search: '', bootstrap: { path: this.path, session: { session: this.session } }, systemDark: false, storedTheme: null });
  }

  // What a person does ---------------------------------------------------------------------

  /** @param {string} href */
  navigate(href) {
    this.drafts = [];
    return this.step({ type: 'navigate', href });
  }

  /** Type into a field of the tenant form `form`. The draft is tracked for I4 when the page holds the form. @param {string} form @param {string} name @param {string} value */
  type(form, name, value) {
    const entry = tenantScreenOfView(this.state.ui, this.state.view?.model?.view);
    const part = entry ? this.state.ui.parts[entry.hash] : null;
    const result = this.step({ type: 'input', form: `tenant:${form}`, name, value });
    if (entry && part?.forms?.[form]?.fields?.some((f) => f.name === name)) {
      this.drafts = this.drafts.filter((d) => !(d.form === form && d.name === name));
      this.drafts.push({ screen: entry.id, form, name, value: String(value) });
    }
    return result;
  }

  /** Submit the tenant form `form` with the values it holds. @param {string} form */
  submit(form) {
    return this.step({ type: 'submit', form: `tenant:${form}`, values: {} });
  }

  /** Press an action of the tenant screen. @param {string} action */
  press(action) {
    return this.step({ type: 'action', name: 'tenant.action', arg: action });
  }

  /** Step a message of any kind, as a test of an invariant that feeds a broken sequence does. @param {any} msg */
  inject(msg) {
    return this.step(msg);
  }

  // The kernel -------------------------------------------------------------------------------

  /** The id of the latest pending request of a kind, or undefined. @param {string} kind */
  pendingId(kind) {
    const ids = Object.entries(this.state.pending ?? {}).filter(([, p]) => /** @type {any} */ (p).kind === kind).map(([id]) => Number(id));
    return ids.length ? Math.max(...ids) : undefined;
  }

  /**
   * Apply a message, route the effects, draw and check the invariants.
   * @param {any} message
   */
  step(message) {
    this.stepNo += 1;
    const n = (this.sim.clock += 1);
    const msg = { at: 1_000 + n * 250, rand: ((Math.imul(n, 2654435761) >>> 0) % 1_000_003) / 1_000_003, ...message };
    const label = this.#label(msg);
    this.history.push({ step: this.stepNo, msg: label });
    const before = this.state;
    /** @type {(invariant: string, detail: string) => never} */
    const fail = (invariant, detail) => {
      throw new InvariantError(invariant, this.name, this.stepNo, label, detail);
    };
    let result;
    try {
      result = this.update(before, msg);
    } catch (error) {
      return fail('I7', `the kernel threw: ${/** @type {Error} */ (error)?.stack ?? error}`);
    }
    if (result.effects.length > this.maxEffects) fail('I7', `${result.effects.length} effects for one message (bound ${this.maxEffects}): ${result.effects.map((e) => e.type).join(', ')}`);
    this.state = result.state;
    if (msg.type === 'ws_closed') this.connected = false;
    this.#route(result.effects);
    this.#bookkeep(before, msg, result.effects, fail);
    this.#check(fail);
    if (result.effects.some((e) => e.type === 'ws_close')) this.#closeSocket();
    return result;
  }

  /** The client closed its socket: it hears of the close at once. */
  #closeSocket() {
    this.epoch += 1;
    this.connected = false;
    this.sim.gateway.disconnect(this.name);
    this.sim.scheduler.drop(this, (i) => i.kind === 'ws');
    // The client decided to close, so it will reconnect by itself: `quiesce` lets it.
    this.autoReconnect = true;
    this.step({ type: 'ws_closed' });
  }

  /** @param {any} msg */
  #label(msg) {
    if (msg.type === 'http_result') {
      const asked = this.requests.find((r) => r.id === msg.id);
      return `http_result#${msg.id}(${asked?.path ?? '?'} ${msg.status})`;
    }
    if (msg.type === 'ws_message') return `ws:${msg.message.type}${msg.message.notice ? `:${msg.message.notice.kind}` : ''}`;
    if (msg.type === 'input') return `input(${msg.form}.${msg.name})`;
    return msg.type + (msg.href ? `(${msg.href})` : msg.form ? `(${msg.form})` : '');
  }

  /** Send the effects where they go: requests to the gateway, the socket to its connection. @param {any[]} effects */
  #route(effects) {
    const { gateway, scheduler } = this.sim;
    for (const effect of effects) {
      if (effect.type === 'http') {
        const kind = requestKind(effect);
        this.requests.push({ step: this.stepNo, id: effect.id, method: effect.method, path: effect.path, kind, epoch: gateway.epoch, hash: kind === 'ui.part' ? effect.path.slice(PART_PATH.length) : undefined });
        if (kind === 'operation') this.ops.set(effect.id, { owner: '', kind: effect.body?.operation ?? '', step: this.stepNo, resolved: false });
        const answer = gateway.handle(effect, { session: this.session });
        const reply = (/** @type {number} */ status, /** @type {any} */ body) => this.step({ type: 'http_result', id: effect.id, ok: status >= 200 && status < 400, status, body });
        scheduler.enqueue(this, {
          kind: 'http', epoch: this.epoch, path: effect.path, method: effect.method, status: answer.status,
          deliver: () => reply(answer.status, answer.body),
          failWith: () => reply(0, undefined),
        });
      } else if (effect.type === 'ws_connect') {
        scheduler.enqueue(this, { kind: 'ws', epoch: this.epoch, frameType: 'connect', deliver: () => this.#open() });
      } else if (effect.type === 'ws_send' && effect.message?.type === 'subscribe') {
        const conn = gateway.connections.get(this.name);
        if (conn) gateway.subscribe(conn, effect.message.after);
      } else if (effect.type === 'timer') {
        this.timers[effect.name] = effect.token;
      }
      // Pings, acks, scrolling, storing and logging need no answer here.
    }
  }

  /** The socket opens: the gateway welcomes it, and the frames it sends after are queued. */
  #open() {
    const epoch = this.epoch;
    this.connected = true;
    this.step({ type: 'ws_open' });
    this.sim.gateway.connect(this.name, (frame) => {
      this.sim.scheduler.enqueue(this, {
        kind: 'ws', epoch, frameType: frame.type, frame, notice: frame.notice,
        deliver: () => this.step({ type: 'ws_message', message: frame }),
      });
    });
    // The welcome arrives with the open, as it does on the socket.
    this.sim.scheduler.deliver(this, (i) => i.kind === 'ws' && i.frameType === 'welcome');
  }

  // Bookkeeping for the invariants -------------------------------------------------------------

  /** @param {any} before @param {any} msg @param {any[]} effects @param {(i: string, d: string) => never} fail */
  #bookkeep(before, msg, effects, fail) {
    for (const effect of effects) {
      if (effect.type === 'http' && requestKind(effect) === 'operation') {
        const op = this.ops.get(effect.id);
        if (op) op.owner = this.state.pending?.[effect.id]?.owner ?? '';
      }
    }
    if (msg.type !== 'http_result') return;
    const pending = before.pending?.[msg.id];
    if (!pending) return;
    // A form that was sent and accepted has no draft left.
    if (pending.kind === 'tenant.submit' && msg.ok) this.drafts = this.drafts.filter((d) => d.form !== pending.owner);
    const op = this.ops.get(msg.id);
    if (!op) return;
    if (op.resolved) fail('I5', `operation ${op.kind} answered twice`);
    op.resolved = true;
    const known = new Set(before.toasts.map((/** @type {any} */ t) => t.id));
    const toasted = this.state.toasts.some((/** @type {any} */ t) => !known.has(t.id));
    const form = this.state.tenant?.forms?.[pending.owner];
    const refusalShown = !!form && (form.error !== '' || Object.keys(form.errors ?? {}).length > 0);
    const refreshed = effects.some((e) => e.type === 'http' && requestKind(e) === 'view') || effects.some((e) => e.type === 'push_url');
    if (!(toasted || refusalShown || (msg.ok && refreshed))) fail('I5', `operation ${op.kind} (${msg.ok ? 'accepted' : `refused, status ${msg.status}`}) shows no outcome: no message, no error on the form, no refresh`);
  }

  // The invariants ---------------------------------------------------------------------------

  /** I1, I2, I4 and I6, after every step. @param {(i: string, d: string) => never} fail */
  #check(fail) {
    const state = this.state;
    const gateway = this.sim.gateway;
    const manifest = state.ui?.manifest ?? null;
    const drawn = this.draw();
    const page = drawn.page;

    // I2: the accepted schema version never decreases.
    if (manifest) {
      if (manifest.schema_version < this.maxVersion) fail('I2', `accepted schema_version went from ${this.maxVersion} down to ${manifest.schema_version}`);
      this.maxVersion = Math.max(this.maxVersion, manifest.schema_version);
    }

    // I1: a drawn tenant screen's part is listed in the newest manifest the tab accepted.
    if (page.kind === 'tenant') {
      const entry = manifest?.parts.find((/** @type {any} */ e) => e.kind === 'screen' && e.id === page.screen);
      if (!entry) fail('I1', `drawn tenant screen ${page.screen} is not listed in the accepted manifest ${manifest?.hash ?? '(none)'}`);
      else if (!(entry.hash in state.ui.parts) && !(entry.hash in state.ui.asked && state.ui.held?.[`screen:${entry.id}`])) fail('I1', `drawn tenant screen ${page.screen} shows a part other than the one the accepted manifest lists (${entry.hash.slice(0, 8)}); the tab holds ${Object.keys(state.ui.parts).map((h) => h.slice(0, 8)).join(',') || 'no parts'}`);
    }

    // I4: no draft value is lost unless its field was removed and the loss is shown.
    this.#checkDrafts(state, drawn, fail);

    // I6: part fetches per upgrade are at most the changed parts the tab needs.
    const needed = neededHashes(state);
    for (let e = 1; e <= gateway.epoch; e += 1) {
      const changed = gateway.epochs[e].changed;
      const set = (this.needed[e] ??= new Set());
      for (const h of needed) if (changed.has(h)) set.add(h);
      const fetched = this.requests.filter((r) => r.kind === 'ui.part' && r.epoch === e);
      if (fetched.length > set.size) fail('I6', `${fetched.length} part fetches in upgrade ${e} (${gateway.epochs[e].version}) but the tab needs ${set.size} changed part(s); fetched ${fetched.map((r) => r.hash?.slice(0, 8)).join(',')}`);
    }
  }

  /** @param {any} state @param {any} drawn @param {(i: string, d: string) => never} fail */
  #checkDrafts(state, drawn, fail) {
    const model = state.view?.model;
    if (!model) return;
    const entry = tenantScreenOfView(state.ui, model.view);
    const part = entry ? state.ui.parts[entry.hash] : null;
    const kept = [];
    for (const draft of this.drafts) {
      const here = entry?.id === draft.screen;
      if (entry && here && !part) {
        kept.push(draft); // The part is on its way.
        continue;
      }
      const form = part?.forms?.[draft.form];
      const field = form?.fields?.find((/** @type {any} */ f) => f.name === draft.name);
      if (here && field) {
        const held = state.tenant?.forms?.[draft.form]?.values?.[draft.name];
        if (held !== draft.value) fail('I4', `draft ${draft.form}.${draft.name} = ${JSON.stringify(draft.value)} is now ${JSON.stringify(held)}`);
        kept.push(draft);
        continue;
      }
      // The field is gone (or its screen): the loss must be shown.
      const words = [draft.name, ...(form?.fields ?? []).map((/** @type {any} */ f) => f.label)].filter(Boolean).map((w) => String(w).toLowerCase());
      const told = [...drawn.toasts.map((/** @type {any} */ t) => t.text), drawn.announce].filter(Boolean).join(' ').toLowerCase();
      if (!words.some((w) => told.includes(w))) fail('I4', `draft ${draft.form}.${draft.name} = ${JSON.stringify(draft.value)} was dropped with its field and nothing on the page says so`);
    }
    this.drafts = kept;
  }

  /** Checks that hold only once nothing is in flight: I3, and the end of I5. */
  checkQuiescent() {
    /** @type {(invariant: string, detail: string) => never} */
    const fail = (invariant, detail) => {
      throw new InvariantError(invariant, this.name, this.stepNo, '(quiescent)', detail);
    };
    const want = this.sim.gateway.manifest;
    const held = this.state.ui?.manifest;
    if (this.connected && held?.hash !== want.hash) {
      fail('I3', `connected, but holds manifest ${String(held?.hash).slice(0, 8)} (schema_version ${held?.schema_version}) while the gateway has ${want.hash.slice(0, 8)} (schema_version ${want.schema_version})`);
    }
    for (const [id, op] of this.ops) {
      if (!op.resolved) fail('I5', `operation ${op.kind} (request ${id}, step ${op.step}) never showed an outcome`);
    }
  }
}

// ---------------------------------------------------------------------------------------------
// The simulator

/** The tabs, the gateway and the scheduler of one scenario. */
export class Simulator {
  /** @param {{ gateway?: FakeGateway, version?: Version }} [options] */
  constructor(options = {}) {
    this.gateway = options.gateway ?? new FakeGateway({ version: options.version });
    this.scheduler = new Scheduler(this);
    /** @type {Map<string, Tab>} */
    this.tabs = new Map();
    this.clock = 0;
  }

  /** Add a tab and open its page. @param {string} name @param {ConstructorParameters<typeof Tab>[2] & { open?: boolean }} [options] */
  addTab(name, options = {}) {
    const tab = new Tab(this, name, options);
    this.tabs.set(name, tab);
    if (options.open !== false) tab.open();
    return tab;
  }

  /** @param {string} name */
  tab(name) {
    const tab = this.tabs.get(name);
    if (!tab) throw new Error(`no tab ${name}`);
    return tab;
  }

  /** @param {Tab | string} tab */
  deliverNext(tab) { return this.scheduler.deliverNext(tab); }
  /** @param {Tab | string} tab @param {(item: Item) => boolean} predicate */
  deliver(tab, predicate) { return this.scheduler.deliver(tab, predicate); }
  /** @param {Tab | string} tab @param {(items: Item[]) => Item[]} order */
  reorder(tab, order) { return this.scheduler.reorder(tab, order); }
  /** @param {Tab | string} tab @param {(item: Item) => boolean} predicate */
  drop(tab, predicate) { return this.scheduler.drop(tab, predicate); }
  /** @param {Tab | string} tab @param {(item: Item) => boolean} predicate @param {number} [deliveries] */
  delay(tab, predicate, deliveries) { return this.scheduler.delay(tab, predicate, deliveries); }

  /** The tab loses its socket and hears of it. Frames still on their way to it are lost. @param {Tab | string} tab */
  disconnect(tab) {
    const t = this.scheduler.tab(tab);
    t.epoch += 1;
    this.gateway.disconnect(t.name);
    this.scheduler.drop(t, (i) => i.kind === 'ws');
    return t.step({ type: 'ws_closed' });
  }

  /**
   * The tab's reconnect timer fires, so its socket opens again: the welcome (and the replay, or
   * with `resync` a resync frame in its place) waits in the tab's queue like anything else.
   * @param {Tab | string} tab @param {{ resync?: boolean }} [options]
   */
  reconnect(tab, options = {}) {
    const t = this.scheduler.tab(tab);
    const token = t.timers.reconnect;
    if (token === undefined) throw new Error(`tab ${t.name} has no reconnect timer`);
    delete t.timers.reconnect;
    t.autoReconnect = false;
    if (options.resync) this.gateway.resyncNext.add(t.name);
    t.step({ type: 'timer', name: 'reconnect', token });
    return t;
  }

  /** Deliver everything in flight to a tab, in queue order. @param {Tab | string} tab @param {number} [limit] */
  flush(tab, limit = 200) {
    const t = this.scheduler.tab(tab);
    let n = 0;
    while (this.scheduler.deliverNext(t)) if ((n += 1) > limit) throw new InvariantError('I7', t.name, t.stepNo, '(flush)', `still delivering after ${limit} deliveries`);
  }

  /**
   * Deliver everything until no queue has anything left, then check what holds at rest (I3, and
   * that every submitted operation showed its outcome). `maxSteps` bounds the deliveries, so an
   * effect loop fails (I7); `rng` (see `seeded`) picks the tab and the item at random.
   * @param {{ maxSteps?: number, rng?: () => number }} [options]
   */
  quiesce(options = {}) {
    const max = options.maxSteps ?? 500;
    let steps = 0;
    for (;;) {
      const ready = [...this.tabs.values()].filter((t) => this.scheduler.queue(t).length > 0);
      if (!ready.length) {
        // A tab whose client closed its own socket (a frame went missing) reconnects after its wait.
        const waiting = [...this.tabs.values()].find((t) => t.autoReconnect && t.timers.reconnect !== undefined);
        if (!waiting) break;
        waiting.autoReconnect = false;
        this.reconnect(waiting);
        continue;
      }
      if ((steps += 1) > max) {
        const worst = ready[0];
        throw new InvariantError('I7', worst.name, worst.stepNo, '(quiesce)', `still delivering after ${max} deliveries; in flight: ${this.scheduler.describe(worst)}`);
      }
      if (options.rng) {
        const tab = ready[Math.floor(options.rng() * ready.length)];
        // Replies may arrive in any order; the frames of the socket stay in order, as a socket's do.
        const queue = this.scheduler.queue(tab);
        const firstFrame = queue.find((i) => i.kind === 'ws');
        const eligible = queue.filter((i) => i.kind === 'http' || i === firstFrame);
        const item = eligible[Math.floor(options.rng() * eligible.length)];
        this.scheduler.deliver(tab, (i) => i === item);
      } else {
        this.scheduler.deliverNext(ready[steps % ready.length]);
      }
    }
    for (const tab of this.tabs.values()) tab.checkQuiescent();
    return steps;
  }
}

// ---------------------------------------------------------------------------------------------
// The page as text

/** The drawn page of a tab as plain text, for assertions. @param {Tab} tab */
export function pageText(tab) {
  const drawn = tab.draw();
  const page = drawn.page;
  const out = [`title: ${drawn.title}`];
  if (page.kind !== 'tenant') {
    out.push(`[${page.kind}]${page.message ? ` ${page.message}` : ''}`);
  } else {
    out.push(`# ${page.heading}`);
    for (const node of [...(page.main ?? []), ...(page.aside ?? [])]) {
      if (node.pattern === 'form') {
        out.push(`form ${node.form}${node.busy ? ' (sending)' : ''}`);
        for (const f of node.fields) out.push(`  ${f.label}: ${f.value}${f.error ? ` !${f.error}` : ''}`);
        out.push(`  [${node.submitLabel}]${node.error ? ` !${node.error}` : ''}`);
        continue;
      }
      out.push(`## ${node.label}${node.state === 'ready' ? '' : ` (${node.state})`}`);
      for (const line of nodeLines(node)) out.push(`- ${line}`.replace(/ +$/, ''));
    }
    for (const a of page.actions) out.push(`[${a.label}]${a.armed ? ' (confirm)' : ''}`);
  }
  for (const t of drawn.toasts) out.push(`toast(${t.kind}): ${t.text}`);
  return out.join('\n');
}

/** The lines a pattern node draws, as text: one per record, notice or group. @param {any} node @returns {string[]} */
function nodeLines(node) {
  if (node.state !== 'ready') return [];
  const facts = (/** @type {any[]} */ meta) => meta.map((m) => ` ${m.label}=${nodeText(m.value)}`).join('');
  switch (node.pattern) {
    case 'record-list':
      return node.items.map((/** @type {any} */ r) => `${r.title ?? ''}${r.state ? ` [${nodeText(r.state)}]` : ''}${facts(r.meta)}`);
    case 'data-table':
      return node.rows.map((/** @type {any} */ r) => r.cells.map((/** @type {any} */ c) => nodeText(c.value)).join(' | '));
    case 'record-detail':
      return [`${node.record.title ?? ''}${node.record.state ? ` [${nodeText(node.record.state)}]` : ''}${facts(node.facts)}`];
    case 'notification-list':
      return node.items.map((/** @type {any} */ i) => `${i.unread ? '* ' : ''}${i.text}`);
    case 'aggregate-bars':
      return node.bars.map((/** @type {any} */ b) => `${b.label}: ${b.text}`);
    case 'stat-tile':
      return [node.value];
    default:
      return [];
  }
}
