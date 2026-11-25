// Tests for the checkout's semantic hash (scripts/spec-hash.mjs): the hash follows the checkout it is
// asked about, and a specification Exegra reads from another checkout is named as a mismatch.
//
// The Exegra test needs an Exegra checkout (EXEGRA_HOME, default ../exegra). Without one it is
// reported as skipped with the reason, never passed; the other tests run without Exegra.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { checkoutHash, checkoutSpecPath, checkoutSpecs, specMismatch } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
const exegraSkip = existsSync(join(exegraHome, 'cli', 'exegra.js')) ? false : `no Exegra checkout at ${exegraHome} (set EXEGRA_HOME); the hash cannot be checked`;

// Each checkout links itself, so an override from the environment would hide what is tested.
delete process.env.EXEGRA_SPECS;

const dirs = [];
test.after(() => {
  for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
});

/**
 * A checkout-like directory: this checkout's specification parts, copied, with `edit` applied to
 * intent.jsonc (the system's version, which Exegra's hash covers).
 */
function makeCheckout(edit = null) {
  const dir = mkdtempSync(join(tmpdir(), 'strata-spec-hash-'));
  dirs.push(dir);
  const specs = join(dir, 'specifications');
  mkdirSync(specs, { recursive: true });
  for (const name of readdirSync(join(root, 'specifications'))) {
    if (name.endsWith('.jsonc')) copyFileSync(join(root, 'specifications', name), join(specs, name));
  }
  if (edit) {
    const file = join(specs, 'intent.jsonc');
    const text = readFileSync(file, 'utf8');
    assert.ok(text.includes(edit.from), `intent.jsonc has no ${edit.from}; the test edit is out of date`);
    writeFileSync(file, text.replace(edit.from, edit.to));
  }
  return dir;
}

test('the checkout links its own specification into a specs directory Exegra reads', () => {
  const dir = makeCheckout();
  const specs = checkoutSpecs(dir, 'strata');
  assert.equal(specs, join(dir, 'var', 'exegra-specs'));
  const link = JSON.parse(readFileSync(join(specs, 'strata', 'project.jsonc'), 'utf8'));
  assert.equal(link.spec, '../../../specifications/spec.jsonc');
  assert.equal(resolve(specs, 'strata', link.spec), checkoutSpecPath(dir));
});

test('a mismatch names both specification paths, and none when they are the same file', () => {
  const here = makeCheckout();
  const there = makeCheckout();
  assert.equal(specMismatch(checkoutSpecPath(here), checkoutSpecPath(here)), null);
  const message = specMismatch(checkoutSpecPath(here), checkoutSpecPath(there));
  assert.ok(message?.includes(checkoutSpecPath(there)), message);
  assert.ok(message?.includes(checkoutSpecPath(here)), message);
  assert.match(specMismatch(checkoutSpecPath(here), null) ?? '', /no specification/);
});

test('the semantic hash follows the checkout it is asked about', { skip: exegraSkip }, () => {
  const same = makeCheckout();
  const edited = makeCheckout({ from: '"version": "0.2-draft"', to: '"version": "0.2-probe"' });
  const first = checkoutHash(exegraHome, same, 'strata');
  const second = checkoutHash(exegraHome, edited, 'strata');
  assert.ok(first.hash, `no hash for the copy: ${first.reason}`);
  assert.ok(second.hash, `no hash for the edited copy: ${second.reason}`);
  assert.notEqual(second.hash, first.hash, 'an edited specification must hash differently');
  assert.equal(checkoutHash(exegraHome, same, 'strata').hash, first.hash, 'asked again, the copy keeps its hash');
  assert.equal(first.mismatch, null, first.mismatch ?? '');
  assert.equal(first.read, checkoutSpecPath(same), 'Exegra reads the specification of the checkout asked about');
});
