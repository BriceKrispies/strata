/-
The browser's cache of view results, against the server it copies (docs/frontend.md, "The cache
and its coherence").

The browser cannot read the server's generation the way a runtime can
(`WorkingSet.checkedRead_linearizes`). It learns it from what arrives: a read reply stamped with the
one generation and erasure epoch it was read at (the request-snapshot rule, `request_snapshot`),
and frames on the socket (an event notice, a heartbeat, the welcome), each carrying the server's
current stamp and the keys that may differ since an earlier generation. Its knowledge lags, so every
statement here is relative to what it has heard and to how long ago it last heard it.

One model holds the whole stamp. The generation `g` stands for the pair (tenant epoch, generation)
of the design, whose only property used is that a changed state never shows a stamp already seen
(C1). The erasure epoch `x` rises at every key destruction (D-350), which changes what an opened
read returns without changing `g`. A **contact** is a frame, or a reply read at the server's
current stamp; an answer is drawn as **current** only when its stamp is the newest heard and the
last contact is at most `B` ticks old. The client keeps its known stamp with its entries, and every
put compares with it and raises it in the same transaction (the persisted guarded put), so a late
reply or another tab's put of an older epoch is dropped.

Proved:

- **what a current answer may miss** (`current_recent`): every commit and every key destruction it
  does not reflect happened within the last `B` ticks. This needs no assumption on notices;
- **what a current answer is** (`current_value`, `current_answer`): under C2, the server's opened
  value at the stamp the client last heard; together with `current_recent`, a value no older than
  `B` ticks, erasures included;
- **nothing of an older erasure epoch stays at rest** (`rest_one_epoch`, `contact_purges`): every
  entry carries the epoch the client knows, and after any contact that is the server's;
- **a read after a confirmed commit** returns the committed value (`read_after_commit`);
- **what fails without each piece**: an incomplete notice keeps a stale value current
  (`incomplete_notice_serves_stale`); a server that reads the erasure epoch after opening stamps
  plaintext as erased-current (`stamp_after_open_serves_erased`); a put that does not compare with
  the persisted known epoch writes erased plaintext back after a purge (`unguarded_put_resurrects`);
  a cache keyed by the generation alone serves erased values (`browser_generation_only_serves_erased`,
  the working set's own counterexample).

Assumptions, all of them:

C1. The server's history is immutable: a commit defines the next generation, a key destruction the
    next erasure epoch, and neither changes a value at an earlier stamp.
C2. A frame is complete for the keys the client holds: every held key whose value may differ
    between the frame's `from` generation and the server's current one is in its key list. It is a
    hypothesis of `current_value` (`RunC2`), never filtered by the step: a frame that breaks it is
    applied, and `incomplete_notice_serves_stale` shows the result. The design obtains C2 by server
    work and, where a change cannot be notified (a policy reading the clock, a read following
    references), by marking the view volatile, which puts it in every frame's key list.
C3. A reply stamps the erasure epoch read before, or atomically with, opening its sealed fields
    (the value under stamp `(g, y)` is `view g y`). `stamp_after_open_serves_erased` is the boundary.
C4. Frames are delivered when sent; their latency is part of `B` in the design.

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
  /-- When the client last heard the server's current stamp. -/
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

/-- A frame carrying the server's current stamp `(b, y)` at time `now`: between `a` and `b` only
    `ks` may differ. Entries at `a` are carried to `b`, the keys named are dropped. -/
def frame (c : Cache K V) (a b y now : Nat) (ks : List K) : Cache K V :=
  let c1 := hearX c y
  { known := max c1.known b, knownX := c1.knownX, contact := now,
    entries := fun k =>
      match c1.entries k with
      | some e => if e.gen = a then (if k ∈ ks then none else some ⟨e.val, b, e.x⟩) else some e
      | none => none }

/-! ## Operations -/

inductive Op (K V : Type) where
  | commit (f : K → V)
  | destroy (er : Nat → K → V)
  | tick
  /-- A read reply, a commit receipt, or another tab's put, for key `k` read at `(g, y)`: any stamp
      the server has reached, so late and duplicated replies are included. -/
  | reply (k : K) (g y : Nat)
  /-- A frame: a notice from generation `a`, a heartbeat (no keys), or the welcome. -/
  | frame (a : Nat) (ks : List K)
  | lose

def step (sc : Server K V × Cache K V) : Op K V → Server K V × Cache K V
  | .commit f => (sc.1.commit f, sc.2)
  | .destroy er => (sc.1.destroy er, sc.2)
  | .tick => ({ sc.1 with now := sc.1.now + 1 }, sc.2)
  | .reply k g y =>
    if g ≤ sc.1.gen ∧ y ≤ sc.1.x then
      let c := put sc.2 k (sc.1.view g y k) g y
      (sc.1, if g = sc.1.gen ∧ y = sc.1.x then { c with contact := sc.1.now } else c)
    else sc
  | .frame a ks => (sc.1, frame sc.2 a sc.1.gen sc.1.x sc.1.now ks)
  | .lose => sc

def run : Server K V × Cache K V → List (Op K V) → Server K V × Cache K V
  | sc, [] => sc
  | sc, op :: ops => run (step sc op) ops

/-- C2 for one operation in the state it happens in. -/
def C2 (sc : Server K V × Cache K V) : Op K V → Prop
  | .frame a ks => ∀ k, k ∉ ks → sc.1.view a sc.1.x k = sc.1.view sc.1.gen sc.1.x k
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

/-- Every entry is the server's opened value at its own stamp. Kept under C2. -/
def Vals (s : Server K V) (c : Cache K V) : Prop :=
  ∀ k e, c.entries k = some e → e.val = s.view e.gen e.x k

theorem base_empty (s : Server K V) (h0 : s.gen = 0) (hx : s.x = 0) :
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
        fun k' e' he' => (h4 k' e' he').2, fun g' h5 h6 => ?_, fun y' h5 h6 => ?_⟩
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

theorem frame_base (s : Server K V) (c : Cache K V) (a : Nat) (ks : List K) (h : Base s c) :
    Base s (frame c a s.gen s.x s.now ks) := by
  have hle : c.knownX ≤ s.x := h.knownX_le
  have hkx := hearX_knownX c s.x hle
  have hkn := hearX_known c s.x
  have hent := hearX_entry c s.x h.entry_x hle
  have hmax : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
  refine ⟨?_, ?_, Nat.le_refl _, ?_, ?_, ?_, ?_⟩
  · simp only [frame, hkn, hmax]; exact Nat.le_refl _
  · simp only [frame, hkx]; exact Nat.le_refl _
  · intro k e he
    simp only [frame, hkn, hmax] at he ⊢
    cases hc : (hearX c s.x).entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      have h0 := h.entry_gen k e0 (hent k e0 hc).1
      split at he
      · split at he
        · cases he
        · simp only [Option.some.injEq] at he; subst he; exact Nat.le_refl _
      · simp only [Option.some.injEq] at he; subst he; exact Nat.le_trans h0 h.known_le
  · intro k e he
    simp only [frame, hkx] at he ⊢
    cases hc : (hearX c s.x).entries k with
    | none => rw [hc] at he; cases he
    | some e0 =>
      rw [hc] at he
      simp only at he
      have h0 := (hent k e0 hc).2
      split at he
      · split at he
        · cases he
        · simp only [Option.some.injEq] at he; subst he; exact h0
      · simp only [Option.some.injEq] at he; subst he; exact h0
  · intro g h1 h2; simp only [frame, hkn, hmax] at h1; omega
  · intro y h1 h2; simp only [frame, hkx] at h1; omega

theorem frame_vals (s : Server K V) (c : Cache K V) (a : Nat) (ks : List K) (h : Base s c)
    (hv : Vals s c) (hc2 : ∀ k, k ∉ ks → s.view a s.x k = s.view s.gen s.x k) :
    Vals s (frame c a s.gen s.x s.now ks) := by
  have hent := hearX_entry c s.x h.entry_x h.knownX_le
  intro k e he
  simp only [frame] at he
  cases hc : (hearX c s.x).entries k with
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

theorem step_base (s : Server K V) (c : Cache K V) (op : Op K V) (h : Base s c) :
    Base (step (s, c) op).1 (step (s, c) op).2 := by
  cases op with
  | commit f =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7⟩ := h
    refine ⟨by simp only [step, Server.commit]; omega, h2, h3, h4, h5, fun g hg1 hg2 => ?_, h7⟩
    simp only [step, Server.commit] at hg2 ⊢
    by_cases hg : g = s.gen + 1
    · simp only [hg, ↓reduceIte]; exact h3
    · simp only [hg, ↓reduceIte]; exact h6 g hg1 (by omega)
  | destroy er =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7⟩ := h
    refine ⟨h1, by simp only [step, Server.destroy]; omega, h3, h4, h5, h6, fun y hy1 hy2 => ?_⟩
    simp only [step, Server.destroy] at hy2 ⊢
    by_cases hy : y = s.x + 1
    · simp only [hy, ↓reduceIte]; exact h3
    · simp only [hy, ↓reduceIte]; exact h7 y hy1 (by omega)
  | tick =>
    obtain ⟨h1, h2, h3, h4, h5, h6, h7⟩ := h
    exact ⟨h1, h2, by simp only [step]; omega, h4, h5, h6, h7⟩
  | reply k g y =>
    simp only [step]
    by_cases hv : g ≤ s.gen ∧ y ≤ s.x
    · simp only [hv, and_self, ↓reduceIte]
      by_cases hcur : g = s.gen ∧ y = s.x
      · obtain ⟨rfl, rfl⟩ := hcur
        simp only [and_self, ↓reduceIte]
        have hp := put_base s c k (s.view s.gen s.x k) s.gen s.x (Nat.le_refl _) (Nat.le_refl _) h
        obtain ⟨hk, hkx, _⟩ := put_known_eq c k (s.view s.gen s.x k) s.gen s.x h.knownX_le
        have hm : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
        refine ⟨hp.known_le, hp.knownX_le, Nat.le_refl _, hp.entry_gen, hp.entry_x,
          fun g' a b => ?_, fun y' a b => ?_⟩
        · simp only at a; rw [hk, hm] at a; omega
        · simp only at a; rw [hkx] at a; omega
      · simp only [hcur, ↓reduceIte]; exact put_base s c k (s.view g y k) g y hv.1 hv.2 h
    · simp only [hv, ↓reduceIte]; exact h
  | frame a ks => exact frame_base s c a ks h
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
  | reply k g y =>
    simp only [step]
    by_cases hr : g ≤ s.gen ∧ y ≤ s.x
    · simp only [hr, and_self, ↓reduceIte]
      by_cases hcur : g = s.gen ∧ y = s.x
      · obtain ⟨rfl, rfl⟩ := hcur
        simp only [and_self, ↓reduceIte]
        exact put_vals s c k s.gen s.x h hv
      · simp only [hcur, ↓reduceIte]; exact put_vals s c k g y h hv
    · simp only [hr, ↓reduceIte]; exact hv
  | frame a ks => exact frame_vals s c a ks h hv hc
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

/-- **What a current answer may miss**: every commit and every key destruction it does not reflect
    began within the last `B` ticks. No assumption on frames.
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
    complete, a current answer is the server's opened value at the generation and erasure epoch
    last heard, and every commit and key destruction it misses began within the last `B` ticks. -/
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

/-- After any frame (a contact), every entry at rest carries the server's erasure epoch.
    Verifies: FE-012 -/
theorem contact_purges (s : Server K V) (c : Cache K V) (h : Base s c) (a : Nat) (ks : List K)
    (k : K) (e : Entry V) (he : (step (s, c) (.frame a ks)).2.entries k = some e) : e.x = s.x := by
  have hb := frame_base s c a ks h
  have := hb.entry_x k e he
  rw [this]
  simp only [frame]
  exact hearX_knownX c s.x h.knownX_le

/-- The same, after other traffic: a receipt at the server's current stamp is read back as current.
    Verifies: FE-002 -/
theorem receipt_read (B : Nat) (s : Server K V) (c : Cache K V) (k : K) (h : Base s c) :
    read B s.now (step (s, c) (.reply k s.gen s.x)).2 k = .current (s.view s.gen s.x k) := by
  have hle : c.knownX ≤ s.x := h.knownX_le
  have hent := hearX_entry c s.x h.entry_x hle
  obtain ⟨hk, hkx, _⟩ := put_known_eq c k (s.view s.gen s.x k) s.gen s.x hle
  have hm : max c.known s.gen = s.gen := Nat.max_eq_right h.known_le
  have he := put_entry_self c k (s.view s.gen s.x k) s.gen s.x hle (fun e he =>
    Nat.le_trans (h.entry_gen k e (hent k e he).1) h.known_le)
  simp only [step, Nat.le_refl, and_self, ↓reduceIte]
  simp [read, he, hk, hm, hkx]

/-- **A read after a confirmed commit returns the committed value**: the receipt is a reply at the
    commit's generation and the current erasure epoch.
    Verifies: FE-002 -/
theorem read_after_commit (B : Nat) (s : Server K V) (c : Cache K V) (f : K → V) (k : K)
    (h : Base s c) :
    read B s.now (run (s, c) [.commit f, .reply k (s.gen + 1) s.x]).2 k = .current (f k) := by
  have hb : Base (s.commit f) c := step_base s c (.commit f) h
  have hr := receipt_read B (s.commit f) c k hb
  have hf : (s.commit f).view (s.commit f).gen (s.commit f).x k = f k := by simp [Server.commit]
  rw [hf] at hr
  exact hr

/-! ## What fails without each piece -/

/-- A server holding 5 at generation 0 and 6 after, a client holding 5 at generation 0. -/
def cxServer : Server Nat Nat :=
  { gen := 0, x := 0, view := fun g y _ => if y = 0 then (if g = 0 then 5 else 6) else 0,
    gTime := fun _ => 0, xTime := fun _ => 0, now := 0 }

def cxCache : Cache Nat Nat := { known := 0, knownX := 0, contact := 0, entries := fun _ => some ⟨5, 0, 0⟩ }

/-- **Without C2, a stale value stays current**: after a commit to 6, a frame that does not name
    the key carries 5 forward, and the read says current 5 while the server holds 6. -/
theorem incomplete_notice_serves_stale :
    read 1 0 (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 []]).2 0 = .current 5 ∧
    (run (cxServer, cxCache) [.commit (fun _ => 6), .frame 0 []]).1.view 1 0 0 = 6 := by
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
    xTime := fun _ => 0, now := now }

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
