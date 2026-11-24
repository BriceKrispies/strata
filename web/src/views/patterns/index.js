// The renderer of each pattern a page model names (docs/design-system.md, "The interpreter"): one
// module per pattern in this directory. A node whose pattern has no renderer draws nothing.
// @ts-check

import { aggregateBars } from './aggregate-bars.js';
import { dataTable } from './data-table.js';
import { form } from './form.js';
import { notificationList } from './notification-list.js';
import { recordDetail } from './record-detail.js';
import { recordList } from './record-list.js';
import { statTile } from './stat-tile.js';

/**
 * Draw one pattern node. `panel` is true in the side column, where a section is a panel.
 * @param {any} node @param {{ panel: boolean }} options
 */
export function drawPattern(node, options) {
  switch (node?.pattern) {
    case 'record-list':
      return recordList(node, options);
    case 'data-table':
      return dataTable(node, options);
    case 'record-detail':
      return recordDetail(node);
    case 'notification-list':
      return notificationList(node, options);
    case 'aggregate-bars':
      return aggregateBars(node, options);
    case 'stat-tile':
      return statTile(node);
    case 'form':
      return form(node);
    default:
      return null;
  }
}
