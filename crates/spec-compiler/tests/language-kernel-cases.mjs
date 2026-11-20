// Runs the shared case table of the tenant rule language (kernel-cases.json) on the JavaScript module
// specc generates for the language (`specc web`), the twin of crates/spec-compiler-language-fixture/
// tests/kernel_cases.rs, which runs the same table on the generated Rust.
//
//   node language-kernel-cases.mjs <generated module> <kernel-cases.json> [--parsed]
//
// For each case: `source` parses to `expr`, `expr` prints as `source`, and evaluating or checking it
// in the case's environment gives `result`, value or error text. With `--parsed` the table's `expr` is
// not read, because the language's AST is written another way (the platform's tuple variants): the
// expression is what `source` parses to, which must print as `source`, and the results are those of
// evaluating and checking it. Prints each failure and exits 1 when there is one; on success prints
// the number of cases run.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const [modulePath, tablePath, flag] = process.argv.slice(2);
if (!modulePath || !tablePath) {
  console.error('usage: node language-kernel-cases.mjs <generated module> <kernel-cases.json> [--parsed]');
  process.exit(2);
}
const parsedOnly = flag === '--parsed';
const lang = await import(pathToFileURL(modulePath).href);
const table = JSON.parse(readFileSync(tablePath, 'utf8'));

/** The kernel's field id rule (`is_name` in crates/kernel/src/ids.rs). */
function isName(s) {
  return /^[a-z][a-z0-9_]*$/.test(s) && Buffer.byteLength(s) <= 64;
}

const fieldIdError = (name) => `invalid field identifier: ${JSON.stringify(name)}`;

/** A record of an eval environment: its fields and system attributes by name. */
function record(fields) {
  if (fields === null || fields === undefined || typeof fields !== 'object') return null;
  return {
    systemAttribute: (name) => fields[name] ?? 'Null',
    field(name) {
      if (!isName(name)) throw new lang.FieldError(fieldIdError(name));
      return fields[name] ?? 'Null';
    },
  };
}

/** The fixture's `rule-env`: record, old, new, param, now, principal and the roles held. No lookup. */
function world(env) {
  const records = { Record: record(env.record), Old: record(env.old), New: record(env.new) };
  const now = env.now && env.now.Time !== undefined ? env.now.Time : 0;
  const principal = env.principal && env.principal.Str !== undefined ? env.principal.Str : null;
  const roles = new Set(env.has_role ?? []);
  return {
    record: (scope) => records[scope],
    follow(_scope, field) {
      throw new lang.EvalError(`${field} cannot be followed here`);
    },
    param: (name) => (env.param ?? {})[name],
    now: () => now,
    principal: () => principal,
    hasRole: (role) => roles.has(role),
  };
}

/** The fixture's `check-env`: the shape of each field, the parameters and the roles declared. */
function site(env) {
  const fields = env.fields ?? {};
  const params = new Set(env.params ?? []);
  const roles = new Set(env.roles ?? []);
  const shapeOf = (name) => {
    if (!isName(name)) throw new lang.CheckError(fieldIdError(name));
    if (!Object.hasOwn(fields, name)) throw new lang.CheckError(`unknown field ${name}`);
    return fields[name];
  };
  return {
    fieldShape(through, name) {
      if (through === null) return shapeOf(name);
      // The target's fields are not declared here: any field id has the any shape.
      if (isName(name)) return 'any';
      throw new lang.CheckError(fieldIdError(name));
    },
    follow(field) {
      if (shapeOf(field) !== 'ref') throw new lang.CheckError(`${field} is not a single reference`);
    },
    hasParam: (name) => params.has(name),
    hasRole: (role) => roles.has(role),
  };
}

const failures = [];

/** The expression a case runs on: `source` parses to `expr` and `expr` prints as `source`. */
function expression(c) {
  if (parsedOnly) {
    let parsed;
    try {
      parsed = lang.parse(c.source);
    } catch (e) {
      failures.push(`${c.name}: ${c.source} does not parse: ${e.message}`);
      return null;
    }
    const printed = lang.print(parsed);
    if (printed !== c.source) failures.push(`${c.name}: prints as ${printed}, not ${c.source}`);
    return parsed;
  }
  try {
    assert.deepStrictEqual(lang.parse(c.source), c.expr);
  } catch (e) {
    failures.push(`${c.name}: ${c.source} does not parse to the table's expression: ${e.message.split('\n')[0]}`);
  }
  const printed = lang.print(c.expr);
  if (printed !== c.source) failures.push(`${c.name}: prints as ${printed}, not ${c.source}`);
  return c.expr;
}

function same(c, got) {
  try {
    assert.deepStrictEqual(got, c.result);
  } catch {
    failures.push(`${c.name}: got ${JSON.stringify(got)}, the table says ${JSON.stringify(c.result)}`);
  }
}

for (const c of table.eval) {
  const expr = expression(c);
  if (expr === null) continue;
  let got;
  try {
    got = { Ok: lang.evaluate(expr, world(c.env)) };
  } catch (e) {
    if (!(e instanceof lang.EvalError)) throw e;
    got = { Err: e.message };
  }
  same(c, got);
}
for (const c of table.check) {
  const expr = expression(c);
  if (expr === null) continue;
  let got;
  try {
    got = { Ok: lang.check(expr, site(c.env)) };
  } catch (e) {
    if (!(e instanceof lang.CheckError)) throw e;
    got = { Err: e.message };
  }
  same(c, got);
}

if (failures.length > 0) {
  console.error(failures.join('\n'));
  process.exit(1);
}
console.log(`${table.eval.length} eval cases and ${table.check.length} check cases agree with the table`);
