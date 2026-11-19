// Records a saved Exegra trace with the specification's semantic hash:
//
//   node scripts/record-trace.mjs <scenario> --project <id> --specs <dir> --out <file>
//
// EXEGRA_HOME (default ../exegra) names the Exegra checkout. The file is Exegra's `result` with
// sorted keys and a `semanticHash` field, which the gate compares with Exegra's current hash.
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { recordedTrace } from './spec-hash.mjs';

const [scenario, ...rest] = process.argv.slice(2);
const flag = (name) => {
  const at = rest.indexOf(name);
  return at >= 0 ? rest[at + 1] : undefined;
};
const project = flag('--project');
const specs = flag('--specs');
const out = flag('--out');
if (!scenario || !project || !specs || !out) {
  console.error('usage: node scripts/record-trace.mjs <scenario> --project <id> --specs <dir> --out <file>');
  process.exit(2);
}
const exegraHome = resolve(import.meta.dirname, '..', process.env.EXEGRA_HOME ?? '../exegra');
const trace = recordedTrace(exegraHome, scenario, project, resolve(specs));
writeFileSync(resolve(out), `${JSON.stringify(trace, null, 2)}\n`);
console.log(`recorded ${scenario} at specification ${trace.semanticHash} to ${out}`);
