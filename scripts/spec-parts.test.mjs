import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { SpecPartsError, readSpec, readSpecValue } from './spec-parts.mjs';
import { checkAgainst, itemPart, splitFile, splitText } from './split-spec.mjs';

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

test('the area rule puts an area item of any section in the area part, and nothing else', () => {
  assert.equal(itemPart('decisions', { id: 'd-fe-1' }), 'frontend.jsonc');
  assert.equal(itemPart('copies', { id: 'copy-browser-view' }), 'frontend.jsonc');
  assert.equal(itemPart('claims', { id: 'fe-001', kind: 'requirement' }), 'frontend.jsonc');
  // negatives: a prefix in the middle of an id, a near miss, and the consumer rule for the rest
  assert.equal(itemPart('decisions', { id: 'd-8250' }), 'intent.jsonc');
  assert.equal(itemPart('claims', { id: 'safe-001', kind: 'requirement' }), 'requirements.jsonc');
  assert.equal(itemPart('claims', { id: 'm-fe-1', kind: 'invariant' }), 'behaviour.jsonc');
  assert.equal(itemPart('copies', { id: 'copy-browserish' }), 'boundary.jsonc');
});

const areaMono = [
  '// h',
  '{',
  '  "exegra": "0.1",',
  '  "decisions": [',
  '    {"id":"d-1"},',
  '    {"id":"d-fe-1"},',
  '    {"id":"d-2"}',
  '  ],',
  '  "copies": [',
  '    {"id":"copy-browser-view"}',
  '  ],',
  '  "claims": [',
  '    {"id":"fe-001","kind":"requirement"},',
  '    {"id":"r-1","kind":"requirement"},',
  '    {"id":"m-1","kind":"invariant"}',
  '  ]',
  '}',
  '',
].join('\n');

test('an area split gives the same parts twice, reassembles, and keeps each part in the monolith order', () => {
  const a = splitText(areaMono, '// h\n');
  const b = splitText(areaMono, '// h\n');
  assert.deepEqual([...a.parts], [...b.parts]);
  assert.match(a.parts.get('frontend.jsonc'), /d-fe-1/);
  assert.match(a.parts.get('frontend.jsonc'), /copy-browser-view/);
  assert.match(a.parts.get('frontend.jsonc'), /fe-001/);
  assert.doesNotMatch(a.parts.get('intent.jsonc'), /d-fe-1/);
  // the consumer part keeps its section for copies, now empty
  assert.match(a.parts.get('boundary.jsonc'), /"copies": \[\]/);
  const dir = mkdtempSync(join(tmpdir(), 'spec-area-'));
  writeFileSync(join(dir, 'spec.jsonc'), a.manifest);
  for (const [name, text] of a.parts) writeFileSync(join(dir, name), text);
  writeFileSync(join(dir, 'mono.jsonc'), areaMono);
  assert.deepEqual(checkAgainst(join(dir, 'spec.jsonc'), join(dir, 'mono.jsonc')), []);
  assert.deepEqual(splitFile(join(dir, 'spec.jsonc'), { dryRun: true }).written, []);
});

test('an area item placed in a consumer part is reported by a dry run, which writes nothing', () => {
  const a = splitText(areaMono, '// h\n');
  const dir = mkdtempSync(join(tmpdir(), 'spec-area-'));
  writeFileSync(join(dir, 'spec.jsonc'), a.manifest);
  for (const [name, text] of a.parts) writeFileSync(join(dir, name), text);
  // move the area decision into the consumer part, as Exegra's apply places a new entity
  const intent = a.parts.get('intent.jsonc').replace('{"id":"d-1"}', '{"id":"d-1"},\n    {"id":"d-fe-1"}');
  const frontend = a.parts.get('frontend.jsonc').replace(/ {4}\{"id":"d-fe-1"\}\n/, '').replace('"decisions": [\n  ]', '"decisions": []');
  writeFileSync(join(dir, 'intent.jsonc'), intent);
  writeFileSync(join(dir, 'frontend.jsonc'), frontend);
  const before = readSpec(join(dir, 'spec.jsonc')).text;
  const { written } = splitFile(join(dir, 'spec.jsonc'), { dryRun: true });
  assert.ok(written.some((f) => f.endsWith('intent.jsonc')));
  assert.equal(readSpec(join(dir, 'spec.jsonc')).text, before);
});

test('the same area id in a consumer part and the area part is refused, naming both files', () => {
  const spec = project({
    'spec.jsonc': manifest('intent.jsonc', 'frontend.jsonc'),
    'intent.jsonc': '{ "decisions": [ {"id":"d-fe-1"} ] }',
    'frontend.jsonc': '{ "decisions": [ {"id":"d-fe-1"} ] }',
  });
  assert.throws(() => readSpec(spec), (e) => e.code === 'duplicate-id' && /intent\.jsonc/.test(e.message) && /frontend\.jsonc/.test(e.message));
});
