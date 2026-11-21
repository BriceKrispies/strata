// The specification gate: is the specification sound, and does the code answer it?
//
//   node scripts/spec-gate.mjs                                run the tests, then every check
//   node scripts/spec-gate.mjs --no-test --allow-recorded    check against the recorded test results
//
// A gate run passes only when every check passes. Each tool's result is read from its structured
// output (JSON fields, counts, exit codes); nothing is decided by matching prose. A tool that
// crashes, prints nothing parseable, or cannot be found is a failed check.
//
// Steps, in order:
//   0. specc check: crates/model and its pure crate crates/model-core are exactly what the compiler
//      writes from the specification; check-web and check-e2e do the same for web/src/generated,
//      tests/e2e/generated (the journeys) and tests/e2e/step-checks (the step fixtures). The product
//      specifications in specifications/products go with --product to every check of the platform
//      specification, which refuses a product that does not pass (WG-4989).
//      check-catalogue does the same for specifications/catalogue.md, check-deploy for deploy/, and check-conformance for the
//      contract conformance suite (crates/bff/tests/conformance/generated) and its fixtures.
//      specc lint applies the requirement wording rules to every requirement; any finding fails the step.
//   0b. The test tiers (scripts/test-tiers.json): a test binary in neither tier fails. Runs with --no-test too.
//   1. cargo test --workspace, lake build, node --test (web kernel and client tests), the generated
//      share tests (scripts/generated-share.test.mjs), tsc, cargo fmt --check, cargo clippy with
//      warnings denied on the hand-written crates, cargo audit and npm audit, recorded under
//      var/test-results (at most two jobs). Not run with --no-test.
//   2. Exegra (EXEGRA_HOME, default ../exegra; EXEGRA_PROJECT, default strata): validate the
//      specification, evaluate its claims.
//   3. Evidence (scripts/gate-evidence.mjs): every requirement of a started milestone (gate.json) has a
//      passing proof: a Rust test or Lean theorem with a `Verifies: <REF>` line that the recorded results show
//      as passed, or, for a construction requirement, verified Exegra claims that model it. A `Verifies:` line
//      naming a ref the specification does not have fails too.
//   4. Counts read from the specification itself, compared with the last successful gate record.
//   4b. The specification and the code agree (scripts/gate-audit-checks.mjs, nothing compiled): every
//      shipped contract has a handler, every table the SQLite adapters create has a shipped record,
//      every environment variable the backend and BFF read is declared, every route the BFF mounts
//      has a contract with a bffRoute, every file path the specification cites exists, every shipped
//      contract with a bffRoute has a generated conformance test (the checks the suite cannot make are
//      counted and printed; scripts/gate-conformance.mjs). Each reports
//      a count and fails on a miss.
//   5. With --browser: the browser suites (npm run e2e, e2e:generated, e2e:steps), which build the
//      binaries and the frontend they run against. A skipped browser test fails the check.
//
// Every child process has a timeout (STRATA_GATE_TIMEOUT_MS, default 30 minutes; the build-heavy
// steps get a longer one). A timeout is a failed check.
//
// A tool that is not installed (rustfmt, clippy, cargo-audit, npm) or a suite that was not asked for
// (the browser suites without --browser) is printed as NOT RUN and counted in the verdict line. It
// is never a silent pass. --require-all turns every NOT RUN into a failure; CI passes it.
//
// --no-test reuses var/test-results. It is refused unless --allow-recorded is given, and even then
// it fails when the recorded run was made against another specification, when any source file
// changed after it, when a result file differs from the hash recorded for it, or when re-reading
// the result files does not show passing tests. A successful full run writes
// var/test-results/gate-record.json.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { auditChecks } from './gate-audit-checks.mjs';
import { runCommand } from './gate-run.mjs';
import { GENERATED as CONFORMANCE_SUITE, conformanceCoverage, conformanceVerdict } from './gate-conformance.mjs';
import { evidenceSources, realisationReport, realisationVerdict } from './gate-realisation.mjs';
import { readGateConfig, reportLines, runEvidence } from './gate-evidence.mjs';
import { readWorkspace } from './test-select.mjs';
import { readTiers, routineSeconds, tierProblems } from './test-tiers.mjs';
import { generatedTests, rustLineShare } from './generated-share.mjs';
import { readSpecValue, specSha256 } from './spec-parts.mjs';
import { envelopeProblems, productArgs, savedTraces, semanticHashDetail, traceFreshness } from './spec-hash.mjs';

const root = resolve(import.meta.dirname, '..');
const exegraHome = resolve(root, process.env.EXEGRA_HOME ?? '../exegra');
// The Exegra project that links this checkout's specification (a worktree links its own).
const exegraProject = process.env.EXEGRA_PROJECT ?? 'strata';
const resultsDir = resolve(root, 'var/test-results');
const recordPath = join(resultsDir, 'gate-record.json');
const specPath = resolve(root, 'specifications/spec.jsonc');
const args = new Set(process.argv.slice(2));
const noTest = args.has('--no-test');
const allowRecorded = args.has('--allow-recorded');
const acceptFewer = args.has('--accept-fewer');
const withBrowser = args.has('--browser');
const requireAll = args.has('--require-all');
const DEFAULT_TIMEOUT_MS = Number(process.env.STRATA_GATE_TIMEOUT_MS ?? 30 * 60_000);
const LONG_TIMEOUT_MS = DEFAULT_TIMEOUT_MS * 3;
const PROBE_TIMEOUT_MS = 60_000;

const checks = [];
const failures = [];
const notes = [];
const notRun = [];

// A check that did not run: shown on its own line and counted, never counted as a pass.
function skipCheck(name, reason) {
  notRun.push(name);
  console.log(`NOT RUN  ${name}  ${reason}`);
}

function check(name, ok, detail = '') {
  checks.push({ name, ok });
  if (!ok) failures.push(name);
  console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  ${detail}` : ''}`);
}

function detail(lines) {
  for (const line of lines) console.log(`      ${line}`);
}

// Runs a command with an argument array and no shell (`options.shell` is for `npm` on Windows only,
// with constant arguments). `ok` is true only when it started, was not killed, did not time out and
// exited with 0. A timeout kills it and is reported as `timedOut`.
// Every command's wall time, printed at the end so a slow step is visible (WG-5411).
const timings = [];
function run(command, commandArgs, options = {}) {
  const started = Date.now();
  const result = runCommand(command, commandArgs, {
    cwd: options.cwd ?? root,
    env: { ...process.env, ...options.env },
    timeoutMs: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    shell: options.shell === true,
  });
  timings.push({ what: [command, ...commandArgs].join(' ').slice(0, 110), seconds: (Date.now() - started) / 1000 });
  return result;
}

// One Exegra command on the project, run once per gate: the evidence step and the Exegra step both
// read `claims`, which is the same answer for the same specification (WG-5411).
const exegraRuns = new Map();
function runExegra(command) {
  if (!exegraRuns.has(command)) {
    const cli = resolve(exegraHome, 'cli/exegra.js');
    exegraRuns.set(command, run(process.execPath, [cli, command, '--project', exegraProject, ...(process.env.EXEGRA_SPECS ? ['--specs', process.env.EXEGRA_SPECS] : [])], { cwd: exegraHome }));
  }
  return exegraRuns.get(command);
}

// Whether a tool is installed: its version command starts and exits 0.
function hasTool(command, versionArgs, options = {}) {
  return run(command, versionArgs, { ...options, timeoutMs: PROBE_TIMEOUT_MS }).ok;
}

function exitText(r) {
  if (r.timedOut) return r.error;
  if (r.error) return `did not start: ${r.error}`;
  if (r.signal) return `killed by ${r.signal}`;
  return `exit ${r.code}`;
}

function lastLine(text) {
  return text.trim().split('\n').filter(Boolean).pop() ?? '';
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

// Readers of a recorded result. Each takes the text of the result and whether the command exited 0
// (`true` when re-reading a recorded file, which holds no exit code, so the text alone must show a pass).
function readCargo(out, exitedOk) {
  const libtest = [...out.matchAll(/^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;/gm)];
  const passed = libtest.reduce((sum, m) => sum + Number(m[1]), 0);
  const failed = libtest.reduce((sum, m) => sum + Number(m[2]), 0);
  const errored = /^error: (could not compile|test failed|\d+ targets? failed)/m.test(out);
  const ok = exitedOk && !errored && libtest.length > 0 && failed === 0 && passed > 0;
  return { ok, passed, failed, binaries: libtest.length, errored };
}

function readLake(out, exitedOk) {
  const sorry = /declaration uses '?sorry|sorryAx/.test(out);
  const completed = /Build completed successfully/.test(out);
  return { ok: exitedOk && completed && !sorry, sorry, completed };
}

function readNodeTest(out, exitedOk) {
  const passed = Number((out.match(/^(?:#|ℹ) pass (\d+)/m) ?? [])[1]);
  const failed = Number((out.match(/^(?:#|ℹ) fail (\d+)/m) ?? [])[1]);
  return { ok: exitedOk && passed > 0 && failed === 0, passed, failed };
}

function readTsc(out, exitedOk) {
  return { ok: exitedOk && !/error TS\d+/.test(out) };
}

// Playwright's list reporter ends with lines such as "  12 passed (3.1m)", "  1 skipped", "  2 failed".
function readPlaywright(out, exitedOk) {
  const count = (word) => Number((out.match(new RegExp(`^\\s*(\\d+) ${word}\\b`, 'm')) ?? [])[1] ?? 0);
  const passed = count('passed');
  const failed = count('failed') + count('flaky') + count('interrupted');
  const skipped = count('skipped') + count('did not run');
  return { ok: exitedOk && passed > 0 && failed === 0 && skipped === 0, passed, failed, skipped };
}

// The packages of the workspace, from the member manifests; the generated ones are named apart.
function workspacePackages() {
  const manifest = readFileSync(resolve(root, 'Cargo.toml'), 'utf8');
  const members = [...(manifest.match(/^members\s*=\s*\[([^\]]*)\]/m)?.[1] ?? '').matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  // crates/model/<area> are the crates by area of the generated model (WG-5371).
  const isGenerated = (dir) => dir === 'crates/model' || dir === 'crates/model-core' || dir.startsWith('crates/model/');
  const all = [];
  const handWritten = [];
  for (const dir of members) {
    const name = readFileSync(resolve(root, dir, 'Cargo.toml'), 'utf8').match(/^name\s*=\s*"([^"]+)"/m)?.[1];
    if (!name) continue;
    all.push(name);
    if (!isGenerated(dir)) handWritten.push(name);
  }
  return { all, handWritten, generated: all.filter((name) => !handWritten.includes(name)) };
}

// Every file under the source roots and the root manifests, with its modification time.
const sourceRoots = ['crates', 'lean', 'tests', 'web', 'tenants', 'scripts', 'specifications'];
const sourceFiles = ['Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json', 'gate.json', 'rust-toolchain.toml'];
const skipDirs = new Set(['target', 'node_modules', '.lake', 'dist', 'test-results', 'screenshots', '.git']);

function sourceList() {
  const out = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir)) {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) {
        if (!skipDirs.has(entry)) walk(path);
      } else {
        out.push(relative(root, path).split('\\').join('/'));
      }
    }
  };
  for (const dir of sourceRoots) if (existsSync(join(root, dir))) walk(join(root, dir));
  for (const file of sourceFiles) if (existsSync(join(root, file))) out.push(file);
  return out;
}

// Content hashes of every source file, so a recorded run can be checked against the current
// files. Modification times are not used: a fresh clone gives every file a new one.
function sourceHashes() {
  const map = {};
  for (const file of sourceList()) map[file] = sha256(join(root, file));
  return map;
}

mkdirSync(resultsDir, { recursive: true });

// ---------------------------------------------------------------------------------------------
// The expected counts come from the specification and the gate config (gate.json).
// ---------------------------------------------------------------------------------------------
const spec = readSpecValue(specPath);
// A statement marked `"check": "documentation"` is for people: no check holds it, and the gate's counts leave it out.
const allRequirements = spec.claims.filter((claim) => claim.kind === 'requirement');
const requirements = allRequirements.filter((claim) => claim.check !== 'documentation');
const documentationCount = allRequirements.length - requirements.length;
const modelClaims = spec.claims.filter((claim) => String(claim.id).startsWith('m-'));
const requirementIds = new Set(allRequirements.map((claim) => claim.id));
let startedList = [];
try {
  startedList = readGateConfig(root).startedMilestones;
} catch (error) {
  check('gate.json names started milestones', false, error.message);
}
const started = new Set(startedList);
const gated = requirements.filter((claim) => started.has(claim.milestone));
const counts = { requirements: requirements.length, gated: gated.length, model: modelClaims.length };
const danglingLinks = modelClaims.filter((claim) => !requirementIds.has(claim.models));

console.log(`specification: ${requirements.length} requirements (${documentationCount} more are documentation, outside the counts), ${modelClaims.length} model claims; started milestones: ${[...started].sort((a, b) => a - b).join(', ') || 'none'}`);
if (started.size === 0) check('gate.json names started milestones', false, 'startedMilestones is empty');

// ---------------------------------------------------------------------------------------------
// 0. Generated code: crates/model and crates/model-core are what specc writes from the
//    specification, byte for byte.
// ---------------------------------------------------------------------------------------------
{
  // The semantic hash the generated files must name: the specification as Exegra reads it now.
  const found = existsSync(resolve(exegraHome, 'cli/exegra.js'))
    ? semanticHashDetail(exegraHome, exegraProject)
    : { hash: null, reason: `no cli/exegra.js under ${exegraHome} (set EXEGRA_HOME and EXEGRA_PROJECT)` };
  const hash = found.hash;
  check('exegra reports the specification semantic hash (generated files and saved traces name it)', hash !== null,
    hash ?? found.reason);
  const hashArgs = hash ? ['--spec-hash', hash] : [];
  const products = productArgs(root);
  const specc = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check', '--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime',
    '--core-out', 'crates/model-core', '--core-crate-name', 'strata-model-core', ...hashArgs, ...products], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check (crates/model and crates/model-core are generated from the specification at its semantic hash)', specc.ok,
    specc.ok ? 'up to date' : `${exitText(specc)}: ${lastLine(specc.out)}; run npm run generate and commit`);

  const web = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-web', '--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated', ...hashArgs, ...products], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-web (web/src/generated is generated from the specification at its semantic hash)', web.ok,
    web.ok ? 'up to date' : `${exitText(web)}: ${lastLine(web.out)}; run npm run generate and commit`);

  // A saved Exegra trace records the hash of the specification it was run on, and must equal the
  // trace Exegra runs now (record it again with node scripts/record-trace.mjs).
  if (existsSync(resolve(exegraHome, 'cli/exegra.js'))) {
    for (const file of savedTraces(root)) {
      const fresh = traceFreshness(exegraHome, root, file);
      check(`saved trace is current (${fresh.detail.split(' ')[0]})`, fresh.ok, fresh.detail);
    }
  } else {
    check('saved traces are current', false, 'no Exegra checkout to compare them with');
  }

  const e2e = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-e2e', '--spec', 'specifications/spec.jsonc', '--out', 'tests/e2e/generated', '--origin', 'http://127.0.0.1:4200', ...products], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-e2e (tests/e2e/generated is generated from the specification)', e2e.ok,
    e2e.ok ? 'up to date' : `${exitText(e2e)}: ${lastLine(e2e.out)}; run npm run generate and commit`);

  const steps = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-e2e', '--spec', 'tests/e2e/step-fixtures/spec.jsonc', '--out', 'tests/e2e/step-checks', '--expect-fail-prefix', 'violates-'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-e2e (tests/e2e/step-checks is generated from the step fixtures)', steps.ok,
    steps.ok ? 'up to date' : `${exitText(steps)}: ${lastLine(steps.out)}; run npm run generate and commit`);

  const catalogue = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-catalogue', '--spec', 'specifications/spec.jsonc', '--out', 'specifications', ...hashArgs], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-catalogue (specifications/catalogue.md is generated from the specification at its semantic hash)', catalogue.ok,
    catalogue.ok ? 'up to date' : `${exitText(catalogue)}: ${lastLine(catalogue.out)}; run npm run generate and commit`);

  const conformance = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-conformance', '--spec', 'specifications/spec.jsonc', '--out', 'crates/bff/tests/conformance/generated', ...products], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-conformance (crates/bff/tests/conformance/generated is generated from the specification)', conformance.ok,
    conformance.ok ? 'up to date' : `${exitText(conformance)}: ${lastLine(conformance.out)}; run npm run generate and commit`);

  const conformanceFixtures = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-conformance', '--spec', 'crates/bff/tests/conformance/fixtures/spec.jsonc', '--out', 'crates/bff/tests/conformance/fixtures/generated', '--expect-fail-prefix', 'violates-'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-conformance (crates/bff/tests/conformance/fixtures/generated is generated from the conformance fixtures)', conformanceFixtures.ok,
    conformanceFixtures.ok ? 'up to date' : `${exitText(conformanceFixtures)}: ${lastLine(conformanceFixtures.out)}; run npm run generate and commit`);

  // The requirement wording lint: any finding fails.
  const lint = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'lint', '--spec', 'specifications/spec.jsonc'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc lint (requirements follow the wording rules)', lint.ok,
    lint.ok ? lastLine(lint.out) : `${exitText(lint)}: ${lastLine(lint.out)}`);
  if (!lint.ok) detail(lint.stdout.split(/\r?\n/).filter(Boolean).slice(0, 40));
}

// ---------------------------------------------------------------------------------------------
// 0b. Test tiers: every Rust test binary is in the routine or the full tier (scripts/test-tiers.json),
//     and the routine tier fits its budget. Reads manifests; nothing is compiled.
// ---------------------------------------------------------------------------------------------
{
  const tiers = readTiers();
  const problems = tierProblems(readWorkspace(root), tiers);
  check('every test binary is in a tier and the routine tier fits its budget (scripts/test-tiers.json)', problems.length === 0,
    problems.length ? `${problems.length} problem(s)` : `routine tier ${routineSeconds(tiers).toFixed(1)} s of ${tiers.budgetSeconds} s`);
  detail(problems);

  const deploy = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-deploy', '--spec', 'specifications/spec.jsonc', '--out', 'deploy'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-deploy (deploy/ compose and Kubernetes files are generated from the specification)', deploy.ok,
    deploy.ok ? 'up to date' : `${exitText(deploy)}: ${lastLine(deploy.out)}; run npm run generate and commit`);
}

// ---------------------------------------------------------------------------------------------
// 1. Tests
// ---------------------------------------------------------------------------------------------
const specSha = specSha256(specPath);
const testFiles = ['cargo-test.txt', 'lake-build.txt', 'web-test.txt', 'share-test.txt', 'check.txt'];
let testsPassed = false;
const startedAtMs = Date.now();
// Taken before any test runs: an edit made during the run leaves the record out of date.
const snapshot = sourceHashes();
// The content hash of each recorded result file, written into the gate record and checked by --no-test.
const resultHashes = () => Object.fromEntries(testFiles.filter((file) => existsSync(join(resultsDir, file))).map((file) => [file, sha256(join(resultsDir, file))]));
// EXEGRA_HOME: the specc differential tests run Exegra and fail without it (no silent skip).
const cargoEnv = { CARGO_BUILD_JOBS: '2', RUST_TEST_THREADS: '2', CARGO_TARGET_DIR: 'target/gate', EXEGRA_HOME: exegraHome };
const nodeTestFiles = ['web/test/kernel.test.js', 'web/test/client.test.js', 'web/test/tenant-ui.test.js'];
const shareTestFile = 'scripts/generated-share.test.mjs';
// The tests of the scripts: the share tests and the tests of the audit checks (gate-*.test.mjs).
const scriptTestFiles = [shareTestFile, ...readdirSync(resolve(root, 'scripts')).filter((f) => /^(gate|test)-.*\.test\.mjs$/.test(f)).sort().map((f) => `scripts/${f}`)];
const tscCli = join(root, 'node_modules', 'typescript', 'bin', 'tsc');

if (!noTest) {
  console.log('running tests (--no-test would reuse the recorded results instead)');
  // A private build directory, so a running development stack does not hold the binaries.
  const cargo = run('cargo', ['test', '--workspace', '--no-fail-fast', '--features', 'strata-backend/development'], { env: cargoEnv, timeoutMs: LONG_TIMEOUT_MS });
  writeFileSync(join(resultsDir, 'cargo-test.txt'), cargo.out);
  const cargoRead = readCargo(cargo.out, cargo.ok);
  const cargoOk = cargoRead.ok;
  check('cargo test --workspace', cargoOk, cargoOk ? `${cargoRead.passed} passed, 0 failed` : `${exitText(cargo)}, ${cargoRead.passed} passed, ${cargoRead.failed} failed, ${cargoRead.binaries} test binaries reported`);

  const lake = run('lake', ['build', '-v'], { cwd: resolve(root, 'lean'), timeoutMs: LONG_TIMEOUT_MS });
  writeFileSync(join(resultsDir, 'lake-build.txt'), lake.out);
  const lakeRead = readLake(lake.out, lake.ok);
  const lakeOk = lakeRead.ok;
  check('lake build', lakeOk, lakeOk ? 'no sorry' : `${exitText(lake)}${lakeRead.sorry ? ', sorry found' : ''}${lakeRead.completed ? '' : ', no completion line'}`);

  const web = run(process.execPath, ['--test', ...nodeTestFiles]);
  writeFileSync(join(resultsDir, 'web-test.txt'), web.out);
  const webRead = readNodeTest(web.out, web.ok);
  const webOk = webRead.ok;
  check('web tests: kernel and client (node --test)', webOk, webOk ? `${webRead.passed} passed` : `${exitText(web)}, pass ${webRead.passed}, fail ${webRead.failed}`);

  const share = run(process.execPath, ['--test', ...scriptTestFiles]);
  writeFileSync(join(resultsDir, 'share-test.txt'), share.out);
  const shareRead = readNodeTest(share.out, share.ok);
  const shareOk = shareRead.ok;
  check('script tests (generated share and audit checks, node --test)', shareOk, shareOk ? `${shareRead.passed} passed` : `${exitText(share)}, pass ${shareRead.passed}, fail ${shareRead.failed}`);

  const tsc = run(process.execPath, [tscCli, '-p', 'web/jsconfig.json']);
  writeFileSync(join(resultsDir, 'check.txt'), tsc.out);
  const tscOk = readTsc(tsc.out, tsc.ok).ok;
  check('type check (tsc -p web/jsconfig.json)', tscOk, tscOk ? '' : `${exitText(tsc)}: ${lastLine(tsc.out)}`);

  testsPassed = cargoOk && lakeOk && webOk && shareOk && tscOk;

  // Lints and audits. They are not recorded: a --no-test run does not repeat them.
  const packages = workspacePackages();
  const lintEnv = { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' };
  if (!hasTool('cargo', ['fmt', '--version'])) {
    skipCheck('cargo fmt --check', 'rustfmt is not installed (rustup component add rustfmt)');
  } else {
    // The generated crates are left out: specc writes them, and `specc check` compares them byte for byte.
    const fmt = run('cargo', ['fmt', '--check', ...packages.handWritten.flatMap((name) => ['-p', name])]);
    check('cargo fmt --check (hand-written crates)', fmt.ok, fmt.ok ? `${packages.handWritten.length} crates` : `${exitText(fmt)}: ${lastLine(fmt.out)}; run cargo fmt`);
  }
  if (!hasTool('cargo', ['clippy', '--version'])) {
    skipCheck('cargo clippy -D warnings', 'clippy is not installed (rustup component add clippy)');
  } else {
    const clippy = run('cargo', ['clippy', '--workspace', ...packages.generated.flatMap((name) => ['--exclude', name]), '--all-targets', '--', '-D', 'warnings'], { env: lintEnv, timeoutMs: LONG_TIMEOUT_MS });
    check('cargo clippy -D warnings (hand-written crates)', clippy.ok, clippy.ok ? `${packages.handWritten.length} crates` : `${exitText(clippy)}: ${lastLine(clippy.out)}`);
  }
  if (!hasTool('cargo', ['audit', '--version'])) {
    skipCheck('cargo audit', 'cargo-audit is not installed (cargo install cargo-audit --locked)');
  } else {
    const audit = run('cargo', ['audit'], { env: lintEnv });
    check('cargo audit (Cargo.lock against the RustSec advisories)', audit.ok, audit.ok ? '' : `${exitText(audit)}: ${lastLine(audit.out)}`);
  }
  // npm is npm.cmd on Windows, which needs a shell; the arguments are constants.
  if (!hasTool('npm', ['--version'], { shell: true })) {
    skipCheck('npm audit', 'npm is not on the path');
  } else {
    const npmAudit = run('npm', ['audit', '--audit-level=high'], { shell: true });
    check('npm audit (high and critical advisories)', npmAudit.ok, npmAudit.ok ? '' : `${exitText(npmAudit)}: ${lastLine(npmAudit.out)}`);
  }

  // The results are bound to the content they were run on by the gate record written at the end
  // (the specification hash and every source file hash). Record only if nothing changed meanwhile.
  const after = sourceHashes();
  const moved = [...new Set([...Object.keys(snapshot), ...Object.keys(after)])].filter((file) => snapshot[file] !== after[file]);
  check('no source changed while the tests ran', moved.length === 0, moved.length ? `${moved.length} changed, e.g. ${moved[0]}` : '');
} else {
  console.log('*** NO TESTS RUN. --no-test reuses the recorded results in var/test-results. This is not a full gate run. ***');
  if (!allowRecorded) {
    check('recorded test results may be used', false, 'pass --allow-recorded to accept them');
  }
  skipCheck('cargo fmt --check, cargo clippy, cargo audit, npm audit', '--no-test does not repeat them');
  if (!existsSync(recordPath)) {
    check('recorded test results exist', false, 'no gate record; run the gate without --no-test first');
  } else {
    const record = JSON.parse(readFileSync(recordPath, 'utf8'));
    const specChanged = record.specSha256 !== specSha;
    check('recorded test results match the specification', !specChanged, specChanged ? 'the specification changed after the recorded run' : `spec hash ${specSha.slice(0, 12)} unchanged`);
    const recorded = record.sourceHashes ?? null;
    const changed = recorded
      ? [...new Set([...Object.keys(snapshot), ...Object.keys(recorded)])].filter((file) => snapshot[file] !== recorded[file])
      : ['(the record has no source hashes)'];
    check('no source changed since the recorded run', changed.length === 0, changed.length ? `${changed.length} changed, e.g. ${changed[0]}` : `${Object.keys(snapshot).length} files unchanged`);
    const missing = testFiles.filter((file) => !existsSync(join(resultsDir, file)));
    check('recorded result files exist', missing.length === 0, missing.join(', '));

    // The record's own boolean is not trusted: the result files are compared with the hashes the
    // gate recorded for them, and read again.
    let reread = false;
    if (missing.length === 0) {
      const now = resultHashes();
      const edited = record.resultHashes ? testFiles.filter((file) => now[file] !== record.resultHashes[file]) : ['(the record has no result hashes)'];
      check('recorded result files are the ones the gate recorded', edited.length === 0, edited.length ? `changed: ${edited.join(', ')}` : `${testFiles.length} files match`);
      const text = (file) => readFileSync(join(resultsDir, file), 'utf8');
      const cargoRead = readCargo(text('cargo-test.txt'), true);
      const lakeRead = readLake(text('lake-build.txt'), true);
      const webRead = readNodeTest(text('web-test.txt'), true);
      const shareRead = readNodeTest(text('share-test.txt'), true);
      const tscRead = readTsc(text('check.txt'), true);
      const rereadOk = cargoRead.ok && lakeRead.ok && webRead.ok && shareRead.ok && tscRead.ok;
      check('recorded results, read again, show passing tests', rereadOk,
        `cargo ${cargoRead.passed} passed ${cargoRead.failed} failed; lake ${lakeRead.ok ? 'ok' : 'not ok'}; web ${webRead.passed} passed ${webRead.failed} failed; share ${shareRead.passed} passed ${shareRead.failed} failed; tsc ${tscRead.ok ? 'ok' : 'errors'}`);
      reread = rereadOk && edited.length === 0;
    }
    testsPassed = !specChanged && changed.length === 0 && missing.length === 0 && reread;
    if (record.testsPassed !== true) notes.push('the recorded run did not pass its tests');
  }
}

// ---------------------------------------------------------------------------------------------
// 5. Browser suites (--browser): the Playwright projects build the binaries and the frontend they
//    run against (scripts/dev.mjs without --no-build) and fail when those are older than the sources.
//    A skipped test is a failure here, not a pass.
// ---------------------------------------------------------------------------------------------
if (withBrowser) {
  const playwright = join(root, 'node_modules', '@playwright', 'test', 'cli.js');
  if (!existsSync(playwright)) {
    check('browser suites', false, 'node_modules/@playwright/test is not installed (npm ci; npx playwright install chromium)');
  } else {
    for (const [name, file, config] of [
      ['hand-written browser tests (npm run e2e)', 'browser-e2e.txt', 'tests/e2e/playwright.config.js'],
      ['generated journeys (npm run e2e:generated)', 'browser-generated.txt', 'tests/e2e/generated/playwright.generated.config.js'],
      ['step fixtures (npm run e2e:steps)', 'browser-steps.txt', 'tests/e2e/step-fixtures/playwright.config.js'],
    ]) {
      const done = run(process.execPath, [playwright, 'test', '-c', config], { timeoutMs: LONG_TIMEOUT_MS, env: { CARGO_BUILD_JOBS: '2' } });
      writeFileSync(join(resultsDir, file), done.out);
      const read = readPlaywright(done.out, done.ok);
      check(name, read.ok, read.ok ? `${read.passed} passed` : `${exitText(done)}, ${read.passed} passed, ${read.failed} failed, ${read.skipped} skipped`);
    }
  }
} else {
  skipCheck('browser suites (e2e, generated journeys, step fixtures)', 'pass --browser; they build the binaries and the frontend, then run Chromium');
}

// ---------------------------------------------------------------------------------------------
// 3. Evidence: every started requirement has a passing proof (scripts/gate-evidence.mjs)
// ---------------------------------------------------------------------------------------------
// A construction requirement is verified by the Exegra claims that model it, so their verdicts are
// read first and written to var/exegra-verdicts.json, where the evidence check and anyone reading
// the gate's output can see them.
{
  const cli = resolve(exegraHome, 'cli/exegra.js');
  const verdicts = existsSync(cli) ? runExegra('claims') : null;
  let claims = null;
  try {
    claims = verdicts ? JSON.parse(verdicts.stdout).result : null;
  } catch {
    claims = null;
  }
  const written = Array.isArray(claims);
  if (written) writeFileSync(resolve(root, 'var/exegra-verdicts.json'), JSON.stringify({ results: claims }));
  check('exegra claim verdicts for construction requirements', written, written ? `${claims.length} claims` : `no verdicts: ${verdicts ? exitText(verdicts) : 'no Exegra checkout'}`);

  let evidence = null;
  try {
    evidence = runEvidence({ root, spec, resultsDir, verdicts: written ? claims : null, started: startedList });
  } catch (error) {
    check('requirement evidence', false, `unreadable: ${error.message}`);
  }
  if (evidence) {
    const lines = reportLines(evidence);
    check('every started requirement has a passing proof, and every Verifies line names a known ref', evidence.ok, `${lines[0]} (milestones ${startedList.join(', ')})`);
    detail(lines.slice(1));
  }
}

// ---------------------------------------------------------------------------------------------
// 2. Exegra
// ---------------------------------------------------------------------------------------------
const exegraCli = resolve(exegraHome, 'cli/exegra.js');
const exegraFound = existsSync(exegraCli);
if (!exegraFound) {
  check('Exegra checkout', false, `no cli/exegra.js under ${exegraHome} (set EXEGRA_HOME)`);
} else {
  const exegra = (command) => {
    const r = runExegra(command);
    let json = null;
    try {
      json = JSON.parse(r.stdout);
    } catch {
      json = null;
    }
    return { r, json };
  };
  const count = (value) => (typeof value === 'number' ? value : Array.isArray(value) ? value.length : NaN);

  const validation = exegra('validate');
  const vr = validation.json?.result;
  const problems = vr ? [count(vr.errors), count(vr.warnings), count(vr.gaps)] : [NaN, NaN, NaN];
  const validOk = validation.json?.ok === true && problems.every((n) => n === 0);
  check('exegra validate', validOk, validOk ? '0 errors, 0 warnings, 0 gaps' : `${validation.json?.ok === true ? '' : 'not ok; '}errors ${problems[0]}, warnings ${problems[1]}, gaps ${problems[2]}; ${exitText(validation.r)}`);
  if (!validOk) detail(envelopeProblems(validation.json));

  const evaluation = exegra('claims');
  const list = Array.isArray(evaluation.json?.result) ? evaluation.json.result : null;
  if (!list) {
    check('exegra claims', false, `no claim list; ${exitText(evaluation.r)}`);
  } else {
    const byId = new Map(list.map((claim) => [claim.claimId, claim]));
    const modelResults = modelClaims.map((claim) => byId.get(claim.id));
    const modelVerified = modelResults.filter((result) => result?.status === 'verified').length;
    const violated = list.filter((claim) => claim.status === 'violated');
    const modelOk = modelVerified === modelClaims.length && violated.length === 0 && evaluation.r.ok;
    check('exegra model claims', modelOk, `${modelVerified} of ${modelClaims.length} verified, ${violated.length} violated`);
    const notVerified = modelClaims.filter((claim, i) => modelResults[i]?.status !== 'verified');
    detail(notVerified.map((claim) => `${claim.id}: ${byId.get(claim.id)?.status ?? 'missing from Exegra output'}`));
    detail(violated.map((claim) => `${claim.claimId}: violated`));
  }
}

// ---------------------------------------------------------------------------------------------
// 4. Counts from the last successful gate run, so that a deleted requirement or claim fails.
// ---------------------------------------------------------------------------------------------
let previous = null;
if (existsSync(recordPath)) previous = JSON.parse(readFileSync(recordPath, 'utf8'));
check('model links name existing requirements', danglingLinks.length === 0, `${modelClaims.length - danglingLinks.length} of ${modelClaims.length} link to a requirement`);
detail(danglingLinks.map((claim) => `${claim.id} models ${JSON.stringify(claim.models)}`));
if (previous?.counts) {
  for (const key of ['requirements', 'gated', 'model']) {
    const before = previous.counts[key];
    const now = counts[key];
    if (now < before) {
      check(`${key} count did not drop`, acceptFewer, `${before} -> ${now}${acceptFewer ? ' (accepted by --accept-fewer)' : ' (pass --accept-fewer if the removal is intended)'}`);
    }
  }
} else {
  notes.push('no previous gate record: counts were not compared with an earlier run');
}

// ---------------------------------------------------------------------------------------------
// 4b. The specification and the code agree. Nothing is compiled: the checks read the specification
//     and the source text (scripts/gate-audit-checks.mjs). A miss fails.
// ---------------------------------------------------------------------------------------------
{
  for (const verdict of auditChecks({ root, spec })) {
    check(verdict.name, verdict.ok, verdict.detail);
    detail(verdict.lines.slice(0, 40));
  }
  // Realisation labels derived from the evidence (scripts/gate-realisation.mjs): a label the evidence
  // contradicts fails; the disagreements and the property claims without a generator are listed.
  const realisation = realisationVerdict(realisationReport({ claims: spec.claims ?? [], uiScenarios: spec.uiScenarios ?? [], sources: evidenceSources(root) }));
  check(realisation.name, realisation.ok, realisation.detail);
  detail(realisation.lines.slice(0, 40));
  // The conformance suite: every skipped check is printed, however many there are.
  const suitePath = resolve(root, CONFORMANCE_SUITE);
  const suite = conformanceVerdict(conformanceCoverage({ contracts: spec.contracts, generated: existsSync(suitePath) ? readFileSync(suitePath, 'utf8') : null }));
  check(suite.name, suite.ok, suite.detail);
  detail(suite.lines);
}

// ---------------------------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------------------------
if (requireAll && notRun.length) check('every check ran (--require-all)', false, `${notRun.length} not run`);
const passed = failures.length === 0 && (!noTest || allowRecorded) && (noTest || testsPassed);
const heldPct = requirements.length ? ((gated.length / requirements.length) * 100).toFixed(1) : '0.0';
console.log('');
console.log(`held to code: ${gated.length} of ${requirements.length} requirements (${heldPct}%). The other ${requirements.length - gated.length} are planned: the gate does not check them against the code.`);
console.log(`model claims: ${counts.model} in the specification; requirements of started milestones: ${gated.length}.`);
// A behaviour claim states whether a listed scenario injects a real fault (fault) or the claim fails only under a defect
// built to negate it (restatement). See specifications/README.md, "Fault basis of a behaviour claim".
const behaviourClaims = modelClaims.filter((claim) => Array.isArray(claim.evaluate?.scenarios));
const faultBased = behaviourClaims.filter((claim) => claim.faultBasis === 'fault');
const restatements = behaviourClaims.filter((claim) => claim.faultBasis === 'restatement');
const unmarked = behaviourClaims.filter((claim) => claim.faultBasis !== 'fault' && claim.faultBasis !== 'restatement');
console.log(`behaviour claims: ${behaviourClaims.length}; ${faultBased.length} hold under a modelled fault, ${restatements.length} are restatements${unmarked.length ? `; ${unmarked.length} carry no fault basis (${unmarked.map((claim) => claim.id).join(', ')})` : ''}.`);
// Generated share: informational, never fails the gate (scripts/generated-share.mjs).
const realised = { construction: 0, checked: 0, operational: 0, missing: 0 };
for (const claim of requirements) {
  if (['construction', 'checked', 'operational'].includes(claim.realisation)) realised[claim.realisation] += 1;
  else realised.missing += 1;
}
const constructionPct = requirements.length ? ((realised.construction / requirements.length) * 100).toFixed(1) : '0.0';
console.log(`construction share: ${realised.construction} of ${requirements.length} requirements (${constructionPct}%); checked ${realised.checked}, operational ${realised.operational}, missing ${realised.missing}`);
const rust = rustLineShare(root);
const generatedPct = rust.total ? ((rust.generated / rust.total) * 100).toFixed(1) : '0.0';
console.log(`generated code: ${rust.generated} of ${rust.total} non-blank Rust lines linked into the shipped binaries (${generatedPct}%); hand-written ${rust.handWritten}; crates ${rust.crates.join(', ')}; generated modules ${rust.generatedModules.join(', ') || 'none'}`);
const cargoRecord = existsSync(join(resultsDir, 'cargo-test.txt')) ? readFileSync(join(resultsDir, 'cargo-test.txt'), 'utf8') : '';
const genTests = generatedTests(root, cargoRecord);
if (genTests.total === 0 && genTests.unmapped === 0) console.log('generated tests: none yet');
else console.log(`generated tests: ${genTests.passed} of ${genTests.total} passed${genTests.unmapped ? `; ${genTests.unmapped} tests of unmapped test binaries not counted` : ''}`);
if (noTest) console.log('RECORDED TEST RESULTS, NOT A RUN OF THE TESTS ON THIS CODE. Accepted only because --allow-recorded was given.');
for (const note of notes) console.log(`note: ${note}`);
if (notRun.length) console.log(`NOT RUN: ${notRun.length} check${notRun.length === 1 ? '' : 's'} did not run (${notRun.join('; ')}). A pass below does not cover them.`);
const timed = [...timings].sort((a, b) => b.seconds - a.seconds).filter((t) => t.seconds >= 1);
console.log(`time: ${process.uptime().toFixed(0)} s in all; commands of a second or more:`);
for (const t of timed) console.log(`  ${t.seconds.toFixed(1).padStart(7)} s  ${t.what}`);
console.log(passed ? (notRun.length ? `GATE PASSED, with ${notRun.length} not run` : 'GATE PASSED') : `GATE FAILED: ${failures.join('; ') || 'recorded test results not accepted'}`);

if (passed && !noTest) {
  writeFileSync(
    recordPath,
    JSON.stringify({ recordedAt: new Date(startedAtMs).toISOString(), specSha256: specSha, sourceHashes: snapshot, resultHashes: resultHashes(), testsPassed: true, counts }, null, 2) + '\n',
  );
} else if (passed && noTest && previous) {
  // The recorded run still holds; only the counts are refreshed. Its tests and hashes are kept.
  writeFileSync(recordPath, JSON.stringify({ ...previous, counts }, null, 2) + '\n');
}

process.exit(passed ? 0 : 1);
