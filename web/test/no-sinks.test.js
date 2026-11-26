// No HTML or script sink in the page's source (WG-5677). Every value a tenant or a record controls
// reaches the document through the renderer's DOM operations (createElement, createTextNode,
// setAttribute), so a source file that parses markup or runs a string as code would be a second way
// in. The scan reads the code of web/src with its comments removed, and fails on any sink it finds.
//
// The scan is a text search with a small tokenizer for comments and strings. It does not parse
// JavaScript, so it is a guard against a sink added by mistake, not a proof: the proof that the
// renderer emits no script is web/../lean/Strata/Client/Dom.lean.
//
// `srcdoc` is flagged only where a file sets it (as a property, or with setAttribute); the renderer's
// own denylist names it as a string, which is a check, not a sink.
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { join, relative } from 'node:path';

const SOURCE = fileURLToPath(new URL('../src/', import.meta.url));

/**
 * The source with its comments blanked out (line breaks kept). Strings, template literals and
 * regular expressions are kept as they are, so a sink written inside a string is still found.
 * @param {string} source
 */
export function stripComments(source) {
  let out = '';
  let i = 0;
  const n = source.length;
  while (i < n) {
    const c = source[i];
    const next = source[i + 1];
    if (c === '/' && next === '/') {
      while (i < n && source[i] !== '\n') {
        out += ' ';
        i += 1;
      }
    } else if (c === '/' && next === '*') {
      out += '  ';
      i += 2;
      while (i < n && !(source[i] === '*' && source[i + 1] === '/')) {
        out += source[i] === '\n' ? '\n' : ' ';
        i += 1;
      }
      out += '  ';
      i += 2;
    } else if (c === '"' || c === "'" || c === '`') {
      out += c;
      i += 1;
      while (i < n && source[i] !== c) {
        if (source[i] === '\\' && i + 1 < n) {
          out += source[i] + source[i + 1];
          i += 2;
          continue;
        }
        out += source[i];
        i += 1;
      }
      if (i < n) {
        out += source[i];
        i += 1;
      }
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/** The sinks the scan looks for, by name. */
const SINKS = [
  ['innerHTML', /\binnerHTML\b/],
  ['outerHTML', /\bouterHTML\b/],
  ['insertAdjacentHTML', /\binsertAdjacentHTML\b/],
  ['document.write', /\bdocument\s*\.\s*write(ln)?\b/],
  ['eval(', /(^|[^.\w$])eval\s*\(/],
  ['new Function', /\bnew\s+Function\b/],
  ['string timer', /\b(setTimeout|setInterval)\s*\(\s*['"`]/],
  ['javascript: literal', /javascript:/i],
  ['srcdoc', /\.\s*srcdoc\b|setAttribute\s*\(\s*['"`]srcdoc\b/],
  ['createContextualFragment', /\bcreateContextualFragment\b/],
  ['parseFromString', /\bparseFromString\b/],
];

/**
 * The sinks a source holds, with the line of each.
 * @param {string} source
 * @returns {{ line: number, sink: string, text: string }[]}
 */
export function findSinks(source) {
  const found = [];
  stripComments(source)
    .split('\n')
    .forEach((text, index) => {
      for (const [sink, pattern] of SINKS) {
        if (pattern.test(text)) found.push({ line: index + 1, sink, text: text.trim() });
      }
    });
  return found;
}

/** Every script or markup file under a directory. @param {string} dir @returns {string[]} */
function sourceFiles(dir) {
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...sourceFiles(path));
    else if (/\.(m?js|ts|jsx|tsx|html)$/.test(entry.name)) files.push(path);
  }
  return files;
}

test('no HTML or script sink appears in web/src', () => {
  const failures = [];
  for (const file of sourceFiles(SOURCE)) {
    for (const hit of findSinks(readFileSync(file, 'utf8'))) {
      failures.push(`${relative(SOURCE, file)}:${hit.line} ${hit.sink}: ${hit.text}`);
    }
  }
  assert.deepEqual(failures, []);
});

test('the scan catches each sink it is written for when one is planted', () => {
  const planted = [
    'el.innerHTML = value;',
    'node.outerHTML = "<b>x</b>";',
    'parent.insertAdjacentHTML("beforeend", s);',
    'document.write(s);',
    'eval(s);',
    'const f = new Function(s);',
    'setTimeout("alert(1)", 1);',
    "const u = 'javascript:alert(1)';",
    'frame.srcdoc = s;',
    'el.setAttribute("srcdoc", s);',
    'range.createContextualFragment(s);',
    'new DOMParser().parseFromString(s, "text/html");',
  ];
  for (const source of planted) {
    assert.ok(findSinks(source).length > 0, `not caught: ${source}`);
  }
});

test('a sink named in a comment is not a finding', () => {
  const source = '// el.innerHTML = value; eval(s);\n/* new Function(s) */\nconst ok = "http://example.test";\n';
  assert.deepEqual(findSinks(source), []);
});
