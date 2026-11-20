// Every contract that is shipped and names a bffRoute has a generated conformance test (WG-5182).
//
// `specc conformance` writes crates/bff/tests/conformance/generated/contracts.rs, one test per such
// contract, each of which drives the contract through the real BFF and backend; the file lists the
// contracts it covers (`COVERED`) and the checks it cannot make with their reasons (`SKIPPED`).
// This module reads those two lists from the generated text; nothing is compiled. A shipped contract
// with a route that the file does not cover fails, so does a covered one that is no longer shipped
// (the file is stale), and the checks not made are counted by check and printed.

export const GENERATED = 'crates/bff/tests/conformance/generated/contracts.rs';

// The Rust string literal `literal` (with its quotes) as text; an escape JSON does not know is kept.
function literalText(literal) {
  try {
    return JSON.parse(literal);
  } catch {
    return literal.slice(1, -1);
  }
}

const STRING = '"(?:[^"\\\\]|\\\\.)*"';

// The body of `pub const <name>: ... = &[ ... ];`, or null when the file has no such constant.
function constBody(text, name) {
  const m = String(text ?? '').match(new RegExp(`pub const ${name}:[^=]*=\\s*&\\[([\\s\\S]*?)\\];`));
  return m ? m[1] : null;
}

// The strings of a `&[&str]` constant, in order; null when the constant is missing.
export function stringList(text, name) {
  const body = constBody(text, name);
  if (body === null) return null;
  return [...body.matchAll(new RegExp(STRING, 'g'))].map((m) => literalText(m[0]));
}

// The (contract, check, reason) tuples of SKIPPED, in order; null when the constant is missing.
export function skippedList(text) {
  const body = constBody(text, 'SKIPPED');
  if (body === null) return null;
  const tuple = new RegExp(`\\(\\s*(${STRING})\\s*,\\s*(${STRING})\\s*,\\s*(${STRING})\\s*,?\\s*\\)`, 'g');
  return [...body.matchAll(tuple)].map((m) => ({ contract: literalText(m[1]), check: literalText(m[2]), reason: literalText(m[3]) }));
}

// The contracts that must have a test: shipped, with a bffRoute.
export function routedShipped(contracts) {
  return (contracts ?? []).filter((c) => c.deployment === 'shipped' && typeof c.bffRoute === 'string').map((c) => c.id);
}

// The coverage of the generated suite: { shipped, covered, missing, stale, skipped, counts, problem }.
// `problem` is set when the generated file cannot be read as a suite.
export function conformanceCoverage({ contracts, generated }) {
  const shipped = routedShipped(contracts);
  if (generated === null || generated === undefined) {
    return { shipped, covered: [], missing: shipped, stale: [], skipped: [], counts: {}, problem: `${GENERATED} does not exist; run npm run generate` };
  }
  const covered = stringList(generated, 'COVERED');
  const skipped = skippedList(generated);
  if (covered === null || skipped === null) {
    return { shipped, covered: [], missing: shipped, stale: [], skipped: [], counts: {}, problem: `${GENERATED} has no COVERED or SKIPPED list` };
  }
  const coveredSet = new Set(covered);
  const shippedSet = new Set(shipped);
  const counts = {};
  for (const s of skipped) counts[s.check] = (counts[s.check] ?? 0) + 1;
  return {
    shipped,
    covered,
    missing: shipped.filter((id) => !coveredSet.has(id)),
    stale: covered.filter((id) => !shippedSet.has(id)),
    skipped,
    counts,
    problem: null,
  };
}

// The gate's verdict: { name, ok, detail, lines }.
export function conformanceVerdict(coverage) {
  const name = 'every shipped contract with a bffRoute has a generated conformance test (specc conformance)';
  const countText = Object.entries(coverage.counts)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([check, n]) => `${check} ${n}`)
    .join(', ');
  const lines = [
    ...(coverage.problem ? [coverage.problem] : []),
    ...coverage.missing.map((id) => `missing: ${id}`),
    ...coverage.stale.map((id) => `stale (no longer shipped with a route): ${id}`),
    ...coverage.skipped.map((s) => `not checked: ${s.contract} ${s.check}: ${s.reason}`),
  ];
  const ok = coverage.problem === null && coverage.missing.length === 0 && coverage.stale.length === 0;
  return {
    name,
    ok,
    detail: `${coverage.shipped.length - coverage.missing.length} of ${coverage.shipped.length} covered; ${coverage.skipped.length} checks not made${countText ? ` (${countText})` : ''}`,
    lines,
  };
}
