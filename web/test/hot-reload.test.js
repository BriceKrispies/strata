// The live update of a tenant's interface definition, with several tabs, a fake gateway and a
// scheduler that controls what arrives when (WG-5346). Every step of every tab is checked against
// the invariants I1-I7 of WG-5344; each invariant also has a test that feeds a broken sequence and
// asserts the check fires.
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';
import { InvariantError, Simulator, loadVersionDir, pageText, seeded } from './support/hot-reload-sim.js';

const h = (c) => c.repeat(64);
const SCREEN = 'ticket_desk__board_screen';
const FORM = 'ticket_desk__open_form';
const VIEW = 'ticket_desk__board';
const OPERATION = 'ticket_desk__open_ticket';
const OBJECT = 'ticket_desk__ticket';
const ERRORS = h('e');

const titleField = { name: 'title', input: 'title', label: 'Title', control: 'text', trim: true };
const summaryField = { name: 'summary', input: 'summary', label: 'Summary', control: 'text', trim: true };

const screenPart = (title, fields = [titleField]) => ({
  kind: 'screen',
  id: SCREEN,
  screen: { route: '/tickets', view: VIEW, title, kind: 'list', auth: 'session', query: [] },
  forms: { [FORM]: { screen: SCREEN, operation: OPERATION, fields, submit_label: 'Open the ticket', on_success: { toast: 'Ticket opened.' }, on_error: 'inline' } },
  actions: {},
  live: { ticket_desk__board_live: { screen: SCREEN, event: 'notice', kinds: ['membership'], objects: true } },
});

const errorsPart = {
  kind: 'errors',
  id: 'errors',
  messages: { ticket_desk__title_needed: { on: 'check', kinds: ['required'], text: 'A ticket needs a title.', field: 'name', applies_to: [FORM] } },
};

/** A version of the ticket desk product: the screen part named by `screenHash`, the errors part unchanged. */
function version(manifestHash, screenHash, title, fields) {
  return {
    manifest: {
      hash: manifestHash,
      schema_version: 0,
      parts: [
        { kind: 'screen', id: SCREEN, hash: screenHash, route: '/tickets', view: VIEW, title, layout: 'list', auth: 'session', query: [] },
        { kind: 'errors', id: 'errors', hash: ERRORS },
      ],
    },
    parts: { [screenHash]: screenPart(title, fields), [ERRORS]: errorsPart },
  };
}

const V1 = version(h('1'), h('a'), 'Open tickets');
const V2 = version(h('2'), h('b'), 'Open tickets (v2)');
const V2B = version(h('3'), h('c'), 'Open tickets (v2b)');
const V3 = version(h('4'), h('d'), 'Open tickets', [summaryField]);

/** A simulator at version 1 with a view, a create operation, one ticket and `names` tabs on the screen. */
function desk(...names) {
  return deskWith({}, ...names);
}

function deskWith(tabOptions, ...names) {
  const sim = new Simulator();
  const gateway = sim.gateway;
  gateway.addVersion('v1', V1).addVersion('v2', V2).addVersion('v2b', V2B).addVersion('v3', V3);
  gateway.install('v1', { silent: true });
  gateway.defineView(VIEW, { title: 'Tickets', dependsOn: [OBJECT], sections: { open: { object: OBJECT } } });
  gateway.defineCreate(OPERATION, OBJECT, { required: ['title'] });
  gateway.createRecord(OBJECT, { title: 'Leak', status: 'open' });
  const tabs = names.map((name) => sim.addTab(name, { path: '/tickets', ...tabOptions }));
  sim.quiesce();
  return { sim, gateway, tabs };
}

const fires = (invariant, tab) => (error) => {
  assert.ok(error instanceof InvariantError, `expected an invariant error, got ${error}`);
  assert.equal(error.invariant, invariant);
  if (tab) assert.equal(error.tab, tab);
  assert.match(error.message, new RegExp(`^${invariant} violated on tab ${tab ?? '\\w+'}, step \\d+, message `));
  return true;
};

test('two tabs on a tenant screen both show a title-only upgrade, fetching only the screen part again', () => {
  const { sim, gateway, tabs } = desk('a', 'b');
  for (const tab of tabs) {
    assert.match(pageText(tab), /# Open tickets\n## Open\n- Leak \[Open\]\nform ticket_desk__open_form\n {2}Title: \n {2}\[Open the ticket\]/);
    assert.deepEqual(tab.partFetches().map((r) => r.hash).sort(), [h('a'), ERRORS].sort());
  }
  gateway.upgrade('v2');
  sim.quiesce();
  for (const tab of tabs) {
    assert.match(pageText(tab), /^title: Open tickets \(v2\) — /);
    assert.equal(tab.state.ui.manifest.hash, h('2'));
    assert.equal(tab.state.ui.manifest.schema_version, 3, 'the install of version 1 was schema_version 2');
    assert.deepEqual(tab.partFetches().filter((r) => r.epoch === 1).map((r) => r.hash), [h('b')], 'only the screen part was fetched again');
    assert.equal(tab.partFetches(ERRORS).length, 1, 'the errors part was kept');
  }
});

test('the manifest a tab holds after a reordered pair of manifest replies is the newest', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  gateway.upgrade('v2');
  gateway.upgrade('v2b');
  sim.deliver(a, (i) => i.notice?.kind === 'schema');
  sim.deliver(a, (i) => i.notice?.kind === 'schema');
  // Two manifest reads are on their way, the older first; the newer arrives first.
  sim.reorder(a, (items) => items.reverse());
  sim.quiesce();
  assert.equal(a.state.ui.manifest.hash, h('3'));
  assert.match(pageText(a), /^title: Open tickets \(v2b\)/);
});

test('a ticket created in one tab appears in the other through the notice', () => {
  const { sim, tabs: [a, b] } = desk('a', 'b');
  a.type(FORM, 'title', 'Broken door');
  a.submit(FORM);
  sim.quiesce();
  assert.match(pageText(a), /toast\(ok\): Ticket opened\./);
  for (const tab of [a, b]) assert.match(pageText(tab), /\n- Broken door\n/);
});

test('a refusal is shown inline in the tenant\'s words', () => {
  const { sim, tabs: [a] } = desk('a');
  a.submit(FORM);
  sim.quiesce();
  assert.match(pageText(a), /Title: +!A ticket needs a title\./);
  assert.match(pageText(a), /\[Open the ticket\] !The request was refused\./);
});

test('a draft survives a title-only upgrade', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  a.type(FORM, 'title', 'Half typed');
  gateway.upgrade('v2');
  sim.quiesce();
  assert.match(pageText(a), /Title: Half typed/);
});

test('a tab that lost its socket catches up by the replay when it reconnects', () => {
  const { sim, gateway, tabs: [, b] } = desk('a', 'b');
  sim.disconnect(b);
  gateway.upgrade('v2');
  sim.quiesce();
  assert.equal(b.state.ui.manifest.hash, h('1'), 'not told while away');
  sim.reconnect(b);
  sim.quiesce();
  assert.equal(b.state.ui.manifest.hash, h('2'));
  assert.match(pageText(b), /^title: Open tickets \(v2\)/);
});

test('seeded random interleavings of two upgrades and a new ticket converge on the newest manifest', () => {
  for (let seed = 1; seed <= 8; seed += 1) {
    const { sim, gateway, tabs } = desk('a', 'b', 'c');
    const rng = seeded(seed);
    gateway.upgrade('v2');
    gateway.createRecord(OBJECT, { title: `Seed ${seed}`, status: 'open' });
    gateway.upgrade('v2b');
    sim.quiesce({ rng });
    for (const tab of tabs) {
      assert.equal(tab.state.ui.manifest.hash, h('3'), `seed ${seed}`);
    }
  }
});

test('a version is read from a directory of manifest.json and parts/*.json', () => {
  const dir = mkdtempSync(join(tmpdir(), 'hot-reload-'));
  mkdirSync(join(dir, 'parts'));
  writeFileSync(join(dir, 'manifest.json'), JSON.stringify(V2.manifest));
  for (const [hash, part] of Object.entries(V2.parts)) writeFileSync(join(dir, 'parts', `${hash}.json`), JSON.stringify(part));
  assert.deepEqual(loadVersionDir(dir), V2);
  const { sim, gateway, tabs: [a] } = desk('a');
  gateway.addVersion('from-dir', { dir }).upgrade('from-dir');
  sim.quiesce();
  assert.match(pageText(a), /^title: Open tickets \(v2\)/);
});

// ---- One test per invariant: a deliberately broken sequence, and the check that fires ----------

test('I1 fires when a stale screen is drawn after a newer manifest was accepted', () => {
  let drawn = null;
  const staleView = (state) => {
    const v = view(state);
    if (v.page.kind === 'tenant') drawn = v;
    else if (drawn && v.page.kind === 'loading' && state.ui.manifest) return drawn; // keeps the old screen up
    return v;
  };
  const sim = new Simulator({ version: V1 });
  sim.gateway.defineView(VIEW, { dependsOn: [OBJECT], sections: { open: { object: OBJECT } } });
  // A kernel that never takes a part in keeps drawing the old screen after the upgrade.
  const deaf = (state, msg) => {
    const refused = msg.type === 'http_result' && state.pending?.[msg.id]?.kind === 'ui.part' && Object.keys(state.ui.held ?? {}).length ?  { ...msg, ok: false, status: 500, body: null } : msg;
    return update(state, refused);
  };
  sim.addTab('a', { path: '/tickets', kernel: { view: staleView, update: deaf } });
  sim.quiesce();
  sim.gateway.upgrade(V2);
  assert.throws(() => sim.quiesce(), fires('I1', 'a'));
});

test('I2 fires when the gateway serves a manifest with a lower schema version', () => {
  // A kernel without the version guard.
  const unguarded = (state, msg) => {
    const result = update(state, msg);
    if (msg.type === 'http_result' && state.pending?.[msg.id]?.kind === 'ui.manifest' && msg.ok) return { ...result, state: { ...result.state, ui: { ...result.state.ui, manifest: msg.body } } };
    return result;
  };
  const { sim, gateway } = deskWith({ kernel: { update: unguarded } }, 'a');
  gateway.upgrade('v2');
  sim.quiesce();
  gateway.install('v1', { schemaVersion: 1 });
  assert.throws(() => sim.quiesce(), (error) => fires('I2', 'a')(error) && /from 3 down to 1/.test(error.detail));
});

test('I3 fires when a connected tab never hears of an upgrade', () => {
  const { sim, gateway } = desk('a', 'b');
  gateway.upgrade('v2');
  assert.equal(sim.drop('b', (item) => item.notice?.kind === 'schema'), 1);
  assert.throws(() => sim.quiesce(), fires('I3', 'b'));
});

test('I4 fires when a draft is dropped with its field and nothing says so', () => {
  // A kernel that says nothing of a lost draft.
  const silent = (state, msg) => {
    const result = update(state, msg);
    return { ...result, state: { ...result.state, toasts: state.toasts, announce: '' } };
  };
  const { sim, gateway, tabs: [a] } = deskWith({ kernel: { update: silent } }, 'a');
  a.type(FORM, 'title', 'Half typed');
  gateway.upgrade('v3');
  assert.throws(() => sim.quiesce(), (error) => fires('I4', 'a')(error) && /nothing on the page says so/.test(error.detail));
});

test('I5 fires when a submitted operation never shows an outcome', () => {
  const { sim, tabs: [a] } = desk('a');
  a.type(FORM, 'title', 'Lost reply');
  a.submit(FORM);
  assert.equal(sim.drop('a', (item) => item.path === '/api/operations/invoke'), 1);
  assert.throws(() => sim.quiesce(), (error) => fires('I5', 'a')(error) && /never showed an outcome/.test(error.detail));
});

test('I6 fires when a kernel fetches parts it holds after an upgrade', () => {
  const chatty = (state, msg) => {
    const result = update(state, msg);
    if (msg.type !== 'http_result' || state.pending?.[msg.id]?.kind !== 'ui.manifest' || !msg.ok) return result;
    const extra = msg.body.parts.map((entry, i) => ({ type: 'http', id: 9000 + i, method: 'GET', path: `/api/ui/parts/${entry.hash}`, write: false, correlation: 'x' }));
    return { ...result, effects: [...result.effects, ...extra] };
  };
  const sim = new Simulator({ version: V1 });
  sim.gateway.addVersion('v2', V2);
  sim.gateway.defineView(VIEW, { dependsOn: [OBJECT], sections: { open: { object: OBJECT } } });
  sim.addTab('a', { path: '/tickets', kernel: { update: chatty } });
  sim.quiesce();
  sim.gateway.upgrade('v2');
  assert.throws(() => sim.quiesce(), (error) => fires('I6', 'a')(error) && /3 part fetches in upgrade 1/.test(error.detail));
});

test('I7 fires when the kernel throws, when one message makes too many effects, and when effects loop', () => {
  const sim = (kernel, options = {}) => {
    const s = new Simulator({ version: V1 });
    s.gateway.defineView(VIEW, { dependsOn: [OBJECT], sections: { open: { object: OBJECT } } });
    s.addTab('a', { path: '/tickets', kernel, ...options });
    return s;
  };
  const throwing = (state, msg) => {
    if (msg.type === 'ws_message' && msg.message.type === 'welcome') throw new Error('boom');
    return update(state, msg);
  };
  assert.throws(() => sim({ update: throwing }).quiesce(), (error) => fires('I7', 'a')(error) && /the kernel threw: Error: boom/.test(error.detail));

  const flooding = (state, msg) => {
    const result = update(state, msg);
    return msg.type === 'init' ? result : { ...result, effects: [...result.effects, ...Array.from({ length: 30 }, () => ({ type: 'log' }))] };
  };
  assert.throws(() => sim({ update: flooding }).quiesce(), (error) => fires('I7', 'a')(error) && /effects for one message/.test(error.detail));

  // Every part that arrives is asked for again: one effect per message, never ending.
  let id = 5000;
  const looping = (state, msg) => {
    const result = update(state, msg);
    if (msg.type !== 'http_result' || !(state.pending?.[msg.id]?.kind === 'ui.part' || msg.id > 5000)) return result;
    return { ...result, effects: [...result.effects, { type: 'http', id: (id += 1), method: 'GET', path: `/api/ui/parts/${ERRORS}`, write: false, correlation: 'x' }] };
  };
  assert.throws(() => sim({ update: looping }).quiesce({ maxSteps: 60 }), (error) => fires('I7', 'a')(error) && /still delivering after 60/.test(error.detail));
});

// ---- Gaps of the client the simulator shows --------------------------------------------------

test('a resync after a missed schema notice leaves the tab on the old manifest', () => {
  const { sim, gateway, tabs: [, b] } = desk('a', 'b');
  sim.disconnect(b);
  gateway.upgrade('v2');
  sim.quiesce();
  sim.reconnect(b, { resync: true });
  sim.quiesce();
  assert.equal(b.state.ui.manifest.hash, h('2'));
});

test('a record notice that arrives while the new screen part is on its way is not lost', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  gateway.upgrade('v2');
  gateway.createRecord(OBJECT, { title: 'During upgrade', status: 'open' });
  sim.deliver(a, (item) => item.notice?.kind === 'schema');
  sim.deliver(a, (item) => item.path === '/api/ui');
  sim.deliver(a, (item) => item.notice?.kind === 'upsert');
  sim.quiesce();
  assert.match(pageText(a), /During upgrade/);
});

test('while the new screen part is on its way the page shows nothing of the screen', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  gateway.upgrade('v2');
  sim.deliver(a, (item) => item.notice?.kind === 'schema');
  sim.deliver(a, (item) => item.path === '/api/ui');
  assert.match(pageText(a), /# Open tickets/);
});

test('a draft in a field an upgrade removed is shown as lost, as a notice', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  a.type(FORM, 'title', 'Half typed');
  gateway.upgrade('v3');
  sim.quiesce();
  const drawn = a.draw();
  assert.ok(drawn.toasts.some((t) => /title/.test(t.text) && /not saved/.test(t.text)));
});

test('a manifest with a lower schema version than the accepted one is ignored', () => {
  const { sim, gateway, tabs: [a] } = desk('a');
  gateway.upgrade('v2');
  sim.quiesce();
  const accepted = a.state.ui.manifest;
  gateway.install('v1', { schemaVersion: 1 });
  // The tab differs from the gateway on purpose (I3), but never goes down (I2).
  assert.throws(() => sim.quiesce(), fires('I3', 'a'));
  assert.equal(a.state.ui.manifest.hash, accepted.hash);
  assert.equal(a.state.ui.manifest.schema_version, accepted.schema_version);
});

test('a request that gets no reply times out as an unreachable site, so a form is not busy for good', () => {
  const { sim, tabs: [a] } = desk('a');
  a.type(FORM, 'title', 'Lost reply');
  a.submit(FORM);
  assert.equal(sim.drop('a', (item) => item.path === '/api/operations/invoke'), 1);
  const id = Object.entries(a.state.pending).find(([, p]) => p.kind === 'tenant.submit')[0];
  assert.ok(Object.values(a.state.tenant.forms).some((f) => f.busy));
  a.step({ type: 'timer', name: 'request', token: Number(id) });
  assert.ok(Object.values(a.state.tenant.forms).every((f) => !f.busy));
  assert.equal(a.state.pending[id], undefined);
});
