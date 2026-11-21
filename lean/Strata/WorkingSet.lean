/-
The tenant working set: what a runtime holds in memory in front of the store, and exactly when
holding it changes no result.

A model of outcomes, not of an algorithm. The eviction and admission policy is a parameter
(`Policy`, with a state of any type), and so is the map from a delta to the keys it invalidates
(`inv`, any map satisfying `InvSound`). What the runtime enforces whatever the policy says is in
the operations: the budget check, the pin set, the generation stamp, the commit guard.

The store is the authority (`Store`): the committed world and a **generation** per tenant. The
generation is not the tenant revision, which a restore sets back. In the model the generation is a
counter; in the design it is a pair (a random nonce rotated whenever the tenant's storage is
replaced or reopened, and a counter raised by every write transaction), compared for equality, so
the property used here is only that a changed state never shows a generation already seen. The
model's store changes only through operations that raise it (`Store.set`, `Store.applyDelta`):
that is assumption W4, and `w4_violation_breaks_checked_read` shows what happens without it.

The runtime knows a generation per tenant (`Sys.known`), which may be behind the store's. A
decision is made on what the runtime holds and committed separately (`submit`), guarded by the
generation it was decided at, compared by the store in the same step that applies the delta. An
**unfenced** write (one the runtime is not told about) is seen at the next generation read, miss
or commit.

Assumptions, all of them:

W1. The kernel is the model of `Strata/Model.lean`; nothing here refers to Rust.
W2. Operations are atomic and sequential. Interleaving is modelled by placing other writers'
    operations (`external`, `unfenced`), lost receipts (`submitLost`) and any other operation
    between a decision and its commit.
W3. The size of an entry is a function of its key and value (`size`): accounted bytes, not
    measured memory.
W4. Every change to what a load returns raises the tenant's generation (`Advances`).

The theorems the design cites:

- Coverage soundness: `decideTenant_faithful`, `decideOn_fetch`, `decideOn_congr`.
- Invalidation: `InvSound` (the criterion), `touchedIn_sound` (a map meeting it, with unordered
  answers invalidated by any record change and ordered pages only by changes above their start).
- Reads: `checkedRead_linearizes` (a checked read returns the store's value, always),
  `readWS_value` and `runWS_reads` (unchecked reads, in covered schedules),
  `unfenced_breaks_reads` (the counterexample outside them).
- Commits: `decideWS_refusal` (a refusal is the kernel's), `submitWS_world` (an accepted commit is
  a kernel step on the state it is applied to), `runWS_faithful` (both, at every step of every
  schedule), `w4_violation_breaks_checked_read` (the boundary of W4).
- Budget: `stepWS_within`, `runWS_within`, `resize_budget_exact`, `resize_budget_le`,
  `machine_resize`.
- Pinning: `stepWS_pins`, `admit_keeps_pinned`, `evict_keeps_pinned`, `resize_keeps_pinned`,
  `pin_resident`.
- Erasure: `below_sealing_erasure_safe`, `above_sealing_serves_erased`.

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
  /-- An ordered answer: up to `limit` ids above `after`, in the stored order (a page). -/
  | page (t : TenantId) (after : RecordId) (limit : Nat)
  deriving DecidableEq

def Key.tenant : Key → TenantId
  | .header t => t
  | .counters t => t
  | .record t _ => t
  | .all t => t
  | .answer t _ _ => t
  | .page t _ _ => t

/-- The value under a key. `none` inside means the tenant does not exist. -/
inductive Payload where
  | header (h : Option Header)
  | counters (n : Option Nat)
  | record (r : Option Record)
  | all (rs : Option (List (RecordId × Record)))
  | answer (ids : Option (List RecordId))
  | page (ids : Option (List RecordId))

/-- The ids of the records whose field `f` is `v`. -/
def answerOf (t : Tenant) (f : FieldId) (v : Val) : List RecordId :=
  (t.records.filter (fun q => q.2.fields.get f == v)).map Prod.fst

/-- Up to `l` ids above `a`, in the stored order. -/
def pageOf (t : Tenant) (a : RecordId) (l : Nat) : List RecordId :=
  ((t.records.filter (fun q => decide (a < q.1))).map Prod.fst).take l

/-- The authority's answer for a key: what the store would load. -/
def fetch (w : World) : Key → Payload
  | .header t => .header ((w t).map Tenant.header)
  | .counters t => .counters ((w t).map Tenant.nextId)
  | .record t id => .record ((w t).bind (fun tn => tn.records.lookup id))
  | .all t => .all ((w t).map Tenant.records)
  | .answer t f v => .answer ((w t).map (fun tn => answerOf tn f v))
  | .page t a l => .page ((w t).map (fun tn => pageOf tn a l))

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
  | .create _ _, .answer _ _ _ => true
  | .replace _ _, .answer _ _ _ => true
  | .remove _, .answer _ _ _ => true
  | .create id' _, .page _ a _ => decide (a < id')
  | .remove id', .page _ a _ => decide (a < id')
  | _, .page _ _ _ => false

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

private theorem page_cons_le (rs : List (RecordId × Record)) (a id : RecordId) (r : Record)
    (h : ¬ a < id) :
    ((id, r) :: rs).filter (fun q => decide (a < q.1)) = rs.filter (fun q => decide (a < q.1)) := by
  simp [List.filter, h]

private theorem page_map_replace (rs : List (RecordId × Record)) (a id : RecordId) (r : Record) :
    ((rs.map (fun q => if q.1 = id then (q.1, r) else q)).filter (fun q => decide (a < q.1))).map
      Prod.fst = (rs.filter (fun q => decide (a < q.1))).map Prod.fst := by
  induction rs with
  | nil => rfl
  | cons q rs ih =>
    obtain ⟨k, v⟩ := q
    by_cases hk : k = id
    · subst hk
      by_cases ha : a < k
      · have hd : decide (a < k) = true := by simp [ha]
        simp [List.filter, hd, ih]
      · have hd : decide (a < k) = false := by simp [ha]
        simp [List.filter, hd, ih]
    · by_cases ha : a < k
      · simp [List.filter, hk, ha, ih]
      · simp [List.filter, hk, ha, ih]

private theorem page_filter_remove (rs : List (RecordId × Record)) (a id : RecordId)
    (h : ¬ a < id) :
    (rs.filter (fun q => q.1 != id)).filter (fun q => decide (a < q.1))
      = rs.filter (fun q => decide (a < q.1)) := by
  rw [List.filter_filter]
  apply List.filter_congr
  intro q _
  by_cases ha : a < q.1
  · have hne : q.1 ≠ id := fun he => h (he ▸ ha)
    simp [ha, hne]
  · simp [ha]

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
  | page t a l =>
    simp only [Key.tenant] at hx; subst hx
    cases d with
    | create id' r =>
      have hne : ¬ a < id' := by simpa [touchedIn] using hin
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, pageOf, page_cons_le _ _ _ _ hne]
    | replace id' r =>
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, pageOf, page_map_replace]
    | remove id' =>
      have hne : ¬ a < id' := by simpa [touchedIn] using hin
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, pageOf, page_filter_remove _ _ _ hne]
    | schema sc =>
      simp only [fetch, applyWorld, upd_same]
      cases w t <;> simp [Delta.apply, pageOf]

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

/-- A decision waiting to be committed: the intent, its delta, and the generation the runtime
    decided it at. The commit carries `expected`; the store compares it in the write transaction. -/
structure Pending where
  ctx : Ctx
  intent : Intent
  delta : Delta
  expected : Nat

structure Sys (σ : Type) where
  cache : Cache
  store : Store
  /-- The generation of each tenant as the runtime last learned it. -/
  known : TenantId → Nat
  pending : Option Pending
  pst : σ

/-- Forget what the runtime holds for `x` and learn `x`'s generation. -/
def refresh (sys : Sys σ) (x : TenantId) : Sys σ :=
  { sys with cache := { sys.cache with entries := dropTenant sys.cache.entries x },
             known := upd sys.known x (sys.store.gen x) }

/-- A generation read: nothing when the runtime is current for `x`, a refresh when not. -/
def sync (sys : Sys σ) (x : TenantId) : Sys σ :=
  if sys.known x = sys.store.gen x then sys else refresh sys x

inductive Op where
  /-- A read served from what the runtime knows (leased mode). -/
  | read (k : Key)
  /-- A read preceded by a generation read (checked mode). -/
  | checkedRead (k : Key)
  | pin (k : Key)
  | unpin (k : Key)
  | evict (ks : List Key)
  | invalidate (k : Key)
  | resize (b : Nat)
  /-- Decide an intent on what the working set serves; nothing is written. -/
  | decide (i : Intent) (c : Ctx)
  /-- Send the pending decision to the store, guarded by the generation it was decided at. -/
  | submit
  /-- The same, when the receipt is lost: the store does what it does, the runtime learns nothing. -/
  | submitLost
  /-- A write the runtime is told about (a fenced restore, an anonymisation in this process). -/
  | external (x : TenantId) (t : Option Tenant)
  /-- A write the runtime is not told about. -/
  | unfenced (x : TenantId) (t : Option Tenant)
  /-- A generation read. -/
  | check (x : TenantId)

inductive Outcome where
  | value (p : Payload)
  | result (r : Except Err Unit)
  /-- The store refused the commit: the generation moved since the decision. -/
  | conflict
  | unknown
  | done

/-- A read: a resident entry stamped with the known generation, else a load, which also tells the
    runtime the tenant's generation. -/
def readWS (sys : Sys σ) (k : Key) : Sys σ × Outcome :=
  match hit sys.cache.entries sys.known k with
  | some e => ({ sys with pst := pol.touch sys.pst k true }, .value e.val)
  | none =>
    ({ sync sys k.tenant with
        cache := admit pol size sys.pst (sync sys k.tenant).cache k (sys.store.gen k.tenant)
          (fetch sys.store.world k),
        pst := pol.touch sys.pst k false },
      .value (fetch sys.store.world k))

/-- Decide on what the working set serves, at the generation the runtime knows. An acceptance is
    left for the guarded commit to confirm; a refusal is confirmed by a generation read first, and
    decided again on fresh state when the runtime was behind. -/
def decideWS (sys : Sys σ) (i : Intent) (c : Ctx) : Sys σ × Outcome :=
  match decideOn (serve sys.cache.entries sys.known sys.store) c.tenant c.principal i with
  | .ok d => ({ sys with pending := some ⟨c, i, d, sys.known c.tenant⟩ }, .done)
  | .error e =>
    if sys.known c.tenant = sys.store.gen c.tenant then
      ({ sys with pending := none }, .result (.error e))
    else
      match decideOn (serve (sync sys c.tenant).cache.entries (sync sys c.tenant).known
          (sync sys c.tenant).store) c.tenant c.principal i with
      | .ok d => ({ sync sys c.tenant with pending := some ⟨c, i, d, sys.store.gen c.tenant⟩ }, .done)
      | .error e' => ({ sync sys c.tenant with pending := none }, .result (.error e'))

/-- The working set after the store accepted `p`: invalidated entries go, current ones of the
    tenant are confirmed at the receipt's generation, a pinned header the delta changed comes back. -/
def acceptCache (sys : Sys σ) (p : Pending) : Cache :=
  if sys.cache.pins.contains (.header p.ctx.tenant) && inv p.delta (.header p.ctx.tenant) then
    admit pol size sys.pst
      { sys.cache with
        entries := carry inv p.ctx.tenant p.delta (sys.store.gen p.ctx.tenant) sys.cache.entries }
      (.header p.ctx.tenant) (sys.store.gen p.ctx.tenant + 1)
      (fetch (applyWorld sys.store.world p.ctx.tenant p.delta) (.header p.ctx.tenant))
  else
    { sys.cache with
      entries := carry inv p.ctx.tenant p.delta (sys.store.gen p.ctx.tenant) sys.cache.entries }

/-- The guarded commit: applied only when the tenant is still at the generation the decision was
    made at, in the same step that raises it (the receipt's generation is `expected + 1`);
    otherwise refused, and the runtime refreshes. -/
def submitWS (sys : Sys σ) : Sys σ × Outcome :=
  match sys.pending with
  | none => (sys, .done)
  | some p =>
    if sys.store.gen p.ctx.tenant = p.expected then
      ({ sys with cache := acceptCache pol size inv sys p,
                  store := sys.store.applyDelta p.ctx.tenant p.delta,
                  known := upd sys.known p.ctx.tenant (sys.store.gen p.ctx.tenant + 1),
                  pending := none }, .result (.ok ()))
    else ({ sync sys p.ctx.tenant with pending := none }, .conflict)

/-- A commit whose receipt is lost: the runtime drops the tenant's entries and keeps what it knew. -/
def submitLostWS (sys : Sys σ) : Sys σ × Outcome :=
  match sys.pending with
  | none => (sys, .done)
  | some p =>
    ({ (submitWS pol size inv sys).1 with
        cache := { (submitWS pol size inv sys).1.cache with
          entries := dropTenant (submitWS pol size inv sys).1.cache.entries p.ctx.tenant },
        known := sys.known }, .unknown)

def stepWS (sys : Sys σ) : Op → Sys σ × Outcome
  | .read k => readWS pol size sys k
  | .checkedRead k => readWS pol size (sync sys k.tenant) k
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
  | .decide i c => decideWS sys i c
  | .submit => submitWS pol size inv sys
  | .submitLost => submitLostWS pol size inv sys
  | .external x t => (refresh { sys with store := sys.store.set x t } x, .done)
  | .unfenced x t => ({ sys with store := sys.store.set x t }, .done)
  | .check x => (sync sys x, .done)

/-- Which tenants may have been written behind the runtime's back since it last read their
    generation. Only the environment knows this; the runtime does not. -/
def nextDirty (dirty : TenantId → Bool) : Op → TenantId → Bool
  | .unfenced x _ => upd dirty x true
  | .check x => upd dirty x false
  | .checkedRead k => upd dirty k.tenant false
  | .external x _ => upd dirty x false
  | _ => dirty

def readOk (dirty : TenantId → Bool) : Op → Bool
  | .read k => !dirty k.tenant
  | _ => true

/-- No unchecked read is served while its tenant may have been written behind the runtime's back:
    the environment condition a lease is there to provide. -/
def covered : (TenantId → Bool) → List Op → Bool
  | _, [] => true
  | dirty, op :: ops => readOk dirty op && covered (nextDirty dirty op) ops

/-! ### Invariants -/

/-- The store's obligation (W4) as a relation between two of its states: generations never fall,
    and a tenant whose generation did not change has the same state. -/
def Advances (s s' : Store) : Prop :=
  ∀ y, s.gen y ≤ s'.gen y ∧ (s'.gen y = s.gen y → s'.world y = s.world y)

def KnownLe (known : TenantId → Nat) (s : Store) : Prop := ∀ y, known y ≤ s.gen y

/-- A pending decision was made at a generation the tenant has reached, and if the tenant is still
    there, it is the kernel's decision on the tenant's current state. -/
def PendingOk (pd : Option Pending) (s : Store) : Prop :=
  ∀ p, pd = some p → p.expected ≤ s.gen p.ctx.tenant ∧
    (p.expected = s.gen p.ctx.tenant →
      decideOn (fetch s.world) p.ctx.tenant p.ctx.principal p.intent = .ok p.delta)

def Inv (sys : Sys σ) : Prop :=
  Sound sys.cache.entries sys.store ∧ KnownLe sys.known sys.store ∧ PendingOk sys.pending sys.store

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

/-! ### The store's obligation -/

theorem advances_refl (s : Store) : Advances s s := fun _ => ⟨Nat.le_refl _, fun _ => rfl⟩

theorem advances_set (s : Store) (x : TenantId) (t : Option Tenant) : Advances s (s.set x t) := by
  intro y
  by_cases hy : y = x
  · subst hy; simp [Store.set]
  · simp [Store.set, hy]

theorem advances_apply (s : Store) (x : TenantId) (d : Delta) : Advances s (s.applyDelta x d) := by
  intro y
  by_cases hy : y = x
  · subst hy; simp [Store.applyDelta]
  · simp [Store.applyDelta, applyWorld, hy]

theorem sound_advance (es : List Entry) (s s' : Store) (hs : Sound es s) (ha : Advances s s') :
    Sound es s' := by
  intro e he
  obtain ⟨hle, hcur⟩ := hs e he
  obtain ⟨hg, hw⟩ := ha e.key.tenant
  refine ⟨Nat.le_trans hle hg, fun h => ?_⟩
  have heq : s'.gen e.key.tenant = s.gen e.key.tenant := by omega
  rw [fetch_congr s'.world s.world e.key (hw heq)]
  exact hcur (by omega)

theorem knownLe_advance (known : TenantId → Nat) (s s' : Store) (h : KnownLe known s)
    (ha : Advances s s') : KnownLe known s' := fun y => Nat.le_trans (h y) (ha y).1

theorem pendingOk_advance (pd : Option Pending) (s s' : Store) (hp : PendingOk pd s)
    (ha : Advances s s') : PendingOk pd s' := by
  intro p hpd
  obtain ⟨hle, hdec⟩ := hp p hpd
  obtain ⟨hg, hw⟩ := ha p.ctx.tenant
  refine ⟨Nat.le_trans hle hg, fun h => ?_⟩
  have heq : s'.gen p.ctx.tenant = s.gen p.ctx.tenant := by omega
  rw [decideOn_congr (fetch s'.world) (fetch s.world) _ _ _
    (fun k hk => fetch_congr _ _ k (by rw [hk]; exact hw heq))]
  exact hdec (by omega)

theorem serve_decide_current (es : List Entry) (known : TenantId → Nat) (s : Store) (x : TenantId)
    (p : PrincipalId) (i : Intent) (hs : Sound es s) (hk : known x = s.gen x) :
    decideOn (serve es known s) x p i = decideOn (fetch s.world) x p i :=
  decideOn_congr _ _ _ _ _ (fun k hkx =>
    serve_eq_fetch _ _ _ _ (by rw [hkx]; exact safe_of_current _ _ _ _ hs hk))

theorem sync_pending (sys : Sys σ) (x : TenantId) : (sync sys x).pending = sys.pending := by
  unfold sync; split <;> rfl

theorem sync_knownLe (sys : Sys σ) (x : TenantId) (h : KnownLe sys.known sys.store) :
    KnownLe (sync sys x).known (sync sys x).store := by
  rw [sync_store]
  unfold sync; split
  · exact h
  · intro y
    by_cases hy : y = x
    · subst hy; simp [refresh]
    · simpa [refresh, hy] using h y

theorem sync_inv (sys : Sys σ) (x : TenantId) (h : Inv sys) : Inv (sync sys x) :=
  ⟨sync_sound sys x h.1, sync_knownLe sys x h.2.1, by rw [sync_pending, sync_store]; exact h.2.2⟩

/-! ### Reads -/

theorem readWS_store (sys : Sys σ) (k : Key) : (readWS pol size sys k).1.store = sys.store := by
  unfold readWS; split
  · rfl
  · exact sync_store sys k.tenant

theorem readWS_pending (sys : Sys σ) (k : Key) : (readWS pol size sys k).1.pending = sys.pending := by
  unfold readWS; split
  · rfl
  · exact sync_pending sys k.tenant

theorem readWS_pins (sys : Sys σ) (k : Key) :
    (readWS pol size sys k).1.cache.pins = sys.cache.pins := by
  unfold readWS; split
  · rfl
  · show (admit pol size _ _ _ _ _).pins = _
    rw [admit_pins]; exact sync_pins sys k.tenant

theorem readWS_within (sys : Sys σ) (k : Key) (hw : Within size sys.cache) :
    Within size (readWS pol size sys k).1.cache := by
  unfold readWS; split
  · exact hw
  · exact admit_within pol size _ _ _ _ _ (sync_within size sys _ hw)

theorem readWS_inv (sys : Sys σ) (k : Key) (h : Inv sys) : Inv (readWS pol size sys k).1 := by
  refine ⟨?_, ?_, ?_⟩
  · rw [readWS_store]
    unfold readWS; split
    · exact h.1
    · have h0 := sync_sound sys k.tenant h.1
      rw [sync_store] at h0
      exact admit_sound pol size _ _ sys.store k _ _ rfl rfl h0
  · rw [readWS_store]
    unfold readWS; split
    · exact h.2.1
    · have := sync_knownLe sys k.tenant h.2.1
      rw [sync_store] at this
      exact this
  · rw [readWS_store, readWS_pending]; exact h.2.2

theorem readWS_safe (sys : Sys σ) (k : Key) (hs : Sound sys.cache.entries sys.store) (y : TenantId)
    (hy : SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (readWS pol size sys k).1.cache.entries (readWS pol size sys k).1.known
      (readWS pol size sys k).1.store y := by
  rw [readWS_store]
  unfold readWS; split
  · exact hy
  · have hsy : SafeAt (sync sys k.tenant).cache.entries (sync sys k.tenant).known sys.store y := by
      by_cases hyx : y = k.tenant
      · subst hyx; have := sync_safe_self sys k.tenant hs; rwa [sync_store] at this
      · have := sync_safe_other sys k.tenant y hyx hy; rwa [sync_store] at this
    exact admit_safe pol size _ _ _ _ _ _ _ hsy

/-- A read whose tenant is safe returns the store's value. -/
theorem readWS_value (sys : Sys σ) (k : Key) (hy : SafeAt sys.cache.entries sys.known sys.store k.tenant) :
    (readWS pol size sys k).2 = .value (fetch sys.store.world k) := by
  unfold readWS; split
  · next e h =>
    have := serve_eq_fetch _ _ _ k hy
    simp only [serve, h] at this
    simp [this]
  · rfl

/-- **Checked reads linearize at their generation read**: whatever the runtime held and however
    stale it was, a checked read returns the store's value at that moment. -/
theorem checkedRead_linearizes (sys : Sys σ) (k : Key) (hs : Sound sys.cache.entries sys.store) :
    (stepWS pol size inv sys (.checkedRead k)).2 = .value (fetch sys.store.world k) := by
  show (readWS pol size (sync sys k.tenant) k).2 = _
  rw [readWS_value pol size (sync sys k.tenant) k (sync_safe_self sys k.tenant hs), sync_store]

/-! ### Decisions -/

theorem decideWS_parts (sys : Sys σ) (i : Intent) (c : Ctx) :
    (decideWS sys i c).1.store = sys.store ∧
    (((decideWS sys i c).1.cache = sys.cache ∧ (decideWS sys i c).1.known = sys.known) ∨
     ((decideWS sys i c).1.cache = (sync sys c.tenant).cache ∧
      (decideWS sys i c).1.known = (sync sys c.tenant).known)) := by
  unfold decideWS; split
  · exact ⟨rfl, Or.inl ⟨rfl, rfl⟩⟩
  · split
    · exact ⟨rfl, Or.inl ⟨rfl, rfl⟩⟩
    · split
      · exact ⟨sync_store _ _, Or.inr ⟨rfl, rfl⟩⟩
      · exact ⟨sync_store _ _, Or.inr ⟨rfl, rfl⟩⟩

theorem decideWS_pendingOk (sys : Sys σ) (i : Intent) (c : Ctx) (h : Inv sys) :
    PendingOk (decideWS sys i c).1.pending sys.store := by
  unfold decideWS
  split
  · next d hd =>
    intro p hp
    simp only [Option.some.injEq] at hp
    subst hp
    refine ⟨h.2.1 c.tenant, fun heq => ?_⟩
    show decideOn (fetch sys.store.world) c.tenant c.principal i = .ok d
    rw [← serve_decide_current _ _ _ _ _ _ h.1 heq]; exact hd
  · split
    · intro p hp; simp at hp
    · split
      · next d hd =>
        intro p hp
        simp only [Option.some.injEq] at hp
        subst hp
        refine ⟨Nat.le_refl _, fun _ => ?_⟩
        show decideOn (fetch sys.store.world) c.tenant c.principal i = .ok d
        rw [← commit_decision sys c i h.1]; exact hd
      · intro p hp; simp at hp

theorem decideWS_inv (sys : Sys σ) (i : Intent) (c : Ctx) (h : Inv sys) :
    Inv (decideWS sys i c).1 := by
  obtain ⟨hst, hck⟩ := decideWS_parts sys i c
  refine ⟨?_, ?_, ?_⟩
  · rw [hst]
    rcases hck with ⟨hc, _⟩ | ⟨hc, _⟩
    · rw [hc]; exact h.1
    · rw [hc]; have := sync_sound sys c.tenant h.1; rwa [sync_store] at this
  · rw [hst]
    rcases hck with ⟨_, hk⟩ | ⟨_, hk⟩
    · rw [hk]; exact h.2.1
    · rw [hk]; have := sync_knownLe sys c.tenant h.2.1; rwa [sync_store] at this
  · rw [hst]; exact decideWS_pendingOk sys i c h

theorem decideWS_safe (sys : Sys σ) (i : Intent) (c : Ctx) (hs : Sound sys.cache.entries sys.store)
    (y : TenantId) (hy : SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (decideWS sys i c).1.cache.entries (decideWS sys i c).1.known
      (decideWS sys i c).1.store y := by
  obtain ⟨hst, hck⟩ := decideWS_parts sys i c
  rw [hst]
  rcases hck with ⟨hc, hk⟩ | ⟨hc, hk⟩
  · rw [hc, hk]; exact hy
  · rw [hc, hk]
    by_cases hyx : y = c.tenant
    · subst hyx; have := sync_safe_self sys c.tenant hs; rwa [sync_store] at this
    · have := sync_safe_other sys c.tenant y hyx hy; rwa [sync_store] at this

/-- **A refusal is the kernel's refusal on the current state**: it is confirmed by a generation
    read, and decided again on fresh state when the runtime was behind. -/
theorem decideWS_refusal (sys : Sys σ) (i : Intent) (c : Ctx) (e : Err)
    (hs : Sound sys.cache.entries sys.store)
    (hr : (decideWS sys i c).2 = .result (.error e)) : step sys.store.world i c = .error e := by
  unfold decideWS at hr
  split at hr
  · simp at hr
  · next e0 he0 =>
    split at hr
    · next hcur =>
      have hre : e0 = e := by simpa using hr
      subst hre
      apply decide_error_step
      rw [← serve_decide_current _ _ _ _ _ _ hs hcur]; exact he0
    · split at hr
      · simp at hr
      · next e1 he1 =>
        have hre : e1 = e := by simpa using hr
        subst hre
        apply decide_error_step
        rw [← commit_decision sys c i hs]; exact he1

/-! ### The guarded commit -/

theorem acceptCache_sound (sys : Sys σ) (p : Pending) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) :
    Sound (acceptCache pol size inv sys p).entries (sys.store.applyDelta p.ctx.tenant p.delta) := by
  have h1 := carry_sound inv sys.cache.entries sys.store p.ctx.tenant p.delta hinv hs
  unfold acceptCache; split
  · exact admit_sound pol size _ { sys.cache with
      entries := carry inv p.ctx.tenant p.delta (sys.store.gen p.ctx.tenant) sys.cache.entries }
      (sys.store.applyDelta p.ctx.tenant p.delta) _ _ _ (by simp [Store.applyDelta, Key.tenant]) rfl h1
  · exact h1

theorem acceptCache_safe (sys : Sys σ) (p : Pending) (hinv : InvSound inv)
    (hs : Sound sys.cache.entries sys.store) (y : TenantId)
    (hy : y ≠ p.ctx.tenant → SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (acceptCache pol size inv sys p).entries
      (upd sys.known p.ctx.tenant (sys.store.gen p.ctx.tenant + 1))
      (sys.store.applyDelta p.ctx.tenant p.delta) y := by
  have h1 := carry_safe inv sys.cache.entries sys.known sys.store p.ctx.tenant p.delta hinv hs y hy
  unfold acceptCache; split
  · exact admit_safe pol size _ _ _ (sys.store.applyDelta p.ctx.tenant p.delta) _ _ _ h1
  · exact h1

theorem acceptCache_within (sys : Sys σ) (p : Pending) (hw : Within size sys.cache) :
    Within size (acceptCache pol size inv sys p) := by
  have h1 : Within size { sys.cache with
      entries := carry inv p.ctx.tenant p.delta (sys.store.gen p.ctx.tenant) sys.cache.entries } :=
    Nat.le_trans (bytes_carry_le size inv _ _ _ _) hw
  unfold acceptCache; split
  · exact admit_within pol size _ _ _ _ _ h1
  · exact h1

theorem acceptCache_pins (sys : Sys σ) (p : Pending) :
    (acceptCache pol size inv sys p).pins = sys.cache.pins := by
  unfold acceptCache; split
  · rw [admit_pins]
  · rfl

theorem submitWS_advances (sys : Sys σ) : Advances sys.store (submitWS pol size inv sys).1.store := by
  unfold submitWS; split
  · exact advances_refl _
  · split
    · exact advances_apply _ _ _
    · show Advances sys.store (sync sys _).store
      rw [sync_store]; exact advances_refl _

theorem submitWS_pending (sys : Sys σ) (p : Pending) (hp : sys.pending = some p) :
    (submitWS pol size inv sys).1.pending = none := by
  unfold submitWS; rw [hp]; simp only []
  split <;> rfl

theorem submitWS_known_other (sys : Sys σ) (p : Pending) (hp : sys.pending = some p) (y : TenantId)
    (hy : y ≠ p.ctx.tenant) : (submitWS pol size inv sys).1.known y = sys.known y := by
  unfold submitWS; rw [hp]; simp only []
  split
  · simp [hy]
  · exact sync_known_other sys _ y hy

theorem submitWS_inv (sys : Sys σ) (hinv : InvSound inv) (h : Inv sys) :
    Inv (submitWS pol size inv sys).1 := by
  unfold submitWS
  split
  · exact h
  · next p hp =>
    split
    · refine ⟨acceptCache_sound pol size inv sys p hinv h.1, ?_, ?_⟩
      · intro y
        by_cases hy : y = p.ctx.tenant
        · subst hy; simp [Store.applyDelta]
        · simpa [Store.applyDelta, hy] using h.2.1 y
      · intro q hq; simp at hq
    · refine ⟨sync_sound sys _ h.1, sync_knownLe sys _ h.2.1, ?_⟩
      intro q hq; simp at hq

theorem submitWS_safe (sys : Sys σ) (hinv : InvSound inv) (h : Inv sys) (y : TenantId)
    (hy : SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (submitWS pol size inv sys).1.cache.entries (submitWS pol size inv sys).1.known
      (submitWS pol size inv sys).1.store y := by
  unfold submitWS
  split
  · exact hy
  · next p hp =>
    split
    · exact acceptCache_safe pol size inv sys p hinv h.1 y (fun _ => hy)
    · by_cases hyx : y = p.ctx.tenant
      · subst hyx; exact sync_safe_self sys _ h.1
      · exact sync_safe_other sys _ y hyx hy

/-- **An accepted commit is a kernel step on the state it was applied to.** The store applies the
    delta only when the tenant is at the generation the decision was made at; then the decision is
    the kernel's on the current state, whatever the runtime held. -/
theorem submitWS_world (sys : Sys σ) (h : Inv sys) :
    (submitWS pol size inv sys).1.store.world = sys.store.world ∨
    ∃ p, sys.pending = some p ∧
      step sys.store.world p.intent p.ctx = .ok (submitWS pol size inv sys).1.store.world := by
  unfold submitWS
  split
  · left; rfl
  · next p hp =>
    split
    · next hg =>
      right
      exact ⟨p, hp, decide_ok_step _ _ _ _ ((h.2.2 p hp).2 hg.symm)⟩
    · left; exact congrArg Store.world (sync_store sys _)

theorem submitWS_within (sys : Sys σ) (hw : Within size sys.cache) :
    Within size (submitWS pol size inv sys).1.cache := by
  unfold submitWS; split
  · exact hw
  · split
    · exact acceptCache_within pol size inv sys _ hw
    · exact sync_within size sys _ hw

theorem submitWS_pins (sys : Sys σ) :
    (submitWS pol size inv sys).1.cache.pins = sys.cache.pins := by
  unfold submitWS; split
  · rfl
  · split
    · exact acceptCache_pins pol size inv sys _
    · exact sync_pins sys _

theorem submitLostWS_parts (sys : Sys σ) :
    (submitLostWS pol size inv sys).1.store = (submitWS pol size inv sys).1.store := by
  unfold submitLostWS; split
  · next hp => unfold submitWS; rw [hp]
  · rfl

theorem submitLostWS_inv (sys : Sys σ) (hinv : InvSound inv) (h : Inv sys) :
    Inv (submitLostWS pol size inv sys).1 := by
  have hr := submitWS_inv pol size inv sys hinv h
  unfold submitLostWS; split
  · exact h
  · next p hp =>
    refine ⟨sound_sub _ _ _ hr.1 (dropTenant_sub _ _),
      knownLe_advance _ _ _ h.2.1 (submitWS_advances pol size inv sys), ?_⟩
    intro q hq
    change (submitWS pol size inv sys).1.pending = some q at hq
    rw [submitWS_pending pol size inv sys p hp] at hq
    cases hq

theorem submitLostWS_safe (sys : Sys σ) (hinv : InvSound inv) (h : Inv sys) (y : TenantId)
    (hy : SafeAt sys.cache.entries sys.known sys.store y) :
    SafeAt (submitLostWS pol size inv sys).1.cache.entries (submitLostWS pol size inv sys).1.known
      (submitLostWS pol size inv sys).1.store y := by
  unfold submitLostWS; split
  · exact hy
  · next p hp =>
    by_cases hyx : y = p.ctx.tenant
    · subst hyx
      intro e he hx _
      exact absurd hx (dropTenant_tenant _ _ e he)
    · have hs := submitWS_safe pol size inv sys hinv h y hy
      intro e he hey hst
      apply hs e (dropTenant_sub _ _ e he) hey
      rw [submitWS_known_other pol size inv sys p hp y hyx]
      exact hst

theorem submitLostWS_within (sys : Sys σ) (hw : Within size sys.cache) :
    Within size (submitLostWS pol size inv sys).1.cache := by
  unfold submitLostWS; split
  · exact hw
  · exact Nat.le_trans (bytes_filter_le size _ _) (submitWS_within pol size inv sys hw)

theorem submitLostWS_pins (sys : Sys σ) :
    (submitLostWS pol size inv sys).1.cache.pins = sys.cache.pins := by
  unfold submitLostWS; split
  · rfl
  · exact submitWS_pins pol size inv sys

/-! ### Every operation -/

theorem stepWS_inv (sys : Sys σ) (op : Op) (hinv : InvSound inv) (h : Inv sys) :
    Inv (stepWS pol size inv sys op).1 := by
  cases op with
  | read k => exact readWS_inv pol size sys k h
  | checkedRead k => exact readWS_inv pol size _ k (sync_inv sys k.tenant h)
  | pin k =>
    exact ⟨admit_sound pol size _ { sys.cache with pins := _ } _ _ _ _ rfl rfl h.1, h.2.1, h.2.2⟩
  | unpin k => exact h
  | evict ks => exact ⟨sound_sub _ _ _ h.1 (dropKeys_sub _ _ _), h.2.1, h.2.2⟩
  | invalidate k => exact ⟨sound_sub _ _ _ h.1 (removeKey_sub _ _), h.2.1, h.2.2⟩
  | resize b => exact ⟨sound_sub _ _ _ h.1 (resize_sub pol size _ _ _), h.2.1, h.2.2⟩
  | decide i c => exact decideWS_inv sys i c h
  | submit => exact submitWS_inv pol size inv sys hinv h
  | submitLost => exact submitLostWS_inv pol size inv sys hinv h
  | external x t =>
    show Inv (refresh { sys with store := sys.store.set x t } x)
    refine ⟨sound_sub _ _ _ (sound_set _ _ x t h.1) (dropTenant_sub _ _), ?_,
      pendingOk_advance _ _ _ h.2.2 (advances_set _ x t)⟩
    intro y
    by_cases hy : y = x
    · subst hy; simp [refresh]
    · have := knownLe_advance _ _ _ h.2.1 (advances_set sys.store x t) y
      simpa [refresh, hy] using this
  | unfenced x t =>
    exact ⟨sound_set _ _ x t h.1, knownLe_advance _ _ _ h.2.1 (advances_set _ x t),
      pendingOk_advance _ _ _ h.2.2 (advances_set _ x t)⟩
  | check x => exact sync_inv sys x h

theorem stepWS_within (sys : Sys σ) (op : Op) (hw : Within size sys.cache) :
    Within size (stepWS pol size inv sys op).1.cache := by
  cases op with
  | read k => exact readWS_within pol size sys k hw
  | checkedRead k => exact readWS_within pol size _ k (sync_within size sys _ hw)
  | pin k => exact admit_within pol size _ { sys.cache with pins := _ } _ _ _ hw
  | unpin k => exact hw
  | evict ks => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | invalidate k => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | resize b => exact resize_within pol size _ _ _
  | decide i c =>
    rcases (decideWS_parts sys i c).2 with ⟨hc, _⟩ | ⟨hc, _⟩
    · show Within size (decideWS sys i c).1.cache; rw [hc]; exact hw
    · show Within size (decideWS sys i c).1.cache; rw [hc]; exact sync_within size sys _ hw
  | submit => exact submitWS_within pol size inv sys hw
  | submitLost => exact submitLostWS_within pol size inv sys hw
  | external x t => exact Nat.le_trans (bytes_filter_le size _ _) hw
  | unfenced x t => exact hw
  | check x => exact sync_within size sys x hw

/-- **Pins change only when asked.** -/
theorem stepWS_pins (sys : Sys σ) (op : Op) :
    (stepWS pol size inv sys op).1.cache.pins =
      match op with
      | .pin k => k :: sys.cache.pins
      | .unpin k => sys.cache.pins.filter (· != k)
      | _ => sys.cache.pins := by
  cases op with
  | read k => exact readWS_pins pol size sys k
  | checkedRead k =>
    show (readWS pol size _ k).1.cache.pins = _
    rw [readWS_pins]; exact sync_pins sys _
  | pin k => exact admit_pins pol size _ _ _ _ _
  | unpin k => rfl
  | evict ks => rfl
  | invalidate k => rfl
  | resize b => rfl
  | decide i c =>
    show (decideWS sys i c).1.cache.pins = _
    rcases (decideWS_parts sys i c).2 with ⟨hc, _⟩ | ⟨hc, _⟩
    · rw [hc]
    · rw [hc]; exact sync_pins sys _
  | submit => exact submitWS_pins pol size inv sys
  | submitLost => exact submitLostWS_pins pol size inv sys
  | external x t => rfl
  | unfenced x t => rfl
  | check x => exact sync_pins sys x

/-- Safety of unchecked reads across one operation: tenants the environment did not write behind
    the runtime's back stay safe, and a check makes its tenant safe. -/
theorem stepWS_safe (sys : Sys σ) (op : Op) (hinv : InvSound inv) (h : Inv sys)
    (dirty : TenantId → Bool)
    (hsafe : ∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) :
    ∀ y, nextDirty dirty op y = false →
      SafeAt (stepWS pol size inv sys op).1.cache.entries (stepWS pol size inv sys op).1.known
        (stepWS pol size inv sys op).1.store y := by
  intro y hy
  cases op with
  | read k => exact readWS_safe pol size sys k h.1 y (hsafe y hy)
  | checkedRead k =>
    have h1 : SafeAt (sync sys k.tenant).cache.entries (sync sys k.tenant).known
        (sync sys k.tenant).store y := by
      by_cases hyx : y = k.tenant
      · subst hyx; exact sync_safe_self sys _ h.1
      · exact sync_safe_other sys _ y hyx (hsafe y (by simpa [nextDirty, upd, hyx] using hy))
    exact readWS_safe pol size _ k (sync_sound sys _ h.1) y h1
  | pin k => exact admit_safe pol size _ { sys.cache with pins := _ } _ _ _ _ _ (hsafe y hy)
  | unpin k => exact hsafe y hy
  | evict ks => exact safe_sub _ _ _ _ _ (hsafe y hy) (dropKeys_sub _ _ _)
  | invalidate k => exact safe_sub _ _ _ _ _ (hsafe y hy) (removeKey_sub _ _)
  | resize b => exact safe_sub _ _ _ _ _ (hsafe y hy) (resize_sub pol size _ _ _)
  | decide i c => exact decideWS_safe sys i c h.1 y (hsafe y hy)
  | submit => exact submitWS_safe pol size inv sys hinv h y (hsafe y hy)
  | submitLost => exact submitLostWS_safe pol size inv sys hinv h y (hsafe y hy)
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
    · subst hyx; exact sync_safe_self sys _ h.1
    · exact sync_safe_other sys x y hyx (hsafe y (by simpa [nextDirty, upd, hyx] using hy))

/-- What each operation guarantees. A safe read returns the store's value; a checked read always
    does; a refusal is the kernel's; and the committed world changes only by a kernel step of a
    decided intent or by another writer's write. -/
def StepFaithful (sys : Sys σ) (op : Op) (r : Sys σ × Outcome) : Prop :=
  (∀ k, op = .read k → SafeAt sys.cache.entries sys.known sys.store k.tenant →
    r.2 = .value (fetch sys.store.world k)) ∧
  (∀ k, op = .checkedRead k → r.2 = .value (fetch sys.store.world k)) ∧
  (∀ i c e, op = .decide i c → r.2 = .result (.error e) → step sys.store.world i c = .error e) ∧
  (r.1.store.world = sys.store.world ∨
    (∃ p, sys.pending = some p ∧ step sys.store.world p.intent p.ctx = .ok r.1.store.world) ∨
    (∃ x t, (op = .external x t ∨ op = .unfenced x t) ∧ r.1.store.world = upd sys.store.world x t))

theorem stepWS_faithful (sys : Sys σ) (op : Op) (h : Inv sys) :
    StepFaithful sys op (stepWS pol size inv sys op) := by
  cases op with
  | read k =>
    refine ⟨fun k' hk hy => ?_, fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk), ?_⟩
    · cases hk; exact readWS_value pol size sys k hy
    · left; exact congrArg Store.world (readWS_store pol size sys k)
  | checkedRead k =>
    refine ⟨fun _ hk => (by cases hk), fun k' hk => ?_, fun _ _ _ hk => (by cases hk), ?_⟩
    · cases hk; exact checkedRead_linearizes pol size inv sys k h.1
    · left
      show (readWS pol size (sync sys k.tenant) k).1.store.world = _
      rw [readWS_store, sync_store]
  | pin k => exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk),
      Or.inl rfl⟩
  | unpin k => exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk),
      Or.inl rfl⟩
  | evict ks => exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk),
      fun _ _ _ hk => (by cases hk), Or.inl rfl⟩
  | invalidate k => exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk),
      fun _ _ _ hk => (by cases hk), Or.inl rfl⟩
  | resize b => exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk),
      fun _ _ _ hk => (by cases hk), Or.inl rfl⟩
  | decide i c =>
    refine ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun i' c' e hk hr => ?_, ?_⟩
    · cases hk; exact decideWS_refusal sys i c e h.1 hr
    · left; exact congrArg Store.world (decideWS_parts sys i c).1
  | submit =>
    refine ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk), ?_⟩
    rcases submitWS_world pol size inv sys h with hw | hw
    · exact Or.inl hw
    · exact Or.inr (Or.inl hw)
  | submitLost =>
    refine ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk), ?_⟩
    show (submitLostWS pol size inv sys).1.store.world = _ ∨
      (∃ p, sys.pending = some p ∧
        step sys.store.world p.intent p.ctx = .ok (submitLostWS pol size inv sys).1.store.world) ∨ _
    rw [submitLostWS_parts]
    rcases submitWS_world pol size inv sys h with hw | hw
    · exact Or.inl hw
    · exact Or.inr (Or.inl hw)
  | external x t =>
    exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk),
      Or.inr (Or.inr ⟨x, t, Or.inl rfl, rfl⟩)⟩
  | unfenced x t =>
    exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk),
      Or.inr (Or.inr ⟨x, t, Or.inr rfl, rfl⟩)⟩
  | check x =>
    exact ⟨fun _ hk => (by cases hk), fun _ hk => (by cases hk), fun _ _ _ hk => (by cases hk),
      Or.inl (congrArg Store.world (sync_store sys x))⟩

/-! ### Whole schedules -/

def RunFaithful : Sys σ → List Op → Prop
  | _, [] => True
  | sys, op :: ops =>
    StepFaithful sys op (stepWS pol size inv sys op) ∧ RunFaithful (stepWS pol size inv sys op).1 ops

/-- **Every schedule** (any policy, any invalidation map meeting the criterion, any interleaving of
    reads, decisions, guarded commits, lost receipts, fenced and unfenced writes, given W4): at
    every step, a checked read returns the store's value, a refusal is the kernel's, and the
    committed world changes only by a kernel step of a decided intent on the state it is applied to,
    or by another writer. -/
theorem runWS_faithful (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ), Inv sys → RunFaithful pol size inv sys ops := by
  induction ops with
  | nil => intro _ _; trivial
  | cons op ops ih =>
    intro sys h
    exact ⟨stepWS_faithful pol size inv sys op h, ih _ (stepWS_inv pol size inv sys op hinv h)⟩

def ReadsTrue : Sys σ → List Op → Prop
  | _, [] => True
  | sys, op :: ops =>
    (∀ k, op = .read k → (stepWS pol size inv sys op).2 = .value (fetch sys.store.world k)) ∧
    ReadsTrue (stepWS pol size inv sys op).1 ops

/-- **Unchecked reads**, in a covered schedule (no unchecked read of a tenant written behind the
    runtime's back since its last generation read), return the store's value at every step. -/
theorem runWS_reads (hinv : InvSound inv) (ops : List Op) :
    ∀ (sys : Sys σ) (dirty : TenantId → Bool), Inv sys →
      (∀ y, dirty y = false → SafeAt sys.cache.entries sys.known sys.store y) →
      covered dirty ops = true → ReadsTrue pol size inv sys ops := by
  induction ops with
  | nil => intro _ _ _ _ _; trivial
  | cons op ops ih =>
    intro sys dirty h hsafe hcov
    simp only [covered, Bool.and_eq_true] at hcov
    refine ⟨fun k hk => ?_, ih _ _ (stepWS_inv pol size inv sys op hinv h)
      (stepWS_safe pol size inv sys op hinv h dirty hsafe) hcov.2⟩
    subst hk
    have hd : dirty k.tenant = false := by simpa [readOk] using hcov.1
    exact readWS_value pol size sys k (hsafe _ hd)

/-- The working set after a schedule. -/
def finalWS : Sys σ → List Op → Cache
  | sys, [] => sys.cache
  | sys, op :: ops => finalWS (stepWS pol size inv sys op).1 ops

theorem runWS_within (ops : List Op) :
    ∀ (sys : Sys σ), Within size sys.cache → Within size (finalWS pol size inv sys ops) := by
  induction ops with
  | nil => intro sys hw; exact hw
  | cons op ops ih =>
    intro sys hw
    exact ih _ (stepWS_within pol size inv sys op hw)

theorem empty_sound (s : Store) : Sound [] s := fun _ h => absurd h List.not_mem_nil

theorem empty_safe (known : TenantId → Nat) (s : Store) (y : TenantId) : SafeAt [] known s y :=
  fun _ h => absurd h List.not_mem_nil

/-- The world of the counterexamples: tenant 1 exists, empty. -/
def cexTenant : Tenant :=
  { schema := [], owner := 0, editors := [], writers := [], records := [], nextId := 1 }

def cexWorld : World := fun x => if x = 1 then some cexTenant else none

/-- A runtime holding tenant 1's header, current at generation 0. -/
def cexSys : Sys Unit :=
  { cache := { entries := [{ key := .header 1, stamp := 0, val := fetch cexWorld (.header 1) }],
               pins := [], budget := 0 },
    store := { world := cexWorld, gen := fun _ => 0 },
    known := fun _ => 0,
    pending := none,
    pst := () }

/-- **Why leased reads need fencing.** An unchecked read after a write behind the runtime's back
    serves the value from before the write. -/
theorem unfenced_breaks_reads (pol : Policy Unit) (size : Key → Payload → Nat)
    (inv : Delta → Key → Bool) :
    Inv cexSys ∧
    (stepWS pol size inv (stepWS pol size inv cexSys (.unfenced 1 none)).1 (.read (.header 1))).2 ≠
      .value (fetch (cexSys.store.set 1 none).world (.header 1)) := by
  refine ⟨⟨?_, fun _ => Nat.le_refl _, fun _ hp => by simp [cexSys] at hp⟩, ?_⟩
  · intro e' he'
    simp only [cexSys, List.mem_singleton] at he'
    subst he'
    exact ⟨Nat.le_refl _, fun _ => rfl⟩
  · simp [stepWS, readWS, Store.set, hit, upd, fetch, cexSys, cexWorld, cexTenant]

/-- The store state after a restore that broke W4: the world changed, the generation did not. -/
def cexRolledBack : Store := { world := upd cexWorld 1 none, gen := fun _ => 0 }

/-- **W4 is load-bearing.** A store change that leaves the generation as it was (a restore that
    brought an old generation back) is not an advance, and after it even a checked read serves the
    old value. -/
theorem w4_violation_breaks_checked_read (pol : Policy Unit) (size : Key → Payload → Nat)
    (inv : Delta → Key → Bool) :
    ¬ Advances cexSys.store cexRolledBack ∧
    (stepWS pol size inv { cexSys with store := cexRolledBack } (.checkedRead (.header 1))).2 ≠
      .value (fetch cexRolledBack.world (.header 1)) := by
  refine ⟨fun ha => ?_, ?_⟩
  · have := (ha 1).2 rfl
    simp [cexRolledBack, cexSys, cexWorld, upd] at this
  · simp [stepWS, readWS, sync, hit, upd, fetch, cexSys, cexRolledBack, cexWorld, cexTenant]

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

/-! ## Erasure: where the working set sits against sealing

A small model of its own. A personal value is stored sealed under one subject key; the keyring says
which keys still open; destroying a key changes no stored value and no generation (D-350). The
working set holds either the sealed values (below sealing, opened on every read) or the values
opened when they were admitted (above sealing). -/

abbrev Keyring := Nat → Bool

def destroy (kr : Keyring) (n : Nat) : Keyring := fun m => if m = n then false else kr m

structure SealedVal where
  key : Nat
  plain : Payload

/-- What opening gives: the plain value while its key lives, the erased marker after. -/
def openWith (kr : Keyring) (v : SealedVal) : Payload := if kr v.key then v.plain else .record none

structure SealedStore where
  vals : Key → SealedVal
  keys : Keyring

def destroyKey (s : SealedStore) (n : Nat) : SealedStore := { s with keys := destroy s.keys n }

inductive SOp where
  | admit (k : Key)
  | destroy (n : Nat)
  | read (k : Key)

/-- Below sealing: entries are sealed values, opened with the keys live at the read. -/
def runBelow : List (Key × SealedVal) → SealedStore → List SOp → List Payload
  | _, _, [] => []
  | c, s, .admit k :: ops => runBelow ((k, s.vals k) :: c) s ops
  | c, s, .destroy n :: ops => runBelow c (destroyKey s n) ops
  | c, s, .read k :: ops => openWith s.keys ((c.lookup k).getD (s.vals k)) :: runBelow c s ops

/-- Above sealing: entries are the values opened at admission. -/
def runAbove : List (Key × Payload) → SealedStore → List SOp → List Payload
  | _, _, [] => []
  | c, s, .admit k :: ops => runAbove ((k, openWith s.keys (s.vals k)) :: c) s ops
  | c, s, .destroy n :: ops => runAbove c (destroyKey s n) ops
  | c, s, .read k :: ops => (c.lookup k).getD (openWith s.keys (s.vals k)) :: runAbove c s ops

/-- What a read should return: the stored value opened with the keys live now. -/
def runTruth : SealedStore → List SOp → List Payload
  | _, [] => []
  | s, .admit _ :: ops => runTruth s ops
  | s, .destroy n :: ops => runTruth (destroyKey s n) ops
  | s, .read k :: ops => openWith s.keys (s.vals k) :: runTruth s ops

/-- The below-sealing working set holds what the store holds. -/
def BelowOk (c : List (Key × SealedVal)) (s : SealedStore) : Prop :=
  ∀ k v, c.lookup k = some v → v = s.vals k

/-- **Below sealing, erasure needs nothing from the working set**: for every schedule of
    admissions, key destructions and reads, every read is the stored value opened with the keys
    live at that read. -/
theorem below_sealing_erasure_safe (ops : List SOp) :
    ∀ (c : List (Key × SealedVal)) (s : SealedStore), BelowOk c s → runBelow c s ops = runTruth s ops := by
  induction ops with
  | nil => intro _ _ _; rfl
  | cons op ops ih =>
    intro c s hc
    cases op with
    | admit k =>
      apply ih
      intro k' v hl
      by_cases hk : k' = k
      · subst hk; simp [List.lookup] at hl; exact hl.symm
      · have : (k' == k) = false := by simp [hk]
        simp [List.lookup, this] at hl
        exact hc k' v hl
    | destroy n => exact ih c _ (fun k v hl => hc k v hl)
    | read k =>
      simp only [runBelow, runTruth]
      congr 1
      · cases hl : c.lookup k with
        | none => rfl
        | some v => simp [hc k v hl]
      · exact ih c s hc

/-- **Above sealing, a destroyed key goes on opening**: a value admitted while its key lived is
    served as it was after the key is destroyed, where the store now gives the erased marker. -/
theorem above_sealing_serves_erased (s : SealedStore) (k : Key) (hlive : s.keys (s.vals k).key = true)
    (hplain : (s.vals k).plain ≠ .record none) :
    runAbove [] s [.admit k, .destroy (s.vals k).key, .read k] ≠
      runTruth s [.admit k, .destroy (s.vals k).key, .read k] := by
  simp [runAbove, runTruth, openWith, destroyKey, destroy, List.lookup, hlive, hplain]

end Strata.WorkingSet
