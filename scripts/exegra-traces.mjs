// Traces of many scenarios from one Exegra process.
//
//   node scripts/exegra-traces.mjs --exegra <checkout> --spec <spec.jsonc> --out <dir> --ids <ids.json>
//        [--shard <i>/<n>]
//
// `exegra trace <scenario>` loads and builds the 5.6 MB specification on every call, so one call per
// scenario (what crates/system-differential/tests/exegra.rs did) pays that load once per scenario.
// This script builds the model once and runs every requested scenario on it, the same
// `runScenario(model, id, {})` and the same stable JSON that `exegra trace --out` writes, to
// `<out>/<scenario>.trace.json`. Exegra is only read: its modules are imported from the checkout.
//
// `--ids` names a JSON array of scenario ids. With `--shard i/n` this process takes the ids whose
// position in the list is congruent to i modulo n (i from 0), so n processes split one list.
// Prints one JSON line: {"ok", "written": [ids], "failed": [{"id", "error"}]}. A scenario that
// cannot be run is reported and does not stop the others. Exit code 1 when any failed.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { readSpec } from './spec-parts.mjs';

/** The ids this shard takes: positions congruent to `index` modulo `count`. */
export function shardOf(ids, index, count) {
  if (!Number.isInteger(index) || !Number.isInteger(count) || count < 1 || index < 0 || index >= count) {
    throw new Error(`bad shard ${index}/${count}`);
  }
  return ids.filter((_, position) => position % count === index);
}

/** `"i/n"` as `[i, n]`. */
export function parseShard(text) {
  const m = /^(\d+)\/(\d+)$/.exec(text ?? '0/1');
  if (!m) throw new Error(`--shard wants i/n, got ${text}`);
  return [Number(m[1]), Number(m[2])];
}

function args(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith('--') || argv[i + 1] === undefined) throw new Error(`bad argument ${argv[i]}`);
    out[argv[i].slice(2)] = argv[i + 1];
  }
  return out;
}

async function main() {
  const a = args(process.argv.slice(2));
  for (const k of ['exegra', 'spec', 'out', 'ids']) if (!a[k]) throw new Error(`--${k} is required`);
  const load = (rel) => import(pathToFileURL(path.join(a.exegra, rel)).href);
  const { Project } = await load('src/core/project.js');
  const { runScenario } = await load('src/core/engine.js');
  const { stableStringify } = await load('src/core/stable-json.js');
  const [index, count] = parseShard(a.shard);
  const ids = shardOf(JSON.parse(fs.readFileSync(a.ids, 'utf8')), index, count);
  const project = new Project({
    name: null,
    specText: readSpec(a.spec).text,
    layoutText: null,
    layers: [],
  });
  if (!project.model) throw new Error('the specification has syntax errors');
  fs.mkdirSync(a.out, { recursive: true });
  const written = [];
  const failed = [];
  for (const id of ids) {
    try {
      if (!project.model.entities.get(id) || project.model.entities.get(id).kind !== 'scenario') {
        throw new Error(`${id} is not a scenario`);
      }
      const trace = runScenario(project.model, id, {});
      fs.writeFileSync(path.join(a.out, `${id}.trace.json`), `${stableStringify(trace, 2)}\n`, 'utf8');
      written.push(id);
    } catch (e) {
      failed.push({ id, error: e?.message ?? String(e) });
    }
  }
  process.stdout.write(`${JSON.stringify({ ok: failed.length === 0, written, failed })}\n`);
  process.exitCode = failed.length === 0 ? 0 : 1;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((e) => {
    process.stdout.write(`${JSON.stringify({ ok: false, written: [], failed: [], error: e?.message ?? String(e) })}\n`);
    process.exitCode = 1;
  });
}
