# The browser client

How the Strata web application should use the browser's own primitives: what it keeps, where, under
whose key, how it stays coherent with the server, how it works offline and reconciles what was
written there, and how it is tested. This is a design. Its safety properties are proved in Lean
(`lean/Strata/Client/`), its requirements are in `specifications/frontend.jsonc`, and the work is
listed at the end in dependency order. The coherence model is the backend working set's
(`docs/working-set.md`, `lean/Strata/WorkingSet.lean`) seen from outside the server. The design was
revised after five adversarial reviews; the answers are in the last section, and everything above it
is the current design.

## The recommendation in one page

Two decisions here are the owner's (2026-11-22): D-FE-11, how browser copies are reached by erasure,
correction, restriction and a reader's loss of permission, and D-FE-13, what happens to the person's
own typed text. Every other D-FE-* is a
proposal of this design, marked so in the specification.

- **Keep the shape that exists.** The pure kernel (`update`, `view`) in a dedicated worker, effects
  run by a driver, the main thread drawing a view model with DOM APIs only, generated platform
  screens, hash-addressed tenant parts (D-8250). Everything below is added around it, as kernel
  messages and effects, so the kernel stays pure and testable in Node.
- **Three stores, each for one kind of content** (D-FE-2). A service worker keeps only
  content-addressed files in the Cache API and passes everything else through. IndexedDB keeps view
  results, the write queue and drafts, encrypted per principal, each with the stamp it was read at,
  beside the partition's known stamp. The kernel worker's memory keeps the hot set and does the
  carry-forward.
- **The browser cache is a stamped copy** (D-FE-1). Every reply and frame carries a stamp: the tenant
  epoch, the generation and an erasure epoch. A value is drawn as current only at the newest stamp
  the browser has heard and within a contact bound of its last **contact**, the answer to a ping or a
  read dated by the request's send time; at the newest stamp past the bound it is drawn as "last
  checked" and not refetched; otherwise it is drawn labelled with its age. The contact bound limits how far
  the browser's stamp lags; the value is right only if the server's notices are complete.
  Correctness never depends on the cache: a miss is a read, and every write is decided again by the
  server.
- **The server owes the browser** (D-FE-8, D-FE-12): an erasure epoch per object type from a
  destruction log, read before opening sealed fields; an access generation raised by every
  membership, role, restriction or permission change, below which the browser keeps nothing; notices
  that name every key that may have changed, coalesced type-level frames for the types
  reference-following policies read, a volatile mark for clock-reading views, and a resync for
  out-of-commit changes. Today's notices do not meet this (see
  "What the server adds").
- **One leader tab per principal, chosen with Web Locks and kept by a heartbeat** (D-FE-3).
- **Offline writes are queued intents** (D-FE-5): a random local id, an idempotency key drawn once
  the input is final, the record stamp, a durable first-sending time, and a lookup by key that fences
  the key before any doubtful resend. Conflicts are shown to the person, never merged silently. Some operations never
  queue (D-FE-6).
- **Data at rest is encrypted under a key held per enrolled device** (D-FE-7). The share is fetched
  only with the device's secret, which sign-out deletes; the device id is not a credential. This
  protects against the next user of the device and a disk read after the key is gone, not against
  code running in the origin.
- **Erasure, correction, restriction and loss of permission reach browser copies at the next
  connection, within the realm's idle session lifetime** (D-FE-11, owner decision). Keeping data usable offline beyond a session is off by default; a realm
  opts in, and an opted-in device is reached only when it next connects.
- **The person's own typed text survives the revocation or erasure that orphaned it** until the
  idle session lifetime or the next erasure after it, with "copy and dismiss" (D-FE-13, owner
  decision). Other people's
  data in a draft goes at once.
- **Web Components draw, the kernel decides** (D-FE-4); **no third-party runtime code** (D-FE-9);
  **the kernel runs the uiScenarios headlessly in Node** (D-FE-10).

## Where the framing should change

The request is the right direction. Taken literally, a few parts of it would make the design worse.

1. **"Load things intelligently into the cache, as the backend's working set does" is half right.**
   The working set's *coherence* model carries over: stamps, a guarded commit, idempotency keys, the
   request-snapshot rule. Its *policy* machinery (GreedyDual-Size-Frequency, TinyLFU admission, a
   cell arbiter) does not: a tab's working set is one person's recent pages, and plain LRU by
   accounted bytes is enough. The browser also cannot do what makes the working set safe in checked
   mode, a generation read in the store's own transaction: it learns stamps from messages, so its
   guarantee is relative to what it has heard: how far its stamp lags is bounded by its last pong
   (`current_recent`), and the value is right only if the server's notices are complete
   (`current_value`).
2. **The browser cache sits above sealing, which the working set rejected for itself.** The gateway
   opens sealed fields before it answers, so the browser holds plaintext. A key destruction (D-350)
   raises no generation, so a cache keyed by generation alone serves erased values: the working
   set's own counterexample applies unchanged (`browser_generation_only_serves_erased`). The fix is a
   new server obligation, an erasure epoch read before opening, and a client one, a persisted known
   stamp checked in every put.
3. **Encryption at rest protects the next user, not the current session.** A non-extractable key
   cannot be exported, but any script of the origin can use it, and the key share arrives in script.
   Cross-site scripting or a malicious extension running while the person is signed in can copy the
   device secret, the share or the plaintext out. What encryption at rest buys is that the next person
   on the device, and someone who reads the disk after the key and secret are gone, cannot read the
   data, even with a session of the same principal. The design and the proofs claim that and nothing
   more.
4. **"A session key" is the wrong lifetime.** Tying the data key to the session makes every expiry
   destroy the person's unsent offline work. The share belongs to an enrolled device instead:
   expiry locks the data, re-authentication as the same person unlocks it, sign-out and revocation
   destroy it.
5. **"Applied at most once" is false as stated for a create**, because the server keeps an
   idempotency key for a retention and a create has no record to guard. It is true from two local
   rules (the server keeps a committed key for the retention; the client never sends a key past it),
   and for an update or delete with an epoch and version stamp it holds whatever is forgotten
   (`at_most_once`, `stamped_at_most_once`).
6. **Notice completeness, which a carried-forward cache needs, is false against today's server.**
   See "What the server adds". The design states it as an assumption of the value theorem, gives the
   server work that would make it true, and makes "current" expire after the contact bound; that
   bounds how stale the stamp is, and nothing about the value, when notices are incomplete.
7. **"Generated parts in the Cache API" is right; "generated code" mostly is not a cache question.**
   `web/src/generated` is compiled into the hashed bundle by Vite.
8. **A SharedWorker is not the multi-tab answer yet.** Chrome for Android re-enabled it only in the
   Chrome 148 beta in 2026 (caniuse lists support from 151), and Samsung Internet does not support it.
   Web Locks and BroadcastChannel are in every engine the platform supports, and work from dedicated
   workers.
9. **A frontend part of the specification cuts across the split by consumer.** It is added as an
   *area part* that `scripts/split-spec.mjs` fills by id prefix, so the tools see the same resolved
   document; `--verify` reports an item outside its part. Removing the area from `AREA_PARTS` and
   running the script moves every item back to its consumer part.

## The current state

| What | Where | What it does |
|---|---|---|
| Kernel | `web/src/kernel/update.js` (1,284 lines), `view.js`, `types.js` | `update(state, msg) -> {state, effects}`, `view(state)`; time and randomness arrive on the message (`at`, `rand`); request ids, correlation ids, a `generation` counter that drops replies to requests made before user state was cleared (`USER_KINDS`, `LATEST_ONLY`) |
| Effects | `web/src/kernel/types.js:75-90` | `http`, `ws_connect/send/close`, `timer`, `push_url`, `store` (localStorage), `scroll_top`, `passkey`, `log` |
| Idempotency keys | `update.js:192-217` (`newKey`, `call`) | A create carries `idempotency-key`, reused for a retry of the same body (D-6595); the key lives in kernel memory only |
| Driver | `web/src/worker/main.js` | Runs the kernel, executes effects (fetch, one WebSocket, timers), posts `[model, applied]` after a `JSON.stringify` dedupe |
| Main thread | `web/src/main.js` | Starts the worker through a Trusted Types policy, draws with `components/dom.js`, delegates DOM events to typed messages; no application request |
| Renderer | `web/src/components/dom.js` | Virtual nodes patched in place with DOM APIs; URL attributes filtered (`safeUrl`); focused controls keep their value |
| Views | `web/src/views/*.js` | `shell(model)` and pages as virtual nodes; `tenant.js` draws the interpreter's page model |
| Tenant interface | `web/src/kernel/tenant-ui.js` | Manifest and parts by hash in kernel memory, held parts during an upgrade, lost-draft notices |
| Generated | `web/src/generated/*.js` | Routes, contracts, forms, actions, live bindings, errors, client machines, from the specification |
| Trusted Types | `web/src/trusted-urls.js`; CSP in `crates/bff/src/routes.rs:2530` | One policy, `strata-worker`; `require-trusted-types-for 'script'`; `worker-src 'self'`, `font-src 'self'`, `connect-src 'self'` |
| Notices | `crates/substrate/src/subscribe.rs:261-470` (`Notice`, `notice`) | `Upsert`, `Remove`, `Schema`, `Membership`, `Setting`, `Ledger`, `Segment`; record visibility evaluated per changed record at the event's time; `MemberChanged` notifies owners and the subject only |
| Idempotency at the store | `crates/storage-api/src/lib.rs:169-206` (`idempotency_retention`), `conformance.rs:880` | Retention per entry (seven days for operations, longer for automation steps); only commits are recorded |
| Persistence | `main.js:41-47, 97-103` | localStorage for the theme only. No IndexedDB, no Cache API, no service worker |
| Multi-tab, offline | none; `worker/main.js:97-103` | Each tab opens its own socket; a socket closed while `navigator.onLine` is false is not taken as refused |
| Third-party code in the browser | `kernel/factors.js:18` (`uqr`), `styles/main.css:5-8` (`@fontsource-variable/*`) | The QR encoder is bundled code; the fonts are bundled into `/assets`, their files and CSS come through npm |
| Tests | `web/test/*.test.js`, `web/test/support/hot-reload-sim.js`, `tests/e2e/generated` | Kernel tests in Node; a seeded multi-tab simulator of tenant parts against a fake gateway; Playwright journeys from the uiScenarios |

`docs/protocol.md:86-87` lists four notice kinds; the code has seven. The protocol document is
corrected by follow-up 2.

## Constraints

- **The kernel stays pure** (K1 for the browser): no fetch, no storage, no crypto, no clock. Storage
  and crypto are effects; their results are messages.
- **The server is the authority.** A browser copy can cost a read; it can never change a decision.
- **Copies have rules** (CPY-001 to CPY-014, PRV-351 to PRV-353): each copy class has a row with
  owner, key, freshness bound and erasure point. The browser copies are three new rows; D-FE-11
  states how they meet the erasure and rectification requirements.
- **Sessions** end 12 hours after last use or 14 days after sign-in by default, and a realm may
  shorten both (D-700, IDN-259); the cookie is `HttpOnly` (D16).
- **No markup is parsed** (D18); **no client framework** (UXA-700, D-28); no third-party runtime code
  (D-FE-9).

## The primitives, weighed

### Service worker and the Cache API

Options: (a) no service worker; (b) a service worker that caches content-addressed files only;
(c) a service worker that also caches API reads and decides freshness.

**Recommendation: (b).** A file named by its hash cannot be stale. Freshness decisions belong to the
kernel, which knows stamps, sessions and principals; a service worker deciding them would be a second
coherence layer that sees every principal's responses. Concretely:

- `/assets/*` and `/api/ui/parts/<hash>`: cache-first, one cache per build, parts dropped when no
  manifest held by any partition lists them.
- **Navigations**: network-first **with a timeout** (lie-fi otherwise hangs the page); on failure or
  timeout the service worker serves a stored **shell**, `GET /app-shell`: `index.html` without the
  bootstrap block, with the page's CSP. The bootstrapped page carries the session and the request
  token (D17) and is **never** stored.
- Everything else (`/api/*` except parts, `/ws`, `/federation`, `/keys`, `/scim`): not intercepted.
- **Updates**: the new script installs its cache and waits; it never activates while the leader is
  draining the queue. The server refuses queued writes from a client older than a minimum version it
  states, and the new kernel rebuilds a queued intent from its stored form values against the new
  contracts or asks the person (contract skew; follow-up 9).
- **Registration** goes through the Trusted Types policy: `acceptWorkerUrl` gains the exact path
  `/sw.js`.

### IndexedDB

Options: localStorage (synchronous, main thread only, strings), the Cache API for JSON (no
transactions), IndexedDB, the Origin Private File System (no transactions across records).

**Recommendation: IndexedDB**, used from the kernel worker only, one database per site:

| Store | Key | Value | Encrypted |
|---|---|---|---|
| `partitions` | principal tag | device id and device secret (in the clear: fetching the share needs the secret, so it cannot be under a key derived from the share), current key id, the tenant epoch `e` with the known generation, floor and erasure epochs of that epoch, time of the last contact, leader heartbeat, offline flag | no personal data; the secret is protected as the threat table says |
| `keys` | principal tag | the non-extractable derived keys, only when the realm opted in and the person kept them | the keys themselves |
| `views` | `[principal tag, HMAC(index key, view key)]` | the stamp it was read at, last read, bytes, IV, ciphertext | yes |
| `queue` | `[principal tag, local id]` | state, attempts, first-sending time (server clock, written before the first byte), key (absent until the input is final), dependency, decided stamp, IV, ciphertext of the intent and of the base values | yes |
| `drafts` | `[principal tag, HMAC(form key)]` | IV, ciphertext of the person's typed text, and separately of values that came from the server | yes |
| `site` | `manifest` | the last interface manifest (public, D-8250) | no |

**Stamps.** A record persists the stamp it was read at, never a carried-forward one; carry-forward
happens in the worker's memory, so a heartbeat rewrites nothing on disk. The encryption's additional
data binds a ciphertext to its principal tag, store, key and erasure epoch only, so it never has to be
re-encrypted when a notice carries it forward. After a reload the worker starts from the persisted
read stamps and the known stamp, and an entry whose read stamp is older than the known one is stale
until a reply or a frame brings it forward.

**Encryption never runs inside a transaction.** An IndexedDB transaction commits as soon as it is
idle, so it cannot span a WebCrypto `await`. Every write is therefore computed (and encrypted)
outside, then **verified and written in one short `readwrite` transaction** over the partition row and
the target store, which reads and compares before writing:

- a put drops a value whose erasure epoch or tenant epoch is older than the known one, or read below
  the floor, raises the known stamp, deletes what an older epoch or floor left, and writes the
  value unless the store holds it at a later generation (`Cache.put`; `rest_one_epoch`,
  `rest_above_floor`; without the checks a purge is undone, `unguarded_put_resurrects`,
  `unfloored_put_resurrects`);
- every write names the key id it encrypted under and is refused unless it is the partition's current
  key id (`rot_stale_append_refused`);
- every write that depends on a value read earlier (a re-encryption, the rewrite of a queued write's
  local id, a person's edit) names the record's version it read and is refused unless the record is
  still at it, a compare-and-swap (`rot_no_lost_update`); a refused write is recomputed and retried.

Queue appends ask for `{durability: 'strict'}`, which is a hint (Chrome's default is relaxed, and
strict does not promise an immediate flush). "Saved on this device" means the transaction completed;
it is not a guarantee against power loss. The queue is bounded per principal (entries and bytes); past
the bound a new write is refused with a message, never queued silently.

### The worker's memory hot set

The decrypted entries of recent pages, the manifest and its parts, the session. **LRU by accounted
bytes**, a budget per tab to be measured, the current view and manifest exempt. Cleared on sign-out,
expiry and principal change.

### Multiple tabs

| | SharedWorker owning socket and cache | Leader chosen with Web Locks, BroadcastChannel to followers |
|---|---|---|
| Support | Chrome desktop, Firefox, Safari 16+; Chrome for Android only from 2026; not Samsung Internet | Web Locks: Chrome 69, Firefox 96, Safari 15.4; BroadcastChannel everywhere; both in dedicated workers |
| Failure of the owner | A crash takes every tab's kernel | The lock is released when the leader's worker ends |
| Kernel per tab | No: one kernel would serve every tab | Yes, unchanged |

**Recommendation: the leader, kept by a heartbeat.** Each tab's worker requests the exclusive lock
`strata/<principal tag>`. The holder opens the socket, drains the queue, posts notices (never record
data) on the BroadcastChannel of the same name, and writes a heartbeat time to the `partitions` row.
A background tab can be frozen while it holds the lock; a follower that sees the heartbeat older than
its bound takes the lock with `steal: true` and the old leader, when it thaws, finds its lock lost and
stops. Any tab may append to the queue. Releasing the lock on `pagehide` for the back-forward cache
is untested: Chromium is changing how held Web Locks affect the cache, so follow-up 8 tests it rather
than assuming it. Two tabs that both send a write are harmless (`at_most_once` allows any number of
senders).

### Web Components

**Recommendation: leaf renderers in the light DOM** (D-FE-4). An element holds no application state;
it takes properties (a property channel in `dom.js`: an attribute named `.options` is set as the
property `options`) and emits `CustomEvent`s. Light DOM because labels and ARIA references cannot
cross a shadow root and the uiScenarios' role queries must find everything; Shadow DOM only for a
leaf with no cross-references (`<strata-qr>`). Form-associated custom elements only for a control
with no native element (a record picker for reference fields). No Declarative Shadow DOM: there is
no server rendering (D17) and using it from script parses markup (D18).

### Trusted Types and CSP

Keep the single policy `strata-worker`, extended to `/sw.js`; no `default` policy. Trusted Types ship
in Chrome 83, Firefox 148 and Safari 26 (caniuse); elsewhere the same check runs on the string. The
CSP needs no new source; `/app-shell` carries the page's CSP.

### Structured clone and transferables

Post the view model as the string the driver already made for its dedupe, and parse it on the main
thread (one serialisation instead of two). Transfer an `ArrayBuffer` only for bulk binary. Keys,
ciphertexts and cache plaintext never cross to the main thread.

## The kernel and its event model

The kernel stays `update(state, msg) -> {state, effects}`. New messages: `store_loaded`,
`store_written`, `queue_loaded`, `drafts_loaded` (storage driver, after decrypting); `keys_ready`,
`keys_gone` (key driver); `leader`, `peer` (Web Locks, BroadcastChannel); `connectivity`,
`visibility` (from the main thread); `pong` (with the echoed ping time); `storage`, `sw`. New effects:
`store_get`, `store_put` (the guarded put), `store_purge`, `queue_append`, `queue_mark`,
`queue_resolve` (the atomic rewrite of a local id), `queue_remove`, `key_fetch`, `key_keep_offline`,
`erase_partition`, `lead`, `broadcast`, `ping`, `persist_storage`, `estimate_storage`,
`activate_update`.

1. **One message at a time.** The worker runs one message to completion; async work is an effect
   whose result is a later message. A put, a resolve and a purge are each one IndexedDB transaction.
2. **The kernel decides; drivers execute.** What is current, what a notice invalidates, what to
   persist, when to send a queued write: kernel (`cache.js`, `queue.js`, the JavaScript of
   `Strata.Client.Cache` and `Strata.Client.Queue`). Encryption, IndexedDB, locks, sockets: drivers.
3. **Connectivity is learned from outcomes**; `navigator.onLine` is a hint.
4. **Time comes on messages, and is re-read before drawing.** A timer every heartbeat interval lets
   the kernel relabel entries whose last contact is older than the contact bound. A hidden or frozen tab
   and a sleeping laptop get no timers, so every `visibility` message (and the first message after a
   gap in timers) re-reads the time and re-evaluates "current" before the main thread paints.
5. **Cache first, then the network, never instead of it.** A navigation draws a current entry and
   asks nothing; draws a non-current one labelled and asks; draws nothing it lacks, and asks.

## The cache and its coherence

### The model

The browser holds entries stamped `(e, g, x)`, and for the current tenant epoch `e` its known
generation, its erasure epochs, its **floor** and the time of its last contact. Generations are
ordered only within a tenant epoch: a restore, a re-created tenant or a lease acquisition rotates the
nonce and starts a new one, whose counter may start again from 1. The tag `e` is random and unordered,
so with it the server sends the epoch's **ordinal**.

**Where the ordinal comes from.** At every nonce rotation the server sets the ordinal to
`max(previous + 1, rotation time in milliseconds)` and persists it with the nonce. The time term is what
keeps it monotone across the events that lose or never had the previous value: a restore from an older
snapshot (the snapshot's ordinal is older, but the rotation time is now), a control-directory restore,
a re-created tenant whose generation counter restarts at 1 under `Generation::first` (no previous value
at all), and an import into another cell carrying an older ordinal. It relies on the clock not going
back past the last rotation; rule (b) covers the case where it does.

**The client's two rules.** The browser keeps, with its epoch, the send time of the request whose
answer made it switch. (a) A message of a **higher ordinal** switches it. (b) An **answer to a request
sent after its last switch**, matched by its echoed nonce, carrying a different tag, switches it
**whatever its ordinal**: a server that answered a request sent after the switch answered from state at
least as new as the switch, so a lower ordinal there means the ordinal went down, not that the answer is
old. The ordinal only orders answers to requests sent before the switch, and frames, which answer no
request. Switching purges every entry and resets the known generation, floor and erasure epochs
together, in one transaction; anything else of another tag is ignored. No list of left epochs is kept
(`tenant_epoch_purges`, `older_epoch_ignored`, `rest_same_tenant_epoch`). **Recovery**: whatever
arrived before, any answer sent in the server's current epoch brings the client to that epoch
(`contact_reaches_current_epoch`); after two rotations with answers arriving out of order (epoch 2, then
a late 1 to an earlier request, then 2) the client ends current in epoch 2
(`out_of_order_epochs_recover`); an answer to a later request is taken whatever its ordinal
(`later_request_is_authoritative`), so a client at ordinal 5000 reaches a re-created tenant at ordinal 1
(`lower_ordinal_recovers`). Recovery needs such an answer to arrive: it is a liveness assumption, not a
theorem about the network. Rule (b) can move a client to an older epoch when an answer to a later
request was served before an answer to an earlier one; that is safe (everything is purged, and the
model keeps the client's epoch at or below the server's), and the next answer moves it on. The kernel
drops a pending nonce when its request is answered, abandoned or older than the contact bound, so the
set of unanswered nonces stays bounded. Without the reset, the new tenant's replies at generation 1 fall below
the old floor and are all dropped (`restarted_counter_dropped_without_reset`). Every lease failover
rotates the nonce, so it resets every connected client's cache; its rate is measured in WG-5435.
Three kinds of message carry stamps, each after a delay the browser cannot measure:

- an **answer to a request** the browser sent, a pong to a ping or a reply to a read, carries the
  stamp current when the server answered: its tenant epoch, generation, the erasure epochs of the
  types concerned, and the **floor generation**: the greater of the principal's access generation (its
  last membership, role, restriction or permission change) and the tenant's **policy generation**
  (its last change of a Read policy, or of the teams and roles a policy reads, which changes what
  every principal may read without a new schema version). It is a **contact**, dated by the request's send time, which the
  browser measured itself: however late it arrives, everything newer than its stamp began after that
  time. A reply also carries the stamp it was read at, which stamps the entry. A cached answer would
  carry an old stamp, and dating it by a new request would make it a false contact. `fetch` does not
  say whether a response came from the HTTP cache, so the driver issues view reads with
  `cache: 'no-store'`, every request carries a fresh nonce, and the server echoes it in the answer's
  stamp; the kernel takes an answer as a contact only when the nonce matches the request it is
  answering (C6);
- a **frame** (a notice or a heartbeat) carries the stamp it was sent with, its `from`, the floor
  generation and every key touched at any point in `(from, to]` (C2); it carries entries forward and raises what the browser
  knows, but it is **not a contact**, because nothing in it says when it was sent.

An answer is **current** iff its stamp is the known stamp and the last contact is at most `B` old;
**checked** iff its stamp is the known stamp but the contact is older (drawn as "last checked at",
and **not fetched again**, so a slow or missing pong never causes a refetch loop); **stale**
otherwise (drawn with its age, and fetched). `B` is a freshness choice, not a latency measurement.

**Contacts across tabs.** The leader pings on welcome and every heartbeat interval; any tab's read is
a contact for that tab. The leader relays a contact to followers on the BroadcastChannel as an epoch
time (`performance.timeOrigin + performance.now()` at the request's send), never a raw
`performance.now()`, whose origin differs per tab.

**Heartbeats and pongs.** The server pushes a heartbeat frame on each connection every interval;
it carries the chain (`from`, stamp, no keys) and carries entries forward. A pong is not a chain frame:
it answers a ping and dates a contact. A pong's stamp can run ahead of the chain; the browser then
knows a newer generation than its entries, which stay checked (not stale) once their stamp is
carried to the known one by the next heartbeat, and stale until then.

Proved, in one model of the whole stamp with carry-forward (`lean/Strata/Client/Cache.lean`):

- **the lag of the stamp, with no assumption on notices** (`current_recent`): every generation and
  erasure epoch newer than the stamp of a current answer began within `B` before the read. A bound on
  the stamp, **not the value** (`fresh_contact_still_stale`);
- **the value, under complete notices** (C2, a hypothesis, never a filter): a current or checked
  answer is the server's opened value at the known stamp (`current_value`, `run_coh`,
  `current_answer`);
- nothing at rest from an older erasure epoch, read below the floor, or from another tenant epoch;
  hearing any of them purges at once, and the floor covers a policy change for every principal
  (`contact_floor`) (`rest_one_epoch`, `rest_above_floor`, `hear_purges`, `hear_access_purges`);
- a reply read and sent at the current stamp is read back as current (`receipt_read`,
  `read_after_commit`).

What fails without each piece: an incomplete notice (`incomplete_notice_serves_stale`,
`fresh_contact_still_stale`); an epoch read after opening (`stamp_after_open_serves_erased`); a put
without the persisted epoch (`unguarded_put_resurrects`) or without the floor
(`unfloored_put_resurrects`).

The model is that of one object type's erasure epoch; a view that depends on several types is
governed by each type's epoch, and the per-type instances compose by conjunction (C5, argued; the
server side is a conformance case in WG-5435: for a view that joins or follows references,
destroying a key changes the reply's epoch vector if and only if a sealed field the view opens
changes, including fields a Read policy or a derivation reads, which relies on `depends_on` being
complete, CPY-004).

### What the server adds (follow-up 2)

- **Stamps** on replies (read stamp and sent stamp), receipts, frames and pongs: `e` (a keyed tag of
  the generation nonce), `g` (WG-5389), the erasure epochs of the types concerned, and the floor
  generation, the tenant epoch's ordinal, and the nonce the request carried. View replies are
  `Cache-Control: no-store`.
- **Erasure epochs per object type, read before opening.** The server keeps a destruction log keyed by
  object type and sealed field, and derives an epoch per type from it: no audience class is
  enumerated (the kernel computes audience classes per result, and individual classes are unbounded).
  A key destruction raises, in its transaction, the epoch of every type with a field sealed under the
  destroyed key. A reply carries the epoch of each type its view depends on; a read takes them before,
  or in the same transaction as, opening its sealed fields (C3). A role change does not touch them, so
  the browser's known epochs never go down. **Disclosure**: a reader of a type learns that an erasure
  touched that type, not whose data it was. **Cost**: a raised epoch makes browsers refetch the views
  of that type; a purge scoped to the affected records stays an open question.
- **The floor generation.** These events, and no others, raise the tenant's **policy generation**, one
  tenant-wide write rather than one per principal: a change of a Read policy, and a change of the
  definition of a team or a role that a policy reads (which, per `crates/backend/src/aggregates.rs`,
  changes the schema without a new version). Adding or removing a member of a team, granting or
  revoking a role, a restriction (PRV-352) and a permission change raise only the **access generation**
  of the principals concerned; if team membership raised the policy generation, every member change
  would purge every browser of the tenant. The purge rate under both is measured in WG-5435.
  Every answer and frame carries the greater of the two; the browser purges everything read below it
  and keeps that floor persisted, so a late reply read before the change is dropped
  (`rest_above_floor`, `contact_floor`). A role change purges and raises the floor, and never lowers a
  known erasure epoch.
- **The lookup fence.** A lookup of an uncommitted idempotency key records a tombstone for it, kept for
  the key's idempotency retention, scoped like the key to the tenant, principal and contract, and
  checked in the same store transaction as a commit: a request held up in the network past the
  gateway's deadline still finds the tombstone and is refused. `fence_holds` models a tombstone that
  outlives every submission of its key, which this retention provides as long as the client never
  sends a key past its retention (`at_most_once`'s client rule); a client whose clock is set back could
  break that rule, which is why the client measures retention on the server's clock (Q4).
- **Frames chained per connection**, `from` and stamp on each; a heartbeat is a frame with no keys; on
  reconnect with `after = seq` the server replays from the generation recorded with that event, or
  answers `resync`.
- **Complete notices (C2).** The gaps against today's `notice` (`crates/substrate/src/subscribe.rs:354`)
  and the server work: membership notices to every subscriber whose visible set may change, a
  restriction notice, evaluation with the schema and members recorded with the event, a `resync` for
  out-of-commit writes, the three undocumented kinds documented; reference-following policies by
  **type-level frames** from `depends_on` (AGG-042), **coalesced per connection within a window** so
  a hot type costs at most one frame per window, with a measured rate criterion in WG-5435. Coalescing
  must not break C2: while a commit is deferred, every frame sent on that connection (the coalesced one
  and every heartbeat inside the window) names the union of what is deferred over its whole interval,
  or holds its `to` below the deferred commit; and
  clock-reading or unresolvable views marked **volatile** by the existing aggregates classifier less
  its principal condition, with a negative case.

### What the browser does

- A frame raises the known stamp, carries forward in memory every entry read in `[from, to]` that it
  does not name, and drops those read in that interval that it names; entries read before `from`
  stay as they are. On a busy tenant an entry fetched between heartbeats is therefore carried forward
  by the next heartbeat, instead of going stale.
- A new erasure epoch for a type deletes, in the same transaction, every view result, base value and
  server value in a draft of that type stamped with an older one; the person's own typed text follows
  D-FE-13.
- A new floor generation (a membership, role, restriction, permission or policy change) deletes every
  view result, base value and server value in a draft read below it, and raises the floor, in one
  transaction.
- A different `e`: purge and reset, as above. A `resync`: every entry becomes stale and is read again.
- **After signing in again** (or unlocking after expiry), the worker purges by the first answer's stamp
  and floor before it decrypts or draws anything from the partition.

## Offline writes

### The queue

A queued write is an intent the person decided while the browser could not reach the gateway or
while a reply was outstanding. It holds the contract and inputs, a **random 128-bit local id**
(random, so tabs and successive leaders never collide), the **decided stamp** (tenant epoch and
record version; none for a create), the base values of the fields it changes, its state, attempts,
and either:

- an **idempotency key**, drawn once the write's input is final and stored with it; or
- a **dependency** on another queued write and no key, while its input depends on that write's
  unknown result (an update of a record created offline, a reference to it).

**Resolving a dependency is computed outside a transaction and committed by compare-and-swap**:
when the earlier write's receipt arrives, the dependents' new inputs (the receipt's record id and
version in place of the local id) are computed and encrypted; then one short transaction checks that
each dependent is still at the version read and has no key, rewrites it, draws its key, records the
local-to-real mapping and removes the earlier write. A refused swap (another tab resolved or the
person edited it) is recomputed. `Queue.cstep`'s `resolve` and `edit` model this; `q2_sent` holds
because no client operation edits a write that has a key. A dependent is **decided again by the person**, not sent, when:

- the earlier write's outcome was learned only by a lookup by key, which returns the record id and
  not the version or values the person saw; or
- the created record differs from what the person saw in a field the dependent touches (a default
  the server computed, a workflow state, a derived field).

Open views and addresses that name a local id are redirected by the kernel to the real id from the
mapping; the mapping is kept until no queued write or open view names the local id.

Unsaved form text is a draft, not a queued write.

### Submitting against the guarded commit

The leader drains in order, one write at a time per record. Before the first byte of a write's first
sending, the time of that sending (on the server's clock, its offset learned from replies) is written
to the queue in its own transaction. Each write is sent with `idempotency-key`, `expected-version` and
`expected-epoch` (follow-up 3); for a contract whose policies follow references, the contract may ask
for a **tenant-generation guard** instead (follow-up 3), so that the write commits only if nothing in
the tenant changed since it was decided. The server answers a committed key with its recorded outcome;
otherwise its kernel decides the intent again on the current state and the store commits it only at
the expected stamp.

**A limitation the guard does not remove.** The version and epoch guard says the record is as the
person saw it; it says nothing of the records the decision also read (a referenced record, a workflow,
a quota). The server's kernel decides again on current state, so nothing invalid commits, but a write
can commit with a meaning the person did not see, for example an approval after a related record
changed. The tenant-generation guard is for the contracts where that matters, at the price of more
conflicts.

Proved (`lean/Strata/Client/Queue.lean`): an accepted first submission was allowed by the kernel and
found the tenant and record at the decided stamp (`submit_ok_guard`); a submission that does not
commit changes no record (`submit_not_ok_unchanged`); the classes (`submit_classes`); at most once from
the server's rule (keep a committed key for the retention) and the client's (never send a key past the
retention from its first sending) (`at_most_once`); for a stamped write, at most once whatever is
forgotten or restored (`stamped_at_most_once`); and the client's queue sends each key with one input
(`q2_sent`). Every queue theorem holds for any kernel decision `allow`; that the kernel's decision on
re-submission is the kernel's step is the working set's `submit_one_generation`.

**Retention and doubtful resends.** Queued writes carry a retention at least as long as the queue's
bound (the store already sets retention per entry, `idempotency_retention`). The client stops resending
a key a margin before its retention ends. A create whose first-sending record is missing (the tab died
between drawing the key and making the record durable) or older than the retention less the margin is
never resent until the server has been asked for the recorded outcome of its key. The lookup returns
the outcome and the record id, and says so when the record was created and has since been deleted.
When the key is not committed, the lookup **fences** it: the server records a tombstone, kept for the
key's idempotency retention and checked in the same store transaction as a commit (server work in
WG-5435), and refuses any later submission of that key, so the original
request still in flight cannot commit after the "not found" (`fence_holds`); the client then resends
under a new key. Without the fence a late original and the resend both apply
(`unfenced_lookup_applies_twice`). A lookup past the key's retention cannot fence a key that was
committed and forgotten, so past the retention the client does not resend: it shows the person's text
and a search.

### Conflict classes, and what the person sees

| Class | Server answer | What the person sees | What happens to the entry |
|---|---|---|---|
| Moved | `conflict` (version, epoch or tenant generation) | "‹Title› was changed after you edited it offline." Their version beside yours | The person chooses yours, theirs, or edits; the choice is a new intent with a new key |
| Gone | `not_found` | "‹Title› was deleted while you were offline." With a copy of your text and "Create as new" | Closed |
| Refused by the rules | `invalid` with `failed` evidence | The form again, the failing fields marked | Closed |
| Not allowed now | `forbidden` / `unauthorized` | "You can no longer change ‹Title›." Your own typed text, with "copy and dismiss" | Server values deleted at once; your typed text kept until dismissed, the idle session lifetime, or the next erasure (D-FE-13) |
| Needs you first | `unauthenticated`, `step_up_required` | "Sign in again to send 3 changes" / the step-up prompt | Held; resent with the same key afterwards |
| Waiting | network failure, `throttled`, `unavailable` | "3 changes waiting to send" | Retried with backoff, same key |
| Outcome unknown | no answer, or no durable first-sending record | "We could not confirm whether ‹Title› was saved." With "Check" | Never resent before the lookup by key; dependents decided again |
| Blocked | a write it depends on is unanswered or unknown | Shown under the earlier one | Waits with it |
| Client too old | the server's minimum client version | "Reload to send your changes" | Kept; the new kernel rebuilds it or asks |

### Durability, ownership, what may not be done offline

The queue survives reloads, crashes and browser restarts, subject to eviction. The leader drains; any
tab appends and tells the others. Only contracts the specification marks may be queued (follow-up 6;
D-FE-6 lists what never may).

**Sign-out works offline**: it deletes the browser's key, the partition's device id and its device
secret at once, so nothing left opens, whatever device id a session of the principal names elsewhere
(`offline_signout_erases`); a marker with no personal data asks the next online load to call
`/api/logout`, which ends the session at the server. A person with unsent changes is told that
signing out deletes them.

### The session expiring while offline

The worker ends the session locally when the session's idle or absolute lifetime passes without
contact: it drops the key from memory, deletes a kept offline key, and keeps the ciphertexts, the
device id and the device secret. Signing in again as the same person unlocks the queue
(`expire_then_same_principal_unlocks`); nobody else can (`unlock_needs_own_session`).

## Privacy at rest

### What is protected, against whom

| Threat | Protection |
|---|---|
| The next person on a shared device after sign-out, online or offline | Key, device id and device secret deleted in the browser (and the share at the server, online): nothing left opens, by this browser or by a session of the same principal that knows the id (`signout_erases`, `offline_signout_erases`) |
| A copy of the disk taken after sign-out, plus a session of the same principal elsewhere | The share is fetched only with the device secret, which the copy no longer holds; the id alone fetches nothing (`fetchById` in the model) |
| The next person after a session expired without sign-out | The first person's key is gone with the session; another principal's session cannot fetch the first's share (`no_cross_principal`, `expire_locks`, `unlock_needs_own_session`) |
| A lost or stolen device | Revoke that device from another one: its share is deleted and its copies are unreadable from its next reload (`revoke_device_erases`); signing out everywhere does the same for every device (`revoke_all_erases`) |
| A copy of the disk taken while the session is live | Not protected: the secret is on it. The design states it |
| Code running in the origin while the person is signed in (XSS, an extension) | **Not protected.** It can read the secret, the share or the plaintext |
| A device whose realm opted in to keeping data offline | The key survives reloads (`offline_key_survives_reload`); its expiry is a check against the local clock, so the device is reached only when it next connects (D-FE-11) |

### Device enrolment, the secret and the key share

- **Enrolment**: the first `POST /api/session/device-key` of a partition with no device enrols the
  device: the identity service issues a device id and a random 256-bit device secret, bound to the
  principal, and a random 256-bit share. It keeps the share and a hash of the secret, sealed at rest.
- **Fetching**: later requests send the device id and the secret; the service returns the share only
  to a session of the enrolled principal that presents the right secret. The device id may be listed
  on the sessions page; it is not a credential (`fetchById` lets any session of the principal name any
  id, and fetches nothing without that device's secret; it presents the partition's own secret, so it
  says nothing about an attacker holding another device's secret).
- **Guards**: a request token (D16), a rate limit per session and per device id, no step-up.
- **Storage and restores**: shares and secret hashes are sealed in the identity database; a restore of
  it re-applies the deletions recorded after its point (P2).
- **Lifetime and rotation**: a share lives for a fixed lifetime from enrolment (the absolute session
  lifetime, 14 days by default). Before it ends the leader rotates, in this order: (1) enrol a new
  share and secret and make them durable in the partition beside the old ones; (2) switch the
  partition's current key id to the new key; (3) re-encrypt the old records one at a time, each read
  outside a transaction and written back by compare-and-swap on its version; (4) delete the old share
  only when the current key id is new and no record is left under the old key. Every append and
  rewrite names the key id it encrypted under and is refused unless it is current, so a tab that has
  not heard of the switch is refused and retries under the new key. A crash at any point leaves both
  keys usable and the next leader resumes (`rot_no_loss`, `rot_no_lost_update`, `rot_finish_retires`);
  without the key-id check a stale tab's append outlives the old key (`rot_stale_append_breaks`), and
  without the compare-and-swap a re-encryption overwrites another tab's update
  (`rot_blind_write_loses_update`).
  Orphaned shares (a device signed out offline, a device never seen again) end with their lifetime.
- **Revocation**: per device, or every device (sign out everywhere, removal, erasure); signing out
  everywhere destroys every device's unsent work, and the confirmation says so.
- **Derived keys**: HKDF-SHA-256 over the share gives a non-extractable AES-GCM-256 key and a
  non-extractable HMAC-SHA-256 key; each record has a random 96-bit IV and additional data
  `principal tag ‖ store ‖ key ‖ erasure epoch`.
- **Online-keyed by default.** Keeping the derived keys in IndexedDB, so data is usable offline across
  reloads and beyond a session, is off by default; a realm opts in, and the person chooses it on a
  device they mark as theirs (D-FE-11).

### Erasure, correction, restriction and permission changes while offline (D-FE-11)

The owner's decision: erasure, correction, restriction (PRV-352) and a reader's loss of permission
reach a browser copy at the browser's next connection, and no later than the realm's idle session
lifetime (12 hours by default). The design meets it so: at
the next connection a new erasure epoch, a notice or a resync deletes or refreshes the copies; offline,
the copy is drawn only labelled, and when the idle session lifetime passes the session ends locally,
which locks the copy in online-keyed mode. A device whose realm opted in to keeping data offline is
reached only when it next connects. Ciphertexts left on disk after a session ended without sign-out are consistent with
this: their key is gone, so they cannot be read; their device id and secret remain, so the same person
signing in again unlocks them, and then the first answer's stamp and floor purge them before anything
is drawn. PRV-351, PRV-352, PRV-353 and, as
needed, CPY-013 and the permission-change requirements are amended to say this (WG-5449).

### The person's own typed text (D-FE-13)

The owner's decision, as a small state machine (`lean/Strata/Client/Drafts.lean`, FE-017):

| State | Event | Next |
|---|---|---|
| normal | a revocation or an erasure touching the draft | orphaned, recording the time and the erasure epoch, after the change, of the object type of the record the draft edits; server values deleted at once |
| orphaned `(x0, t0)` | an erasure epoch `x ≤ x0` of that type heard, before `t0 + idle` | orphaned (the text survives the change that orphaned it) |
| orphaned `(x0, t0)` | an erasure epoch `x > x0` of that type heard | typed text deleted |
| orphaned `(x0, t0)` | the time reaches `t0 + idle` | typed text deleted |
| any | "copy and dismiss" | typed text deleted |

Other people's data in a draft is removed at orphaning (`orphan_drops_server_values`); the typed text
survives the orphaning erasure (`survives_orphaning_erasure`) and goes at the next one or at the idle
lifetime (`gone_after_next_erasure`, `gone_after_idle`). Drafts and queue entries therefore keep the person's typed text and the values that
came from the server in separate ciphertexts, so the second can be deleted without touching the first.

### Storage eviction

- **Safari** deletes all script-writable storage of a site after seven days of Safari use without the
  person interacting with the site; a web app added to the Home Screen is exempt. Unsent work left
  that long is lost without the server knowing; the queue panel says so.
- **Quota**: on `QuotaExceededError` the driver evicts view results by last read, never queue entries
  or drafts; if a queue append still fails the person is told the change is not saved on the device.
- **Persistence**: `navigator.storage.persist()` exists in Chrome 55, Firefox 57 and Safari 15.2
  (caniuse); whether it is granted is each engine's decision. It is asked for only with offline kept.
- An evicted view is read again; eviction is never an error.

## No third-party runtime code

- **QR encoder** (`web/src/kernel/qr.js`, pure): byte mode, error correction M, versions 1 to 10,
  Reed-Solomon over GF(256), the eight masks with the four penalty rules, format and version
  information. Tests take expected matrices from ISO/IEC 18004's worked example and from fixtures
  frozen once from an independent encoder. `uqr` leaves `package.json`.
- **Fonts**: the variable `woff2` files vendored into `web/src/fonts/` with their OFL licences,
  subset, declared with `font-display: swap`; the `@fontsource-variable/*` packages leave. A build
  check fails when the bundle contains a module from `node_modules`.

## Testing

### The kernel runs the uiScenarios in Node

A Node runner (`web/test/scenarios/`) runs each of the 45 uiScenarios with the real kernel and view
functions: Node's `fetch` and `WebSocket` against the real gateway (cookie jar, `Origin` header),
storage over an in-memory map with the guarded-put semantics, Node's WebCrypto. No fake server. Roles
and names come from `shell(model)`'s virtual nodes through a small table. It answers `goto`,
`gotoPath`, `back`, `fill`, `submit`, `click`, `clickAction`, `press`, `expectTitle`, `expectRole`,
`expectText`, `expectNoText`, `expectCount`, `expectAttribute`, `expectScreen`, `expectNoReload`,
`recordRequests`, `expectRequests`, `expectNoRequest`, `apiCall`, `expectStatus`, `switchBrowser`,
`goOffline`, `goOnline`, `reload`, `waitFor`, `mark`, `capture`, `expectCookie`, `setCookie`; it
reports `axeScan`, `expectNoOverflow`, `expectBeside`, `emulate`, `focus`, `logScan`, `expectLog` as
skipped with their reason. Faults come from a seed at the driver's boundary (D19): lost, delayed or
duplicated replies, a dropped socket, a network cut, a reload between any two messages, a second tab
sharing storage and the lock. After every step it checks: no current entry with a stamp other than
the known one or a contact older than `B`; no entry at rest of an older erasure epoch; no queue entry
sent without a key; no plaintext of another principal in a kernel's state.

### In the browser, and offline faults

Playwright covers drawing and accessibility, the service worker (offline reload, navigation timeout,
update), IndexedDB with real encryption, Web Locks failover including a frozen leader and the
back-forward cache, and Trusted Types. Offline faults: drop the network mid-edit and restore (sent
once); reload while `sending` (same key, applied once); kill or freeze the leader during a drain;
two tabs appending; change the record elsewhere while offline (the "moved" screen, no silent merge);
create then update offline (the update waits for the create's id); sign out offline and read
IndexedDB directly (nothing decrypts, and signing in again as the same person does not unlock the
old data); let the session expire offline and sign in as the same person (the queue drains); sign in
as another person (nothing of the first); erase a subject while a reader is offline and reconnect
(old-epoch entries gone, and a late reply of the old epoch dropped).

## The specification part

`specifications/frontend.jsonc` is an area part. It holds thirteen decisions: D-FE-11 and D-FE-13 are
the owner's (decided 2026-11-22, D-FE-11 extended the same day to restriction and loss of permission,
D-FE-13 read as the orphaning state machine); the others are proposals of this design, with no
decision date and a source saying so. It holds three copy classes and nineteen requirements, FE-001
to FE-019, milestone 6, each naming Lean theorems with a `Verifies:` line as its check and stated of
its model with the model's hypotheses. Requirements whose checks are web tests are added with those
tests (follow-up 1):

| Planned requirement | Its check | Added by |
|---|---|---|
| The web application draws a cached view as current only at the newest stamp heard and within the contact bound, and as last checked (not refetched) at the newest stamp past it | kernel test of `cache.js`, and the differential against `Strata.Client.Cache` | 4, 5 |
| A dependent write has no key until its input is final, and its rewrite is a compare-and-swap | headless fault runs | 9 |
| Only the leader sends queued writes, and a frozen leader is replaced | Playwright | 8 |
| A contract not marked for queueing is refused while offline | kernel test over the generated contracts | 6, 9 |
| After sign-out, online or offline, no IndexedDB record of the principal decrypts, and the device id alone fetches nothing | Playwright and a gateway test | 7 |
| The bundle holds no module from `node_modules` | build check with a negative fixture | 12 |
| The QR encoder's matrix for the ISO worked example is the standard's | kernel test | 11 |

The Rust reader of the parts (`strata-spec-parts`) has no test of a section split across a consumer
part and the area part. It reads the current files without error in `specc lint` and in generation;
if it rejected them, specc would fail, and the gate with it, so a regression would be loud. A test
that pins it is WG-5450.

## What is proved

`lean/Strata/Client/Cache.lean`, `Privacy.lean`, `Queue.lean` and `Drafts.lean`, imported from
`lean/Strata.lean` and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom;
`lake env lean Audit.lean` lists only `propext`, `Classical.choice` and `Quot.sound`.

| Property | Theorem |
|---|---|
| Every generation and erasure epoch newer than a current answer's stamp began within the contact bound; no assumption on notices; a bound on the stamp, not the value | `current_recent` |
| Under complete notices, a current or checked answer is the server's opened value at the known stamp | `current_value`, `run_coh`, `current_answer`, `current_fetch` |
| A non-current answer is labelled with its generation | `read_stale_older` |
| Nothing at rest from an older erasure epoch, read below the floor, or from another tenant epoch; hearing any of them purges at once; the floor covers a policy change for every principal | `rest_one_epoch`, `rest_above_floor`, `rest_same_tenant_epoch`, `hear_purges`, `hear_access_purges`, `tenant_epoch_purges`, `older_epoch_ignored`, `contact_floor` |
| Recovery: an answer of the server's current tenant epoch brings the client to it; out-of-order answers across two rotations end current in the newest epoch | `contact_reaches_current_epoch`, `out_of_order_epochs_recover` |
| Rule (b): an answer to a request sent after the last switch is taken whatever its ordinal; a client at a high ordinal reaches a re-created tenant at ordinal 1 | `later_request_is_authoritative`, `lower_ordinal_recovers` |
| A re-created tenant compared with the old epoch's numbers drops everything; with the reset it is kept | `restarted_counter_dropped_without_reset`, `reset_accepts` |
| A reply read and sent at the current stamp is read back as current | `receipt_read`, `read_after_commit` |
| Counterexamples: incomplete notice, incomplete notice despite fresh contacts, epoch read after opening, unguarded put, unfloored put, generation alone | `incomplete_notice_serves_stale`, `fresh_contact_still_stale`, `stamp_after_open_serves_erased`, `unguarded_put_resurrects`, `unfloored_put_resurrects` (and `guarded_put_drops`, `floored_put_drops`), `browser_generation_only_serves_erased` |
| The privacy invariant; separation; sign-out online and offline against any later state and any session knowing the device id; expiry; revocation | `reachable_inv`, `no_cross_principal`, `served_own`, `signout_erases`, `offline_signout_erases`, `expire_locks`, `unlock_needs_own_session`, `expire_then_same_principal_unlocks`, `revoke_device_erases`, `revoke_all_erases`, `offline_key_survives_reload` |
| Rotation loses no record and no update, against stale tabs and concurrent rewrites; the old key retires; without the key-id check or the compare-and-swap it fails | `rot_no_loss`, `rot_no_lost_update`, `rot_finish_retires`, `rot_stale_append_breaks`, `rot_stale_append_refused`, `rot_blind_write_loses_update`, `rot_cas_keeps_update` |
| A first submission commits only if allowed and at the decided epoch and version; others change nothing | `submit_ok_guard`, `submit_not_ok_unchanged`, `submit_classes` |
| At most once from the two retention rules; for a stamped write whatever is forgotten or restored | `at_most_once`, `stamped_at_most_once` |
| The client's scheme sends each key with one input (Q2), with resolves and edits by compare-and-swap | `q2_sent` |
| A key fenced by a lookup never commits; without the fence a late original and a resend apply twice | `fence_holds`, `fenced_lookup_applies_once`, `unfenced_lookup_applies_twice` |
| D-FE-13: orphaning drops server values; the typed text survives the orphaning erasure and goes at the next erasure or the idle lifetime | `orphan_drops_server_values`, `survives_orphaning_erasure`, `gone_after_next_erasure`, `gone_after_idle`, `kept_until` |
| Counterexamples: create resent after forgetting, rekeyed create, restore without the epoch; with it, once | `create_twice_after_forget`, `rekeyed_create_applies_twice`, `restore_without_epoch_applies_twice`, `restore_with_epoch_applies_once` |

The central statements:

```lean
theorem current_recent (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Base s c)
    (hr : read B s.now c k = .current v) :
    (∀ g, c.known < g → g ≤ s.gen → s.now ≤ s.gTime g + B) ∧
    (∀ y, c.knownX < y → y ≤ s.x → s.now ≤ s.xTime y + B)

theorem current_value (B now : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V)
    (hv : Vals s c) (hr : read B now c k = .current v ∨ read B now c k = .checked v) :
    v = s.view c.known c.knownX k

theorem rest_above_floor (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : c.floor ≤ e.gen

theorem rot_no_lost_update (r : Rot) (h : ReadOk r) (j i : Nat) (x : RRec)
    (hx : (rstep r (.rwrite j)).recs[i]? = some x) : ∃ y, r.recs[i]? = some y ∧ x.val = y.val

theorem fence_holds (k : Nat) (ops : List FOp) :
    ∀ s : FSrv, s.tomb k = true → s.seen k = false →
      (frun s ops).applied.count k = s.applied.count k ∧ (frun s ops).seen k = false

theorem gone_after_next_erasure (I : Nat) (d : Draft) (x0 t0 x t : Nat)
    (ho : d.orphaned = some (x0, t0)) (hx : x0 < x) : (dstep I d (.hear x t)).typed = none
```

### What is not proved, and what turned out false

False as first stated, with the strongest true statement:

- **"A queued offline write is applied at most once"**: false for a create resent after the server
  forgets its key, or after a "not found" lookup that does not fence. True from the two retention
  rules (`at_most_once`), for a stamped write always (`stamped_at_most_once`), with the lookup fence
  (`fence_holds`), and for what the client sends given its scheme (`q2_sent`).
- **Tenant epoch tags and ordinals are one value in the model.** The server's epoch only grows there,
  so "equal ordinal, different tag is ignored" is not modelled, nor is a lower ordinal on the server
  side: `lower_ordinal_recovers` and `later_request_is_authoritative` are statements about the client's
  rule, not about a server run in which the ordinal went down. The split-brain case, two lease holders
  rotating to the same ordinal with different tags, is the same gap; rule (b) resolves it in the
  implementation, since the client follows whichever holder answered its latest request.
- **"The cache never serves an entry staler than its last known generation"**: the stamp part holds
  with no assumption (`current_recent`); the value part needs complete notices (`current_value`).
- **"After sign-out no cached plaintext is readable"**: true for ciphertext at rest and the hot set,
  online and offline, against any later browser state and any session that knows the device id; not
  against a disk copy taken while the session is live, code in the origin, or pixels already drawn.
- **Round 2's `rot_no_loss`** assumed tabs switch keys at once; with the key-id check in every write
  it now holds against stale tabs.

Not proved, each with the follow-up that carries it:

- **The JavaScript against the models** (D20): follow-up 5.
- **C2 and C3** and the stamps in the server: follow-up 2 and WG-5389.
- **C5**, the composition of the per-type erasure epochs (a conformance case in WG-5435 checks the
  server side), and **C6**, that no cached or conditional reply is taken as a contact (WG-5441).
- **The fence model has no expiry**: it holds while the tombstone outlives every submission of its
  key, which the server provides by keeping it for the key's retention (WG-5435).
- **The client's write-ahead record equals the queue model's `firstSent`** (up to Q4's margin), and the
  write-ahead and lookup paths are in no model; `q2_sent` holds because no client operation edits a
  keyed write. The fence model is separate from the queue model.
- **The meaning of a re-decided intent** (the guard does not cover related records).
- **Ideal encryption, secret unguessability and key deletion**; one device per instance; the rotation
  and draft models are separate from the privacy model; `fetchById` presents the partition's own
  secret, so it says nothing about an attacker holding another device's secret.
- **Liveness and clocks** (C4): tested, not proved.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5415 with its acceptance criteria (in Workgraph, which is the
record). Server items unblock client items; within the client, the stores precede the queue.

1. (WG-5434) **Evidence check indexes web tests.**
2. (WG-5435) **Stamps, the access generation, per-type erasure epochs from a destruction log read
   before opening, chained frames, complete notices, coalesced type-level frames, volatile views,
   restriction notices.** Needs WG-5389.
3. (WG-5436) **`expected-epoch`, and an optional tenant-generation guard per contract.**
4. (WG-5441) **Pure `cache.js`.** Needs 1 and 2.
5. (WG-5448) **Differential check of the client models.** Needs 4 and 9.
6. (WG-5437) **Contracts declare whether they may be queued.**
7. (WG-5442) **Device enrolment with a secret, key share, rotation by key id and compare-and-swap,
   encrypted IndexedDB.** Needs 1.
8. (WG-5444) **Leader tab with a heartbeat.** Needs 7.
9. (WG-5447) **The write queue with local ids by compare-and-swap, write-ahead first sending, and the
   fenced lookup.** Needs 3, 4, 6, 7, 8.
10. (WG-5445) **Service worker and the shell.** Needs 7.
11. (WG-5438) **In-house QR encoder.**
12. (WG-5439) **Vendored fonts and the bundle check.**
13. (WG-5443) **Headless uiScenarios.**
14. (WG-5446) **Web Components.** Needs 11.
15. (WG-5440) **Structured clone once.**
16. (WG-5449) **PRV-351, PRV-352, PRV-353, CPY-013 and the permission-change requirements amended for
    browser copies**, citing the owner decision D-FE-11.
17. (WG-5450) **The gate runs `split-spec --verify`; the Rust reader is tested on area parts.**

## Open questions

- Should a key destruction purge only the affected records in a browser, at the price of telling it
  which records held the subject's data?
- Should keeping data offline beyond a session require a periodic online check, so that its bound is
  enforced rather than "at the next connection"?

## Answers to the reviews

### Sixth review

| Item | Answer |
|---|---|
| Where the ordinal comes from, and monotonicity across restore, control-directory restore, re-creation and import | The ordinal is `max(previous + 1, rotation time in ms)`, persisted with the nonce; the time term covers every event that loses the previous value. WG-5435's criterion now tests a restore from an older snapshot, a control-directory restore, a re-created tenant and an import |
| Client rule (b) | Adopted with (a): an answer, matched by nonce, to a request sent after the last switch is authoritative whatever its ordinal. Modelled: `Cache.switched`, `epochOf c e t`, `later_request_is_authoritative`, `lower_ordinal_recovers`; added to WG-5441 |
| Recovery is a liveness assumption | Said in the cache model section |
| Equal ordinal with a different tag, split brain | Not modelled (tag and ordinal are one value); listed under what is not proved, with rule (b) as the resolution |
| Unanswered nonces | Dropped when answered, abandoned or older than the contact bound; in WG-5441 |
| Membership notices that change other principals' visible sets | Named in WG-5435's criterion (a policy reading who shares a team) |

### Fifth review

| Item | Answer |
|---|---|
| Tenant epoch ordering could leave a client stuck | Fixed: the server sends a monotone epoch ordinal with the tag; the client switches only to a higher ordinal and ignores a lower or equal one with a different tag; the retired list is gone. Recovery is proved in general (`contact_reaches_current_epoch`) and on the review's schedule (`out_of_order_epochs_recover`); FE-019; the ordinal is in WG-5435's stamp and the purge rule in WG-5441 |
| C6 could not be implemented | Reworded: view reads use `cache: 'no-store'` and the server echoes a per-request nonce, which the kernel checks; WG-5441's negative test is a replayed answer with a stale nonce |
| Coalescing against C2 | Every frame sent while a commit is deferred names the union of what is deferred over its interval, or holds `to` below it; criterion and test in WG-5435 |
| Policy generation events | Listed: Read-policy changes and team or role definition changes raise it; team membership, role grants, restrictions and permission changes raise only the members' access generation; purge rate measured in WG-5435 |
| Tombstone expiry and clocks | Stated: expiry at retention relies on the client rule, which a clock set back could break; the client measures retention on the server's clock |
| Lease failover resets every client | Stated, with a measured rate in WG-5435 |

### Fourth review

| # | Finding | Answer |
|---|---|---|
| 1 | The device secret was "encrypted under a key derived from the share", which is circular | Fixed: the secret is stored in the clear in `partitions`; the threat table is unchanged |
| 2 | Policy changes fell out of the floor | Fixed: the floor is the greater of the principal's access generation and the tenant's policy generation, one tenant-wide write per policy change; modelled (`Server.policy`, `contact_floor`), in WG-5435 and WG-5441 |
| 3 | Ordering across tenant epochs undefined | Fixed: known generation, floor and erasure epochs are kept per tenant epoch and purged and reset together on another epoch, and left epochs are ignored; modelled (`epochOf`, `tenant_epoch_purges`, `retired_epoch_ignored`, `rest_same_tenant_epoch`, `restarted_counter_dropped_without_reset`), in WG-5441 |
| 4 | Carry-forward only for entries read exactly at `from` | Fixed: C2 now names every key touched in `(from, to]`, and every entry read in `[from, to]` and not named is carried forward; modelled (`carry`, `frame_vals`), in WG-5441 |
| 5 | The fence tombstone's lifetime was too short | Fixed: kept for the key's idempotency retention, scoped per tenant, principal and contract, checked in the commit's store transaction; moved to WG-5435 as server work; the doc states that `fence_holds` assumes a tombstone outliving every submission |
| 6 | C5 needs a conformance case | Added to WG-5435 |
| 7 | D-FE-13 type; D-FE-11 sentence | The draft records the epoch of the type of the record it edits; D-FE-11 says the device id and secret remain, so the same principal unlocks the copy |
| 8 | A cached or conditional reply must not count as contact | View replies are `no-store` and the kernel ignores a 304 (C6); in WG-5441 |

### Third review

| # | Finding | Answer |
|---|---|---|
| Owner | D-FE-11 extended; D-FE-13 read as a state machine | Recorded as decided; D-FE-11 now covers restriction and loss of permission, and WG-5449 amends PRV-352 and the permission-change requirements too; D-FE-13 is the orphaning state machine of `Drafts.lean` (FE-017) |
| B1 | A per-class erasure epoch cannot be enumerated and has no tag | Fixed differently from both suggestions' first half: the epoch is per object type, derived from a destruction log keyed by type and field, so no class is enumerated and a role change never lowers it; role changes are handled by the access floor (B2). The model keeps one type's epoch; composition across types is C5, argued |
| B2 | The membership/restriction purge has no persisted floor | Fixed: a persisted access floor raised by every access change and checked in every put; `rest_above_floor`, `hear_access_purges`, counterexample `unfloored_put_resurrects` |
| B3 | "One transaction" cannot span WebCrypto awaits | Fixed: compute outside, verify and write in one short transaction by compare-and-swap; modelled with values for rotation (`rot_no_lost_update`, `rot_blind_write_loses_update`) and for resolve and edit in the client queue; in WG-5442 and WG-5447 |
| B4 | A stale tab appends under the old key after `finish` | Fixed: every append and rewrite names its key id and is checked against the partition's current one; `rot_stale_append_breaks` is the review's run, `rot_stale_append_refused` and `rot_no_loss` hold with the check |
| B5 | Pong-only contact causes a refetch loop | Fixed: a reply is a contact dated by its request's send time; an entry at the known stamp past the bound is "checked", not refetched; ping on welcome; contacts relayed to followers as epoch times; the server pushes heartbeat frames, a pong is not a chain frame and its stamp may run ahead |
| — | Type-level frames | Coalesced per connection within a window, with a measured rate criterion in WG-5435 |
| — | Device secret wording, `fetchById` | "While the session is live"; `fetchById` presents the partition's own secret and says nothing about another device's secret |
| — | Ciphertexts after expiry | Stated as consistent with D-FE-11: the key is gone |
| — | Unlock ordering | After signing in again, purge by the first answer's stamp and floor before decrypting or drawing |
| — | Overclaims | `rot_no_loss` restated with the key-id check; `q2_sent` stated as holding by construction; `firstSent` stated as a ghost, argued |
| — | Lookup then resend | Fixed by a fence: the lookup records a tombstone for an uncommitted key; `fence_holds`; resend under a new key |
| — | Scoped purge | Still open |

### Second review

| # | Finding | Answer |
|---|---|---|
| 1 | `current_recent` bounds the stamp, not the value | Fixed: restated in the theorem, the Cache.lean header, the doc and FE-003 (now "The cache and its coherence"); `fresh_contact_still_stale` is the review's counterexample |
| 2 | Contact is an oracle; `B` is not realisable | Fixed: only a pong is a contact, dated by the echoed send time of its ping; frames and replies are not; a reply carries its read stamp and its sent stamp; "current" is re-evaluated on every visibility message; `B` is a freshness choice, not a latency measurement |
| 3 | C2 half addressed | Fixed in design and WG-5435: volatile views by the existing classifier (less its principal condition), with a negative case; reference-following policies by type-level frames from `depends_on` (AGG-042), with the disclosure weighed; a restriction notice; the erasure epoch per audience class (round 3 replaced it by an epoch per object type, since classes cannot be enumerated) |
| 4 | The device id is not a credential | Fixed: a per-device secret, hashed at the server, deleted by sign-out; `fetchById` models a session that knows any id; `offline_signout_erases` holds against it |
| 5 | Carry-forward contradicts the ciphertext binding | Fixed: records persist their read stamp, the partition its known stamp, carry-forward is in memory, and the additional data binds the erasure epoch only |
| 6 | Rotation unmodelled and racy | Fixed: the order (new share durable, switch writers, re-encrypt one at a time, delete when none is left), modelled with crashes and appends: `rot_no_loss`, `rot_finish_retires`, `rot_early_delete_loses` |
| 7 | Local-id substitution gaps | Fixed: random local ids; the rewrite, key draw, mapping and removal in one transaction; dependents decided again after a lookup or when the created record differs from what the person saw; redirects for open views; write-ahead first-sending time; lookup by key before any doubtful resend; `q2_sent` shows the scheme meets Q2 |
| 8 | Copy-class bounds | Settled by the owner (D-FE-11) and implemented in the copy classes, the privacy section and WG-5449 |
| 9 | Typed text | Settled by the owner (D-FE-13) and implemented: separate ciphertexts for typed text and server values, "copy and dismiss", deletion at the idle session lifetime or the next erasure |
| 10 | `allow` justifies the proofs, not the meaning | Agreed: stated as a limitation, with a per-contract tenant-generation guard (WG-5436) |
| 11 | Remaining overclaims | Fixed: the Cache.lean header, FE-003's label and wording, the Privacy.lean and FE-005 wording after the secret, and the reader claim, now stated with what the gate does before WG-5450 |
| — | Smaller points | Purge cost stated, with the scoped purge as an open question; the queue bounded per principal; lookup by key reports a created-then-deleted record |

### First review

| # | Finding | Answer |
|---|---|---|
| 1 | The erasure theorem read the server's live stamp | Fixed. One model with a lagging known stamp, a contact bound and carry-forward (`current_recent`, `current_value`, `current_answer`; `contact_purges` became `hear_purges`); round 2 made the pong the only contact |
| 2 | `x` must be read before opening | Fixed in the design (C3, D-FE-8) and in WG-5435's criteria with a test; `stamp_after_open_serves_erased` is the boundary |
| 3 | A purge races a guarded put | Fixed. The known stamp is persisted per partition and checked and raised in every put's transaction; `rest_one_epoch`, `unguarded_put_resurrects`, `guarded_put_drops` |
| 4 | `run_coh` held by filtering notices; `.lose` left reads current | Fixed. C2 is a hypothesis (`RunC2`), the step applies every frame, `incomplete_notice_serves_stale` is the counterexample, and the contact bound demotes "current" without contact |
| 5 | C2 is false against the current code | Confirmed against `subscribe.rs`. Open as server work in WG-5435 with each missing case; views the server cannot track are volatile (D-FE-12) |
| 6 | Offline sign-out was the model's `expire` | Fixed differently from the suggestion: the server issues device ids and offline sign-out deletes the device id with the key, so the old share can never be fetched for that browser (`offline_signout_erases`), with no dependence on the logout marker. A share bound to the session was considered and not taken: it destroys unsent work at every expiry |
| 7 | The strong-attacker claim was overstated | Fixed. The design and the model now claim protection against the next user and a disk read after the key is gone; code in the origin is out of scope |
| 8 | A kept offline key makes the bounds unenforceable | Agreed. The copy classes and D-FE-11 state no bound for a device that kept a key; a realm may forbid it; open question on enforcing one |
| 9 | Copy bounds conflict with CPY/PRV/IDN requirements | Reconciled by D-FE-11 (erasure at the next contact, within the realm's idle lifetime; restriction as a permission change; receipts name the classes); amending PRV-351 and PRV-353 is WG-5449 |
| 10 | Queue and drafts hold others' plaintext | Fixed for base values and prefilled values, which are deleted with view results. Disputed for the person's own typed text, which is kept until dismissed: it is the person's own input |
| 11 | Share lifecycle inconsistent and underspecified | Fixed: fixed lifetime from enrolment with rotation and collection, server-issued device ids, per-device revocation, sealing at rest, deletions re-applied after a restore, request token and rate limit; in WG-5442's criteria |
| 12 | Queue gaps | Fixed: local ids and keys drawn once input is final; no silent merge; retention per entry and the client rule on the server's clock; "Check" by key; the table records commits only; the epoch in the model with `restore_without_epoch_applies_twice`; `at_most_once` from two local rules; contract skew by a minimum client version. Disputed in part: `allow` stays an oracle, because the kernel's decision on re-submission is proved faithful in the working set (`submit_one_generation`) |
| 13 | Carry-forward rarely fires | Fixed: `from` is the previous frame on the connection, a heartbeat is a frame with no keys, and a reconnect replays from the generation recorded with `after` or resyncs |
| — | Platform items | Fixed in the text: navigation timeout, frozen leader heartbeat and steal, back-forward cache untested, durability as a hint; sources rechecked (SharedWorker on Chrome for Android, Trusted Types in Firefox 148 and Safari 26, `persist()` in Safari 15.2) |
| — | Spec and tooling | FE requirements reworded to the models with their hypotheses; `split-spec --verify` and tests of the area rule (`scripts/spec-parts.test.mjs`); the resolved order of an area item changes within its section, which neither the semantic hash nor specc's output depends on (regenerating changed only the hash headers); the Rust reader accepted the parts in `specc lint` and generation, and a test of it is WG-5450 |

