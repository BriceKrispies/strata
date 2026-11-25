// The Settings page (WG-5601): what the page says of each setting, what a save asks for, and what
// a refused save says. Pure kernel functions, no browser.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { controlOf, parseValue, plainOf, refusalWords, settingsInit, settingsPage, settingsUpdate, valueWords } from '../src/kernel/settings.js';
import { initialState, update } from '../src/kernel/update.js';

const stamp = { at: 1_000, rand: 0.5 };
const reply = (state, effect, body, ok, status) => update(state, { type: 'http_result', id: effect.id, ok, status, body, ...stamp });

// The definitions as the route answers them: values in the tagged form, the revisions a write names.
const ENTRIES = [
  {
    id: 'reading_width', label: 'Reading width', description: 'How wide the text is.', area: 'publication', type: 'string',
    allowed: { closed: { values: [{ Str: 'narrow' }, { Str: 'standard' }, { Str: 'wide' }] } },
    value: { Str: 'standard' }, source: 'default', team: null, locked: false, writable_levels: ['tenant', 'person'],
    tenant_revision: 0, person_revision: 3,
  },
  {
    id: 'digest_hour', label: 'Digest hour', area: null, type: 'int', allowed: { range: { min: 0, max: 23 } },
    value: { Int: 8 }, source: 'tenant', team: null, locked: false, writable_levels: ['person'],
    tenant_revision: 2, person_revision: 0,
  },
  {
    id: 'member_names', label: 'Who sees member names', area: null, type: 'string',
    allowed: { closed: { values: [{ Str: 'members' }, { Str: 'everyone' }, { Str: 'nobody' }] } },
    value: { Str: 'members' }, source: 'platform', team: null, locked: true, writable_levels: [],
    tenant_revision: 0, person_revision: 0,
  },
  {
    id: 'compact_lists', label: 'Compact lists', area: 'publication', type: 'bool', allowed: null,
    value: { Bool: true }, source: 'person', team: null, locked: false, writable_levels: ['person'],
    tenant_revision: 0, person_revision: 1,
  },
];

function withSettings(list) {
  const state = initialState();
  return { ...state, settings: { ...settingsInit(), list } };
}

const rowOf = (page, id) => page.areas.flatMap((area) => area.settings).find((row) => row.id === id);

test('the page groups the settings by product area, with the general ones last, in words', () => {
  const page = settingsPage(withSettings(ENTRIES));
  assert.equal(page.status, 'ready');
  assert.deepEqual(page.areas.map((area) => area.label), ['Publication', 'General']);
  assert.deepEqual(page.areas[0].settings.map((row) => row.id), ['reading_width', 'compact_lists']);
});

test('a setting says its value and where the value comes from', () => {
  const page = settingsPage(withSettings(ENTRIES));
  const width = rowOf(page, 'reading_width');
  assert.equal(width.value, 'Standard');
  assert.equal(width.source, 'The default');
  assert.deepEqual(width.levels.map((level) => level.label), ['Organisation', 'Your choice']);
  assert.equal(rowOf(page, 'digest_hour').source, 'Set by your organisation');
  assert.equal(rowOf(page, 'compact_lists').source, 'Your choice');
  assert.equal(rowOf(page, 'compact_lists').value, 'On');
});

test('a locked setting is read only and says why, and a level the viewer may not change has no control', () => {
  const page = settingsPage(withSettings(ENTRIES));
  const locked = rowOf(page, 'member_names');
  assert.deepEqual(locked.levels, []);
  assert.equal(locked.locked, true);
  assert.match(locked.readOnly, /Your organisation has locked this setting/);
  const hour = rowOf(page, 'digest_hour');
  assert.deepEqual(hour.levels.map((level) => level.level), ['person']);
});

test('the controls follow the type and the allowed values', () => {
  assert.deepEqual(controlOf(ENTRIES[0]).options.map((o) => o.label), ['Narrow', 'Standard', 'Wide']);
  assert.deepEqual(controlOf(ENTRIES[1]), { kind: 'number', min: 0, max: 23 });
  assert.equal(controlOf(ENTRIES[3]).kind, 'switch');
  assert.equal(controlOf({ type: 'string', allowed: { reference: { object: 'x__channel' } } }).kind, 'text');
});

test('a tagged value reads as a plain one, and a refused value is said before any request', () => {
  assert.equal(plainOf({ Str: 'wide' }), 'wide');
  assert.equal(valueWords(ENTRIES[0], controlOf(ENTRIES[0])), 'Standard');
  assert.deepEqual(parseValue(ENTRIES[1], controlOf(ENTRIES[1]), 'x'), { ok: false, words: 'Enter a whole number.' });
  assert.deepEqual(parseValue(ENTRIES[1], controlOf(ENTRIES[1]), '30'), { ok: false, words: 'Enter a number from 0 to 23.' });
  assert.deepEqual(parseValue(ENTRIES[1], controlOf(ENTRIES[1]), '7'), { ok: true, value: 7 });
});

test('a person saves through the person contract, naming the revision the page read', () => {
  const state = withSettings(ENTRIES);
  const saved = update(state, { type: 'submit', form: 'settings:person:reading_width', values: { value: 'wide' }, ...stamp });
  const call = saved.effects.find((e) => e.type === 'http');
  assert.equal(call.method, 'POST');
  assert.equal(call.path, '/api/settings/set-person');
  assert.deepEqual(call.body, { setting: 'reading_width', value: 'wide', expected_version: 3 });
  assert.equal(saved.state.settings.busy.reading_width, 'person');
});

test('the organisation saves through the tenant contract, and only where the viewer may write it', () => {
  const state = withSettings(ENTRIES);
  const saved = update(state, { type: 'submit', form: 'settings:tenant:reading_width', values: { value: 'narrow' }, ...stamp });
  const call = saved.effects.find((e) => e.type === 'http');
  assert.equal(call.path, '/api/settings/set-tenant');
  assert.deepEqual(call.body, { setting: 'reading_width', value: 'narrow', expected_version: 0 });
  const refused = update(state, { type: 'submit', form: 'settings:tenant:digest_hour', values: { value: '5' }, ...stamp });
  assert.equal(refused.effects.length, 0, 'a level the viewer may not change sends nothing');
});

test('a refused save says why in words, and the page reads the setting again', () => {
  const state = withSettings(ENTRIES);
  const saved = update(state, { type: 'submit', form: 'settings:person:reading_width', values: { value: 'wide' }, ...stamp });
  const call = saved.effects.find((e) => e.type === 'http');
  const refused = reply(saved.state, call, { code: 'conflict', message: 'The setting is at revision 4.' }, false, 409);
  const page = settingsPage(refused.state);
  assert.match(rowOf(page, 'reading_width').refusal, /changed somewhere else/);
  assert.equal(refused.state.settings.busy.reading_width, undefined);
  assert.ok(refused.effects.some((e) => e.type === 'http' && e.path === '/api/settings/definitions'));
});

test('an answered save says it saved and reads the definitions again', () => {
  const state = withSettings(ENTRIES);
  const saved = update(state, { type: 'submit', form: 'settings:person:reading_width', values: { value: 'wide' }, ...stamp });
  const call = saved.effects.find((e) => e.type === 'http');
  const done = reply(saved.state, call, { revision: 4, event_seq: 9 }, true, 200);
  assert.ok(done.state.toasts.some((t) => t.text === 'Saved.'));
  assert.ok(done.effects.some((e) => e.type === 'http' && e.path === '/api/settings/definitions'));
});

test('a refusal is said in words for each code, and an unknown one keeps its own message', () => {
  assert.match(refusalWords({ code: 'level_not_allowed' }), /cannot be changed at this level/);
  assert.match(refusalWords({ code: 'not_found' }), /no longer exists/);
  assert.equal(refusalWords({ code: 'odd', message: 'Plainly said.' }), 'Plainly said.');
  assert.equal(refusalWords({}), 'That did not work. Try again.');
});

test('a definitions reply fills the page, and a failed one says so', () => {
  const io = { call: (s, kind, contract) => [s, { type: 'http', kind, contract }], toast: (s) => s };
  const state = initialState();
  const filled = settingsUpdate(state, { pending: { kind: 'settings.definitions' }, error: null, outputs: { settings: ENTRIES } }, io);
  assert.equal(filled.state.settings.list.length, 4);
  const failed = settingsUpdate(state, { pending: { kind: 'settings.definitions' }, error: { message: 'The site could not be reached.' } }, io);
  assert.equal(settingsPage(failed.state).status, 'error');
});
