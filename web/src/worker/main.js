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
/** @type {WebSocket | null} */
let socket = null;

/** Run one message through the kernel and carry out what it asks for. */
function dispatch(message) {
  const stamped = { ...message, at: Date.now(), rand: Math.random() };
  const result = update(state, stamped);
  state = result.state;
  for (const effect of result.effects) execute(effect);
  render();
}

function render() {
  const model = view(state);
  const encoded = JSON.stringify(model);
  if (encoded === lastRendered) return;
  lastRendered = encoded;
  self.postMessage({ type: 'render', model });
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
    headers['x-strata-request'] = '1';
  }
  if (effect.csrf) headers['x-csrf-token'] = effect.csrf;
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

function connect() {
  const scheme = self.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const opened = new WebSocket(`${scheme}//${self.location.host}/ws`);
  socket = opened;
  opened.onopen = () => dispatch({ type: 'ws_open' });
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
    dispatch({ type: 'ws_closed' });
  };
}

/** @param {Effect} effect */
function execute(effect) {
  switch (effect.type) {
    case 'http':
      void http(effect);
      break;
    case 'ws_connect':
      if (!socket) connect();
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
      diag(effect.level, effect.event, effect.correlation, effect.detail);
      break;
    default:
      // Effects on the page itself (URL, storage, scroll) belong to the main thread.
      self.postMessage({ type: 'effect', effect });
  }
}

self.onmessage = (event) => {
  const message = event.data;
  if (message?.type === 'debug_drop_socket') {
    // Development aid used by the browser tests: lose the connection without telling anyone.
    socket?.close();
    return;
  }
  dispatch(message);
};
