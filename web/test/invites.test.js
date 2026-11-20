// The owner's invitation (`/login#i=<challenge>&t=<token>`): the fragment is read once and taken
// out of the address bar, a password is chosen with the wording the server gives, and each refusal
// has its message. Pure kernel functions, no browser.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { initialState, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';

const stamp = { at: 1_000, rand: 0.5 };
const theme = { accent: 'moss', paper: 'linen' };
const site = { id: 1, version: 1, fields: { name: 'Fieldnotes', tagline: 'Essays.' }, refs: {} };
const anonymous = { authenticated: false, tenant: 'fieldnotes', tenant_name: 'Fieldnotes', roles: ['public'], may: {}, may_manage_schema: false, csrf_token: 'issued' };
const loginModel = { view: 'login', title: 'Sign in', path: '/login', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site } };
const homeModel = { view: 'home', title: 'Home', path: '/', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site, featured: null, recent: [], categories: [] } };

const CHALLENGE = 'a'.repeat(64);
const TOKEN = 'b'.repeat(64);
const HASH = `#i=${CHALLENGE}&t=${TOKEN}`;
const REFUSED = 'That invitation was not accepted. Ask for a new one.';
const SHORT = 'Choose a password of at least 10 characters.';

function open(path, hash, model) {
  return update(initialState(), { type: 'init', path, search: '', hash, bootstrap: { path, session: { session: anonymous }, view: model }, systemDark: false, storedTheme: null, ...stamp });
}

const reply = (state, effect, body, ok = true, status = 200) => update(state, { type: 'http_result', id: effect.id, ok, status, body, ...stamp });
const typed = (state, password) => update(state, { type: 'input', seq: 1, form: 'invite', name: 'password', value: password, ...stamp }).state;
const submit = (state, password) => update(state, { type: 'submit', form: 'invite', values: { password }, ...stamp });
const posts = (effects) => effects.filter((e) => e.type === 'http' && e.method === 'POST');

test('the invitation in the fragment is read once, taken out of the address bar and not drawn', () => {
  const opened = open('/login', HASH, loginModel);
  assert.deepEqual(opened.state.invites.landing, { challenge: CHALLENGE, token: TOKEN });
  assert.deepEqual(opened.effects.filter((e) => e.type === 'push_url'), [{ type: 'push_url', href: '/login', replace: true }], 'the address loses its fragment at once');
  const { page } = view(opened.state);
  assert.equal(page.kind, 'login');
  assert.equal(page.invite.open, true);
  const drawn = JSON.stringify(view(opened.state));
  assert.equal(drawn.includes(CHALLENGE) || drawn.includes(TOKEN), false, 'the secrets of the link are not in the view model');
  assert.equal(posts(opened.effects).length, 0, 'nothing is sent before a password is chosen');
});

test('an invitation fragment on any other page is left alone', () => {
  const opened = open('/', HASH, homeModel);
  assert.equal(opened.state.invites.landing, null);
  assert.equal(opened.effects.some((e) => e.type === 'push_url'), false);
});

test('a fragment that names an invitation but is malformed is removed and refused without a request', () => {
  const opened = open('/login', '#i=short&t=alsoshort', loginModel);
  assert.equal(opened.state.invites.landing, null);
  assert.equal(opened.state.invites.failed, true);
  assert.equal(opened.state.invites.error, REFUSED);
  assert.equal(opened.effects.some((e) => e.type === 'push_url' && e.replace === true && e.href === '/login'), true);
  assert.equal(view(opened.state).page.invite.open, false, 'no form is shown');
  assert.equal(posts(opened.effects).length, 0);
});

test('a password that is too short is refused on the page, with the wording the server gives', () => {
  const state = typed(open('/login', HASH, loginModel).state, 'short');
  const sent = submit(state, 'short');
  assert.equal(posts(sent.effects).length, 0, 'nothing is sent');
  assert.equal(sent.state.invites.fieldError, SHORT);
  assert.equal(sent.state.announce, SHORT);
  assert.equal(sent.state.invites.landing !== null, true, 'the invitation is kept for another try');
  assert.equal(view(sent.state).page.invite.fieldError, SHORT);
});

test('a chosen password is posted with the challenge and the token of the link, with the request token', () => {
  const sent = submit(open('/login', HASH, loginModel).state, 'a long enough password');
  const [post] = posts(sent.effects);
  assert.equal(post.path, '/api/invite/accept');
  assert.deepEqual(post.body, { challenge: CHALLENGE, token: TOKEN, password: 'a long enough password' });
  assert.equal(post.csrf, 'issued');
  assert.equal(sent.state.invites.busy, true);
  assert.equal(posts(submit(sent.state, 'a long enough password').effects).length, 0, 'a second press while waiting sends nothing');
});

test('an accepted invitation takes the owner to sign in as the reply names, with a confirmation', () => {
  const sent = submit(open('/login', HASH, loginModel).state, 'a long enough password');
  const done = reply(sent.state, posts(sent.effects)[0], { accepted: true, login: 'admin' });
  assert.equal(done.state.invites.landing, null, 'the secrets are dropped');
  assert.equal(done.state.invites.busy, false);
  assert.equal(done.state.forms.login.values.login, 'admin', 'the sign-in form holds the login');
  assert.equal(done.state.forms.login.values.password, '');
  assert.equal(done.state.session?.authenticated, false, 'accepting signs nobody in');
  assert.equal(done.state.toasts.some((t) => t.kind === 'ok' && t.text.includes('admin')), true, 'the confirmation names the login');
  assert.equal(view(done.state).page.invite.open, false);
  assert.equal(JSON.stringify(done.state).includes(TOKEN), false, 'the token is nowhere in the state');
});

test('a 401 says to ask for a new link and does not say which part was wrong', () => {
  const sent = submit(open('/login', HASH, loginModel).state, 'a long enough password');
  const failed = reply(sent.state, posts(sent.effects)[0], { code: 'unauthenticated', message: 'That invitation was not accepted. Ask for a new one.' }, false, 401);
  assert.equal(failed.state.invites.failed, true);
  assert.equal(failed.state.invites.landing, null, 'the link is of no more use');
  assert.equal(failed.state.invites.error, REFUSED);
  assert.equal(failed.state.announce, REFUSED);
  const { page } = view(failed.state);
  assert.equal(page.invite.open, false, 'the form is gone');
  assert.equal(page.invite.failed, true);
  assert.equal(page.invite.error, REFUSED);
  assert.equal(JSON.stringify(failed.state).includes(TOKEN), false);
});

test('the refusal of a password the page let through keeps the form and says what to change', () => {
  const sent = submit(open('/login', HASH, loginModel).state, 'a long enough password');
  const failed = reply(sent.state, posts(sent.effects)[0], { code: 'invalid', message: SHORT }, false, 400);
  assert.equal(failed.state.invites.failed, false);
  assert.equal(failed.state.invites.landing !== null, true, 'the owner may choose again');
  assert.equal(failed.state.invites.busy, false);
  assert.equal(failed.state.invites.error, SHORT);
});

test('a throttled caller and an unreachable site each get their own message and may try again', () => {
  const sent = submit(open('/login', HASH, loginModel).state, 'a long enough password');
  const throttled = reply(sent.state, posts(sent.effects)[0], { code: 'throttled', message: 'Too many attempts. Try again later.' }, false, 429);
  assert.equal(throttled.state.invites.error, 'Too many attempts. Try again later.');
  assert.equal(throttled.state.invites.landing !== null, true);

  const again = submit(throttled.state, 'a long enough password');
  const down = reply(again.state, posts(again.effects)[0], null, false, 0);
  assert.equal(down.state.invites.error, 'Your password could not be set just now. Try again.');
  assert.equal(down.state.invites.landing !== null, true);
});
