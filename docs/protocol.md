# Protocols

Types for everything here are in `crates/protocol`.

This is the prototype's protocol. Sign-in, sign-out and the WebSocket upgrade carry a request token
(`gwy-256`, `gwy-257`) and the session cookie carries the host name prefix and `Secure` (`idn-262`).

A browser with no session is given a request token with its session reply (`csrf_token` in
`GET /api/session` and in the page's bootstrap data): a keyed hash of the site and an expiry
under a secret the gateway draws at start. A signed-in browser carries its session's own
`csrf_token` instead. Both go in `x-csrf-token`, or in `?csrf=` on the WebSocket address, where
a browser cannot set a header.

## Browser to BFF: HTTP

All under `/api`, JSON in and out. The tenant is taken from the request's host name.

| Method and path | Purpose | Guard |
|---|---|---|
| `GET /api/session` | Who am I: roles, what I may attempt, the request token | none |
| `POST /api/login` | Start a session; sets the cookie | allowed origin, request token |
| `POST /api/logout` | End the session; clears the cookie | allowed origin, request token |
| `GET /api/view?path=/posts/x&q=...` | The view model for an address | none (rows are filtered by policy) |
| `POST /api/records` | Create, update or delete a record | allowed origin, CSRF token |
| `POST /api/members/<operation>` | Change who holds which role: `assign-role`, `revoke-role`, `add-team-member`, `remove-team-member`, `invite`, `redeem` (inputs as the JSON body) | allowed origin, CSRF token; the kernel decides |
| `POST /api/preview` | Constrained Markdown to blocks | allowed origin |
| `GET /api/lab/overview`, `GET /api/lab/runs/:id` | Impact Lab reads | may manage schema |
| `POST /api/lab/run`, `/api/lab/replay`, `/api/lab/verify` | Impact Lab actions | allowed origin, CSRF token, may manage schema |
| `GET /api/diagnostics` | Machine-readable state of both services, for the caller's tenant only | may manage schema |
| `POST /api/dev/expire-session`, `/api/dev/reset` | Fixtures: end the caller's session; reinstall the caller's tenant | `STRATA_DEV=1`, allowed origin, CSRF token, the site's owner |

An allowed origin is one of the configured origins (`STRATA_ORIGINS`) whose host name serves the
same site as the request: a page of one tenant is not an allowed origin on another tenant's site.

The session cookie is `__Host-strata_sid`: `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`, no
`Domain`. It holds a random token; the backend stores only its SHA-256. Browsers accept a `Secure`
cookie from `http://localhost` and `http://127.0.0.1`, which is how local development works.

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

`GET /ws?csrf=<request token>`, upgraded only for an allowed `Origin` and the request token of the
session (the issued token for a browser with no session). Text frames, one JSON object each, discriminated
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
