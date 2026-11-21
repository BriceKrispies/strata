// Every requirement id a Rust test names with `Verifies:` at one git revision is still named by a test
// at another (WG-5355: a trim may delete tests, never the last evidence of a requirement).
//
//   node scripts/verifies-kept.mjs <before-rev> [<after-rev>]   (after defaults to the working tree)
//
// Reads crates/**/*.rs at both revisions; prints the ids that disappeared and exits 1 when any did.
import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const ID = /Verifies:\s*([A-Za-z0-9_, -]+)/g;

function idsIn(text, into) {
  for (const m of text.matchAll(ID)) {
    for (const id of m[1].split(/[,\s]+/)) if (/^[A-Za-z]+-\d+$/.test(id)) into.add(id.toUpperCase());
  }
}

function atRev(rev) {
  const ids = new Set();
  const files = execFileSync('git', ['ls-tree', '-r', '--name-only', rev, 'crates'], { encoding: 'utf8', maxBuffer: 1 << 26 })
    .split('\n').filter((f) => f.endsWith('.rs'));
  for (const f of files) idsIn(execFileSync('git', ['show', `${rev}:${f}`], { encoding: 'utf8', maxBuffer: 1 << 26 }), ids);
  return ids;
}

function inTree(dir, ids = new Set()) {
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) { if (e !== 'target') inTree(p, ids); }
    else if (e.endsWith('.rs')) idsIn(readFileSync(p, 'utf8'), ids);
  }
  return ids;
}

const [before, after] = process.argv.slice(2);
if (!before) { console.error('usage: node scripts/verifies-kept.mjs <before-rev> [<after-rev>]'); process.exit(2); }
const was = atRev(before);
const now = after ? atRev(after) : inTree('crates');
const lost = [...was].filter((id) => !now.has(id)).sort();
console.log(`${was.size} ids before, ${now.size} after, ${lost.length} lost`);
for (const id of lost) console.log(`lost: ${id}`);
process.exit(lost.length ? 1 : 0);
