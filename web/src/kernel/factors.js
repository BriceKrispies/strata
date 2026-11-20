// Second factors, recovery codes and step-up. Pure, like the rest of the kernel.
//
// Three things live here, all of them the gateway's (`/api/factors...`, `/api/session/step-up`,
// `/api/sign-in/second-factor`):
//
// - the code prompt at sign-in: a first factor that is not enough answers with a waiting sign-in,
//   and the page asks for a code, a recovery code or a passkey before there is a session;
// - the desk's section: add an authenticator (a QR code and the secret in text), see how many
//   recovery codes are left, make a new set, which is shown once, and remove the authenticator;
// - the step-up prompt: an operation that needs a fresh authentication is refused with the kind
//   `step_up_required`; the page asks for a password or a code, and then asks again.
//
// The QR code is drawn from `uqr`, a small maintained encoder with no dependencies, which turns
// the `otpauth` address into a matrix of modules; the secret is also shown as text for people who
// type it.
// @ts-check

import { encode } from 'uqr';

export function factorsInit() {
  return {
    loaded: false,
    loading: false,
    /** `{ totp, passkeys, recovery_codes, realm_requires }` from the gateway. */
    summary: /** @type {any} */ (null),
    error: '',
    busy: false,
    /** An authenticator being added: its secret, address, QR matrix and the code typed. */
    enrol: /** @type {null | { secret: string, uri: string, qr: boolean[][], error: string }} */ (null),
    /** A new set of recovery codes, shown once until it is dismissed. */
    codes: /** @type {null | string[]} */ (null),
    /** The waiting sign-in the login page answers. */
    prompt: /** @type {any} */ (null),
    /** An operation that was refused for want of a fresh authentication, to be asked again. */
    stepUp: /** @type {null | { retry: { name: string, arg?: any }, error: string, busy: boolean }} */ (null),
    /** What is typed into the three forms. */
    fields: { code: '', confirm: '', secret: '' },
    /** The passkey ceremony in flight for a second factor. */
    ceremony: /** @type {null | { id: number, passkeyChallenge: string }} */ (null),
  };
}

function withFactors(state, patch) {
  return { ...state, factors: { ...state.factors, ...patch } };
}

/** The matrix of an address, with a quiet border of two modules. */
function matrix(uri) {
  return encode(uri, { ecc: 'M', border: 2 }).data;
}

/** Ask for the person's factors, when the desk is shown and someone is signed in. */
export function factorsEnter(state, msg, { request }) {
  if (!state.session?.authenticated || state.factors.loading) return { state, effects: [] };
  const [next, effect] = request(withFactors(state, { loading: true, error: '' }), 'factors.summary', { path: '/api/factors' }, msg);
  return { state: next, effects: [effect] };
}

/**
 * A first factor was right and a second is needed: show the prompt.
 * @param {any} state @param {any} body
 */
export function factorsWaiting(state, body) {
  const w = body.second_factor;
  const enrol = w.enrol ? { secret: w.enrol.secret, uri: w.enrol.uri, qr: matrix(w.enrol.uri), error: '' } : null;
  const methods = w.methods ?? [];
  const prompt = { challenge: w.challenge, methods, enrol, factor: methods.find((m) => m !== 'passkey') ?? 'totp', error: '', busy: false };
  return withFactors(state, { prompt, fields: { ...state.factors.fields, code: '' } });
}

/** Typing in one of the factor forms. */
export function factorsInput(state, name, value) {
  if (!(name in state.factors.fields)) return state;
  return withFactors(state, { fields: { ...state.factors.fields, [name]: String(value) } });
}

/** The refusal that asks for a fresh authentication. */
export function isStepUp(msg) {
  return msg.ok === false && msg.status === 403 && msg.error?.kind === 'step_up_required';
}

/** Show the step-up prompt for an operation that was refused, to be asked again once confirmed. */
export function factorsRequireStepUp(state, retry) {
  const idle = {
    sessions: { ...state.sessions, busy: false },
    passkeys: { ...state.passkeys, busy: false },
  };
  // Focus goes to the prompt's field, so keyboard and screen-reader users land in the dialog.
  const focus = { target: 'stepup-secret', token: state.nextId };
  return withFactors({ ...state, ...idle, nextId: state.nextId + 1, focus }, { busy: false, stepUp: { retry, error: '', busy: false }, fields: { ...state.factors.fields, secret: '' } });
}

/** An action carries its own retry, so a refused one can be asked again. */
const retryOf = (msg) => ({ retry: { name: msg.name, arg: msg.arg } });

/**
 * An action, a submit, the browser's passkey answer, or the answer to one of the requests.
 * `signedIn` carries out what follows a session, `redispatch` asks a refused action again.
 * @returns {{ state: any, effects: any[] } | null}
 */
export function factorsUpdate(state, msg, { request, toast, signedIn, redispatch }) {
  const f = state.factors;
  if (msg.type === 'action') {
    switch (msg.name) {
      case 'factors.enrol_start': {
        if (f.busy) return { state, effects: [] };
        const [next, effect] = request(withFactors(state, { busy: true, error: '' }), 'factors.enrol_start', { method: 'POST', path: '/api/factors/totp/start', body: {}, write: true, meta: retryOf(msg) }, msg);
        return { state: next, effects: [effect] };
      }
      case 'factors.enrol_cancel':
        return { state: withFactors(state, { enrol: null, fields: { ...f.fields, confirm: '' } }), effects: [] };
      case 'factors.remove_totp': {
        if (f.busy) return { state, effects: [] };
        const [next, effect] = request(withFactors(state, { busy: true, error: '' }), 'factors.remove_totp', { method: 'POST', path: '/api/factors/totp/remove', body: {}, write: true, meta: retryOf(msg) }, msg);
        return { state: next, effects: [effect] };
      }
      case 'factors.new_codes': {
        if (f.busy) return { state, effects: [] };
        const [next, effect] = request(withFactors(state, { busy: true, error: '' }), 'factors.new_codes', { method: 'POST', path: '/api/factors/recovery-codes', body: {}, write: true, meta: retryOf(msg) }, msg);
        return { state: next, effects: [effect] };
      }
      case 'factors.dismiss_codes':
        return { state: withFactors(state, { codes: null }), effects: [] };
      case 'factors.use':
        return f.prompt && typeof msg.arg?.factor === 'string'
          ? { state: withFactors(state, { prompt: { ...f.prompt, factor: msg.arg.factor, error: '' }, fields: { ...f.fields, code: '' } }), effects: [] }
          : { state, effects: [] };
      case 'factors.use_passkey': {
        if (!f.prompt || f.prompt.busy || !state.passkeys.supported) return { state, effects: [] };
        const [next, effect] = request(withFactors(state, { prompt: { ...f.prompt, busy: true, error: '' } }), 'factors.pk_challenge', { method: 'POST', path: '/api/passkeys/challenge', body: {}, write: true }, msg);
        return { state: next, effects: [effect] };
      }
      case 'factors.stepup_cancel':
        return { state: withFactors(state, { stepUp: null }), effects: [] };
      default:
        return null;
    }
  }

  if (msg.type === 'submit') {
    if (msg.form === 'second' && f.prompt && !f.prompt.busy) {
      const code = String(msg.values.code ?? '').trim();
      if (!code) return { state: withFactors(state, { prompt: { ...f.prompt, error: 'Enter the code.' } }), effects: [] };
      const [next, effect] = request(
        withFactors(state, { prompt: { ...f.prompt, busy: true, error: '' } }),
        'factors.second',
        { method: 'POST', path: '/api/sign-in/second-factor', body: { challenge: f.prompt.challenge, factor: f.prompt.factor, code }, write: true },
        msg,
      );
      return { state: next, effects: [effect] };
    }
    if (msg.form === 'totpconfirm' && f.enrol && !f.busy) {
      const code = String(msg.values.confirm ?? '').trim();
      if (!code) return { state: withFactors(state, { enrol: { ...f.enrol, error: 'Enter the code your authenticator shows.' } }), effects: [] };
      const [next, effect] = request(withFactors(state, { busy: true }), 'factors.enrol_finish', { method: 'POST', path: '/api/factors/totp/confirm', body: { code }, write: true, meta: { retry: { name: 'factors.enrol_start' } } }, msg);
      return { state: next, effects: [effect] };
    }
    if (msg.form === 'stepup' && f.stepUp && !f.stepUp.busy) {
      const secret = String(msg.values.secret ?? '');
      if (!secret) return { state: withFactors(state, { stepUp: { ...f.stepUp, error: 'Enter it to go on.' } }), effects: [] };
      const body = f.summary?.totp ? { code: secret.trim() } : { password: secret };
      const [next, effect] = request(withFactors(state, { stepUp: { ...f.stepUp, busy: true, error: '' } }), 'factors.stepup', { method: 'POST', path: '/api/session/step-up', body, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    return null;
  }

  if (msg.type === 'passkey_result') {
    if (!f.ceremony || f.ceremony.id !== msg.id) return null;
    const { passkeyChallenge } = f.ceremony;
    const cleared = withFactors(state, { ceremony: null });
    if (!msg.ok || !f.prompt) {
      return { state: withFactors(cleared, { prompt: f.prompt ? { ...f.prompt, busy: false, error: msg.cancelled ? '' : 'The passkey was not used.' } : null }), effects: [] };
    }
    const [next, effect] = request(
      cleared,
      'factors.second',
      { method: 'POST', path: '/api/sign-in/second-factor', body: { challenge: f.prompt.challenge, factor: 'passkey', passkey_challenge: passkeyChallenge, assertion: msg.credential }, write: true },
      msg,
    );
    return { state: next, effects: [effect] };
  }

  if (msg.type !== 'http_result') return null;
  const kind = msg.pending.kind;
  if (!msg.ok) {
    const text = msg.error?.message || 'That could not be done.';
    switch (kind) {
      case 'factors.second':
      case 'factors.pk_challenge':
        return { state: withFactors(state, { prompt: f.prompt ? { ...f.prompt, busy: false, error: text } : null }), effects: [] };
      case 'factors.stepup':
        return { state: withFactors(state, { stepUp: f.stepUp ? { ...f.stepUp, busy: false, error: text } : null }), effects: [] };
      case 'factors.enrol_finish':
        return { state: withFactors(state, { busy: false, enrol: f.enrol ? { ...f.enrol, error: text } : null }), effects: [] };
      default:
        return { state: withFactors(state, { loading: false, busy: false, error: text }), effects: [] };
    }
  }
  switch (kind) {
    case 'factors.summary':
      return { state: withFactors(state, { loaded: true, loading: false, summary: msg.body, error: '' }), effects: [] };
    case 'factors.enrol_start':
      return { state: withFactors(state, { busy: false, enrol: { secret: msg.body.secret, uri: msg.body.uri, qr: matrix(msg.body.uri), error: '' }, fields: { ...f.fields, confirm: '' } }), effects: [] };
    case 'factors.enrol_finish': {
      const done = toast(withFactors(state, { busy: false, enrol: null, fields: { ...f.fields, confirm: '' } }), 'ok', 'Authenticator added.');
      return factorsEnter(withFactors(done, { loading: false }), msg, { request });
    }
    case 'factors.remove_totp': {
      const done = toast(withFactors(state, { busy: false }), 'ok', 'Authenticator removed.');
      return factorsEnter(withFactors(done, { loading: false }), msg, { request });
    }
    case 'factors.new_codes': {
      const done = withFactors(state, { busy: false, codes: msg.body.codes ?? [] });
      return factorsEnter(withFactors(done, { loading: false }), msg, { request });
    }
    case 'factors.pk_challenge': {
      const id = state.nextId;
      return {
        state: withFactors({ ...state, nextId: id + 1 }, { ceremony: { id, passkeyChallenge: msg.body.challenge } }),
        effects: [{ type: 'passkey', op: 'get', id, options: msg.body.options }],
      };
    }
    case 'factors.second':
      return signedIn(withFactors(state, { prompt: null, fields: { ...f.fields, code: '' } }), msg);
    case 'factors.stepup': {
      // Confirmed: the operation that was refused is asked again.
      const retry = f.stepUp?.retry;
      const cleared = withFactors(state, { stepUp: null, fields: { ...f.fields, secret: '' } });
      return retry ? redispatch(cleared, retry, msg) : { state: cleared, effects: [] };
    }
    default:
      return { state, effects: [] };
  }
}
