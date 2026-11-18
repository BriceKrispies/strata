// The specification gate: is the specification sound, and does the code answer it?
//
//   node scripts/spec-gate.mjs            run the tests, then every check
//   node scripts/spec-gate.mjs --no-test  reuse the recorded test results
//
// Steps, in order:
//   1. cargo test and lake build, recorded under var/test-results (at most two jobs)
//   2. Exegra: validate the specification and evaluate the claims it can evaluate itself
//   3. Concordance: lint the requirements (strict), then reconcile them with the code and
//      the recorded results, failing on a proven violation
//
// Exegra is found through EXEGRA_HOME (default ../exegra), where this specification is linked
// as the project "strata". Concordance is expected on PATH.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
const results = resolve(root, 'var/test-results');
const skipTests = process.argv.includes('--no-test');
let failed = false;

function run(command, args, options = {}) {
  const done = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    env: { ...process.env, ...options.env },
    encoding: 'utf8',
    maxBuffer: 1 << 28,
    shell: process.platform === 'win32' && command !== process.execPath,
  });
  return { code: done.status ?? 1, out: `${done.stdout ?? ''}${done.stderr ?? ''}`, stdout: done.stdout ?? '' };
}

function report(name, ok, detail) {
  if (!ok) failed = true;
  console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  ${detail}` : ''}`);
}

mkdirSync(results, { recursive: true });

if (!skipTests) {
  // A private build directory, so a running development stack does not hold the binaries.
  const cargo = run('cargo', ['test', '--workspace'], {
    env: { CARGO_BUILD_JOBS: '2', RUST_TEST_THREADS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  const lines = cargo.out.split('\n').filter((line) => !line.startsWith('{'));
  writeFileSync(resolve(results, 'cargo-test.txt'), lines.join('\n'));
  report('cargo test', cargo.code === 0, `${lines.filter((line) => /^test .* ok$/.test(line.trim())).length} passed`);

  const lake = run('lake', ['build', '-v'], { cwd: resolve(root, 'lean') });
  writeFileSync(resolve(results, 'lake-build.txt'), lake.out);
  report('lake build', lake.code === 0);
}

const exegra = (command) => run(process.execPath, [resolve(exegraHome, 'cli/exegra.js'), command, '--project', 'strata'], { cwd: exegraHome });

const validation = exegra('validate');
try {
  const { result } = JSON.parse(validation.stdout);
  const count = (list) => (Array.isArray(list) ? list.length : 0);
  const errors = count(result.errors);
  const warnings = count(result.warnings);
  const gaps = count(result.gaps);
  report('exegra validate', errors + warnings + gaps === 0, `${errors} errors, ${warnings} warnings, ${gaps} gaps`);
} catch {
  report('exegra validate', false, validation.out.slice(0, 300));
}

const evaluation = exegra('claims');
try {
  const { result } = JSON.parse(evaluation.stdout);
  const claims = Array.isArray(result.claims) ? result.claims : Object.values(result.claims ?? result);
  const own = claims.filter((claim) => String(claim.claimId ?? claim.id).startsWith('m-'));
  const notVerified = own.filter((claim) => claim.status !== 'verified');
  const broken = claims.filter((claim) => claim.status === 'violated');
  report('exegra model claims', notVerified.length === 0 && broken.length === 0, `${own.length - notVerified.length} of ${own.length} verified, ${broken.length} violated in all`);
  for (const claim of [...notVerified, ...broken]) console.log(`      ${claim.claimId ?? claim.id}: ${claim.status}`);
} catch {
  report('exegra model claims', false, evaluation.out.slice(0, 300));
}

const lint = run('concordance', ['lint', '--config', 'concordance.toml']);
report('concordance lint', lint.code === 0, lint.out.trim().split('\n').pop());

const scan = run('concordance', ['scan', '--config', 'concordance.toml', '--json', '--fail-on', 'proven', '--exegra-results', 'var/concordance-results.json', '--no-generated-at']);
try {
  const { counts, findings } = JSON.parse(scan.stdout);
  const open = findings.filter((finding) => finding.status === 'violated' || (['unverified', 'ambiguous', 'stale'].includes(finding.status) && /Requirement `/.test(finding.explanation)));
  report('concordance scan', scan.code === 0 && open.length === 0, `${counts.verified} verified, ${counts.violated} violated, ${counts.planned} planned, ${open.length} of the started milestones open`);
  for (const finding of open) console.log(`      ${finding.status}: ${finding.title.slice(0, 120)}`);
  writeFileSync(resolve(root, 'var/scan.json'), scan.stdout);
} catch {
  report('concordance scan', false, scan.out.slice(-300));
}

process.exit(failed ? 1 : 0);
