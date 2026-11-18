# Decision log

Decisions that affect correctness, in the order they were made. Each says what was chosen, what
it costs, and where it is enforced.

**D1. The kernel owns schema types and transition semantics; the substrate owns the forms around
them.** `transition` has to validate against the schema, so the schema cannot live in a crate the
kernel does not see. The substrate holds what interprets kernel data for people and programs:
tenant bundles, the expression syntax, view projection, Markdown, themes, proposals, subscription
notices. Both are pure.

**D2. A snapshot holds one tenant and says what it covers.** `State` carries one tenant and a
`Coverage`. Every record the kernel looks at goes through `rules::Lookup`, which refuses a
question the snapshot does not cover and names the missing `Need`. Cross-tenant modification is
therefore unrepresentable in a transition, and a partial snapshot cannot produce a wrong
acceptance. `needs` is a plan the shell loads from; when it falls short the kernel says so and the
shell loads more. Checked by `partial_snapshots_never_change_a_decision`.

**D3. Expressions use three-valued logic.** A comparison with `null` is `null`; a constraint is
violated only by `false`; a policy, guard or filter passes only on `true`; a type error is an
error, never `false`. This is SQL's behaviour and has SQL's trap: `len(new.x) >= 1` does not
forbid a missing `x`. The publication template's cover rule was first written that way and the
template test caught it. Write `not is_null(...)` when absence must fail.

**D4. A schema change says what happens to existing records.** `Reject` refuses the change while
any record would not conform. `Grandfather` accepts it, lists the non-conforming records in the
evidence, and lets them fail on their next write. The invariant "every record conforms" is
therefore preserved by everything except a grandfathered change, and the Lean theorem states
exactly that exclusion, with a lemma showing it is necessary.

**D5. Record ids are per-tenant counters.** Deterministic, replayable, and deliberately
overlapping between tenants, so that any code path that forgot the tenant would be caught by the
isolation tests rather than hidden by globally unique ids.

**D6. Canonical form is JSON with sorted keys.** `canon::canonical_json` sorts object keys
itself instead of relying on a serializer feature that dependency unification could flip. All
maps are `BTreeMap`; the purity scan forbids `HashMap` in the kernel and substrate.

**D7. Standard ordered maps and `Arc`, not a persistent-collections crate.** Records and schemas
are shared by reference; a step copies maps of pointers. Simple and sufficient at this scale. A
structural-sharing map can replace it behind `TenantState` without changing any signature.

**D8. View definitions are kernel schema.** A view is a named bundle of queries with argument
bindings. Keeping it in the schema means it is versioned, validated for referential integrity and
migrated with everything else. The route is an opaque address string to the kernel.

**D9. Deleting a referenced record restricts or clears; it never cascades.** A cascade needs
records the intent cannot name in advance, which complicates coverage for little present use.

**D10. A `protocol` crate was added to the initial layout.** The backend and the BFF must agree
on wire types; one small crate is better than the BFF depending on the backend.

**D11. The store trait is synchronous and the SQLite adapter holds one connection.** The kernel
is synchronous, SQLite has one writer, and the backend moves store calls off the async threads.
Commits are serialisable by construction. Read concurrency within the process is given up; the
capability statement says so.

**D12. The event table is the outbox.** Events are written in the commit transaction with
`published = 0`. A publisher task reads them in commit order, hands them to the hub and marks them
published. Nothing is published before commit; an event committed just before a crash is still
pending at the next start. Delivery is at-least-once and subscribers skip sequence numbers they
have seen.

**D13. Who hears about an event is the kernel's read decision.** `subscribe::notice` evaluates
`visible` on the record before and after the change, with the schema and members current at
publication. A draft produces no notice for a reader; a withdrawal produces a removal. A notice
carries no record contents; the client reads again through the authorized path.

**D14. Two counters on the socket.** `seq` is the tenant's event sequence and may skip for a
subscriber who may not see everything. `n` counts deliveries on the connection and never skips. A
gap in `n` means a lost frame and the client reconnects; after reconnecting it resumes from `seq`.

**D15. The tenant comes from the host name in the BFF's own configuration.** No header, query
parameter or body field from a browser can name a tenant. A session token presented on another
tenant's host resolves to an anonymous visitor.

**D16. CSRF: allowed origin plus the session's token.** State-changing requests must carry an
`Origin` from the configured list and the session's CSRF token in a header. Login, which has no
session, needs the origin and a custom header that a cross-site form cannot send. The cookie is
`HttpOnly` and `SameSite=Strict`.

**D17. The first page is bootstrapped, not server-rendered.** The BFF embeds the first view model
and session as a JSON data block and sets title, description and theme attributes. There is one
renderer, in the browser. The cost is that a browser without JavaScript sees only a notice.

**D18. Rich text is parsed on the server into blocks.** Tenant Markdown never becomes HTML on
either side. The browser builds elements from structured blocks with DOM APIs, links and image
sources are filtered twice, and the Content Security Policy allows no inline script or style.

**D19. Simulated faults are a function of the seed and the subject.** Whether step 12's commit
loses a race is decided by hashing the seed with the step, not by drawing the next number from a
stream. A branch that runs one extra intent therefore sees the same faults at the same places as
its baseline.

**D20. The Lean model is small and says so.** One object type, three value kinds, ownership-based
authorization, one schema change. Its update replaces a record where the kernel patches; the two
coincide on a patch that names every field. Agreement with the Rust kernel is checked by running
both on the same generated scenarios, and is labelled evidence, not proof.

**D21. Development logins live beside the tenant, not in it.** `accounts.dev.json` is read only
when `STRATA_DEV=1`. Passwords are stored as Argon2id hashes; a missing login costs the same time
as a wrong password; five failures lock a login for thirty seconds.
