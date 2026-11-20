// Every contract whose `deployment` is `shipped` has a handler (WG-5038, from audit finding 4 of
// reports/drift.md: a contract says "shipped" and no code answers it).
//
// A handler is one of:
//   - an `Implements: <ref>` mark in hand-written Rust that lists the contract's `ref` (comma
//     separated, as in `Implements: H-034, IDN-006`);
//   - a route: the contract's `bffRoute` is mounted by the BFF, or the contract's path
//     (`binding.path`, else `/<implementedBy>/<id>`) is a string literal of hand-written Rust;
//   - a function: the contract is derived and names a function of the specification that exists
//     (`derivation.function`, `decide`, `load`, `needs`), or hand-written Rust has `fn <id>` with
//     the id in snake case.
// Generated code (crates/model, crates/model-core) is not hand-written and is never a source.

import { servesRoute, parseBffRoute } from './gate-bff-routes.mjs';

const REFERENCE = /^[A-Z]+-\d+$/;

// The references that `Implements:` marks list, from sources [{ path, text }].
export function implementsMarks(sources) {
  const refs = new Set();
  for (const { text } of sources) {
    for (const line of text.matchAll(/Implements:[ \t]*([^\n]*)/g)) {
      for (const ref of line[1].split(/[,\s]+/)) if (REFERENCE.test(ref)) refs.add(ref);
    }
  }
  return refs;
}

export function snakeCase(id) {
  return id.replace(/-/g, '_');
}

const DERIVATION_FUNCTIONS = ['function', 'decide', 'load', 'needs'];

// How the contract is handled: the kind of handler found, or null.
export function handlerOf(contract, context) {
  if (contract.ref && context.marks.has(contract.ref)) return 'implements mark';
  const named = parseBffRoute(contract.bffRoute);
  if (named && context.mounted.some((route) => servesRoute(route, named))) return 'bff route';
  const path = contract.binding?.path ?? (contract.implementedBy ? `/${contract.implementedBy}/${contract.id}` : null);
  if (path && context.literals.has(path)) return 'route';
  if (DERIVATION_FUNCTIONS.some((key) => context.functions.has(contract.derivation?.[key]))) return 'specification function';
  if (context.fns.has(snakeCase(contract.id))) return 'function';
  return null;
}

// The ids of the shipped contracts without a handler, and how many are shipped.
export function shippedWithoutHandler({ contracts, functions, sources, mounted }) {
  const context = {
    marks: implementsMarks(sources),
    mounted,
    functions: new Set(functions.map((f) => f.id)),
    literals: new Set(sources.flatMap(({ text }) => [...text.matchAll(/"([^"\n]*)"/g)].map((m) => m[1]))),
    fns: new Set(sources.flatMap(({ text }) => [...text.matchAll(/\bfn\s+([A-Za-z_][A-Za-z_0-9]*)/g)].map((m) => m[1]))),
  };
  const shipped = contracts.filter((c) => c.deployment === 'shipped');
  return { shipped: shipped.length, missing: shipped.filter((c) => handlerOf(c, context) === null).map((c) => c.id) };
}
