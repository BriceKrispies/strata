/-
The tenant working set: what a runtime holds in memory in front of the store, and why holding it
changes no result.

This is a model of outcomes, not of an algorithm. The eviction and admission policy is a parameter
(`Policy`), with a state of its own of any type, so every theorem here holds for LRU, CLOCK, ARC,
LFU with decay, GreedyDual-Size-Frequency, a knapsack threshold, W-TinyLFU or anything else that
can be written as a function. What the runtime enforces whatever the policy says is in the
operations themselves: the budget check, the refusal to evict a pinned entry, the revision stamp.

The store is the authority (`Store`: the committed world and a revision per tenant). The working set
holds entries (`Entry`) under keys (`Key`): a tenant's header (schema, members, counter), one
record (present or absent), or the complete record list of a tenant. Each entry carries the
revision it was read or last confirmed at.

Assumptions, all of them:

W1. The kernel is the model of `Strata/Model.lean`; nothing here refers to Rust.
W2. One store, whose commits to one tenant are serialised and bump that tenant's revision by one
    (the store contract's conflict check, `docs/storage.md`). Commits go through the working set.
W3. The size of an entry is a function of its key and value (`size`), given as a parameter: the
    model does not say how bytes are measured, only that the same entry always measures the same.
W4. A miss is answered by the store at its current revision (read-through).

Theorems, in the order the design document cites them:

- Coherence: `hit_current`, `serve_eq_fetch`, `read_after_commit`, `commit_sound`, `runWS_sound`,
  `sound_of_advance` (another writer behind the working set's back).
- Invalidation completeness: `untouched_unchanged` (the delta names every key it changes).
- Coverage soundness in the model: `decideTenant_faithful`, `decideOn_fetch` (a decision made from
  the header and the needed records alone is the kernel's step).
- Budget: `stepWS_within`, `runWS_within`, `machine_within`, `resize_budget_exact`.
- Pinning and isolation: `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned`,
  `pin_resident`, `commit_keeps_other_tenants`, `commit_keeps_untouched`.
- Transparency: `stepWS_transparent`, `runWS_transparent`.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model

namespace Strata.WorkingSet

open Strata

/-! ## The header: what every decision about a tenant reads -/

/-- Everything of a tenant except its records: what the store's `load` always returns, whatever
    the needs (`partial_state` in `crates/storage-api/src/lib.rs`). -/
structure Header where
  schema : Schema
  owner : PrincipalId
  editors : List PrincipalId
  writers : List PrincipalId
  nextId : Nat

def _root_.Strata.Tenant.header (t : Tenant) : Header :=
  { schema := t.schema, owner := t.owner, editors := t.editors, writers := t.writers,
    nextId := t.nextId }

/-- A header as a tenant with no records, so the model's permission functions apply to it. They
    read no record of the tenant. -/
def Header.asTenant (h : Header) : Tenant :=
  { schema := h.schema, owner := h.owner, editors := h.editors, writers := h.writers,
    records := [], nextId := h.nextId }

/-! ## Keys, values and the store -/

/-- What can be resident. -/
inductive Key where
  | header (t : TenantId)
  /-- One record, by id. Its value says whether the record exists: absence is a fact too. -/
  | record (t : TenantId) (id : RecordId)
  /-- Every record of the tenant. The only key that answers a question about all records. -/
  | all (t : TenantId)
  deriving DecidableEq, Repr

def Key.tenant : Key → TenantId
  | .header t => t
  | .record t _ => t
  | .all t => t

/-- The value under a key. `none` inside `header` and `all` means the tenant does not exist. -/
inductive Payload where
  | header (h : Option Header)
  | record (r : Option Record)
  | all (rs : Option (List (RecordId × Record)))

/-- The authority's answer for a key: what the store would load. -/
def fetch (w : World) : Key → Payload
  | .header t => .header ((w t).map Tenant.header)
  | .record t id => .record ((w t).bind (fun tn => tn.records.lookup id))
  | .all t => .all ((w t).map Tenant.records)

/-- The store: the committed world and the revision of each tenant. -/
structure Store where
  world : World
  rev : TenantId → Nat

/-! ## Deltas and decisions from partial state -/

/-- What an accepted intent changes, as the Rust kernel's `StateDelta` says it: the operations,
    not the resulting tenant. -/
inductive Delta where
  | create (id : RecordId) (r : Record)
  | replace (id : RecordId) (r : Record)
  | remove (id : RecordId)
  | schema (s : Schema)

def Delta.apply (t : Tenant) : Delta → Tenant
  | .create id r => { t with records := (id, r) :: t.records, nextId := id + 1 }
  | .replace id r => { t with records := t.records.map (fun q => if q.1 == id then (q.1, r) else q) }
  | .remove id => { t with records := t.records.filter (fun q => q.1 != id) }
  | .schema s => { t with schema := s }

/-- The kernel's decision for one tenant, from its header, a lookup of single records and the
    record list. Each intent reads only what `needs` would name: create reads the header; update
    and delete one record; a strict schema change the whole list. -/
def decideTenant (h : Header) (look : RecordId → Option Record) (all : List (RecordId × Record))
    (p : PrincipalId) : Intent → Except Err Delta
  | .create fs =>
    if !canCreate h.asTenant p then .error .unauthorized
    else
      let r : Record := { owner := p, fields := normalise fs }
      if conforms h.schema r then .ok (.create h.nextId r) else .error .invalid
  | .update id fs =>
    match look id with
    | none => .error (missingRecord h.asTenant p)
    | some old =>
      if !canEdit h.asTenant p old then .error .unauthorized
      else
        let r : Record := { owner := old.owner, fields := normalise fs }
        if conforms h.schema r then .ok (.replace id r) else .error .invalid
  | .delete id =>
    match look id with
    | none => .error (missingRecord h.asTenant p)
    | some old =>
      if !canEdit h.asTenant p old then .error .unauthorized
      else .ok (.remove id)
  | .setRequired f strict =>
    if p != h.owner then .error .unauthorized
    else
      let schema' := makeRequired h.schema f
      if strict && !(all.all (fun q => conforms schema' q.2)) then .error .migrationRejected
      else .ok (.schema schema')

/-- A decision made through any source of key values: the working set, or the store itself. -/
def decideOn (get : Key → Payload) (x : TenantId) (p : PrincipalId) (i : Intent) :
    Except Err Delta :=
  match get (.header x) with
  | .header (some h) =>
    let look := fun id => match get (.record x id) with
      | .record r => r
      | _ => none
    let all := match get (.all x) with
      | .all (some rs) => rs
      | _ => []
    decideTenant h look all p i
  | _ => .error .noTenant

/-- The world after a delta to one tenant. -/
def applyWorld (w : World) (x : TenantId) (d : Delta) : World :=
  fun y => if y = x then (w x).map (fun t => d.apply t) else w y

def Store.applyDelta (s : Store) (x : TenantId) (d : Delta) : Store :=
  { world := applyWorld s.world x d,
    rev := fun y => if y = x then s.rev y + 1 else s.rev y }

/-! ### Coverage soundness in the model -/

private theorem except_map_bind {α β ε : Type} (x : Except ε α) (f : α → β) :
    x.map f = (match x with | .ok a => .ok (f a) | .error e => .error e) := by
  cases x <;> rfl

@[simp] private theorem header_schema (t : Tenant) : t.header.schema = t.schema := rfl
@[simp] private theorem header_owner (t : Tenant) : t.header.owner = t.owner := rfl
@[simp] private theorem header_nextId (t : Tenant) : t.header.nextId = t.nextId := rfl
@[simp] private theorem canCreate_asTenant (t : Tenant) (p : PrincipalId) :
    canCreate t.header.asTenant p = canCreate t p := rfl
@[simp] private theorem canEdit_asTenant (t : Tenant) (p : PrincipalId) (r : Record) :
    canEdit t.header.asTenant p r = canEdit t p r := rfl
@[simp] private theorem missingRecord_asTenant (t : Tenant) (p : PrincipalId) :
    missingRecord t.header.asTenant p = missingRecord t p := rfl

/-- A decision from the header, the tenant's own lookup and its own record list is the kernel's
    step: the step depends on nothing else, and the delta reproduces the step's tenant. -/
theorem decideTenant_faithful (t : Tenant) (p : PrincipalId) (i : Intent) :
    (decideTenant t.header (fun id => t.records.lookup id) t.records p i).map (Delta.apply t)
      = stepTenant t p i := by
  cases i with
  | create fs =>
    simp only [decideTenant, stepTenant, canCreate_asTenant, header_schema, header_nextId]
    by_cases hc : canCreate t p = true
    · by_cases hk : conforms t.schema { owner := p, fields := normalise fs } = true
      · simp [hc, hk, Delta.apply, Except.map]
      · simp [hc, hk, Except.map]
    · simp [hc, Except.map]
  | update id fs =>
    simp only [decideTenant, stepTenant, canEdit_asTenant, missingRecord_asTenant, header_schema]
    cases hl : t.records.lookup id with
    | none => simp [Except.map]
    | some old =>
      by_cases hc : canEdit t p old = true
      · by_cases hk : conforms t.schema { owner := old.owner, fields := normalise fs } = true
        · simp [hc, hk, Delta.apply, Except.map]
        · simp [hc, hk, Except.map]
      · simp [hc, Except.map]
  | delete id =>
    simp only [decideTenant, stepTenant, canEdit_asTenant, missingRecord_asTenant]
    cases hl : t.records.lookup id with
    | none => simp [Except.map]
    | some old =>
      by_cases hc : canEdit t p old = true
      · simp [hc, Delta.apply, Except.map]
      · simp [hc, Except.map]
  | setRequired f strict =>
    simp only [decideTenant, stepTenant, header_owner, header_schema]
    by_cases ho : p = t.owner
    · by_cases hs : (strict && !(t.records.all fun q => conforms (makeRequired t.schema f) q.2)) = true
      · simp [ho, hs, Except.map]
      · simp [ho, hs, Delta.apply, Except.map]
    · simp [ho, Except.map]

/-- A decision made through the store's answers is the kernel's step on the store's world. -/
theorem decideOn_fetch (w : World) (i : Intent) (c : Ctx) :
    (decideOn (fetch w) c.tenant c.principal i).map (applyWorld w c.tenant) = step w i c := by
  unfold decideOn step
  cases hw : w c.tenant with
  | none => simp [fetch, hw, Except.map]
  | some t =>
    simp only [fetch, hw, Option.map_some, Option.bind_some]
    have h := decideTenant_faithful t c.principal i
    rw [except_map_bind] at h
    rw [except_map_bind, ← h]
    cases hd : decideTenant t.header (fun id => t.records.lookup id) t.records c.principal i with
    | error e => rfl
    | ok d =>
      simp only []
      congr 1
      funext y
      simp [applyWorld, hw]

/-! ### Invalidation completeness -/

/-- Whether a delta to tenant `x` may change the value under a key. A create moves the counter
    (header), adds a record (its id) and grows the list; a replacement or removal changes one
    record and the list; a schema change changes the header only. -/
def touchedIn : Delta → Key → Bool
  | .create _ _, .header _ => true
  | .schema _, .header _ => true
  | _, .header _ => false
  | .create id' _, .record _ id => id == id'
  | .replace id' _, .record _ id => id == id'
  | .remove id', .record _ id => id == id'
  | .schema _, .record _ _ => false
  | .schema _, .all _ => false
  | _, .all _ => true

def touched (x : TenantId) (d : Delta) (k : Key) : Bool :=
  k.tenant == x && touchedIn d k

private theorem beq_false_of_ne {a b : Nat} (h : a ≠ b) : (a == b) = false := by simp [h]

private theorem lookup_cons_ne (rs : List (RecordId × Record)) (id id' : RecordId) (r : Record)
    (h : id ≠ id') : ((id', r) :: rs).lookup id = rs.lookup id := by
  simp [List.lookup, beq_false_of_ne h]

private theorem lookup_map_replace (rs : List (RecordId × Record)) (id id' : RecordId)
    (r : Record) (h : id ≠ id') :
    (rs.map (fun q => if q.1 = id' then (q.1, r) else q)).lookup id = rs.lookup id := by
  induction rs with
  | nil => rfl
  | cons q rs ih =>
    obtain ⟨k, v⟩ := q
    by_cases hk : k = id'
    · subst hk
      simp [List.lookup, beq_false_of_ne h, ih]
    · by_cases hik : id = k
      · subst hik
        simp [List.lookup, hk]
      · simp [List.lookup, hk, beq_false_of_ne hik, ih]

private theorem lookup_filter_remove (rs : List (RecordId × Record)) (id id' : RecordId)
    (h : id ≠ id') : (rs.filter (fun q => q.1 != id')).lookup id = rs.lookup id := by
  induction rs with
  | nil => rfl
  | cons q rs ih =>
    obtain ⟨k, v⟩ := q
    by_cases hk : k = id'
    · subst hk
      simp [List.lookup, List.filter, beq_false_of_ne h, ih]
    · have h3 : (k != id') = true := by simp [hk]
      by_cases hik : id = k
      · subst hik
        simp [List.lookup, List.filter, h3]
      · simp [List.lookup, List.filter, h3, beq_false_of_ne hik, ih]

/-- The delta names every key it changes: the value under a key it does not touch is the same
    before and after. This is what lets a commit keep the rest of the working set. -/
theorem untouched_unchanged (w : World) (x : TenantId) (d : Delta) (k : Key)
    (h : touched x d k = false) : fetch (applyWorld w x d) k = fetch w k := by
  by_cases hx : k.tenant = x
  · have hin : touchedIn d k = false := by simpa [touched, hx] using h
    cases k with
    | header t =>
      simp only [Key.tenant] at hx
      subst hx
      cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
        (cases w t <;> simp [Delta.apply, Tenant.header])
    | record t id =>
      simp only [Key.tenant] at hx
      subst hx
      cases d with
      | create id' r =>
        have hne : id ≠ id' := by simpa [touchedIn] using hin
        simp only [fetch, applyWorld]
        cases w t <;> simp [Delta.apply, lookup_cons_ne _ _ _ _ hne]
      | replace id' r =>
        have hne : id ≠ id' := by simpa [touchedIn] using hin
        simp only [fetch, applyWorld]
        cases w t <;> simp [Delta.apply, lookup_map_replace _ _ _ _ hne]
      | remove id' =>
        have hne : id ≠ id' := by simpa [touchedIn] using hin
        simp only [fetch, applyWorld]
        cases w t <;> simp [Delta.apply, lookup_filter_remove _ _ _ hne]
      | schema s =>
        simp only [fetch, applyWorld]
        cases w t <;> simp [Delta.apply]
    | all t =>
      simp only [Key.tenant] at hx
      subst hx
      cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
        (cases w t <;> simp [Delta.apply])
  · cases k with
    | header t => simp only [Key.tenant] at hx; simp [fetch, applyWorld, hx]
    | record t id => simp only [Key.tenant] at hx; simp [fetch, applyWorld, hx]
    | all t => simp only [Key.tenant] at hx; simp [fetch, applyWorld, hx]

/-! ## The working set -/

structure Entry where
  key : Key
  /-- The tenant revision at which the value was read from the store or last confirmed. -/
  stamp : Nat
  val : Payload
  /-- Structural: never chosen for eviction, re-admitted by the commit that changes it. -/
  pinned : Bool

structure Cache where
  entries : List Entry
  budget : Nat

section Bytes

variable (size : Key → Payload → Nat)

def bytes (es : List Entry) : Nat := (es.map (fun e => size e.key e.val)).sum

/-- The budget invariant. -/
def Within (c : Cache) : Prop := bytes size c.entries ≤ c.budget

end Bytes

/-- The coherence invariant against the store: no entry is stamped with a revision the tenant has
    not reached, and an entry stamped with the tenant's current revision holds the store's value. -/
def Sound (es : List Entry) (s : Store) : Prop :=
  ∀ e ∈ es, e.stamp ≤ s.rev e.key.tenant ∧
    (e.stamp = s.rev e.key.tenant → e.val = fetch s.world e.key)

/-- The entry a read may be served from: same key, stamped with the tenant's current revision. -/
def hit (es : List Entry) (s : Store) (k : Key) : Option Entry :=
  es.find? (fun e => e.key == k && e.stamp == s.rev k.tenant)

/-- What a read returns: the resident entry when it is current, the store's answer otherwise. -/
def serve (es : List Entry) (s : Store) (k : Key) : Payload :=
  match hit es s k with
  | some e => e.val
  | none => fetch s.world k

/-! ### Coherence -/

/-- No entry older than the tenant's last committed revision is served. -/
theorem hit_current (es : List Entry) (s : Store) (k : Key) (e : Entry)
    (h : hit es s k = some e) : e ∈ es ∧ e.key = k ∧ e.stamp = s.rev k.tenant := by
  unfold hit at h
  have hp := List.find?_some h
  have hm := List.mem_of_find?_eq_some h
  simp only [Bool.and_eq_true, beq_iff_eq] at hp
  exact ⟨hm, hp.1, hp.2⟩

/-- Every read through a sound working set returns the store's current value. -/
theorem serve_eq_fetch (es : List Entry) (s : Store) (k : Key) (hs : Sound es s) :
    serve es s k = fetch s.world k := by
  unfold serve
  cases h : hit es s k with
  | none => rfl
  | some e =>
    obtain ⟨hm, hk, hst⟩ := hit_current es s k e h
    subst hk
    exact (hs e hm).2 hst

theorem sound_sub (es es' : List Entry) (s : Store) (hs : Sound es s)
    (hsub : ∀ e ∈ es', e ∈ es) : Sound es' s :=
  fun e he => hs e (hsub e he)

theorem fetch_congr (w w' : World) (k : Key) (h : w k.tenant = w' k.tenant) :
    fetch w k = fetch w' k := by
  cases k <;> simp only [Key.tenant] at h <;> simp [fetch, h]

/-- **Coherence without a single writer.** If the store moves on behind the working set (another
    process commits), the working set stays sound provided every tenant whose state changed has a
    higher revision: its entries simply stop being served. What this costs is that the runtime
    must read the tenant's current revision before serving, which is the header read every load
    already makes. -/
theorem sound_of_advance (es : List Entry) (s s' : Store) (hs : Sound es s)
    (hadv : ∀ y, (s'.rev y = s.rev y ∧ s'.world y = s.world y) ∨ s.rev y < s'.rev y) :
    Sound es s' := by
  intro e he
  obtain ⟨hle, hcur⟩ := hs e he
  rcases hadv e.key.tenant with ⟨hr, hw⟩ | hlt
  · refine ⟨by rw [hr]; exact hle, fun h => ?_⟩
    rw [fetch_congr s'.world s.world e.key hw]
    exact hcur (by rw [← hr]; exact h)
  · exact ⟨by omega, fun h => by omega⟩

/-! ### Policies and operations -/

/-- An eviction and admission policy, with a state of its own. The runtime takes its advice and
    enforces the budget, pinning and stamps itself, so nothing a policy says can break a theorem. -/
structure Policy (σ : Type) where
  /-- Whether an unpinned entry of this many bytes is worth admitting at all (TinyLFU's frequency
      test, a value-per-byte threshold). -/
  admits : σ → List Entry → Key → Nat → Bool
  /-- Which keys to give up to free at least this many bytes. -/
  victims : σ → List Entry → Nat → List Key
  /-- The policy's bookkeeping after a read: hit or miss. -/
  touch : σ → Key → Bool → σ

variable {σ : Type} (pol : Policy σ) (size : Key → Payload → Nat)

/-- Drop the named keys, except pinned entries. -/
def dropKeys (es : List Entry) (ks : List Key) : List Entry :=
  es.filter (fun e => e.pinned || !(ks.contains e.key))

def removeKey (es : List Entry) (k : Key) : List Entry :=
  es.filter (fun e => e.key != k)

/-- What is left after the policy's victims for an entry of `n` bytes under key `k` are given up
    (never a pinned entry), with any older entry of `k` removed. -/
def base (st : σ) (c : Cache) (k : Key) (n : Nat) : List Entry :=
  removeKey (dropKeys c.entries (pol.victims st c.entries (bytes size c.entries + n - c.budget))) k

/-- The room an admission makes: the policy's choice, and for a pinned entry that still does not
    fit, every unpinned entry as well. -/
def room (st : σ) (c : Cache) (k : Key) (n : Nat) (pin : Bool) : List Entry :=
  if pin && c.budget < bytes size (base pol size st c k n) + n then
    (base pol size st c k n).filter (·.pinned)
  else base pol size st c k n

/-- Admit a value read from the store at `stamp`. An unpinned entry the policy does not want is not
    admitted; otherwise room is made, and if the entry still does not fit nothing changes. -/
def admit (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) (pin : Bool) : Cache :=
  if !pin && !pol.admits st c.entries k (size k v) then c
  else if bytes size (room pol size st c k (size k v) pin) + size k v ≤ c.budget then
    { c with entries := ⟨k, stamp, v, pin⟩ :: room pol size st c k (size k v) pin }
  else c

/-- The entries left after the policy gives up enough to come down to `b` bytes. -/
def shrink (st : σ) (c : Cache) (b : Nat) : List Entry :=
  if b < bytes size c.entries then
    dropKeys c.entries (pol.victims st c.entries (bytes size c.entries - b))
  else c.entries

/-- Every unpinned entry goes when the policy's choice was not enough. -/
def trimTo (es : List Entry) (b : Nat) : List Entry :=
  if b < bytes size es then es.filter (·.pinned) else es

/-- Change the budget (the machine's arbiter moving memory between tenants). The working set never
    goes below its pinned bytes: the budget becomes the larger of the request and what is left. -/
def resize (st : σ) (c : Cache) (b : Nat) : Cache :=
  { entries := trimTo size (shrink pol size st c b) b,
    budget := max b (bytes size (trimTo size (shrink pol size st c b) b)) }

/-- The working set after a commit of `d` to tenant `x`, which was at revision `old`: entries the
    delta touches are dropped, current entries of `x` it does not touch are confirmed at `old + 1`,
    and everything else is left as it is. -/
def carry (x : TenantId) (d : Delta) (old : Nat) (es : List Entry) : List Entry :=
  es.filterMap fun e =>
    if touched x d e.key then none
    else if e.key.tenant = x ∧ e.stamp = old then some { e with stamp := old + 1 }
    else some e

/-- Whether the header of `x` is pinned and resident. -/
def headerPinned (es : List Entry) (x : TenantId) : Bool :=
  es.any (fun e => e.key == .header x && e.pinned)

inductive Op where
  | read (k : Key)
  /-- Make a key structural: admitted pinned (a tenant's header, an index, a reference closure). -/
  | pin (k : Key)
  /-- Memory pressure: the policy names keys to give up. -/
  | evict (ks : List Key)
  /-- An explicit invalidation (an erasure destroying a key, a restored snapshot). -/
  | invalidate (k : Key)
  | resize (b : Nat)
  | commit (i : Intent) (c : Ctx)

inductive Outcome where
  | value (p : Payload)
  | result (r : Except Err Unit)
  | done

structure Sys (σ : Type) where
  cache : Cache
  store : Store
  pst : σ

/-- The commit path through the working set: decide on what the working set serves, commit the
    delta to the store, carry the working set forward, and re-admit a pinned header the delta
    changed. -/
def commitWS (sys : Sys σ) (i : Intent) (c : Ctx) : Sys σ × Outcome :=
  let es := sys.cache.entries
  let s := sys.store
  match decideOn (serve es s) c.tenant c.principal i with
  | .error e => (sys, .result (.error e))
  | .ok d =>
    let s' := s.applyDelta c.tenant d
    let c1 : Cache := { sys.cache with entries := carry c.tenant d (s.rev c.tenant) es }
    let c2 := if headerPinned es c.tenant && touched c.tenant d (.header c.tenant) then
        admit pol size sys.pst c1 (.header c.tenant) (s'.rev c.tenant)
          (fetch s'.world (.header c.tenant)) true
      else c1
    ({ sys with cache := c2, store := s' }, .result (.ok ()))

def stepWS (sys : Sys σ) : Op → Sys σ × Outcome
  | .read k =>
    match hit sys.cache.entries sys.store k with
    | some e => ({ sys with pst := pol.touch sys.pst k true }, .value e.val)
    | none =>
      let v := fetch sys.store.world k
      ({ sys with
          cache := admit pol size sys.pst sys.cache k (sys.store.rev k.tenant) v false,
          pst := pol.touch sys.pst k false }, .value v)
  | .pin k =>
    ({ sys with
        cache := admit pol size sys.pst sys.cache k (sys.store.rev k.tenant)
          (fetch sys.store.world k) true },
      .done)
  | .evict ks => ({ sys with cache := { sys.cache with entries := dropKeys sys.cache.entries ks } },
      .done)
  | .invalidate k =>
    ({ sys with cache := { sys.cache with entries := removeKey sys.cache.entries k } }, .done)
  | .resize b => ({ sys with cache := resize pol size sys.pst sys.cache b }, .done)
  | .commit i c => commitWS pol size sys i c

def runWS (sys : Sys σ) : List Op → Sys σ × List Outcome
  | [] => (sys, [])
  | op :: ops =>
    let (sys', o) := stepWS pol size sys op
    let (sys'', os) := runWS sys' ops
    (sys'', o :: os)

/-- The same schedule with no working set: reads go to the world, commits are the kernel's step,
    and everything about memory does nothing. -/
def directStep (w : World) : Op → World × Outcome
  | .read k => (w, .value (fetch w k))
  | .commit i c =>
    match step w i c with
    | .ok w' => (w', .result (.ok ()))
    | .error e => (w, .result (.error e))
  | _ => (w, .done)

def directRun (w : World) : List Op → World × List Outcome
  | [] => (w, [])
  | op :: ops =>
    let (w', o) := directStep w op
    let (w'', os) := directRun w' ops
    (w'', o :: os)

/-! ### Lemmas about the list operations -/

theorem dropKeys_sub (es : List Entry) (ks : List Key) : ∀ e ∈ dropKeys es ks, e ∈ es :=
  fun _ he => (List.mem_filter.mp he).1

theorem removeKey_sub (es : List Entry) (k : Key) : ∀ e ∈ removeKey es k, e ∈ es :=
  fun _ he => (List.mem_filter.mp he).1

theorem bytes_filter_le (es : List Entry) (p : Entry → Bool) :
    bytes size (es.filter p) ≤ bytes size es := by
  induction es with
  | nil => simp [bytes]
  | cons e es ih =>
    unfold bytes at *
    by_cases hp : p e = true
    · simp [List.filter, hp]
      omega
    · simp [List.filter, hp]
      omega

theorem bytes_carry_le (x : TenantId) (d : Delta) (old : Nat) (es : List Entry) :
    bytes size (carry x d old es) ≤ bytes size es := by
  induction es with
  | nil => simp [carry, bytes]
  | cons e es ih =>
    unfold carry bytes at *
    simp only [List.filterMap_cons]
    by_cases ht : touched x d e.key = true
    · simp only [ht, ite_true]
      simp only [List.map_cons, List.sum_cons]
      omega
    · simp only [ht]
      by_cases hc : e.key.tenant = x ∧ e.stamp = old
      · simp only [hc, and_self, ite_true, Bool.false_eq_true, ite_false]
        simp only [List.map_cons, List.sum_cons]
        omega
      · simp only [hc, ite_false, Bool.false_eq_true]
        simp only [List.map_cons, List.sum_cons]
        omega

/-! ### Budget -/

theorem base_sub (st : σ) (c : Cache) (k : Key) (n : Nat) :
    ∀ e ∈ base pol size st c k n, e ∈ c.entries :=
  fun e he => dropKeys_sub _ _ e (removeKey_sub _ _ e he)

theorem room_sub (st : σ) (c : Cache) (k : Key) (n : Nat) (pin : Bool) :
    ∀ e ∈ room pol size st c k n pin, e ∈ c.entries := by
  intro e he
  unfold room at he
  split at he
  · exact base_sub pol size _ _ _ _ e (List.mem_filter.mp he).1
  · exact base_sub pol size _ _ _ _ e he

theorem admit_within (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) (pin : Bool)
    (hw : Within size c) : Within size (admit pol size st c k stamp v pin) := by
  unfold admit
  split
  · exact hw
  · split
    · rename_i hfit
      simp only [Within, bytes, List.map_cons, List.sum_cons] at hfit ⊢
      omega
    · exact hw

theorem resize_within (st : σ) (c : Cache) (b : Nat) :
    Within size (resize pol size st c b) := by
  simp only [Within, resize]
  exact Nat.le_max_right _ _

theorem admit_sub (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) (pin : Bool) :
    ∀ e ∈ (admit pol size st c k stamp v pin).entries, e ∈ c.entries ∨ e = ⟨k, stamp, v, pin⟩ := by
  intro e he
  unfold admit at he
  split at he
  · exact Or.inl he
  · split at he
    · rcases List.mem_cons.mp he with h | h
      · exact Or.inr h
      · exact Or.inl (room_sub pol size _ _ _ _ _ e h)
    · exact Or.inl he

theorem shrink_sub (st : σ) (c : Cache) (b : Nat) :
    ∀ e ∈ shrink pol size st c b, e ∈ c.entries := by
  intro e he
  unfold shrink at he
  split at he
  · exact dropKeys_sub _ _ e he
  · exact he

theorem trimTo_sub (es : List Entry) (b : Nat) : ∀ e ∈ trimTo size es b, e ∈ es := by
  intro e he
  unfold trimTo at he
  split at he
  · exact (List.mem_filter.mp he).1
  · exact he

theorem resize_sub (st : σ) (c : Cache) (b : Nat) :
    ∀ e ∈ (resize pol size st c b).entries, e ∈ c.entries :=
  fun e he => shrink_sub pol size _ _ _ e (trimTo_sub size _ _ e he)

/-! ### Coherence across operations -/

theorem admit_sound (st : σ) (c : Cache) (s : Store) (k : Key) (pin : Bool)
    (hs : Sound c.entries s) :
    Sound (admit pol size st c k (s.rev k.tenant) (fetch s.world k) pin).entries s := by
  intro e he
  rcases admit_sub pol size st c k _ _ pin e he with h | h
  · exact hs e h
  · subst h
    exact ⟨Nat.le_refl _, fun _ => rfl⟩

theorem carry_sound (es : List Entry) (s : Store) (x : TenantId) (d : Delta)
    (hs : Sound es s) : Sound (carry x d (s.rev x) es) (s.applyDelta x d) := by
  intro e he
  unfold carry at he
  obtain ⟨e₀, he₀, hf⟩ := List.mem_filterMap.mp he
  obtain ⟨hle, hcur⟩ := hs e₀ he₀
  by_cases ht : touched x d e₀.key = true
  · simp [ht] at hf
  · have ht' : touched x d e₀.key = false := by simpa using ht
    have hun := untouched_unchanged s.world x d e₀.key ht'
    simp only [ht', Bool.false_eq_true, ite_false] at hf
    by_cases hc : e₀.key.tenant = x ∧ e₀.stamp = s.rev x
    · simp only [hc, and_self, ite_true, Option.some.injEq] at hf
      subst hf
      obtain ⟨hx, hst⟩ := hc
      refine ⟨?_, ?_⟩
      · simp [Store.applyDelta, hx]
      · intro _
        show e₀.val = fetch (applyWorld s.world x d) e₀.key
        rw [hun]
        exact hcur (by rw [hx]; exact hst)
    · simp only [hc, ite_false, Option.some.injEq] at hf
      subst hf
      by_cases hx : e₀.key.tenant = x
      · have hne : e₀.stamp ≠ s.rev x := fun h => hc ⟨hx, h⟩
        have hlt : e₀.stamp < s.rev x := by rw [hx] at hle; omega
        refine ⟨?_, ?_⟩
        · simp [Store.applyDelta, hx]; omega
        · intro h
          simp [Store.applyDelta, hx] at h
          omega
      · refine ⟨?_, ?_⟩
        · simp [Store.applyDelta, hx]; exact hle
        · intro h
          simp [Store.applyDelta, hx] at h
          show e₀.val = fetch (applyWorld s.world x d) e₀.key
          rw [hun]
          exact hcur h

/-- The working set stays sound across every operation, whatever the policy. -/
theorem stepWS_sound (sys : Sys σ) (op : Op) (hs : Sound sys.cache.entries sys.store) :
    Sound (stepWS pol size sys op).1.cache.entries (stepWS pol size sys op).1.store := by
  cases op with
  | read k =>
    simp only [stepWS]
    cases hit sys.cache.entries sys.store k with
    | some e => exact hs
    | none => exact admit_sound pol size _ _ _ _ _ hs
  | pin k => simp only [stepWS]; exact admit_sound pol size _ _ _ _ _ hs
  | evict ks => simp only [stepWS]; exact sound_sub _ _ _ hs (dropKeys_sub _ _)
  | invalidate k => simp only [stepWS]; exact sound_sub _ _ _ hs (removeKey_sub _ _)
  | resize b => simp only [stepWS]; exact sound_sub _ _ _ hs (resize_sub pol size _ _ _)
  | commit i c =>
    simp only [stepWS, commitWS]
    split
    · exact hs
    · rename_i d _
      have h1 := carry_sound _ _ c.tenant d hs
      split
      · exact admit_sound pol size _ _ (sys.store.applyDelta c.tenant d) _ _ h1
      · exact h1

/-- The budget holds across every operation, whatever the policy. -/
theorem stepWS_within (sys : Sys σ) (op : Op) (hw : Within size sys.cache) :
    Within size (stepWS pol size sys op).1.cache := by
  cases op with
  | read k =>
    simp only [stepWS]
    cases hit sys.cache.entries sys.store k with
    | some e => exact hw
    | none => exact admit_within pol size _ _ _ _ _ _ hw
  | pin k => simp only [stepWS]; exact admit_within pol size _ _ _ _ _ _ hw
  | evict ks =>
    simp only [stepWS]
    exact Nat.le_trans (bytes_filter_le size _ _) hw
  | invalidate k =>
    simp only [stepWS]
    exact Nat.le_trans (bytes_filter_le size _ _) hw
  | resize b => simp only [stepWS]; exact resize_within pol size _ _ _
  | commit i c =>
    simp only [stepWS, commitWS]
    split
    · exact hw
    · rename_i d _
      have h1 : Within size { sys.cache with
          entries := carry c.tenant d (sys.store.rev c.tenant) sys.cache.entries } :=
        Nat.le_trans (bytes_carry_le size _ _ _ _) hw
      split
      · exact admit_within pol size _ _ _ _ _ _ h1
      · exact h1

/-- After a commit through the working set, every read returns the committed value, and the
    committed world is the kernel's step. -/
theorem read_after_commit (sys : Sys σ) (i : Intent) (c : Ctx) (k : Key)
    (hs : Sound sys.cache.entries sys.store) :
    let r := stepWS pol size sys (.commit i c)
    serve r.1.cache.entries r.1.store k = fetch r.1.store.world k :=
  serve_eq_fetch _ _ _ (stepWS_sound pol size sys _ hs)

/-! ### Transparency -/

/-- One operation through the working set has the outcome and the committed world it has with no
    working set. -/
theorem stepWS_transparent (sys : Sys σ) (op : Op) (hs : Sound sys.cache.entries sys.store) :
    (stepWS pol size sys op).1.store.world = (directStep sys.store.world op).1 ∧
    (stepWS pol size sys op).2 = (directStep sys.store.world op).2 := by
  cases op with
  | read k =>
    simp only [stepWS, directStep]
    cases h : hit sys.cache.entries sys.store k with
    | none => exact ⟨rfl, rfl⟩
    | some e =>
      refine ⟨rfl, ?_⟩
      have := serve_eq_fetch _ _ k hs
      simp only [serve, h] at this
      simp [this]
  | pin k => exact ⟨rfl, rfl⟩
  | evict ks => exact ⟨rfl, rfl⟩
  | invalidate k => exact ⟨rfl, rfl⟩
  | resize b => exact ⟨rfl, rfl⟩
  | commit i c =>
    have hserve : serve sys.cache.entries sys.store = fetch sys.store.world :=
      funext (fun k => serve_eq_fetch _ _ k hs)
    have hstep := decideOn_fetch sys.store.world i c
    simp only [stepWS, commitWS, directStep, hserve]
    cases hd : decideOn (fetch sys.store.world) c.tenant c.principal i with
    | error e =>
      rw [hd] at hstep
      simp only [Except.map] at hstep
      rw [← hstep]
      exact ⟨rfl, rfl⟩
    | ok d =>
      rw [hd] at hstep
      simp only [Except.map] at hstep
      rw [← hstep]
      split
      · next e he => cases he
      · next d' hd' =>
        cases hd'
        exact ⟨rfl, rfl⟩

/-- **Transparency.** For every schedule of reads, pins, evictions, invalidations, resizes and
    commits, and every policy, the outcomes and the committed world are those of the same schedule
    with no working set. -/
theorem runWS_transparent (ops : List Op) :
    ∀ (sys : Sys σ), Sound sys.cache.entries sys.store →
      (runWS pol size sys ops).1.store.world = (directRun sys.store.world ops).1 ∧
      (runWS pol size sys ops).2 = (directRun sys.store.world ops).2 := by
  induction ops with
  | nil => intro sys _; exact ⟨rfl, rfl⟩
  | cons op ops ih =>
    intro sys hs
    obtain ⟨hw, ho⟩ := stepWS_transparent pol size sys op hs
    have hs' := stepWS_sound pol size sys op hs
    obtain ⟨ihw, iho⟩ := ih _ hs'
    simp only [runWS, directRun]
    rw [← hw]
    exact ⟨ihw, by rw [ho, iho]⟩

/-- **Budget**, for a whole schedule: every intermediate working set is within its budget. -/
theorem runWS_within (ops : List Op) :
    ∀ (sys : Sys σ), Within size sys.cache →
      Within size (runWS pol size sys ops).1.cache := by
  induction ops with
  | nil => intro sys hw; exact hw
  | cons op ops ih =>
    intro sys hw
    simp only [runWS]
    exact ih _ (stepWS_within pol size sys op hw)

/-- **Coherence**, for a whole schedule: the working set stays sound, so no read at any point is
    served from an entry older than the tenant's last commit. -/
theorem runWS_sound (ops : List Op) :
    ∀ (sys : Sys σ), Sound sys.cache.entries sys.store →
      Sound (runWS pol size sys ops).1.cache.entries (runWS pol size sys ops).1.store := by
  induction ops with
  | nil => intro sys hs; exact hs
  | cons op ops ih =>
    intro sys hs
    simp only [runWS]
    exact ih _ (stepWS_sound pol size sys op hs)

/-- An empty working set is sound and within any budget: the start of every schedule. -/
theorem empty_sound (s : Store) : Sound [] s := fun _ h => absurd h (List.not_mem_nil)

theorem empty_within (b : Nat) : Within size { entries := [], budget := b } := by
  simp [Within, bytes]

/-! ### Pinning -/

theorem admit_keeps_pinned (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload)
    (pin : Bool) (e : Entry) (he : e ∈ c.entries) (hp : e.pinned = true) (hk : e.key ≠ k) :
    e ∈ (admit pol size st c k stamp v pin).entries := by
  have h1 : e ∈ base pol size st c k (size k v) := by
    simp [base, removeKey, dropKeys, List.mem_filter, he, hp, hk]
  have h2 : e ∈ room pol size st c k (size k v) pin := by
    unfold room
    split
    · exact List.mem_filter.mpr ⟨h1, hp⟩
    · exact h1
  unfold admit
  split
  · exact he
  · split
    · exact List.mem_cons_of_mem _ h2
    · exact he

theorem evict_keeps_pinned (es : List Entry) (ks : List Key) (e : Entry) (he : e ∈ es)
    (hp : e.pinned = true) : e ∈ dropKeys es ks := by
  simp [dropKeys, List.mem_filter, he, hp]

theorem resize_keeps_pinned (st : σ) (c : Cache) (b : Nat) (e : Entry) (he : e ∈ c.entries)
    (hp : e.pinned = true) : e ∈ (resize pol size st c b).entries := by
  have h0 : e ∈ shrink pol size st c b := by
    unfold shrink
    split
    · exact evict_keeps_pinned _ _ e he hp
    · exact he
  show e ∈ trimTo size (shrink pol size st c b) b
  unfold trimTo
  split
  · exact List.mem_filter.mpr ⟨h0, hp⟩
  · exact h0

/-- Giving up keys never changes a selection of pinned entries. -/
private theorem filter_dropKeys_of_pinned (es : List Entry) (ks : List Key) (q : Entry → Bool)
    (hq : ∀ e, q e = true → e.pinned = true) : (dropKeys es ks).filter q = es.filter q := by
  unfold dropKeys
  rw [List.filter_filter]
  apply List.filter_congr
  intro e _
  by_cases h : q e = true
  · simp [h, hq e h]
  · simp [h]

/-- **A pinned entry is resident whenever the pinned reserve fits**, for any policy: if the pinned
    entries other than `k`, with the new value, fit in the budget, the pinned admission succeeds. -/
theorem pin_resident (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload)
    (hfit : bytes size ((c.entries.filter (·.pinned)).filter (fun e => e.key != k)) + size k v
      ≤ c.budget) :
    ⟨k, stamp, v, true⟩ ∈ (admit pol size st c k stamp v true).entries := by
  unfold admit
  rw [ite_eq_right (by simp)]
  have hpin : (base pol size st c k (size k v)).filter (·.pinned)
      = (c.entries.filter (·.pinned)).filter (fun e => e.key != k) := by
    unfold base removeKey
    rw [List.filter_filter, List.filter_filter,
      filter_dropKeys_of_pinned _ _ _ (fun e h => by simp at h; exact h.1)]
    apply List.filter_congr
    intro e _
    exact Bool.and_comm _ _
  have hroom : bytes size (room pol size st c k (size k v) true) + size k v ≤ c.budget := by
    unfold room
    by_cases hb : c.budget < bytes size (base pol size st c k (size k v)) + size k v
    · rw [ite_eq_left (by simp [hb]), hpin]
      exact hfit
    · rw [ite_eq_right (by simp [hb])]
      omega
  rw [ite_eq_left hroom]
  exact List.mem_cons_self

/-- A commit to one tenant leaves the entries of every other tenant exactly as they were. -/
theorem commit_keeps_other_tenants (x : TenantId) (d : Delta) (old : Nat) (es : List Entry)
    (e : Entry) (he : e ∈ es) (hx : e.key.tenant ≠ x) : e ∈ carry x d old es := by
  unfold carry
  apply List.mem_filterMap.mpr
  refine ⟨e, he, ?_⟩
  have ht : touched x d e.key = false := by simp [touched, hx]
  simp [ht, hx]

/-- A commit keeps every current entry it does not touch, confirmed at the new revision. -/
theorem commit_keeps_untouched (x : TenantId) (d : Delta) (old : Nat) (es : List Entry)
    (e : Entry) (he : e ∈ es) (hx : e.key.tenant = x) (hst : e.stamp = old)
    (ht : touched x d e.key = false) : { e with stamp := old + 1 } ∈ carry x d old es := by
  unfold carry
  apply List.mem_filterMap.mpr
  refine ⟨e, he, ?_⟩
  simp [ht, hx, hst]

/-- `commit_sound` restated for the record: the coherence invariant survives a commit. -/
theorem commit_sound (sys : Sys σ) (i : Intent) (c : Ctx)
    (hs : Sound sys.cache.entries sys.store) :
    Sound (stepWS pol size sys (.commit i c)).1.cache.entries
      (stepWS pol size sys (.commit i c)).1.store :=
  stepWS_sound pol size sys _ hs

/-- The arbiter can always take a tenant's working set down to its pinned bytes: a requested
    budget at or above them is the budget it gets. -/
theorem resize_budget_exact (st : σ) (c : Cache) (b : Nat)
    (hb : bytes size (c.entries.filter (·.pinned)) ≤ b) :
    (resize pol size st c b).budget = b := by
  simp only [resize]
  apply Nat.max_eq_left
  unfold trimTo
  split
  · have hs : (shrink pol size st c b).filter (·.pinned) = c.entries.filter (·.pinned) := by
      unfold shrink
      split
      · exact filter_dropKeys_of_pinned _ _ _ (fun _ h => h)
      · rfl
    rw [hs]
    exact hb
  · omega

/-! ### One machine, many working sets -/

/-- Working sets that each keep their own budget, under budgets that sum to at most the machine's
    memory, together use at most the machine's memory. -/
theorem machine_within (cs : List Cache) (m : Nat)
    (hw : ∀ c ∈ cs, Within size c) (hm : (cs.map (·.budget)).sum ≤ m) :
    (cs.map (fun c => bytes size c.entries)).sum ≤ m := by
  induction cs generalizing m with
  | nil => simp
  | cons c cs ih =>
    simp only [List.map_cons, List.sum_cons] at hm ⊢
    have hc := hw c List.mem_cons_self
    have := ih ((cs.map (·.budget)).sum) (fun c' h => hw c' (List.mem_cons_of_mem _ h)) (Nat.le_refl _)
    unfold Within at hc
    omega

end Strata.WorkingSet
