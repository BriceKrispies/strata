// Regenerate everything specc writes from the specification:
//
//   npm run generate
//
//   1. crates/model/src/lib.rs and crates/model-core/src/lib.rs: if one is missing, a placeholder is
//      written first. Cargo cannot load the workspace while a member has no target, and specc runs
//      through the workspace, so a clean checkout (or one where a src directory was deleted) could
//      not regenerate itself.
//   2. The backend model, from specifications/spec.jsonc, in two crates: the pure crate
//      crates/model-core (the declared types that reach no runtime type, which the kernel can
//      depend on) and crates/model, which depends on it and re-exports its types.
//   3. The web modules: web/src/generated, from the same specification.
//   4. The browser journeys: tests/e2e/generated, from the same specification's uiScenarios,
//      written against the stack on http://127.0.0.1:4200 (STRATA_E2E_BASE_URL moves them).
//   5. The step fixtures: tests/e2e/step-checks, from tests/e2e/step-fixtures/spec.jsonc.
//   6. The capability catalogue: specifications/catalogue.md, from the same specification.
//
// The headers carry the specification's semantic hash, which Exegra reports (see spec-hash.mjs).
//
// Product specifications in specifications/products (WG-4989) are passed to steps 2 to 4 with
// --product: specc checks them against the platform and refuses a bad one, and generates nothing
// from them (they are tenant metadata), so every file written is the platform's alone.
//
// The first failing step stops the run. Cargo runs with two jobs.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { productArgs, semanticHash } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
const libs = [join(root, 'crates', 'model', 'src', 'lib.rs'), join(root, 'crates', 'model-core', 'src', 'lib.rs')];
// The specification's semantic hash from Exegra (EXEGRA_HOME, default ../exegra; EXEGRA_PROJECT,
// default strata). Every generated file's header records it, so a file made from another version
// of the specification is visibly stale and the gate fails on it.
const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
const specHash = semanticHash(exegraHome, process.env.EXEGRA_PROJECT ?? 'strata');
if (!specHash) {
  console.error('regenerate: Exegra gave no semantic hash (set EXEGRA_HOME and EXEGRA_PROJECT); the headers need it');
  process.exit(1);
}
console.log(`specification semantic hash ${specHash}`);
const env = { ...process.env, CARGO_BUILD_JOBS: '2' };
const products = productArgs(root);

// Runs one specc command through cargo. Output goes straight to the terminal.
function specc(name, specArgs) {
  console.log(`\n== ${name}`);
  const done = spawnSync('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--', ...specArgs], {
    cwd: root,
    env,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });
  if (done.error) {
    console.error(`\nregenerate: ${name} did not start: ${done.error.message}`);
    process.exit(1);
  }
  if (done.signal) {
    console.error(`\nregenerate: ${name} was killed by ${done.signal}`);
    process.exit(1);
  }
  if (done.status !== 0) {
    console.error(`\nregenerate: ${name} failed with exit ${done.status}; nothing after it was regenerated`);
    process.exit(done.status ?? 1);
  }
}

for (const lib of libs) {
  if (existsSync(lib)) continue;
  mkdirSync(dirname(lib), { recursive: true });
  writeFileSync(lib, '// Placeholder so that Cargo can load the workspace. specc replaces this file.\n');
  console.log(`wrote placeholder ${lib}`);
}

specc('generate the backend model (crates/model and crates/model-core)', [
  'generate', '--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime',
  '--core-out', 'crates/model-core', '--core-crate-name', 'strata-model-core', '--spec-hash', specHash, ...products,
]);
specc('generate the web modules (web/src/generated)', [
  'web', '--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated', '--spec-hash', specHash, ...products,
]);
specc('generate the browser journeys (tests/e2e/generated)', [
  'e2e', '--spec', 'specifications/spec.jsonc', '--out', 'tests/e2e/generated', '--origin', 'http://127.0.0.1:4200', ...products,
]);
specc('generate the step fixtures (tests/e2e/step-checks)', [
  'e2e', '--spec', 'tests/e2e/step-fixtures/spec.jsonc', '--out', 'tests/e2e/step-checks', '--expect-fail-prefix', 'violates-',
]);
specc('generate the capability catalogue (specifications/catalogue.md)', [
  'catalogue', '--spec', 'specifications/spec.jsonc', '--out', 'specifications', '--spec-hash', specHash,
]);

console.log('\nregenerate: done');
