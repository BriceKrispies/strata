// Merge the fundamentals every inventory and the baseline name into one registry.
//
//   node docs/plinth/merge-fundamentals.mjs
//
// Reads docs/plinth/inventory/*.json and the F- ids in docs/plinth/baseline.md, folds aliases into
// canonical capabilities (the CANON table below is the one place that judgement lives), and writes
// docs/plinth/fundamentals.json (machine-readable) and docs/plinth/fundamentals.md (for people).
// Fails when an id is neither canonical nor an alias, so a new inventory cannot add a capability
// unnoticed.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

// theme, canonical id: { title, task, security, aliases }
const CANON = {
  'Authorization and admin': {
    'F-role-definition-authority': { title: 'A change to a role, policy or invoker list cannot grant its maker powers they lack', task: 'WG-5616', security: true, aliases: [] },
    'F-scoped-role-assignment': { title: 'Role assignments and grants narrowed by a scope predicate over the target', task: 'WG-5499', security: true, aliases: ['F-row-scoped-grants'] },
    'F-time-bound-grants': { title: 'Grants and assignments with expiry and status', task: 'WG-5499', security: true, aliases: ['F-time-bound-access'] },
    'F-principal-attributes': { title: 'Principal attributes (region, department) usable in predicates', task: 'WG-5500', security: true, aliases: [] },
    'F-role-composition': { title: 'Role implication and conflicting-role declarations', task: 'WG-5500', security: true, aliases: [] },
    'F-explicit-deny-grants': { title: 'Deny grants that override allows', task: '', security: true, aliases: [] },
    'F-column-masks': { title: 'Field masks on grants', task: '', security: true, aliases: [] },
    'F-separation-of-duties': { title: 'Self-action refusal and separation of duties as platform rules', task: 'WG-5499', security: true, aliases: ['F-self-action-refusal', 'F-separation-of-duties-policy'] },
    'F-last-owner-guard': { title: 'No change on any path leaves a tenant without an administrator', task: 'WG-5486', security: true, aliases: ['F-last-owner-on-every-path'] },
    'F-account-suspension': { title: 'Local account suspension and administrator session revocation', task: 'WG-5490', security: true, aliases: ['F-local-account-suspension'] },
    'F-session-self-management': { title: 'A person lists and revokes their own sessions and agent connections', task: 'WG-5490', security: true, aliases: [] },
    'F-delegated-acting-principal': { title: 'Acting for another principal, recorded on every write, with SoD across the pair', task: 'WG-5499', security: true, aliases: [] },
    'F-identity-admin-capabilities': { title: 'Identity administration (service accounts, tokens, SCIM, keys) gated by capabilities with bounds', task: '', security: true, aliases: ['F-service-account-role-bounds', 'F-capability-scoped-tokens'] },
    'F-platform-admin-role': { title: 'A platform admin role products may name', task: 'WG-5474', security: true, aliases: [] },
    'F-held-operations': { title: 'Held operations and changes: approver rules, named approvers, replay as the requester, expiry', task: 'WG-5496', security: true, aliases: ['F-held-changes', 'F-operation-approval-hold', 'F-held-operation-replay', 'F-proposal-gated-changes', 'F-record-named-approvers', 'F-conditional-approval-decision', 'F-declared-sensitive-operations'] },
    'F-state-conditional-writes': { title: 'Writes and decisions conditional on the state read (one decision when approvers race)', task: 'WG-5488', security: true, aliases: ['F-concurrent-decision-guard'] },
    'F-agent-governance': { title: 'Agent ceiling intersected with the delegator, per-operation agent policy, agent changes held', task: 'WG-5483', security: true, aliases: ['F-agent-ceiling', 'F-agent-action-policy', 'F-agent-change-holds'] },
    'F-untrusted-text-marking': { title: 'Free text in agent-facing results marked untrusted', task: '', security: true, aliases: [] },
    'F-visibility': { title: 'Server-decided visibility per role, row, field and event audience', task: 'WG-5465', security: true, aliases: ['F-visibility-per-role', 'F-audience-scoped-events', 'F-record-scoped-event-readers'] },
    'F-curated-view-sources': { title: 'Curated read-only sources (views) with grants applied', task: '', security: true, aliases: [] },
  },
  'Audit and integrity': {
    'F-authorization-audit': { title: 'Every authorization-relevant event audited and attributed (tokens, delegations, sessions, providers)', task: 'WG-5506', security: true, aliases: ['F-session-end-audit', 'F-provider-change-attribution', 'F-audit-actor-chain'] },
    'F-audit-tamper-evidence': { title: 'Anchored audit chain head so truncation is detected', task: 'WG-5506', security: true, aliases: ['F-audit-chain-head-anchor'] },
    'F-append-only-records': { title: 'Records declared append-only: no update or delete by any path', task: 'WG-5503', security: true, aliases: [] },
    'F-record-history': { title: 'Record revisions, diffs, published revision and audited undo', task: 'WG-5503', security: true, aliases: [] },
  },
  'Ledger': {
    'F-double-entry-journal': { title: 'Balanced multi-leg transactions with a header, and an account hierarchy', task: 'WG-5504', security: true, aliases: ['F-balanced-journal-entries', 'F-account-hierarchy'] },
    'F-ledger-tamper-evidence': { title: 'Chained, anchored ledger journal; postings audited; restore cannot drop entries', task: '', security: true, aliases: ['F-ledger-posting-audit', 'F-ledger-read-access-audit'] },
    'F-ledger-proofs': { title: 'Machine-checked ledger invariants (conservation, append-only, idempotency)', task: '', security: true, aliases: ['F-ledger-machine-checked-invariants'] },
    'F-ledger-lot-expiry': { title: 'Per-grant expiry lots consumed earliest first', task: '', security: true, aliases: [] },
    'F-ledger-facts-in-rules': { title: 'Balance, lifetime total and tier readable by rules, segments and computed fields', task: 'WG-5513', security: true, aliases: ['F-ledger-derived-facts', 'F-ledger-balance-in-rules'] },
    'F-ledger-queries': { title: 'Queries and aggregates over ledger entries and accounts', task: 'WG-5507', security: true, aliases: [] },
    'F-ledger-reconciliation': { title: 'Scheduled whole-ledger trial balance and drift report', task: '', security: true, aliases: ['F-scheduled-reconciliation', 'F-external-reconciliation'] },
    'F-reversal-idempotent-replay': { title: 'A retried reversal replays its first entry', task: '', security: true, aliases: [] },
  },
  'Data and query': {
    'F-cross-type-query': { title: 'One ordered, paged read over several record types', task: '', security: false, aliases: ['F-union-query'] },
    'F-ranked-text-search': { title: 'Ranked text search with per-type totals', task: '', security: false, aliases: [] },
    'F-faceted-listing': { title: 'Facet counts and per-viewer state in listings', task: 'WG-5507', security: false, aliases: [] },
    'F-report-scale': { title: 'Reporting over large sources', task: 'WG-5507', security: false, aliases: [] },
    'F-windowed-aggregates': { title: 'Counts, sums and streaks over time windows', task: 'WG-5503', security: false, aliases: ['F-activity-windows'] },
    'F-computed-fields': { title: 'Derived fields', task: 'WG-5513', security: false, aliases: [] },
    'F-conditional-uniqueness': { title: 'Uniqueness over matching rows only', task: '', security: false, aliases: [] },
    'F-per-owner-limits': { title: 'Per-owner, per-object quantity limits', task: '', security: false, aliases: [] },
    'F-identifier-sequences': { title: 'Gapless human-readable numbers', task: 'WG-5505', security: false, aliases: [] },
    'F-text-derived-references': { title: 'References derived from text at write time (mentions)', task: '', security: false, aliases: [] },
    'F-cross-product-refs': { title: 'Records naming records of other products', task: 'WG-5493', security: false, aliases: [] },
    'F-organisation-structure': { title: 'Groups as organisation units, membership readable by rules', task: 'WG-5500', security: true, aliases: [] },
    'F-segment-membership-in-rules': { title: 'in_segment in expressions and policies', task: '', security: true, aliases: [] },
    'F-recurrence-occurrences': { title: 'Occurrences of a recurrence within a range', task: '', security: false, aliases: [] },
    'F-named-assets': { title: 'Named assets and file domains', task: 'WG-5517', security: false, aliases: [] },
    'F-priced-quote': { title: 'Ranked, stackable, budgeted rule evaluation with a quote fingerprint', task: '', security: false, aliases: [] },
    'F-tenant-export-archive': { title: 'Tenant export archive', task: 'WG-5502', security: false, aliases: [] },
  },
  'Process and automation': {
    'F-human-tasks': { title: 'Human task primitive and inbox', task: 'WG-5496', security: false, aliases: ['F-human-task-primitive'] },
    'F-bounded-fan-out': { title: 'A step repeated over a list or a computed set, bounded', task: 'WG-5495', security: false, aliases: ['F-operation-fan-out'] },
    'F-run-lifecycle-control': { title: 'Run retry, cancel, start paused', task: 'WG-5497', security: false, aliases: [] },
    'F-run-limits-per-subject': { title: 'Per-subject run caps and cooldowns', task: '', security: false, aliases: [] },
    'F-step-error-continuation': { title: 'Continue a run after a failed step', task: '', security: false, aliases: [] },
    'F-event-loop-guard': { title: 'Refuse triggers caused by the automation\'s own writes', task: '', security: false, aliases: [] },
    'F-definition-dry-run': { title: 'Dry run of a whole definition with no writes', task: '', security: true, aliases: [] },
    'F-threshold-triggers': { title: 'Triggers on a query crossing a threshold', task: 'WG-5607', security: false, aliases: [] },
    'F-scheduled-record-events': { title: 'Timed events read from record fields', task: '', security: false, aliases: [] },
    'F-webhook-outcome-to-run': { title: 'Webhook delivery outcome recorded on the run', task: 'WG-5489', security: false, aliases: [] },
    'F-threaded-messages': { title: 'Message threads an automation can write to', task: '', security: false, aliases: [] },
    'F-sandbox-tenant-lifecycle': { title: 'Sandbox tenants: create, reset, discard', task: '', security: false, aliases: [] },
  },
  'Realtime and UI': {
    'F-live-aggregate-counts': { title: 'Per-principal counts pushed after changes', task: '', security: false, aliases: [] },
    'F-compositions': { title: 'Pages composed from placed widgets', task: 'WG-5464', security: false, aliases: ['F-screen-composition'] },
    'F-composite-patterns': { title: 'Composite patterns and a widget event bus', task: 'WG-5479', security: false, aliases: ['F-composite-widget-patterns', 'F-widget-event-bus'] },
    'F-charts-and-boards': { title: 'Charts, boards, KPIs and time grids', task: 'WG-5480', security: false, aliases: ['F-time-grid-view'] },
    'F-keyboard-gestures': { title: 'Keyboard paths for every gesture', task: 'WG-5481', security: false, aliases: ['F-gesture-keyboard-paths'] },
    'F-app-shell-navigation': { title: 'Permission-filtered app shell and navigation', task: 'WG-5468', security: true, aliases: [] },
    'F-rendering-contract-versions': { title: 'Versioned rendering contract', task: 'WG-5478', security: false, aliases: [] },
    'F-conversational-interface': { title: 'A conversational interface over the agent surface', task: '', security: false, aliases: [] },
  },
  'Accessibility': {
    'F-accessibility-contract': { title: 'Accessible by construction: decided rule, definition-time checks, accessible primitives, alt text, live announcements', task: 'WG-5473', security: false, aliases: ['F-accessible-by-construction', 'F-definition-time-a11y-checks', 'F-accessible-primitive-library', 'F-image-text-alternative', 'F-live-status-announcements'] },
    'F-accessibility-gate': { title: 'Axe, keyboard and contrast checks in the gate', task: 'WG-5473', security: false, aliases: ['F-browser-accessibility-gate', 'F-contrast-verification-in-gate'] },
    'F-request-language-and-direction': { title: 'Request language and text direction', task: '', security: false, aliases: [] },
  },
};

const canonOf = new Map();
const entries = new Map();
for (const [theme, items] of Object.entries(CANON)) {
  for (const [id, c] of Object.entries(items)) {
    entries.set(id, { id, theme, ...c, neededBy: [], groups: new Set(), named: new Set([id]) });
    canonOf.set(id, id);
    for (const a of c.aliases) canonOf.set(a, id);
  }
}

const unknown = [];
const invDir = join(here, 'inventory');
for (const f of readdirSync(invDir).filter((x) => x.endsWith('.json'))) {
  const doc = JSON.parse(readFileSync(join(invDir, f), 'utf8'));
  const group = f.replace(/\.json$/, '');
  for (const x of doc.fundamentals) {
    const id = canonOf.get(x.id);
    if (!id) { unknown.push(`${group}: ${x.id}`); continue; }
    const e = entries.get(id);
    e.groups.add(group); e.named.add(x.id);
    e.neededBy.push(...x.neededBy.map((n) => `${group}:${n}`));
    if (x.security) e.security = true;
  }
}
for (const m of readFileSync(join(here, 'baseline.md'), 'utf8').matchAll(/F-[a-z0-9-]+/g)) {
  const id = canonOf.get(m[0]);
  if (!id) { unknown.push(`baseline: ${m[0]}`); continue; }
  entries.get(id).groups.add('baseline'); entries.get(id).named.add(m[0]);
}
if (unknown.length) {
  console.log(`ids that are neither canonical nor an alias (add them to CANON):\n  ${[...new Set(unknown)].join('\n  ')}`);
  process.exit(1);
}

const list = [...entries.values()].map((e) => ({
  id: e.id, theme: e.theme, title: e.title, task: e.task, security: e.security,
  namedAs: [...e.named].sort(), sources: [...e.groups].sort(), neededBy: [...new Set(e.neededBy)].sort(),
}));
writeFileSync(join(here, 'fundamentals.json'), JSON.stringify({ generatedBy: 'docs/plinth/merge-fundamentals.mjs', fundamentals: list }, null, 2) + '\n');

let md = '# Fundamentals the Plinth port needs\n\nGenerated by `docs/plinth/merge-fundamentals.mjs` from the inventories and `baseline.md`; do not edit by hand.\n';
md += `${list.length} capabilities, ${list.filter((x) => x.security).length} security-relevant, ${list.filter((x) => !x.task).length} without an existing task.\n`;
for (const theme of Object.keys(CANON)) {
  const rows = list.filter((x) => x.theme === theme).sort((a, b) => b.neededBy.length - a.neededBy.length);
  md += `\n## ${theme}\n\n| Fundamental | Capability | Task | Security | Items needing it | Found by |\n|---|---|---|---|---|---|\n`;
  for (const r of rows) md += `| ${r.id} | ${r.title} | ${r.task || 'new'} | ${r.security ? 'yes' : ''} | ${r.neededBy.length} | ${r.sources.join(', ')} |\n`;
}
writeFileSync(join(here, 'fundamentals.md'), md);
console.log(`${list.length} fundamentals (${list.filter((x) => x.security).length} security, ${list.filter((x) => !x.task).length} new) written to fundamentals.json and fundamentals.md`);
