// The frontend kernel is pure, so it is tested as plain functions: no browser, no worker.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { prose } from '../src/components/parts.js';
import { explain } from '../src/generated/errors.js';
import * as forms from '../src/generated/forms.js';
import { initialState, parseHref, slugify, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';

const stamp = { at: 1_000, rand: 0.5 };

function homeModel(extra = {}) {
  return {
    view: 'home',
    title: 'Home',
    path: '/',
    params: {},
    depends_on: ['post', 'author', 'category', 'publication'],
    revision: 3,
    theme: { accent: 'moss', paper: 'linen' },
    data: {
      site: { id: 1, version: 1, fields: { name: 'Fieldnotes', tagline: 'Essays.' }, refs: {} },
      featured: null,
      recent: [],
      categories: [],
    },
    ...extra,
  };
}

// A visitor with no session still holds a request token, which the gateway issued with the reply.
const anonymous = { authenticated: false, tenant: 'fieldnotes', tenant_name: 'Fieldnotes', roles: ['public'], may: {}, may_manage_schema: false, csrf_token: 'issued' };

function boot(bootstrap) {
  return update(initialState(), { type: 'init', path: '/', search: '', bootstrap, systemDark: false, storedTheme: null, ...stamp });
}

function run(state, messages) {
  const effects = [];
  for (const message of messages) {
    const result = update(state, { ...stamp, ...message });
    state = result.state;
    effects.push(...result.effects);
  }
  return { state, effects };
}

test('update never changes the state it was given', () => {
  const frozen = structuredClone(initialState());
  const before = JSON.stringify(frozen);
  const freeze = (value) => {
    if (value && typeof value === 'object') {
      Object.values(value).forEach(freeze);
      Object.freeze(value);
    }
    return value;
  };
  update(freeze(frozen), { type: 'init', path: '/posts/x', search: '?q=1', bootstrap: null, systemDark: true, storedTheme: 'dark', ...stamp });
  assert.equal(JSON.stringify(frozen), before);
});

test('the same state and message give the same result', () => {
  const message = { type: 'navigate', href: '/topics', ...stamp };
  const { state } = boot({ path: '/', session: { session: anonymous }, view: homeModel() });
  assert.deepEqual(update(state, message), update(state, message));
});

test('a bootstrapped start draws at once and only opens the socket', () => {
  const { state, effects } = boot({ path: '/', session: { session: anonymous }, view: homeModel() });
  assert.deepEqual(effects, [{ type: 'ws_connect', csrf: 'issued' }]);
  assert.equal(view(state).page.kind, 'home');
  assert.equal(view(state).title, 'Fieldnotes');
});

test('a cold start asks for the session and the view, each with its own correlation id', () => {
  const { effects } = boot(null);
  const requests = effects.filter((e) => e.type === 'http');
  assert.deepEqual(requests.map((e) => e.path), ['/api/session', '/api/view?path=%2F']);
  assert.equal(new Set(requests.map((e) => e.correlation)).size, 2);
  assert.match(requests[0].correlation, /^c-\d+-[0-9a-f]{6}$/);
});

test('navigating pushes the address, scrolls up and fetches; a stale reply is ignored', () => {
  const start = boot({ path: '/', session: { session: anonymous }, view: homeModel() }).state;
  const first = update(start, { type: 'navigate', href: '/topics', ...stamp });
  assert.deepEqual(first.effects.map((e) => e.type), ['push_url', 'scroll_top', 'http', 'timer']);
  const second = update(first.state, { type: 'navigate', href: '/about', ...stamp });
  const staleId = first.effects[2].id;
  const afterStale = update(second.state, { type: 'http_result', id: staleId, ok: true, status: 200, body: homeModel({ view: 'topics' }), ...stamp });
  assert.equal(afterStale.state.view.status, 'loading', 'the reply to the abandoned navigation does not land');
});

function connected() {
  const start = boot({ path: '/', session: { session: anonymous }, view: homeModel() }).state;
  return run(start, [{ type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'c', head: 40, session: 'anonymous', heartbeat_ms: 10000 } }]);
}

test('on welcome the kernel subscribes and starts the heartbeat', () => {
  const { state, effects } = connected();
  assert.equal(state.ws.status, 'open');
  assert.equal(state.ws.lastSeq, 40);
  // The first subscription starts at the server head, so the page that was read before it is read again.
  assert.deepEqual(effects.map((e) => e.type), ['ws_send', 'timer', 'http', 'timer']);
  assert.equal(effects[0].message.type, 'subscribe');
  assert.equal('after' in effects[0].message, false, 'nothing to resume on a first connection');
});

test('an event about something on the page refetches it quietly and is acknowledged', () => {
  const { state } = connected();
  const event = { type: 'event', sub: 'site', n: 1, seq: 41, correlation: 'c-9', notice: { kind: 'upsert', object: 'post', id: 7, version: 2 } };
  const result = update(state, { type: 'ws_message', message: event, ...stamp });
  assert.deepEqual(result.effects.map((e) => e.type), ['ws_send', 'http', 'timer']);
  assert.deepEqual(result.effects[0].message, { type: 'ack', n: 1 });
  assert.equal(result.state.view.status, 'ready', 'the page stays on screen while it refreshes');
  assert.equal(result.state.ws.lastSeq, 41);
});

test('an event about something the page does not show is only acknowledged', () => {
  const { state } = connected();
  const event = { type: 'event', sub: 'site', n: 1, seq: 41, correlation: 'c-9', notice: { kind: 'upsert', object: 'invoice', id: 7, version: 2 } };
  assert.deepEqual(update(state, { type: 'ws_message', message: event, ...stamp }).effects.map((e) => e.type), ['ws_send']);
});

test('a gap in the connection counter drops the socket instead of carrying on stale', () => {
  const { state } = connected();
  const skipped = { type: 'event', sub: 'site', n: 2, seq: 42, correlation: '', notice: { kind: 'upsert', object: 'post', id: 7, version: 2 } };
  const result = update(state, { type: 'ws_message', message: skipped, ...stamp });
  assert.deepEqual(result.effects.map((e) => e.type), ['ws_close', 'http', 'timer'], 'what was missed is read again at once');
  assert.equal(result.state.view.status, 'ready', 'the page stays on screen while it refreshes');
  assert.equal(result.state.ws.lastSeq, null, 'the next connection starts from scratch');
});

test('after a disconnect the kernel backs off, reconnects and resumes from the last sequence', () => {
  const { state } = connected();
  const seen = update(state, { type: 'ws_message', message: { type: 'pong', nonce: 1, head: 45 }, ...stamp }).state;
  const closed = update(seen, { type: 'ws_closed', ...stamp });
  assert.equal(closed.state.ws.status, 'waiting');
  const [timer] = closed.effects;
  assert.equal(timer.type, 'timer');
  // The wait is the machine's own, whatever the random stamp: the specification models no jitter.
  assert.equal(timer.ms, 500);
  const other = update(seen, { type: 'ws_closed', at: 1_000, rand: 0.01 });
  assert.equal(other.effects[0].ms, 500);
  const retried = update(closed.state, { type: 'timer', name: 'reconnect', token: timer.token, ...stamp });
  assert.deepEqual(retried.effects, [{ type: 'ws_connect', csrf: 'issued' }]);
  const again = update(retried.state, { type: 'ws_message', message: { type: 'welcome', v: 1, conn: 'd', head: 50, session: 'anonymous', heartbeat_ms: 10000 }, ...stamp });
  assert.equal(again.effects[0].message.after, 45, 'resume from what was last seen, not from the new head');

  // The delay never exceeds eight seconds however long the outage.
  let s = closed.state;
  for (let i = 0; i < 12; i += 1) {
    const next = update(s, { type: 'ws_closed', at: 0, rand: 0.999 });
    assert.ok(next.effects[0].ms <= 8000);
    s = next.state;
  }
  // A timer from an earlier attempt is ignored.
  assert.deepEqual(update(s, { type: 'timer', name: 'reconnect', token: timer.token, ...stamp }).effects, []);
});

test('each failed reconnect waits its own delay: 0.5, 1, 2, 4, then 8 seconds, the cap', () => {
  const { state } = connected();
  // The first close of an open socket waits 500 ms; a close while waiting to reconnect is a failed
  // attempt, and each one waits twice as long, to the cap. The stamp's random part changes nothing.
  const waits = [];
  let s = state;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const next = update(s, { type: 'ws_closed', at: 1_000 + attempt, rand: attempt % 2 ? 0.01 : 0.999 });
    assert.equal(next.state.ws.status, 'waiting');
    const timer = next.effects.find((e) => e.type === 'timer');
    waits.push(timer.ms);
    s = next.state;
  }
  assert.deepEqual(waits, [500, 1000, 2000, 4000, 8000, 8000, 8000, 8000]);
});

test('a delayed emit of the connection machine is performed when its timer fires, and once', () => {
  const { state } = connected();
  const waiting = { ...state, ws: { ...state.ws, delayed: { 77: { to: 'x', delivery: 'at-most-once', event: 'ui-live-ping', payload: {}, afterMs: 250 } } } };
  const fired = update(waiting, { type: 'timer', name: 'emit', token: 77, ...stamp });
  assert.deepEqual(fired.effects.map((e) => e.message?.type), ['ping']);
  assert.deepEqual(fired.state.ws.delayed, {});
  assert.deepEqual(update(fired.state, { type: 'timer', name: 'emit', token: 77, ...stamp }).effects, []);
});

test('a resync makes the page read again from the server head', () => {
  const { state } = connected();
  const result = update(state, { type: 'ws_message', message: { type: 'resync', reason: 'too far behind', head: 90 }, ...stamp });
  assert.equal(result.state.ws.lastSeq, 90);
  assert.deepEqual(result.effects.map((e) => e.type), ['http', 'timer']);
});

test('writes carry the session CSRF token; reads do not', () => {
  const session = { ...anonymous, authenticated: true, principal: 'user:ines', display_name: 'Inès', csrf_token: 'tok', roles: ['public', 'editor'], may: { post: ['create'] } };
  const start = boot({ path: '/', session: { session }, view: homeModel() }).state;
  const logout = update(start, { type: 'action', name: 'logout', ...stamp }).effects[0];
  assert.equal(logout.csrf, 'tok');
  const read = update(start, { type: 'navigate', href: '/about', ...stamp }).effects.find((e) => e.type === 'http');
  assert.equal(read.csrf, undefined);
});

test('signing in and the socket carry the request token the gateway issued', () => {
  const start = boot({ path: '/', session: { session: anonymous }, view: homeModel() }).state;
  const signIn = update(start, { type: 'submit', form: 'login', values: { login: 'ines', password: 'pw' }, ...stamp }).effects[0];
  assert.equal(signIn.path, '/api/login');
  assert.equal(signIn.csrf, 'issued');

  // A cold start has no token until the session reply arrives, and opens the socket only then.
  const cold = boot(null);
  assert.deepEqual(cold.effects.filter((e) => e.type === 'ws_connect'), []);
  const session = cold.effects.find((e) => e.type === 'http' && e.path === '/api/session');
  const replied = update(cold.state, { type: 'http_result', id: session.id, ok: true, status: 200, body: { session: anonymous, state: 'anonymous' }, ...stamp });
  assert.deepEqual(replied.effects.filter((e) => e.type === 'ws_connect'), [{ type: 'ws_connect', csrf: 'issued' }]);
});

test('rich text from the server is drawn as text and as a fixed set of elements, never as markup', () => {
  const hostile = '<script>alert(1)</script> and <img src=x onerror=alert(1)>';
  const blocks = [
    { type: 'heading', level: 2, inlines: [{ type: 'text', text: '<h1>not a heading</h1>' }] },
    { type: 'paragraph', inlines: [{ type: 'text', text: hostile }, { type: 'strong', children: [{ type: 'text', text: '<b>x</b>' }] }, { type: 'link', href: 'https://example.com', children: [{ type: 'text', text: '<a href=x>y</a>' }] }] },
    { type: 'quote', blocks: [{ type: 'paragraph', inlines: [{ type: 'code', text: '<svg onload=alert(1)>' }] }] },
    { type: 'list', ordered: false, items: [[{ type: 'text', text: '<li>one</li>' }]] },
    { type: 'code', lang: '', text: '<iframe src=x></iframe>' },
    { type: 'figure', src: '/media/kiln.svg', alt: '<b>alt</b>', caption: '<i>caption</i>' },
    { type: 'html', html: '<script>alert(2)</script>' },
  ];
  const tags = new Set();
  const strings = [];
  const walk = (node) => {
    if (typeof node === 'string') return strings.push(node);
    tags.add(node.tag);
    node.children.forEach(walk);
  };
  prose(blocks).filter(Boolean).forEach(walk);
  // The only elements are the ones the renderer itself names: nothing the text says adds one.
  assert.deepEqual(
    [...tags].sort(),
    ['a', 'blockquote', 'code', 'figcaption', 'figure', 'h2', 'img', 'li', 'p', 'pre', 'strong', 'ul'],
  );
  // The markup survives as text, character for character.
  for (const text of [hostile, '<h1>not a heading</h1>', '<b>x</b>', '<a href=x>y</a>', '<svg onload=alert(1)>', '<li>one</li>', '<iframe src=x></iframe>', '<i>caption</i>']) {
    assert.ok(strings.some((s) => s.includes(text)), `${text} was not kept as text`);
  }
  assert.ok(!strings.some((s) => s.includes('alert(2)')), 'an unknown block kind is dropped, not passed through');
});

test('nothing in the frontend parses text as markup or runs it as code', () => {
  const sinks = /innerHTML|outerHTML|insertAdjacentHTML|document\.write|DOMParser|createContextualFragment|setHTML|dangerouslySet|new Function|\beval\(/;
  const files = [];
  const visit = (dir) => {
    for (const name of readdirSync(dir)) {
      const path = `${dir}/${name}`;
      if (statSync(path).isDirectory()) visit(path);
      else if (path.endsWith('.js')) files.push(path);
    }
  };
  visit(fileURLToPath(new URL('../src', import.meta.url)));
  assert.ok(files.length > 10, 'the sources were not found');
  for (const file of files) {
    const hit = readFileSync(file, 'utf8').match(sinks);
    assert.equal(hit, null, `${file} uses ${hit?.[0]}`);
  }
});

test('the theme control cycles and is stored', () => {
  let { state } = boot({ path: '/', session: { session: anonymous }, view: homeModel() });
  const modes = [];
  for (let i = 0; i < 3; i += 1) {
    const result = update(state, { type: 'action', name: 'theme', ...stamp });
    state = result.state;
    modes.push(result.effects[0].value);
  }
  assert.deepEqual(modes, ['light', 'dark', 'auto']);
  assert.equal(view(update(state, { type: 'system_theme', dark: true, ...stamp }).state).theme.scheme, 'dark');
});

test('kernel checks are explained in terms of the field they are about', () => {
  const form = 'form-new-article-save';
  assert.deepEqual(explain(form, { rule: 'required:post.title', detail: 'a value is required' }), { field: 'title', text: 'This is required.' });
  assert.deepEqual(
    explain(form, { rule: 'constraint:post.excerpt_length', detail: 'An excerpt has at most 280 characters [(len(new.excerpt) <= 280)]' }),
    { field: 'excerpt', text: 'An excerpt has at most 280 characters.' },
  );
  assert.equal(explain(form, { rule: 'constraint:post.cover_is_described', detail: 'A cover needs a description [x]' }).field, 'cover');
  assert.equal(explain(form, { rule: 'unique:post.slug', detail: 'record 3 already has this value' }).field, 'slug');
  assert.equal(explain(form, { rule: 'policy:authors_own_posts', detail: '...' }).field, null);
});

test('addresses and slugs', () => {
  assert.deepEqual(parseHref('/search?q=wood+kiln&x=%C3%A9'), { path: '/search', query: { q: 'wood kiln', x: 'é' } });
  assert.deepEqual(parseHref('/topics/'), { path: '/topics', query: {} });
  assert.equal(slugify('  What the Kiln Decides: Part 2!  '), 'what-the-kiln-decides-part-2');
  assert.equal(slugify('Inès & the Café'), 'ines-the-cafe');
});

test('a guarded page shows a sign-in prompt to a visitor, whatever the server sent', () => {
  const model = homeModel({ view: 'desk', data: { site: homeModel().data.site, posts: [] } });
  const { state } = boot({ path: '/', session: { session: anonymous }, view: model });
  assert.equal(view(state).page.kind, 'signin_required');
});

test('a signed-out visit to a guarded page whose view is refused for want of a session shows the sign-in prompt', () => {
  const started = boot({ path: '/', session: { session: anonymous }, view: homeModel() }).state;
  const visit = update(started, { type: 'navigate', href: '/desk', ...stamp });
  const request = visit.effects.find((e) => e.type === 'http' && e.path.startsWith('/api/view'));
  const refused = update(visit.state, { type: 'http_result', id: request.id, ok: false, status: 401, body: { code: 'unauthenticated', message: 'Sign in to do this.' }, ...stamp });
  assert.deepEqual(view(refused.state).page, { kind: 'signin_required', next: '/desk' });
});

test('a server failure on a guarded page is still drawn as an error', () => {
  const started = boot({ path: '/', session: { session: anonymous }, view: homeModel() }).state;
  const visit = update(started, { type: 'navigate', href: '/desk', ...stamp });
  const request = visit.effects.find((e) => e.type === 'http' && e.path.startsWith('/api/view'));
  const failed = update(visit.state, { type: 'http_result', id: request.id, ok: false, status: 500, body: { code: 'internal', message: 'Something broke.' }, ...stamp });
  assert.equal(view(failed.state).page.kind, 'error');
});

test('a message on screen is given a timer and leaves when it fires', () => {
  const { state } = boot({ path: '/', session: { session: anonymous }, view: homeModel() });
  const shown = update(state, { type: 'action', name: 'logout', ...stamp });
  const replied = update(shown.state, { type: 'http_result', id: shown.effects[0].id, ok: false, status: 0, body: null, ...stamp });
  assert.equal(replied.state.toasts.length, 1);
  const timer = replied.effects.find((e) => e.type === 'timer' && e.name === 'toast');
  assert.equal(timer.token, replied.state.toasts[0].id);
  assert.equal(update(replied.state, { type: 'timer', name: 'toast', token: timer.token, ...stamp }).state.toasts.length, 0);
});

// A person signed in, as the gateway answers the session read.
const member = { ...anonymous, authenticated: true, principal: 'user:ines', display_name: 'Inès', csrf_token: 'tok', roles: ['public', 'editor'], may: { post: ['create'] } };

// F1 (gap analysis, section 7): on a page that needs no session, a session end kept the view model read
// for the former person, and the page drew it for the visitor until the next reply. The spec's
// `switched` drops the model whatever the page, and the view is read again.
test('a session end drops the view read for the former person, on a page that needs no session', () => {
  const start = boot({ path: '/', session: { session: member }, view: homeModel() }).state;
  assert.equal(start.view.model.view, 'home', 'the page holds the model read for the signed-in person');
  const ended = update(start, { type: 'ws_message', message: { type: 'session', state: 'expired' }, ...stamp });
  assert.equal(ended.state.view.model, null, 'the former person\'s model is not drawn for the visitor');
  assert.equal(ended.state.view.status, 'loading', 'the page waits for the view it reads again');
  assert.ok(ended.effects.some((e) => e.type === 'http'), 'the view is read again');
});

// F3: a session read that names another person, or nobody, left the former person's state in place: the
// session changed and the model read for the former person stayed on the page. The spec's sessionChanged
// is a change of person (switched): the model is dropped and the view is read again.
test('a session read naming another person, or nobody, drops what the former person held', () => {
  const others = [{ ...member, principal: 'user:omar', display_name: 'Omar', csrf_token: 'tok2' }, anonymous];
  for (const session of others) {
    const start = boot({ path: '/', session: { session: member }, view: homeModel() }).state;
    // A refused socket upgrade reads the session again; the read answers with the other session.
    const refused = update(start, { type: 'ws_closed', refused: true, ...stamp });
    const read = refused.effects.find((e) => e.type === 'http' && e.path === '/api/session');
    const replied = update(refused.state, { type: 'http_result', id: read.id, ok: true, status: 200, body: { session }, ...stamp });
    assert.equal(replied.state.view.model, null, 'the model read for the former person is dropped');
    assert.equal(replied.state.generation, 1, 'the change of person is counted');
    assert.ok(replied.effects.some((e) => e.type === 'http' && e.path.startsWith('/api/view')), 'the view is read again');
  }
});

// F2: a view asked for before the person changed was applied after the change, because a view request was not
// a person-scoped kind: its reply was judged by its request id alone. The spec drops the replies to the former
// person's requests (the generation moves on). The change here is a session read naming another person; a
// session end refreshes the view quietly and so already replaces the request id, which is why the session
// read is the path that shows it on the current code.
test('a view asked for before the person changed is not applied after the change', () => {
  const start = boot({ path: '/', session: { session: member }, view: homeModel() }).state;
  const visit = update(start, { type: 'navigate', href: '/topics', ...stamp });
  const asked = visit.effects.find((e) => e.type === 'http');
  const refused = update(visit.state, { type: 'ws_closed', refused: true, ...stamp });
  const read = refused.effects.find((e) => e.type === 'http' && e.path === '/api/session');
  const changed = update(refused.state, { type: 'http_result', id: read.id, ok: true, status: 200, body: { session: { ...member, principal: 'user:omar', csrf_token: 'tok2' } }, ...stamp });
  const late = update(changed.state, { type: 'http_result', id: asked.id, ok: true, status: 200, body: homeModel({ view: 'topics', path: '/topics' }), ...stamp });
  assert.equal(late.state.view.model, null, 'the reply for the former person\'s view is not held');
  assert.equal(late.state.view.status, 'loading', 'the page waits for the view it reads again');
});

// F4: a cold start held a view that answered before the session was known, and never read the view again once the
// session answered. The spec holds no view until the session is known: an early answer is dropped, and the view is
// read again when the session answers.
test('a cold start holds no view that answers before the session does, and reads it again once the session is known', () => {
  const cold = boot(null);
  const session = cold.effects.find((e) => e.type === 'http' && e.path === '/api/session');
  const asked = cold.effects.find((e) => e.type === 'http' && e.path.startsWith('/api/view'));
  const early = update(cold.state, { type: 'http_result', id: asked.id, ok: true, status: 200, body: homeModel(), ...stamp });
  assert.equal(early.state.view.model, null, 'the view is not held before the session is known');
  const learned = update(early.state, { type: 'http_result', id: session.id, ok: true, status: 200, body: { session: anonymous }, ...stamp });
  assert.equal(learned.state.view.model, null, 'and nothing is drawn while the view is read again');
  assert.ok(learned.effects.some((e) => e.type === 'http' && e.path.startsWith('/api/view')), 'the view is read again');
});

// F6: a change of person kept the login name typed into the sign-in form (only the password was cleared). The
// spec's switched clears the whole form: its values and its refusal.
test('a change of person clears the sign-in form, the typed name too', () => {
  const start = boot({ path: '/', session: { session: member }, view: homeModel() }).state;
  const typed = update(start, { type: 'input', form: 'login', name: 'login', value: 'ines', ...stamp }).state;
  assert.equal(typed.forms.login.values.login, 'ines', 'the name is typed in');
  const ended = update(typed, { type: 'ws_message', message: { type: 'session', state: 'expired' }, ...stamp }).state;
  assert.deepEqual(ended.forms.login.values, forms.initialValues('form-sign-in', {}), 'the sign-in form is cleared');
  assert.equal(ended.forms.login.error, '');
});
