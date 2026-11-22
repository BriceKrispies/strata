/-
Writes made offline, queued, and submitted later (docs/frontend.md, "Offline writes").

A queued write carries an idempotency key, drawn once when the person decides it and stored with
it in the durable queue, and the stamp of the record it was decided on: the record's version under
the tenant's epoch (a restore rotates the epoch, so a stamp is never seen twice, as the working
set's generation is never seen twice: W4). A create has no record and so no stamp.

The server is the authority. A submission is answered from the idempotency table when its key is
there; otherwise the kernel decides it again on the current state, and the store commits it only if
the record is still at the stamp the write was decided on. Submissions may be repeated, reordered,
duplicated by two tabs, resent after a reload or a crash, and their replies lost: the model lets
any submission of any write happen any number of times, in any order, between any other writes.

Proved:

- **commits against the stamp it was decided on, or is reported**: an accepted submission found
  the record at the write's stamp; every other answer is a conflict (the record moved), a refusal
  by the kernel, or the recorded answer of an earlier submission, and changes no record
  (`submit_ok_guard`, `submit_not_ok_unchanged`, `submit_classes`);
- **at most once**, while the server keeps the key (`at_most_once`), and **for a write with a
  stamp, at most once even after the server forgets the key** (`stamped_at_most_once`).

False as first stated, with the strongest true statement:

- "a queued write is applied at most once" is false for a **create** resent after the server's
  retention of its key ended (D-6595 keeps a key for 24 hours): `create_twice_after_forget`. The
  true statement is `at_most_once` (no submission after the key is forgotten) together with
  `stamped_at_most_once` (updates and deletes need no such condition). The design therefore never
  resends a create whose first sending is older than the retention less a margin; it asks the
  person instead (docs/frontend.md, conflict class "outcome unknown").
- A client that draws a new key when it resends (after a reload, say) applies a create twice:
  `rekeyed_create_applies_twice`. The key is drawn once and stored with the queued write.

Assumptions, all of them:

Q1. Every change to a record gives it a stamp never used before (the server's `clock`): W4 for
    record stamps.
Q2. A key names one write: every submission of key `k` is of the same write (`W k`). The client
    draws keys from a random source of 128 bits; a collision is outside the model.
Q3. The idempotency table and the record write are one transaction (the store's commit).

No `sorry`, no added axioms.
-/

import Strata.Model

namespace Strata.Client.Queue

open Strata

abbrev IKey := Nat

structure Write where
  key : IKey
  target : RecordId
  /-- The record's stamp the write was decided on; `none` for a create. -/
  expected : Option Nat
  val : Nat

inductive Outcome where
  | ok
  | conflict
  | refused
  deriving DecidableEq

structure Srv where
  clock : Nat
  stamp : RecordId → Nat
  val : RecordId → Nat
  /-- The idempotency table: the answer recorded for each key the server still keeps. -/
  seen : IKey → Option Outcome
  /-- Ghost: every application of a key, newest first. -/
  applied : List IKey

def setR {β : Type} (f : Nat → β) (x : Nat) (b : β) : Nat → β := fun y => if y = x then b else f y

/-- Apply a write: the record changes and takes a fresh stamp. -/
def Srv.apply (s : Srv) (w : Write) : Srv :=
  { clock := s.clock + 1, stamp := setR s.stamp w.target (s.clock + 1), val := setR s.val w.target w.val,
    seen := setR s.seen w.key (some .ok), applied := w.key :: s.applied }

/-- One submission. `allow` is the kernel's decision on the current state (permission, rules). -/
def Srv.submit (s : Srv) (allow : Bool) (w : Write) : Srv × Outcome :=
  match s.seen w.key with
  | some o => (s, o)
  | none =>
    if allow then
      match w.expected with
      | some e =>
        if s.stamp w.target = e then (s.apply w, .ok)
        else ({ s with seen := setR s.seen w.key (some .conflict) }, .conflict)
      | none => (s.apply w, .ok)
    else ({ s with seen := setR s.seen w.key (some .refused) }, .refused)

inductive Op where
  /-- A submission of a queued write, by any tab, at any time, any number of times. -/
  | submit (allow : Bool) (w : Write)
  /-- Another writer changes a record (or a restore replaces it). -/
  | change (r : RecordId) (v : Nat)
  /-- The server's retention of a key ends. -/
  | forget (k : IKey)

def step (s : Srv) : Op → Srv
  | .submit allow w => (s.submit allow w).1
  | .change r v => { s with clock := s.clock + 1, stamp := setR s.stamp r (s.clock + 1), val := setR s.val r v }
  | .forget k => { s with seen := setR s.seen k none }

def run : Srv → List Op → Srv
  | s, [] => s
  | s, op :: ops => run (step s op) ops

/-! ## Each submission -/

/-- **An accepted submission commits against the stamp it was decided on**: the key was not yet
    answered, and a write with a stamp found its record at that stamp.
    Verifies: FE-007 -/
theorem submit_ok_guard (s : Srv) (allow : Bool) (w : Write) (h : (s.submit allow w).2 = .ok)
    (hnew : s.seen w.key = none) : allow = true ∧ ∀ e, w.expected = some e → s.stamp w.target = e := by
  unfold Srv.submit at h
  rw [hnew] at h
  simp only at h
  cases ha : allow with
  | false => rw [ha] at h; simp at h
  | true =>
    rw [ha] at h
    simp only [ite_true] at h
    refine ⟨rfl, fun e he => ?_⟩
    rw [he] at h
    simp only at h
    by_cases hs : s.stamp w.target = e
    · exact hs
    · simp [hs] at h

/-- **Any other answer changes no record**: a conflict, a refusal and a recorded answer leave the
    records, their stamps and the applications as they were.
    Verifies: FE-010 -/
theorem submit_not_ok_unchanged (s : Srv) (allow : Bool) (w : Write) (h : (s.submit allow w).2 ≠ .ok) :
    (s.submit allow w).1.stamp = s.stamp ∧ (s.submit allow w).1.val = s.val ∧
      (s.submit allow w).1.applied = s.applied := by
  unfold Srv.submit at h ⊢
  cases hs : s.seen w.key with
  | some o => simp
  | none =>
    rw [hs] at h
    simp only at h ⊢
    cases allow with
    | false => simp
    | true =>
      simp only [ite_true] at h ⊢
      cases he : w.expected with
      | none => rw [he] at h; simp at h
      | some e =>
        rw [he] at h
        simp only at h ⊢
        by_cases hm : s.stamp w.target = e
        · simp [hm] at h
        · simp [hm]

/-- **What a first submission answers**: committed at the decided stamp, a conflict because the
    record moved, or a refusal by the kernel.
    Verifies: FE-007 -/
theorem submit_classes (s : Srv) (allow : Bool) (w : Write) (hnew : s.seen w.key = none) :
    ((s.submit allow w).2 = .ok ∧ (∀ e, w.expected = some e → s.stamp w.target = e)) ∨
    ((s.submit allow w).2 = .conflict ∧ ∃ e, w.expected = some e ∧ s.stamp w.target ≠ e) ∨
    ((s.submit allow w).2 = .refused ∧ allow = false) := by
  unfold Srv.submit
  rw [hnew]
  simp only
  cases allow with
  | false => right; right; simp
  | true =>
    simp only [ite_true]
    cases he : w.expected with
    | none => left; simp
    | some e =>
      simp only
      by_cases hm : s.stamp w.target = e
      · left; simp [hm]
      · right; left; simp [hm]

/-! ## At most once -/

/-- Each key applied at most once, and an applied key answered `ok` from the table. -/
def AMO (s : Srv) : Prop :=
  ∀ k, s.applied.count k = 0 ∨ (s.applied.count k = 1 ∧ s.seen k = some .ok)

def NoForget : List Op → Prop
  | [] => True
  | .forget _ :: _ => False
  | _ :: ops => NoForget ops

theorem count_cons_ne (k k' : IKey) (l : List IKey) (h : k ≠ k') : (k' :: l).count k = l.count k := by
  simp [Ne.symm h]

theorem amo_submit (s : Srv) (allow : Bool) (w : Write) (h : AMO s) : AMO (s.submit allow w).1 := by
  unfold Srv.submit
  cases hs : s.seen w.key with
  | some o => exact h
  | none =>
    have h0 : s.applied.count w.key = 0 := by
      rcases h w.key with h0 | ⟨_, h1⟩
      · exact h0
      · rw [hs] at h1; cases h1
    have happly : AMO (s.apply w) := by
      intro k
      by_cases hk : k = w.key
      · subst hk; right; simp [Srv.apply, setR, h0]
      · simp only [Srv.apply, setR, hk, ↓reduceIte, count_cons_ne k w.key s.applied hk]
        exact h k
    have hrecord : ∀ o, AMO { s with seen := setR s.seen w.key (some o) } := by
      intro o k
      by_cases hk : k = w.key
      · subst hk; left; exact h0
      · simp only [setR, hk, ↓reduceIte]; exact h k
    simp only
    cases allow with
    | false => exact hrecord _
    | true =>
      simp only [ite_true]
      cases w.expected with
      | none => exact happly
      | some e =>
        simp only
        by_cases hm : s.stamp w.target = e
        · simp only [hm, ite_true]; exact happly
        · simp only [hm]; exact hrecord _

theorem amo_change (s : Srv) (r : RecordId) (v : Nat) (h : AMO s) : AMO (step s (.change r v)) := h

/-- **At most once, while the server keeps the keys**: in every schedule of submissions (repeated,
    duplicated by tabs, reordered, after reloads) and other writers' changes, no key is applied
    twice.
    Verifies: FE-008 -/
theorem at_most_once (ops : List Op) (hf : NoForget ops) : ∀ s, AMO s → AMO (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih =>
    intro s h
    cases op with
    | submit allow w => exact ih (by simpa [NoForget] using hf) _ (amo_submit s allow w h)
    | change r v => exact ih (by simpa [NoForget] using hf) _ (amo_change s r v h)
    | forget k => simp [NoForget] at hf

/-! ### Writes with a stamp: at most once, whatever the server forgets -/

/-- One write per key (Q2): `W k` is the write every submission of `k` carries. -/
def Keyed (W : IKey → Write) : List Op → Prop
  | [] => True
  | .submit _ w :: ops => W w.key = w ∧ Keyed W ops
  | _ :: ops => Keyed W ops

/-- Stamps never exceed the clock, and a key whose write has a stamp, once applied, has moved its
    record off that stamp, which is no later than the clock. -/
def SAMO (W : IKey → Write) (s : Srv) : Prop :=
  (∀ r, s.stamp r ≤ s.clock) ∧
  ∀ k e, (W k).expected = some e →
    s.applied.count k = 0 ∨ (s.applied.count k = 1 ∧ s.stamp (W k).target ≠ e ∧ e ≤ s.clock)

theorem samo_bump (W : IKey → Write) (s : Srv) (r : RecordId) (h : SAMO W s) :
    (∀ r', setR s.stamp r (s.clock + 1) r' ≤ s.clock + 1) ∧
    ∀ k e, (W k).expected = some e → s.applied.count k = 1 → setR s.stamp r (s.clock + 1) (W k).target ≠ e ∧ e ≤ s.clock + 1 := by
  obtain ⟨hc, hk⟩ := h
  refine ⟨fun r' => ?_, fun k e he h1 => ?_⟩
  · simp only [setR]; split
    · exact Nat.le_refl _
    · have := hc r'; omega
  · rcases hk k e he with h0 | ⟨_, hne, hle⟩
    · omega
    · refine ⟨?_, by omega⟩
      simp only [setR]; split
      · omega
      · exact hne

theorem samo_step (W : IKey → Write) (s : Srv) (op : Op) (hk : Keyed W [op]) (h : SAMO W s) :
    SAMO W (step s op) := by
  cases op with
  | change r v =>
    have hb := samo_bump W s r h
    refine ⟨hb.1, fun k e he => ?_⟩
    rcases h.2 k e he with h0 | ⟨h1, _, _⟩
    · left; exact h0
    · right; exact ⟨h1, hb.2 k e he h1⟩
  | forget k0 =>
    exact ⟨h.1, fun k e he => h.2 k e he⟩
  | submit allow w =>
    have hw : W w.key = w := hk.1
    simp only [step, Srv.submit]
    cases hs : s.seen w.key with
    | some o => exact h
    | none =>
      have hrecord : ∀ o, SAMO W { s with seen := setR s.seen w.key (some o) } :=
        fun o => ⟨h.1, fun k e he => h.2 k e he⟩
      have happly : (∀ e, w.expected = some e → s.stamp w.target = e) → SAMO W (s.apply w) := by
        intro hguard
        have hb := samo_bump W s w.target h
        refine ⟨hb.1, fun k e he => ?_⟩
        by_cases hkk : k = w.key
        · subst hkk
          rw [hw] at he
          have hst : s.stamp w.target = e := hguard e he
          rcases h.2 w.key e (by rw [hw]; exact he) with h0 | ⟨_, hne, _⟩
          · right
            refine ⟨by simp [Srv.apply, h0], ?_, ?_⟩
            · simp only [Srv.apply, setR, hw, ite_true]
              have := h.1 w.target; omega
            · simp only [Srv.apply]; have := h.1 w.target; omega
          · exact absurd hst (by rw [hw] at hne; exact hne)
        · rcases h.2 k e he with h0 | ⟨h1, _, _⟩
          · left; simp only [Srv.apply, count_cons_ne k w.key s.applied hkk]; exact h0
          · right
            refine ⟨by simp only [Srv.apply, count_cons_ne k w.key s.applied hkk]; exact h1, ?_⟩
            exact hb.2 k e he h1
      simp only
      cases allow with
      | false => exact hrecord _
      | true =>
        simp only [ite_true]
        cases hex : w.expected with
        | none => exact happly (fun e he => by rw [hex] at he; cases he)
        | some e =>
          simp only
          by_cases hm : s.stamp w.target = e
          · simp only [hm, ite_true]
            exact happly (fun e' he' => by rw [hex] at he'; cases he'; exact hm)
          · simp only [hm]; exact hrecord _

/-- **A write with a stamp is applied at most once, even after the server forgets its key**: the
    first application moves the record off the stamp for good (Q1), so any later submission of
    the same write is a conflict.
    Verifies: FE-011 -/
theorem stamped_at_most_once (W : IKey → Write) (ops : List Op) (hk : Keyed W ops) :
    ∀ s, SAMO W s → SAMO W (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih =>
    intro s h
    have h1 : Keyed W [op] := by
      cases op with
      | submit allow w => exact ⟨hk.1, trivial⟩
      | change r v => trivial
      | forget k => trivial
    have h2 : Keyed W ops := by
      cases op with
      | submit allow w => exact hk.2
      | change r v => exact hk
      | forget k => exact hk
    exact ih h2 _ (samo_step W s op h1 h)

/-! ## What is false as first stated -/

def s0 : Srv := { clock := 0, stamp := fun _ => 0, val := fun _ => 0, seen := fun _ => none, applied := [] }

/-- A create (no stamp). -/
def create1 : Write := { key := 7, target := 1, expected := none, val := 42 }

/-- **A create resent after the server forgot its key is applied twice.** -/
theorem create_twice_after_forget :
    (run s0 [.submit true create1, .forget create1.key, .submit true create1]).applied.count create1.key = 2 := by
  decide

/-- **A create resent under a new key is applied twice**: the key must be drawn once and kept with
    the queued write across reloads. -/
theorem rekeyed_create_applies_twice :
    (run s0 [.submit true create1, .submit true { create1 with key := 8 }]).applied.length = 2 := by
  decide

/-- Kept keys: the same create sent twice without forgetting is applied once. -/
theorem create_once_while_kept :
    (run s0 [.submit true create1, .submit true create1]).applied.count create1.key = 1 := by
  decide

end Strata.Client.Queue
