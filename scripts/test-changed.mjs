// Checks scoped to what a change touches.
//
//   node scripts/test-changed.mjs [--base <ref>] [--slow] [--dry-run] [--files <path>...]
//
// The files are those `git diff --name-only <base>` names (the working tree against the base,
// default `integration` when it exists, else `main`) plus untracked files, or the ones given with
// `--files`. From them it runs:
//
//   - the unit tests and the routine and full-tier test binaries of every package that owns a changed
//     file or depends on one that does (`scripts/test-tiers.json`; the very slow binaries, those marked
//     `slow`, only with `--slow`);
//   - the web tests and the type check when `web/` changed, the script tests when `scripts/` changed;
//   - Exegra validation and the specc freshness checks when `specifications/` changed.
//
// A change to the root manifests or the lock file selects every package. Selection reads manifests
// only (scripts/test-select.mjs). It does not run the browser suites, `lake build`, `cargo clippy`
// or the audits; those stay with the gate (`npm run spec`).
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { cargoArgs, readWorkspace, scopedTargets, selectScope } from './test-select.mjs';
import { readTiers } from './test-tiers.mjs';
import { checkoutHash, productArgs } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');

/** The options of an argument list: `{base, slow, dryRun, files}`. */
export function parseArgs(argv) {
  const out = { base: null, slow: false, dryRun: false, files: null };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--slow') out.slow = true;
    else if (a === '--dry-run') out.dryRun = true;
    else if (a === '--base') out.base = argv[++i];
    else if (a === '--files') out.files = argv.slice(i + 1);
    else throw new Error(`unknown argument ${a}`);
    if (a === '--files') break;
  }
  return out;
}

function git(args) {
  const done = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  return done.status === 0 ? done.stdout : null;
}

function changedFiles(base) {
  const ref = base ?? (git(['rev-parse', '--verify', '--quiet', 'integration']) ? 'integration' : 'main');
  const diff = git(['diff', '--name-only', ref]);
  if (diff === null) throw new Error(`git diff against ${ref} failed`);
  const untracked = git(['ls-files', '--others', '--exclude-standard']) ?? '';
  return [...new Set([...diff.split('\n'), ...untracked.split('\n')].map((s) => s.trim()).filter(Boolean))];
}

function run(label, command, args, env = {}) {
  console.log(`\n== ${label}: ${command} ${args.join(' ')}`);
  const done = spawnSync(command, args, { cwd: root, stdio: 'inherit', env: { ...process.env, ...env } });
  return done.status === 0;
}

function main(argv) {
  const options = parseArgs(argv);
  const files = options.files ?? changedFiles(options.base);
  const graph = readWorkspace(root);
  const tiers = readTiers();
  const scope = selectScope(files, graph);
  const targets = scopedTargets(scope, tiers, { slow: options.slow });
  const env = { CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? '2', RUST_TEST_THREADS: process.env.RUST_TEST_THREADS ?? '2' };

  console.log(`${files.length} changed file(s); ${scope.all ? 'every package' : `${scope.packages.length} package(s)`} affected`);
  for (const [pkg, why] of Object.entries(scope.reasons)) console.log(`  ${pkg}: ${why.slice(0, 3).join(', ')}${why.length > 3 ? `, and ${why.length - 3} more` : ''}`);
  if (scope.other.length) console.log(`  not owned by a package or a known reader (nothing selected for them): ${scope.other.slice(0, 5).join(', ')}`);
  for (const t of targets.skipped) console.log(`  skipped (slow, ${t.seconds} s): ${t.package}:${t.target}; add --slow to run it`);

  const steps = [];
  if (scope.packages.length) steps.push(['cargo test', 'cargo', cargoArgs(scope, targets), env]);
  if (scope.web) {
    steps.push(['web tests', process.execPath, ['--test', 'web/test/kernel.test.js', 'web/test/client.test.js', 'web/test/tenant-ui.test.js'], {}]);
    steps.push(['type check', process.execPath, [join(root, 'node_modules', 'typescript', 'bin', 'tsc'), '-p', 'web/jsconfig.json'], {}]);
  }
  if (scope.scripts) {
    const tests = readdirSync(join(root, 'scripts')).filter((f) => f.endsWith('.test.mjs')).sort().map((f) => `scripts/${f}`);
    steps.push(['script tests', process.execPath, ['--test', ...tests], {}]);
  }
  if (scope.specification) {
    const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
    steps.push(['exegra validate', process.execPath, [join(exegraHome, 'cli', 'exegra.js'), 'validate', '--spec', 'specifications/spec.jsonc', ...productArgs(root)], {}]);
    const hash = existsSync(join(exegraHome, 'cli/exegra.js')) ? checkoutHash(exegraHome, root, process.env.EXEGRA_PROJECT ?? 'strata').hash : null;
    const products = productArgs(root);
    const specc = (name, extra) => ['cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--', name, ...extra], env];
    const hashArgs = hash ? ['--spec-hash', hash] : [];
    for (const [name, extra] of [
      ['check', ['--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime', '--core-out', 'crates/model-core', '--core-crate-name', 'strata-model-core', ...hashArgs, ...products]],
      ['check-web', ['--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated', ...hashArgs, ...products]],
      ['check-e2e', ['--spec', 'specifications/spec.jsonc', '--out', 'tests/e2e/generated', '--origin', 'http://127.0.0.1:4200', ...products]],
      ['check-catalogue', ['--spec', 'specifications/spec.jsonc', '--out', 'specifications', ...hashArgs]],
    ]) {
      const [command, args, stepEnv] = specc(name, extra);
      steps.push([`specc ${name}`, command, args, stepEnv]);
    }
  }
  if (steps.length === 0) {
    console.log('nothing to run for these files');
    return 0;
  }
  if (options.dryRun) {
    for (const [label, command, args] of steps) console.log(`would run ${label}: ${command} ${args.join(' ')}`);
    return 0;
  }
  const results = steps.map(([label, command, args, stepEnv]) => [label, run(label, command, args, stepEnv)]);
  console.log('');
  for (const [label, ok] of results) console.log(`${ok ? 'pass' : 'FAIL'}  ${label}`);
  console.log('not run here: browser suites, lake build, clippy, audits (the gate, npm run spec, runs them)');
  return results.every(([, ok]) => ok) ? 0 : 1;
}

import { pathToFileURL } from 'node:url';
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) process.exitCode = main(process.argv.slice(2));
