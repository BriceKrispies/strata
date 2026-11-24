// The interpreter's drawing of a tenant screen (D-8250, INT-3257): the page model that
// `kernel/compose.js` builds from the screen's part of the tenant's interface definition, drawn with
// the design system's primitives and one renderer per pattern (views/patterns). Nothing here belongs
// to a product: every tenant screen of every product is drawn by these functions.
//
// The page is a template: a header with the screen's title and actions, a main column and, when the
// screen has secondary sections (aggregates, notices, small lists, forms beside the data), a side
// column. Every element has a stable key, so a redraw patches in place and keeps focus.
// @ts-check

import { h } from '../components/dom.js';
import { button } from '../components/ds.js';
import { drawPattern } from './patterns/index.js';

/** The actions of the screen, as buttons; one with a confirmation asks for a second press. @param {any[]} actions */
function actions(actions) {
  if (!actions?.length) return null;
  return h(
    'div',
    { class: 'ds-page__actions ds-cluster' },
    actions.map((a) =>
      button({ variant: a.armed ? 'danger' : 'secondary', 'data-action': 'tenant.action', 'data-arg': JSON.stringify(a.id), key: a.id }, a.armed && a.confirm ? a.confirm : a.label),
    ),
  );
}

/** A tenant screen. @param {any} page */
export function screen(page) {
  const split = page.template === 'split';
  return h(
    'div',
    { class: 'ds-page', 'data-template': page.layout, 'aria-busy': page.state === 'loading' ? 'true' : null },
    h('header', { class: 'ds-page__header' }, h('h1', { class: 'ds-page__title' }, page.heading), actions(page.actions)),
    h(
      'div',
      { class: split ? 'ds-page__body ds-page__body--split' : 'ds-page__body' },
      h('div', { class: 'ds-page__main', key: 'main' }, (page.main ?? []).map((/** @type {any} */ node) => drawPattern(node, { panel: false }))),
      split ? h('aside', { class: 'ds-page__aside', key: 'aside', 'aria-label': 'More on this page' }, (page.aside ?? []).map((/** @type {any} */ node) => drawPattern(node, { panel: true }))) : null,
    ),
  );
}
