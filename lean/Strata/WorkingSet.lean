/-
The tenant working set: what a runtime holds in memory in front of the store, and exactly when
holding it changes no result.

A model of outcomes, not of an algorithm. The eviction and admission policy is a parameter
(`Policy`, with a state of any type), and so is the map from a delta to the keys it invalidates
(`inv`, any map satisfying `InvSound`). What the runtime enforces whatever the policy says is in
the operations: the budget check, the pin set, the generation stamp, the commit guard.

The store is the authority (`Store`): the committed world and a **generation** per tenant. The
generation is not the tenant revision. The revision can go back (a restored snapshot, a tenant
deleted and created again: `crates/storage-api/src/memory.rs:1573`, `crates/storage-sqlite/src/
lib.rs:4850`); the generation rises on every change of anything a load returns, whoever makes it
(`Store.set`, `Store.applyDelta`). That is the store's obligation, stated here as the only way the
model's store can change, and in the design as a contract method with a conformance test.

The runtime knows a generation per tenant (`Sys.known`), which may be behind the store's. It is
brought up to date by every miss (a load returns the generation), by a commit, by a fenced write
(one the runtime is told about) and by an explicit check. An **unfenced** write (another process,
a restore from outside the cell) is not seen until one of those happens.

Assumptions, all of them:

W1. The kernel is the model of `Strata/Model.lean`; nothing here refers to Rust.
W2. Operations are atomic and sequential. Interleaving is modelled by putting other writers'
    operations (`external`, `unfenced`) and lost receipts (`ambiguous`) between the runtime's.
W3. The size of an entry is a function of its key and value (`size`): accounted bytes, not
    measured memory.
W4. Every change to what a load returns raises the tenant's generation.

The theorems the design cites:

- Coverage soundness: `decideTenant_faithful`, `decideOn_fetch`, `decideOn_congr`.
- Invalidation: `InvSound` (the criterion: a key not invalidated has the same store answer after
  the delta), `touchedIn_sound` (one map meeting it, answers included).
- Coherence: `hit_stamp`, `serve_eq_fetch`, `sync_safe_self`, `commitWS_safe`,
  `unfenced_breaks_reads` (the counterexample that makes the check necessary).
- Transparency: `commitWS_transparent` (commits, in every schedule), `runWS_world` (the committed
  world, in every schedule), `runWS_transparent` (every outcome, in covered schedules).
- Budget: `stepWS_within`, `runWS_within`, `resize_budget_exact`, `resize_budget_le`,
  `machine_resize`.
- Pinning: `stepWS_pins`, `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned`,
  `pin_resident`.
- Erasure: `below_sealing_serves_current`, `above_sealing_serves_erased`.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.Model

namespace Strata.WorkingSet

open Strata

/-! ## Header and counters -/

/-- The structural part of a tenant: what changes only with a schema or membership change. The
    counters (the record counter here; the revision and event sequence in Rust) are a key of their
    own, because every commit changes them. -/
structure Header where
  schema : Schema
  owner : PrincipalId
  editors : List PrincipalId
  writers : List PrincipalId

def _root_.Strata.Tenant.header (t : Tenant) : Header :=
  { schema := t.schema, owner := t.owner, editors := t.editors, writers := t.writers }

/-- A header as a tenant with no records, so the model's permission functions apply to it. They
    read neither records nor the counter. -/
def Header.asTenant (h : Header) : Tenant :=
  { schema := h.schema, owner := h.owner, editors := h.editors, writers := h.writers,
    records := [], nextId := 0 }

/-! ## Keys, values and the store -/

inductive Key where
  | header (t : TenantId)
  | counters (t : TenantId)
  /-- One record, by id. Its value says whether the record exists: absence is a fact too. -/
  | record (t : TenantId) (id : RecordId)
  /-- Every record of the tenant. -/
  | all (t : TenantId)
  /-- The complete answer to a lookup by field value: the ids of the records with `f = v`. -/
  | answer (t : TenantId) (f : FieldId) (v : Val)
  deriving DecidableEq

def Key.tenant : Key → TenantId
  | .header t => t
  | .counters t => t
  | .record t _ => t
  | .all t => t
  | .answer t _ _ => t

/-- The value under a key. `none` inside means the tenant does not exist. -/
inductive Payload where
  | header (h : Option Header)
  | counters (n : Option Nat)
  | record (r : Option Record)
  | all (rs : Option (List (RecordId × Record)))
  | answer (ids : Option (List RecordId))

/-- The ids of the records whose field `f` is `v`. -/
def answerOf (t : Tenant) (f : FieldId) (v : Val) : List RecordId :=
  (t.records.filter (fun q => q.2.fields.get f == v)).map Prod.fst

/-- The authority's answer for a key: what the store would load. -/
def fetch (w : World) : Key → Payload
  | .header t => .header ((w t).map Tenant.header)
  | .counters t => .counters ((w t).map Tenant.nextId)
  | .record t id => .record ((w t).bind (fun tn => tn.records.lookup id))
  | .all t => .all ((w t).map Tenant.records)
  | .answer t f v => .answer ((w t).map (fun tn => answerOf tn f v))

theorem fetch_congr (w w' : World) (k : Key) (h : w k.tenant = w' k.tenant) :
    fetch w k = fetch w' k := by
  cases k <;> simp only [Key.tenant] at h <;> simp [fetch, h]

/-- A function with one point changed. -/
def upd {α : Type} (f : TenantId → α) (x : TenantId) (a : α) : TenantId → α :=
  fun y => if y = x then a else f y

@[simp] theorem upd_same {α : Type} (f : TenantId → α) (x : TenantId) (a : α) : upd f x a x = a := by
  simp [upd]

@[simp] theorem upd_other {α : Type} (f : TenantId → α) (x y : TenantId) (a : α) (h : y ≠ x) :
    upd f x a y = f y := by
  simp [upd, h]

theorem fetch_upd_other (w : World) (x : TenantId) (t : Option Tenant) (k : Key)
    (h : k.tenant ≠ x) : fetch (upd w x t) k = fetch w k :=
  fetch_congr _ _ k (upd_other w x k.tenant t h)

structure Store where
  world : World
  /-- Rises on every change to what a load of the tenant returns (W4). -/
  gen : TenantId → Nat

/-- Any change to one tenant made outside the runtime's commit path: a restored snapshot (from the
    store or from outside the cell), a delete and re-create, an anonymisation, a member removal. -/
def Store.set (s : Store) (x : TenantId) (t : Option Tenant) : Store :=
  { world := upd s.world x t, gen := upd s.gen x (s.gen x + 1) }

/-! ## Deltas and decisions from partial state -/

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

def applyWorld (w : World) (x : TenantId) (d : Delta) : World :=
  upd w x ((w x).map (fun t => d.apply t))

def Store.applyDelta (s : Store) (x : TenantId) (d : Delta) : Store :=
  { world := applyWorld s.world x d, gen := upd s.gen x (s.gen x + 1) }

/-- The kernel's decision for one tenant from its header, its counter, single records and the
    record list; each intent reads only what `needs` would name. -/
def decideTenant (h : Header) (n : Nat) (look : RecordId → Option Record)
    (all : List (RecordId × Record)) (p : PrincipalId) : Intent → Except Err Delta
  | .create fs =>
    if !canCreate h.asTenant p then .error .unauthorized
    else
      let r : Record := { owner := p, fields := normalise fs }
      if conforms h.schema r then .ok (.create n r) else .error .invalid
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

/-- A decision made through any source of key values. It reads keys of tenant `x` only. -/
def decideOn (get : Key → Payload) (x : TenantId) (p : PrincipalId) (i : Intent) :
    Except Err Delta :=
  match get (.header x), get (.counters x) with
  | .header (some h), .counters (some n) =>
    decideTenant h n
      (fun id => match get (.record x id) with
        | .record r => r
        | _ => none)
      (match get (.all x) with
        | .all (some rs) => rs
        | _ => [])
      p i
  | _, _ => .error .noTenant

theorem decideOn_congr (get get' : Key → Payload) (x : TenantId) (p : PrincipalId) (i : Intent)
    (h : ∀ k, k.tenant = x → get k = get' k) : decideOn get x p i = decideOn get' x p i := by
  have hh := h (.header x) rfl
  have hc := h (.counters x) rfl
  have ha := h (.all x) rfl
  have hr : ∀ id, get (.record x id) = get' (.record x id) := fun id => h _ rfl
  simp only [decideOn, hh, hc, ha, hr]

@[simp] private theorem header_schema (t : Tenant) : t.header.schema = t.schema := rfl
@[simp] private theorem header_owner (t : Tenant) : t.header.owner = t.owner := rfl
@[simp] private theorem canCreate_asTenant (t : Tenant) (p : PrincipalId) :
    canCreate t.header.asTenant p = canCreate t p := rfl
@[simp] private theorem canEdit_asTenant (t : Tenant) (p : PrincipalId) (r : Record) :
    canEdit t.header.asTenant p r = canEdit t p r := rfl
@[simp] private theorem missingRecord_asTenant (t : Tenant) (p : PrincipalId) :
    missingRecord t.header.asTenant p = missingRecord t p := rfl

private theorem except_map_match {α β ε : Type} (x : Except ε α) (f : α → β) :
    x.map f = (match x with | .ok a => .ok (f a) | .error e => .error e) := by
  cases x <;> rfl

/-- A decision from the header, the counter, the tenant's own lookup and record list, applied as
    a delta, is the kernel's step. -/
theorem decideTenant_faithful (t : Tenant) (p : PrincipalId) (i : Intent) :
    (decideTenant t.header t.nextId (fun id => t.records.lookup id) t.records p i).map
      (Delta.apply t) = stepTenant t p i := by
  cases i with
  | create fs =>
    simp only [decideTenant, stepTenant, canCreate_asTenant, header_schema]
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
    rw [except_map_match] at h
    rw [except_map_match, ← h]
    cases decideTenant t.header t.nextId (fun id => t.records.lookup id) t.records c.principal i with
    | error e => rfl
    | ok d =>
      simp only []
      congr 1
      funext y
      simp [applyWorld, upd, hw]

theorem decide_ok_step (w : World) (i : Intent) (c : Ctx) (d : Delta)
    (h : decideOn (fetch w) c.tenant c.principal i = .ok d) :
    step w i c = .ok (applyWorld w c.tenant d) := by
  rw [← decideOn_fetch, h]; rfl

theorem decide_error_step (w : World) (i : Intent) (c : Ctx) (e : Err)
    (h : decideOn (fetch w) c.tenant c.principal i = .error e) : step w i c = .error e := by
  rw [← decideOn_fetch, h]; rfl

/-! ## Invalidation -/

/-- **The criterion an invalidation map must meet.** A key of the committed tenant that the map
    does not invalidate has the same store answer after the delta, in every world. Any map meeting
    this is sound to carry the working set forward with; the theorems below take it as a parameter.
    Keys of other tenants are never changed by a delta (`fetch_applyWorld_other`). -/
def InvSound (inv : Delta → Key → Bool) : Prop :=
  ∀ (w : World) (x : TenantId) (d : Delta) (k : Key),
    k.tenant = x → inv d k = false → fetch (applyWorld w x d) k = fetch w k

theorem fetch_applyWorld_other (w : World) (x : TenantId) (d : Delta) (k : Key)
    (h : k.tenant ≠ x) : fetch (applyWorld w x d) k = fetch w k :=
  fetch_upd_other w x _ k h

/-- One map: a create moves the counter, adds a record and changes every answer and the list; a
    replacement or removal changes one record, every answer and the list; a schema change changes
    the header only. Answers are invalidated by any record change of their tenant, the granularity
    of the per-object-type change counters (D-5064). -/
def touchedIn : Delta → Key → Bool
  | .schema _, .header _ => true
  | _, .header _ => false
  | .create _ _, .counters _ => true
  | _, .counters _ => false
  | .create id' _, .record _ id => id == id'
  | .replace id' _, .record _ id => id == id'
  | .remove id', .record _ id => id == id'
  | .schema _, .record _ _ => false
  | .schema _, .all _ => false
  | _, .all _ => true
  | .schema _, .answer _ _ _ => false
  | _, .answer _ _ _ => true

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

theorem touchedIn_sound : InvSound touchedIn := by
  intro w x d k hx hin
  cases k with
  | header t =>
    simp only [Key.tenant] at hx; subst hx
    cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
      (cases w t <;> simp [Delta.apply, Tenant.header])
  | counters t =>
    simp only [Key.tenant] at hx; subst hx
    cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
      (cases w t <;> simp [Delta.apply])
  | record t id =>
    simp only [Key.tenant] at hx; subst hx
    cases d with
    | create id' r =>
      have hne : id ≠ id' := by simpa [touchedIn] using hin
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, lookup_cons_ne _ _ _ _ hne]
    | replace id' r =>
      have hne : id ≠ id' := by simpa [touchedIn] using hin
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, lookup_map_replace _ _ _ _ hne]
    | remove id' =>
      have hne : id ≠ id' := by simpa [touchedIn] using hin
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, lookup_filter_remove _ _ _ hne]
    | schema s =>
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply]
  | all t =>
    simp only [Key.tenant] at hx; subst hx
    cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
      (cases w t <;> simp [Delta.apply])
  | answer t f v =>
    simp only [Key.tenant] at hx; subst hx
    cases d <;> simp_all [touchedIn, fetch, applyWorld] <;>
      (cases w t <;> simp [Delta.apply, answerOf])

/-! ## The working set -/

structure Entry where
  key : Key
  /-- The tenant generation at which the value was read or last confirmed. -/
  stamp : Nat
  val : Payload

structure Cache where
  entries : List Entry
  /-- Structural keys: a property of the key, not of an entry, so a stale pinned entry replaced by
      a fresh one stays pinned. -/
  pins : List Key
  budget : Nat

section Bytes
variable (size : Key → Payload → Nat)

def bytes (es : List Entry) : Nat := (es.map (fun e => size e.key e.val)).sum

/-- The budget invariant, on accounted bytes. -/
def Within (c : Cache) : Prop := bytes size c.entries ≤ c.budget

def pinnedBytes (c : Cache) : Nat := bytes size (c.entries.filter (fun e => c.pins.contains e.key))
end Bytes

/-- Coherence against the store: no stamp is ahead of the tenant's generation, and an entry
    stamped with the current generation holds the store's value. Survives every operation. -/
def Sound (es : List Entry) (s : Store) : Prop :=
  ∀ e ∈ es, e.stamp ≤ s.gen e.key.tenant ∧
    (e.stamp = s.gen e.key.tenant → e.val = fetch s.world e.key)

/-- Safety of reads of tenant `y`: an entry the runtime would serve (stamped with what the runtime
    knows as `y`'s generation) holds the store's value. Broken only by an unfenced write to `y`. -/
def SafeAt (es : List Entry) (known : TenantId → Nat) (s : Store) (y : TenantId) : Prop :=
  ∀ e ∈ es, e.key.tenant = y → e.stamp = known y → e.val = fetch s.world e.key

def hit (es : List Entry) (known : TenantId → Nat) (k : Key) : Option Entry :=
  es.find? (fun e => e.key == k && e.stamp == known k.tenant)

/-- What a read returns: a resident entry stamped with the known generation, else the store. -/
def serve (es : List Entry) (known : TenantId → Nat) (s : Store) (k : Key) : Payload :=
  match hit es known k with
  | some e => e.val
  | none => fetch s.world k

theorem hit_stamp (es : List Entry) (known : TenantId → Nat) (k : Key) (e : Entry)
    (h : hit es known k = some e) : e ∈ es ∧ e.key = k ∧ e.stamp = known k.tenant := by
  unfold hit at h
  have hp := List.find?_some h
  have hm := List.mem_of_find?_eq_some h
  simp only [Bool.and_eq_true, beq_iff_eq] at hp
  exact ⟨hm, hp.1, hp.2⟩

theorem serve_eq_fetch (es : List Entry) (known : TenantId → Nat) (s : Store) (k : Key)
    (hs : SafeAt es known s k.tenant) : serve es known s k = fetch s.world k := by
  unfold serve
  cases h : hit es known k with
  | none => rfl
  | some e =>
    obtain ⟨hm, hk, hst⟩ := hit_stamp es known k e h
    subst hk
    exact hs e hm rfl hst

theorem sound_sub (es es' : List Entry) (s : Store) (hs : Sound es s)
    (hsub : ∀ e ∈ es', e ∈ es) : Sound es' s := fun e he => hs e (hsub e he)

theorem safe_sub (es es' : List Entry) (known : TenantId → Nat) (s : Store) (y : TenantId)
    (hs : SafeAt es known s y) (hsub : ∀ e ∈ es', e ∈ es) : SafeAt es' known s y :=
  fun e he => hs e (hsub e he)

theorem safe_of_current (es : List Entry) (known : TenantId → Nat) (s : Store) (y : TenantId)
    (hs : Sound es s) (hk : known y = s.gen y) : SafeAt es known s y := by
  intro e he hy hst
  subst hy
  exact (hs e he).2 (hst.trans hk)

theorem sound_set (es : List Entry) (s : Store) (x : TenantId) (t : Option Tenant)
    (hs : Sound es s) : Sound es (s.set x t) := by
  intro e he
  obtain ⟨hle, hcur⟩ := hs e he
  by_cases hx : e.key.tenant = x
  · simp only [Store.set, hx, upd_same]
    rw [hx] at hle
    exact ⟨by omega, fun h => by omega⟩
  · simp only [Store.set, upd_other _ _ _ _ hx]
    exact ⟨hle, fun h => by rw [fetch_upd_other _ _ _ _ hx]; exact hcur h⟩

theorem safe_set_other (es : List Entry) (known : TenantId → Nat) (s : Store) (x y : TenantId)
    (t : Option Tenant) (hs : SafeAt es known s y) (hy : y ≠ x) :
    SafeAt es known (s.set x t) y := by
  intro e he hey hst
  simp only [Store.set]
  rw [fetch_upd_other _ _ _ _ (by rw [hey]; exact hy)]
  exact hs e he hey hst

/-! ### Policies and the list operations -/

structure Policy (σ : Type) where
  /-- Whether an unpinned entry of this many bytes is worth admitting at all. -/
  admits : σ → List Entry → Key → Nat → Bool
  /-- Which keys to give up to free at least this many bytes. -/
  victims : σ → List Entry → Nat → List Key
  /-- The policy's bookkeeping after a read: hit or miss. -/
  touch : σ → Key → Bool → σ

variable {σ : Type} (pol : Policy σ) (size : Key → Payload → Nat) (inv : Delta → Key → Bool)

/-- Drop the named keys, except pinned ones. -/
def dropKeys (pins : List Key) (es : List Entry) (ks : List Key) : List Entry :=
  es.filter (fun e => pins.contains e.key || !(ks.contains e.key))

def removeKey (es : List Entry) (k : Key) : List Entry := es.filter (fun e => e.key != k)

def dropTenant (es : List Entry) (x : TenantId) : List Entry :=
  es.filter (fun e => e.key.tenant != x)

def base (st : σ) (c : Cache) (k : Key) (n : Nat) : List Entry :=
  removeKey (dropKeys c.pins c.entries (pol.victims st c.entries (bytes size c.entries + n - c.budget))) k

def room (st : σ) (c : Cache) (k : Key) (n : Nat) : List Entry :=
  if c.pins.contains k && c.budget < bytes size (base pol size st c k n) + n then
    (base pol size st c k n).filter (fun e => c.pins.contains e.key)
  else base pol size st c k n

/-- Admit a value read from the store at generation `stamp`. Pinned keys skip the admission test
    and may displace every unpinned entry; if the entry does not fit, nothing changes. -/
def admit (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) : Cache :=
  if !c.pins.contains k && !pol.admits st c.entries k (size k v) then c
  else if bytes size (room pol size st c k (size k v)) + size k v ≤ c.budget then
    { c with entries := ⟨k, stamp, v⟩ :: room pol size st c k (size k v) }
  else c

def shrink (st : σ) (c : Cache) (b : Nat) : List Entry :=
  if b < bytes size c.entries then
    dropKeys c.pins c.entries (pol.victims st c.entries (bytes size c.entries - b))
  else c.entries

def trimTo (pins : List Key) (es : List Entry) (b : Nat) : List Entry :=
  if b < bytes size es then es.filter (fun e => pins.contains e.key) else es

/-- Change the budget. It never goes below the pinned bytes; the arbiter grants at least those. -/
def resize (st : σ) (c : Cache) (b : Nat) : Cache :=
  { c with entries := trimTo size c.pins (shrink pol size st c b) b,
           budget := max b (bytes size (trimTo size c.pins (shrink pol size st c b) b)) }

/-- The working set after a commit of `d` to tenant `x` at generation `old`: invalidated entries go,
    current entries of `x` that stay are confirmed at `old + 1`, everything else is left alone. -/
def carry (x : TenantId) (d : Delta) (old : Nat) (es : List Entry) : List Entry :=
  es.filterMap fun e =>
    if e.key.tenant = x then
      if inv d e.key then none
      else if e.stamp = old then some { e with stamp := old + 1 } else some e
    else some e

/-! ### The system and its operations -/

structure Sys (σ : Type) where
  cache : Cache
  store : Store
  /-- The generation of each tenant as the runtime last learned it. -/
  known : TenantId → Nat
  pst : σ

/-- Forget what the runtime holds for `x` and learn `x`'s generation. -/
def refresh (sys : Sys σ) (x : TenantId) : Sys σ :=
  { sys with cache := { sys.cache with entries := dropTenant sys.cache.entries x },
             known := upd sys.known x (sys.store.gen x) }

/-- Make the runtime current for `x`: nothing when it already is, a refresh when not. -/
def sync (sys : Sys σ) (x : TenantId) : Sys σ :=
  if sys.known x = sys.store.gen x then sys else refresh sys x

inductive Op where
  | read (k : Key)
  | pin (k : Key)
  | unpin (k : Key)
  | evict (ks : List Key)
  | invalidate (k : Key)
  | resize (b : Nat)
  | commit (i : Intent) (c : Ctx)
  /-- A commit the store applied whose receipt the runtime never got. -/
  | ambiguous (i : Intent) (c : Ctx)
  /-- A write the runtime is told about (a restore through the control plane, an anonymisation in
      this process). -/
  | external (x : TenantId) (t : Option Tenant)
  /-- A write the runtime is not told about. -/
  | unfenced (x : TenantId) (t : Option Tenant)
  /-- A cheap read of the tenant's generation. -/
  | check (x : TenantId)

inductive Outcome where
  | value (p : Payload)
  | result (r : Except Err Unit)
  | unknown
  | done

/-- The commit path: become current for the tenant if the runtime is not (the conflict the store's
    guard would report, answered by one refresh), decide on what the working set serves, commit,
    carry the working set forward, re-admit a pinned header the delta invalidated. -/
def commitWS (sys : Sys σ) (i : Intent) (c : Ctx) : Sys σ × Outcome :=
  match decideOn (serve (sync sys c.tenant).cache.entries (sync sys c.tenant).known
      (sync sys c.tenant).store) c.tenant c.principal i with
  | .error e => (sync sys c.tenant, .result (.error e))
  | .ok d =>
    ({ sync sys c.tenant with
        cache :=
          if (sync sys c.tenant).cache.pins.contains (.header c.tenant) && inv d (.header c.tenant)
          then admit pol size (sync sys c.tenant).pst
            { (sync sys c.tenant).cache with
              entries := carry inv c.tenant d (sys.store.gen c.tenant) (sync sys c.tenant).cache.entries }
            (.header c.tenant) (sys.store.gen c.tenant + 1)
            (fetch (applyWorld sys.store.world c.tenant d) (.header c.tenant))
          else { (sync sys c.tenant).cache with
              entries := carry inv c.tenant d (sys.store.gen c.tenant) (sync sys c.tenant).cache.entries },
        store := sys.store.applyDelta c.tenant d,
        known := upd (sync sys c.tenant).known c.tenant (sys.store.gen c.tenant + 1) },
      .result (.ok ()))

def stepWS (sys : Sys σ) : Op → Sys σ × Outcome
  | .read k =>
    match hit sys.cache.entries sys.known k with
    | some e => ({ sys with pst := pol.touch sys.pst k true }, .value e.val)
    | none =>
      ({ sync sys k.tenant with
          cache := admit pol size sys.pst (sync sys k.tenant).cache k (sys.store.gen k.tenant)
            (fetch sys.store.world k),
          pst := pol.touch sys.pst k false },
        .value (fetch sys.store.world k))
  | .pin k =>
    ({ sys with
        cache := admit pol size sys.pst { sys.cache with pins := k :: sys.cache.pins } k
          (sys.store.gen k.tenant) (fetch sys.store.world k) },
      .done)
  | .unpin k => ({ sys with cache := { sys.cache with pins := sys.cache.pins.filter (· != k) } }, .done)
  | .evict ks =>
    ({ sys with cache := { sys.cache with entries := dropKeys sys.cache.pins sys.cache.entries ks } },
      .done)
  | .invalidate k =>
    ({ sys with cache := { sys.cache with entries := removeKey sys.cache.entries k } }, .done)
  | .resize b => ({ sys with cache := resize pol size sys.pst sys.cache b }, .done)
  | .commit i c => commitWS pol size inv sys i c
  | .ambiguous i c =>
    ({ (commitWS pol size inv sys i c).1 with
        cache := { (commitWS pol size inv sys i c).1.cache with
          entries := dropTenant (commitWS pol size inv sys i c).1.cache.entries c.tenant },
        known := sys.known }, .unknown)
  | .external x t => (refresh { sys with store := sys.store.set x t } x, .done)
  | .unfenced x t => ({ sys with store := sys.store.set x t }, .done)
  | .check x => (sync sys x, .done)

def runWS (sys : Sys σ) : List Op → Sys σ × List Outcome
  | [] => (sys, [])
  | op :: ops => ((runWS (stepWS pol size inv sys op).1 ops).1,
      (stepWS pol size inv sys op).2 :: (runWS (stepWS pol size inv sys op).1 ops).2)

/-- The same schedule with no working set. -/
def directStep (w : World) : Op → World × Outcome
  | .read k => (w, .value (fetch w k))
  | .commit i c =>
    match step w i c with
    | .ok w' => (w', .result (.ok ()))
    | .error e => (w, .result (.error e))
  | .ambiguous i c =>
    match step w i c with
    | .ok w' => (w', .unknown)
    | .error _ => (w, .unknown)
  | .external x t => (upd w x t, .done)
  | .unfenced x t => (upd w x t, .done)
  | _ => (w, .done)

def directRun (w : World) : List Op → World × List Outcome
  | [] => (w, [])
  | op :: ops => ((directRun (directStep w op).1 ops).1,
      (directStep w op).2 :: (directRun (directStep w op).1 ops).2)

/-- Which tenants may have been written behind the runtime's back since it last learned their
    generation, after one operation. -/
def nextDirty (dirty : TenantId → Bool) : Op → TenantId → Bool
  | .unfenced x _ => upd dirty x true
  | .check x => upd dirty x false
  | .external x _ => upd dirty x false
  | .commit _ c => upd dirty c.tenant false
  | .ambiguous _ c => upd dirty c.tenant false
  | _ => dirty

/-- A read is allowed when its tenant is not dirty. -/
def readOk (dirty : TenantId → Bool) : Op → Bool
  | .read k => !dirty k.tenant
  | _ => true

/-- A schedule is covered when no read is served while its tenant may have been written behind the
    runtime's back. -/
def covered : (TenantId → Bool) → List Op → Bool
  | _, [] => true
  | dirty, op :: ops => readOk dirty op && covered (nextDirty dirty op) ops

/-! ### List lemmas -/

theorem dropKeys_sub (pins : List Key) (es : List Entry) (ks : List Key) :
    ∀ e ∈ dropKeys pins es ks, e ∈ es := fun _ he => (List.mem_filter.mp he).1

theorem removeKey_sub (es : List Entry) (k : Key) : ∀ e ∈ removeKey es k, e ∈ es :=
  fun _ he => (List.mem_filter.mp he).1

theorem dropTenant_sub (es : List Entry) (x : TenantId) : ∀ e ∈ dropTenant es x, e ∈ es :=
  fun _ he => (List.mem_filter.mp he).1

theorem dropTenant_tenant (es : List Entry) (x : TenantId) (e : Entry)
    (he : e ∈ dropTenant es x) : e.key.tenant ≠ x := by
  have := (List.mem_filter.mp he).2
  simpa using this

theorem bytes_filter_le (es : List Entry) (p : Entry → Bool) :
    bytes size (es.filter p) ≤ bytes size es := by
  induction es with
  | nil => simp [bytes]
  | cons e es ih =>
    unfold bytes at *
    by_cases hp : p e = true
    · simp [List.filter, hp]; omega
    · simp [List.filter, hp]; omega

theorem bytes_carry_le (x : TenantId) (d : Delta) (old : Nat) (es : List Entry) :
    bytes size (carry inv x d old es) ≤ bytes size es := by
  induction es with
  | nil => simp [carry, bytes]
  | cons e es ih =>
    unfold carry bytes at *
    simp only [List.filterMap_cons]
    by_cases hx : e.key.tenant = x
    · by_cases ht : inv d e.key = true
      · simp only [hx, ht, ite_true, List.map_cons, List.sum_cons]; omega
      · by_cases hs : e.stamp = old
        · simp only [hx, ht, hs, ite_true, Bool.false_eq_true, ite_false, List.map_cons,
            List.sum_cons]; omega
        · simp only [hx, ht, hs, ite_true, Bool.false_eq_true, ite_false, List.map_cons,
            List.sum_cons]; omega
    · simp only [hx, ite_false, List.map_cons, List.sum_cons]; omega

theorem base_sub (st : σ) (c : Cache) (k : Key) (n : Nat) :
    ∀ e ∈ base pol size st c k n, e ∈ c.entries :=
  fun e he => dropKeys_sub _ _ _ e (removeKey_sub _ _ e he)

theorem room_sub (st : σ) (c : Cache) (k : Key) (n : Nat) :
    ∀ e ∈ room pol size st c k n, e ∈ c.entries := by
  intro e he
  unfold room at he
  split at he
  · exact base_sub pol size _ _ _ _ e (List.mem_filter.mp he).1
  · exact base_sub pol size _ _ _ _ e he

theorem admit_sub (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) :
    ∀ e ∈ (admit pol size st c k stamp v).entries, e ∈ c.entries ∨ e = ⟨k, stamp, v⟩ := by
  intro e he
  unfold admit at he
  split at he
  · exact Or.inl he
  · split at he
    · rcases List.mem_cons.mp he with h | h
      · exact Or.inr h
      · exact Or.inl (room_sub pol size _ _ _ _ e h)
    · exact Or.inl he

theorem admit_pins (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) :
    (admit pol size st c k stamp v).pins = c.pins := by
  unfold admit; split
  · rfl
  · split <;> rfl

theorem admit_within (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload)
    (hw : Within size c) : Within size (admit pol size st c k stamp v) := by
  unfold admit
  split
  · exact hw
  · split
    · rename_i hfit
      simp only [Within, bytes, List.map_cons, List.sum_cons] at hfit ⊢
      omega
    · exact hw

theorem shrink_sub (st : σ) (c : Cache) (b : Nat) : ∀ e ∈ shrink pol size st c b, e ∈ c.entries := by
  intro e he
  unfold shrink at he
  split at he
  · exact dropKeys_sub _ _ _ e he
  · exact he

theorem trimTo_sub (pins : List Key) (es : List Entry) (b : Nat) :
    ∀ e ∈ trimTo size pins es b, e ∈ es := by
  intro e he
  unfold trimTo at he
  split at he
  · exact (List.mem_filter.mp he).1
  · exact he

theorem resize_sub (st : σ) (c : Cache) (b : Nat) :
    ∀ e ∈ (resize pol size st c b).entries, e ∈ c.entries :=
  fun e he => shrink_sub pol size _ _ _ e (trimTo_sub size _ _ _ e he)

theorem resize_within (st : σ) (c : Cache) (b : Nat) : Within size (resize pol size st c b) := by
  simp only [Within, resize]
  exact Nat.le_max_right _ _

/-! ### Sync and refresh -/

theorem sync_store (sys : Sys σ) (x : TenantId) : (sync sys x).store = sys.store := by
  unfold sync; split <;> rfl

theorem sync_pst (sys : Sys σ) (x : TenantId) : (sync sys x).pst = sys.pst := by
  unfold sync; split <;> rfl

theorem sync_pins (sys : Sys σ) (x : TenantId) : (sync sys x).cache.pins = sys.cache.pins := by
  unfold sync; split <;> rfl

theorem sync_budget (sys : Sys σ) (x : TenantId) : (sync sys x).cache.budget = sys.cache.budget := by
  unfold sync; split <;> rfl

theorem sync_sub (sys : Sys σ) (x : TenantId) :
    ∀ e ∈ (sync sys x).cache.entries, e ∈ sys.cache.entries := by
  unfold sync; split
  · exact fun _ h => h
  · exact dropTenant_sub _ _

theorem sync_known_other (sys : Sys σ) (x y : TenantId) (h : y ≠ x) :
    (sync sys x).known y = sys.known y := by
  unfold sync; split
  · rfl
  · simp [refresh, h]

theorem sync_current (sys : Sys σ) (x : TenantId) : (sync sys x).known x = sys.store.gen x := by
  unfold sync; split
  · assumption
  · simp [refresh]

theorem sync_within (sys : Sys σ) (x : TenantId) (hw : Within size sys.cache) :
    Within size (sync sys x).cache := by
  unfold sync; split
  · exact hw
  · exact Nat.le_trans (bytes_filter_le size _ _) hw

theorem sync_sound (sys : Sys σ) (x : TenantId) (hs : Sound sys.cache.entries sys.store) :
    Sound (sync sys x).cache.entries (sync sys x).store := by
  rw [sync_store]; exact sound_sub _ _ _ hs (sync_sub sys x)

/-- After `sync`, every read of `x` is safe. -/
theorem sync_safe_self (sys : Sys σ) (x : TenantId) (hs : Sound sys.cache.entries sys.store) :
    SafeAt (sync sys x).cache.entries (sync sys x).known (sync sys x).store x := by
  unfold sync; split
  · rename_i h; exact safe_of_current _ _ _ _ hs h
  · intro e he hx _
    exact absurd hx (dropTenant_tenant _ _ e he)

theorem sync_safe_other (sys : Sys σ) (x y : TenantId) (hy : y ≠ x)
    (hs : SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (sync sys x).cache.entries (sync sys x).known (sync sys x).store y := by
  intro e he hey hst
  rw [sync_store]
  rw [sync_known_other sys x y hy] at hst
  exact hs e (sync_sub sys x e he) hey hst

theorem refresh_safe (sys : Sys σ) (x y : TenantId)
    (hy : y ≠ x → SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (refresh sys x).cache.entries (refresh sys x).known (refresh sys x).store y := by
  by_cases hyx : y = x
  · subst hyx
    intro e he hx _
    exact absurd hx (dropTenant_tenant _ _ e he)
  · intro e he hey hst
    simp only [refresh, upd_other _ _ _ _ hyx] at hst ⊢
    exact hy hyx e (dropTenant_sub _ _ e he) hey hst

/-! ### Admission keeps coherence -/

theorem admit_sound (st : σ) (c : Cache) (s : Store) (k : Key) (stamp : Nat) (v : Payload)
    (hst : stamp = s.gen k.tenant) (hv : v = fetch s.world k) (hs : Sound c.entries s) :
    Sound (admit pol size st c k stamp v).entries s := by
  intro e he
  rcases admit_sub pol size st c k _ _ e he with h | h
  · exact hs e h
  · subst h; exact ⟨Nat.le_of_eq hst, fun _ => hv⟩

theorem admit_safe (st : σ) (c : Cache) (known : TenantId → Nat) (s : Store) (k : Key)
    (stamp : Nat) (y : TenantId) (hs : SafeAt c.entries known s y) :
    SafeAt (admit pol size st c k stamp (fetch s.world k)).entries known s y := by
  intro e he hy hst
  rcases admit_sub pol size st c k _ _ e he with h | h
  · exact hs e h hy hst
  · subst h; rfl

/-! ### The commit path -/

theorem carry_sound (es : List Entry) (s : Store) (x : TenantId) (d : Delta) (hinv : InvSound inv)
    (hs : Sound es s) : Sound (carry inv x d (s.gen x) es) (s.applyDelta x d) := by
  intro e he
  obtain ⟨e₀, he₀, hf⟩ := List.mem_filterMap.mp he
  obtain ⟨hle, hcur⟩ := hs e₀ he₀
  by_cases hx : e₀.key.tenant = x
  · by_cases ht : inv d e₀.key = true
    · simp [hx, ht] at hf
    · have ht' : inv d e₀.key = false := by simpa using ht
      have hun := hinv s.world x d e₀.key hx ht'
      by_cases hst : e₀.stamp = s.gen x
      · simp only [hx, ite_true, ht', Bool.false_eq_true, ite_false, hst, Option.some.injEq] at hf
        subst hf
        refine ⟨by simp [Store.applyDelta, hx], fun _ => ?_⟩
        show e₀.val = fetch (applyWorld s.world x d) e₀.key
        rw [hun]; exact hcur (by rw [hx]; exact hst)
      · simp only [hx, ite_true, ht', Bool.false_eq_true, ite_false, hst, Option.some.injEq] at hf
        subst hf
        rw [hx] at hle
        refine ⟨by simp [Store.applyDelta, hx]; omega, fun h => ?_⟩
        simp [Store.applyDelta, hx] at h; omega
  · simp only [hx, ite_false, Option.some.injEq] at hf
    subst hf
    refine ⟨by simp [Store.applyDelta, hx]; exact hle, fun h => ?_⟩
    simp [Store.applyDelta, hx] at h
    show e₀.val = fetch (applyWorld s.world x d) e₀.key
    rw [fetch_applyWorld_other _ _ _ _ hx]; exact hcur h

theorem carry_safe (es : List Entry) (known : TenantId → Nat) (s : Store) (x : TenantId)
    (d : Delta) (hinv : InvSound inv) (hs : Sound es s) (y : TenantId)
    (hy : y ≠ x → SafeAt es known s y) :
    SafeAt (carry inv x d (s.gen x) es) (upd known x (s.gen x + 1)) (s.applyDelta x d) y := by
  intro e he hey hst
  by_cases hyx : y = x
  · have hsd := carry_sound inv es s x d hinv hs e he
    have hst' : e.stamp = (s.applyDelta x d).gen e.key.tenant := by
      rw [hyx] at hst
      rw [hey, hyx]
      simpa [Store.applyDelta] using hst
    exact hsd.2 hst'
  · obtain ⟨e₀, he₀, hf⟩ := List.mem_filterMap.mp he
    have hx0 : e₀.key.tenant ≠ x := by
      intro hx0
      by_cases ht : inv d e₀.key = true
      · simp [hx0, ht] at hf
      · by_cases h2 : e₀.stamp = s.gen x
        · simp [hx0, ht, h2] at hf; subst hf; exact hyx (hey.symm.trans hx0)
        · simp [hx0, ht, h2] at hf; subst hf; exact hyx (hey.symm.trans hx0)
    simp only [hx0, ite_false, Option.some.injEq] at hf
    subst hf
    simp only [upd_other _ _ _ _ hyx] at hst
    show e₀.val = fetch (applyWorld s.world x d) e₀.key
    rw [fetch_applyWorld_other _ _ _ _ hx0]
    exact hy hyx e₀ he₀ hey hst

/-- The decision the commit path makes is the decision on the store's own answers. -/
theorem commit_decision (sys : Sys σ) (c : Ctx) (i : Intent)
    (hs : Sound sys.cache.entries sys.store) :
    decideOn (serve (sync sys c.tenant).cache.entries (sync sys c.tenant).known
      (sync sys c.tenant).store) c.tenant c.principal i
      = decideOn (fetch sys.store.world) c.tenant c.principal i := by
  apply decideOn_congr
  intro k hk
  rw [serve_eq_fetch _ _ _ _ (by rw [hk]; exact sync_safe_self sys c.tenant hs), sync_store]

/-- **Commits are transparent in every schedule**: whatever the runtime holds and however stale it
    is, a commit through it has the outcome and the committed world of the kernel's step. -/
theorem commitWS_transparent (sys : Sys σ) (i : Intent) (c : Ctx)
    (hs : Sound sys.cache.entries sys.store) :
    (commitWS pol size inv sys i c).1.store.world = (directStep sys.store.world (.commit i c)).1 ∧
    (commitWS pol size inv sys i c).2 = (directStep sys.store.world (.commit i c)).2 := by
  unfold commitWS
  rw [commit_decision sys c i hs]
  simp only [directStep]
  cases hd : decideOn (fetch sys.store.world) c.tenant c.principal i with
  | error e =>
    rw [decide_error_step _ _ _ _ hd]
    exact ⟨by rw [sync_store], rfl⟩
  | ok d =>
    rw [decide_ok_step _ _ _ _ hd]
    exact ⟨rfl, rfl⟩

theorem commitWS_sound (sys : Sys σ) (i : Intent) (c : Ctx) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) :
    Sound (commitWS pol size inv sys i c).1.cache.entries (commitWS pol size inv sys i c).1.store := by
  have h0 := sync_sound sys c.tenant hs
  rw [sync_store] at h0
  unfold commitWS
  split
  · exact sync_sound sys c.tenant hs
  · rename_i d _
    have h1 := carry_sound inv _ _ c.tenant d hinv h0
    split
    · exact admit_sound pol size _ _ (sys.store.applyDelta c.tenant d) _ _ _
        (by simp [Store.applyDelta, Key.tenant]) rfl h1
    · exact h1

/-- After a commit, every read of the committed tenant is safe, and reads of other tenants stay as
    safe as they were. -/
theorem commitWS_safe (sys : Sys σ) (i : Intent) (c : Ctx) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) (y : TenantId)
    (hy : y ≠ c.tenant → SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (commitWS pol size inv sys i c).1.cache.entries (commitWS pol size inv sys i c).1.known
      (commitWS pol size inv sys i c).1.store y := by
  have h0 := sync_sound sys c.tenant hs
  rw [sync_store] at h0
  have hy' : y ≠ c.tenant → SafeAt (sync sys c.tenant).cache.entries (sync sys c.tenant).known
      sys.store y := by
    intro hne
    have := sync_safe_other sys c.tenant y hne (hy hne)
    rwa [sync_store] at this
  unfold commitWS
  split
  · by_cases hyx : y = c.tenant
    · subst hyx; exact sync_safe_self sys _ hs
    · exact sync_safe_other sys _ y hyx (hy hyx)
  · rename_i d _
    have h1 := carry_safe inv _ _ _ c.tenant d hinv h0 y hy'
    split
    · exact admit_safe pol size _ _ _ (sys.store.applyDelta c.tenant d) _ _ _ h1
    · exact h1

theorem commitWS_known_other (sys : Sys σ) (i : Intent) (c : Ctx) (y : TenantId)
    (hy : y ≠ c.tenant) : (commitWS pol size inv sys i c).1.known y = sys.known y := by
  unfold commitWS
  split
  · exact sync_known_other sys _ y hy
  · simp only [upd_other _ _ _ _ hy]; exact sync_known_other sys _ y hy

theorem commitWS_pins (sys : Sys σ) (i : Intent) (c : Ctx) :
    (commitWS pol size inv sys i c).1.cache.pins = sys.cache.pins := by
  unfold commitWS
  split
  · exact sync_pins sys _
  · split
    · rw [admit_pins]; exact sync_pins sys _
    · exact sync_pins sys _

theorem commitWS_within (sys : Sys σ) (i : Intent) (c : Ctx) (hw : Within size sys.cache) :
    Within size (commitWS pol size inv sys i c).1.cache := by
  have h0 := sync_within size sys c.tenant hw
  unfold commitWS
  split
  · exact h0
  · rename_i d _
    have h1 : Within size { (sync sys c.tenant).cache with
        entries := carry inv c.tenant d (sys.store.gen c.tenant) (sync sys c.tenant).cache.entries } :=
      Nat.le_trans (bytes_carry_le size inv _ _ _ _) h0
    split
    · exact admit_within pol size _ _ _ _ _ h1
    · exact h1

/-! ### Every operation -/

theorem stepWS_sound (sys : Sys σ) (op : Op) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) :
    Sound (stepWS pol size inv sys op).1.cache.entries (stepWS pol size inv sys op).1.store := by
  cases op with
  | read k =>
    simp only [stepWS]
    split
    · exact hs
    · have h0 := sync_sound sys k.tenant hs
      rw [sync_store] at h0 ⊢
      exact admit_sound pol size _ _ _ _ _ _ rfl rfl h0
  | pin k => exact admit_sound pol size _ { sys.cache with pins := _ } _ _ _ _ rfl rfl hs
  | unpin k => exact hs
  | evict ks => exact sound_sub _ _ _ hs (dropKeys_sub _ _ _)
  | invalidate k => exact sound_sub _ _ _ hs (removeKey_sub _ _)
  | resize b => exact sound_sub _ _ _ hs (resize_sub pol size _ _ _)
  | commit i c => exact commitWS_sound pol size inv sys i c hinv hs
  | ambiguous i c =>
    exact sound_sub _ _ _ (commitWS_sound pol size inv sys i c hinv hs) (dropTenant_sub _ _)
  | external x t =>
    exact sound_sub _ _ _ (sound_set _ _ x t hs) (dropTenant_sub _ _)
  | unfenced x t => exact sound_set _ _ x t hs
  | check x => exact sync_sound sys x hs

theorem stepWS_within (sys : Sys σ) (op : Op) (hw : Within size sys.cache) :
    Within size (stepWS pol size inv sys op).1.cache := by
  cases op with
  | read k =>
    simp only [stepWS]
    split
    · exact hw
    · exact admit_within pol size _ _ _ _ _ (sync_within size sys _ hw)
  | pin k => exact admit_within pol size _ { sys.cache with pins := _ } _ _ _ hw
  | unpin k => exact hw
  | evict ks => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | invalidate k => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | resize b => exact resize_within pol size _ _ _
  | commit i c => exact commitWS_within pol size inv sys i c hw
  | ambiguous i c =>
    exact Nat.le_trans (bytes_filter_le size _ _) (commitWS_within pol size inv sys i c hw)
  | external x t => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | unfenced x t => exact hw
  | check x => exact sync_within size sys x hw

/-- **Pins change only when asked.** No read, eviction, invalidation, resize, commit or write
    behind the runtime's back adds or removes a pin. -/
theorem stepWS_pins (sys : Sys σ) (op : Op) :
    (stepWS pol size inv sys op).1.cache.pins =
      match op with
      | .pin k => k :: sys.cache.pins
      | .unpin k => sys.cache.pins.filter (· != k)
      | _ => sys.cache.pins := by
  cases op with
  | read k =>
    simp only [stepWS]
    split
    · rfl
    · simp only [admit_pins]; exact sync_pins sys _
  | pin k => exact admit_pins pol size _ _ _ _ _
  | unpin k => rfl
  | evict ks => rfl
  | invalidate k => rfl
  | resize b => rfl
  | commit i c => exact commitWS_pins pol size inv sys i c
  | ambiguous i c => exact commitWS_pins pol size inv sys i c
  | external x t => rfl
  | unfenced x t => rfl
  | check x => exact sync_pins sys x

/-- Safety of reads across one operation: every tenant not dirtied by the operation keeps safe
    reads, and the tenants a commit, a check or a fenced write touch become safe. -/
theorem stepWS_safe (sys : Sys σ) (op : Op) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) (dirty : TenantId → Bool)
    (hsafe : ∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) :
    ∀ y, nextDirty dirty op y = false →
      SafeAt (stepWS pol size inv sys op).1.cache.entries (stepWS pol size inv sys op).1.known
        (stepWS pol size inv sys op).1.store y := by
  intro y hy
  cases op with
  | read k =>
    simp only [stepWS]
    split
    · exact hsafe y hy
    · simp only []
      have hsy : SafeAt (sync sys k.tenant).cache.entries (sync sys k.tenant).known sys.store y := by
        by_cases hyx : y = k.tenant
        · subst hyx; have := sync_safe_self sys k.tenant hs; rwa [sync_store] at this
        · have := sync_safe_other sys _ y hyx (hsafe y hy); rwa [sync_store] at this
      rw [sync_store]
      exact admit_safe pol size _ _ _ _ _ _ _ hsy
  | pin k => exact admit_safe pol size _ { sys.cache with pins := _ } _ _ _ _ _ (hsafe y hy)
  | unpin k => exact hsafe y hy
  | evict ks => exact safe_sub _ _ _ _ _ (hsafe y hy) (dropKeys_sub _ _ _)
  | invalidate k => exact safe_sub _ _ _ _ _ (hsafe y hy) (removeKey_sub _ _)
  | resize b => exact safe_sub _ _ _ _ _ (hsafe y hy) (resize_sub pol size _ _ _)
  | commit i c =>
    apply commitWS_safe pol size inv sys i c hinv hs y
    intro hne
    exact hsafe y (by simpa [nextDirty, upd, hne] using hy)
  | ambiguous i c =>
    by_cases hyx : y = c.tenant
    · subst hyx
      intro e he hx _
      exact absurd hx (dropTenant_tenant _ _ e he)
    · have hsy := commitWS_safe pol size inv sys i c hinv hs y
        (fun _ => hsafe y (by simpa [nextDirty, upd, hyx] using hy))
      intro e he hey hst
      apply hsy e (dropTenant_sub _ _ e he) hey
      rw [commitWS_known_other pol size inv sys i c y hyx]
      exact hst
  | external x t =>
    apply refresh_safe { sys with store := sys.store.set x t } x y
    intro hne
    exact safe_set_other _ _ _ x y t (hsafe y (by simpa [nextDirty, upd, hne] using hy)) hne
  | unfenced x t =>
    by_cases hyx : y = x
    · subst hyx; simp [nextDirty] at hy
    · exact safe_set_other _ _ _ x y t (hsafe y (by simpa [nextDirty, upd, hyx] using hy)) hyx
  | check x =>
    by_cases hyx : y = x
    · subst hyx; exact sync_safe_self sys _ hs
    · exact sync_safe_other sys x y hyx (hsafe y (by simpa [nextDirty, upd, hyx] using hy))

/-- One operation through the working set commits the same world as with no working set, always;
    and has the same outcome, provided a read is not of a tenant written behind the runtime's back. -/
theorem stepWS_transparent (sys : Sys σ) (op : Op) (hs : Sound sys.cache.entries sys.store) :
    (stepWS pol size inv sys op).1.store.world = (directStep sys.store.world op).1 ∧
    ((∀ k, op = .read k → SafeAt sys.cache.entries sys.known sys.store k.tenant) →
      (stepWS pol size inv sys op).2 = (directStep sys.store.world op).2) := by
  cases op with
  | read k =>
    refine ⟨?_, fun hr => ?_⟩
    · simp only [stepWS, directStep]; split
      · rfl
      · simp only []; rw [sync_store]
    · simp only [stepWS, directStep]
      split
      · rename_i e h
        have := serve_eq_fetch _ _ _ k (hr k rfl)
        simp only [serve, h] at this
        simp [this]
      · rfl
  | pin k => exact ⟨rfl, fun _ => rfl⟩
  | unpin k => exact ⟨rfl, fun _ => rfl⟩
  | evict ks => exact ⟨rfl, fun _ => rfl⟩
  | invalidate k => exact ⟨rfl, fun _ => rfl⟩
  | resize b => exact ⟨rfl, fun _ => rfl⟩
  | commit i c =>
    obtain ⟨h1, h2⟩ := commitWS_transparent pol size inv sys i c hs
    exact ⟨h1, fun _ => h2⟩
  | ambiguous i c =>
    obtain ⟨h1, _⟩ := commitWS_transparent pol size inv sys i c hs
    refine ⟨?_, fun _ => ?_⟩
    · show (commitWS pol size inv sys i c).1.store.world = _
      rw [h1]; simp only [directStep]; split <;> rfl
    · simp only [stepWS, directStep]; split <;> rfl
  | external x t => exact ⟨rfl, fun _ => rfl⟩
  | unfenced x t => exact ⟨rfl, fun _ => rfl⟩
  | check x => exact ⟨by simp only [stepWS, directStep]; rw [sync_store], fun _ => rfl⟩

/-! ### Whole schedules -/

/-- **The committed world is transparent in every schedule**: any policy, any invalidation map
    meeting the criterion, any mix of reads, commits, lost receipts, fenced and unfenced writes. -/
theorem runWS_world (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ), Sound sys.cache.entries sys.store →
      (runWS pol size inv sys ops).1.store.world = (directRun sys.store.world ops).1 := by
  induction ops with
  | nil => intro sys _; rfl
  | cons op ops ih =>
    intro sys hs
    simp only [runWS, directRun]
    rw [← (stepWS_transparent pol size inv sys op hs).1]
    exact ih _ (stepWS_sound pol size inv sys op hinv hs)

/-- **Transparency.** In a covered schedule (no read of a tenant written behind the runtime's back
    since the runtime last learned its generation), every outcome and the committed world equal
    those of the same schedule with no working set. -/
theorem runWS_transparent (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ) (dirty : TenantId → Bool), Sound sys.cache.entries sys.store →
      (∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) →
      covered dirty ops = true →
      (runWS pol size inv sys ops).1.store.world = (directRun sys.store.world ops).1 ∧
      (runWS pol size inv sys ops).2 = (directRun sys.store.world ops).2 := by
  induction ops with
  | nil => intro sys _ _ _ _; exact ⟨rfl, rfl⟩
  | cons op ops ih =>
    intro sys dirty hs hsafe hcov
    have hstep := stepWS_transparent pol size inv sys op hs
    have hs' := stepWS_sound pol size inv sys op hinv hs
    have hsafe' := stepWS_safe pol size inv sys op hinv hs dirty hsafe
    have hout : (stepWS pol size inv sys op).2 = (directStep sys.store.world op).2 := by
      apply hstep.2
      intro k hk
      subst hk
      simp only [covered, readOk, Bool.and_eq_true, Bool.not_eq_true'] at hcov
      exact hsafe k.tenant hcov.1
    have hcov' : covered (nextDirty dirty op) ops = true := by
      simp only [covered, Bool.and_eq_true] at hcov
      exact hcov.2
    obtain ⟨ihw, iho⟩ := ih _ _ hs' hsafe' hcov'
    simp only [runWS, directRun]
    rw [← hstep.1]
    exact ⟨ihw, by rw [hout, iho]⟩

theorem runWS_within (ops : List Op) :
    ∀ (sys : Sys σ), Within size sys.cache → Within size (runWS pol size inv sys ops).1.cache := by
  induction ops with
  | nil => intro sys hw; exact hw
  | cons op ops ih =>
    intro sys hw
    simp only [runWS]
    exact ih _ (stepWS_within pol size inv sys op hw)

theorem empty_sound (s : Store) : Sound [] s := fun _ h => absurd h List.not_mem_nil

theorem empty_safe (known : TenantId → Nat) (s : Store) (y : TenantId) : SafeAt [] known s y :=
  fun _ h => absurd h List.not_mem_nil

/-- The world of the counterexample: tenant 1 exists, empty. -/
def cexTenant : Tenant :=
  { schema := [], owner := 0, editors := [], writers := [], records := [], nextId := 1 }

def cexWorld : World := fun x => if x = 1 then some cexTenant else none

/-- A runtime holding tenant 1's header, current at generation 0. -/
def cexSys : Sys Unit :=
  { cache := { entries := [{ key := .header 1, stamp := 0, val := fetch cexWorld (.header 1) }],
               pins := [], budget := 0 },
    store := { world := cexWorld, gen := fun _ => 0 },
    known := fun _ => 0,
    pst := () }

/-- **Why the check is necessary.** A write behind the runtime's back followed by a read with no
    check, miss or commit in between serves the value from before the write, for every policy and
    invalidation map. -/
theorem unfenced_breaks_reads (pol : Policy Unit) (size : Key → Payload → Nat)
    (inv : Delta → Key → Bool) :
    Sound cexSys.cache.entries cexSys.store ∧
    (runWS pol size inv cexSys [.unfenced 1 none, .read (.header 1)]).2 ≠
      (directRun cexSys.store.world [.unfenced 1 none, .read (.header 1)]).2 := by
  refine ⟨?_, ?_⟩
  · intro e' he'
    simp only [cexSys, List.mem_singleton] at he'
    subst he'
    exact ⟨Nat.le_refl _, fun _ => rfl⟩
  · simp [runWS, directRun, stepWS, directStep, Store.set, hit, upd, fetch, cexSys, cexWorld,
      cexTenant]

/-! ### Pinning -/

def Resident (c : Cache) (k : Key) : Prop := ∃ e ∈ c.entries, e.key = k

/-- A resident pinned key stays resident through any admission (of any key, the same key
    included: a replacement keeps it resident). -/
theorem admit_keeps_pinned (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload) (k' : Key)
    (hp : c.pins.contains k' = true) (hr : Resident c k') :
    Resident (admit pol size st c k stamp v) k' := by
  obtain ⟨e, he, hk⟩ := hr
  unfold admit
  split
  · exact ⟨e, he, hk⟩
  · split
    · by_cases hkk : k' = k
      · exact ⟨⟨k, stamp, v⟩, List.mem_cons_self, hkk.symm⟩
      · have hp' : c.pins.contains e.key = true := by rw [hk]; exact hp
        have h1 : e ∈ base pol size st c k (size k v) := by
          simp only [base, removeKey, dropKeys, List.mem_filter, he, hp', Bool.true_or, and_self,
            true_and, bne_iff_ne, ne_eq]
          rw [hk]; exact hkk
        have h2 : e ∈ room pol size st c k (size k v) := by
          unfold room; split
          · exact List.mem_filter.mpr ⟨h1, hp'⟩
          · exact h1
        exact ⟨e, List.mem_cons_of_mem _ h2, hk⟩
    · exact ⟨e, he, hk⟩

theorem evict_keeps_pinned (c : Cache) (ks : List Key) (k' : Key)
    (hp : c.pins.contains k' = true) (hr : Resident c k') :
    Resident { c with entries := dropKeys c.pins c.entries ks } k' := by
  obtain ⟨e, he, hk⟩ := hr
  refine ⟨e, ?_, hk⟩
  simp only [dropKeys, List.mem_filter]
  exact ⟨he, by rw [hk, hp]; rfl⟩

theorem resize_keeps_pinned (st : σ) (c : Cache) (b : Nat) (k' : Key)
    (hp : c.pins.contains k' = true) (hr : Resident c k') :
    Resident (resize pol size st c b) k' := by
  obtain ⟨e, he, hk⟩ := hr
  have hp' : c.pins.contains e.key = true := by rw [hk]; exact hp
  have h0 : e ∈ shrink pol size st c b := by
    unfold shrink; split
    · simp only [dropKeys, List.mem_filter]
      exact ⟨he, by rw [hp']; rfl⟩
    · exact he
  refine ⟨e, ?_, hk⟩
  show e ∈ trimTo size c.pins (shrink pol size st c b) b
  unfold trimTo; split
  · exact List.mem_filter.mpr ⟨h0, hp'⟩
  · exact h0

private theorem filter_dropKeys_of_pinned (pins : List Key) (es : List Entry) (ks : List Key)
    (q : Entry → Bool) (hq : ∀ e, q e = true → pins.contains e.key = true) :
    (dropKeys pins es ks).filter q = es.filter q := by
  unfold dropKeys
  rw [List.filter_filter]
  apply List.filter_congr
  intro e _
  by_cases h : q e = true
  · have hp := hq e h
    simp only [List.contains_iff_mem] at hp
    simp [h, hp]
  · simp [h]

/-- **A pinned key is admitted whenever the pinned reserve fits**, for any policy. -/
theorem pin_resident (st : σ) (c : Cache) (k : Key) (stamp : Nat) (v : Payload)
    (hk : c.pins.contains k = true)
    (hfit : bytes size ((c.entries.filter (fun e => c.pins.contains e.key)).filter
      (fun e => e.key != k)) + size k v ≤ c.budget) :
    ⟨k, stamp, v⟩ ∈ (admit pol size st c k stamp v).entries := by
  have hk' : k ∈ c.pins := by simpa using hk
  unfold admit
  rw [ite_eq_right (by simp [hk'])]
  have hpin : (base pol size st c k (size k v)).filter (fun e => c.pins.contains e.key)
      = (c.entries.filter (fun e => c.pins.contains e.key)).filter (fun e => e.key != k) := by
    unfold base removeKey
    rw [List.filter_filter, List.filter_filter,
      filter_dropKeys_of_pinned _ _ _ _ (fun e h => by simp at h; simp [h.1])]
    apply List.filter_congr
    intro e _
    exact Bool.and_comm _ _
  have hroom : bytes size (room pol size st c k (size k v)) + size k v ≤ c.budget := by
    unfold room
    by_cases hb : c.budget < bytes size (base pol size st c k (size k v)) + size k v
    · rw [ite_eq_left (by simp [hk', hb]), hpin]; exact hfit
    · rw [ite_eq_right (by simp [hb])]; omega
  rw [ite_eq_left hroom]
  exact List.mem_cons_self

/-! ### Budgets on one machine -/

private theorem shrink_pinned (st : σ) (c : Cache) (b : Nat) :
    (shrink pol size st c b).filter (fun e => c.pins.contains e.key)
      = c.entries.filter (fun e => c.pins.contains e.key) := by
  unfold shrink; split
  · exact filter_dropKeys_of_pinned _ _ _ _ (fun _ h => h)
  · rfl

/-- The arbiter gets any budget at or above the pinned bytes. -/
theorem resize_budget_exact (st : σ) (c : Cache) (b : Nat) (hb : pinnedBytes size c ≤ b) :
    (resize pol size st c b).budget = b := by
  simp only [resize]
  apply Nat.max_eq_left
  unfold trimTo; split
  · rw [shrink_pinned]; exact hb
  · omega

/-- Whatever is granted, the budget exceeds the grant by at most the pinned bytes. -/
theorem resize_budget_le (st : σ) (c : Cache) (b : Nat) :
    (resize pol size st c b).budget ≤ max b (pinnedBytes size c) := by
  simp only [resize]
  apply Nat.max_le.mpr
  refine ⟨Nat.le_max_left _ _, ?_⟩
  unfold trimTo; split
  · rw [shrink_pinned]; exact Nat.le_max_right _ _
  · rename_i h; exact Nat.le_trans (Nat.le_of_not_lt h) (Nat.le_max_left _ _)

/-- **One machine.** When the arbiter grants each working set at least its pinned bytes, and the
    grants sum to at most the machine's memory, the resized working sets together use at most it. -/
theorem machine_resize (st : σ) (cs : List (Cache × Nat)) (m : Nat)
    (hfloor : ∀ cb ∈ cs, pinnedBytes size cb.1 ≤ cb.2) (hm : (cs.map (·.2)).sum ≤ m) :
    (cs.map (fun cb => bytes size (resize pol size st cb.1 cb.2).entries)).sum ≤ m := by
  induction cs generalizing m with
  | nil => simp
  | cons cb cs ih =>
    simp only [List.map_cons, List.sum_cons] at hm ⊢
    have hw := resize_within pol size st cb.1 cb.2
    rw [Within, resize_budget_exact pol size st cb.1 cb.2 (hfloor cb List.mem_cons_self)] at hw
    have := ih ((cs.map (·.2)).sum) (fun c h => hfloor c (List.mem_cons_of_mem _ h)) (Nat.le_refl _)
    omega

/-! ## Erasure: where the working set sits against sealing -/

/-- The subject keys that still open (D-350): destroying one is not a change to the world and does
    not raise any generation. -/
abbrev Keyring := Nat → Bool

/-- An erasure destroys a subject key. -/
def destroy (kr : Keyring) (n : Nat) : Keyring := fun m => if m = n then false else kr m

/-- Below sealing the working set holds what the store holds (sealed values) and opens on every
    serve with the keys live at that moment; its entries are coherent with the store as above. A
    read is the opened store value whatever keys have been destroyed since the entry was admitted. -/
theorem below_sealing_serves_current (opening : Keyring → Payload → Payload) (es : List Entry)
    (known : TenantId → Nat) (s : Store) (kr : Keyring) (n : Nat) (k : Key)
    (hs : SafeAt es known s k.tenant) :
    opening (destroy kr n) (serve es known s k) = opening (destroy kr n) (fetch s.world k) := by
  rw [serve_eq_fetch _ _ _ _ hs]

/-- Above sealing the working set holds opened values. After a key is destroyed, with no change to
    the world or the generation, it still serves the value the key used to open. -/
theorem above_sealing_serves_erased :
    ∃ (opening : Keyring → Payload → Payload) (kr : Keyring) (v : Payload),
      opening kr v ≠ opening (destroy kr 0) v := by
  refine ⟨fun kr p => if kr 0 then p else .counters none, fun _ => true, .counters (some 1), ?_⟩
  simp [destroy]

end Strata.WorkingSet
