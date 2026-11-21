/-
The cell arbiter: how a cell's memory is divided between its tenants' working sets.

Each tenant has a floor (its pinned bytes, below which its working set cannot shrink: see
`resize_budget_exact`) and a want above it (what its miss-cost curve says the next bytes are
worth; any policy). The arbiter grants every floor first, then shares what is left by the wants in
order. When the floors alone do not fit, it raises the over-placement alarm and grants floors in
order until the memory runs out.

Proved for any wants: when the floors fit, every grant meets its floor and the grants never exceed
the total; the alarm is raised exactly when the floors do not fit; and the grants never exceed the
total even then.
-/

namespace Strata.WorkingSet.Arbiter

/-- Two lists of the same length, each element of the first at most the matching one of the second. -/
def LeAll : List Nat → List Nat → Prop
  | [], [] => True
  | a :: as, b :: bs => a ≤ b ∧ LeAll as bs
  | _, _ => False

/-- Give each want what it asks, while `r` lasts. -/
def fill : List Nat → Nat → List Nat
  | [], _ => []
  | w :: ws, r => min w r :: fill ws (r - min w r)

theorem fill_length (ws : List Nat) (r : Nat) : (fill ws r).length = ws.length := by
  induction ws generalizing r with
  | nil => rfl
  | cons w ws ih => simp [fill, ih]

theorem fill_sum_le (ws : List Nat) (r : Nat) : (fill ws r).sum ≤ r := by
  induction ws generalizing r with
  | nil => simp [fill]
  | cons w ws ih =>
    simp only [fill, List.sum_cons]
    have := ih (r - min w r)
    have hm : min w r ≤ r := Nat.min_le_right _ _
    omega

theorem fill_le (ws : List Nat) (r : Nat) : LeAll (fill ws r) ws := by
  induction ws generalizing r with
  | nil => trivial
  | cons w ws ih => exact ⟨Nat.min_le_left _ _, ih _⟩

/-- The grants, and whether the over-placement alarm is raised. Each tenant is `(floor, want)`. -/
def arbiter (ts : List (Nat × Nat)) (total : Nat) : List Nat × Bool :=
  if (ts.map Prod.fst).sum ≤ total then
    (List.zipWith (· + ·) (ts.map Prod.fst)
      (fill (ts.map Prod.snd) (total - (ts.map Prod.fst).sum)), false)
  else (fill (ts.map Prod.fst) total, true)

private theorem sum_zipWith_add (a b : List Nat) (h : a.length = b.length) :
    (List.zipWith (· + ·) a b).sum = a.sum + b.sum := by
  induction a generalizing b with
  | nil => cases b <;> simp_all
  | cons x a ih =>
    cases b with
    | nil => simp at h
    | cons y b =>
      simp only [List.zipWith_cons_cons, List.sum_cons]
      rw [ih b (by simpa using h)]
      omega

private theorem forall2_zipWith_add (a b : List Nat) (h : a.length = b.length) :
    LeAll a (List.zipWith (· + ·) a b) := by
  induction a generalizing b with
  | nil => cases b with
    | nil => trivial
    | cons _ _ => simp at h
  | cons x a ih =>
    cases b with
    | nil => simp at h
    | cons y b =>
      simp only [List.zipWith_cons_cons]
      exact ⟨Nat.le_add_right _ _, ih b (by simpa using h)⟩

/-- **The alarm is raised exactly when the floors do not fit.** -/
theorem arbiter_alarm (ts : List (Nat × Nat)) (total : Nat) :
    (arbiter ts total).2 = true ↔ total < (ts.map Prod.fst).sum := by
  unfold arbiter
  split
  · rename_i h; simp only [Bool.false_eq_true, false_iff]; omega
  · rename_i h; simp only [true_iff]; omega

/-- **The grants never exceed the total**, whether or not the floors fit. -/
theorem arbiter_within (ts : List (Nat × Nat)) (total : Nat) :
    (arbiter ts total).1.sum ≤ total := by
  unfold arbiter
  split
  · rename_i h
    simp only []
    rw [sum_zipWith_add _ _ (by rw [fill_length]; simp)]
    have := fill_sum_le (ts.map Prod.snd) (total - (ts.map Prod.fst).sum)
    omega
  · exact fill_sum_le _ _

/-- **Every grant meets its floor when the floors fit.** -/
theorem arbiter_floors (ts : List (Nat × Nat)) (total : Nat)
    (h : (ts.map Prod.fst).sum ≤ total) :
    LeAll (ts.map Prod.fst) (arbiter ts total).1 := by
  unfold arbiter
  rw [ite_eq_left h]
  exact forall2_zipWith_add _ _ (by rw [fill_length]; simp)

/-- One grant per tenant. -/
theorem arbiter_length (ts : List (Nat × Nat)) (total : Nat) :
    (arbiter ts total).1.length = ts.length := by
  unfold arbiter
  split
  · simp [fill_length]
  · simp [fill_length]

/-- Under over-placement, no tenant gets more than its floor. -/
theorem arbiter_overplaced_le_floors (ts : List (Nat × Nat)) (total : Nat)
    (h : total < (ts.map Prod.fst).sum) :
    LeAll (arbiter ts total).1 (ts.map Prod.fst) := by
  unfold arbiter
  rw [ite_eq_right (by omega)]
  exact fill_le _ _

end Strata.WorkingSet.Arbiter
