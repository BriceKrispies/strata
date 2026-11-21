import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { SpecPartsError, readSpec, readSpecValue } from './spec-parts.mjs';
import { checkAgainst, splitText } from './split-spec.mjs';

function project(files) {
  const dir = mkdtempSync(join(tmpdir(), 'spec-parts-'));
  for (const [name, text] of Object.entries(files)) writeFileSync(join(dir, name), text);
  return join(dir, 'spec.jsonc');
}

const manifest = (...names) => `// header\n{ "specParts": ${JSON.stringify(names)} }\n`;

test('parts assemble in manifest order and array sections concatenate', () => {
  const spec = project({
    'spec.jsonc': manifest('a.jsonc', 'b.jsonc'),
    'a.jsonc': '{ "system": {"id": "s"}, "claims": [ {"id":"m-1"} ] }',
    'b.jsonc': '// c\n{ "claims": [ {"id":"r-1"}, {"id":"r-2"} ] }',
  });
  const value = readSpecValue(spec);
  assert.deepEqual(value.claims.map((c) => c.id), ['m-1', 'r-1', 'r-2']);
  assert.deepEqual(value.system, { id: 's' });
});

test('a missing part is refused', () => {
  const spec = project({ 'spec.jsonc': manifest('a.jsonc', 'gone.jsonc'), 'a.jsonc': '{}' });
  assert.throws(() => readSpec(spec), (e) => e instanceof SpecPartsError && e.code === 'missing-part' && /gone\.jsonc/.test(e.message));
});

test('a section that is not an array in two parts is refused, naming both files', () => {
  const spec = project({
    'spec.jsonc': manifest('a.jsonc', 'b.jsonc'),
    'a.jsonc': '{ "system": {"id": "s"} }',
    'b.jsonc': '{ "system": {"id": "t"} }',
  });
  assert.throws(() => readSpec(spec), (e) => e.code === 'duplicate-section' && /a\.jsonc/.test(e.message) && /b\.jsonc/.test(e.message));
});

test('an id repeated across parts is refused, naming both files', () => {
  const spec = project({
    'spec.jsonc': manifest('a.jsonc', 'b.jsonc'),
    'a.jsonc': '{ "claims": [ {"id":"c-1"} ] }',
    'b.jsonc': '{ "claims": [ {"id":"c-2"}, {"id":"c-1"} ] }',
  });
  assert.throws(() => readSpec(spec), (e) => e.code === 'duplicate-id' && /c-1/.test(e.message) && /a\.jsonc/.test(e.message) && /b\.jsonc/.test(e.message));
});

test('splitting a one-file specification gives the same parts twice and reassembles to it', () => {
  const mono = '// h\n{\n  "exegra": "0.1",\n  "claims": [\n    {"id":"r-1","kind":"requirement"},\n    {"id":"m-1","kind":"invariant"},\n    {"id":"r-2","kind":"requirement"}\n  ],\n  "types": [\n    {"id":"t"}\n  ]\n}\n';
  const a = splitText(mono, '// h\n');
  const b = splitText(mono, '// h\n');
  assert.deepEqual([...a.parts], [...b.parts]);
  const dir = mkdtempSync(join(tmpdir(), 'spec-split-'));
  writeFileSync(join(dir, 'spec.jsonc'), a.manifest);
  for (const [name, text] of a.parts) writeFileSync(join(dir, name), text);
  writeFileSync(join(dir, 'mono.jsonc'), mono);
  assert.deepEqual(checkAgainst(join(dir, 'spec.jsonc'), join(dir, 'mono.jsonc')), []);
});
