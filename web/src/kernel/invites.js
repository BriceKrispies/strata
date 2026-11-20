// The invitation of a new tenant's owner: the link the operator's message carries
// (`{origin}/login#i=<challenge>&t=<token>`), the form that chooses a password, and the sending of
// both to `accept-owner-invitation`. Pure, like the rest of the kernel. The link carries its
// secrets in the fragment of its address, which the browser does not send anywhere (D-703): the
// page reads them once, takes them out of the address bar at once, and sends them in the body of
// the request. The wording of every refusal is the specification's (`errorMessages` of
// `form-accept-invitation`); the server answers a wrong, a used, an expired and a foreign-origin
// invitation alike, so the page says one thing for all four. Accepting signs nobody in: the owner
// is sent on to sign in, with the login the reply names.
// @ts-check

import * as contracts from '../generated/contracts.js';
import * as refusals from '../generated/errors.js';
import * as forms from '../generated/forms.js';
import { SIGN_IN_PATH } from '../generated/screens.js';

export const INVITE_FORM = 'form-accept-invitation';

export function invitesInit() {
  return {
    /** The invitation this page was opened with: `{ challenge, token }`; null when it holds none. */
    landing: /** @type {null | { challenge: string, token: string }} */ (null),
    /** What was typed in the password field. */
    password: '',
    /** The message under the password field, from the form's own validation or the server. */
    fieldError: '',
    /** A refusal that is about the form as a whole. */
    error: '',
    busy: false,
    /** The link cannot be used: the form is not shown, the message says to ask for a new one. */
    failed: false,
  };
}

function withInvites(state, patch) {
  return { ...state, invites: { ...state.invites, ...patch } };
}

const HEX64 = /^[0-9a-f]{64}$/i;

/**
 * Whether a fragment is an invitation's (it names an `i`), and the invitation it holds, which is
 * null when what it names is malformed.
 * @param {string} hash
 * @returns {{ named: boolean, invitation: { challenge: string, token: string } | null }}
 */
export function inviteFromHash(hash) {
  const params = new URLSearchParams(hash.replace(/^#/, ''));
  const challenge = params.get('i');
  if (challenge === null) return { named: false, invitation: null };
  const token = params.get('t') ?? '';
  return { named: true, invitation: HEX64.test(challenge) && HEX64.test(token) ? { challenge, token } : null };
}

/**
 * The refusal of a link that is not an invitation at all: not worth a request.
 * @returns {string}
 */
function unusable() {
  return refusals.refusal(INVITE_FORM, { code: 'unauthenticated' }).formError;
}

/**
 * Take in the invitation the page was opened with: keep it, and have it taken out of the address
 * bar at once. A fragment that names an invitation but is malformed is taken out as well, and the
 * page says the link was not accepted.
 * @returns {{ state: any, effects: any[] }}
 */
export function invitesOpened(state, hash, href) {
  if (state.route.path !== SIGN_IN_PATH) return { state, effects: [] };
  const { named, invitation } = inviteFromHash(hash);
  if (!named) return { state, effects: [] };
  const effects = [{ type: 'push_url', href, replace: true }];
  if (!invitation) return { state: withInvites(state, { landing: null, failed: true, error: unusable() }), effects };
  return { state: withInvites(state, { landing: invitation, password: '', fieldError: '', error: '', failed: false }), effects };
}

/** Typing in the password field. */
export function invitesInput(state, name, value) {
  return name === 'password' ? withInvites(state, { password: String(value), fieldError: '', error: '' }) : state;
}

/**
 * The form was submitted, or its request was answered. `scopeOf` gives the scope the generated
 * expressions read; `navigate` carries the page on to the address the form's success names.
 * @param {any} state @param {any} msg
 * @param {{ call: Function, scopeOf: Function, toast: Function, navigate: Function }} helpers
 * @returns {{ state: any, effects: any[] } | null}
 */
export function invitesUpdate(state, msg, { call, scopeOf, toast, navigate }) {
  const held = state.invites.landing;
  if (msg.type === 'submit') {
    if (!held || state.invites.busy) return { state, effects: [] };
    const values = forms.submitted(INVITE_FORM, msg.values);
    const errors = forms.validate(INVITE_FORM, values);
    if (errors.password) {
      return { state: { ...withInvites(state, { password: values.password, fieldError: errors.password, error: '' }), announce: errors.password }, effects: [] };
    }
    const scope = scopeOf(state, { form: values, local: { invitation_challenge: held.challenge, invitation_token: held.token } });
    const [next, effect] = call(
      withInvites(state, { password: '', fieldError: '', error: '', busy: true }),
      'invites.accept',
      forms.FORMS[INVITE_FORM].contract,
      forms.inputs(INVITE_FORM, values, scope),
      msg,
      { values: { ...values, password: '' } },
    );
    return { state: next, effects: [effect] };
  }

  if (msg.type !== 'http_result') return null;
  if (!msg.ok) {
    const { errors, formError } = refusals.refusal(INVITE_FORM, msg.error);
    // The server's one answer for every invitation it will not take; the link is of no more use.
    if (msg.error?.code === 'unauthenticated') {
      return { state: { ...withInvites(state, { landing: null, busy: false, failed: true, error: formError }), announce: formError }, effects: [] };
    }
    const fieldError = errors.password ?? '';
    return {
      state: { ...withInvites(state, { busy: false, fieldError, error: fieldError ? '' : formError }), announce: fieldError || formError },
      effects: [],
    };
  }

  // Accepted: the form is spent, and the owner goes on to sign in as the reply says.
  const outputs = msg.outputs ?? contracts.decode(msg.pending.contract, msg.body, msg.pending.inputs);
  const login = String(outputs?.login ?? '');
  const outcome = forms.succeeded(INVITE_FORM, scopeOf(state, { form: msg.pending.values, response: outputs }));
  let next = withInvites(state, { landing: null, busy: false, failed: false, error: '', fieldError: '', password: '' });
  next = { ...next, forms: { ...next.forms, login: { ...next.forms.login, error: '', values: { ...next.forms.login.values, login, password: '' } } } };
  if (outcome.toast) next = toast(next, 'ok', outcome.toast);
  const moved = navigate(next, outcome.navigate ?? SIGN_IN_PATH ?? '/', true, msg);
  return { state: { ...moved.state, focus: { target: 'login-password', token: msg.id } }, effects: moved.effects };
}
