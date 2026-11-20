// The page builders are a registry keyed by the kind of a generated screen, and the renderers
// build every address of another screen from the generated routes.
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { SCREENS } from '../src/generated/screens.js';
import { pageBuilderOf, registerPageBuilder, registeredBuilderKinds } from '../src/kernel/page-builders.js';
import { initialState, update } from '../src/kernel/update.js';
import { view } from '../src/kernel/view.js';
import '../src/kernel/publication-pages.js';

const stamp = { at: 1_000, rand: 0.5 };
const theme = { accent: 'moss', paper: 'linen' };
const site = { id: 1, version: 1, fields: { name: 'Fieldnotes', tagline: 'Essays.' }, refs: {} };
const anonymous = { authenticated: false, tenant: 'fieldnotes', tenant_name: 'Fieldnotes', roles: ['public'], may: {}, may_manage_schema: false, csrf_token: 'issued' };
const homeModel = { view: 'home', title: 'Home', path: '/', params: {}, depends_on: [], revision: 1, schema_version: 1, theme, data: { site, featured: null, recent: [], categories: [] } };

const home = () => update(initialState(), { type: 'init', path: '/', search: '', bootstrap: { path: '/', session: { session: anonymous }, view: homeModel }, systemDark: false, storedTheme: null, ...stamp }).state;

test('every screen that names a page kind has a builder, except the one that is the missing page', () => {
  const kinds = [...new Set(SCREENS.map((s) => s.kind).filter((k) => k !== null && k !== 'missing'))];
  assert.ok(kinds.length > 5, 'the screens were not found');
  const missing = kinds.filter((k) => pageBuilderOf(k) === null);
  assert.deepEqual(missing, [], 'a screen kind without a builder draws the missing page');
  assert.ok(registeredBuilderKinds().includes('home'));
});

test('a view whose screen has no builder gives the missing page', () => {
  const state = home();
  assert.equal(view(state).page.kind, 'home');
  const unknown = { ...state, view: { ...state.view, model: { ...state.view.model, view: 'no-such-view' } } };
  assert.deepEqual(view(unknown).page, { kind: 'missing', path: '/' });
});

test('a builder is registered by kind, a later one replaces it, and none is found for an unknown kind', () => {
  assert.equal(pageBuilderOf('a-product-kind'), null);
  assert.equal(pageBuilderOf(null), null);
  const first = () => ({ kind: 'a-product-kind', n: 1 });
  const second = () => ({ kind: 'a-product-kind', n: 2 });
  registerPageBuilder('a-product-kind', first);
  assert.equal(pageBuilderOf('a-product-kind'), first);
  registerPageBuilder('a-product-kind', second);
  assert.equal(pageBuilderOf('a-product-kind'), second);
});

test('the renderers write no address of a screen by hand', () => {
  const dir = fileURLToPath(new URL('../src/views', import.meta.url));
  for (const name of readdirSync(dir).filter((n) => n.endsWith('.js'))) {
    const text = readFileSync(`${dir}/${name}`, 'utf8');
    assert.equal(/['"`]\/desk|['"`]\/lab['"`]/.test(text), false, `${name} holds a fixed address of a screen; use hrefFor`);
  }
});
