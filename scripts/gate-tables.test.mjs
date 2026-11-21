// Tests for scripts/gate-tables.mjs. Run with: node --test scripts/gate-tables.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createdTables, recordNamesTable, tablesWithoutRecord } from './gate-tables.mjs';

const SQL = `
const STEPS: &[&str] = &["
    CREATE TABLE tenant_directory (id TEXT);
    CREATE TABLE IF NOT EXISTS passkeys (id TEXT);
    // CREATE TABLE commented_out (id TEXT);
"];
let sql = "CREATE TABLE IF NOT EXISTS runtime_migrations (step INTEGER)";
`;

test('every CREATE TABLE of the sources is found, comments left out', () => {
  assert.deepEqual(createdTables([{ path: 'crates/storage-sqlite/src/lib.rs', text: SQL }]).map((t) => t.name), [
    'tenant_directory',
    'passkeys',
    'runtime_migrations',
  ]);
});

test('a shipped record names a table by its id or by its description', () => {
  assert.ok(recordNamesTable({ id: 'tbl-tenant-directory', realisation: 'shipped' }, 'tenant_directory'));
  assert.ok(recordNamesTable({ id: 'x', realisation: 'shipped', description: 'Table passkeys of a tenant file: the keys.' }, 'passkeys'));
  assert.ok(!recordNamesTable({ id: 'x', realisation: 'shipped', description: 'The table passkeys_old.' }, 'passkeys'));
  assert.ok(!recordNamesTable({ id: 'tbl-passkeys', realisation: 'target' }, 'passkeys'));
});

test('a table with no shipped record is reported', () => {
  const tables = createdTables([{ path: 'a.rs', text: SQL }]);
  const records = [
    { id: 'tbl-tenant-directory', realisation: 'shipped' },
    { id: 'tbl-passkeys', realisation: 'target', description: 'Table passkeys of a tenant file.' },
  ];
  assert.deepEqual(tablesWithoutRecord(tables, records), { total: 3, missing: ['passkeys', 'runtime_migrations'] });
});
