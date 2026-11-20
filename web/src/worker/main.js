// The worker driver: the imperative shell of the frontend.
//
// It owns every network connection the application makes. It stamps each message with the time
// and a random number, runs the pure kernel, executes the effects the kernel returns, and posts
// the resulting view model to the main thread. It holds no application logic of its own.
// @ts-check

import { initialState, update } from '../kernel/update.js';
import { view } from '../kernel/view.js';

/** @typedef {import('../kernel/types.js').Effect} Effect */

let state = initialState();
let lastRendered = '';
// The newest input the main thread numbered that the kernel has taken in, sent back with every
// render so the page can tell a stale picture from a current one.
let applied = 0;
/** @type {WebSocket | null} */
let socket = null;

/** Run one message through the kernel and carry out what it asks for. */
function dispatch(message) {
  if (message.type === 'input' && typeof message.seq === 'number') applied = message.seq;
  const stamped = { ...message, at: Date.now(), rand: Math.random() };
  const result = update(state, stamped);
  state = result.state;
  for (const effect of result.effects) execute(effect);
  render();
}

function render() {
  const model = view(state);
  const encoded = JSON.stringify([model, applied]);
  if (encoded === lastRendered) return;
  lastRendered = encoded;
  self.postMessage({ type: 'render', model, applied });
}

function diag(level, event, correlation, detail) {
  if (socket?.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: 'diag', level, event, correlation, detail: String(detail).slice(0, 500) }));
  }
}

/** @param {Extract<Effect, { type: 'http' }>} effect */
async function http(effect) {
  /** @type {Record<string, string>} */
  const headers = { 'x-correlation-id': effect.correlation, accept: 'application/json' };
  if (effect.method === 'POST') {
    headers['content-type'] = 'application/json';
  }
  if (effect.csrf) headers['x-csrf-token'] = effect.csrf;
  if (effect.idempotencyKey) headers['idempotency-key'] = effect.idempotencyKey;
  let reply = { ok: false, status: 0, body: null };
  try {
    const response = await fetch(effect.path, {
      method: effect.method,
      headers,
      credentials: 'same-origin',
      body: effect.method === 'POST' ? JSON.stringify(effect.body ?? {}) : undefined,
    });
    const text = await response.text();
    let body = null;
    try {
      body = text ? JSON.parse(text) : null;
    } catch {
      body = null;
    }
    reply = { ok: response.ok, status: response.status, body };
  } catch (error) {
    reply = { ok: false, status: 0, body: null };
  }
  if (!reply.ok && reply.status !== 404 && reply.status !== 401 && reply.status !== 422) {
    diag('warn', 'http.failed', effect.correlation, `${effect.method} ${effect.path.split('?')[0]} -> ${reply.status}`);
  }
  dispatch({ type: 'http_result', id: effect.id, ...reply });
}

/** @param {string | undefined} csrf The request token of the session. A browser cannot set a header on a socket, so it goes in the address. */
function connect(csrf) {
  const scheme = self.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const opened = new WebSocket(`${scheme}//${self.location.host}/ws?csrf=${encodeURIComponent(csrf ?? '')}`);
  socket = opened;
  let wasOpen = false;
  opened.onopen = () => {
    wasOpen = true;
    dispatch({ type: 'ws_open' });
  };
  opened.onmessage = (event) => {
    if (socket !== opened || typeof event.data !== 'string') return;
    try {
      dispatch({ type: 'ws_message', message: JSON.parse(event.data) });
    } catch {
      opened.close();
    }
  };
  opened.onclose = () => {
    if (socket !== opened) return;
    socket = null;
    // A browser does not say why an upgrade failed. A socket that never opened while the network
    // is up is taken as refused (an expired token, say): the kernel reads the session again.
    dispatch({ type: 'ws_closed', refused: !wasOpen && self.navigator.onLine !== false });
  };
}

/** @param {Effect} effect */
function execute(effect) {
  switch (effect.type) {
    case 'http':
      void http(effect);
      break;
    case 'ws_connect':
      if (!socket) connect(effect.csrf);
      break;
    case 'ws_send':
      if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(effect.message));
      break;
    case 'ws_close':
      socket?.close();
      break;
    case 'timer':
      setTimeout(() => dispatch({ type: 'timer', name: effect.name, token: effect.token }), effect.ms);
      break;
    case 'log':
      if (import.meta.env.DEV) console.warn(effect.event, effect.detail);
      diag(effect.level, effect.event, effect.correlation, effect.detail);
      break;
    default:
      // Effects on the page itself (URL, storage, scroll) belong to the main thread.
      self.postMessage({ type: 'effect', effect });
  }
}

// The hook that drops the socket exists in development builds only (or when a test build asks for it).
self.onmessage = (event) => {
  const message = event.data;
  if ((import.meta.env.DEV || import.meta.env.VITE_STRATA_TEST_HOOKS === 'true') && message?.type === 'debug_drop_socket') {
    // Development aid used by the browser tests: lose the connection without telling anyone.
    socket?.close();
    return;
  }
  dispatch(message);
};
