// The development loop: Vite in front, the BFF behind it, the same worker and WebSocket path.
import { expect, test } from '@playwright/test';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// The test edits files, so it edits the throwaway copy that tests/e2e/hmr-stack.mjs serves, never web/src.
const web = fileURLToPath(new URL('../../var/hmr-web/src/', import.meta.url));

async function ready(page) {
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
}

test('under Vite the page still gets its data and live connection through the BFF', async ({ page }) => {
  const urls = [];
  page.on('request', (request) => urls.push(new URL(request.url())));
  await page.goto('/');
  await ready(page);
  await expect(page.getByRole('heading', { level: 2, name: 'The Jig Is the Work' })).toBeVisible();
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');
  // Everything the browser asked for went to the Vite origin; Vite passed /api and /ws on.
  expect(urls.every((url) => url.port === '5183')).toBe(true);
  expect(urls.some((url) => url.pathname === '/api/view')).toBe(true);
});

test('editing a stylesheet restyles the open page without reloading it', async ({ page }) => {
  await page.goto('/');
  await ready(page);
  await page.evaluate(() => {
    window.__notReloaded = true;
  });
  const file = `${web}styles/components.css`;
  const original = readFileSync(file, 'utf8');
  const spacing = () => page.evaluate(() => getComputedStyle(document.querySelector('.masthead__name')).letterSpacing);
  const before = await spacing();
  try {
    writeFileSync(file, `${original}\n@layer components { .masthead__name { letter-spacing: 0.5em; } }\n`);
    await expect.poll(spacing, { timeout: 15_000 }).not.toBe(before);
    expect(await page.evaluate(() => window.__notReloaded === true)).toBe(true);
  } finally {
    writeFileSync(file, original);
  }
  await expect.poll(spacing, { timeout: 15_000 }).toBe(before);
});

test('editing a view module updates the browser by itself', async ({ page }) => {
  await page.goto('/');
  await ready(page);
  await expect(page.getByRole('heading', { name: 'Earlier essays' })).toBeVisible();
  const file = `${web}views/pages.js`;
  const original = readFileSync(file, 'utf8');
  try {
    writeFileSync(file, original.replace("'Earlier essays'", "'Essays from before'"));
    await expect(page.getByRole('heading', { name: 'Essays from before' })).toBeVisible({ timeout: 15_000 });
  } finally {
    writeFileSync(file, original);
  }
  await expect(page.getByRole('heading', { name: 'Earlier essays' })).toBeVisible({ timeout: 15_000 });
});
