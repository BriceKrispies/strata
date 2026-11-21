# strata-simulation: what it runs, and what the deployment runs

The simulator must run exactly what the deployed system runs, so that anything it reports is a
property of Strata and not of the simulator (`docs/assurance.md`, "The simulator"). This file is
the audit of that: every path the backend takes for a request, a job, an outbox delivery and an
automation run, and for each whether the simulator **runs** the same code, **skips** it, or
**imitates** it with logic of its own.

Where the logic lives:

- **kernel** (`crates/kernel`, pure): `transition`, `apply_delta`, `World::step`, `needs`,
  `refine_needs`, trigger decisions (`triggers`), automation expressions (`expr`).
- **store contract** (`crates/storage-api`): `execute` / `execute_admitting` / `execute_at`
  (load what the kernel needs, decide, commit, retry a lost race, idempotency), `Commit::of`
  (journal entry, idempotency record, trigger jobs via `jobs::trigger_intake`), the job lease
  state machine (`jobs::apply_lease`, `jobs::apply_report`), segment decisions
  (`segments::decide_change`), and the adapters (`memory::MemoryStore`, SQLite, PostgreSQL) under
  the sealing layer (`sealed::SealedStore`).
- **runtime** (`crates/runtime`): the machine host `System` that runs the generated outbox
  machine, its `Clock`, its retry and dead-letter policy.
- **backend shell** (`crates/backend`): `ops::run_intent*` (file and quota checks, live sessions
  of a role change, the deadline, `execute_admitting` with `quota::JobAdmitter`, change audit,
  the commit hook `hub.committed()`), `outbox::Outbox` and `hub::publish_pending`, the automation
  worker (`worker::tick`, `worker::leases`, `worker::steps` and the step kinds), the outbound gate
  and webhook deliveries (`outbound::gate`, `outbound::deliveries`), the mail dispatcher
  (`dispatch::deliver`, `mail::Mailer`), `seed::seed`, `lab::replay_live`.
- **backend HTTP layer** (`crates/backend/src/http.rs`): the service-token `gate` middleware,
  `precheck::admit`, `quota::admit`, `auth::caller` / `auth::identify` (session), admission to the
  blocking pool (`App::blocking_for`), JSON decoding and `substrate::plain::coerce_fields`.

## Audit of the simulator before WG-5378

| Backend path | Backend code | Simulator (before) |
|---|---|---|
| Request: HTTP gate, precheck, request quota, session, admission | `http::gate`, `precheck::admit`, `quota::admit`, `http::as_caller`, `auth::identify`, `App::blocking_for` | **Skips.** Scheduled intents carry a ready `Context`. |
| Request: JSON to intent | `ops::mutate`, `substrate::plain::coerce_fields` | **Skips.** The workload builds intents directly. |
| Request: shell checks before the decision | `ops::run_intent_unaudited_at`: `files::attach::check_intent`, `quota::check_write`, `live_sessions_of` | **Skips.** |
| Request: decision and commit | `storage_api::execute_admitting` over `SealedStore` over the adapter: `needs`/`refine_needs` load rounds, `transition`, `segments::decide_change`, `Commit::of`, `commit_with_segments`, idempotency lookup | **Imitates.** `engine::run_with` calls `World::step` on an in-memory `World`: the kernel decision is the real one, but no store contract, no partial loads, no idempotency, no segments, no job intake, no sealing. |
| Request: lost race | `execute_guarded`: `StoreError::Conflict` from the adapter, decide again on fresh state, at most 8 retries | **Imitates.** `Faults::commit_conflict` calls `World::step` again and discards the result; nothing can actually conflict. |
| Request: change audit and commit hook | `change_audit::record`, `hub.committed()` | **Skips.** |
| Journal | `Commit::of` writes `JournalEntry`; adapters store it | **Imitates** in the test `a_journal_of_real_commits_replays_to_the_live_state`, which builds a journal "the way the store does" from `World::step`. `journal::replay_journal` itself is the real check (kernel replay of recorded inputs). |
| Outbox delivery | `outbox::Outbox` (generated machine `arc-outbox-delivery-flow-impl` on `runtime::System`, 500 ms sweep, kick on commit), `Publisher::dispatch`, `hub::publish_pending` (`Store::outbox`, per-tenant header load, hub fan-out, `mark_published`) | **Imitates.** `engine::run_with`'s `pump` keeps its own queue of `Effect::Publish` sequence numbers, drops deliveries with `Faults::delivery_drop` and retries them itself; its invariant `events_delivered_once_in_order` checks the pump, not Strata. |
| Jobs: finding, leasing, running, reporting | `worker::tick` / `worker::spawn`, `Store::jobs_due`, `leases::drain_tenant`, `Store::job_lease`, `steps::run_job`, `Store::job_report` with fencing | **Skips.** No job is ever created or run. |
| Automation runs and workflow steps | `jobs::trigger_intake` (from the kernel's `Effect::StartAutomation`), `steps::start_run`, `steps::run_step`, `steps::act` (each step decided by `ops::run_intent` as the automation principal, keyed by the step key), waits, branches, saga, approvals | **Skips.** |
| Effects through adapters | `outbound::deliveries::run_delivery` through `outbound::gate::Gate` (`Resolve`, `Transport`); `dispatch::deliver::run_job` through `mail::Mailer` | **Skips.** |
| Clock | `App::now` (system clock plus development skew); the outbox machine's `SystemClock`; the worker's `clock` argument | Logical time in `Context::now` only; nothing else reads a clock because nothing else runs. |
| Randomness | `getrandom` in `auth::random_hex`, `outbound::random_uuid`, `sealing::seal_with`, `subject_keys::random` | Not reached. |
| Process start | `crate::run`: `seed::seed`, first `Outbox::committed` | **Imitates.** Tests install bundles with `substrate::bundle::install_into` over a `World`. |
| Faults | the specification's `failureModes` | Two imitated faults (`commit_conflict`, `delivery_drop`), neither reaching any code that can fail. |

What the kernel-level engine is still for: `engine::run_with` and `project::project` decide
counterfactual branches for the Impact Lab (`lab.rs`, CT-391) on a copy of a tenant's state, where
only the kernel's verdicts are compared. That is a statement about the kernel, and it stays.
