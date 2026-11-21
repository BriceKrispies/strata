// The five audit checks of scripts/spec-gate.mjs (WG-5038): the specification and the code agree
// on what is shipped. Nothing here compiles or runs anything; each check reads the specification
// and source text and reports a count. The decisions are the pure functions of the gate-*.mjs
// modules; this file only reads the files they look at.
//
//   shipped contracts have a handler       gate-handlers.mjs
//   created SQLite tables have a record    gate-tables.mjs
//   environment variables are declared     gate-env.mjs
//   BFF routes have a contract             gate-bff-routes.mjs
//   cited file paths exist                 gate-cited-paths.mjs

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { mountedRoutes, routesWithoutContract } from './gate-bff-routes.mjs';
import { citedPaths, missingPaths } from './gate-cited-paths.mjs';
import { envVarsRead, undeclaredEnvVars } from './gate-env.mjs';
import { shippedWithoutHandler } from './gate-handlers.mjs';
import { createdTables, tablesWithoutRecord } from './gate-tables.mjs';

export const BFF_ROUTER = 'crates/bff/src/routes.rs';
export const TABLE_SOURCES = ['crates/storage-sqlite/src/lib.rs', 'crates/runtime/src/sqlite.rs'];
// The mains read the variables by name; the shared server module reads the drain and connection ones.
export const ENV_SOURCES = ['crates/backend/src/main.rs', 'crates/bff/src/main.rs', 'crates/runtime/src/server.rs'];
// Crates whose source is generated or is a fixture of the compiler: never a handler.
const NOT_HAND_WRITTEN = (name) => name === 'model' || name === 'model-core' || name.startsWith('spec-compiler');

function rustFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...rustFiles(path));
    else if (entry.endsWith('.rs')) out.push(path);
  }
  return out;
}

// Every .rs file under src/ of a hand-written crate, as { path, text }.
export function handWrittenSources(root) {
  const crates = join(root, 'crates');
  if (!existsSync(crates)) return [];
  const out = [];
  for (const name of readdirSync(crates)) {
    const src = join(crates, name, 'src');
    if (NOT_HAND_WRITTEN(name) || !existsSync(src)) continue;
    for (const file of rustFiles(src)) out.push({ path: file.slice(root.length + 1).split('\\').join('/'), text: readFileSync(file, 'utf8') });
  }
  return out;
}

// The named files, as { path, text }, and the ones that cannot be read.
function readNamed(root, paths) {
  const sources = [];
  const unreadable = [];
  for (const path of paths) {
    try {
      sources.push({ path, text: readFileSync(join(root, path), 'utf8') });
    } catch {
      unreadable.push(path);
    }
  }
  return { sources, unreadable };
}

// The verdict of one audit check: it passes only with zero misses, and lists each miss. `total` is
// how many things the check looked at.
export function auditVerdict(name, total, misses) {
  return {
    name,
    ok: misses.length === 0,
    detail: `${total - misses.length} of ${total} ok, ${misses.length} missing`,
    lines: misses.map((miss) => `missing: ${miss}`),
  };
}

// A check that cannot look at what it checks fails, so a moved file is not a silent pass.
function unreadableVerdict(name, unreadable) {
  return { name, ok: false, detail: `cannot read ${unreadable.join(', ')}`, lines: [] };
}

// Runs the five checks on a checkout at `root`, with `spec` (the parsed specification). Returns [{ name, ok, detail, lines }].
export function auditChecks({ root, spec }) {
  const verdicts = [];
  const router = readNamed(root, [BFF_ROUTER]);
  const mounted = router.sources.length ? mountedRoutes(router.sources[0].text) : [];

  const handlerName = 'every shipped contract has a handler (route, Implements mark or function)';
  if (router.unreadable.length) {
    verdicts.push(unreadableVerdict(handlerName, router.unreadable));
  } else {
    const found = shippedWithoutHandler({ contracts: spec.contracts ?? [], functions: spec.functions ?? [], sources: handWrittenSources(root), mounted });
    verdicts.push(auditVerdict(handlerName, found.shipped, found.missing));
  }

  const tableName = 'every table the SQLite adapters create has a shipped spec record';
  const tableFiles = readNamed(root, TABLE_SOURCES);
  if (tableFiles.unreadable.length) {
    verdicts.push(unreadableVerdict(tableName, tableFiles.unreadable));
  } else {
    const found = tablesWithoutRecord(createdTables(tableFiles.sources), spec.records ?? []);
    verdicts.push(auditVerdict(tableName, found.total, found.missing));
  }

  const envName = 'every environment variable the backend and BFF read is declared in the specification';
  const envFiles = readNamed(root, ENV_SOURCES);
  if (envFiles.unreadable.length) {
    verdicts.push(unreadableVerdict(envName, envFiles.unreadable));
  } else {
    const read = envVarsRead(envFiles.sources);
    verdicts.push(auditVerdict(envName, read.length, undeclaredEnvVars(read, JSON.stringify(spec))));
  }

  const routeName = 'every route the BFF mounts has a contract with a matching bffRoute';
  if (router.unreadable.length) {
    verdicts.push(unreadableVerdict(routeName, router.unreadable));
  } else if (mounted.length === 0) {
    verdicts.push({ name: routeName, ok: false, detail: `found no route in \`fn router\` of ${BFF_ROUTER}`, lines: [] });
  } else {
    verdicts.push(auditVerdict(routeName, mounted.length, routesWithoutContract(mounted, spec.contracts ?? [])));
  }

  const cited = citedPaths(spec);
  verdicts.push(
    auditVerdict('every file path the specification cites exists', cited.length, missingPaths(cited, (path) => existsSync(join(root, path)))),
  );
  return verdicts;
}
