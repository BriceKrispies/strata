// The `notification-list` pattern: notices with their read state. Unread is said in words ("New")
// as well as by the dot, so the state is never carried by colour alone.
// @ts-check

import { h } from '../../components/dom.js';
import { badge, displayValue, emptyState, loadingWords, notice, sectionFrame, skeletonLine } from '../../components/ds.js';

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
            { class: item.unread ? 'ds-notification ds-notification--unread' : 'ds-notification', key: item.key },
            h('span', { class: 'ds-notification__dot', 'aria-hidden': 'true' }),
            h(
              'div',
              { class: 'ds-notification__main' },
              h('p', { class: 'ds-notification__text' }, item.href ? h('a', { href: item.href }, item.text) : item.text),
              h(
                'p',
                { class: 'ds-notification__meta' },
                item.unread ? badge('New', { tone: 'info' }) : h('span', { class: 'visually-hidden' }, 'Read'),
                item.kind ? h('span', null, item.kind) : null,
                item.time ? displayValue(item.time) : null,
                item.number ? h('span', { class: 'ds-number' }, item.number) : null,
              ),
            ),
          ),
        ),
      );
  }
  const count = node.state === 'ready' && node.unread > 0 ? `${node.unread} new` : null;
  return sectionFrame({ id: node.id, label: node.label, state: node.state, count, panel }, content);
}
