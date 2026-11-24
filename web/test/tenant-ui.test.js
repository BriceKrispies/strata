// A tenant screen with no compiled code (D-8250): the kernel reads the tenant's interface
// definition, resolves the screen's address from it, and builds the page from the screen's part
// with the fixed interpreter. Plain functions, no browser and no worker.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { nodesOf } from '../src/kernel/compose.js';
import { explainRefusal, matchTenantRoute, tenantTitle } from '../src/kernel/tenant-ui.js';
import { initialState, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';

const stamp = { at: 1_000, rand: 0.5 };
const member = { authenticated: true, tenant: 'acme', tenant_name: 'Acme', principal: 'user:ines', display_name: 'Ines', roles: ['member'], may: {}, may_manage_schema: false, csrf_token: 'tok' };
const anonymous = { authenticated: false, tenant: 'acme', tenant_name: 'Acme', roles: ['public'], may: {}, may_manage_schema: false, csrf_token: 'issued' };

const SCREEN_HASH = 'a'.repeat(64);
const ERRORS_HASH = 'b'.repeat(64);

/** The manifest the gateway serves for the ticket desk product (strata_substrate::ui). */
const manifest = {
  hash: 'c'.repeat(64),
  schema_version: 4,
  parts: [
    { kind: 'screen', id: 'ticket_desk__board_screen', hash: SCREEN_HASH, route: '/tickets', view: 'ticket_desk__board', title: 'Open tickets', layout: 'list', auth: 'session', query: [] },
    { kind: 'errors', id: 'errors', hash: ERRORS_HASH },
  ],
};

/** The screen's part: the screen with its form, action and live binding. */
const screenPart = {
  kind: 'screen',
  id: 'ticket_desk__board_screen',
  screen: { route: '/tickets', view: 'ticket_desk__board', title: 'Open tickets', kind: 'list', auth: 'session', query: [] },
  forms: {
    ticket_desk__open_form: {
      screen: 'ticket_desk__board_screen',
      operation: 'ticket_desk__open_ticket',
      fields: [{ name: 'title', input: 'title', label: 'Title', control: 'text', trim: true }],
      submit_label: 'Open the ticket',
      on_success: { toast: 'Ticket opened.' },
      on_error: 'inline',
    },
  },
  actions: {
    ticket_desk__open_again: {
      screen: 'ticket_desk__board_screen', label: 'Open it again', operation: 'ticket_desk__open_ticket', form: 'ticket_desk__open_form',
      arguments: { title: 'form.title' }, on_success: { toast: 'Opened again.' }, on_error: 'toast',
    },
  },
  live: { ticket_desk__board_live: { screen: 'ticket_desk__board_screen', event: 'notice', kinds: ['membership'], objects: true } },
};

const errorsPart = {
  kind: 'errors',
  id: 'errors',
  messages: {
    ticket_desk__title_needed: { on: 'check', kinds: ['required'], text: 'A ticket needs a title.', field: 'name', applies_to: ['ticket_desk__open_form'] },
  },
};

/** The view model of the tenant view at /tickets, as the gateway projects it. */
const boardModel = {
  view: 'ticket_desk__board',
  title: 'Tickets',
  path: '/tickets',
  params: {},
  depends_on: ['ticket_desk__ticket'],
  revision: 9,
  schema_version: 4,
  theme: { accent: 'moss', paper: 'linen' },
  data: { open: [{ id: 3, version: 1, fields: { title: 'Leak', status: 'open' }, refs: {} }] },
  next: {},
};

function run(state, messages) {
  const effects = [];
  for (const message of messages) {
    const result = update(state, { ...stamp, ...message });
    state = result.state;
    effects.push(...result.effects);
  }
  return { state, effects };
}

/** The form nodes of a page model. */
const formsOf = (page) => nodesOf(page).filter((n) => n.pattern === 'form');

const http = (effects) => effects.filter((e) => e.type === 'http');
const answer = (effect, body, ok = true, status = 200) => ({ type: 'http_result', id: effect.id, ok, status, body });

/** A page opened at /tickets with no view in its bootstrap, signed in as `session`. */
function opened(session = member) {
  return update(initialState(), { type: 'init', path: '/tickets', search: '', bootstrap: { path: '/tickets', session: { session } }, systemDark: false, storedTheme: null, ...stamp });
}

/** The page at /tickets once the view, the manifest and both parts have arrived. */
function loaded(session = member) {
  const start = opened(session);
  const [viewAsked, manifestAsked] = http(start.effects);
  const withManifest = run(start.state, [answer(manifestAsked, manifest)]);
  const parts = http(withManifest.effects);
  const screen = parts.find((e) => e.path === `/api/ui/parts/${SCREEN_HASH}`);
  const errors = parts.find((e) => e.path === `/api/ui/parts/${ERRORS_HASH}`);
  return run(withManifest.state, [answer(viewAsked, boardModel), answer(screen, screenPart), answer(errors, errorsPart)]).state;
}

/// Verifies: INT-3256
test('an address no platform screen answers asks for the tenant interface definition, then routes to the tenant screen', () => {
  const start = opened();
  const paths = http(start.effects).map((e) => e.path);
  assert.deepEqual(paths, ['/api/view?path=%2Ftickets', '/api/ui']);
  // While the definition is on its way the page waits, rather than say there is nothing here.
  assert.equal(view(start.state).page.kind, 'loading');

  const withManifest = update(start.state, { ...stamp, ...answer(http(start.effects)[1], manifest) });
  assert.deepEqual(
    http(withManifest.effects).map((e) => e.path).sort(),
    [`/api/ui/parts/${SCREEN_HASH}`, `/api/ui/parts/${ERRORS_HASH}`].sort(),
    'the screen\'s part and the error messages are fetched by their hashes',
  );
  // A navigation to the screen now reads the tenant view of its route.
  const again = update(withManifest.state, { type: 'navigate', href: '/tickets?status=open', ...stamp });
  assert.ok(http(again.effects).some((e) => e.path === '/api/view?path=%2Ftickets'), 'a query parameter the screen does not take is not sent');
  assert.equal(http(again.effects).filter((e) => e.path.startsWith('/api/ui/parts/')).length, 0, 'parts already asked for are not asked again');
});

/// Verifies: INT-3257
test('a tenant screen is drawn by the interpreter from its part and the view model', () => {
  const page = view(loaded()).page;
  assert.equal(page.kind, 'tenant');
  assert.equal(page.screen, 'ticket_desk__board_screen');
  assert.equal(page.layout, 'list');
  assert.equal(page.heading, 'Open tickets');
  assert.equal(view(loaded()).pageTitle, 'Open tickets', 'the title the screen states, not the view\'s');
  // The list of tickets in the main column, as cards; the form beside it.
  assert.equal(page.template, 'split');
  const [open] = page.main;
  assert.equal(open.pattern, 'record-list');
  assert.equal(open.label, 'Open');
  assert.equal(open.state, 'ready');
  assert.equal(open.items[0].title, 'Leak');
  assert.equal(open.items[0].state, null, 'an open ticket in a list of open tickets says nothing of its state');
  assert.deepEqual(page.aside, [
    {
      pattern: 'form', id: 'form:ticket_desk__open_form', form: 'ticket_desk__open_form', title: 'Open ticket', submitLabel: 'Open the ticket',
      busy: false, error: '', notice: '', collapsible: false, open: true,
      fields: [{ name: 'title', label: 'Title', control: 'text', options: [], value: '', error: '', required: null }],
    },
  ]);
  assert.deepEqual(page.actions, [{ id: 'ticket_desk__open_again', label: 'Open it again', confirm: null, armed: false }]);
});

test('a tenant screen for signed-in people asks a visitor to sign in', () => {
  assert.equal(view(loaded(anonymous)).page.kind, 'signin_required');
});

test('a tenant form invokes its operation with its fields as inputs, and a refusal is worded by the tenant', () => {
  const typed = run(loaded(), [
    { type: 'input', form: 'tenant:ticket_desk__open_form', name: 'title', value: '  ' },
    { type: 'submit', form: 'tenant:ticket_desk__open_form', values: { title: '  ' } },
  ]);
  const [invoked] = http(typed.effects);
  assert.equal(invoked.method, 'POST');
  assert.equal(invoked.path, '/api/operations/invoke');
  assert.deepEqual(invoked.body, { operation: 'ticket_desk__open_ticket', inputs: { title: '' } }, 'the field is trimmed');
  assert.ok(invoked.idempotencyKey, 'an invocation carries a key');
  assert.equal(invoked.csrf, 'tok');
  assert.equal(formsOf(view(typed.state).page)[0].busy, true);

  const refusal = { code: 'invalid', message: 'The ticket was refused.', failed: [{ rule: 'required:ticket_desk__ticket.title', detail: 'title is required', passed: false }] };
  const refused = update(typed.state, { ...stamp, ...answer(invoked, refusal, false, 422) });
  const form = formsOf(view(refused.state).page)[0];
  assert.equal(form.busy, false);
  assert.equal(form.fields[0].error, 'A ticket needs a title.');
  assert.equal(form.error, 'The ticket was refused.');

  const accepted = update(typed.state, { ...stamp, ...answer(invoked, { result: {}, event_seq: 10, checks_passed: 3, dry_run: false }) });
  assert.equal(accepted.state.toasts.at(-1).text, 'Ticket opened.');
  assert.ok(http(accepted.effects).some((e) => e.path === '/api/view?path=%2Ftickets'), 'the view is read again');
});

test('a tenant action reads the form it names', () => {
  const pressed = run(loaded(), [
    { type: 'input', form: 'tenant:ticket_desk__open_form', name: 'title', value: 'Leak again' },
    { type: 'action', name: 'tenant.action', arg: 'ticket_desk__open_again' },
  ]);
  const [invoked] = http(pressed.effects);
  assert.deepEqual(invoked.body, { operation: 'ticket_desk__open_ticket', inputs: { title: 'Leak again' } });
});

/// Verifies: INT-3258
test('a schema notice reads the manifest again, and the screen\'s own live binding decides the refresh', () => {
  const welcome = { type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'c', head: 40, session: 'active', heartbeat_ms: 10000 } };
  const live = run(loaded(), [welcome]).state;
  const frame = (n, notice) => ({ type: 'ws_message', message: { type: 'event', sub: 'site', n, seq: 40 + n, correlation: '', notice } });
  const schema = update(live, { ...stamp, ...frame(1, { kind: 'schema', version: 5 }) });
  const paths = http(schema.effects).map((e) => e.path);
  assert.ok(paths.includes('/api/ui'), 'the manifest is read again');
  assert.ok(!paths.some((p) => p.startsWith('/api/view')), 'the binding names no schema notice');

  const ticket = update(live, { ...stamp, ...frame(1, { kind: 'upsert', object: 'ticket_desk__ticket', id: 3, version: 2 }) });
  assert.ok(http(ticket.effects).some((e) => e.path === '/api/view?path=%2Ftickets'), 'a change to an object the view depends on refreshes it');
  const other = update(live, { ...stamp, ...frame(1, { kind: 'upsert', object: 'post', id: 1, version: 2 }) });
  assert.ok(!http(other.effects).some((e) => e.path.startsWith('/api/view')), 'one to another object does not');
});

test('the tenant router prefers literal segments and keeps only the screen\'s query parameters', () => {
  const ui = {
    manifest: {
      hash: 'h',
      schema_version: 1,
      parts: [
        { kind: 'screen', id: 'one', hash: '1', route: '/tickets/:slug', view: 'v1', title: 'T ${data.ticket.fields.title}', layout: 'detail', auth: 'anyone', query: ['tab'] },
        { kind: 'screen', id: 'new', hash: '2', route: '/tickets/new', view: 'v2', title: 'New', layout: 'editor', auth: 'anyone', query: [] },
      ],
    },
    parts: {},
    asked: {},
  };
  assert.equal(matchTenantRoute(ui, '/tickets/new')?.screen, 'new');
  const one = matchTenantRoute(ui, '/tickets/leak', { tab: 'notes', other: 'x' });
  assert.deepEqual([one?.screen, one?.params, one?.query], ['one', { slug: 'leak' }, { tab: 'notes' }]);
  assert.equal(matchTenantRoute(ui, '/elsewhere'), null);
  assert.equal(tenantTitle(ui.manifest.parts[0].title, { data: { ticket: { fields: { title: 'Leak' } } } }), 'T Leak');
});

test('a refusal with no wording of the tenant says what the server said', () => {
  const said = explainRefusal(null, 'f', { fields: [{ name: 'title' }] }, { code: 'invalid', message: 'No.', failed: [{ rule: 'constraint:x.title', detail: 'too long' }] });
  assert.deepEqual(said, { message: 'No.', fields: { title: 'too long' } });
});
