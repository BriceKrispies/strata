/-
Machine-checked properties of the abstract double-entry ledger (D-8432, LDG-2929 to LDG-2932).

The model is the step function of the journal, stated apart from the Rust code (the kernel is
tested against it, and the proofs do not replace those tests):

* an entry names an account, a unit, a signed amount and the key of the transaction that wrote it;
* a transaction is an idempotency key and legs, each a signed amount to an account of a unit;
* `step` replays a transaction whose key the journal already holds, refuses one whose key was used
  for another request, refuses one whose legs do not net to zero for each unit, refuses one that
  takes a touched account past its floor or cap, and otherwise appends one entry per leg.

Assumptions, all of them: the floor and the cap of an account are fixed while a step runs (a
change of limits is a schema change, which is not a step of the journal), and a posting may leave
a balance outside a limit only when it moves a balance that is already outside it back toward it
(LDG-013).

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

namespace Strata.Ledger

/-- One leg of a transaction: a signed amount for the account of a unit. -/
structure Leg where
  account : Nat
  unit : Nat
  amount : Int
  deriving DecidableEq

/-- One entry of the journal; `key` is the idempotency key of the transaction that wrote it. -/
structure Entry where
  account : Nat
  unit : Nat
  amount : Int
  key : Nat
  deriving DecidableEq

/-- What a transaction asks of the ledger. -/
structure Tx where
  key : Nat
  legs : List Leg

/-- The ledger: its journal, oldest entry first, and the limits that hold for its accounts. -/
structure State where
  journal : List Entry
  floor : Nat → Nat → Option Int
  cap : Nat → Nat → Option Int

/-- The entries a transaction writes: one per leg, in order. -/
def entriesOf (t : Tx) : List Entry :=
  t.legs.map fun l => ⟨l.account, l.unit, l.amount, t.key⟩

/-- The sum of the amounts of the entries of a unit. -/
def total (j : List Entry) (u : Nat) : Int :=
  ((j.filter fun e => e.unit == u).map Entry.amount).sum

/-- The balance of an account of a unit: the sum of its entries. -/
def balance (j : List Entry) (a u : Nat) : Int :=
  ((j.filter fun e => e.account == a && e.unit == u).map Entry.amount).sum

/-- The entries the journal holds under a key. -/
def recorded (j : List Entry) (k : Nat) : List Entry :=
  j.filter fun e => e.key == k

/-- The sum of the amounts of the legs of a unit. -/
def legSum (ls : List Leg) (u : Nat) : Int :=
  ((ls.filter fun l => l.unit == u).map Leg.amount).sum

/-- A transaction has at least two legs, and its legs net to zero for each unit (LDG-2903). -/
def balanced (t : Tx) : Bool :=
  decide (2 ≤ t.legs.length) && t.legs.all fun l => legSum t.legs l.unit == 0

/-- An account moves from `b0` to `b1` within its floor and cap, or moves toward them from outside
(LDG-013). -/
def rangeOk (fl cp : Option Int) (b0 b1 : Int) : Bool :=
  (match fl with
    | none => true
    | some f => decide (f ≤ b1) || (decide (b0 < f) && decide (b0 ≤ b1))) &&
  (match cp with
    | none => true
    | some c => decide (b1 ≤ c) || (decide (c < b0) && decide (b1 ≤ b0)))

/-- Every account a transaction names stays within its limits (LDG-2932). -/
def limitsOk (s : State) (t : Tx) : Bool :=
  t.legs.all fun l =>
    rangeOk (s.floor l.account l.unit) (s.cap l.account l.unit)
      (balance s.journal l.account l.unit)
      (balance (s.journal ++ entriesOf t) l.account l.unit)

/-- Why a step is refused. -/
inductive Refusal where
  | mismatch
  | unbalanced
  | outOfRange

/-- The step function: the new state and whether the transaction was a replay. -/
def step (s : State) (t : Tx) : Except Refusal (State × Bool) :=
  if recorded s.journal t.key ≠ [] then
    if recorded s.journal t.key = entriesOf t then .ok (s, true) else .error .mismatch
  else if balanced t = false then .error .unbalanced
  else if limitsOk s t = false then .error .outOfRange
  else .ok ({ s with journal := s.journal ++ entriesOf t }, false)

/-! ## Facts about the sums -/

theorem total_append (j k : List Entry) (u : Nat) : total (j ++ k) u = total j u + total k u := by
  simp [total, List.filter_append, List.map_append, List.sum_append]

theorem balance_append (j k : List Entry) (a u : Nat) :
    balance (j ++ k) a u = balance j a u + balance k a u := by
  simp [balance, List.filter_append, List.map_append, List.sum_append]

theorem total_entriesOf (t : Tx) (u : Nat) : total (entriesOf t) u = legSum t.legs u := by
  unfold entriesOf total legSum
  induction t.legs with
  | nil => simp
  | cons l ls ih =>
    by_cases h : l.unit == u
    · simp [h, ih]
    · simp [h, ih]

theorem legSum_zero (t : Tx) (h : balanced t = true) (u : Nat) : legSum t.legs u = 0 := by
  by_cases hu : ∃ l ∈ t.legs, l.unit = u
  · obtain ⟨l, hl, rfl⟩ := hu
    simp only [balanced, Bool.and_eq_true, List.all_eq_true, beq_iff_eq] at h
    exact h.2 l hl
  · have : t.legs.filter (fun l => l.unit == u) = [] := by
      apply List.filter_eq_nil_iff.mpr
      intro l hl
      simp only [beq_iff_eq]
      exact fun e => hu ⟨l, hl, e⟩
    simp [legSum, this]

theorem balance_map_untouched (k : Nat) (a u : Nat) :
    ∀ ls : List Leg, (∀ l ∈ ls, ¬ (l.account = a ∧ l.unit = u)) →
      balance (ls.map fun l => ⟨l.account, l.unit, l.amount, k⟩) a u = 0 := by
  intro ls
  induction ls with
  | nil => intro _; simp [balance]
  | cons l ls ih =>
    intro h
    have hl := h l (by simp)
    have hrest : ∀ l' ∈ ls, ¬ (l'.account = a ∧ l'.unit = u) := fun l' hl' => h l' (by simp [hl'])
    have hne : (l.account == a && l.unit == u) = false := by
      simp only [Bool.and_eq_false_iff, beq_eq_false_iff_ne, ne_eq]
      by_cases h1 : l.account = a
      · exact Or.inr (fun h2 => hl ⟨h1, h2⟩)
      · exact Or.inl h1
    have := ih hrest
    simp only [balance, List.map_cons, List.filter_cons, hne] at this ⊢
    simpa using this

theorem balance_entriesOf_untouched (t : Tx) (a u : Nat)
    (h : ∀ l ∈ t.legs, ¬ (l.account = a ∧ l.unit = u)) : balance (entriesOf t) a u = 0 :=
  balance_map_untouched t.key a u t.legs h

theorem entriesOf_ne_nil (t : Tx) (h : balanced t = true) : entriesOf t ≠ [] := by
  simp only [balanced, Bool.and_eq_true, decide_eq_true_eq] at h
  intro e
  have : t.legs = [] := by simpa [entriesOf] using e
  rw [this] at h
  simp at h

theorem recorded_entriesOf (t : Tx) : recorded (entriesOf t) t.key = entriesOf t := by
  unfold recorded entriesOf
  apply List.filter_eq_self.mpr
  intro e he
  obtain ⟨l, _, rfl⟩ := List.mem_map.mp he
  simp

/-! ## Accepted steps -/

/-- The cases of an accepted step: a replay that leaves the state as it was, or a posting that
appends the entries of the transaction to the journal and leaves the limits as they were. -/
theorem step_ok_cases (s s' : State) (t : Tx) (r : Bool) (h : step s t = .ok (s', r)) :
    (s' = s ∧ r = true ∧ recorded s.journal t.key = entriesOf t ∧ entriesOf t ≠ []) ∨
    (s' = { s with journal := s.journal ++ entriesOf t } ∧ r = false ∧
      recorded s.journal t.key = [] ∧ balanced t = true ∧ limitsOk s t = true) := by
  unfold step at h
  split at h
  · rename_i hne
    split at h
    · rename_i heq
      cases h
      exact Or.inl ⟨rfl, rfl, heq, by rw [← heq]; exact hne⟩
    · cases h
  · rename_i hnil
    have hnil : recorded s.journal t.key = [] := by simpa using hnil
    split at h
    · cases h
    · rename_i hb
      split at h
      · cases h
      · rename_i hl
        cases h
        exact Or.inr ⟨rfl, rfl, hnil, by simpa using hb, by simpa using hl⟩

/-! ## Conservation of units (LDG-2929) -/

/-- The entries of every unit sum to zero. -/
def Conserved (s : State) : Prop := ∀ u, total s.journal u = 0

theorem total_split (j : List Entry) (a u : Nat) :
    total j u = balance j a u + total (j.filter fun e => e.account != a) u := by
  induction j with
  | nil => simp [total, balance]
  | cons e es ih =>
    by_cases h1 : e.account = a <;> by_cases h2 : e.unit = u <;>
      simp [total, balance, h1, h2] at ih ⊢ <;> omega

theorem sum_balances (accts : List Nat) :
    ∀ (j : List Entry) (u : Nat), accts.Nodup → (∀ e ∈ j, e.account ∈ accts) →
      (accts.map fun a => balance j a u).sum = total j u := by
  induction accts with
  | nil =>
    intro j u _ hcov
    cases j with
    | nil => simp [total]
    | cons e rest => exact absurd (hcov e (by simp)) (by simp)
  | cons a rest ih =>
    intro j u hnd hcov
    have hnd' := List.nodup_cons.mp hnd
    -- the entries not of `a`
    let j' := j.filter fun e => e.account != a
    have split : total j u = balance j a u + total j' u := total_split j a u
    have same : ∀ b, b ∈ rest → balance j b u = balance j' b u := by
      intro b hb
      have hba : (b == a) = false := by
        have : b ≠ a := fun e => hnd'.1 (e ▸ hb)
        simpa using this
      simp only [balance, j', List.filter_filter]
      congr 2
      apply List.filter_congr
      intro e _
      by_cases h : e.account == b
      · have h' : e.account = b := beq_iff_eq.mp h
        have hne : e.account ≠ a := by
          intro x
          apply hnd'.1
          rw [← x, h']
          exact hb
        simp [h, hne]
      · simp [h]
    have cov' : ∀ e ∈ j', e.account ∈ rest := by
      intro e he
      simp only [j', List.mem_filter, bne_iff_ne, ne_eq] at he
      have := hcov e he.1
      simp at this
      rcases this with h | h
      · exact absurd h he.2
      · exact h
    rw [List.map_cons, List.sum_cons, split]
    have hmap : (rest.map fun b => balance j b u) = (rest.map fun b => balance j' b u) :=
      List.map_congr_left same
    rw [hmap, ih j' u hnd'.2 cov']

/-- After every accepted step the entries of each unit sum to zero, and so do the balances of all
the accounts of the journal.

Verifies: LDG-2929 -/
theorem ledger_conserves_units (s s' : State) (t : Tx) (r : Bool)
    (hs : Conserved s) (h : step s t = .ok (s', r)) :
    Conserved s' ∧
      ∀ accts : List Nat, accts.Nodup → (∀ e ∈ s'.journal, e.account ∈ accts) →
        ∀ u, (accts.map fun a => balance s'.journal a u).sum = 0 := by
  have hc : Conserved s' := by
    rcases step_ok_cases s s' t r h with ⟨rfl, _⟩ | ⟨rfl, _, _, hb, _⟩
    · exact hs
    · intro u
      simp only [total_append, total_entriesOf]
      rw [hs u, legSum_zero t hb u]
      rfl
  refine ⟨hc, ?_⟩
  intro accts hnd hcov u
  rw [sum_balances accts s'.journal u hnd hcov]
  exact hc u

/-! ## Append-only journal (LDG-2930) -/

/-- No accepted step changes or removes an entry already in the journal: the journal after the
step extends the journal before it, entry for entry and in order.

Verifies: LDG-2930 -/
theorem ledger_append_only (s s' : State) (t : Tx) (r : Bool) (h : step s t = .ok (s', r)) :
    s.journal <+: s'.journal ∧ ∀ e ∈ s.journal, e ∈ s'.journal := by
  have hp : s.journal <+: s'.journal := by
    rcases step_ok_cases s s' t r h with ⟨rfl, _⟩ | ⟨rfl, _⟩
    · exact List.prefix_refl _
    · exact ⟨entriesOf t, rfl⟩
  exact ⟨hp, fun e he => hp.subset he⟩

/-! ## Idempotent replay (LDG-2931) -/

/-- A transaction whose key the journal already holds for the same request returns the first
entries and leaves the state unchanged; and a transaction that was just posted, asked again, is
that replay.

Verifies: LDG-2931 -/
theorem ledger_replay_idempotent (s : State) (t : Tx) :
    (recorded s.journal t.key = entriesOf t → entriesOf t ≠ [] → step s t = .ok (s, true)) ∧
    ∀ s', step s t = .ok (s', false) → step s' t = .ok (s', true) := by
  constructor
  · intro heq hne
    unfold step
    have : recorded s.journal t.key ≠ [] := by rw [heq]; exact hne
    simp [heq, hne]
  · intro s' h
    rcases step_ok_cases s s' t false h with ⟨_, hr, _⟩ | ⟨rfl, _, hnil, hb, _⟩
    · cases hr
    · have hrec : recorded (s.journal ++ entriesOf t) t.key = entriesOf t := by
        have : recorded (s.journal ++ entriesOf t) t.key
            = recorded s.journal t.key ++ recorded (entriesOf t) t.key := by
          simp [recorded, List.filter_append]
        rw [this, hnil, recorded_entriesOf]
        rfl
      have hne := entriesOf_ne_nil t hb
      unfold step
      have : recorded (s.journal ++ entriesOf t) t.key ≠ [] := by rw [hrec]; exact hne
      simp [hrec, hne]

/-! ## No overdraft (LDG-2932) -/

/-- No accepted step takes an account below its floor or above its cap: an account that is below
its floor after a step was below it before and has not moved away from it, and likewise for the
cap. -/
theorem ledger_no_overdraft (s s' : State) (t : Tx) (r : Bool) (h : step s t = .ok (s', r))
    (a u : Nat) :
    (∀ f, s.floor a u = some f → balance s'.journal a u < f →
      balance s.journal a u < f ∧ balance s.journal a u ≤ balance s'.journal a u) ∧
    (∀ c, s.cap a u = some c → c < balance s'.journal a u →
      c < balance s.journal a u ∧ balance s'.journal a u ≤ balance s.journal a u) := by
  rcases step_ok_cases s s' t r h with ⟨rfl, _⟩ | ⟨rfl, _, _, _, hl⟩
  · exact ⟨fun f _ hb => ⟨hb, Int.le_refl _⟩, fun c _ hb => ⟨hb, Int.le_refl _⟩⟩
  · by_cases hu : ∃ l ∈ t.legs, l.account = a ∧ l.unit = u
    · obtain ⟨l, hl', rfl, rfl⟩ := hu
      simp only [limitsOk, List.all_eq_true] at hl
      have hok := hl l hl'
      simp only [rangeOk, Bool.and_eq_true] at hok
      constructor
      · intro f hf hb
        have h1 := hok.1
        simp only [hf, Bool.or_eq_true, decide_eq_true_eq, Bool.and_eq_true] at h1
        simp only at hb
        rcases h1 with h1 | ⟨h1, h2⟩
        · exact absurd h1 (Int.not_le.mpr hb)
        · exact ⟨h1, h2⟩
      · intro c hc hb
        have h1 := hok.2
        simp only [hc, Bool.or_eq_true, decide_eq_true_eq, Bool.and_eq_true] at h1
        simp only at hb
        rcases h1 with h1 | ⟨h1, h2⟩
        · exact absurd h1 (Int.not_le.mpr hb)
        · exact ⟨h1, h2⟩
    · have hun : ∀ l ∈ t.legs, ¬ (l.account = a ∧ l.unit = u) := fun l hl' hh => hu ⟨l, hl', hh⟩
      have hsame : balance (s.journal ++ entriesOf t) a u = balance s.journal a u := by
        rw [balance_append, balance_entriesOf_untouched t a u hun]
        omega
      simp only [hsame]
      exact ⟨fun f _ hb => ⟨hb, Int.le_refl _⟩, fun c _ hb => ⟨hb, Int.le_refl _⟩⟩

end Strata.Ledger
