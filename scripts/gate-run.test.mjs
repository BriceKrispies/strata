// Tests for scripts/gate-run.mjs and for what scripts/spec-gate.mjs runs through it. Run with:
// node --test scripts/gate-run.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runCommand } from './gate-run.mjs';

const here = import.meta.dirname;
const node = process.execPath;
const options = (timeoutMs) => ({ cwd: here, env: process.env, timeoutMs });

test('a command that exits 0 passes and its output is kept', () => {
  const r = runCommand(node, ['-e', 'console.log("out"); console.error("err")'], options(30_000));
  assert.equal(r.ok, true);
  assert.equal(r.code, 0);
  assert.match(r.out, /out/);
  assert.match(r.out, /err/);
});

test('a command that exits non-zero fails', () => {
  const r = runCommand(node, ['-e', 'process.exit(3)'], options(30_000));
  assert.equal(r.ok, false);
  assert.equal(r.code, 3);
  assert.equal(r.timedOut, false);
});

test('a command that runs past its timeout is killed and fails, naming the timeout', () => {
  const started = Date.now();
  const r = runCommand(node, ['-e', 'setTimeout(() => {}, 120000)'], options(500));
  assert.equal(r.ok, false);
  assert.equal(r.timedOut, true);
  assert.match(r.error, /timed out/);
  assert.ok(Date.now() - started < 60_000, 'the child was not waited for');
});

test('a command that does not start fails', () => {
  const r = runCommand('strata-no-such-tool-0123', [], options(30_000));
  assert.equal(r.ok, false);
  assert.ok(r.error, 'the reason is reported');
});

const gate = readFileSync(resolve(here, 'spec-gate.mjs'), 'utf8');

test('the gate starts every child through the one runner that has a timeout', () => {
  assert.doesNotMatch(gate, /\b(spawnSync|spawn|execSync|execFileSync|exec)\(/, 'a child started outside run()');
  assert.match(gate, /runCommand\(command, commandArgs, \{[\s\S]*?timeoutMs: options\.timeoutMs \?\? DEFAULT_TIMEOUT_MS/);
});

test('the gate runs clippy, the format check, both audits, the client tests and the generated journeys', () => {
  assert.match(gate, /'clippy'[^\n]*'-D', 'warnings'/);
  assert.match(gate, /'fmt', '--check'/);
  assert.match(gate, /run\('cargo', \['audit'\]/);
  assert.match(gate, /run\('npm', \['audit', '--audit-level=high'\]/);
  assert.match(gate, /nodeTestFiles = \[[^\]]*web\/test\/client\.test\.js/);
  assert.match(gate, /tests\/e2e\/generated\/playwright\.generated\.config\.js/);
});

test('a tool that is not installed is counted as not run, and --require-all fails the gate for it', () => {
  assert.match(gate, /function skipCheck\(name, reason\) \{\s*notRun\.push\(name\)/);
  assert.match(gate, /requireAll/);
});
