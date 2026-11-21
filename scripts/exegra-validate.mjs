// Exegra's `validate` for many projects from one process.
//
//   node scripts/exegra-validate.mjs --exegra <checkout> --jobs <jobs.json> --out <dir> [--shard <i>/<n>]
//
// `node cli/exegra.js validate --project <name> --specs <dir>` starts Node, loads Exegra and its
// Lean kernel once per call. `--jobs` names a JSON array of {"key", "specs", "project"}; this script
// loads Exegra once and, for each job, runs the very command the CLI runs (`runCommand('validate',
// ['--project', project, '--specs', specs])`, from cli/commands.js), writing the envelope the CLI
// would print on stdout to `<out>/<key>.json`. With `--shard i/n` the process takes the jobs whose
// position is congruent to i modulo n, so n processes split one list. Exegra is only read.
// Prints one JSON line: {"ok", "written": [keys]} (ok is false only when the script itself broke;
// a project Exegra refuses is a written envelope with ok false, as for the CLI).
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

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
  for (const k of ['exegra', 'jobs', 'out']) if (!a[k]) throw new Error(`--${k} is required`);
  const m = /^(\d+)\/(\d+)$/.exec(a.shard ?? '0/1');
  if (!m || Number(m[2]) < 1 || Number(m[1]) >= Number(m[2])) throw new Error(`--shard wants i/n, got ${a.shard}`);
  const [index, count] = [Number(m[1]), Number(m[2])];
  const { runCommand } = await import(pathToFileURL(path.join(a.exegra, 'cli', 'commands.js')).href);
  const jobs = JSON.parse(fs.readFileSync(a.jobs, 'utf8')).filter((_, position) => position % count === index);
  fs.mkdirSync(a.out, { recursive: true });
  const written = [];
  for (const job of jobs) {
    const envelope = await runCommand('validate', ['--project', job.project, '--specs', job.specs], {});
    fs.writeFileSync(path.join(a.out, `${job.key}.json`), JSON.stringify(envelope), 'utf8');
    written.push(job.key);
  }
  process.stdout.write(`${JSON.stringify({ ok: true, written })}\n`);
}

main().catch((e) => {
  process.stdout.write(`${JSON.stringify({ ok: false, written: [], error: e?.message ?? String(e) })}\n`);
  process.exitCode = 1;
});
