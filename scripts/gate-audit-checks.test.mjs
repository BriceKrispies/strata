// Tests for scripts/gate-audit-checks.mjs: the five checks on a small checkout that fails each one.
// Run with: node --test scripts/gate-audit-checks.test.mjs
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, test } from 'node:test';
import { auditChecks } from './gate-audit-checks.mjs';

const made = [];

function checkout(files) {
  const dir = mkdtempSync(join(tmpdir(), 'gate-audit-'));
  made.push(dir);
  for (const [rel, text] of Object.entries(files)) {
    const path = join(dir, rel);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
  }
  return dir;
}

after(() => {
  for (const dir of made) rmSync(dir, { recursive: true, force: true });
});

const FILES = {
  'crates/bff/src/routes.rs': 'pub fn router() -> Router { Router::new().route("/api/login", post(login)).route("/api/secret", get(secret)) }',
  'crates/bff/src/main.rs': 'let a = var("STRATA_BFF_ADDR", ""); let b = var("STRATA_HIDDEN", "");',
  'crates/backend/src/main.rs': 'let a = var("STRATA_DB", "");',
  'crates/runtime/src/server.rs': '',
  'crates/backend/src/auth.rs': '/// Implements: H-003\nfn start() {}',
  'crates/storage-sqlite/src/lib.rs': 'CREATE TABLE sessions (id TEXT); CREATE TABLE secrets (id TEXT);',
  'crates/runtime/src/sqlite.rs': '',
  'crates/model/src/lib.rs': '/// Implements: CT-777 (generated code is never a handler)',
  'specifications/real.md': '# real',
};

const SPEC = {
  contracts: [
    { id: 'issue-token', ref: 'H-003', deployment: 'shipped' },
    { id: 'orphan', ref: 'CT-777', deployment: 'shipped' },
    { id: 'sign-in', deployment: 'shipped', bffRoute: 'POST /api/login' },
    { id: 'later', deployment: 'target' },
  ],
  functions: [],
  records: [{ id: 'tbl-sessions', realisation: 'shipped' }],
  claims: [{ statement: 'Reads STRATA_BFF_ADDR and STRATA_DB. See specifications/real.md and specifications/gone.md.' }],
};

function run(files, spec) {
  const verdicts = auditChecks({ root: checkout(files), spec });
  assert.equal(verdicts.length, 5);
  return Object.fromEntries(verdicts.map((v, i) => [['handlers', 'tables', 'env', 'bffRoutes', 'paths'][i], v]));
}

test('each check fails on its own miss and reports a count', () => {
  const v = run(FILES, SPEC);
  assert.deepEqual(v.handlers.lines, ['missing: orphan']);
  assert.match(v.handlers.detail, /^2 of 3 ok/);
  assert.deepEqual(v.tables.lines, ['missing: secrets']);
  assert.deepEqual(v.env.lines, ['missing: STRATA_HIDDEN']);
  assert.deepEqual(v.bffRoutes.lines, ['missing: GET /api/secret']);
  assert.deepEqual(v.paths.lines, ['missing: specifications/gone.md']);
  for (const verdict of Object.values(v)) assert.equal(verdict.ok, false);
});

test('a check with no miss passes', () => {
  const files = { ...FILES, 'crates/bff/src/main.rs': 'let a = var("STRATA_BFF_ADDR", "");', 'crates/storage-sqlite/src/lib.rs': 'CREATE TABLE sessions (id TEXT);' };
  const spec = {
    ...SPEC,
    contracts: [{ id: 'issue-token', ref: 'H-003', deployment: 'shipped' }, { id: 'sign-in', deployment: 'shipped', bffRoute: 'POST /api/login' }, { id: 'secret', deployment: 'shipped', bffRoute: 'GET /api/secret', ref: 'H-003' }],
    claims: [{ statement: 'Reads STRATA_BFF_ADDR and STRATA_DB. See specifications/real.md.' }],
  };
  for (const verdict of Object.values(run(files, spec))) assert.deepEqual(verdict.lines, [], verdict.name);
});

test('a checkout without the files a check reads fails the check', () => {
  const v = run({ 'specifications/real.md': '' }, SPEC);
  assert.equal(v.handlers.ok, false);
  assert.match(v.handlers.detail, /cannot read crates\/bff\/src\/routes\.rs/);
  assert.equal(v.tables.ok, false);
  assert.equal(v.env.ok, false);
  assert.equal(v.bffRoutes.ok, false);
});
