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
//
// The first failing step stops the run. Cargo runs with two jobs.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const libs = [join(root, 'crates', 'model', 'src', 'lib.rs'), join(root, 'crates', 'model-core', 'src', 'lib.rs')];
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

for (const lib of libs) {
  if (existsSync(lib)) continue;
  mkdirSync(dirname(lib), { recursive: true });
  writeFileSync(lib, '// Placeholder so that Cargo can load the workspace. specc replaces this file.\n');
  console.log(`wrote placeholder ${lib}`);
}

specc('generate the backend model (crates/model and crates/model-core)', [
  'generate', '--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime',
  '--core-out', 'crates/model-core', '--core-crate-name', 'strata-model-core',
]);
specc('generate the web modules (web/src/generated)', [
  'web', '--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated',
]);

console.log('\nregenerate: done');
