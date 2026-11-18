// Add a `Verifies: <ID>` line to the test or theorem that proves a requirement.
//
//   node scripts/tag-proofs.mjs <proofs.json>
//
// The file is a list of [requirement id, test function or Lean theorem name]. A proof that
// already carries the line is left alone, so the script can be run again after adding pairs.
// Concordance reads the lines; see specifications/README.md.
import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';

const proofs = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const skip = new Set(['target', 'node_modules', '.lake']);

function sources(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      if (!skip.has(entry)) sources(path, out);
    } else if (entry.endsWith('.rs') || entry.endsWith('.lean')) {
      out.push(path);
    }
  }
  return out;
}

const files = [...sources('crates'), ...sources('lean')];
let tagged = 0;
for (const [ref, name] of proofs) {
  let found = false;
  for (const file of files) {
    let text = readFileSync(file, 'utf8');
    if (text.includes(`Verifies: ${ref}`)) {
      found = true;
      break;
    }
    if (file.endsWith('.lean')) {
      if (!name.startsWith('Strata.')) continue;
      const marker = `-/\ntheorem ${name.slice('Strata.'.length)} `;
      const at = text.indexOf(marker);
      if (at < 0) continue;
      // The line goes inside the theorem's doc comment, before its closing delimiter.
      const before = text.slice(0, at).replace(/[ \t]+$/, '');
      text = `${before}\n\n    Verifies: ${ref} ${text.slice(at)}`;
    } else {
      const lines = text.split('\n');
      const at = lines.findIndex((line) => {
        const trimmed = line.trim().replace(/^pub /, '').replace(/^async /, '');
        return trimmed.startsWith(`fn ${name}(`);
      });
      if (at < 0) continue;
      let first = at;
      while (first > 0 && lines[first - 1].trim().startsWith('#[')) first -= 1;
      const indent = lines[at].slice(0, lines[at].length - lines[at].trimStart().length);
      lines.splice(first, 0, `${indent}/// Verifies: ${ref}`);
      text = lines.join('\n');
    }
    writeFileSync(file, text);
    found = true;
    tagged += 1;
    break;
  }
  if (!found) console.log(`not found: ${ref} ${name}`);
}
console.log(`tagged ${tagged} of ${proofs.length}`);
