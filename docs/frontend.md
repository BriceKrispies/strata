# The browser client

How the Strata web application should use the browser's own primitives: what it keeps, where, under
whose key, how it stays coherent with the server, how it works offline and reconciles what was
written there, and how it is tested. This is a design. Its safety properties are proved in Lean
(`lean/Strata/Client/`), its requirements are in `specifications/frontend.jsonc`, and the work is
listed at the end in dependency order. The coherence model is the backend working set's
(`docs/working-set.md`, `lean/Strata/WorkingSet.lean`) seen from outside the server. The design was
revised after one adversarial review; the answers are in the last section, and everything above it
is the current design.

## The recommendation in one page

- **Keep the shape that exists.** The pure kernel (`update`, `view`) in a dedicated worker, effects
  run by a driver, the main thread drawing a view model with DOM APIs only, generated platform
  screens, hash-addressed tenant parts (D-8250). Everything below is added around it, as kernel
  messages and effects, so the kernel stays pure and testable in Node.
- **Three stores, each for one kind of content** (D-FE-2). A service worker keeps only
  content-addressed files (built assets, fonts, `/api/ui/parts/<hash>`) in the Cache API and passes
  everything else through. IndexedDB keeps stamped view results, the write queue and drafts,
  encrypted per principal, with the known stamp beside them. The kernel worker's memory keeps the
  hot set.
- **The browser cache is a stamped copy** (D-FE-1). Every reply and frame carries one stamp: the
  tenant epoch, the generation and an erasure epoch. A value is drawn as current only at the newest
  stamp the browser has heard and within a contact bound of its last contact with the gateway;
  otherwise it is drawn labelled with its age. Correctness never depends on it: a miss is a read,
  and every write is decided again by the server.
- **The server owes the browser three things** (D-FE-8, D-FE-12): the erasure epoch, read before
  opening sealed fields; notices that name every key that may have changed since the previous frame
  on the connection, with a volatile mark where it cannot know; and a resync for every out-of-commit
  change. Today's notices do not meet the second (see "What the server adds").
- **One leader tab per principal, chosen with Web Locks and kept by a heartbeat** (D-FE-3). It owns
  the socket and drains the write queue; other tabs hear notices on a BroadcastChannel.
- **Offline writes are queued intents** (D-FE-5) with an idempotency key drawn once their input is
  final and a stamp (tenant epoch and record version), sent on reconnecting against the server's
  guarded commit. Conflicts are shown to the person, never merged silently. Some operations never
  queue (D-FE-6).
- **Data at rest is encrypted under a key held per enrolled device** (D-FE-7). The identity service
  issues the device id and keeps the key share; sign-out deletes the browser's key and device id
  (and, online, the share), so nothing left behind is readable. This protects against the next user
  of the device and a disk read after the key is gone, not against code running in the origin.
- **Web Components draw, the kernel decides** (D-FE-4): light DOM, form-associated only for controls
  with no native element, no Declarative Shadow DOM.
- **No third-party runtime code** (D-FE-9): an in-house QR encoder replaces `uqr`; the fonts are
  vendored into the repository.
- **The kernel runs the uiScenarios headlessly in Node** against the real gateway (D-FE-10), with
  faults injected at the driver; Playwright keeps what only a browser can answer.

## Where the framing should change

The request is the right direction. Taken literally, a few parts of it would make the design worse.

1. **"Load things intelligently into the cache, as the backend's working set does" is half right.**
   The working set's *coherence* model carries over: stamps, a guarded commit, idempotency keys, the
   request-snapshot rule. Its *policy* machinery (GreedyDual-Size-Frequency, TinyLFU admission, a
   cell arbiter) does not: a tab's working set is one person's recent pages, and plain LRU by
   accounted bytes is enough. The browser also cannot do what makes the working set safe in checked
   mode, a generation read in the store's own transaction: it learns stamps from messages, so its
   guarantee is relative to what it has heard and to how long ago (`current_recent`,
   `current_value`).
2. **The browser cache sits above sealing, which the working set rejected for itself.** The gateway
   opens sealed fields before it answers, so the browser holds plaintext. A key destruction (D-350)
   raises no generation, so a cache keyed by generation alone serves erased values: the working
   set's own counterexample applies unchanged (`browser_generation_only_serves_erased`). The fix is a
   new server obligation, an erasure epoch read before opening, and a client one, a persisted known
   stamp checked in every put.
3. **Encryption at rest protects the next user, not the current session.** A non-extractable key
   cannot be exported, but any script of the origin can use it, and the key share arrives in script.
   Cross-site scripting or a malicious extension running while the person is signed in can copy the
   share or the plaintext out. What encryption at rest buys is that the next person on the device,
   and someone who reads the disk after the key is gone, cannot read the data. The design and the
   proofs claim that and nothing more.
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
   See "What the server adds". The design states it as an assumption of the coherence theorem, gives
   the server work that would make it true, and makes "current" expire after the contact bound even
   when it holds.
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
| `partitions` | principal tag | device id, known stamp `{e, g, x}`, last contact, offline key flag | no personal data |
| `keys` | principal tag | the non-extractable derived keys, only when offline is kept | the keys themselves |
| `views` | `[principal tag, HMAC(index key, view key)]` | stamp, last read, bytes, IV, ciphertext | yes |
| `queue` | `[principal tag, sequence]` | state, attempts, first sent (server time), IV, ciphertext of the intent, key or local id, decided stamp, base values | yes |
| `drafts` | `[principal tag, HMAC(form key)]` | IV, ciphertext of the fields the person changed, and prefilled values separately | yes |
| `site` | `manifest` | the last interface manifest (public, D-8250) | no |

**Every put is one `readwrite` transaction over the partition row and the target store**: it reads
the known stamp, drops a value whose erasure epoch or tenant epoch is older, raises the known stamp,
deletes what an older erasure epoch left, and writes the value unless the store holds it at a later
generation. This is `Cache.put` (`rest_one_epoch`); without it a purge is undone by a late reply or
another tab's put (`unguarded_put_resurrects`).

Queue appends ask for `{durability: 'strict'}`, which is a hint (Chrome's default is relaxed, and
strict does not promise an immediate flush). "Saved on this device" means the transaction completed;
it is a statement about the browser's commit, not a guarantee against power loss.

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
`visibility` (hints from the main thread); `storage`, `sw`. New effects: `store_get`, `store_put`
(the guarded put above), `store_purge`, `queue_append`, `queue_mark`, `queue_remove`, `key_fetch`,
`key_keep_offline`, `erase_partition`, `lead`, `broadcast`, `persist_storage`, `estimate_storage`,
`activate_update`.

1. **One message at a time.** The worker runs one message to completion; async work is an effect
   whose result is a later message. A put is atomic because it is one IndexedDB transaction.
2. **The kernel decides; drivers execute.** What is current, what a notice invalidates, what to
   persist, when to send a queued write: kernel (`cache.js`, `queue.js`, the JavaScript of
   `Strata.Client.Cache` and `Strata.Client.Queue`). Encryption, IndexedDB, locks, sockets: drivers.
3. **Connectivity is learned from outcomes**; `navigator.onLine` is a hint.
4. **Time comes on messages.** A timer message every heartbeat lets the kernel demote entries whose
   contact is older than the contact bound.
5. **Cache first, then the network, never instead of it.** A navigation draws a current entry and
   asks nothing; draws a non-current one labelled and asks; draws nothing it lacks, and asks.

## The cache and its coherence

### The model

The browser holds entries stamped `(g, x)` (the model's `g` stands for the tenant epoch and
generation together; its only property used is that a stamp never repeats) and a known stamp with
the time of its last contact. A **contact** is a frame (a notice, a heartbeat, the welcome), or a
reply read at the server's current stamp. An answer is **current** iff its stamp is the known stamp
and the last contact is at most the contact bound `B` old; `B` is twice the heartbeat interval plus
the frame latency the deployment measures.

Proved, in one model of the whole stamp with carry-forward (`lean/Strata/Client/Cache.lean`):

- **with no assumption on notices**, every commit and every key destruction a current answer does
  not reflect began within the last `B` ticks (`current_recent`);
- **if every frame is complete** (assumption C2, a hypothesis, never a filter), a current answer is
  the server's opened value at the known stamp (`current_value`, `run_coh`; together,
  `current_answer`);
- every entry at rest carries the known erasure epoch, and after a contact the server's
  (`rest_one_epoch`, `contact_purges`);
- a receipt at the server's current stamp is read back as current (`receipt_read`,
  `read_after_commit`).

And what fails without each piece: an incomplete notice keeps a stale value current indefinitely
(`incomplete_notice_serves_stale`, which is why C2 matters even with heartbeats); an epoch read after
opening stamps plaintext as erased-current with no lag (`stamp_after_open_serves_erased`); a put that
does not check the persisted epoch resurrects a purged value (`unguarded_put_resurrects`).

### What the server adds (follow-up 2)

- **Stamps** `{e, g, x}` on view replies, receipts and `welcome`/`pong`/`event` frames. `e` is a keyed
  tag of the generation nonce, `g` the generation counter (WG-5389), `x` the erasure epoch.
- **The erasure epoch, read before opening.** `x` is raised in the transaction of every key
  destruction. A read takes `x` before, or in the same transaction as, opening its sealed fields;
  taking it after opening lets a concurrent destruction stamp the new epoch on erased plaintext,
  which the browser then serves as current until the next change (C3,
  `stamp_after_open_serves_erased`).
- **Frames chained per connection.** Every frame carries `from` (the `to` of the previous frame on
  that connection) and `to` (the server's current stamp). A heartbeat is a frame with no keys, so it
  carries every entry forward: an invisible commit costs nothing, and carry-forward does not need a
  frame per commit. On reconnect with `after = seq`, the server looks up the generation recorded with
  that event in its log and starts the replay's `from` there; when it cannot (the log no longer
  holds it), it answers `resync`.
- **Complete notices (C2).** A frame names every key the subscriber holds that may differ between
  `from` and `to`. Today's `notice` (`crates/substrate/src/subscribe.rs:354`) does not:
  - it evaluates record visibility per changed record, so a Read policy that follows references
    (AGG-042) or reads the clock changes what a principal sees with no notice;
  - `MemberChanged` reaches owners and the subject only, though a role change can change what other
    members see;
  - publication evaluates with the schema and members current when it publishes, which can differ
    from those at commit;
  - out-of-commit writes (`docs/working-set.md:91`) send nothing;
  - the `Setting`, `Ledger` and `Segment` kinds exist in code and not in `docs/protocol.md`.

  The server work is: a membership notice to every subscriber whose visible set a member change may
  alter; evaluation with the schema and members recorded with the event; a `resync` for every
  out-of-commit write; the three kinds documented and mapped to the views they affect; and, where
  the server cannot know (clock-reading and reference-following policies), the view's reply marked
  **volatile**, which puts it in every frame's key list (D-FE-12), so it is current only between its
  own reply and the next frame.

### What the browser does

- A frame raises the known stamp to its `to`, re-stamps entries at `from` that it does not name, and
  drops the ones it names; a new `x` deletes, in the same transaction, every view result, every base
  value of a queued write and every prefilled draft value stamped with an older one.
- A different `e` or a `resync`: every entry becomes stale and is read again.
- A `membership`, `schema` or restriction notice, or a session reply whose roles differ: every view
  result, base value and prefilled draft value of the partition is deleted.
- The kernel's invalidation map (notice to view keys) uses `depends_on`, which CPY-004 requires to be
  complete.

## Offline writes

### The queue

A queued write is an intent the person decided while the browser could not reach the gateway or
while a reply was outstanding. It holds the contract and inputs, the **decided stamp** (tenant epoch
and record version; none for a create), the base values of the fields it changes, its state,
attempts and the server time of its first sending, and either:

- an **idempotency key**, 128 random bits drawn once the write's input is final and stored with it
  (`rekeyed_create_applies_twice` is why it is never redrawn); or
- a **local id** and no key, while its input depends on another write's unknown result: an update of
  a record created offline, a reference to it. When the earlier write's receipt arrives, the kernel
  substitutes the real id and the version from the receipt, and only then draws the key. A write is
  never sent before its key exists, so changing its input before the first sending cannot cause an
  idempotency mismatch. If the earlier write's outcome is unknown, its dependents stay blocked with
  it.

Unsaved form text is a draft, not a queued write.

### Submitting against the guarded commit

The leader reads the session, then drains in order, one write at a time per record. Each is sent
with `idempotency-key`, `expected-version` and `expected-epoch` (follow-up 3). The server answers a
committed key with its recorded outcome; otherwise its kernel decides the intent **again on the
current state**, and the store commits it only at the expected epoch and version. The browser's
offline checks are advice to the person, never the decision.

Proved (`lean/Strata/Client/Queue.lean`): an accepted first submission was allowed by the kernel and
found the tenant and record at the decided stamp (`submit_ok_guard`); any submission that does not
commit changes no record (`submit_not_ok_unchanged`); the classes (`submit_classes`); at most once
from two local rules, the server's (keep a committed key for the retention) and the client's (never
send a key past the retention from its first sending) (`at_most_once`); and for a write with an epoch
and version stamp, at most once whatever the server forgets and whatever a restore does
(`stamped_at_most_once`). Without the epoch, a restore that sets the version back lets an update
apply twice (`restore_without_epoch_applies_twice`).

**Retention.** The store already sets retention per entry (`idempotency_retention`), seven days for
operations. Queued writes carry a retention at least as long as the queue's own erasure bound, and
the client stops resending a key a margin before it ends, measured on the server's clock (its offset
learned from every reply, since a lost reply gives no time of its own).

### Conflict classes, and what the person sees

| Class | Server answer | What the person sees | What happens to the entry |
|---|---|---|---|
| Moved | `conflict` (version or epoch) | "‹Title› was changed after you edited it offline." Their version beside yours, field by field | The person chooses yours, theirs, or edits; the choice is a new intent with a new key. Nothing is merged without the person, because policies, workflows, validations and derived fields make "different fields" unreliable |
| Gone | `not_found` | "‹Title› was deleted while you were offline. Your changes were not saved." With a copy of your text and "Create as new" | Closed |
| Refused by the rules | `invalid` with `failed` evidence | The form again, the failing fields marked | Closed; resubmitting is a new intent |
| Not allowed now | `forbidden` / `unauthorized` | "You can no longer change ‹Title›." Your own typed text, to copy | Closed; base values and prefilled values deleted at once, the person's own text kept until dismissed |
| Needs you first | `unauthenticated`, `step_up_required` | "Sign in again to send 3 changes" / the step-up prompt | Held; resent with the same key afterwards |
| Waiting | network failure, `throttled`, `unavailable` | "3 changes waiting to send" | Retried with backoff, same key |
| Outcome unknown | no answer, and the retention less the margin has passed | "We could not confirm whether ‹Title› was saved." With "Check" | Never resent automatically; "Check" asks the server for the recorded outcome of the key (a key lookup the store already keeps, returning the record id), and past the retention shows the person's text and a search |
| Blocked | an earlier write it depends on is unanswered or unknown | Shown under the earlier one | Waits with it |
| Client too old | the server's minimum client version | "Reload to send your changes" | Kept; the new kernel rebuilds it or asks |

### Durability, ownership, what may not be done offline

The queue survives reloads, crashes and browser restarts, subject to eviction. A write marked
`sending` when the tab died is resent by the next leader with the same key. The leader drains; any
tab appends and tells the others. Only contracts the specification marks may be queued (follow-up 6;
D-FE-6 lists what never may).

**Sign-out works offline**: it deletes the browser's key and the partition's device id at once, so
nothing left opens (`offline_signout_erases`), whether or not the gateway ever hears of it; a marker
with no personal data asks the next online load to call `/api/logout`, which ends the session at the
server. A person with unsent changes is told that signing out deletes them.

### The session expiring while offline

The session reply gains `idle_expires_at` and `absolute_expires_at`. The worker ends the session
locally when either passes without contact: it drops the key from memory, deletes a kept offline
key, keeps the ciphertexts and the device id, and shows "Your session ended. Sign in again as ‹name›
to send 3 changes." Signing in again as the same person fetches the same share and unlocks the queue
(`expire_then_same_principal_unlocks`); signing in as someone else cannot (`unlock_needs_own_session`).

## Privacy at rest

### What is protected, against whom

| Threat | Protection |
|---|---|
| The next person on a shared device after sign-out, online or offline | Key and device id deleted in the browser (and the share at the server, online): nothing left opens, ever (`signout_erases`, `offline_signout_erases`) |
| The next person after a session expired without sign-out | The first person's key is gone with the session; another principal's session cannot fetch the first's share (`no_cross_principal`, `expire_locks`, `unlock_needs_own_session`) |
| A lost or stolen device | Revoke that device from another one: its share is deleted and its copies are unreadable from its next reload (`revoke_device_erases`); signing out everywhere does the same for every device (`revoke_all_erases`) |
| A disk read after the key is gone | Ciphertext only, under a key no longer on the device |
| Code running in the origin while the person is signed in (XSS, an extension) | **Not protected.** It can read the share as it arrives, or the plaintext |
| A device that kept an offline key | The key survives reloads (`offline_key_survives_reload`); its expiry is a check against the local clock, which can be set back, so no bound is enforced. A realm may forbid keeping a key, and the copy class says so |

### Device enrolment and the key share

- **Enrolment**: the first `POST /api/session/device-key` of a partition with no device id enrols the
  device: the identity service issues a random device id, bound to the principal, and a random
  256-bit share. Later requests name the device id; the service returns the share only to a session
  of the principal the device was enrolled for, and refuses any other.
- **Guards**: a request token (D16), as every state-changing call; a rate limit per session; no
  step-up, since every page load needs it.
- **Storage**: shares are sealed at rest in the identity database like other secrets. A restore of
  that database re-applies the share deletions recorded after its point, so a deleted share never
  comes back (assumption P2, as D-7727 does for key destructions).
- **Lifetime**: fixed from enrolment (the session's absolute lifetime, 14 days by default, which a
  realm may shorten). Before it ends, the online client rotates: it fetches a new share for a new
  enrolment, re-encrypts the partition, and deletes the old enrolment. A share whose lifetime ended is
  deleted, so orphaned shares (a device signed out offline, a device never seen again) are collected.
- **Revocation**: per device (the sessions page lists enrolled devices) or for every device (sign
  out everywhere, removal, erasure). Signing out everywhere destroys every device's unsent work, and
  the confirmation says so.
- **Derived keys**: HKDF-SHA-256 over the share gives a non-extractable AES-GCM-256 key and a
  non-extractable HMAC-SHA-256 key; each record has a random 96-bit IV and additional data
  `principal tag ‖ store ‖ key ‖ stamp`.
- **Online-keyed (default)**: the derived keys live in the worker's memory only. **Offline kept
  (opt-in, a realm may forbid)**: the derived `CryptoKey` objects are persisted in the `keys` store.

### Partitioning, permission changes and erasure while offline

One partition per principal per site. The BroadcastChannel and lock names carry the principal tag;
no message carries record data. A partition whose device id the server answers "unknown" is deleted.

The platform cannot reach a browser that is offline (D-FE-11). A deletion, rectification,
restriction, permission change or erasure reaches a browser copy at its next contact: membership and
restriction notices and a new erasure epoch delete view results, base values and prefilled values.
Offline, the copy is drawn only labelled, and the session ends locally when the realm's idle
lifetime passes, which locks the copy. That is the erasure bound the copy classes state for CPY-013
and PRV-351. An erasure receipt (PRV-353) names the browser copy classes with that erasure point and
cannot name individual browsers; a device that kept an offline key is reached at its next contact,
with no bound. Restriction (PRV-352) is handled as a permission change. Amending the texts of PRV-351
and PRV-353 to say so is follow-up 16.

### Storage eviction

- **Safari** deletes all script-writable storage of a site after seven days of Safari use without the
  person interacting with the site; a web app added to the Home Screen is exempt. Unsent work left
  that long is lost without the server knowing. The queue panel says how long work is kept on this
  browser and, on Safari, recommends adding the site to the Home Screen for offline work.
- **Quota**: on `QuotaExceededError` the driver evicts view results by last read, never queue entries
  or drafts; if a queue append still fails the person is told the change is not saved on the device.
- **Persistence**: `navigator.storage.persist()` exists in Chrome 55, Firefox 57 and Safari 15.2
  (caniuse); whether it is granted is each engine's decision (an engagement signal in Chrome, a
  prompt in Firefox, WebKit's storage policy since Safari 17). It is asked for only with offline
  kept, and a refusal is shown.
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

`specifications/frontend.jsonc` is an area part. It holds twelve decisions (D-FE-1 to D-FE-12), three
copy classes, and twelve requirements, FE-001 to FE-012, milestone 6. Each names Lean theorems with a
`Verifies:` line as its check. They are stated of the models ("in the client cache model, ..."), as
KRN-013 to KRN-015 are stated of the kernel model, and carry the models' hypotheses (FE-001 is
conditional on complete notices, FE-008 on the two retention rules), so they read no stronger than
what is proved. Requirements whose checks are web tests are added with those tests, since a declared
check must exist and the evidence check does not yet index web tests (follow-up 1):

| Planned requirement | Its check | Added by |
|---|---|---|
| The web application draws a cached view as current only at the newest stamp heard and within the contact bound | kernel test of `cache.js`, and the differential against `Strata.Client.Cache` | 4, 5 |
| A queued write keeps its key across reloads; a dependent write has no key until its input is final | headless fault runs | 9 |
| Only the leader sends queued writes, and a frozen leader is replaced | Playwright | 8 |
| A contract not marked for queueing is refused while offline | kernel test over the generated contracts | 6, 9 |
| After sign-out, online or offline, no IndexedDB record of the principal decrypts | Playwright reading IndexedDB | 7 |
| The bundle holds no module from `node_modules` | build check with a negative fixture | 12 |
| The QR encoder's matrix for the ISO worked example is the standard's | kernel test | 11 |

## What is proved

`lean/Strata/Client/Cache.lean`, `Privacy.lean` and `Queue.lean`, imported from `lean/Strata.lean`
and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom; `lake env lean Audit.lean` lists
only `propext`, `Classical.choice` and `Quot.sound`.

| Property | Theorem |
|---|---|
| Every commit and key destruction a current answer misses began within the contact bound; no assumption on notices | `current_recent` |
| Under complete notices, a current answer is the server's opened value at the known stamp; coherence at every step | `current_value`, `run_coh`, `current_answer`; with the working set's keys, `current_fetch` |
| A non-current answer is labelled with its generation, never ahead of what was heard | `read_stale_older` |
| Entries at rest carry the known erasure epoch; after a contact, the server's | `rest_one_epoch`, `contact_purges` |
| A receipt at the current stamp is read back as current | `receipt_read`, `read_after_commit` |
| Without complete notices / with the epoch read after opening / without the persisted guard / with the generation alone | `incomplete_notice_serves_stale`, `stamp_after_open_serves_erased`, `unguarded_put_resurrects` (and `guarded_put_drops`), `browser_generation_only_serves_erased` |
| The privacy invariant in every reachable state | `reachable_inv` |
| A ciphertext opens only with its owner's key; what a session is served was written by its principal | `no_cross_principal`, `served_own` |
| After sign-out, online or offline, nothing the principal left opens in any later state | `signout_erases`, `offline_signout_erases` |
| After expiry nothing opens; only a session of that principal unlocks; the same principal does | `expire_locks`, `unlock_needs_own_session`, `expire_then_same_principal_unlocks` |
| A revoked device, or all devices, unreadable from the next reload; a kept key survives | `revoke_device_erases`, `revoke_all_erases`, `offline_key_survives_reload` |
| A first submission commits only if allowed and at the decided epoch and version; others change nothing | `submit_ok_guard`, `submit_not_ok_unchanged`, `submit_classes` |
| At most once from the two retention rules; at most once for a stamped write whatever is forgotten or restored | `at_most_once`, `stamped_at_most_once` |
| Counterexamples: create resent after forgetting, rekeyed create, restore without the epoch; with it, once | `create_twice_after_forget`, `rekeyed_create_applies_twice`, `restore_without_epoch_applies_twice`, `restore_with_epoch_applies_once` |

The central statements:

```lean
theorem current_recent (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Base s c)
    (hr : read B s.now c k = .current v) :
    (∀ g, c.known < g → g ≤ s.gen → s.now ≤ s.gTime g + B) ∧
    (∀ y, c.knownX < y → y ≤ s.x → s.now ≤ s.xTime y + B)

theorem current_answer (B : Nat) (ops : List (Op K V)) (sc : Server K V × Cache K V)
    (h : Base sc.1 sc.2) (hv : Vals sc.1 sc.2) (hr2 : RunC2 sc ops) (k : K) (v : V)
    (hr : read B (run sc ops).1.now (run sc ops).2 k = .current v) :
    v = (run sc ops).1.view (run sc ops).2.known (run sc ops).2.knownX k ∧ (...the two bounds above...)

theorem rest_one_epoch (h : Base s c) (he : c.entries k = some e) : e.x = c.knownX

theorem offline_signout_erases (h : Inv s) (hs : s.session = some p) (hc : c ∈ s.disk)
    (ho : c.owner = p) (ops : List (Op α)) : ¬ Opens (run (step s .signOutOffline) ops) c

theorem submit_ok_guard (h : (s.submit allow w).2 = .ok) (hnew : s.seen w.key = false) :
    allow = true ∧ ∀ ex, w.expected = some ex →
      s.stamp w.target = ex.version ∧ ∀ e, ex.epoch = some e → s.epoch = e

theorem at_most_once (R : Nat) (ops : List Op) : ∀ s, AMO R s → Rules R s ops → AMO R (run s ops)

theorem stamped_at_most_once (W : IKey → Write) (ops : List Op) (hk : Keyed W ops) :
    ∀ s, SAMO W s → SAMO W (run s ops)
```

### What is not proved, and what turned out false

False as first stated, with the strongest true statement:

- **"A queued offline write is applied at most once"**: false for a create resent after the server
  forgets its key (`create_twice_after_forget`). True from the server's and the client's retention
  rules (`at_most_once`), and for a write with an epoch and version stamp always
  (`stamped_at_most_once`).
- **"A read after a confirmed commit returns the committed value"**: false when a newer stamp is
  already known when the receipt arrives. True for a receipt at the current stamp (`receipt_read`).
- **"The cache never serves an entry staler than its last known generation"**: as a statement about
  the server's current value it needs complete notices (`incomplete_notice_serves_stale`), which
  today's server does not give. True without them: what a current answer misses began within the
  contact bound (`current_recent`); true with them: it is the value at the known stamp
  (`current_value`).
- **"After sign-out no cached plaintext is readable"**: true for ciphertext at rest and the hot set,
  online and offline (`signout_erases`, `offline_signout_erases`); not for what code in the origin
  copied out while the person was signed in, nor for pixels already drawn.
- **The first revision's erasure theorem** matched entries against the server's live epoch, which
  the browser cannot read; it is replaced by `current_recent` and `contact_purges`.

Not proved, each with the follow-up that carries it:

- **The JavaScript against the models** (D20): follow-up 5.
- **C2**, complete notices, and **C3**, the epoch read before opening: the server's obligations,
  follow-up 2.
- **The stamps and epochs in the server**: W4 for the generation (WG-5389), `x` in the destruction's
  transaction, the epoch tag (follow-up 2), `expected-epoch` (follow-up 3).
- **The kernel's decision on re-submission** is `allow` in the queue model; its faithfulness to the
  kernel is the working set's `submit_one_generation`, and the composition is an argument.
- **Ideal encryption and key deletion** (P1, P3); **one device per instance** of the privacy model.
- **Liveness**: the queue drains when the network returns, a frozen leader is replaced, and `B`
  against real clocks: tested (follow-ups 8, 9, 13), not proved.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5415 with its acceptance criteria. Server items unblock client
items; within the client, the stores precede the queue.

1. (WG-5434) **Evidence check indexes web tests.** *Accepted when*: a requirement whose `checkedBy`
   names a web test passes with a passing recorded result and fails with a failing or missing one
   (negative fixtures); `specc lint` accepts the name form.
2. (WG-5435) **Stamps, chained frames, complete notices and the erasure epoch on the server.** Needs
   WG-5389. *Accepted when*: a view reply's stamp is the generation it was read at; a key destruction
   raises `x` and no `g`; a read that opens a sealed field takes `x` before opening (a test destroying
   a key between the epoch read and the open finds the reply stamped with the old epoch); a restore
   changes `e`; frames chain `from`/`to` per connection and a heartbeat carries `from`/`to` with no
   keys; a reconnect replays from the generation recorded with `after` or answers `resync`; a role
   change notifies every subscriber whose visible set it may alter; publication uses the schema and
   members recorded with the event; every out-of-commit write sends `resync`; a view whose policy reads
   the clock or follows references is marked volatile; `docs/protocol.md` lists all seven notice kinds.
3. (WG-5436) **`expected-epoch` on write contracts.** *Accepted when*: a stale epoch with a matching
   version is refused `conflict`; a correct one commits.
4. (WG-5441) **Pure `cache.js`.** Needs 1 and 2. *Accepted when*: kernel tests from FE-001, FE-009,
   FE-002, FE-003 and FE-012 pass, including the contact bound demoting an entry and a late put of an
   older epoch dropped; the kernel imports no storage, crypto or network API.
5. (WG-5448) **Differential check of the client models.** Needs 4 and 9. *Accepted when*: 10,000
   seeded schedules agree with the Lean executable; mutants (no stamp check, no persisted epoch check,
   key redrawn on resend) are caught.
6. (WG-5437) **Contracts declare whether they may be queued.** *Accepted when*: specc refuses the
   property on never-queued kinds with a negative fixture; the generated module lists exactly the
   marked contracts.
7. (WG-5442) **Device enrolment, key share and encrypted IndexedDB.** Needs 1. *Accepted when*: no
   plaintext in IndexedDB; after online or offline sign-out no record decrypts and signing in again as
   the same person does not unlock the old data; another principal reads nothing; a revoked device's
   records are unreadable after a reload; the endpoint refuses another principal's session, requires
   the request token and is rate limited; shares are sealed at rest, rotated before their lifetime
   ends, and collected after it; a restore of the identity database does not bring a deleted share
   back.
8. (WG-5444) **Leader tab.** Needs 7. *Accepted when*: one socket with two tabs; closing the leader
   moves it; a frozen leader (Playwright's page freeze) is replaced through the heartbeat and steals;
   the back-forward cache case is tested, not assumed; no record data on the channel.
9. (WG-5447) **The write queue.** Needs 3, 4, 6, 7, 8. *Accepted when*: reload while `sending` applies
   once; create then update offline sends the update with the created id after the create's receipt
   and no idempotency mismatch; a reference to an offline-created record is substituted; a create past
   the retention less the margin is not resent and "Check" finds its outcome by key; a conflict is
   never merged without the person; a client below the server's minimum version keeps its queue and
   is asked to reload; expiry then the same principal drains; another principal sees none of it.
10. (WG-5445) **Service worker and the shell.** Needs 7. *Accepted when*: offline reload reaches the
    shell; a slow network falls back after the timeout; no bootstrapped page or API reply is in the
    Cache API; a new build waits for the drain; `/app-shell` has the page's CSP.
11. (WG-5438) **In-house QR encoder.** *Accepted when*: the ISO worked example and frozen fixtures
    match; `uqr` is gone.
12. (WG-5439) **Vendored fonts and the bundle check.** *Accepted when*: fonts load from
    `web/src/fonts`; the packages are gone; the bundle check has a negative fixture.
13. (WG-5443) **Headless uiScenarios.** *Accepted when*: all 45 run with browser-only steps skipped
    with a reason; a seeded fault run of `ui-reconnect-catches-up` passes and reproduces from its seed;
    a reconnect mutant fails it.
14. (WG-5446) **Web Components.** Needs 11. *Accepted when*: axe passes; the record picker submits
    through the native form and shows `:invalid`; role queries find it.
15. (WG-5440) **Structured clone once.** *Accepted when*: the driver posts its string, the main thread
    parses it, an unchanged model posts nothing.
16. (WG-5449) **PRV-351 and PRV-353 name the browser copies.** *Accepted when*: the requirements say
    how a copy outside the platform's reach is rectified and named in a receipt, citing D-FE-11, and
    `specc lint` passes.
17. (WG-5450) **The gate runs `split-spec --verify`, and the Rust reader is tested on area parts.**
    *Accepted when*: the gate fails when an area item sits in a consumer part; a `strata-spec-parts`
    test assembles decisions and copies split across a consumer part and an area part, and refuses an
    id in both.

## Open questions

- Is a realm's idle lifetime (12 hours by default) the right offline reading window, or should a
  realm set a shorter one for browser copies alone?
- Should keeping an offline key require something the server can withdraw, such as a periodic online
  check, so that its bound is enforced rather than stated as absent?

## Answers to the review

### First review

| # | Finding | Answer |
|---|---|---|
| 1 | The erasure theorem read the server's live stamp | Fixed. One model with a lagging known stamp, frames and replies as contacts, a contact bound, and carry-forward: `current_recent`, `current_value`, `current_answer`, `contact_purges` |
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
