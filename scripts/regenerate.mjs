// Regenerate everything specc writes from the specification:
//
//   npm run generate
//
//   1. crates/model/src/lib.rs: if it is missing, a placeholder is written first. Cargo cannot load
//      the workspace while a member has no target, and specc runs through the workspace, so a
//      clean checkout (or one where crates/model/src was deleted) could not regenerate itself.
//   2. The backend model: crates/model, from specifications/spec.jsonc.
//   3. The web modules: web/src/generated, from the same specification.
//
// The headers carry the specification's semantic hash, which Exegra reports (see spec-hash.mjs).
//
// The first failing step stops the run. Cargo runs with two jobs.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { semanticHash } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
const modelLib = join(root, 'crates', 'model', 'src', 'lib.rs');
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

if (!existsSync(modelLib)) {
  mkdirSync(join(root, 'crates', 'model', 'src'), { recursive: true });
  writeFileSync(modelLib, '// Placeholder so that Cargo can load the workspace. specc replaces this file.\n');
  console.log(`wrote placeholder ${modelLib}`);
}

specc('generate the backend model (crates/model)', [
  'generate', '--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime', '--spec-hash', specHash,
]);
specc('generate the web modules (web/src/generated)', [
  'web', '--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated', '--spec-hash', specHash,
]);

console.log('\nregenerate: done');
