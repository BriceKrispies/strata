// Tests for scripts/gate-bff-routes.mjs. Run with: node --test scripts/gate-bff-routes.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mountedRoutes, parseBffRoute, patternMatches, routesWithoutContract } from './gate-bff-routes.mjs';

const SOURCE = `
async fn session() {}
// .route("/not-a-route", get(nothing))
pub fn router(bff: Shared) -> Router {
    let api = Router::new()
        .route("/session", get(session))
        .route("/members/:op", post(members))
        .route("/identity/*action", get(identity_read).post(identity_write))
        .fallback(api_not_found);
    Router::new()
        .nest("/api", api)
        .route("/ws", get(socket))
        .route("/scim/v2/*rest", any(scim))
        .nest_service("/assets", ServeDir::new(bff.cfg.dist.join("assets")))
        .fallback(get(page))
        .with_state(bff)
}
`;

test('the router lists every mounted route with its nesting prefix and methods', () => {
  const found = mountedRoutes(SOURCE).map((r) => `${r.method} ${r.path}`);
  assert.deepEqual(found, [
    'GET /api/session',
    'POST /api/members/:op',
    'GET /api/identity/*action',
    'POST /api/identity/*action',
    'GET /ws',
    'ANY /scim/v2/*rest',
  ]);
});

test('a source with no router mounts nothing', () => {
  assert.deepEqual(mountedRoutes('fn other() {}'), []);
});

test('a bffRoute reads as a method and a path', () => {
  assert.deepEqual(parseBffRoute('POST /api/records records create post'), { method: 'POST', path: '/api/records' });
  assert.equal(parseBffRoute('records'), null);
  assert.equal(parseBffRoute(undefined), null);
});

test('a pattern matches the paths its parameters stand for', () => {
  assert.ok(patternMatches('/api/members/:op', '/api/members/assign-role'));
  assert.ok(patternMatches('/scim/v2/*rest', '/scim/v2/Users/7'));
  assert.ok(!patternMatches('/api/members/:op', '/api/members'));
  assert.ok(!patternMatches('/api/members/:op', '/api/members/a/b'));
  assert.ok(!patternMatches('/api/session', '/api/login'));
});

test('a mounted route with no contract that names it is reported', () => {
  const mounted = mountedRoutes(SOURCE);
  const contracts = [
    { id: 'read-session-view', bffRoute: 'GET /api/session' },
    { id: 'assign-role', bffRoute: 'POST /api/members/assign-role' },
  ];
  assert.deepEqual(routesWithoutContract(mounted, contracts), [
    'GET /api/identity/*action',
    'POST /api/identity/*action',
    'GET /ws',
    'ANY /scim/v2/*rest',
  ]);
  // A contract of another method does not serve the route.
  assert.deepEqual(routesWithoutContract(mounted.slice(0, 1), [{ id: 'x', bffRoute: 'POST /api/session' }]), ['GET /api/session']);
  // A route mounted for any method is served by a contract of one method.
  assert.equal(routesWithoutContract(mounted, [{ id: 'any', bffRoute: 'GET /scim/v2/Users' }]).includes('ANY /scim/v2/*rest'), false);
});
