/-
The browser's cache of view results, against the server it copies (docs/frontend.md, "The cache
and its coherence").

The browser cannot read the server's generation the way a runtime can
(`WorkingSet.checkedRead_linearizes`). It learns stamps from what arrives, each after a delay it
cannot measure: a reply carries the stamp it was read at and the stamp current when it was sent; a
frame (a notice or a heartbeat) carries the stamp it was sent with and the keys that may differ
since an earlier generation; a pong carries the stamp current when the server answered, and echoes
the send time of the ping it answers. Only a pong is a **contact**, dated by the ping's send time,
which the browser measured itself. An answer is drawn as **current** when its stamp is the newest
heard and the last contact is at most `B` ticks old.

The generation `g` stands for the pair (tenant epoch, generation) of the design: its only property
used is that a stamp never repeats (C1). The erasure epoch `x` rises at every key destruction
(D-350), which changes what an opened read returns without changing `g`. The client keeps its known
stamp with its entries and every put compares with it and raises it in one transaction (the
persisted guarded put).

Proved:

- **the lag of the stamp** (`current_recent`): every commit and every key destruction that the
  stamp of a current answer has not heard of began within the last `B` ticks. This bounds the
  stamp, not the value: with an incomplete notice the value can be wrong however recent the
  contact (`fresh_contact_still_stale`);
- **the value, under C2** (`current_value`, `current_answer`): if every frame is complete, a
  current answer is the server's opened value at the stamp last heard;
- **nothing of an older erasure epoch stays at rest** (`rest_one_epoch`, `hear_purges`);
- **a read after a confirmed commit**, while the last contact is recent (`read_after_commit`,
  `receipt_read`);
- **what fails without each piece**: an incomplete notice (`incomplete_notice_serves_stale`,
  `fresh_contact_still_stale`); an epoch read after opening (`stamp_after_open_serves_erased`); a put
  that does not check the persisted epoch (`unguarded_put_resurrects`); the generation alone
  (`browser_generation_only_serves_erased`).

Assumptions, all of them:

C1. The server's history is immutable: a commit defines the next generation, a key destruction the
    next erasure epoch, and neither changes a value at an earlier stamp.
C2. A frame is complete: every key whose value differs between its `from` generation and its stamp
    is named. A hypothesis of `current_value` (`RunC2`), never a filter of the step.
C3. A reply stamps the erasure epoch read before, or atomically with, opening its sealed fields.
C4. The browser's clock does not run backwards between a ping and the reads that rely on its pong;
    a hidden or frozen page re-reads the time before it draws.

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
  /-- What an opened read at generation `g` and erasure epoch `y` returns. Meaningful for
      `g ≤ gen` and `y ≤ x`. -/
  view : Nat → Nat → K → V
  /-- When each generation, and each erasure epoch, began. -/
  gTime : Nat → Nat
  xTime : Nat → Nat
  now : Nat
  /-- Pongs sent and not yet delivered: the echoed send time of the ping, and the generation and
      erasure epoch current when the pong was sent. -/
  inflight : List (Nat × Nat × Nat)

variable {K V : Type} [DecidableEq K]

/-- A commit, by anyone, or any other change to what a read returns. -/
def Server.commit (s : Server K V) (f : K → V) : Server K V :=
  { s with gen := s.gen + 1, view := fun g y => if g = s.gen + 1 then f else s.view g y,
           gTime := fun g => if g = s.gen + 1 then s.now else s.gTime g }

/-- A key destruction: what an opened read returns becomes `er g` at every generation. -/
def Server.destroy (s : Server K V) (er : Nat → K → V) : Server K V :=
  { s with x := s.x + 1, view := fun g y => if y = s.x + 1 then er g else s.view g y,
           xTime := fun y => if y = s.x + 1 then s.now else s.xTime y }

/-! ## The client's cache -/

structure Entry (V : Type) where
  val : V
  gen : Nat
  x : Nat

structure Cache (K V : Type) where
  /-- The newest generation heard, and the newest erasure epoch heard. Persisted with the entries. -/
  known : Nat
  knownX : Nat
  /-- The send time of the newest ping whose pong has arrived (echoed by the pong). -/
  contact : Nat
  entries : K → Option (Entry V)

inductive Answer (V : Type) where
  | current (v : V)
  | stale (v : V) (gen : Nat)
  | miss
  deriving DecidableEq

def setK (f : K → Option (Entry V)) (k : K) (x : Option (Entry V)) : K → Option (Entry V) :=
  fun k' => if k' = k then x else f k'

/-- A read at time `now` with contact bound `B`. -/
def read (B now : Nat) (c : Cache K V) (k : K) : Answer V :=
  match c.entries k with
  | none => .miss
  | some e =>
    if e.gen = c.known ∧ e.x = c.knownX ∧ now ≤ c.contact + B then .current e.val
    else .stale e.val e.gen

/-- Hearing an erasure epoch: a newer one deletes every entry of an older one. -/
def hearX (c : Cache K V) (y : Nat) : Cache K V :=
  if c.knownX < y then
    { c with knownX := y,
             entries := fun k => match c.entries k with
               | some e => if e.x < y then none else some e
               | none => none }
  else c

/-- The persisted guarded put: one transaction reads the known stamp, drops a value of an older
    erasure epoch, raises the known stamp, and never replaces an entry read at a later generation. -/
def put (c : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  if y < c.knownX then c
  else
    let c1 := hearX c y
    match c1.entries k with
    | some e =>
      if g < e.gen then { c1 with known := max c1.known g }
      else { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y⟩) }
    | none => { c1 with known := max c1.known g, entries := setK c1.entries k (some ⟨v, g, y⟩) }

/-- A frame sent with stamp `(b, y)`: between `a` and `b` only `ks` may differ. A frame of an older
    erasure epoch than the client knows changes nothing; otherwise entries at `a` are carried to `b`
    and the keys named are dropped. A frame is not a contact: it says nothing of when it was sent. -/
def frame (c : Cache K V) (a b y : Nat) (ks : List K) : Cache K V :=
  if y < c.knownX then c
  else
    let c1 := hearX c y
    { known := max c1.known b, knownX := c1.knownX, contact := c1.contact,
      entries := fun k =>
        match c1.entries k with
        | some e => if e.gen = a then (if k ∈ ks then none else some ⟨e.val, b, e.x⟩) else some e
        | none => none }

/-- A pong: the server's stamp `(gp, xp)` when it answered the ping sent at `t`. The only contact. -/
def pongIn (c : Cache K V) (t gp xp : Nat) : Cache K V :=
  if xp < c.knownX then { c with known := max c.known gp, contact := max c.contact t }
  else
    let c1 := hearX c xp
    { c1 with known := max c1.known gp, contact := max c1.contact t }

/-- Raising the known generation to a stamp the server has reached. -/
def raise (c : Cache K V) (g : Nat) : Cache K V := { c with known := max c.known g }

/-! ## Operations -/

inductive Op (K V : Type) where
  | commit (f : K → V)
  | destroy (er : Nat → K → V)
  | tick
  /-- A read reply, a commit receipt, or another tab's put, for key `k`: read at `(g, y)` and sent
      when the server was at `(gs, xs)`. Late and duplicated replies are included. -/
  | reply (k : K) (g y gs xs : Nat)
  /-- A frame: a notice from generation `a`, sent with stamp `(b, y)`; a heartbeat names no keys. -/
  | frame (a b y : Nat) (ks : List K)
  /-- The server answers a ping the client sent at `t`. -/
  | answer (t : Nat)
  /-- A pong arrives, whenever: it echoes the ping's send time. -/
  | pong (t gp xp : Nat)
  | lose

/-- One step. The guards say only what a server can send (a stamp it has reached, a pong it sent);
    no guard asks whether a notice is complete. -/
def step (sc : Server K V × Cache K V) : Op K V → Server K V × Cache K V
  | .commit f => (sc.1.commit f, sc.2)
  | .destroy er => (sc.1.destroy er, sc.2)
  | .tick => ({ sc.1 with now := sc.1.now + 1 }, sc.2)
  | .reply k g y gs xs =>
    if g ≤ gs ∧ gs ≤ sc.1.gen ∧ y ≤ xs ∧ xs ≤ sc.1.x then
      (sc.1, raise (put (hearX sc.2 (max sc.2.knownX xs)) k (sc.1.view g y k) g y) gs)
    else sc
  | .frame a b y ks => if b ≤ sc.1.gen ∧ y ≤ sc.1.x then (sc.1, frame sc.2 a b y ks) else sc
  | .answer t =>
    if t ≤ sc.1.now then ({ sc.1 with inflight := (t, sc.1.gen, sc.1.x) :: sc.1.inflight }, sc.2)
    else sc
  | .pong t gp xp => if (t, gp, xp) ∈ sc.1.inflight then (sc.1, pongIn sc.2 t gp xp) else sc
  | .lose => sc

def run : Server K V × Cache K V → List (Op K V) → Server K V × Cache K V
  | sc, [] => sc
  | sc, op :: ops => run (step sc op) ops

/-- C2 for one operation: a frame names every key whose value differs between `a` and `b`. -/
def C2 (sc : Server K V × Cache K V) : Op K V → Prop
  | .frame a b y ks => ∀ k, k ∉ ks → sc.1.view a y k = sc.1.view b y k
  | _ => True

/-- C2 at every step of a schedule. -/
def RunC2 : Server K V × Cache K V → List (Op K V) → Prop
  | _, [] => True
  | sc, op :: ops => C2 sc op ∧ RunC2 (step sc op) ops

/-! ## Invariants -/

/-- What every step keeps, whatever the frames say. -/
structure Base (s : Server K V) (c : Cache K V) : Prop where
  known_le : c.known ≤ s.gen
  knownX_le : c.knownX ≤ s.x
  contact_le : c.contact ≤ s.now
  entry_gen : ∀ k e, c.entries k = some e → e.gen ≤ c.known
  entry_x : ∀ k e, c.entries k = some e → e.x = c.knownX
  /-- A generation the client has not heard of began after its last contact. -/
  gap_gen : ∀ g, c.known < g → g ≤ s.gen → c.contact ≤ s.gTime g
  /-- An erasure epoch the client has not heard of began after its last contact. -/
  gap_x : ∀ y, c.knownX < y → y ≤ s.x → c.contact ≤ s.xTime y
  /-- A pong in flight echoes a ping sent before every generation and epoch newer than its stamp. -/
  flight : ∀ p ∈ s.inflight, p.1 ≤ s.now ∧ p.2.1 ≤ s.gen ∧ p.2.2 ≤ s.x ∧
    (∀ g, p.2.1 < g → g ≤ s.gen → p.1 ≤ s.gTime g) ∧ (∀ y, p.2.2 < y → y ≤ s.x → p.1 ≤ s.xTime y)

/-- Every entry is the server's opened value at its own stamp. Kept under C2. -/
def Vals (s : Server K V) (c : Cache K V) : Prop :=
  ∀ k e, c.entries k = some e → e.val = s.view e.gen e.x k

theorem base_empty (s : Server K V) (h0 : s.gen = 0) (hx : s.x = 0) (hi : s.inflight = []) :
    Base s (⟨0, 0, 0, fun _ => none⟩ : Cache K V) := by
  constructor <;> intros <;> simp_all

/-! ### Lemmas on hearing an epoch -/

theorem hearX_known (c : Cache K V) (y : Nat) : (hearX c y).known = c.known := by
  unfold hearX; split <;> rfl

theorem hearX_contact (c : Cache K V) (y : Nat) : (hearX c y).contact = c.contact := by
  unfold hearX; split <;> rfl

theorem hearX_knownX (c : Cache K V) (y : Nat) (h : c.knownX ≤ y) : (hearX c y).knownX = y := by
  unfold hearX; split
  · rfl
  · omega

theorem hearX_entry (c : Cache K V) (y : Nat) (hx : ∀ k e, c.entries k = some e → e.x = c.knownX)
    (h : c.knownX ≤ y) (k : K) (e : Entry V) (he : (hearX c y).entries k = some e) :
    c.entries k = some e ∧ e.x = y := by
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
  · exact ⟨he, by have := hx k e he; omega⟩

/-! ### Base is kept by every step -/

theorem put_base (s : Server K V) (c : Cache K V) (k : K) (v : V) (g y : Nat) (hg : g ≤ s.gen)
    (hy : y ≤ s.x) (h : Base s c) : Base s (put c k v g y) := by
  unfold put
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact h
  · simp only [hlt, ↓reduceIte]
    have hle : c.knownX ≤ y := by omega
    have hkx := hearX_knownX c y hle
    have hkn := hearX_known c y
    have hct := hearX_contact c y
    have hent := hearX_entry c y h.entry_x hle
    -- the facts every branch needs
    have hb : ∀ (c2 : Cache K V), c2.known = max (hearX c y).known g → c2.knownX = y → c2.contact = c.contact →
        (∀ k' e', c2.entries k' = some e' → e'.gen ≤ c2.known ∧ e'.x = c2.knownX) → Base s c2 := by
      intro c2 h1 h2 h3 h4
      rw [hkn] at h1
      refine ⟨by rw [h1]; exact Nat.max_le.mpr ⟨h.known_le, hg⟩, by rw [h2]; exact hy,
        by rw [h3]; exact h.contact_le, fun k' e' he' => (h4 k' e' he').1,
        fun k' e' he' => (h4 k' e' he').2, fun g' h5 h6 => ?_, fun y' h5 h6 => ?_, h.flight⟩
      · rw [h3]; exact h.gap_gen g' (by rw [h1] at h5; omega) h6
      · rw [h3]; exact h.gap_x y' (by rw [h2] at h5; omega) h6
    have hold : ∀ k' e', (hearX c y).entries k' = some e' → e'.gen ≤ max (hearX c y).known g ∧ e'.x = y :=
      fun k' e' he' => by
        obtain ⟨h1, h2⟩ := hent k' e' he'
        rw [hkn]
        exact ⟨Nat.le_trans (h.entry_gen k' e' h1) (Nat.le_max_left _ _), h2⟩
    have hset : ∀ k' e', setK (hearX c y).entries k (some ⟨v, g, y⟩) k' = some e' →
        e'.gen ≤ max (hearX c y).known g ∧ e'.x = y := by
      intro k' e' he'
      simp only [setK] at he'
      by_cases hk : k' = k
      · simp only [hk, ↓reduceIte, Option.some.injEq] at he'
        subst he'; exact ⟨Nat.le_max_right _ _, rfl⟩
      · simp only [hk, ↓reduceIte] at he'; exact hold k' e' he'
    cases hc : (hearX c y).entries k with
    | none =>
      exact hb _ rfl hkx hct (fun k' e' he' => by simpa [hkx] using hset k' e' he')
    | some e =>
      simp only
      by_cases hge : g < e.gen
      · simp only [hge, ↓reduceIte]
        exact hb _ rfl hkx hct (fun k' e' he' => by simpa [hkx] using hold k' e' he')
      · simp only [hge, ↓reduceIte]
        exact hb _ rfl hkx hct (fun k' e' he' => by simpa [hkx] using hset k' e' he')

theorem put_known_eq (c : Cache K V) (k : K) (v : V) (g y : Nat) (hy : c.knownX ≤ y) :
    (put c k v g y).known = max c.known g ∧ (put c k v g y).knownX = y ∧
    (put c k v g y).contact = c.contact := by
  have hkx := hearX_knownX c y hy
  have hkn := hearX_known c y
  have hct := hearX_contact c y
  unfold put
  have hn : ¬ y < c.knownX := by omega
  simp only [hn, ↓reduceIte]
  cases (hearX c y).entries k with
  | none => exact ⟨by simp [hkn], hkx, hct⟩
  | some e =>
    simp only
    split
    · exact ⟨by simp [hkn], hkx, hct⟩
    · exact ⟨by simp [hkn], hkx, hct⟩

theorem put_entry_self (c : Cache K V) (k : K) (v : V) (g y : Nat) (hy : c.knownX ≤ y)
    (hg : ∀ e, (hearX c y).entries k = some e → e.gen ≤ g) :
    (put c k v g y).entries k = some ⟨v, g, y⟩ := by
  unfold put
  have hn : ¬ y < c.knownX := by omega
  simp only [hn, ↓reduceIte]
  cases hc : (hearX c y).entries k with
  | none => simp [setK]
  | some e =>
    have : ¬ g < e.gen := by have := hg e hc; omega
    simp [this, setK]

theorem put_known_ge (c : Cache K V) (k : K) (v : V) (g y : Nat) :
    c.known ≤ (put c k v g y).known ∧ c.knownX ≤ (put c k v g y).knownX := by
  by_cases hlt : y < c.knownX
  · unfold put; simp [hlt]
  · obtain ⟨h1, h2, _⟩ := put_known_eq c k v g y (by omega)
    rw [h1, h2]; exact ⟨Nat.le_max_left _ _, by omega⟩

theorem put_vals (s : Server K V) (c : Cache K V) (k : K) (g y : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (put c k (s.view g y k) g y) := by
  unfold put
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact hv
  · simp only [hlt, ↓reduceIte]
    have hent := hearX_entry c y h.entry_x (by omega)
    have hold : ∀ k' e', (hearX c y).entries k' = some e' → e'.val = s.view e'.gen e'.x k' :=
      fun k' e' he' => hv k' e' (hent k' e' he').1
    have hset : ∀ k' e', setK (hearX c y).entries k (some ⟨s.view g y k, g, y⟩) k' = some e' →
        e'.val = s.view e'.gen e'.x k' := by
      intro k' e' he'
      simp only [setK] at he'
      by_cases hk : k' = k
      · simp only [hk, ↓reduceIte, Option.some.injEq] at he'; subst he'; subst hk; rfl
      · simp only [hk, ↓reduceIte] at he'; exact hold k' e' he'
    cases hc : (hearX c y).entries k with
    | none => exact hset
    | some e =>
      simp only
      split
      · exact hold
      · exact hset

theorem hearX_base (s : Server K V) (c : Cache K V) (y : Nat) (h : Base s c) (hle : c.knownX ≤ y)
    (hy : y ≤ s.x) : Base s (hearX c y) := by
  have hkx := hearX_knownX c y hle
  have hkn := hearX_known c y
  have hct := hearX_contact c y
  have hent := hearX_entry c y h.entry_x hle
  refine ⟨by rw [hkn]; exact h.known_le, by rw [hkx]; exact hy, by rw [hct]; exact h.contact_le,
    fun k e he => ?_, fun k e he => ?_, fun g a b => ?_, fun y' a b => ?_, h.flight⟩
  · rw [hkn]; exact h.entry_gen k e (hent k e he).1
  · rw [hkx]; exact (hent k e he).2
  · rw [hct]; rw [hkn] at a; exact h.gap_gen g a b
  · rw [hct]; rw [hkx] at a; exact h.gap_x y' (by omega) b

theorem hearX_vals (s : Server K V) (c : Cache K V) (y : Nat) (h : Base s c) (hle : c.knownX ≤ y)
    (hv : Vals s c) : Vals s (hearX c y) :=
  fun k e he => hv k e (hearX_entry c y h.entry_x hle k e he).1

theorem raise_base (s : Server K V) (c : Cache K V) (g : Nat) (h : Base s c) (hg : g ≤ s.gen) :
    Base s (raise c g) :=
  ⟨Nat.max_le.mpr ⟨h.known_le, hg⟩, h.knownX_le, h.contact_le,
   fun k e he => Nat.le_trans (h.entry_gen k e he) (Nat.le_max_left _ _), h.entry_x,
   fun g' a b => h.gap_gen g' (by simp only [raise] at a; omega) b, h.gap_x, h.flight⟩

theorem frame_base (s : Server K V) (c : Cache K V) (a b y : Nat) (ks : List K) (h : Base s c)
    (hb : b ≤ s.gen) (hy : y ≤ s.x) : Base s (frame c a b y ks) := by
  unfold frame
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact h
  · simp only [hlt, ↓reduceIte]
    have hle : c.knownX ≤ y := by omega
    have h1 := hearX_base s c y h hle hy
    refine ⟨Nat.max_le.mpr ⟨h1.known_le, hb⟩, h1.knownX_le, h1.contact_le, fun k e he => ?_,
      fun k e he => ?_, fun g a' b' => h1.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) a') b', h1.gap_x, h1.flight⟩
    · simp only at he
      cases hc : (hearX c y).entries k with
      | none => rw [hc] at he; cases he
      | some e0 =>
        rw [hc] at he
        simp only at he
        have h0 := h1.entry_gen k e0 hc
        split at he
        · split at he
          · cases he
          · simp only [Option.some.injEq] at he; subst he; exact Nat.le_max_right _ _
        · simp only [Option.some.injEq] at he; subst he; exact Nat.le_trans h0 (Nat.le_max_left _ _)
    · simp only at he
      cases hc : (hearX c y).entries k with
      | none => rw [hc] at he; cases he
      | some e0 =>
        rw [hc] at he
        simp only at he
        have h0 := h1.entry_x k e0 hc
        split at he
        · split at he
          · cases he
          · simp only [Option.some.injEq] at he; subst he; exact h0
        · simp only [Option.some.injEq] at he; subst he; exact h0

theorem frame_vals (s : Server K V) (c : Cache K V) (a b y : Nat) (ks : List K) (h : Base s c)
    (hv : Vals s c) (hc2 : ∀ k, k ∉ ks → s.view a y k = s.view b y k) :
    Vals s (frame c a b y ks) := by
  unfold frame
  by_cases hlt : y < c.knownX
  · simp only [hlt, ↓reduceIte]; exact hv
  · simp only [hlt, ↓reduceIte]
    have hent := hearX_entry c y h.entry_x (by omega)
    intro k e he
    simp only at he
    cases hc : (hearX c y).entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      obtain ⟨h1, h2⟩ := hent k e0 hc
      have hv0 := hv k e0 h1
      by_cases hg : e0.gen = a
      · simp only [hg, ↓reduceIte] at he
        by_cases hm : k ∈ ks
        · simp [hm] at he
        · simp only [hm, ↓reduceIte, Option.some.injEq] at he
          subst he
          simp only
          rw [hv0, hg, h2]
          exact hc2 k hm
      · simp only [hg, ↓reduceIte, Option.some.injEq] at he
        subst he; exact hv0

theorem pong_base (s : Server K V) (c : Cache K V) (t gp xp : Nat) (h : Base s c)
    (hin : (t, gp, xp) ∈ s.inflight) : Base s (pongIn c t gp xp) := by
  obtain ⟨ht, hgp, hxp, hgg, hgx⟩ := h.flight _ hin
  unfold pongIn
  by_cases hlt : xp < c.knownX
  · simp only [hlt, ↓reduceIte]
    refine ⟨Nat.max_le.mpr ⟨h.known_le, hgp⟩, h.knownX_le, Nat.max_le.mpr ⟨h.contact_le, ht⟩,
      fun k e he => Nat.le_trans (h.entry_gen k e he) (Nat.le_max_left _ _), h.entry_x,
      fun g a b => ?_, fun y a b => ?_, h.flight⟩
    · have a' : max c.known gp < g := a
      exact Nat.max_le.mpr ⟨h.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) a') b,
        hgg g (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) a') b⟩
    · have a' : c.knownX < y := a
      exact Nat.max_le.mpr ⟨h.gap_x y a' b, hgx y (Nat.lt_trans hlt a') b⟩
  · simp only [hlt, ↓reduceIte]
    have hle : c.knownX ≤ xp := by omega
    have h1 := hearX_base s c xp h hle hxp
    have hkx := hearX_knownX c xp hle
    have hkn := hearX_known c xp
    have hct := hearX_contact c xp
    refine ⟨Nat.max_le.mpr ⟨h1.known_le, hgp⟩, h1.knownX_le, Nat.max_le.mpr ⟨h1.contact_le, ht⟩,
      fun k e he => Nat.le_trans (h1.entry_gen k e he) (Nat.le_max_left _ _), h1.entry_x,
      fun g a b => ?_, fun y a b => ?_, h1.flight⟩
    · have a' : max (hearX c xp).known gp < g := a
      rw [hkn] at a'
      show max (hearX c xp).contact t ≤ s.gTime g
      rw [hct]
      exact Nat.max_le.mpr ⟨h.gap_gen g (Nat.lt_of_le_of_lt (Nat.le_max_left _ _) a') b,
        hgg g (Nat.lt_of_le_of_lt (Nat.le_max_right _ _) a') b⟩
    · have a' : (hearX c xp).knownX < y := a
      rw [hkx] at a'
      show max (hearX c xp).contact t ≤ s.xTime y
      rw [hct]
      exact Nat.max_le.mpr ⟨h.gap_x y (Nat.lt_of_le_of_lt hle a') b, hgx y a' b⟩

theorem pong_vals (s : Server K V) (c : Cache K V) (t gp xp : Nat) (h : Base s c) (hv : Vals s c) :
    Vals s (pongIn c t gp xp) := by
  unfold pongIn
  by_cases hlt : xp < c.knownX
  · simp only [hlt, ↓reduceIte]; exact hv
  · simp only [hlt, ↓reduceIte]
    exact hearX_vals s c xp h (by omega) hv

theorem reply_base (s : Server K V) (c : Cache K V) (k : K) (g y gs xs : Nat) (h : Base s c)
    (hv : g ≤ gs ∧ gs ≤ s.gen ∧ y ≤ xs ∧ xs ≤ s.x) :
    Base s (raise (put (hearX c (max c.knownX xs)) k (s.view g y k) g y) gs) := by
  have h1 := hearX_base s c (max c.knownX xs) h (Nat.le_max_left _ _)
    (Nat.max_le.mpr ⟨h.knownX_le, hv.2.2.2⟩)
  exact raise_base s _ gs (put_base s _ k _ g y (Nat.le_trans hv.1 hv.2.1) (Nat.le_trans hv.2.2.1 hv.2.2.2) h1) hv.2.1

theorem reply_vals (s : Server K V) (c : Cache K V) (k : K) (g y xs : Nat) (h : Base s c)
    (hv : Vals s c) (hxs : xs ≤ s.x) (gs : Nat) :
    Vals s (raise (put (hearX c (max c.knownX xs)) k (s.view g y k) g y) gs) := by
  have h1 := hearX_base s c (max c.knownX xs) h (Nat.le_max_left _ _)
    (Nat.max_le.mpr ⟨h.knownX_le, hxs⟩)
  have hv1 := hearX_vals s c (max c.knownX xs) h (Nat.le_max_left c.knownX xs) hv
  exact put_vals s _ k g y h1 hv1

theorem step_base (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c) :
    Base (step (s, c) op).1 (step (s, c) op).2 := by
  cases op with
  | commit f =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
    refine ⟨by simp only [step, Server.commit]; omega, h2, h3, h4, h5, fun g hg1 hg2 => ?_, h7,
      fun p hp => ?_⟩
    · simp only [step, Server.commit] at hg2 ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact h3
      · simp only [hg, ↓reduceIte]; exact h6 g hg1 (by omega)
    · obtain ⟨a1, a2, a3, a4, a5⟩ := h8 p hp
      refine ⟨a1, by simp only [step, Server.commit]; omega, a3, fun g b1 b2 => ?_, a5⟩
      simp only [step, Server.commit] at b2 ⊢
      by_cases hg : g = s.gen + 1
      · simp only [hg, ↓reduceIte]; exact a1
      · simp only [hg, ↓reduceIte]; exact a4 g b1 (by omega)
  | destroy er =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
    refine ⟨h1, by simp only [step, Server.destroy]; omega, h3, h4, h5, h6, fun y hy1 hy2 => ?_,
      fun p hp => ?_⟩
    · simp only [step, Server.destroy] at hy2 ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact h3
      · simp only [hy, ↓reduceIte]; exact h7 y hy1 (by omega)
    · obtain ⟨a1, a2, a3, a4, a5⟩ := h8 p hp
      refine ⟨a1, a2, by simp only [step, Server.destroy]; omega, a4, fun y b1 b2 => ?_⟩
      simp only [step, Server.destroy] at b2 ⊢
      by_cases hy : y = s.x + 1
      · simp only [hy, ↓reduceIte]; exact a1
      · simp only [hy, ↓reduceIte]; exact a5 y b1 (by omega)
  | tick =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
    exact ⟨h1, h2, by simp only [step]; omega, h4, h5, h6, h7, fun p hp => by
      obtain ⟨a1, a2, a3, a4, a5⟩ := h8 p hp
      exact ⟨by simp only [step]; omega, a2, a3, a4, a5⟩⟩
  | reply k g y gs xs =>
    simp only [step]
    by_cases hv : g ≤ gs ∧ gs ≤ s.gen ∧ y ≤ xs ∧ xs ≤ s.x
    · simp only [hv, and_self, ↓reduceIte]; exact reply_base s c k g y gs xs h hv
    · simp only [hv, ↓reduceIte]; exact h
  | frame a b y ks =>
    simp only [step]
    by_cases hv : b ≤ s.gen ∧ y ≤ s.x
    · simp only [hv, and_self, ↓reduceIte]; exact frame_base s c a b y ks h hv.1 hv.2
    · simp only [hv, ↓reduceIte]; exact h
  | answer t =>
    simp only [step]
    by_cases ht : t ≤ s.now
    · simp only [ht, ↓reduceIte]
      obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
      refine ⟨h1, h2, h3, h4, h5, h6, h7, fun p hp => ?_⟩
      rcases List.mem_cons.mp hp with hp | hp
      · subst hp
        exact ⟨ht, Nat.le_refl _, Nat.le_refl _, fun g a b => by simp only at a b; omega, fun y a b => by simp only at a b; omega⟩
      · exact h8 p hp
    · simp only [ht, ↓reduceIte]; exact h
  | pong t gp xp =>
    simp only [step]
    by_cases hin : (t, gp, xp) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact pong_base s c t gp xp h hin
    · simp only [hin, ↓reduceIte]; exact h
  | lose => exact h

theorem step_vals (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c)
    (hv : Vals s c) (hc : C2 (s, c) op) : Vals (step (s, c) op).1 (step (s, c) op).2 := by
  cases op with
  | commit f =>
    intro k e he
    have h1 := hv k e he
    have hg := Nat.le_trans (h.entry_gen k e he) h.known_le
    simp only [step, Server.commit]
    simp only [show ¬ (e.gen = s.gen + 1) by omega, ↓reduceIte]; exact h1
  | destroy er =>
    intro k e he
    have h1 := hv k e he
    have hx : e.x ≤ s.x := by rw [h.entry_x k e he]; exact h.knownX_le
    simp only [step, Server.destroy]
    simp only [show ¬ (e.x = s.x + 1) by omega, ↓reduceIte]; exact h1
  | tick => exact hv
  | reply k g y gs xs =>
    simp only [step]
    by_cases hr : g ≤ gs ∧ gs ≤ s.gen ∧ y ≤ xs ∧ xs ≤ s.x
    · simp only [hr, and_self, ↓reduceIte]; exact reply_vals s c k g y xs h hv hr.2.2.2 gs
    · simp only [hr, ↓reduceIte]; exact hv
  | frame a b y ks =>
    simp only [step]
    by_cases hr : b ≤ s.gen ∧ y ≤ s.x
    · simp only [hr, and_self, ↓reduceIte]; exact frame_vals s c a b y ks h hv hc
    · simp only [hr, ↓reduceIte]; exact hv
  | answer t =>
    simp only [step]
    by_cases ht : t ≤ s.now
    · simp only [ht, ↓reduceIte]; exact hv
    · simp only [ht, ↓reduceIte]; exact hv
  | pong t gp xp =>
    simp only [step]
    by_cases hin : (t, gp, xp) ∈ s.inflight
    · simp only [hin, ↓reduceIte]; exact pong_vals s c t gp xp h hv
    · simp only [hin, ↓reduceIte]; exact hv
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
    value: with an incomplete frame the value can be wrong however recent the contact
    (`fresh_contact_still_stale`). No assumption on frames.
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
    by_cases hcond : e.gen = c.known ∧ e.x = c.knownX ∧ s.now ≤ c.contact + B
    · have hn := hcond.2.2
      exact ⟨fun g a b => by have := h.gap_gen g a b; omega,
             fun y a b => by have := h.gap_x y a b; omega⟩
    · simp only [hcond, ↓reduceIte] at hr; cases hr

/-- **A current answer is the server's opened value at the stamp last heard** (under C2, through
    `Vals`).
    Verifies: FE-001 -/
theorem current_value (B now : Nat) (s : Server K V) (c : Cache K V) (k : K) (v : V)
    (hv : Vals s c) (hr : read B now c k = .current v) : v = s.view c.known c.knownX k := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hcond : e.gen = c.known ∧ e.x = c.knownX ∧ now ≤ c.contact + B
    · simp only [hcond, and_self, ↓reduceIte, Answer.current.injEq] at hr
      rw [← hr, hv k e hc, hcond.1, hcond.2.1]
    · simp only [hcond, ↓reduceIte] at hr; cases hr

/-- **The whole stamp with carry-forward, in one statement**: in every schedule whose frames are
    complete (C2), a current answer is the server's opened value at the generation and erasure
    epoch last heard, and every generation and erasure epoch newer than that began within the last
    `B` ticks. The value part rests on C2; only the stamp part holds without it. -/
theorem current_answer (B : Nat) (ops : List (Op K V)) (sc : Server K V × Cache K V)
    (h : Base sc.1 sc.2) (hv : Vals sc.1 sc.2) (hr2 : RunC2 sc ops) (k : K) (v : V)
    (hr : read B (run sc ops).1.now (run sc ops).2 k = .current v) :
    v = (run sc ops).1.view (run sc ops).2.known (run sc ops).2.knownX k ∧
    (∀ g, (run sc ops).2.known < g → g ≤ (run sc ops).1.gen → (run sc ops).1.now ≤ (run sc ops).1.gTime g + B) ∧
    (∀ y, (run sc ops).2.knownX < y → y ≤ (run sc ops).1.x → (run sc ops).1.now ≤ (run sc ops).1.xTime y + B) :=
  ⟨current_value B _ _ _ k v (run_coh ops sc h hv hr2) hr,
   current_recent B _ _ k v (run_base ops sc h) hr⟩

/-- **Anything else held is labelled**: a non-current answer carries its generation; under `Base`
    it is never ahead of what the client heard.
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
    by_cases hcond : e.gen = c.known ∧ e.x = c.knownX ∧ now ≤ c.contact + B
    · simp only [hcond, and_self, ↓reduceIte] at hr; cases hr
    · simp only [hcond, ↓reduceIte, Answer.stale.injEq] at hr
      obtain ⟨hv', hg⟩ := hr
      exact ⟨by rw [← hg]; exact h.entry_gen k e hc, e, rfl, hg, hv'⟩

/-- **Nothing of an older erasure epoch stays at rest**: every entry carries the epoch the client
    knows.
    Verifies: FE-012 -/
theorem rest_one_epoch (s : Server K V) (c : Cache K V) (h : Base s c) (k : K) (e : Entry V)
    (he : c.entries k = some e) : e.x = c.knownX :=
  h.entry_x k e he

/-- Hearing an erasure epoch (from a frame, a pong, or a reply's sent stamp) deletes, in the same
    step, every entry of an older one.
    Verifies: FE-012 -/
theorem hear_purges (s : Server K V) (c : Cache K V) (h : Base s c) (y : Nat) (hle : c.knownX ≤ y)
    (k : K) (e : Entry V) (he : (hearX c y).entries k = some e) : e.x = y :=
  (hearX_entry c y h.entry_x hle k e he).2

/-- A receipt read and sent at the server's current stamp is read back as current, while the last
    contact (a pong) is within the bound.
    Verifies: FE-002 -/
theorem receipt_read (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (h : Base s c)
    (hc : s.now ≤ c.contact + B) :
    read B s.now (step (s, c) (.reply k s.gen s.x s.gen s.x)).2 k = .current (s.view s.gen s.x k) := by
  have hmx : max c.knownX s.x = s.x := Nat.max_eq_right h.knownX_le
  have h1 := hearX_base s c s.x h h.knownX_le (Nat.le_refl _)
  have hkx1 := hearX_knownX c s.x h.knownX_le
  have hkn1 := hearX_known c s.x
  have hct1 := hearX_contact c s.x
  have hent := hearX_entry c s.x h.entry_x h.knownX_le
  obtain ⟨hk, hkx, hct⟩ := put_known_eq (hearX c s.x) k (s.view s.gen s.x k) s.gen s.x (by simp [hkx1])
  have he := put_entry_self (hearX c s.x) k (s.view s.gen s.x k) s.gen s.x (by simp [hkx1])
    (fun e he => by
      have h0 := (hearX_entry (hearX c s.x) s.x h1.entry_x (by simp [hkx1]) k e he).1
      have := h1.entry_gen k e h0
      rw [hkn1] at this; exact Nat.le_trans this h.known_le)
  have hm : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
  simp only [step, Nat.le_refl, and_self, ↓reduceIte, hmx]
  simp only [read, raise, he, hk, hkx, hct, hkn1, hm, hct1, Nat.max_self]
  simp [hc]

/-- **A read after a confirmed commit returns the committed value**: the receipt is a reply read
    and sent at the commit's generation and the current erasure epoch.
    Verifies: FE-002 -/
theorem read_after_commit (B : Nat) (s : Server K V) (c : Cache K V) (f : K → V) (k : K)
    (h : Base s c) (hc : s.now ≤ c.contact + B) :
    read B s.now (run (s, c) [.commit f, .reply k (s.gen + 1) s.x (s.gen + 1) s.x]).2 k = .current (f k) := by
  have hb : Base (s.commit f) c := step_base s c (.commit f) h
  have hr := receipt_read B (s.commit f) c k hb hc
  have hf : (s.commit f).view (s.commit f).gen (s.commit f).x k = f k := by simp [Server.commit]
  rw [hf] at hr
  exact hr

/-! ## What fails without each piece -/

/-- A server holding 5 at generation 0 and 6 after, a client holding 5 at generation 0. -/
def cxServer : Server Nat Nat :=
  { gen := 0, x := 0, view := fun g y _ => if y = 0 then (if g = 0 then 5 else 6) else 0,
    gTime := fun _ => 0, xTime := fun _ => 0, now := 0, inflight := [] }

def cxCache : Cache Nat Nat := { known := 0, knownX := 0, contact := 0, entries := fun _ => some ⟨5, 0, 0⟩ }

/-- **Without C2, a stale value stays current**: after a commit to 6, a frame that does not name
    the key carries 5 forward, and the read says current 5 while the server holds 6. -/
theorem incomplete_notice_serves_stale :
    read 1 0 (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 []]).2 0 = .current 5 ∧
    (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 1 0 []]).1.view 1 0 0 = 6 := by
  decide

/-- The schedule of the review: after the incomplete frame, time passes and fresh pongs keep
    arriving, so the stamp is current and the contact recent, and the read still says current 5
    while the server holds 6. `current_recent` bounds the lag of the stamp, never the value. -/
def cxLater : List (Op Nat Nat) :=
  [.commit (fun _ => 6), .frame 0 1 0 [], .tick, .tick, .tick, .tick, .tick, .answer 5, .pong 5 1 0]

theorem fresh_contact_still_stale :
    read 1 5 (run (cxServer, cxCache) cxLater).2 0 = .current 5 ∧
    (run (cxServer, cxCache) cxLater).2.contact = 5 ∧
    (run (cxServer, cxCache) cxLater).1.view (run (cxServer, cxCache) cxLater).1.gen 0 0 = 6 := by
  decide

/-- **A server that reads the erasure epoch after opening stamps plaintext as current**: after a
    destruction (epoch 1, opened value 0), a reply carrying the plaintext 5 stamped with epoch 1 is
    read as current 5, with no lag: the client already knows epoch 1. -/
theorem stamp_after_open_serves_erased :
    let s := cxServer.destroy (fun _ _ => 0)
    let c := put cxCache 0 5 0 1
    read 1 0 c 0 = .current 5 ∧ c.knownX = s.x ∧ s.view 0 s.x 0 = 0 := by
  decide

/-- A put that does not compare with the persisted known epoch. -/
def putUnguarded (c : Cache K V) (k : K) (v : V) (g y : Nat) : Cache K V :=
  { c with entries := setK c.entries k (some ⟨v, g, y⟩) }

/-- **Without the persisted guard, a purge is undone**: after the client hears epoch 1 (purging), a
    late put of epoch 0 writes the erased plaintext back to rest. -/
theorem unguarded_put_resurrects :
    let c := putUnguarded (hearX cxCache 1) 0 5 0 0
    ∃ e, c.entries 0 = some e ∧ e.x < c.knownX := by
  decide

/-- The guarded put drops the same late value. -/
theorem guarded_put_drops : (put (hearX cxCache 1) 0 5 0 0).entries 0 = none := by
  decide

/-! ## The working set's keys -/

/-- A server whose generations are the worlds `worlds 0, worlds 1, ...` (no erasures). -/
def ofWorlds (worlds : Nat → World) (gen now : Nat) (gTime : Nat → Nat) :
    Server WorkingSet.Key WorkingSet.Payload :=
  { gen := gen, x := 0, view := fun g _ k => WorkingSet.fetch (worlds g) k, gTime := gTime,
    xTime := fun _ => 0, now := now, inflight := [] }

theorem current_fetch (B now : Nat) (worlds : Nat → World) (gen n : Nat) (gT : Nat → Nat)
    (c : Cache WorkingSet.Key WorkingSet.Payload) (k : WorkingSet.Key) (v : WorkingSet.Payload)
    (hv : Vals (ofWorlds worlds gen n gT) c) (hr : read B now c k = .current v) :
    v = WorkingSet.fetch (worlds c.known) k :=
  current_value B now _ c k v hv hr

open WorkingSet in
/-- **A browser cache stamped by generation alone serves erased values.** This is the working
    set's counterexample; the browser cache holds opened values, which is `runAbove`. -/
theorem browser_generation_only_serves_erased (s : SealedStore) (k : Key)
    (hlive : s.keys (s.vals k).key = true) (hplain : (s.vals k).plain ≠ .record none) :
    runAbove [] s [.admitEntry k, .destroy (s.vals k).key, .read k] ≠
      runTruth s [.admitEntry k, .destroy (s.vals k).key, .read k] :=
  above_sealing_serves_erased s k hlive hplain

end Strata.Client.Cache
