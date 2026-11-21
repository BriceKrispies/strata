// Strata in containers, with podman. The compose file is generated from the specification
// (`specc deploy`, see deploy/); the images are built from deploy/Containerfile.
//
//   node scripts/containers.mjs build            build the images (npm run containers:build)
//   node scripts/containers.mjs up [--fresh]     start the stack and wait until it answers
//   node scripts/containers.mjs down [--volumes] stop it; --volumes also deletes the database
//   node scripts/containers.mjs e2e [-- args]    fresh stack, then the site tests against it
//
// The service token is generated on every `up` and handed to the containers through the
// environment of this process; it is never written to disk. STRATA_PORT (default 4100) is the
// host port of the gateway, the only port the stack publishes. STRATA_DEV is off unless set to 1;
// only `e2e` and `idp` set it, because development logins and fixture endpoints must never be on in a
// stack anyone else can reach, and the images carry no development accounts.
//
// STRATA_AWS=1 selects the local AWS profile: the runtime keeps its data in a PostgreSQL container,
// and MiniStack emulates the AWS APIs (deploy/compose.local-aws.yaml).
//
// STRATA_IDP=1 selects the local identity-provider profile: Keycloak runs beside the stack as a real
// OpenID Connect, SAML and brokering provider, and the runtime trusts it (deploy/compose.local-idp.yaml,
// deploy/keycloak/). The runtime reaches a provider on this machine only through the outbound gate's
// development exception, so STRATA_IDP=1 implies STRATA_DEV=1 and this script sets it.
//   node scripts/containers.mjs idp [-- args]    fresh stack with Keycloak, then the idp tests against it
//
// The compose provider is `podman compose`, or `podman-compose` when that is missing; set
// STRATA_COMPOSE to a command line to override.

import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const file = 'deploy/compose.yaml';
const [command, ...rest] = process.argv.slice(2);
const flag = (name) => rest.includes(`--${name}`);
const port = process.env.STRATA_PORT ?? '4100';
const keycloakPort = 8180;

if (process.env.STRATA_IDP === '1' && process.env.STRATA_DEV !== '1') {
  console.log('STRATA_IDP=1 implies STRATA_DEV=1: the runtime may reach a provider on this machine only in development mode. Setting STRATA_DEV=1.');
  process.env.STRATA_DEV = '1';
}

function run(program, args, env = {}) {
  const done = spawnSync(program, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  if (done.error) {
    console.error(`${program}: ${done.error.message}`);
    process.exit(1);
  }
  return done.status ?? 1;
}

function composeCommand() {
  if (process.env.STRATA_COMPOSE) return process.env.STRATA_COMPOSE.split(/\s+/);
  const probe = spawnSync('podman', ['compose', 'version'], { cwd: root, stdio: 'ignore' });
  return probe.status === 0 ? ['podman', 'compose'] : ['podman-compose'];
}

// The e2e stack adds tests/e2e/compose.e2e.yaml (development accounts mounted read-only). The
// local AWS profile (STRATA_AWS=1) adds deploy/compose.local-aws.yaml: PostgreSQL for the store and
// MiniStack for the AWS APIs.
const files = () => [
  file,
  ...(process.env.STRATA_DEV === '1' ? ['tests/e2e/compose.e2e.yaml'] : []),
  ...(process.env.STRATA_AWS === '1' ? ['deploy/compose.local-aws.yaml'] : []),
  ...(process.env.STRATA_IDP === '1' ? ['deploy/compose.local-idp.yaml'] : []),
];

function compose(args, env) {
  const [program, ...head] = composeCommand();
  return run(program, [...head, ...files().flatMap((f) => ['-f', f]), ...args], env);
}

async function waitForStack() {
  const url = `http://127.0.0.1:${port}/api/session`;
  for (let attempt = 0; attempt < 240; attempt += 1) {
    try {
      if ((await fetch(url)).ok) return true;
    } catch {
      // Not listening yet.
    }
    await new Promise((done) => setTimeout(done, 500));
  }
  return false;
}

// Keycloak answers once its realm import is done. It is asked at the loopback address; with
// KC_HOSTNAME set it names itself keycloak.localhost whatever address it is asked at.
async function waitForKeycloak() {
  const url = `http://127.0.0.1:${keycloakPort}/realms/strata/.well-known/openid-configuration`;
  for (let attempt = 0; attempt < 480; attempt += 1) {
    try {
      if ((await fetch(url)).ok) return true;
    } catch {
      // Not listening yet.
    }
    await new Promise((done) => setTimeout(done, 500));
  }
  return false;
}

// The images the generated compose file names, by Containerfile target. Built with `podman build`
// directly: podman-compose on Windows loses the `dockerfile` path of a build block. The docker
// image format keeps the HEALTHCHECK of the Containerfile.
const IMAGES = { runtime: 'strata/runtime:dev', gateway: 'strata/gateway:dev' };

function build() {
  for (const [target, tag] of Object.entries(IMAGES)) {
    // A stack with development logins needs a backend built with the development feature.
    const features = process.env.STRATA_DEV === '1' ? ['--build-arg', 'CARGO_FEATURES=strata-backend/development'] : [];
    const status = run('podman', ['build', '--format', 'docker', '-f', 'deploy/Containerfile', '--target', target, ...features, '-t', tag, '.']);
    if (status !== 0) process.exit(status);
  }
}

function down(volumes) {
  // The token only has to be set; the stack is stopped whatever it was.
  return compose(['down', ...(volumes ? ['--volumes'] : [])], { STRATA_SERVICE_TOKEN: 'unused-by-down-unused-by-down' });
}

async function up(fresh) {
  if (fresh) down(true);
  const token = process.env.STRATA_SERVICE_TOKEN ?? randomBytes(32).toString('hex');
  const status = compose(['up', '-d', '--no-build'], { STRATA_SERVICE_TOKEN: token, STRATA_DEV: process.env.STRATA_DEV ?? '0' });
  if (status !== 0) process.exit(status);
  if (process.env.STRATA_IDP === '1' && !(await waitForKeycloak())) {
    console.error(`Keycloak did not answer at http://127.0.0.1:${keycloakPort}/realms/strata. See: podman logs strata-keycloak`);
    process.exit(1);
  }
  if (!(await waitForStack())) {
    console.error(`The stack did not answer at http://127.0.0.1:${port}/api/session. See: podman logs strata-gateway strata-runtime`);
    process.exit(1);
  }
  if (process.env.STRATA_IDP === '1') {
    console.log(`
Keycloak is the identity provider (development values only).

  Keycloak       http://keycloak.localhost:${keycloakPort}/          admin / strata-dev-keycloak-admin
  Realm strata   olive, sana, cora (password strata-dev-<name>); sign in with upstream: una / strata-dev-una
  Logs           podman logs strata-keycloak
`);
  }
  console.log(`
Strata is running in containers.

  Fieldnotes     http://127.0.0.1:${port}/
  Second tenant  http://tidewater.localhost:${port}/
  Store          ${process.env.STRATA_AWS === '1' ? 'PostgreSQL (strata-postgres), AWS APIs on MiniStack (strata-ministack)' : 'SQLite in the strata-data volume'}
  Logs           podman logs strata-runtime ; podman logs strata-gateway
  Stop           npm run containers:down      (add -- --volumes to delete the database)
`);
}

switch (command) {
  case 'build':
    build();
    break;
  case 'up':
    await up(flag('fresh'));
    break;
  case 'down':
    process.exit(down(flag('volumes')));
    break;
  case 'e2e': {
    // A fresh database each time: the tests assume a clean start. The tests sign in with the
    // development accounts, so only this throwaway stack turns them on.
    process.env.STRATA_DEV = '1';
    await up(true);
    const forwarded = rest.filter((arg) => arg !== '--');
    const status = run(process.execPath, [resolve(root, 'node_modules/@playwright/test/cli.js'), 'test', '-c', 'tests/e2e/playwright.config.js', '--project', 'site', ...forwarded], {
      STRATA_E2E_BASE_URL: `http://127.0.0.1:${port}`,
      STRATA_E2E_BACKEND_URL: 'http://127.0.0.1:4101',
    });
    process.exit(status);
    break;
  }
  case 'idp': {
    // A fresh stack with Keycloak, then the identity-provider journeys against it: no mock servers.
    process.env.STRATA_IDP = '1';
    process.env.STRATA_DEV = '1';
    await up(true);
    const forwarded = rest.filter((arg) => arg !== '--');
    const status = run(process.execPath, [resolve(root, 'node_modules/@playwright/test/cli.js'), 'test', '-c', 'tests/e2e/playwright.config.js', '--project', 'idp', ...forwarded], {
      STRATA_E2E_BASE_URL: `http://127.0.0.1:${port}`,
    });
    process.exit(status);
    break;
  }
  default:
    console.error('usage: node scripts/containers.mjs build | up [--fresh] | down [--volumes] | e2e [-- playwright args] | idp [-- playwright args]');
    process.exit(2);
}
