// Sign-in at a real identity provider. Keycloak runs in the container stack started with STRATA_IDP=1
// (deploy/compose.local-idp.yaml, deploy/keycloak/), and the journeys go through its own pages: the
// browser leaves the site, signs in at Keycloak and comes back. Nothing here is a mock. Run them with
// `npm run idp:containers`, which starts a fresh stack and then this project.
//
// The realm files name the people. Realm `strata` has olive (OpenID Connect), sana (SAML) and cora
// (whose client asks for consent, so that she can cancel); realm `upstream` has una, whom realm
// `strata` offers as "upstream". Their passwords are strata-dev-<name>, development values.
import { expect, test } from '@playwright/test';

const BASE = (process.env.STRATA_E2E_BASE_URL ?? 'http://127.0.0.1:4100').replace(/\/$/, '');
const KEYCLOAK = 'http://keycloak.localhost:8180';

async function ready(page) {
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
}

async function chooseProvider(page, label) {
  await page.goto('/login');
  await ready(page);
  await page.getByRole('button', { name: `Sign in with ${label}`, exact: true }).click();
  // The browser leaves the site for the provider.
  await page.waitForURL((url) => url.origin === KEYCLOAK);
}

/** The form of a Keycloak login page, whichever realm it belongs to. */
async function keycloakLogin(page, name, password = `strata-dev-${name}`) {
  await page.locator('#username').fill(name);
  await page.locator('#password').fill(password);
  await page.locator('#kc-login').click();
}

async function landedSignedIn(page, displayName) {
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Messages' }).getByText(`Signed in as ${displayName}`)).toBeVisible();
  // What the provider sent is not left in the address bar.
  expect(page.url().startsWith(BASE)).toBe(true);
  expect(page.url()).not.toContain('code=');
  expect(page.url()).not.toContain('SAMLResponse');
  expect(page.url()).not.toContain('#');
}

async function session(page) {
  return (await (await page.request.get('/api/session')).json()).session;
}

test('signing in through Keycloak over OpenID Connect makes the person once and lands signed in', async ({ page }) => {
  await chooseProvider(page, 'Keycloak OIDC');
  await expect(page).toHaveURL(/\/realms\/strata\/protocol\/openid-connect\/auth/);
  await keycloakLogin(page, 'olive');
  await landedSignedIn(page, 'Olive Oidc');

  // The principal is the one the site made for this provider's subject, with the one default role.
  const who = await session(page);
  expect(who.authenticated).toBe(true);
  expect(who.principal).toMatch(/^user:oidc-[0-9a-f]{24}$/);
  expect(who.roles).toContain('author');
  expect(who.roles).not.toContain('owner');
});

test('signing in through Keycloak over SAML lands signed in with a signed assertion', async ({ page }) => {
  await chooseProvider(page, 'Keycloak SAML');
  await expect(page).toHaveURL(/\/realms\/strata\/protocol\/saml/);
  await keycloakLogin(page, 'sana');
  // The SAML name attribute is Keycloak's first name.
  await landedSignedIn(page, 'Sana');

  const who = await session(page);
  expect(who.authenticated).toBe(true);
  expect(who.principal).toMatch(/^user:saml-[0-9a-f]{24}$/);
  expect(who.roles).toContain('author');
  expect(who.roles).not.toContain('owner');
});

test('signing in through the upstream realm that Keycloak brokers lands signed in', async ({ page }) => {
  await chooseProvider(page, 'Keycloak OIDC');
  // Realm strata offers the upstream realm beside its own password form.
  await page.locator('#social-upstream').click();
  await page.waitForURL(/\/realms\/upstream\//);
  await keycloakLogin(page, 'una');
  // Una has a complete profile and her email is trusted, so Keycloak asks nothing more and the
  // browser is back at the site: Strata trusts Keycloak, and Keycloak vouches for upstream.
  await landedSignedIn(page, 'Una Upstream');

  const who = await session(page);
  expect(who.authenticated).toBe(true);
  expect(who.principal).toMatch(/^user:oidc-[0-9a-f]{24}$/);
  expect(who.roles).toContain('author');
});

test('cancelling at Keycloak lands back signed out with the error the site gives', async ({ page }) => {
  await chooseProvider(page, 'Keycloak with consent');
  await keycloakLogin(page, 'cora');
  // The client of this provider asks for consent; the person says no.
  await page.locator('#kc-cancel').click();
  await page.waitForURL((url) => url.origin === BASE);
  await ready(page);

  await expect(page.getByRole('alert')).toContainText('did not sign you in');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
  expect(page.url()).not.toContain('error=');
  expect((await session(page)).authenticated).toBe(false);
});
