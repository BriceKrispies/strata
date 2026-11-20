// Sign in at an identity provider: the buttons the page offers, the start, and the return. Pure,
// like the rest of the kernel. The providers come from `/api/sign-in/providers` (the ones the
// site's realm configured and enabled). Pressing a button asks `/api/sign-in/oidc/start` for the
// provider's address and the main thread sends the browser there. The provider sends the browser
// back to the gateway, which moves the code and the state into the fragment of this page's
// address (`#s=<state>&x=<code>`); the page takes them out of the address bar once and sends them
// in the body of `/api/sign-in/oidc`. The nonce that binds the sign-in to this browser is a cookie
// the page cannot read, so it is not in anything here.
//
// A SAML provider is the same page flow with its own names: the list says which protocol a
// provider is signed in with, the start is `/api/sign-in/saml/start`, and the provider's POST to
// the gateway comes back as `#m=saml&r=<response>&t=<relay state>`, which the page sends in the
// body of `/api/sign-in/saml`.
// @ts-check

import { SIGN_IN_PATH } from '../generated/screens.js';

export function oidcInit() {
  return {
    /** The providers the site offers, or `null` until they have been asked for. */
    providers: /** @type {null | { id: string, label: string, protocol: 'oidc' | 'saml' }[]} */ (null),
    asked: false,
    busy: false,
    error: '',
    /** The answer the provider sent the page back with, until it has been sent. */
    landing: /** @type {null | { state: string, code: string } | { saml: true, response: string, relay: string }} */ (null),
    /** `working` while the answer is being checked, `failed` when it was not accepted. */
    landingStatus: /** @type {'' | 'working' | 'failed'} */ (''),
    landingError: '',
  };
}

function withOidc(state, patch) {
  return { ...state, oidc: { ...state.oidc, ...patch } };
}

const STATE = /^[0-9a-f]{64}$/i;
const CODE = /^[\x21-\x7e]{1,2048}$/;
const SAML_RESPONSE = /^[A-Za-z0-9+/=]{1,49152}$/;

/**
 * What a page was opened with after the provider: the answer, or that the provider sent none.
 * @param {string} hash
 * @returns {{ state: string, code: string } | { saml: true, response: string, relay: string } | { failed: true } | null}
 */
export function oidcFromHash(hash) {
  const params = new URLSearchParams(hash.replace(/^#/, ''));
  if (params.get('e')) return { failed: true };
  if (params.get('m') === 'saml') {
    const response = params.get('r') ?? '';
    const relay = params.get('t') ?? '';
    return SAML_RESPONSE.test(response) && STATE.test(relay) ? { saml: true, response, relay } : null;
  }
  const stateValue = params.get('s') ?? '';
  const code = params.get('x') ?? '';
  return STATE.test(stateValue) && CODE.test(code) ? { state: stateValue, code } : null;
}

/**
 * Take in what the page was opened with: keep it, and have it taken out of the address bar.
 * @returns {{ state: any, effects: any[] }}
 */
export function oidcOpened(state, hash, href) {
  const opened = state.route.path === SIGN_IN_PATH ? oidcFromHash(hash) : null;
  if (!opened) return { state, effects: [] };
  const replace = [{ type: 'push_url', href, replace: true }];
  if ('failed' in opened) {
    return {
      state: withOidc(state, { landingStatus: 'failed', landingError: 'The identity provider did not sign you in.' }),
      effects: replace,
    };
  }
  return { state: withOidc(state, { landing: opened, landingStatus: 'working', landingError: '' }), effects: replace };
}

/**
 * Send the answer, once the request token every sign-in carries is known.
 * @returns {{ state: any, effects: any[] }}
 */
export function oidcLand(state, msg, { request }) {
  const landing = state.oidc.landing;
  if (!landing || !state.session?.csrf_token) return { state, effects: [] };
  const [next, effect] = request(
    withOidc(state, { landing: null }),
    'oidc.sign_in',
    'saml' in landing
      ? { method: 'POST', path: '/api/sign-in/saml', body: { response: landing.response, relay_state: landing.relay }, write: true }
      : { method: 'POST', path: '/api/sign-in/oidc', body: { state: landing.state, code: landing.code }, write: true },
    msg,
  );
  return { state: next, effects: [effect] };
}

/**
 * Ask which providers the site offers, when the sign-in page is shown.
 * @returns {{ state: any, effects: any[] }}
 */
export function oidcEnter(state, msg, { request }) {
  if (state.route.path !== SIGN_IN_PATH || state.oidc.asked) return { state, effects: [] };
  const [next, effect] = request(withOidc(state, { asked: true }), 'oidc.providers', { path: '/api/sign-in/providers' }, msg);
  return { state: next, effects: [effect] };
}

/** The list as the page may use it: only what is a name and a label. */
function listed(body) {
  const list = Array.isArray(body?.providers) ? body.providers : [];
  return list
    .filter((p) => p && typeof p.id === 'string' && typeof p.label === 'string')
    .map((p) => ({ id: p.id, label: p.label, protocol: p.protocol === 'saml' ? 'saml' : 'oidc' }));
}

/**
 * A button was pressed, or a request of the sign-in was answered.
 * `signedIn` carries out what follows a sign-in, which is the page's own.
 * @returns {{ state: any, effects: any[] } | null}
 */
export function oidcUpdate(state, msg, { request, signedIn }) {
  if (msg.type === 'action') {
    if (msg.name !== 'oidc.start') return null;
    const provider = msg.arg?.provider;
    const protocol = msg.arg?.protocol === 'saml' ? 'saml' : 'oidc';
    if (state.oidc.busy || typeof provider !== 'string') return { state, effects: [] };
    const body = { provider, ...(state.route.query.next ? { next: state.route.query.next } : {}) };
    const [next, effect] = request(
      withOidc(state, { busy: true, error: '' }),
      'oidc.start',
      { method: 'POST', path: `/api/sign-in/${protocol}/start`, body, write: true },
      msg,
    );
    return { state: next, effects: [effect] };
  }

  if (msg.type !== 'http_result') return null;
  switch (msg.pending.kind) {
    case 'oidc.providers':
      return { state: withOidc(state, { providers: msg.ok ? listed(msg.body) : [] }), effects: [] };
    case 'oidc.start':
      if (!msg.ok || typeof msg.body?.redirect !== 'string') {
        return { state: withOidc(state, { busy: false, error: msg.error?.message || 'That provider could not be reached. Try another way to sign in.' }), effects: [] };
      }
      return { state, effects: [{ type: 'navigate_away', href: msg.body.redirect }] };
    default:
      // The answer was sent for checking.
      if (!msg.ok) {
        return {
          state: withOidc(state, { landingStatus: 'failed', landingError: msg.error?.message || 'The sign-in with the provider was not accepted.' }),
          effects: [],
        };
      }
      return signedIn(withOidc(state, { landingStatus: '', landingError: '' }), msg);
  }
}
