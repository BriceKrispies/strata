// Hostile values through the real page (WG-5677). Every string the page draws from its view model
// (a record field, a setting label, a tenant screen's text, a link address) is replaced, in the
// response of the view endpoint, by one of a fixed, seeded set of hostile values: markup, event
// handler attributes, script and data URLs, encoded and case-mangled variants, SVG and CSS payloads.
// The page renders what it receives through its own renderer, and the live DOM is then checked:
// no script element beyond the page's own, no event-handler, style or srcdoc attribute, no URL
// attribute with a script, data or VBScript scheme, no frame, object or embed element in the body,
// and nothing sets `window.__xss` (every payload that runs sets it).
//
// The values are generated from a fixed seed, so a failure names a reproducible value. The test also
// checks that the Content Security Policy and Trusted Types headers are sent with the page.
import { expect, test } from '@playwright/test';

/** The page's own paths that show data without a session. */
const PATHS = ['/', '/topics', '/authors', '/about', '/search?q=hostile'];

/** The number of hostile values the run feeds in, at least. */
const VALUES = 1000;

/** The view endpoint the worker reads (web/src/generated/contracts.js, `viewPath`). */
const VIEW = '/api/view';

/**
 * Keys whose string values the page needs to read as structure (a kind, a state, a route, an action
 * name). Every other string is replaced, including the addresses (`href`, `src`).
 */
const STRUCTURAL = new Set([
  'kind', 'view', 'status', 'type', 'state', 'mark', 'layout', 'template', 'screen', 'auth', 'level',
  'tag', 'key', 'id', 'code', 'scheme', 'mode', 'theme', 'accent', 'paper', 'preset', 'rev', 'version',
  'route', 'path', 'action', 'name', 'arg', 'tone', 'size', 'variant', 'connection', 'lang', 'datetime',
]);

const BASE = [
  '<script>window.__xss = 1</script>',
  '<img src=x onerror="window.__xss = 1">',
  '<img src="x" onerror=window.__xss=1>',
  'javascript:window.__xss = 1',
  'JaVaScRiPt:window.__xss = 1',
  '" onmouseover="window.__xss = 1"',
  "' onfocus='window.__xss = 1' autofocus='",
  '<svg onload="window.__xss = 1"></svg>',
  '<svg><script>window.__xss = 1</script></svg>',
  '<svg><animate attributeName=href values="javascript:window.__xss=1" /><a><text>x</text></a></svg>',
  '<iframe srcdoc="<script>window.__xss = 1</script>"></iframe>',
  '<body onload=window.__xss=1>',
  '<style>body{background:url(javascript:window.__xss=1)}</style>',
  'expression(window.__xss = 1)',
  '<a style="x:expression(window.__xss = 1)" href="#">x</a>',
  '<a href="javascript:window.__xss = 1">link</a>',
  '<a href="&#106;avascript:window.__xss=1">link</a>',
  '<a href="&#x6A;avascript&colon;window.__xss=1">link</a>',
  '%3Cscript%3Ewindow.__xss%3D1%3C%2Fscript%3E',
  '&lt;script&gt;window.__xss = 1&lt;/script&gt;',
  '\\u003cscript\\u003ewindow.__xss = 1\\u003c/script\\u003e',
  'javascript&colon;window.__xss = 1',
  'java\tscript:window.__xss = 1',
  'java\nscript:window.__xss = 1',
  'data:text/html,<script>window.__xss = 1</script>',
  'vbscript:msgbox(1)',
  '//evil.example/x',
  '/\\evil.example/x',
  '<meta http-equiv="refresh" content="0;url=javascript:window.__xss = 1">',
  '<base href="javascript:window.__xss = 1//">',
  '<form action="javascript:window.__xss = 1"><button>x</button></form>',
  '<button formaction="javascript:window.__xss = 1">x</button>',
  '<object data="javascript:window.__xss = 1"></object>',
  '<embed src="javascript:window.__xss = 1">',
  '<link rel="import" href="data:text/html,<script>window.__xss=1</script>">',
  '<template><script>window.__xss = 1</script></template>',
  '<math><mtext><table><mglyph><style><img src=x onerror=window.__xss=1>',
  '<details open ontoggle="window.__xss = 1">',
  '<video><source onerror="window.__xss = 1">',
  '\u0000<script>window.__xss = 1</script>',
  '​<img src=x onerror=window.__xss=1>',
];

/**
 * A seeded generator in [0, 1) (mulberry32). The same seed gives the same sequence.
 * @param {number} seed
 */
function random(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const CONTROL = ['\t', '\n', '\r', ' ', '​', '\u0000', '﻿'];

/**
 * The hostile values: each base payload, mangled by the generator (case flips, control characters
 * inside a scheme, percent and entity encoding of a character), and numbered so each is distinct.
 * @param {number} count
 * @param {number} seed
 * @returns {string[]}
 */
function hostileValues(count, seed) {
  const rand = random(seed);
  const values = [];
  for (let i = 0; i < count; i += 1) {
    let value = BASE[i % BASE.length];
    const edits = 1 + Math.floor(rand() * 3);
    for (let e = 0; e < edits; e += 1) {
      const at = Math.floor(rand() * (value.length + 1));
      const kind = Math.floor(rand() * 5);
      if (kind === 0) {
        // Flip the case of a letter.
        const letter = value.slice(at).search(/[a-z]/i);
        if (letter >= 0) {
          const pos = at + letter;
          const c = value[pos];
          value = value.slice(0, pos) + (c === c.toLowerCase() ? c.toUpperCase() : c.toLowerCase()) + value.slice(pos + 1);
        }
      } else if (kind === 1) {
        // A control character inside the value (a scheme split by a tab, say).
        value = value.slice(0, at) + CONTROL[Math.floor(rand() * CONTROL.length)] + value.slice(at);
      } else if (kind === 2) {
        // Percent-encode the character at the spot.
        const c = value[at];
        if (c !== undefined) {
          const code = c.charCodeAt(0);
          if (code < 256) value = value.slice(0, at) + `%${code.toString(16).padStart(2, '0')}` + value.slice(at + 1);
        }
      } else if (kind === 3) {
        // Entity-encode an angle bracket or a quote.
        value = value.replace(/[<>"']/, (c) => `&#${c.charCodeAt(0)};`);
      } else {
        // Repeat the value, so a length-based check does not stop at the first copy.
        value = `${value}${value}`.slice(0, 400);
      }
    }
    values.push(`${value} #${seed}-${i}`);
  }
  return values;
}

/**
 * The view model with every string the page may draw replaced by the next hostile value. Counts
 * the replacements in `tally.injected`.
 * @param {any} node @param {string} key @param {() => string} next @param {{ injected: number }} tally
 * @returns {any}
 */
function mutate(node, key, next, tally) {
  if (typeof node === 'string') {
    if (STRUCTURAL.has(key)) return node;
    tally.injected += 1;
    return next();
  }
  if (Array.isArray(node)) return node.map((item) => mutate(item, key, next, tally));
  if (node !== null && typeof node === 'object') {
    return Object.fromEntries(Object.entries(node).map(([k, v]) => [k, mutate(v, k, next, tally)]));
  }
  return node;
}

/** What the live page holds that must not be there. Runs in the page. */
function snapshot() {
  const bad = [];
  for (const el of document.querySelectorAll('*')) {
    for (const attr of el.attributes) {
      const name = attr.name.toLowerCase();
      if (name.startsWith('on') || name === 'style' || name === 'srcdoc' || name === 'formaction') {
        bad.push(`${el.localName} ${name}`);
      }
      if (['href', 'src', 'action', 'formaction', 'xlink:href'].includes(name)) {
        const value = attr.value.replace(/[\u0000- \u007f-\u009f]/g, '');
        if (/^(javascript|vbscript|data):/i.test(value)) bad.push(`${el.localName} ${name}=${attr.value}`);
      }
    }
  }
  const forbidden = [...document.body.querySelectorAll('iframe,object,embed,frame,frameset,applet,base,meta,template,noscript,link,style')].map((el) => el.localName);
  const scripts = [...document.querySelectorAll('script')];
  return {
    scripts: scripts.length,
    scriptsWithPayload: scripts.filter((s) => /__xss/.test(s.textContent ?? '')).length,
    bad,
    forbidden,
    flag: Object.prototype.hasOwnProperty.call(window, '__xss'),
    sawText: document.body.textContent.includes('<script>'),
  };
}

async function ready(page) {
  await page.locator('main[data-page]:not([data-page="loading"])').waitFor();
}

test('the page is served with the Content Security Policy and Trusted Types', async ({ page }) => {
  const response = await page.goto('/');
  expect(response).not.toBeNull();
  const csp = response.headers()['content-security-policy'] ?? '';
  for (const directive of [
    "default-src 'self'",
    "script-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "require-trusted-types-for 'script'",
    'trusted-types strata-worker',
  ]) {
    expect(csp, directive).toContain(directive);
  }
});

test('hostile values from the page\'s view model leave the live DOM inert', async ({ page }) => {
  test.setTimeout(900_000);
  await page.goto('/');
  await ready(page);
  const baseline = await page.evaluate(snapshot);
  expect(baseline.bad).toEqual([]);
  expect(baseline.forbidden).toEqual([]);
  expect(baseline.flag).toBe(false);

  const values = hostileValues(VALUES, 0x5eed);
  let cursor = 0;
  const next = () => {
    const value = values[cursor % values.length];
    cursor += 1;
    return value;
  };
  const tally = { injected: 0 };
  let routed = 0;
  // The worker's own requests to the view endpoint pass through here; their replies are rewritten.
  await page.route((url) => url.pathname === VIEW, async (route) => {
    const response = await route.fetch();
    let body;
    try {
      body = await response.json();
    } catch {
      await route.fulfill({ response });
      return;
    }
    routed += 1;
    await route.fulfill({ response, json: mutate(body, '', next, tally) });
  });

  let sawText = false;
  for (let round = 0; tally.injected < VALUES && round < 60; round += 1) {
    for (const path of PATHS) {
      await page.goto(path);
      await ready(page);
      const seen = await page.evaluate(snapshot);
      expect(seen.scripts, `scripts on ${path}`).toBe(baseline.scripts);
      expect(seen.scriptsWithPayload, `script text on ${path}`).toBe(0);
      expect(seen.bad, `attributes on ${path}`).toEqual([]);
      expect(seen.forbidden, `elements on ${path}`).toEqual([]);
      expect(seen.flag, `window.__xss on ${path}`).toBe(false);
      sawText ||= seen.sawText;
    }
  }

  // The run reached the page: its replies were rewritten, and the hostile values were drawn as text.
  expect(routed).toBeGreaterThan(0);
  expect(tally.injected).toBeGreaterThanOrEqual(VALUES);
  expect(sawText).toBe(true);
});
