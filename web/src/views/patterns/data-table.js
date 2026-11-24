// The `data-table` pattern: rows and columns for records with many short fields. Below a narrow
// container width the rows stack as cards, each cell with its column's label (ds.css), so the
// table reflows at 320 CSS pixels without scrolling in two directions.
// @ts-check

import { h } from '../../components/dom.js';
import { displayValue, emptyState, loadingWords, moreButton, notice, sectionFrame, skeletonLine } from '../../components/ds.js';

/** @param {number} rows */
function tableSkeleton(rows) {
  return h(
    'div',
    { class: 'ds-table-wrap ds-table-wrap--skeleton' },
    Array.from({ length: rows }, (_, i) => h('div', { class: 'ds-table-skeleton__row', key: `s${i}` }, skeletonLine('line'), skeletonLine('short'), skeletonLine('short'))),
  );
}

/** @param {any} node @param {{ panel: boolean }} options */
export function dataTable(node, { panel }) {
  let content;
  switch (node.state) {
    case 'loading':
      content = [loadingWords(), tableSkeleton(node.skeleton?.rows ?? 3)];
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
          'div',
          { class: 'ds-table-wrap' },
          h(
            'table',
            { class: 'ds-table' },
            h('caption', { class: 'visually-hidden' }, node.label),
            h(
              'thead',
              null,
              h('tr', null, node.columns.map((/** @type {any} */ c) => h('th', { scope: 'col', key: c.id, class: c.numeric ? 'ds-table__num' : null }, c.label))),
            ),
            h(
              'tbody',
              null,
              node.rows.map((/** @type {any} */ row) =>
                h(
                  'tr',
                  { key: row.key },
                  row.cells.map((/** @type {any} */ cell, /** @type {number} */ i) =>
                    h(
                      i === 0 ? 'th' : 'td',
                      { key: cell.column, scope: i === 0 ? 'row' : null, 'data-label': cell.label, class: cell.numeric ? 'ds-table__num' : null },
                      displayValue(cell.value),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
        moreButton(node.more),
      ];
  }
  return sectionFrame({ id: node.id, label: node.label, state: node.state, count: node.state === 'ready' ? node.count : null, panel }, content);
}
