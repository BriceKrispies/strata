// Tests for scripts/gate-scope-report.mjs, on a small fixture repository. Run with: node --test scripts/gate-scope-report.test.mjs
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { buildScopeReport, formatScopeReport, refPrefix } from './gate-scope-report.mjs';

const here = dirname(fileURLToPath(import.meta.url));

const claim = (id, ref, milestone, area, section) => ({ id, kind: 'requirement', ref, milestone, area, section, statement: `${ref} holds.`, realisation: 'checked' });

/** A repository with two gate scopes, a proposed one, a passing test for LDG-001, and a tests file that names LDG-. */
function fixture({ results = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'gate-scope-'));
  const write = (path, text) => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), text);
  };
  write('specifications/spec.jsonc', JSON.stringify({
    claims: [
      claim('l-1', 'LDG-001', 11, '10-product', 'Accounts'),
      claim('l-2', 'LDG-002', 11, '10-product', 'Accounts'),
      claim('l-3', 'LDG-003', 13, '10-product', 'Postings'),
      claim('a-1', 'ADM-1088', 2, '09-administration', 'Roles'),
      claim('k-1', 'KRN-001', 1, '01-core', 'Decisions'),
    ],
  }));
  write('gate.json', JSON.stringify({
    startedMilestones: [1],
    startedScopes: [{ name: 'ledger', prefixes: ['LDG-'], excludeMilestones: [13] }],
  }));
  write('docs/plinth/gate-scopes.json', JSON.stringify({ scopes: [{ name: 'security', refs: ['ADM-1088'] }] }));
  write('crates/ledger/tests/ledger.rs', '/// Verifies: LDG-001\n#[test]\nfn posts() {}\n');
  const resultsDir = join(root, 'var/test-results');
  mkdirSync(resultsDir, { recursive: true });
  if (results) {
    writeFileSync(join(resultsDir, 'cargo-test.txt'), [
      'running 1 tests',
      'test posts ... ok',
      '',
      'test result: ok. 1 passed; 0 failed',
      '   Running tests\\ledger.rs (target\\debug\\deps\\ledger-0123456789abcdef.exe)',
    ].join('\n'));
  }
  return { root, resultsDir };
}

test('refPrefix keeps the letters and the dash of a ref', () => {
  assert.equal(refPrefix('LDG-001'), 'LDG-');
  assert.equal(refPrefix('A-B-12.3'), 'A-B-');
  assert.equal(refPrefix('ADM-1088'), 'ADM-');
});

test('each scope reports matched, proven and unproven, and its unproven refs grouped by area and section with the tests that name the prefix', () => {
  const { root, resultsDir } = fixture();
  const report = buildScopeReport({ root, resultsDir });
  assert.equal(report.resultsRecorded, true);
  const [ledger, security] = report.scopes;
  assert.deepEqual([ledger.name, ledger.origin, ledger.matched, ledger.proven, ledger.unproven], ['ledger', 'started', 2, 1, 1]);
  assert.equal(ledger.groups.length, 1, 'the groups are built from the unproven refs only');
  assert.deepEqual([ledger.groups[0].area, ledger.groups[0].section], ['10-product', 'Accounts']);
  assert.deepEqual(ledger.groups[0].refs.map((r) => r.ref), ['LDG-002']);
  assert.deepEqual(ledger.groups[0].prefixes, { 'LDG-': ['crates/ledger/tests/ledger.rs'] });
  assert.deepEqual([security.name, security.origin, security.matched, security.proven, security.unproven], ['security', 'proposed', 1, 0, 1]);
  assert.deepEqual(security.groups[0].prefixes, { 'ADM-': [] }, 'no test names the prefix yet');
  assert.deepEqual(security.problems, []);
});

test('without recorded results, nothing is proven and the matched sets are still printed', () => {
  const { root, resultsDir } = fixture({ results: false });
  const report = buildScopeReport({ root, resultsDir, only: 'ledger' });
  assert.equal(report.resultsRecorded, false);
  assert.equal(report.scopes.length, 1);
  assert.deepEqual([report.scopes[0].matched, report.scopes[0].proven, report.scopes[0].unproven], [2, 0, 2]);
  const lines = formatScopeReport(report);
  assert.match(lines[0], /^no test results in .*: nothing is proven/);
  assert.ok(lines.includes('scope ledger (started): 2 matched, 0 proven, 2 unproven'));
});

test('a scope that the report cannot read is shown with its problems, and --scope picks one scope', () => {
  const { root, resultsDir } = fixture();
  const bad = join(root, 'docs/plinth/gate-scopes.json');
  writeFileSync(bad, JSON.stringify({ scopes: [{ name: 'security', prefixes: ['NOPE-'], owner: 'x' }] }));
  const report = buildScopeReport({ root, resultsDir, only: 'security' });
  assert.equal(report.scopes.length, 1);
  assert.match(report.scopes[0].problems[0], /unknown key "owner"/);
  assert.match(report.scopes[0].problems[1], /prefix "NOPE-" matches no requirement/);
  assert.equal(report.scopes[0].matched, 0);
  assert.ok(formatScopeReport(report).some((l) => l.includes('scope problem: scope security: unknown key "owner"')));
});

test('the command line prints the report, and --json gives the same data', () => {
  const { root, resultsDir } = fixture();
  const script = resolve(here, 'gate-scope-report.mjs');
  const json = spawnSync(process.execPath, [script, '--root', root, '--results-dir', resultsDir, '--json', '--scope', 'ledger'], { encoding: 'utf8' });
  assert.equal(json.status, 0, json.stderr);
  const parsed = JSON.parse(json.stdout);
  assert.deepEqual(parsed.scopes.map((s) => [s.name, s.matched, s.proven, s.unproven]), [['ledger', 2, 1, 1]]);
  const text = spawnSync(process.execPath, [script, '--root', root, '--results-dir', resultsDir], { encoding: 'utf8' });
  assert.equal(text.status, 0, text.stderr);
  assert.match(text.stdout, /scope ledger \(started\): 2 matched, 1 proven, 1 unproven/);
  const missing = spawnSync(process.execPath, [script, '--root', root, '--results-dir', resultsDir, '--scope', 'nope'], { encoding: 'utf8' });
  assert.equal(missing.status, 1);
});
