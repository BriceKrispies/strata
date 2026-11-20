// Runs the step fixtures (tests/e2e/step-checks, generated from ./spec.jsonc) against the pages of
// ./server.mjs. The violates-* journeys are generated with test.fail(): each passes only when the
// generated assertion it ends with fails on the page that violates it.
//
//   npm run e2e:steps
import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';

const root = fileURLToPath(new URL('../../..', import.meta.url)).replace(/[\\/]$/, '');

export default defineConfig({
  testDir: '../step-checks',
  testMatch: '*.spec.js',
  outputDir: '../test-results/step-checks',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 30_000,
  expect: { timeout: 2_000 },
  reporter: [['list']],
  use: { ...devices['Desktop Chrome'], baseURL: 'http://127.0.0.1:4300', actionTimeout: 2_000, viewport: { width: 1280, height: 860 } },
  webServer: {
    command: 'node tests/e2e/step-fixtures/server.mjs 4300',
    cwd: root,
    url: 'http://127.0.0.1:4300/p/basic',
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
