// Browser end-to-end tests. Two stacks are started on ports of their own, each with a fresh
// database, so the tests never touch a development database or a running `npm run dev`.
//
//   site  backend 4201, BFF 4200 serving the built frontend (what a deployment looks like)
//   oidc  a mock OpenID Connect provider on 4290, which the site stack trusts for Fieldnotes
//   hmr   backend 4211, BFF 4210, Vite 5183 (the development loop, for the hot-reload test)
import { defineConfig, devices } from '@playwright/test';

const root = new URL('../..', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');

export default defineConfig({
  testDir: '.',
  outputDir: './test-results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 45_000,
  expect: { timeout: 8_000 },
  reporter: [['list']],
  use: { baseURL: 'http://127.0.0.1:4200', trace: 'retain-on-failure', ...devices['Desktop Chrome'], viewport: { width: 1280, height: 860 } },
  projects: [
    { name: 'site', testMatch: 'site.spec.js' },
    { name: 'hmr', testMatch: 'hmr.spec.js', use: { baseURL: 'http://127.0.0.1:5183' } },
  ],
  webServer: [
    // A local OpenID Connect provider, which the site stack is configured to trust for Fieldnotes.
    {
      command: 'node tests/e2e/mock-oidc.mjs 4290',
      cwd: root,
      url: 'http://127.0.0.1:4290/.well-known/openid-configuration',
      reuseExistingServer: false,
      timeout: 30_000,
    },
    {
      command: 'node scripts/dev.mjs --no-vite --no-build --fresh --quiet --db var/e2e.db --tenant-files var/e2e-tenants --backend-port 4201 --bff-port 4200',
      cwd: root,
      url: 'http://127.0.0.1:4200/api/session',
      reuseExistingServer: false,
      timeout: 120_000,
      env: { STRATA_OIDC_CONFIG: `${root}/tests/e2e/oidc.config.json`, STRATA_E2E_OIDC_SECRET: 'e2e-secret' },
    },
    {
      command: 'node scripts/dev.mjs --no-build --quiet --store memory --backend-port 4211 --bff-port 4210 --vite-port 5183',
      cwd: root,
      url: 'http://127.0.0.1:5183/api/session',
      reuseExistingServer: false,
      timeout: 120_000,
    },
  ],
});
