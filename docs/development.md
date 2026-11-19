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
| `--fresh` | Delete the development database first |
| `--no-vite` | No Vite: the BFF serves `web/dist` (builds it if missing) |
| `--no-build` | Use the binaries already in `target/debug` |
| `--store memory` | Keep everything in memory |
| `--db PATH` | Another database file |
| `--backend-port`, `--bff-port`, `--vite-port` | Other ports |
| `--quiet` | Only warnings and errors on screen; everything still goes to the log files |

The service token is generated on each start and exists only in the two processes' environment.

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

`POST /api/dev/reset` (development only) empties the store and installs the bundles again.

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

`GET /api/diagnostics` (schema managers only), or the Diagnostics tab of the Impact Lab:
store capabilities, health and layout history; tenants with revision, schema version and event
sequence; live sessions (ids, never tokens); WebSocket connections with their subscriptions and
delivery counters; operations in flight; kernel transitions accepted and rejected by intent;
rejections by kind; the last fifty rejections with the checks that failed; the last fifty
published events; recent simulation runs. `GET /internal/health` on the backend needs no token
and reports the store's own integrity check.

## Tests

| Command | What runs |
|---|---|
| `cargo test --workspace` | 91 Rust tests: kernel behaviour and properties, substrate, storage conformance on both adapters, simulation, verification (with Lean when installed), backend and BFF as running services |
| `npm run test:web` | 20 tests of the frontend kernel as plain functions |
| `npm run check` | `tsc --checkJs` over the JSDoc-typed JavaScript |
| `npm run build && npm run e2e` | 26 Playwright tests on two private stacks (ports 4200/4201 and 4210/4211/5183) |
| `STRATA_BLESS=1 cargo test -p strata-verification` | Regenerate the Lean correspondence fixtures after an intended change |
| `npm run spec` | The specification gate: `cargo test --workspace`, `lake build`, the web kernel tests and `npm run check`, then Exegra and Concordance against `specifications/spec.jsonc`. Not the e2e tests. `node scripts/spec-gate.mjs --no-test --allow-recorded` reuses the recorded results. See `specifications/README.md` |

`node scripts/shots.mjs [base-url] [filter]` captures screenshots of a running stack into
`var/shots/` for visual review; the browser tests also save theirs under
`tests/e2e/screenshots/`. `python scripts/covers.py` redraws the cover plates.

## Environment

| Variable | Service | Default |
|---|---|---|
| `STRATA_SERVICE_TOKEN` | both | none; required, at least 24 characters |
| `STRATA_DEV` | both | unset; `1` enables development logins and fixture endpoints |
| `STRATA_BACKEND_ADDR` | backend | `127.0.0.1:4101` (must be loopback) |
| `STRATA_STORE`, `STRATA_DB` | backend | `sqlite`, `var/strata.db` |
| `STRATA_TENANTS`, `STRATA_LEAN_DIR` | backend | `tenants`, `lean` |
| `STRATA_SESSION_TTL_MS`, `STRATA_SESSION_IDLE_MS` | backend | 12 hours, 2 hours |
| `STRATA_BFF_ADDR`, `STRATA_BACKEND_URL` | BFF | `127.0.0.1:4100`, `http://127.0.0.1:4101` |
| `STRATA_SITES` | BFF | `127.0.0.1=fieldnotes,localhost=fieldnotes,tidewater.localhost=tidewater` |
| `STRATA_ORIGINS` | BFF | every site host on the BFF port and on 5173 |
| `STRATA_WEB_DIST` | BFF | `web/dist` |
