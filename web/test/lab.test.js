// The Impact Lab asks for a projection at the revision it showed, and says so when the site has
// moved on (AGT-422). Pure kernel functions, no browser.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { STALE } from '../src/kernel/lab.js';
import { initialState, update } from '../src/kernel/update.js';

const stamp = { at: 1_000, rand: 0.5 };
const values = { change: 'require post.excerpt', existing: 'grandfather', source: 'generated', steps: '60', seed: '1' };
const reply = (state, effect, body, ok, status) => update(state, { type: 'http_result', id: effect.id, ok, status, body, ...stamp });

function withOverview(revision) {
  const state = initialState();
  return { ...state, lab: { ...state.lab, overview: { presets: [], tenant: { revision } } } };
}

const submitted = (state) => update(state, { type: 'submit', form: 'lab', values, ...stamp });

test('a projection is asked for at the revision the page showed, and only while the site is still there', () => {
  const asked = submitted(withOverview(7));
  const call = asked.effects.find((e) => e.type === 'http' && e.path === '/api/lab/run');
  assert.ok(call, 'the run is requested');
  assert.equal(call.body.revision, 7);
  assert.equal(call.body.require_current, true);
});

test('a page that has not read the site sends no revision', () => {
  const asked = submitted(initialState());
  const call = asked.effects.find((e) => e.type === 'http' && e.path === '/api/lab/run');
  assert.ok(call);
  assert.equal('revision' in call.body, false);
  assert.equal('require_current' in call.body, false);
});

test('a conflict says the site changed, asks to project again, and reads the site afresh', () => {
  const asked = submitted(withOverview(7));
  const call = asked.effects.find((e) => e.type === 'http' && e.path === '/api/lab/run');
  const refused = reply(asked.state, call, { code: 'conflict', message: 'The site is at revision 9, not 7.' }, false, 409);
  assert.equal(STALE, 'The site changed since this projection: project again.');
  assert.equal(refused.state.lab.formError, STALE);
  assert.equal(refused.state.lab.run, null, 'no run is left waiting');
  assert.ok(
    refused.effects.some((e) => e.type === 'http' && e.path === '/api/lab/overview'),
    'the overview is read again so the next projection names the new revision',
  );
});

test('another refusal keeps the backend wording', () => {
  const asked = submitted(withOverview(7));
  const call = asked.effects.find((e) => e.type === 'http' && e.path === '/api/lab/run');
  const refused = reply(asked.state, call, { code: 'invalid', message: 'post has no field nonsense' }, false, 422);
  assert.equal(refused.state.lab.formError, 'post has no field nonsense');
});
