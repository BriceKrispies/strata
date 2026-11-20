// Drives the generated JavaScript machine through an Exegra trace and compares it with the trace
// after every step: the state, the variables, the emits (with receiver and delivery), the timers
// set and the timers pending. Used by tests/web.rs.
//
//   node web-differential.mjs <generated dir> <machine id> <trace.json> <spec.json>
//
// <trace.json> is the `result` of `exegra trace`, <spec.json> the specification as JSON. Exegra's
// time is read as milliseconds.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const [dir, machineId, tracePath, specPath] = process.argv.slice(2);
const machine = await import(pathToFileURL(`${dir}/${machineId}.js`).href);
const trace = JSON.parse(readFileSync(tracePath, 'utf8'));
const spec = JSON.parse(readFileSync(specPath, 'utf8'));
const behavior = spec.behaviors.find((b) => b.id === machineId);
assert.ok(behavior, `no behavior ${machineId} in the specification`);

/** A payload without its absent (null) optional fields, as both sides leave them out. */
const norm = (payload) =>
  Object.fromEntries(Object.entries(payload ?? {}).filter(([, v]) => v !== null && v !== undefined));
const eventDecl = (id) => spec.events.find((e) => e.id === id);
const prefix = `${machineId}/`;

function checkMachine(context, state, vars, config) {
  const m = config.machines[machineId];
  assert.ok(m, `${context}: no machine ${machineId} in the configuration`);
  assert.equal(state, m.state, `${context}: state`);
  assert.deepStrictEqual(vars, m.vars, `${context}: variables`);
}

function pendingTimers(config) {
  return Object.keys(config.timers ?? {})
    .filter((id) => id.startsWith(prefix))
    .map((id) => id.slice(prefix.length))
    .sort();
}

assert.equal(trace.outcome, 'quiescent', 'the scenario must end');

let { state, vars } = machine.initial('the-instance');
checkMachine('initially', state, vars, trace.initial);

const pending = new Set();
let firedSteps = 0;
for (const step of trace.steps) {
  const time = step.time;
  const id = step.event.id;
  const context = `step ${step.index} (t=${time}, ${id})`;
  const fired = step.fired.find((f) => f.behavior === machineId);
  if (typeof step.event.timer === 'string' && step.event.timer.startsWith(prefix)) {
    pending.delete(step.event.timer.slice(prefix.length));
  }

  // Exegra delivers an event only to the machines that react to it (its `fired` and `ignored`
  // lists); the generated machine refuses any other event, so it is not fed one.
  const ignoredHere = (step.ignored ?? []).some((i) => i.behavior === machineId);
  if (!fired && !ignoredHere) {
    checkMachine(`${context}, not delivered`, state, vars, step.config);
    continue;
  }

  const before = JSON.stringify(vars);
  let outcome;
  try {
    outcome = machine.step(state, vars, { event: id, payload: step.event.payload ?? {} }, time);
  } catch (error) {
    // Exegra lists every machine of the scenario that did not fire as ignored, including one that
    // has no transition on the event at all. The generated machine refuses an event it does not
    // receive (as the Rust machine does), which is the same outcome: nothing changes. Any other
    // error, or a refusal where Exegra fired, is a disagreement.
    if (!fired && error?.name === 'StepError' && error.kind === 'invalid-event') {
      checkMachine(`${context}, not received`, state, vars, step.config);
      continue;
    }
    throw error;
  }
  assert.equal(JSON.stringify(vars), before, `${context}: the step changed its input variables`);

  if (!fired) {
    assert.deepStrictEqual(outcome, { ignored: true }, `${context}: expected the event to be ignored`);
    checkMachine(context, state, vars, step.config);
    continue;
  }
  firedSteps += 1;
  assert.ok(!outcome.ignored, `${context}: ignored, but Exegra fired ${fired.transition}`);
  assert.equal(outcome.state, fired.to, `${context}: target state`);
  checkMachine(context, outcome.state, outcome.vars, step.config);

  const expectedEmits = [];
  const expectedTimers = [];
  // Exegra lists, in order, what each emit effect and each timer effect of the transition
  // produced; which one an entry is follows from the effect's kind, not from its event.
  const firedTransition = behavior.transitions.find((t) => t.id === fired.transition);
  const producing = (firedTransition.effects ?? []).filter((e) => e.timer || e.emit);
  assert.equal(
    producing.length,
    fired.emitted.length,
    `${context}: Exegra's emitted entries and the effects of ${fired.transition} that emit or set a timer`,
  );
  for (const [i, e] of fired.emitted.entries()) {
    assert.equal(producing[i].emit, e.event, `${context}: effect ${i} of ${fired.transition} produces ${e.event}`);
    if (producing[i].timer) {
      expectedTimers.push({ event: e.event, payload: norm(e.payload), afterMs: e.time - time });
    } else {
      expectedEmits.push({ event: e.event, payload: norm(e.payload) });
    }
  }
  assert.deepStrictEqual(
    outcome.emits.map((e) => ({ event: e.event, payload: norm(e.payload) })),
    expectedEmits,
    `${context}: emitted events`,
  );
  const transition = behavior.transitions.find((t) => t.id === fired.transition);
  const receivers = (transition.effects ?? []).filter((e) => e.emit && !e.timer);
  assert.equal(outcome.emits.length, receivers.length, `${context}: emits of ${fired.transition}`);
  outcome.emits.forEach((emit, i) => {
    assert.equal(emit.to, receivers[i].to, `${context}: receiver of ${emit.event}`);
    assert.equal(emit.delivery, eventDecl(emit.event).delivery, `${context}: delivery of ${emit.event}`);
  });

  const set = [];
  for (const command of outcome.timers) {
    if ('cancel' in command) {
      pending.delete(command.cancel);
    } else {
      set.push({ event: command.event, payload: norm(command.payload), afterMs: command.afterMs });
      pending.add(command.set);
    }
  }
  assert.deepStrictEqual(set, expectedTimers, `${context}: timers set`);
  assert.deepStrictEqual([...pending].sort(), pendingTimers(step.config), `${context}: timers pending`);

  state = outcome.state;
  vars = outcome.vars;
}
checkMachine('finally', state, vars, trace.final);
assert.ok(firedSteps >= 8, `the scenario should exercise the machine; it fired ${firedSteps} times`);
console.log(`ok: ${firedSteps} fired steps of ${trace.steps.length}`);
