/-
The browser's cache of view results, against the server it copies (docs/frontend.md, "The cache
and its coherence").

The browser cannot read the server's generation the way a runtime can
(`WorkingSet.checkedRead_linearizes`). It learns stamps from what arrives, each after a delay it
cannot measure. Every request the browser sends (a ping, a read) is answered with the stamp current
when the server answered: the tenant epoch, the generation, the erasure epoch, and the **floor
generation**, the greater of the principal's access generation (its last membership, role,
restriction or permission change) and the tenant's policy generation (its last change of a Read
policy or of the teams and roles policies read). Such an answer is a **contact**, dated by the send
time of the request, which the browser measured itself. A reply also carries the stamp it was read
at. A frame (a notice or a heartbeat) carries the stamp it was sent with, its `from` generation and
the keys touched since; it is not a contact.

An answer is **current** when its stamp is the newest heard and the last contact is at most `B`
ticks old; **checked** when its stamp is the newest heard but the contact is older (drawn as "last
checked at ..." and not refetched); otherwise **stale**.

Stamps are ordered only within a tenant epoch: a restore, a re-created tenant or a lease acquisition
rotates the nonce and starts a new epoch, whose counter may start again from 1. With the epoch's tag
the server sends its **ordinal**, raised at every rotation and never reset. The client keeps its known
stamp, its floor and its entries for one epoch; it switches only to a higher ordinal (purging and
resetting), and ignores anything of a lower ordinal. In the model the ordinal stands for the tag as
well, and the generation `g` is one order across epochs, which the client never relies on across
them.

Proved:

- **the lag of the stamp** (`current_recent`): every generation and erasure epoch newer than the
  stamp of a current answer began within `B` before the read. A bound on the stamp, not the value:
  with an incomplete notice the value can be wrong however recent the contact
  (`fresh_contact_still_stale`);
- **the value, under C2** (`current_value`, `current_answer`): if every frame names every key
  touched in its interval, a current or checked answer is the server's opened value at the stamp
  last heard;
- **nothing at rest from an older erasure epoch, read below the floor, or from another tenant
  epoch** (`rest_one_epoch`, `rest_above_floor`, `rest_same_tenant_epoch`), and hearing a newer
  erasure epoch, floor or tenant epoch purges at once, while a lower tenant epoch is ignored
  (`hear_purges`, `hear_access_purges`, `older_epoch_ignored`,
  `tenant_epoch_purges`); the floor covers a policy change for every principal at once
  (`contact_floor`);
- **recovery across tenant epochs** (`contact_reaches_current_epoch`): whatever arrived before, any
  answer sent in the server's current epoch brings the client to that epoch; answers out of order
  across two rotations cannot leave it stuck (`out_of_order_epochs_recover`);
- **a read after a confirmed commit** returns the committed value (`receipt_read`,
  `read_after_commit`);
- **what fails without each piece**: an incomplete notice (`incomplete_notice_serves_stale`,
  `fresh_contact_still_stale`); an epoch read after opening (`stamp_after_open_serves_erased`); a put
  without the persisted epoch (`unguarded_put_resurrects`) or without the floor
  (`unfloored_put_resurrects`); a re-created tenant compared with the old one's numbers
  (`restarted_counter_dropped_without_reset`); the generation alone
  (`browser_generation_only_serves_erased`).

Assumptions, all of them:

C1. The server's history is immutable: a commit defines the next generation, a key destruction the
    next erasure epoch, and neither changes a value at an earlier stamp.
C2. A frame names every key touched at any point in its interval `(from, to]`: a key it does not
    name has the same value at every generation in `[from, to]`. A hypothesis of `current_value`
    (`RunC2`), never a filter of the step.
C3. A reply stamps the erasure epoch read before, or atomically with, opening its sealed fields.
C4. The browser's clock does not run backwards between a request and the reads that rely on its
    answer; a hidden or frozen page re-reads the time before it draws.
C5. A view depending on several object types is governed by each type's epoch; the per-type
    instances of this model compose by conjunction (argued, not proved; a conformance case in
    WG-5435 checks the server side).
C6. A reply that did not come from the server for this request is never taken as an answer: the
    driver issues view reads with `cache: 'no-store'`, and the server echoes a per-request nonce in
    the answer's stamp, which the kernel checks (a replayed or cached answer fails it).

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.WorkingSet

namespace Strata.Client.Cache

open Strata

set_option linter.unusedSectionVars false

/-! ## The server, as a client observes it -/

structure Server (K V : Type) where
  gen : Nat
  x : Nat
  /-- The principal's access generation. -/
  acc : Nat
  /-- The tenant's policy generation. -/
  pol : Nat
  /-- The tenant epoch: a restore or a re-created tenant starts a new one. -/
  ep : Nat
  view : Nat → Nat → K → V
  gTime : Nat → Nat
  xTime : Nat → Nat
  now : Nat
  /-- Answers sent and not yet delivered: the send time of the request, and the generation, erasure
      epoch, floor generation and tenant epoch current when the answer was sent. -/
  inflight : List (Nat × Nat × Nat × Nat × Nat)

variable {K V : Type} [DecidableEq K]

/-- The floor generation: the greater of the access and the policy generation. -/
def Server.fl (s : Server K V) : Nat := max s.acc s.pol

def Server.commit (s : Server K V) (f : K → V) : Server K V :=
  { s with gen := s.gen + 1, view := fun g y => if g = s.gen + 1 then f else s.view g y,
           gTime := fun g => if g = s.gen + 1 then s.now else s.gTime g }

/-- A commit that changes what this principal may read. -/
def Server.revoke (s : Server K V) (f : K → V) : Server K V := { s.commit f with acc := s.gen + 1 }

/-- A commit that changes a Read policy, or the teams and roles a policy reads, for the whole tenant. -/
def Server.policy (s : Server K V) (f : K → V) : Server K V := { s.commit f with pol := s.gen + 1 }

/-- A restore or a re-created tenant: a new tenant epoch. -/
def Server.recreate (s : Server K V) (f : K → V) : Server K V := { s.commit f with ep := s.ep + 1 }

def Server.destroy (s : Server K V) (er : Nat → K → V) : Server K V :=
  { s with x := s.x + 1, view := fun g y => if y = s.x + 1 then er g else s.view g y,
           xTime := fun y => if y = s.x + 1 then s.now else s.xTime y }

/-! ## The client's cache -/

structure Entry (V : Type) where
  val : V
  gen : Nat
  x : Nat
  /-- The tenant epoch it was read in. -/
  ep : Nat

structure Cache (K V : Type) where
  known : Nat
  knownX : Nat
  floor : Nat
  contact : Nat
  /-- The ordinal of the tenant epoch the known stamp, floor and entries belong to. -/
  ce : Nat
  entries : K → Option (Entry V)

inductive Answer (V : Type) where
  | current (v : V)
  | checked (v : V)
  | stale (v : V) (gen : Nat)
  | miss
  deriving DecidableEq

def setK (f : K → Option (Entry V)) (k : K) (x : Option (Entry V)) : K → Option (Entry V) :=
  fun k' => if k' = k then x else f k'

def read (B now : Nat) (c : Cache K V) (k : K) : Answer V :=
  match c.entries k with
  | none => .miss
  | some e =>
    if e.gen = c.known ∧ e.x = c.knownX then
      (if now ≤ c.contact + B then .current e.val else .checked e.val)
    else .stale e.val e.gen

/-- Taking in a message of tenant epoch ordinal `e`: the same epoch goes on, a lower one is ignored
    (`none`), a higher one purges and resets everything, in one transaction. -/
def epochOf (c : Cache K V) (e : Nat) : Option (Cache K V) :=
  if e = c.ce then some c
  else if e < c.ce then none
  else some { known := 0, knownX := 0, floor := 0, contact := 0, ce := e, entries := fun _ => none }

def hearX (c : Cache K V) (y : Nat) : Cache K V :=
  if c.knownX < y then
    { c with knownX := y,
             entries := fun k => match c.entries k with
               | some e => if e.x < y then none else some e
               | none => none }
  else c

def hearAcc (c : Cache K V) (a : Nat) : Cache K V :=
  if c.floor < a then
    { c with floor := a,
             entries := fun k => match c.entries k with
               | some e => if e.gen < a then none else some e
               | none => none }
  else c

def insertAt (c1 : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  match c1.entries k with
  | some e =>
    if g < e.gen then { c1 with known := max c1.known g }
    else { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y, c1.ce⟩) }
  | none => { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y, c1.ce⟩) }

/-- The persisted guarded put of a value read at `(g, y)` in tenant epoch `er`. -/
def put (c : Cache K V) (k : K) (v : V) (g y er : Nat) : Cache K V :=
  if er ≠ c.ce ∨ y < c.knownX ∨ g < c.floor then c else insertAt (hearX c y) k v g y

/-- An answer to a request sent at `t`, carrying the server's stamp when it answered. -/
def contactIn (c : Cache K V) (t gp xp fp ep : Nat) : Cache K V :=
  match epochOf c ep with
  | none => c
  | some c0 =>
    let c1 := hearAcc (hearX c0 xp) fp
    { c1 with known := max c1.known gp, contact := max c1.contact t }

/-- The entries after a frame from `a` to `b` naming `ks`: those read in `[a, b]` and not named are
    carried to `b`; those read in `[a, b]` and named are dropped. -/
def carry (es : K → Option (Entry V)) (a b : Nat) (ks : List K) : K → Option (Entry V) :=
  fun k => match es k with
    | some e => if a ≤ e.gen ∧ e.gen ≤ b then (if k ∈ ks then none else some ⟨e.val, b, e.x, e.ep⟩) else some e
    | none => none

/-- A frame of tenant epoch `e`, sent with stamp `(b, y, f)`. -/
def frame (c : Cache K V) (a b y f e : Nat) (ks : List K) : Cache K V :=
  match epochOf c e with
  | none => c
  | some c0 =>
    if y < c0.knownX then c0
    else
      let c1 := hearAcc (hearX c0 y) f
      { c1 with known := max c1.known b, entries := carry c1.entries a b ks }

/-! ## Operations -/

inductive Op (K V : Type) where
  | commit (f : K → V)
  | revoke (f : K → V)
  | policy (f : K → V)
  | recreate (f : K → V)
  | destroy (er : Nat → K → V)
  | tick
  | answer (t : Nat)
  | pong (t gp xp fp ep : Nat)
  /-- An answer to a read of `k`, read at `(g, y)` in tenant epoch `er`. -/
  | reply (k : K) (g y er t gs xs fs es : Nat)
  | frame (a b y f e : Nat) (ks : List K)
  | lose

def step (sc : Server K V × Cache K V) : Op K V → Server K V × Cache K V
  | .commit f => (sc.1.commit f, sc.2)
  | .revoke f => (sc.1.revoke f, sc.2)
  | .policy f => (sc.1.policy f, sc.2)
  | .recreate f => (sc.1.recreate f, sc.2)
  | .destroy er => (sc.1.destroy er, sc.2)
  | .tick => ({ sc.1 with now := sc.1.now + 1 }, sc.2)
  | .answer t =>
    if t ≤ sc.1.now then
      ({ sc.1 with inflight := (t, sc.1.gen, sc.1.x, sc.1.fl, sc.1.ep) :: sc.1.inflight }, sc.2)
    else sc
  | .pong t gp xp fp ep =>
    if (t, gp, xp, fp, ep) ∈ sc.1.inflight then (sc.1, contactIn sc.2 t gp xp fp ep) else sc
  | .reply k g y er t gs xs fs es =>
    if (t, gs, xs, fs, es) ∈ sc.1.inflight ∧ g ≤ gs ∧ y ≤ xs then
      (sc.1, put (contactIn sc.2 t gs xs fs es) k (sc.1.view g y k) g y er)
    else sc
  | .frame a b y f e ks =>
    if a ≤ b ∧ b ≤ sc.1.gen ∧ y ≤ sc.1.x ∧ f ≤ sc.1.fl ∧ e ≤ sc.1.ep then
      (sc.1, frame sc.2 a b y f e ks)
    else sc
  | .lose => sc

def run : Server K V × Cache K V → List (Op K V) → Server K V × Cache K V
  | sc, [] => sc
  | sc, op :: ops => run (step sc op) ops

/-- C2: a frame names every key touched in `(a, b]`. -/
def C2 (sc : Server K V × Cache K V) : Op K V → Prop
  | .frame a b y _ _ ks => ∀ k, k ∉ ks → ∀ g, a ≤ g → g ≤ b → sc.1.view g y k = sc.1.view b y k
  | _ => True

def RunC2 : Server K V × Cache K V → List (Op K V) → Prop
  | _, [] => True
  | sc, op :: ops => C2 sc op ∧ RunC2 (step sc op) ops

/-! ## Invariants -/

structure Base (s : Server K V) (c : Cache K V) : Prop where
  known_le : c.known ≤ s.gen
  knownX_le : c.knownX ≤ s.x
  floor_le : c.floor ≤ s.fl
  acc_le : s.acc ≤ s.gen
  pol_le : s.pol ≤ s.gen
  contact_le : c.contact ≤ s.now
  ce_le : c.ce ≤ s.ep
  entry_gen : ∀ k e, c.entries k = some e → e.gen ≤ c.known
  entry_x : ∀ k e, c.entries k = some e → e.x = c.knownX
  entry_floor : ∀ k e, c.entries k = some e → c.floor ≤ e.gen
  entry_ep : ∀ k e, c.entries k = some e → e.ep = c.ce
  gap_gen : ∀ g, c.known < g → g ≤ s.gen → c.contact ≤ s.gTime g
  gap_x : ∀ y, c.knownX < y → y ≤ s.x → c.contact ≤ s.xTime y
  flight : ∀ p ∈ s.inflight, p.1 ≤ s.now ∧ p.2.1 ≤ s.gen ∧ p.2.2.1 ≤ s.x ∧ p.2.2.2.1 ≤ s.fl ∧
    (∀ g, p.2.1 < g → g ≤ s.gen → p.1 ≤ s.gTime g) ∧ (∀ y, p.2.2.1 < y → y ≤ s.x → p.1 ≤ s.xTime y) ∧
    p.2.2.2.2 ≤ s.ep

def Vals (s : Server K V) (c : Cache K V) : Prop :=
  ∀ k e, c.entries k = some e → e.val = s.view e.gen e.x k

/-! ### Tenant epochs -/

theorem epochOf_base (s : Server K V) (c : Cache K V) (e : Nat) (c' : Cache K V) (h : Base s c)
    (hle : e ≤ s.ep) (he : epochOf c e = some c') : Base s c' := by
  unfold epochOf at he
  split at he
  · cases he; exact h
  · split at he
    · cases he
    · cases he
      exact ⟨Nat.zero_le _, Nat.zero_le _, Nat.zero_le _, h.acc_le, h.pol_le, Nat.zero_le _, hle,
        (fun _ _ h => by cases h), (fun _ _ h => by cases h), (fun _ _ h => by cases h),
        (fun _ _ h => by cases h), (fun _ _ _ => Nat.zero_le _), (fun _ _ _ => Nat.zero_le _), h.flight⟩

theorem epochOf_vals (s : Server K V) (c : Cache K V) (e : Nat) (c' : Cache K V) (hv : Vals s c)
    (he : epochOf c e = some c') : Vals s c' := by
  unfold epochOf at he
  split at he
  · cases he; exact hv
  · split at he
    · cases he
    · cases he; intro _ _ h; cases h

theorem epochOf_ce (c : Cache K V) (e : Nat) (c' : Cache K V) (he : epochOf c e = some c') :
    c'.ce = e := by
  unfold epochOf at he
  split at he
  · rename_i h; cases he; exact h.symm
  · split at he
    · cases he
    · cases he; rfl

/-! ### What hearing does to the fields -/

theorem hearX_fields (c : Cache K V) (y : Nat) :
    (hearX c y).known = c.known ∧ (hearX c y).floor = c.floor ∧ (hearX c y).contact = c.contact ∧
    (hearX c y).knownX = max c.knownX y ∧ (hearX c y).ce = c.ce := by
  unfold hearX
  split
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_right (Nat.le_of_lt h)).symm, rfl⟩
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_left (Nat.le_of_not_lt h)).symm, rfl⟩

theorem hearX_entry (c : Cache K V) (y : Nat) (hx : ∀ k e, c.entries k = some e → e.x = c.knownX)
    (k : K) (e : Entry V) (he : (hearX c y).entries k = some e) :
    c.entries k = some e ∧ e.x = max c.knownX y := by
  unfold hearX at he
  split at he
  · rename_i hlt
    simp only at he
    cases hc : c.entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      have := hx k e0 hc
      by_cases hl : e0.x < y
      · simp [hl] at he
      · omega
  · rename_i hn
    exact ⟨he, by have := hx k e he; rw [Nat.max_eq_left (Nat.le_of_not_lt hn)]; exact this⟩

theorem hearAcc_fields (c : Cache K V) (a : Nat) :
    (hearAcc c a).known = c.known ∧ (hearAcc c a).knownX = c.knownX ∧ (hearAcc c a).contact = c.contact ∧
    (hearAcc c a).floor = max c.floor a ∧ (hearAcc c a).ce = c.ce := by
  unfold hearAcc
  split
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_right (Nat.le_of_lt h)).symm, rfl⟩
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_left (Nat.le_of_not_lt h)).symm, rfl⟩

theorem hearAcc_entry (c : Cache K V) (a : Nat) (hf : ∀ k e, c.entries k = some e → c.floor ≤ e.gen)
    (k : K) (e : Entry V) (he : (hearAcc c a).entries k = some e) :
    c.entries k = some e ∧ max c.floor a ≤ e.gen := by
  unfold hearAcc at he
  split at he
  · rename_i hlt
    simp only at he
    cases hc : c.entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      by_cases hl : e0.gen < a
      · simp [hl] at he
      · simp only [hl, ↓reduceIte, Option.some.injEq] at he
        subst he
        exact ⟨rfl, by rw [Nat.max_eq_right (Nat.le_of_lt hlt)]; omega⟩
  · rename_i hn
    exact ⟨he, by rw [Nat.max_eq_left (Nat.le_of_not_lt hn)]; exact hf k e he⟩

/-- The entries after hearing an epoch and a floor come from the entries before. -/
theorem hear_entry (s : Server K V) (c : Cache K V) (y a : Nat) (h : Base s c) (k : K) (e : Entry V)
    (he : (hearAcc (hearX c y) a).entries k = some e) :
    c.entries k = some e ∧ e.x = max c.knownX y ∧ max c.floor a ≤ e.gen := by
  have hf : ∀ k e, (hearX c y).entries k = some e → (hearX c y).floor ≤ e.gen := fun k e he => by
    rw [(hearX_fields c y).2.1]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1
  obtain ⟨h1, h2⟩ := hearAcc_entry _ a hf k e he
  obtain ⟨h3, h4⟩ := hearX_entry c y h.entry_x k e h1
  rw [(hearX_fields c y).2.1] at h2
  exact ⟨h3, h4, h2⟩

theorem hear_base (s : Server K V) (c : Cache K V) (y a : Nat) (h : Base s c) (hy : y ≤ s.x)
    (ha : a ≤ s.fl) : Base s (hearAcc (hearX c y) a) := by
  obtain ⟨x1, x2, x3, x4, x5⟩ := hearX_fields c y
  obtain ⟨a1, a2, a3, a4, a5⟩ := hearAcc_fields (hearX c y) a
  refine ⟨?_, ?_, ?_, h.acc_le, h.pol_le, ?_, ?_, fun k e he => ?_, fun k e he => ?_, fun k e he => ?_,
    fun k e he => ?_, fun g p q => ?_, fun y' p q => ?_, h.flight⟩
  · rw [a1, x1]; exact h.known_le
  · rw [a2, x4]; exact Nat.max_le.mpr ⟨h.knownX_le, hy⟩
  · rw [a4, x2]; exact Nat.max_le.mpr ⟨h.floor_le, ha⟩
  · rw [a3, x3]; exact h.contact_le
  · rw [a5, x5]; exact h.ce_le
  · rw [a1, x1]; exact h.entry_gen k e (hear_entry s c y a h k e he).1
  · rw [a2, x4]; exact (hear_entry s c y a h k e he).2.1
  · rw [a4, x2]; exact (hear_entry s c y a h k e he).2.2
  · rw [a5, x5]; exact h.entry_ep k e (hear_entry s c y a h k e he).1
  · rw [a3, x3]; rw [a1, x1] at p; exact h.gap_gen g p q
  · rw [a3, x3]; rw [a2, x4] at p; exact h.gap_x y' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q

theorem hear_vals (s : Server K V) (c : Cache K V) (y a : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (hearAcc (hearX c y) a) :=
  fun k e he => hv k e (hear_entry s c y a h k e he).1

/-! ### Contacts -/

theorem contact_base (s : Server K V) (c : Cache K V) (t gp xp fp ep : Nat) (h : Base s c)
    (hin : (t, gp, xp, fp, ep) ∈ s.inflight) : Base s (contactIn c t gp xp fp ep) := by
  obtain ⟨ht, hgp, hxp, hfp, hgg, hgx, hep⟩ := h.flight _ hin
  unfold contactIn
  cases hc : epochOf c ep with
  | none => exact h
  | some c0 =>
    simp only
    have h0 := epochOf_base s c ep c0 h hep hc
    have h1 := hear_base s c0 xp fp h0 hxp hfp
    obtain ⟨x1, x2, x3, x4, x5⟩ := hearX_fields c0 xp
    obtain ⟨a1, a2, a3, a4, a5⟩ := hearAcc_fields (hearX c0 xp) fp
    refine ⟨Nat.max_le.mpr ⟨h1.known_le, hgp⟩, h1.knownX_le, h1.floor_le, h.acc_le, h.pol_le,
      Nat.max_le.mpr ⟨h1.contact_le, ht⟩, h1.ce_le,
      fun k e he => Nat.le_trans (h1.entry_gen k e he) (Nat.le_max_left _ _), h1.entry_x, h1.entry_floor,
      h1.entry_ep, fun g p q => ?_, fun y p q => ?_, h.flight⟩
    · have p' : max (hearAcc (hearX c0 xp) fp).known gp < g := p
      rw [a1, x1] at p'
      show max (hearAcc (hearX c0 xp) fp).contact t ≤ s.gTime g
      rw [a3, x3]
      exact Nat.max_le.mpr ⟨h0.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p') q,
        hgg g (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) p') q⟩
    · have p' : (hearAcc (hearX c0 xp) fp).knownX < y := p
      rw [a2, x4] at p'
      show max (hearAcc (hearX c0 xp) fp).contact t ≤ s.xTime y
      rw [a3, x3]
      exact Nat.max_le.mpr ⟨h0.gap_x y (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p') q,
        hgx y (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) p') q⟩

theorem contact_vals (s : Server K V) (c : Cache K V) (t gp xp fp ep : Nat) (h : Base s c)
    (hin : (t, gp, xp, fp, ep) ∈ s.inflight) (hv : Vals s c) : Vals s (contactIn c t gp xp fp ep) := by
  unfold contactIn
  cases hc : epochOf c ep with
  | none => exact hv
  | some c0 =>
    exact hear_vals s c0 xp fp (epochOf_base s c ep c0 h (by
      obtain ⟨_, _, _, _, _, _, hep⟩ := h.flight _ hin; exact hep) hc) (epochOf_vals s c ep c0 hv hc)

/-! ### The put -/

theorem insert_fields (c1 : Cache K V) (k : K) (v : V) (g y : Nat) :
    (insertAt c1 k v g y).known = max c1.known g ∧ (insertAt c1 k v g y).knownX = c1.knownX ∧
    (insertAt c1 k v g y).floor = c1.floor ∧ (insertAt c1 k v g y).contact = c1.contact ∧
    (insertAt c1 k v g y).ce = c1.ce ∧
    ∀ k' e, (insertAt c1 k v g y).entries k' = some e → c1.entries k' = some e ∨ (k' = k ∧ e = ⟨v, g, y, c1.ce⟩) := by
  unfold insertAt
  cases hc : c1.entries k with
  | none =>
    refine ⟨rfl, rfl, rfl, rfl, rfl, fun k' e he => ?_⟩
    simp only [setK] at he
    by_cases hk : k' = k
    · simp only [hk, ↓reduceIte, Option.some.injEq] at he; exact Or.inr ⟨hk, he.symm⟩
    · simp only [hk, ↓reduceIte] at he; exact Or.inl he
  | some e0 =>
    simp only
    split
    · exact ⟨rfl, rfl, rfl, rfl, rfl, fun k' e he => Or.inl he⟩
    · refine ⟨rfl, rfl, rfl, rfl, rfl, fun k' e he => ?_⟩
      simp only [setK] at he
      by_cases hk : k' = k
      · simp only [hk, ↓reduceIte, Option.some.injEq] at he; exact Or.inr ⟨hk, he.symm⟩
      · simp only [hk, ↓reduceIte] at he; exact Or.inl he

theorem put_base (s : Server K V) (c : Cache K V) (k : K) (v : V) (g y er : Nat) (hg : g ≤ s.gen)
    (hy : y ≤ s.x) (h : Base s c) : Base s (put c k v g y er) := by
  unfold put
  by_cases hd : er ≠ c.ce ∨ y < c.knownX ∨ g < c.floor
  · simp only [hd, ↓reduceIte]; exact h
  · simp only [hd, ↓reduceIte]
    have hy' : c.knownX ≤ y := by omega
    have hg' : c.floor ≤ g := by omega
    obtain ⟨x1, x2, x3, x4, x5⟩ := hearX_fields c y
    have hmx : max c.knownX y = y := Nat.max_eq_right hy'
    obtain ⟨i1, i2, i3, i4, i5, i7⟩ := insert_fields (hearX c y) k v g y
    refine ⟨?_, ?_, ?_, h.acc_le, h.pol_le, ?_, ?_, fun k' e he => ?_, fun k' e he => ?_,
      fun k' e he => ?_, fun k' e he => ?_, fun g' p q => ?_, fun y' p q => ?_, h.flight⟩
    · rw [i1, x1]; exact Nat.max_le.mpr ⟨h.known_le, hg⟩
    · rw [i2, x4, hmx]; exact hy
    · rw [i3, x2]; exact h.floor_le
    · rw [i4, x3]; exact h.contact_le
    · rw [i5, x5]; exact h.ce_le
    · rw [i1, x1]
      rcases i7 k' e he with he | ⟨_, rfl⟩
      · exact Nat.le_trans (h.entry_gen k' e (hearX_entry c y h.entry_x k' e he).1) (Nat.le_max_left _ _)
      · exact Nat.le_max_right _ _
    · rw [i2, x4, hmx]
      rcases i7 k' e he with he | ⟨_, rfl⟩
      · rw [← hmx]; exact (hearX_entry c y h.entry_x k' e he).2
      · rfl
    · rw [i3, x2]
      rcases i7 k' e he with he | ⟨_, rfl⟩
      · exact h.entry_floor k' e (hearX_entry c y h.entry_x k' e he).1
      · exact hg'
    · rw [i5, x5]
      rcases i7 k' e he with he | ⟨_, rfl⟩
      · exact h.entry_ep k' e (hearX_entry c y h.entry_x k' e he).1
      · exact x5
    · rw [i4, x3]; rw [i1, x1] at p
      exact h.gap_gen g' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q
    · rw [i4, x3]; rw [i2, x4] at p
      exact h.gap_x y' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q

theorem put_vals (s : Server K V) (c : Cache K V) (k : K) (g y er : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (put c k (s.view g y k) g y er) := by
  unfold put
  by_cases hd : er ≠ c.ce ∨ y < c.knownX ∨ g < c.floor
  · simp only [hd, ↓reduceIte]; exact hv
  · simp only [hd, ↓reduceIte]
    intro k' e he
    rcases (insert_fields (hearX c y) k (s.view g y k) g y).2.2.2.2.2 k' e he with he | ⟨hk, rfl⟩
    · exact hv k' e (hearX_entry c y h.entry_x k' e he).1
    · subst hk; rfl

/-! ### The frame -/

theorem carry_entry (es : K → Option (Entry V)) (a b : Nat) (ks : List K) (k : K) (e : Entry V)
    (he : carry es a b ks k = some e) :
    ∃ e0, es k = some e0 ∧ ((a ≤ e0.gen ∧ e0.gen ≤ b ∧ k ∉ ks ∧ e = ⟨e0.val, b, e0.x, e0.ep⟩) ∨
      (¬ (a ≤ e0.gen ∧ e0.gen ≤ b) ∧ e = e0)) := by
  unfold carry at he
  cases hc : es k with
  | none => rw [hc] at he; cases he
  | some e0 =>
    rw [hc] at he
    simp only at he
    refine ⟨e0, rfl, ?_⟩
    by_cases hin : a ≤ e0.gen ∧ e0.gen ≤ b
    · simp only [hin, and_self, ↓reduceIte] at he
      by_cases hm : k ∈ ks
      · simp [hm] at he
      · simp only [hm, ↓reduceIte, Option.some.injEq] at he
        exact Or.inl ⟨hin.1, hin.2, hm, he.symm⟩
    · simp only [hin, ↓reduceIte, Option.some.injEq] at he
      exact Or.inr ⟨hin, he.symm⟩

theorem frame_base (s : Server K V) (c : Cache K V) (a b y f e : Nat) (ks : List K) (h : Base s c)
    (hb : b ≤ s.gen) (hy : y ≤ s.x) (hf : f ≤ s.fl) (he : e ≤ s.ep) : Base s (frame c a b y f e ks) := by
  unfold frame
  cases hc : epochOf c e with
  | none => exact h
  | some c0 =>
    simp only
    have h0 := epochOf_base s c e c0 h he hc
    by_cases hlt : y < c0.knownX
    · simp only [hlt, ↓reduceIte]; exact h0
    · simp only [hlt, ↓reduceIte]
      have h1 := hear_base s c0 y f h0 hy hf
      refine ⟨Nat.max_le.mpr ⟨h1.known_le, hb⟩, h1.knownX_le, h1.floor_le, h.acc_le, h.pol_le,
        h1.contact_le, h1.ce_le, fun k e he => ?_, fun k e he => ?_, fun k e he => ?_, fun k e he => ?_,
        fun g p q => h1.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q, h1.gap_x, h.flight⟩
      · obtain ⟨e0, he0, hcase⟩ := carry_entry _ a b ks k e he
        rcases hcase with ⟨_, _, _, heq⟩ | ⟨_, heq⟩ <;> rw [heq]
        · exact Nat.le_max_right _ _
        · exact Nat.le_trans (h1.entry_gen k e0 he0) (Nat.le_max_left _ _)
      · obtain ⟨e0, he0, hcase⟩ := carry_entry _ a b ks k e he
        rcases hcase with ⟨_, _, _, heq⟩ | ⟨_, heq⟩ <;> rw [heq]
        · exact h1.entry_x k e0 he0
        · exact h1.entry_x k e0 he0
      · obtain ⟨e0, he0, hcase⟩ := carry_entry _ a b ks k e he
        rcases hcase with ⟨_, hb', _, heq⟩ | ⟨_, heq⟩ <;> rw [heq]
        · exact Nat.le_trans (h1.entry_floor k e0 he0) hb'
        · exact h1.entry_floor k e0 he0
      · obtain ⟨e0, he0, hcase⟩ := carry_entry _ a b ks k e he
        rcases hcase with ⟨_, _, _, heq⟩ | ⟨_, heq⟩ <;> rw [heq]
        · exact h1.entry_ep k e0 he0
        · exact h1.entry_ep k e0 he0

theorem frame_vals (s : Server K V) (c : Cache K V) (a b y f e : Nat) (ks : List K) (h : Base s c)
    (he : e ≤ s.ep) (hv : Vals s c) (hc2 : ∀ k, k ∉ ks → ∀ g, a ≤ g → g ≤ b → s.view g y k = s.view b y k) :
    Vals s (frame c a b y f e ks) := by
  unfold frame
  cases hc : epochOf c e with
  | none => exact hv
  | some c0 =>
    simp only
    have h0 := epochOf_base s c e c0 h he hc
    have hv0 := epochOf_vals s c e c0 hv hc
    by_cases hlt : y < c0.knownX
    · simp only [hlt, ↓reduceIte]; exact hv0
    · simp only [hlt, ↓reduceIte]
      have hy : c0.knownX ≤ y := by omega
      have h1v := hear_vals s c0 y f h0 hv0
      intro k e' he
      obtain ⟨e0, he0, hcase⟩ := carry_entry _ a b ks k e' he
      rcases hcase with ⟨ha, hb, hm, heq⟩ | ⟨_, heq⟩ <;> rw [heq]
      · have hx := (hear_entry s c0 y f h0 k e0 he0).2.1
        rw [Nat.max_eq_right hy] at hx
        simp only
        rw [h1v k e0 he0, hx]
        exact hc2 k hm e0.gen ha hb
      · exact h1v k e0 he0

/-! ### Every step -/

theorem step_base (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c) :
    Base (step (s, c) op).1 (step (s, c) op).2 := by
  have commitB : ∀ (f : K → V) (a p e : Nat), s.acc ≤ a → s.pol ≤ p → a ≤ s.gen + 1 → p ≤ s.gen + 1 →
      s.ep ≤ e → Base { s.commit f with acc := a, pol := p, ep := e } c := by
    intro f a p e ha1 hp1 ha2 hp2 he
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, h14⟩ := h
    have hfl : s.fl ≤ max a p := by
      simp only [Server.fl]; exact Nat.max_le.mpr ⟨Nat.le_trans ha1 (Nat.le_max_left _ _),
        Nat.le_trans hp1 (Nat.le_max_right _ _)⟩
    refine ⟨by simp only [Server.commit]; omega, h2, Nat.le_trans h3 hfl, ha2, hp2, h6, Nat.le_trans h7 he,
      h8, h9, h10, h11, fun g p' q => ?_, h13, fun pr hp => ?_⟩
    · simp only [Server.commit] at q ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact h6
      · simp only [hg, ↓reduceIte]; exact h12 g p' (by omega)
    · obtain ⟨b1, b2, b3, b4, b5, b6, b7⟩ := h14 pr hp
      refine ⟨b1, by simp only [Server.commit]; omega, b3, Nat.le_trans b4 hfl, fun g p' q => ?_, b6,
        Nat.le_trans b7 he⟩
      simp only [Server.commit] at q ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact b1
      · simp only [hg, ↓reduceIte]; exact b5 g p' (by omega)
  have hacc := h.acc_le
  have hpol := h.pol_le
  cases op with
  | commit f => exact commitB f s.acc s.pol s.ep (Nat.le_refl _) (Nat.le_refl _) (by omega) (by omega) (Nat.le_refl _)
  | revoke f => exact commitB f (s.gen + 1) s.pol s.ep (by omega) (Nat.le_refl _) (Nat.le_refl _) (by omega) (Nat.le_refl _)
  | policy f => exact commitB f s.acc (s.gen + 1) s.ep (Nat.le_refl _) (by omega) (by omega) (Nat.le_refl _) (Nat.le_refl _)
  | recreate f => exact commitB f s.acc s.pol (s.ep + 1) (Nat.le_refl _) (Nat.le_refl _) (by omega) (by omega) (by omega)
  | destroy er =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, h14⟩ := h
    refine ⟨h1, by simp only [step, Server.destroy]; omega, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12,
      fun y p q => ?_, fun pr hp => ?_⟩
    · simp only [step, Server.destroy] at q ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact h6
      · simp only [hy, ↓reduceIte]; exact h13 y p (by omega)
    · obtain ⟨b1, b2, b3, b4, b5, b6, b7⟩ := h14 pr hp
      refine ⟨b1, b2, by simp only [step, Server.destroy]; omega, b4, b5, fun y p q => ?_, b7⟩
      simp only [step, Server.destroy] at q ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact b1
      · simp only [hy, ↓reduceIte]; exact b6 y p (by omega)
  | tick =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, h14⟩ := h
    exact ⟨h1, h2, h3, h4, h5, by simp only [step]; omega, h7, h8, h9, h10, h11, h12, h13, fun pr hp => by
      obtain ⟨b1, b2, b3, b4, b5, b6, b7⟩ := h14 pr hp
      exact ⟨by simp only [step]; omega, b2, b3, b4, b5, b6, b7⟩⟩
  | answer t =>
    simp only [step]
    by_cases ht : t ≤ s.now
    · simp only [ht, ↓reduceIte]
      obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, h14⟩ := h
      refine ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12, h13, fun pr hp => ?_⟩
      rcases List.mem_cons.mp hp with hp | hp
      · subst hp
        exact ⟨ht, Nat.le_refl _, Nat.le_refl _, Nat.le_refl _, fun g a b => by simp only at a b; omega,
          fun y a b => by simp only at a b; omega, Nat.le_refl _⟩
      · exact h14 pr hp
    · simp only [ht, ↓reduceIte]; exact ⟨h.1, h.2, h.3, h.4, h.5, h.6, h.7, h.8, h.9, h.10, h.11, h.12, h.13, h.14⟩
  | pong t gp xp fp ep =>
    simp only [step]
    by_cases hin : (t, gp, xp, fp, ep) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact contact_base s c t gp xp fp ep h hin
    · simp only [hin, ↓reduceIte]; exact h
  | reply k g y er t gs xs fs es =>
    simp only [step]
    by_cases hv : (t, gs, xs, fs, es) ∈ s.inflight ∧ g ≤ gs ∧ y ≤ xs
    · simp only [hv, and_self, ↓reduceIte]
      obtain ⟨_, hgs, hxs, _, _, _, _⟩ := h.flight _ hv.1
      exact put_base s _ k _ g y er (Nat.le_trans hv.2.1 hgs) (Nat.le_trans hv.2.2 hxs)
        (contact_base s c t gs xs fs es h hv.1)
    · simp only [hv, ↓reduceIte]; exact h
  | frame a b y f e ks =>
    simp only [step]
    by_cases hv : a ≤ b ∧ b ≤ s.gen ∧ y ≤ s.x ∧ f ≤ s.fl ∧ e ≤ s.ep
    · simp only [hv, and_self, ↓reduceIte]; exact frame_base s c a b y f e ks h hv.2.1 hv.2.2.1 hv.2.2.2.1 hv.2.2.2.2
    · simp only [hv, ↓reduceIte]; exact h
  | lose => exact h

theorem step_vals (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c)
    (hv : Vals s c) (hc : C2 (s, c) op) : Vals (step (s, c) op).1 (step (s, c) op).2 := by
  have commitV : ∀ (f : K → V) (a p e : Nat), Vals { s.commit f with acc := a, pol := p, ep := e } c := by
    intro f a p e k en he
    have h1 := hv k en he
    have hg := Nat.le_trans (h.entry_gen k en he) h.known_le
    simp only [Server.commit]
    simp only [show ¬ (en.gen = s.gen + 1) by omega, ↓reduceIte]; exact h1
  cases op with
  | commit f => exact commitV f s.acc s.pol s.ep
  | revoke f => exact commitV f (s.gen + 1) s.pol s.ep
  | policy f => exact commitV f s.acc (s.gen + 1) s.ep
  | recreate f => exact commitV f s.acc s.pol (s.ep + 1)
  | destroy er =>
    intro k e he
    have h1 := hv k e he
    have hx : e.x ≤ s.x := by rw [h.entry_x k e he]; exact h.knownX_le
    simp only [step, Server.destroy]
    simp only [show ¬ (e.x = s.x + 1) by omega, ↓reduceIte]; exact h1
  | tick => exact hv
  | answer t =>
    simp only [step]
    by_cases ht : t ≤ s.now
    · simp only [ht, ↓reduceIte]; exact hv
    · simp only [ht, ↓reduceIte]; exact hv
  | pong t gp xp fp ep =>
    simp only [step]
    by_cases hin : (t, gp, xp, fp, ep) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact contact_vals s c t gp xp fp ep h hin hv
    · simp only [hin, ↓reduceIte]; exact hv
  | reply k g y er t gs xs fs es =>
    simp only [step]
    by_cases hr : (t, gs, xs, fs, es) ∈ s.inflight ∧ g ≤ gs ∧ y ≤ xs
    · simp only [hr, and_self, ↓reduceIte]
      exact put_vals s _ k g y er (contact_base s c t gs xs fs es h hr.1) (contact_vals s c t gs xs fs es h hr.1 hv)
    · simp only [hr, ↓reduceIte]; exact hv
  | frame a b y f e ks =>
    simp only [step]
    by_cases hr : a ≤ b ∧ b ≤ s.gen ∧ y ≤ s.x ∧ f ≤ s.fl ∧ e ≤ s.ep
    · simp only [hr, and_self, ↓reduceIte]; exact frame_vals s c a b y f e ks h hr.2.2.2.2 hv hc
    · simp only [hr, ↓reduceIte]; exact hv
  | lose => exact hv

theorem run_base (ops : List (Op K V)) :
    ∀ sc : Server K V × Cache K V, Base sc.1 sc.2 → Base (run sc ops).1 (run sc ops).2 := by
  induction ops with
  | nil => intro sc h; exact h
  | cons op ops ih => intro sc h; exact ih _ (step_base sc.1 sc.2 op h)

/-- **Coherence under C2**: every entry is the server's value at its own stamp, at every step of
    every schedule whose frames name every key touched in their interval.
    Verifies: FE-001, FE-009 -/
theorem run_coh (ops : List (Op K V)) :
    ∀ sc : Server K V × Cache K V, Base sc.1 sc.2 → Vals sc.1 sc.2 → RunC2 sc ops →
      Vals (run sc ops).1 (run sc ops).2 := by
  induction ops with
  | nil => intro sc _ hv _; exact hv
  | cons op ops ih =>
    intro sc h hv hr
    exact ih _ (step_base sc.1 sc.2 op h) (step_vals sc.1 sc.2 op h hv hr.1) hr.2

/-! ## What a read returns -/

/-- **The lag of the stamp**: every generation and every erasure epoch newer than the stamp of a
    current answer began within the last `B` ticks before the read. This bounds the stamp, not the
    value (`fresh_contact_still_stale`). No assumption on frames.
    Verifies: FE-003 -/
theorem current_recent (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Base s c)
    (hr : read B s.now c k = .current v) :
    (∀ g, c.known < g → g ≤ s.gen → s.now ≤ s.gTime g + B) ∧
    (∀ y, c.knownX < y → y ≤ s.x → s.now ≤ s.xTime y + B) := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hs : e.gen = c.known ∧ e.x = c.knownX
    · by_cases hn : s.now ≤ c.contact + B
      · exact ⟨fun g a b => by have := h.gap_gen g a b; omega,
               fun y a b => by have := h.gap_x y a b; omega⟩
      · simp only [hs, and_self, hn, ↓reduceIte] at hr; cases hr
    · simp only [hs, ↓reduceIte] at hr; cases hr

/-- **A current or checked answer is the server's opened value at the stamp last heard** (under C2,
    through `Vals`).
    Verifies: FE-001 -/
theorem current_value (B now : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V)
    (hv : Vals s c) (hr : read B now c k = .current v ∨ read B now c k = .checked v) :
    v = s.view c.known c.knownX k := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; simp at hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hs : e.gen = c.known ∧ e.x = c.knownX
    · have : e.val = v := by
        by_cases hn : now ≤ c.contact + B
        · simp [hs, hn] at hr; exact hr
        · simp [hs, hn] at hr; exact hr
      rw [← this, hv k e hc, hs.1, hs.2]
    · simp [hs] at hr

/-- **The whole stamp with carry-forward, in one statement.** -/
theorem current_answer (B : Nat) (ops : List (Op K V)) (sc : Server K V × Cache K V)
    (h : Base sc.1 sc.2) (hv : Vals sc.1 sc.2) (hr2 : RunC2 sc ops) (k : K) (v : V)
    (hr : read B (run sc ops).1.now (run sc ops).2 k = .current v) :
    v = (run sc ops).1.view (run sc ops).2.known (run sc ops).2.knownX k ∧
    (∀ g, (run sc ops).2.known < g → g ≤ (run sc ops).1.gen → (run sc ops).1.now ≤ (run sc ops).1.gTime g + B) ∧
    (∀ y, (run sc ops).2.knownX < y → y ≤ (run sc ops).1.x → (run sc ops).1.now ≤ (run sc ops).1.xTime y + B) :=
  ⟨current_value B _ _ _ k v (run_coh ops sc h hv hr2) (Or.inl hr),
   current_recent B _ _ k v (run_base ops sc h) hr⟩

/-- **Anything else held is labelled with its generation.**
    Verifies: FE-009 -/
theorem read_stale_older (B now : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V) (g : Nat)
    (h : Base s c) (hr : read B now c k = .stale v g) :
    g ≤ c.known ∧ ∃ e, c.entries k = some e ∧ e.gen = g ∧ e.val = v := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hs : e.gen = c.known ∧ e.x = c.knownX
    · by_cases hn : now ≤ c.contact + B
      · simp [hs, hn] at hr
      · simp [hs, hn] at hr
    · simp only [hs, ↓reduceIte, Answer.stale.injEq] at hr
      obtain ⟨hv', hg⟩ := hr
      exact ⟨by rw [← hg]; exact h.entry_gen k e hc, e, rfl, hg, hv'⟩

/-- **Nothing of an older erasure epoch stays at rest.**
    Verifies: FE-012 -/
theorem rest_one_epoch (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : e.x = c.knownX :=
  h.entry_x k e he

/-- **Nothing read below the floor stays at rest.**
    Verifies: FE-015 -/
theorem rest_above_floor (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : c.floor ≤ e.gen :=
  h.entry_floor k e he

/-- **Nothing from another tenant epoch stays at rest.**
    Verifies: FE-018 -/
theorem rest_same_tenant_epoch (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : e.ep = c.ce :=
  h.entry_ep k e he

/-- Hearing an erasure epoch deletes, in the same step, every entry of an older one.
    Verifies: FE-012 -/
theorem hear_purges (s : Server K V) (c : Cache K V) (h : Base s c) (y : Nat) (k : K) (e : Entry V)
    (he : (hearX c y).entries k = some e) : y ≤ e.x := by
  have := (hearX_entry c y h.entry_x k e he).2
  rw [this]; exact Nat.le_max_right _ _

/-- Hearing a floor deletes, in the same step, every entry read below it.
    Verifies: FE-015 -/
theorem hear_access_purges (s : Server K V) (c : Cache K V) (h : Base s c) (a : Nat) (k : K)
    (e : Entry V) (he : (hearAcc c a).entries k = some e) : a ≤ e.gen :=
  Nat.le_trans (Nat.le_max_right _ _) (hearAcc_entry c a h.entry_floor k e he).2

/-- **An answer raises the floor to the tenant's policy generation as well as the principal's
    access generation**: one tenant-wide write covers every principal.
    Verifies: FE-015 -/
theorem contact_floor (s : Server K V) (c : Cache K V) (t : Nat) (hce : c.ce = s.ep) :
    s.pol ≤ (contactIn c t s.gen s.x s.fl s.ep).floor ∧ s.acc ≤ (contactIn c t s.gen s.x s.fl s.ep).floor := by
  unfold contactIn
  have he : epochOf c s.ep = some c := by simp [epochOf, hce]
  rw [he]
  simp only
  have := (hearAcc_fields (hearX c s.x) s.fl).2.2.2.1
  show s.pol ≤ (hearAcc (hearX c s.x) s.fl).floor ∧ s.acc ≤ (hearAcc (hearX c s.x) s.fl).floor
  rw [this]
  exact ⟨Nat.le_trans (Nat.le_max_right s.acc s.pol) (Nat.le_max_right _ _),
         Nat.le_trans (Nat.le_max_left s.acc s.pol) (Nat.le_max_right _ _)⟩

/-- **Hearing a higher tenant epoch purges and resets together**: nothing is left at rest, and the
    known stamp, erasure epoch and floor start again from that epoch.
    Verifies: FE-018 -/
theorem tenant_epoch_purges (c c' : Cache K V) (e : Nat) (hgt : c.ce < e) (he : epochOf c e = some c') :
    (∀ k, c'.entries k = none) ∧ c'.known = 0 ∧ c'.knownX = 0 ∧ c'.floor = 0 ∧ c'.ce = e := by
  unfold epochOf at he
  have hne : e ≠ c.ce := by omega
  have hnlt : ¬ e < c.ce := by omega
  simp only [hne, hnlt, ↓reduceIte, Option.some.injEq] at he
  subst he
  exact ⟨fun _ => rfl, rfl, rfl, rfl, rfl⟩

/-- **A message of a lower tenant epoch ordinal changes nothing.**
    Verifies: FE-018 -/
theorem older_epoch_ignored (c : Cache K V) (e : Nat) (hlt : e < c.ce)
    (a b y f : Nat) (ks : List K) (t gp xp : Nat) :
    frame c a b y f e ks = c ∧ contactIn c t gp xp f e = c := by
  have hne : e ≠ c.ce := by omega
  have hn : epochOf c e = none := by simp [epochOf, hne, hlt]
  exact ⟨by simp [frame, hn], by simp [contactIn, hn]⟩

/-- **Recovery across tenant epochs**: whatever arrived before, an answer sent in the server's
    current epoch brings the client to that epoch.
    Verifies: FE-018 -/
theorem contact_reaches_current_epoch (s : Server K V) (c : Cache K V) (h : Base s c)
    (t gp xp fp : Nat) : (contactIn c t gp xp fp s.ep).ce = s.ep := by
  unfold contactIn
  have hle := h.ce_le
  cases hc : epochOf c s.ep with
  | none =>
    unfold epochOf at hc
    by_cases heq : s.ep = c.ce
    · simp [heq] at hc
    · have : ¬ s.ep < c.ce := by omega
      simp [heq, this] at hc
  | some c0 =>
    simp only
    have := epochOf_ce c s.ep c0 hc
    show (hearAcc (hearX c0 xp) fp).ce = s.ep
    rw [(hearAcc_fields _ fp).2.2.2.2, (hearX_fields c0 xp).2.2.2.2, this]

/-- The review's schedule: the tenant rotates twice (ordinals 0, 1, 2); the client hears an answer of
    epoch 2, then a late answer of epoch 1, then another answer of epoch 2 with a reply, and ends in
    epoch 2 with the reply current. -/
def cx0 : Cache Nat Nat :=
  { known := 0, knownX := 0, floor := 0, contact := 0, ce := 0, entries := fun _ => none }

theorem out_of_order_epochs_recover :
    let c1 := contactIn cx0 5 10 0 0 2
    let c2 := contactIn c1 6 3 0 0 1
    let c3 := put (contactIn c2 7 11 0 0 2) 0 9 11 0 2
    c2.ce = 2 ∧ c3.ce = 2 ∧ read 1 7 c3 0 = .current 9 := by
  decide

set_option linter.unusedSimpArgs false in
/-- **A reply read and sent at the server's current stamp is read back as current**: the reply is
    itself a contact, dated by its request's send time.
    Verifies: FE-002 -/
theorem receipt_read (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (h : Base s c) (hce : c.ce = s.ep)
    (t : Nat) (ht : t ≤ s.now) (hB : s.now ≤ t + B) :
    read B s.now
      (run (s, c) [.answer t, .reply k s.gen s.x s.ep t s.gen s.x s.fl s.ep]).2 k = .current (s.view s.gen s.x k) := by
  let s1 : Server K V := { s with inflight := (t, s.gen, s.x, s.fl, s.ep) :: s.inflight }
  have hin : (t, s.gen, s.x, s.fl, s.ep) ∈ s1.inflight := List.mem_cons_self ..
  have hb1 : Base s1 c := step_base s c (.answer t) h |> fun hb => by
    simp only [step, ht, ↓reduceIte] at hb; exact hb
  have hc1 := contact_base s1 c t s.gen s.x s.fl s.ep hb1 hin
  have he : epochOf c s.ep = some c := by simp [epochOf, hce]
  have hci : contactIn c t s.gen s.x s.fl s.ep =
      { hearAcc (hearX c s.x) s.fl with known := max (hearAcc (hearX c s.x) s.fl).known s.gen,
                                        contact := max (hearAcc (hearX c s.x) s.fl).contact t } := by
    simp only [contactIn, he]
  obtain ⟨x1, x2, x3, x4, x5⟩ := hearX_fields c s.x
  obtain ⟨a1, a2, a3, a4, a5⟩ := hearAcc_fields (hearX c s.x) s.fl
  have hm : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
  have hmx : max c.knownX s.x = s.x := Nat.max_eq_right h.knownX_le
  have hfl : max c.floor s.fl = s.fl := Nat.max_eq_right h.floor_le
  have hflg : s.fl ≤ s.gen := Nat.max_le.mpr ⟨h.acc_le, h.pol_le⟩
  have c1 : (contactIn c t s.gen s.x s.fl s.ep).known = s.gen := by rw [hci]; show max _ _ = _; rw [a1, x1, hm]
  have c2 : (contactIn c t s.gen s.x s.fl s.ep).knownX = s.x := by rw [hci]; show _ = _; rw [a2, x4, hmx]
  have c3 : (contactIn c t s.gen s.x s.fl s.ep).floor = s.fl := by rw [hci]; show _ = _; rw [a4, x2, hfl]
  have c4 : (contactIn c t s.gen s.x s.fl s.ep).contact = max c.contact t := by
    rw [hci]; show max _ _ = _; rw [a3, x3]
  have c5 : (contactIn c t s.gen s.x s.fl s.ep).ce = s.ep := by rw [hci]; show _ = _; rw [a5, x5, hce]
  show read B s.now (step (step (s, c) (.answer t)) (.reply k s.gen s.x s.ep t s.gen s.x s.fl s.ep)).2 k = _
  simp only [step, ht, ↓reduceIte]
  simp only [hin, Nat.le_refl, and_self, ↓reduceIte]
  have hnd : ¬ (s.ep ≠ (contactIn c t s.gen s.x s.fl s.ep).ce ∨ s.x < (contactIn c t s.gen s.x s.fl s.ep).knownX ∨
      s.gen < (contactIn c t s.gen s.x s.fl s.ep).floor) := by
    rw [c5, c2, c3]; omega
  unfold put
  simp only [hnd, ↓reduceIte]
  have hx1 := hearX_fields (contactIn c t s.gen s.x s.fl s.ep) s.x
  have hent : ∀ e, (hearX (contactIn c t s.gen s.x s.fl s.ep) s.x).entries k = some e → e.gen ≤ s.gen := by
    intro e he'
    have := (hearX_entry _ s.x hc1.entry_x k e he').1
    have := hc1.entry_gen k e this
    rw [c1] at this; exact this
  unfold insertAt
  cases he' : (hearX (contactIn c t s.gen s.x s.fl s.ep) s.x).entries k with
  | none =>
    simp only [read, setK, ↓reduceIte, hx1.1, hx1.2.2.1, hx1.2.2.2.1, c1, c2, c4, Nat.max_self]
    have : s.now ≤ max c.contact t + B := by omega
    simp [this, setK]
  | some e =>
    have hle := hent e he'
    have : ¬ s.gen < e.gen := by omega
    simp only [this, ↓reduceIte, read, setK, hx1.1, hx1.2.2.1, hx1.2.2.2.1, c1, c2, c4, Nat.max_self]
    have : s.now ≤ max c.contact t + B := by omega
    simp [this, setK]

/-- **A read after a confirmed commit returns the committed value.**
    Verifies: FE-002 -/
theorem read_after_commit (B : Nat) (s : Server K V) (c : Cache K V) (f : K → V) (k : K)
    (h : Base s c) (hce : c.ce = s.ep) (t : Nat) (ht : t ≤ s.now) (hB : s.now ≤ t + B) :
    read B s.now (run (s, c) [.commit f, .answer t,
      .reply k (s.gen + 1) s.x s.ep t (s.gen + 1) s.x s.fl s.ep]).2 k = .current (f k) := by
  have hb : Base (s.commit f) c := step_base s c (.commit f) h
  have hr := receipt_read B (s.commit f) c k hb hce t ht hB
  have hf : (s.commit f).view (s.commit f).gen (s.commit f).x k = f k := by simp [Server.commit]
  rw [hf] at hr
  exact hr

/-! ## What fails without each piece -/

def cxServer : Server Nat Nat :=
  { gen := 0, x := 0, acc := 0, pol := 0, ep := 0,
    view := fun g y _ => if y = 0 then (if g = 0 then 5 else 6) else 0,
    gTime := fun _ => 0, xTime := fun _ => 0, now := 0, inflight := [] }

def cxCache : Cache Nat Nat :=
  { known := 0, knownX := 0, floor := 0, contact := 0, ce := 0,
    entries := fun _ => some ⟨5, 0, 0, 0⟩ }

/-- **Without C2, a stale value stays current.** -/
theorem incomplete_notice_serves_stale :
    read 1 0 (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 0 0 []]).2 0 = .current 5 ∧
    (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 0 0 []]).1.view 1 0 0 = 6 := by
  decide

def cxLater : List (Op Nat Nat) :=
  [.commit (fun _ => 6), .frame 0 1 0 0 0 [], .tick, .tick, .tick, .tick, .tick, .answer 5, .pong 5 1 0 0 0]

/-- The review's schedule: after the incomplete frame and fresh contacts, the read still says
    current 5 while the server holds 6. -/
theorem fresh_contact_still_stale :
    read 1 5 (run (cxServer, cxCache) cxLater).2 0 = .current 5 ∧
    (run (cxServer, cxCache) cxLater).2.contact = 5 ∧
    (run (cxServer, cxCache) cxLater).1.view (run (cxServer, cxCache) cxLater).1.gen 0 0 = 6 := by
  decide

/-- **A server that reads the erasure epoch after opening stamps plaintext as current.** -/
theorem stamp_after_open_serves_erased :
    let s := cxServer.destroy (fun _ _ => 0)
    let c := put cxCache 0 5 0 1 0
    read 1 0 c 0 = .current 5 ∧ c.knownX = s.x ∧ s.view 0 s.x 0 = 0 := by
  decide

def putUnguarded (c : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  { c with entries := setK c.entries k (some ⟨v, g, y, c.ce⟩) }

/-- **Without the persisted epoch, a purge is undone.** -/
theorem unguarded_put_resurrects :
    let c := putUnguarded (hearX cxCache 1) 0 5 0 0
    ∃ e, c.entries 0 = some e ∧ e.x < c.knownX := by
  decide

theorem guarded_put_drops : (put (hearX cxCache 1) 0 5 0 0 0).entries 0 = none := by
  decide

/-- **Without the floor, revoked data comes back.** -/
theorem unfloored_put_resurrects :
    let c := putUnguarded (hearAcc cxCache 1) 0 5 0 0
    ∃ e, c.entries 0 = some e ∧ e.gen < c.floor := by
  decide

theorem floored_put_drops : (put (hearAcc cxCache 1) 0 5 0 0 0).entries 0 = none := by
  decide

/-- A client that had reached floor 5000 in one tenant epoch. -/
def cxOld : Cache Nat Nat :=
  { known := 5000, knownX := 0, floor := 5000, contact := 0, ce := 0, entries := fun _ => none }

/-- **Comparing a re-created tenant with the old one's numbers drops everything**: its counter
    starts again, so a reply at generation 1 is below the old floor. -/
theorem restarted_counter_dropped_without_reset : (put cxOld 0 7 1 0 0).entries 0 = none := by
  decide

/-- With the reset on a new tenant epoch, the same reply is kept. -/
theorem reset_accepts :
    ((epochOf cxOld 1).map (fun c => put c 0 7 1 0 1)).map (fun c => (c.entries 0).map (·.val)) = some (some 7) := by
  decide

/-! ## The working set's keys -/

def ofWorlds (worlds : Nat → World) (gen now : Nat) (gTime : Nat → Nat) :
    Server WorkingSet.Key WorkingSet.Payload :=
  { gen := gen, x := 0, acc := 0, pol := 0, ep := 0, view := fun g _ k => WorkingSet.fetch (worlds g) k,
    gTime := gTime, xTime := fun _ => 0, now := now, inflight := [] }

theorem current_fetch (B now : Nat) (worlds : Nat → World) (gen n : Nat) (gT : Nat → Nat)
    (c : Cache WorkingSet.Key WorkingSet.Payload) (k : WorkingSet.Key) (v : WorkingSet.Payload)
    (hv : Vals (ofWorlds worlds gen n gT) c) (hr : read B now c k = .current v) :
    v = WorkingSet.fetch (worlds c.known) k :=
  current_value B now _ c k v hv (Or.inl hr)

open WorkingSet in
/-- **A browser cache stamped by generation alone serves erased values.** -/
theorem browser_generation_only_serves_erased (s : SealedStore) (k : Key)
    (hlive : s.keys (s.vals k).key = true) (hplain : (s.vals k).plain ≠ .record none) :
    runAbove [] s [.admitEntry k, .destroy (s.vals k).key, .read k] ≠
      runTruth s [.admitEntry k, .destroy (s.vals k).key, .read k] :=
  above_sealing_serves_erased s k hlive hplain

end Strata.Client.Cache
