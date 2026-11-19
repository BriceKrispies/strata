// Passkeys: sign in with one, add one on the desk, list and remove one's own. Pure, like the rest
// of the kernel. The browser's own part (the prompt, the authenticator) is an effect the main
// thread carries out, and its answer comes back as a `passkey_result` message. Every request is
// the gateway's (`/api/passkeys...`, `/api/sign-in/passkey`); what the server keeps of a
// credential is never in what comes back, only its identifier, its name and its times.
// @ts-check

export function passkeysInit() {
  return {
    supported: false,
    loaded: false,
    loading: false,
    list: /** @type {any[]} */ ([]),
    error: '',
    busy: false,
    /** The sign-in on the login page. */
    signingIn: false,
    signInError: '',
    /** The ceremony in flight: what it is for, and the challenge the server issued. */
    ceremony: /** @type {null | { id: number, purpose: 'sign-in' | 'register', challenge: string }} */ (null),
  };
}

function withPasskeys(state, patch) {
  return { ...state, passkeys: { ...state.passkeys, ...patch } };
}

/** What a person is told when a ceremony did not complete. */
const NOT_DONE = 'The passkey was not used.';

/** Ask for the list, when the page that shows it has been loaded and signed in. */
export function passkeysEnter(state, msg, { request }) {
  if (!state.session?.authenticated || state.passkeys.loading || !state.passkeys.supported) return { state, effects: [] };
  const [next, effect] = request(withPasskeys(state, { loading: true, error: '' }), 'passkeys.list', { path: '/api/passkeys' }, msg);
  return { state: next, effects: [effect] };
}

/** Start the browser's part of a ceremony. */
function ceremony(state, purpose, challenge, op, options) {
  const id = state.nextId;
  return {
    state: withPasskeys({ ...state, nextId: id + 1 }, { ceremony: { id, purpose, challenge } }),
    effects: [{ type: 'passkey', op, id, options }],
  };
}

/**
 * An action of the passkeys, the answer to one of their requests, or the browser's answer.
 * `signedIn` carries out what follows a sign-in, which is the page's own.
 * @returns {{ state: any, effects: any[] } | null}
 */
export function passkeysUpdate(state, msg, { request, toast, signedIn }) {
  const p = state.passkeys;
  if (msg.type === 'action') {
    if (msg.name === 'passkeys.sign_in') {
      if (!p.supported || p.signingIn) return { state, effects: [] };
      const [next, effect] = request(withPasskeys(state, { signingIn: true, signInError: '' }), 'passkeys.challenge', { method: 'POST', path: '/api/passkeys/challenge', body: {}, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    if (p.busy) return { state, effects: [] };
    if (msg.name === 'passkeys.add' && p.supported) {
      const [next, effect] = request(withPasskeys(state, { busy: true, error: '' }), 'passkeys.register_start', { method: 'POST', path: '/api/passkeys/register/start', body: {}, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    if (msg.name === 'passkeys.remove' && typeof msg.arg?.id === 'string') {
      const [next, effect] = request(withPasskeys(state, { busy: true, error: '' }), 'passkeys.remove', { method: 'POST', path: '/api/passkeys/remove', body: { id: msg.arg.id }, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    return null;
  }

  if (msg.type === 'passkey_result') {
    const current = p.ceremony;
    if (!current || current.id !== msg.id) return { state, effects: [] };
    const cleared = withPasskeys(state, { ceremony: null });
    if (!msg.ok) {
      return current.purpose === 'sign-in'
        ? { state: withPasskeys(cleared, { signingIn: false, signInError: msg.cancelled ? '' : NOT_DONE }), effects: [] }
        : { state: withPasskeys(cleared, { busy: false, error: msg.cancelled ? '' : NOT_DONE }), effects: [] };
    }
    if (current.purpose === 'sign-in') {
      const [next, effect] = request(cleared, 'passkeys.sign_in', { method: 'POST', path: '/api/sign-in/passkey', body: { challenge: current.challenge, assertion: msg.credential }, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    const name = `Passkey, ${new Date(msg.at).toISOString().slice(0, 10)}`;
    const [next, effect] = request(cleared, 'passkeys.register_finish', { method: 'POST', path: '/api/passkeys/register/finish', body: { challenge: current.challenge, credential: msg.credential, name }, write: true }, msg);
    return { state: next, effects: [effect] };
  }

  if (msg.type !== 'http_result') return null;
  const kind = msg.pending.kind;
  const forSignIn = kind === 'passkeys.challenge' || kind === 'passkeys.sign_in';
  if (!msg.ok) {
    const text = msg.error?.message || 'The passkeys could not be reached.';
    return {
      state: withPasskeys(state, forSignIn ? { signingIn: false, signInError: text } : { loading: false, busy: false, error: text }),
      effects: [],
    };
  }
  switch (kind) {
    case 'passkeys.list':
      return { state: withPasskeys(state, { loaded: true, loading: false, list: msg.body.passkeys ?? [], error: '' }), effects: [] };
    case 'passkeys.challenge':
      return ceremony(state, 'sign-in', msg.body.challenge, 'get', msg.body.options);
    case 'passkeys.register_start':
      return ceremony(state, 'register', msg.body.challenge, 'create', msg.body.options);
    case 'passkeys.sign_in':
      return signedIn(withPasskeys(state, { signingIn: false, signInError: '' }), msg);
    default: {
      // An addition or a removal went through: say so, and ask for the list again.
      const text = kind === 'passkeys.remove' ? 'That passkey has been removed.' : 'Passkey added.';
      const done = toast(withPasskeys(state, { busy: false }), 'ok', text);
      return passkeysEnter(done, msg, { request });
    }
  }
}
