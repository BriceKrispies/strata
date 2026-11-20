// @ts-check
// The page's check of a record before it is sent (WG-5143). The server tells the editor what the
// kernel will check on a record type, for this reader and nothing about a field the reader may
// not read (`strata_kernel::editor_rules`); this module checks a draft against that with the
// generated evaluator of the tenant rule language (`tenant-rule-language.js`), which is held to
// the server's by the shared case table, and answers in the kernel's own failed checks, so the
// generated refusal wording (`errors.js`) shows them as it shows the server's.
//
// What the page cannot evaluate is the server's to decide: a rule that follows a reference, one
// that reads a field or system attribute it was not sent, one it cannot parse, a field whose
// wire value it cannot read as the type. Those are reported as `undecided`, never as a pass shown
// to the person and never as a block. The server's answer always wins (`disagreement`).
//
// The draft is the record as it would be stored, in the wire form of the records endpoint: the
// fields of the request the form makes, over the stored record's on an update.
import { EvalError, ParseError, evaluate, parse } from '../generated/tenant-rule-language.js';

/**
 * The kernel's words for what it refuses, `crates/kernel/src/rules.rs` (`validate_record`),
 * `limits.rs` (`value_problem`) and `transition.rs` (a workflow step). The one table of them.
 */
export const MESSAGES = {
  required: () => 'a value is required',
  stringTooLong: (/** @type {number} */ bytes, /** @type {number} */ max) => `a string of ${bytes} bytes is longer than the maximum of ${max}`,
  listTooLong: (/** @type {number} */ items, /** @type {number} */ max) => `a list of ${items} items is longer than the maximum of ${max}`,
  uniqueTooLong: (/** @type {number} */ max) => `a value in a unique field is at most ${max} bytes encoded`,
  constraint: (/** @type {string} */ message, /** @type {string} */ rule) => `${message} [${rule}]`,
  evaluation: (/** @type {string} */ rule, /** @type {string} */ reason) => `${rule}: ${reason}`,
  noTransition: (/** @type {string} */ from, /** @type {string} */ to) => `no transition from ${debugState(from)} to ${debugState(to)}`,
  guardNotTrue: (/** @type {string} */ step) => `${step}: guard is not true`,
};

/** A workflow state as Rust's `Debug` writes the value in the kernel's message. */
function debugState(/** @type {string | null} */ state) {
  return state === null ? 'Null' : `Str(${JSON.stringify(state)})`;
}

/**
 * What the page could not decide: its reasoning, in the server's rule name.
 */
class Undecided extends Error {}

/**
 * @typedef {{ id: string, label: string, type: string, type_name: string, required: boolean, unique: boolean }} RuleField
 * @typedef {{ name: string, from: string, to: string, guard: string | null, server_decides: boolean, sets: string[] }} RuleStep
 * @typedef {{ id: string, field: string, initial: string[], state: string | null, transitions: RuleStep[] }} RuleFlow
 * @typedef {{ object: string, fields: RuleField[], limits: { string_bytes: number, list_items: number, unique_bytes: number, record_bytes: number }, constraints: { id: string, rule: string, message: string }[], workflows: RuleFlow[] }} RecordRules
 * @typedef {{ rule: string, detail: string }} FailedCheck
 * @typedef {{ failed: FailedCheck[], undecided: { rule: string, reason: string }[] }} Verdict
 */

/**
 * A wire value of a field as a value of the rule language, or a refusal to read it.
 * @param {string} typeName The kernel's name of the field's type.
 * @param {unknown} plain
 * @returns {any}
 */
export function valueOf(typeName, plain) {
  if (plain === null || plain === undefined) return 'Null';
  if (typeName === 'string' || typeName === 'file' || typeName === 'language') {
    if (typeof plain === 'string') return { Str: plain };
  } else if (typeName === 'int') {
    if (typeof plain === 'number' && Number.isInteger(plain)) return { Int: plain };
  } else if (typeName === 'bool') {
    if (typeof plain === 'boolean') return { Bool: plain };
  } else if (typeName === 'timestamp') {
    if (typeof plain === 'number' && Number.isInteger(plain)) return { Time: plain };
  } else if (typeName.startsWith('reference<')) {
    if (typeof plain === 'number' && Number.isInteger(plain)) return { Ref: plain };
  } else if (typeName.startsWith('list<') && typeName.endsWith('>')) {
    if (Array.isArray(plain)) return { List: plain.map((item) => valueOf(typeName.slice(5, -1), item)) };
  }
  throw new Undecided(`a value the page cannot read as ${typeName}`);
}

/** @param {string} text */
function utf8Bytes(text) {
  return new TextEncoder().encode(text).length;
}

/**
 * The first limit a value breaks, as `limits::value_problem` words it, or null.
 * @param {any} value
 * @param {RecordRules['limits']} limits
 * @returns {string | null}
 */
function valueProblem(value, limits) {
  if (value === 'Null') return null;
  const [tag, payload] = Object.entries(value)[0];
  if (tag === 'Str' && utf8Bytes(payload) > limits.string_bytes) return MESSAGES.stringTooLong(utf8Bytes(payload), limits.string_bytes);
  if (tag === 'List') {
    if (payload.length > limits.list_items) return MESSAGES.listTooLong(payload.length, limits.list_items);
    for (const item of payload) {
      const problem = valueProblem(item, limits);
      if (problem) return problem;
    }
  }
  return null;
}

/**
 * The fields an expression reads, by walking its JSON.
 * @param {any} node
 * @param {Set<string>} out
 */
function fieldsRead(node, out) {
  if (Array.isArray(node)) {
    for (const item of node) fieldsRead(item, out);
  } else if (node && typeof node === 'object') {
    for (const [key, value] of Object.entries(node)) {
      if (key === 'Var' && Array.isArray(value) && typeof value[1] === 'string') out.add(value[1]);
      else fieldsRead(value, out);
    }
  }
}

/**
 * A record the evaluator reads: the values of the fields the page was told about; anything else
 * (a field it was not sent, a system attribute) is not the page's to decide.
 * @param {Map<string, any> | null} values
 */
function recordOf(values) {
  if (values === null) return null;
  return {
    systemAttribute() {
      throw new Undecided('a system attribute is the server\'s');
    },
    /** @param {string} name */
    field(name) {
      const value = values.get(name);
      if (value === undefined) throw new Undecided(`${name} was not sent to the page`);
      return value;
    },
  };
}

/**
 * The environment an expression is evaluated in: `record` is the stored record or, for a
 * constraint, the draft, as the kernel binds it.
 * @param {{ record: Map<string, any> | null, old: Map<string, any> | null, draft: Map<string, any> }} scopes
 * @param {{ principal: string | null, roles: string[], now: number }} who
 */
function envOf(scopes, who) {
  const records = { Record: recordOf(scopes.record), Old: recordOf(scopes.old), New: recordOf(scopes.draft) };
  return {
    /** @param {'Record' | 'Old' | 'New'} scope */
    record: (scope) => records[scope],
    follow() {
      throw new Undecided('the rule follows a reference');
    },
    param: () => undefined,
    now: () => who.now,
    principal: () => who.principal,
    hasRole: (/** @type {string} */ role) => who.roles.includes(role),
  };
}

/**
 * Evaluate a rule's text. 'true' or 'false' (a value that is neither is 'other'), or a thrown
 * `Undecided`; an evaluation error is the kernel's, with its own message.
 * @param {string} text
 * @param {ReturnType<typeof envOf>} env
 * @returns {{ result: 'true' | 'false' | 'other' } | { error: string }}
 */
function run(text, env) {
  let expr;
  try {
    expr = parse(text);
  } catch (e) {
    if (e instanceof ParseError) throw new Undecided(`the page cannot read the rule: ${e.message}`);
    throw e;
  }
  try {
    const value = evaluate(expr, env);
    if (typeof value === 'object' && value !== null && value.Bool === false) return { result: 'false' };
    return { result: typeof value === 'object' && value !== null && value.Bool === true ? 'true' : 'other' };
  } catch (e) {
    if (e instanceof EvalError) return { error: e.message };
    throw e;
  }
}

/**
 * Check a draft record against what the server sent of the record type.
 * @param {RecordRules} rules
 * @param {Record<string, unknown>} draft The record as it would be stored, wire form; null or absent is unset.
 * @param {{ old?: Record<string, unknown> | null, principal?: string | null, roles?: string[], now?: number }} [context]
 *   `old` is the stored record on an update, null on a create.
 * @returns {Verdict}
 */
export function checkRecord(rules, draft, context = {}) {
  const who = { principal: context.principal ?? null, roles: [...(context.roles ?? []), 'public'], now: context.now ?? 0 };
  /** @type {FailedCheck[]} */
  const failed = [];
  /** @type {{ rule: string, reason: string }[]} */
  const undecided = [];
  const name = rules.object;
  /** @type {Map<string, any>} */
  const values = new Map();
  /** @type {Map<string, any>} */
  const stored = new Map();
  const old = context.old ?? null;

  for (const field of rules.fields) {
    const at = `${name}.${field.id}`;
    let value;
    try {
      value = valueOf(field.type_name, draft[field.id]);
      if (old) stored.set(field.id, valueOf(field.type_name, old[field.id]));
    } catch (e) {
      if (!(e instanceof Undecided)) throw e;
      undecided.push({ rule: `type:${at}`, reason: e.message });
      continue;
    }
    values.set(field.id, value);
    if (value === 'Null') {
      if (field.required) failed.push({ rule: `required:${at}`, detail: MESSAGES.required() });
      continue;
    }
    const problem = valueProblem(value, rules.limits);
    if (problem) {
      failed.push({ rule: `limit:${at}`, detail: problem });
      continue;
    }
    if (field.unique) {
      const encoded = utf8Bytes(JSON.stringify(value));
      if (encoded > rules.limits.unique_bytes) {
        failed.push({ rule: `limit:unique:${at}`, detail: MESSAGES.uniqueTooLong(rules.limits.unique_bytes) });
        continue;
      }
      undecided.push({ rule: `unique:${at}`, reason: 'whether another record has this value is the server\'s' });
    }
  }
  // A value the page could not read leaves its field out; a rule that reads it is undecided.

  // A workflow step the draft attempts: the guard of a transition from the stored state.
  /** @type {Set<string>} */
  const assigned = new Set();
  if (old) {
    for (const flow of rules.workflows) {
      const from = typeof old[flow.field] === 'string' ? /** @type {string} */ (old[flow.field]) : null;
      const to = typeof draft[flow.field] === 'string' ? /** @type {string} */ (draft[flow.field]) : null;
      if (to === null || from === to) continue;
      const rule = `workflow:${flow.id}`;
      const steps = flow.transitions.filter((step) => step.to === to);
      if (!steps.length) {
        failed.push({ rule, detail: MESSAGES.noTransition(/** @type {string} */ (from), to) });
        continue;
      }
      const refused = [];
      let open = false;
      let unknown = false;
      for (const step of steps) {
        if (step.guard === null && !step.server_decides) {
          open = true;
          step.sets.forEach((field) => assigned.add(field));
          break;
        }
        if (step.guard === null) {
          unknown = true;
          continue;
        }
        try {
          const outcome = run(step.guard, envOf({ record: stored, old: stored, draft: values }, who));
          if ('error' in outcome) refused.push(MESSAGES.evaluation(step.name, outcome.error));
          else if (outcome.result === 'true') {
            open = true;
            step.sets.forEach((field) => assigned.add(field));
            break;
          } else refused.push(MESSAGES.guardNotTrue(step.name));
        } catch (e) {
          if (!(e instanceof Undecided)) throw e;
          unknown = true;
          undecided.push({ rule, reason: `${step.name}: ${e.message}` });
        }
      }
      if (unknown && !open) undecided.push({ rule, reason: 'a guard the page cannot decide' });
      else if (!open) failed.push({ rule, detail: refused.join('; ') });
    }
  }

  // The kernel asks its constraints only of a record whose fields passed the checks above.
  if (!failed.length) {
    const env = envOf({ record: values, old: old ? stored : null, draft: values }, who);
    for (const constraint of rules.constraints) {
      const rule = `constraint:${name}.${constraint.id}`;
      try {
        const read = new Set();
        fieldsRead(parse(constraint.rule), read);
        // A workflow step assigns fields before the constraints run; their final values are the server's.
        if ([...read].some((field) => assigned.has(field))) throw new Undecided('a field a workflow step assigns');
        const outcome = run(constraint.rule, env);
        if ('error' in outcome) failed.push({ rule, detail: MESSAGES.evaluation(constraint.rule, outcome.error) });
        else if (outcome.result === 'false') failed.push({ rule, detail: MESSAGES.constraint(constraint.message, constraint.rule) });
      } catch (e) {
        if (e instanceof ParseError) undecided.push({ rule, reason: `the page cannot read the rule: ${e.message}` });
        else if (e instanceof Undecided) undecided.push({ rule, reason: e.message });
        else throw e;
      }
    }
  }
  return { failed, undecided };
}

/**
 * The draft of an editor submission: the fields of the records request the form makes, over the
 * stored record's on an update (a field the request sends as null is unset).
 * @param {{ op?: string, fields?: Record<string, unknown> } | undefined} body The records request's body.
 * @param {Record<string, unknown> | null} stored
 * @returns {Record<string, unknown>}
 */
export function draftOf(body, stored) {
  /** @type {Record<string, unknown>} */
  const draft = body?.op === 'update' && stored ? { ...stored } : {};
  for (const [field, value] of Object.entries(body?.fields ?? {})) {
    if (value === null) delete draft[field];
    else draft[field] = value;
  }
  return draft;
}

/** The kinds of check the page evaluates, by the name before the colon in a check's rule. */
const EVALUATED = ['required', 'limit', 'constraint', 'workflow'];

/**
 * Where the server's refusal of a draft the page passed shows that the page was wrong: the
 * server's failed checks of a kind the page evaluates, that the page neither failed nor left
 * undecided. Null when the two agree (or the refusal is of something the page does not judge).
 * @param {Verdict} verdict The page's verdict on what it sent.
 * @param {{ failed?: FailedCheck[] } | null | undefined} error The server's refusal.
 * @returns {string | null}
 */
export function disagreement(verdict, error) {
  const undecided = new Set(verdict.undecided.map((u) => u.rule));
  const missed = (error?.failed ?? []).filter((check) => {
    const kind = check.rule.split(':')[0];
    return EVALUATED.includes(kind) && !undecided.has(check.rule) && !verdict.failed.some((f) => f.rule === check.rule);
  });
  if (!missed.length) return null;
  return `the page passed a draft the server refused: ${missed.map((c) => `${c.rule} (${c.detail})`).join('; ')}`;
}
