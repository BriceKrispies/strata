// Regenerate everything specc writes from the specification:
//
//   npm run generate
//
//   1. crates/model/src/lib.rs, crates/model-core/src/lib.rs and the crates by area of the model
//      (crates/model/<area>, workspace members): if one is missing, a placeholder is written
//      first. Cargo cannot load the workspace while a member has no target, and specc runs through
//      the workspace, so a clean checkout (or one where a src directory was deleted) could not
//      regenerate itself.
//   2. The backend model, from specifications/spec.jsonc: the pure crate crates/model-core (the
//      declared types that reach no runtime type, which the kernel can depend on, with the function
//      bodies and no example tests: Exegra evaluates every example) and crates/model, a facade that re-exports
//      the crates by area in crates/model/<area>, which depend on the pure crate.
//   3. The web modules: web/src/generated, from the same specification.
//   4. The browser journeys: tests/e2e/generated, from the same specification's uiScenarios,
//      written against the stack on http://127.0.0.1:4200 (STRATA_E2E_BASE_URL moves them).
//   5. The step fixtures: tests/e2e/step-checks, from tests/e2e/step-fixtures/spec.jsonc.
//   6. The capability catalogue: specifications/catalogue.md, from the same specification.
//   7. The contract conformance suite: crates/bff/tests/conformance/generated, from the same
//      specification's shipped contracts with a bffRoute (WG-5182).
//   8. Its fixtures: crates/bff/tests/conformance/fixtures/generated, from
//      crates/bff/tests/conformance/fixtures/spec.jsonc.
//   9. The deployment files: deploy/compose.yaml, deploy/kubernetes, deploy/monitoring, from the
//      specification and specifications/deploy.jsonc.
//
// The headers of crates/model, crates/model-core, web/src/generated and specifications/catalogue.md carry
// the specification's semantic hash, which Exegra reports (see spec-hash.mjs). The journeys, step
// fixtures and conformance suites (steps 4, 5, 7 and 8) carry no hash; the gate's byte comparison
// catches their drift.
//
// Product specifications in specifications/products (WG-4989) are passed to steps 2 to 4 with
// --product: specc checks them against the platform and refuses a bad one, and generates nothing
// from them (they are tenant metadata), so every file written is the platform's alone.
//
// The first failing step stops the run. Cargo runs with two jobs.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { productArgs, semanticHashDetail } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
const libs = [join(root, 'crates', 'model', 'src', 'lib.rs'), join(root, 'crates', 'model-core', 'src', 'lib.rs')];
// The specification's semantic hash from Exegra (EXEGRA_HOME, default ../exegra; EXEGRA_PROJECT,
// default strata). Every generated file's header records it, so a file made from another version
// of the specification is visibly stale and the gate fails on it.
const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
const { hash: specHash, reason } = semanticHashDetail(exegraHome, process.env.EXEGRA_PROJECT ?? 'strata');
if (!specHash) {
  console.error(`regenerate: Exegra gave no semantic hash, and the headers need it: ${reason}`);
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

// The crates by area of the model (crates/model/<area>, WG-5371) are workspace members too: a
// missing one gets a placeholder manifest as well as a placeholder lib.rs.
const manifest = readFileSync(join(root, 'Cargo.toml'), 'utf8');
const areaDirs = [...manifest.matchAll(/"(crates\/model\/[^"]+)"/g)].map((m) => m[1]);
for (const dir of areaDirs) {
  const toml = join(root, dir, 'Cargo.toml');
  libs.push(join(root, dir, 'src', 'lib.rs'));
  if (existsSync(toml)) continue;
  mkdirSync(dirname(toml), { recursive: true });
  const name = `strata-model-${dir.split('/').pop()}`;
  writeFileSync(toml, `# Placeholder so that Cargo can load the workspace. specc replaces this file.\n[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n`);
  console.log(`wrote placeholder ${toml}`);
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
specc('generate the contract conformance suite (crates/bff/tests/conformance/generated)', [
  'conformance', '--spec', 'specifications/spec.jsonc', '--out', 'crates/bff/tests/conformance/generated', ...products,
]);
specc('generate the conformance fixtures (crates/bff/tests/conformance/fixtures/generated)', [
  'conformance', '--spec', 'crates/bff/tests/conformance/fixtures/spec.jsonc', '--out', 'crates/bff/tests/conformance/fixtures/generated',
  '--expect-fail-prefix', 'violates-',
]);

specc('generate the deployment files (deploy)', [
  'deploy', '--spec', 'specifications/spec.jsonc', '--out', 'deploy',
]);

console.log('\nregenerate: done');
