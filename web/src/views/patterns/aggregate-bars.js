// The `aggregate-bars` pattern: an aggregate grouped by a field, as a ranked list of bars. The bar
// is decoration (its width is quantised in ds.css, so nothing is written inline); the label and
// the count are text.
// @ts-check

import { h } from '../../components/dom.js';
import { emptyState, loadingWords, notice, sectionFrame, skeletonLine } from '../../components/ds.js';

/** @param {any} node @param {{ panel: boolean }} options */
export function aggregateBars(node, { panel }) {
  let content;
  switch (node.state) {
    case 'loading':
      content = [loadingWords(), h('div', { class: 'ds-bars' }, Array.from({ length: node.skeleton?.rows ?? 3 }, (_, i) => h('div', { class: 'ds-bar', key: `s${i}` }, skeletonLine('short'), h('div', { class: 'ds-bar__track' }))))];
      break;
    case 'empty':
      content = emptyState(node.empty);
      break;
    case 'error':
      content = notice(node.error, { tone: 'danger' });
      break;
    default:
      content = [
        h(
          'ol',
          { class: 'ds-bars' },
          node.bars.map((/** @type {any} */ bar) =>
            h(
              'li',
              { class: 'ds-bar', key: bar.key },
              h('span', { class: 'ds-bar__label' }, bar.label),
              h('span', { class: 'ds-bar__value' }, bar.text),
              h('span', { class: 'ds-bar__track', 'aria-hidden': 'true' }, h('span', { class: 'ds-bar__fill', 'data-fill': String(bar.fill) })),
            ),
          ),
        ),
        node.rest ? h('p', { class: 'ds-bars__rest' }, node.rest) : null,
      ];
  }
  const count = node.state === 'ready' && node.total ? `${node.total} in all` : null;
  return sectionFrame({ id: node.id, label: node.label, state: node.state, count, panel }, content);
}
