# Strata tasks (Workgraph export)

720 tasks linked to the strata repository or tagged strata. Full records: `tasks.json`.

| Status | Count |
|---|---:|
| ready | 25 |
| blocked | 1 |
| proposed | 56 |
| backlog | 83 |
| accepted | 123 |
| cancelled | 432 |

## ready (25)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5433 | high | Field-level read permissions and opaque principal ids |  |
| WG-5432 | high | Gate green: RustSec advisories, remaining gate failures, generated-code formatting |  |
| WG-5431 | medium | End-to-end journeys and real-stack tests |  |
| WG-5430 | high | Simulator fidelity: faults, delivery guarantees and machine trace parity |  |
| WG-5429 | medium | Cloud deployment: control plane, replica state, edge protection, mail reputation, orchestration |  |
| WG-5428 | high | Identity follow-ups: provider kinds behind one trait, verified address, SAML verification |  |
| WG-5427 | medium | Workflow engine correctness |  |
| WG-5426 | medium | Reads and queries: filter pushdown, pair-page bounds, search in views, shared copy store |  |
| WG-5425 | high | Request edge and generated routes agree with the backend |  |
| WG-5424 | high | Schema evolution in the running system: persisted pins, rebase, SplitField, history generator and verified checker |  |
| WG-5423 | medium | Enterprise-size installs and scale evidence |  |
| WG-5422 | high | Web client and tenant UI: screens for shipped features, schema-driven lists and forms, live client control |  |
| WG-5421 | high | Plan quotas, enterprise caps and connection admission |  |
| WG-5420 | high | Privacy and erasure: restriction, legal hold, retention, subject export, sealing, keys and restore |  |
| WG-5419 | high | Files: delivery and acceptance safety, domain events, tenant limits and the erasure receipt |  |
| WG-5418 | high | Messaging: notification service, mail dispatcher and webhooks completed |  |
| WG-5417 | high | Rule language reads, settings and ledger: balance, in_segment, setting, setting types, reader-visible balance |  |
| WG-5416 | high | Exegra and specc: refusal rules, checker gaps and verifier quality |  |
| WG-5412 | high | Strata: hold the full test suite to a strict 10-minute budget in the gate; fold or remove tests to fit |  |
| WG-5391 | medium | Resident store: ResidentStore below sealing, invalidation map, set-valued entries, specification changes | WG-5385 |
| WG-5384 | medium | Requirements rewrite for planned areas: each area brought under the check rule when its milestone starts or its area is rewritten | WG-5372 |
| WG-5383 | medium | Requirements rewrite: started STO, RUN, GWY, SIM and CMS requirements restated with real checks | WG-5372 |
| WG-5382 | high | Requirements rewrite: kernel (KRN) held to the Lean differential and literal-valued tests | WG-5372 |
| WG-5380 | medium | Agents implement behaviour against the generated boundary, with separated roles and mechanical architecture rules | WG-5372 |
| WG-5373 | high | Kernel: make the Lean model the single reference model of the kernel's decision surface | WG-5372 |

## blocked (1)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5397 | medium | Working-set evidence: simulator fault schedule, Lean differential check, sampled read trace, experiments | WG-5385 |

## proposed (56)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5679 | high | Personal data erasure as a first-class kernel primitive (redesign before WG-5673 lands) | WG-5648 |
| WG-5678 | high | Tenant-script sandbox: isolated worker, WebAssembly host functions, fuel, validator | WG-5648 |
| WG-5677 | high | No XSS: proved DOM-operation vocabulary, sink-free adapter check, browser fuzzing | WG-5654 |
| WG-5676 | high | Browser kernel spec: the tenant-script boundary as an untrusted oracle | WG-5654 |
| WG-5675 | high | Spec: tenant scripts in the browser sandbox (narrow NG-3 and D-506 to the server) | WG-5648 |
| WG-5674 | medium | Strata: realm-spray throttle test is timing-sensitive and failed once in CI |  |
| WG-5673 | medium | Kernel spec slice: erasure and retention | WG-5373 |
| WG-5672 | medium | Kernel spec slice: segments | WG-5373 |
| WG-5671 | medium | Kernel spec slice: triggers and automations | WG-5373 |
| WG-5670 | medium | Kernel spec slice: operations | WG-5373 |
| WG-5669 | medium | Kernel spec slice: definitions by schema change, read declarations in the tenant, link to Strata.Evolution | WG-5373 |
| WG-5668 | medium | Kernel spec slice: ledger (finish): floor/cap and balance theorems, PostLedgerTransaction, expiry account order | WG-5373 |
| WG-5667 | medium | Kernel: a lax schema change can leave dangling references (retype or removal under Grandfather) |  |
| WG-5666 | high | Kernel: a workflow transition's assignments can move or clear another workflow field without its transition |  |
| WG-5665 | high | Kernel: a failing expression default before the permission check reveals that an object type exists |  |
| WG-5664 | high | Kernel: a delete clears a referrer's reference without re-checking the referrer's constraints |  |
| WG-5663 | high | Browser kernel: former person's view survives a session change (found by the B1 Lean spec) |  |
| WG-5662 | medium | A4: kernel and protocol requirements point to the Lean theorems that state them | WG-5648 |
| WG-5661 | medium | C3: backend host steps as proved Lean functions behind thin trusted I/O shells, with a SQLite specification | WG-5648 |
| WG-5660 | medium | C4: machine-checked reduction for concurrent optimistic transactions and host steps | WG-5648 |
| WG-5659 | medium | B2: Lean browser kernel proved against its spec, compiled to WebAssembly and run in the worker | WG-5648 |
| WG-5658 | medium | A3: the backend runs the compiled Lean kernel; the Rust kernel's decision code is deleted | WG-5648 |
| WG-5657 | high | Strata: accept agent tokens on the request path so agent governance applies to real calls | WG-5610 |
| WG-5656 | medium | C2: TLA-style liveness library in Lean | WG-5648 |
| WG-5655 | medium | C1: Lean system spec and backend protocol hosts, with the refinement proof | WG-5648 |
| WG-5654 | medium | B1: Lean spec of the browser kernel (update and view) | WG-5648 |
| WG-5653 | medium | A-codec: verified canonical byte codec for kernel values across the Lean/Rust boundary | WG-5648 |
| WG-5652 | medium | A2: executable Lean kernel proved to refine the Lean kernel spec | WG-5648 |
| WG-5651 | medium | A0: differential random testing of the Rust kernel against the Lean model, at scale | WG-5648 |
| WG-5650 | high | S2: spike - a compiled Lean program as WebAssembly in the browser worker | WG-5648 |
| WG-5649 | high | S1: spike - compiled Lean called from the Rust backend through a C ABI | WG-5648 |
| WG-5648 | high | Strata formal verification: IronFleet method in Lean (kernel, browser kernel, backend) |  |
| WG-5641 | medium | Strata: outbox publish costs 130-150 ms per committed step | WG-5610 |
| WG-5635 | medium | Strata: list-jobs reply carries fields its contract does not declare |  |
| WG-5620 | high | Strata: model and prove that no principal gains a power through a change it made (ADM-1088) | WG-5610 |
| WG-5610 | high | Strata: port Plinth Suite's features onto Strata fundamentals, spec first, enterprise grade |  |
| WG-5609 | high | Strata: browser tests failing for causes predating 2026-11-24 (editor save toast, first paint, reconnect hook, Impact Lab, Lean proofs, assign-role reply, domain events) |  |
| WG-5608 | high | Exegra: shared kernel case table and fixture language in step with Strata (ancestor-of, graphemes, is-emoji) |  |
| WG-5607 | medium | Schedule refresh-computed for time-dependent computed fields | WG-5513 |
| WG-5606 | medium | Strata: an operation step that writes a setting (person level for the invoker, tenant level for admins) | WG-5595 |
| WG-5605 | medium | Strata: order-dependent flakiness in the site browser suite (mail sign-in, invitation, identity provider, service account, team roles) |  |
| WG-5604 | high | Strata: a guarded page visited signed out shows 'could not be loaded' instead of the sign-in prompt (13 browser tests red) |  |
| WG-5603 | high | Exegra: graphemes and is-emoji primitives for the tenant rule language |  |
| WG-5602 | medium | Strata design system: comment-thread, reaction-bar, emoji picker and reply disclosure | WG-5594 |
| WG-5601 | medium | Strata settings: the generated Settings screen by area, and a setting binding kind | WG-5595 |
| WG-5600 | medium | Strata settings: area, description and allowed values; definitions route; person writes from the client; live refresh | WG-5595 |
| WG-5599 | medium | Strata: reader-relative row marks and joining one section's aggregate onto another's rows | WG-5594 |
| WG-5598 | medium | Strata: ledger reversal, and limits that count only additions | WG-5594 |
| WG-5597 | medium | Strata: text rules over graphemes, an emoji predicate and normalisation on write | WG-5594 |
| WG-5596 | medium | Strata: operation steps that find a record by its fields and branch on what they find | WG-5594 |
| WG-5595 | medium | Strata investigation: end-user settings per area of the system | WG-5457 |
| WG-5594 | medium | Strata investigation: threaded replies and any-emoji reactions from platform fundamentals | WG-5457 |
| WG-5593 | high | Strata: regenerate and the spec gate hash the main checkout's specification from any worktree |  |
| WG-5592 | high | Strata design system: no pills; state said in words where it departs from normal | WG-5457 |
| WG-5591 | high | Strata: cargo audit fails on integration with five RustSec advisories |  |
| WG-5590 | high | Strata design system visual slice: the showcase tenant before and after | WG-5457 |

## backlog (83)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5517 | medium | Named assets, URL values and tenant file domains |  |
| WG-5516 | low | Strata design system 27: cross-product draft sets | WG-5457 |
| WG-5514 | medium | Strata: data migrations as platform jobs in bounded batches, invisible to triggers | WG-5457 |
| WG-5513 | low | Strata: kernel-computed fields, recomputed on write and on the clock | WG-5457 |
| WG-5512 | medium | Strata design system 26: process timeline pattern | WG-5457 |
| WG-5511 | medium | Strata design system 25: task inbox, decision form and the decide action | WG-5457 |
| WG-5510 | low | Hierarchies and recursion in rules and queries | WG-5484 |
| WG-5509 | low | Data subjects without a login | WG-5484 |
| WG-5508 | low | Document generation and e-signature | WG-5484 |
| WG-5507 | low | Query and report scale beyond the 10,000-record scan bound | WG-5484 |
| WG-5506 | low | Compliance surface: read-access audit, retention range and holds, anchored audit | WG-5484 |
| WG-5505 | medium | Identifier sequences for cases, invoices and numbers | WG-5484 |
| WG-5504 | low | Double-entry journals and chart of accounts on the ledger | WG-5484 |
| WG-5503 | low | Record history, as-of reads and effective dating | WG-5484 |
| WG-5502 | medium | Bulk data: import, tenant export, event feed, replay and reconciliation | WG-5484 |
| WG-5501 | medium | Integration connectors: outbound requests with responses, inbound parsing, ingestion channels | WG-5484 |
| WG-5500 | medium | Organisation structure: runtime teams, role implication, principal attributes, conflicting roles | WG-5484 |
| WG-5499 | medium | Access lifecycle: time-bound grants, role steps, delegation, scoped admin, break-glass | WG-5484 |
| WG-5498 | medium | Business calendars and pausable SLA clocks | WG-5484 |
| WG-5497 | medium | Run lifecycle control: cancel, suspend, resume, repair, reopen, revoke | WG-5484 |
| WG-5496 | medium | Human-task primitive and task inbox | WG-5484 |
| WG-5495 | medium | Process structure: parallel branches, joins, quorum, multi-instance, reactive waits, call activity | WG-5484 |
| WG-5494 | high | Exact numbers and civil time in the value algebra | WG-5484 |
| WG-5493 | high | Cross-product references and tenant-wide roles | WG-5484 |
| WG-5492 | high | Automations and operations can read and act on case data | WG-5484 |
| WG-5491 | low | Inbound webhook: a repeated event id with a different body is reported, not absorbed | WG-5484 |
| WG-5490 | medium | Local account suspension and administrator session revocation | WG-5484 |
| WG-5489 | medium | A call-webhook step's real delivery outcome must reach the run | WG-5484 |
| WG-5488 | medium | Concurrent approval decisions: the decision write must be conditional on the state read | WG-5484 |
| WG-5487 | medium | Fan-out member key must include the step-visit sequence | WG-5484 |
| WG-5486 | high | SCIM must not disable or delete the last owner | WG-5484 |
| WG-5485 | high | Restore must not re-fire outbound work: quiesce restored mail, webhook and outbox jobs | WG-5484 |
| WG-5484 | high | Enterprise scenario sweep: what an arbitrary enterprise client needs (long branching workflows, organisation and access, data/time/compliance, integrations and failure, industry stress tests), gap-analysed against Strata |  |
| WG-5483 | medium | Strata design system 24: the agent surface: screens, bindings, shapes, policy read, page-model and screenshot tool | WG-5457 |
| WG-5482 | medium | Strata design system 23: provenance and structured diagnostics with a convergence contract | WG-5457 |
| WG-5481 | medium | Strata design system 22: gestures, wizards and pending state | WG-5457 |
| WG-5480 | medium | Strata design system 21: structured bindings, charts, boards, time grids, planes, record-defined forms, shared filters | WG-5457 |
| WG-5479 | medium | Strata design system 20: composite patterns and templates as data, platform blocks, renderer registry, catalogue gaps | WG-5457 |
| WG-5478 | medium | Strata design system 19: rendering contract versions, golden outputs, corpus replay and version skew | WG-5457 |
| WG-5477 | medium | Strata design system 18: localisable strings, direction, the logical-property lint, RTL and CJK coverage | WG-5457 |
| WG-5475 | medium | Strata design system 17: the showcase re-expressed and built by an agent script (acceptance target) | WG-5457 |
| WG-5474 | medium | Strata: a platform admin role products may name (showcase gap 2, backend half) | WG-5457 |
| WG-5473 | medium | Strata design system 16: gallery, screenshot regression, axe and keyboard checks | WG-5457 |
| WG-5472 | medium | Strata design system 15: staged installs over the 200-change cap (showcase gap 13) | WG-5457 |
| WG-5471 | medium | Strata design system 14: agent loop platform: compile, structured diagnostics, draft upgrades, step labels | WG-5457 |
| WG-5470 | medium | Strata design system 13: preview as a set of roles | WG-5457 |
| WG-5469 | medium | Strata design system 12: tenant layout overlays and the layout editor | WG-5457 |
| WG-5468 | medium | Strata design system 11: app shell, navigation and the notification bell | WG-5457 |
| WG-5467 | medium | Strata design system 10: the interpreter and pattern renderers | WG-5457 |
| WG-5466 | medium | Strata design system 9: action affordances from a shared preflight, row arguments, prefill, reference coercion | WG-5457 |
| WG-5465 | medium | Strata design system 8: visibility at every level, per-class manifests and parts, cache rules | WG-5457 |
| WG-5464 | medium | Strata design system 7: compositions, placeholders, drafts and publishing checked by the kernel | WG-5457 |
| WG-5463 | medium | Strata design system 6: section shapes in parts, the people table and reference titles | WG-5457 |
| WG-5462 | medium | Strata design system 5: display types and presentation elements | WG-5457 |
| WG-5461 | medium | Strata design system 4: primitives as class contracts and behavioural custom elements | WG-5457 |
| WG-5460 | medium | Strata design system 3: theme seeds, derivation, contrast refusal and CSSOM delivery | WG-5457 |
| WG-5459 | medium | Strata design system 2: the UI catalogue format, generator and compatibility check | WG-5457 |
| WG-5458 | high | Strata design system 1: token layer and preset themes as catalogue data | WG-5457 |
| WG-5457 | high | Strata design system and agentic UI composition: tokens, primitives, patterns, templates, display types, tenant layouts, wireframes, and an agent build loop the tenant can watch |  |
| WG-5454 | medium | Experiment: a feature-rich showcase tenant built only from platform capabilities (roles, admin panel, profiles, pages with widgets, layout editor, notifications, message center, badges with admin rules), seeded in SQLite |  |
| WG-5450 | medium | Strata frontend 17: the gate runs split-spec --verify; the Rust reader is tested on area parts | WG-5415 |
| WG-5449 | medium | Strata frontend 16: amend PRV-351, PRV-352, PRV-353, CPY-013 and permission-change requirements for browser copies (owner decision D-FE-11) | WG-5415 |
| WG-5448 | medium | Strata frontend 5: differential check of cache.js and queue.js against the Lean client models | WG-5415 |
| WG-5447 | medium | Strata frontend 9: the offline write queue with idempotency keys, stamps and conflict classes | WG-5415 |
| WG-5446 | medium | Strata frontend 14: Web Components as renderers (property channel, strata-qr, connection indicator, record picker) | WG-5415 |
| WG-5445 | medium | Strata frontend 10: service worker for content-addressed files and the offline shell | WG-5415 |
| WG-5444 | medium | Strata frontend 8: leader tab with Web Locks, a heartbeat and BroadcastChannel | WG-5415 |
| WG-5443 | medium | Strata frontend 13: the kernel runs the uiScenarios headlessly in Node | WG-5415 |
| WG-5442 | medium | Strata frontend 7: device enrolment with a secret, key share, rotation and encrypted IndexedDB | WG-5415 |
| WG-5441 | medium | Strata frontend 4: pure cache.js in the kernel, the JavaScript of Strata.Client.Cache | WG-5415 |
| WG-5440 | medium | Strata frontend 15: post the view model once as the string already compared | WG-5415 |
| WG-5439 | medium | Strata frontend 12: vendor the fonts and refuse node_modules code in the bundle | WG-5415 |
| WG-5438 | medium | Strata frontend 11: in-house QR encoder replaces uqr | WG-5415 |
| WG-5437 | medium | Strata frontend 6: contracts declare whether they may be queued offline | WG-5415 |
| WG-5436 | medium | Strata frontend 3: expected-epoch on write contracts, and an optional tenant-generation guard | WG-5415 |
| WG-5435 | medium | Strata frontend 2: stamps, access generation, per-type erasure epochs read before opening, chained frames and complete notices | WG-5415 |
| WG-5434 | medium | Strata frontend 1: evidence check indexes web tests with Verifies lines | WG-5415 |
| WG-5401 | medium | Tenant leases, the fence token and the restore runbook (OPS-3121) | WG-5385 |
| WG-5394 | medium | Working-set memory: accounting and calibration, eviction policies, hot-set warmup, cell memory arbiter | WG-5385 |
| WG-5381 | medium | Kernel read: does the system principal see no records? (found while modelling the kernel in Lean) |  |
| WG-5379 | medium | Retire the transcribed specification functions once the Lean differential covers them | WG-5372 |
| WG-5375 | medium | Kernel: mutation testing of crates/kernel, with survivors filed as gaps | WG-5372 |
| WG-5372 | high | Strata assurance and generation programme: verified kernel, boundary generation, specification-led tests, accurate simulator |  |

## accepted (123)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5647 | medium | Strata: keep the last 10 runs of the gate and test suites with per-step and per-test timings | WG-5610 |
| WG-5646 | medium | Spec: shared files (Drive-like) fundamentals, milestone 14 | WG-5610 |
| WG-5645 | medium | Plinth fundamentals: F-authorization-audit, F-audit-tamper-evidence, F-append-only-records | WG-5610 |
| WG-5644 | medium | Plinth fundamentals: F-agent-governance (agent ceiling, per-operation policy, held agent changes) | WG-5610 |
| WG-5643 | high | Plinth fundamentals: finish the double-entry ledger (operation step, reversal as a transaction, account hierarchy, held postings, Lean proofs) | WG-5610 |
| WG-5642 | high | Plinth fundamentals: finish scoped roles (list/query reads, route, product-declared separations, spec functions) | WG-5610 |
| WG-5640 | medium | Plinth fundamentals: F-double-entry-journal and F-ledger-tamper-evidence | WG-5610 |
| WG-5639 | medium | Plinth fundamentals: F-scoped-role-assignment, F-separation-of-duties and F-last-owner-guard | WG-5610 |
| WG-5638 | medium | Plinth fundamentals: F-visibility and F-curated-view-sources | WG-5610 |
| WG-5637 | medium | Plinth fundamentals: F-held-operations and F-state-conditional-writes | WG-5610 |
| WG-5634 | medium | Strata tests: simulation_slow and the backend lab test under 20 s together | WG-5610 |
| WG-5633 | medium | Strata tests: system_differential_slow under 8 s | WG-5610 |
| WG-5632 | medium | Strata tests: corpus_slow and verification_slow under 10 s each | WG-5610 |
| WG-5631 | medium | Strata tests: spec_compiler_slow under 15 s | WG-5610 |
| WG-5630 | medium | Strata tests: contract conformance on one shared stack | WG-5610 |
| WG-5629 | medium | Strata tests: silence log output to stdout during tests | WG-5610 |
| WG-5628 | high | Strata gate under 5 minutes: restructure the non-test steps and set the budget to 300 s | WG-5610 |
| WG-5627 | medium | Plinth port: prove the generator on a module with live operations, and rank by live declarations | WG-5610 |
| WG-5626 | high | Strata gate: start requirements by area and ref prefix, with a scope report | WG-5610 |
| WG-5624 | medium | Plinth port: generator from inventory to Strata product skeletons, and the porting recipe | WG-5610 |
| WG-5623 | medium | Plinth port spec: realtime and UI fundamentals (8) | WG-5610 |
| WG-5622 | medium | Plinth port spec: process and automation fundamentals (12) | WG-5610 |
| WG-5621 | medium | Plinth port spec: data and query fundamentals (17) | WG-5610 |
| WG-5619 | high | Plinth port spec: accessibility by construction (D-DS-18 decided, definition-time ui.a11y checks, gate checks) | WG-5610 |
| WG-5618 | high | Plinth port spec: ledger fundamentals (double entry, tamper evidence, lot expiry, facts in rules, queries, reconciliation, proofs, reversal replay) | WG-5610 |
| WG-5617 | high | Plinth port spec: authorization, admin and audit fundamentals, with the strict rules for Plinth's own holes | WG-5610 |
| WG-5616 | critical | Strata security: widening a role one holds through a schema change escalates privilege without the conferral check |  |
| WG-5615 | medium | Strata baseline audit for the Plinth port: authorization and admin, ledger guarantees, accessibility of generated UI | WG-5610 |
| WG-5614 | medium | Plinth inventory G4: authorization, audit, admin and settings, pages and widgets, reporting, condition grammar (§3, §4, M12, M13, M17, M18, §13) | WG-5610 |
| WG-5613 | medium | Plinth inventory G3: automation, content, people/groups/segments, calendar (M7, M8, M9, M16) | WG-5610 |
| WG-5612 | medium | Plinth inventory G2: points and ledger, store, badges, promotions (M3, M4, M5, M6) | WG-5610 |
| WG-5611 | medium | Plinth inventory G1: shell and me, messages, notifications, tasks, approvals, home, search (M0, M1, M2, M10, M11, M14, M15) | WG-5610 |
| WG-5415 | high | Strata frontend: deep design and spec for a vanilla browser client (worker kernel, Cache API, IndexedDB, Web Components, offline writes), with Lean proofs and adversarial review |  |
| WG-5409 | medium | Subject export ignores field read rules: defect or intended? (KRN-1589, D-6586) |  |
| WG-5408 | high | Backend install: a process crash between the creating commit and begin_provisioning leaves a half tenant |  |
| WG-5407 | high | Working set: product scenarios over the enterprise corpus and a measurement harness (no cache, header-resident, full working set) | WG-5385 |
| WG-5406 | high | Working set design: close the open obligations (leases, multi-round decisions, in-runtime concurrency, bounded conflicts, arbiter feasibility) in the Lean model | WG-5385 |
| WG-5398 | medium | Enterprise corpus loader and population generator | WG-5385 |
| WG-5390 | medium | Header residency (CPY-014) | WG-5385 |
| WG-5389 | medium | Tenant generation and guarded commits in the store contract | WG-5385 |
| WG-5388 | medium | Impact Lab: retire the kernel engine's imitated faults or run projections through the shell path | WG-5372 |
| WG-5387 | high | Backend install: a lost answer to the create-tenant commit leaves a half tenant that is never rolled back | WG-5372 |
| WG-5386 | high | Split specifications/spec.jsonc by consumer: intent, boundary model, behaviour models, requirements, functions; Exegra, specc and Concordance read the parts |  |
| WG-5385 | high | Strata runtime: per-tenant in-memory working set — design with alternatives, Lean proofs against the kernel model, simulation plan on the enterprise corpus |  |
| WG-5378 | high | Simulator: run exactly what the deployment runs (shell path, real failure modes, checked against production) | WG-5372 |
| WG-5377 | high | specc: generate the boundary only; stop generating per-example tests; drop Exegra's per-branch example readiness rule | WG-5372 |
| WG-5376 | high | Spec authoring rules: every requirement names the check that holds it or is documentation; pilot on kernel limits | WG-5372 |
| WG-5374 | high | Kernel: differential testing of the Rust kernel against the Lean model at scale | WG-5372 |
| WG-5371 | high | specc: generate smaller crates (split strata-model, move model-core's example tests out, derive only what crosses a boundary) to cut compile memory |  |
| WG-5370 | high | Strata: remove every exemption mechanism (gate-exemptions.json, specc lint --exemptions, contract convention exemptions) and fix the one non-lint exemption |  |
| WG-5369 | medium | Strata spec: clear the requirement lint exemptions (415 findings) |  |
| WG-5368 | high | specc: lint Strata requirements (wording rules ported from Concordance) with accepted/refused fixtures and a gate step |  |
| WG-5367 | high | Strata: replace Concordance's evidence check with a gate script and remove every Concordance reference |  |
| WG-5365 | high | Strata conformance generator: separate the refusal probe's input from the success example, and let the harness set up state a success call needs |  |
| WG-5364 | medium | Strata tests: replace the badge-milestones example product with a minimal ledger-trigger fixture named after the primitives it tests |  |
| WG-5362 | high | Concordance: lint and scan skip spec files over 8 MiB, so Strata's 9.9 MB spec.jsonc is not linted at all |  |
| WG-5360 | high | Strata spec: SQLite-per-tenant cells on AWS as the storage target, with a backend port so RDS PostgreSQL is a per-cell configuration |  |
| WG-5358 | critical | Strata: fix the failing tests by root cause (149 on integration 792ae6b) |  |
| WG-5357 | high | Strata tests: cut the Rust integration tests to spec-traced tests only (one per requirement, plus a small toolchain guard) |  |
| WG-5355 | high | Strata tests: hard trim of the integration tests (delete low-value and duplicate tests, shrink slow ones) |  |
| WG-5354 | high | Strata tests: consolidate every crate's integration tests into at most two binaries (routine and slow) |  |
| WG-5353 | high | Strata client: hot reload gaps the simulator found (resync, part swap, version guard, lost drafts, lost replies) |  |
| WG-5346 | high | Hot reload simulator: N tabs on the real client kernel, a fake gateway, a controllable scheduler and invariants I1-I7 | WG-5344 |
| WG-5345 | high | Hot reload fixtures: a versioned multi-entity product family with generated manifests and parts | WG-5344 |
| WG-5343 | high | Strata: trim the specc layers test binary to its essentials |  |
| WG-5336 | high | Strata spec: identity provider seam with provider kinds and per-provider conformance (Google, Microsoft Entra, Facebook, Apple, GitHub, SAML, broker) |  |
| WG-5335 | high | Strata: Keycloak in a local container profile, proving OIDC, SAML and brokered sign-in against a real identity provider |  |
| WG-5326 | high | Schema evolution Lean: schema graph and change algebra model, theorems 1-3 and 9 | WG-5322 |
| WG-5324 | high | Schema evolution spec: dependency graph and product dependencies | WG-5322 |
| WG-5323 | high | Schema evolution: weird-situations catalogue as kernel tests (what breaks today) | WG-5322 |
| WG-5297 | high | Strata scale S-3: Tenant UI has no install path: bundles refuse screens and forms, and the router and BFF are generated from the platform spec only | WG-5292 |
| WG-5294 | medium | Strata: enterprise tenant corpus (500+ entities as product files) | WG-5292 |
| WG-5293 | medium | Strata: adversarial review of the spec against a 500+ entity enterprise tenant | WG-5292 |
| WG-4958 | critical | Integration fixes: failures from combining WG-4899/4901/4902/4905/4882 on the Strata integration | WG-4847 |
| WG-4952 | critical | Exegra type-checks scenario inputs; fix the 25 Strata scenarios the differential exposed | WG-4888 |
| WG-4921 | medium | Count used generated items, not whole modules | WG-4895 |
| WG-4919 | high | Publication template policy: authors cannot set another author's byline, feature, or publish without an editor | WG-4909 |
| WG-4918 | critical | Security fixes: dev endpoints, dev accounts, cross-tenant diagnostics, draft and principal leaks, per-site origins, lab cancel | WG-4909 |
| WG-4905 | high | Runtime Host hardening: undecodable timers, at-most-once claims, timer-only events, record constraints on save | WG-4887 |
| WG-4902 | critical | One value semantics for simulator and generated code: typed integers, equality, absent values, time units and epoch | WG-4888 |
| WG-4901 | critical | Runtime Host runs the simulator's queue semantics: generated dispatcher, (time, seq) order, timers at deadline, atomic steps | WG-4888 |
| WG-4899 | critical | Differential and scenario tests: no silent skips, cover the real machines, match Exegra's delivery | WG-4887 |
| WG-4896 | critical | Telemetry in the spec: structured logs of every domain event, generated, and checked by the e2e tests | WG-4847 |
| WG-4894 | medium | Runtime Host on the real store: strata-storage-api instead of the runtime's own tables | WG-4886 |
| WG-4891 | medium | WG-4861 automation: scenario requirements as implementation machines | WG-4861 |
| WG-4890 | medium | WG-4861 tenancy: scenario requirements as implementation machines | WG-4861 |
| WG-4889 | medium | WG-4861 identity: scenario requirements as implementation machines | WG-4861 |
| WG-4885 | medium | specc can't be built when its generated crate is missing | WG-4847 |
| WG-4884 | high | Sign-in form intermittently drops a typed character | WG-4847 |
| WG-4882 | medium | UI constructs round 4: the frontend parts still hand-written after WG-4870 | WG-4847 |
| WG-4881 | medium | Wire naming and a json type: generated clients and routes speak the platform's snake_case JSON | WG-4859 |
| WG-4879 | medium | Delayed emits end to end: runtime outbox delay, specc generation, and refines requires a delayed emit (not a timer) | WG-4847 |
| WG-4878 | medium | Strata web: give the elements the UI scenarios address a role and accessible name |  |
| WG-4875 | medium | Strata spec: map every implementation machine onto its record, so specc generates all 34 | WG-4847 |
| WG-4874 | medium | Exegra + specc + runtime: storage field mapping and record creation for implementation machines | WG-4847 |
| WG-4873 | medium | Strata: the Lean correspondence test fails on its first run in a fresh checkout |  |
| WG-4870 | medium | specc frontend: generate the web kernel's types, routes and transitions from screens, forms and actions | WG-4859 |
| WG-4869 | medium | specc e2e: generate Playwright tests from the specification's UI scenarios | WG-4847 |
| WG-4868 | medium | Strata spec: UI scenarios covering the 23 existing end-to-end tests | WG-4847 |
| WG-4867 | medium | Strata spec: screens, forms, actions and live bindings for the existing web UI | WG-4847 |
| WG-4866 | medium | specc: JavaScript target for expressions and implementation machines | WG-4847 |
| WG-4863 | medium | Strata spec: real durations and no cross-behaviour reads in implementation machines | WG-4838 |
| WG-4860 | medium | Gate: report how much of the platform is generated | WG-4847 |
| WG-4859 | medium | Web front end from a view specification | WG-4847 |
| WG-4858 | medium | specc: generate deployment descriptors from components, trust zones and environments | WG-4847 |
| WG-4857 | medium | specc and Strata spec: backends for the cross-cutting constructs, and moving checked requirements to construction | WG-4847 |
| WG-4855 | medium | specc: generate integration tests from the specification's scenarios | WG-4847 |
| WG-4854 | medium | specc: wire machines, contracts and relationships into the runtime (handlers, routes, clients, dependency wiring) | WG-4847 |
| WG-4853 | medium | Runtime boundary: separate Strata's reusable runtime from platform-specific logic, as the target of generated code | WG-4847 |
| WG-4851 | medium | specc: generate crates/model from the Strata spec and run specc check in the gate | WG-4847 |
| WG-4850 | medium | specc: implementation machines backend (state enums, event enums, pure step functions, timers and emits) | WG-4847 |
| WG-4849 | medium | spec-expr: the Exegra expression language in Rust, translated to typed Rust expressions | WG-4847 |
| WG-4848 | medium | specc: compiler core, loader, IR, and the types, records and contracts backends | WG-4847 |
| WG-4846 | medium | Strata spec: implementation-level machines refining each behaviour model | WG-4838 |
| WG-4845 | medium | Strata spec: class every requirement as construction, checked or operational | WG-4838 |
| WG-4844 | medium | Strata spec: contracts for every boundary, and the hand-written holes | WG-4838 |
| WG-4843 | medium | Strata spec: declared types, and every record fully typed and constrained | WG-4838 |
| WG-4831 | high | Strata: what the audit confirmed in the spec and prototype and did not fix | WG-4817 |
| WG-4827 | medium | Adversarial audit: does the Strata spec, Concordance and Exegra each do what it claims |  |
| WG-4824 | medium | Strata spec: devops review — deployment lifecycle and tenant migration guarantees | WG-4817 |
| WG-4822 | medium | Strata spec: adversarial review from seven angles, merged back and checked | WG-4817 |
| WG-4816 | low | Strata: PostgreSQL storage adapter |  |
| WG-4805 | high | Strata: metadata-driven multi-tenant application substrate prototype (C:\dev\strata) |  |

## cancelled (432)

| Key | Priority | Title | Parent |
|---|---|---|---|
| WG-5414 | high | Strata integration gate: clear the failures that predate the working-set stack (tsc import.meta.env, clippy on generated model-core, cargo audit advisories, spec gaps, ignored-only proofs) |  |
| WG-5413 | high | Strata: clear the new RustSec advisories (libsqlite3-sys CVE, crypto-bigint, rsa Marvin, chrono) |  |
| WG-5411 | high | Strata gate: the whole run (tests and checks) fits 10 minutes |  |
| WG-5410 | medium | cargo fmt --all reformats the generated crates/model-core/src/functions.rs |  |
| WG-5405 | medium | Specification changes for the working set | WG-5385 |
| WG-5404 | medium | Tenant restore runbook (OPS-3121) | WG-5385 |
| WG-5403 | medium | Cell memory arbiter | WG-5385 |
| WG-5402 | medium | Hot-set manifest and warmup | WG-5385 |
| WG-5400 | medium | Working set experiments: coherence and cost runs | WG-5385 |
| WG-5399 | medium | Sampled read trace | WG-5385 |
| WG-5396 | medium | Working set differential check against the Lean model | WG-5385 |
| WG-5395 | medium | Working set policies: CLOCK and cost-aware TinyLFU with GDSF | WG-5385 |
| WG-5393 | medium | Answer, page, all and ledger entries | WG-5385 |
| WG-5392 | medium | Invalidation map with its criterion | WG-5385 |
| WG-5366 | medium | Exegra and specc: refuse a contract whose auth has csrf or origin but declares no forbidden error |  |
| WG-5363 | high | specc and Exegra: refuse a comparison with the null literal in a rule expression (it can never hold) |  |
| WG-5361 | medium | Strata spec: who a send-digest step with no recipient reaches is stated two ways (send-digest-step vs D-8200) |  |
| WG-5359 | medium | Strata spec: the parameters of a webhook-started automation are named two ways (D-6523 vs CT-172) |  |
| WG-5356 | medium | Strata backend tests: every test server start hashes nine dev-account passwords with full-cost Argon2 |  |
| WG-5352 | high | Strata tenant UI: a screen's auth cannot name a role, a list screen has no columns, and a view cannot include a field read through a reference |  |
| WG-5351 | high | Strata products: an upgrade never renames a field; a renamed field becomes remove plus add |  |
| WG-5350 | medium | Exegra: check layered refusal parity with specc for every Strata refused product fixture, in Exegra's own tests |  |
| WG-5349 | medium | Hot reload real-stack scenarios: Playwright with several users against the dev stack | WG-5344 |
| WG-5348 | medium | Hot reload random interleavings: seeded timelines over the product family with invariants after every step | WG-5344 |
| WG-5347 | high | Hot reload scenario catalogue: 24 named scenarios in the simulator | WG-5344 |
| WG-5344 | high | Strata hot reload scenario programme: multi-tab simulation and real-stack scenarios for live tenant UI updates |  |
| WG-5342 | medium | Strata tenant UI: a list screen draws each row as raw field names and values |  |
| WG-5341 | medium | Strata kernel: a total split-field change with rewrite of dependents | WG-5322 |
| WG-5340 | high | Strata backend: rebase a stale schema change instead of answering SchemaConflict | WG-5322 |
| WG-5339 | high | Strata kernel: persist the PinBook with the tenant schema | WG-5322 |
| WG-5338 | critical | Strata integration: the full gate was red before the schema evolution work (about 236 failing tests and 12 failing checks) |  |
| WG-5337 | medium | Strata: implement identity provider kinds behind one trait (issuer templates, OAuth 2.0 with userinfo, broker) with fake providers for each named provider |  |
| WG-5334 | medium | Strata kernel: a workflow transition may set a value that is not a state of its workflow | WG-5322 |
| WG-5333 | high | Strata kernel: removing and redefining a field of the same type in one change silently rebinds dependents (ABA) | WG-5322 |
| WG-5332 | high | Schema evolution: client UI definitions as pinned dependents (live client control) | WG-5322 |
| WG-5331 | high | Schema evolution: verified per-change checker in the install and release gate (theorem 8) | WG-5322 |
| WG-5330 | high | Schema evolution: random development-history generator over the enterprise corpus, with Lean/Rust correspondence | WG-5322 |
| WG-5329 | high | Schema evolution: kernel dependency graph and classifier | WG-5322 |
| WG-5328 | high | Schema evolution: concurrent changes commute or rebase, with Lean theorem 6 | WG-5322 |
| WG-5327 | high | Schema evolution: rewrite of dependents and projections for pinned dependents, with Lean theorems 4, 5 and 7 | WG-5322 |
| WG-5325 | high | Schema evolution spec: change algebra, classifier, structural versions, pins, projections, expand/contract, concurrency | WG-5322 |
| WG-5322 | high | Strata schema evolution: absorb changes to dependent entities, specified, simulated and proved in Lean |  |
| WG-5321 | high | Strata scale S-27: products cannot reference each other, so an enterprise tenant cannot be split into modules | WG-5292 |
| WG-5320 | low | Strata scale S-26: Export and archive have no size bound, seg-023 does not parse as a count, and batch sizes differ by operation | WG-5292 |
| WG-5319 | medium | Strata scale S-25: The gate verifies that the platform refuses enterprise scale, has no scale requirements, and the README counts are stale | WG-5292 |
| WG-5318 | high | Strata scale S-24: The Lean model is one object type and four intents; per-release decide over a definition will not scale | WG-5292 |
| WG-5317 | high | Strata scale S-23: No performance evidence; every budget is stated at a planning load of 10 connections per tenant | WG-5292 |
| WG-5316 | low | Strata scale S-22: Search examines 1,000 candidates in an unstated order, and making a field searchable rebuilds a whole type in one transaction | WG-5292 |
| WG-5315 | medium | Strata scale S-21: Reference chains load one level per round with a 32-round cap and no stated outcome; no depth or cycle rule | WG-5292 |
| WG-5314 | low | Strata scale S-20: A video master above 100 MiB is refused whatever the plan | WG-5292 |
| WG-5313 | low | Strata scale S-19: Content caps (64 KB string, 1,000-item list, 256 KB record, 200 KB Markdown) are below long CMS content, and the parser's behaviour past 200 KB is unstated | WG-5292 |
| WG-5312 | medium | Strata scale S-18: Nothing bounds roles, teams, members, workflows, views, templates, tokens or invitations | WG-5292 |
| WG-5311 | medium | Strata scale S-17: Plan quotas do not bound entities, policies, segments, triggers, roles, teams or users; fan-out creates one job per member | WG-5292 |
| WG-5310 | medium | Strata scale S-16: Non-indexable time-dependent segments scan every person at each refresh, far beyond the job quota | WG-5292 |
| WG-5309 | medium | Strata scale S-15: Restores and predicate changes re-evaluate every person in every segment, beyond the daily job quota; seg-1544 contradicts seg-021 | WG-5292 |
| WG-5308 | medium | Strata scale S-14: The journal is kept forever and every kernel upgrade replays all of it | WG-5292 |
| WG-5307 | medium | Strata scale S-13: A UI definition for 300 screens and 500 forms is several MB with no unit smaller than the whole document | WG-5292 |
| WG-5306 | medium | Strata scale S-12: One tenant-wide schema version keys every view and aggregate copy, so any schema change expires all of them | WG-5292 |
| WG-5305 | medium | Strata scale S-11: Connections capped at 2,048 per process with a per-process registry, and relay queues bound messages not bytes | WG-5292 |
| WG-5304 | high | Strata scale S-10: Event fan-out costs connections x subscriptions x (teams + policies); visibility is recomputed per connection | WG-5292 |
| WG-5303 | high | Strata scale S-9: View and socket admission is keyed by source address only, so one NAT'd enterprise gets 32 sockets and 600 view reads a minute in total | WG-5292 |
| WG-5302 | high | Strata scale S-8: Every schema or setting notice refreshes every bound screen whole, and resync/reconnect re-reads everything | WG-5292 |
| WG-5301 | medium | Strata scale S-7: The 100-trigger cap is only in code and a function description, and event kinds are capped at 32 | WG-5292 |
| WG-5300 | medium | Strata scale S-6: Segments capped at 100 per tenant and 25 per object type, below enterprise marketing needs | WG-5292 |
| WG-5299 | high | Strata scale S-5: Aggregates and scans are refused above 10,000 records, so analytics over large types cannot be answered | WG-5292 |
| WG-5298 | high | Strata scale S-4: A product install is not one commit: segments are defined after the schema commit and an upgrade cannot be undone; spec and code disagree | WG-5292 |
| WG-5296 | high | Strata scale S-2: Schema caps (200 object types, 100 fields, 500 policies, 500 queries) sit below enterprise size, and every authorization check scans all policies | WG-5292 |
| WG-5295 | high | Strata scale S-1: A bundle install is one ChangeSchema intent but the kernel refuses more than 200 changes, counted per field | WG-5292 |
| WG-5292 | high | Strata: live client control at enterprise scale (500+ tenant entities): adversarial review and corpus |  |
| WG-5291 | medium | Strata: isolate mail sending reputation per tenant |  |
| WG-5290 | high | Strata: reduce the risk of hand-built SAML signature verification |  |
| WG-5289 | medium | Strata: edge protection (WAF, DDoS, bot control) in the cloud profile |  |
| WG-5288 | low | Strata: hand deployment orchestration to the platform instead of a bespoke release controller |  |
| WG-5287 | medium | Strata: move per-process state (quota windows, notices, files) behind shared-store adapters |  |
| WG-5286 | high | Strata decision: tenant placement tiers, backup and erasure model on managed PostgreSQL |  |
| WG-5279 | high | Strata: redo the Harbourline fold for the fold-only requirements, grounded and under the strengthened admit |  |
| WG-5272 | medium | Exegra: a lookup of eval that can mark one target of a field as not covered | WG-5014 |
| WG-5270 | medium | Exegra: accept the product-file section eventKinds (event-kind-decl) | WG-5014 |
| WG-5269 | high | Strata: the rule language reads balance, in_segment and setting in the spec, specc, the kernel and product files | WG-5014 |
| WG-5268 | high | Exegra: add the ledger-balance, segment-member and setting-value primitives of the tenant rule language | WG-5014 |
| WG-5266 | medium | Strata/Exegra: mutation scoring of the verifier — random spec breakages, fraction caught by admit |  |
| WG-5265 | high | Strata: verifier calibration corpus — substantial spec changes with expected verdicts, run through exegra admit |  |
| WG-5262 | high | Contracts and BFF disagree on replies and auth: undeclared reply members, either-or capabilities, diagnostics, sessions on views and sockets | WG-5014 |
| WG-5261 | high | Edge: refuse on origin, request token, capability and tenant before a request body is parsed | WG-5014 |
| WG-5260 | medium | Exegra: object-valued vocabulary properties, and a check of conformanceExample against the contract's inputs | WG-5014 |
| WG-5259 | low | Mirror what a trigger may not read and the missing record of a condition once specc compiles recursion | WG-5014 |
| WG-5258 | medium | Function examples: uuid values in arguments and results, and a way to build a uuid in a function body | WG-5014 |
| WG-5257 | medium | Function language: a SHA-256 hash and the canonical JSON of a value, for the run key of a trigger and the id of a ledger account | WG-5014 |
| WG-5256 | low | Ledger: restart the chain of expiry jobs of a tenant whose job failed for good | WG-5014 |
| WG-5255 | low | Spec: policy-problems checks the condition of a ledger policy against the fields of the ledger | WG-5014 |
| WG-5254 | medium | Ledger: derive a balance from the entries a reader may read (CT-288, LDG-025) | WG-5014 |
| WG-5253 | low | Read the other implementation machines of automation-worker against the worker | WG-5014 |
| WG-5252 | low | A schedule whose job failed for good and could not advance has no way back | WG-5014 |
| WG-5251 | medium | Erasure of an invoker removes the run records of their tenant operations | WG-5014 |
| WG-5250 | low | Exegra: refuse a stored copy of tenant records with no sealed-or-expiry erasure point | WG-5014 |
| WG-5249 | medium | Rotate tenant keys and re-wrap them when the process key rotates | WG-5014 |
| WG-5248 | medium | Export carries sealed fields and wrapped keys only | WG-5014 |
| WG-5247 | medium | Anchor the audit chain head and serve verify-audit-chain | WG-5014 |
| WG-5246 | high | A restore re-applies the key destructions and erasures recorded after its point | WG-5014 |
| WG-5245 | medium | Seal the personal values the sealed store does not reach: operation inputs, invitation addresses, job parameters | WG-5014 |
| WG-5244 | low | Bound pair pages by a range on the order attribute | WG-5014 |
| WG-5243 | low | specc: compile uuid in function bodies and examples | WG-5014 |
| WG-5242 | medium | Restate value-limit-problem, value-json-bytes and policy-problems with structural recursion | WG-5014 |
| WG-5241 | high | Mirror reference-following on the read side: visible, kernel-read filters and redact with a lookup | WG-5014 |
| WG-5240 | medium | Remove webhook subscriptions and inbound endpoints | WG-5014 |
| WG-5239 | low | Web pages for webhook endpoints, subscriptions and the delivery log | WG-5014 |
| WG-5238 | medium | Webhook secrets are sealed under the platform key, not a key of the tenant | WG-5014 |
| WG-5237 | medium | Webhooks: expire delivery log entries and remembered inbound event ids | WG-5014 |
| WG-5236 | low | Extend the kernel type round trips with the template blocks and the setting argument | WG-5014 |
| WG-5235 | medium | Key and class a stored view copy by the settings its view reads | WG-5014 |
| WG-5234 | medium | specc and Exegra refuse a product template whose body is not blocks of the template schema | WG-5014 |
| WG-5233 | medium | Erasure path test for segments: sealed exit events and no re-entry | WG-5014 |
| WG-5232 | medium | Segment jobs: daily job quota from the plan, and the held outcome for batches | WG-5014 |
| WG-5231 | low | Files: per-tenant stored-bytes quota | WG-5014 |
| WG-5230 | medium | Files: production scanner adapter and streaming delivery | WG-5014 |
| WG-5229 | high | Files: refuse a file field value that names no readable stored file | WG-5014 |
| WG-5228 | medium | Erasure receipt: name the files removed (WG-5118 follow-up) | WG-5014 |
| WG-5227 | low | Files: renditions in a sandboxed process | WG-5014 |
| WG-5226 | medium | Files: the web client's uploader | WG-5014 |
| WG-5225 | medium | Files: apply the tenant file limits (tenant-settings.files) | WG-5014 |
| WG-5224 | high | Files: serve uploaded files from a file domain, never a tenant host | WG-5014 |
| WG-5223 | medium | Files: kernel event bodies file-scanned and rendition-ready | WG-5014 |
| WG-5222 | medium | Exegra: refuse a mail send contract without idempotency key, category and header-break refusal | WG-5014 |
| WG-5221 | medium | Web: the subscription page that follows an unsubscribe or resubscribe link | WG-5014 |
| WG-5220 | high | Mail: erase a subject's entries of the mail log | WG-5014 |
| WG-5219 | medium | Mail: owner notices and domain events of the breaker, delivery reports and unsubscribes | WG-5014 |
| WG-5218 | medium | Mail: send-message inbox channel, delivery time and class release order across tenants | WG-5014 |
| WG-5217 | medium | Mail: message categories as tenant metadata under the schema version | WG-5014 |
| WG-5216 | medium | Mail: move the dispatcher's storage from documents to tables with indexes and atomic counters | WG-5014 |
| WG-5215 | high | Mail: delivery report adapter for the configured provider (receive-delivery-report) | WG-5014 |
| WG-5214 | low | A compensating step is refused at definition when it waits, approves, branches or ends | WG-5014 |
| WG-5213 | medium | Deciding an approval is atomic in the store | WG-5014 |
| WG-5212 | medium | A run finishes under the definition version that started it (MIG-562, workflow-version-flow) | WG-5014 |
| WG-5211 | medium | Activation records the projection it requires (WFL-002) | WG-5014 |
| WG-5210 | low | Workflow definitions default to draft until activated (D-7681) | WG-5014 |
| WG-5209 | low | Conformance cases for Store::search_candidates in the storage-api suite | WG-5014 |
| WG-5208 | low | Carry the searchable member of object-type-def through the spec functions that rebuild it | WG-5014 |
| WG-5207 | medium | Search section in the view machinery and the publication template's search view | WG-5014 |
| WG-5206 | low | Exegra: refuse a contract that delivers the personal data of a subject without a step-up | WG-5014 |
| WG-5205 | low | Legal hold on a single record (PRV-355 names each record) | WG-5014 |
| WG-5204 | low | Retention: sweep the types that declare no rule with the tenant default (PRV-356) | WG-5014 |
| WG-5203 | low | Subject export: bound the final assembly and list the identities at identity providers | WG-5014 |
| WG-5202 | medium | Restriction of processing (PRV-352): aggregates, named queries and mail must not read a restricted person's personal fields | WG-5014 |
| WG-5201 | medium | Restriction of processing (PRV-352): segments must not enter or evaluate a restricted person | WG-5014 |
| WG-5200 | medium | Restriction of processing (PRV-352): automations must not process a restricted person's records | WG-5014 |
| WG-5199 | medium | Plan request rate and connections at the gateway, and the aggregate rate of the BFF | WG-5014 |
| WG-5198 | medium | Wire the mail dispatcher to the mail quota counter of the limit store | WG-5014 |
| WG-5197 | medium | Count the jobs the worker and the segment refresh create against the plan's job quota | WG-5014 |
| WG-5196 | medium | Give plans their allowances as data in the specification (plan.allowances) and generate them | WG-5014 |
| WG-5195 | medium | Share the quota counters of every window between processes through the database | WG-5014 |
| WG-5194 | low | Aggregate copies: one computation for simultaneous misses and a store shared between processes | WG-5014 |
| WG-5193 | low | Inbox storage: index by recipient, events inbox-notice-read and ev-inbox-item-ended, erasure of the record a notice names | WG-5014 |
| WG-5192 | medium | Notification settings: ship time-zone, quiet-hours and mute-until, a zone database, and release deferred pushes | WG-5014 |
| WG-5191 | high | Notification service: wire the mail dispatcher and its message categories into the inbox | WG-5014 |
| WG-5190 | medium | Platform-produced notices: approval, form, file scan and mail breaker topics (NTF-152, D-6566) | WG-5014 |
| WG-5189 | medium | Web client inbox: list, badge, read, remove and preferences | WG-5014 |
| WG-5188 | medium | Notification subscriptions: subscribe-notifications, rec-subscription and a topic selector on the socket | WG-5014 |
| WG-5187 | medium | Notification digests: send-digest and the digest mode of a category | WG-5014 |
| WG-5186 | medium | Web file uploader: chunked upload for file fields | WG-5014 |
| WG-5185 | medium | Web inbox view: list notices and mark them read | WG-5014 |
| WG-5184 | medium | Generated code that nothing uses: check tables against generated records, and stop shipping target machines | WG-5014 |
| WG-5183 | high | Serve the request edge from the generated authorizer, contracts and validations | WG-5014 |
| WG-5182 | high | Generated contract conformance: every shipped contract is exercised through the running BFF and backend | WG-5014 |
| WG-5181 | low | Serve an equality on one field ordered by another from an index | WG-5014 |
| WG-5180 | medium | Stop query and view loads widening a missing record to its whole object type | WG-5014 |
| WG-5179 | medium | Mirror named query index plans into the spec functions and remove differential difference 11 | WG-5014 |
| WG-5178 | high | Reconcile apply-change after the three slices that add schema-change variants | WG-5014 |
| WG-5177 | medium | State the block schema of a template and the syntax that places a token in one | WG-5014 |
| WG-5176 | medium | Refuse a schema change from outside a product to an entity the product owns | WG-5014 |
| WG-5175 | medium | Installer takes the sections capabilities, segments, settings and ledger declarations of a bundle | WG-5014 |
| WG-5174 | high | Exegra: accept product.version and product.platform in the product header | WG-5014 |
| WG-5173 | high | Exegra: take the typed sections queries, views and workflows of a product file | WG-5014 |
| WG-5172 | low | Ledger: account limits out of the tenant schema, verify-ledger-balance, and socket notices for postings | WG-5014 |
| WG-5171 | medium | Erasure clears the principal from the ledger (call Store::anonymise_ledger_principal) | WG-5014 |
| WG-5170 | medium | Mirror the tenant ledger in the spec functions and remove difference 11 | WG-5014 |
| WG-5169 | low | Ledger: credit expiry by period (D-6553, LDG-1545 to LDG-1547) | WG-5014 |
| WG-5168 | medium | Ledger: policies over the ledger resource and policy-filtered balances | WG-5014 |
| WG-5167 | medium | Segments: erasure wiring, snapshot restore, and subscription selector | WG-5014 |
| WG-5166 | high | Install, upgrade and remove the segments section of a product bundle | WG-5014 |
| WG-5165 | high | Job worker runs segment batches, refreshes and the segment event log | WG-5014 |
| WG-5164 | medium | Automation step effects can outlive the 24-hour idempotency retention | WG-5014 |
| WG-5163 | medium | Automation step key repeats when a run visits a step twice (D-6528) | WG-5014 |
| WG-5162 | low | Identity: refuse principal ids that start with automation: | WG-5014 |
| WG-5161 | medium | Automation worker: its machines and its component status | WG-5014 |
| WG-5160 | medium | Spec functions: mirror the automation principal in roles-of and reserved-principal | WG-5014 |
| WG-5159 | low | Automation worker: serve schedule-job, lease-job, report-job-result, list-jobs, cancel-job and retry-job | WG-5014 |
| WG-5158 | low | Automation worker: retention of jobs, runs, steps and exceptions | WG-5014 |
| WG-5157 | medium | Automation worker: run keys and ordering by run key (SCH-1534, SCH-1536) | WG-5014 |
| WG-5156 | medium | Automation worker: compensation of completed steps (SCH-1525, WFL-005) | WG-5014 |
| WG-5155 | medium | Automation worker: schedules (define-schedule, SCH-003, SCH-005) | WG-5014 |
| WG-5154 | low | Automation worker: invoke-operation has no step kind in the catalogue | WG-5014 |
| WG-5153 | medium | Automation worker: build the request-approval step kind | WG-5014 |
| WG-5152 | medium | Automation worker: build the fan-out step kind | WG-5014 |
| WG-5151 | medium | Automation worker: build the call-webhook step kind | WG-5014 |
| WG-5150 | medium | Automation worker: build the create-notice step kind | WG-5014 |
| WG-5149 | medium | Automation worker: build the send-mail step kind | WG-5014 |
| WG-5148 | medium | Automation worker: build the post-ledger step kind | WG-5014 |
| WG-5147 | medium | State the tenant-operation decision in the spec functions (intent-decision, needs) | WG-5014 |
| WG-5146 | high | Wire tenant-operation posting steps to the ledger engine | WG-5014 |
| WG-5145 | medium | Seven-day run record for tenant operations (rec-tenant-operation-run) and the idempotency-mismatch domain error | WG-5014 |
| WG-5144 | medium | Add parse-refusal cases to the shared tenant rule language case table, so the Rust, JavaScript and Exegra parsers are held to one set of refusals | WG-5014 |
| WG-5143 | medium | Carry a record type's constraints and field shapes to the editor so the page refuses a record before the round trip | WG-5014 |
| WG-5142 | medium | Settings: float and declared types, and views, dispatcher and live pages that depend on a setting | WG-5014 |
| WG-5141 | medium | Settings: the platform level in the control plane | WG-5014 |
| WG-5140 | high | Spec: wire the generated kernel functions for settings and product capabilities | WG-5014 |
| WG-5139 | high | Substrate: install the capabilities and settings sections of a product bundle through the kernel | WG-5014 |
| WG-5138 | low | Backend telemetry: log the transition the event names instead of inferring it | WG-5014 |
| WG-5137 | low | Event selectors: segment, ledger-unit and tenant-declared events, and the generated Selector | WG-5014 |
| WG-5136 | medium | Mirror trigger decisions, fired transitions and definers in the spec functions | WG-5014 |
| WG-5135 | high | Shell: refuse a person whose principal id starts with automation: | WG-5014 |
| WG-5134 | low | Exegra: a byte-length operation for strings | WG-5014 |
| WG-5133 | medium | specc: compile structural recursion in function bodies | WG-5014 |
| WG-5132 | medium | specc: compile eval of a language with a lookup (the fourth argument) in function bodies | WG-5014 |
| WG-5131 | medium | A journey with a real owner invitation: provision a tenant from the e2e stack | WG-5014 |
| WG-5130 | low | Journey steps: virtual authenticator and TOTP code for passkey and second-factor journeys | WG-5014 |
| WG-5129 | medium | Journey steps: fill a control by label, forward, and dropping the live connection | WG-5014 |
| WG-5128 | medium | Journey steps: form-encoded apiCall bodies and response header assertions | WG-5014 |
| WG-5127 | high | Exegra: accept gotoPath fragment, captureMail and expectUrl in user journeys | WG-5014 |
| WG-5126 | medium | Exegra: refuse an unkeyed creating machine whose key the runtime cannot fill, and a retry-event beyond the runtime's attempt cap | WG-5014 |
| WG-5125 | low | aggregate-run-signed-in (CT-311) and depends-on through Read policies | WG-5014 |
| WG-5124 | low | Tenant bundles declare aggregates and declared fields | WG-5014 |
| WG-5123 | medium | Mirror aggregates in the specification functions and the differential | WG-5014 |
| WG-5122 | medium | Per-source quota q-aggregate-run on GET /api/aggregate | WG-5014 |
| WG-5121 | high | Kernel derives audience classes; aggregate results are keyed and stored per class | WG-5014 |
| WG-5120 | high | Exegra: the product-expression checker does not know the rule-language arithmetic and time operators |  |
| WG-5119 | low | Exegra: a shipped table or copy that can hold personal data must say how erase-subject treats it | WG-5014 |
| WG-5118 | low | Erasure: files and file bytes of personal file fields are not reached | WG-5014 |
| WG-5117 | low | Erasure: invitations addressed to the subject but not yet redeemed are not reached | WG-5014 |
| WG-5116 | medium | Erasure: delete records whose retention has ended, and resolve subject fields that reference a person record | WG-5014 |
| WG-5115 | medium | Subject export (export-subject-data, CT-210) is not built | WG-5014 |
| WG-5114 | medium | Legal holds (PRV-355) are not built, so a hold cannot suspend an erasure | WG-5014 |
| WG-5113 | medium | Restrictions of processing (PRV-352) are not built, so erasure cannot report them | WG-5014 |
| WG-5112 | medium | Record index writes personal values whether or not the field is searchable | WG-5014 |
| WG-5111 | medium | Erasure: schema history keeps older invitations and teams of the subject | WG-5014 |
| WG-5110 | high | Erasure: the audit log keeps identifiers and addresses of the subject | WG-5014 |
| WG-5109 | medium | Erasure: backups and exports of the tenant keep the personal values | WG-5014 |
| WG-5108 | high | Erasure: snapshots of the tenant state keep the personal values | WG-5014 |
| WG-5107 | high | Erasure: journal entries keep the intents and contexts that created the erased values | WG-5014 |
| WG-5106 | high | Erasure: events and the outbox keep the before and after images of erased records | WG-5014 |
| WG-5105 | medium | Lab apply route that checks the revision a projection was taken at | WG-5014 |
| WG-5104 | high | Push query filters and sort orders into storage, and stop views loading whole tables | WG-5014 |
| WG-5103 | medium | Bundles: install triggers, automations and operations sections as schema changes | WG-5014 |
| WG-5102 | medium | Spec: may-confer counts the policies of a role as rules.rs may_confer does | WG-5014 |
| WG-5101 | medium | Kernel: check catalogue references of triggers, automations and operations (segments, units, pools, templates, settings, schedules) | WG-5014 |
| WG-5100 | high | Kernel: invoke a tenant operation (decide-tenant-operation, CT-806) | WG-5014 |
| WG-5099 | medium | Kernel: record the definer of an automation and suspend it when the definer loses authority (WFL-1529) | WG-5014 |
| WG-5098 | high | Kernel: decide automation steps as the automation principal automation:<workflow-id> with its run-as roles | WG-5014 |
| WG-5097 | high | Kernel: evaluate the triggers of each committed event and start their runs in the same transaction | WG-5014 |
| WG-5096 | low | State reference-following rules in the spec authorize and filters | WG-5014 |
| WG-5095 | medium | Bring the spec schema-problems to the problems of TenantSchema::problems | WG-5014 |
| WG-5094 | high | Mirror the multi-reference delete in the spec delete-record | WG-5014 |
| WG-5093 | medium | Mirror field-level read conditions in the spec kernel-read | WG-5014 |
| WG-5092 | medium | Mirror the size limits of limits.rs in the spec validate-record | WG-5014 |
| WG-5091 | high | Spec kernel-transition functions must authorize before looking a record or object type up (KRN-1080) | WG-5014 |
| WG-5090 | medium | Unkeyed machines that create their records cannot be started by the runtime | WG-5014 |
| WG-5089 | medium | specc: generate the tenant rule language as JavaScript and run the shared case table on it | WG-5014 |
| WG-5088 | medium | Rule language: reads in expression scope (balance, in_segment, setting) in Exegra, specc and the kernel | WG-5014 |
| WG-5087 | high | Backend install-bundle route keeps the catalogue record and commits it with the schema change | WG-5014 |
| WG-5086 | high | Object type declarations (retention, publication, language-field, translation-group) have no schema change | WG-5014 |
| WG-5085 | high | Kernel takes triggers, automations and tenant operations from a bundle | WG-5014 |
| WG-5084 | medium | Kernel takes the catalogue edits for mail templates, notice templates and tokens | WG-5014 |
| WG-5083 | medium | Kernel takes the catalogue edits for ledger units, pools and limits | WG-5014 |
| WG-5082 | high | Kernel takes the catalogue edits for capabilities, segments and settings | WG-5014 |
| WG-5081 | medium | Product files declare queries, views and workflows | WG-5014 |
| WG-5080 | high | Product header accepts version and platform range in specc and Exegra | WG-5014 |
| WG-5079 | low | Event selectors: record the fired workflow transition in the event and emit segment, ledger-unit and tenant-declared events | WG-5014 |
| WG-5078 | medium | Erase-subject must remove the principal's credentials through Store::remove_principal | WG-5014 |
| WG-5077 | low | Spec function object-problems lacks the per-field read check that TenantSchema::problems has | WG-5014 |
| WG-5076 | medium | A schema change that declares the retention of an object type | WG-5014 |
| WG-5075 | low | Honour idempotency-key on the BFF mutation routes that the new contracts declare non-idempotent | WG-5014 |
| WG-5074 | medium | State a retention bound for the dead letters and records of the runtime machine store | WG-5014 |
| WG-5073 | low | Make the inputs of contracts that gained a bffRoute match the BFF request bodies | WG-5014 |
| WG-5072 | medium | Extend the bffRoute vocabulary to methods, wildcard segments and path parameters | WG-5014 |
| WG-5071 | medium | uiScenario step to open an address with a fragment, then the invitation scenario | WG-5014 |
| WG-5070 | medium | Give invoke-tenant-operation an idempotency-key input of its own | WG-5014 |
| WG-5069 | medium | Install a bundle as one product change | WG-5014 |
| WG-5068 | medium | Lab projection at a named revision | WG-5014 |
| WG-5067 | medium | Sign-in page: require Trusted Types | WG-5014 |
| WG-5066 | medium | Deliver-view: field selection and cursor | WG-5014 |
| WG-5065 | high | Backend follow-ups from the audit wave: revoke-invitation route, credentials removed with an account, event selector fields | WG-5014 |
| WG-5064 | high | Rule language: checked arithmetic, time arithmetic and text operators, in Exegra, specc, the kernel and the spec | WG-5014 |
| WG-5063 | medium | specc turns a product file into a tenant bundle | WG-5014 |
| WG-5062 | low | Restore segment eligibility, per-person limit and per-promotion stock in promotions.jsonc | WG-5014 |
| WG-5061 | low | Product object types with a personal field must carry a retention rule | WG-5014 |
| WG-5060 | medium | Parse the expression text of product typed sections with the tenant rule language | WG-5014 |
| WG-5059 | low | Client page builders and product renderer links without hand lists | WG-5014 |
| WG-5058 | medium | fast-apply: support vocabulary property declarations in operations files | WG-5014 |
| WG-5057 | medium | Web sign-in page: accept the owner invitation link (#i=...&t=...) | WG-5014 |
| WG-5056 | high | Add the tenant-mismatch error to the other public contracts with a tenant-id input | WG-5014 |
| WG-5055 | critical | Resolve the operations files that overwrite each other's errors on 15 contracts (tenant-mismatch and step-up-required) | WG-5014 |
| WG-5054 | medium | Create or stop citing docs/agents/verification.md, specifications/21-verification/cases/budget-measurement.md and tests/fixtures/language/kernel-cases.json | WG-5014 |
| WG-5053 | low | Decide the specification entities that cite Markdown files that no longer exist | WG-5014 |
| WG-5052 | low | Contracts with a bffRoute for the lab, diagnostics, dev and socket routes the BFF mounts | WG-5014 |
| WG-5051 | medium | Contracts with a bffRoute for the identity routes the BFF mounts | WG-5014 |
| WG-5050 | medium | Declare STRATA_ARGON2_PER_TENANT, STRATA_BLOCKING_PER_TENANT, STRATA_BLOCKING_QUEUE and STRATA_MAX_CONNECTIONS_PER_SOURCE in the specification | WG-5014 |
| WG-5049 | medium | Add shipped spec records for the runtime_* SQLite tables | WG-5014 |
| WG-5048 | medium | Give the shipped contracts health-check, deliver-view, subscribe-events, show-sign-in-page, show-authoring-view, run-scenario, project-change, install-bundle and parse-bundle a handler the gate can find | WG-5014 |
| WG-5047 | medium | Mark the Store trait methods that handle the store-contract contracts with Implements | WG-5014 |
| WG-5046 | medium | Product files have no section for object types, segments, ledger units or mail templates | WG-5014 |
| WG-5045 | high | Kernel: tenant-schema triggers and automations maps and their schema-change variants | WG-5014 |
| WG-5044 | high | Exegra: read product sections triggers and automations and refuse steps outside the catalogue | WG-5014 |
| WG-5043 | medium | Bring the contracts that break the conventions of D-6595 but are not listed in it | WG-5014 |
| WG-5042 | high | Bring the contracts listed in D-6595 to the contract conventions | WG-5014 |
| WG-5041 | medium | Exegra: the browser app reloads product files, and CI can register the Strata project |  |
| WG-5040 | medium | Edge follow-ups: tenant hosts without a restart, socket close on shutdown, proxy and write-timeout safety | WG-5014 |
| WG-5039 | high | Generate navigation, guards, entry hooks and row actions so a new screen needs no hand edit | WG-5014 |
| WG-5038 | high | Gate and specc refusals for the audit fixes: every finding that can be a rule becomes one | WG-5014 |
| WG-5037 | high | Exegra: two tests fail because they assert the live Strata spec has no readiness diagnostics |  |
| WG-5036 | medium | Kernel and store correctness: delete with two references, migrations without events, throttle lock, fleet upgrades | WG-5014 |
| WG-5035 | high | Web client and BFF: state kept across sign-out, missed live updates, blog-shaped hard-coding | WG-5014 |
| WG-5034 | high | Product data model gaps: bundle install, personal data, field reads, files, capabilities, content | WG-5014 |
| WG-5033 | high | Messaging and notifications: no product-callable send, thin inbox, one suppression list | WG-5014 |
| WG-5032 | critical | Primitives do not connect: ledger, segments and settings are unreachable from rules, operations and members | WG-5014 |
| WG-5031 | critical | Automation cannot be expressed: triggers, step catalogue, delays, run-as authority | WG-5014 |
| WG-5030 | high | Checks that can pass without checking: differential, obligations, goldens, gate coverage | WG-5014 |
| WG-5029 | high | The spec's assurances must be about the code that runs | WG-5014 |
| WG-5028 | high | Production safety: durable audit, operator routes, dev switches and startup defaults | WG-5014 |
| WG-5027 | high | Identity hardening: federated linking, step-up coverage, key storage, invitations in production | WG-5014 |
| WG-5026 | high | One tenant can starve the rest: add per-tenant limits to requests, hashing, events and rate tables | WG-5014 |
| WG-5025 | high | Reads and writes need bounds: paging, pushdown, size limits, idempotency keys | WG-5014 |
| WG-5024 | high | Tenant lifecycle: provision, suspend, take down, delete, restore and fork are unspecified or unbuilt | WG-5014 |
| WG-5023 | high | Spec: owner-level operations that expose or destroy everything need tighter rules | WG-5014 |
| WG-5022 | high | Layering: refuse product entities that claim platform powers | WG-5014 |
| WG-5021 | high | specc: refuse duplicate keys and ids, anchor type patterns, reserve generated file names | WG-5014 |
| WG-5020 | critical | specc: tenant scope must be required and enforced on every non-local contract | WG-5014 |
| WG-5019 | high | A revoked session can be revived by the last-seen write | WG-5014 |
| WG-5018 | critical | Kernel: role conferral ignores policies, and revocation needs no authority over the role | WG-5014 |
| WG-5017 | high | Kernel: updating a record that holds many references is refused after 32 reload rounds | WG-5014 |
| WG-5016 | critical | SQLite store: one broken tenant must not stop outbox, sessions, health or purges for all tenants | WG-5014 |
| WG-5015 | critical | Markdown parser: one request of nested quotes crashes the process for every tenant | WG-5014 |
| WG-5014 | high | Audit the whole Strata platform for gaps and likely problems: spec, code and tooling | WG-4988 |
| WG-5011 | high | A product spec can declare record types and operations as tenant metadata | WG-4988 |
| WG-5009 | high | Spec: declare workflow step kinds and triggers as data | WG-4988 |
| WG-5008 | high | Reference feature: badges built as a product module from primitives only | WG-4988 |
| WG-5007 | high | Bundle install: a product module lands in a tenant as reviewed metadata | WG-4988 |
| WG-5006 | medium | Outbound gate, webhook delivery and inbound webhooks | WG-4988 |
| WG-5005 | medium | Mail dispatcher with templates, suppression, quotas and delivery reports | WG-4988 |
| WG-5004 | medium | Notification service: inbox, live delivery, preferences and digests | WG-4988 |
| WG-5003 | high | Workflow runs: definitions, activation, steps, approval and compensation | WG-4988 |
| WG-5002 | high | Decide: non-goal ng-8 against campaigns and promotions | WG-4988 |
| WG-5001 | medium | Spec: aggregates in named queries | WG-4988 |
| WG-5000 | medium | Spec: typed settings layered platform, tenant, team and person | WG-4988 |
| WG-4999 | medium | Spec: segments, saved predicates over people that stay current | WG-4988 |
| WG-4998 | medium | Spec: a ledger primitive for points, caps and budgets | WG-4988 |
| WG-4997 | medium | Audit log and sealed personal fields | WG-4988 |
| WG-4996 | medium | PostgreSQL adapter, tenant directory and control plane | WG-4988 |
| WG-4995 | medium | Limit store enforces tenant quotas, plans and feature flags | WG-4988 |
| WG-4994 | medium | Search index per tenant | WG-4988 |
| WG-4993 | medium | File store with upload scanning and signed media addresses | WG-4988 |
| WG-4992 | high | Automation worker runs jobs, leases, schedules and triggers | WG-4988 |
| WG-4991 | high | Checks scoped to one changed module finish in under 2 minutes | WG-4988 |
| WG-4990 | medium | specc generates a compact capability catalogue of the platform | WG-4988 |
| WG-4989 | high | A product spec is its own file that imports the platform spec's named entities | WG-4988 |
| WG-4988 | high | Strata can host a product spec built only from platform primitives |  |
| WG-4976 | medium | Spec functions: authorize honours role object permissions; schema-problems checks role permissions | WG-4954 |
| WG-4972 | medium | specc and Strata functions for WG-4968's format additions | WG-4895 |
| WG-4971 | medium | Cover the remaining 146 function branches; remove defensive arms | WG-4965 |
| WG-4970 | medium | Set accounts' verified email from identity verification | WG-4954 |
| WG-4969 | medium | client-live retries never climb: lv-closed-open-2..5 unreachable | WG-4965 |
| WG-4968 | medium | Exegra format gaps from the read/view functions: contract target realisation, untagged wire, route-supplied inputs, null, lookup, list order | WG-4895 |
| WG-4967 | medium | Role administration: members route, verified invitation address, capability strings in tenant roles | WG-4954 |
| WG-4966 | medium | specc support for WG-4957's function-language features; drop intent-texts | WG-4895 |
| WG-4965 | high | Coverage and exercise of every branch: measured in the gate, exhaustive machine transitions, example branch coverage, mutation testing | WG-4847 |
| WG-4963 | medium | Shared example fixtures for function examples (spec size) | WG-4895 |
| WG-4961 | medium | Generate the differential's machine registry instead of hand-listing it | WG-4888 |
| WG-4960 | high | Exegra accepts shared<T>; apply WG-4928's spec transaction | WG-4895 |
| WG-4957 | medium | Function and rule-language gaps: has_role bug, null literal, unit, print, recursion, string ops | WG-4895 |
| WG-4956 | medium | Exegra and specc type gaps: set<T>, tuple variants, nested generics; uuid ids in model-core | WG-4895 |
| WG-4955 | medium | Small Exegra items: route.ambiguous, {expr, simulate} timer check, two stale docs | WG-4948 |
| WG-4954 | high | Capabilities and roles: permission sets (ADM-150..153), teams, grants, plans, flags; generated authorizer and operation catalogue | WG-4847 |
| WG-4953 | high | Field-level read permissions or opaque principal ids: author.principal is public | WG-4909 |
| WG-4951 | high | Control plane generated from the spec: tenant directory, provisioning, suspend, export, restore, delete, domain binding | WG-4886 |
| WG-4950 | high | Data model in the spec: platform metadata store vs tenant data described only by tenant schemas; database per tenant | WG-4895 |
| WG-4948 | critical | Every finding becomes a refusal: specc refuses a spec that lacks what reviews found missing | WG-4847 |
| WG-4947 | medium | Close-out | WG-4895 |
| WG-4946 | medium | Tenancy and bundles on generated code | WG-4895 |
| WG-4945 | medium | Live notices on generated code | WG-4895 |
| WG-4944 | medium | Sessions and sign-in on generated machines | WG-4895 |
| WG-4943 | medium | Outbox publisher on the Host | WG-4895 |
| WG-4942 | medium | Remove publication knowledge from platform code | WG-4895 |
| WG-4941 | medium | Backend and BFF adopt the generated records endpoint, view and execute derivations | WG-4895 |
| WG-4940 | medium | Kernel adopts the generated decision and read code | WG-4895 |
| WG-4939 | medium | Spec: read and view functions, generic read-view and mutate-record, facades | WG-4895 |
| WG-4938 | medium | Spec: intent-decision functions | WG-4895 |
| WG-4937 | medium | Spec: validation, schema problems, defaults and coerce functions | WG-4895 |
| WG-4936 | medium | Spec: policy functions | WG-4895 |
| WG-4935 | medium | Exegra and specc: derived and facade contracts | WG-4895 |
| WG-4934 | medium | spec-expr and specc: compile functions to Rust | WG-4895 |
| WG-4933 | medium | Exegra: the function construct with examples | WG-4895 |
| WG-4932 | medium | Kernel and substrate adopt the generated language | WG-4895 |
| WG-4931 | medium | specc: generate the language | WG-4895 |
| WG-4930 | medium | Spec: the tenant-expr language | WG-4895 |
| WG-4929 | medium | Exegra: the language construct | WG-4895 |
| WG-4928 | medium | Kernel adopts the generated types | WG-4895 |
| WG-4927 | medium | Spec: exact kernel data types | WG-4895 |
| WG-4926 | medium | specc: generate unions and maps, and emit the pure crate model-core | WG-4895 |
| WG-4925 | medium | Exegra: union and map types, type constraints, wire encoding | WG-4895 |
| WG-4924 | medium | Spec: apply the machine classification | WG-4895 |
| WG-4923 | medium | specc and Concordance honour target and memory | WG-4895 |
| WG-4922 | medium | Exegra: machine realisation (shipped/target) and memory durability | WG-4895 |
| WG-4920 | medium | Resource limits: sockets per session and address, request timeouts, lab threads, Lean verify | WG-4909 |
| WG-4917 | medium | Identity 7: provisioning, binding and metadata; verify and revoke; service clients; outage | WG-4910 |
| WG-4916 | medium | Identity 6: SAML sign-in | WG-4910 |
| WG-4915 | medium | Identity 5: factors, step-up and recovery | WG-4910 |
| WG-4914 | medium | Identity 4: OpenID Connect sign-in and linking policy | WG-4910 |
| WG-4913 | medium | Identity 3: emailed sign-in links, mail delivery and safe return addresses | WG-4910 |
| WG-4912 | medium | Identity 2: passkeys (WebAuthn) registration and sign-in | WG-4910 |
| WG-4911 | medium | Identity 1: sign-in method framework, sessions and enumeration defences | WG-4910 |
| WG-4910 | high | Identity: every sign-in method and auth feature in the spec, working in the running app | WG-4847 |
| WG-4909 | high | Security review of the running app's auth: sessions, CSRF, origin, tenancy, dev logins, throttling | WG-4887 |
| WG-4908 | critical | Auth as structured spec: public vs private exposure, schemes, default deny, generated authorizer | WG-4847 |
| WG-4907 | high | Machine instance keys: whole record key and tenant scope | WG-4887 |
| WG-4906 | high | Idempotency replay: key on tenant, caller and request; reserve in-flight keys; bound by bytes | WG-4887 |
| WG-4904 | critical | Browser machines and trace freshness match the simulator: jitter, expr timers, resync, delayed emits, spec hash | WG-4888 |
| WG-4903 | critical | Faults, delivery guarantees and record creation: the simulator models what the runtime can actually do | WG-4888 |
| WG-4900 | high | Web client defects: confirm survives navigation, weak same-site check, client validation differs from server, route ties | WG-4887 |
| WG-4898 | high | specc code emission: comment and identifier injection, unescaped literals, string-replace on compiled JS | WG-4887 |
| WG-4897 | critical | specc e2e generates nothing for Strata: second-set journey steps unimplemented, weak waits and checks | WG-4887 |
| WG-4895 | medium | Design: the kernel interpreter as spec and generated code, and the spec's machines reconciled with the app | WG-4886 |
| WG-4893 | medium | specc routes: per-contract serving, error status per kind, the app's error body and auth | WG-4886 |
| WG-4892 | medium | Run Strata in Alpine Linux containers under podman | WG-4847 |
| WG-4888 | medium | Gaps between Exegra's simulator semantics and specc's generated machines | WG-4847 |
| WG-4887 | medium | Adversarial review of specc's generated code | WG-4847 |
| WG-4886 | high | Run the generated backend: the services use strata-model instead of hand-written equivalents | WG-4847 |
| WG-4883 | medium | Exegra retention: keep for a duration after an event |  |
| WG-4861 | medium | Strata spec: express each area's scenario requirements as implementation machines, area by area | WG-4847 |
| WG-4847 | medium | specc: a Rust compiler from the Exegra specification to the backend's structural Rust code |  |
| WG-4838 | medium | Make the Strata specification complete enough to generate the backend's structural code from |  |
| WG-4817 | high | Strata: specification in Exegra JSONC, aligned with Exegra and Concordance |  |
| WG-4815 | medium | Strata: field-level read policies and author binding on posts |  |
| WG-4814 | medium | Strata: extend the Lean model to constraints, uniqueness and references |  |
| WG-4813 | medium | Strata: push query filters down to the store |  |
