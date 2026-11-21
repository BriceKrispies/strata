// Every file path the specification cites exists (WG-5038). A path is cited when a string of the
// specification holds one that starts at a directory of the repository (crates, web, tests, scripts,
// lean, docs, tenants, specifications) or names a root file. A trailing `:line`, a glob and a
// trailing dot are not part of the path; a directory counts as existing.

const ROOTS = ['crates', 'web', 'tests', 'scripts', 'lean', 'docs', 'tenants', 'specifications', '.github'];
const ROOT_FILES = ['Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json', 'gate.json', 'rust-toolchain.toml'];
const PATH = new RegExp(
  `(?<![A-Za-z0-9_./-])((?:${ROOTS.map((r) => r.replace('.', '\\.')).join('|')})/[A-Za-z0-9_.\\-/]*[A-Za-z0-9_]|${ROOT_FILES.map((f) => f.replace(/\./g, '\\.')).join('|')})(?![A-Za-z0-9_-])`,
  'g',
);

// Every string in a JSON value.
function* strings(value) {
  if (typeof value === 'string') yield value;
  else if (Array.isArray(value)) for (const item of value) yield* strings(item);
  else if (value && typeof value === 'object') for (const item of Object.values(value)) yield* strings(item);
}

// The distinct paths the specification (a parsed JSON value) cites, in order of first appearance.
export function citedPaths(spec) {
  const found = new Set();
  for (const text of strings(spec)) for (const m of text.matchAll(PATH)) found.add(m[1]);
  return [...found];
}

// The cited paths for which `exists` is false.
export function missingPaths(paths, exists) {
  return paths.filter((path) => !exists(path));
}
