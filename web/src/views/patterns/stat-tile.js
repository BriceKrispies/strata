// The `stat-tile` pattern: one number with its label.
// @ts-check

import { h } from '../../components/dom.js';
import { emptyState, loadingWords, notice, skeletonLine } from '../../components/ds.js';

/** @param {any} node */
export function statTile(node) {
  const headingId = `${node.id.replace(/[^a-zA-Z0-9_-]/g, '-')}-title`;
  let value;
  switch (node.state) {
    case 'loading':
      value = [loadingWords(), skeletonLine('title')];
      break;
    case 'empty':
      value = emptyState(node.empty);
      break;
    case 'error':
      value = notice(node.error, { tone: 'danger' });
      break;
    default:
      value = h('p', { class: 'ds-stat__value' }, node.value);
  }
  return h(
    'section',
    { class: 'ds-stat', 'aria-labelledby': headingId, 'aria-busy': node.state === 'loading' ? 'true' : null, key: node.id },
    h('h2', { class: 'ds-stat__label', id: headingId, tabindex: '-1' }, node.label),
    value,
  );
}
