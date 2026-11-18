# Protocols

Types for everything here are in `crates/protocol`.

This is the prototype's protocol. The specification's gateway requirements are the target:
sign-in, sign-out and the WebSocket upgrade must carry a request token that the gateway issued to
the browser (`gwy-256`, `gwy-257`), and the session cookie carries the host name prefix (`idn-262`).
The guards below are what the prototype checks today.

## Browser to BFF: HTTP

All under `/api`, JSON in and out. The tenant is taken from the request's host name.

| Method and path | Purpose | Guard |
|---|---|---|
| `GET /api/session` | Who am I: roles, what I may attempt, CSRF token | none |
| `POST /api/login` | Start a session; sets the cookie | allowed origin, `x-strata-request: 1` |
| `POST /api/logout` | End the session; clears the cookie | allowed origin, CSRF token |
| `GET /api/view?path=/posts/x&q=...` | The view model for an address | none (rows are filtered by policy) |
| `POST /api/records` | Create, update or delete a record | allowed origin, CSRF token |
| `POST /api/preview` | Constrained Markdown to blocks | allowed origin |
| `GET /api/lab/overview`, `GET /api/lab/runs/:id` | Impact Lab reads | may manage schema |
| `POST /api/lab/run`, `/api/lab/replay`, `/api/lab/verify` | Impact Lab actions | allowed origin, CSRF token, may manage schema |
| `GET /api/diagnostics` | Machine-readable state of both services | may manage schema |
| `POST /api/dev/expire-session`, `/api/dev/reset` | Fixtures | `STRATA_DEV=1`, allowed origin |

The session cookie is `strata_sid`: `HttpOnly`, `SameSite=Strict`, `Path=/`, `Secure` when
`STRATA_SECURE_COOKIES=1`. It holds a random token; the backend stores only its SHA-256.

A failure is always

```json
{ "code": "invalid", "message": "...", "kind": "ConstraintViolated",
  "failed": [{ "rule": "constraint:post.slug_format", "passed": false, "detail": "..." }],
  "correlation": "c-12-a3f09c" }
```

with `code` one of `unauthenticated` (401), `forbidden` (403), `not_found` (404), `conflict`
(409), `invalid` (422), `throttled` (429), `unavailable` (503), `internal` (500). `failed` is the
kernel's evidence for a rejection, which is how a form knows which field to mark.

Every request may carry `x-correlation-id`; the worker generates one per request. It is logged by
the BFF and the backend, stored in the domain events the request causes, and comes back on the
WebSocket with each resulting notice.

## Browser to BFF: WebSocket protocol v1

`GET /ws`, upgraded only for an allowed `Origin`. Text frames, one JSON object each, discriminated
by `type`. Frames over 16 KB, unknown types and unknown fields are refused.

Client to server:

| Type | Fields | Meaning |
|---|---|---|
| `subscribe` | `req`, `sub`, `objects?`, `after?` | Start a subscription. `after` replays visible events above that sequence |
| `unsubscribe` | `req`, `sub` | Stop it |
| `ack` | `n` | Highest delivery counter processed |
| `ping` | `nonce` | Heartbeat |
| `cancel` | `req`, `run` | Stop a simulation |
| `diag` | `level`, `event`, `correlation`, `detail` | A browser log line, development only |

Server to client:

| Type | Fields | Meaning |
|---|---|---|
| `welcome` | `v`, `conn`, `head`, `session`, `heartbeat_ms` | First frame. `head` is the tenant's current event sequence |
| `ok` / `error` | `req`, (`code`, `message`) | Answer to a request |
| `event` | `sub`, `n`, `seq`, `correlation`, `notice` | A change the subscriber may know about |
| `resync` | `reason`, `head` | Too far behind: read everything again |
| `pong` | `nonce`, `head` | Heartbeat answer |
| `session` | `state` | The session ended (`expired` or `revoked`); the connection closes |
| `run` | `run`, `kind`, `payload` | Simulation progress, result, or a verification report |

A `notice` is `{kind: "upsert", object, id, version}`, `{kind: "remove", object, id}`,
`{kind: "schema", version}` or `{kind: "membership"}`. It never contains record data.

Rules:

1. `n` rises by exactly one per `event` on a connection. A gap means a lost frame: reconnect.
2. `seq` is the tenant's event sequence. It may skip, because events a subscriber may not see are
   not delivered. A `pong`'s `head` tells an idle client it is up to date.
3. After reconnecting, subscribe with `after` set to the last `seq` processed. The server replays
   up to 500 visible events or answers `resync`.
4. More than 512 unacknowledged events, a full outbound queue (256 frames in the backend, 128 in
   the BFF), or three missed heartbeats end the connection. The client reconnects and resumes;
   the event log is durable, so nothing is lost.
5. A connection is bound to the identity it was opened with. Signing in or out reconnects.

## BFF to backend

HTTP on loopback under `/internal`, plus one upstream WebSocket per browser connection. Every
request carries `x-strata-service` (a shared secret generated at each start and passed to both
processes through their environment), `x-strata-site` (the tenant the BFF resolved),
`x-strata-session` (the browser's opaque token, if any) and `x-correlation-id`. The backend
refuses anything without the service token, including requests from a browser that finds the
port. It derives tenant and principal from the session it holds; a session that belongs to
another tenant than the site is treated as no session.
