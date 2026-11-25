# Gate scopes

The specification gate holds a requirement when its milestone is started (`gate.json`, `startedMilestones`). A scope
holds requirements a second way: by ref prefix and by ref, with milestones it leaves out. The owner can therefore gate
the security, ledger and accessibility requirements without starting the milestones they sit in.

## Format

`gate.json` takes an optional `startedScopes` array. Each entry is an object:

```json
{
  "startedMilestones": [1],
  "startedScopes": [
    { "name": "ledger", "prefixes": ["LDG-"], "refs": ["ADM-1088"], "excludeMilestones": [13] }
  ]
}
```

- `name` is required, unique, and printed in the gate's summary line.
- `prefixes` and `refs` are optional, but a scope needs at least one of them. A requirement is held when its ref starts
  with a prefix or equals a listed ref. Matching ignores case.
- `excludeMilestones` is optional. A requirement in a listed milestone is not held by that scope, even if a prefix or ref
  names it.

A held requirement needs a passing proof exactly as a started milestone's does. The evidence check reports the scopes in
its summary line (for example "and scopes ledger, security"), and each scope's matched, proven and unproven counts.

The check fails, with a message, when a scope has an unknown key, has no name or a repeated name, has neither prefixes
nor refs, names a prefix that matches no requirement, or names a ref that is not a requirement of the specification.

`docs/plinth/gate-scopes.json` holds proposed scopes in the same shape, under a `scopes` key. They are not started.

## Proposed scopes

Each scope excludes milestone 13. The counts are from `node scripts/gate-scope-report.mjs` run on this worktree. No test
results are recorded here (`var/test-results` is absent), so every matched requirement is unproven and the proven
counts are zero by construction. The matched counts are the figures that do not depend on results.

| Scope | Holds | Matched | Proven | Unproven |
|---|---|---|---|---|
| `ledger` | prefix `LDG-` | 63 | 0 | 63 |
| `security` | prefixes `IDN-`, `FED-`, `TOK-`, `ADM-`, `TEN-`, `SEC-`, `THR-`, `PRV-`; refs for the audit of authorization (below) | 427 | 0 | 427 |
| `a11y` | refs listed in the file (below) | 42 | 0 | 42 |

**ledger** is every `LDG-` requirement outside milestone 13. Milestone 13 holds 70 ledger refs, which are left out.

**security** covers identity (`IDN-`, `FED-`, `TOK-`), administration and authorization (`ADM-`, `TEN-`), keys and
credentials (`SEC-`), threats (`THR-`) and privacy (`PRV-`). The audit and observability of authorization is added by ref:
the Audit section of `OBS-` (`OBS-006` to `OBS-008`, `OBS-1215` to `OBS-1222`, `OBS-3121` to `OBS-3123`); the refusal of
a configuration change, a credential and a platform operation (`OPS-329`, `OPS-3120`, `OPS-3128`); the audit of the
backend's authentication and diagnostics (`RUN-1380`, `RUN-3070`, `RUN-3071`); and the agent limits on authority and
audit queries (`AGS-413`, `AGS-425`, `AGT-412`, `AGT-413`, `AGT-1209`). The `OBS-` telemetry refs (`OBS-001` to
`OBS-005`) are not in it. The list follows `docs/plinth/baseline.md`, which audited these areas.

**a11y** holds the accessibility refs:

- `IDN-023` (sign-in pages meet WCAG 2.2 AA; milestone 3) and `CMS-012` (the authoring application meets WCAG 2.2 AA;
  milestone 11).
- `CNV-237` to `CNV-241`, `CNV-243`, `CNV-244` and `CNV-246` (the conversation's keyboard, live region, motion, target size,
  reflow and language rules; milestones 9 and 11). `CNV-242` and `CNV-245` are network rules and are left out.
- `DS-005` and `DS-006` (WCAG 2.2 AA contrast of the theme model; milestone 7).
- `DS-024` to `DS-053` (the design system's Accessibility and Accessible primitives sections; milestone 7).

The `DS-062` to `DS-075` widget rules that touch keyboard and focus are in milestone 13 and are not in this scope.

**Correction to the brief.** The brief said the accessibility refs are all in milestone 13. They are not. `DS-024` to
`DS-053` are in milestone 7, `IDN-023` in milestone 3, `CNV-237` to `CNV-246` in milestones 9 and 11, and `CMS-012` in
milestone 11. None of the refs in this scope is in milestone 13. The milestone 13 widget rules that touch keyboard and focus
(`DS-062` to `DS-075`) are left out of it. Most of the
milestone 7 statements describe the generated renderer and its primitives. `docs/plinth/baseline.md` (section 3)
records that those primitives are not built and that no `ui.a11y.*` check exists, so a test for them needs that work
first. The axe and keyboard checks run only under `--browser`.

## What is started in gate.json

`gate.json` has `"startedScopes": []`. None is started. A scope can be started only when every requirement it holds is
proven, because a started scope fails the gate on each unproven requirement. Against the recorded results of this
worktree, none of the three scopes has a proven requirement. The baseline records tags on some refs (for example
LDG 40 of 63 by grep), but a tag proves nothing until the test has passed in a recorded run.

## Backfilling

Run `node scripts/gate-scope-report.mjs --scope NAME` after a test run. It prints the matched, proven and unproven
counts, then the unproven refs grouped by area and section. For each group it lists the test files that already carry
`Verifies:` lines for refs with the same prefix, which is where a new test for the group's refs belongs. Add `--json` for
the same data as JSON. Without recorded results the report says so and still prints the matched sets.
