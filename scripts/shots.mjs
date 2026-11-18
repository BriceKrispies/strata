// Capture screenshots of a running Strata for visual review.
//
//   node scripts/shots.mjs [base-url] [name-filter]
//
// Writes PNG files to var/shots/. The stack must already be running (see scripts/dev.mjs).

import { chromium } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const base = process.argv[2] ?? 'http://127.0.0.1:4100';
const only = process.argv[3] ?? '';
const out = join(root, 'var', 'shots');
mkdirSync(out, { recursive: true });

const desktop = { width: 1360, height: 900 };
const phone = { width: 390, height: 844 };

/** @type {{ name: string, path: string, viewport: any, scheme?: 'dark' | 'light', login?: string, full?: boolean, prepare?: (page: any) => Promise<void> }[]} */
const shots = [
  { name: 'home-desktop', path: '/', viewport: desktop, full: true },
  { name: 'home-phone', path: '/', viewport: phone, full: true },
  { name: 'home-dark', path: '/', viewport: desktop, scheme: 'dark' },
  { name: 'post-desktop', path: '/posts/the-jig-is-the-work', viewport: desktop, full: true },
  { name: 'post-phone', path: '/posts/what-the-kiln-decides', viewport: phone },
  { name: 'post-dark-phone', path: '/posts/a-margin-is-a-promise', viewport: phone, scheme: 'dark' },
  { name: 'topics', path: '/topics', viewport: desktop },
  { name: 'about', path: '/about', viewport: desktop, full: true },
  { name: 'search', path: '/search?q=kiln', viewport: desktop },
  { name: 'login', path: '/login', viewport: desktop },
  { name: 'desk', path: '/desk', viewport: desktop, login: 'ines' },
  { name: 'desk-phone', path: '/desk', viewport: phone, login: 'ines' },
  { name: 'editor', path: '/desk/edit/notes-toward-a-workbench', viewport: desktop, login: 'tomasz', full: true },
  { name: 'settings', path: '/desk/settings', viewport: desktop, login: 'ines' },
  { name: 'lab', path: '/lab', viewport: desktop, login: 'admin', full: true },
  {
    name: 'lab-result',
    path: '/lab',
    viewport: desktop,
    login: 'admin',
    full: true,
    prepare: async (page) => {
      await page.getByRole('button', { name: 'Run the projection' }).click();
      await page.getByRole('heading', { name: 'What would change' }).waitFor({ timeout: 60_000 });
    },
  },
  { name: 'lab-phone', path: '/lab', viewport: phone, login: 'admin' },
];

const browser = await chromium.launch();
for (const shot of shots.filter((s) => s.name.includes(only))) {
  const context = await browser.newContext({ viewport: shot.viewport, colorScheme: shot.scheme ?? 'light', deviceScaleFactor: 1 });
  const page = await context.newPage();
  if (shot.login) {
    await page.goto(`${base}/login`);
    await page.getByLabel('Name').fill(shot.login);
    await page.getByLabel('Password').fill(`strata-dev-${shot.login}`);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.getByRole('heading', { name: 'Writing desk' }).waitFor();
  }
  await page.goto(`${base}${shot.path}`);
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
  await page.evaluate(() => document.fonts.ready);
  if (shot.prepare) await shot.prepare(page);
  await page.waitForTimeout(700);
  await page.screenshot({ path: join(out, `${shot.name}.png`), fullPage: shot.full ?? false });
  console.log(`captured ${shot.name}`);
  await context.close();
}
await browser.close();
