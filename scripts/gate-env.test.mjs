// Tests for scripts/gate-env.mjs. Run with: node --test scripts/gate-env.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { envVarsRead, undeclaredEnvVars } from './gate-env.mjs';

const MAIN = `
//! | \`STRATA_DOC_ONLY\` | 1 | named in a doc comment, not read |
let addr = var("STRATA_BACKEND_ADDR", "127.0.0.1:4101");
let n = number(
    "STRATA_LAB_QUEUE",
    8,
);
let again = var("STRATA_BACKEND_ADDR", "");
let message = "STRATA_SMTP_URL: failed";
`;

test('the variables read are the whole string literals, once each', () => {
  assert.deepEqual(envVarsRead([{ path: 'crates/backend/src/main.rs', text: MAIN }]).map((v) => v.name), [
    'STRATA_BACKEND_ADDR',
    'STRATA_LAB_QUEUE',
  ]);
});

test('a variable the specification does not name is reported', () => {
  const read = envVarsRead([{ path: 'main.rs', text: MAIN }]);
  const specText = 'The backend SHALL read STRATA_BACKEND_ADDR and STRATA_LAB_QUEUE_EXTRA and no others.';
  assert.deepEqual(undeclaredEnvVars(read, specText), ['STRATA_LAB_QUEUE']);
  assert.deepEqual(undeclaredEnvVars(read, `${specText} STRATA_LAB_QUEUE.`), []);
});
