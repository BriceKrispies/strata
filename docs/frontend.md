# The browser client

How the Strata web application should use the browser's own primitives: what it keeps, where, under
whose key, how it stays coherent with the server, how it works offline and reconciles what was
written there, and how it is tested. This is a design. Its safety properties are proved in Lean
(`lean/Strata/Client/`), its requirements are in `specifications/frontend.jsonc`, and the work is
listed at the end in dependency order. The coherence model is the backend working set's
(`docs/working-set.md`, `lean/Strata/WorkingSet.lean`) seen from outside the server. The design was
revised after two adversarial reviews; the answers are in the last section, and everything above it
is the current design.

## The recommendation in one page

Two decisions here are the owner's (2026-11-22): D-FE-11, how browser copies are reached by erasure
and correction, and D-FE-13, what happens to the person's own typed text. Every other D-FE-* is a
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
  the browser has heard and within a contact bound of its last **pong**, dated by the send time of
  the ping it echoes; otherwise it is drawn labelled with its age. The contact bound limits how far
  the browser's stamp lags; the value is right only if the server's notices are complete.
  Correctness never depends on the cache: a miss is a read, and every write is decided again by the
  server.
- **The server owes the browser** (D-FE-8, D-FE-12): an erasure epoch per audience class, read
  before opening sealed fields; notices that name every key that may have changed, type-level frames
  for the types reference-following policies read, a volatile mark for clock-reading views, a
  restriction notice, and a resync for out-of-commit changes. Today's notices do not meet this (see
  "What the server adds").
- **One leader tab per principal, chosen with Web Locks and kept by a heartbeat** (D-FE-3).
- **Offline writes are queued intents** (D-FE-5): a random local id, an idempotency key drawn once
  the input is final, the record stamp, a durable first-sending time, and a lookup by key before any
  doubtful resend. Conflicts are shown to the person, never merged silently. Some operations never
  queue (D-FE-6).
- **Data at rest is encrypted under a key held per enrolled device** (D-FE-7). The share is fetched
  only with the device's secret, which sign-out deletes; the device id is not a credential. This
  protects against the next user of the device and a disk read after the key is gone, not against
  code running in the origin.
- **Browser copies are reached at the next connection, within the realm's idle session lifetime**
  (D-FE-11, owner decision). Keeping data usable offline beyond a session is off by default; a realm
  opts in, and an opted-in device is reached only when it next connects.
- **The person's own typed text survives a revocation or an erasure** until the idle session
  lifetime or the next erasure, with "copy and dismiss" (D-FE-13, owner decision). Other people's
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
| `partitions` | principal tag | device id, known stamp `{e, g, x}`, time of the last pong's ping, leader heartbeat, offline flag | no personal data; the device secret is encrypted under a key derived from the share |
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

**Every put is one `readwrite` transaction over the partition row and the target store**: it reads
the known stamp, drops a value whose erasure epoch or tenant epoch is older, raises the known stamp,
deletes what an older erasure epoch left, and writes the value unless the store holds it at a later
generation. This is `Cache.put` (`rest_one_epoch`, `hear_purges`); without it a purge is undone by a
late reply or another tab's put (`unguarded_put_resurrects`).

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
   the kernel demote entries whose last pong is older than the contact bound. A hidden or frozen tab
   and a sleeping laptop get no timers, so every `visibility` message (and the first message after a
   gap in timers) re-reads the time and re-evaluates "current" before the main thread paints.
5. **Cache first, then the network, never instead of it.** A navigation draws a current entry and
   asks nothing; draws a non-current one labelled and asks; draws nothing it lacks, and asks.

## The cache and its coherence

### The model

The browser holds entries stamped `(g, x)` (the model's `g` stands for the tenant epoch and
generation together; its only property used is that a stamp never repeats), its known stamp, and the
time of its last contact. Three kinds of message carry stamps, each after a delay the browser cannot
measure:

- a **reply** carries the stamp it was read at and the stamp current when it was sent; the first
  stamps the entry, the second raises what the browser knows;
- a **frame** (a notice or a heartbeat) carries the stamp it was sent with, its `from`, and the keys
  that may differ; it carries entries forward and raises what the browser knows, but it is **not a
  contact**, because nothing in it says when it was sent;
- a **pong** answers a ping the browser sent, echoes the ping's nonce, and carries the stamp current
  when the server answered. It is the **only contact**, dated by the ping's send time, which the
  browser measured itself; however late the pong arrives, every generation and epoch newer than its
  stamp began after that time.

An answer is **current** iff its stamp is the known stamp and the last contact is at most `B` old.
`B` is a choice, not a measurement of latency: the browser pings every heartbeat interval, and `B`
(twice the interval, say) only decides how stale a stamp may be before the page says so. A slow or
lost pong costs freshness, never correctness of the stamp bound.

Proved, in one model of the whole stamp with carry-forward (`lean/Strata/Client/Cache.lean`):

- **the lag of the stamp, with no assumption on notices** (`current_recent`): every generation and
  erasure epoch newer than the stamp of a current answer began within `B` before the read. This bounds
  the stamp, **not the value**: after an incomplete notice the value is wrong however recent the
  contact (`fresh_contact_still_stale`);
- **the value, under complete notices** (C2, a hypothesis, never a filter): a current answer is the
  server's opened value at the known stamp (`current_value`, `run_coh`; together with the stamp bound,
  `current_answer`);
- every entry at rest carries the known erasure epoch, and hearing a new one deletes the older ones in
  the same step (`rest_one_epoch`, `hear_purges`);
- a receipt read and sent at the current stamp is read back as current while the last pong is recent
  (`receipt_read`, `read_after_commit`).

And what fails without each piece: an incomplete notice (`incomplete_notice_serves_stale`,
`fresh_contact_still_stale`); an epoch read after opening (`stamp_after_open_serves_erased`); a put
that does not check the persisted epoch (`unguarded_put_resurrects`).

### What the server adds (follow-up 2)

- **Stamps** `{e, g, x}` on view replies (read stamp and sent stamp), receipts, frames and pongs. `e`
  is a keyed tag of the generation nonce, `g` the generation counter (WG-5389), `x` the erasure epoch
  of the subscriber's audience class.
- **The erasure epoch, per audience class, read before opening.** At a key destruction the server
  raises, in the destruction's transaction, the epoch of every audience class (D-15) that could read a
  field sealed under the destroyed key. A read takes its class's epoch before, or in the same
  transaction as, opening its sealed fields; taking it after lets a concurrent destruction stamp the
  new epoch on erased plaintext (C3, `stamp_after_open_serves_erased`). **Disclosure**: only principals
  who could read the erased data learn that an erasure happened, and they would see the erased fields
  at their next read anyway. A tenant-wide epoch would tell every reader of the tenant, which this
  design rejects. **Cost**: a raised epoch makes the browsers of that class refetch their views; a
  purge scoped to the affected records would save that, at the price of telling the browser which
  records held the subject's data, and is left as an open question.
- **Frames chained per connection.** Every frame carries `from` (the stamp of the previous frame on
  that connection) and its own stamp. A heartbeat is a frame with no keys, which carries every entry
  forward. On reconnect with `after = seq`, the server replays from the generation recorded with that
  event, or answers `resync`.
- **Complete notices (C2).** Today's `notice` (`crates/substrate/src/subscribe.rs:354`) evaluates
  visibility per changed record, notifies a membership change to owners and the subject only,
  publishes with the schema current at publication, sends nothing for out-of-commit writes, and has
  three kinds (`Setting`, `Ledger`, `Segment`) that `docs/protocol.md` omits. The server work:
  - a membership notice to every subscriber whose visible set a member change may alter, and a
    **restriction notice** (PRV-352) to every subscriber that may read the restricted fields;
  - evaluation with the schema and members recorded with the event;
  - a `resync` for every out-of-commit write;
  - **reference-following policies**: AGG-042 already puts the object types a Read policy follows in
    the view's `depends_on`. A commit to such a type sends each subscriber whose views depend on it a
    frame naming the type, with no record id. That discloses that something changed in a type the
    subscriber's policies already read; the alternative, marking every such view volatile, turns it
    into polling. This design proposes the frames;
  - **clock-reading policies and unresolvable queries** make a view **volatile**, decided by the
    classifier the server already uses to refuse a shared copy (`crates/backend/src/aggregates.rs`,
    with `strata_kernel::audience`), without its principal condition, since a browser copy is kept for
    one principal. Every frame names a volatile view, so it is current only between its own reply and
    the next frame. A view that reads neither the clock nor a reference is never volatile.

### What the browser does

- A frame raises the known stamp, carries forward in memory the entries at its `from` that it does
  not name, and drops the ones it names; a new `x` deletes, in the same transaction, every view result,
  every base value of a queued write and every server value in a draft stamped with an older one,
  keeping the person's own typed text under D-FE-13.
- A different `e` or a `resync`: every entry becomes stale and is read again.
- A membership, schema or restriction notice, or a session reply whose roles differ: every view
  result, base value and server value in a draft of the partition is deleted.

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

**Resolving a dependency is one transaction**: when the earlier write's receipt arrives, the
receipt's record id and version replace the local id in every dependent's input, each dependent gets
its key, the local-to-real mapping is recorded, and the earlier write is removed, all at once
(`Queue.cstep`'s `resolve`; `q2_sent` proves the scheme meets Q2, so the at-most-once results apply
to what the client actually sends). A dependent is **decided again by the person**, not sent, when:

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
never resent until the server has been asked for the recorded outcome of its key; the lookup returns
the outcome and the record id, and says so when the record was created and has since been deleted.

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
| A copy of the disk taken while signed in, before sign-out | Not protected: the secret is on it. The design states it |
| Code running in the origin while the person is signed in (XSS, an extension) | **Not protected.** It can read the secret, the share or the plaintext |
| A device whose realm opted in to keeping data offline | The key survives reloads (`offline_key_survives_reload`); its expiry is a check against the local clock, so the device is reached only when it next connects (D-FE-11) |

### Device enrolment, the secret and the key share

- **Enrolment**: the first `POST /api/session/device-key` of a partition with no device enrols the
  device: the identity service issues a device id and a random 256-bit device secret, bound to the
  principal, and a random 256-bit share. It keeps the share and a hash of the secret, sealed at rest.
- **Fetching**: later requests send the device id and the secret; the service returns the share only
  to a session of the enrolled principal that presents the right secret. The device id may be listed
  on the sessions page; it is not a credential (`fetchById` lets any session of the principal name any
  id, and fetches nothing without the secret).
- **Guards**: a request token (D16), a rate limit per session and per device id, no step-up.
- **Storage and restores**: shares and secret hashes are sealed in the identity database; a restore of
  it re-applies the deletions recorded after its point (P2).
- **Lifetime and rotation**: a share lives for a fixed lifetime from enrolment (the absolute session
  lifetime, 14 days by default). Before it ends the leader rotates, in this order: (1) enrol a new
  share and secret and make them durable in the partition beside the old ones; (2) switch every writer
  to the new key, under the leader lock and told to the other tabs; (3) re-encrypt the old ciphertexts
  one at a time, each in its own transaction; (4) delete the old share only when no ciphertext is left
  under it. A crash at any point leaves both keys usable and the next leader resumes; an append from
  another tab during the pass uses whichever key writers use, both held (`rot_no_loss`,
  `rot_finish_retires`). Deleting the old share before the pass loses data (`rot_early_delete_loses`).
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

The owner's decision: erasure and correction reach a browser copy at the browser's next connection,
and no later than the realm's idle session lifetime (12 hours by default). The design meets it so: at
the next connection a new erasure epoch, a notice or a resync deletes or refreshes the copies; offline,
the copy is drawn only labelled, and when the idle session lifetime passes the session ends locally,
which locks the copy in online-keyed mode. A device whose realm opted in to keeping data offline is
reached only when it next connects. Restriction (PRV-352) and permission changes take the same path.
PRV-351, PRV-353 and, as needed, CPY-013 are amended to say this (WG-5449).

### The person's own typed text (D-FE-13)

The owner's decision: after a revocation or an erasure, the person's own typed text is kept so they
can copy it out, with a "copy and dismiss" action; it is deleted at the realm's idle session lifetime
or by any erasure (a change of the erasure epoch), whichever comes first. Other people's data in a draft
is removed at once. Drafts and queue entries therefore keep the person's typed text and the values that
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
the owner's (decided 2026-11-22); the others are proposals of this design, with no decision date and a
source saying so. It holds three copy classes and fourteen requirements, FE-001 to FE-014, milestone 6.
Each names Lean theorems with a `Verifies:` line as its check. They are stated of the models ("in the
client cache model, ...") and carry the models' hypotheses: FE-001 is conditional on complete notices,
FE-003 bounds the lag of the stamp and no value, FE-008 rests on the two retention rules.
Requirements whose checks are web tests are added with those tests (follow-up 1):

| Planned requirement | Its check | Added by |
|---|---|---|
| The web application draws a cached view as current only at the newest stamp heard and within the contact bound of its last pong | kernel test of `cache.js`, and the differential against `Strata.Client.Cache` | 4, 5 |
| A dependent write has no key until its input is final, and the rewrite and key are one transaction | headless fault runs | 9 |
| Only the leader sends queued writes, and a frozen leader is replaced | Playwright | 8 |
| A contract not marked for queueing is refused while offline | kernel test over the generated contracts | 6, 9 |
| After sign-out, online or offline, no IndexedDB record of the principal decrypts, and the device id alone fetches nothing | Playwright and a gateway test | 7 |
| The bundle holds no module from `node_modules` | build check with a negative fixture | 12 |
| The QR encoder's matrix for the ISO worked example is the standard's | kernel test | 11 |

The Rust reader of the parts (`strata-spec-parts`) has no test of a section split across a consumer
part and the area part. It reads the current files without error in `specc lint` and in generation;
if it rejected them, specc would fail, and the gate with it (the gate runs specc's check and lint
steps), so a regression would be loud. A test that pins it is WG-5450.

## What is proved

`lean/Strata/Client/Cache.lean`, `Privacy.lean` and `Queue.lean`, imported from `lean/Strata.lean`
and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom; `lake env lean Audit.lean` lists
only `propext`, `Classical.choice` and `Quot.sound`.

| Property | Theorem |
|---|---|
| Every generation and erasure epoch newer than a current answer's stamp began within the contact bound; no assumption on notices; a bound on the stamp, not the value | `current_recent` |
| Under complete notices, a current answer is the server's opened value at the known stamp | `current_value`, `run_coh`, `current_answer`; with the working set's keys, `current_fetch` |
| A non-current answer is labelled with its generation | `read_stale_older` |
| Entries at rest carry the known erasure epoch; hearing a new one deletes the older ones at once | `rest_one_epoch`, `hear_purges` |
| A receipt at the current stamp is read back as current while the last pong is recent | `receipt_read`, `read_after_commit` |
| Counterexamples: incomplete notice, incomplete notice despite fresh pongs, epoch read after opening, unguarded put, generation alone | `incomplete_notice_serves_stale`, `fresh_contact_still_stale`, `stamp_after_open_serves_erased`, `unguarded_put_resurrects` (and `guarded_put_drops`), `browser_generation_only_serves_erased` |
| The privacy invariant in every reachable state | `reachable_inv` |
| A ciphertext opens only with its owner's key; what a session is served was written by its principal | `no_cross_principal`, `served_own` |
| After sign-out, online or offline, nothing the principal left opens in any later state, whatever device id a session names | `signout_erases`, `offline_signout_erases` |
| After expiry nothing opens; only a session of that principal unlocks; the same principal does | `expire_locks`, `unlock_needs_own_session`, `expire_then_same_principal_unlocks` |
| A revoked device, or all devices, unreadable from the next reload; a kept key survives | `revoke_device_erases`, `revoke_all_erases`, `offline_key_survives_reload` |
| Rotation loses nothing at any point, retires the old key, and early deletion loses data | `rot_no_loss`, `rot_finish_retires`, `rot_early_delete_loses` |
| A first submission commits only if allowed and at the decided epoch and version; others change nothing | `submit_ok_guard`, `submit_not_ok_unchanged`, `submit_classes` |
| At most once from the two retention rules; at most once for a stamped write whatever is forgotten or restored | `at_most_once`, `stamped_at_most_once` |
| The client's local-id and key scheme sends each key with one input (Q2) | `q2_sent` |
| Counterexamples: create resent after forgetting, rekeyed create, restore without the epoch; with it, once | `create_twice_after_forget`, `rekeyed_create_applies_twice`, `restore_without_epoch_applies_twice`, `restore_with_epoch_applies_once` |

The central statements:

```lean
theorem current_recent (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Base s c)
    (hr : read B s.now c k = .current v) :
    (∀ g, c.known < g → g ≤ s.gen → s.now ≤ s.gTime g + B) ∧
    (∀ y, c.knownX < y → y ≤ s.x → s.now ≤ s.xTime y + B)

theorem fresh_contact_still_stale :
    read 1 5 (run (cxServer, cxCache) cxLater).2 0 = .current 5 ∧
    (run (cxServer, cxCache) cxLater).2.contact = 5 ∧
    (run (cxServer, cxCache) cxLater).1.view (run (cxServer, cxCache) cxLater).1.gen 0 0 = 6

theorem current_value (B now : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V)
    (hv : Vals s c) (hr : read B now c k = .current v) : v = s.view c.known c.knownX k

theorem offline_signout_erases (h : Inv s) (hs : s.session = some p) (hc : c ∈ s.disk)
    (ho : c.owner = p) (ops : List (Op α)) : ¬ Opens (run (step s .signOutOffline) ops) c

theorem rot_no_loss (ops : List ROp) : ∀ r, ROk r → ROk (rrun r ops)

theorem at_most_once (R : Nat) (ops : List Op) : ∀ s, AMO R s → Rules R s ops → AMO R (run s ops)

theorem q2_sent (ops : List COp) : ∀ q, CInv q → ∀ p ∈ (crun q ops).sent, ∀ p' ∈ (crun q ops).sent,
    p.1 = p'.1 → p.2 = p'.2
```

### What is not proved, and what turned out false

False as first stated, with the strongest true statement:

- **"A queued offline write is applied at most once"**: false for a create resent after the server
  forgets its key (`create_twice_after_forget`). True from the two retention rules (`at_most_once`),
  for a stamped write always (`stamped_at_most_once`), and for what the client sends, given its
  scheme (`q2_sent`).
- **"A read after a confirmed commit returns the committed value"**: true for a receipt at the current
  stamp while the last pong is recent (`receipt_read`).
- **"The cache never serves an entry staler than its last known generation"**: the stamp part holds
  with no assumption (`current_recent`); the value part needs complete notices (`current_value`), and
  without them the value is wrong however recent the contact (`fresh_contact_still_stale`).
- **"After sign-out no cached plaintext is readable"**: true for ciphertext at rest and the hot set,
  online and offline, against any later browser state and any session that knows the device id
  (`signout_erases`, `offline_signout_erases`); not against a disk copy taken while signed in, code in
  the origin, or pixels already drawn.
- **Round 1's `current_recent` was read as a value bound** in the doc and FE-003; it bounds the stamp.

Not proved, each with the follow-up that carries it:

- **The JavaScript against the models** (D20): follow-up 5.
- **C2 and C3** (complete notices, the epoch read before opening) and the stamps in the server:
  follow-up 2 and WG-5389.
- **The meaning of a re-decided intent.** The queue theorems hold for any kernel decision; that the
  person's intent means the same on current state is not a theorem, and the guard does not make it one
  (see "A limitation the guard does not remove").
- **Ideal encryption, secret unguessability and key deletion** (P1, P3); **one device per instance**
  of the privacy model; the rotation model is separate from the privacy model and joined by the order
  it states.
- **Liveness and clocks**: the queue drains, a frozen leader is replaced, the browser clock does not
  run backwards between a ping and the reads relying on its pong (C4): tested, not proved.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5415 with its acceptance criteria (in Workgraph, which is the
record). Server items unblock client items; within the client, the stores precede the queue.

1. (WG-5434) **Evidence check indexes web tests.**
2. (WG-5435) **Stamps, chained frames, complete notices, type-level frames, volatile views,
   restriction notices and the per-class erasure epoch read before opening.** Needs WG-5389.
3. (WG-5436) **`expected-epoch`, and an optional tenant-generation guard per contract.**
4. (WG-5441) **Pure `cache.js`.** Needs 1 and 2.
5. (WG-5448) **Differential check of the client models.** Needs 4 and 9.
6. (WG-5437) **Contracts declare whether they may be queued.**
7. (WG-5442) **Device enrolment with a secret, key share, rotation and encrypted IndexedDB.** Needs 1.
8. (WG-5444) **Leader tab with a heartbeat.** Needs 7.
9. (WG-5447) **The write queue with local ids, write-ahead first sending and lookup by key.** Needs 3,
   4, 6, 7, 8.
10. (WG-5445) **Service worker and the shell.** Needs 7.
11. (WG-5438) **In-house QR encoder.**
12. (WG-5439) **Vendored fonts and the bundle check.**
13. (WG-5443) **Headless uiScenarios.**
14. (WG-5446) **Web Components.** Needs 11.
15. (WG-5440) **Structured clone once.**
16. (WG-5449) **PRV-351, PRV-353 and CPY-013 amended for browser copies**, citing the owner decision
    D-FE-11.
17. (WG-5450) **The gate runs `split-spec --verify`; the Rust reader is tested on area parts.**

## Open questions

- Should a key destruction purge only the affected records in a browser, at the price of telling it
  which records held the subject's data?
- Should keeping data offline beyond a session require a periodic online check, so that its bound is
  enforced rather than "at the next connection"?

## Answers to the reviews

### Second review

| # | Finding | Answer |
|---|---|---|
| 1 | `current_recent` bounds the stamp, not the value | Fixed: restated in the theorem, the Cache.lean header, the doc and FE-003 (now "The cache and its coherence"); `fresh_contact_still_stale` is the review's counterexample |
| 2 | Contact is an oracle; `B` is not realisable | Fixed: only a pong is a contact, dated by the echoed send time of its ping; frames and replies are not; a reply carries its read stamp and its sent stamp; "current" is re-evaluated on every visibility message; `B` is a freshness choice, not a latency measurement |
| 3 | C2 half addressed | Fixed in design and WG-5435: volatile views by the existing classifier (less its principal condition), with a negative case; reference-following policies by type-level frames from `depends_on` (AGG-042), with the disclosure weighed; a restriction notice; the erasure epoch per audience class, so only principals who could read the erased data learn of it |
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

