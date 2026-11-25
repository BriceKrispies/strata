/-
IronFleet's toy lock service, proved with the temporal library.

N hosts pass one lock around a ring. The lock is either held by a host (`holder`) or in flight to
one (`packet`). Each host runs an always-enabled action: if it holds the lock it grants it to the next
host in the ring, and otherwise it does nothing. The network delivers an in-flight grant. The
assumptions are the spec's fairness: every host's action is weakly fair, and the network's delivery
is weakly fair.

The theorem `lock_liveness` says every host eventually holds the lock. `lock_nonvacuous` exhibits a
behaviour that satisfies the spec, so the assumptions are consistent and the theorem is not vacuous.
-/

import Strata.Temporal.Rules

namespace Strata.Temporal.Lock

open Strata.Temporal

/-- The state: who holds the lock, and where a grant in flight is headed. The lock is with exactly one
of them in every reachable state. -/
structure LockS where
  holder : Option Nat
  packet : Option Nat
  deriving DecidableEq

/-- The next host in the ring of `N` hosts. -/
def nextHost (N j : Nat) : Nat := if j + 1 < N then j + 1 else 0

/-- Host `j`, always enabled. If it holds the lock it passes it to the next host; otherwise it does
nothing. -/
abbrev hostAct (N j : Nat) (s s' : LockS) : Prop :=
  (s.holder = some j ∧ s'.holder = none ∧ s'.packet = some (nextHost N j)) ∨
  (s.holder ≠ some j ∧ s' = s)

/-- The network delivers an in-flight grant to its destination, which then holds the lock. -/
abbrev deliver (s s' : LockS) : Prop :=
  ∃ j, s.packet = some j ∧ s.holder = none ∧ s'.holder = some j ∧ s'.packet = none

/-- One non-stuttering step: some host acts, or the network delivers. -/
abbrev lockNext (N : Nat) (s s' : LockS) : Prop :=
  (∃ j, j < N ∧ hostAct N j s s') ∨ deliver s s'

/-- A step of the spec: a `lockNext` step, or no change at all. -/
abbrev lockStep (N : Nat) (s s' : LockS) : Prop := lockNext N s s' ∨ id s' = id s

/-- Initially host 0 holds the lock and nothing is in flight. -/
abbrev lockInit (s : LockS) : Prop := s.holder = some 0 ∧ s.packet = none

/-- The fairness assumptions: the network delivers weakly fairly, and every host acts weakly fairly. -/
abbrev lockFair (N : Nat) : Temporal LockS :=
  conj (weakFair deliver) (fun b => ∀ j, j < N → weakFair (hostAct N j) b)

/-- The lock service's spec. -/
abbrev lockSpec (N : Nat) : Temporal LockS := spec lockInit (lockNext N) id (lockFair N)

/-- Host `j`'s action is enabled in every state: it either passes the lock or does nothing. -/
theorem lock_enabled (N j : Nat) : ∀ s : LockS, enabled (hostAct N j) s := by
  intro s
  by_cases h : s.holder = some j
  · exact ⟨⟨none, some (nextHost N j)⟩, Or.inl ⟨h, rfl, rfl⟩⟩
  · exact ⟨s, Or.inr ⟨h, rfl⟩⟩

/-- A grant in flight to `j` is delivered: `packet = j` leads to `holder = j`. -/
theorem lock_deliver_leadsTo (N j : Nat) (b : Behaviour LockS)
    (hsteps : always (act (lockStep N)) b) (hwf : weakFair deliver b) :
    leadsTo (lift (fun s => s.packet = some j ∧ s.holder = none))
      (lift (fun s => s.holder = some j)) b := by
  refine wf1 b hsteps ?_ ?_ ?_ hwf
  · intro s s' hP hst
    obtain ⟨hP', hN⟩ := hP
    rcases hst with (⟨k, _, hA⟩ | hD) | hid
    · rcases hA with ⟨hk, _, _⟩ | ⟨_, rfl⟩
      · rw [hN] at hk
        cases hk
      · exact Or.inl ⟨hP', hN⟩
    · obtain ⟨j', hp', _, hq, _⟩ := hD
      have hjj : j' = j := Option.some.inj (hp'.symm.trans hP')
      exact Or.inr (show s'.holder = some j by rw [hq, hjj])
    · have e : s' = s := hid
      subst e
      exact Or.inl ⟨hP', hN⟩
  · intro s s' hP _ hA
    obtain ⟨hp, _⟩ := hP
    obtain ⟨j', hp', _, hq, _⟩ := hA
    have hjj : j' = j := Option.some.inj (hp'.symm.trans hp)
    show s'.holder = some j
    rw [hq, hjj]
  · intro s hP
    obtain ⟨hp, hh⟩ := hP
    exact ⟨⟨some j, none⟩, j, hp, hh, rfl, rfl⟩

/-- A host holding the lock passes it on: `holder = j` leads to a grant in flight to the next host. -/
theorem lock_pass_leadsTo (N j : Nat) (b : Behaviour LockS)
    (hsteps : always (act (lockStep N)) b) (hwf : weakFair (hostAct N j) b) :
    leadsTo (lift (fun s => s.holder = some j))
      (lift (fun s => s.packet = some (nextHost N j) ∧ s.holder = none)) b := by
  refine always_enabled_leadsTo b hsteps ?_ ?_ (lock_enabled N j) hwf
  · intro s s' hP hst
    have hP' : s.holder = some j := hP
    rcases hst with (⟨k, _, hA⟩ | ⟨_, _, hn, _, _⟩) | hid
    · rcases hA with ⟨hk, hh', hpk⟩ | ⟨_, rfl⟩
      · have hkj : k = j := Option.some.inj (hk.symm.trans hP')
        subst hkj
        exact Or.inr ⟨hpk, hh'⟩
      · exact Or.inl hP
    · rw [hP'] at hn
      cases hn
    · have e : s' = s := hid
      subst e
      exact Or.inl hP
  · intro s s' hP _ hA
    have hP' : s.holder = some j := hP
    rcases hA with ⟨_, hh', hpk⟩ | ⟨hk, _⟩
    · exact ⟨hpk, hh'⟩
    · exact absurd hP' hk

/-- The lock reaches every host of the ring, in order, from the initial holder. -/
theorem lock_liveness (N : Nat) (b : Behaviour LockS) (h : lockSpec N b) :
    ∀ i, i < N → eventually (lift (fun s => s.holder = some i)) b := by
  have hsteps : always (act (lockStep N)) b := h.2.1
  have hdel : weakFair deliver b := h.2.2.1
  have hhost : ∀ j, j < N → weakFair (hostAct N j) b := h.2.2.2
  have hinit : lockInit (b 0) := h.1
  have hA : ∀ j, leadsTo (lift (fun s => s.packet = some j ∧ s.holder = none))
      (lift (fun s => s.holder = some j)) b :=
    fun j => lock_deliver_leadsTo N j b hsteps hdel
  have hB : ∀ j, j < N → leadsTo (lift (fun s => s.holder = some j))
      (lift (fun s => s.packet = some (nextHost N j) ∧ s.holder = none)) b :=
    fun j hj => lock_pass_leadsTo N j b hsteps (hhost j hj)
  have hC : ∀ j, j < N → leadsTo (lift (fun s => s.holder = some j))
      (lift (fun s => s.holder = some (nextHost N j))) b :=
    fun j hj => leadsTo_trans b (hB j hj) (hA (nextHost N j))
  have hchain : ∀ i, i < N →
      leadsTo (lift (fun s => s.holder = some 0)) (lift (fun s => s.holder = some i)) b := by
    intro i
    induction i with
    | zero =>
      intro _
      exact leadsTo_refl _ b
    | succ i ih =>
      intro hi
      have e : nextHost N i = i + 1 := by
        unfold nextHost
        simp [show i + 1 < N by omega]
      have hstep := hC i (by omega)
      rw [e] at hstep
      exact leadsTo_trans b (ih (by omega)) hstep
  intro i hi
  exact leadsTo_apply b (hchain i hi) hinit.1

/-! ### A behaviour that satisfies the spec -/

/-- The four states of the two-host ring, indexed by the phase `t mod 4`: host 0 holds the lock, passes
it, and the network delivers it to host 1; host 1 passes it back and it is delivered to host 0. -/
def cyc : Nat → LockS
  | 0 => ⟨some 0, none⟩
  | 1 => ⟨none, some 1⟩
  | 2 => ⟨some 1, none⟩
  | _ => ⟨none, some 0⟩

/-- The period-four behaviour of the two-host ring. -/
def cycBeh : Behaviour LockS := fun t => cyc (t % 4)

/-- Each step of `cycBeh` is a step of the spec. -/
theorem cyc_step (t : Nat) : lockStep 2 (cycBeh t) (cycBeh (t + 1)) := by
  have h : t % 4 = 0 ∨ t % 4 = 1 ∨ t % 4 = 2 ∨ t % 4 = 3 := by omega
  show lockStep 2 (cyc (t % 4)) (cyc ((t + 1) % 4))
  rcases h with h | h | h | h
  · rw [h, show (t + 1) % 4 = 1 by omega]
    exact Or.inl (Or.inl ⟨0, by decide, by decide⟩)
  · rw [h, show (t + 1) % 4 = 2 by omega]
    exact Or.inl (Or.inr ⟨1, by decide⟩)
  · rw [h, show (t + 1) % 4 = 3 by omega]
    exact Or.inl (Or.inl ⟨1, by decide, by decide⟩)
  · rw [h, show (t + 1) % 4 = 0 by omega]
    exact Or.inl (Or.inr ⟨0, by decide⟩)

/-- A periodic behaviour takes an action at phase `r` infinitely often. -/
theorem recur_of_period4 (A : LockS → LockS → Prop) (r : Nat) (hr : r < 4)
    (hA : ∀ t, t % 4 = r → A (cycBeh t) (cycBeh (t + 1))) :
    always (eventually (act A)) cycBeh := by
  intro n
  refine ⟨(r + 4 - n % 4) % 4, ?_⟩
  rw [suffix_suffix]
  exact hA (n + (r + 4 - n % 4) % 4) (by omega)

theorem cycBeh_deliver_one :
    ∀ t, t % 4 = 1 → deliver (cycBeh t) (cycBeh (t + 1)) := by
  intro t ht
  show deliver (cyc (t % 4)) (cyc ((t + 1) % 4))
  rw [ht, show (t + 1) % 4 = 2 by omega]
  exact ⟨1, by decide⟩

theorem cycBeh_host_zero :
    ∀ t, t % 4 = 0 → hostAct 2 0 (cycBeh t) (cycBeh (t + 1)) := by
  intro t ht
  show hostAct 2 0 (cyc (t % 4)) (cyc ((t + 1) % 4))
  rw [ht, show (t + 1) % 4 = 1 by omega]
  exact Or.inl ⟨by decide, by decide, by decide⟩

theorem cycBeh_host_one :
    ∀ t, t % 4 = 2 → hostAct 2 1 (cycBeh t) (cycBeh (t + 1)) := by
  intro t ht
  show hostAct 2 1 (cyc (t % 4)) (cyc ((t + 1) % 4))
  rw [ht, show (t + 1) % 4 = 3 by omega]
  exact Or.inl ⟨by decide, by decide, by decide⟩

/-- The lock service's spec is satisfiable, so `lock_liveness` is not vacuous. -/
theorem lock_nonvacuous : ∃ b, lockSpec 2 b := by
  refine ⟨cycBeh, ?_, ?_, ?_, ?_⟩
  · exact ⟨by decide, by decide⟩
  · exact fun t => cyc_step t
  · exact fun _ => recur_of_period4 deliver 1 (by decide) cycBeh_deliver_one
  · intro j hj
    rcases (by omega : j = 0 ∨ j = 1) with rfl | rfl
    · exact fun _ => recur_of_period4 (hostAct 2 0) 0 (by decide) cycBeh_host_zero
    · exact fun _ => recur_of_period4 (hostAct 2 1) 2 (by decide) cycBeh_host_one

end Strata.Temporal.Lock
