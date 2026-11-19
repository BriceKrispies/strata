// The main thread: start the worker, draw what it sends, tell it what the person did.
//
// This file issues no application requests. It loads the page's own assets, owns the DOM, and
// translates DOM events into typed messages for the kernel in the worker. Everything the
// application knows and decides lives on the other side of `postMessage`.
// @ts-check

import './styles/main.css';
import { holdValues, render } from './components/dom.js';
import { passkeysSupported, runPasskey } from './passkey.js';
import { shell } from './views/shell.js';

const app = /** @type {HTMLElement} */ (document.getElementById('app'));
const worker = new Worker(new URL('./worker/main.js', import.meta.url), { type: 'module', name: 'strata-kernel' });
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
let lastFocus = 0;
// Inputs sent, and the latest the worker has said it applied. Renders made before it caught up
// are older than what is on screen, so they must not rewrite the controls.
let inputsSent = 0;

/** @param {any} message */
function send(message) {
  worker.postMessage(message);
}

function stored(key) {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function bootstrap() {
  const block = document.getElementById('strata-bootstrap');
  try {
    return block ? JSON.parse(block.textContent ?? 'null') : null;
  } catch {
    return null;
  }
}

function draw(model, applied) {
  const root = document.documentElement;
  root.dataset.accent = model.theme.accent;
  root.dataset.paper = model.theme.paper;
  root.dataset.scheme = model.theme.scheme;
  if (model.title) document.title = model.title;
  holdValues(applied < inputsSent);
  try {
    render(app, shell(model));
  } finally {
    holdValues(false);
  }
  if (model.focus.token !== lastFocus) {
    lastFocus = model.focus.token;
    // After a navigation, move focus to the new page so keyboard and screen-reader users
    // start at its top; after a failed submit, to the first field that needs attention.
    document.getElementById(model.focus.target)?.focus({ preventScroll: model.focus.target === 'main' });
  }
}

worker.onmessage = (event) => {
  const message = event.data;
  if (message.type === 'render') {
    draw(message.model, message.applied ?? 0);
    return;
  }
  const effect = message.effect;
  switch (effect?.type) {
    case 'push_url':
      if (effect.replace) window.history.replaceState(null, '', effect.href);
      else window.history.pushState(null, '', effect.href);
      break;
    case 'store':
      try {
        window.localStorage.setItem(effect.key, effect.value);
      } catch {
        // Storage can be unavailable; the choice then lasts for this visit only.
      }
      break;
    case 'scroll_top':
      window.scrollTo(0, 0);
      break;
    case 'passkey':
      void runPasskey(effect).then(send);
      break;
    default:
  }
};

send({
  type: 'init',
  path: window.location.pathname,
  search: window.location.search,
  bootstrap: bootstrap(),
  systemDark: darkQuery.matches,
  storedTheme: stored('strata.theme'),
  passkeys: passkeysSupported(),
  hash: window.location.hash,
});

darkQuery.addEventListener('change', (event) => send({ type: 'system_theme', dark: event.matches }));
window.addEventListener('popstate', () => send({ type: 'popstate', path: window.location.pathname, search: window.location.search }));

document.addEventListener('click', (event) => {
  const target = /** @type {Element} */ (event.target);
  const mark = target.closest('[data-mark]');
  if (mark) {
    applyMark(/** @type {HTMLElement} */ (mark).dataset.mark ?? '');
    return;
  }
  const action = /** @type {HTMLElement | null} */ (target.closest('[data-action]'));
  if (action) {
    let arg;
    try {
      arg = action.dataset.arg === undefined ? undefined : JSON.parse(action.dataset.arg);
    } catch {
      arg = action.dataset.arg;
    }
    send({ type: 'action', name: action.dataset.action, arg });
    return;
  }
  const link = /** @type {HTMLAnchorElement | null} */ (target.closest('a[href]'));
  if (!link || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  if (link.target || link.hasAttribute('download') || link.origin !== window.location.origin) return;
  if (link.pathname.startsWith('/media/') || link.pathname.startsWith('/assets/')) return;
  if (link.pathname === window.location.pathname && link.hash) return;
  event.preventDefault();
  send({ type: 'navigate', href: link.pathname + link.search });
});

function fieldValue(control) {
  return control instanceof HTMLInputElement && control.type === 'checkbox' ? control.checked : control.value;
}

document.addEventListener('input', (event) => {
  const control = /** @type {HTMLInputElement} */ (event.target);
  const form = control.closest?.('form[data-form]');
  if (!form || !control.name || form.getAttribute('data-form') === 'search') return;
  send({ type: 'input', seq: ++inputsSent, form: form.getAttribute('data-form'), name: control.name, value: fieldValue(control) });
});

document.addEventListener('submit', (event) => {
  const form = /** @type {HTMLFormElement} */ (event.target);
  if (!form.matches('form[data-form]')) return;
  event.preventDefault();
  /** @type {Record<string, string | boolean>} */
  const values = {};
  for (const control of Array.from(form.elements)) {
    const input = /** @type {HTMLInputElement} */ (control);
    if (!input.name) continue;
    if (input.type === 'radio' && !input.checked) continue;
    values[input.name] = fieldValue(input);
  }
  send({ type: 'submit', form: form.getAttribute('data-form'), values });
});

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && document.querySelector('.masthead__menu--open')) {
    send({ type: 'action', name: 'menu' });
    /** @type {HTMLElement | null} */ (document.querySelector('.masthead__toggle'))?.focus();
  }
});

/** Wrap or prefix the selection in the article text, then tell the kernel the new text. */
function applyMark(mark) {
  const area = /** @type {HTMLTextAreaElement | null} */ (document.getElementById('editor-body'));
  if (!area) return;
  const { selectionStart: start, selectionEnd: end, value } = area;
  const selected = value.slice(start, end);
  const lineStart = value.lastIndexOf('\n', start - 1) + 1;
  const wrap = (before, after, placeholder) => {
    const inner = selected || placeholder;
    area.setRangeText(before + inner + after, start, end, 'end');
    area.setSelectionRange(start + before.length, start + before.length + inner.length);
  };
  const prefix = (text) => {
    area.setRangeText(text, lineStart, lineStart, 'end');
    area.setSelectionRange(start + text.length, end + text.length);
  };
  switch (mark) {
    case 'bold':
      wrap('**', '**', 'bold text');
      break;
    case 'italic':
      wrap('*', '*', 'italic text');
      break;
    case 'link':
      wrap('[', '](https://)', 'link text');
      break;
    case 'heading':
      prefix('## ');
      break;
    case 'quote':
      prefix('> ');
      break;
    case 'list':
      prefix('- ');
      break;
    default:
      return;
  }
  area.focus();
  send({ type: 'input', seq: ++inputsSent, form: 'editor', name: 'body', value: area.value });
}

// Development aid for the browser tests: a way to lose the socket as a bad network would.
Object.defineProperty(window, '__strataDropSocket', { value: () => send({ type: 'debug_drop_socket' }) });
