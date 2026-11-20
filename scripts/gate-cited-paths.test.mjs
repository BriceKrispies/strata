// Tests for scripts/gate-cited-paths.mjs. Run with: node --test scripts/gate-cited-paths.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { citedPaths, missingPaths } from './gate-cited-paths.mjs';

const SPEC = {
  claims: [
    { statement: 'Built in crates/backend/src/auth.rs:120 and crates/bff/src/routes.rs, not in crates/*/src or web/src/<name>.js.' },
    { statement: 'See specifications/00-vision/whitepaper.md.\nAnd tests/e2e/site.spec.js. The Cargo.toml too, but not xcrates/a.rs.' },
  ],
  nested: { list: ['scripts/spec-gate.mjs', 7, null] },
};

test('the cited paths are found in every string, without line numbers, globs or trailing dots', () => {
  assert.deepEqual(citedPaths(SPEC), [
    'crates/backend/src/auth.rs',
    'crates/bff/src/routes.rs',
    // The directory before a template placeholder.
    'web/src',
    'specifications/00-vision/whitepaper.md',
    'tests/e2e/site.spec.js',
    'Cargo.toml',
    'scripts/spec-gate.mjs',
  ]);
});

test('a cited path that does not exist is reported', () => {
  const exists = (path) => path !== 'specifications/00-vision/whitepaper.md';
  assert.deepEqual(missingPaths(citedPaths(SPEC), exists), ['specifications/00-vision/whitepaper.md']);
});
