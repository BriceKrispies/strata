// Tests for scripts/ui-tokens.mjs and scripts/check-style-literals.mjs.
// Run with: node --test scripts/ui-tokens.test.mjs
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { checkStyles, findLiterals } from './check-style-literals.mjs';
import { parseJsonc } from './jsonc.mjs';
import { filesFor, loadCatalogue, outDir, ratio, parseHex, refusals, renderTheme, expandPresets, writeTokens } from './ui-tokens.mjs';

const fixtureFile = new URL('./fixtures/ui-tokens/low-contrast.jsonc', import.meta.url);
const made = [];
after(() => made.forEach((dir) => rmSync(dir, { recursive: true, force: true })));

test('the contrast arithmetic matches WCAG', () => {
  assert.equal(ratio(parseHex('#000'), parseHex('#fff')).toFixed(2), '21.00');
  assert.equal(ratio(parseHex('#777777'), parseHex('#ffffff')).toFixed(2), '4.48');
});

test('both real presets pass every contrast pair in both schemes, every variant', () => {
  const catalogue = loadCatalogue();
  assert.deepEqual(
    expandPresets(catalogue).map((t) => t.id),
    ['neutral', 'fieldnotes'],
  );
  assert.deepEqual(refusals(catalogue), []);
});

test('the catalogue lists the non-text pairs on every surface', () => {
  const { contrastPairs } = loadCatalogue();
  for (const fg of ['border-strong', 'control-border', 'focus', 'accent']) {
    const pair = contrastPairs.find((p) => p.fg === fg);
    assert.equal(pair?.min, 3, fg);
    assert.deepEqual(pair.on, ['surface', 'surface-sunk', 'surface-raised', 'surface-overlay']);
  }
});

test('a preset below the minimum is refused, naming the pair, the scheme and the ratio', () => {
  const catalogue = loadCatalogue();
  catalogue.themes.push(...parseJsonc(readFileSync(fixtureFile, 'utf8')).themes);
  const lines = refusals(catalogue);
  assert.ok(lines.length >= 3, lines.join('\n'));
  assert.ok(lines.every((l) => l.includes('preset low-contrast')), lines.join('\n'));
  assert.ok(lines.some((l) => /border-strong on surface \(light scheme, .*\) is 1\.\d\d:1, minimum 3:1/.test(l)), lines.join('\n'));
  assert.ok(lines.some((l) => /control-border on surface-raised \(light scheme/.test(l)), lines.join('\n'));
  assert.ok(lines.some((l) => /text-muted on surface \(dark scheme.*minimum 4\.5:1/.test(l)), lines.join('\n'));
  assert.throws(() => filesFor(catalogue), /border-strong on surface/);
});

test('an accepted shortfall that is no longer needed, or that sinks under its floor, is refused', () => {
  const catalogue = loadCatalogue();
  const fieldnotes = catalogue.themes.find((t) => t.id === 'fieldnotes');
  fieldnotes.acceptedShortfalls[0].floor = 2.99;
  assert.ok(refusals(catalogue).some((l) => /below its accepted floor/.test(l)));
  fieldnotes.acceptedShortfalls = [];
  assert.ok(refusals(catalogue).some((l) => /border-strong on surface/.test(l)));
});

test('a preset that omits a colour token is refused', () => {
  const catalogue = loadCatalogue();
  delete catalogue.themes[0].light.focus;
  assert.ok(refusals(catalogue).some((l) => /colour token focus has no light value/.test(l)));
});

test('generate writes the committed stylesheets, and a second run changes nothing', () => {
  const dir = mkdtempSync(join(tmpdir(), 'ui-tokens-'));
  made.push(dir);
  const first = writeTokens({ dir });
  assert.equal(first.length, 2);
  assert.deepEqual(writeTokens({ dir }), []);
  for (const id of ['neutral', 'fieldnotes']) {
    assert.equal(readFileSync(join(dir, `tokens-${id}.css`), 'utf8'), readFileSync(join(outDir, `tokens-${id}.css`), 'utf8'), id);
  }
});

test('data-theme selects the neutral preset, and fieldnotes is the default when none is set', () => {
  const catalogue = loadCatalogue();
  const [neutral, fieldnotes] = expandPresets(catalogue).map((t) => renderTheme(catalogue, t));
  assert.match(fieldnotes, /:root:where\(:not\(\[data-theme\]\), \[data-theme='fieldnotes'\]\) \{/);
  assert.match(neutral, /:root:where\(\[data-theme='neutral'\]\) \{/);
  assert.doesNotMatch(neutral, /:not\(\[data-theme\]\)/);
  for (const accent of ['clay', 'ink', 'ochre', 'plum']) assert.match(fieldnotes, new RegExp(`\\[data-accent='${accent}'\\]`));
  for (const paper of ['stone', 'chalk']) assert.match(fieldnotes, new RegExp(`\\[data-paper='${paper}'\\]`));
  assert.match(fieldnotes, /--font-display: 'Fraunces Variable'/);
});

test('the neutral preset uses the system font stack and names no font file or web font', () => {
  const catalogue = loadCatalogue();
  const neutral = expandPresets(catalogue)[0];
  for (const id of ['font-display', 'font-text', 'font-ui']) {
    assert.match(neutral.shared[id], /^system-ui, /, id);
    assert.doesNotMatch(neutral.shared[id], /Fraunces|Newsreader|Instrument|Variable/, id);
  }
  const css = renderTheme(catalogue, neutral);
  assert.doesNotMatch(css, /@font-face|url\(|@import|Variable/);
});

test('the generated stylesheets write no physical (left, right, top, bottom) properties', () => {
  const css = Object.values(filesFor(loadCatalogue())).join('\n');
  assert.doesNotMatch(css, /(margin|padding|border)-(left|right|top|bottom)|(^|\s)(left|right|top|bottom|width|height)\s*:/m);
});

test('the style literal check finds colours and font families and passes tokens', () => {
  const css = [
    '.a { color: #fff; }',
    '.b { background: rgb(0 0 0 / 0.5); }',
    '.c { border: 1px solid red; }',
    '.d { font-family: Georgia, serif; }',
    '.e { color: var(--text); background: transparent; text-decoration-color: currentColor; }',
    '.f { font-family: var(--font-ui); }',
    '/* color: #123456; */',
    '#main { color: var(--text); }',
  ].join('\n');
  const found = findLiterals(css);
  assert.deepEqual(found.map((f) => [f.line, f.kind]), [[1, 'colour'], [2, 'colour'], [3, 'colour'], [4, 'font family']]);
});

test('no stylesheet in web/src/styles names a literal colour or font outside the generated tokens', () => {
  assert.deepEqual(checkStyles(), []);
});
