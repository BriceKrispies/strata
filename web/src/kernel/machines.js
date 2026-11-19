// The kernel's side of the generated client machines: the session, the live connection and form
// submission (web/src/generated/client-*-flow-impl.js). The machines decide; this file keeps
// their state in the kernel's, turns kernel messages into their events and their emits and
// timers into effects. Pure, like the rest of the kernel.
// @ts-check

import * as live from '../generated/client-live-flow-impl.js';
import * as session from '../generated/client-session-flow-impl.js';
import * as submit from '../generated/client-submit-flow-impl.js';

/** @typedef {import('./types.js').Effect} Effect */

// ---------------------------------------------------------------------------------------------
// The live connection.

const STATUS = { 'lv-idle-i': 'idle', 'lv-connecting-i': 'connecting', 'lv-open-i': 'open', 'lv-waiting-i': 'waiting' };

/**
 * The kernel's view of the connection: the machine, the token of each running timer, and the
 * fields the rest of the kernel and the view read (status, lastSeq with null for none, ...).
 * @param {{ machine: { state: import('../generated/client-live-flow-impl.js').ClientLiveFlowImplState, vars: import('../generated/client-live-flow-impl.js').ClientLiveFlowImplVars }, timers: Record<string, number>, timerSeq: number }} ws
 */
function project(ws) {
  const v = ws.machine.vars;
  return {
    ...ws,
    status: STATUS[ws.machine.state],
    attempts: v.attempts,
    lastSeq: v.lastSeq === 0 ? null : v.lastSeq,
    lastN: v.lastN,
    heartbeatMs: v.heartbeatMs,
    missedPongs: v.missedPongs,
  };
}

/** The connection before the page starts. */
export function liveInit() {
  return project({ machine: live.initial(), timers: {}, timerSeq: 0 });
}

/**
 * The connection with its last sequence set from outside the machine: a resync names the head
 * to read from (the machine has no event for it).
 * @param {any} ws
 * @param {number} head
 */
export function liveResync(ws, head) {
  return project({ ...ws, machine: { ...ws.machine, vars: { ...ws.machine.vars, lastSeq: head } } });
}

/**
 * Step the live connection on an event. Returns the new state and the effects the machine's
 * emits and timers ask for (a reconnect waits a random half to all of the machine's delay), and
 * a `ws_connect` when it moves from waiting to connecting; null when the machine ignores it.
 * @param {any} state
 * @param {string} event
 * @param {Record<string, unknown>} payload
 * @param {{ at: number, rand: number, token?: number }} msg
 * @returns {{ state: any, effects: Effect[] } | null}
 */
export function liveStep(state, event, payload, msg) {
  const ws = state.ws;
  const out = live.step(ws.machine.state, ws.machine.vars, /** @type {any} */ ({ event, payload }), msg.at);
  if ('ignored' in out) return null;
  /** @type {Effect[]} */
  const effects = [];
  for (const e of out.emits) {
    switch (e.event) {
      case 'ui-live-subscribe': {
        const after = /** @type {{ after: number }} */ (e.payload).after;
        effects.push({ type: 'ws_send', message: { type: 'subscribe', req: `r${state.nextId}`, sub: 'site', ...(after === 0 ? {} : { after }) } });
        break;
      }
      case 'ui-live-ack':
        effects.push({ type: 'ws_send', message: { type: 'ack', n: /** @type {{ n: number }} */ (e.payload).n } });
        break;
      case 'ui-live-ping':
        effects.push({ type: 'ws_send', message: { type: 'ping', nonce: msg.token ?? 0 } });
        break;
      case 'ui-live-close-requested':
        effects.push({ type: 'ws_close' });
        break;
      default:
        break;
    }
  }
  let timers = { ...ws.timers };
  let timerSeq = ws.timerSeq;
  for (const t of out.timers) {
    if ('cancel' in t) {
      delete timers[t.cancel];
      continue;
    }
    timerSeq += 1;
    timers = { ...timers, [t.set]: timerSeq };
    const ms = t.set === 'reconnect' ? Math.round(t.afterMs * (0.5 + msg.rand / 2)) : t.afterMs;
    effects.push({ type: 'timer', name: t.set, token: timerSeq, ms });
  }
  if (ws.machine.state === 'lv-waiting-i' && out.state === 'lv-connecting-i') {
    effects.push({ type: 'ws_connect', csrf: state.session?.csrf_token });
  }
  const next = project({ machine: { state: out.state, vars: out.vars }, timers, timerSeq });
  return { state: { ...state, ws: next }, effects };
}

/**
 * A timer of the connection fired: the machine's event when it is the running timer of that
 * name, else nothing (an earlier timer of the same name was replaced).
 * @param {any} state
 * @param {{ name: string, token: number, at: number, rand: number }} msg
 * @returns {{ state: any, effects: Effect[] } | null}
 */
export function liveTimer(state, msg) {
  const event = { heartbeat: 'ui-live-heartbeat-due', reconnect: 'ui-live-reconnect-due' }[msg.name];
  if (!event || state.ws.timers[msg.name] !== msg.token) return null;
  const { [msg.name]: _fired, ...timers } = state.ws.timers;
  return liveStep({ ...state, ws: { ...state.ws, timers } }, event, {}, msg);
}

// ---------------------------------------------------------------------------------------------
// The session.

/** The session machine before the session is read. */
export function sessionInit() {
  return session.initial();
}

/**
 * Step the session machine. Returns the new state and whether it asks for the session to be
 * read again; null when it ignores the event.
 * @param {any} state
 * @param {string} event
 * @param {Record<string, unknown>} payload
 * @param {number} at
 * @returns {{ state: any, readSession: boolean } | null}
 */
export function sessionStep(state, event, payload, at) {
  const flow = state.sessionFlow;
  const out = session.step(flow.state, flow.vars, /** @type {any} */ ({ event, payload }), at);
  if ('ignored' in out) return null;
  return {
    state: { ...state, sessionFlow: { state: out.state, vars: out.vars } },
    readSession: out.emits.some((e) => e.event === 'ui-session-requested'),
  };
}

/**
 * The session machine told that the session was read, whatever it decides.
 * @param {any} state
 * @param {boolean} authenticated
 * @param {number} at
 */
export function sessionRead(state, authenticated, at) {
  return sessionStep(state, 'ui-session-read', { authenticated: authenticated ? 1 : 0 }, at)?.state ?? state;
}

// ---------------------------------------------------------------------------------------------
// Form submission.

/** A form that has not been submitted. */
export function submitInit() {
  return submit.initial();
}

/**
 * Step a form's submission machine. Returns the new machine and whether it sends the request;
 * null when it ignores the event.
 * @param {{ state: any, vars: any }} flow
 * @param {'ui-form-submitted' | 'ui-form-accepted' | 'ui-form-refused'} event
 * @param {Record<string, unknown>} payload
 * @param {number} at
 * @returns {{ flow: { state: any, vars: any }, sends: boolean } | null}
 */
export function submitStep(flow, event, payload, at) {
  const out = submit.step(flow.state, flow.vars, /** @type {any} */ ({ event, payload }), at);
  if ('ignored' in out) return null;
  return { flow: { state: out.state, vars: out.vars }, sends: out.emits.some((e) => e.event === 'ui-request-sent') };
}

/**
 * True while a form waits for the reply to its request.
 * @param {{ state: string } | undefined} flow
 */
export function busy(flow) {
  return flow?.state === 'fs-busy-i';
}
