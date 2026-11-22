/-
The browser's cache of view results, against the server it copies (docs/frontend.md, "The cache
and its coherence").

The browser cannot read the server's generation the way a runtime can
(`WorkingSet.checkedRead_linearizes`). It learns stamps from what arrives, each after a delay it
cannot measure. Every request the browser sends (a ping, a read) is answered with the stamp current
when the server answered: its generation, its erasure epoch and its access generation (the
generation of the last change to what this principal may read: a membership, role, restriction or
permission change). Such an answer is a **contact**, dated by the send time of the request, which
the browser measured itself: however late it arrives, everything newer than its stamp began after
that time. A reply also carries the stamp it was read at. A frame (a notice or a heartbeat) carries
the stamp it was sent with, its `from` generation, and the keys that may differ; it is not a
contact, because nothing in it says when it was sent.

An answer is **current** when its stamp is the newest heard and the last contact is at most `B`
ticks old; **checked** when its stamp is the newest heard but the contact is older (the page says
"last checked at ..." and does not refetch); otherwise **stale**.

The generation `g` stands for the pair (tenant epoch, generation): its only property used is that a
stamp never repeats (C1). The erasure epoch `x` stands for the epoch of one object type (the design
keeps one per object type, from a destruction log keyed by type and field, so no class is
enumerated); a view that depends on several types carries each type's epoch, and the model is that
of one type (C5). The client persists its known stamp and an **access floor** with its entries; every
put compares with them and raises them in one transaction.

Proved:

- **the lag of the stamp** (`current_recent`): every generation and erasure epoch newer than the
  stamp of a current answer began within `B` before the read. A bound on the stamp, not the value:
  with an incomplete notice the value can be wrong however recent the contact
  (`fresh_contact_still_stale`);
- **the value, under C2** (`current_value`, `current_answer`): if every frame is complete, a current
  or checked answer is the server's opened value at the stamp last heard;
- **nothing at rest from an older erasure epoch, or read before an access change the client has
  heard** (`rest_one_epoch`, `rest_above_floor`), and hearing either purges at once (`hear_purges`,
  `hear_access_purges`);
- **a read after a confirmed commit** returns the committed value (`receipt_read`,
  `read_after_commit`): a reply is itself a contact, so a fresh read is current and never refetched
  in a loop;
- **what fails without each piece**: an incomplete notice (`incomplete_notice_serves_stale`,
  `fresh_contact_still_stale`); an epoch read after opening (`stamp_after_open_serves_erased`); a put
  without the persisted epoch (`unguarded_put_resurrects`) or without the access floor
  (`unfloored_put_resurrects`); the generation alone (`browser_generation_only_serves_erased`).

Assumptions, all of them:

C1. The server's history is immutable: a commit defines the next generation, a key destruction the
    next erasure epoch, and neither changes a value at an earlier stamp.
C2. A frame is complete: every key whose value differs between its `from` generation and its stamp
    is named. A hypothesis of `current_value` (`RunC2`), never a filter of the step.
C3. A reply stamps the erasure epoch read before, or atomically with, opening its sealed fields.
C4. The browser's clock does not run backwards between a request and the reads that rely on its
    answer; a hidden or frozen page re-reads the time before it draws.
C5. A view depending on several object types is governed by each type's epoch; the per-type
    instances of this model compose by conjunction (argued, not proved).

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.WorkingSet

namespace Strata.Client.Cache

open Strata

set_option linter.unusedSectionVars false

/-! ## The server, as a client observes it -/

structure Server (K V : Type) where
  /-- The generation reached (the design's epoch and generation together). -/
  gen : Nat
  /-- The erasure epoch reached. -/
  x : Nat
  /-- The access generation: the generation of the last change to what the principal may read. -/
  acc : Nat
  /-- What an opened read at generation `g` and erasure epoch `y` returns. -/
  view : Nat → Nat → K → V
  gTime : Nat → Nat
  xTime : Nat → Nat
  now : Nat
  /-- Answers sent and not yet delivered: the send time of the request they answer, and the
      generation, erasure epoch and access generation current when the answer was sent. -/
  inflight : List (Nat × Nat × Nat × Nat)

variable {K V : Type} [DecidableEq K]

def Server.commit (s : Server K V) (f : K → V) : Server K V :=
  { s with gen := s.gen + 1, view := fun g y => if g = s.gen + 1 then f else s.view g y,
           gTime := fun g => if g = s.gen + 1 then s.now else s.gTime g }

/-- A commit that also changes what the principal may read. -/
def Server.revoke (s : Server K V) (f : K → V) : Server K V :=
  { s.commit f with acc := s.gen + 1 }

def Server.destroy (s : Server K V) (er : Nat → K → V) : Server K V :=
  { s with x := s.x + 1, view := fun g y => if y = s.x + 1 then er g else s.view g y,
           xTime := fun y => if y = s.x + 1 then s.now else s.xTime y }

/-! ## The client's cache -/

structure Entry (V : Type) where
  val : V
  gen : Nat
  x : Nat

structure Cache (K V : Type) where
  known : Nat
  knownX : Nat
  /-- No entry read before this generation may be kept: the last access change heard. -/
  floor : Nat
  /-- The send time of the newest request whose answer has arrived. -/
  contact : Nat
  entries : K → Option (Entry V)

inductive Answer (V : Type) where
  | current (v : V)
  /-- The newest stamp heard, but the last contact is older than the bound: "last checked at". -/
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

/-- Hearing an erasure epoch: a newer one deletes every entry of an older one. -/
def hearX (c : Cache K V) (y : Nat) : Cache K V :=
  if c.knownX < y then
    { c with knownX := y,
             entries := fun k => match c.entries k with
               | some e => if e.x < y then none else some e
               | none => none }
  else c

/-- Hearing an access generation: a newer one deletes every entry read before it. -/
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
    else { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y⟩) }
  | none => { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y⟩) }

/-- The persisted guarded put: one transaction drops a value of an older erasure epoch or read
    below the access floor, raises the known stamp, and never replaces a later entry. -/
def put (c : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  if y < c.knownX ∨ g < c.floor then c else insertAt (hearX c y) k v g y

/-- An answer to a request sent at `t`, carrying the server's stamp when it answered. -/
def contactIn (c : Cache K V) (t gp xp ap : Nat) : Cache K V :=
  let c1 := hearAcc (hearX c xp) ap
  { c1 with known := max c1.known gp, contact := max c1.contact t }

/-- A frame sent with stamp `(b, y, ac)`: between `a` and `b` only `ks` may differ. -/
def frame (c : Cache K V) (a b y ac : Nat) (ks : List K) : Cache K V :=
  if y < c.knownX then c
  else
    let c1 := hearAcc (hearX c y) ac
    { c1 with known := max c1.known b,
              entries := fun k =>
                match c1.entries k with
                | some e => if e.gen = a then (if k ∈ ks then none else some ⟨e.val, b, e.x⟩) else some e
                | none => none }

/-! ## Operations -/

inductive Op (K V : Type) where
  | commit (f : K → V)
  /-- A commit that changes what the principal may read (membership, role, restriction, permission). -/
  | revoke (f : K → V)
  | destroy (er : Nat → K → V)
  | tick
  /-- The server answers a request (a ping or a read) the client sent at `t`. -/
  | answer (t : Nat)
  /-- An answer to a ping arrives. -/
  | pong (t gp xp ap : Nat)
  /-- An answer to a read of `k` arrives: read at `(g, y)`, sent with the stamp of the answer. -/
  | reply (k : K) (g y t gs xs ap : Nat)
  /-- A frame: a notice from generation `a`, sent with stamp `(b, y, ac)`; a heartbeat names no keys. -/
  | frame (a b y ac : Nat) (ks : List K)
  | lose

/-- One step. The guards say only what a server can send; none asks whether a notice is complete. -/
def step (sc : Server K V × Cache K V) : Op K V → Server K V × Cache K V
  | .commit f => (sc.1.commit f, sc.2)
  | .revoke f => (sc.1.revoke f, sc.2)
  | .destroy er => (sc.1.destroy er, sc.2)
  | .tick => ({ sc.1 with now := sc.1.now + 1 }, sc.2)
  | .answer t =>
    if t ≤ sc.1.now then ({ sc.1 with inflight := (t, sc.1.gen, sc.1.x, sc.1.acc) :: sc.1.inflight }, sc.2)
    else sc
  | .pong t gp xp ap => if (t, gp, xp, ap) ∈ sc.1.inflight then (sc.1, contactIn sc.2 t gp xp ap) else sc
  | .reply k g y t gs xs ap =>
    if (t, gs, xs, ap) ∈ sc.1.inflight ∧ g ≤ gs ∧ y ≤ xs then
      (sc.1, put (contactIn sc.2 t gs xs ap) k (sc.1.view g y k) g y)
    else sc
  | .frame a b y ac ks =>
    if a ≤ b ∧ b ≤ sc.1.gen ∧ y ≤ sc.1.x ∧ ac ≤ sc.1.acc then (sc.1, frame sc.2 a b y ac ks) else sc
  | .lose => sc

def run : Server K V × Cache K V → List (Op K V) → Server K V × Cache K V
  | sc, [] => sc
  | sc, op :: ops => run (step sc op) ops

def C2 (sc : Server K V × Cache K V) : Op K V → Prop
  | .frame a b y _ ks => ∀ k, k ∉ ks → sc.1.view a y k = sc.1.view b y k
  | _ => True

def RunC2 : Server K V × Cache K V → List (Op K V) → Prop
  | _, [] => True
  | sc, op :: ops => C2 sc op ∧ RunC2 (step sc op) ops

/-! ## Invariants -/

structure Base (s : Server K V) (c : Cache K V) : Prop where
  known_le : c.known ≤ s.gen
  knownX_le : c.knownX ≤ s.x
  floor_le : c.floor ≤ s.acc
  acc_le : s.acc ≤ s.gen
  contact_le : c.contact ≤ s.now
  entry_gen : ∀ k e, c.entries k = some e → e.gen ≤ c.known
  entry_x : ∀ k e, c.entries k = some e → e.x = c.knownX
  entry_floor : ∀ k e, c.entries k = some e → c.floor ≤ e.gen
  gap_gen : ∀ g, c.known < g → g ≤ s.gen → c.contact ≤ s.gTime g
  gap_x : ∀ y, c.knownX < y → y ≤ s.x → c.contact ≤ s.xTime y
  flight : ∀ p ∈ s.inflight, p.1 ≤ s.now ∧ p.2.1 ≤ s.gen ∧ p.2.2.1 ≤ s.x ∧ p.2.2.2 ≤ s.acc ∧
    (∀ g, p.2.1 < g → g ≤ s.gen → p.1 ≤ s.gTime g) ∧ (∀ y, p.2.2.1 < y → y ≤ s.x → p.1 ≤ s.xTime y)

def Vals (s : Server K V) (c : Cache K V) : Prop :=
  ∀ k e, c.entries k = some e → e.val = s.view e.gen e.x k

/-! ### What hearing does to the fields -/

theorem hearX_fields (c : Cache K V) (y : Nat) :
    (hearX c y).known = c.known ∧ (hearX c y).floor = c.floor ∧ (hearX c y).contact = c.contact ∧
    (hearX c y).knownX = max c.knownX y := by
  unfold hearX
  split
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_right (Nat.le_of_lt h)).symm⟩
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_left (Nat.le_of_not_lt h)).symm⟩

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
    (hearAcc c a).floor = max c.floor a := by
  unfold hearAcc
  split
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_right (Nat.le_of_lt h)).symm⟩
  · rename_i h; exact ⟨rfl, rfl, rfl, (Nat.max_eq_left (Nat.le_of_not_lt h)).symm⟩

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

/-- Hearing an epoch and an access generation the server has reached keeps `Base`. -/
theorem hear_base (s : Server K V) (c : Cache K V) (y a : Nat) (h : Base s c) (hy : y ≤ s.x)
    (ha : a ≤ s.acc) : Base s (hearAcc (hearX c y) a) := by
  obtain ⟨x1, x2, x3, x4⟩ := hearX_fields c y
  obtain ⟨a1, a2, a3, a4⟩ := hearAcc_fields (hearX c y) a
  refine ⟨?_, ?_, ?_, h.acc_le, ?_, fun k e he => ?_, fun k e he => ?_, fun k e he => ?_,
    fun g p q => ?_, fun y' p q => ?_, h.flight⟩
  · rw [a1, x1]; exact h.known_le
  · rw [a2, x4]; exact Nat.max_le.mpr ⟨h.knownX_le, hy⟩
  · rw [a4, x2]; exact Nat.max_le.mpr ⟨h.floor_le, ha⟩
  · rw [a3, x3]; exact h.contact_le
  · obtain ⟨h1, _⟩ := hearAcc_entry _ a (fun k e he => by
      rw [x2]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1) k e he
    rw [a1, x1]; exact h.entry_gen k e (hearX_entry c y h.entry_x k e h1).1
  · obtain ⟨h1, _⟩ := hearAcc_entry _ a (fun k e he => by
      rw [x2]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1) k e he
    rw [a2, x4]; exact (hearX_entry c y h.entry_x k e h1).2
  · obtain ⟨_, h2⟩ := hearAcc_entry _ a (fun k e he => by
      rw [x2]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1) k e he
    rw [a4]; exact h2
  · rw [a3, x3]; rw [a1, x1] at p; exact h.gap_gen g p q
  · rw [a3, x3]; rw [a2, x4] at p; exact h.gap_x y' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q

theorem hear_vals (s : Server K V) (c : Cache K V) (y a : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (hearAcc (hearX c y) a) := by
  intro k e he
  have hf : ∀ k e, (hearX c y).entries k = some e → (hearX c y).floor ≤ e.gen := fun k e he => by
    rw [(hearX_fields c y).2.1]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1
  exact hv k e (hearX_entry c y h.entry_x k e (hearAcc_entry _ a hf k e he).1).1

theorem contact_base (s : Server K V) (c : Cache K V) (t gp xp ap : Nat) (h : Base s c)
    (hin : (t, gp, xp, ap) ∈ s.inflight) : Base s (contactIn c t gp xp ap) := by
  obtain ⟨ht, hgp, hxp, hap, hgg, hgx⟩ := h.flight _ hin
  have h1 := hear_base s c xp ap h hxp hap
  obtain ⟨x1, x2, x3, x4⟩ := hearX_fields c xp
  obtain ⟨a1, a2, a3, a4⟩ := hearAcc_fields (hearX c xp) ap
  refine ⟨Nat.max_le.mpr ⟨h1.known_le, hgp⟩, h1.knownX_le, h1.floor_le, h.acc_le,
    Nat.max_le.mpr ⟨h1.contact_le, ht⟩,
    fun k e he => Nat.le_trans (h1.entry_gen k e he) (Nat.le_max_left _ _), h1.entry_x, h1.entry_floor,
    fun g p q => ?_, fun y p q => ?_, h.flight⟩
  · have p' : max (hearAcc (hearX c xp) ap).known gp < g := p
    rw [a1, x1] at p'
    show max (hearAcc (hearX c xp) ap).contact t ≤ s.gTime g
    rw [a3, x3]
    exact Nat.max_le.mpr ⟨h.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p') q,
      hgg g (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) p') q⟩
  · have p' : (hearAcc (hearX c xp) ap).knownX < y := p
    rw [a2, x4] at p'
    show max (hearAcc (hearX c xp) ap).contact t ≤ s.xTime y
    rw [a3, x3]
    exact Nat.max_le.mpr ⟨h.gap_x y (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p') q,
      hgx y (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) p') q⟩

theorem contact_vals (s : Server K V) (c : Cache K V) (t gp xp ap : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (contactIn c t gp xp ap) :=
  hear_vals s c xp ap h hv

theorem contact_fields (c : Cache K V) (t gp xp ap : Nat) :
    (contactIn c t gp xp ap).known = max c.known gp ∧ (contactIn c t gp xp ap).knownX = max c.knownX xp ∧
    (contactIn c t gp xp ap).floor = max c.floor ap ∧ (contactIn c t gp xp ap).contact = max c.contact t := by
  obtain ⟨x1, x2, x3, x4⟩ := hearX_fields c xp
  obtain ⟨a1, a2, a3, a4⟩ := hearAcc_fields (hearX c xp) ap
  refine ⟨?_, ?_, ?_, ?_⟩
  · show max (hearAcc (hearX c xp) ap).known gp = _; rw [a1, x1]
  · show (hearAcc (hearX c xp) ap).knownX = _; rw [a2, x4]
  · show (hearAcc (hearX c xp) ap).floor = _; rw [a4, x2]
  · show max (hearAcc (hearX c xp) ap).contact t = _; rw [a3, x3]

/-! ### The put -/

theorem insert_fields (c1 : Cache K V) (k : K) (v : V) (g y : Nat) :
    (insertAt c1 k v g y).known = max c1.known g ∧ (insertAt c1 k v g y).knownX = c1.knownX ∧
    (insertAt c1 k v g y).floor = c1.floor ∧ (insertAt c1 k v g y).contact = c1.contact ∧
    ∀ k' e, (insertAt c1 k v g y).entries k' = some e → c1.entries k' = some e ∨ (k' = k ∧ e = ⟨v, g, y⟩) := by
  unfold insertAt
  cases hc : c1.entries k with
  | none =>
    refine ⟨rfl, rfl, rfl, rfl, fun k' e he => ?_⟩
    simp only [setK] at he
    by_cases hk : k' = k
    · simp only [hk, ↓reduceIte, Option.some.injEq] at he; exact Or.inr ⟨hk, he.symm⟩
    · simp only [hk, ↓reduceIte] at he; exact Or.inl he
  | some e0 =>
    simp only
    split
    · exact ⟨rfl, rfl, rfl, rfl, fun k' e he => Or.inl he⟩
    · refine ⟨rfl, rfl, rfl, rfl, fun k' e he => ?_⟩
      simp only [setK] at he
      by_cases hk : k' = k
      · simp only [hk, ↓reduceIte, Option.some.injEq] at he; exact Or.inr ⟨hk, he.symm⟩
      · simp only [hk, ↓reduceIte] at he; exact Or.inl he

theorem put_base (s : Server K V) (c : Cache K V) (k : K) (v : V) (g y : Nat) (hg : g ≤ s.gen)
    (hy : y ≤ s.x) (h : Base s c) : Base s (put c k v g y) := by
  unfold put
  by_cases hd : y < c.knownX ∨ g < c.floor
  · simp only [hd, ↓reduceIte]; exact h
  · simp only [hd, ↓reduceIte]
    have hy' : c.knownX ≤ y := by omega
    have hg' : c.floor ≤ g := by omega
    obtain ⟨x1, x2, x3, x4⟩ := hearX_fields c y
    have hmx : max c.knownX y = y := Nat.max_eq_right hy'
    obtain ⟨i1, i2, i3, i4, i5⟩ := insert_fields (hearX c y) k v g y
    refine ⟨?_, ?_, ?_, h.acc_le, ?_, fun k' e he => ?_, fun k' e he => ?_, fun k' e he => ?_,
      fun g' p q => ?_, fun y' p q => ?_, h.flight⟩
    · rw [i1, x1]; exact Nat.max_le.mpr ⟨h.known_le, hg⟩
    · rw [i2, x4, hmx]; exact hy
    · rw [i3, x2]; exact h.floor_le
    · rw [i4, x3]; exact h.contact_le
    · rw [i1, x1]
      rcases i5 k' e he with he | ⟨_, rfl⟩
      · exact Nat.le_trans (h.entry_gen k' e (hearX_entry c y h.entry_x k' e he).1) (Nat.le_max_left _ _)
      · exact Nat.le_max_right _ _
    · rw [i2, x4, hmx]
      rcases i5 k' e he with he | ⟨_, rfl⟩
      · rw [← hmx]; exact (hearX_entry c y h.entry_x k' e he).2
      · rfl
    · rw [i3, x2]
      rcases i5 k' e he with he | ⟨_, rfl⟩
      · exact h.entry_floor k' e (hearX_entry c y h.entry_x k' e he).1
      · exact hg'
    · rw [i4, x3]; rw [i1, x1] at p
      exact h.gap_gen g' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q
    · rw [i4, x3]; rw [i2, x4] at p
      exact h.gap_x y' (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q

theorem put_vals (s : Server K V) (c : Cache K V) (k : K) (g y : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (put c k (s.view g y k) g y) := by
  unfold put
  by_cases hd : y < c.knownX ∨ g < c.floor
  · simp only [hd, ↓reduceIte]; exact hv
  · simp only [hd, ↓reduceIte]
    intro k' e he
    rcases (insert_fields (hearX c y) k (s.view g y k) g y).2.2.2.2 k' e he with he | ⟨hk, rfl⟩
    · exact hv k' e (hearX_entry c y h.entry_x k' e he).1
    · subst hk; rfl

/-! ### The frame -/

theorem frame_base (s : Server K V) (c : Cache K V) (a b y ac : Nat) (ks : List K) (h : Base s c)
    (hab : a ≤ b) (hb : b ≤ s.gen) (hy : y ≤ s.x) (hac : ac ≤ s.acc) : Base s (frame c a b y ac ks) := by
  unfold frame
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact h
  · simp only [hlt, ↓reduceIte]
    have h1 := hear_base s c y ac h hy hac
    refine ⟨Nat.max_le.mpr ⟨h1.known_le, hb⟩, h1.knownX_le, h1.floor_le, h.acc_le, h1.contact_le,
      fun k e he => ?_, fun k e he => ?_, fun k e he => ?_,
      fun g p q => h1.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) p) q, h1.gap_x, h.flight⟩
    all_goals
      simp only at he
      cases hc : (hearAcc (hearX c y) ac).entries k with
      | none => rw [hc] at he; cases he
      | some e0 =>
        rw [hc] at he
        simp only at he
        have g0 := h1.entry_gen k e0 hc
        have x0 := h1.entry_x k e0 hc
        have f0 := h1.entry_floor k e0 hc
        by_cases hg : e0.gen = a
        · simp only [hg, ↓reduceIte] at he
          by_cases hm : k ∈ ks
          · simp [hm] at he
          · simp only [hm, ↓reduceIte, Option.some.injEq] at he
            subst he
            first
              | exact Nat.le_max_right _ _
              | exact x0
              | (show (hearAcc (hearX c y) ac).floor ≤ b; rw [← hg] at hab; omega)
        · simp only [hg, ↓reduceIte, Option.some.injEq] at he
          subst he
          first
            | exact Nat.le_trans g0 (Nat.le_max_left _ _)
            | exact x0
            | exact f0

theorem frame_vals (s : Server K V) (c : Cache K V) (a b y ac : Nat) (ks : List K) (h : Base s c)
    (hv : Vals s c) (hc2 : ∀ k, k ∉ ks → s.view a y k = s.view b y k) :
    Vals s (frame c a b y ac ks) := by
  unfold frame
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact hv
  · simp only [hlt, ↓reduceIte]
    have hy : c.knownX ≤ y := by omega
    have h1v := hear_vals s c y ac h hv
    have hx : ∀ k e, (hearAcc (hearX c y) ac).entries k = some e → e.x = y := fun k e he => by
      have hf : ∀ k e, (hearX c y).entries k = some e → (hearX c y).floor ≤ e.gen := fun k e he => by
        rw [(hearX_fields c y).2.1]; exact h.entry_floor k e (hearX_entry c y h.entry_x k e he).1
      have := (hearX_entry c y h.entry_x k e (hearAcc_entry _ ac hf k e he).1).2
      rw [Nat.max_eq_right hy] at this; exact this
    intro k e he
    simp only at he
    cases hc : (hearAcc (hearX c y) ac).entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      have hv0 := h1v k e0 hc
      by_cases hg : e0.gen = a
      · simp only [hg, ↓reduceIte] at he
        by_cases hm : k ∈ ks
        · simp [hm] at he
        · simp only [hm, ↓reduceIte, Option.some.injEq] at he
          subst he
          simp only
          rw [hv0, hg, hx k e0 hc]
          exact hc2 k hm
      · simp only [hg, ↓reduceIte, Option.some.injEq] at he
        subst he; exact hv0

/-! ### Every step -/

theorem step_base (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c) :
    Base (step (s, c) op).1 (step (s, c) op).2 := by
  have commitB : ∀ f : K → V, ∀ a : Nat, c.floor ≤ a → s.acc ≤ a → a ≤ s.gen + 1 →
      Base { s.commit f with acc := a } c := by
    intro f a ha1 ha2 ha3
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
    refine ⟨by simp only [Server.commit]; omega, h2, ha1, ha3, h5, h6, h7, h8, fun g p q => ?_, h10,
      fun pr hp => ?_⟩
    · simp only [Server.commit] at q ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact h5
      · simp only [hg, ↓reduceIte]; exact h9 g p (by omega)
    · obtain ⟨b1, b2, b3, b4, b5, b6⟩ := h11 pr hp
      refine ⟨b1, by simp only [Server.commit]; omega, b3, Nat.le_trans b4 ha2, fun g p q => ?_, b6⟩
      simp only [Server.commit] at q ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact b1
      · simp only [hg, ↓reduceIte]; exact b5 g p (by omega)
  cases op with
  | commit f =>
    have := commitB f s.acc h.floor_le (Nat.le_refl _) (by have := h.acc_le; omega)
    exact this
  | revoke f =>
    exact commitB f (s.gen + 1) (by have := h.floor_le; have := h.acc_le; omega)
      (by have := h.acc_le; omega) (Nat.le_refl _)
  | destroy er =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
    refine ⟨h1, by simp only [step, Server.destroy]; omega, h3, h4, h5, h6, h7, h8, h9,
      fun y p q => ?_, fun pr hp => ?_⟩
    · simp only [step, Server.destroy] at q ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact h5
      · simp only [hy, ↓reduceIte]; exact h10 y p (by omega)
    · obtain ⟨b1, b2, b3, b4, b5, b6⟩ := h11 pr hp
      refine ⟨b1, b2, by simp only [step, Server.destroy]; omega, b4, b5, fun y p q => ?_⟩
      simp only [step, Server.destroy] at q ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact b1
      · simp only [hy, ↓reduceIte]; exact b6 y p (by omega)
  | tick =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
    exact ⟨h1, h2, h3, h4, by simp only [step]; omega, h6, h7, h8, h9, h10, fun pr hp => by
      obtain ⟨b1, b2, b3, b4, b5, b6⟩ := h11 pr hp
      exact ⟨by simp only [step]; omega, b2, b3, b4, b5, b6⟩⟩
  | answer t =>
    simp only [step]
    by_cases ht : t ≤ s.now
    · simp only [ht, ↓reduceIte]
      obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
      refine ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, fun pr hp => ?_⟩
      rcases List.mem_cons.mp hp with hp | hp
      · subst hp
        exact ⟨ht, Nat.le_refl _, Nat.le_refl _, Nat.le_refl _, fun g a b => by simp only at a b; omega,
          fun y a b => by simp only at a b; omega⟩
      · exact h11 pr hp
    · simp only [ht, ↓reduceIte]; exact h
  | pong t gp xp ap =>
    simp only [step]
    by_cases hin : (t, gp, xp, ap) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact contact_base s c t gp xp ap h hin
    · simp only [hin, ↓reduceIte]; exact h
  | reply k g y t gs xs ap =>
    simp only [step]
    by_cases hv : (t, gs, xs, ap) ∈ s.inflight ∧ g ≤ gs ∧ y ≤ xs
    · simp only [hv, and_self, ↓reduceIte]
      obtain ⟨_, hgs, hxs, _, _, _⟩ := h.flight _ hv.1
      exact put_base s _ k _ g y (Nat.le_trans hv.2.1 hgs) (Nat.le_trans hv.2.2 hxs)
        (contact_base s c t gs xs ap h hv.1)
    · simp only [hv, ↓reduceIte]; exact h
  | frame a b y ac ks =>
    simp only [step]
    by_cases hv : a ≤ b ∧ b ≤ s.gen ∧ y ≤ s.x ∧ ac ≤ s.acc
    · simp only [hv, and_self, ↓reduceIte]; exact frame_base s c a b y ac ks h hv.1 hv.2.1 hv.2.2.1 hv.2.2.2
    · simp only [hv, ↓reduceIte]; exact h
  | lose => exact h

theorem step_vals (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c)
    (hv : Vals s c) (hc : C2 (s, c) op) : Vals (step (s, c) op).1 (step (s, c) op).2 := by
  have commitV : ∀ f : K → V, Vals (s.commit f) c := by
    intro f k e he
    have h1 := hv k e he
    have hg := Nat.le_trans (h.entry_gen k e he) h.known_le
    simp only [Server.commit]
    simp only [show ¬ (e.gen = s.gen + 1) by omega, ↓reduceIte]; exact h1
  cases op with
  | commit f => exact commitV f
  | revoke f => exact commitV f
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
  | pong t gp xp ap =>
    simp only [step]
    by_cases hin : (t, gp, xp, ap) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact contact_vals s c t gp xp ap h hv
    · simp only [hin, ↓reduceIte]; exact hv
  | reply k g y t gs xs ap =>
    simp only [step]
    by_cases hr : (t, gs, xs, ap) ∈ s.inflight ∧ g ≤ gs ∧ y ≤ xs
    · simp only [hr, and_self, ↓reduceIte]
      exact put_vals s _ k g y (contact_base s c t gs xs ap h hr.1) (contact_vals s c t gs xs ap h hv)
    · simp only [hr, ↓reduceIte]; exact hv
  | frame a b y ac ks =>
    simp only [step]
    by_cases hr : a ≤ b ∧ b ≤ s.gen ∧ y ≤ s.x ∧ ac ≤ s.acc
    · simp only [hr, and_self, ↓reduceIte]; exact frame_vals s c a b y ac ks h hv hc
    · simp only [hr, ↓reduceIte]; exact hv
  | lose => exact hv

theorem run_base (ops : List (Op K V)) :
    ∀ sc : Server K V × Cache K V, Base sc.1 sc.2 → Base (run sc ops).1 (run sc ops).2 := by
  induction ops with
  | nil => intro sc h; exact h
  | cons op ops ih => intro sc h; exact ih _ (step_base sc.1 sc.2 op h)

/-- **Coherence under C2**: every entry is the server's value at its own stamp, at every step of
    every schedule whose frames are complete.
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

/-- **A current answer is the server's opened value at the stamp last heard** (under C2, through
    `Vals`).
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

/-- **The whole stamp with carry-forward, in one statement**: in every schedule whose frames are
    complete (C2), a current answer is the server's opened value at the stamp last heard, and every
    generation and erasure epoch newer than that began within the last `B` ticks. -/
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

/-- **Nothing read before the last access change heard stays at rest.**
    Verifies: FE-015 -/
theorem rest_above_floor (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : c.floor ≤ e.gen :=
  h.entry_floor k e he

/-- Hearing an erasure epoch deletes, in the same step, every entry of an older one.
    Verifies: FE-012 -/
theorem hear_purges (s : Server K V) (c : Cache K V) (h : Base s c) (y : Nat) (k : K) (e : Entry V)
    (he : (hearX c y).entries k = some e) : y ≤ e.x := by
  have := (hearX_entry c y h.entry_x k e he).2
  rw [this]; exact Nat.le_max_right _ _

/-- Hearing an access change deletes, in the same step, every entry read before it.
    Verifies: FE-015 -/
theorem hear_access_purges (s : Server K V) (c : Cache K V) (h : Base s c) (a : Nat) (k : K)
    (e : Entry V) (he : (hearAcc c a).entries k = some e) : a ≤ e.gen :=
  Nat.le_trans (Nat.le_max_right _ _) (hearAcc_entry c a h.entry_floor k e he).2

set_option linter.unusedSimpArgs false in
/-- **A reply read and sent at the server's current stamp is read back as current**: the reply is
    itself a contact, dated by its request's send time.
    Verifies: FE-002 -/
theorem receipt_read (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (h : Base s c) (t : Nat)
    (ht : t ≤ s.now) (hB : s.now ≤ t + B) :
    read B s.now
      (run (s, c) [.answer t, .reply k s.gen s.x t s.gen s.x s.acc]).2 k = .current (s.view s.gen s.x k) := by
  let s1 : Server K V := { s with inflight := (t, s.gen, s.x, s.acc) :: s.inflight }
  have hin : (t, s.gen, s.x, s.acc) ∈ s1.inflight := List.mem_cons_self ..
  have hb1 : Base s1 c := step_base s c (.answer t) h |> fun hb => by
    simp only [step, ht, ↓reduceIte] at hb; exact hb
  have hc1 := contact_base s1 c t s.gen s.x s.acc hb1 hin
  obtain ⟨c1, c2, c3, c4⟩ := contact_fields c t s.gen s.x s.acc
  have hm : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
  have hmx : max c.knownX s.x = s.x := Nat.max_eq_right h.knownX_le
  have hfl : max c.floor s.acc = s.acc := Nat.max_eq_right h.floor_le
  have hct : max c.contact t ≥ t := Nat.le_max_right _ _
  have hacc : s.acc ≤ s.gen := h.acc_le
  show read B s.now (step (step (s, c) (.answer t)) (.reply k s.gen s.x t s.gen s.x s.acc)).2 k = _
  simp only [step, ht, ↓reduceIte]
  simp only [hin, Nat.le_refl, and_self, ↓reduceIte]
  -- the put is not dropped, and writes the entry
  have hnd : ¬ (s.x < (contactIn c t s.gen s.x s.acc).knownX ∨ s.gen < (contactIn c t s.gen s.x s.acc).floor) := by
    rw [c2, c3, hmx, hfl]; omega
  unfold put
  simp only [hnd, ↓reduceIte]
  have hx1 := hearX_fields (contactIn c t s.gen s.x s.acc) s.x
  have hent : ∀ e, (hearX (contactIn c t s.gen s.x s.acc) s.x).entries k = some e → e.gen ≤ s.gen := by
    intro e he
    have := (hearX_entry _ s.x hc1.entry_x k e he).1
    have := hc1.entry_gen k e this
    rw [c1, hm] at this; exact this
  unfold insertAt
  cases he : (hearX (contactIn c t s.gen s.x s.acc) s.x).entries k with
  | none =>
    simp only [read, setK, ↓reduceIte, hx1.1, hx1.2.2.1, hx1.2.2.2, c1, c2, c4, hm, hmx, Nat.max_self]
    have : s.now ≤ max c.contact t + B := by omega
    simp [this, setK]
  | some e =>
    have hle := hent e he
    have : ¬ s.gen < e.gen := by omega
    simp only [this, ↓reduceIte, read, setK, hx1.1, hx1.2.2.1, hx1.2.2.2, c1, c2, c4, hm, hmx, Nat.max_self]
    have : s.now ≤ max c.contact t + B := by omega
    simp [this, setK]

/-- **A read after a confirmed commit returns the committed value**: the receipt is a reply read and
    sent at the commit's generation and the current erasure epoch.
    Verifies: FE-002 -/
theorem read_after_commit (B : Nat) (s : Server K V) (c : Cache K V) (f : K → V) (k : K)
    (h : Base s c) (t : Nat) (ht : t ≤ s.now) (hB : s.now ≤ t + B) :
    read B s.now (run (s, c) [.commit f, .answer t,
      .reply k (s.gen + 1) s.x t (s.gen + 1) s.x s.acc]).2 k = .current (f k) := by
  have hb : Base (s.commit f) c := step_base s c (.commit f) h
  have hr := receipt_read B (s.commit f) c k hb t ht hB
  have hf : (s.commit f).view (s.commit f).gen (s.commit f).x k = f k := by simp [Server.commit]
  rw [hf] at hr
  exact hr

/-! ## What fails without each piece -/

def cxServer : Server Nat Nat :=
  { gen := 0, x := 0, acc := 0, view := fun g y _ => if y = 0 then (if g = 0 then 5 else 6) else 0,
    gTime := fun _ => 0, xTime := fun _ => 0, now := 0, inflight := [] }

def cxCache : Cache Nat Nat :=
  { known := 0, knownX := 0, floor := 0, contact := 0, entries := fun _ => some ⟨5, 0, 0⟩ }

/-- **Without C2, a stale value stays current.** -/
theorem incomplete_notice_serves_stale :
    read 1 0 (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 0 []]).2 0 = .current 5 ∧
    (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 0 []]).1.view 1 0 0 = 6 := by
  decide

/-- The review's schedule: after the incomplete frame, time passes and a fresh answer arrives, so the
    stamp is current and the contact recent, and the read still says current 5 while the server
    holds 6. `current_recent` bounds the lag of the stamp, never the value. -/
def cxLater : List (Op Nat Nat) :=
  [.commit (fun _ => 6), .frame 0 1 0 0 [], .tick, .tick, .tick, .tick, .tick, .answer 5, .pong 5 1 0 0]

theorem fresh_contact_still_stale :
    read 1 5 (run (cxServer, cxCache) cxLater).2 0 = .current 5 ∧
    (run (cxServer, cxCache) cxLater).2.contact = 5 ∧
    (run (cxServer, cxCache) cxLater).1.view (run (cxServer, cxCache) cxLater).1.gen 0 0 = 6 := by
  decide

/-- **A server that reads the erasure epoch after opening stamps plaintext as current.** -/
theorem stamp_after_open_serves_erased :
    let s := cxServer.destroy (fun _ _ => 0)
    let c := put cxCache 0 5 0 1
    read 1 0 c 0 = .current 5 ∧ c.knownX = s.x ∧ s.view 0 s.x 0 = 0 := by
  decide

/-- A put that does not compare with the persisted known epoch or floor. -/
def putUnguarded (c : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  { c with entries := setK c.entries k (some ⟨v, g, y⟩) }

/-- **Without the persisted epoch, a purge is undone.** -/
theorem unguarded_put_resurrects :
    let c := putUnguarded (hearX cxCache 1) 0 5 0 0
    ∃ e, c.entries 0 = some e ∧ e.x < c.knownX := by
  decide

/-- The guarded put drops the same late value. -/
theorem guarded_put_drops : (put (hearX cxCache 1) 0 5 0 0).entries 0 = none := by
  decide

/-- **Without the access floor, revoked data comes back**: after the client hears an access change
    at generation 1 (purging), a late reply read at generation 0, before the change, is written back
    to rest. -/
theorem unfloored_put_resurrects :
    let c := putUnguarded (hearAcc cxCache 1) 0 5 0 0
    ∃ e, c.entries 0 = some e ∧ e.gen < c.floor := by
  decide

/-- The floored put drops it. -/
theorem floored_put_drops : (put (hearAcc cxCache 1) 0 5 0 0).entries 0 = none := by
  decide

/-! ## The working set's keys -/

def ofWorlds (worlds : Nat → World) (gen now : Nat) (gTime : Nat → Nat) :
    Server WorkingSet.Key WorkingSet.Payload :=
  { gen := gen, x := 0, acc := 0, view := fun g _ k => WorkingSet.fetch (worlds g) k, gTime := gTime,
    xTime := fun _ => 0, now := now, inflight := [] }

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
