// The page's check of a record before it is sent (WG-5143): `web/src/kernel/record-rules.js`
// against what the server sends of a record type, with the generated evaluator of the tenant
// rule language (`web/src/generated/tenant-rule-language.js`). The editor's use of it is in
// record-rules-editor.test.js.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { refusal } from '../src/generated/errors.js';
import { parse, print } from '../src/generated/tenant-rule-language.js';
import { MESSAGES, checkRecord, disagreement, draftOf } from '../src/kernel/record-rules.js';

const rule = (text) => print(parse(text));

/** What the server sends of an article for an editor who may read every field. */
function rules(extra = {}) {
  return {
    object: 'post',
    fields: [
      { id: 'body', label: 'Body', type: 'str', type_name: 'string', required: true, unique: false },
      { id: 'slug', label: 'Slug', type: 'str', type_name: 'string', required: true, unique: true },
      { id: 'status', label: 'Status', type: 'str', type_name: 'string', required: true, unique: false },
      { id: 'tags', label: 'Tags', type: 'list', type_name: 'list<string>', required: false, unique: false },
      { id: 'title', label: 'Title', type: 'str', type_name: 'string', required: true, unique: false },
    ],
    limits: { string_bytes: 20, list_items: 2, unique_bytes: 200, record_bytes: 262144 },
    constraints: [{ id: 'title_length', rule: rule('len(new.title) >= 1 and len(new.title) <= 24'), message: 'A title has between 1 and 24 characters' }],
    workflows: [
      {
        id: 'post_status',
        field: 'status',
        initial: ['draft'],
        state: 'draft',
        transitions: [{ name: 'publish', from: 'draft', to: 'published', guard: rule('has_role(editor)'), server_decides: false, sets: [] }],
      },
    ],
    ...extra,
  };
}

const complete = { title: 'A title', slug: 'a-title', body: 'Text', status: 'draft' };

test('a draft that meets every rule fails nothing', () => {
  const verdict = checkRecord(rules(), complete, { roles: ['editor'], now: 5 });
  assert.deepEqual(verdict.failed, []);
});

test('a missing required field and a value over a limit fail in the kernel\'s words', () => {
  const verdict = checkRecord(rules(), { ...complete, body: undefined, title: 'x'.repeat(21), tags: ['a', 'b', 'c'] }, {});
  assert.deepEqual(verdict.failed, [
    { rule: 'required:post.body', detail: 'a value is required' },
    { rule: 'limit:post.tags', detail: 'a list of 3 items is longer than the maximum of 2' },
    { rule: 'limit:post.title', detail: 'a string of 21 bytes is longer than the maximum of 20' },
  ]);
  assert.equal(MESSAGES.stringTooLong(21, 20), verdict.failed[2].detail);
});

test('a constraint is violated only by false, and says its message and its rule', () => {
  // An empty string is a value, not an absent one: it passes `required` and fails the constraint.
  const broken = checkRecord(rules(), { ...complete, title: '' }, {});
  assert.deepEqual(broken.failed.map((f) => f.rule), ['constraint:post.title_length']);
  assert.deepEqual(checkRecord(rules(), { ...complete, title: null }, {}).failed.map((f) => f.rule), ['required:post.title']);
  const long = checkRecord(rules({ limits: { string_bytes: 100, list_items: 2, unique_bytes: 200, record_bytes: 1 } }), { ...complete, title: 'x'.repeat(30) }, {});
  assert.deepEqual(long.failed, [
    { rule: 'constraint:post.title_length', detail: `A title has between 1 and 24 characters [${rule('len(new.title) >= 1 and len(new.title) <= 24')}]` },
  ]);
  assert.deepEqual(checkRecord(rules({ limits: { string_bytes: 100, list_items: 2, unique_bytes: 200, record_bytes: 1 } }), { ...complete, title: 'x'.repeat(24) }, {}).failed, []);
});

test('the check is shown as the server\'s refusal is: the failed checks give the fields their messages', () => {
  const { failed } = checkRecord(rules(), { ...complete, body: null }, {});
  const shown = refusal('form-new-article-save', { failed });
  assert.deepEqual(shown.errors, { body: 'This is required.' });
});

test('a constraint over a field the page was not sent is the server\'s to decide, never a pass or a block', () => {
  const secretive = rules({ constraints: [{ id: 'hidden', rule: rule('len(new.secret) <= 3'), message: 'No' }] });
  const verdict = checkRecord(secretive, complete, {});
  assert.deepEqual(verdict.failed, []);
  assert.deepEqual(verdict.undecided.map((u) => u.rule).filter((r) => r.startsWith('constraint')), ['constraint:post.hidden']);
});

test('a rule that follows a reference, or reads a system attribute, is left to the server', () => {
  const rule1 = parse('new.title == \'x\'');
  assert.ok(rule1);
  const through = rules({ constraints: [{ id: 'follows', rule: 'new.slug == \'x\'', message: 'No' }] });
  // The page never follows a reference: this evaluates on the draft alone and is decided.
  assert.equal(checkRecord(through, complete, {}).failed.length, 1);
  const system = rules({ constraints: [{ id: 'owner', rule: rule('new.$created_by == principal'), message: 'No' }] });
  const verdict = checkRecord(system, complete, { principal: 'user:a' });
  assert.deepEqual(verdict.failed, []);
  assert.equal(verdict.undecided.at(-1).rule, 'constraint:post.owner');
});

test('a unique value is never decided by the page, and a rule the page cannot read is not a block', () => {
  const verdict = checkRecord(rules({ constraints: [{ id: 'odd', rule: '(1 +', message: 'No' }] }), complete, {});
  assert.deepEqual(verdict.failed, []);
  assert.ok(verdict.undecided.some((u) => u.rule === 'unique:post.slug'));
  assert.ok(verdict.undecided.some((u) => u.rule === 'constraint:post.odd'));
});

test('the guard of the transition being attempted is evaluated with the session\'s roles', () => {
  const asReader = checkRecord(rules(), { ...complete, status: 'published' }, { old: complete, roles: ['reader'] });
  assert.deepEqual(asReader.failed, [{ rule: 'workflow:post_status', detail: 'publish: guard is not true' }]);
  const asEditor = checkRecord(rules(), { ...complete, status: 'published' }, { old: complete, roles: ['editor'] });
  assert.deepEqual(asEditor.failed, []);
  const nowhere = checkRecord(rules(), { ...complete, status: 'archived' }, { old: complete, roles: ['editor'] });
  assert.deepEqual(nowhere.failed, [{ rule: 'workflow:post_status', detail: 'no transition from Str("draft") to Str("archived")' }]);
});

test('a guard the server kept to itself is not decided by the page', () => {
  const hidden = rules();
  hidden.workflows[0].transitions[0] = { name: 'publish', from: 'draft', to: 'published', guard: null, server_decides: true, sets: [] };
  const verdict = checkRecord(hidden, { ...complete, status: 'published' }, { old: complete });
  assert.deepEqual(verdict.failed, []);
  assert.ok(verdict.undecided.some((u) => u.rule === 'workflow:post_status'));
});

test('a constraint over a field a workflow step assigns waits for the server', () => {
  const stepped = rules({ constraints: [{ id: 'dated', rule: rule('is_null(new.status)'), message: 'No' }] });
  stepped.workflows[0].transitions[0] = { name: 'publish', from: 'draft', to: 'published', guard: null, server_decides: false, sets: ['status'] };
  const verdict = checkRecord(stepped, { ...complete, status: 'published' }, { old: complete });
  assert.deepEqual(verdict.failed, []);
  assert.ok(verdict.undecided.some((u) => u.rule === 'constraint:post.dated'));
});

test('the draft of an update is the stored record under the fields the request sends', () => {
  const stored = { title: 'Old', slug: 'old', body: 'b', excerpt: 'e' };
  assert.deepEqual(draftOf({ op: 'update', fields: { title: 'New', excerpt: null } }, stored), { title: 'New', slug: 'old', body: 'b' });
  assert.deepEqual(draftOf({ op: 'create', fields: { title: 'New' } }, stored), { title: 'New' });
});

test('a refusal the page did not foresee is a disagreement, one it left to the server is not', () => {
  const verdict = checkRecord(rules(), complete, {});
  assert.equal(disagreement(verdict, { failed: [{ rule: 'unique:post.slug', detail: 'record 3 already has this value' }] }), null);
  assert.equal(disagreement(verdict, { failed: [{ rule: 'reference:post.author', detail: 'gone' }] }), null);
  assert.match(disagreement(verdict, { failed: [{ rule: 'constraint:post.title_length', detail: 'No [rule]' }] }), /constraint:post\.title_length/);
});
