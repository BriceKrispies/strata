// Which Rust test binaries a change can affect. Pure logic over the manifests: nothing here runs
// cargo or reads anything but `Cargo.toml` files and the tier list (`scripts/test-tiers.json`).
//
// - `readWorkspace(root)`: the member packages, their directories, the workspace packages each depends
//   on (normal, dev and build dependencies) and their test targets.
// - `dependants(graph, names)`: those packages and everything that depends on them, directly or not.
// - `ownerOf(file, graph)`: the package a changed file belongs to.
// - `selectScope(changed, graph)`: the packages a list of changed files affects, with the reasons and
//   the checks outside cargo it calls for.
// - `scopedPlan(scope, tiers, options)`: the cargo arguments that run exactly those packages' routine
//   and full-tier test binaries (the very slow ones only with `options.slow`).
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

/** The section a manifest header opens: `{dependencies: true, name?}` or null. */
export function dependencySection(header) {
  const m = /^\[(?:target\..+?\.)?(?:dev-|build-)?dependencies(?:\.([A-Za-z0-9_-]+))?\]\s*$/.exec(header.trim());
  return m ? { name: m[1] ?? null } : null;
}

/** The text of a manifest without comments: a `#` outside a string starts one. */
function stripComments(text) {
  return text
    .split(/\r?\n/)
    .map((line) => {
      let quote = null;
      for (let i = 0; i < line.length; i += 1) {
        const c = line[i];
        if (quote) {
          if (c === quote) quote = null;
        } else if (c === '"' || c === "'") quote = c;
        else if (c === '#') return line.slice(0, i);
      }
      return line;
    })
    .join('\n');
}

/**
 * What a member manifest says that selection needs: its package name, the names it depends on (any
 * dependency section; the caller keeps only workspace packages), and its declared test targets.
 */
export function parseManifest(text) {
  const lines = stripComments(text).split('\n');
  let section = null; // the current header text
  let dependencySectionOpen = null;
  let name = null;
  const deps = new Set();
  const declaredTests = [];
  let autotests = true;
  let currentTest = null;
  for (const raw of lines) {
    const line = raw.trim();
    if (!line) continue;
    if (line.startsWith('[')) {
      section = line;
      dependencySectionOpen = dependencySection(line);
      if (dependencySectionOpen?.name) deps.add(dependencySectionOpen.name);
      currentTest = null;
      if (line === '[[test]]') {
        currentTest = { name: null, path: null };
        declaredTests.push(currentTest);
      }
      continue;
    }
    if (dependencySectionOpen && !dependencySectionOpen.name) {
      const key = /^([A-Za-z0-9_-]+)(?:\.[A-Za-z0-9_-]+)?\s*=/.exec(line);
      if (key) deps.add(key[1]);
    } else if (section === '[package]') {
      const m = /^name\s*=\s*"([^"]+)"/.exec(line);
      if (m) name = m[1];
      if (/^autotests\s*=\s*false\b/.test(line)) autotests = false;
    } else if (currentTest) {
      const m = /^(name|path)\s*=\s*"([^"]+)"/.exec(line);
      if (m) currentTest[m[1]] = m[2];
    }
  }
  return { name, deps: [...deps], declaredTests, autotests };
}

/** The workspace members' directories, from the root manifest's `members = [...]`. */
export function memberDirs(rootManifest) {
  const body = /^members\s*=\s*\[([^\]]*)\]/m.exec(rootManifest)?.[1] ?? '';
  return [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

/**
 * The integration test targets of a package directory: `tests/<name>.rs`, `tests/<name>/main.rs`, and
 * the `[[test]]` entries of the manifest (which can name a target by another path). With
 * `autotests = false` in the manifest only the `[[test]]` entries are targets.
 */
export function testTargets(dir, declaredTests, list = defaultLister, autotests = true) {
  const names = new Set();
  const tests = join(dir, 'tests');
  for (const entry of autotests ? list(tests) : []) {
    if (entry.isFile && entry.name.endsWith('.rs')) names.add(entry.name.slice(0, -3));
    if (entry.isDirectory && list(join(tests, entry.name)).some((e) => e.isFile && e.name === 'main.rs')) names.add(entry.name);
  }
  for (const t of declaredTests) if (t.name) names.add(t.name);
  return [...names].sort();
}

function defaultLister(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).map((name) => {
    const stat = statSync(join(dir, name));
    return { name, isFile: stat.isFile(), isDirectory: stat.isDirectory() };
  });
}

/** The workspace: `{packages: [{name, dir, deps, tests}]}`, dependencies restricted to members. */
export function readWorkspace(root, { read = (p) => readFileSync(p, 'utf8'), list = defaultLister } = {}) {
  const dirs = memberDirs(read(resolve(root, 'Cargo.toml')));
  const parsed = dirs.map((dir) => ({ dir, ...parseManifest(read(resolve(root, dir, 'Cargo.toml'))) }));
  const names = new Set(parsed.map((p) => p.name));
  return {
    packages: parsed.map((p) => ({
      name: p.name,
      dir: p.dir,
      deps: p.deps.filter((d) => names.has(d) && d !== p.name).sort(),
      tests: testTargets(resolve(root, p.dir), p.declaredTests, list, p.autotests),
    })),
  };
}

/** The names in `names` and every package that depends on one of them, transitively. */
export function dependants(graph, names) {
  const out = new Set(names);
  for (let grew = true; grew; ) {
    grew = false;
    for (const p of graph.packages) {
      if (!out.has(p.name) && p.deps.some((d) => out.has(d))) {
        out.add(p.name);
        grew = true;
      }
    }
  }
  return [...out].sort();
}

/** The package whose directory holds `file` (the longest matching directory), or null. */
export function ownerOf(file, graph) {
  const f = file.split('\\').join('/');
  let best = null;
  for (const p of graph.packages) {
    if (f === p.dir || f.startsWith(`${p.dir}/`)) if (!best || p.dir.length > best.dir.length) best = p;
  }
  return best?.name ?? null;
}

/**
 * Files outside a package that a package's tests read: a change to one affects those packages (and
 * their dependants). Found by searching the tests for the paths; keep it in step when a test starts
 * reading another tree.
 */
export const READERS = [
  { prefix: 'tenants/', why: 'tenant bundles are read by the tests that install them', packages: ['strata-backend', 'strata-bff', 'strata-kernel', 'strata-protocol', 'strata-simulation'] },
  { prefix: 'lean/', why: 'the Lean correspondence tests read the Lean project', packages: ['strata-verification'] },
  {
    prefix: 'specifications/',
    why: 'the specification is read by the specc, expression, storage and backend tests',
    packages: ['strata-spec-compiler', 'strata-spec-compiler-core-fixture', 'strata-spec-expr', 'strata-storage-api', 'strata-storage-sqlite', 'strata-backend', 'strata-system-differential'],
  },
];

/** Root files that change every build: all packages are affected. */
export const ROOT_FILES = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'];

/**
 * What a list of changed files affects.
 * Returns `{all, packages, specification, web, scripts, other, reasons}`: `all` when the whole
 * workspace is affected; `packages` the affected package names with their dependants; `specification`
 * when `specifications/` changed (the spec checks apply); `web`, `scripts` when those trees changed.
 */
export function selectScope(changed, graph) {
  const hit = new Set();
  const reasons = {};
  const note = (pkg, why) => {
    hit.add(pkg);
    (reasons[pkg] ??= new Set()).add(why);
  };
  let all = false;
  let specification = false;
  let web = false;
  let scripts = false;
  const other = [];
  for (const raw of changed) {
    const file = raw.split('\\').join('/');
    if (ROOT_FILES.includes(file)) {
      all = true;
      continue;
    }
    if (file.startsWith('specifications/')) specification = true;
    if (file.startsWith('web/')) web = true;
    if (file.startsWith('scripts/')) scripts = true;
    const owner = ownerOf(file, graph);
    if (owner) note(owner, file);
    let matched = Boolean(owner);
    for (const reader of READERS) {
      if (file.startsWith(reader.prefix)) {
        matched = true;
        for (const p of reader.packages) note(p, `${file} (${reader.why})`);
      }
    }
    if (!matched && !file.startsWith('web/') && !file.startsWith('scripts/') && !file.startsWith('docs/') && !file.endsWith('.md')) other.push(file);
  }
  const packages = all ? graph.packages.map((p) => p.name).sort() : dependants(graph, hit);
  return {
    all,
    packages,
    specification,
    web,
    scripts,
    other,
    reasons: Object.fromEntries(Object.entries(reasons).map(([k, v]) => [k, [...v]])),
  };
}

/**
 * Packages every cargo run names whatever the change: the backend carries the `development` feature the
 * service tests need, and the specc crate enables `target-models` on the generated model. With them
 * selected the feature set, and so the build, is the same as the full run's: a selection without them
 * would rebuild the 80,000-line generated crate.
 */
export const ALWAYS_SELECTED = ['strata-backend', 'strata-spec-compiler'];

/**
 * The test binaries of the affected packages, from the tier list: `{package, target, tier, seconds,
 * slow}` for each; the entries with `slow` only when `options.slow`. Also the affected packages that
 * have no entry (a package with unit tests only).
 */
export function scopedTargets(scope, tiers, { slow = false } = {}) {
  const wanted = new Set(scope.packages);
  const targets = tiers.tests.filter((t) => wanted.has(t.package));
  const skipped = targets.filter((t) => t.slow && !slow);
  return { run: targets.filter((t) => !(t.slow && !slow)), skipped };
}

/**
 * The cargo invocation for a scope: unit tests (`--lib --bins`) of the selected packages and the
 * named integration tests. A `--test` name applies to every selected package that has one, so a name
 * two packages share runs in both.
 */
export function cargoArgs(scope, selected, { features = 'strata-backend/development' } = {}) {
  const packages = [...new Set([...scope.packages, ...ALWAYS_SELECTED])].sort();
  const names = [...new Set(selected.run.map((t) => t.target))].sort();
  return [
    'test',
    ...packages.flatMap((p) => ['-p', p]),
    '--lib',
    '--bins',
    ...names.flatMap((n) => ['--test', n]),
    '--no-fail-fast',
    '--features',
    features,
  ];
}
