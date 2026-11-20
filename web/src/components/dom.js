// A small view-model renderer: virtual nodes in, DOM patched in place.
//
// Nodes are created with DOM APIs only (createElement, setAttribute, text nodes). Nothing here
// parses HTML, so no value from tenant data can become markup. Patching in place keeps focus,
// selection, scroll position and the browser's own form state across updates.
// @ts-check

import { sitePath } from '../generated/site-path.js';

const SVG_NS = 'http://www.w3.org/2000/svg';
const URL_ATTRS = new Set(['href', 'src', 'action']);
const PROPS = new Set(['value', 'checked', 'selected']);

// True while the picture being drawn is older than what the person has typed since: the worker
// has not yet answered their latest keystrokes, so its `value` and `checked` are stale.
let holding = false;

/**
 * Tell the renderer whether the next render was made before the latest input was applied. While
 * it was, controls keep what they hold instead of being rewound to an earlier value.
 * @param {boolean} stale
 */
export function holdValues(stale) {
  holding = stale;
}

/**
 * @typedef {{ tag: string, attrs: Record<string, any>, children: Child[], key: string | null, el?: Element }} VNode
 * @typedef {VNode | string} Child
 */

/**
 * Describe an element. Falsy children are dropped, arrays are flattened, numbers become text.
 * @param {string} tag
 * @param {Record<string, any> | null} [attrs]
 * @param {...any} children
 * @returns {VNode}
 */
export function h(tag, attrs, ...children) {
  /** @type {Child[]} */
  const flat = [];
  const add = (child) => {
    if (child === null || child === undefined || child === false || child === '') return;
    if (Array.isArray(child)) child.forEach(add);
    else if (typeof child === 'object') flat.push(child);
    else if (typeof flat[flat.length - 1] === 'string') flat[flat.length - 1] += String(child);
    else flat.push(String(child));
  };
  children.forEach(add);
  const { key = null, ...rest } = attrs ?? {};
  return { tag, attrs: rest, children: flat, key: key === null ? null : String(key) };
}

/**
 * Only addresses that stay on the page's own protocols are ever written to the DOM. A path is
 * kept only when the URL parser reads it as a path on this site (the shared check), so a
 * backslash or a tab cannot turn it into another site.
 * @param {string} name @param {unknown} value
 * @returns {string | null}
 */
export function safeUrl(name, value) {
  const text = String(value);
  if (name === 'src' || text.startsWith('/')) return sitePath(text);
  return /^(#|https?:|mailto:)/i.test(text) ? text : null;
}

/** @param {Element} el @param {string} name @param {any} value @param {any} previous */
function setAttr(el, name, value, previous) {
  if (PROPS.has(name)) {
    // Never fight the person typing: the focused control keeps what it holds unless the
    // form itself was replaced (signalled by a changed `data-rev`).
    const typing = (el === document.activeElement && name === 'value') || (holding && (name === 'value' || name === 'checked'));
    if (!typing && /** @type {any} */ (el)[name] !== value) /** @type {any} */ (el)[name] = value;
    return;
  }
  if (value === previous) return;
  if (value === null || value === undefined || value === false) {
    el.removeAttribute(name);
    return;
  }
  if (URL_ATTRS.has(name)) {
    const url = safeUrl(name, value);
    if (url === null) el.removeAttribute(name);
    else el.setAttribute(name, url);
    return;
  }
  el.setAttribute(name, value === true ? '' : String(value));
}

/** @param {Child} node @param {boolean} svg @returns {Node} */
function create(node, svg) {
  if (typeof node === 'string') return document.createTextNode(node);
  const inSvg = svg || node.tag === 'svg';
  const el = inSvg ? document.createElementNS(SVG_NS, node.tag) : document.createElement(node.tag);
  for (const [name, value] of Object.entries(node.attrs)) {
    if (!PROPS.has(name)) setAttr(el, name, value, undefined);
  }
  node.el = el;
  for (const child of node.children) el.appendChild(create(child, inSvg));
  // Properties last: a select's value only takes once its options exist.
  for (const [name, value] of Object.entries(node.attrs)) {
    if (PROPS.has(name)) /** @type {any} */ (el)[name] = value;
  }
  return el;
}

/** @param {Element} el @param {VNode} before @param {VNode} after @param {boolean} svg */
function patchElement(el, before, after, svg) {
  after.el = el;
  const replaced = before.attrs['data-rev'] !== after.attrs['data-rev'];
  for (const name of Object.keys(before.attrs)) {
    if (!(name in after.attrs)) el.removeAttribute(name);
  }
  patchChildren(el, before.children, after.children, svg || after.tag === 'svg');
  for (const [name, value] of Object.entries(after.attrs)) {
    if (PROPS.has(name) && replaced) /** @type {any} */ (el)[name] = value;
    else setAttr(el, name, value, before.attrs[name]);
  }
}

/**
 * Make `parent`'s children match `after`, reusing nodes where tag and key agree.
 * @param {Element} parent @param {Child[]} before @param {Child[]} after @param {boolean} svg
 */
function patchChildren(parent, before, after, svg) {
  const keyed = new Map();
  before.forEach((node, index) => {
    if (typeof node !== 'string' && node.key !== null) keyed.set(`${node.tag}:${node.key}`, index);
  });
  const used = new Set();
  const nodes = Array.from(parent.childNodes);
  /** @type {Node[]} */
  const result = [];
  after.forEach((node, index) => {
    let from = -1;
    if (typeof node !== 'string' && node.key !== null) {
      from = keyed.get(`${node.tag}:${node.key}`) ?? -1;
    } else if (index < before.length && !used.has(index)) {
      const old = before[index];
      const same = typeof node === 'string' ? typeof old === 'string' : typeof old !== 'string' && old.tag === node.tag && old.key === null;
      if (same) from = index;
    }
    if (from >= 0 && !used.has(from) && nodes[from]) {
      used.add(from);
      const dom = nodes[from];
      const old = before[from];
      if (typeof node === 'string') {
        if (dom.nodeValue !== node) dom.nodeValue = node;
      } else {
        patchElement(/** @type {Element} */ (dom), /** @type {VNode} */ (old), node, svg);
      }
      result.push(dom);
    } else {
      result.push(create(node, svg));
    }
  });
  nodes.forEach((dom, index) => {
    if (!used.has(index)) parent.removeChild(dom);
  });
  result.forEach((dom, index) => {
    const current = parent.childNodes[index];
    if (current !== dom) parent.insertBefore(dom, current ?? null);
  });
}

const mounted = new WeakMap();

/**
 * Render children into a container, patching whatever an earlier call left there.
 * @param {Element} container @param {Child[]} children
 */
export function render(container, children) {
  const before = mounted.get(container);
  if (!before) container.replaceChildren();
  patchChildren(container, before ?? [], children, false);
  mounted.set(container, children);
}
