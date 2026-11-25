// The Settings page of the frontend kernel (WG-5601). Pure, like the rest of the kernel: no fetch,
// no DOM, no clock.
//
// The page reads the definitions route (CT-307) and shows, per setting, the value the person reads,
// where that value comes from, and the levels this person may change. A save is the generated
// contract of its level: CT-306 for the person's own level, CT-304 for the organisation's level,
// which only a tenant administrator is offered. The reply to a save, or a notice that a setting
// changed, reads the page again, so every open page shows the value the site now holds.
//
// The words of a source and of a refusal are the ones the page says, never a record id, a revision
// or a code: a value is said in words ("On", "Narrow") and a locked setting says why it is read only.
// @ts-check

import { humanise } from './compose.js';
import { registerPageBuilder } from './page-builders.js';

/** @typedef {'platform' | 'tenant' | 'team' | 'person'} Level */

/** The write contract of each level this page can change (CT-306, CT-304). */
export const WRITE_CONTRACT = { person: 'setting-set-person', tenant: 'setting-set-tenant' };

/** Where the value a person reads comes from, in words. */
export const SOURCE_WORDS = {
  default: 'The default',
  platform: 'Set by the platform',
  tenant: 'Set by your organisation',
  team: 'Set for your team',
  person: 'Your choice',
};

/** The name of each level on its own control. */
const LEVEL_WORDS = { tenant: 'Organisation', person: 'Your choice' };

/** What a refused save says, by the code of its error (the contract's error names). */
const REFUSED = {
  level_not_allowed: 'Your organisation has set this, so it cannot be changed at this level.',
  wrong_type: 'That value is not one this setting takes.',
  invalid: 'That value is not one this setting allows.',
  conflict: 'It was changed somewhere else. The page shows the value it holds now: choose again.',
  forbidden: 'You are not allowed to change this setting.',
  not_found: 'This setting no longer exists.',
  unauthenticated: 'Your session has ended. Sign in again to save.',
};

/** What is said of an area that no product owns. */
const GENERAL = 'General';

/** The page's own state. `list` holds the definitions as they were last read, null before the first reply. */
export function settingsInit() {
  return {
    list: /** @type {any[] | null} */ (null),
    error: '',
    /** The setting id -> the level whose save is in flight. */
    busy: /** @type {Record<string, string>} */ ({}),
    /** The setting id -> the words of its last refused save. */
    refused: /** @type {Record<string, string>} */ ({}),
  };
}

function withSettings(state, patch) {
  return { ...state, settings: { ...state.settings, ...patch } };
}

/**
 * A value of the definitions reply as a plain value: `{"Str": "wide"}` is "wide", and a plain value
 * is itself. The reply sends the tagged form; a write takes the plain one.
 * @param {any} value
 * @returns {any}
 */
export function plainOf(value) {
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const keys = Object.keys(value);
    if (keys.length === 1) return value[keys[0]];
  }
  return value;
}

/**
 * The control a setting is edited with, by its type and its allowed values: a select for a closed
 * list, a number field for a range, a switch for a bool, a text field otherwise (a reference takes
 * the record's id in a text field, since the definitions reply names no records to choose from).
 * @param {any} entry
 * @returns {{ kind: 'select' | 'number' | 'switch' | 'text', options?: { value: any, label: string }[], min?: number, max?: number }}
 */
export function controlOf(entry) {
  const allowed = entry.allowed ?? null;
  if (entry.type === 'bool') return { kind: 'switch' };
  if (allowed?.closed) {
    return { kind: 'select', options: allowed.closed.values.map((v) => ({ value: plainOf(v), label: humanise(String(plainOf(v))) })) };
  }
  if (allowed?.range) return { kind: 'number', min: allowed.range.min, max: allowed.range.max };
  if (entry.type === 'int') return { kind: 'number' };
  return { kind: 'text' };
}

/**
 * What a setting's value reads as, in words.
 * @param {any} entry
 * @param {any} control
 */
export function valueWords(entry, control) {
  const value = plainOf(entry.value);
  if (control.kind === 'switch') return value === true ? 'On' : 'Off';
  if (value === null || value === undefined || value === '') return 'Not set';
  if (control.kind === 'select') {
    const option = control.options.find((o) => String(o.value) === String(value));
    return option ? option.label : String(value);
  }
  return String(value);
}

/**
 * A typed value from what the person entered. A refusal is said in words, before any request.
 * @param {any} entry
 * @param {any} control
 * @param {unknown} raw
 * @returns {{ ok: true, value: any } | { ok: false, words: string }}
 */
export function parseValue(entry, control, raw) {
  if (control.kind === 'switch') return { ok: true, value: raw === true || raw === 'true' || raw === 'on' };
  if (control.kind === 'number') {
    const text = String(raw ?? '').trim();
    const number = Number(text);
    if (text === '' || !Number.isInteger(number)) return { ok: false, words: 'Enter a whole number.' };
    if (control.min !== undefined && control.max !== undefined && (number < control.min || number > control.max)) {
      return { ok: false, words: `Enter a number from ${control.min} to ${control.max}.` };
    }
    return { ok: true, value: number };
  }
  if (control.kind === 'select') {
    const option = control.options.find((o) => String(o.value) === String(raw));
    return option ? { ok: true, value: option.value } : { ok: false, words: 'Choose one of the values listed.' };
  }
  const text = String(raw ?? '').trim();
  return text === '' ? { ok: false, words: 'Enter a value.' } : { ok: true, value: text };
}

/**
 * The words of a refused save: by the error's code, else its own message, else a plain line.
 * @param {any} error
 * @returns {string}
 */
export function refusalWords(error) {
  const code = String(error?.code ?? '');
  if (REFUSED[code]) return REFUSED[code];
  const message = typeof error?.message === 'string' ? error.message.trim() : '';
  return message || 'That did not work. Try again.';
}

/** The entry of a setting in the page's definitions, or null. @param {any} state @param {string} id */
function entryOf(state, id) {
  return state.settings.list?.find((entry) => entry.id === id) ?? null;
}

/**
 * Read the definitions when the page is shown, and again whenever its view is read again (a
 * setting changed, or a save was answered). Each reply is applied only when it is the latest.
 * @param {any} state @param {any} msg
 * @param {{ call: (state: any, kind: string, contract: string, inputs: Record<string, any>, msg: any, meta?: any) => [any, any] }} io
 */
export function settingsEnter(state, msg, io) {
  const [next, effect] = io.call(withSettings(state, { error: '' }), 'settings.definitions', 'setting-definitions', {}, msg);
  return { state: next, effects: [effect] };
}

/**
 * A save of one setting at one level: the form's name is `settings:<level>:<setting id>`, and its
 * value is the one the person entered. The revision the write names is the one the page read.
 * @param {any} state @param {any} msg
 * @param {{ call: (state: any, kind: string, contract: string, inputs: Record<string, any>, msg: any, meta?: any) => [any, any] }} io
 */
export function settingsSubmit(state, msg, io) {
  const [, level, ...rest] = String(msg.form).split(':');
  const id = rest.join(':');
  const entry = entryOf(state, id);
  if (!entry || !(level in WRITE_CONTRACT) || !(entry.writable_levels ?? []).includes(level)) {
    return { state, effects: [] };
  }
  if (state.settings.busy[id]) return { state, effects: [] };
  const control = controlOf(entry);
  const parsed = parseValue(entry, control, msg.values?.value);
  if (!parsed.ok) {
    return { state: withSettings(state, { refused: { ...state.settings.refused, [id]: parsed.words } }), effects: [] };
  }
  const expected = level === 'person' ? entry.person_revision : entry.tenant_revision;
  const cleared = { ...state.settings.refused };
  delete cleared[id];
  const pending = withSettings(state, { busy: { ...state.settings.busy, [id]: level }, refused: cleared });
  const [next, effect] = io.call(pending, 'settings.write', WRITE_CONTRACT[/** @type {'person' | 'tenant'} */ (level)], {
    setting: id,
    value: parsed.value,
    expected_version: typeof expected === 'number' ? expected : 0,
  }, msg, { setting: id, level });
  return { state: next, effects: [effect] };
}

/**
 * The answers of the page's requests: the definitions, and the save each answers. A save that is
 * answered, either way, reads the definitions again so the page shows what the site holds now.
 * @param {any} state
 * @param {any} msg the reply, with `pending` and `error` (null when it succeeded) and `outputs`
 * @param {{ call: (state: any, kind: string, contract: string, inputs: Record<string, any>, msg: any, meta?: any) => [any, any], toast: (state: any, kind: string, text: string) => any }} io
 */
export function settingsUpdate(state, msg, io) {
  if (msg.pending.kind === 'settings.definitions') {
    if (msg.error) return { state: withSettings(state, { error: refusalWords(msg.error) }), effects: [] };
    return { state: withSettings(state, { list: msg.outputs?.settings ?? [], error: '' }), effects: [] };
  }
  if (msg.pending.kind === 'settings.write') {
    const id = msg.pending.setting;
    const { [id]: _done, ...busy } = state.settings.busy;
    let next = withSettings(state, { busy });
    if (msg.error) {
      next = withSettings(next, { refused: { ...next.settings.refused, [id]: refusalWords(msg.error) } });
    } else {
      next = io.toast(next, 'ok', 'Saved.');
    }
    const [read, effect] = io.call(next, 'settings.definitions', 'setting-definitions', {}, msg);
    return { state: read, effects: [effect] };
  }
  return { state, effects: [] };
}

/** The page's model: the areas, each with its settings, in words the renderer draws as they are. */
export function settingsPage(state) {
  const page = state.settings;
  if (page.list === null) {
    return { kind: 'preferences', status: page.error ? 'error' : 'loading', error: page.error, areas: [] };
  }
  const areas = new Map();
  for (const entry of page.list) {
    const key = entry.area ?? null;
    if (!areas.has(key)) areas.set(key, { id: key ?? 'general', label: key ? humanise(key) : GENERAL, settings: [] });
    areas.get(key).settings.push(rowOf(entry, page));
  }
  const ordered = [...areas.values()].sort((a, b) => (a.id === 'general' ? 1 : b.id === 'general' ? -1 : a.label.localeCompare(b.label)));
  return { kind: 'preferences', status: 'ready', error: '', areas: ordered };
}

/**
 * One setting as the page draws it: its value and where it comes from, the words of a lock, and a
 * control for each level this person may change (a read-only setting has none).
 * @param {any} entry
 * @param {{ busy: Record<string, string>, refused: Record<string, string> }} page
 */
function rowOf(entry, page) {
  const control = controlOf(entry);
  const writable = (entry.writable_levels ?? []).filter((level) => level in WRITE_CONTRACT);
  const levels = writable.map((level) => ({
    level,
    label: LEVEL_WORDS[/** @type {'person' | 'tenant'} */ (level)],
    control,
    current: plainOf(entry.value),
    busy: page.busy[entry.id] === level,
  }));
  let readOnly = null;
  if (!levels.length) {
    readOnly = entry.locked ? 'Your organisation has locked this setting, so it cannot be changed here.' : 'Only your organisation can change this setting.';
  }
  return {
    id: entry.id,
    label: entry.label,
    description: entry.description ?? null,
    value: valueWords(entry, control),
    source: SOURCE_WORDS[/** @type {keyof typeof SOURCE_WORDS} */ (entry.source)] ?? '',
    locked: Boolean(entry.locked),
    readOnly,
    levels,
    refusal: page.refused[entry.id] ?? null,
  };
}

registerPageBuilder('preferences', (state) => settingsPage(state));
