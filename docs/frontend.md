# The browser client

How the Strata web application should use the browser's own primitives: what it keeps, where, under
whose key, how it stays coherent with the server, how it works offline and reconciles what was
written there, and how it is tested. This is a design. Its safety properties are proved in Lean
(`lean/Strata/Client/`), its requirements are in `specifications/frontend.jsonc`, and the work is
listed at the end in dependency order. The coherence model is the backend working set's
(`docs/working-set.md`, `lean/Strata/WorkingSet.lean`) seen from outside the server.

## The recommendation in one page

- **Keep the shape that exists.** The pure kernel (`update`, `view`) in a dedicated worker, effects
  run by a driver, the main thread drawing a view model with DOM APIs only, generated platform
  screens, hash-addressed tenant parts (D-8250). Everything below is added around it, as kernel
  messages and effects, so the kernel stays pure and testable in Node.
- **Three stores, each for one kind of content** (D-FE-2). A service worker keeps only
  content-addressed files (built assets, fonts, `/api/ui/parts/<hash>`) in the Cache API and passes
  everything else through. IndexedDB keeps stamped view results, the write queue and drafts,
  encrypted per principal. The kernel worker's memory keeps the hot set.
- **The browser cache is a stamped copy** (D-FE-1). Every reply carries one stamp: the tenant epoch,
  the generation and an erasure epoch. A value is drawn as current only at the newest stamp the
  browser has heard of, carried forward by notices, and otherwise drawn labelled with its age.
  Correctness never depends on it: a miss is a read, and every write is decided again by the server.
- **One leader tab per principal, chosen with Web Locks** (D-FE-3). It owns the socket and drains
  the write queue; other tabs hear notices on a BroadcastChannel. Not a SharedWorker.
- **Offline writes are queued intents** (D-FE-5) with an idempotency key drawn once and a record
  stamp, sent on reconnecting against the server's guarded commit. Five conflict classes, each with
  what the person sees. Some operations never queue (D-FE-6).
- **Data at rest is encrypted under a key the server can take away** (D-FE-7): a per-principal,
  per-device key share held by the identity service, from which the worker derives a
  non-extractable key. Sign-out deletes the share and the key, so nothing left behind is readable.
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
   cell arbiter, pinning closures) does not: a tab's working set is one person's recent pages, a few
   megabytes, and plain LRU by accounted bytes is enough. The browser also cannot do what makes the
   working set safe in checked mode, a generation read in the store's own transaction: it learns
   generations from messages, so its guarantee is weaker and relative to what it knows
   ("never staler than the last known generation"), and the proofs say exactly that.
2. **The browser cache sits above sealing, which the working set rejected for itself.** The gateway
   opens sealed fields before it answers, so the browser holds plaintext. A key destruction (D-350)
   raises no generation, so a cache keyed by generation alone serves erased values: that is the
   working set's own counterexample (`above_sealing_serves_erased`), and it applies unchanged
   (`browser_generation_only_serves_erased`). The fix is a new server obligation, an erasure epoch
   in every stamp (D-FE-8), not something the client can do alone.
3. **"Encrypted IndexedDB under a non-extractable session key" does not by itself separate
   principals.** A non-extractable key cannot be exported, but any script of the origin can use it.
   If the key lives on the device, separation between two people on one browser profile rests on
   the code choosing the right partition, and on nothing else. What separates them cryptographically
   is a key the device cannot produce without the server: a key share held by the identity service,
   given only to a session of its principal (D-FE-7). The proofs take the strong attacker model (any
   code may try any key the browser holds).
4. **"A session key" is the wrong lifetime.** Tying the data key to the session makes every expiry
   destroy the person's unsent offline work. The share is per principal and device instead: session
   expiry locks the data, re-authentication as the same person unlocks it, sign-out and revocation
   destroy it (`expire_locks`, `unlock_needs_own_session`, `signout_erases`).
5. **"Applied at most once" is false as stated for a create**, because the server keeps an
   idempotency key for 24 hours (D-6595) and a create has no record to guard. The proofs give the
   counterexample and the strongest true statement (`create_twice_after_forget`,
   `at_most_once`, `stamped_at_most_once`), and the design follows it: a create is not resent once
   its first sending is older than the retention less a margin.
6. **"Generated parts in the Cache API" is right; "generated code" mostly is not a cache question.**
   `web/src/generated` is compiled into the hashed bundle by Vite, so it is cached as the bundle is.
   Only the tenant's interface parts are fetched at run time.
7. **A SharedWorker is not the multi-tab answer yet.** Chrome for Android began shipping it only in
   2026 (re-enabled in the Chrome 148 beta; caniuse lists it from 151) and Samsung Internet does not
   support it. Web Locks and BroadcastChannel are in every engine the platform supports, and work
   from dedicated workers.
8. **A frontend part of the specification cuts across the split by consumer.** The parts are split by
   the tool that reads them; a frontend part is an area. It is added as an *area part* that
   `scripts/split-spec.mjs` fills by id prefix, so the tools see the same resolved document. If the
   owner prefers the consumer split, removing the area from `AREA_PARTS` and running the script
   moves every item to its consumer part mechanically.

## The current state

| What | Where | What it does |
|---|---|---|
| Kernel | `web/src/kernel/update.js` (1,284 lines), `view.js`, `types.js` | `update(state, msg) -> {state, effects}`, `view(state)`; time and randomness arrive on the message (`at`, `rand`); request ids, correlation ids, a `generation` counter that drops replies to requests made before user state was cleared (`USER_KINDS`, `LATEST_ONLY`) |
| Effects | `web/src/kernel/types.js:75-90` | `http`, `ws_connect/send/close`, `timer`, `push_url`, `store` (localStorage), `scroll_top`, `passkey`, `log` |
| Idempotency keys | `update.js:192-217` (`newKey`, `call`) | A create carries `idempotency-key`, reused for a retry of the same body (D-6595); the key lives in kernel memory only |
| Driver | `web/src/worker/main.js` | Runs the kernel, executes effects (fetch, one WebSocket, timers), posts `[model, applied]` after a `JSON.stringify` dedupe |
| Main thread | `web/src/main.js` | Starts the worker through a Trusted Types policy, draws with `components/dom.js`, delegates DOM events to typed messages, handles page effects; no application request |
| Renderer | `web/src/components/dom.js` | Virtual nodes patched in place with DOM APIs; URL attributes filtered (`safeUrl`); focused controls keep their value; `holdValues` against stale renders |
| Views | `web/src/views/*.js` | `shell(model)` and pages as virtual nodes; `tenant.js` draws the interpreter's page model |
| Tenant interface | `web/src/kernel/tenant-ui.js` | Manifest and parts by hash in kernel memory (`uiInit`, `acceptManifest`, `partsToFetch`), held parts during an upgrade, lost-draft notices |
| Generated | `web/src/generated/*.js` | Routes, contracts, forms, actions, live bindings, errors, client machines (`client-*-flow-impl.js`), from the specification |
| Trusted Types | `web/src/trusted-urls.js`; CSP in `crates/bff/src/routes.rs:2530` | One policy, `strata-worker`, for the worker's address; `require-trusted-types-for 'script'`; `worker-src 'self'`, `font-src 'self'`, `connect-src 'self'` |
| Persistence | `main.js:41-47, 97-103` | localStorage for the theme only. No IndexedDB, no Cache API, no service worker |
| Multi-tab | none | Each tab opens its own socket |
| Offline | `worker/main.js:97-103` | A socket that closes while `navigator.onLine` is false is not taken as refused; nothing else |
| Third-party code in the browser | `kernel/factors.js:18` (`uqr`), `styles/main.css:5-8` (`@fontsource-variable/*`) | The QR encoder is bundled code. The fonts are bundled by Vite into `/assets`, so their bytes already come from the site; what reaches the browser from a third party is the font files and their CSS, through npm |
| Tests | `web/test/*.test.js` (`node --test`), `web/test/support/hot-reload-sim.js`, `tests/e2e/generated` | Kernel tests in Node; a seeded multi-tab simulator of the hot reload of tenant parts against a fake gateway; Playwright journeys generated from the uiScenarios |

The protocol is `docs/protocol.md`: view models by `GET /api/view`, notices on the socket carry no
record data (`upsert`, `remove`, `schema`, `membership`), `n` counts deliveries and `seq` may skip.

## Constraints

- **The kernel stays pure** (K1 for the browser): no fetch, no storage, no crypto, no clock. Storage
  and crypto are effects; their results are messages.
- **The server is the authority.** A browser copy can cost a read; it can never change a decision.
- **Copies have rules** (CPY-001 to CPY-014): each copy class has a row with owner, key, freshness
  bound and erasure point. The browser copies are three new rows (`copy-browser-view`,
  `copy-browser-queue`, `copy-browser-interface`).
- **Sessions** end 12 hours after last use or 14 days after sign-in (D-700, IDN-259); the cookie is
  `HttpOnly` and invisible to scripts (D16).
- **No markup is parsed** (D18): nothing becomes HTML; the CSP allows no inline script or style.
- **No client framework** (UXA-700, D-28), and now no third-party runtime code at all (D-FE-9).

## The primitives, weighed

### Service worker and the Cache API

Options: (a) no service worker; (b) a service worker that caches content-addressed files only;
(c) a service worker that also caches API reads and decides freshness.

**Recommendation: (b).** A file named by its hash cannot be stale, so caching it needs no coherence
argument at all. Freshness decisions belong to the kernel, which knows stamps, sessions and
principals; a service worker deciding them would be a second, untestable coherence layer, and it
would see responses of every principal. Concretely:

- `/assets/*` (the Vite bundle, including `web/src/generated`, fonts and the worker) and
  `/api/ui/parts/<hash>` (`private, max-age=31536000, immutable`): cache-first, one cache per build
  (`strata-assets-<build>`), parts in `strata-parts` keyed by hash and dropped when no manifest held
  by any partition lists them.
- **Navigations**: network-first. The bootstrapped page carries the session, the request token and
  the first view model (D17), so it is **never** stored. For an offline reload the service worker
  serves a cached **shell**: a new route `GET /app-shell` that returns `index.html` without the
  bootstrap block, with the same CSP headers as the page. The kernel boots from it without bootstrap
  data and enters offline mode (below).
- Everything else (`/api/*` except parts, `/ws`, `/federation`, `/keys`, `/scim`): not intercepted.
- **Updates**: the new script installs its cache and waits; the kernel is told a new version waits
  and offers a reload. It never takes over while the leader is draining the queue. Queue entries
  carry a format version and the new kernel migrates them forward.
- **Registration** goes through the Trusted Types policy (a service worker's address is a script
  sink): `acceptWorkerUrl` gains the single exact path `/sw.js` (unhashed, so its scope is `/`).

### IndexedDB

Options: (a) localStorage (synchronous, main-thread only, strings, a few megabytes); (b) the Cache
API for JSON (no transactions, no indexes, request/response shaped); (c) IndexedDB; (d) the Origin
Private File System (fast files, no transactions across records).

**Recommendation: IndexedDB**, used from the kernel worker only, one database `strata` per site
(the origin is the tenant's host, so the browser already separates tenants):

| Store | Key | Value | Encrypted |
|---|---|---|---|
| `partitions` | principal tag | device id, created, last unlocked, offline key flag | no (no personal data) |
| `keys` | principal tag | the non-extractable derived keys, only when offline is kept | the keys themselves |
| `views` | `[principal tag, HMAC(index key, view key)]` | stamp, last read, bytes, IV, ciphertext | yes |
| `queue` | `[principal tag, sequence]` | state, attempts, first sent (server time), IV, ciphertext of the intent, key, decided stamp and base values | yes |
| `drafts` | `[principal tag, HMAC(form key)]` | IV, ciphertext of unsaved form values | yes |
| `site` | `manifest` | the last interface manifest (public, D-8250) | no |

The principal tag is an HMAC of the principal id under a key derived from the share, so the store
does not list who used the device; view keys (addresses with queries) are indexed by HMAC so the
store does not list which pages were read. Writes are **guarded puts**: in one `readwrite`
transaction, read the entry and write only if the new stamp is not older (the client's analogue of
the guarded commit, `Cache.put`). Queue appends use `{durability: 'strict'}`, and "saved on this
device" is shown only after the transaction completes.

### The worker's memory hot set

The decrypted entries of the current page and recent pages, the manifest and its parts, the session.
**Policy: LRU by accounted bytes** (the JSON length of the entry), a budget per tab (8 MB to start,
to be measured), the current view and the manifest exempt. The working set's GDSF/TinyLFU apparatus
earns its cost across tenants in a shared cell; a tab has one person and a few pages. Memory is
cleared on sign-out, expiry and principal change (`endSession` in the privacy model).

### Multiple tabs

| | SharedWorker owning socket and cache | Leader chosen with Web Locks, BroadcastChannel to followers |
|---|---|---|
| Support | Chrome desktop, Firefox, Safari 16+; Chrome for Android only from 2026; **not Samsung Internet** | Web Locks: Chrome 69, Firefox 96, Safari 15.4; BroadcastChannel: all current engines; both available in dedicated workers |
| Failure of the owner | The SharedWorker lives while any tab does; a crash takes every tab's kernel with it | The lock is released when the leader's worker ends; the next waiting tab's request is granted |
| Kernel per tab | No: one kernel would serve every tab's route and forms, so the kernel's state becomes multi-tab | Yes, unchanged |
| Debugging, tests | Separate inspector; harder in Playwright | Ordinary workers |

**Recommendation: the leader.** Each tab's kernel worker requests the exclusive lock
`strata/<principal tag>` with `navigator.locks.request` and holds it while it lives. The holder opens
the socket, drains the queue, and posts notices (never record data) on the BroadcastChannel
`strata/<principal tag>`; followers apply them to their own kernels and read the shared IndexedDB.
Any tab may append to the queue. When a page goes into the back-forward cache (`pagehide` with
`persisted`), the main thread tells its worker to give up the lock and reconnect on `pageshow`.
Two tabs that both believe they lead (a bug, a lock released late) are harmless: idempotency keys
keep a write applied once (`at_most_once`, which allows any number of senders).

### Web Components

Options: (a) keep virtual nodes and plain elements only; (b) custom elements for everything;
(c) custom elements as leaf renderers where an element earns it.

**Recommendation: (c), in the light DOM** (D-FE-4):

- An element holds no application state; it takes properties set by the renderer and emits
  `CustomEvent`s, which `main.js` turns into kernel messages like any DOM event. `dom.js` gains a
  property channel (an attribute named `.options` is set as the property `options`), so structured
  data reaches elements without serialising it into attributes.
- **Light DOM** by default: labels (`for`), `aria-labelledby` and `aria-describedby` cannot reach into
  a shadow root (the cross-root reference proposal has not shipped), the site's token styles apply
  directly, and the role queries of the uiScenarios find everything. Shadow DOM only for a leaf with
  no cross-references and its own drawing: `<strata-qr>`, the connection indicator.
- **Form-associated custom elements** (`static formAssociated = true`, `attachInternals()`,
  `setFormValue`, `setValidity`, ARIA through `ElementInternals`) only for a control with no native
  element: the first is a record picker for reference fields (a combobox whose options the kernel
  fetches). Native `input`, `select` and `textarea` stay for everything the interpreter draws today.
  Support: Chrome 77, Firefox 98, Safari 16.4.
- **Declarative Shadow DOM: no.** It serves server rendering, which Strata does not do (D17), and
  using it from script means parsing markup (D18).

### Trusted Types and CSP

Keep the single policy `strata-worker` and extend its acceptance to `/sw.js`; create no `default`
policy. Trusted Types now ship in Chrome 83, Firefox 148 and Safari 26; where they are missing the
same check runs on the string (`workerScriptUrl`). The CSP needs no new source: `worker-src 'self'`
covers the service worker, `connect-src 'self'` covers the same-host socket in current engines,
`font-src 'self'` covers vendored fonts. `/app-shell` carries the same CSP as the page. Custom
element definitions are not sinks.

### Structured clone and transferables

Today the driver stringifies the view model to compare it with the last one and then posts the
object, which structured-clones it again. **Recommendation: post the string already made** and parse
it on the main thread (one serialisation instead of two, and the dedupe stays). Transfer an
`ArrayBuffer` only for bulk binary (an upload's chunks, main thread to worker); `File` and `Blob` are
passed by reference anyway. Keys, ciphertexts and plaintext of the cache never cross to the main
thread; it receives view models only.

## The kernel and its event model

The kernel stays `update(state, msg) -> {state, effects}`. What is added is messages and effects.

| New messages (into the kernel) | From |
|---|---|
| `store_loaded {key, entry}`, `store_written {req, ok}`, `queue_loaded {entries}`, `drafts_loaded` | the storage driver, after decrypting |
| `keys_ready {mode}`, `keys_gone {reason}` | the key driver (`gone`: share deleted at the server) |
| `leader {held}`, `peer {notice}` | Web Locks, BroadcastChannel |
| `connectivity {hint}`, `visibility {hidden}` | the main thread (`online`/`offline`, `visibilitychange`), as hints |
| `storage {usage, quota, persisted}`, `sw {waiting}` | `navigator.storage`, the service worker |

| New effects (out of the kernel) | Done by |
|---|---|
| `store_get`, `store_put {key, stamp, value}` (guarded), `store_purge {scope}` | storage driver |
| `queue_append`, `queue_mark {seq, state}`, `queue_remove` | storage driver |
| `key_fetch`, `key_keep_offline`, `erase_partition` | key driver |
| `lead`, `broadcast {notice}` | coordination driver |
| `persist_storage`, `estimate_storage`, `activate_update` | page and service worker |

Rules:

1. **One message at a time.** The worker's event loop runs one message to completion: each is the
   atomic step the models assume (W2 of the working set). Async work is an effect; its result is a
   later message carrying the request id. The kernel never waits.
2. **The kernel decides; drivers execute.** Whether an entry is current, what a notice invalidates,
   what to persist, when to send a queued write, what the person is told: kernel. Encryption,
   IndexedDB, locks, sockets: drivers. So `web/src/kernel/cache.js` and `queue.js` are pure and are
   the JavaScript of `Strata.Client.Cache` and `Strata.Client.Queue`.
3. **Connectivity is learned from outcomes.** `navigator.onLine` and the `online` event are hints
   that prompt a retry; a request that fails with status 0 and a socket that closes are the facts.
4. **Responsive by construction.** Input messages are small and handled first in arrival order; no
   message does unbounded work in the kernel (lists are paged; decryption of many entries happens in
   the driver and arrives as several messages). The driver posts at most one render per task, after
   the task's messages; the main thread draws only the newest model per animation frame.
5. **Cache first, then the network, never instead of it.** A navigation draws a current entry at
   once and asks nothing when the socket is live; draws a non-current one labelled
   (`refreshing`, which exists today) and asks; draws nothing it does not hold, and asks.

## The cache and its coherence

### What the server adds

- **A stamp on every reply and frame.** `GET /api/view` replies, commit receipts and the socket's
  `welcome`, `pong` and `event` frames carry `stamp: {e, g, x}`: `e` a tag of the tenant's epoch (a
  keyed hash of the generation nonce, so the nonce itself is not revealed), `g` the generation
  counter, `x` the erasure epoch. A view reply is read at **one** generation (the request-snapshot
  rule, `request_snapshot`), so it has one stamp. This needs the tenant generation in the store
  contract (WG-5389) and the read path's snapshot rule.
- **Notices that say what did not change.** Each `event` carries `from` and `to` stamps: between them
  only the notice's record (and so the views whose `depends_on` names its type or record, CPY-004)
  may differ for this principal. Invisible commits produce no frame, which is sound by D13 (they
  change nothing the principal may read); an out-of-commit change that alters what a principal may
  read (a restore, an anonymisation) sends `resync`. This is assumption C2 of the model.
- **The erasure epoch.** A per-tenant counter raised in the transaction of every key destruction,
  sent in every stamp (D-FE-8).

### What the browser does

- An entry is stamped with its reply's stamp. **Current** iff its stamp equals the newest the
  browser has heard of (`known`); otherwise **stale**, drawn with its age (`read_current_eq`,
  `read_stale_older`).
- A notice `from → to` re-stamps entries at `from` that it does not invalidate and drops those it
  does (`advance`); a reply ahead of the notice stream raises `known`, and the stream catches the
  older entries up.
- A commit receipt is put at the commit's generation, so a read after a confirmed save returns what
  was saved (`read_after_commit`), unless a later generation is already known, in which case the read
  is the newer value or labelled stale (`receipt_read`).
- A different `e` (a restore, a re-created tenant) or a `resync`: every entry becomes stale and is
  read again. A different `x`: every view result of the partition is **deleted**, not just hidden
  (`purge_leaves_current`), because it may hold an erased subject's plaintext.
- A `membership` or `schema` notice, or a session reply whose roles differ from the last one:
  every view result of the partition is deleted. Drawing a labelled stale copy is acceptable for
  age, not for content the person may no longer read.
- The kernel's invalidation map (notice to view keys) uses `depends_on`; it is sound when
  `depends_on` is complete, which CPY-004 requires of the server.

## Offline writes

### The queue

A queued write is an intent the person decided ("Save" pressed) while the browser could not reach
the gateway, or while a reply was outstanding. It holds:

- the contract and its inputs, as the generated request builder makes them;
- an **idempotency key**, 128 random bits drawn once when the intent is queued and stored with it
  (`rekeyed_create_applies_twice` is why it is never drawn again);
- the **decided stamp**: the record's version and the tenant epoch tag of the copy the person edited
  (none for a create), and the base values of the fields the intent changes (for rebasing);
- state (`queued`, `sending`, `answered`), attempts, and the server time of its first sending.

Unsaved form text is not a queued write; it is a draft (`drafts`), kept so that a reload loses no
typing (G-206), and never sent without the person's decision.

### Submitting against the guarded commit

On reconnecting, the leader reads the session, then drains the queue in order, one write at a time
per record (later writes to a record wait for the earlier one's answer, since they were decided on
its result). Each is sent with `idempotency-key` and `expected-version` (D-6595), plus a new
`expected-epoch` input (follow-up 3) so that a restore, which can set versions back, never lets a
stale write match. The server answers a known key with the recorded outcome; otherwise the kernel
decides the intent **again on the current state**, and the store commits it only at the expected
stamp. The browser's offline checks (form rules, record rules) are advice to the person, never the
decision.

What is proved of this (`Strata.Client.Queue`): an accepted write found its record at the stamp it
was decided on (`submit_ok_guard`); every other answer changes no record (`submit_not_ok_unchanged`)
and is one of the classes below (`submit_classes`); a write is applied at most once however often
and by however many tabs it is sent while the server keeps its key (`at_most_once`), and a write
with a stamp at most once even after that (`stamped_at_most_once`). A create sent again after its
key was forgotten is applied twice (`create_twice_after_forget`), so **a create whose first sending
is older than the key's retention less one hour is not resent**: it moves to "outcome unknown".

### Conflict classes, and what the person sees

| Class | Server answer | What the person sees | What happens to the entry |
|---|---|---|---|
| Moved | `conflict` (`version-conflict`, epoch mismatch) | "‹Title› was changed after you edited it offline." Their version beside yours, field by field | If the fields you changed were not changed meanwhile: re-decided at once on the new version with a new key, and a quiet note "merged with a change made meanwhile". Otherwise: you choose yours, theirs, or edit; your choice is a new intent |
| Gone | `not_found` | "‹Title› was deleted while you were offline. Your changes were not saved." With a copy of your text and "Create as new" | Closed; "Create as new" queues a create |
| Refused by the rules | `invalid` with `failed` evidence | The form again, the failing fields marked by the evidence, as for an online submit | Closed; resubmitting is a new intent |
| Not allowed now | `forbidden` / `unauthorized` | "You can no longer change ‹Title›." Your text kept to copy | Closed, not retried |
| Needs you first | `unauthenticated`, `step_up_required` | "Sign in again to send 3 changes" / the step-up prompt | Held; resent with the same key after sign-in or step-up |
| Waiting | network failure, `throttled`, `unavailable` | The connection indicator: "3 changes waiting to send" | Retried with backoff, same key |
| Outcome unknown | no answer within the key's retention (creates only) | "We could not confirm whether ‹Title› was saved." With "Check" (reads the list or record) | Never resent automatically; the person decides |
| Mismatched key | `idempotency-mismatch` | Reported as an error of the application | Closed; logged (it means a client defect) |

A person can always open the queue (from the connection indicator), see each waiting change, and
discard one.

### Durability, crashes and reloads

The queue is in IndexedDB with strict durability; "saved on this device" appears only after the
transaction completes. A write marked `sending` when the tab died is resent by the next leader with
the same key, which is safe by `at_most_once`. The queue survives reloads, crashes and browser
restarts, subject to eviction (below). Its entries are readable only with the principal's key, so a
reload while offline in online-keyed mode keeps them locked until the next sign-in (below).

### Which tab owns it

The leader (Web Locks) drains; any tab appends. A tab that appends tells the others on the
BroadcastChannel, so the leader wakes.

### What may not be done offline (D-FE-6)

A contract allows queueing only if the specification marks it (a new contract property,
follow-up 6); by default nothing queues. Never queued: sign-in, step-up and anything that needs a
fresh authentication (15 minutes, D-700), payments and plan changes, roles, members, invitations,
sessions, passkeys and second factors, schema changes and the Impact Lab, subject rights requests,
and any operation whose policy reads a fresh value the browser cannot have. Their controls say
"needs a connection" while offline. **Sign-out works offline**: it erases at once (below) and leaves
an unencrypted marker with no personal data, so the next online page load calls `/api/logout`. A
person with unsent changes is told signing out deletes them.

### The session expiring while offline

The session reply gains `idle_expires_at` and `absolute_expires_at`. The worker ends the session
locally when either passes without contact (it cannot learn of a server-side revocation while
offline): it drops the key from memory, deletes a kept offline key, keeps the ciphertexts, and
shows "Your session ended. Sign in again as ‹name› to send 3 changes." Signing in again as the same
person fetches the same share and unlocks the queue (`unlock_needs_own_session`); signing in as
someone else opens a separate partition and the first stays locked and invisible.

## Privacy at rest

### What is protected, against whom

| Threat | Protection |
|---|---|
| The next person on a shared device after sign-out | Share deleted at the server, key deleted in the browser: nothing left opens, ever (`signout_erases`) |
| The next person after the first one's session expired without sign-out | The first person's key is gone with the session; the second person's session cannot fetch the first's share (`no_cross_principal`, `expire_locks`) |
| A stolen or lost device | Sign out every session from another device: the share is deleted and the device's copies are unreadable at its next reload (`revoke_reload_erases`), immediately in online-keyed mode once memory is gone |
| Code of the origin trying other partitions | Separation rests on which keys are present, not on code (`Opens`, `served_own`) |
| Disk forensics of deleted records | Out of scope, and why the offline key is kept only on a device the person marks as theirs |
| A person signed in, at their own unlocked device | Not protected, and not meant to be: they may read their own data |

### Keys and their lifetime

- **Key share**: 256 random bits per (principal, device), held by the identity service, returned by
  `POST /api/session/device-key {device}` to a session of that principal only. Deleted at sign-out
  on that device, at signing out every session, when the principal is removed or erased, and 14 days
  after it was last fetched (D-700's absolute lifetime), which bounds `copy-browser-queue`.
- **Device id**: random, in the `partitions` store; not a secret.
- **Derived keys**: HKDF-SHA-256 over the share gives an AES-GCM-256 key (`strata/at-rest/v1`) and an
  HMAC-SHA-256 key (`strata/index/v1`), both **non-extractable**. Each record has a random 96-bit IV
  and additional data `principal tag ‖ store ‖ key ‖ stamp`, so a ciphertext cannot be moved to
  another key or partition.
- **Online-keyed (the default)**: the derived keys live in the worker's memory only. A reload
  fetches the share again. Offline within a running page works; offline after a reload shows that
  the data is locked until the next connection.
- **Offline kept (opt-in)**: "Keep my work available on this device" persists the derived
  `CryptoKey` objects in the `keys` store (structured clone keeps them non-extractable), and asks
  for `navigator.storage.persist()`. A realm may forbid it. Kept until sign-out or expiry; at expiry
  the kept key is deleted and the share keeps the queue recoverable.

### Partitioning on shared devices

One partition per principal per site, named by the principal tag; one cookie session per browser
profile at a time (the cookie is the profile's). A partition whose share fetch answers "gone" is
deleted. The BroadcastChannel and lock names carry the tag, and no message carries record data.

### When permissions change, or a subject is erased, while a copy sits offline

- **Permissions**: the browser cannot learn of a change while offline. The copy is labelled stale
  and is bounded by the session's idle lifetime (12 hours by default, `copy-browser-view`). On the
  next contact a membership notice, a changed role set in the session reply, or a new epoch deletes
  every view result of the partition. Queued writes are decided again by the server, so a revoked
  permission refuses them ("Not allowed now").
- **The principal itself removed or erased**: the share is deleted, so every copy on every device is
  unreadable from its next reload (`revoke_reload_erases`), and in online-keyed mode from the moment
  the worker's memory goes.
- **Another data subject erased (D-350)**: the browser holds that subject's opened fields in view
  results. At the next contact the new erasure epoch deletes them (`epoch_stamp_erasure_safe`,
  `purge_leaves_current`). While offline they stay readable to the person who could read them,
  within the same 12-hour bound. That is the stated erasure point of `copy-browser-view`.

### Storage eviction

- **Safari** deletes all script-writable storage of a site (IndexedDB, Cache API, service worker
  registrations) after seven days of Safari use without the person interacting with the site; a web
  app added to the Home Screen is exempt. A queue left unsent that long is lost without the server
  knowing. The design states it: the queue panel says how long work is kept on this browser, and on
  Safari recommends adding the site to the Home Screen for offline work. Correctness is unaffected;
  only unsent work is at risk, and the person was told.
- **Quota**: `navigator.storage.estimate()` is shown in the queue panel; on `QuotaExceededError` the
  driver evicts view results by last read, never queue entries or drafts; if a queue append still
  fails the person is told the change is **not** saved on the device.
- **Persistence**: `navigator.storage.persist()` is asked for only with offline kept (it needs an
  engagement signal in Chrome, prompts in Firefox, and is supported in Safari 17+); a refusal is
  shown, not hidden.
- **Eviction of the cache is never an error**: an evicted view is read again (CPY-011's rule for the
  browser).

## No third-party runtime code

- **QR encoder** (`web/src/kernel/qr.js`, pure, in the kernel): byte mode, error correction M,
  versions 1 to 10 (an `otpauth` address of up to 213 bytes), Reed-Solomon over GF(256) with the
  standard generator polynomials, the eight masks scored by the four penalty rules, format and version
  information with their BCH codes. Tests take expected matrices from ISO/IEC 18004's worked example
  and from fixtures frozen once from an independent encoder, never from running ours. `uqr` leaves
  `package.json`.
- **Fonts**: the variable `woff2` files of Fraunces, Newsreader and Instrument Sans are vendored into
  `web/src/fonts/` with their OFL licences, subset to the scripts the site serves, declared in
  `web/src/styles/fonts.css` with `font-display: swap`; the `@fontsource-variable/*` packages leave
  `package.json`. A build check fails when the bundle contains a module from `node_modules`.

## Testing

### The kernel runs the uiScenarios in Node

The 45 uiScenarios (`specifications/behaviour.jsonc`) are journeys of steps. A Node runner
(`web/test/scenarios/`) runs each with the real kernel and the real view functions:

- **Driver**: Node's `fetch` and `WebSocket` against the real gateway of the development stack, with
  a cookie jar and the `Origin` header the gateway checks; the storage driver over an in-memory map
  with the same guarded-put semantics; crypto through Node's WebCrypto (`globalThis.crypto.subtle`,
  the same API). No fake server is written: the server's behaviour is the server's.
- **Drawing**: `shell(model)` gives virtual nodes; the runner computes roles and accessible names
  from them (a small table from tag and attributes to role, as the steps use them) and finds what
  `click`, `fill` and `submit` act on.
- **Steps it answers**: `goto`, `gotoPath`, `back`, `fill`, `submit`, `click`, `clickAction`,
  `press` (as the kernel message the key would make), `expectTitle`, `expectRole`, `expectText`,
  `expectNoText`, `expectCount`, `expectAttribute`, `expectScreen`, `expectNoReload`, `recordRequests`,
  `expectRequests`, `expectNoRequest`, `apiCall`, `expectStatus`, `switchBrowser` (a second kernel
  with its own cookie jar), `goOffline`, `goOnline`, `reload` (a new kernel over the same storage),
  `waitFor`, `mark`, `capture`, `expectCookie`, `setCookie`. Steps only a browser answers (`axeScan`,
  `expectNoOverflow`, `expectBeside`, `emulate`, `focus`, `logScan`, `expectLog`) are reported as
  skipped with their reason, and stay in the Playwright journeys (`tests/e2e/generated`).
- **Faults from a seed**, at the driver's boundary, in the manner of D19 (a fault is a function of
  the seed and the subject): a reply lost, delayed or duplicated; the socket dropped; the network cut
  and restored; a reload between any two messages; a second tab with its own kernel sharing storage
  and the lock. After every step the runner checks the client invariants: no current entry with a
  stamp other than `known`, no queue entry without its key, no plaintext of another principal in a
  kernel's state.

### In the browser

Playwright covers what Node cannot: drawing and accessibility (axe), the service worker (offline
reload from the shell, update), IndexedDB with real encryption (records are ciphertext; a second
principal on the same profile reads nothing), Web Locks failover (close the leader, a follower
takes over), Trusted Types (a non-policy worker address is refused), and the offline faults below.

### Offline faults

| Fault | Check |
|---|---|
| Drop the network mid-edit, save, restore | The write is sent once (server state and the idempotency table), the indicator goes from "waiting" to clear |
| Reload while a write is `sending` | Resent with the same key; applied once |
| Kill the leader tab during a drain | A follower takes the lock and finishes; nothing applied twice |
| Two tabs append and drain concurrently | Each write applied once, in each record's order |
| Change the record from another browser while offline, then reconnect | The "moved" class with both versions; a disjoint change merges with a note |
| Sign out while offline | The queue's ciphertexts remain on disk but nothing opens (read IndexedDB directly); the next online load calls logout |
| Let the session expire offline, sign in as the same person | The queue unlocks and drains |
| Sign in as another person on the same profile | Nothing of the first person is drawn, readable or in the second kernel's state |
| Erase a subject while a reader is offline, reconnect | The reader's view results stamped with the old erasure epoch are deleted |

## The specification part

`specifications/frontend.jsonc` is an area part (see "Where the framing should change", item 8, and
`specifications/README.md`). It holds ten decisions (D-FE-1 to D-FE-10), three copy classes, and
eleven requirements FE-001 to FE-011, milestone 6. Each requirement names its check: Lean theorems
with a `Verifies:` line, `"check": "test"`. They are stated at the boundary of the client models, as
KRN-013 to KRN-015 are stated of the kernel model, because a model is what can be checked today.

Requirements that need checks not yet written are **not** added before their checks exist, since a
declared check must exist (`scripts/gate-evidence.mjs`), and today the evidence check indexes Rust
tests and Lean theorems, not `node --test` results. Follow-up 1 makes it index web tests; each later
item adds its requirements with their tests:

| Planned requirement | Its check | Added by |
|---|---|---|
| The web application draws a cached view as current only at the newest stamp it has heard of | kernel test of `cache.js` and the differential against `Strata.Client.Cache` | 4, 5 |
| A queued write keeps its idempotency key across reloads | headless fault run (reload while `sending`) | 9 |
| Only the leader tab sends queued writes | Playwright: two tabs, leader killed | 8 |
| A contract not marked for queueing is refused while offline | kernel test over the generated contract list | 6, 9 |
| After sign-out no IndexedDB record of the principal decrypts | Playwright reading IndexedDB with the remaining keys | 7 |
| The bundle holds no module from `node_modules` | build check script with a negative fixture | 12 |
| The QR encoder's matrix for the ISO worked example is the standard's | kernel test | 11 |

## What is proved

`lean/Strata/Client/Cache.lean`, `Privacy.lean` and `Queue.lean`, imported from `lean/Strata.lean`
and listed in `lean/Audit.lean`. No `sorry`, `admit` or added axiom; `lake env lean Audit.lean` lists
only `propext`, `Classical.choice` and `Quot.sound` (several need none).

| Property | Theorem |
|---|---|
| Coherence (every entry is the server's value at its own stamp, none ahead of what the client knows) holds at every step of every schedule of commits, replies, notices and losses | `run_coh` |
| A current answer is the server's value at the last known generation | `read_current_eq`; with the working set's keys and `fetch`, `read_current_fetch` |
| Anything older is strictly older and labelled with its stamp | `read_stale_older` |
| A read after a confirmed commit returns the committed value; after other traffic, when nothing newer is known | `read_after_commit`, `receipt_read`, `notice_keeps_current` |
| A cache stamped by generation alone serves erased values (the browser is above sealing) | `browser_generation_only_serves_erased` |
| With the erasure epoch in the stamp, no read serves an erased value, over every schedule of admissions, writes, destructions and reads | `epoch_stamp_erasure_safe` |
| Purging at a new epoch leaves only current-epoch entries at rest | `purge_leaves_current` |
| The privacy invariant holds in every reachable state | `reachable_inv` |
| A ciphertext opens only with its owner's key; what a session is served was written by its principal | `no_cross_principal`, `served_own` |
| After sign-out, nothing the principal left opens in any later state | `signout_erases` |
| After expiry nothing of the principal opens; only a session of that principal unlocks it | `expire_locks`, `unlock_needs_own_session` |
| A share deleted at the server makes the device's copies unreadable from its next reload; a kept offline key survives it | `revoke_reload_erases`, `offline_key_survives_reload` |
| An accepted queued write found its record at the decided stamp; other answers change nothing; the answer classes | `submit_ok_guard`, `submit_not_ok_unchanged`, `submit_classes` |
| At most once while the key is kept; at most once for a stamped write regardless | `at_most_once`, `stamped_at_most_once` |
| A create resent after the key is forgotten, or resent under a new key, applies twice | `create_twice_after_forget`, `rekeyed_create_applies_twice`, `create_once_while_kept` |

The central statements:

```lean
theorem read_current_eq (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Coh s c)
    (hr : read c k = .current v) : v = s.hist c.known k ∧ c.known ≤ s.gen

theorem read_after_commit (s : Server K V) (c : Cache K V) (f : K → V) (k : K) (h : Coh s c) :
    read (run (s, c) [.commit f, .reply k (s.gen + 1)]).2 k = .current (f k)

theorem epoch_stamp_erasure_safe (ops : List SOp) :
    ∀ (c : List EEntry) (s : EStore), EOk c s → runEpoch c s ops = runTruth s.base ops

theorem no_cross_principal (s : St α) (h : Inv s) (c : Sealed α) (hc : c ∈ s.disk)
    (q : PrincipalId) (hq : s.keys q = some c.key) : c.owner = q

theorem signout_erases (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOut) ops) c

theorem submit_ok_guard (s : Srv) (allow : Bool) (w : Write) (h : (s.submit allow w).2 = .ok)
    (hnew : s.seen w.key = none) : allow = true ∧ ∀ e, w.expected = some e → s.stamp w.target = e

theorem at_most_once (ops : List Op) (hf : NoForget ops) : ∀ s, AMO s → AMO (run s ops)

theorem stamped_at_most_once (W : IKey → Write) (ops : List Op) (hk : Keyed W ops) :
    ∀ s, SAMO W s → SAMO W (run s ops)
```

### What is not proved, and what turned out false

False as stated, with the strongest true statement:

- **"A queued offline write is applied at most once"**: false for a create resent after the server
  forgets its key (`create_twice_after_forget`). True: at most once while the key is kept
  (`at_most_once`), and for a write with a record stamp always (`stamped_at_most_once`). The design
  enforces the condition for creates by not resending them past the retention less a margin.
- **"A read after a confirmed commit returns the committed value"**: false when the browser has
  already heard of a later generation when the receipt arrives (someone wrote after). True: it
  returns the committed value when nothing newer is known (`read_after_commit`, `receipt_read`), and
  otherwise the server's value at the newest known generation or a labelled stale answer (`run_coh`
  with `read_current_eq`, `read_stale_older`).
- **"The cache never serves an entry staler than its last known generation"** is true as a statement
  about answers drawn as current; an older entry is still returned, labelled, which is what offline
  reading needs (`read_stale_older`).
- **"After sign-out no cached plaintext is readable"** holds for ciphertext at rest and the hot set,
  in every later state (`signout_erases`). It does not cover plaintext the main thread has drawn on
  screen or the browser's own process memory before garbage collection, which no web API can erase.

Not proved, each with the follow-up that carries it:

- **The JavaScript against the models.** The models are evidence about the design, not proof about
  the code (D20). The differential check of `cache.js` and `queue.js` against the Lean models is
  follow-up 5.
- **C2, notice completeness**, is the server's obligation: the visible-notice rule (D13) plus a
  `resync` for out-of-commit changes. Its conformance case is part of follow-up 2.
- **The server realises stamps and epochs**: W4 for the generation (WG-5389), the erasure epoch
  raised in every destruction's transaction (follow-up 2).
- **Encryption is ideal** (P1) and **deletion removes a key** (P3); disk forensics are outside.
- **The privacy model has one device.** Several devices of one principal are several instances of
  it with the server state shared; their composition is an argument, not a theorem.
- **Liveness**: that the queue drains when the network returns, and the bound of the offline window
  against real clocks, are tested (follow-ups 9 and 10), not proved.

## Follow-up work, by dependency

Each is a Workgraph task under WG-5415 with its acceptance criteria. Server items unblock client items;
within the client, the stores precede the queue.

1. **Evidence check indexes web tests.** `scripts/gate-evidence.mjs` indexes `web/test/**/*.test.js`
   tests whose leading comment holds `Verifies: <REF>` and reads `node --test` results recorded by
   the gate. *Accepted when*: a requirement whose `checkedBy` names a web test passes with a passing
   result and fails with a failing or missing one (negative fixtures in
   `scripts/gate-evidence.test.mjs`); `specc lint` accepts the name form.
2. **Stamps, notices and the erasure epoch on the server.** Needs WG-5389. View replies, receipts and
   `welcome`/`pong`/`event` frames carry `{e, g, x}`; events carry `from`/`to`; out-of-commit changes
   send `resync`; key destruction raises `x` in its transaction. *Accepted when*: a conformance case
   shows a view reply's stamp is the generation it was read at; destroying a subject key raises `x`
   and changes no `g`; a restore changes `e`; a commit invisible to a subscriber sends it no frame and
   the next frame's `from` covers it.
3. **`expected-epoch` on write contracts.** *Accepted when*: an update carrying a stale epoch tag
   with a matching version is refused `conflict` (test at the boundary), and a correct one commits.
4. **Pure `cache.js` in the kernel**, the JavaScript of `Strata.Client.Cache`. Needs 2. *Accepted
   when*: kernel tests from FE-001, FE-009 and FE-002's wording (current only at `known`; older
   labelled; receipt read back; notice carry-forward; purge on new `x`; delete on membership notice)
   pass, and the kernel imports no storage or crypto API (source scan).
5. **Differential check of the client models.** `lean/Main.lean` gains the client operations; a Node
   test runs `cache.js` and `queue.js` and the Lean executable on the same generated schedules.
   Needs 4 and 9. *Accepted when*: 10,000 seeded schedules agree, and a mutant `cache.js` that skips
   the stamp check is caught.
6. **Contracts say whether they may be queued.** A contract property (`offline: "queue"`, default
   never), generated into `web/src/generated/contracts.js`. *Accepted when*: specc refuses the
   property on the never-queued kinds (D-FE-6) with a negative fixture, and the generated module
   lists exactly the marked contracts.
7. **Key share and encrypted IndexedDB.** `POST /api/session/device-key` in the identity service and
   gateway; the key and storage drivers; partitions; sign-out and expiry erasure; offline kept as an
   opt-in a realm can forbid. Needs 1. *Accepted when*: Playwright reads IndexedDB and finds no
   plaintext; after sign-out no record decrypts with any key present (FE-005's check in the browser);
   a second principal on the profile reads nothing; a revoked share makes records unreadable after a
   reload.
8. **Leader tab.** Web Locks, BroadcastChannel, bfcache release. Needs 7. *Accepted when*: with two
   tabs one socket is open; closing the leader moves the socket within one lock grant; notices reach
   the follower; no record data crosses the channel (test inspecting messages).
9. **The write queue.** Durable queue, keys drawn once, decided stamps, drain order per record, the
   conflict classes and their screens, outcome unknown for creates past the retention less one
   hour. Needs 3, 6, 7, 8. *Accepted when*: the headless fault runs of the offline table pass,
   including reload while `sending` (applied once) and an old create (not resent, "outcome unknown"
   shown).
10. **Service worker and the shell.** `/sw.js`, the content-addressed caches, `GET /app-shell` with
    the page's CSP, offline reload, update flow, the Trusted Types acceptance of `/sw.js`. Needs 7
    for offline mode. *Accepted when*: Playwright reloads offline and reaches the shell; no
    bootstrapped page or `/api/view` reply is ever in the Cache API (test enumerating caches); a new
    build waits until the drain finishes.
11. **In-house QR encoder.** Needs nothing. *Accepted when*: the ISO/IEC 18004 worked example and
    frozen fixtures for versions 1 to 10 match; `uqr` is gone from `package.json`.
12. **Vendored fonts and the bundle check.** Needs nothing. *Accepted when*: the fonts load from
    `web/src/fonts`; the `@fontsource-variable/*` packages are gone; a script fails the build when the
    bundle holds a module from `node_modules`, with a negative fixture.
13. **Headless uiScenarios.** The Node runner, the vnode role table, the step interpreter, the fault
    layer. Needs 4 for cache-aware runs, nothing for the first run. *Accepted when*: every scenario
    runs headlessly with its browser-only steps reported as skipped with a reason; a seeded fault run
    of `ui-reconnect-catches-up` passes; deliberately breaking the kernel's reconnect fails it.
14. **Web Components.** The property channel in `dom.js`, `<strata-qr>`, the connection indicator
    with the queue count, the form-associated record picker. Needs 11 for the QR element.
    *Accepted when*: axe passes on the desk pages; the record picker submits through the native form
    and its validity shows as `:invalid`; the uiScenarios' role queries find it.
15. **Structured clone of the view model once.** *Accepted when*: the driver posts the string it
    compared, the main thread parses it, and a kernel test shows an unchanged model posts nothing.

## Open questions

- Should the identity service extend the idempotency key retention for queued writes beyond 24
  hours (D-6595), so that creates written offline across a weekend can be resent without asking?
- Is a 12-hour offline reading window right for every realm, or should a realm set a shorter one?
- Should a person be able to see, from another device, which devices hold offline work?
