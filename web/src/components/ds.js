// The design system's primitives as virtual nodes (docs/design-system.md, "Layer 2: primitives"):
// class contracts (`ds-*`, web/src/styles/ds.css) on native elements, in the light DOM. None of
// them knows anything about Strata data; the pattern renderers (views/patterns) compose them, and a
// display node from the kernel (kernel/display.js) is drawn by `displayValue`.
//
// There is no badge, chip or pill primitive (D-DS-31): a state is said in words by `stateLine`,
// only where it departs from normal; a short list is a sentence; counts and numbers are plain text.
// @ts-check

import { h } from './dom.js';
import { prose } from './parts.js';

/** @typedef {import('./dom.js').VNode} VNode */

/**
 * A button. Variants: primary, secondary, quiet, danger; `small` for dense places. Everything else
 * (type, data-action, disabled) passes through as attributes.
 * @param {{ variant?: 'primary' | 'secondary' | 'quiet' | 'danger', small?: boolean } & Record<string, any>} attrs @param {...any} children
 */
export function button({ variant = 'secondary', small = false, ...attrs }, ...children) {
  const cls = `ds-button ds-button--${variant}${small ? ' ds-button--small' : ''}`;
  return h('button', { type: 'button', ...attrs, class: cls }, ...children);
}

/** The lead word of a state, ending as a sentence does. @param {string} label */
function leadWord(label) {
  return /[.!?]$/.test(label) ? label : `${label}.`;
}

/**
 * A state line, under a record's title: the state's label as a bold lead word and, when the
 * presentation says one, what it means for the reader ("Draft. Only administrators can see this
 * until it is published."), in the colour of its mark with no fill. A normal state never reaches
 * here: the kernel drops it.
 * @param {any} state
 */
export function stateLine(state) {
  if (!state) return null;
  return h(
    'p',
    { class: `ds-state ds-state--${state.mark}` },
    h('strong', { class: 'ds-state__lead' }, leadWord(state.label)),
    state.means ? ` ${state.means}` : null,
  );
}

/**
 * An avatar: the image when there is one (decorative, the name is beside it), else the initials.
 * @param {{ initials?: string, src?: string | null, size?: 'sm' | 'md' | 'lg' }} person
 */
export function avatar({ initials = '', src = null, size = 'md' }) {
  return h(
    'span',
    { class: `ds-avatar ds-avatar--${size}`, 'aria-hidden': 'true' },
    src ? h('img', { src, alt: '', loading: 'lazy' }) : initials || '?',
  );
}

/** A person chip: avatar and name. @param {{ name: string, initials?: string, avatar?: string | null }} person @param {'sm' | 'md'} [size] */
export function personChip(person, size = 'sm') {
  return h('span', { class: 'ds-person' }, avatar({ initials: person.initials || '', src: person.avatar ?? null, size }), h('span', { class: 'ds-person__name' }, person.name));
}

/** The dash an empty value is drawn as, with words for assistive technology. */
export function emptyValue() {
  return h('span', { class: 'ds-nil' }, h('span', { 'aria-hidden': 'true' }, '—'), h('span', { class: 'visually-hidden' }, 'Not set'));
}

/**
 * A display node of the kernel (display.js) as elements. Text is always set as text; addresses go
 * through the renderer's URL check.
 * @param {any} node @returns {any}
 */
export function displayValue(node) {
  if (!node) return emptyValue();
  switch (node.kind) {
    case 'empty':
      return emptyValue();
    case 'text':
      return h('span', { class: `${node.muted ? 'ds-muted' : ''}${node.multiline ? ' ds-pre' : ''}`.trim() || null }, node.text);
    case 'rich':
      return h('span', { class: 'ds-pre' }, node.markdown);
    case 'time':
      return h('time', { datetime: node.iso, class: 'ds-time' }, node.text);
    case 'number':
    case 'money':
      return h('span', { class: 'ds-num' }, node.text);
    case 'boolean':
      return h('span', { class: node.value ? 'ds-bool ds-bool--yes' : 'ds-bool ds-bool--no' }, node.text);
    case 'state':
      return h('span', { class: `ds-state ds-state--${node.mark}` }, h('strong', { class: 'ds-state__lead' }, node.label), node.means ? ` ${node.means}` : null);
    case 'person':
      return personChip(node);
    case 'link':
      return node.href ? h('a', { class: 'ds-link', href: node.href }, node.text) : h('span', { class: 'ds-ref' }, node.text);
    case 'list':
      // A prose list: each item drawn as itself (a link stays a link), the language's separators between.
      return h('span', { class: 'ds-list-prose' }, (node.parts ?? []).map((/** @type {any} */ part) => (part.item ? displayValue(part.item) : part.literal)));
    case 'language':
      return h('span', { lang: node.tag }, node.text);
    case 'media':
      return node.image
        ? h('img', { class: 'ds-media', src: node.src, alt: node.alt ?? '', loading: 'lazy' })
        : node.src
          ? h('a', { class: 'ds-link', href: node.src }, node.text)
          : h('span', null, node.text);
    case 'url':
      return h('a', { class: 'ds-link', href: node.href, rel: /^https?:/i.test(node.href) ? 'noopener noreferrer' : null }, node.text);
    case 'email':
      return h('a', { class: 'ds-link', href: node.href }, node.text);
    case 'code':
      return h('code', { class: 'ds-code' }, node.text);
    case 'title':
      return h(
        'span',
        { class: 'ds-cell-title' },
        node.avatar ? avatar({ ...node.avatar, size: 'sm' }) : null,
        node.href ? h('a', { class: 'ds-link ds-link--strong', href: node.href }, node.text) : h('span', null, node.text),
      );
    default:
      return typeof node.text === 'string' ? h('span', null, node.text) : emptyValue();
  }
}

/** A body of text: prose blocks from the server when there are some, else the text with its line breaks. @param {{ text?: string, blocks?: any[] } | null} body @param {string} cls */
export function bodyText(body, cls) {
  if (!body) return null;
  if (Array.isArray(body.blocks)) return h('div', { class: `${cls} ds-prose` }, prose(body.blocks));
  return h('p', { class: `${cls} ds-pre` }, body.text ?? '');
}

/**
 * A labelled region of the page: heading, an optional count and the content. In the side column
 * it is a panel. The heading is focusable from script, so focus can return to it after a redraw.
 * @param {{ id: string, label: string, state?: string, count?: string | null, panel?: boolean, extra?: any, hideLabel?: boolean }} section @param {...any} content
 */
export function sectionFrame({ id, label, state, count = null, panel = false, extra = null, hideLabel = false }, ...content) {
  const headingId = `${id.replace(/[^a-zA-Z0-9_-]/g, '-')}-title`;
  return h(
    'section',
    { class: panel ? 'ds-section ds-section--panel' : 'ds-section', 'aria-labelledby': headingId, 'aria-busy': state === 'loading' ? 'true' : null, key: id },
    h(
      'header',
      { class: hideLabel ? 'ds-section__head visually-hidden' : 'ds-section__head' },
      // A count is heading text, muted, with no container: "31", "8 unread", "122 in all".
      h(
        'h2',
        { class: 'ds-section__title', id: headingId, tabindex: '-1' },
        label,
        count ? h('span', { class: 'ds-section__count' }, ' ', /^\d[\d,.]*\+?$/.test(count) ? h('span', { class: 'visually-hidden' }, 'count ') : null, count) : null,
      ),
      extra,
    ),
    ...content,
  );
}

/** An empty state: an outline icon, a title and a line of text. @param {{ title: string, text?: string }} words */
export function emptyState(words) {
  return h(
    'div',
    { class: 'ds-empty' },
    h(
      'svg',
      { class: 'ds-empty__icon', viewBox: '0 0 24 24', width: 32, height: 32, 'aria-hidden': 'true', focusable: 'false' },
      h('path', { d: 'M4 13h4l1.5 2.5h5L16 13h4M5.5 6.5 4 13v5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-5l-1.5-6.5a1 1 0 0 0-1-.75h-11a1 1 0 0 0-1 .75Z', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.5', 'stroke-linejoin': 'round' }),
    ),
    h('p', { class: 'ds-empty__title' }, words.title),
    words.text ? h('p', { class: 'ds-empty__text' }, words.text) : null,
  );
}

/** A notice: a short message with a tone. @param {string} text @param {{ tone?: string, role?: string }} [options] */
export function notice(text, { tone = 'info', role } = {}) {
  return h('p', { class: `ds-notice ds-notice--${tone}`, role: role ?? null }, text);
}

/** A skeleton line. @param {'title' | 'line' | 'short' | 'block'} [shape] @param {string} [key] */
export function skeletonLine(shape = 'line', key) {
  return h('div', { class: `ds-skeleton ds-skeleton--${shape}`, 'aria-hidden': 'true', key });
}

/** The words a loading region says to assistive technology. */
export function loadingWords() {
  return h('span', { class: 'visually-hidden', role: 'status' }, 'Loading…');
}

/**
 * The button that loads the next page of a list section, when the view has one.
 * @param {{ section: string, busy: boolean } | null | undefined} more
 */
export function moreButton(more) {
  if (!more) return null;
  return h(
    'div',
    { class: 'ds-more' },
    button({ variant: 'secondary', disabled: more.busy, 'aria-busy': more.busy ? 'true' : null, 'data-action': 'view.more', 'data-arg': JSON.stringify(more.section) }, more.busy ? 'Loading…' : 'Show more'),
  );
}

/**
 * A description list: each term above its value, in a grid that reflows.
 * @param {{ id: string, label: string, value: any }[]} facts @param {string} [cls]
 */
export function descriptionList(facts, cls = '') {
  if (!facts.length) return null;
  return h(
    'dl',
    { class: `ds-dl ${cls}`.trim() },
    facts.map((f) => h('div', { class: 'ds-dl__item', key: f.id }, h('dt', null, f.label), h('dd', null, displayValue(f.value)))),
  );
}
