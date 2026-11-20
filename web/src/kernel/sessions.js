// The signed-in person's own sessions: list them, end one, end all the others. Pure, like the
// rest of the kernel. Every call is the gateway's (`/api/sessions`); the secret token of a
// session is never in what comes back, only its public identifier and its times.
// @ts-check

export function sessionsInit() {
  return { loaded: false, loading: false, list: /** @type {any[]} */ ([]), error: '', busy: false };
}

function withSessions(state, patch) {
  return { ...state, sessions: { ...state.sessions, ...patch } };
}

/** Ask for the list, when the page that shows it has been loaded and signed in. */
export function sessionsEnter(state, msg, { request }) {
  if (!state.session?.authenticated || state.sessions.loading) return { state, effects: [] };
  const [next, effect] = request(withSessions(state, { loading: true, error: '' }), 'sessions.list', { path: '/api/sessions' }, msg);
  return { state: next, effects: [effect] };
}

/**
 * An action of the sessions section, or the answer to one of its requests.
 * @returns {{ state: any, effects: any[] } | null}
 */
export function sessionsUpdate(state, msg, { request, toast }) {
  if (msg.type === 'action') {
    if (state.sessions.busy) return { state, effects: [] };
    if (msg.name === 'sessions.revoke' && typeof msg.arg?.id === 'string') {
      const [next, effect] = request(withSessions(state, { busy: true, error: '' }), 'sessions.revoke', { method: 'POST', path: '/api/sessions/revoke', body: { id: msg.arg.id }, write: true }, msg);
      return { state: next, effects: [effect] };
    }
    if (msg.name === 'sessions.revoke_others') {
      const [next, effect] = request(withSessions(state, { busy: true, error: '' }), 'sessions.revoke_others', { method: 'POST', path: '/api/sessions/revoke-others', body: {}, write: true, meta: { retry: { name: msg.name, arg: msg.arg } } }, msg);
      return { state: next, effects: [effect] };
    }
    return null;
  }
  if (msg.type !== 'http_result') return null;
  const kind = msg.pending.kind;
  if (!msg.ok) {
    const text = msg.error?.message || 'The sessions could not be reached.';
    return { state: withSessions(state, { loading: false, busy: false, error: text }), effects: [] };
  }
  if (kind === 'sessions.list') {
    return { state: withSessions(state, { loaded: true, loading: false, list: msg.body.sessions ?? [], error: '' }), effects: [] };
  }
  // An end was accepted: say so, and ask for the list again.
  const text = kind === 'sessions.revoke_others' ? (msg.body.revoked === 1 ? 'Signed out 1 other session.' : `Signed out ${msg.body.revoked ?? 0} other sessions.`) : 'That session has been signed out.';
  const done = toast(withSessions(state, { busy: false }), 'ok', text);
  return sessionsEnter(done, msg, { request });
}
