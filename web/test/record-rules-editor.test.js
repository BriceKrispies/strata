// The editor's use of the record rules (WG-5143): the page refuses a draft the kernel would refuse
// before any request, in the kernel's words, and logs a server refusal of a draft it passed.
// Imports the kernel's update function, which needs the web dependencies installed.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parse, print } from '../src/generated/tenant-rule-language.js';
import { initialState, update } from '../src/kernel/update.js';

const rule = (text) => print(parse(text));

/** What the server sends of an article for an editor who may read every field. */
function rules(extra = {}) {
  return {
    object: 'post',
    fields: [
      { id: 'body', label: 'Body', type: 'str', type_name: 'string', required: true, unique: false },
      { id: 'slug', label: 'Slug', type: 'str', type_name: 'string', required: true, unique: true },
      { id: 'title', label: 'Title', type: 'str', type_name: 'string', required: true, unique: false },
    ],
    limits: { string_bytes: 65536, list_items: 1000, unique_bytes: 200, record_bytes: 262144 },
    constraints: [],
    workflows: [],
    ...extra,
  };
}

const stamp = { at: 1_000, rand: 0.5 };
const session = { authenticated: true, tenant: 'fieldnotes', tenant_name: 'Fieldnotes', principal: 'user:ines', display_name: 'Ines', csrf_token: 'tok', roles: ['public', 'editor'], may: {}, may_manage_schema: false };

function editorState(recordRules) {
  const view = {
    view: 'desk_new',
    title: 'New article',
    path: '/desk/new',
    params: {},
    depends_on: ['post'],
    revision: 1,
    schema_version: 1,
    theme: { accent: 'moss', paper: 'linen' },
    data: { site: { id: 1, object: 'publication', version: 1, fields: { name: 'Fieldnotes' }, refs: {} }, categories: [], authors: [], ...(recordRules ? { $record_rules: { post: recordRules } } : {}) },
    topics: [{ value: '', label: 'No topic' }],
    authors: [{ value: '4', label: 'Ines' }],
  };
  const booted = update(initialState(), { type: 'init', path: '/desk/new', search: '', bootstrap: { path: '/desk/new', session: { session }, view }, systemDark: false, storedTheme: null, ...stamp });
  return booted.state;
}

function typeInto(state, values) {
  let current = state;
  for (const [name, value] of Object.entries(values)) current = update(current, { type: 'input', form: 'editor', name, value, ...stamp }).state;
  return current;
}

test('the editor asks the server for the rules of an article with its view', () => {
  const reloaded = update(editorState(null), { type: 'action', name: 'reload', ...stamp });
  const asked = reloaded.effects.filter((e) => e.type === 'http');
  assert.equal(asked.length, 1);
  assert.match(asked[0].path, /^\/api\/view\?path=%2Fdesk%2Fnew.*&rules=post$/);
});

test('a draft that breaks a tenant constraint shows its message before any request is sent', () => {
  const constraint = { id: 'title_length', rule: rule('len(new.title) <= 5'), message: 'A title has at most 5 characters' };
  const base = editorState(rules({ constraints: [constraint] }));
  const typed = typeInto(base, { title: 'A far too long title', slug: 'a-far-too-long-title', body: 'Text' });
  const saved = update(typed, { type: 'submit', form: 'editor', values: {}, ...stamp });
  assert.deepEqual(saved.effects.filter((e) => e.type === 'http'), [], 'no request is sent');
  assert.equal(saved.state.forms.editor.errors.title, 'A title has at most 5 characters.');
  assert.equal(saved.state.focus.target, 'editor-title');
  assert.equal(saved.state.announce, 'Some fields need attention.');
});

test('a draft that meets the constraint shows no message and is sent', () => {
  const constraint = { id: 'title_length', rule: rule('len(new.title) <= 5'), message: 'A title has at most 5 characters' };
  const base = editorState(rules({ constraints: [constraint]}));
  const typed = typeInto(base, { title: 'Short', slug: 'short', body: 'Text' });
  const saved = update(typed, { type: 'submit', form: 'editor', values: {}, ...stamp });
  assert.deepEqual(saved.state.forms.editor.errors, {});
  assert.equal(saved.effects.filter((e) => e.type === 'http' && e.write).length, 1);
});

test('a view that carried no rules leaves everything to the server', () => {
  const typed = typeInto(editorState(null), { title: 'A far too long title', slug: 'x', body: 'Text' });
  const saved = update(typed, { type: 'submit', form: 'editor', values: {}, ...stamp });
  assert.equal(saved.effects.filter((e) => e.type === 'http' && e.write).length, 1);
});

test('the server\'s refusal of a draft the page passed is shown and logged as drift', () => {
  const base = editorState(rules({ constraints: []}));
  const typed = typeInto(base, { title: 'Short', slug: 'short', body: 'Text' });
  const sent = update(typed, { type: 'submit', form: 'editor', values: {}, ...stamp });
  const request = sent.effects.find((e) => e.type === 'http' && e.write);
  const refused = update(sent.state, {
    type: 'http_result',
    id: request.id,
    ok: false,
    status: 422,
    body: { code: 'rejected', message: 'No', failed: [{ rule: 'constraint:post.title_length', detail: 'Too long [(len(new.title) <= 3)]' }] },
    ...stamp,
  });
  assert.equal(refused.state.forms.editor.errors.title, 'Too long.');
  const log = refused.effects.find((e) => e.type === 'log');
  assert.equal(log?.event, 'record-rules.disagreement');
});
