// The `record-list` pattern: a card per record (docs/design-system.md, "Layer 3: patterns",
// `record-card` and `record-list`). A feed reads down one column; other records sit in a grid.
// @ts-check

import { h } from '../../components/dom.js';
import { avatar, bodyText, stateLine, displayValue, emptyState, loadingWords, moreButton, notice, personChip, sectionFrame, skeletonLine } from '../../components/ds.js';

/**
 * One record as a card: title (a link to its screen when it has one), the state line when its state
 * departs from normal (with the rule on its leading edge), person, time, number, body and its other
 * facts. Heading level 3 under the section's heading. Nothing on it is a pill (D-DS-31).
 * @param {any} record @param {{ compact: boolean, clamp: boolean }} options
 */
export function recordCard(record, { compact, clamp }) {
  const meta = record.meta ?? [];
  // Many facts read better as a grid of terms; a few as one line.
  const many = !compact && meta.length > 4;
  const titleText = record.title ?? '';
  return h(
    'li',
    { class: `ds-record${compact ? ' ds-record--compact' : ''}`, 'data-edge': record.edge ?? null, key: record.key },
    h(
      'article',
      { class: 'ds-record__inner' },
      h(
        'div',
        { class: 'ds-record__head' },
        record.avatar ? avatar({ ...record.avatar, size: compact ? 'sm' : 'md' }) : null,
        h(
          'div',
          { class: 'ds-record__heading' },
          titleText
            ? h('h3', { class: 'ds-record__title' }, record.href ? h('a', { href: record.href }, titleText) : titleText)
            : null,
          stateLine(record.state),
          record.subtitle ? h('p', { class: 'ds-record__subtitle' }, record.subtitle) : null,
        ),
      ),
      record.person || record.time || record.number
        ? h(
            'p',
            { class: 'ds-record__byline' },
            record.person ? personChip(record.person) : null,
            record.time ? h('span', { class: 'ds-record__time' }, record.timeLabel ? h('span', { class: 'visually-hidden' }, `${record.timeLabel}: `) : null, displayValue(record.time)) : null,
            record.number ? h('span', { class: 'ds-number' }, record.number) : null,
          )
        : null,
      bodyText(record.body, clamp ? 'ds-record__body ds-record__body--clamp' : 'ds-record__body'),
      meta.length
        ? many
          ? h('dl', { class: 'ds-dl ds-dl--compact' }, meta.map((/** @type {any} */ m) => h('div', { class: 'ds-dl__item', key: m.id }, h('dt', null, m.label), h('dd', null, displayValue(m.value)))))
          : h(
              'dl',
              { class: 'ds-record__meta' },
              meta.map((/** @type {any} */ m) => h('div', { class: 'ds-meta', key: m.id }, h('dt', { class: 'ds-meta__label' }, m.label), h('dd', { class: 'ds-meta__value' }, displayValue(m.value)))),
            )
        : null,
    ),
  );
}

/** The skeleton of a list of cards: always the same number of cards. @param {number} rows */
function cardsSkeleton(rows) {
  return h(
    'ul',
    { class: 'ds-list ds-list--grid' },
    Array.from({ length: rows }, (_, i) =>
      h('li', { class: 'ds-record ds-record--skeleton', key: `s${i}` }, skeletonLine('title'), skeletonLine('line'), skeletonLine('short')),
    ),
  );
}

/** @param {any} node @param {{ panel: boolean }} options */
export function recordList(node, { panel }) {
  let content;
  switch (node.state) {
    case 'loading':
      content = [loadingWords(), cardsSkeleton(node.skeleton?.rows ?? 3)];
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
          'ul',
          { class: `ds-list ds-list--${node.layout === 'grid' ? 'grid' : 'stack'}` },
          node.items.map((/** @type {any} */ item) => recordCard(item, { compact: node.compact === true, clamp: node.clamp === true })),
        ),
        moreButton(node.more),
      ];
  }
  return sectionFrame({ id: node.id, label: node.label, state: node.state, count: node.state === 'ready' ? node.count : null, panel }, content);
}

