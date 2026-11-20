// Tests for scripts/test-select.mjs and scripts/test-changed.mjs. Run with: node --test scripts/test-select.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseArgs } from './test-changed.mjs';
import { cargoArgs, dependants, dependencySection, memberDirs, ownerOf, parseManifest, readWorkspace, scopedTargets, selectScope, testTargets } from './test-select.mjs';

const MANIFESTS = {
  'Cargo.toml': '[workspace]\nmembers = [\n  "crates/kernel", # the pure core\n  "crates/runtime",\n  "crates/model",\n  "crates/backend",\n]\n',
  'crates/kernel/Cargo.toml': '[package]\nname = "strata-kernel"\n\n[dependencies]\nserde.workspace = true\n\n[dev-dependencies]\nstrata-kernel = { path = ".", features = ["testkit"] }\n',
  'crates/runtime/Cargo.toml': '[package]\nname = "strata-runtime"\n\n[dependencies]\nstrata-kernel.workspace = true\ntokio.workspace = true\n',
  'crates/model/Cargo.toml': '[package]\nname = "strata-model"\n\n[dependencies]\nstrata-runtime = { path = "../runtime" }\n',
  'crates/backend/Cargo.toml': [
    '[package]',
    'name = "strata-backend"',
    '',
    '[dependencies.strata-model]',
    'path = "../model"',
    '',
    '[target.\'cfg(windows)\'.dev-dependencies]',
    'argon2.workspace = true',
    '',
    '[[test]]',
    'name = "service"',
    'required-features = ["development"]',
    '',
  ].join('\n'),
};
const FILES = {
  'crates/kernel/tests': [['behaviour.rs'], ['limits.rs']],
  'crates/backend/tests': [['service.rs'], ['spec_agreement.rs'], ['big', 'dir'], ['support', 'dir']],
  'crates/backend/tests/big': [['main.rs']],
  'crates/backend/tests/support': [['mod.rs']],
};
const relative = (path) => {
  const p = path.replace(/\\/g, '/');
  return p.includes('crates/') ? p.slice(p.indexOf('crates/')) : p.replace(/^.*\/(?=Cargo\.toml$)/, '');
};
const read = (path) => {
  const key = relative(path);
  if (!(key in MANIFESTS)) throw new Error(`no manifest ${key}`);
  return MANIFESTS[key];
};
const list = (dir) => (FILES[relative(dir)] ?? []).map(([name, kind]) => ({ name, isFile: kind !== 'dir', isDirectory: kind === 'dir' }));
const graph = () => readWorkspace('/ws', { read, list });

test('a manifest gives its name, every dependency section and its declared tests', () => {
  const p = parseManifest(MANIFESTS['crates/backend/Cargo.toml']);
  assert.equal(p.name, 'strata-backend');
  assert.deepEqual(p.deps.sort(), ['argon2', 'strata-model']);
  assert.deepEqual(p.declaredTests, [{ name: 'service', path: null }]);
  assert.deepEqual(dependencySection('[dependencies.strata-model]'), { name: 'strata-model' });
  assert.equal(dependencySection('[features]'), null);
});

test('workspace members are read past comments', () => {
  assert.deepEqual(memberDirs(MANIFESTS['Cargo.toml']), ['crates/kernel', 'crates/runtime', 'crates/model', 'crates/backend']);
});

test('test targets are the files, the main.rs directories and the declared names, not support modules', () => {
  const g = graph();
  assert.deepEqual(g.packages.find((p) => p.name === 'strata-backend').tests, ['big', 'service', 'spec_agreement']);
  assert.deepEqual(g.packages.find((p) => p.name === 'strata-kernel').tests, ['behaviour', 'limits']);
  assert.deepEqual(testTargets('/nowhere', [{ name: 'only', path: 'x.rs' }], () => []), ['only']);
});

test('a package depends only on workspace packages other than itself', () => {
  const g = graph();
  assert.deepEqual(g.packages.find((p) => p.name === 'strata-kernel').deps, []);
  assert.deepEqual(g.packages.find((p) => p.name === 'strata-runtime').deps, ['strata-kernel']);
  assert.deepEqual(g.packages.find((p) => p.name === 'strata-backend').deps, ['strata-model']);
});

test('dependants are transitive', () => {
  const g = graph();
  assert.deepEqual(dependants(g, ['strata-kernel']), ['strata-backend', 'strata-kernel', 'strata-model', 'strata-runtime']);
  assert.deepEqual(dependants(g, ['strata-model']), ['strata-backend', 'strata-model']);
  assert.deepEqual(dependants(g, ['strata-backend']), ['strata-backend']);
});

test('a changed file belongs to the package with the longest matching directory', () => {
  const g = graph();
  assert.equal(ownerOf('crates/runtime/src/lib.rs', g), 'strata-runtime');
  assert.equal(ownerOf('crates\\runtime\\src\\lib.rs', g), 'strata-runtime');
  assert.equal(ownerOf('crates/runtime-extra/src/lib.rs', g), null);
  assert.equal(ownerOf('docs/development.md', g), null);
});

test('a change selects its package and the packages above it, and says why', () => {
  const s = selectScope(['crates/runtime/src/lib.rs', 'docs/development.md'], graph());
  assert.deepEqual(s.packages, ['strata-backend', 'strata-model', 'strata-runtime']);
  assert.deepEqual(s.reasons['strata-runtime'], ['crates/runtime/src/lib.rs']);
  assert.equal(s.specification, false);
  assert.deepEqual(s.other, []);
});

test('a root manifest or the lock file selects every package', () => {
  assert.equal(selectScope(['Cargo.lock'], graph()).all, true);
  assert.equal(selectScope(['Cargo.toml'], graph()).packages.length, 4);
});

test('a specification change turns on the spec checks and selects the packages that read it', () => {
  const s = selectScope(['specifications/spec.jsonc'], graph());
  assert.equal(s.specification, true);
  assert.ok(s.packages.includes('strata-backend'));
  assert.deepEqual(s.other, []);
});

test('web and scripts changes turn on their checks and select no package', () => {
  const s = selectScope(['web/src/app.js', 'scripts/dev.mjs'], graph());
  assert.deepEqual([s.web, s.scripts, s.packages], [true, true, []]);
});

test('an unowned file outside the known trees is reported, not silently dropped', () => {
  assert.deepEqual(selectScope(['var/x.bin'], graph()).other, ['var/x.bin']);
});

const TIERS = {
  tests: [
    { package: 'strata-kernel', target: 'behaviour', tier: 'routine', seconds: 0.03 },
    { package: 'strata-backend', target: 'service', tier: 'full', seconds: 16, reason: 'r' },
    { package: 'strata-backend', target: 'big', tier: 'full', seconds: 900, slow: true, reason: 'r' },
  ],
};

test('scoped targets leave out the very slow binaries unless asked', () => {
  const scope = selectScope(['crates/backend/src/lib.rs'], graph());
  assert.deepEqual(scopedTargets(scope, TIERS).run.map((t) => t.target), ['service']);
  assert.deepEqual(scopedTargets(scope, TIERS).skipped.map((t) => t.target), ['big']);
  assert.deepEqual(scopedTargets(scope, TIERS, { slow: true }).run.map((t) => t.target).sort(), ['big', 'service']);
});

test('the cargo arguments name the packages, unit tests and test binaries, with the backend feature', () => {
  const scope = selectScope(['crates/kernel/src/lib.rs'], graph());
  const args = cargoArgs(scope, scopedTargets(scope, TIERS));
  assert.equal(args[0], 'test');
  assert.ok(args.includes('--lib') && args.includes('--bins') && args.includes('--no-fail-fast'));
  assert.deepEqual(args.slice(args.indexOf('--features')), ['--features', 'strata-backend/development']);
  // The backend and the specc crate are always selected, so the feature set is the gate's.
  const named = args.filter((_, i) => args[i - 1] === '-p');
  assert.ok(named.includes('strata-backend') && named.includes('strata-spec-compiler') && named.includes('strata-kernel'));
  assert.ok(args.filter((_, i) => args[i - 1] === '--test').includes('behaviour'));
});

test('arguments: base, slow, dry run, and files last', () => {
  assert.deepEqual(parseArgs(['--base', 'main', '--slow', '--dry-run']), { base: 'main', slow: true, dryRun: true, files: null });
  assert.deepEqual(parseArgs(['--files', 'a', 'b']).files, ['a', 'b']);
  assert.throws(() => parseArgs(['--nope']));
});
