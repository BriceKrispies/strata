// The `notification-list` pattern: notices with their read state. An unread notice has a bold text
// and the rule on its leading edge, and says "Unread" to assistive technology; the heading says how
// many in words ("8 unread"). No pill, no dot (D-DS-31).
// @ts-check

import { h } from '../../components/dom.js';
import { displayValue, emptyState, loadingWords, notice, sectionFrame, skeletonLine } from '../../components/ds.js';

/** @param {any} node @param {{ panel: boolean }} options */
export function notificationList(node, { panel }) {
  let content;
  switch (node.state) {
    case 'loading':
      content = [
        loadingWords(),
        h('ul', { class: 'ds-notices' }, Array.from({ length: node.skeleton?.rows ?? 3 }, (_, i) => h('li', { class: 'ds-notification', key: `s${i}` }, h('span', { class: 'ds-notification__dot' }), h('div', { class: 'ds-stack ds-stack--tight' }, skeletonLine('line'), skeletonLine('short'))))),
      ];
      break;
    case 'empty':
      content = emptyState(node.empty);
      break;
    case 'error':
      content = notice(node.error, { tone: 'danger' });
      break;
    default:
      content = h(
        'ul',
        { class: 'ds-notices' },
        node.items.map((/** @type {any} */ item) =>
          h(
            'li',
            { class: item.unread ? 'ds-notification ds-notification--unread' : 'ds-notification', 'data-edge': item.edge ?? null, key: item.key },
            h(
              'div',
              { class: 'ds-notification__main' },
              h('p', { class: 'ds-notification__text' }, item.href ? h('a', { href: item.href }, item.text) : item.text),
              h(
                'p',
                { class: 'ds-notification__meta' },
                h('span', { class: 'visually-hidden' }, item.unread ? 'Unread' : 'Read'),
                item.kind ? h('span', null, item.kind) : null,
                item.time ? displayValue(item.time) : null,
                item.number ? h('span', { class: 'ds-number' }, item.number) : null,
              ),
            ),
          ),
        ),
      );
  }
  const count = node.state === 'ready' ? node.count ?? null : null;
  return sectionFrame({ id: node.id, label: node.label, state: node.state, count, panel }, content);
}
