/-
Writes made offline, queued, and submitted later (docs/frontend.md, "Offline writes").

A queued write carries an idempotency key, drawn once its input is final and stored with it in the
durable queue, and, for an update or a delete, the stamp of the record it was decided on: the
tenant epoch (`expected-epoch`, which a restore changes) and the record's version
(`expected-version`, which a restore can set back). A create has no record and so no stamp.

The server is the authority. A submission is answered from the idempotency table when its key is
there (the table records commits only, as the store does); otherwise the kernel decides it again
on the current state (`allow`: its decision, which `WorkingSet.submit_one_generation` shows
faithful to the kernel), and the store commits it only if the tenant is at the expected epoch and
the record at the expected version. Submissions may be repeated, reordered, duplicated by two
tabs, resent after a reload or a crash, and their replies lost: any submission of any write may
happen any number of times, in any order, between any other writes, restores and forgettings.

Proved:

- **commits against the stamp it was decided on, or changes nothing** (`submit_ok_guard`,
  `submit_not_ok_unchanged`, `submit_classes`);
- **at most once, from two local rules** (`at_most_once`): the server keeps a committed key at
  least `R` ticks after its commit (`ServerRule`), and the client never sends a key more than `R`
  ticks after it first sent it (`ClientRule`). Neither rule mentions the other's state;
- **a write with an epoch and version stamp is applied at most once whatever the server forgets
  and whatever a restore does** (`stamped_at_most_once`).

What fails without each piece:

- a create sent again after the server forgot its key applies twice (`create_twice_after_forget`):
  the schedule breaks the client rule or the server rule;
- a create resent under a new key applies twice (`rekeyed_create_applies_twice`);
- a version guard without the epoch applies an update twice after a restore sets the version back
  (`restore_without_epoch_applies_twice`); with the epoch it applies once
  (`restore_with_epoch_applies_once`).

Assumptions, all of them:

Q1. Within one tenant epoch, every change to a record gives it a version never used before in that
    epoch; a restore starts a new epoch.
Q2. A key names one write: every submission of key `k` is of the same write (`W k`). A key is drawn
    from 128 random bits once the write's input is final; a collision is outside the model.
Q3. The idempotency table and the record write are one transaction (the store's commit).
Q4. The client measures `R` on the server's clock (its offset learned from replies), with the
    margin covering the offset's error and the latency of a submission.

No `sorry`, no added axioms.
-/

import Strata.Model

namespace Strata.Client.Queue

open Strata

abbrev IKey := Nat

/-- The stamp a write was decided on: the tenant epoch (absent for a client that does not send
    `expected-epoch`) and the record's version. -/
structure Exp where
  epoch : Option Nat
  version : Nat
  deriving DecidableEq

structure Write where
  key : IKey
  target : RecordId
  /-- `none` for a create. -/
  expected : Option Exp
  val : Nat

inductive Outcome where
  | ok
  | conflict
  | refused
  deriving DecidableEq

structure Srv where
  epoch : Nat
  clock : Nat
  stamp : RecordId → Nat
  val : RecordId → Nat
  /-- The idempotency table: keys of committed writes the server still keeps. -/
  seen : IKey → Bool
  now : Nat
  /-- When each key was committed, and when each key was first sent (ghosts for the statements). -/
  appliedAt : IKey → Nat
  firstSent : IKey → Option Nat
  /-- Ghost: every application of a key, newest first. -/
  applied : List IKey

def setR {β : Type} (f : Nat → β) (x : Nat) (b : β) : Nat → β := fun y => if y = x then b else f y

/-- Whether the store's guard passes: the tenant at the expected epoch (if one is sent) and the
    record at the expected version. A create has no guard. -/
def Srv.guard (s : Srv) (w : Write) : Bool :=
  match w.expected with
  | none => true
  | some ex => (match ex.epoch with | some e => s.epoch == e | none => true) && s.stamp w.target == ex.version

def Srv.apply (s : Srv) (w : Write) : Srv :=
  { s with clock := s.clock + 1, stamp := setR s.stamp w.target (s.clock + 1),
           val := setR s.val w.target w.val, seen := setR s.seen w.key true,
           appliedAt := setR s.appliedAt w.key s.now, applied := w.key :: s.applied }

/-- The first sending of a key is recorded. -/
def Srv.sent (s : Srv) (k : IKey) : Srv :=
  match s.firstSent k with
  | some _ => s
  | none => { s with firstSent := setR s.firstSent k (some s.now) }

/-- One submission, after recording its first sending. -/
def Srv.submit (s0 : Srv) (allow : Bool) (w : Write) : Srv × Outcome :=
  let s := s0.sent w.key
  if s.seen w.key then (s, .ok)
  else if !allow then (s, .refused)
  else if s.guard w then (s.apply w, .ok)
  else (s, .conflict)

inductive Op where
  | submit (allow : Bool) (w : Write)
  /-- Another writer changes a record. -/
  | change (r : RecordId) (v : Nat)
  /-- A restore: a new epoch, versions set back to any earlier values. -/
  | restore (st : RecordId → Nat)
  /-- The server forgets a committed key. -/
  | forget (k : IKey)
  | tick

def step (s : Srv) : Op → Srv
  | .submit allow w => (s.submit allow w).1
  | .change r v => { s with clock := s.clock + 1, stamp := setR s.stamp r (s.clock + 1), val := setR s.val r v }
  | .restore st => { s with epoch := s.epoch + 1, stamp := fun r => min (st r) s.clock }
  | .forget k => { s with seen := setR s.seen k false }
  | .tick => { s with now := s.now + 1 }

def run : Srv → List Op → Srv
  | s, [] => s
  | s, op :: ops => run (step s op) ops

/-! ## Each submission -/

theorem sent_eq (s : Srv) (k : IKey) :
    (s.sent k).seen = s.seen ∧ (s.sent k).stamp = s.stamp ∧ (s.sent k).val = s.val ∧
    (s.sent k).applied = s.applied ∧ (s.sent k).epoch = s.epoch ∧ (s.sent k).clock = s.clock ∧
    (s.sent k).now = s.now ∧ (s.sent k).appliedAt = s.appliedAt := by
  unfold Srv.sent; cases s.firstSent k <;> simp

theorem guard_sent (s : Srv) (k : IKey) (w : Write) : (s.sent k).guard w = s.guard w := by
  obtain ⟨_, h2, _, _, h5, _⟩ := sent_eq s k
  simp [Srv.guard, h2, h5]

/-- **An accepted first submission commits against the stamp it was decided on**: the kernel
    allowed it, and a write with a stamp found the tenant at its epoch and the record at its version.
    Verifies: FE-007 -/
theorem submit_ok_guard (s : Srv) (allow : Bool) (w : Write) (h : (s.submit allow w).2 = .ok)
    (hnew : s.seen w.key = false) :
    allow = true ∧ ∀ ex, w.expected = some ex →
      s.stamp w.target = ex.version ∧ ∀ e, ex.epoch = some e → s.epoch = e := by
  have hs := (sent_eq s w.key).1
  have hg := guard_sent s w.key w
  unfold Srv.submit at h
  simp only [hs, hnew, Bool.false_eq_true, ↓reduceIte, hg] at h
  cases ha : allow with
  | false => simp [ha] at h
  | true =>
    simp only [ha, Bool.not_true, Bool.false_eq_true, ↓reduceIte] at h
    refine ⟨rfl, fun ex hex => ?_⟩
    by_cases hgd : s.guard w = true
    · simp only [Srv.guard, hex] at hgd
      cases he : ex.epoch with
      | none => simp [he] at hgd; exact ⟨hgd, fun e h' => by cases h'⟩
      | some e =>
        simp [he] at hgd
        exact ⟨hgd.2, fun e' h' => by cases h'; exact hgd.1⟩
    · simp [hgd] at h

/-- **Any submission that does not commit changes no record**: a conflict, a refusal and a repeated
    key leave every record, its version and its value as they were.
    Verifies: FE-010 -/
theorem submit_not_ok_unchanged (s : Srv) (allow : Bool) (w : Write)
    (hc : (s.submit allow w).2 ≠ .ok ∨ s.seen w.key = true) :
    (s.submit allow w).1.stamp = s.stamp ∧ (s.submit allow w).1.val = s.val ∧
      (s.submit allow w).1.applied = s.applied := by
  obtain ⟨hs, h2, h3, h4, _⟩ := sent_eq s w.key
  have hg := guard_sent s w.key w
  unfold Srv.submit at hc ⊢
  simp only [hs, hg] at hc ⊢
  by_cases hseen : s.seen w.key = true
  · simp [hseen, h2, h3, h4]
  · simp only [hseen, Bool.false_eq_true, ↓reduceIte, or_false] at hc ⊢
    cases allow with
    | false => simp [h2, h3, h4]
    | true =>
      simp only [Bool.not_true, Bool.false_eq_true, ↓reduceIte] at hc ⊢
      by_cases hgd : s.guard w = true
      · simp [hgd] at hc
      · simp [hgd, h2, h3, h4]

/-- **What a first submission answers**: committed at the decided stamp, a conflict because the
    tenant or the record moved, or a refusal by the kernel.
    Verifies: FE-007 -/
theorem submit_classes (s : Srv) (allow : Bool) (w : Write) (hnew : s.seen w.key = false) :
    ((s.submit allow w).2 = .ok ∧ s.guard w = true ∧ allow = true) ∨
    ((s.submit allow w).2 = .conflict ∧ s.guard w = false ∧ allow = true) ∨
    ((s.submit allow w).2 = .refused ∧ allow = false) := by
  have hs := (sent_eq s w.key).1
  have hg := guard_sent s w.key w
  unfold Srv.submit
  simp only [hs, hnew, Bool.false_eq_true, ↓reduceIte, hg]
  cases allow with
  | false => right; right; simp
  | true =>
    simp only [Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    cases hgd : s.guard w with
    | true => left; simp
    | false => right; left; simp

/-! ## At most once, from two local rules -/

/-- The server's rule: a committed key is forgotten only `R` ticks or more after its commit. -/
def ServerRule (R : Nat) (s : Srv) : Op → Prop
  | .forget k => s.seen k = true → s.appliedAt k + R ≤ s.now
  | _ => True

/-- The client's rule: a key is sent only less than `R` ticks after its first sending. -/
def ClientRule (R : Nat) (s : Srv) : Op → Prop
  | .submit _ w => ∀ t, s.firstSent w.key = some t → s.now < t + R
  | _ => True

def Rules (R : Nat) : Srv → List Op → Prop
  | _, [] => True
  | s, op :: ops => ServerRule R s op ∧ ClientRule R s op ∧ Rules R (step s op) ops

/-- Each key applied at most once; an applied key is still kept, or its retention has passed; its
    first sending was no later than its commit; no first sending lies in the future. -/
def AMO (R : Nat) (s : Srv) : Prop :=
  (∀ k t, s.firstSent k = some t → t ≤ s.now) ∧
  ∀ k, s.applied.count k = 0 ∨
    (s.applied.count k = 1 ∧ (s.seen k = true ∨ s.appliedAt k + R ≤ s.now) ∧
      ∃ t, s.firstSent k = some t ∧ t ≤ s.appliedAt k ∧ s.appliedAt k ≤ s.now)

theorem count_cons_ne (k k' : IKey) (l : List IKey) (h : k ≠ k') : (k' :: l).count k = l.count k := by
  simp [Ne.symm h]

theorem sent_first (s : Srv) (k : IKey) :
    ∃ t, (s.sent k).firstSent k = some t ∧ (∀ t', s.firstSent k = some t' → t = t') ∧
      (s.firstSent k = none → t = s.now) := by
  unfold Srv.sent
  cases hf : s.firstSent k with
  | some t => exact ⟨t, (by simp [hf]), (fun t' h => by cases h; rfl), (fun h => by cases h)⟩
  | none => exact ⟨s.now, (by simp [setR]), (fun t' h => by cases h), (fun _ => rfl)⟩

theorem sent_first_other (s : Srv) (k k' : IKey) (h : k' ≠ k) :
    (s.sent k).firstSent k' = s.firstSent k' := by
  unfold Srv.sent
  cases s.firstSent k with
  | some t => rfl
  | none => simp [setR, h]

theorem amo_sent (R : Nat) (s : Srv) (k : IKey) (h : AMO R s) : AMO R (s.sent k) := by
  obtain ⟨hs, _, _, ha, _, _, hn, hat⟩ := sent_eq s k
  obtain ⟨t0, ht0, hsame, hnew⟩ := sent_first s k
  obtain ⟨hf, hk⟩ := h
  refine ⟨fun k' t ht => ?_, fun k' => ?_⟩
  · rw [hn]
    by_cases hkk : k' = k
    · subst hkk; rw [ht0] at ht; cases ht
      cases hfk : s.firstSent k' with
      | some t' => rw [hsame t' hfk]; exact hf k' t' hfk
      | none => simp [hnew hfk]
    · rw [sent_first_other s k k' hkk] at ht; exact hf k' t ht
  · rcases hk k' with h0 | ⟨h1, h2, t, ht, h3, h4⟩
    · left; rw [ha]; exact h0
    · right
      refine ⟨by rw [ha]; exact h1, by rw [hs, hat, hn]; exact h2, t, ?_, by rw [hat]; exact h3,
        by rw [hat, hn]; exact h4⟩
      by_cases hkk : k' = k
      · subst hkk; rw [ht0, hsame t ht]
      · rw [sent_first_other s k k' hkk]; exact ht

theorem amo_submit (R : Nat) (s : Srv) (allow : Bool) (w : Write) (h : AMO R s)
    (hc : ∀ t, s.firstSent w.key = some t → s.now < t + R) : AMO R (s.submit allow w).1 := by
  have h' := amo_sent R s w.key h
  obtain ⟨hs, _, _, ha, _, _, hn, hat⟩ := sent_eq s w.key
  obtain ⟨t0, ht0, _, _⟩ := sent_first s w.key
  unfold Srv.submit
  simp only
  by_cases hseen : (s.sent w.key).seen w.key = true
  · simp only [hseen, ↓reduceIte]; exact h'
  · simp only [hseen, Bool.false_eq_true, ↓reduceIte]
    cases allow with
    | false => simp only [Bool.not_false, ↓reduceIte]; exact h'
    | true =>
      simp only [Bool.not_true, Bool.false_eq_true, ↓reduceIte]
      by_cases hg : (s.sent w.key).guard w = true
      · simp only [hg, ↓reduceIte]
        -- the key had not been applied: an applied key is kept, or its retention passed, which
        -- the client's rule excludes
        have h0 : s.applied.count w.key = 0 := by
          rcases h.2 w.key with h0 | ⟨_, h2, t, ht, h3, _⟩
          · exact h0
          · exfalso
            have := hc t ht
            rcases h2 with h2 | h2
            · rw [hs] at hseen; exact hseen h2
            · omega
        have ht0le : t0 ≤ (s.sent w.key).now := h'.1 w.key t0 ht0
        refine ⟨fun k t ht => ?_, fun k => ?_⟩
        · simp only [Srv.apply] at ht ⊢; exact h'.1 k t ht
        · by_cases hk : k = w.key
          · subst hk
            right
            refine ⟨by simp [Srv.apply, ha, h0], Or.inl (by simp [Srv.apply, setR]), t0, ?_, ?_, ?_⟩
            · simp only [Srv.apply]; exact ht0
            · simp only [Srv.apply, setR, ↓reduceIte]; exact ht0le
            · simp only [Srv.apply, setR, ↓reduceIte]; exact Nat.le_refl _
          · rcases h'.2 k with hk0 | ⟨h1, h2, t', ht', h3, h4⟩
            · left; simp only [Srv.apply, count_cons_ne k w.key _ hk]; exact hk0
            · right
              refine ⟨by simp only [Srv.apply, count_cons_ne k w.key _ hk]; exact h1, ?_, t', ?_, ?_, ?_⟩
              · simp only [Srv.apply, setR, hk, ↓reduceIte]; exact h2
              · simp only [Srv.apply]; exact ht'
              · simp only [Srv.apply, setR, hk, ↓reduceIte]; exact h3
              · simp only [Srv.apply, setR, hk, ↓reduceIte]; exact h4
      · simp only [hg, Bool.false_eq_true, ↓reduceIte]; exact h'

theorem amo_step (R : Nat) (s : Srv) (op : Op) (hs : ServerRule R s op) (hc : ClientRule R s op)
    (h : AMO R s) : AMO R (step s op) := by
  obtain ⟨hf, hk⟩ := h
  cases op with
  | submit allow w => exact amo_submit R s allow w ⟨hf, hk⟩ hc
  | change r v => exact ⟨hf, hk⟩
  | restore st => exact ⟨hf, hk⟩
  | forget k0 =>
    refine ⟨hf, fun k => ?_⟩
    rcases hk k with h0 | ⟨h1, h2, t, ht, h3, h4⟩
    · left; exact h0
    · right
      refine ⟨h1, ?_, t, ht, h3, h4⟩
      by_cases hkk : k = k0
      · subst hkk
        right
        rcases h2 with h2 | h2
        · exact hs h2
        · exact h2
      · simp only [step, setR, hkk, ↓reduceIte]; exact h2
  | tick =>
    refine ⟨fun k t ht => by have := hf k t ht; simp only [step]; omega, fun k => ?_⟩
    rcases hk k with h0 | ⟨h1, h2, t, ht, h3, h4⟩
    · left; exact h0
    · right
      refine ⟨h1, ?_, t, ht, h3, by simp only [step]; omega⟩
      rcases h2 with h2 | h2
      · exact Or.inl h2
      · right; simp only [step]; omega

/-- **At most once, from two local rules**: in every schedule in which the server keeps each
    committed key `R` ticks and the client sends each key only within `R` ticks of its first
    sending, no key is applied twice, whatever is resent, duplicated by tabs, reordered or lost.
    Verifies: FE-008 -/
theorem at_most_once (R : Nat) (ops : List Op) : ∀ s, AMO R s → Rules R s ops → AMO R (run s ops) := by
  induction ops with
  | nil => intro s h _; exact h
  | cons op ops ih =>
    intro s h hr
    exact ih _ (amo_step R s op hr.1 hr.2.1 h) hr.2.2

/-! ### Writes with an epoch and version stamp -/

/-- One write per key (Q2): `W k` is the write every submission of `k` carries. -/
def Keyed (W : IKey → Write) : List Op → Prop
  | [] => True
  | .submit _ w :: ops => W w.key = w ∧ Keyed W ops
  | _ :: ops => Keyed W ops

/-- Versions never exceed the clock, and a key whose write has an epoch and version stamp, once
    applied, has moved its tenant or its record off that stamp for good. -/
def SAMO (W : IKey → Write) (s : Srv) : Prop :=
  (∀ r, s.stamp r ≤ s.clock) ∧
  ∀ k e v, (W k).expected = some ⟨some e, v⟩ →
    s.applied.count k = 0 ∨
    (s.applied.count k = 1 ∧ e ≤ s.epoch ∧ (e = s.epoch → s.stamp (W k).target ≠ v ∧ v ≤ s.clock))

/-- A record change keeps the obligation of every applied key. -/
theorem samo_bump (W : IKey → Write) (s : Srv) (r : RecordId) (h : SAMO W s) :
    (∀ r', setR s.stamp r (s.clock + 1) r' ≤ s.clock + 1) ∧
    ∀ k e v, (W k).expected = some ⟨some e, v⟩ → s.applied.count k = 1 → e ≤ s.epoch →
      (e = s.epoch → setR s.stamp r (s.clock + 1) (W k).target ≠ v ∧ v ≤ s.clock + 1) := by
  obtain ⟨hc, hk⟩ := h
  refine ⟨fun r' => ?_, fun k e v he h1 _ hep => ?_⟩
  · simp only [setR]; split
    · exact Nat.le_refl _
    · have := hc r'; omega
  · rcases hk k e v he with h0 | ⟨_, _, h3⟩
    · omega
    · obtain ⟨hne, hle⟩ := h3 hep
      refine ⟨?_, by omega⟩
      simp only [setR]; split
      · omega
      · exact hne

theorem samo_step (W : IKey → Write) (s : Srv) (op : Op) (hk : Keyed W [op]) (h : SAMO W s) :
    SAMO W (step s op) := by
  cases op with
  | change r v =>
    have hb := samo_bump W s r h
    refine ⟨hb.1, fun k e v' he => ?_⟩
    rcases h.2 k e v' he with h0 | ⟨h1, h2, _⟩
    · left; exact h0
    · right; exact ⟨h1, h2, hb.2 k e v' he h1 h2⟩
  | restore st =>
    refine ⟨fun r => by simp only [step]; omega, fun k e v he => ?_⟩
    rcases h.2 k e v he with h0 | ⟨h1, h2, _⟩
    · left; exact h0
    · right; refine ⟨h1, by simp only [step]; omega, fun hep => ?_⟩
      simp only [step] at hep; omega
  | forget k0 => exact ⟨h.1, fun k e v he => h.2 k e v he⟩
  | tick => exact ⟨h.1, fun k e v he => h.2 k e v he⟩
  | submit allow w =>
    have hw : W w.key = w := hk.1
    obtain ⟨hs, h2, h3, ha, h5, h6, _⟩ := sent_eq s w.key
    have hsent : SAMO W (s.sent w.key) := by
      refine ⟨fun r => by rw [h2, h6]; exact h.1 r, fun k e v he => ?_⟩
      rw [ha, h5, h2, h6]; exact h.2 k e v he
    simp only [step, Srv.submit]
    by_cases hseen : (s.sent w.key).seen w.key = true
    · simp only [hseen, ↓reduceIte]; exact hsent
    · simp only [hseen, Bool.false_eq_true, ↓reduceIte]
      cases allow with
      | false => simp only [Bool.not_false, ↓reduceIte]; exact hsent
      | true =>
        simp only [Bool.not_true, Bool.false_eq_true, ↓reduceIte]
        by_cases hg : (s.sent w.key).guard w = true
        · simp only [hg, ↓reduceIte]
          have hb := samo_bump W (s.sent w.key) w.target hsent
          refine ⟨hb.1, fun k e v he => ?_⟩
          by_cases hkk : k = w.key
          · subst hkk
            rw [hw] at he
            -- the guard held, so the key had not been applied
            have hgd : (s.sent w.key).epoch = e ∧ (s.sent w.key).stamp w.target = v := by
              simp only [Srv.guard, he] at hg
              simp at hg; exact ⟨hg.1, hg.2⟩
            rcases hsent.2 w.key e v (by rw [hw]; exact he) with h0 | ⟨_, _, h3'⟩
            · right
              refine ⟨by simp [Srv.apply, h0], by simp only [Srv.apply]; omega, fun _ => ⟨?_, ?_⟩⟩
              · simp only [Srv.apply, setR, hw, ↓reduceIte]
                have := hsent.1 w.target; omega
              · simp only [Srv.apply]; have := hsent.1 w.target; omega
            · exfalso
              obtain ⟨hne, _⟩ := h3' hgd.1.symm
              rw [hw] at hne; exact hne hgd.2
          · rcases hsent.2 k e v he with h0 | ⟨h1, h2', _⟩
            · left; simp only [Srv.apply, count_cons_ne k w.key _ hkk]; exact h0
            · right
              refine ⟨by simp only [Srv.apply, count_cons_ne k w.key _ hkk]; exact h1, h2', ?_⟩
              exact hb.2 k e v he h1 h2'
        · simp only [hg, Bool.false_eq_true, ↓reduceIte]; exact hsent

/-- **A write with an epoch and version stamp is applied at most once, whatever the server forgets
    and whatever a restore does.**
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
      | _ => trivial
    have h2 : Keyed W ops := by
      cases op with
      | submit allow w => exact hk.2
      | _ => exact hk
    exact ih h2 _ (samo_step W s op h1 h)

/-! ## What fails without each piece -/

def s0 : Srv :=
  { epoch := 0, clock := 0, stamp := fun _ => 0, val := fun _ => 0, seen := fun _ => false, now := 0,
    appliedAt := fun _ => 0, firstSent := fun _ => none, applied := [] }

/-- A create (no stamp). -/
def create1 : Write := { key := 7, target := 1, expected := none, val := 42 }

/-- **A create sent again after the server forgot its key is applied twice.** -/
theorem create_twice_after_forget :
    (run s0 [.submit true create1, .forget create1.key, .submit true create1]).applied.count create1.key = 2 := by
  decide

/-- **A create resent under a new key is applied twice.** -/
theorem rekeyed_create_applies_twice :
    (run s0 [.submit true create1, .submit true { create1 with key := 8 }]).applied.length = 2 := by
  decide

/-- An update decided on version 0 without the epoch, and with it. -/
def updNoEpoch : Write := { key := 9, target := 1, expected := some ⟨none, 0⟩, val := 5 }
def updEpoch : Write := { key := 9, target := 1, expected := some ⟨some 0, 0⟩, val := 5 }

/-- **Without the epoch, a restore that sets the version back lets an update apply twice.** -/
theorem restore_without_epoch_applies_twice :
    (run s0 [.submit true updNoEpoch, .restore (fun _ => 0), .forget 9, .submit true updNoEpoch]).applied.count 9 = 2 := by
  decide

/-- **With the epoch, the same schedule applies it once.** -/
theorem restore_with_epoch_applies_once :
    (run s0 [.submit true updEpoch, .restore (fun _ => 0), .forget 9, .submit true updEpoch]).applied.count 9 = 1 := by
  decide

end Strata.Client.Queue
