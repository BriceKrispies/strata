// Tests for scripts/gate-exemptions.mjs. Run with: node --test scripts/gate-exemptions.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applyExemptions, auditVerdict } from './gate-exemptions.mjs';

const groups = [{ reason: 'Known gap (WG-1).', items: ['a', 'b'] }];

test('misses are split into exempt and remaining, and an exemption no miss matches is stale', () => {
  assert.deepEqual(applyExemptions(['a', 'c'], groups), { remaining: ['c'], exempt: ['a'], stale: ['b'], problems: [] });
});

test('a group without a reason or items, and an item listed twice, are problems of the list', () => {
  const bad = [{ reason: ' ', items: ['a'] }, { reason: 'r' }, { reason: 'r', items: ['a'] }];
  assert.deepEqual(applyExemptions(['a'], bad).problems, ['exemption group 0 has no reason', 'exemption group 1 has no items', 'exemption a is listed twice']);
});

test('a check passes only with no unexempt miss, no stale exemption and a sound list', () => {
  assert.equal(auditVerdict('x', 4, ['a', 'b'], groups).ok, true);
  const missing = auditVerdict('x', 4, ['a', 'b', 'c'], groups);
  assert.equal(missing.ok, false);
  assert.deepEqual(missing.lines, ['missing: c']);
  assert.equal(missing.detail, '1 of 4 ok, 2 exempt, 1 missing, 0 stale exemptions');
  const stale = auditVerdict('x', 4, ['a'], groups);
  assert.equal(stale.ok, false);
  assert.deepEqual(stale.lines, ['stale exemption (it matches nothing; delete it): b']);
});
