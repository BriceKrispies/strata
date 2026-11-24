// The `record-detail` pattern: one record, with its title, status, person and time at the top,
// its body as reading text, and its other fields as a description list.
// @ts-check

import { h } from '../../components/dom.js';
import { avatar, bodyText, stateLine, descriptionList, displayValue, emptyState, loadingWords, notice, personChip, sectionFrame, skeletonLine } from '../../components/ds.js';

/** @param {any} node */
export function recordDetail(node) {
  let content;
  switch (node.state) {
    case 'loading':
      content = [
        loadingWords(),
        h('div', { class: 'ds-detail ds-detail--skeleton' }, skeletonLine('title'), Array.from({ length: node.skeleton?.rows ?? 3 }, (_, i) => skeletonLine(i % 2 ? 'short' : 'line', `s${i}`))),
      ];
      break;
    case 'empty':
      content = emptyState(node.empty);
      break;
    case 'error':
      content = notice(node.error, { tone: 'danger' });
      break;
    default: {
      const r = node.record;
      content = h(
        'article',
        { class: 'ds-detail', 'data-edge': r.edge ?? null },
        h(
          'div',
          { class: 'ds-detail__head' },
          r.avatar ? avatar({ ...r.avatar, size: 'lg' }) : null,
          h(
            'div',
            { class: 'ds-detail__heading' },
            r.title ? h('h3', { class: 'ds-detail__title' }, r.title) : null,
            stateLine(r.state),
            r.subtitle ? h('p', { class: 'ds-detail__subtitle' }, r.subtitle) : null,
            r.person || r.number
              ? h(
                  'div',
                  { class: 'ds-cluster' },
                  r.person ? personChip(r.person) : null,
                  r.number ? h('span', { class: 'ds-number' }, r.number) : null,
                )
              : null,
          ),
        ),
        bodyText(r.body, 'ds-detail__body'),
        descriptionList(node.facts ?? [], 'ds-detail__facts'),
      );
    }
  }
  // The record's own title heads it; the section's label is there for assistive technology.
  return sectionFrame({ id: node.id, label: node.label, state: node.state, hideLabel: node.state === 'ready' && !!node.record?.title }, content);
}
