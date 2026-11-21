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

## The shell path (since WG-5378)

`shell::run_shell` drives the backend's own code through `strata_backend::simulated`, which
implements `shell::Deploy` and `shell::Process`. The backend is a dev-dependency of this crate
(the backend depends on this crate's library for its Impact Lab), so the traits live here and
the implementation lives in the backend.

| Backend path | Simulator now |
|---|---|
| Process start | **Runs** `App::new` over the given store, `seed::seed` and the first `Outbox::on_commit`, as `crate::run` does. |
| Request: shell checks, decision, commit, lost race, idempotency, change audit | **Runs** `ops::run_intent` (the function the HTTP handlers call after authentication): `files::attach::check_intent`, `quota::check_write`, `execute_admitting` with `quota::JobAdmitter` over `SealedStore` over `memory::MemoryStore`, retries of a real `StoreError::Conflict`, idempotency records, `change_audit`. |
| Request: HTTP gate, precheck, request quota, session, admission, JSON | **Skips.** A scheduled request carries its `Context` as `auth::identify` would have built it. |
| Commit hook | **Runs** `Outbox::on_commit` after every request and worker pass that committed (the store layer counts commits). The backend spawns one per commit; the simulator makes one per batch, which publishes the same events. |
| Outbox delivery | **Runs** the generated outbox machine on `runtime::System` with the logical clock (`Outbox::with_clock`), its sweep timer and retry and dead-letter policy (`Outbox::sweep` at logical times in place of the host's 50 ms loop), `Publisher::dispatch` and `hub::publish_pending`; what subscribers hear is read from `hub.events`. |
| Jobs | **Runs** `worker::tick` (wake index, `leases::drain_tenant`, `Store::job_lease`, `steps::run_job`, `Store::job_report` with fencing) at the logical time, one pass at a time. The running worker's concurrent passes, per-tenant caps and pool admission (`worker::spawn`, `App::blocking_for`) are not run. |
| Automation runs and steps | **Runs** trigger intake in the commit, `steps::start_run`, `steps::run_step`, `steps::act` (each step decided by `ops::run_intent` as the automation principal with its step key), waits, the hop limit. |
| Effects through adapters | **Runs** `outbound::deliveries::run_delivery` through the real `outbound::gate::Gate`, with a `Resolve` and a `Transport` whose answers the simulation decides; the mail service is a `mail::Mailer` that does the same (`Adapters`). |
| Clock | **Logical** everywhere the paths read one: `App::now` (seam `App::set_clock`), the outbox machine's timers and retries (`Outbox::with_clock`), the worker's clock. |
| Randomness | **Seeded** on the simulator's thread (seam `strata_runtime::entropy`): identifiers, secrets, sealing nonces and subject keys made on these paths. |
| Journal | **Runs** the backend's own replay (`lab::replay_tenant`, what `lab::replay_live` answers) over the journal the store wrote; a run's journal is its recording (`shell::Recording`), replayed through a fresh deployment by `shell::replay_recording`. |

Faults are injected where they happen in a deployment: under the sealing layer as the adapter's
errors (`faults::FaultyStore`), in the arrival of requests, in the life of the process, and in
the adapters.

| Failure mode | Fault | Test (`tests/shell.rs`, binary `simulation_slow`) |
|---|---|---|
| fm-worker-stalls | `report_lost`: the report is held and sent `STALL_MS` later | `a_worker_that_stalls_holding_a_lease_applies_no_step_twice` |
| fm-runtime-crash | `crash`: the process stops after an answer, before the commit hook, and a new one starts over the same store | `a_process_stopped_between_commit_and_publication_publishes_on_restart` |
| fm-poison-event, lost publication mark | `publish_unavailable`, `publish_ack_lost` | `a_publisher_that_cannot_read_or_mark_the_outbox_publishes_everything_in_order` |
| fm-trigger-loop | an automation whose note starts it again | `a_trigger_loop_ends_at_the_hop_limit` |
| fm-automation-worker-partition | `lease_partition` | `a_worker_cut_off_from_its_tenant_database_takes_no_lease_and_resumes` |
| fm-provisioning-step-fails | `stop_before_commit` while the first process installs | `a_failed_installation_leaves_no_half_tenant_and_the_next_start_installs_it` |
| a stopped process mid-commit (fm-database-failover, fm-disk-full as a request meets them) | `commit_unavailable` | `a_process_stopped_before_its_write_commits_nothing_and_the_retry_commits_once` |
| a lost answer after a durable commit | `commit_reply_lost` | `a_commit_whose_answer_is_lost_is_answered_from_its_key` |
| a store conflict | `commit_conflict` | `a_commit_that_loses_a_race_is_decided_again` |
| a repeated message (env-cloud message-duplication) | `request_repeated` | `a_repeated_request_commits_once` |
| a delayed message | `request_delayed` | `a_delayed_request_arrives_after_later_ones` |
| an adapter failing or timing out | `adapter_fails`, `adapter_times_out` | `a_webhook_receiver_that_fails_or_times_out_is_retried_and_answered_once` |

Checks against production: `the_shell_path_decides_as_the_kernel_does` (no faults: every answer
and the final tenant equal the kernel's), `a_recorded_workload_replays_to_its_recorded_outcomes`,
and SIM-004 (`a_journal_of_real_commits_replays_to_the_live_state`), whose journal is now the one
the backend's store wrote. Every shell run also checks: the world invariants, the backend's own
journal replay against the live state, that every committed event reached the subscribers the
first time in order without a gap, and that no request with an idempotency key committed twice.

### What still is not the deployment

- The HTTP layer (service-token gate, prechecks, request quotas, sessions, pool admission, JSON
  decoding) is not run; requests start at `ops::run_intent`.
- The worker runs one pass at a time; its concurrency and admission limits are not run.
- One commit hook per batch of commits, and publisher sweeps at chosen logical times, in place of
  a spawned hook per commit and the host's 50 ms loop: the same publications, a coarser timing.
- `execute`'s pause between conflict retries is a real `std::thread::sleep` (5 to 40 ms): it
  costs wall time and changes no result.
- Wall-clock reads that reach no trace remain: `App::started_at`, audit-ring and subject-key
  document timestamps (`now_ms`), the platform settings cache age (`Instant`).
- A stalled worker's late report is sent straight to the in-memory adapter, as the write the
  sealing layer had already produced.
- Failure modes of one process that are on other paths are not injected: fm-revocation-notice-lost
  (gateway), fm-migration-interrupted (segment migration runs), fm-bad-layout-change (storage
  migrations), fm-move-copy-fails and fm-move-concurrent-write (tenant moves). fm-mail-outage and
  fm-provider-outage are provider-scope; the mail adapter is simulated but no test sends mail.
- The Impact Lab's kernel-level engine keeps its imitated `Faults` (WG-5388).
- Finding: a lost answer to the create-tenant commit of an installation leaves a half tenant
  that is never rolled back (WG-5387).
