// Browser end-to-end tests. The stacks build the backend, the BFF and the frontend bundle they run
// against (scripts/dev.mjs without --no-build), so a run never tests binaries older than the sources.
// Two stacks are started on ports of their own, each with a fresh
// database, so the tests never touch a development database or a running `npm run dev`.
//
//   site  backend 4201, BFF 4200 serving the built frontend (what a deployment looks like)
//   oidc  a mock OpenID Connect provider on 4290, which the site stack trusts for Fieldnotes
//   saml  a mock SAML provider on 4291, which the site stack trusts for Fieldnotes too
//   hmr   backend 4211, BFF 4210, Vite 5183 (the development loop, for the hot-reload test), served
//         from a throwaway copy of web/ in var/hmr-web, because that test edits source files
//
// Both stacks start the binaries that `node scripts/dev.mjs` built (`--no-build`), which it builds
// with the backend's `development` feature. A backend built without it refuses to start with
// STRATA_DEV=1, so a stale build fails at start-up with that reason.
import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';

const root = fileURLToPath(new URL('../..', import.meta.url)).replace(/[\\/]$/, '');

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
    // A local SAML provider, which the site stack is configured to trust for Fieldnotes.
    {
      command: 'node tests/e2e/mock-saml.mjs 4291',
      cwd: root,
      url: 'http://127.0.0.1:4291/metadata',
      reuseExistingServer: false,
      timeout: 30_000,
    },
    {
      command: 'node scripts/dev.mjs --no-vite --fresh --quiet --db var/e2e.db --tenant-files var/e2e-tenants --backend-port 4201 --bff-port 4200',
      cwd: root,
      url: 'http://127.0.0.1:4200/api/session',
      reuseExistingServer: false,
      // The first start builds the binaries and the bundle.
      timeout: 900_000,
      env: {
        STRATA_OIDC_CONFIG: `${root}/tests/e2e/oidc.config.json`,
        STRATA_E2E_OIDC_SECRET: 'e2e-secret',
        STRATA_SAML_CONFIG: `${root}/tests/e2e/saml.config.json`,
        // Every test signs in from this one address; the per-source limit is for strangers, and
        // the per-login lockout, which a test checks, is a limit of its own.
        STRATA_LOGINS_PER_SOURCE: '10000',
        STRATA_SOCKETS_PER_SOURCE: '10000',
      },
    },
    {
      command: 'node tests/e2e/hmr-stack.mjs --quiet --store memory --backend-port 4211 --bff-port 4210 --vite-port 5183',
      cwd: root,
      url: 'http://127.0.0.1:5183/api/session',
      reuseExistingServer: false,
      timeout: 120_000,
    },
  ],
});
