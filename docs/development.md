# Development

## One command

```sh
npm run dev          # node scripts/dev.mjs
nu scripts/dev.nu    # the same, from Nushell
```

It builds the two Rust services, starts the backend on 127.0.0.1:4101, waits for it, starts the
BFF on 127.0.0.1:4100, starts Vite on 127.0.0.1:5173, prints the addresses and the development
logins, and stops all three on Ctrl+C.

| Flag | Effect |
|---|---|
| `--fresh` | Delete the development databases first (the control database and its tenant files) |
| `--no-vite` | No Vite: the BFF serves `web/dist` (builds it if missing) |
| `--no-build` | Use the binaries already in `target/debug` |
| `--store memory` | Keep everything in memory |
| `--db PATH` | Another control database; its tenant files go in `<stem>-tenants/` beside it |
| `--tenant-files DIR` | Keep the tenant files in another directory |
| `--backend-port`, `--bff-port`, `--vite-port` | Other ports |
| `--quiet` | Only warnings and errors on screen; everything still goes to the log files |

The service token is generated on each start and exists only in the two processes' environment.

The SQLite store keeps a control database (`var/strata.db`: the tenant directory and the session
directory, no tenant content) and one database file per tenant (`var/strata-tenants/<tenant>.db`)
holding that tenant's records, schema history, events, journal, snapshots, sessions and logins.
A database from before this layout, with every tenant in one file, is refused on start with a
message: development data is reseeded, so delete `var/strata.db` (with its `-wal` and `-shm`
files) or start with `--fresh`.

## Two kinds of live update

**Code.** Vite serves `web/` with hot module replacement. A stylesheet change is applied in
place; a script change reloads the page. Vite proxies `/api` and `/ws` to the BFF and keeps the
`Host` header, because the BFF picks the tenant from it.

**Data.** A committed change becomes a domain event, the outbox publisher hands it to the hub,
each connection asks the kernel whether its principal may know, and the browser's worker receives
a notice and reads the affected view again. No rebuild, no reload; focus, scroll position and
unsaved form text stay where they were. Try it: open the front page in one window, sign in as
`ines` in another, and change the accent colour under Publication settings.

The two do not interfere: one changes modules, the other changes state held in the worker.

## Tenants are files

`tenants/<name>/tenant.json` is a bundle: roles, object types, constraints, workflows, policies,
queries, views, members and seed records, with expressions in the authoring syntax
(`crates/substrate/src/syntax.rs`). `"template"` merges in the structural sections of another
file; `{ "$file": "posts/x.md" }` reads a field from a file. On start, any bundle whose tenant
does not exist is installed by issuing its intents through the ordinary command path, with the
timestamps written in the bundle, so the same bundles always produce the same state.

`POST /api/dev/reset` (development only, the site's owner signed in) removes the owner's own tenant
and installs its bundle again; other tenants are not touched. Development logins
(`accounts.dev.json`) are installed only in development mode, and a start without `STRATA_DEV=1`
removes any that still open with their published password and ends their sessions.

## Logs

Both services write one JSON object per line: `ts`, `level`, `service`, `event`, `correlation`
and event-specific fields. `scripts/dev.mjs` shows a short form on screen and appends the full
lines to `var/logs/backend.jsonl`, `bff.jsonl` and `vite.jsonl`.

The domain events (`tenant.created`, `schema.changed`, `member.changed`, `record.created`,
`record.updated`, `record.deleted`, `session.started`, `session.ended`) are written through the
generated `strata_model::telemetry` functions. Which fields each line holds is stated in the
specification (`event.logging`): the tenant and an allowlist of scalar fields, never record
contents, tokens or secrets. `record.updated` names the workflow `transition` an update took, such
as `publish` or `unpublish`. End-to-end journeys check these lines with `expectLog`.

To follow one request end to end, search for its correlation id:

```sh
grep '"c-41-9f3a02"' var/logs/*.jsonl
```

```
bff      bff.request        POST /api/records 200
backend  api.request        /internal/session 200        (the CSRF check)
backend  kernel.accepted    update_record revision 31, 22 checks
backend  api.request        /internal/mutate 200
backend  record.updated     tenant=fieldnotes seq=31 revision=31 object=post id=9 version=4 transition=publish
```

The same id is in the `correlation` field of the `event` frame each subscriber receives. In
development the worker also sends its own failures over the socket; they appear with
`service: "browser"`.

Passwords, session tokens and CSRF tokens are never passed to the logger. A browser test reads
the log files and fails if any of them appears.

## Diagnostics

`GET /api/diagnostics` (schema managers only), or the Diagnostics tab of the Impact Lab, reports
the caller's own tenant and nothing of any other: store capabilities, health and layout history;
the tenant's revision, schema version and event sequence; its live sessions (ids, never tokens);
its WebSocket connections with their subscriptions and delivery counters; operations in flight;
its kernel transitions accepted and rejected by intent; its rejections by kind; its recent
rejections with the checks that failed; its recent published events; its recent simulation runs.

`POST /internal/platform/diagnostics` on the backend reports every tenant, plus the audit entries
(such as a session token presented on another tenant's site). It needs only the service token and
the BFF has no route to it. `GET /internal/health` on the backend needs no token and reports the
store's own integrity check.

## Tests

| Command | What runs |
|---|---|
| `cargo test --workspace` | The Rust tests: kernel behaviour and properties, substrate, storage conformance on both adapters, simulation, verification (with Lean when installed), backend and BFF as running services |
| `npm run test:web` | The tests of the frontend kernel and client (`web/test/kernel.test.js`, `web/test/client.test.js`, `web/test/tenant-ui.test.js`) as plain functions |
| `npm run test:scripts` | The tests of `scripts/generated-share.mjs` |
| `npm run check` | `tsc --checkJs` over the JSDoc-typed JavaScript |
| `npm run e2e` | The Playwright tests (`tests/e2e/site.spec.js`, `hmr.spec.js`) on two private stacks (ports 4200/4201 and 4210/4211/5183). The stacks build the backend, the BFF and the frontend first. The Lean test fails when Lean is missing unless `STRATA_E2E_SKIP_LEAN=1` |
| `npm run e2e:generated`, `npm run e2e:steps` | The generated journeys, and the step fixtures against their own server |
| `STRATA_BLESS=1 cargo test -p strata-verification` | Regenerate the Lean correspondence fixtures after an intended change |
| `npm run spec` | The specification gate: `cargo test --workspace`, `lake build`, the web and script tests, the type check, `cargo fmt --check`, `cargo clippy -D warnings`, `cargo audit` and `npm audit`, then Exegra against `specifications/spec.jsonc` and the requirement-to-evidence check (`scripts/gate-evidence.mjs`). A tool that is missing is printed as NOT RUN and counted; `--require-all` fails on it. `npm run spec:browser` adds the three browser suites. `node scripts/spec-gate.mjs --no-test --allow-recorded` re-reads the recorded results. See `specifications/README.md` |

### Tiers

A routine check has to be quick enough to run before every change is stacked, so the Rust test binaries
are in two tiers, listed with their measured times in `scripts/test-tiers.json`:

- **Routine**: the unit tests of every package and the integration tests that run in seconds. The
  budget is two minutes of test time, compilation excluded, on the development machine; the list holds
  the budget and the measured total, and the gate fails when the total passes it.
- **Full**: everything, including the binaries kept out of the routine tier. Each of those is in the
  list with its measured time and the reason it is out (the whole-specification comparisons with Exegra,
  the Lean correspondence, the running-service suites). The gate (`npm run spec`) runs the full tier.

The gate also fails when a test binary exists that is in neither tier, or an entry names one that does
not, so a new test file has to be given a tier in the same change. Run builds with
`CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2`: the machine has 16 GB.

### The test suite budget

The whole test phase of the gate (`cargo test --workspace` build and run, `lake build`, the web and script
node tests, `tsc`) has a budget of `fullBudgetSeconds` (600) in `scripts/test-tiers.json`, and every test
binary's run has a cap of `binaryCapSeconds` (90). The gate times each step by wall clock and each binary
from cargo's `finished in` line, from the run it makes. The check "the test suite fits its budget" fails when
the phase total passes the budget or any binary passes the cap, and it names the steps and binaries over.
No flag skips or waives it. `--no-test --allow-recorded` judges the measurement stored in the gate record,
and fails if there is none. `node scripts/test-tiers.mjs check` judges the `measured` record in the tier list.

The way under the budget is to make tests faster, fold test binaries together, or remove tests that do not
prove a requirement. Raising `fullBudgetSeconds` or `binaryCapSeconds` needs a recorded owner decision, written
into the tier list with the reason; a change to either number is not a fix for a slow suite.

| Command | What runs |
|---|---|
| `npm run test:routine` | The routine tier: `cargo test --workspace` on the unit tests and the routine binaries (with the backend's `development` feature), then the web tests. Fails when its test time passes the budget |
| `npm run test:changed` | Only what the change touches: the files `git diff --name-only` names against the base (`--base <ref>`, default `integration`, else `main`) select the packages that own them and every package that depends on those, and their test binaries from both tiers run; the web tests and type check run when `web/` changed, the script tests when `scripts/` changed, Exegra validation and the specc freshness checks when `specifications/` changed. The very slow binaries are skipped unless `--slow`; `--dry-run` prints the plan |
| `npm run test:module -- <module>` | One module of the specification (a component id such as `kernel` or `mail-dispatcher`; `--list` names them): Exegra evaluates the module's claims in one process, the specc freshness checks run, the system-differential test runs the module's scenarios only (`STRATA_MODULE_SCENARIOS_FILE`), and the generated journeys of the uiScenarios that verify its claims run. Sets `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2`, fails when any step fails or is killed or the run passes 120 s (`--budget`), and records the step times and the machine's peak used memory in `var/module-runs/<module>.json`. `--dry-run` prints the plan |
| `npm run test:tiers` | The tier check alone: every test binary is in a tier. Reads manifests, builds nothing |
| `node scripts/test-tiers.mjs times <cargo test log> [--write] [--steps <json>] [--machine <note>]` | Times every binary in a `cargo test --workspace` log (the log must keep cargo's `Running` lines, which the gate's `var/test-results/cargo-test.txt` does). `--write` records the times, the marks on the very slow ones, and a `measured` record (date, machine, total, per-step seconds from `--steps`, per-binary seconds) in the tier list |

The two whole-specification comparisons with Exegra (`crates/system-differential/tests/exegra.rs`
and `crates/spec-compiler/tests/strata_differential.rs`) get their traces from one Exegra process per
job (`scripts/exegra-traces.mjs`, `STRATA_TRACE_SHARDS` and `STRATA_DIFFERENTIAL_JOBS`, two by
default) that loads the specification once; `exegra trace` once per scenario loaded it once per
scenario.

Dependencies are optimised in dev builds (`[profile.dev.package."*"]` in `Cargo.toml`) because the
tests run Argon2id, signature checks and SQLite; crates no test calls are listed there at the default
level to shorten a cold build.

`node scripts/shots.mjs [base-url] [filter]` captures screenshots of a running stack into
`var/shots/` for visual review; the browser tests also save theirs under
`tests/e2e/screenshots/`. `python scripts/covers.py` redraws the cover plates.

## Environment

| Variable | Service | Default |
|---|---|---|
| `STRATA_SERVICE_TOKEN` | both | none; required, at least 24 characters |
| `STRATA_DEV` | both | unset; `1` enables development logins and fixture endpoints |
| `STRATA_BACKEND_ADDR` | backend | `127.0.0.1:4101` (must be loopback) |
| `STRATA_STORE`, `STRATA_DB` | backend | `sqlite`, `var/strata.db` (the control database) |
| `STRATA_TENANT_FILES` | backend | `var/strata-tenants` (one `<tenant>.db` per tenant) |
| `STRATA_TENANTS`, `STRATA_LEAN_DIR` | backend | `tenants`, `lean` |
| `STRATA_SESSION_TTL_MS`, `STRATA_SESSION_IDLE_MS` | backend | 12 hours, 2 hours |
| `STRATA_BFF_ADDR`, `STRATA_BACKEND_URL` | BFF | `127.0.0.1:4100`, `http://127.0.0.1:4101` |
| `STRATA_SITES` | BFF | `127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater` |
| `STRATA_ORIGINS` | BFF | every site host on the BFF port and on 5173 |
| `STRATA_WEB_DIST` | BFF | `web/dist` |

### Resource limits

Every limit refuses with `429` (or closes the connection) and none of them is a quality knob: a
deployment sets them from the memory and cores it has.

| Variable | Service | Default |
|---|---|---|
| `STRATA_HEADER_TIMEOUT_SECS`, `STRATA_REQUEST_TIMEOUT_SECS` | both | 10, 30 (a request over its time is answered `408`; sockets are not subject to the second) |
| `STRATA_MAX_CONNECTIONS` | both | 2048 |
| `STRATA_WRITE_IDLE_TIMEOUT_SECS` | both | 30 (a connection whose client has not read for this long while there are bytes to send is dropped) |
| `STRATA_HOSTS_MAX_AGE_SECS`, `STRATA_HOSTS_MISS_INTERVAL_SECS` | gateway | 60, 5 (how old the tenant hosts learned from the backend may be; the least time between two calls for them) |
| `STRATA_TRUSTED_PROXIES` | BFF | none. Comma-separated proxy addresses; only their `X-Forwarded-For` is believed, the rightmost hop that is not itself a trusted proxy is the visitor, and from any other peer the header is ignored. Without it every visitor behind a reverse proxy shares the proxy's address |
| `STRATA_BFF_SOCKETS_PER_SESSION`, `STRATA_BFF_SOCKETS_PER_SOURCE` | BFF | 8, 32 |
| `STRATA_SOCKETS_PER_SESSION`, `STRATA_SOCKETS_PER_SOURCE` | backend | 8, 32 |
| `STRATA_DIAG_PER_SOCKET`, `STRATA_DIAG_PER_SOURCE` | backend | 50 per socket, 200 per source per minute |
| `STRATA_LOGINS_PER_SOURCE` | backend | 30 per source per minute |
| `STRATA_ARGON2_MEMORY_MIB`, `STRATA_ARGON2_WAIT_MS` | backend | 64 (three 19 MiB verifications at once), 3000 |
| `STRATA_LAB_WORKERS`, `STRATA_LAB_QUEUE`, `STRATA_LAB_RUNS_PER_TENANT` | backend | 2, 4, 2 |
| `STRATA_VERIFY_INTERVAL_MS` | backend | 60000 (and one Lean build at a time on the whole backend) |

The BFF states the visitor's source to the backend in `x-strata-source`, replacing anything a
browser sent under that name. A backend call without it (an operator's tool) is not limited per
source.
