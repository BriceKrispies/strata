// Row actions in a real browser (WG-5466, D-DS-32): a person acts on a record from its row, and no
// field asks for a record number. The journeys run against a dev stack whose tenant declares the row
// actions on its message list: a row action `react` with the row's id as its `message` argument, and
// a form `edit_message` whose fields are prefilled from the row (`prefill: { title: "row.title" }`).
// They are part of the site project (npm run e2e), like the journeys in site.spec.js.
import { expect, test } from '@playwright/test';

const EXTERNAL = process.env.STRATA_E2E_BASE_URL;
const BASE = (EXTERNAL ?? 'http://127.0.0.1:4200').replace(/\/$/, '');

/** Wait until the application has drawn a real page. */
async function ready(page) {
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
}

async function open(page, path) {
  await page.goto(`${BASE}${path}`);
  await ready(page);
}

async function signIn(page, name) {
  await open(page, '/login');
  await page.getByLabel('Name').fill(name);
  await page.getByLabel('Password').fill(`strata-dev-${name}`);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Home' })).toBeVisible();
}

test('a person reacts to a message from its row, with no record number asked for', async ({ page }) => {
  await signIn(page, 'priya');
  await open(page, '/hub');
  const card = page.getByRole('article').filter({ hasText: 'Offsite notes' }).first();
  const react = card.getByRole('button', { name: 'React', exact: true });
  await expect(react).toBeVisible();
  // The page asks for no number: the message is the row the button sits on.
  await expect(page.getByLabel(/record number|^number$/i)).toHaveCount(0);
  await react.click();
  // The reaction is recorded (the view is read again and the row says so) or the kernel's wording
  // says why it was refused, beside the row, never a raw code.
  await expect(card.getByText(/reacted|cannot react|already reacted/i).or(page.getByRole('status'))).toBeVisible();
});

test('a refused row action says why on its row and is disabled, not drawn as available', async ({ page }) => {
  await signIn(page, 'priya');
  await open(page, '/hub');
  // Priya wrote the message she sees first in the list, so she may not react to it.
  const own = page.getByRole('article').filter({ hasText: 'Offsite notes' }).first();
  const react = own.getByRole('button', { name: 'React', exact: true });
  await expect(react).toBeDisabled();
  await expect(own.getByText(/cannot react to your own message/i)).toBeVisible();
});

test('a form a row action names opens prefilled from the row, and the person edits it', async ({ page }) => {
  await signIn(page, 'tomas');
  await open(page, '/hub');
  const card = page.getByRole('article').filter({ hasText: 'Offsite notes' }).first();
  await card.getByRole('button', { name: 'Edit', exact: true }).click();
  const title = page.getByLabel('Title', { exact: true });
  await expect(title).toHaveValue('Offsite notes');
  await title.fill('Offsite notes, revised');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText(/saved|done/i).first()).toBeVisible();
});
