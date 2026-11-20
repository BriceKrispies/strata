// Shared presentational pieces: dates, article entries, the marginal notes, prose blocks.
// @ts-check

import { h } from './dom.js';

const DAY = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });
const SHORT = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' });
const STAMP = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });

/** A date as a `<time>` element. */
export function date(ms, style = 'day') {
  if (ms == null) return null;
  const format = style === 'short' ? SHORT : style === 'stamp' ? STAMP : DAY;
  return h('time', { datetime: new Date(ms).toISOString() }, format.format(new Date(ms)));
}

export function minutes(n) {
  return n ? `${n} minute read` : null;
}

function inline(node) {
  switch (node.type) {
    case 'text':
      return node.text;
    case 'em':
      return h('em', null, node.children.map(inline));
    case 'strong':
      return h('strong', null, node.children.map(inline));
    case 'code':
      return h('code', null, node.text);
    case 'link': {
      const external = /^https?:/i.test(node.href);
      return h('a', { href: node.href, rel: external ? 'noopener noreferrer' : null }, node.children.map(inline));
    }
    default:
      return null;
  }
}

/** Structured Markdown blocks as elements. The blocks come parsed from the server. */
export function prose(blocks) {
  return (blocks ?? []).map((block) => {
    switch (block.type) {
      case 'heading':
        return h(`h${Math.min(Math.max(block.level, 2), 4)}`, null, block.inlines.map(inline));
      case 'paragraph':
        return h('p', null, block.inlines.map(inline));
      case 'quote':
        return h('blockquote', null, prose(block.blocks));
      case 'list':
        return h(block.ordered ? 'ol' : 'ul', null, block.items.map((item) => h('li', null, item.map(inline))));
      case 'code':
        return h('pre', null, h('code', null, block.text));
      case 'figure':
        return h(
          'figure',
          null,
          h('img', { src: block.src, alt: block.alt, loading: 'lazy' }),
          block.caption ? h('figcaption', null, block.caption) : null,
        );
      case 'rule':
        return h('hr', null);
      default:
        return null;
    }
  });
}

/** The notes that sit in the margin beside an article or an index entry. */
export function marginNotes(entry, { link = true } = {}) {
  return h(
    'dl',
    { class: 'notes' },
    entry.date != null ? h('div', null, h('dt', null, 'Published'), h('dd', null, date(entry.date))) : null,
    entry.author
      ? h('div', null, h('dt', null, 'Written by'), h('dd', null, link ? h('a', { href: entry.author.href }, entry.author.name) : entry.author.name))
      : null,
    entry.topic ? h('div', null, h('dt', null, 'Filed under'), h('dd', null, h('a', { href: entry.topic.href }, entry.topic.name))) : null,
    entry.minutes ? h('div', null, h('dt', null, 'Length'), h('dd', null, minutes(entry.minutes))) : null,
  );
}

/** One article in an index: notes in the margin, title and excerpt beside them. */
export function indexEntry(entry, level = 'h3') {
  return h(
    'li',
    { key: entry.id, class: 'index__item' },
    h(
      'article',
      { class: 'entry' },
      marginNotes(entry),
      h(
        'div',
        { class: 'entry__body' },
        h(level, { class: 'entry__title' }, h('a', { href: entry.href }, entry.title)),
        entry.excerpt ? h('p', { class: 'entry__excerpt' }, entry.excerpt) : null,
      ),
      entry.cover ? h('img', { class: 'entry__art', src: entry.cover, alt: '', loading: 'lazy', width: 320, height: 240 }) : null,
    ),
  );
}

export function index(entries, level = 'h3') {
  return h('ol', { class: 'index' }, entries.map((entry) => indexEntry(entry, level)));
}

/** A labelled form control with its error wired up for assistive technology. */
export function control(id, label, field, input, hint) {
  const describedBy = [hint ? `${id}-hint` : null, field.error ? `${id}-error` : null].filter(Boolean).join(' ') || null;
  const withAria = { ...input, attrs: { ...input.attrs, id, 'aria-describedby': describedBy, 'aria-invalid': field.error ? 'true' : null } };
  return h(
    'div',
    { class: field.error ? 'field field--error' : 'field' },
    h('label', { for: id }, label),
    hint ? h('p', { class: 'field__hint', id: `${id}-hint` }, hint) : null,
    withAria,
    field.error ? h('p', { class: 'field__error', id: `${id}-error` }, field.error) : null,
  );
}

/**
 * A QR code from a matrix of modules (true is dark). The code is always dark on white, because
 * scanners read that and not the page's theme; the matrix already holds its quiet border.
 * @param {boolean[][]} modules @param {string} label
 */
export function qrCode(modules, label) {
  const size = modules.length;
  let path = '';
  modules.forEach((row, y) => {
    let x = 0;
    while (x < size) {
      if (!row[x]) {
        x += 1;
        continue;
      }
      let end = x;
      while (end < size && row[end]) end += 1;
      path += `M${x} ${y}h${end - x}v1h-${end - x}z`;
      x = end;
    }
  });
  return h(
    'svg',
    { class: 'qr', viewBox: `0 0 ${size} ${size}`, width: 192, height: 192, role: 'img', 'aria-label': label, 'shape-rendering': 'crispEdges' },
    h('rect', { width: size, height: size, fill: '#ffffff' }),
    h('path', { d: path, fill: '#000000' }),
  );
}

/**
 * The button that loads the next page of a list section, when the view has one (`more` of a page
 * model: the section's name and whether its next page is on its way).
 * @param {{ section: string, busy: boolean } | null | undefined} more
 */
export function loadMore(more) {
  if (!more) return null;
  return h(
    'p',
    { class: 'toolbar' },
    h('button', { class: 'button button--quiet', type: 'button', disabled: more.busy, 'data-action': 'view.more', 'data-arg': JSON.stringify(more.section) }, more.busy ? 'Loading…' : 'Load more'),
  );
}
