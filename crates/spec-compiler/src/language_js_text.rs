//! The text of the JavaScript module of a language (`language.rs`) that does not depend on the language: its helpers, written once, as the module of the language fixture shows them (`tests/fixtures/language/golden/rule-language.js`). Where a helper names a value variant on the wire, the text holds a placeholder (`@NULL@`, `@BOOL@`, `@LIST@`, `@K_BOOL@`, `@M_BOOL@`) that `Plan::subst` fills in.

pub(crate) const J_DOCS: &str = r####"// Representation. An expression and a value are the JSON of the declared union types `expr` and `value`, externally tagged as serde writes the Rust types: a variant without fields is its name as a string ('Null', 'Now'), a variant with one unnamed field is { Name: payload }, a variant with named fields is { Name: { field: ... } }. A scope or a case field is the name of its enum value. An expression the server sends is evaluated as it stands. Values and expressions are never changed.
//
// Integers. Checked arithmetic accepts only integers within +-(2^53-1), the integers a JavaScript number holds exactly, and refuses anything else, an operand or a result, with the evaluation error `integer-overflow`. It is computed on numbers with explicit range tests, not on BigInt: the operands are safe integers, so a sum, a difference or a product is exact when its magnitude is at most 2^53-1 and rounds to a magnitude of at least 2^53 when it is not, and the range test on the result finds every overflow. Values and results cross the page as JSON numbers, which a BigInt would not. Negative zero is written as 0. A number literal in an expression's text beyond +-(2^53-1) is refused by `parse` (the Rust parser accepts up to the 64-bit range).
//
// Text. `len` counts Unicode scalar values, as `chars().count()` does; `lower` is the full Unicode lower-casing, as `to_lowercase()` is; ordering compares by code point, as Rust compares strings by their bytes of UTF-8; `concat` measures its result in bytes of UTF-8.
//
// Errors. `parse` throws ParseError, `evaluate` throws EvalError and `check` throws CheckError, each with the message the Rust module gives."####;

pub(crate) const J_CLASSES: &str = r####"/** The error of an expression that cannot be parsed. */
export class ParseError extends Error {
  /** @param {string} message */
  constructor(message) {
    super(message);
    this.name = 'ParseError';
  }
}

/** An evaluation error: a type error, or a read the environment refused. Never a silent `false`. */
export class EvalError extends Error {
  /** @param {string} message */
  constructor(message) {
    super(message);
    this.name = 'EvalError';
  }
}

/** The error of an expression that does not check against its definition site. */
export class CheckError extends Error {
  /** @param {string} message */
  constructor(message) {
    super(message);
    this.name = 'CheckError';
  }
}

/** What a record's `field` throws when the name is not a valid field id: the reason is the message. */
export class FieldError extends Error {
  /** @param {string} message */
  constructor(message) {
    super(message);
    this.name = 'FieldError';
  }
}"####;

pub(crate) const J_ENV_HEAD: &str = r####"/**
 * Everything an expression may observe.
 * @typedef {object} Env"####;

pub(crate) const J_ENV_RECORD: &str = r####" * @property {(scope: string) => RuleRecord | null | undefined} record The record a record scope reads, or nothing when the scope has none (such as the old record on a create)."####;

pub(crate) const J_ENV_FOLLOW: &str = r####" * @property {(scope: string, field: string, target: number) => RuleRecord | null | undefined} follow The record a reference field of the scope's record points at: nothing when it does not exist; throws EvalError when the field is not a single reference, or the reference cannot be followed here."####;

pub(crate) const J_ENV_PARAM: &str = r####" * @property {(name: string) => any} param A named parameter, undefined when it is not supplied."####;

pub(crate) const J_ENV_NOW: &str =
    r####" * @property {() => number} now The context timestamp."####;

pub(crate) const J_ENV_PRINCIPAL: &str = r####" * @property {() => string | null} principal The acting principal's id for a user, null for an anonymous or system principal."####;

pub(crate) const J_ENV_HAS_ROLE: &str = r####" * @property {(role: string) => boolean} hasRole Whether the acting principal holds the role."####;

pub(crate) const J_TYPE_ENV_HEAD: &str = r####"/**
 * What an expression may refer to at its definition site.
 * @typedef {object} TypeEnv"####;

pub(crate) const J_TYPE_ENV_FIELD_SHAPE: &str = r####" * @property {(through: string | null, name: string) => string} fieldShape The shape of a field that is not a system attribute: of the definition site's record, or, with `through`, of the record that reference field points at. Throws CheckError when the name is not a field id or names no field."####;

pub(crate) const J_TYPE_ENV_FOLLOW: &str = r####" * @property {(field: string) => void} follow Checks that a field of the definition site's record is a single reference to an existing object type; throws CheckError when it is not."####;

pub(crate) const J_TYPE_ENV_HAS_PARAM: &str = r####" * @property {(name: string) => boolean} hasParam Whether the definition site declares the parameter."####;

pub(crate) const J_TYPE_ENV_HAS_ROLE: &str = r####" * @property {(role: string) => boolean} hasRole Whether the role is declared at the definition site."####;

pub(crate) const J_RECORD_TYPEDEF: &str = r####"/**
 * A record an expression reads.
 * @typedef {object} RuleRecord
 * @property {(name: string) => any} systemAttribute A system attribute the scope declares (only those are asked for).
 * @property {(name: string) => any} field A field's value, 'Null' when unset; throws FieldError, with the reason, when the name is not a valid field id.
 */"####;

pub(crate) const J_PARTS: &str = r####"/**
 * The variant name and the payload of an expression or a value.
 * @param {any} node
 * @returns {[string, any]}
 */
function parts(node) {
  if (typeof node === 'string') return [node, undefined];
  if (typeof node === 'object' && node !== null && !Array.isArray(node)) {
    const names = Object.keys(node);
    if (names.length === 1) return [names[0], node[names[0]]];
  }
  throw new TypeError(`not an expression or a value of the language: ${JSON.stringify(node)}`);
}"####;

pub(crate) const J_DEBUG: &str = r####"// --- The text of errors: how Rust prints what the messages embed ---------------------------

/**
 * A character as Rust's `Debug` writes it inside the given quote, for ASCII; other characters are written as they are.
 * @param {string} c
 * @param {string} quote
 */
function escapeDebug(c, quote) {
  if (c === '\\') return '\\\\';
  if (c === '\n') return '\\n';
  if (c === '\r') return '\\r';
  if (c === '\t') return '\\t';
  if (c === '\0') return '\\0';
  if (c === quote) return `\\${c}`;
  const code = c.codePointAt(0) ?? 0;
  if (code < 0x20 || code === 0x7f) return `\\u{${code.toString(16)}}`;
  return c;
}

/** A string as Rust's `Debug` writes it. @param {string} text */
function debugText(text) {
  let out = '"';
  for (const c of text) out += escapeDebug(c, '"');
  return `${out}"`;
}

/** A character as Rust's `Debug` writes it. @param {string} c */
function debugChar(c) {
  return `'${escapeDebug(c, "'")}'`;
}

/** A token as Rust's `Debug` writes it. @param {{ kind: string, value: any }} token */
function debugToken(token) {
  switch (token.kind) {
    case 'ident':
      return `Ident(${debugText(token.value)})`;
    case 'int':
      return `Int(${token.value})`;
    case 'str':
      return `Str(${debugText(token.value)})`;
    default:
      return `Sym(${debugText(token.value)})`;
  }
}

/** `Some(token)` or `None`, as Rust writes an `Option` of a token. @param {{ kind: string, value: any } | undefined} token */
function debugOption(token) {
  return token === undefined ? 'None' : `Some(${debugToken(token)})`;
}"####;

pub(crate) const J_PARSER_DIVIDER: &str = r####"// --- Parser ---------------------------------------------------------------------------------"####;

pub(crate) const J_CHAR_HELPERS: &str = r####"/** Rust's `char::is_whitespace`: the Unicode White_Space property. @param {string} c */
function isWhitespace(c) {
  const code = c.codePointAt(0) ?? 0;
  return (
    (code >= 0x9 && code <= 0xd) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

/** @param {string} c */
function isAsciiDigit(c) {
  return c.length === 1 && c >= '0' && c <= '9';
}

/** @param {string} c */
function isAsciiAlphabetic(c) {
  return c.length === 1 && ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'));
}

/** @param {string} c */
function isAsciiAlphanumeric(c) {
  return isAsciiAlphabetic(c) || isAsciiDigit(c);
}"####;

pub(crate) const J_ENDS_OPERAND: &str = r####"/**
 * Whether a token ends an operand, so that a `-` after it is the binary operator rather than the sign of a literal.
 * @param {{ kind: string, value: any } | undefined} token
 */
function endsOperand(token) {
  if (token === undefined) return false;
  if (token.kind === 'int' || token.kind === 'str') return true;
  if (token.kind === 'sym') return token.value === ')' || token.value === ']';
  return !WORD_OPERATORS.includes(token.value);
}"####;

pub(crate) const J_TOKENIZE: &str = r####"/**
 * @param {string} source
 * @returns {{ kind: string, value: any }[]}
 */
function tokenize(source) {
  const chars = Array.from(source);
  /** @type {{ kind: string, value: any }[]} */
  const tokens = [];
  let i = 0;
  while (i < chars.length) {
    const c = chars[i];
    if (isWhitespace(c)) {
      i += 1;
    } else if (isAsciiAlphabetic(c) || c === '_' || c === '$') {
      const start = i;
      while (i < chars.length && (isAsciiAlphanumeric(chars[i]) || chars[i] === '_' || chars[i] === '$')) {
        i += 1;
      }
      tokens.push({ kind: 'ident', value: chars.slice(start, i).join('') });
    } else if (isAsciiDigit(c) || (c === '-' && i + 1 < chars.length && isAsciiDigit(chars[i + 1])@SIGN_TEST@)) {
      const start = i;
      i += 1;
      while (i < chars.length && isAsciiDigit(chars[i])) {
        i += 1;
      }
      const text = chars.slice(start, i).join('');
      const number = Number(text);
      if (!Number.isSafeInteger(number)) throw new ParseError(`number out of range: ${text}`);
      tokens.push({ kind: 'int', value: number === 0 ? 0 : number });
    } else if (c === "'") {
      let text = '';
      i += 1;
      for (;;) {
        const d = chars[i];
        if (d === undefined) throw new ParseError('unterminated string');
        if (d === "'") break;
        if (d === '\\') {
          i += 1;
          const e = chars[i];
          if (e === 'n') text += '\n';
          else if (e === "'" || e === '\\') text += e;
          else throw new ParseError(`bad escape: ${e === undefined ? 'None' : `Some(${debugChar(e)})`}`);
        } else {
          text += d;
        }
        i += 1;
      }
      i += 1;
      tokens.push({ kind: 'str', value: text });
    } else {
      const rest = chars.slice(i, i + LONGEST_SYMBOL).join('');
      const symbol = SYMBOLS.find((s) => rest.startsWith(s));
      if (symbol === undefined) throw new ParseError(`unexpected character ${debugChar(c)}`);
      tokens.push({ kind: 'sym', value: symbol });
      i += symbol.length;
    }
  }
  return tokens;
}"####;

pub(crate) const J_PARSER_HEAD: &str = r####"class Parser {
  /** @param {{ kind: string, value: any }[]} tokens */
  constructor(tokens) {
    this.tokens = tokens;
    this.at = 0;
  }"####;

pub(crate) const J_PEEK: &str = r####"  peek() {
    return this.tokens[this.at];
  }"####;

pub(crate) const J_NEXT: &str = r####"  next() {
    const token = this.tokens[this.at];
    this.at += 1;
    return token;
  }"####;

pub(crate) const J_WORD: &str = r####"  /** @param {string} word */
  word(word) {
    const token = this.peek();
    if (token !== undefined && token.kind === 'ident' && token.value === word) {
      this.at += 1;
      return true;
    }
    return false;
  }"####;

pub(crate) const J_SYM: &str = r####"  /** @param {string} symbol */
  sym(symbol) {
    const token = this.peek();
    if (token !== undefined && token.kind === 'sym' && token.value === symbol) {
      this.at += 1;
      return true;
    }
    return false;
  }"####;

pub(crate) const J_EXPECT: &str = r####"  /** @param {string} symbol */
  expect(symbol) {
    if (this.sym(symbol)) return;
    throw new ParseError(`expected ${debugText(symbol)}, found ${debugOption(this.peek())}`);
  }"####;

pub(crate) const J_IDENT: &str = r####"  ident() {
    const token = this.next();
    if (token !== undefined && token.kind === 'ident') return token.value;
    throw new ParseError(`expected a name, found ${debugOption(token)}`);
  }"####;

pub(crate) const J_INT_ARG: &str = r####"  intArg() {
    this.expect('(');
    const token = this.next();
    if (token === undefined || token.kind !== 'int') throw new ParseError(`expected a number, found ${debugOption(token)}`);
    this.expect(')');
    return token.value;
  }"####;

pub(crate) const J_ARGS: &str = r####"  /** @param {number} count */
  args(count) {
    this.expect('(');
    const args = [];
    for (let i = 0; i < count; i += 1) {
      if (i > 0) this.expect(',');
      args.push(this.@TOP@());
    }
    this.expect(')');
    return args;
  }"####;

pub(crate) const J_PARSE: &str = r####"/**
 * Parses an expression from its authoring syntax.
 * @param {string} source
 * @returns {any}
 */
export function parse(source) {
  const parser = new Parser(tokenize(source));
  let expr;
  try {
    expr = parser.@TOP@();
  } catch (e) {
    if (e instanceof ParseError) throw new ParseError(`${e.message} in ${debugText(source)}`);
    throw e;
  }
  const extra = parser.peek();
  if (extra !== undefined) {
    throw new ParseError(`unexpected ${debugToken(extra)} after the expression in ${debugText(source)}`);
  }
  return expr;
}"####;

pub(crate) const J_PRINTER_DIVIDER: &str = r####"// --- Printer --------------------------------------------------------------------------------"####;

pub(crate) const J_WRITE_JOINED: &str = r####"/**
 * The items of an n-ary operator joined by its word; the identity when there are none.
 * @param {any[]} items
 * @param {string} word
 * @param {string} identity
 * @returns {string}
 */
function writeJoined(items, word, identity) {
  if (items.length === 0) return identity;
  return `(${items.map(print).join(` ${word} `)})`;
}"####;

pub(crate) const J_EVAL_DIVIDER: &str = r####"// --- Evaluator ------------------------------------------------------------------------------"####;

pub(crate) const J_TYPE_ERROR: &str = r####"/**
 * @param {string} op
 * @param {any} value
 */
function typeError(op, value) {
  return new EvalError(`${op} does not apply to a ${kind(value)}`);
}"####;

pub(crate) const J_VALUE_EQ: &str = r####"/**
 * Structural equality of two values, as Rust's derived `PartialEq`.
 * @param {any} a
 * @param {any} b
 * @returns {boolean}
 */
function valueEq(a, b) {
  const [ta, pa] = parts(a);
  const [tb, pb] = parts(b);
  if (ta !== tb) return false;
  if (ta === @LIST@) return pa.length === pb.length && pa.every((/** @type {any} */ x, /** @type {number} */ i) => valueEq(x, pb[i]));
  return pa === pb;
}"####;

pub(crate) const J_ATTRIBUTE: &str = r####"/**
 * A field or system attribute of a record; null when there is no record.
 * @param {RuleRecord | null | undefined} record
 * @param {string} scope
 * @param {string} name
 * @returns {any}
 */
function attribute(record, scope, name) {
  if (record === null || record === undefined) return @NULL@;
  if (systemAttributeShape(scope, name) !== null) return record.systemAttribute(name);
  try {
    return record.field(name);
  } catch (e) {
    if (e instanceof FieldError) throw new EvalError(`unknown attribute ${name}`);
    throw e;
  }
}"####;

pub(crate) const J_READ_FIELD: &str = r####"/**
 * A field of a record, with the reason the record gives when the name is not a field id.
 * @param {RuleRecord} record
 * @param {string} name
 * @returns {any}
 */
function readField(record, name) {
  try {
    return record.field(name);
  } catch (e) {
    if (e instanceof FieldError) throw new EvalError(e.message);
    throw e;
  }
}"####;

pub(crate) const J_KLEENE: &str = r####"/**
 * Kleene logic over operands evaluated left to right: `decisive` (false for and, true for or) ends the run.
 * @param {any[]} items
 * @param {Env} env
 * @param {boolean} decisive
 * @param {string} op
 * @returns {any}
 */
function kleene(items, env, decisive, op) {
  let sawNull = false;
  for (const item of items) {
    const value = evaluate(item, env);
    const [t, x] = parts(value);
    if (t === @BOOL@) {
      if (x === decisive) return { @K_BOOL@: decisive };
    } else if (t === @NULL@) {
      sawNull = true;
    } else {
      throw typeError(op, value);
    }
  }
  return sawNull ? @NULL@ : { @K_BOOL@: !decisive };
}"####;

pub(crate) const J_COMPARABLE: &str = r####"/**
 * Null when either side is null; an error when the two are of different kinds.
 * @param {any} left
 * @param {any} right
 * @returns {boolean}
 */
function comparable(left, right) {
  if (left === @NULL@ || right === @NULL@) return false;
  if (kind(left) !== kind(right)) throw new EvalError(`cannot compare a ${kind(left)} with a ${kind(right)}`);
  return true;
}"####;

pub(crate) const J_EQUALITY: &str = r####"/**
 * @param {any} left
 * @param {any} right
 * @param {boolean} equal
 * @returns {any}
 */
function equality(left, right, equal) {
  if (!comparable(left, right)) return @NULL@;
  return { @K_BOOL@: valueEq(left, right) === equal };
}"####;

pub(crate) const J_COMPARE_TEXT: &str = r####"/**
 * Strings compare by code point, as Rust compares them by their bytes of UTF-8.
 * @param {string} a
 * @param {string} b
 * @returns {number}
 */
function compareText(a, b) {
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const x = a.codePointAt(i) ?? 0;
    const y = b.codePointAt(j) ?? 0;
    if (x !== y) return x < y ? -1 : 1;
    i += x > 0xffff ? 2 : 1;
    j += y > 0xffff ? 2 : 1;
  }
  if (i < a.length) return 1;
  if (j < b.length) return -1;
  return 0;
}"####;

pub(crate) const J_IS_SLUG: &str = r####"/**
 * Whether a string is a slug: `[a-z0-9]+(-[a-z0-9]+)*`.
 * @param {string} s
 * @returns {boolean}
 */
export function isSlug(s) {
  return s !== '' && s.split('-').every((part) => /^[a-z0-9]+$/.test(part));
}"####;

pub(crate) const J_SAFE: &str = r####"/** The largest magnitude of an integer of checked arithmetic, 2^53-1: what Rust and JavaScript both hold exactly. */
const SAFE_INT = 9007199254740991;

/**
 * An operand of checked arithmetic: an integer within +-(2^53-1), or the error `integer-overflow`. Negative zero is 0.
 * @param {any} value
 * @returns {number}
 */
function safe(value) {
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= -SAFE_INT && value <= SAFE_INT) {
    return value === 0 ? 0 : value;
  }
  throw new EvalError('integer-overflow');
}

/**
 * The result of checked arithmetic on safe integers: within +-(2^53-1), or the error `integer-overflow`. Never wrapped, saturated or rounded.
 * @param {number} value
 * @returns {number}
 */
function checked(value) {
  return safe(value);
}"####;

pub(crate) const J_CONCAT_HELPERS: &str = r####"/** The longest string `concat` builds, in bytes of UTF-8. */
const MAX_TEXT_BYTES = 4096;

/**
 * The length of a string in bytes of UTF-8.
 * @param {string} s
 * @returns {number}
 */
function utf8Length(s) {
  let bytes = 0;
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff && i + 1 < s.length && s.charCodeAt(i + 1) >= 0xdc00 && s.charCodeAt(i + 1) <= 0xdfff) {
      bytes += 4;
      i += 1;
    } else bytes += 3;
  }
  return bytes;
}"####;

pub(crate) const J_CODE_POINT_COUNT: &str = r####"/**
 * The number of Unicode scalar values of a string, as Rust's `chars().count()`.
 * @param {string} s
 * @returns {number}
 */
function codePointCount(s) {
  let count = 0;
  for (const _ of s) count += 1;
  return count;
}"####;

pub(crate) const J_HOLDS: &str = r####"/**
 * Evaluates to a definite truth value: only `true` passes.
 * @param {any} expr
 * @param {Env} env
 * @returns {boolean}
 */
export function holds(expr, env) {
  const value = evaluate(expr, env);
  return typeof value === 'object' && value !== null && value@M_BOOL@ === true;
}"####;

pub(crate) const J_CHECKER_DIVIDER: &str = r####"// --- Checker --------------------------------------------------------------------------------"####;

pub(crate) const J_SHAPE_NAME: &str = r####"/**
 * The name of a shape as the checker's messages write it.
 * @param {string} shape
 * @returns {string}
 */
function shapeName(shape) {
  return SHAPE_NAMES[/** @type {keyof typeof SHAPE_NAMES} */ (shape)];
}"####;

pub(crate) const J_EXPECT_SHAPE: &str = r####"/**
 * @param {string} shape
 * @param {string[]} allowed
 * @param {string} what
 */
function expectShape(shape, allowed, what) {
  if (shape === 'any' || allowed.includes(shape)) return;
  throw new CheckError(`${what} does not accept a ${shapeName(shape)} operand`);
}"####;

pub(crate) const J_ATTRIBUTE_SHAPE: &str = r####"/**
 * @param {TypeEnv} env
 * @param {string | null} through
 * @param {string} scope
 * @param {string} name
 * @returns {string}
 */
function attributeShape(env, through, scope, name) {
  const shape = systemAttributeShape(scope, name);
  return shape !== null ? shape : env.fieldShape(through, name);
}"####;
