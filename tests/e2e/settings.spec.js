// The Settings page in a real browser (WG-5601): a signed-in person changes a setting they may
// change, and a second page of the same person shows the new value without a reload. Not yet
// registered in playwright.config.js: it runs against the dev stack once that stack is confirmed to
// hold the publication template's reading_width setting for the dev tenants.
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
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
}

/** A marker that only survives if the document is never reloaded. */
async function mark(page) {
  await page.evaluate(() => {
    window.__settingsMark = 'kept';
  });
}

/** The card of the reading width setting, which says its value in its list of facts. */
function widthCard(page) {
  return page.locator('#setting-reading_width');
}

test('a signed-in person changes a setting they may change, and their other open page follows without a reload', async ({ page, browser }) => {
  await signIn(page, 'ines');
  await open(page, '/settings');
  await expect(page.getByRole('heading', { level: 1, name: 'Settings' })).toBeVisible();
  await mark(page);

  const other = await (await browser.newContext()).newPage();
  await signIn(other, 'ines');
  await open(other, '/settings');
  await mark(other);

  // The person's own level is a control; the value and its source are said in words.
  const choice = page.locator('form[data-form="settings:person:reading_width"]');
  await expect(widthCard(page).locator('dt', { hasText: 'Where it comes from' })).toBeVisible();
  await choice.getByLabel('Reading width').selectOption({ label: 'Wide' });
  await choice.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Saved.')).toBeVisible();
  await expect(widthCard(page).locator('dd', { hasText: /^Wide$/ })).toBeVisible();
  await expect(widthCard(page).locator('dd', { hasText: /^Your choice$/ })).toBeVisible();

  // The other page of the same person is read again by the setting's notice, with no reload.
  await expect(widthCard(other).locator('dd', { hasText: /^Wide$/ })).toBeVisible();
  expect(await other.evaluate(() => window.__settingsMark)).toBe('kept');

  // Put it back, so the next run starts from the default.
  await choice.getByLabel('Reading width').selectOption({ label: 'Standard' });
  await choice.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(widthCard(page).locator('dd', { hasText: /^Standard$/ })).toBeVisible();
  await other.context().close();
});
