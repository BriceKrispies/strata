// What the page does with what the specification generates: the delete confirmation, the live
// filter, the generated names, the principal and the addresses it writes into the DOM.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { safeUrl } from '../src/components/dom.js';
import * as actions from '../src/generated/actions.js';
import { initialState, principalOf, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';

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
