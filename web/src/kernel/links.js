// Emailed sign-in links: ask for one, and sign in with the one that was opened. Pure, like the rest
// of the kernel. Asking is `/api/sign-in/link/request`; the answer is the same whether or not the
// address has an account, so this module never says which. The link carries its token in the
// fragment of its address, which the browser does not send anywhere (D-703): the page the link
// opens reads it once, takes it out of the address bar, and sends it in the body of
// `/api/sign-in/link`. The nonce that binds the link to this browser is a cookie the page cannot
// read, so it is not in anything here.
// @ts-check

export function linksInit() {
  return {
    /** What was typed in the request form. */
    email: '',
    busy: false,
    /** An answer to a request has come: the page says a link is on its way, whoever asked. */
    sent: false,
    error: '',
    /** The link this page was opened with, until it has been sent: `{ challenge, token }`. */
    landing: /** @type {null | { challenge: string, token: string }} */ (null),
    /** `working` while the link is being checked, `failed` when it was not accepted. */
    landingStatus: /** @type {'' | 'working' | 'failed'} */ (''),
    landingError: '',
  };
}

function withLinks(state, patch) {
  return { ...state, links: { ...state.links, ...patch } };
}

const HEX64 = /^[0-9a-f]{64}$/i;

/**
 * The link a page was opened with, from the fragment of its address (`#c=<challenge>&t=<token>`).
 * @param {string} hash
 * @returns {{ challenge: string, token: string } | null}
 */
export function linkFromHash(hash) {
  const params = new URLSearchParams(hash.replace(/^#/, ''));
  const challenge = params.get('c') ?? '';
  const token = params.get('t') ?? '';
  return HEX64.test(challenge) && HEX64.test(token) ? { challenge, token } : null;
}

/**
 * Take in the link the page was opened with: keep it, and have it taken out of the address bar.
 * @returns {{ state: any, effects: any[] }}
 */
export function linksOpened(state, hash, href) {
  const landing = state.route.path === '/login' ? linkFromHash(hash) : null;
  if (!landing) return { state, effects: [] };
  return {
    state: withLinks(state, { landing, landingStatus: 'working', landingError: '' }),
    effects: [{ type: 'push_url', href, replace: true }],
  };
}

/**
 * Send the link, once the request token every sign-in carries is known.
 * @returns {{ state: any, effects: any[] }}
 */
export function linksLand(state, msg, { request }) {
  const landing = state.links.landing;
  if (!landing || !state.session?.csrf_token) return { state, effects: [] };
  const [next, effect] = request(
    withLinks(state, { landing: null }),
    'links.sign_in',
    { method: 'POST', path: '/api/sign-in/link', body: { challenge: landing.challenge, token: landing.token }, write: true },
    msg,
  );
  return { state: next, effects: [effect] };
}

/** The address typed is plausible enough to send. */
function plausible(email) {
  return /^[^\s@]+@[^\s@]+$/.test(email.trim());
}

/** Typing in the request form. */
export function linksInput(state, name, value) {
  return name === 'email' ? withLinks(state, { email: String(value), sent: false, error: '' }) : state;
}

/**
 * The request form was submitted, or a request of the links was answered.
 * `signedIn` carries out what follows a sign-in, which is the page's own.
 * @returns {{ state: any, effects: any[] } | null}
 */
export function linksUpdate(state, msg, { request, signedIn }) {
  if (msg.type === 'submit') {
    const email = String(msg.values.email ?? '').trim();
    if (state.links.busy) return { state, effects: [] };
    if (!plausible(email)) {
      return { state: withLinks(state, { email, sent: false, error: 'Enter the email address of your account.' }), effects: [] };
    }
    const body = { email, ...(state.route.query.next ? { next: state.route.query.next } : {}) };
    const [next, effect] = request(
      withLinks(state, { email, busy: true, sent: false, error: '' }),
      'links.request',
      { method: 'POST', path: '/api/sign-in/link/request', body, write: true },
      msg,
    );
    return { state: next, effects: [effect] };
  }

  if (msg.type !== 'http_result') return null;
  if (msg.pending.kind === 'links.request') {
    return msg.ok
      ? { state: withLinks(state, { busy: false, sent: true, error: '' }), effects: [] }
      : { state: withLinks(state, { busy: false, error: msg.error?.message || 'The link could not be requested.' }), effects: [] };
  }
  // The link was sent for checking.
  if (!msg.ok) {
    return {
      state: withLinks(state, { landingStatus: 'failed', landingError: msg.error?.message || 'The link was not accepted. Ask for a new one.' }),
      effects: [],
    };
  }
  return signedIn(withLinks(state, { landingStatus: '', landingError: '' }), msg);
}
