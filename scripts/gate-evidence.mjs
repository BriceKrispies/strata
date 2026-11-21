// The requirement-to-evidence check of the specification gate.
//
// A requirement of a started milestone must have a passing proof. A proof is one of:
//   - a Rust `#[test]` whose doc comment holds a `Verifies: <REF>` line, and that the recorded
//     `cargo test` output (var/test-results/cargo-test.txt) shows as passed;
//   - a Lean theorem whose doc comment holds a `Verifies: <REF>` line, whose module the recorded
//     `lake build` output (var/test-results/lake-build.txt) shows as built, with no `sorry` and no error
//     inside the theorem. A theorem whose statement is `True` establishes nothing: it is reported and not counted;
//   - for a requirement realised by `construction`: the Exegra verdicts of the `m-` claims that model it,
//     all `verified` (var/exegra-verdicts.json, which the gate writes from `exegra claims`).
//
// A requirement may declare the check that holds it (`check`, `checkedBy`; specifications/README.md, "What holds a
// requirement"). The declaration is read against the code: a test it names must exist and carry a `Verifies:` line
// for the requirement, an m- claim it names must model the requirement, a contract it names must exist. A statement
// declared `documentation` is outside the counts.
//
// The check fails when a requirement of a started milestone has no passing proof, when a declared check does not
// stand, and when a `Verifies:` line names a ref the specification does not have. A requirement of a milestone that is not started is
// counted as planned and is not held to the code.
//
// Only the pure functions below read nothing from disk; `collectEvidence` and the command line do.
//
//   node scripts/gate-evidence.mjs [--root <dir>] [--results-dir <dir>] [--verdicts <file>] [--spec <file>] [--config <file>]
//
// Staleness is not decided here. The gate record binds the recorded results to the specification and to
// every source file by content hash (scripts/spec-gate.mjs), so results recorded before a change are refused.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readSpecValue } from './spec-parts.mjs';

// ---------------------------------------------------------------------------------------------
// Refs and `Verifies:` lines
// ---------------------------------------------------------------------------------------------

/** Whether `s` has the shape of a requirement ref: `KRN-001`, `EML-1562`, `MIG-602`, `A-B-12.3`. */
export function isRequirementRef(s) {
  const parts = s.split('-');
  if (parts.length < 2 || s.length > 40) return false;
  const last = parts.pop();
  const dot = last.indexOf('.');
  const num = dot < 0 ? last : last.slice(0, dot);
  const sub = dot < 0 ? undefined : last.slice(dot + 1);
  const digits = (x) => /^[0-9]{1,4}$/.test(x);
  const tag = /^[A-Z]*/.exec(num)[0].length;
  if (tag > 2 || !digits(num.slice(tag)) || (sub !== undefined && !digits(sub))) return false;
  return /^[A-Z]/.test(parts[0]) && parts.every((p) => /^[A-Z0-9]{1,10}$/.test(p));
}

/** The refs named by `Verifies: REF, REF` lines of a doc comment text, upper-cased, in order, once each. */
export function verifiesFromDoc(doc) {
  const out = [];
  for (const raw of doc.split('\n')) {
    const line = raw.trim().replace(/^[*-]+\s*/, '');
    const m = /^verifies(?::|\s)(.*)$/i.exec(line);
    if (!m) continue;
    for (const token of m[1].split(/[\s,;]+/)) {
      let t = token.replace(/^[`*[\]():]+|[`*[\]():]+$/g, '');
      if (t.endsWith('.')) t = t.slice(0, -1);
      t = t.toUpperCase();
      if (t && isRequirementRef(t) && !out.includes(t)) out.push(t);
    }
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Rust: the `#[test]` functions that carry a `Verifies:` line
// ---------------------------------------------------------------------------------------------

function rustTokens(text) {
  const toks = [];
  let i = 0;
  let line = 1;
  const n = text.length;
  while (i < n) {
    const c = text[i];
    if (c === '\n') {
      line += 1;
      i += 1;
    } else if (/\s/.test(c)) {
      i += 1;
    } else if (c === '/' && text[i + 1] === '/') {
      let j = i;
      while (j < n && text[j] !== '\n') j += 1;
      const body = text.slice(i, j);
      // a doc comment, or a plain `//` comment above the test: the crates use both for `Verifies:`
      if (body.startsWith('///') && !body.startsWith('////')) toks.push({ t: 'doc', v: body.slice(3), line });
      else if (!body.startsWith('//!')) toks.push({ t: 'doc', v: body.slice(2), line });
      i = j;
    } else if (c === '/' && text[i + 1] === '*') {
      const isDoc = text[i + 2] === '*' && text[i + 3] !== '*' && text[i + 3] !== '/';
      const startLine = line;
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (text[j] === '/' && text[j + 1] === '*') {
          depth += 1;
          j += 2;
        } else if (text[j] === '*' && text[j + 1] === '/') {
          depth -= 1;
          j += 2;
        } else {
          if (text[j] === '\n') line += 1;
          j += 1;
        }
      }
      if (isDoc) toks.push({ t: 'doc', v: text.slice(i + 3, j - 2), line: startLine });
      i = j;
    } else if (c === '"' || ((c === 'r' || c === 'b') && /^(?:b?r#*"|b")/.test(text.slice(i, i + 12)))) {
      // a string, a byte string or a raw string: skipped
      const raw = /^b?r(#*)"/.exec(text.slice(i, i + 12));
      let j;
      if (raw) {
        const close = `"${raw[1]}`;
        j = text.indexOf(close, i + raw[0].length);
        j = j < 0 ? n : j + close.length;
      } else {
        j = i + (c === 'b' ? 2 : 1);
        while (j < n && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
        j += 1;
      }
      for (let k = i; k < Math.min(j, n); k += 1) if (text[k] === '\n') line += 1;
      toks.push({ t: 'str', v: '', line });
      i = j;
    } else if (c === "'") {
      // a char literal, or a lifetime
      if (text[i + 1] === '\\') {
        let j = i + 2;
        while (j < n && text[j] !== "'") j += 1;
        i = j + 1;
      } else if (text[i + 2] === "'") {
        i += 3;
      } else {
        i += 1;
      }
    } else if (/[A-Za-z_]/.test(c)) {
      let j = i + 1;
      while (j < n && /[A-Za-z0-9_]/.test(text[j])) j += 1;
      toks.push({ t: 'id', v: text.slice(i, j), line });
      i = j;
    } else {
      toks.push({ t: 'p', v: c, line });
      i += 1;
    }
  }
  return toks;
}

/**
 * Where a Rust file's tests run: the variants `{prefix, binaries}` it can be compiled as, that is the module path
 * its test names start with and the binaries that can report them. A file of tests/ is a binary of its own and
 * may also be mounted as a module of another binary (`mod x;` or `#[path = "x.rs"] mod x;` in tests/y.rs),
 * which is how the crates gather their integration tests (`crateInfo[dir].mounts`).
 */
export function rustFileScope(path, crateInfo = {}) {
  const m = /^crates\/([^/]+)\/(src|tests)\/(.+)\.rs$/.exec(path);
  if (!m) return [];
  const mounts = (crateInfo[m[1]]?.mounts ?? {})[`${m[2]}/${m[3]}.rs`] ?? [];
  return [rustOwnScope(m, crateInfo), ...mounts.map((x) => ({ prefix: [x.mod], binaries: [x.binary] }))];
}

function rustOwnScope(m, crateInfo) {
  const [, dir, kind, rest] = m;
  const pkg = (crateInfo[dir]?.name ?? dir).replace(/-/g, '_');
  const lib = (crateInfo[dir]?.lib ?? pkg).replace(/-/g, '_');
  const segs = rest.split('/');
  if (kind === 'tests') {
    if (segs.length === 1) return { prefix: [], binaries: [segs[0]] };
    const [stem, ...more] = segs;
    if (more.length === 1 && more[0] === 'main') return { prefix: [], binaries: [stem] };
    return { prefix: more[more.length - 1] === 'mod' ? more.slice(0, -1) : more, binaries: [stem] };
  }
  if (segs[0] === 'bin') {
    if (segs.length === 2) return { prefix: [], binaries: [segs[1]] };
    const [, stem, ...more] = segs;
    return { prefix: more.length === 1 && more[0] === 'main' ? [] : more, binaries: [stem] };
  }
  const prefix = segs[segs.length - 1] === 'mod' ? segs.slice(0, -1) : segs;
  if (prefix.length === 1 && (prefix[0] === 'lib' || prefix[0] === 'main')) return { prefix: [], binaries: [...new Set([pkg, lib])] };
  return { prefix, binaries: [...new Set([pkg, lib])] };
}

/**
 * The `#[test]` functions of one Rust file that have a `Verifies:` doc line:
 * `{file, line, name, binaries, refs}`. `name` is the path cargo prints for the test.
 */
export function indexRust(path, text, crateInfo = {}) {
  const variants = rustFileScope(path, crateInfo);
  const toks = rustTokens(text);
  const out = [];
  const stack = [];
  let docs = [];
  let isTest = false;
  let pendingMod = null;
  for (let i = 0; i < toks.length; i += 1) {
    const tk = toks[i];
    if (tk.t === 'doc') {
      docs.push(tk.v.replace(/^ /, ''));
    } else if (tk.v === '#' && (toks[i + 1]?.v === '[' || (toks[i + 1]?.v === '!' && toks[i + 2]?.v === '['))) {
      const inner = toks[i + 1].v === '!';
      let j = i + (inner ? 3 : 2);
      const start = j;
      let depth = 1;
      while (j < toks.length && depth > 0) {
        if (toks[j].v === '[') depth += 1;
        else if (toks[j].v === ']') depth -= 1;
        j += 1;
      }
      if (!inner) {
        const path = [];
        for (let k = start; k < j - 1; k += 1) {
          if (toks[k].t === 'id') path.push(toks[k].v);
          else if (toks[k].v !== ':') break;
        }
        if (['test', 'rstest'].includes(path[path.length - 1])) isTest = true;
      }
      i = j - 1;
    } else if (tk.t === 'id' && tk.v === 'mod' && toks[i + 1]?.t === 'id' && toks[i + 2]?.v === '{') {
      pendingMod = toks[i + 1].v;
    } else if (tk.t === 'id' && tk.v === 'fn' && toks[i + 1]?.t === 'id') {
      if (isTest) {
        const refs = verifiesFromDoc(docs.join('\n'));
        if (refs.length > 0) {
          const inner = [...stack.filter((s) => s !== null), toks[i + 1].v];
          const names = variants.map((v) => ({ name: [...v.prefix, ...inner].join('::'), binaries: v.binaries }));
          out.push({ file: path, line: tk.line, name: names[0]?.name ?? inner.join('::'), names, refs });
        }
      }
      docs = [];
      isTest = false;
    } else if (tk.v === '{') {
      stack.push(pendingMod);
      pendingMod = null;
      docs = [];
      isTest = false;
    } else if (tk.v === '}') {
      stack.pop();
      docs = [];
      isTest = false;
    } else if (tk.v === ';') {
      docs = [];
      isTest = false;
    }
  }
  return out;
}

const RANK = { passed: 0, ignored: 1, failed: 2 };

/**
 * The outcomes in the output of `cargo test`: `{name, outcome, binary}` for each `test NAME ... ok|FAILED|ignored`.
 * The recorded text is stdout followed by stderr, so the `Running ... (target\debug\deps\x-hash.exe)` headers are
 * at the end; the n-th `running N tests` block of stdout belongs to the n-th header. When the counts differ the
 * binary is unknown (`null`) and a result is matched by its name alone.
 */
export function parseCargo(text) {
  const lines = text.split('\n').map((l) => l.replace(/\r$/, ''));
  const headers = [];
  for (const l of lines) {
    const run = /^\s+Running (?:unittests )?(.*?) \((.*)\)\s*$/.exec(l);
    if (run) {
      const exe = run[2].split(/[\\/]/).pop().replace(/\.exe$/, '').replace(/-[0-9a-f]{16}$/, '');
      headers.push(exe);
      continue;
    }
    if (/^\s+Doc-tests /.test(l)) headers.push('doc-tests');
  }
  const blocks = lines.reduce((n, l) => n + (/^running \d+ tests?$/.test(l) ? 1 : 0), 0);
  const known = blocks > 0 && blocks === headers.length;
  const results = [];
  const outcome = (word) => (word === 'ok' ? 'passed' : word === 'FAILED' ? 'failed' : 'ignored');
  let block = -1;
  // A test that prints while it runs puts its output between `test NAME ... ` and the result, so the
  // result then stands at the start of a later line. The pending names wait for it, in order.
  const pending = [];
  for (const l of lines) {
    if (/^running \d+ tests?$/.test(l)) {
      block += 1;
      pending.length = 0;
      continue;
    }
    const m = /^test (\S+) \.\.\. (.*)$/.exec(l);
    if (m) {
      const done = /^(ok|FAILED|ignored)\b/.exec(m[2]);
      if (done) results.push({ name: m[1], outcome: outcome(done[1]), binary: known ? headers[block] ?? null : null });
      else pending.push({ name: m[1], binary: known ? headers[block] ?? null : null });
      continue;
    }
    const late = /^(ok|FAILED|ignored)\b/.exec(l);
    if (late && pending.length) results.push({ ...pending.shift(), outcome: outcome(late[1]) });
  }
  return results;
}

/** The outcome of a Rust test site against parsed cargo results: 'passed', 'failed', 'ignored' or null (no result). */
export function rustOutcome(site, results) {
  let worst = null;
  for (const r of results) {
    if (!site.names.some((v) => v.name === r.name && (r.binary === null || v.binaries.includes(r.binary)))) continue;
    if (worst === null || RANK[r.outcome] > RANK[worst]) worst = r.outcome;
  }
  return worst;
}

// ---------------------------------------------------------------------------------------------
// Lean: theorems that carry a `Verifies:` line
// ---------------------------------------------------------------------------------------------

function leanLex(text) {
  const toks = [];
  const comments = [];
  let i = 0;
  let line = 1;
  let col = 0;
  let lineStart = true;
  const n = text.length;
  const advance = (k) => {
    for (let j = 0; j < k; j += 1) {
      if (text[i + j] === '\n') {
        line += 1;
        col = 0;
        lineStart = true;
      } else {
        col += 1;
      }
    }
    i += k;
  };
  while (i < n) {
    const c = text[i];
    if (c === '\n' || c === ' ' || c === '\t' || c === '\r') {
      advance(1);
    } else if (c === '-' && text[i + 1] === '-') {
      let j = i;
      while (j < n && text[j] !== '\n') j += 1;
      advance(j - i);
    } else if (c === '/' && text[i + 1] === '-') {
      const startLine = line;
      const own = lineStart;
      const kind = text[i + 2] === '-' ? 'doc' : text[i + 2] === '!' ? 'module' : 'block';
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (text[j] === '/' && text[j + 1] === '-') {
          depth += 1;
          j += 2;
        } else if (text[j] === '-' && text[j + 1] === '/') {
          depth -= 1;
          j += 2;
        } else {
          j += 1;
        }
      }
      const body = text.slice(i, j);
      advance(j - i);
      comments.push({ kind, own, startLine, endLine: line, text: body.replace(/^\/-[-!]?/, '').replace(/-\/$/, '') });
    } else if (c === '"') {
      let j = i + 1;
      while (j < n && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      toks.push({ t: 's', v: '"', line, col, first: lineStart });
      lineStart = false;
      advance(Math.min(j + 1, n) - i);
    } else if (/[\p{L}_]/u.test(c)) {
      let j = i + 1;
      while (j < n && /[\p{L}\p{N}_'.!?]/u.test(text[j])) j += 1;
      const word = text.slice(i, j).replace(/\.+$/, '');
      toks.push({ t: 'id', v: word, line, col, first: lineStart });
      lineStart = false;
      advance(word.length);
    } else {
      toks.push({ t: 'p', v: c, line, col, first: lineStart });
      lineStart = false;
      advance(1);
    }
  }
  return { toks, comments };
}

const LEAN_MODIFIERS = new Set(['private', 'protected', 'noncomputable', 'nonrec', 'partial', 'unsafe']);

/**
 * The theorems and lemmas of one Lean file that have a `Verifies:` doc comment:
 * `{file, module, name, startLine, endLine, refs, trivial, sorry}`. `module` is the dotted path of the file under `leanRoot`.
 */
export function indexLean(path, text, leanRoot = 'lean') {
  const rel = path.startsWith(`${leanRoot}/`) ? path.slice(leanRoot.length + 1) : path;
  const module = rel.replace(/\.lean$/, '').split('/').join('.');
  const { toks, comments } = leanLex(text);
  const out = [];
  for (let i = 0; i < toks.length; i += 1) {
    const tk = toks[i];
    if (tk.t !== 'id' || (tk.v !== 'theorem' && tk.v !== 'lemma')) continue;
    // walk back over modifiers and attributes to the first token of the command
    let first = i;
    for (;;) {
      const prev = toks[first - 1];
      if (prev && prev.t === 'id' && LEAN_MODIFIERS.has(prev.v)) {
        first -= 1;
      } else if (prev && prev.v === ']') {
        let k = first - 1;
        let depth = 0;
        for (; k >= 0; k -= 1) {
          if (toks[k].v === ']') depth += 1;
          else if (toks[k].v === '[') {
            depth -= 1;
            if (depth === 0) break;
          }
        }
        if (k > 0 && toks[k - 1].v === '@') first = k - 1;
        else break;
      } else {
        break;
      }
    }
    if (!toks[first].first) continue;
    // the doc comment directly above, through adjacent own-line comments
    const parts = [];
    let want = toks[first].line;
    const prevEnd = first > 0 ? toks[first - 1].line : 0;
    for (let k = comments.length - 1; k >= 0; k -= 1) {
      const cm = comments[k];
      if (cm.startLine >= want || cm.startLine < prevEnd) continue;
      if (!cm.own || cm.kind === 'module' || cm.endLine + 1 < want) break;
      parts.unshift(cm.text);
      want = cm.startLine;
    }
    const refs = verifiesFromDoc(parts.join('\n'));
    if (refs.length === 0) continue;
    // the command runs to the next token in column 0
    let end = i + 1;
    while (end < toks.length && !(toks[end].first && toks[end].col === 0)) end += 1;
    const endLine = end < toks.length ? toks[end].line - 1 : text.split('\n').length;
    const name = toks[i + 1]?.t === 'id' ? toks[i + 1].v : '';
    // the statement: after the first `:` outside brackets, up to `:=`, `where` or `|` outside brackets
    let depth = 0;
    let statement = null;
    for (let k = i + 2; k < end; k += 1) {
      const v = toks[k].v;
      if ('([{⟨'.includes(v)) depth += 1;
      else if (')]}⟩'.includes(v)) depth -= 1;
      else if (depth === 0 && statement === null && v === ':' && toks[k + 1]?.v !== '=') statement = [];
      else if (depth === 0 && statement !== null && ((v === ':' && toks[k + 1]?.v === '=') || v === 'where' || v === '|')) break;
      else if (statement !== null) statement.push(v);
    }
    const trivial = statement !== null && statement.length === 1 && statement[0] === 'True';
    const sorry = toks.slice(i, end).some((t) => t.t === 'id' && t.v === 'sorry');
    out.push({ file: path, module, name, startLine: toks[first].line, endLine, refs, trivial, sorry });
  }
  return out;
}

/**
 * What `lake build` reported: the modules built (or replayed from its cache), the modules that failed,
 * and the `[file, line]` of each error and each `declaration uses 'sorry'` warning.
 */
export function parseLake(text) {
  const out = { built: new Set(), failed: new Set(), errors: [], sorries: [], completed: false };
  const norm = (p) => p.trim().replace(/\\/g, '/').replace(/^\.\//, '');
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (line.includes('Build completed successfully')) out.completed = true;
    const prefixed = /^(error|warning): (.+?\.lean):(\d+):\d+:?\s*(.*)$/.exec(line);
    if (prefixed) {
      const [, level, file, ln, msg] = prefixed;
      if (level === 'error') out.errors.push([norm(file), Number(ln)]);
      else if (/declaration uses ['`]sorry['`]/.test(msg)) out.sorries.push([norm(file), Number(ln)]);
      continue;
    }
    const direct = /^(\.\/)?(\S+?\.lean):(\d+):\d+:\s*(.*)$/.exec(line);
    if (direct) {
      const [, , file, ln, msg] = direct;
      if (msg.startsWith('error')) out.errors.push([norm(file), Number(ln)]);
      else if (/declaration uses ['`]sorry['`]/.test(msg)) out.sorries.push([norm(file), Number(ln)]);
      continue;
    }
    const mark = line[0];
    const rest = /^[\u2714\u26a0\u2716\u2139]/.test(mark) ? line.slice(1).trim() : line;
    const progress = /^(?:\[\d+\/\d+\]\s+)?(Built|Building|Compiling|Replayed)\s+(\S+)/.exec(rest);
    if (!progress) continue;
    const module = progress[2].split(':')[0].replace(/[«»]/g, '');
    if (!/^[\p{L}«]/u.test(module)) continue;
    if (mark === '\u2716') out.failed.add(module);
    else if (progress[1] === 'Built' || progress[1] === 'Replayed') out.built.add(module);
  }
  return out;
}

const sameFile = (rel, file) => rel === file || rel.endsWith(`/${file}`) || file.endsWith(`/${rel}`);

/** The outcome of a Lean theorem against a parsed lake build: 'passed', 'failed' or null (the log says nothing of it). */
export function leanOutcome(site, lake, leanRoot = 'lean') {
  const rel = site.file.startsWith(`${leanRoot}/`) ? site.file.slice(leanRoot.length + 1) : site.file;
  const inside = (list) => list.some(([f, l]) => sameFile(rel, f) && l >= site.startLine && l <= site.endLine);
  if (site.sorry || inside(lake.sorries) || inside(lake.errors)) return 'failed';
  if (lake.failed.has(site.module) || lake.errors.some(([f]) => sameFile(rel, f))) return 'failed';
  return lake.built.has(site.module) ? 'passed' : null;
}

// ---------------------------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------------------------

/**
 * Judge every requirement.
 *   requirements  claims of kind requirement: `{id, ref, milestone, realisation, check, checkedBy}`; those with check `documentation` are left out of every count
 *   knownRefs     every ref the specification defines (contracts too), upper-case; defaults to the requirements' refs.
 *                 A `Verifies:` line naming a ref outside it is reported; one naming a known ref that is not a
 *                 requirement (a contract) is accepted and proves nothing.
 *   modelClaims   the `m-` claims: `{id, models}`
 *   verdicts      Exegra claim verdicts: `{claimId, status}[]`, or null when there are none
 *   rustTests     `indexRust` results, with `rustResults` from `parseCargo`
 *   leanTheorems  `indexLean` results, with `lake` from `parseLake`
 *   started       the started milestone numbers
 * Returns `{ok, proven, unproven, unknownRefs, checkProblems, trivial, counts}`. `unproven` lists the started requirements with no passing proof.
 */
export function evaluateEvidence({ requirements: allRequirements, knownRefs = null, modelClaims = [], verdicts = null, rustTests = [], rustResults = [], leanTheorems = [], lake = null, started }) {
  const startedSet = new Set(started);
  let requirements = allRequirements;
  const byRef = new Map();
  for (const r of requirements) if (r.ref) byRef.set(String(r.ref).toUpperCase(), r);

  // the proofs of each ref
  const proofs = new Map(); // REF -> [{kind, where, outcome}]
  const unknownRefs = [];
  const trivial = [];
  const known = knownRefs ? new Set(knownRefs) : new Set(byRef.keys());
  const add = (ref, proof) => {
    if (!known.has(ref)) {
      unknownRefs.push({ ref, where: proof.where });
      return;
    }
    if (!byRef.has(ref)) return;
    if (!proofs.has(ref)) proofs.set(ref, []);
    proofs.get(ref).push(proof);
  };
  const sites = { rust: rustTests.length, rustPassed: 0, lean: leanTheorems.length, leanPassed: 0 };
  for (const site of rustTests) {
    const outcome = rustOutcome(site, rustResults);
    if (outcome === 'passed') sites.rustPassed += 1;
    for (const ref of site.refs) add(ref, { kind: 'rust', where: `${site.file}:${site.line} ${site.name}`, outcome: outcome ?? 'no result' });
  }
  for (const site of leanTheorems) {
    const outcome = lake ? leanOutcome(site, lake) : null;
    const where = `${site.file}:${site.startLine} ${site.name}`;
    if (site.trivial) trivial.push(where);
    else if (outcome === 'passed') sites.leanPassed += 1;
    for (const ref of site.refs) add(ref, { kind: 'lean', where, outcome: site.trivial ? 'trivial' : outcome ?? 'no result' });
  }

  const verdictOf = new Map((verdicts ?? []).map((v) => [v.claimId, v.status]));
  const modellers = (r) => modelClaims.filter((c) => c.models === r.id || (r.ref && String(c.models).toUpperCase() === String(r.ref).toUpperCase()));

  // a declared check must exist and name the requirement back
  const checkProblems = [];
  for (const r of requirements) {
    if (!r.check || r.check === 'documentation') continue;
    const ref = String(r.ref ?? '').toUpperCase();
    const where = r.ref ?? r.id;
    for (const name of r.checkedBy ?? []) {
      if (r.check === 'test' || r.check === 'differential') {
        const at = name.indexOf('::');
        const file = name.slice(0, at);
        const fn = name.slice(at + 2);
        const rust = rustTests.find((s) => s.file === file && (s.name === fn || (s.names ?? []).some((n) => n.name === fn)));
        const lean = leanTheorems.find((s) => s.file === file && s.name === fn);
        const site = rust ?? lean;
        if (!site) checkProblems.push(`${where}: ${r.check} ${name} is not a test or theorem with a Verifies: line`);
        else if (!site.refs.includes(ref)) checkProblems.push(`${where}: ${name} carries no Verifies: line for ${where}`);
      } else if (r.check === 'model-claim') {
        if (!modelClaims.some((c) => c.id === name && (c.models === r.id || String(c.models).toUpperCase() === ref))) checkProblems.push(`${where}: ${name} is not an m- claim that models it`);
      } else if (r.check === 'conformance') {
        if (!known.has(name.toUpperCase())) checkProblems.push(`${where}: ${name} is not a contract of the specification`);
      }
    }
  }

  const documentation = requirements.filter((r) => r.check === 'documentation').length;
  requirements = requirements.filter((r) => r.check !== 'documentation');
  const proven = [];
  const unproven = [];
  const judge = (r) => {
    if (r.realisation === 'construction') {
      const claims = modellers(r);
      if (claims.length === 0) return { ok: false, reason: 'construction requirement modelled by no m- claim' };
      const bad = claims.filter((c) => verdictOf.get(c.id) !== 'verified');
      if (bad.length > 0) return { ok: false, reason: `Exegra: ${bad.map((c) => `${c.id} ${verdictOf.get(c.id) ?? 'has no verdict'}`).join(', ')}` };
      return { ok: true, by: `Exegra ${claims.map((c) => c.id).join(', ')}` };
    }
    if (!r.ref) return { ok: false, reason: 'no ref, so no test can name it' };
    const list = proofs.get(String(r.ref).toUpperCase()) ?? [];
    if (list.length === 0) return { ok: false, reason: 'no test or theorem names it with a Verifies: line' };
    const failed = list.filter((p) => p.outcome === 'failed');
    if (failed.length > 0) return { ok: false, reason: `failed: ${failed.map((p) => p.where).join(', ')}` };
    const passed = list.filter((p) => p.outcome === 'passed');
    if (passed.length === 0) return { ok: false, reason: `named by ${list.map((p) => `${p.where} (${p.outcome})`).join(', ')}, none passing` };
    return { ok: true, by: passed.map((p) => p.where).join(', ') };
  };
  let planned = 0;
  let startedCount = 0;
  let startedProven = 0;
  for (const r of requirements) {
    const isStarted = startedSet.has(r.milestone);
    if (!isStarted) planned += 1;
    else startedCount += 1;
    const verdict = judge(r);
    if (verdict.ok) {
      proven.push({ id: r.id, ref: r.ref, milestone: r.milestone, by: verdict.by });
      if (isStarted) startedProven += 1;
    } else if (isStarted) {
      unproven.push({ id: r.id, ref: r.ref, milestone: r.milestone, reason: verdict.reason });
    }
  }
  const counts = { requirements: requirements.length, documentation, proven: proven.length, started: startedCount, startedProven, planned, sites };
  return { ok: unproven.length === 0 && unknownRefs.length === 0 && checkProblems.length === 0, proven, unproven, unknownRefs, checkProblems, trivial, counts };
}

/** The report as lines, for the gate's output: the counts first, then each problem. */
export function reportLines(report, limit = 30) {
  const { counts } = report;
  const lines = [
    `${counts.startedProven} of ${counts.started} started requirements proven; ${counts.proven} proven in all; ${counts.planned} planned (not held to the code)`,
    `proofs indexed: ${counts.sites.rustPassed} of ${counts.sites.rust} Rust tests passed, ${counts.sites.leanPassed} of ${counts.sites.lean} Lean theorems built`,
  ];
  for (const u of report.unproven.slice(0, limit)) lines.push(`unproven: ${u.ref ?? u.id} (${u.id}, milestone ${u.milestone}): ${u.reason}`);
  if (report.unproven.length > limit) lines.push(`... and ${report.unproven.length - limit} more unproven`);
  for (const u of report.unknownRefs.slice(0, limit)) lines.push(`unknown ref: ${u.ref} in ${u.where}`);
  if (report.unknownRefs.length > limit) lines.push(`... and ${report.unknownRefs.length - limit} more unknown refs`);
  for (const p of (report.checkProblems ?? []).slice(0, limit)) lines.push(`declared check does not stand: ${p}`);
  for (const t of report.trivial) lines.push(`not counted: theorem states True: ${t}`);
  return lines;
}

// ---------------------------------------------------------------------------------------------
// Reading the repository
// ---------------------------------------------------------------------------------------------

function walk(dir, files, skip) {
  for (const entry of readdirSync(dir)) {
    if (skip.has(entry)) continue;
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) walk(path, files, skip);
    else files.push(path);
  }
}

const rel = (root, path) => path.slice(root.length + 1).split('\\').join('/');

/** The package and library name of each crate under crates/, from its Cargo.toml. */
function crateInfoOf(root) {
  const info = {};
  const dir = join(root, 'crates');
  if (!existsSync(dir)) return info;
  for (const name of readdirSync(dir)) {
    const manifest = join(dir, name, 'Cargo.toml');
    if (!existsSync(manifest)) continue;
    const text = readFileSync(manifest, 'utf8');
    const pkg = /^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms.exec(text)?.[1];
    const lib = /^\[lib\][^[]*?^name\s*=\s*"([^"]+)"/ms.exec(text)?.[1];
    const mounts = {};
    const tests = join(dir, name, 'tests');
    if (existsSync(tests)) {
      for (const file of readdirSync(tests).filter((f) => f.endsWith('.rs'))) {
        const stem = file.slice(0, -3);
        for (const m of readFileSync(join(tests, file), 'utf8').matchAll(/(?:#\[path\s*=\s*"([^"]+)"\]\s*)?(?:pub\s+)?mod\s+(\w+)\s*;/g)) {
          const target = m[1] ?? `${m[2]}.rs`;
          if (target === file || target.includes('/')) continue;
          (mounts[`tests/${target}`] ??= []).push({ binary: stem, mod: m[2] });
        }
      }
    }
    info[name] = { name: pkg ?? name, lib, mounts };
  }
  return info;
}

/** Index the repository: the Rust tests and Lean theorems that carry a `Verifies:` line. */
export function collectEvidence(root) {
  const skip = new Set(['target', 'node_modules', '.lake', '.git', 'fixtures']);
  const crateInfo = crateInfoOf(root);
  const rustTests = [];
  const leanTheorems = [];
  const rustFiles = [];
  if (existsSync(join(root, 'crates'))) walk(join(root, 'crates'), rustFiles, skip);
  for (const file of rustFiles.filter((f) => f.endsWith('.rs'))) {
    rustTests.push(...indexRust(rel(root, file), readFileSync(file, 'utf8'), crateInfo));
  }
  const leanFiles = [];
  if (existsSync(join(root, 'lean'))) walk(join(root, 'lean'), leanFiles, skip);
  for (const file of leanFiles.filter((f) => f.endsWith('.lean'))) {
    leanTheorems.push(...indexLean(rel(root, file), readFileSync(file, 'utf8')));
  }
  return { rustTests, leanTheorems };
}

/**
 * The whole check on a checkout. `spec` is the parsed specification, `resultsDir` holds cargo-test.txt and
 * lake-build.txt, `verdicts` is the parsed `exegra claims` list (or null).
 */
export function runEvidence({ root, spec, resultsDir, verdicts, started }) {
  const read = (name) => (existsSync(join(resultsDir, name)) ? readFileSync(join(resultsDir, name), 'utf8') : null);
  const cargoText = read('cargo-test.txt');
  const lakeText = read('lake-build.txt');
  const claims = spec.claims ?? [];
  const evidence = collectEvidence(root);
  return evaluateEvidence({
    requirements: claims.filter((c) => c.kind === 'requirement'),
    knownRefs: collectRefs(spec),
    modelClaims: claims.filter((c) => String(c.id).startsWith('m-')),
    verdicts,
    rustTests: evidence.rustTests,
    rustResults: cargoText === null ? [] : parseCargo(cargoText),
    leanTheorems: evidence.leanTheorems,
    lake: lakeText === null ? null : parseLake(lakeText),
    started,
  });
}

/** Every `ref` string anywhere in the specification, upper-cased: the requirements' and the contracts'. */
export function collectRefs(spec) {
  const refs = new Set();
  const visit = (value) => {
    if (Array.isArray(value)) value.forEach(visit);
    else if (value && typeof value === 'object') {
      for (const [key, v] of Object.entries(value)) {
        if (key === 'ref' && typeof v === 'string') refs.add(v.toUpperCase());
        else visit(v);
      }
    }
  };
  visit(spec);
  return refs;
}

/** The started milestones of the gate config (gate.json at the repository root). */
export function readGateConfig(root, file = 'gate.json') {
  const config = JSON.parse(readFileSync(resolve(root, file), 'utf8'));
  if (!Array.isArray(config.startedMilestones) || !config.startedMilestones.every(Number.isInteger)) {
    throw new Error('gate.json: startedMilestones must be an array of milestone numbers');
  }
  return config;
}

// ---------------------------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------------------------
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const flag = (name, fallback) => (args.includes(name) ? args[args.indexOf(name) + 1] : fallback);
  const root = resolve(flag('--root', resolve(dirname(fileURLToPath(import.meta.url)), '..')));
  const resultsDir = resolve(flag('--results-dir', join(root, 'var/test-results')));
  const verdictsPath = resolve(flag('--verdicts', join(root, 'var/exegra-verdicts.json')));
  const spec = readSpecValue(flag('--spec', join(root, 'specifications/spec.jsonc')));
  const verdicts = existsSync(verdictsPath) ? JSON.parse(readFileSync(verdictsPath, 'utf8')).results ?? null : null;
  const started = readGateConfig(root, flag('--config', 'gate.json')).startedMilestones;
  const report = runEvidence({ root, spec, resultsDir, verdicts, started });
  for (const line of reportLines(report)) console.log(line);
  process.exit(report.ok ? 0 : 1);
}
