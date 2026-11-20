// Lists the contracts whose authorisation prose (auth.description) is not settled by the structured
// auth beside it, so that a person reviews them (WG-4908). Reads specifications/spec.jsonc and prints
// one line per contract, "<id> <ref>: <reasons>", sorted by contract id, then the count.
// Usage: node scripts/list-ambiguous-auth.mjs [spec path] [--json]
import fs from 'node:fs';

const args = process.argv.slice(2);
const json = args.includes('--json');
const specPath = args.find((a) => !a.startsWith('--')) ?? 'specifications/spec.jsonc';
const text = fs.readFileSync(specPath, 'utf8').replace(/^\uFEFF/, '').replace(/^\s*\/\/.*$/gm, '');
const spec = JSON.parse(text);

// Each test returns a reason, or null when the structured auth and the prose agree.
const tests = [
  // The prose names more than one kind of caller, so one scheme cannot be the whole truth.
  (c, a, p) => (/\b(either|or a|or an|or the|, or |and holders|for a sign-in)\b/i.test(p) && !/\bor the tenant\b|or an owner of the tenant/i.test(p) && !/ or more\b/.test(p) ? 'the prose offers more than one kind of caller' : null),
  // The prose says anyone may call, and the scheme is not anonymous.
  (c, a, p) => (/^anyone\b/i.test(p) && a.scheme !== 'anonymous' ? `the prose says anyone may call and the scheme is ${a.scheme}` : null),
  // The scheme is anonymous, and the prose names a signed-in caller.
  (c, a, p) => (a.scheme === 'anonymous' && /signed-in/i.test(p) ? 'the scheme is anonymous and the prose names a signed-in caller' : null),
  // A caller that is not anonymous or in-process, with neither a required capability nor a role.
  (c, a) => (!['anonymous', 'in-process'].includes(a.scheme) && !(a.requires?.length) && !(a.roles?.length) ? `the scheme is ${a.scheme} and no capability or role is required` : null),
  // The prose says policies decide, and the structured auth does not say that tenant policies apply.
  (c, a, p) => (/\bpolicies\b/i.test(p) && !a.tenantPolicies ? 'the prose says policies decide and tenantPolicies is not set' : null),
  // The prose names a role that the roles list does not hold.
  (c, a, p) => (/\bowner\b/i.test(p) && !/signed-in owner of the site|operator/i.test(p) && !(a.roles ?? []).includes('owner') ? 'the prose names the owner and roles does not' : null),
  // A browser session of a public contract that changes something, with no request token or origin check.
  (c, a) => (c.exposure === 'public' && a.scheme === 'session' && !c.readOnly && !(a.csrf && a.origin) ? 'a public session contract that changes state lacks csrf or origin' : null),
  // A public anonymous contract that changes state and gives no justification.
  (c, a) => (c.exposure === 'public' && a.scheme === 'anonymous' && !c.readOnly && !a.justification ? 'an anonymous public contract that changes state gives no justification' : null),
];

const rows = [];
for (const c of spec.contracts ?? []) {
  const a = c.auth ?? {};
  const p = a.description ?? '';
  const reasons = tests.map((t) => t(c, a, p)).filter(Boolean);
  if (reasons.length) rows.push({ id: c.id, ref: c.ref, reasons });
}
rows.sort((x, y) => (x.id < y.id ? -1 : 1));
if (json) console.log(JSON.stringify(rows, null, 1));
else {
  for (const r of rows) console.log(`${r.id} ${r.ref}: ${r.reasons.join('; ')}`);
  console.log(`${rows.length} of ${(spec.contracts ?? []).length} contracts`);
}
