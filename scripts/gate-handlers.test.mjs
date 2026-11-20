// Tests for scripts/gate-handlers.mjs. Run with: node --test scripts/gate-handlers.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mountedRoutes } from './gate-bff-routes.mjs';
import { implementsMarks, shippedWithoutHandler } from './gate-handlers.mjs';

const sources = [
  { path: 'crates/backend/src/auth.rs', text: '/// Implements: H-003\nfn start_session() {}\n//! Implements: H-034, IDN-006\n' },
  { path: 'crates/backend/src/http.rs', text: 'fn health() {}\nfn put_session() {}\n.route("/identity-service/rotate-keys", post(rotate))' },
];
const mounted = mountedRoutes('fn router() -> Router { Router::new().route("/api/login", post(login)) }');
const functions = [{ id: 'kernel-transition' }];

function missing(contracts) {
  return shippedWithoutHandler({ contracts, functions, sources, mounted });
}

test('Implements marks list references separated by commas', () => {
  assert.deepEqual([...implementsMarks(sources)].sort(), ['H-003', 'H-034', 'IDN-006']);
  assert.deepEqual([...implementsMarks([{ path: 'a.rs', text: 'Implements: not a ref, CT-9' }])], ['CT-9']);
});

test('a shipped contract with each kind of handler is accepted', () => {
  const result = missing([
    { id: 'issue-token', ref: 'H-003', deployment: 'shipped' },
    { id: 'sign-in', deployment: 'shipped', bffRoute: 'POST /api/login' },
    { id: 'rotate-keys', implementedBy: 'identity-service', deployment: 'shipped' },
    { id: 'decide-intent', deployment: 'shipped', derivation: { function: 'kernel-transition' } },
    { id: 'put-session', deployment: 'shipped' },
  ]);
  assert.deepEqual(result, { shipped: 5, missing: [] });
});

test('a shipped contract with no handler is reported', () => {
  const result = missing([
    { id: 'export-form-submissions', ref: 'CT-999', implementedBy: 'gateway', deployment: 'shipped' },
    { id: 'derived-nothing', deployment: 'shipped', derivation: { function: 'no-such-function' } },
    { id: 'sign-out', deployment: 'shipped', bffRoute: 'POST /api/logout' },
  ]);
  assert.deepEqual(result, { shipped: 3, missing: ['export-form-submissions', 'derived-nothing', 'sign-out'] });
});

test('a contract that is not shipped needs no handler', () => {
  assert.deepEqual(missing([{ id: 'fork-tenant', deployment: 'target' }, { id: 'undeclared' }]), { shipped: 0, missing: [] });
});
