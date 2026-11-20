// Tests for scripts/example-fixtures.mjs. Run with: node --test scripts/example-fixtures.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { canonical, exampleArgs, exampleBytes, operations, overrides, planFixtures, report } from './example-fixtures.mjs';

const tenant = (extra = {}) => ({ members: { 'alpha:owner': ['owner'], 'alpha:writer': ['writer'] }, policies: { note: 'x'.repeat(120) }, ...extra });

const SPEC = {
  functions: [
    {
      id: 'f-one',
      params: [{ name: 'tenant', type: 'tenant' }, { name: 'n', type: 'number' }],
      examples: [
        { args: { tenant: tenant(), n: 1 }, result: 1 },
        { args: { tenant: tenant(), n: 2 }, result: 2 },
        { args: { tenant: tenant({ extra: 'one more' }), n: 3 }, result: 3 },
        { args: { tenant: { members: {}, policies: {} }, n: 4 }, result: 4 },
      ],
    },
    {
      id: 'f-two',
      params: [{ name: 'tenant', type: 'tenant' }],
      examples: [{ args: { tenant: { policies: { note: 'x'.repeat(120) }, members: { 'alpha:owner': ['owner'], 'alpha:writer': ['writer'] } } }, result: 0 }],
    },
  ],
};

test('equal values have one canonical text whatever the key order', () => {
  assert.equal(canonical({ b: 1, a: [2, { d: 1, c: 2 }] }), canonical({ a: [2, { c: 2, d: 1 }], b: 1 }));
  assert.notEqual(canonical({ a: 1 }), canonical({ a: 2 }));
});

test('overrides: changed values and added keys are paths, an index goes by number, a removed key replaces its object', () => {
  assert.deepEqual(overrides({ a: 1, b: { c: 2 } }, { a: 1, b: { c: 3 } }), [['b.c', 3]]);
  assert.deepEqual(overrides({ a: 1 }, { a: 1, z: 9 }), [['z', 9]]);
  assert.deepEqual(overrides({ l: [1, 2] }, { l: [1, 5] }), [['l.1', 5]]);
  assert.deepEqual(overrides({ l: [1, 2] }, { l: [1, 2, 3] }), [['l', [1, 2, 3]]]);
  assert.deepEqual(overrides({ o: { a: 1, b: 2 } }, { o: { a: 1 } }), [['o', { a: 1 }]]);
  assert.deepEqual(overrides({ a: 1 }, { a: 1 }), []);
  assert.equal(overrides(1, 2), null, 'the root cannot be replaced');
  assert.deepEqual(overrides({ m: { 'a.b': 1 } }, { m: { 'a.b': 2 } }), [['m', { 'a.b': 2 }]], 'a key with a dot is not named');
});

test('example arguments are read with the type of their parameter, and references are skipped', () => {
  const args = exampleArgs({ functions: [{ id: 'f', params: [{ name: 'a', type: 'T' }], examples: [{ args: { a: { fixture: 'x' } } }, { args: { a: { v: 1 } } }] }] });
  assert.deepEqual(args.map((a) => [a.fn, a.index, a.param, a.type]), [['f', 1, 'a', 'T']]);
});

test('a repeated value becomes a fixture, a near copy a reference with overrides, a small value stays', () => {
  const plan = planFixtures(SPEC, { minBytes: 100 });
  assert.equal(plan.fixtures.length, 1);
  assert.equal(plan.fixtures[0].id, 'fx-tenant-1');
  assert.equal(plan.fixtures[0].type, 'tenant');
  assert.equal(plan.fixtures[0].uses, 4);
  const byKey = Object.fromEntries(plan.refs.map((r) => [`${r.fn}.${r.index}.${r.param}`, r.ref]));
  assert.deepEqual(byKey['f-one.0.tenant'], { fixture: 'fx-tenant-1' });
  assert.deepEqual(byKey['f-one.1.tenant'], { fixture: 'fx-tenant-1' });
  assert.deepEqual(byKey['f-two.0.tenant'], { fixture: 'fx-tenant-1' }, 'key order does not matter');
  assert.deepEqual(byKey['f-one.2.tenant'], { fixture: 'fx-tenant-1', with: { extra: 'one more' } });
  assert.equal(byKey['f-one.3.tenant'], undefined);
  assert.equal(byKey['f-one.0.n'], undefined);
});

test('the operations create each fixture once and set only the arguments they replace', () => {
  const ops = operations(planFixtures(SPEC, { minBytes: 100 }));
  assert.equal(ops.filter((o) => o.type === 'CreateEntity').length, 1);
  assert.deepEqual(ops[0].data.id, 'fx-tenant-1');
  const sets = ops.filter((o) => o.type === 'SetProperty');
  assert.deepEqual(sets.map((o) => `${o.id}:${o.property}`).sort(), ['f-one:examples.0.args.tenant', 'f-one:examples.1.args.tenant', 'f-one:examples.2.args.tenant', 'f-two:examples.0.args.tenant']);
});

test('the sizes after are the examples with references plus the fixtures, and smaller than before', () => {
  const plan = planFixtures(SPEC, { minBytes: 100 });
  assert.equal(plan.before.examples, exampleBytes(SPEC).examples);
  assert.ok(plan.after.total < plan.before.total);
  const rewritten = structuredClone(SPEC);
  for (const r of plan.refs) rewritten.functions.find((f) => f.id === r.fn).examples[r.index].args[r.param] = r.ref;
  rewritten.exampleFixtures = plan.fixtures;
  assert.equal(plan.after.examples, exampleBytes(rewritten).examples);
  assert.equal(plan.after.total, exampleBytes(rewritten).total);
  assert.match(report(plan).join('\n'), /function examples before: \d+ bytes[\s\S]*function examples after/);
});

test('a specification already using references is not planned again', () => {
  const plan = planFixtures(SPEC, { minBytes: 100 });
  const rewritten = structuredClone(SPEC);
  for (const r of plan.refs) rewritten.functions.find((f) => f.id === r.fn).examples[r.index].args[r.param] = r.ref;
  assert.equal(planFixtures(rewritten, { minBytes: 100 }).refs.length, 0);
});
