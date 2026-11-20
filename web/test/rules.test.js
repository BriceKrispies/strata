// The page reads the rules the server carries to it as text with its own copy of the tenant rule
// language, generated from the language declaration. The case table that holds the copy to the
// server's lives in specc (crates/spec-compiler/tests/language_js.rs); these tests are the page's
// use of it.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EvalError, evaluate, parse, print } from '../src/generated/tenant-rule-language.js';
import { ruleDisagreement } from '../src/kernel/rules.js';

test('a rule printed in the authoring syntax is read back unchanged', () => {
  for (const source of ['(new.count < 3)', '((record.count + 1) <= 10)', 'not(is_null(new.title))', '(has_role(editor) or (len(new.title) > 0))', 'concat(text(new.count), \'!\')']) {
    const printed = print(parse(source));
    assert.equal(ruleDisagreement(printed), null, printed);
  }
});

test('a rule the page prints differently, or cannot read, is a disagreement it reports', () => {
  assert.equal(ruleDisagreement('new.count<3'), 'the page prints it as (new.count < 3)');
  assert.match(String(ruleDisagreement('(1 +')), /unexpected None/);
  assert.equal(ruleDisagreement(null), null);
  assert.equal(ruleDisagreement(undefined), null);
});

test('the page refuses checked arithmetic that leaves the exact integers, as the server does', () => {
  const env = { record: () => null, follow: () => null, param: () => undefined, now: () => 0, principal: () => null, hasRole: () => false };
  assert.deepEqual(evaluate(parse('(441650591 * 20394401)'), env), { Int: 9007199254740991 });
  assert.throws(() => evaluate(parse('(4503599627370496 * 2)'), env), (e) => e instanceof EvalError && e.message === 'integer-overflow');
});
