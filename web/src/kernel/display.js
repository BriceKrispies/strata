// The display layer of the data-to-UI contract (docs/design-system.md, "Display types" and
// "International and accessible by rule").
//
// A display type maps a field's value to a display node: a plain object that says what to draw,
// never HTML. The renderer turns nodes into elements. This module is pure. It has no DOM, no
// globals and no clock: the value, the field's type, the display type with its props, and a context
// (request language, realm time zone, the current time in epoch milliseconds, and the people table
// of the view reply) come in through its arguments.
//
// The rules this module keeps by construction:
// - nothing is drawn as text that is an epoch number, a record id, a principal string or a JSON
//   object; an unknown object or array draws as an empty node or as chips of its string members;
// - numbers and money are formatted from exact decimal strings with BigInt, so values beyond 2^53
//   and fractional minor units stay exact; Intl supplies only the separators and the currency
//   pattern, and the digits are ours;
// - a civil date (`2026-11-24`) is formatted as itself, in UTC, so no zone can move it a day.
// @ts-check

/**
 * @typedef {'neutral' | 'info' | 'success' | 'warning' | 'danger'} Tone
 * @typedef {{ kind: string, [key: string]: any }} DisplayNode
 * @typedef {{ lang?: string, timeZone?: string, now?: number, people?: Record<string, { name: string | null, avatar?: string | null }> }} DisplayContext
 * @typedef {{ neg: boolean, digits: bigint, scale: number }} Decimal
 */

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;
const MAX_INSTANT = 8.64e15;
const ANON = 'A member';
const NOT_AVAILABLE = 'Not available';
const PRINCIPAL = /^(user|org|svc|service|role|group):\S+$/;
// A string of nine or more digits is an identifier or an epoch in digits, never prose: it is not drawn as text.
const IDENTIFIER = /^\d{9,}$/;
const DECIMAL = /^([+-]?)(\d+)(?:\.(\d+))?$/;
const CIVIL_DATE = /^(\d{4})-(\d{2})-(\d{2})$/;
const ROUNDING = ['half-even', 'half-up', 'half-down', 'down', 'up', 'floor', 'ceil'];
const TONES = ['neutral', 'info', 'success', 'warning', 'danger'];
const KNOWN = new Set([
  'text', 'rich-text', 'datetime', 'date', 'duration', 'count', 'number', 'money', 'percent', 'boolean',
  'status', 'choice', 'person', 'reference', 'references', 'tags', 'language', 'media', 'url', 'email', 'code',
]);

/** The tone of a status word when the presentation names none. @type {Record<string, Tone>} */
const DEFAULT_TONES = {
  published: 'success', active: 'success', approved: 'success', accepted: 'success', done: 'success',
  complete: 'success', completed: 'success', enabled: 'success', live: 'success',
  review: 'info', in_review: 'info', in_progress: 'info', scheduled: 'info', sent: 'info',
  pending: 'warning', waiting: 'warning', overdue: 'warning',
  draft: 'neutral', archived: 'neutral', closed: 'neutral', inactive: 'neutral', new: 'neutral',
  hidden: 'danger', removed: 'danger', rejected: 'danger', deleted: 'danger', failed: 'danger',
  expired: 'danger', suspended: 'danger',
};

/**
 * The display type a field with no declared display type is drawn by (the Fallbacks paragraph).
 * `list` draws as `tags`, the display type whose rendering is chips.
 * @param {string | undefined} fieldType
 * @returns {string}
 */
export function fallbackDisplayType(fieldType) {
  switch (fieldType) {
    case 'time': return 'datetime';
    case 'ref': return 'reference';
    case 'bool': return 'boolean';
    case 'list': return 'tags';
    case 'file': return 'media';
    case 'int': return 'number';
    case 'language': return 'language';
    case 'created_by': return 'person';
    default: return 'text';
  }
}

/**
 * Draw a value as a display node. A null, undefined or empty value is an empty node; a value the
 * declared display type cannot draw is an empty node too, never its raw form.
 * @param {{ value: any, fieldType?: string, displayType?: string, props?: Record<string, any> | null, ctx?: DisplayContext }} args
 * @returns {DisplayNode}
 */
export function formatValue({ value, fieldType, displayType, props, ctx }) {
  const c = context(ctx);
  const p = props ?? {};
  if (value === null || value === undefined || value === '') return empty();
  // created_by is always a person, whatever the declaration says.
  const type = fieldType === 'created_by' ? 'person' : KNOWN.has(displayType ?? '') ? /** @type {string} */ (displayType) : fallbackDisplayType(fieldType);
  return draw(type, value, p, c) ?? empty();
}

/**
 * The relative form of an instant ("just now", "5 minutes ago", "yesterday", "in 3 days"), or the
 * absolute date for an instant more than about a week away.
 * @param {number} ms the instant, in epoch milliseconds
 * @param {number} now the current time, in epoch milliseconds
 * @param {string} [lang] BCP 47 tag, default 'en'
 * @param {string} [timeZone] IANA zone, default 'UTC'; used for the calendar days and the absolute date
 * @returns {{ text: string, relative: boolean }}
 */
export function relativeTime(ms, now, lang = 'en', timeZone = 'UTC') {
  if (!isInstant(ms) || !isInstant(now)) return { text: '', relative: false };
  const diff = ms - now;
  const abs = Math.abs(diff);
  if (abs < MINUTE) return { text: 'just now', relative: true };
  const rtf = make(Intl.RelativeTimeFormat, lang, { numeric: 'auto' });
  if (abs < HOUR) return { text: rtf.format(Math.trunc(diff / MINUTE), 'minute'), relative: true };
  if (abs < DAY) return { text: rtf.format(Math.trunc(diff / HOUR), 'hour'), relative: true };
  const days = civilDay(ms, timeZone) - civilDay(now, timeZone);
  if (Math.abs(days) <= 7) return { text: rtf.format(days, 'day'), relative: true };
  return { text: absoluteDate(ms, lang, timeZone), relative: false };
}

// ---- context and small helpers ----------------------------------------------------------------

/** @param {DisplayContext | undefined} ctx */
function context(ctx) {
  const c = ctx ?? {};
  return {
    lang: typeof c.lang === 'string' && c.lang !== '' ? c.lang : 'en',
    timeZone: typeof c.timeZone === 'string' && c.timeZone !== '' ? c.timeZone : 'UTC',
    now: isInstant(c.now) ? /** @type {number} */ (c.now) : null,
    people: isRecord(c.people) ? c.people : {},
  };
}

/** @returns {DisplayNode} */
function empty() {
  return { kind: 'empty', text: '—' };
}

/** @param {unknown} v @returns {v is Record<string, any>} */
function isRecord(v) {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

/** @param {unknown} v @returns {boolean} */
function isInstant(v) {
  return typeof v === 'number' && Number.isFinite(v) && Math.abs(v) <= MAX_INSTANT;
}

/**
 * An own property of a plain map, so that a key such as `constructor` never reaches the prototype.
 * @param {any} obj @param {string} key @returns {any}
 */
function own(obj, key) {
  return obj !== null && typeof obj === 'object' && Object.prototype.hasOwnProperty.call(obj, key) ? obj[key] : undefined;
}

/** A word from an identifier or a status: `in_review` is "In review". @param {string} s */
function wordOf(s) {
  const spaced = s.replace(/[_-]+/g, ' ').trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** The label of an object type: `showcase_community__message` is "Message". @param {string} object */
function objectLabelOf(object) {
  return wordOf(object.split('__').pop() ?? '') || 'Item';
}

/** A link target the renderer may use: web addresses and same-origin paths only. @param {unknown} url */
function safeHref(url) {
  if (typeof url !== 'string') return null;
  if (/^https?:\/\/\S+$/i.test(url)) return url;
  if (/^\/(?!\/)\S*$/.test(url)) return url;
  return null;
}

/**
 * An Intl object for a language and options. An unknown language or zone falls back to English,
 * then to the default zone, so a bad setting degrades instead of throwing.
 * @param {any} Ctor @param {string} lang @param {Record<string, any>} options @returns {any}
 */
function make(Ctor, lang, options) {
  const { timeZone: _zone, ...withoutZone } = options;
  for (const [l, o] of /** @type {[string, Record<string, any>][]} */ ([[lang, options], ['en', options], ['en', withoutZone]])) {
    try {
      return new Ctor(l, o);
    } catch {
      // try the next attempt
    }
  }
  return new Ctor('en', {});
}

/** @param {string} lang @param {string} timeZone */
function absoluteDate(ms, lang, timeZone) {
  return make(Intl.DateTimeFormat, lang, { dateStyle: 'medium', timeZone }).format(new Date(ms));
}

/** The civil day number (days since the epoch) of an instant, in a zone. @param {number} ms @param {string} timeZone */
function civilDay(ms, timeZone) {
  const parts = make(Intl.DateTimeFormat, 'en-US', { timeZone, year: 'numeric', month: 'numeric', day: 'numeric' })
    .formatToParts(new Date(ms));
  /** @param {string} type */
  const field = (type) => Number(parts.find((/** @type {any} */ p) => p.type === type)?.value);
  return Date.UTC(field('year'), field('month') - 1, field('day')) / DAY;
}

/** The epoch milliseconds of a civil date at UTC midnight, or null when it is not a real date. @param {string} value */
function civilDateMs(value) {
  const m = CIVIL_DATE.exec(value);
  if (!m) return null;
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  const ms = Date.UTC(y, mo - 1, d);
  const check = new Date(ms);
  return check.getUTCFullYear() === y && check.getUTCMonth() === mo - 1 && check.getUTCDate() === d ? ms : null;
}

// ---- exact decimals ---------------------------------------------------------------------------

/**
 * Read an integer or a decimal string exactly. A JSON number is accepted only when it is a safe
 * integer, so no value passes through floating point.
 * @param {unknown} input @returns {Decimal | null}
 */
function parseDecimal(input) {
  let text;
  if (typeof input === 'number') {
    if (!Number.isSafeInteger(input)) return null;
    text = String(input);
  } else if (typeof input === 'string') {
    text = input;
  } else {
    return null;
  }
  const m = DECIMAL.exec(text);
  if (!m) return null;
  const frac = m[3] ?? '';
  const digits = BigInt(m[2] + frac);
  return { neg: m[1] === '-' && digits !== 0n, digits, scale: frac.length };
}

/** The same digits read with `by` more decimal places: a division by 10^by. @param {Decimal} d @param {number} by */
function shiftDown(d, by) {
  return { ...d, scale: d.scale + by };
}

/**
 * Round to `places` decimal places. The modes follow the usual names: `half-even` (banker's, the
 * default), `half-up` and `half-down` round halves away from or towards zero; `down` truncates,
 * `up` rounds away from zero, `floor` and `ceil` round towards minus and plus infinity.
 * @param {Decimal} d @param {number} places @param {string} mode @returns {Decimal}
 */
function roundTo(d, places, mode) {
  if (d.scale <= places) return { ...d, digits: d.digits * 10n ** BigInt(places - d.scale), scale: places };
  const div = 10n ** BigInt(d.scale - places);
  let q = d.digits / div;
  const r = d.digits % div;
  if (r !== 0n) {
    const half = r * 2n === div ? 0 : r * 2n > div ? 1 : -1;
    let up;
    switch (mode) {
      case 'down': up = false; break;
      case 'up': up = true; break;
      case 'floor': up = d.neg; break;
      case 'ceil': up = !d.neg; break;
      case 'half-up': up = half >= 0; break;
      case 'half-down': up = half > 0; break;
      default: up = half > 0 || (half === 0 && q % 2n === 1n); break;
    }
    if (up) q += 1n;
  }
  return { neg: d.neg && q !== 0n, digits: q, scale: places };
}

/** Integer and fraction digits of a decimal, as plain digit strings. @param {Decimal} d */
function plainParts(d) {
  const s = d.digits.toString().padStart(d.scale + 1, '0');
  return { int: s.slice(0, s.length - d.scale), frac: s.slice(s.length - d.scale) };
}

/** @param {string} digits @param {string} separator */
function groupDigits(digits, separator) {
  let out = '';
  for (let i = 0; i < digits.length; i++) {
    if (i > 0 && (digits.length - i) % 3 === 0) out += separator;
    out += digits[i];
  }
  return out;
}

/** The group and decimal separators of a language. @param {string} lang */
function separators(lang) {
  const parts = make(Intl.NumberFormat, lang, {}).formatToParts(1234567.5);
  return {
    group: parts.find((/** @type {any} */ p) => p.type === 'group')?.value ?? ',',
    decimal: parts.find((/** @type {any} */ p) => p.type === 'decimal')?.value ?? '.',
  };
}

/**
 * Render an exact decimal in a language with a style (decimal, percent or currency). Intl gives the
 * pattern: where the sign, the currency and the percent sign go. The digits are ours, so the
 * number is never converted to floating point.
 * @param {string} lang @param {Decimal} d @param {Record<string, any>} style @returns {string}
 */
function renderDecimal(lang, d, style) {
  const sep = separators(lang);
  const digits = d.scale > 0 ? 1 : 0;
  const parts = make(Intl.NumberFormat, lang, { ...style, minimumFractionDigits: digits, maximumFractionDigits: digits })
    .formatToParts(d.neg ? -1 : 1);
  const { int, frac } = plainParts(d);
  let out = '';
  for (const part of parts) {
    switch (part.type) {
      case 'integer': out += groupDigits(int, sep.group); break;
      case 'group': case 'plusSign': case 'decimal': case 'fraction': break;
      case 'minusSign': if (d.neg) out += part.value; break;
      default: out += part.value; break;
    }
    if (part.type === 'decimal' && d.scale > 0) out += sep.decimal;
    if (part.type === 'fraction') out += frac;
  }
  return out;
}

/** @param {string} code a currency code, or null when it is not one */
function validCurrency(code) {
  if (typeof code !== 'string' || !/^[A-Za-z]{3}$/.test(code)) return null;
  const upper = code.toUpperCase();
  try {
    new Intl.NumberFormat('en', { style: 'currency', currency: upper });
    return upper;
  } catch {
    return null;
  }
}

/** @param {string} code */
function currencyDigits(code) {
  try {
    return new Intl.NumberFormat('en', { style: 'currency', currency: code }).resolvedOptions().maximumFractionDigits ?? 2;
  } catch {
    return 2;
  }
}

/** @param {unknown} scale @returns {number} */
function declaredScale(scale) {
  return Number.isInteger(scale) && /** @type {number} */ (scale) >= 0 && /** @type {number} */ (scale) <= 30 ? /** @type {number} */ (scale) : 0;
}

// ---- display types ----------------------------------------------------------------------------

/**
 * @param {string} type
 * @param {any} value
 * @param {Record<string, any>} props
 * @param {ReturnType<typeof context>} c
 * @returns {DisplayNode | null}
 */
function draw(type, value, props, c) {
  switch (type) {
    case 'text': return textNode(value, props);
    case 'rich-text': return typeof value === 'string' ? { kind: 'rich', markdown: value } : null;
    case 'datetime': return timeNode(value, 'datetime', props, c);
    case 'date': return typeof value === 'string' ? civilDateNode(value, c.lang) : timeNode(value, 'date', props, c);
    case 'duration': return textOf(durationText(c.lang, value));
    case 'count': return countNode(value, props, c.lang);
    case 'number': return numberNode(value, props, c.lang);
    case 'money': return moneyNode(value, props, c.lang);
    case 'percent': return numberNode(value, { scale: props.scale }, c.lang, '%');
    case 'boolean': return boolNode(value, props);
    case 'status': return statusNode(value, props);
    case 'choice': return choiceNode(value, props);
    case 'person': return personNode(value, c);
    case 'reference': return refNode(value, props);
    case 'references': return Array.isArray(value) && value.length > 0 ? chips(value.map((v) => refNode(v, props))) : null;
    case 'tags': return tagsNode(value);
    case 'language': return languageNode(value, c.lang);
    case 'media': return mediaNode(value);
    case 'url': return urlNode(value);
    case 'email': return emailNode(value);
    case 'code': return codeNode(value);
    default: return null;
  }
}

/** @param {string | null} text @returns {DisplayNode | null} */
function textOf(text) {
  return text === null ? null : { kind: 'text', text };
}

/** @param {DisplayNode[]} items */
function chips(items) {
  return { kind: 'chips', items };
}

/** Text is drawn only when it is a string and not a principal. @param {any} value @param {Record<string, any>} props */
function textNode(value, props) {
  if (typeof value !== 'string' || PRINCIPAL.test(value) || IDENTIFIER.test(value)) return null;
  return props.multiline === true ? { kind: 'text', text: value, multiline: true } : { kind: 'text', text: value };
}

/**
 * @param {any} value @param {'datetime' | 'date'} displayType @param {Record<string, any>} props @param {ReturnType<typeof context>} c
 * @returns {DisplayNode | null}
 */
function timeNode(value, displayType, props, c) {
  if (!isInstant(value)) return null;
  const iso = new Date(value).toISOString();
  if (displayType === 'date') return { kind: 'time', text: absoluteDate(value, c.lang, c.timeZone), iso, relative: false };
  if (props.style !== 'absolute' && c.now !== null) {
    const r = relativeTime(value, c.now, c.lang, c.timeZone);
    return { kind: 'time', text: r.text, iso, relative: r.relative };
  }
  const text = make(Intl.DateTimeFormat, c.lang, { dateStyle: 'medium', timeStyle: 'short', timeZone: c.timeZone }).format(new Date(value));
  return { kind: 'time', text, iso, relative: false };
}

/** A civil date is formatted as itself: UTC midnight, formatted in UTC, so no zone moves the day. @param {string} value @param {string} lang */
function civilDateNode(value, lang) {
  const ms = civilDateMs(value);
  if (ms === null) return null;
  const text = make(Intl.DateTimeFormat, lang, { dateStyle: 'medium', timeZone: 'UTC' }).format(new Date(ms));
  return { kind: 'time', text, iso: value, relative: false };
}

/** Seconds as "2 hr 5 min" (unit names from Intl). @param {string} lang @param {any} value @returns {string | null} */
function durationText(lang, value) {
  const d = parseDecimal(value);
  if (!d || d.neg || d.scale > 0 || !Number.isSafeInteger(Number(d.digits))) return null;
  const s = Number(d.digits);
  /** @type {[number, string][]} */
  const units = [
    [Math.floor(s / 3600), 'hour'],
    [Math.floor((s % 3600) / 60), 'minute'],
    [s % 60, 'second'],
  ];
  const parts = units
    .filter(([n]) => n > 0)
    .map(([n, unit]) => make(Intl.NumberFormat, lang, { style: 'unit', unit, unitDisplay: 'short' }).format(n));
  if (parts.length === 0) return make(Intl.NumberFormat, lang, { style: 'unit', unit: 'second', unitDisplay: 'short' }).format(0);
  return parts.join(' ');
}

/** @param {any} value @param {Record<string, any>} props @param {string} lang */
function countNode(value, props, lang) {
  const d = parseDecimal(value);
  if (!d) return null;
  if (props.compact === true && d.scale === 0 && Number.isSafeInteger(Number(d.digits))) {
    const n = d.neg ? -Number(d.digits) : Number(d.digits);
    return { kind: 'number', text: make(Intl.NumberFormat, lang, { notation: 'compact' }).format(n) };
  }
  return { kind: 'number', text: renderDecimal(lang, d, { style: 'decimal' }) };
}

/**
 * A number from an int or decimal string, with a declared scale: `scale` places are read as
 * decimals (4250 at scale 2 is 42.50). `suffix` '%' draws the value as a percentage.
 * @param {any} value @param {Record<string, any>} props @param {string} lang @param {string} [suffix]
 */
function numberNode(value, props, lang, suffix) {
  const d = parseDecimal(value);
  if (!d) return null;
  const shifted = shiftDown(d, declaredScale(props.scale));
  const text = suffix ? renderDecimal(lang, shifted, { style: 'percent' }) : renderDecimal(lang, shifted, { style: 'decimal' });
  return { kind: 'number', text };
}

/**
 * Money from minor units. The currency comes from props; the scale (places read as decimals in the
 * stored integer) defaults to the currency's minor units; the result is rounded to the currency's
 * minor units with the rounding mode (half-even by default), on the exact value.
 * @param {any} value @param {Record<string, any>} props @param {string} lang
 */
function moneyNode(value, props, lang) {
  const d0 = parseDecimal(value);
  if (!d0) return null;
  const currency = validCurrency(props.currency);
  const digits = currency ? currencyDigits(currency) : 2;
  const scale = Number.isInteger(props.scale) && props.scale >= 0 ? props.scale : digits;
  const rounding = ROUNDING.includes(props.rounding) ? props.rounding : 'half-even';
  const d = roundTo(shiftDown(d0, scale), digits, rounding);
  const style = currency ? { style: 'currency', currency } : { style: 'decimal' };
  return { kind: 'money', text: renderDecimal(lang, d, style) };
}

/** @param {any} value @param {Record<string, any>} props */
function boolNode(value, props) {
  if (typeof value !== 'boolean') return null;
  const yes = typeof props.yes === 'string' ? props.yes : 'Yes';
  const no = typeof props.no === 'string' ? props.no : 'No';
  return { kind: 'boolean', value, text: value ? yes : no };
}

/** @param {any} value @param {Record<string, any>} props */
function statusNode(value, props) {
  if (typeof value !== 'string' || PRINCIPAL.test(value)) return null;
  const label = own(props.labels, value);
  const explicit = own(props.tones, value);
  /** @type {Tone} */
  const tone = typeof explicit === 'string' && TONES.includes(explicit)
    ? /** @type {Tone} */ (explicit)
    : own(DEFAULT_TONES, value.toLowerCase().replace(/[\s-]+/g, '_')) ?? 'neutral';
  return { kind: 'status', text: typeof label === 'string' ? label : wordOf(value), tone };
}

/** @param {any} value @param {Record<string, any>} props */
function choiceNode(value, props) {
  if (typeof value !== 'string' || PRINCIPAL.test(value)) return null;
  /** @type {unknown} */
  let label;
  if (Array.isArray(props.options)) {
    const hit = props.options.find((/** @type {any} */ o) => isRecord(o) && o.value === value);
    label = hit?.label;
  } else {
    label = own(props.options, value);
  }
  return { kind: 'text', text: typeof label === 'string' ? label : wordOf(value) };
}

/**
 * A person: the name from the reply's people table, by its opaque handle. A bare principal string,
 * an unknown handle or a missing name draws "A member".
 * @param {any} value @param {ReturnType<typeof context>} c
 */
function personNode(value, c) {
  const handle = isRecord(value) && typeof value.person === 'string' ? value.person : null;
  const entry = handle === null ? undefined : own(c.people, handle);
  const raw = isRecord(entry) && typeof entry.name === 'string' ? entry.name.trim() : '';
  if (raw === '' || PRINCIPAL.test(raw)) return { kind: 'person', name: ANON, initials: '', avatar: null };
  const avatar = isRecord(entry) ? safeHref(entry.avatar) : null;
  const initials = raw.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => Array.from(w)[0].toUpperCase()).join('');
  return { kind: 'person', name: raw, initials, avatar };
}

/**
 * A reference: a link whose text is the target's title, or the object type's label when the title
 * is null. An unreadable target (`{ ref: null }`) and a bare id are "Not available", never a link
 * and never the id.
 * @param {any} value @param {Record<string, any>} props
 * @returns {DisplayNode}
 */
function refNode(value, props) {
  const unavailable = { kind: 'text', text: NOT_AVAILABLE, muted: true };
  const ref = isRecord(value) ? value.ref : undefined;
  if (!isRecord(ref) || typeof ref.object !== 'string' || !(typeof ref.id === 'string' || typeof ref.id === 'number')) return unavailable;
  const title = typeof ref.title === 'string' && ref.title.trim() !== '' ? ref.title : null;
  const label = typeof props.objectLabel === 'string' && props.objectLabel !== '' ? props.objectLabel : objectLabelOf(ref.object);
  return { kind: 'link', text: title ?? label, ref: { object: ref.object, id: ref.id } };
}

/** Tags: the string members of a list as chips; anything else is left out. @param {any} value */
function tagsNode(value) {
  if (!Array.isArray(value)) return null;
  const items = value
    .filter((v) => typeof v === 'string' && v !== '' && !PRINCIPAL.test(v) && !IDENTIFIER.test(v))
    .map((v) => ({ kind: 'text', text: v }));
  return items.length > 0 ? chips(items) : null;
}

/** @param {any} value @param {string} lang */
function languageNode(value, lang) {
  if (typeof value !== 'string' || !/^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(value)) return null;
  try {
    const text = make(Intl.DisplayNames, lang, { type: 'language' }).of(value);
    return typeof text === 'string' && text !== '' ? { kind: 'language', text, tag: value } : null;
  } catch {
    return null;
  }
}

/** A file: an image with its alt text, or a file link with its name, type and size. @param {any} value */
function mediaNode(value) {
  if (!isRecord(value)) return null;
  const src = safeHref(value.url);
  const type = typeof value.type === 'string' ? value.type : null;
  const size = Number.isSafeInteger(value.size) && value.size >= 0 ? value.size : null;
  const name = typeof value.name === 'string' && value.name !== '' ? value.name : 'File';
  const image = src !== null && type !== null && type.startsWith('image/');
  const alt = typeof value.alt === 'string' ? value.alt : '';
  return { kind: 'media', image, src, alt, text: name, type, size };
}

/** @param {any} value */
function urlNode(value) {
  if (typeof value !== 'string' || value === '' || PRINCIPAL.test(value)) return null;
  const href = safeHref(value);
  return href !== null ? { kind: 'url', text: value, href } : { kind: 'text', text: value };
}

/** @param {any} value */
function emailNode(value) {
  if (typeof value !== 'string' || !/^[^\s@?#/]+@[^\s@?#/]+$/.test(value)) return null;
  return { kind: 'email', text: value, href: `mailto:${value}` };
}

/** Code is drawn as it is, and only when declared as code. @param {any} value */
function codeNode(value) {
  if (typeof value === 'string') return PRINCIPAL.test(value) ? null : { kind: 'code', text: value };
  if (typeof value === 'number' && Number.isSafeInteger(value)) return { kind: 'code', text: String(value) };
  return null;
}
