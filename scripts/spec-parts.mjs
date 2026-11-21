// The specification file as a manifest and parts (WG-5386), and the one way scripts read it.
//
// specifications/spec.jsonc lists part files:   { "specParts": ["intent.jsonc", "boundary.jsonc", ...] }
// Each part is a JSONC object holding some top-level sections of the specification. Resolving the
// manifest gives the single document a monolithic spec.jsonc would hold:
//
//   * array sections are concatenated in manifest order (`claims` may be in two parts);
//   * a section that is not an array (system, vocabulary, exegra, ...) may be in one part only;
//   * an entity id that two parts give the same array section is refused;
//   * a missing part, a part that is itself a manifest, and a manifest with other sections are refused.
//
// Every refusal names the files. The same rule is implemented by specc (crates/spec-parts), Exegra
// (tools/spec-manifest.js) and Concordance (concordance-exegra/src/parts.rs); keep them in step.
//
// The assembled text is canonical, so each implementation produces the same bytes (a text hash of it
// is the specification's revision): `{`, one member per line at two spaces, members in the order
// their key first appears, an array as `[`, its items one per line at four spaces (each item's
// own text unchanged, comments between items dropped), then `]`, a value that is not an array
// unchanged after `"key": `, and `}` with a final newline.
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { parseJsonc } from './jsonc.mjs';

export const MANIFEST_KEY = 'specParts';
/** The optional member of a manifest that says which part new sections and entities go to (Exegra's `apply` reads it; resolving ignores it). */
export const PLACEMENT_KEY = 'specSections';

export class SpecPartsError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'SpecPartsError';
    this.code = code;
  }
}

/**
 * The top-level members of a JSONC object text, with the offsets of each value and, for an array,
 * of each item. Comments and trailing commas are skipped, never copied.
 * @returns {{header: string, members: {key: string, start: number, end: number, items: {start: number, end: number}[] | null}[]}}
 */
export function scanTopLevel(text, what = 'the file') {
  const src = text.charCodeAt(0) === 0xfeff ? ` ${text.slice(1)}` : text;
  let i = 0;
  const fail = (message) => {
    const line = src.slice(0, i).split('\n').length;
    throw new SpecPartsError('syntax', `${what}: ${message} (line ${line})`);
  };
  const skip = () => {
    for (;;) {
      while (i < src.length && /\s/.test(src[i])) i += 1;
      if (src[i] === '/' && src[i + 1] === '/') {
        while (i < src.length && src[i] !== '\n') i += 1;
      } else if (src[i] === '/' && src[i + 1] === '*') {
        const end = src.indexOf('*/', i + 2);
        if (end < 0) fail('unterminated block comment');
        i = end + 2;
      } else return;
    }
  };
  const string = () => {
    const start = i;
    i += 1;
    while (i < src.length && src[i] !== '"') i += src[i] === '\\' ? 2 : 1;
    if (i >= src.length) fail('unterminated string');
    i += 1;
    return src.slice(start, i);
  };
  const value = () => {
    const c = src[i];
    if (c === '"') {
      string();
    } else if (c === '{' || c === '[') {
      let depth = 0;
      do {
        const d = src[i];
        if (d === '"') {
          string();
          continue;
        }
        if (d === '/' && (src[i + 1] === '/' || src[i + 1] === '*')) {
          skip();
          continue;
        }
        if (d === '{' || d === '[') depth += 1;
        else if (d === '}' || d === ']') depth -= 1;
        i += 1;
        if (i > src.length) fail('unterminated value');
      } while (depth > 0 && i < src.length);
      if (depth > 0) fail('unterminated value');
    } else {
      const start = i;
      while (i < src.length && !/[\s,}\]]/.test(src[i]) && !(src[i] === '/' && (src[i + 1] === '/' || src[i + 1] === '*'))) i += 1;
      if (i === start) fail('a value was expected');
    }
  };
  skip();
  const headerEnd = i;
  if (src[i] !== '{') fail('the top level is not an object');
  const header = src.slice(0, headerEnd);
  i += 1;
  const members = [];
  for (;;) {
    skip();
    if (src[i] === '}') break;
    if (src[i] !== '"') fail('a member name was expected');
    const key = JSON.parse(string());
    skip();
    if (src[i] !== ':') fail('a colon was expected');
    i += 1;
    skip();
    const start = i;
    /** @type {{start: number, end: number}[] | null} */
    let items = null;
    if (src[i] === '[') {
      items = [];
      i += 1;
      for (;;) {
        skip();
        if (src[i] === ']') {
          i += 1;
          break;
        }
        const s = i;
        value();
        items.push({ start: s, end: i });
        skip();
        if (src[i] === ',') i += 1;
      }
    } else {
      value();
    }
    members.push({ key, start, end: i, items });
    skip();
    if (src[i] === ',') i += 1;
    else if (src[i] !== '}') fail('a comma or the closing brace was expected');
  }
  return { header, members, src };
}

/** Whether a text is a manifest: its top level has the `specParts` key. */
export function isManifest(text) {
  if (!text.includes(MANIFEST_KEY)) return false;
  try {
    return scanTopLevel(text).members.some((m) => m.key === MANIFEST_KEY);
  } catch {
    return false;
  }
}

/**
 * The part file names a manifest lists, in order.
 * @returns {string[]}
 */
export function manifestParts(text, what = 'the manifest') {
  const top = scanTopLevel(text, what);
  const others = top.members.filter((m) => m.key !== MANIFEST_KEY && m.key !== PLACEMENT_KEY).map((m) => m.key);
  if (others.length) {
    throw new SpecPartsError('manifest-sections', `${what} is a manifest and lists parts, so it holds no section of its own; found ${others.map((k) => `"${k}"`).join(', ')}`);
  }
  const member = top.members.filter((m) => m.key === MANIFEST_KEY);
  if (member.length !== 1) throw new SpecPartsError('manifest-shape', `${what}: "${MANIFEST_KEY}" appears ${member.length} times`);
  let list;
  try {
    list = parseJsonc(top.src.slice(member[0].start, member[0].end));
  } catch (e) {
    throw new SpecPartsError('manifest-shape', `${what}: "${MANIFEST_KEY}" is not valid JSON: ${e.message}`);
  }
  if (!Array.isArray(list) || list.length === 0 || !list.every((p) => typeof p === 'string' && p.length > 0)) {
    throw new SpecPartsError('manifest-shape', `${what}: "${MANIFEST_KEY}" must be a non-empty list of part file names`);
  }
  const seen = new Set();
  for (const p of list) {
    if (seen.has(p)) throw new SpecPartsError('manifest-shape', `${what} lists the part ${p} twice`);
    seen.add(p);
  }
  return list;
}

/**
 * Assembles the parts. `parts` is [{file, text}] in manifest order; `file` is the name shown in errors.
 * @returns {{text: string, sections: {key: string, file: string}[]}}
 */
export function assembleParts(parts) {
  /** @type {Map<string, {array: boolean, items: string[], files: string[], ids: Map<string, string>}>} */
  const sections = new Map();
  const tops = new Map();
  for (const part of parts) {
    const top = scanTopLevel(part.text, part.file);
    tops.set(part, top);
    const seenHere = new Set();
    for (const m of top.members) {
      if (m.key === MANIFEST_KEY) throw new SpecPartsError('nested-manifest', `${part.file} is a manifest; a part holds sections, not a list of parts`);
      if (seenHere.has(m.key)) throw new SpecPartsError('duplicate-section', `${part.file} names the section "${m.key}" twice`);
      seenHere.add(m.key);
      const array = m.items !== null;
      let s = sections.get(m.key);
      if (s && (!array || !s.array)) {
        throw new SpecPartsError(
          'duplicate-section',
          `the section "${m.key}" is in both ${s.files[0]} and ${part.file}; only an array section may be in more than one part`,
        );
      }
      if (!s) {
        s = { array, items: [], files: [], ids: new Map() };
        sections.set(m.key, s);
      }
      s.files.push(part.file);
      if (!array) {
        s.items.push(top.src.slice(m.start, m.end));
        continue;
      }
      for (const item of m.items) {
        const itemText = top.src.slice(item.start, item.end);
        s.items.push(itemText);
      }
    }
  }
  // Duplicate ids, only where a section is in several parts (within one part the loader's own checks apply).
  for (const [key, s] of sections) {
    if (!s.array || s.files.length < 2) continue;
    const owner = new Map();
    for (const part of parts) {
      const top = tops.get(part);
      const m = top.members.find((x) => x.key === key);
      if (!m) continue;
      for (const item of m.items) {
        let id;
        try {
          id = parseJsonc(top.src.slice(item.start, item.end))?.id;
        } catch {
          id = undefined;
        }
        if (typeof id !== 'string') continue;
        const first = owner.get(id);
        if (first !== undefined && first !== part.file) {
          throw new SpecPartsError('duplicate-id', `the id "${id}" of section "${key}" is in both ${first} and ${part.file}`);
        }
        owner.set(id, part.file);
      }
    }
  }
  const out = [];
  const where = [];
  for (const [key, s] of sections) {
    where.push({ key, file: s.files[0] });
    if (s.array) {
      out.push(s.items.length ? `  ${JSON.stringify(key)}: [\n${s.items.map((t) => `    ${t}`).join(',\n')}\n  ]` : `  ${JSON.stringify(key)}: []`);
    } else {
      out.push(`  ${JSON.stringify(key)}: ${s.items[0]}`);
    }
  }
  return { text: `{\n${out.join(',\n')}\n}\n`, sections: where };
}

/**
 * Reads a specification file: a manifest is resolved to the assembled text, anything else is
 * returned as it is.
 * @returns {{text: string, files: string[], manifest: boolean}} `files` are the absolute paths read
 */
export function readSpec(path, read = (p) => readFileSync(p, 'utf8')) {
  const file = resolve(path);
  const text = read(file);
  if (!isManifest(text)) return { text, files: [file], manifest: false };
  const names = manifestParts(text, file);
  const dir = dirname(file);
  const parts = names.map((name) => {
    const abs = resolve(dir, name);
    if (!existsSync(abs)) throw new SpecPartsError('missing-part', `${file} lists the part ${name}, which does not exist (${abs})`);
    return { file: abs, text: read(abs) };
  });
  return { text: assembleParts(parts).text, files: [file, ...parts.map((p) => p.file)], manifest: true };
}

/** The specification as a parsed value. */
export function readSpecValue(path) {
  return parseJsonc(readSpec(path).text);
}

/** sha256 hex of the assembled specification text (the file's own bytes when it is not a manifest). */
export function specSha256(path) {
  return createHash('sha256').update(readSpec(path).text, 'utf8').digest('hex');
}
