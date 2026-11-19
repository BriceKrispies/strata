// Steps the generated JavaScript machines on request, for tests/strata_differential.rs.
//
//   node strata-differential-node.mjs <generated dir>
//
// Reads one JSON request per line on stdin and answers each with one JSON line:
//
//   { "op": "initial", "machine": id, "key": ... }
//   { "op": "step", "machine": id, "state": ..., "vars": ..., "event": { event, payload }, "now": ms }
//
// The answer is { "ok": <what the machine returned> }, { "refused": { kind, message } } when the
// machine threw a StepError (an event it does not receive, a payload not of its types, an
// overflow), or { "crashed": <text> } for any other error. The machines are stateless: the test
// holds the state and variables and sends them with every step.

import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';

const dir = process.argv[2];
const modules = new Map();

async function machine(id) {
  if (!modules.has(id)) modules.set(id, await import(pathToFileURL(join(dir, `${id}.js`)).href));
  return modules.get(id);
}

for await (const line of createInterface({ input: process.stdin })) {
  if (line.trim() === '') continue;
  let reply;
  try {
    const request = JSON.parse(line);
    const m = await machine(request.machine);
    if (request.op === 'initial') {
      reply = { ok: m.initial(request.key) };
    } else {
      reply = { ok: m.step(request.state, request.vars, request.event, request.now) };
    }
  } catch (error) {
    reply = error && error.name === 'StepError'
      ? { refused: { kind: error.kind, message: error.message } }
      : { crashed: String((error && error.stack) || error) };
  }
  process.stdout.write(`${JSON.stringify(reply)}\n`);
}
