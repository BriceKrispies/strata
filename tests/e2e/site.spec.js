// End-to-end tests in a real browser, against the built frontend served by the BFF.
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { mkdirSync } from 'node:fs';

const BASE = 'http://127.0.0.1:4200';
const SHOTS = new URL('./screenshots/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
mkdirSync(SHOTS, { recursive: true });

/** Wait until the application has drawn a real page. */
async function ready(page) {
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
}

async function open(page, path) {
  await page.goto(path);
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
    window.__notReloaded = true;
  });
}

async function stillTheSameDocument(page) {
  expect(await page.evaluate(() => window.__notReloaded === true)).toBe(true);
}

async function writeDraft(page, title, body = 'A first paragraph written by a test.\n\n## A heading\n\nAnd a second one.') {
  await page.getByRole('link', { name: 'Write a new article' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'New article' })).toBeVisible();
  await page.getByLabel('Title').fill(title);
  await page.getByLabel('Text', { exact: true }).fill(body);
  await page.getByRole('button', { name: 'Save draft' }).click();
  await expect(page.locator('.toast', { hasText: 'Draft saved.' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 1, name: 'Edit article' })).toBeVisible();
}

async function axe(page, name) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21aa', 'wcag22aa']).analyze();
  const problems = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length}) ${v.nodes[0]?.target}`);
  expect(problems, `accessibility problems on ${name}`).toEqual([]);
}

test('the front page shows the featured essay and the earlier ones', async ({ page }) => {
  await open(page, '/');
  await expect(page).toHaveTitle('Fieldnotes');
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Fieldnotes');
  await expect(page.getByRole('heading', { level: 2, name: 'The Jig Is the Work' })).toBeVisible();
  const earlier = page.getByRole('region', { name: 'Earlier essays' }).getByRole('article');
  await expect(earlier).toHaveCount(5);
  await expect(earlier.first().getByRole('heading')).toHaveText('The Second Hundred Hours');
  await expect(page.getByText('Notes Toward a Workbench')).toHaveCount(0);
  await expect(page.getByRole('img', { name: /plywood fixture holding a chair leg/ })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-accent', 'moss');
  await page.screenshot({ path: `${SHOTS}home.png`, fullPage: true });
});

test('the first paint comes from the server-rendered bootstrap, without a request for the view', async ({ page }) => {
  const viewRequests = [];
  page.on('request', (request) => request.url().includes('/api/view') && viewRequests.push(request.url()));
  await open(page, '/posts/slow-software');
  await expect(page.getByRole('heading', { level: 1, name: 'Slow Software' })).toBeVisible();
  expect(viewRequests).toEqual([]);
});

test('following a link opens the article without a page load and moves focus to it', async ({ page }) => {
  await open(page, '/');
  await mark(page);
  await page.getByRole('link', { name: 'What the Kiln Decides' }).click();
  await expect(page).toHaveURL(`${BASE}/posts/what-the-kiln-decides`);
  await expect(page.getByRole('heading', { level: 1, name: 'What the Kiln Decides' })).toBeVisible();
  await expect(page).toHaveTitle('What the Kiln Decides — Fieldnotes');
  await expect(page.locator('main')).toBeFocused();
  await expect(page.getByRole('complementary', { name: 'About this essay' }).getByRole('link', { name: 'Mirela Ashby' })).toBeVisible();
  await expect(page.locator('.prose blockquote')).toContainText('You do not finish a pot');
  await stillTheSameDocument(page);
  await page.goBack();
  await expect(page.getByRole('heading', { level: 2, name: 'The Jig Is the Work' })).toBeVisible();
  await stillTheSameDocument(page);
});

test('topics, authors, about and search all work', async ({ page }) => {
  await open(page, '/topics');
  await page.getByRole('link', { name: 'Design' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Design' })).toBeVisible();
  await expect(page.getByRole('article')).toHaveCount(2);

  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Authors' }).click();
  await page.getByRole('link', { name: 'Tomasz Wren' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Tomasz Wren' })).toBeVisible();
  await expect(page.getByRole('article')).toHaveCount(2);

  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'About' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'About Fieldnotes' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Colophon' })).toBeVisible();

  await page.getByRole('searchbox', { name: 'Search the essays' }).first().fill('kiln');
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(`${BASE}/search?q=kiln`);
  await expect(page.getByText('1 essay mentions “kiln”.')).toBeVisible();
  await expect(page.getByRole('article').getByRole('heading')).toHaveText('What the Kiln Decides');
  await open(page, '/search?q=zzzz');
  await expect(page.getByText('Nothing mentions “zzzz”.')).toBeVisible();
});

test('an unknown address says so, with the right status', async ({ page }) => {
  const response = await page.goto('/posts/no-such-essay');
  expect(response.status()).toBe(200);
  await ready(page);
  await expect(page.getByRole('heading', { level: 1, name: 'There is nothing at this address' })).toBeVisible();
  expect((await page.goto('/no/such/route')).status()).toBe(404);
});

test('every application request comes from the worker and goes to the BFF only', async ({ page }) => {
  // Count anything the main thread itself tries to fetch or connect to.
  await page.addInitScript(() => {
    window.__mainThreadCalls = [];
    const fetch0 = window.fetch;
    window.fetch = (...args) => {
      window.__mainThreadCalls.push(`fetch ${args[0]}`);
      return fetch0(...args);
    };
    const WebSocket0 = window.WebSocket;
    window.WebSocket = class extends WebSocket0 {
      constructor(...args) {
        window.__mainThreadCalls.push(`ws ${args[0]}`);
        super(...args);
      }
    };
    const open0 = XMLHttpRequest.prototype.open;
    XMLHttpRequest.prototype.open = function (...args) {
      window.__mainThreadCalls.push(`xhr ${args[1]}`);
      return open0.apply(this, args);
    };
  });
  const urls = [];
  page.on('request', (request) => urls.push(request.url()));
  const sockets = [];
  page.on('websocket', (socket) => sockets.push(socket.url()));
  await open(page, '/');
  await page.getByRole('link', { name: 'Drawing the Thing You Mean' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Drawing the Thing You Mean' })).toBeVisible();
  await page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Topics' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Topics' })).toBeVisible();
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');

  expect(await page.evaluate(() => window.__mainThreadCalls)).toEqual([]);
  expect(page.workers().length).toBe(1);
  expect(urls.filter((url) => url.includes('/api/view')).length).toBeGreaterThanOrEqual(2);
  for (const url of [...urls, ...sockets]) {
    expect(url.startsWith(BASE) || url.startsWith('ws://127.0.0.1:4200/'), url).toBe(true);
  }
  // The backend refuses a browser outright, even one that finds its port.
  const direct = await page.request.post('http://127.0.0.1:4201/internal/view', { data: { path: '/' }, failOnStatusCode: false });
  expect(direct.status()).toBe(401);
});

test('signing in with a wrong password explains itself; signing in and out works', async ({ page }) => {
  await open(page, '/login');
  await page.getByLabel('Name').fill('ines');
  await page.getByLabel('Password').fill('not-the-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('The name or password is not right.');
  await expect(page.getByLabel('Password')).toBeFocused();
  await expect(page.getByLabel('Password')).toHaveValue('');

  await page.getByLabel('Password').fill('strata-dev-ines');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  await expect(page.locator('.toast', { hasText: 'Signed in as Inès Okafor.' })).toBeVisible();
  const cookies = await page.context().cookies();
  const session = cookies.find((cookie) => cookie.name === '__Host-strata_sid');
  expect(session).toMatchObject({ httpOnly: true, secure: true, sameSite: 'Strict', path: '/' });
  expect(await page.evaluate(() => document.cookie)).toBe('');

  // An editor sees every article, including the draft another author wrote.
  await expect(page.getByRole('row', { name: /Notes Toward a Workbench/ })).toContainText('Draft');

  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.locator('.toast', { hasText: 'Signed out.' })).toBeVisible();
  await expect(page.getByRole('banner').getByRole('link', { name: 'Sign in' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 2, name: 'The Jig Is the Work' })).toBeVisible();
  expect((await page.context().cookies()).find((cookie) => cookie.name === '__Host-strata_sid')).toBeUndefined();
  await open(page, '/desk');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in to continue' })).toBeVisible();
});

test('requests that skip the page are refused: no session, no CSRF token, foreign origin', async ({ page, request }) => {
  const change = { op: 'create', object: 'category', fields: { name: 'Forged', slug: 'forged' } };
  const anonymous = await request.post('/api/records', { data: change, headers: { origin: BASE } });
  expect(anonymous.status()).toBe(401);

  await signIn(page, 'ines');
  // The browser's own cookie jar is used here, so the session cookie is sent.
  const withoutToken = await page.request.post('/api/records', { data: change, headers: { origin: BASE } });
  expect(withoutToken.status()).toBe(403);
  const foreignOrigin = await page.request.post('/api/records', { data: change, headers: { origin: 'http://evil.example', 'x-csrf-token': 'x' } });
  expect(foreignOrigin.status()).toBe(403);
  await open(page, '/topics');
  await expect(page.getByText('Forged')).toHaveCount(0);
});

test('an author writes a draft, sees validation from the kernel, publishes, and readers see it live', async ({ page, browser }) => {
  const reader = await (await browser.newContext()).newPage();
  await open(reader, '/');
  await mark(reader);
  await expect(reader.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');

  await signIn(page, 'tomasz');
  await writeDraft(page, 'On Sharpening');
  await expect(page).toHaveURL(`${BASE}/desk/edit/on-sharpening`);
  await expect(page.getByLabel('Address')).toHaveValue('on-sharpening');

  // A draft is invisible to readers: not on the page, and not at its address.
  await expect(reader.getByText('On Sharpening')).toHaveCount(0);
  const hidden = await (await browser.newContext()).newPage();
  await open(hidden, '/posts/on-sharpening');
  await expect(hidden.getByRole('heading', { level: 1, name: 'There is nothing at this address' })).toBeVisible();
  await hidden.context().close();

  // The kernel's constraints come back as field errors.
  await page.getByLabel('Address').fill('Not A Slug');
  await page.getByLabel('Excerpt').fill('x'.repeat(300));
  await page.getByRole('button', { name: 'Save draft' }).click();
  await expect(page.getByRole('alert').first()).toHaveText('2 fields need attention before this can be saved.');
  await expect(page.getByLabel('Address')).toHaveAttribute('aria-invalid', 'true');
  await expect(page.getByLabel('Address')).toBeFocused();
  await expect(page.locator('#editor-slug-error')).toHaveText('A slug uses lower-case letters, digits and single hyphens, at most 80 characters.');
  await expect(page.locator('#editor-excerpt-error')).toHaveText('An excerpt has at most 280 characters.');
  await page.screenshot({ path: `${SHOTS}editor-errors.png`, fullPage: true });

  await page.getByLabel('Address').fill('on-sharpening');
  await page.getByLabel('Excerpt').fill('A dull edge is a decision somebody put off.');
  // The preview is produced by the server from the same Markdown rules as the published page.
  await page.getByRole('button', { name: 'Preview' }).click();
  await expect(page.locator('.editor__preview h2')).toHaveText('A heading');
  await page.getByRole('button', { name: 'Write', exact: true }).click();
  await page.getByLabel('Text', { exact: true }).focus();
  await page.keyboard.press('Control+End');
  await page.getByRole('button', { name: 'Bold' }).click();
  await expect(page.getByLabel('Text', { exact: true })).toHaveValue(/\*\*bold text\*\*$/);

  await page.getByRole('button', { name: 'Publish' }).click();
  await expect(page.locator('.toast', { hasText: 'Published.' })).toBeVisible();
  await expect(page.locator('.editor__state .status')).toHaveText('Published');

  // The reader's open page updates by itself, with no reload and no navigation.
  await expect(reader.getByRole('heading', { name: 'On Sharpening' })).toBeVisible();
  await expect(reader.getByText('A dull edge is a decision somebody put off.')).toBeVisible();
  await stillTheSameDocument(reader);
  await reader.screenshot({ path: `${SHOTS}home-after-publish.png` });

  await reader.getByRole('link', { name: 'On Sharpening' }).click();
  await expect(reader.locator('.prose strong')).toHaveText('bold text');

  // Unpublishing from the desk withdraws it from the reader's page as they watch.
  await page.getByRole('link', { name: 'Back to the desk' }).click();
  await page.getByRole('button', { name: 'Unpublish On Sharpening' }).click();
  await expect(page.locator('.toast', { hasText: 'Moved back to drafts.' })).toBeVisible();
  await expect(reader.getByRole('heading', { level: 1, name: 'There is nothing at this address' })).toBeVisible();
  await stillTheSameDocument(reader);

  // Deleting takes two presses.
  await page.getByRole('button', { name: 'Delete On Sharpening', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm deleting On Sharpening' }).click();
  await expect(page.locator('.toast', { hasText: 'Deleted.' })).toBeVisible();
  await expect(page.getByRole('row', { name: /On Sharpening/ })).toHaveCount(0);
  await reader.context().close();
});

test('an author cannot edit another author’s article; an editor can', async ({ page }) => {
  await signIn(page, 'mirela');
  await expect(page.getByRole('row', { name: /The Jig Is the Work/ })).toHaveCount(0);
  await expect(page.getByRole('row', { name: /What the Kiln Decides/ })).toBeVisible();
  // Guessing the address of someone else's draft finds nothing.
  await open(page, '/desk/edit/notes-toward-a-workbench');
  await expect(page.getByRole('heading', { level: 1, name: 'There is nothing at this address' })).toBeVisible();
  // A forged write to that draft finds nothing either: it answers exactly as an article that
  // does not exist, so it does not tell her the draft is there.
  const csrf = (await (await page.request.get('/api/session')).json()).session.csrf_token;
  const forge = (id) =>
    page.request.post('/api/records', {
      data: { op: 'update', object: 'post', id, fields: { title: 'Taken over' } },
      headers: { origin: BASE, 'x-csrf-token': csrf },
    });
  const hidden = await forge(15);
  const missing = await forge(9999);
  expect(hidden.status()).toBe(404);
  expect(missing.status()).toBe(404);
  const [hiddenBody, missingBody] = [await hidden.json(), await missing.json()];
  expect({ ...hiddenBody, message: '', failed: [], correlation: '' }).toEqual({ ...missingBody, message: '', failed: [], correlation: '' });
  expect(hiddenBody.failed.map((c) => c.rule)).toEqual(missingBody.failed.map((c) => c.rule));
  // An article she can read but did not write is refused by the kernel's policy, not by the page.
  const forged = await forge(14);
  expect(forged.status()).toBe(403);
  expect((await forged.json()).failed[0].rule).toBe('policy:authors_own_posts');
});

test('an author cannot write under another byline, feature a post or choose when it was published', async ({ page }) => {
  await signIn(page, 'tomasz');
  const csrf = (await (await page.request.get('/api/session')).json()).session.csrf_token;
  const headers = { origin: BASE, 'x-csrf-token': csrf };
  const INES = 2; // author records: Inès is the first, Tomasz the second
  const TOMASZ = 3;
  const OWN_DRAFT = 15; // "Notes Toward a Workbench", Tomasz's own unpublished draft
  const farFuture = Date.UTC(2099, 0, 1);
  const rules = async (response) => (await response.json()).failed.map((check) => check.rule);
  const create = (fields) =>
    page.request.post('/api/records', {
      data: { op: 'create', object: 'post', fields: { title: 'Forged', slug: 'forged-byline', body: 'Text.', author: TOMASZ, ...fields } },
      headers,
    });
  const update = (fields) => page.request.post('/api/records', { data: { op: 'update', object: 'post', id: OWN_DRAFT, fields }, headers });

  // A post under Inès's byline is refused by the policy that grants authors creation.
  const byline = await create({ author: INES });
  expect(byline.ok()).toBe(false);
  expect(await rules(byline)).toContain('policy:authors_write_posts');
  // Featuring, or dating a post in the far future, is for editors.
  const featured = await create({ featured: true });
  expect(featured.ok()).toBe(false);
  expect(await rules(featured)).toContain('constraint:post.featured_by_editors');
  const dated = await create({ published_at: farFuture });
  expect(dated.ok()).toBe(false);
  expect(await rules(dated)).toContain('constraint:post.published_at_by_editors');
  // The same on a post he does own: its byline, its feature and its date stay as they are.
  expect(await rules(await update({ author: INES }))).toContain('constraint:post.author_fixed');
  expect(await rules(await update({ featured: true }))).toContain('constraint:post.featured_by_editors');
  expect(await rules(await update({ published_at: farFuture }))).toContain('constraint:post.published_at_by_editors');

  // Nothing above got through: the front page still shows the editor's feature, and the draft is a draft.
  await open(page, '/');
  await expect(page.getByRole('heading', { level: 2, name: 'The Jig Is the Work' })).toBeVisible();
  await open(page, '/desk');
  await expect(page.getByRole('row', { name: /Notes Toward a Workbench/ })).toContainText('Draft');
});

test('a session that ends is noticed at once and the page falls back to signed out', async ({ page }) => {
  // The session-expiry fixture is the owner's.
  await signIn(page, 'admin');
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');
  await mark(page);
  const csrf = (await (await page.request.get('/api/session')).json()).session.csrf_token;
  const expired = await page.request.post('/api/dev/expire-session', { headers: { origin: BASE, 'x-csrf-token': csrf } });
  expect((await expired.json()).expired).toBe(true);
  await expect(page.locator('.toast', { hasText: 'Your session has expired. Sign in again to keep writing.' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in to continue' })).toBeVisible();
  await stillTheSameDocument(page);
  const write = await page.request.post('/api/records', { data: { op: 'delete', object: 'post', id: 9 }, headers: { origin: BASE, 'x-csrf-token': 'stale' } });
  expect(write.status()).toBe(401);
});

test('after losing the connection the page reconnects and catches up on what it missed', async ({ page, browser }) => {
  await open(page, '/');
  await mark(page);
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');

  // Cut the network, drop the socket, and publish something while the reader is away.
  await page.context().setOffline(true);
  await page.evaluate(() => window.__strataDropSocket());
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'waiting');

  const author = await (await browser.newContext()).newPage();
  await signIn(author, 'mirela');
  await writeDraft(author, 'While You Were Away');
  await author.getByRole('button', { name: 'Publish' }).click();
  await expect(author.locator('.toast', { hasText: 'Published.' })).toBeVisible();
  await expect(page.getByText('While You Were Away')).toHaveCount(0);

  await page.context().setOffline(false);
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open', { timeout: 20_000 });
  await expect(page.getByRole('heading', { name: 'While You Were Away' })).toBeVisible();
  await stillTheSameDocument(page);

  await author.getByRole('button', { name: 'Unpublish' }).click();
  await expect(page.getByRole('heading', { name: 'While You Were Away' })).toHaveCount(0);
  await author.context().close();
});

test('tenant settings are data: changing the accent restyles every open page live', async ({ page, browser }) => {
  const reader = await (await browser.newContext()).newPage();
  await open(reader, '/about');
  await mark(reader);
  const accent = () => reader.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--accent').trim());
  expect(await accent()).toBe('#4f6140');

  await signIn(page, 'ines');
  await page.getByRole('link', { name: 'Publication settings' }).click();
  await page.getByText('Plum', { exact: true }).click();
  await page.getByLabel('Tagline').fill('Essays on making things, and what they teach.');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.locator('.toast', { hasText: 'Settings saved.' })).toBeVisible();

  await expect(reader.locator('html')).toHaveAttribute('data-accent', 'plum');
  expect(await accent()).toBe('#6b3f5b');
  await expect(reader.locator('.colophon')).toContainText('Essays on making things, and what they teach.');
  await stillTheSameDocument(reader);
  await reader.screenshot({ path: `${SHOTS}about-plum.png` });

  // A value outside the token schema is refused by the tenant's own constraint.
  const csrf = (await (await page.request.get('/api/session')).json()).session.csrf_token;
  const injected = await page.request.post('/api/records', {
    data: { op: 'update', object: 'publication', id: 1, fields: { accent: 'red;background:url(//evil)' } },
    headers: { origin: BASE, 'x-csrf-token': csrf },
  });
  expect(injected.status()).toBe(422);
  expect((await injected.json()).failed[0].rule).toBe('constraint:publication.accent_token');

  await page.getByText('Moss', { exact: true }).click();
  await page.getByLabel('Tagline').fill('Essays on making things, and on what making them teaches.');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(reader.locator('html')).toHaveAttribute('data-accent', 'moss');
  await reader.context().close();
});

test('the theme control cycles device, light and dark, and is remembered', async ({ page }) => {
  await page.emulateMedia({ colorScheme: 'dark' });
  await open(page, '/');
  await expect(page.locator('html')).toHaveAttribute('data-scheme', 'dark');
  await page.getByRole('button', { name: /Change theme/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-scheme', 'light');
  await page.getByRole('button', { name: /Change theme/ }).click();
  await expect(page.locator('html')).toHaveAttribute('data-scheme', 'dark');
  await page.emulateMedia({ colorScheme: 'light' });
  await page.reload();
  await ready(page);
  await expect(page.locator('html')).toHaveAttribute('data-scheme', 'dark');
  await page.screenshot({ path: `${SHOTS}home-dark.png` });
  await axe(page, 'the front page in the dark theme');
});

test('the whole site can be used from the keyboard', async ({ page }) => {
  await open(page, '/');
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'Skip to the content' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.locator('main')).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(page.getByRole('link', { name: 'The Jig Is the Work' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { level: 1, name: 'The Jig Is the Work' })).toBeVisible();

  // Sign in and publish nothing, entirely without a pointer.
  await open(page, '/login');
  await page.getByLabel('Name').focus();
  await page.keyboard.type('tomasz');
  await page.keyboard.press('Tab');
  await page.keyboard.type('strata-dev-tomasz');
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  const focusRing = await page.evaluate(() => {
    const link = document.querySelector('.toolbar a');
    link.focus();
    return getComputedStyle(link).outlineStyle;
  });
  expect(focusRing).not.toBe('none');
});

test('a slow render never rewinds what was typed into a field', async ({ page }) => {
  // Hold every message from the kernel worker back, so each draw reaches the page after the
  // keystrokes that came later than the state it shows.
  await page.addInitScript(() => {
    const Native = window.Worker;
    const own = Object.getOwnPropertyDescriptor(Native.prototype, 'onmessage');
    window.Worker = class extends Native {
      constructor(...args) {
        super(...args);
        Object.defineProperty(this, 'onmessage', {
          set: (fn) => own.set.call(this, fn && ((event) => setTimeout(() => fn.call(this, event), 150))),
          get: () => own.get.call(this),
        });
      }
    };
  });
  await open(page, '/login');
  const name = page.getByLabel('Name');
  await name.focus();
  await page.keyboard.type('tomasz');
  // Leave the field, so the focused-field rule no longer protects it, and watch it while the
  // delayed draws of the earlier keystrokes arrive.
  await page.keyboard.press('Tab');
  const seen = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const input = document.getElementById('login-name');
        const values = new Set([input.value]);
        const timer = setInterval(() => values.add(input.value), 2);
        setTimeout(() => {
          clearInterval(timer);
          resolve([...values]);
        }, 600);
      }),
  );
  expect(seen).toEqual(['tomasz']);
  await expect(name).toHaveValue('tomasz');
});

test('on a phone the layout folds, the menu opens and closes, and nothing overflows', async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const page = await context.newPage();
  await open(page, '/');
  const nav = page.getByRole('navigation', { name: 'Sections' });
  await expect(nav).toBeHidden();
  const toggle = page.getByRole('button', { name: 'Menu' });
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await toggle.click();
  await expect(nav).toBeVisible();
  await expect(page.getByRole('button', { name: 'Close menu' })).toHaveAttribute('aria-expanded', 'true');
  await page.keyboard.press('Escape');
  await expect(nav).toBeHidden();
  await page.getByRole('button', { name: 'Menu' }).click();
  await nav.getByRole('link', { name: 'Topics' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Topics' })).toBeVisible();
  await expect(nav).toBeHidden();

  for (const path of ['/', '/posts/a-margin-is-a-promise', '/about', '/login']) {
    await open(page, path);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow, `horizontal overflow on ${path}`).toBeLessThanOrEqual(0);
  }
  // On a phone the notes sit in a line above the title instead of in a margin column.
  await open(page, '/');
  const entry = page.locator('.entry').first();
  const notes = await entry.locator('.notes').boundingBox();
  const title = await entry.locator('.entry__title').boundingBox();
  expect(notes.y + notes.height).toBeLessThanOrEqual(title.y + 1);
  await page.screenshot({ path: `${SHOTS}home-phone.png`, fullPage: true });
  await axe(page, 'the front page on a phone');
  await context.close();
});

test('on a wide screen the notes sit in the margin beside the text', async ({ page }) => {
  await open(page, '/posts/the-second-hundred-hours');
  const margin = await page.locator('.essay__margin').boundingBox();
  const prose = await page.locator('.prose').boundingBox();
  expect(margin.x + margin.width).toBeLessThanOrEqual(prose.x);
  expect(prose.width).toBeLessThanOrEqual(600);
  await page.screenshot({ path: `${SHOTS}article.png`, fullPage: true });
});

test('reader pages pass automated accessibility checks', async ({ page }) => {
  for (const path of ['/', '/posts/the-jig-is-the-work', '/topics', '/authors', '/about', '/search?q=jig', '/login', '/posts/nowhere']) {
    await open(page, path);
    await axe(page, path);
    expect(await page.locator('h1').count(), `one h1 on ${path}`).toBe(1);
  }
});

test('the desk, editor and settings pass automated accessibility checks', async ({ page }) => {
  await signIn(page, 'ines');
  await axe(page, 'the writing desk');
  await page.screenshot({ path: `${SHOTS}desk.png`, fullPage: true });
  await open(page, '/desk/edit/notes-toward-a-workbench');
  await expect(page.getByRole('heading', { level: 1, name: 'Edit article' })).toBeVisible();
  await axe(page, 'the editor');
  await page.screenshot({ path: `${SHOTS}editor.png`, fullPage: true });
  await open(page, '/desk/settings');
  await axe(page, 'publication settings');
  await open(page, '/desk/new');
  await axe(page, 'a new article');
});

test('a second tenant on the same substrate shares nothing with the first', async ({ page, browser }) => {
  const tide = 'http://tidewater.localhost:4200';
  await page.goto(`${tide}/`);
  await ready(page);
  await expect(page).toHaveTitle('Tidewater Review');
  await expect(page.getByRole('heading', { level: 2, name: 'High Water at Six' })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-accent', 'ink');
  await expect(page.locator('html')).toHaveAttribute('data-paper', 'stone');
  await expect(page.getByText('The Jig Is the Work')).toHaveCount(0);
  await page.screenshot({ path: `${SHOTS}tidewater.png` });

  // Fieldnotes logins do not exist there.
  await page.goto(`${tide}/login`);
  await ready(page);
  await page.getByLabel('Name').fill('ines');
  await page.getByLabel('Password').fill('strata-dev-ines');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('The name or password is not right.');

  // A Fieldnotes owner's session, replayed against the Tidewater host, is nobody there.
  const owner = await browser.newContext();
  const ownerPage = await owner.newPage();
  await signIn(ownerPage, 'admin');
  const cookie = (await owner.cookies()).find((c) => c.name === '__Host-strata_sid');
  const stolen = await browser.newContext();
  await stolen.addCookies([{ name: '__Host-strata_sid', value: cookie.value, domain: 'tidewater.localhost', path: '/', httpOnly: true, secure: true, sameSite: 'Strict' }]);
  const thief = await stolen.newPage();
  await thief.goto(`${tide}/desk`);
  await ready(thief);
  await expect(thief.getByRole('heading', { level: 1, name: 'Sign in to continue' })).toBeVisible();
  const seen = await thief.evaluate(async () => {
    const get = async (path) => (await fetch(path)).json();
    return { session: await get('/api/session'), desk: await get('/api/view?path=/desk'), draft: await get('/api/view?path=/posts/tidewater-private-draft') };
  });
  expect(seen.session.session.authenticated).toBe(false);
  expect(seen.desk.data.posts).toEqual([]);
  expect(seen.draft.data.post).toBeNull();

  // The Fieldnotes owner sees every Fieldnotes article and nothing of Tidewater's.
  const everything = await (await ownerPage.request.get('/api/view?path=/desk')).text();
  expect(everything).toContain('Notes Toward a Workbench');
  expect(everything).not.toContain('Tidewater');
  expect(everything).not.toContain('High Water');

  // Events do not cross either: a Tidewater reader's page is untouched by a Fieldnotes change.
  await page.goto(`${tide}/`);
  await ready(page);
  await mark(page);
  await expect(page.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');
  const frames = [];
  page.on('websocket', (socket) => socket.on('framereceived', (frame) => frames.push(String(frame.payload))));
  const viewRequests = [];
  page.on('request', (request) => request.url().includes('/api/view') && viewRequests.push(request.url()));
  await ownerPage.getByRole('link', { name: 'Write a new article' }).click();
  await ownerPage.getByLabel('Title').fill('A Fieldnotes Secret');
  await ownerPage.getByLabel('Text', { exact: true }).fill('Only for Fieldnotes.');
  await ownerPage.getByRole('button', { name: 'Publish' }).click();
  await expect(ownerPage.locator('.toast', { hasText: 'Published.' })).toBeVisible();
  await page.waitForTimeout(1200);
  expect(viewRequests).toEqual([]);
  expect(frames.filter((frame) => frame.includes('"event"'))).toEqual([]);
  await expect(page.getByText('A Fieldnotes Secret')).toHaveCount(0);
  await ownerPage.getByRole('button', { name: 'Unpublish' }).click();
  await expect(ownerPage.locator('.toast', { hasText: 'Moved back to drafts.' })).toBeVisible();
  await owner.close();
  await stolen.close();
});

test('the Impact Lab projects a schema change, explains the differences, and leaves live data alone', async ({ page }) => {
  test.setTimeout(120_000);
  await signIn(page, 'tomasz');
  await open(page, '/lab');
  await expect(page.getByRole('heading', { level: 1, name: 'Your account cannot open this page' })).toBeVisible();
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.locator('.toast', { hasText: 'Signed out.' })).toBeVisible();

  await signIn(page, 'admin');
  await page.getByRole('link', { name: 'Impact Lab' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Impact Lab' })).toBeVisible();
  const snapshot = await page.locator('.lab-snapshot').innerText();
  expect(snapshot).toMatch(/Fieldnotes at revision \d+: schema version 2/);

  // The tenant's metadata is there to read, as data.
  await page.getByRole('button', { name: 'Metadata' }).click();
  await expect(page.getByRole('heading', { level: 2, name: /Post/ })).toBeVisible();
  await expect(page.getByRole('row', { name: /post\.excerpt string no no/ })).toBeVisible();
  await expect(page.getByText('public_reads_published')).toBeVisible();
  await axe(page, 'Impact Lab metadata');

  // A proposal that does not parse is explained next to the field.
  await page.getByRole('button', { name: 'Projection' }).click();
  await page.getByLabel('Proposed change').fill('require post.nonsense');
  await page.getByRole('button', { name: 'Run the projection' }).click();
  await expect(page.getByRole('alert')).toHaveText('line 1: post has no field nonsense');

  await page.getByRole('button', { name: 'Require an excerpt on every article' }).click();
  await expect(page.getByLabel('Proposed change')).toHaveValue('require post.excerpt');
  await page.getByLabel('Inject faults: lost commit races and dropped event deliveries').check();
  await page.getByRole('button', { name: 'Run the projection' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'What would change' })).toBeVisible({ timeout: 60_000 });
  await expect(page.locator('.lab-verdict').first()).toContainText(/The kernel accepts the change\. \d+ of 60 replayed operations then end differently\./);

  // All five kinds of evidence are present and labelled.
  for (const label of ['Proven', 'Reproduced', 'Sound approximation', 'Estimated', 'Unknown']) {
    await expect(page.locator('.findings .evidence', { hasText: label }).first()).toBeVisible();
  }
  await expect(page.getByRole('row', { name: /post \d+ Slow Software required:post\.excerpt/ })).toBeVisible();
  await expect(page.getByRole('row', { name: /first divergence/ })).toContainText('Rejected: MissingRequired');
  await expect(page.getByText('Smallest schedule found that still shows the first divergence')).toBeVisible();
  await expect(page.getByRole('row', { name: /Every record has only known fields/ })).toContainText('Fails');
  await expect(page.getByText('Tenant isolation: every other tenant in the simulated world is identical in both executions.')).toBeVisible();
  await expect(page.getByText(/60 of 6\d steps agree when re-run against the bare kernel/)).toBeVisible();
  await expect(page.getByText(/event deliveries dropped and retried; every event still arrived once, in order/)).toBeVisible();
  await page.screenshot({ path: `${SHOTS}lab.png`, fullPage: true });
  await axe(page, 'Impact Lab results');

  // Running it again reproduces the same number of divergences.
  const first = await page.locator('.lab-verdict').first().innerText();
  await page.getByRole('button', { name: 'Run the projection' }).click();
  await expect(page.locator('.toast', { hasText: 'Projection finished' }).first()).toBeVisible({ timeout: 60_000 });
  // The strict variant: the kernel refuses the change and names the records in the way.
  await page.getByLabel('Refuse the change').check();
  await page.getByRole('button', { name: 'Run the projection' }).click();
  await expect(page.locator('.lab-verdict').first()).toContainText(/The kernel refuses the change: \d+ existing record\(s\) would not conform to the new schema\./, { timeout: 60_000 });
  expect(first).toContain('The kernel accepts the change');

  // What actually happened, replayed with the change in force from the start.
  await page.getByLabel('Leave them as they are').check();
  await page.getByLabel('What actually happened').check();
  await page.getByRole('button', { name: 'Run the projection' }).click();
  await expect(page.locator('.lab-verdict').first()).toContainText(/replayed operations then end differently/, { timeout: 60_000 });
  await expect(page.getByRole('row', { name: /first divergence/ })).toContainText('create record');

  // The journal of real commits replays to the live state.
  await page.getByRole('button', { name: 'Evidence' }).click();
  await page.getByRole('button', { name: 'Replay the journal' }).click();
  await expect(page.getByText(/committed operations replay to exactly the recorded result, and the replayed state is the live state\./)).toBeVisible();

  // Diagnostics are there, machine-readable, and hold no secrets.
  await page.getByRole('button', { name: 'Diagnostics' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Kernel transitions' })).toBeVisible();
  await expect(page.getByRole('row', { name: /change_schema/ })).toBeVisible();
  const cookie = (await page.context().cookies()).find((c) => c.name === '__Host-strata_sid').value;
  const report = await (await page.request.get('/api/diagnostics')).text();
  expect(report).not.toContain(cookie);
  expect(report).not.toContain('strata-dev-');
  expect(JSON.parse(report).backend.recent_runs.length).toBeGreaterThanOrEqual(4);
  await axe(page, 'Impact Lab diagnostics');

  // Nothing above changed the live publication: same revision, excerpt still optional.
  await page.reload();
  await ready(page);
  await expect(page.locator('.lab-snapshot')).toHaveText(snapshot);
  await page.getByRole('button', { name: 'Metadata' }).click();
  await expect(page.getByRole('row', { name: /post\.excerpt string no no/ })).toBeVisible();
});

test('the Lean proofs can be checked from the Lab when Lean is installed', async ({ page }) => {
  test.setTimeout(180_000);
  await signIn(page, 'admin');
  await open(page, '/lab');
  await page.getByRole('button', { name: 'Evidence' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'The five kinds of evidence' })).toBeVisible();
  await page.getByRole('button', { name: 'Check the proofs now' }).click();
  const verdict = page.locator('.lab-block', { hasText: 'Machine-checked proofs' }).locator('.lab-verdict');
  await expect(verdict).toBeVisible({ timeout: 150_000 });
  const text = await verdict.innerText();
  test.skip(text.includes('could not run lake'), 'Lean is not installed on this machine; the report says so honestly.');
  expect(text).toMatch(/All \d+ theorems are machine-checked by Lean \(version 4\.\d+\.\d+/);
  await expect(page.getByRole('row', { name: /A tenant-scoped transition cannot modify another tenant/ })).toContainText('step_isolated checked');
  await expect(page.getByText(/print identical results for \d+ operations across 60 generated scenarios/)).toBeVisible();
  await page.screenshot({ path: `${SHOTS}lab-evidence.png`, fullPage: true });
  await axe(page, 'Impact Lab evidence');
});

test('no log line holds a password, a session token or a CSRF token', async ({ page }) => {
  await signIn(page, 'ines');
  const session = (await (await page.request.get('/api/session')).json()).session;
  const cookie = (await page.context().cookies()).find((c) => c.name === '__Host-strata_sid').value;
  const { readFileSync } = await import('node:fs');
  const root = new URL('../../var/logs/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
  const logs = ['backend.jsonl', 'bff.jsonl'].map((name) => readFileSync(`${root}${name}`, 'utf8')).join('\n');
  expect(logs.length).toBeGreaterThan(1000);
  for (const secret of [cookie, session.csrf_token, 'strata-dev-ines', 'strata-dev-admin', 'not-the-password']) {
    expect(logs.includes(secret), `a secret appears in the logs: ${secret.slice(0, 6)}…`).toBe(false);
  }
  // One request can be followed by its correlation id across both services.
  const correlation = logs.match(/"correlation":"(c-\d+-[0-9a-f]{6})"/)?.[1];
  expect(correlation).toBeTruthy();
  expect(logs.split('\n').filter((line) => line.includes(correlation)).map((line) => JSON.parse(line).service)).toEqual(expect.arrayContaining(['backend', 'bff']));
});

test('the development fixtures refuse a visitor who holds only the request token', async ({ request }) => {
  const { session } = await (await request.get('/api/session')).json();
  expect(session.authenticated).toBe(false);
  for (const action of ['reset', 'expire-session']) {
    const refused = await request.post(`/api/dev/${action}`, { headers: { origin: BASE, 'x-csrf-token': session.csrf_token } });
    expect(refused.status()).toBe(401);
  }
});

test('sign-in, publish and unpublish are logged as domain events, with their allowlisted fields and correlation', async ({ page }) => {
  const { readFileSync } = await import('node:fs');
  const root = new URL('../../var/logs/', import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, '$1');
  const bytes = (service) => {
    try {
      return readFileSync(`${root}${service}.jsonl`);
    } catch {
      return Buffer.alloc(0);
    }
  };
  // Only the lines written during this test: the files hold the whole run.
  const from = { backend: bytes('backend').length, bff: bytes('bff').length };
  const lines = (service) => bytes(service).subarray(from[service]).toString('utf8').split('\n').flatMap((text) => {
    try {
      return [JSON.parse(text)];
    } catch {
      return [];
    }
  });
  // The backend writes its lines asynchronously, so each expected line is polled for.
  const line = async (event, fields) => {
    const find = () => lines('backend').find((l) => l.event === event && Object.entries(fields).every(([k, v]) => l[k] === v));
    await expect.poll(() => find() !== undefined, { message: `a backend line ${event} with ${JSON.stringify(fields)}`, timeout: 5_000 }).toBe(true);
    return find();
  };
  // The line carries the request's correlation id, which the BFF logged for the same request.
  const correlated = (found) => {
    expect(found.correlation).toMatch(/^c-\d+-[0-9a-f]{6}$/);
    expect(lines('bff').some((l) => l.correlation === found.correlation)).toBe(true);
  };
  const common = ['correlation', 'event', 'level', 'service', 'tenant', 'ts'];

  await signIn(page, 'tomasz');
  const signedIn = await line('session.started', { service: 'backend', level: 'info', tenant: 'fieldnotes', principal: 'user:tomasz', method: 'password' });
  expect(signedIn.session).toMatch(/^s_/);
  expect(typeof signedIn.ts).toBe('number');
  expect(Object.keys(signedIn).sort()).toEqual([...common, 'method', 'principal', 'session'].sort());
  correlated(signedIn);

  await writeDraft(page, 'Logged Along The Way');
  const created = await line('record.created', { tenant: 'fieldnotes', object: 'post' });
  expect(Object.keys(created).sort()).toEqual([...common, 'id', 'object', 'revision', 'seq', 'version'].sort());
  correlated(created);

  await page.getByRole('button', { name: 'Publish' }).click();
  await expect(page.locator('.toast', { hasText: 'Published.' })).toBeVisible();
  const published = await line('record.updated', { tenant: 'fieldnotes', object: 'post', id: created.id, transition: 'publish' });
  expect(Object.keys(published).sort()).toEqual([...common, 'id', 'object', 'revision', 'seq', 'transition', 'version'].sort());
  expect(published.seq).toBeGreaterThan(created.seq);
  correlated(published);

  await page.getByRole('link', { name: 'Back to the desk' }).click();
  await page.getByRole('button', { name: 'Unpublish Logged Along The Way' }).click();
  await expect(page.locator('.toast', { hasText: 'Moved back to drafts.' })).toBeVisible();
  const unpublished = await line('record.updated', { tenant: 'fieldnotes', object: 'post', id: created.id, transition: 'unpublish' });
  expect(unpublished.seq).toBeGreaterThan(published.seq);
  expect(unpublished.correlation).not.toBe(published.correlation);
  correlated(unpublished);

  // No line of these events holds the article's text.
  expect(lines('backend').filter((l) => JSON.stringify(l).includes('A first paragraph written by a test'))).toEqual([]);

  await page.getByRole('button', { name: 'Delete Logged Along The Way', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm deleting Logged Along The Way' }).click();
  await expect(page.locator('.toast', { hasText: 'Deleted.' })).toBeVisible();
  await line('record.deleted', { tenant: 'fieldnotes', object: 'post', id: created.id });
});

/** Sign out every other session of the person at the desk, so that a test starts from one. */
async function leaveOnlyThisSession(page) {
  await open(page, '/desk');
  const section = page.getByRole('region', { name: 'Where you are signed in' });
  await expect(section).toBeVisible();
  const others = section.getByRole('button', { name: 'Sign out all other sessions' });
  if (await others.count()) {
    await others.click();
    await expect(section.getByText('This is the only place you are signed in.')).toBeVisible();
  }
}

test('a person sees where they are signed in and signs out another session from the desk', async ({ page, browser }) => {
  await signIn(page, 'tomasz');
  await leaveOnlyThisSession(page);
  const section = page.getByRole('region', { name: 'Where you are signed in' });
  await expect(section.getByRole('row')).toHaveCount(2); // the header and this device

  const elsewhere = await (await browser.newContext({ baseURL: BASE })).newPage();
  await signIn(elsewhere, 'tomasz');
  await expect(elsewhere.getByRole('region', { name: 'Where you are signed in' }).getByRole('row')).toHaveCount(3);
  await expect(elsewhere.locator('.colophon__live')).toHaveAttribute('data-connection', 'open');

  await open(page, '/desk');
  await expect(section.getByRole('row')).toHaveCount(3);
  await expect(section.locator('tr[data-current="true"]')).toHaveCount(1);
  await expect(section.locator('tr[data-current="false"]')).toContainText('Another device');
  // Nothing of the session's secret is on the page.
  expect(await page.content()).not.toContain('csrf');

  await section.getByRole('button', { name: 'Sign out that session' }).click();
  await expect(page.locator('.toast', { hasText: 'That session has been signed out.' })).toBeVisible();
  await expect(section.getByRole('row')).toHaveCount(2);
  await expect(section.getByText('This is the only place you are signed in.')).toBeVisible();

  // The session that was ended is noticed where it is open, and its page falls back to signed out.
  await expect(elsewhere.getByRole('heading', { level: 1, name: 'Sign in to continue' })).toBeVisible();
  // This one was not touched.
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
});

test('a person signs out all their other sessions at once', async ({ page, browser }) => {
  await signIn(page, 'mirela');
  await leaveOnlyThisSession(page);
  const others = [];
  for (let i = 0; i < 2; i++) {
    const elsewhere = await (await browser.newContext({ baseURL: BASE })).newPage();
    await signIn(elsewhere, 'mirela');
    others.push(elsewhere);
  }
  await open(page, '/desk');
  const section = page.getByRole('region', { name: 'Where you are signed in' });
  await expect(section.getByRole('row')).toHaveCount(4);
  await section.getByRole('button', { name: 'Sign out all other sessions' }).click();
  await expect(page.locator('.toast', { hasText: 'Signed out 2 other sessions.' })).toBeVisible();
  await expect(section.getByRole('row')).toHaveCount(2);
  for (const elsewhere of others) {
    await expect(elsewhere.getByRole('heading', { level: 1, name: 'Sign in to continue' })).toBeVisible();
  }
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
});

test('ten wrong passwords lock a login for a while, whether or not it exists', async ({ page }) => {
  await open(page, '/login');
  // A login that does not exist, so that no real account is locked for the tests that follow:
  // the limit counts the name that was typed, and nothing about it says whether it has an account.
  const csrf = (await (await page.request.get('/api/session')).json()).session.csrf_token;
  const attempt = () =>
    page.request.post('/api/login', {
      data: { login: '  Lockout-Probe ', password: 'not-the-password' },
      headers: { origin: BASE, 'x-csrf-token': csrf },
    });
  for (let i = 0; i < 10; i++) {
    const refused = await attempt();
    expect(refused.status()).toBe(401);
    expect((await refused.json()).message).toBe('The name or password is not right.');
  }
  // The eleventh, another spelling of the same login, through the page.
  await page.getByLabel('Name').fill('LOCKOUT-PROBE');
  await page.getByLabel('Password').fill('not-the-password');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Too many attempts');
  const locked = await attempt();
  expect(locked.status()).toBe(429);
  expect(locked.headers()['set-cookie']).toBeUndefined();

  // Another login is not affected, and signing in still works.
  await signIn(page, 'ines');
});

// Passkeys. A virtual authenticator (the Chrome DevTools Protocol's WebAuthn domain) stands in
// for the person's device. WebAuthn needs the site to be reached by a name, so these use
// `localhost`; at the address `127.0.0.1` the site offers no passkeys, which the first test checks.
const NAMED = 'http://localhost:4200';

async function virtualAuthenticator(page) {
  const client = await page.context().newCDPSession(page);
  await client.send('WebAuthn.enable');
  const { authenticatorId } = await client.send('WebAuthn.addVirtualAuthenticator', {
    options: { protocol: 'ctap2', transport: 'internal', hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  return { client, authenticatorId, credentials: async () => (await client.send('WebAuthn.getCredentials', { authenticatorId })).credentials };
}

async function signInNamed(page, name) {
  await open(page, `${NAMED}/login`);
  await page.getByLabel('Name').fill(name);
  await page.getByLabel('Password').fill(`strata-dev-${name}`);
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
}

test('a site reached by an address that is not a name offers no passkeys', async ({ page }) => {
  await open(page, '/login');
  await expect(page.getByRole('button', { name: 'Sign in with a passkey' })).toHaveCount(0);
  await signIn(page, 'ines');
  await expect(page.getByRole('region', { name: 'Passkeys' })).toHaveCount(0);
});

test('a person adds a passkey at the desk, signs out, signs in with it and removes it', async ({ page }) => {
  const authenticator = await virtualAuthenticator(page);
  await signInNamed(page, 'ines');
  const section = page.getByRole('region', { name: 'Passkeys' });
  await expect(section.getByText('You have no passkeys.')).toBeVisible();

  await section.getByRole('button', { name: 'Add a passkey' }).click();
  await expect(page.locator('.toast', { hasText: 'Passkey added.' })).toBeVisible();
  await expect(section.locator('tr[data-passkey]')).toHaveCount(1);
  await expect(section.locator('tr[data-passkey]')).toContainText('Never');
  expect(await authenticator.credentials()).toHaveLength(1);
  // The server's relying party is the address the person is at.
  expect((await authenticator.credentials())[0].rpId).toBe('localhost');

  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await open(page, `${NAMED}/login`);
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  await expect(page.getByText('Signed in as Ines')).toBeVisible();
  // The use is noted, and the signature counter moved on.
  await expect(section.locator('tr[data-passkey]')).not.toContainText('Never');
  expect((await authenticator.credentials())[0].signCount).toBeGreaterThan(0);

  await section.getByRole('button', { name: /^Remove/ }).click();
  await expect(page.locator('.toast', { hasText: 'That passkey has been removed.' })).toBeVisible();
  await expect(section.getByText('You have no passkeys.')).toBeVisible();

  // A removed passkey signs no one in: the authenticator still holds it, the server does not.
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await open(page, `${NAMED}/login`);
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await expect(page.getByRole('alert')).toContainText('not accepted');
});

test('an assertion made for one address is refused at another', async ({ page }) => {
  const authenticator = await virtualAuthenticator(page);
  await signInNamed(page, 'ines');
  const section = page.getByRole('region', { name: 'Passkeys' });
  await section.getByRole('button', { name: 'Add a passkey' }).click();
  await expect(section.locator('tr[data-passkey]')).toHaveCount(1);
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  expect(await authenticator.credentials()).toHaveLength(1);

  // The browser answers a challenge issued at localhost, as a page at localhost.
  const answer = await page.evaluate(async () => {
    const bytes = (text) => Uint8Array.from(atob(text.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
    const text = (buffer) => btoa(String.fromCharCode(...new Uint8Array(buffer))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const csrf = (await (await fetch('/api/session')).json()).session.csrf_token;
    const issued = await (await fetch('/api/passkeys/challenge', { method: 'POST', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf }, body: '{}' })).json();
    const options = issued.options.publicKey;
    const credential = await navigator.credentials.get({ publicKey: { challenge: bytes(options.challenge), rpId: options.rpId, userVerification: 'required' } });
    const response = credential.response;
    return {
      challenge: issued.challenge,
      assertion: {
        id: credential.id,
        rawId: text(credential.rawId),
        type: credential.type,
        response: { authenticatorData: text(response.authenticatorData), clientDataJSON: text(response.clientDataJSON), signature: text(response.signature), userHandle: text(response.userHandle) },
        extensions: credential.getClientExtensionResults(),
      },
    };
  });

  // Presented from another address of the same site, it is refused and starts no session.
  const other = 'http://127.0.0.1:4200';
  const csrf = (await (await page.request.get(`${other}/api/session`)).json()).session.csrf_token;
  const refused = await page.request.post(`${other}/api/sign-in/passkey`, { data: answer, headers: { origin: other, 'x-csrf-token': csrf }, failOnStatusCode: false });
  expect(refused.status()).toBe(401);
  expect(refused.headers()['set-cookie']).toBeUndefined();

  // The challenge was spent by that attempt, so the right address cannot use it either.
  const csrfHere = (await (await page.request.get(`${NAMED}/api/session`)).json()).session.csrf_token;
  const replay = await page.request.post(`${NAMED}/api/sign-in/passkey`, { data: answer, headers: { origin: NAMED, 'x-csrf-token': csrfHere }, failOnStatusCode: false });
  expect(replay.status()).toBe(401);

  // Clean up: the passkey goes, so that no later test finds it.
  await signInNamed(page, 'ines');
  await page.getByRole('region', { name: 'Passkeys' }).getByRole('button', { name: /^Remove/ }).click();
  await expect(page.locator('.toast', { hasText: 'That passkey has been removed.' })).toBeVisible();
});

// Emailed sign-in links. Nothing is sent to a real address: the development sink keeps the
// message, and the site's owner reads it through the development route.
const LINK_ADDRESS = 'mirela@fieldnotes.example';

/** The links in the messages the development sink holds for an address, oldest first. */
async function linksSent(owner, address) {
  const reply = await owner.evaluate(async () => {
    const csrf = (await (await fetch('/api/session')).json()).session.csrf_token;
    const response = await fetch('/api/dev/mail-outbox', { method: 'POST', headers: { 'content-type': 'application/json', 'x-csrf-token': csrf }, body: '{}' });
    return response.json();
  });
  return reply.messages.filter((mail) => mail.to === address).map((mail) => mail.text.match(/https?:\/\/\S+/)?.[0]);
}

async function askForLink(page, address) {
  await open(page, '/login');
  await page.getByLabel('Email address').fill(address);
  await page.getByRole('button', { name: 'Email me a sign-in link' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'a sign-in link is on its way' })).toBeVisible();
}

test('an emailed link signs in once, in the browser that asked for it and in no other', async ({ page, browser }) => {
  const ownerContext = await browser.newContext();
  const owner = await ownerContext.newPage();
  await signIn(owner, 'admin');

  // The answer is the same for an address with no account, and no message is kept for it.
  await askForLink(page, 'nobody@fieldnotes.example');
  expect(await linksSent(owner, 'nobody@fieldnotes.example')).toEqual([]);

  // A link opened in another browser is refused, and its token does not stay in the address bar.
  await askForLink(page, LINK_ADDRESS);
  await expect.poll(async () => (await linksSent(owner, LINK_ADDRESS)).length).toBe(1);
  const [first] = await linksSent(owner, LINK_ADDRESS);
  expect(first).toContain('#c=');
  const otherContext = await browser.newContext();
  const other = await otherContext.newPage();
  await other.goto(first);
  await expect(other.getByRole('alert')).toContainText('not accepted');
  expect(other.url()).not.toContain('t=');
  await expect(other.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
  await otherContext.close();

  // A second request from the first browser keeps its binding; its link signs in, once.
  await askForLink(page, LINK_ADDRESS);
  await expect.poll(async () => (await linksSent(owner, LINK_ADDRESS)).length).toBe(2);
  const second = (await linksSent(owner, LINK_ADDRESS))[1];
  await page.goto(second);
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  await expect(page.getByText('Signed in as Mirela Ashby')).toBeVisible();
  expect(page.url()).not.toContain('#');

  // The same link, a second time, in the same browser: refused.
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await page.goto(second);
  await expect(page.getByRole('alert')).toContainText('not accepted');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();

  await ownerContext.close();
});

// Sign-in at an identity provider. A mock OpenID Connect provider (tests/e2e/mock-oidc.mjs) stands
// in for it: it approves at once, as whoever the test says, and the site is configured to trust it
// for Fieldnotes and to let its first sign-ins in as authors. It is also bound to the address
// domain `bound.example`, so a password for an address there is refused.
const PROVIDER = 'http://127.0.0.1:4290';

/** Tell the mock provider who signs in next and what, if anything, is wrong with the token. */
async function provider(request, fields = {}) {
  const reply = await request.post(`${PROVIDER}/__control`, { data: { reset: true, ...fields } });
  expect(reply.ok()).toBe(true);
}

async function chooseProvider(page) {
  await open(page, '/login');
  await page.getByRole('button', { name: 'Sign in with Mock Provider' }).click();
}

test('a password for an address in a domain the provider decides for is refused', async ({ page }) => {
  await open(page, '/login');
  await page.getByLabel('Name').fill('ines@bound.example');
  await page.getByLabel('Password').fill('strata-dev-ines');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveText('The name or password is not right.');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();

  // The same password for the same person's own login is accepted: only the bound domain is closed.
  await page.getByLabel('Name').fill('ines');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
});

test('the sign-in page offers the provider, and signing in through it makes the person once', async ({ page, request }) => {
  const subject = `e2e-${Date.now()}`;
  await provider(request, { sub: subject, email: 'nora@bound.example', email_verified: true, name: 'Nora Newcomer' });
  const callbacks = [];
  page.on('request', (r) => r.url().includes('/federation/oidc/callback') && callbacks.push(r.url()));

  await chooseProvider(page);
  await expect(page.getByRole('heading', { level: 1, name: 'Writing desk' })).toBeVisible();
  await expect(page.getByText('Signed in as Nora Newcomer')).toBeVisible();
  expect(page.url()).not.toContain('#');
  expect(page.url()).not.toContain('code=');
  expect(callbacks).toHaveLength(1);

  // The second time the provider names the same person by another name, and the site keeps its own.
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();
  await provider(request, { sub: subject, email: 'nora@bound.example', email_verified: true, name: 'Someone Else' });
  await chooseProvider(page);
  await expect(page.getByText('Signed in as Nora Newcomer')).toBeVisible();
  await page.getByRole('button', { name: 'Sign out', exact: true }).click();

  // A callback that was answered once is not answered again.
  await page.goto(callbacks[0]);
  await expect(page.getByRole('alert')).toContainText('not accepted');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
});

test('an identity token that is wrong in any way signs no one in', async ({ page, request }) => {
  const wrong = [
    { name: 'audience', fields: { aud: 'another-client' } },
    { name: 'nonce', fields: { nonce: 'a-nonce-that-was-never-asked-for' } },
    { name: 'expiry', fields: { exp_offset: -3600 } },
    { name: 'key', fields: { kid: 'a-key-that-is-not-published' } },
  ];
  for (const { name, fields } of wrong) {
    await provider(request, { sub: `e2e-wrong-${name}`, email: 'x@bound.example', email_verified: true, name: 'Nobody', ...fields });
    await chooseProvider(page);
    await expect(page.getByRole('alert'), name).toContainText('not accepted');
    await expect(page.getByRole('heading', { level: 1, name: 'Sign in' }), name).toBeVisible();
    expect(page.url(), name).not.toContain('#');
  }

  // A provider that refuses the person sends no code at all.
  await provider(request, { deny: true });
  await chooseProvider(page);
  await expect(page.getByRole('alert')).toContainText('did not sign you in');
});

test('a callback that no sign-in of this browser started is refused', async ({ page }) => {
  await open(page, '/login');
  const forged = `${'0123456789abcdef'.repeat(4)}`;
  await page.goto(`/federation/oidc/callback?state=${forged}&code=a-code-nobody-issued`);
  await expect(page.getByRole('alert')).toContainText('not accepted');
  await expect(page.getByRole('heading', { level: 1, name: 'Sign in' })).toBeVisible();
  expect(page.url()).not.toContain('code=');
});
