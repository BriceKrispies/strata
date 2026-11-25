// What the page does with what the specification generates: the delete confirmation, the live
// filter, the generated names, the principal and the addresses it writes into the DOM.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isPageAddress } from '../src/address.js';
import { safeUrl } from '../src/components/dom.js';
import * as actions from '../src/generated/actions.js';
import { NAVIGATION, ROW_ACTIONS, SCREENS, SIGN_IN_PATH, guardedView, pageTitle, rowAction, rowActionAsked } from '../src/generated/screens.js';
import { enterHookNames, initialState, principalOf, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';
import { registeredKinds } from '../src/views/renderers.js';

const stamp = { at: 1_000, rand: 0.5 };
const site = { id: 1, version: 1, fields: { name: 'Fieldnotes', tagline: 'Essays.' }, refs: {} };
const theme = { accent: 'moss', paper: 'linen' };
const anonymous = { authenticated: false, tenant: 'fieldnotes', tenant_name: 'Fieldnotes', roles: ['public'], may: {}, may_manage_schema: false, csrf_token: 'issued' };
const editor = { ...anonymous, authenticated: true, principal: 'user:ines', display_name: 'Inès', csrf_token: 'tok', roles: ['public', 'editor', 'owner'], may: { post: ['create'], publication: ['update'] } };

const row = { id: 7, version: 3, created_at: 1, updated_at: 2, refs: {}, fields: { title: 'On Sharpening', slug: 'on-sharpening', body: 'b', status: 'draft' } };
const deskModel = {
  view: 'desk',
  title: 'Desk',
  path: '/desk',
  params: {},
  depends_on: ['post'],
  revision: 1,
  schema_version: 1,
  theme,
  data: { site, posts: [row] },
  articles: [{ article_id: 7, version: 3, title: 'On Sharpening', slug: 'on-sharpening', body: 'b', status: 'draft' }],
};
const homeShell = () => ({ view: 'home', title: 'Home', path: '/', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site, featured: null, recent: [], categories: [] } });
const missingModel = { view: 'missing', title: 'Not found', path: '/nowhere', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site } };

function boot(path, session, model) {
  return update(initialState(), { type: 'init', path, search: '', bootstrap: { path, session: { session }, view: model }, systemDark: false, storedTheme: null, ...stamp });
}

function connected(start) {
  const welcome = { type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'c', head: 40, session: 'active', heartbeat_ms: 10000 }, ...stamp };
  return update(start, welcome).state;
}

const notice = (n, kind) => ({ type: 'ws_message', message: { type: 'event', sub: 'site', n, seq: 40 + n, correlation: '', notice: { kind, object: 'post', id: 7, version: 2 } }, ...stamp });

test('a delete confirmation does not survive leaving the page, going back, or a new view', () => {
  const asked = update(boot('/desk', editor, deskModel).state, { type: 'action', name: 'post.ask_delete', arg: { id: 7 }, ...stamp }).state;
  assert.equal(asked.confirmDelete, 7);

  const away = update(asked, { type: 'navigate', href: '/about', ...stamp }).state;
  assert.equal(away.confirmDelete, null, 'navigating disarms it');
  const back = update(away, { type: 'navigate', href: '/desk', ...stamp }).state;
  const pressed = update(back, { type: 'action', name: 'post.delete', arg: { id: 7, version: 3 }, ...stamp });
  assert.deepEqual(pressed.effects, [], 'one press after coming back deletes nothing');

  assert.equal(update(asked, { type: 'popstate', path: '/about', search: '', ...stamp }).state.confirmDelete, null, 'going back disarms it');

  const waiting = { ...asked, view: { ...asked.view, request: 99 }, pending: { 99: { kind: 'view', href: '/desk' } } };
  const replaced = update(waiting, { type: 'http_result', id: 99, ok: true, status: 200, body: deskModel, ...stamp }).state;
  assert.equal(replaced.confirmDelete, null, 'a view that replaces the page disarms it');
});

test('a notice of a change of membership refreshes the view only where the live binding says so', () => {
  const httpPaths = (effects) => effects.filter((e) => e.type === 'http').map((e) => e.path);
  // The not-found screen has no binding for a notice, so the view is left alone; the session is read.
  const missing = connected(boot('/nowhere', anonymous, missingModel).state);
  const apart = update(missing, notice(1, 'membership'));
  assert.equal(httpPaths(apart.effects).some((p) => p.startsWith('/api/view')), false);
  assert.equal(httpPaths(apart.effects).length, 1, 'only the session is read again');
  // A screen with a binding reads both.
  const desk = connected(boot('/desk', editor, deskModel).state);
  const both = update(desk, notice(1, 'membership'));
  assert.equal(httpPaths(both.effects).some((p) => p.startsWith('/api/view')), true);
  assert.equal(httpPaths(both.effects).length, 2);
});

const settingNotice = (n, setting) => ({ type: 'ws_message', message: { type: 'event', sub: 'site', n, seq: 40 + n, correlation: '', notice: { kind: 'setting', setting, level: 'tenant' } }, ...stamp });

test('a notice of a setting change reads again only the view whose arguments read that setting', () => {
  const httpPaths = (effects) => effects.filter((e) => e.type === 'http').map((e) => e.path);
  // A view that reads page_size in its arguments names it in its model, and is read again for it.
  const reading = connected(boot('/nowhere', anonymous, { ...missingModel, settings: ['page_size'] }).state);
  assert.equal(httpPaths(update(reading, settingNotice(1, 'page_size')).effects).some((p) => p.startsWith('/api/view')), true);
  // Another setting leaves it alone, and so does a view that reads none.
  assert.equal(httpPaths(update(reading, settingNotice(2, 'ui_theme')).effects).some((p) => p.startsWith('/api/view')), false);
  const plain = connected(boot('/nowhere', anonymous, missingModel).state);
  assert.equal(httpPaths(update(plain, settingNotice(3, 'page_size')).effects).some((p) => p.startsWith('/api/view')), false);
});

test('the desk names its buttons with the labels the specification generates', () => {
  const { page } = view(boot('/desk', editor, deskModel).state);
  const item = { article_id: 7, title: 'On Sharpening' };
  assert.deepEqual(page.posts[0].labels, {
    publish: actions.label('act-desk-publish', item),
    unpublish: actions.label('act-desk-unpublish', item),
    remove: actions.label('act-desk-delete', item),
    confirmRemove: actions.confirmText('act-desk-delete', item),
  });
  assert.equal(page.posts[0].labels.confirmRemove, 'Confirm deleting On Sharpening');
});

test('a principal is judged by all its roles', () => {
  const principal = principalOf(editor);
  assert.deepEqual(principal.roles, ['public', 'editor', 'owner']);
  assert.equal('role' in principal, false, 'no single role stands for the principal');
  assert.equal(principalOf(anonymous), null);
});

test('only a path on this site is written into an address attribute', () => {
  for (const bad of ['/\\evil.com', '/\t/evil.com', '/\n/evil.com', '//evil.com', '/\\/evil.com', 'javascript:alert(1)', '\\\\evil.com']) {
    assert.equal(safeUrl('href', bad), null, JSON.stringify(bad));
    assert.equal(safeUrl('src', bad), null, JSON.stringify(bad));
  }
  assert.equal(safeUrl('href', '/posts/slow?x=1#y'), '/posts/slow?x=1#y');
  assert.equal(safeUrl('src', '/media/kiln.svg'), '/media/kiln.svg');
  assert.equal(safeUrl('href', 'https://example.org/a'), 'https://example.org/a');
  assert.equal(safeUrl('href', '#top'), '#top');
  assert.equal(safeUrl('src', 'https://example.org/a.png'), null);
});

// ---------------------------------------------------------------------------------------------
// State of the person signed in, stale replies, live updates and the click handler.

const other = { ...editor, principal: 'user:omar', display_name: 'Omar', csrf_token: 'tok2' };
const homeModel = { view: 'home', title: 'Home', path: '/', params: {}, depends_on: ['post'], revision: 3, schema_version: 1, theme, data: { site, featured: null, recent: [], categories: [] } };
const httpPaths = (effects) => effects.filter((e) => e.type === 'http').map((e) => e.path);
const isViewRead = (path) => path.startsWith('/api/view');
const reply = (state, effect, body, ok = true, status = 200) => update(state, { type: 'http_result', id: effect.id, ok, status, body, ...stamp });

/** A signed-in desk holding everything one person can leave behind on a tab. */
function leftBehind() {
  const state = boot('/desk', editor, deskModel).state;
  return {
    ...state,
    factors: { ...state.factors, loaded: true, summary: { totp: true, recovery_codes: 3 }, codes: ['aaaa-bbbb'], enrol: { secret: 'JBSWY3DP', uri: 'otpauth://x', qr: [[true]], error: '' }, stepUp: { retry: { name: 'factors.new_codes' }, error: '', busy: false } },
    sessions: { ...state.sessions, loaded: true, list: [{ id: 'sess-A1' }] },
    passkeys: { ...state.passkeys, supported: true, loaded: true, list: [{ id: 'pk-A1', name: 'Laptop' }] },
    lab: { ...state.lab, overview: { presets: [{ id: 'preset-A1' }] }, result: { comparison: { note: 'result-A1' } }, diagnostics: { rows: 1 }, run: { id: 'r1', status: 'finished' } },
    forms: { ...state.forms, editor: { key: 'x', id: 7, dirty: true, values: { title: 'Private draft' }, errors: {} } },
  };
}

function assertNothingOfThePersonBefore(state) {
  const text = JSON.stringify([state.factors, state.sessions, state.passkeys.list, state.lab, state.forms.editor, state.forms.settings, state.optimistic]);
  for (const secret of ['aaaa-bbbb', 'JBSWY3DP', 'sess-A1', 'pk-A1', 'Laptop', 'Private draft', 'result-A1', 'preset-A1']) {
    assert.ok(!text.includes(secret), `${secret} is still in the state`);
  }
  assert.equal(state.passkeys.supported, true, 'what the browser can do is not the person\'s');
}

test('signing out clears the recovery codes, secret, lists, Lab results and drafts of the person', () => {
  const out = update(leftBehind(), { type: 'action', name: 'logout', ...stamp });
  const done = reply(out.state, out.effects[0], { session: anonymous });
  assertNothingOfThePersonBefore(done.state);
  assert.equal(done.state.view.model, null, 'the rows of the person are not shown while the page loads');
  assert.ok(httpPaths(done.effects).some(isViewRead), 'the guarded page is read again');
});

test('the end of a session clears the same, and reads the guarded page again', () => {
  const ended = update(connected(leftBehind()), { type: 'ws_message', message: { type: 'session', state: 'expired' }, ...stamp });
  assertNothingOfThePersonBefore(ended.state);
  assert.equal(ended.state.view.model, null);
  assert.ok(httpPaths(ended.effects).some(isViewRead));
});

test('signing in as another person on the same tab keeps nothing of the first', () => {
  const submitted = update(leftBehind(), { type: 'submit', form: 'login', values: { login: 'omar', password: 'pw' }, ...stamp });
  const done = reply(submitted.state, submitted.effects[0], { session: other });
  assertNothingOfThePersonBefore(done.state);
  assert.equal(done.state.session.principal, 'user:omar');
  assert.equal(done.state.view.model, null, 'the first person\'s desk is not drawn for the second while it loads');
  assert.equal(view(done.state).page.kind, 'loading');
});

test('a reply to a request made before the sign-out is dropped when it arrives', () => {
  const state = leftBehind();
  const [listId] = Object.entries(state.pending).find(([, p]) => p.kind === 'sessions.list');
  const out = update(state, { type: 'action', name: 'logout', ...stamp });
  const signedOut = reply(out.state, out.effects[0], { session: anonymous }).state;
  const late = update(signedOut, { type: 'http_result', id: Number(listId), ok: true, status: 200, body: { sessions: [{ id: 'old' }] }, ...stamp });
  assert.deepEqual(late.state.sessions.list, []);
  assert.equal(late.state.sessions.loaded, false);
});

test('only the latest reply of a kind is applied', () => {
  const start = boot('/desk', editor, deskModel).state;
  const [firstId] = Object.entries(start.pending).find(([, p]) => p.kind === 'sessions.list');
  const secondId = 900;
  const asked = { ...start, pending: { ...start.pending, [secondId]: { kind: 'sessions.list', correlation: 'c-900', generation: 0 } }, latest: { ...start.latest, 'sessions.list': secondId } };
  const newer = reply(asked, { id: secondId }, { sessions: [{ id: 'new' }] }).state;
  const older = reply(newer, { id: Number(firstId) }, { sessions: [{ id: 'old' }] }).state;
  assert.deepEqual(older.sessions.list.map((s) => s.id), ['new']);
});

test('a late anonymous session read does not overwrite the session of someone who has signed in', () => {
  const start = update(initialState(), { type: 'init', path: '/', search: '', bootstrap: null, systemDark: false, storedTheme: null, ...stamp });
  const read = start.effects.find((e) => e.path === '/api/session');
  const submitted = update(start.state, { type: 'submit', form: 'login', values: { login: 'ines', password: 'pw' }, ...stamp });
  const signedIn = reply(submitted.state, submitted.effects[0], { session: editor }).state;
  const late = reply(signedIn, read, { session: anonymous }).state;
  assert.equal(late.session.principal, 'user:ines');
  assert.equal(late.session.csrf_token, 'tok');
});

test('the click handler leaves addresses the routes do not name to the browser, and the routes to the page', () => {
  for (const path of ['/keys', '/.well-known/jwks.json', '/scim/v2/Users', '/federation/saml/metadata/x', '/api/session', '/media/jig.svg', '/assets/index.js', '/nowhere']) {
    assert.equal(isPageAddress(path), false, path);
  }
  for (const path of ['/', '/posts/slow', '/topics', '/authors/ines', '/search', '/login', '/desk', '/desk/edit/slow', '/lab']) {
    assert.equal(isPageAddress(path), true, path);
  }
});

test('after a frame gap, a first welcome or a pong that shows a higher head, the page is read again', () => {
  const live = connected(boot('/', anonymous, homeModel).state);
  const gapFrame = { type: 'event', sub: 'site', n: 3, seq: 43, correlation: '', notice: { kind: 'upsert', object: 'invoice', id: 1, version: 1 } };
  const gap = update(live, { type: 'ws_message', message: gapFrame, ...stamp });
  assert.ok(httpPaths(gap.effects).some(isViewRead), 'after a frame gap');
  const waiting = update(gap.state, { type: 'ws_closed', ...stamp });
  const due = update(waiting.state, { type: 'timer', name: 'reconnect', token: waiting.effects[0].token, ...stamp });
  const back = update(due.state, { type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'd', head: 99, session: 'anonymous', heartbeat_ms: 10000 }, ...stamp });
  assert.ok(httpPaths(back.effects).some(isViewRead), 'a welcome that starts from the head');

  const resumedWaiting = update(back.state, { type: 'ws_closed', ...stamp });
  const resumedDue = update(resumedWaiting.state, { type: 'timer', name: 'reconnect', token: resumedWaiting.effects[0].token, ...stamp });
  const again = update(resumedDue.state, { type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'e', head: 120, session: 'anonymous', heartbeat_ms: 10000 }, ...stamp });
  assert.deepEqual(httpPaths(again.effects), [], 'a welcome that resumes from the last sequence replays what was missed');

  const pong = update(back.state, { type: 'ws_message', message: { type: 'pong', nonce: 1, head: 130 }, ...stamp });
  assert.ok(httpPaths(pong.effects).some(isViewRead), 'a pong with a higher head');
  const same = update(back.state, { type: 'ws_message', message: { type: 'pong', nonce: 1, head: 99 }, ...stamp });
  assert.deepEqual(httpPaths(same.effects), []);
});

test('a refused socket upgrade reads the session again, backs off and stops visibly after repeated refusals', () => {
  let state = connected(boot('/', anonymous, homeModel).state);
  const refusedOnce = () => {
    const closed = update(state, { type: 'ws_closed', refused: true, ...stamp });
    const read = closed.effects.find((e) => e.path === '/api/session');
    assert.ok(read, 'the session is read again');
    const timer = closed.effects.find((e) => e.type === 'timer' && e.name === 'reconnect');
    const refreshed = reply(closed.state, read, { session: { ...anonymous, csrf_token: 'fresh' } });
    const due = update(refreshed.state, { type: 'timer', name: 'reconnect', token: timer.token, ...stamp });
    return { due, timer };
  };
  const first = refusedOnce();
  assert.equal(first.timer.ms, 500);
  assert.deepEqual(first.due.effects, [{ type: 'ws_connect', csrf: 'fresh' }], 'the retry carries the refreshed token');
  state = first.due.state;
  const waits = [];
  for (let i = 0; i < 4; i += 1) {
    const next = refusedOnce();
    waits.push(next.timer.ms);
    state = next.due.state;
  }
  assert.deepEqual(waits, [1000, 2000, 4000, 8000], 'it backs off');
  assert.equal(state.wsStopped, true);
  assert.equal(view(state).shell.connection, 'stopped', 'the footer says so');
  const closed = update(state, { type: 'ws_closed', refused: true, ...stamp });
  const timer = closed.effects.find((e) => e.type === 'timer' && e.name === 'reconnect');
  assert.deepEqual(update(closed.state, { type: 'timer', name: 'reconnect', token: timer.token, ...stamp }).effects, [], 'no further attempt');
});

test('a close that was not a refusal (the network is down) does not read the session or count', () => {
  const offline = update(connected(boot('/', anonymous, homeModel).state), { type: 'ws_closed', ...stamp });
  assert.deepEqual(httpPaths(offline.effects), []);
  assert.equal(offline.state.wsStopped, false);
});

test('the Lab result poll stops when not signed in and backs off, with a cap, on other failures', () => {
  const booted = boot('/desk', editor, deskModel).state;
  const start = { ...booted, lab: { ...booted.lab, run: { id: 'r1', status: 'running', progress: null } } };
  const failing = (state, status) => {
    const asked = update(state, { type: 'timer', name: 'lab.poll', token: state.lab.timer, ...stamp });
    return reply(asked.state, asked.effects[0], null, false, status);
  };
  const stopped = failing(start, 401);
  assert.equal(stopped.state.lab.run, null);
  assert.deepEqual(stopped.effects.filter((e) => e.type === 'timer' && e.name === 'lab.poll'), []);

  let state = start;
  const waits = [];
  for (let i = 0; i < 12 && state.lab.run; i += 1) {
    const next = failing(state, 500);
    const timer = next.effects.find((e) => e.type === 'timer' && e.name === 'lab.poll');
    if (timer) waits.push(timer.ms);
    state = next.state;
  }
  assert.ok(waits.length >= 2 && waits.every((ms, i) => i === 0 || ms >= waits[i - 1]), 'the waits grow');
  assert.ok(Math.max(...waits) <= 15000, 'up to a cap');
  assert.equal(state.lab.run, null, 'and the poll ends');
});

test('the same message said twice is announced twice, and the sign-in link keeps the query', () => {
  const start = boot('/nowhere', anonymous, missingModel).state;
  const asked = update(start, { type: 'action', name: 'logout', ...stamp });
  const failed = reply(asked.state, asked.effects[0], null, false, 0);
  assert.ok(failed.state.announce, 'announced');
  const next = update(failed.state, { type: 'system_theme', dark: true, ...stamp });
  assert.equal(next.state.announce, '', 'the next message clears the live region, so a repeat changes it again');

  assert.equal(view(start).shell.loginHref, '/login?next=%2Fnowhere');
  const searching = update(start, { type: 'navigate', href: '/search?q=kiln', ...stamp }).state;
  assert.equal(view(searching).shell.loginHref, '/login?next=%2Fsearch%3Fq%3Dkiln');
});

test('a step-up prompt takes the focus', () => {
  const state = boot('/desk', editor, deskModel).state;
  const asked = update(state, { type: 'action', name: 'factors.new_codes', ...stamp });
  const refused = reply(asked.state, asked.effects[0], { kind: 'step_up_required', message: 'Confirm that it is you.' }, false, 403);
  assert.equal(refused.state.factors.stepUp.retry.name, 'factors.new_codes');
  assert.equal(refused.state.focus.target, 'stepup-secret');
  assert.notEqual(refused.state.focus.token, state.focus.token);
});

// WG-5039: the page frame derives navigation, guards, entry hooks, renderers, row buttons and the
// sign-in path from the generated screens module; a screen added to the specification needs no
// hand-written change here.
test('the navigation, the tools and the current entry come from the generated screens', () => {
  const labels = (items) => items.map((i) => i.label);
  const anon = view(boot('/', anonymous, homeShell()).state).shell;
  assert.deepEqual(labels(anon.nav), NAVIGATION.filter((n) => n.area === 'menu' && n.visibleTo === 'anyone').map((n) => n.label));
  assert.deepEqual(anon.tools, [], 'a visitor sees no entry that is for a session');
  assert.deepEqual(anon.nav.filter((i) => i.current).map((i) => i.href), ['/']);
  const topic = view(boot('/topics/joinery', anonymous, homeShell()).state).shell;
  assert.deepEqual(topic.nav.filter((i) => i.current).map((i) => i.href), ['/topics']);
  const desk = view(boot('/desk', editor, deskModel).state).shell;
  assert.deepEqual(desk.tools.map((i) => [i.href, i.current]), [['/desk', true]]);
  const lab = view(boot('/lab', editor, { ...deskModel, view: 'lab', path: '/lab' }).state).shell;
  assert.deepEqual(lab.tools.map((i) => i.current), [true], 'the lab is current on the desk entry');
});

test('a screen added to the generated screens is in the navigation with no change to the client', () => {
  const entry = { screen: 'screen-guide', href: '/guide', label: 'Guide', order: 5, area: 'menu', visibleTo: 'anyone', current: ['/guide*'] };
  NAVIGATION.push(entry);
  try {
    const shell = view(boot('/guide/start', anonymous, homeShell()).state).shell;
    const guide = shell.nav.find((i) => i.label === 'Guide');
    assert.ok(guide, 'in the menu');
    assert.equal(guide.href, '/guide');
    assert.equal(guide.current, true);
  } finally {
    NAVIGATION.pop();
  }
});

test('every screen that is guarded states its view, and its entry sections and renderer exist', () => {
  assert.equal(SIGN_IN_PATH, '/login');
  const kinds = registeredKinds();
  const hooks = enterHookNames();
  for (const s of SCREENS) {
    assert.equal(guardedView(s.view), s.auth !== 'none', s.screen);
    for (const name of s.onEnter) assert.ok(hooks.includes(name), `${s.screen} loads ${name} on entry, which no hook is registered for`);
    if (s.kind !== null) assert.ok(kinds.includes(s.kind), `${s.screen} has no renderer of kind ${s.kind}`);
    if (s.auth !== 'none') assert.ok(s.title !== null, `${s.screen} states no title`);
  }
  assert.equal(guardedView('desk_settings'), true);
  assert.equal(guardedView('home'), false);
  assert.equal(guardedView('no-such-view'), false);
});

test('the row buttons and the page titles come from the generated screens', () => {
  assert.equal(rowAction('post.publish').action, 'act-desk-publish');
  assert.equal(rowAction('post.unpublish').marker, 'draft');
  assert.equal(rowAction('post.delete').confirm, true);
  assert.equal(rowActionAsked('post.ask_delete').action, 'act-desk-delete');
  assert.equal(rowAction('post.ask_delete'), null, 'the first press of a confirmation is not the action');
  assert.ok(ROW_ACTIONS.every((b) => b.rowKey === 'article_id'));
  assert.equal(pageTitle('post', { data: { post: { fields: { title: 'On Sharpening' } } } }), 'On Sharpening');
  assert.equal(pageTitle('topics', {}), 'Topics');
  assert.equal(pageTitle('home', {}), '');
  const titled = view(boot('/desk', editor, deskModel).state);
  assert.equal(titled.pageTitle, 'Writing desk');
  assert.equal(titled.title, 'Writing desk — Fieldnotes');
});

test('the desk is entered through the hooks its screen names', () => {
  const desk = boot('/desk', editor, deskModel);
  const kinds = desk.effects.filter((e) => e.type === 'http').map((e) => e.path);
  assert.ok(kinds.includes('/api/sessions'), 'the sessions are asked for');
  assert.ok(kinds.includes('/api/factors'), 'the factors are asked for');
  const home = boot('/', anonymous, homeShell());
  assert.equal(home.effects.filter((e) => e.type === 'http').some((e) => e.path === '/api/sessions'), false);
});

// Verifies: CMS-164
test('a list that has more rows loads its next page and the rows continue with none repeated', () => {
  const article = (id) => ({ id, version: 1, created_at: 1, updated_at: 2, refs: {}, fields: { title: `Essay ${id}`, slug: `essay-${id}`, status: 'published', published_at: '2026-01-01T00:00:00Z' } });
  const model = { ...homeShell(), data: { site, featured: null, recent: [article(1), article(2)], categories: [] }, next: { recent: 'recent:o2' } };
  const start = boot('/', anonymous, model).state;
  assert.deepEqual(view(start).page.more, { section: 'recent', busy: false }, 'a section with a cursor offers more');

  const asked = update(start, { type: 'action', name: 'view.more', arg: 'recent', ...stamp });
  const [effect] = asked.effects;
  assert.equal(effect.method, 'GET');
  assert.equal(effect.path, '/api/view?path=%2F&cursor=recent%3Ao2');
  assert.equal(view(asked.state).page.more.busy, true);
  assert.deepEqual(update(asked.state, { type: 'action', name: 'view.more', arg: 'recent', ...stamp }).effects, [], 'one page at a time');

  // The second page starts where the first ended; a row both hold is held once.
  const reply = { type: 'http_result', id: effect.id, ok: true, status: 200, body: { data: { recent: [article(2), article(3)] }, next: {} }, ...stamp };
  const done = update(asked.state, reply).state;
  assert.deepEqual(done.view.model.data.recent.map((r) => r.id), [1, 2, 3]);
  assert.equal(view(done).page.more, null, 'the end of the list offers no more');

  // A reply for a page the reader has left is dropped.
  const left = update(asked.state, { type: 'navigate', href: '/about', ...stamp }).state;
  assert.equal(update(left, reply).state.view.model?.data?.recent?.length ?? 0, left.view.model?.data?.recent?.length ?? 0);
});

// Verifies: OPS-318
test('a create is sent with a key made once, and a retry of the same create reuses it', () => {
  const newArticle = { view: 'desk_new', title: 'New article', path: '/desk/new', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site, categories: [], authors: [] } };
  const start = boot('/desk/new', editor, newArticle).state;
  const press = (state) => update(state, { type: 'action', name: 'editor.publish', ...stamp });
  const sentOf = (result) => result.effects.find((e) => e.type === 'http' && e.write);

  const first = press(start);
  const key = sentOf(first).idempotencyKey;
  assert.match(key, /^[\x21-\x7e]{1,128}$/, 'a key is visible ASCII of at most 128 bytes');

  // No answer came: the create may have been made, so the retry carries the same key.
  const lost = update(first.state, { type: 'http_result', id: sentOf(first).id, ok: false, status: 0, body: null, ...stamp }).state;
  const retry = press(lost);
  assert.equal(sentOf(retry).idempotencyKey, key);

  // A refusal answered: nothing was made, and a new attempt is a new action with a key of its own.
  const refused = update(retry.state, { type: 'http_result', id: sentOf(retry).id, ok: false, status: 422, body: { code: 'invalid', message: 'No.', failed: [], correlation: 'c' }, ...stamp }).state;
  assert.notEqual(sentOf(press(refused)).idempotencyKey, key);

  // A change that is not a create carries none.
  const update_ = boot('/desk', editor, deskModel).state;
  const asked = update(update(update_, { type: 'action', name: 'post.ask_delete', arg: { id: 7 }, ...stamp }).state, { type: 'action', name: 'post.delete', arg: { id: 7, version: 3 }, ...stamp });
  assert.equal(sentOf(asked)?.idempotencyKey, undefined);
});
