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
| `GET /api/ui` | The manifest of the site tenant's interface definition: its hash, the schema version, and each part with its hash (a screen also with its route, view, title, layout, access and query parameters). `no-store` | none |
| `GET /api/ui/parts/<hash>` | One part of that definition: a screen with its forms, actions and live bindings, or the error messages. Only a part the site tenant's current definition holds; `private, max-age=31536000, immutable` | none |
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

## View replies: people, references and section shapes

`GET /api/view` answers a model whose `data` holds each section of the view: a list section is an
array of rows, a single section one row or null. Three things in it name people or records of other
types, and none of them is a bare identifier.

### The people table

A reply carries `people` beside `data`:

```json
{
  "data": { "posts": [ { "id": 7, "created_by": { "person": "p1" }, "fields": { "title": "..." } } ] },
  "people": { "p1": { "name": "Ada Okafor", "avatar": null, "self": true } }
}
```

- A person value (today the author of a record, `created_by`) is `{"person": "<handle>"}`, or null
  when the reader may not know the author. The handle is opaque and belongs to this reply: `p1`,
  `p2`, ... in order of first appearance. The principal string is not sent anywhere in the reply.
- `people[handle]` is `{"name": string | null, "avatar": string | null}`. `name: null` means the
  client draws "A member". `avatar` is a file address, or null (no avatar is stored yet).
- `self: true` is added to the entry of the reader's own handle, so a page can tell "this is mine"
  without a principal to compare. Other entries carry no `self`.
- A name is resolved only for a principal the reader was already given as an author, and only as far
  as the tenant's `member_names` setting allows: `members` (the default: a signed-in reader sees
  display names), `everyone` (anonymous readers too) or `nobody`. A reader always sees their own
  name. The display name is the one the account holds (the `display_name` of the session reply).

### Reference fields

A value of a reference field (and each item of a list of references) is

```json
{ "ref": { "object": "author", "id": 3, "title": "Mirela Ashby" } }
```

when the reader may read the target, with `title` null when the object type has no title field
and the target has no text for it, and

```json
{ "ref": null }
```

when the target is not readable: no id and no target are sent. The title is the text of the target
object type's title field: a required string field named `title` or `name`, else the first required
string field (the specification declares no title field yet; one declared later is used first). A
null reference stays null. Targets are read under the reader's policies, as the view's `include`
reads them, and `refs` (the included records) is unchanged. The reference fields of records inside
`refs` are not resolved and keep their bare ids.

### Section shapes in a screen part

A screen part (`GET /api/ui/parts/<hash>`) carries, beside `screen`, `forms`, `actions` and `live`:

```json
{
  "sections": {
    "posts": {
      "single": false,
      "shape": {
        "object": "post",
        "fields": [
          { "id": "author", "type": "ref<author>", "required": true },
          { "id": "title", "type": "str", "required": true }
        ],
        "fingerprint": "<64 hex digits>"
      }
    }
  }
}
```

- One entry for each section of the screen's view. `shape.object` is the object type the section's
  query reads; `fields` lists every field of it by id with its `type` (`str`, `int`, `bool`, `time`,
  `file`, `language`, `ref<object>`, `list<type>`) and `required`, and a `displayType` when one is
  declared (none is yet, so it is left out). An aggregate section has `aggregate` (its id) in place
  of `fields`.
- A section whose query declares the tree form (a descendants query, WG-5510) adds `depth` to its `shape`, as
  the type `"int"`: each of its rows carries the same key (see below), so a screen binds it as it binds a field.
- Each form and action in the part gains `shape`: `{"operation": id, "inputs": [{"name", "type",
  "required"}], "fingerprint"}`.
- A list section that declares row marks or row joins (see "Row marks and joins") carries them in its
  `shape` by name: `"marks": {"mine": {"object", "aggregate", "key", "second": null}}` and
  `"joins": {"reactions": {"object", "aggregate", "key", "second"}}`. `object` is the object type the
  lookup's query reads, `key` the reference field of that object that names the section's row, and
  `second` the field a join's groups are made by (null for a mark). They are part of the fingerprint.
- `fingerprint` is the SHA-256, in lower-case hex, of the canonical JSON of the shape without it. A
  change to a field's type, or to what an operation takes, changes the shape, the fingerprint and
  the part's hash.

### Descendants: the depth of a row

A named query may declare the tree form (WG-5510): the records below the record a parameter names, through a
single reference of the query's own object type. A product writes it in its query, with the keys of the
product file (`max-depth`, kebab-case like every key of a product):

```json
"queries": [
  {
    "id": "thread__replies",
    "object": "thread__comment",
    "params": ["root"],
    "order": [],
    "tree": { "via": "parent", "root": "root", "max-depth": 8 }
  }
]
```

`via` names a single reference field of the object type that refers to the record's parent, and `root` a
parameter of the query. A query with a tree takes no `order`, `limit` or aggregates. The product compiler refuses a
`via` that is not such a reference, a `root` that is not a parameter, and a `max-depth` outside 1 to 32
(`layer.tree-invalid`); the kernel refuses the same at install.

A view section that reads the query takes its root from another section of the view, as a `Section` argument
with `attribute` `$id` (the record the other section holds):

```json
"replies": {
  "query": "thread__replies",
  "args": { "root": { "Section": { "section": "thread", "attribute": "$id" } } },
  "include": [], "markdown": [], "single": false
}
```

The rows of such a section are the records below the root, the root excluded, in level order and then by
creation order. Each row object of the view model (`data.<section>[i]`) carries `depth`, the level of the row
below the root (1 for a reply to the root), beside `id`, `object`, `fields` and the other keys of a row. A reader
does not see a record below a record the reader may not read: the descendants of a hidden record are absent. A
tree deeper than `max-depth`, a cycle, or a record with more than 200 children makes the read fail with a
message that names the cause; the section is not sent shorter than the tree is.

A screen whose section reads such a query has `depth` in its section `shape` (above), so the client can bind it.

### Row marks and joins

A list section may declare, beside its rows, two things that depend on the reader and on other
records of the same tenant (WG-5599):

- a **mark** of a row is a boolean: true when the reader holds a record that the mark's aggregate
  counts for that row. The mark's query filters the records by the row (`record.<key> in
  param.<list>`) and may read the reader (`principal`); the aggregate is a count grouped by the
  reference field that names the row. Each record counts only as far as the reader may read it.
- a **join** of a row is the groups of a grouped aggregate of another object type, keyed by a
  reference to the row: the aggregate groups by that reference and by a second field, and the row
  receives its groups as a list. The reader's policies decide which records are counted, so a record
  the reader may not read is in no group.

Both are asked for the rows of a page in batches of at most sixteen row identifiers (the bound of an
`in` filter on an indexed field), one aggregate per batch, never one per row. A view that declares
one, or that reads its reader through a `principal` argument, is individual to its reader: its reply
is never stored as a copy that another reader is given.

In the reply each row of the section carries them by name, whether or not a row holds any:

```json
{
  "data": {
    "messages": [
      { "id": 12, "fields": { "title": "Offsite notes" },
        "marks": { "mine": true },
        "joins": { "reactions": [ { "key": "👍", "value": 3 }, { "key": "🎉", "value": 1 } ] } }
    ]
  }
}
```

- `marks[name]` is a boolean. `joins[name]` is a list of `{ "key", "value" }`: `key` is the value of
  the join's second field (a string here), `value` the aggregate's value (the count), and each
  group appears once. A join with no group for the row is `[]`.
- A row sends no principal: `mine` is the reader's own state, and no other reader's is in the reply.

In a product, a section declares them as `"marks": {"mine": {"query": "my_reactions", "aggregate": "mine", "key": "ids"}}` and `"joins": {"reactions": {"query": "reactions_by_message", "aggregate": "per_emoji", "key": "ids"}}`. The lookup's `args` give its query's other parameters, each as an argument source, so `"principal"` passes the reader to a parameter; a query filter may also read `principal` directly.

### Presentations: person fields and display types

A tenant's presentation of an object type (a `presentations` section of a product, `ui-presentation`
in the specification) declares the display type of its fields. Two things reach the reply:

- A field declared `"display": "person"` holds a principal in a string field. Its value in a row
  (and in an included record) is sent as `{"person": "<handle>"}`, with an entry in `people`, under
  the same rules as `created_by`: the value is one the reader's read of the record already gave
  them, the name follows the `member_names` setting, and the principal string is not sent. An empty
  value stays an empty string.
- A section's `shape` in the screen part gains, for each field a presentation declares,
  `"displayType"` (the display type, e.g. `"rich-text"`) and `"props"` (an object of strings, when
  the declaration has any), and the shape gains `"title"`: the presentation's title field, else the
  field the reply's reference titles use. A declared display type changes the shape's fingerprint.

The title of a reference (`{"ref": {"title"}}`) is the target object type's presentation `title`
when it declares one, else the choice described under "Reference fields".
