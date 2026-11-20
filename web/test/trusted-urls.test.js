// The worker-address policy: what the Trusted Types policy lets reach `new Worker`.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { WORKER_POLICY_NAME, acceptWorkerUrl, createWorkerPolicy } from '../src/trusted-urls.js';

const ORIGIN = 'https://tidewater.example';

test('an address under /assets/ on the page origin is accepted', () => {
  assert.equal(acceptWorkerUrl('/assets/main-DdxQ3KJs.js', ORIGIN, false), `${ORIGIN}/assets/main-DdxQ3KJs.js`);
  assert.equal(acceptWorkerUrl(`${ORIGIN}/assets/main.js`, ORIGIN, false), `${ORIGIN}/assets/main.js`);
});

test('another origin, data:, blob:, javascript: and a path outside /assets/ are refused', () => {
  for (const value of [
    'https://evil.example/assets/main.js',
    'http://tidewater.example/assets/main.js',
    '//evil.example/assets/main.js',
    'data:text/javascript,postMessage(1)',
    'blob:https://tidewater.example/0b3f',
    'javascript:alert(1)',
    '/api/x.js',
    '/src/worker/main.js',
    '/assets/../api/x.js',
    '/assets/%2e%2e/api/x.js',
    'https://user:pass@tidewater.example/assets/main.js',
    '',
  ]) {
    assert.equal(acceptWorkerUrl(value, ORIGIN, false), null, value);
  }
});

test('the development server path is accepted only in development', () => {
  assert.equal(acceptWorkerUrl('/src/worker/main.js?worker_file&type=module', ORIGIN, true), `${ORIGIN}/src/worker/main.js?worker_file&type=module`);
  assert.equal(acceptWorkerUrl('/src/worker/main.js', ORIGIN, false), null);
});

test('the policy has the name the content security policy lists and throws on a refused address', () => {
  /** @type {any[]} */
  const made = [];
  const factory = { createPolicy: (name, rules) => (made.push(name), rules) };
  const policy = createWorkerPolicy(factory, ORIGIN, false);
  assert.deepEqual(made, [WORKER_POLICY_NAME]);
  assert.equal(WORKER_POLICY_NAME, 'strata-worker');
  assert.equal(policy.createScriptURL('/assets/a.js'), `${ORIGIN}/assets/a.js`);
  assert.throws(() => policy.createScriptURL('blob:https://tidewater.example/1'), TypeError);
  assert.throws(() => policy.createScriptURL('data:text/javascript,1'), TypeError);
  assert.throws(() => policy.createScriptURL('https://evil.example/assets/a.js'), TypeError);
});
