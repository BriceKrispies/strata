// Display nodes (web/src/kernel/display.js): a table per display type, the fallback of each field
// type, and a property check that no fallback draws an epoch, a record id, a principal or JSON.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { fallbackDisplayType, formatValue, relativeTime } from '../src/kernel/display.js';

const HOUR = 3_600_000;
const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 24, 12, 0, 0);
const EMPTY = { kind: 'empty', text: '—' };
const MESSAGE = 'showcase_community__message';

const ctx = {
  lang: 'en',
  timeZone: 'UTC',
  now: NOW,
  people: { h1: { name: 'Ada Lovelace', avatar: null }, h2: { name: null, avatar: null } },
};
const fmt = (/** @type {Record<string, any>} */ args) => formatValue({ ctx, ...args });

test('display types: text, rich text, and the empty value', () => {
  assert.deepEqual(fmt({ value: 'Hello', fieldType: 'str', displayType: 'text' }), { kind: 'text', text: 'Hello' });
  assert.deepEqual(fmt({ value: 'a\nb', fieldType: 'str', displayType: 'text', props: { multiline: true } }), { kind: 'text', text: 'a\nb', multiline: true });
  assert.deepEqual(fmt({ value: 'user:ada', fieldType: 'str', displayType: 'text' }), EMPTY);
  assert.deepEqual(fmt({ value: 42, fieldType: 'str', displayType: 'text' }), EMPTY);
  assert.deepEqual(fmt({ value: { a: 1 }, fieldType: 'str', displayType: 'text' }), EMPTY);
  assert.deepEqual(fmt({ value: null, fieldType: 'str', displayType: 'text' }), EMPTY);
  assert.deepEqual(fmt({ value: '**hi**', fieldType: 'str', displayType: 'rich-text' }), { kind: 'rich', markdown: '**hi**' });
});

test('display type: datetime is relative by default, and never the raw epoch', () => {
  const five = NOW - 5 * 60_000;
  assert.deepEqual(fmt({ value: five, fieldType: 'time', displayType: 'datetime' }), {
    kind: 'time', text: '5 minutes ago', iso: new Date(five).toISOString(), relative: true,
  });
  assert.equal(fmt({ value: NOW - 10_000, fieldType: 'time', displayType: 'datetime' }).text, 'just now');
  assert.equal(fmt({ value: NOW - 3 * HOUR, fieldType: 'time', displayType: 'datetime' }).text, '3 hours ago');
  assert.equal(fmt({ value: NOW - 26 * HOUR, fieldType: 'time', displayType: 'datetime' }).text, 'yesterday');
  assert.equal(fmt({ value: NOW + 3 * DAY, fieldType: 'time', displayType: 'datetime' }).text, 'in 3 days');
  const old = fmt({ value: NOW - 10 * DAY, fieldType: 'time', displayType: 'datetime' });
  assert.equal(old.text, 'Nov 14, 2026');
  assert.equal(old.relative, false);
});

test('display type: datetime with style absolute shows the date', () => {
  const node = fmt({ value: Date.UTC(2026, 10, 22, 14, 5), fieldType: 'time', displayType: 'datetime', props: { style: 'absolute' } });
  assert.equal(node.kind, 'time');
  assert.ok(node.text.includes('Nov 22, 2026'), node.text);
  assert.equal(node.relative, false);
});

test('display type: datetime refuses values that are not instants', () => {
  assert.deepEqual(fmt({ value: 'user:ada', fieldType: 'time', displayType: 'datetime' }), EMPTY);
  assert.deepEqual(fmt({ value: 1e20, fieldType: 'time', displayType: 'datetime' }), EMPTY);
});

test('display type: date formats a civil date as itself, in any zone', () => {
  for (const timeZone of ['Etc/GMT+12', 'Pacific/Kiritimati', 'UTC']) {
    assert.deepEqual(
      formatValue({ value: '2026-11-24', fieldType: 'time', displayType: 'date', ctx: { ...ctx, timeZone } }),
      { kind: 'time', text: 'Nov 24, 2026', iso: '2026-11-24', relative: false },
    );
  }
  assert.deepEqual(fmt({ value: '2026-02-30', fieldType: 'time', displayType: 'date' }), EMPTY);
  assert.equal(fmt({ value: NOW, fieldType: 'time', displayType: 'date' }).text, 'Nov 24, 2026');
});

test('display types: duration, count, number, percent', () => {
  assert.deepEqual(fmt({ value: 7500, fieldType: 'int', displayType: 'duration' }), { kind: 'text', text: '2 hr 5 min' });
  assert.deepEqual(fmt({ value: 1234567, fieldType: 'int', displayType: 'count' }), { kind: 'number', text: '1,234,567' });
  assert.deepEqual(fmt({ value: 1200, fieldType: 'int', displayType: 'count', props: { compact: true } }), { kind: 'number', text: '1.2K' });
  assert.deepEqual(fmt({ value: '12345678901234567890123', fieldType: 'int', displayType: 'number' }), {
    kind: 'number', text: '12,345,678,901,234,567,890,123',
  });
  assert.deepEqual(fmt({ value: '12345678901234567890123', fieldType: 'int', displayType: 'number', props: { scale: 2 } }), {
    kind: 'number', text: '123,456,789,012,345,678,901.23',
  });
  assert.deepEqual(fmt({ value: '0.1', fieldType: 'int', displayType: 'number' }), { kind: 'number', text: '0.1' });
  assert.deepEqual(fmt({ value: 4250, fieldType: 'int', displayType: 'percent', props: { scale: 2 } }), { kind: 'number', text: '42.50%' });
});

test('display type: money is exact, rounds half-even by default, and takes currency and scale from props', () => {
  assert.deepEqual(fmt({ value: '12345678901234567890123', fieldType: 'int', displayType: 'money', props: { currency: 'EUR' } }), {
    kind: 'money', text: '€123,456,789,012,345,678,901.23',
  });
  // 1005.5 minor units is 10.055 dollars: half-even keeps 10.06 (5 is odd), half-down keeps 10.05.
  assert.equal(fmt({ value: '1005.5', fieldType: 'int', displayType: 'money', props: { currency: 'USD' } }).text, '$10.06');
  assert.equal(fmt({ value: '1005.5', fieldType: 'int', displayType: 'money', props: { currency: 'USD', rounding: 'half-down' } }).text, '$10.05');
  assert.equal(fmt({ value: '-5', fieldType: 'int', displayType: 'money', props: { currency: 'USD' } }).text, '-$0.05');
  assert.equal(fmt({ value: '1500', fieldType: 'int', displayType: 'money', props: { currency: 'JPY' } }).text, '¥1,500');
  assert.equal(fmt({ value: '250', fieldType: 'int', displayType: 'money' }).text, '2.50');
});

test('display types: boolean, status, choice', () => {
  assert.deepEqual(fmt({ value: true, fieldType: 'bool', displayType: 'boolean' }), { kind: 'boolean', value: true, text: 'Yes' });
  assert.deepEqual(fmt({ value: false, fieldType: 'bool', displayType: 'boolean', props: { no: 'Nope' } }), { kind: 'boolean', value: false, text: 'Nope' });
  // A status is a state said in words (D-DS-31): a label, a mark and, when declared, what it means.
  assert.deepEqual(fmt({ value: 'published', fieldType: 'str', displayType: 'status' }), { kind: 'state', value: 'published', label: 'Published', mark: 'normal', means: null });
  assert.deepEqual(fmt({ value: 'hidden', fieldType: 'str', displayType: 'status' }), { kind: 'state', value: 'hidden', label: 'Hidden', mark: 'attention', means: null });
  for (const value of ['published', 'active', 'open', 'done', 'sent', 'read', 'approved']) assert.equal(fmt({ value, fieldType: 'str', displayType: 'status' }).mark, 'normal', value);
  for (const value of ['draft', 'pending', 'scheduled', 'queued']) assert.equal(fmt({ value, fieldType: 'str', displayType: 'status' }).mark, 'note', value);
  for (const value of ['hidden', 'rejected', 'failed', 'overdue', 'left', 'archived', 'removed', 'suspended']) assert.equal(fmt({ value, fieldType: 'str', displayType: 'status' }).mark, 'attention', value);
  assert.equal(fmt({ value: 'zzz', fieldType: 'str', displayType: 'status' }).mark, 'note', 'an undeclared value is never hidden');
  assert.equal(fmt({ value: 'in_review', fieldType: 'str', displayType: 'status' }).label, 'In review');
  // The presentation's props, flat as a string map holds them, or nested.
  const declared = { 'state.hidden': 'danger', 'means.hidden': 'Members cannot see this message.', 'label.hidden': 'Hidden by a moderator', 'state.published': 'normal' };
  assert.deepEqual(fmt({ value: 'hidden', fieldType: 'str', displayType: 'status', props: declared }), {
    kind: 'state', value: 'hidden', label: 'Hidden by a moderator', mark: 'danger', means: 'Members cannot see this message.',
  });
  assert.equal(fmt({ value: 'draft', fieldType: 'str', displayType: 'status', props: { state: { draft: 'normal' } } }).mark, 'normal');
  assert.equal(fmt({ value: 'draft', fieldType: 'str', displayType: 'status', props: { 'state.draft': 'loud' } }).mark, 'note', 'a mark outside the list is ignored');
  assert.equal(fmt({ value: 'published', fieldType: 'str', displayType: 'status', props: { labels: { published: 'Live' } } }).label, 'Live');
  const options = [{ value: 'general', label: 'General' }];
  assert.deepEqual(fmt({ value: 'general', fieldType: 'str', displayType: 'choice', props: { options } }), { kind: 'text', text: 'General' });
});

test('display type: person resolves through the people table, else "A member"', () => {
  assert.deepEqual(fmt({ value: { person: 'h1' }, fieldType: 'str', displayType: 'person' }), {
    kind: 'person', name: 'Ada Lovelace', initials: 'AL', avatar: null,
  });
  const withAvatar = { ...ctx, people: { h1: { name: 'Ada Lovelace', avatar: 'https://example.com/a.png' } } };
  assert.equal(formatValue({ value: { person: 'h1' }, fieldType: 'created_by', ctx: withAvatar }).avatar, 'https://example.com/a.png');
  for (const value of [{ person: 'h2' }, { person: 'h9' }, 'user:ada', 'user:ines', { person: 'user:ada' }]) {
    assert.deepEqual(fmt({ value, fieldType: 'created_by' }), { kind: 'person', name: 'A member', initials: '', avatar: null }, JSON.stringify(value));
  }
});

test('display types: reference, references', () => {
  const target = { ref: { object: MESSAGE, id: 'm1', title: 'Hello there' } };
  assert.deepEqual(fmt({ value: target, fieldType: 'ref', displayType: 'reference' }), {
    kind: 'link', text: 'Hello there', ref: { object: MESSAGE, id: 'm1' },
  });
  const untitled = { ref: { object: MESSAGE, id: 'm1', title: null } };
  assert.equal(fmt({ value: untitled, fieldType: 'ref', displayType: 'reference' }).text, 'Message');
  assert.equal(fmt({ value: untitled, fieldType: 'ref', displayType: 'reference', props: { objectLabel: 'Post' } }).text, 'Post');
  const unreadable = { kind: 'text', text: 'Not available', muted: true };
  assert.deepEqual(fmt({ value: { ref: null }, fieldType: 'ref', displayType: 'reference' }), unreadable);
  assert.deepEqual(fmt({ value: 'm1', fieldType: 'ref', displayType: 'reference' }), unreadable);
  assert.deepEqual(fmt({ value: 42, fieldType: 'ref', displayType: 'reference' }), unreadable);
  const link = { kind: 'link', text: 'Hello there', ref: { object: MESSAGE, id: 'm1' } };
  assert.deepEqual(fmt({ value: [target, { ref: null }], fieldType: 'list', displayType: 'references' }), {
    kind: 'list', text: 'Hello there and Not available', parts: [{ item: link }, { literal: ' and ' }, { item: unreadable }],
  });
});

test('display types: tags keep only the string members, as a prose list', () => {
  assert.deepEqual(fmt({ value: ['ada', 'user:ada', { x: 1 }, 'grace'], fieldType: 'list', displayType: 'tags' }), {
    kind: 'list', text: 'ada and grace', parts: [{ item: { kind: 'text', text: 'ada' } }, { literal: ' and ' }, { item: { kind: 'text', text: 'grace' } }],
  });
  assert.equal(fmt({ value: ['English', 'Tamil', 'Hindi'], fieldType: 'list' }).text, 'English, Tamil, and Hindi');
  assert.equal(fmt({ value: ['English', 'Tamil'], fieldType: 'list', ctx: { ...ctx, lang: 'fr' } }).text, 'English et Tamil');
  assert.deepEqual(fmt({ value: ['user:ada'], fieldType: 'list', displayType: 'tags' }), EMPTY);
});

test('display types: language, media, url, email, code', () => {
  assert.deepEqual(fmt({ value: 'fr', fieldType: 'language', displayType: 'language' }), { kind: 'language', text: 'French', tag: 'fr' });
  assert.deepEqual(fmt({ value: { name: 'cat.png', type: 'image/png', url: '/files/cat.png', alt: 'A cat', size: 2048 }, fieldType: 'file', displayType: 'media' }), {
    kind: 'media', image: true, src: '/files/cat.png', alt: 'A cat', text: 'cat.png', type: 'image/png', size: 2048,
  });
  assert.equal(fmt({ value: { name: 'a.pdf', type: 'application/pdf', url: 'https://example.com/a.pdf' }, fieldType: 'file', displayType: 'media' }).image, false);
  assert.equal(fmt({ value: { name: 'x', type: 'image/png', url: 'javascript:alert(1)' }, fieldType: 'file', displayType: 'media' }).src, null);
  assert.deepEqual(fmt({ value: 'https://example.com/x', fieldType: 'str', displayType: 'url' }), { kind: 'url', text: 'https://example.com/x', href: 'https://example.com/x' });
  assert.deepEqual(fmt({ value: 'javascript:alert(1)', fieldType: 'str', displayType: 'url' }), { kind: 'text', text: 'javascript:alert(1)' });
  assert.deepEqual(fmt({ value: 'ada@example.com', fieldType: 'str', displayType: 'email' }), { kind: 'email', text: 'ada@example.com', href: 'mailto:ada@example.com' });
  assert.deepEqual(fmt({ value: 'ab-12', fieldType: 'str', displayType: 'code' }), { kind: 'code', text: 'ab-12' });
  assert.deepEqual(fmt({ value: 42, fieldType: 'int', displayType: 'code' }), { kind: 'code', text: '42' });
});

test('fallbackDisplayType follows the Fallbacks paragraph', () => {
  const table = {
    time: 'datetime', ref: 'reference', bool: 'boolean', list: 'tags', file: 'media', int: 'number',
    str: 'text', language: 'language', created_by: 'person', unknown: 'text',
  };
  for (const [fieldType, displayType] of Object.entries(table)) assert.equal(fallbackDisplayType(fieldType), displayType, fieldType);
});

test('fallbacks draw each field type by its own rule', () => {
  assert.equal(fmt({ value: NOW - 5 * 60_000, fieldType: 'time' }).text, '5 minutes ago');
  assert.equal(fmt({ value: { ref: { object: MESSAGE, id: 'm1', title: null } }, fieldType: 'ref' }).text, 'Message');
  assert.equal(fmt({ value: true, fieldType: 'bool' }).text, 'Yes');
  assert.equal(fmt({ value: ['a'], fieldType: 'list' }).kind, 'list');
  assert.equal(fmt({ value: { name: 'a.png', type: 'image/png', url: '/a.png' }, fieldType: 'file' }).kind, 'media');
  assert.equal(fmt({ value: '12345678901234', fieldType: 'int' }).text, '12,345,678,901,234');
  assert.equal(fmt({ value: 'hello', fieldType: 'str' }).text, 'hello');
  assert.equal(fmt({ value: 'de', fieldType: 'language' }).text, 'German');
  assert.equal(fmt({ value: { person: 'h1' }, fieldType: 'created_by', displayType: 'text' }).kind, 'person');
  assert.deepEqual(fmt({ value: { a: 1, b: [2] }, fieldType: 'str' }), EMPTY);
  assert.deepEqual(fmt({ value: { a: 1 }, fieldType: 'list' }), EMPTY);
});

test('relativeTime: the wording and the switch to an absolute date', () => {
  assert.deepEqual(relativeTime(NOW, NOW, 'en', 'UTC'), { text: 'just now', relative: true });
  assert.deepEqual(relativeTime(NOW + 2 * DAY, NOW, 'en', 'UTC'), { text: 'in 2 days', relative: true });
  assert.deepEqual(relativeTime(NOW - 30 * DAY, NOW, 'en', 'UTC'), { text: 'Oct 25, 2026', relative: false });
});

test('a bad language or zone degrades instead of throwing', () => {
  const node = formatValue({ value: NOW - 5 * 60_000, fieldType: 'time', ctx: { lang: '!!', timeZone: 'Nowhere/Zone', now: NOW } });
  assert.equal(node.text, '5 minutes ago');
});

test('the module reads no clock, no globals and no page', () => {
  const source = readFileSync(fileURLToPath(new URL('../src/kernel/display.js', import.meta.url)), 'utf8');
  for (const pattern of [/Date\.now\(/, /performance\./, /\bwindow\b/, /\bdocument\b/, /globalThis/, /\bprocess\b/]) {
    assert.doesNotMatch(source, pattern);
  }
});

// ---- property check ---------------------------------------------------------------------------

/** A seeded generator (mulberry32), so a failure reproduces. @param {number} seed */
function generator(seed) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** @template T @param {() => number} r @param {T[]} items @returns {T} */
function pick(r, items) {
  return items[Math.floor(r() * items.length)];
}

/** Letters and spaces only: no colon, no digits, so a string never looks like a principal or an id. @param {() => number} r */
function word(r) {
  const letters = 'abcdefghijklmnopqrstuvwxyz ';
  let out = '';
  const n = 1 + Math.floor(r() * 12);
  for (let i = 0; i < n; i++) out += letters[Math.floor(r() * letters.length)];
  return out.trim() || 'x';
}

/** @param {() => number} r @param {number} n */
function digits(r, n) {
  let out = String(1 + Math.floor(r() * 9));
  for (let i = 1; i < n; i++) out += String(Math.floor(r() * 10));
  return out;
}

/** Values no fallback may draw as they are: nested objects, epochs, principals, big numbers. @param {() => number} r */
function junk(r) {
  const epoch = Math.floor(r() * 2e12);
  return pick(r, [
    'user:ada', `user:${word(r)}`, { user: word(r) },
    { nested: { deep: [epoch, 'user:x', { id: `m${digits(r, 6)}` }] } },
    [epoch, digits(r, 14), { a: 1 }], epoch, digits(r, 14), true, null, word(r), { person: 'user:ada' },
  ]);
}

/**
 * A value of a field type, drawn at random: plausible shapes, epochs, principals, nested junk.
 * @param {string} fieldType @param {() => number} r @returns {{ value: any, id?: string }}
 */
function valueOf(fieldType, r) {
  const id = `m${digits(r, 9)}`;
  const epoch = Math.floor(r() * 2e12);
  switch (fieldType) {
    case 'time': return { value: r() < 0.6 ? epoch : junk(r) };
    case 'ref': {
      const target = { ref: { object: MESSAGE, id, title: r() < 0.5 ? null : word(r) } };
      return { value: pick(r, [target, { ref: null }, id, Number(digits(r, 8)), junk(r)]), id };
    }
    case 'bool': return { value: pick(r, [true, false, epoch, digits(r, 14), junk(r)]) };
    case 'list': {
      const items = [];
      for (let i = Math.floor(r() * 5); i > 0; i--) items.push(pick(r, [word(r), 'user:ada', epoch, { x: 1 }, digits(r, 13), junk(r)]));
      return { value: items };
    }
    case 'file': return { value: r() < 0.5 ? { name: word(r), type: pick(r, ['image/png', 'application/pdf']), url: `/f/${word(r).replace(/ /g, '')}`, size: Math.floor(r() * 1e6), alt: word(r) } : junk(r) };
    case 'int': return { value: r() < 0.5 ? digits(r, 1 + Math.floor(r() * 25)) : pick(r, [epoch, Math.floor(r() * 1e9), `-${digits(r, 20)}`, junk(r)]) };
    case 'str': return { value: r() < 0.7 ? word(r) : junk(r) };
    case 'language': return { value: pick(r, ['en', 'fr', 'de', 'ja', junk(r)]) };
    case 'created_by': return { value: r() < 0.7 ? { person: pick(r, ['h1', 'h2', 'h9']) } : junk(r) };
    default: throw new Error(`no generator for ${fieldType}`);
  }
}

const KINDS = new Set(['text', 'rich', 'time', 'person', 'link', 'state', 'list', 'boolean', 'number', 'money', 'code', 'url', 'email', 'media', 'empty', 'language']);

/** Every string a node draws. @param {any} node @param {string[]} out @returns {string[]} */
function drawnText(node, out = []) {
  for (const key of ['text', 'name', 'alt', 'label', 'means']) if (typeof node[key] === 'string') out.push(node[key]);
  if (Array.isArray(node.items)) for (const item of node.items) drawnText(item, out);
  if (Array.isArray(node.parts)) for (const part of node.parts) if (part.item) drawnText(part.item, out); else out.push(part.literal);
  return out;
}

/** @param {any} node @param {string | undefined} id */
function assertDrawable(node, id, label) {
  assert.ok(KINDS.has(node.kind), `${label}: unknown kind ${node.kind}`);
  for (const text of drawnText(node)) {
    assert.doesNotMatch(text, /^\d{12,}$/, `${label}: epoch-like text ${text}`);
    assert.ok(!text.includes('user:'), `${label}: principal drawn as ${text}`);
    assert.ok(!text.includes('{"'), `${label}: JSON drawn as ${text}`);
    assert.ok(!text.includes('[object') && !text.includes('NaN') && !text.includes('undefined'), `${label}: ${text}`);
    if (id !== undefined) assert.notEqual(text, id, `${label}: a record id drawn as text`);
  }
}

test('property: no fallback draws an epoch, a record id, a principal or JSON', () => {
  const r = generator(20261124);
  for (const fieldType of ['time', 'ref', 'bool', 'list', 'file', 'int', 'str', 'language', 'created_by']) {
    for (let i = 0; i < 150; i++) {
      const { value, id } = valueOf(fieldType, r);
      const node = formatValue({ value, fieldType, ctx });
      assertDrawable(node, id, `${fieldType} ${JSON.stringify(value)}`);
    }
  }
});

test('property: every declared display type also keeps the rules on junk', () => {
  const r = generator(7);
  // code is left out: it is the one display type that draws an identifier as it is, when declared.
  const displayTypes = ['text', 'datetime', 'date', 'duration', 'count', 'number', 'money', 'percent', 'boolean', 'status', 'person', 'reference', 'references', 'tags', 'language', 'media', 'url', 'email'];
  for (const displayType of displayTypes) {
    for (let i = 0; i < 60; i++) {
      const value = junk(r);
      const node = formatValue({ value, fieldType: 'str', displayType, ctx });
      assertDrawable(node, undefined, `${displayType} ${JSON.stringify(value)}`);
    }
  }
});
