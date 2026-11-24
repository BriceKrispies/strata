// Tests for scripts/ui-tokens.mjs and scripts/check-style-literals.mjs, with the No-pills check of
// D-DS-31 (DS-022, DS-023) and its failing fixture in scripts/fixtures/style-literals.
// Run with: node --test scripts/ui-tokens.test.mjs
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';
import { checkStyles, findLiterals, findPills } from './check-style-literals.mjs';
import { parseJsonc } from './jsonc.mjs';
import { filesFor, loadCatalogue, outDir, pillRefusals, ratio, parseHex, refusals, renderTheme, expandPresets, writeTokens } from './ui-tokens.mjs';

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

test('no stylesheet in web/src/styles names a literal colour or font outside the generated tokens, or draws a pill', () => {
  assert.deepEqual(checkStyles(), []);
});

/// The failing fixture of the No-pills check (D-DS-31): every refused rule is found, no allowed one.
test('the no-pills check refuses pill radii and capsule class names, and passes avatars, dots and tokens', () => {
  const dir = fileURLToPath(new URL('./fixtures/style-literals/', import.meta.url));
  const problems = checkStyles(dir);
  const found = (file, line) => problems.some((p) => p.includes(`fixtures/style-literals/${file}:${line}:`));
  for (const line of [8, 13, 18, 23, 27, 28, 32, 40]) assert.ok(found('pills.css', line), `pills.css:${line} refused\n${problems.join('\n')}`);
  assert.ok(found('ds-fixture.css', 7), `a non-token radius in a design-system stylesheet is refused\n${problems.join('\n')}`);
  assert.equal(problems.length, 9, problems.join('\n'));
  assert.ok(problems.every((p) => p.endsWith('the design system has no pills (D-DS-31)')));
});

test('findPills reads each declaration with the selector of its block', () => {
  const css = [
    '@layer components {',
    '  .ds-avatar { border-radius: 50%; }',
    '  .swatch__dot { border-radius: 50%; }',
    '  .ring { border-radius: 50%; }',
    '  @media (min-width: 1px) { .x { border-end-end-radius: 999px; } }',
    '  .ds-chips { color: inherit; }',
    '  .ds-pill--new { color: inherit; }',
    '}',
  ].join('\n');
  assert.deepEqual(findPills(css).map((f) => [f.line, f.selector, f.kind]), [
    [4, '.ring', 'pill radius (50% on something that is not an avatar or a dot)'],
    [5, '.x', 'pill radius'],
    [7, '.ds-pill--new', 'capsule class'],
  ]);
  assert.deepEqual(findPills('.a { border-radius: 2rem; }'), [], 'outside the design system a fixed radius is not refused');
  assert.equal(findPills('.a { border-radius: 2rem; }', { designSystem: true }).length, 1);
  assert.deepEqual(findPills('.a { border-radius: var(--radius-1) 0; }', { designSystem: true }), []);
});

test('the catalogue has no pill token, and a pill token or a capsule primitive is refused', () => {
  const catalogue = loadCatalogue();
  assert.ok(!catalogue.tokens.some((t) => /pill/.test(t.id)), 'radius-pill is gone');
  assert.deepEqual(pillRefusals(catalogue), []);
  catalogue.tokens.push({ id: 'radius-pill', group: 'Shape', type: 'length' });
  catalogue.primitives = [{ id: 'badge' }, { id: 'button' }, 'chip', { id: 'tag-capsule' }];
  const lines = pillRefusals(catalogue);
  assert.equal(lines.length, 4, lines.join('\n'));
  assert.ok(lines.some((l) => /token radius-pill makes a pill/.test(l)));
  for (const id of ['badge', 'chip', 'tag-capsule']) assert.ok(lines.some((l) => l.includes(`primitive ${id} is a capsule label`)), id);
  assert.ok(refusals(catalogue).some((l) => /radius-pill/.test(l)), 'the generator refuses it');
});
