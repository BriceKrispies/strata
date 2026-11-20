// Tests for scripts/gate-realisation.mjs. Run with: node --test scripts/gate-realisation.test.mjs
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { deriveLabel, evidenceOf, realisationReport, realisationVerdict, verifiedRefs } from './gate-realisation.mjs';

const requirement = (id, ref, realisation, method = 'scenario') => ({ id, ref, kind: 'requirement', realisation, evaluate: { method } });

const SOURCES = [
  { path: 'crates/a/tests/t.rs', text: '// Verifies: AAA-001\n#[test] fn one() {}\n/// Verifies: AAA-002, aaa-003\nfn two() {}' },
  { path: 'crates/a/tests/prop.rs', text: '// Verifies: PRP-001\nfn p() { let mut rng = StdRng::seed_from_u64(7); }' },
  { path: 'crates/a/tests/example.rs', text: '// Verifies: PRP-002\nfn p() { assert_eq!(1, 1); }' },
];

test('Verifies comments name refs in any case, several per line', () => {
  const refs = verifiedRefs(SOURCES);
  assert.deepEqual([...refs.keys()].sort(), ['aaa-001', 'aaa-002', 'aaa-003', 'prp-001', 'prp-002']);
  assert.deepEqual([...refs.get('aaa-003')], ['crates/a/tests/t.rs']);
});

test('the label follows the evidence: a budget, a test or journey, a model or a construct, nothing', () => {
  const none = { tests: [], journeys: [], models: [], realisedBy: [], budget: false };
  assert.equal(deriveLabel({ ...none, budget: true, tests: ['x'] }), 'operational');
  assert.equal(deriveLabel({ ...none, tests: ['x'], models: ['m'] }), 'checked');
  assert.equal(deriveLabel({ ...none, journeys: ['ui-1'] }), 'checked');
  assert.equal(deriveLabel({ ...none, models: ['m-1'] }), 'construction');
  assert.equal(deriveLabel({ ...none, realisedBy: ['policy-1'] }), 'construction');
  assert.equal(deriveLabel(none), null);
});

test('evidence is read from tests by ref, journeys by claim id, models by `models`', () => {
  const claims = [requirement('aaa-001', 'AAA-001', 'checked'), { id: 'm-1', kind: 'invariant', models: 'aaa-001', evaluate: { method: 'never' } }];
  const evidence = evidenceOf(claims[0], { refs: verifiedRefs(SOURCES), uiScenarios: [{ id: 'ui-1', verifies: ['aaa-001'] }], claims });
  assert.deepEqual(evidence, { tests: ['crates/a/tests/t.rs'], journeys: ['ui-1'], models: ['m-1'], realisedBy: [], budget: false });
});

const CLAIMS = [
  requirement('aaa-001', 'AAA-001', 'checked'),
  requirement('aaa-004', 'AAA-004', 'checked'),
  requirement('bbb-001', 'BBB-001', 'checked'),
  { id: 'm-bbb', kind: 'invariant', models: 'bbb-001', evaluate: { method: 'never' } },
  requirement('ccc-001', 'CCC-001', 'construction'),
  requirement('ddd-001', 'DDD-001', 'checked', 'budget'),
  requirement('eee-001', 'EEE-001', 'operational', 'budget'),
];

test('the report counts declared against derived labels and names each disagreement', () => {
  const report = realisationReport({ claims: CLAIMS, sources: SOURCES });
  assert.equal(report.requirements, 6);
  assert.deepEqual(report.declared, { construction: 1, checked: 4, operational: 1, missing: 0 });
  assert.deepEqual(report.derived, { construction: 1, checked: 1, operational: 2, none: 2 });
  assert.deepEqual(report.differ.map((d) => [d.id, d.declared, d.derived]), [
    ['aaa-004', 'checked', null],
    ['bbb-001', 'checked', 'construction'],
    ['ccc-001', 'construction', null],
    ['ddd-001', 'checked', 'operational'],
  ]);
});

test('a label the evidence contradicts fails the verdict, and a label with no evidence only is reported', () => {
  const report = realisationReport({ claims: CLAIMS, sources: SOURCES });
  assert.deepEqual(report.contradicted, [
    'ccc-001 is declared construction and no model claim, construct or test stands behind it',
    'ddd-001 is declared checked but a budget measures it (derived: operational)',
  ]);
  const verdict = realisationVerdict(report);
  assert.equal(verdict.ok, false);
  assert.ok(verdict.lines.some((l) => /1 requirement\(s\) declared checked, derived none \(no evidence\): aaa-004/.test(l)));

  const fine = realisationVerdict(realisationReport({ claims: [CLAIMS[0], CLAIMS[1], CLAIMS[2], CLAIMS[3], CLAIMS[6]], sources: SOURCES }));
  assert.equal(fine.ok, true);
});

test('a requirement without a label, or with one that is not a label, is contradicted', () => {
  const report = realisationReport({ claims: [requirement('x-1', 'X-1', undefined), requirement('x-2', 'X-2', 'derived')], sources: [] });
  assert.equal(report.contradicted.length, 2);
  assert.equal(report.declared.missing, 2);
});

test('a property claim no test names, or named only by tests with no generator, is reported', () => {
  const claims = [
    requirement('prp-001', 'PRP-001', 'checked', 'property'),
    requirement('prp-002', 'PRP-002', 'checked', 'property'),
    requirement('prp-003', 'PRP-003', 'checked', 'property'),
    requirement('aaa-001', 'AAA-001', 'checked'),
  ];
  const { properties } = realisationReport({ claims, sources: SOURCES });
  assert.equal(properties.total, 3);
  assert.deepEqual(properties.untested, ['prp-003']);
  assert.deepEqual(properties.withoutGenerator, ['prp-002']);
  const verdict = realisationVerdict(realisationReport({ claims, sources: SOURCES }));
  assert.ok(verdict.lines.some((l) => /property claims: 3; 1 no test names; 1 named only by tests with no generator: prp-002/.test(l)));
});
