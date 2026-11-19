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
//   0. specc check: crates/model is exactly what the compiler writes from the specification.
//   1. cargo test --workspace, lake build, node --test (web), npm run check (tsc), recorded under
//      var/test-results (at most two jobs). Not run with --no-test.
//   2. Exegra (EXEGRA_HOME, default ../exegra; EXEGRA_PROJECT, default strata): validate the
//      specification, evaluate its claims.
//   3. Concordance: lint (strict), then scan with the recorded results and the Exegra results file.
//   4. Counts read from the specification itself, compared with the last successful gate record.
//
// Not run here: the browser tests (npm run e2e). They need a build and two stacks.
//
// --no-test reuses var/test-results. It is refused unless --allow-recorded is given, and even then
// it fails when the recorded run was made against another specification, or when any source file
// changed after it. A successful full run writes var/test-results/gate-record.json.
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { generatedTests, rustLineShare } from './generated-share.mjs';

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

const checks = [];
const failures = [];
const notes = [];

function check(name, ok, detail = '') {
  checks.push({ name, ok });
  if (!ok) failures.push(name);
  console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  ${detail}` : ''}`);
}

function detail(lines) {
  for (const line of lines) console.log(`      ${line}`);
}

// Runs a command. `ok` is true only when it started, was not killed, and exited with 0.
function run(command, commandArgs, options = {}) {
  const done = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? root,
    env: { ...process.env, ...options.env },
    encoding: 'utf8',
    maxBuffer: 1 << 28,
    shell: process.platform === 'win32' && command !== process.execPath,
  });
  const error = done.error ? done.error.message : null;
  const ok = error === null && done.signal === null && done.status === 0;
  return {
    ok,
    code: done.status,
    signal: done.signal,
    error,
    stdout: done.stdout ?? '',
    out: `${done.stdout ?? ''}${done.stderr ?? ''}`,
  };
}

function exitText(r) {
  if (r.error) return `did not start: ${r.error}`;
  if (r.signal) return `killed by ${r.signal}`;
  return `exit ${r.code}`;
}

function lastLine(text) {
  return text.trim().split('\n').filter(Boolean).pop() ?? '';
}

// JSON with comments and trailing commas (the specification's format), as a plain value.
function parseJsonc(src) {
  const BS = String.fromCharCode(92);
  let out = '';
  let i = 0;
  let inString = false;
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (inString) {
      out += c;
      if (c === BS) {
        out += n;
        i += 2;
        continue;
      }
      if (c === '"') inString = false;
      i += 1;
      continue;
    }
    if (c === '"') {
      inString = true;
      out += c;
      i += 1;
      continue;
    }
    if (c === '/' && n === '/') {
      while (i < src.length && src[i] !== '\n') i += 1;
      continue;
    }
    if (c === '/' && n === '*') {
      i += 2;
      while (i < src.length && !(src[i] === '*' && src[i + 1] === '/')) i += 1;
      i += 2;
      continue;
    }
    out += c;
    i += 1;
  }
  return JSON.parse(out.replace(/,(\s*[}\]])/g, '$1'));
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

// Every file under the source roots and the root manifests, with its modification time.
const sourceRoots = ['crates', 'lean', 'tests', 'web', 'tenants', 'scripts', 'specifications'];
const sourceFiles = ['Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json', 'concordance.toml', 'rust-toolchain.toml'];
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
// The expected counts come from the specification and the Concordance configuration.
// ---------------------------------------------------------------------------------------------
const spec = parseJsonc(readFileSync(specPath, 'utf8'));
const requirements = spec.claims.filter((claim) => claim.kind === 'requirement');
const modelClaims = spec.claims.filter((claim) => String(claim.id).startsWith('m-'));
const requirementIds = new Set(requirements.map((claim) => claim.id));
const config = readFileSync(resolve(root, 'concordance.toml'), 'utf8');
const startedMatch = config.match(/^\s*started_milestones\s*=\s*\[([^\]]*)\]/m);
const started = new Set(startedMatch ? startedMatch[1].split(',').map((s) => Number(s.trim())).filter(Number.isFinite) : []);
const gated = requirements.filter((claim) => started.has(claim.milestone));
const counts = { requirements: requirements.length, gated: gated.length, model: modelClaims.length };
const danglingLinks = modelClaims.filter((claim) => !requirementIds.has(claim.models));

console.log(`specification: ${requirements.length} requirements, ${modelClaims.length} model claims; started milestones: ${[...started].sort((a, b) => a - b).join(', ') || 'none'}`);
if (!startedMatch || started.size === 0) check('concordance.toml names started milestones', false, 'no [gate] started_milestones');

// ---------------------------------------------------------------------------------------------
// 0. Generated code: crates/model is what specc writes from the specification, byte for byte.
// ---------------------------------------------------------------------------------------------
{
  const specc = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check', '--spec', 'specifications/spec.jsonc', '--out', 'crates/model', '--crate-name', 'strata-model', '--runtime', 'crates/runtime'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check (crates/model is generated from the specification)', specc.ok,
    specc.ok ? 'up to date' : `${exitText(specc)}: ${lastLine(specc.out)}; run npm run generate and commit`);

  const web = run('cargo', ['run', '--quiet', '-p', 'strata-spec-compiler', '--bin', 'specc', '--',
    'check-web', '--spec', 'specifications/spec.jsonc', '--out', 'web/src/generated'], {
    env: { CARGO_BUILD_JOBS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  check('specc check-web (web/src/generated is generated from the specification)', web.ok,
    web.ok ? 'up to date' : `${exitText(web)}: ${lastLine(web.out)}; run npm run generate and commit`);
}

// ---------------------------------------------------------------------------------------------
// 1. Tests
// ---------------------------------------------------------------------------------------------
const specSha = sha256(specPath);
const testFiles = ['cargo-test.txt', 'lake-build.txt', 'web-test.txt', 'check.txt'];
let testsPassed = false;
const startedAtMs = Date.now();
// Taken before any test runs: an edit made during the run leaves the record out of date.
const snapshot = sourceHashes();

if (!noTest) {
  console.log('running tests (--no-test would reuse the recorded results instead)');
  // A private build directory, so a running development stack does not hold the binaries.
  const cargo = run('cargo', ['test', '--workspace', '--no-fail-fast'], {
    env: { CARGO_BUILD_JOBS: '2', RUST_TEST_THREADS: '2', CARGO_TARGET_DIR: 'target/gate' },
  });
  writeFileSync(join(resultsDir, 'cargo-test.txt'), cargo.out);
  const libtest = [...cargo.out.matchAll(/^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed;/gm)];
  const passed = libtest.reduce((sum, m) => sum + Number(m[1]), 0);
  const failed = libtest.reduce((sum, m) => sum + Number(m[2]), 0);
  const cargoOk = cargo.ok && libtest.length > 0 && failed === 0 && passed > 0;
  check('cargo test --workspace', cargoOk, cargoOk ? `${passed} passed, 0 failed` : `${exitText(cargo)}, ${passed} passed, ${failed} failed, ${libtest.length} test binaries reported`);

  const lake = run('lake', ['build', '-v'], { cwd: resolve(root, 'lean') });
  writeFileSync(join(resultsDir, 'lake-build.txt'), lake.out);
  const sorry = /declaration uses '?sorry|sorryAx/.test(lake.out);
  const lakeOk = lake.ok && /Build completed successfully/.test(lake.out) && !sorry;
  check('lake build', lakeOk, lakeOk ? 'no sorry' : `${exitText(lake)}${sorry ? ', sorry found' : ''}${/Build completed successfully/.test(lake.out) ? '' : ', no completion line'}`);

  const web = run(process.execPath, ['--test', 'web/test/kernel.test.js']);
  writeFileSync(join(resultsDir, 'web-test.txt'), web.out);
  const webPass = Number((web.out.match(/^(?:#|ℹ) pass (\d+)/m) ?? [])[1]);
  const webFail = Number((web.out.match(/^(?:#|ℹ) fail (\d+)/m) ?? [])[1]);
  const webOk = web.ok && webPass > 0 && webFail === 0;
  check('web kernel tests (node --test)', webOk, webOk ? `${webPass} passed` : `${exitText(web)}, pass ${webPass}, fail ${webFail}`);

  const tsc = run('npm', ['run', 'check']);
  writeFileSync(join(resultsDir, 'check.txt'), tsc.out);
  check('type check (npm run check)', tsc.ok, tsc.ok ? '' : `${exitText(tsc)}: ${lastLine(tsc.out)}`);

  testsPassed = cargoOk && lakeOk && webOk && tsc.ok;

  // Concordance accepts a result only when a manifest written right after the run says which source
  // content and which requirement statements it was run on. Record only if nothing changed meanwhile.
  const after = sourceHashes();
  const moved = [...new Set([...Object.keys(snapshot), ...Object.keys(after)])].filter((file) => snapshot[file] !== after[file]);
  check('no source changed while the tests ran', moved.length === 0, moved.length ? `${moved.length} changed, e.g. ${moved[0]}` : '');
  if (moved.length === 0) {
    const record = run('concordance', ['record', '--config', 'concordance.toml']);
    check('concordance record', record.ok, record.ok ? lastLine(record.out) : `${exitText(record)}: ${lastLine(record.out)}`);
  }
} else {
  console.log('*** NO TESTS RUN. --no-test reuses the recorded results in var/test-results. This is not a full gate run. ***');
  if (!allowRecorded) {
    check('recorded test results may be used', false, 'pass --allow-recorded to accept them');
  }
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
    testsPassed = !specChanged && changed.length === 0 && missing.length === 0 && record.testsPassed === true;
    if (record.testsPassed !== true) notes.push('the recorded run did not pass its tests');
  }
}

// ---------------------------------------------------------------------------------------------
// 3. Concordance
// ---------------------------------------------------------------------------------------------
const lint = run('concordance', ['lint', '--config', 'concordance.toml']);
check('concordance lint (strict)', lint.ok, lint.ok ? lastLine(lint.out) : `${exitText(lint)}: ${lastLine(lint.out)}`);

// A construction requirement is verified by the Exegra claims that model it, so Concordance reads
// their verdicts. The model claims do not depend on Concordance's results, so they are evaluated
// first, here, and written where the scan reads them.
{
  const cli = resolve(exegraHome, 'cli/exegra.js');
  const verdicts = existsSync(cli) ? run(process.execPath, [cli, 'claims', '--project', exegraProject], { cwd: exegraHome }) : null;
  let claims = null;
  try {
    claims = verdicts ? JSON.parse(verdicts.stdout).result : null;
  } catch {
    claims = null;
  }
  const written = Array.isArray(claims);
  if (written) writeFileSync(resolve(root, 'var/exegra-verdicts.json'), JSON.stringify({ results: claims }));
  check('exegra claim verdicts for construction requirements', written, written ? `${claims.length} claims` : `no verdicts: ${verdicts ? exitText(verdicts) : 'no Exegra checkout'}`);
}

const scan = run('concordance', ['scan', '--config', 'concordance.toml', '--json', '--fail-on', 'unverified', '--exegra-results', 'var/concordance-results.json', '--exegra-verdicts', 'var/exegra-verdicts.json', '--no-generated-at']);
let report = null;
try {
  report = JSON.parse(scan.stdout);
} catch {
  report = null;
}
if (!report || !Array.isArray(report.findings) || !report.counts) {
  check('concordance scan', false, `no report; ${exitText(scan)}: ${lastLine(scan.out)}`);
} else {
  writeFileSync(resolve(root, 'var/scan.json'), scan.stdout);
  const provenOk = scan.ok && report.gate?.failed === false;
  check('concordance scan (fail-on unverified)', provenOk, provenOk ? `${report.counts.violated} violated` : `${exitText(scan)}, gate.failed ${report.gate?.failed}`);

  let results = null;
  try {
    results = JSON.parse(readFileSync(resolve(root, 'var/concordance-results.json'), 'utf8'));
  } catch (error) {
    check('concordance results file', false, `unreadable: ${error.message}`);
  }
  const proofless = results
    ? gated.filter((claim) => results.claims?.[claim.id]?.status !== 'verified' || results.claims[claim.id].outcome !== 'pass')
    : gated;
  check('every started requirement has a passing proof', results !== null && proofless.length === 0, `${gated.length - proofless.length} of ${gated.length} requirements of the started milestones (${[...started].sort((a, b) => a - b).join(', ')})`);
  detail(proofless.slice(0, 20).map((claim) => `${claim.id}: ${results?.claims?.[claim.id]?.status ?? 'no entry'}`));

  const gatedFindings = report.findings.filter((finding) => started.has(finding.milestone));
  const openGated = gatedFindings.filter((finding) => finding.status !== 'verified');
  check('no open finding on a started milestone', openGated.length === 0, `${gatedFindings.length} findings on started milestones`);
  detail(openGated.slice(0, 20).map((finding) => `${finding.status}: ${finding.title.slice(0, 120)}`));

  const violatedFindings = report.findings.filter((finding) => finding.status === 'violated');
  check('no violated finding anywhere', violatedFindings.length === 0, `${violatedFindings.length} violated`);
  detail(violatedFindings.slice(0, 20).map((finding) => `${finding.title.slice(0, 120)}`));
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
    const r = run(process.execPath, [exegraCli, command, '--project', exegraProject], { cwd: exegraHome });
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
  const concordanceWarnings = Array.isArray(vr?.concordance) ? vr.concordance.length : NaN;
  const validOk = validation.json?.ok === true && problems.every((n) => n === 0) && concordanceWarnings === 0;
  check('exegra validate', validOk, validOk ? '0 errors, 0 warnings, 0 gaps' : `${validation.json?.ok === true ? '' : 'not ok; '}errors ${problems[0]}, warnings ${problems[1]}, gaps ${problems[2]}, concordance warnings ${concordanceWarnings}; ${exitText(validation.r)}`);

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

    const unsupported = gated.filter((claim) => !['supported', 'verified'].includes(byId.get(claim.id)?.status));
    check('exegra shows started requirements as supported', unsupported.length === 0, `${gated.length - unsupported.length} of ${gated.length}`);
    detail(unsupported.slice(0, 20).map((claim) => `${claim.id}: ${byId.get(claim.id)?.status ?? 'missing'}`));
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
// Verdict
// ---------------------------------------------------------------------------------------------
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
console.log(`generated code: ${rust.generated} of ${rust.total} non-blank Rust lines under crates/ (${generatedPct}%); hand-written ${rust.handWritten}`);
const cargoRecord = existsSync(join(resultsDir, 'cargo-test.txt')) ? readFileSync(join(resultsDir, 'cargo-test.txt'), 'utf8') : '';
const genTests = generatedTests(root, cargoRecord);
if (genTests.total === 0 && genTests.unmapped === 0) console.log('generated tests: none yet');
else console.log(`generated tests: ${genTests.passed} of ${genTests.total} passed${genTests.unmapped ? `; ${genTests.unmapped} tests of unmapped test binaries not counted` : ''}`);
if (noTest) console.log('RECORDED TEST RESULTS, NOT A RUN OF THE TESTS ON THIS CODE. Accepted only because --allow-recorded was given.');
for (const note of notes) console.log(`note: ${note}`);
console.log(passed ? 'GATE PASSED' : `GATE FAILED: ${failures.join('; ') || 'recorded test results not accepted'}`);

if (passed && !noTest) {
  writeFileSync(
    recordPath,
    JSON.stringify({ recordedAt: new Date(startedAtMs).toISOString(), specSha256: specSha, sourceHashes: snapshot, testsPassed: true, counts }, null, 2) + '\n',
  );
} else if (passed && noTest && previous) {
  // The recorded run still holds; only the counts are refreshed. Its tests and hashes are kept.
  writeFileSync(recordPath, JSON.stringify({ ...previous, counts }, null, 2) + '\n');
}

process.exit(passed ? 0 : 1);
