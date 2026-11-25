// Tests for scripts/gate-specc.mjs and the asynchronous runner of scripts/gate-run.mjs. Run with:
// node --test scripts/gate-specc.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { splitPasses } from './gate-specc.mjs';
import { startCommand } from './gate-run.mjs';

const whole = {
  ok: false,
  code: 1,
  error: null,
  signal: null,
  timedOut: false,
  stdout: [
    'specc-pass check begin', 'crates/model is up to date', 'specc-pass check end 0',
    'specc-pass web begin', 'differs: a.js', 'web is not what specc generate would write', 'specc-pass web end 1',
  ].join('\n'),
  stderr: ['specc-pass check begin', 'specc-pass check end 0', 'specc-pass web begin', 'specc: note', 'specc-pass web end 1'].join('\n'),
};

test('each pass gets its own output, in the order a separate run prints it (standard output, then standard error), and its own result', () => {
  const r = splitPasses(whole, ['check', 'web']);
  assert.equal(r.get('check').ok, true);
  assert.equal(r.get('check').out, 'crates/model is up to date\n');
  assert.equal(r.get('web').ok, false);
  assert.equal(r.get('web').code, 1);
  assert.equal(r.get('web').stdout, 'differs: a.js\nweb is not what specc generate would write\n');
  assert.equal(r.get('web').out, 'differs: a.js\nweb is not what specc generate would write\nspecc: note\n');
});

test('a pass that never reported fails with the reason of the whole run', () => {
  const r = splitPasses({ ...whole, error: 'timed out after 5s', timedOut: true }, ['check', 'lint']);
  assert.equal(r.get('check').ok, true);
  assert.equal(r.get('lint').ok, false);
  assert.equal(r.get('lint').timedOut, true);
  assert.match(r.get('lint').error, /timed out/);
  const crashed = splitPasses({ ...whole, stdout: '', stderr: 'error: could not compile' }, ['check']);
  assert.equal(crashed.get('check').ok, false);
  assert.match(crashed.get('check').error, /before pass check reported/);
});

test('a started command is awaited for its result, apart from standard error, and a timeout kills it', async () => {
  const options = (timeoutMs) => ({ cwd: import.meta.dirname, env: process.env, timeoutMs });
  const done = await startCommand(process.execPath, ['-e', 'console.log("out"); console.error("err"); process.exit(0)'], options(30_000));
  assert.equal(done.ok, true);
  assert.equal(done.stdout.trim(), 'out');
  assert.equal(done.stderr.trim(), 'err');
  const failed = await startCommand(process.execPath, ['-e', 'process.exit(3)'], options(30_000));
  assert.equal(failed.ok, false);
  assert.equal(failed.code, 3);
  const slow = await startCommand(process.execPath, ['-e', 'setTimeout(() => {}, 120000)'], options(500));
  assert.equal(slow.ok, false);
  assert.equal(slow.timedOut, true);
  assert.match(slow.error, /timed out/);
  const missing = await startCommand('strata-no-such-tool-0123', [], options(30_000));
  assert.equal(missing.ok, false);
  assert.ok(missing.error);
});
