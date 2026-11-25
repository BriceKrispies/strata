/-
Concurrent optimistic transactions of one tenant (WG-5660, part (a) of the reduction argument).

The backend runs a tenant's requests concurrently. Each transaction loads the tenant at its
current revision, computes the kernel's verdict on that snapshot, and commits with a revision
guard: the commit is applied only if the tenant is still at the revision the snapshot was taken
at, and otherwise the transaction loads again. This file models that at the level of the
transaction steps, with arbitrary interleavings, and proves:

- `serial_equivalence`: in every reachable state the tenant's history is the serial execution of
  the committed transactions in revision order, and every decision's result is the kernel's
  verdict on the state at its revision, which is the state after the previous commit;
- `conflict_no_effect`, `refusal_no_effect`: a conflicting attempt and a refused decision leave the
  tenant's history and revision unchanged;
- `decideOk_refines_store`, `decideErr_refines_store`, `conflict_refines_store`,
  `step_refines_store`: each decision and each conflict is a step of
  the protocol's store (`PNext.storeAccept`, `PNext.storeRefuse`, `PNext.storeConflict` in
  `Strata/System/Protocol.lean`), so the finer model refines the atomic store step.

The protocol's store already decides a commit atomically; this model shows why that atomic step
is faithful to concurrent backends, and it is the refinement target of the store step.

No unfinished proof and no added axiom.
-/

import Strata.System.Protocol

namespace Strata.Reduction

open Strata Strata.System

/-- A transaction is named by its request id. -/
abbrev TxnId := RequestId

/-- Where one transaction is, as the backend runs it. `loaded` holds the revision it loaded and the
    tenant there; `computed` holds the revision it will commit against and the kernel's verdict on
    that snapshot. -/
inductive TPhase where
  | idle
  | loaded (base : Nat) (snap : Option Tenant)
  | computed (base : Nat) (res : Except Err Tenant)
  | finished

/-- A decision of the store: the transaction, the revision it was decided at, and the outcome. -/
structure Dec where
  txn : TxnId
  base : Nat
  res : Except Err Tenant

/-- The store and the transactions of one tenant. `hist k` is the tenant at revision `k`, `rev` the
    current revision, `decs` the decisions in the order the store made them, `ph` each transaction's
    phase. -/
structure TState where
  rev : Nat
  hist : Nat → Option Tenant
  decs : List Dec
  ph : TxnId → TPhase

/-- A decision that changed the tenant. -/
def accepted (d : Dec) : Bool :=
  match d.res with
  | .ok _ => true
  | .error _ => false

def upd (f : TxnId → TPhase) (i : TxnId) (x : TPhase) : TxnId → TPhase :=
  fun j => if j = i then x else f j

theorem upd_self (f : TxnId → TPhase) (i : TxnId) (x : TPhase) : upd f i x i = x := by
  simp [upd]

theorem upd_ne (f : TxnId → TPhase) (i : TxnId) (x : TPhase) (j : TxnId) (h : j ≠ i) :
    upd f i x j = f j := by
  simp [upd, h]

/-- One step of the serial reference: the verdict on the state applied, or no change. -/
def serialStep (K : Kernel) (st : Option Tenant) (q : PrincipalId × Intent) : Option Tenant :=
  match verdict K st q.1 q.2 with
  | .ok t => some t
  | .error _ => st

/-- The serial execution of a list of intents from the initial tenant. -/
def serialRun (K : Kernel) (w0 : Option Tenant) (xs : List (PrincipalId × Intent)) :
    Option Tenant :=
  xs.foldl (serialStep K) w0

theorem serialRun_append (K : Kernel) (w0 : Option Tenant) (xs ys : List (PrincipalId × Intent)) :
    serialRun K w0 (xs ++ ys) = serialRun K (serialRun K w0 xs) ys := by
  simp [serialRun, List.foldl_append]

/-- The intents of the committed transactions, in revision order. -/
def committedReqs (req : TxnId → PrincipalId × Intent) (ds : List Dec) :
    List (PrincipalId × Intent) :=
  (ds.filter accepted).map (fun d => req d.txn)

/-- The start: revision 0 holds the initial tenant, nothing is decided, nothing is loading. -/
def TInit (w0 : Option Tenant) (S : TState) : Prop :=
  S.rev = 0 ∧ (∀ k, S.hist k = w0) ∧ S.decs = [] ∧ ∀ i, S.ph i = .idle

/-- One step of the transactions and the store. A transaction loads the current revision, computes
    the verdict on its snapshot, and is decided only if its revision is still current; a stale
    commit is a conflict and the transaction goes back to loading. -/
inductive TNext (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant) :
    TState → TState → Prop
  /-- A transaction loads the tenant at the current revision. -/
  | load (S : TState) (i : TxnId) (h : S.ph i = .idle) :
      TNext K req w0 S { S with ph := upd S.ph i (.loaded S.rev (S.hist S.rev)) }
  /-- A transaction computes the kernel's verdict on its snapshot. -/
  | compute (S : TState) (i : TxnId) (b : Nat) (v : Option Tenant) (h : S.ph i = .loaded b v) :
      TNext K req w0 S
        { S with ph := upd S.ph i (.computed b (verdict K v (req i).1 (req i).2)) }
  /-- The commit is guarded by the revision and succeeds: the tenant moves on. -/
  | decideOk (S : TState) (i : TxnId) (b : Nat) (t' : Tenant)
      (h : S.ph i = .computed b (.ok t')) (hb : b = S.rev) :
      TNext K req w0 S
        { rev := S.rev + 1
          hist := fun k => if k = b + 1 then some t' else S.hist k
          decs := S.decs ++ [⟨i, b, .ok t'⟩]
          ph := upd S.ph i .finished }
  /-- The commit is guarded by the revision and refused: nothing about the tenant changes. -/
  | decideErr (S : TState) (i : TxnId) (b : Nat) (e : Err)
      (h : S.ph i = .computed b (.error e)) (hb : b = S.rev) :
      TNext K req w0 S
        { S with decs := S.decs ++ [⟨i, b, .error e⟩], ph := upd S.ph i .finished }
  /-- The revision moved since the snapshot: the commit is refused and the transaction loads again. -/
  | conflict (S : TState) (i : TxnId) (b : Nat) (res : Except Err Tenant)
      (h : S.ph i = .computed b res) (hb : b ≠ S.rev) :
      TNext K req w0 S { S with ph := upd S.ph i .idle }

/-- The reachable states of one tenant's transactions. -/
def TReachable (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant)
    (S : TState) : Prop :=
  ∃ S0, TInit w0 S0 ∧ Steps (TNext K req w0) S0 S

/-- The invariant of reachable states. -/
structure TInv (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant)
    (S : TState) : Prop where
  /-- The committed decisions are numbered by revision: the k-th one was made at revision k. -/
  bases : (S.decs.filter accepted).map Dec.base = List.range S.rev
  /-- Every decision was made at a revision that exists, and its result is the verdict on the
      tenant at that revision. -/
  decOk : ∀ d ∈ S.decs, d.base ≤ S.rev ∧
    d.res = verdict K (S.hist d.base) (req d.txn).1 (req d.txn).2
  /-- Every revision is the serial execution of the committed transactions before it. -/
  serial : ∀ k, k ≤ S.rev → S.hist k = serialRun K w0 ((committedReqs req S.decs).take k)
  /-- A loaded snapshot is the tenant at the revision it names, and that revision exists. -/
  loadOk : ∀ i b v, S.ph i = .loaded b v → v = S.hist b ∧ b ≤ S.rev
  /-- A computed verdict is the kernel's verdict on the snapshot it was computed from. -/
  compOk : ∀ i b res, S.ph i = .computed b res →
    b ≤ S.rev ∧ res = verdict K (S.hist b) (req i).1 (req i).2
  /-- A transaction is finished exactly when the store has decided it. -/
  finIff : ∀ i, S.ph i = .finished ↔ ∃ d ∈ S.decs, d.txn = i
  /-- A transaction is decided at most once. -/
  nodup : (S.decs.map Dec.txn).Nodup
  /-- A committed result is the tenant at the revision after the one it was decided at. -/
  accHist : ∀ d ∈ S.decs, ∀ u, d.res = .ok u → S.hist (d.base + 1) = some u

theorem committedReqs_length (req : TxnId → PrincipalId × Intent) (S : TState)
    (hb : (S.decs.filter accepted).map Dec.base = List.range S.rev) :
    (committedReqs req S.decs).length = S.rev := by
  have h1 := congrArg List.length hb
  simpa [committedReqs] using h1

theorem filter_snoc_acc (ds : List Dec) (d : Dec) (h : accepted d = true) :
    (ds ++ [d]).filter accepted = ds.filter accepted ++ [d] := by
  simp [List.filter_append, h]

theorem filter_snoc_rej (ds : List Dec) (d : Dec) (h : accepted d = false) :
    (ds ++ [d]).filter accepted = ds.filter accepted := by
  simp [List.filter_append, h]

theorem tinv_init (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant)
    (S : TState) (h : TInit w0 S) : TInv K req w0 S := by
  obtain ⟨hr, hh, hd, hp⟩ := h
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · simp [hd, hr]
  · intro d hdm; simp [hd] at hdm
  · intro k hk
    have hk0 : k = 0 := by omega
    subst hk0
    simp [hh, hd, committedReqs, serialRun]
  · intro i b v hi; rw [hp i] at hi; cases hi
  · intro i b res hi; rw [hp i] at hi; cases hi
  · intro i; simp [hp i, hd]
  · simp [hd]
  · intro d hdm; simp [hd] at hdm

theorem accepted_of_ok {d : Dec} {u : Tenant} (h : d.res = .ok u) : accepted d = true := by
  simp [accepted, h]

theorem mem_accepted_base {S : TState} {d : Dec} (hI : (S.decs.filter accepted).map Dec.base =
    List.range S.rev) (hd : d ∈ S.decs) (ha : accepted d = true) : d.base < S.rev := by
  have hm : d.base ∈ (S.decs.filter accepted).map Dec.base :=
    List.mem_map_of_mem (List.mem_filter.mpr ⟨hd, ha⟩)
  rw [hI] at hm
  exact List.mem_range.mp hm

/-- The invariant is preserved by every step. -/
theorem tinv_step {K : Kernel} {req : TxnId → PrincipalId × Intent} {w0 : Option Tenant}
    {S S' : TState} (hI : TInv K req w0 S) (h : TNext K req w0 S S') : TInv K req w0 S' := by
  cases h with
  | load i hi =>
    refine ⟨hI.bases, fun d hd => hI.decOk d hd, hI.serial, ?_, ?_, ?_, hI.nodup, ?_⟩
    · intro j b' v' hj
      have hj' : upd S.ph i (.loaded S.rev (S.hist S.rev)) j = .loaded b' v' := hj
      by_cases hji : j = i
      · subst hji
        rw [upd_self] at hj'
        injection hj' with hb hv
        subst hb; subst hv
        exact ⟨rfl, Nat.le_refl _⟩
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.loadOk j b' v' hj'
    · intro j b' res hj
      have hj' : upd S.ph i (.loaded S.rev (S.hist S.rev)) j = .computed b' res := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.compOk j b' res hj'
    · intro j
      show upd S.ph i (.loaded S.rev (S.hist S.rev)) j = _ ↔ _
      by_cases hji : j = i
      · subst hji
        rw [upd_self]
        constructor
        · intro hf; cases hf
        · rintro ⟨d, hd, hdj⟩
          have := (hI.finIff j).mpr ⟨d, hd, hdj⟩
          rw [hi] at this; cases this
      · rw [upd_ne _ _ _ _ hji]
        exact hI.finIff j
    · exact fun d hd u hu => hI.accHist d hd u hu
  | compute i b v hi =>
    have hv := hI.loadOk i b v hi
    refine ⟨hI.bases, fun d hd => hI.decOk d hd, hI.serial, ?_, ?_, ?_, hI.nodup, ?_⟩
    · intro j b' v' hj
      have hj' : upd S.ph i (.computed b (verdict K v (req i).1 (req i).2)) j = .loaded b' v' := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.loadOk j b' v' hj'
    · intro j b' res hj
      have hj' : upd S.ph i (.computed b (verdict K v (req i).1 (req i).2)) j =
          .computed b' res := hj
      by_cases hji : j = i
      · subst hji
        rw [upd_self] at hj'
        injection hj' with hb' hr'
        subst hb'; subst hr'
        refine ⟨hv.2, ?_⟩
        exact congrArg (fun x => verdict K x (req j).1 (req j).2) hv.1
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.compOk j b' res hj'
    · intro j
      show upd S.ph i (.computed b (verdict K v (req i).1 (req i).2)) j = _ ↔ _
      by_cases hji : j = i
      · subst hji
        rw [upd_self]
        constructor
        · intro hf; cases hf
        · rintro ⟨d, hd, hdj⟩
          have := (hI.finIff j).mpr ⟨d, hd, hdj⟩
          rw [hi] at this; cases this
      · rw [upd_ne _ _ _ _ hji]
        exact hI.finIff j
    · exact fun d hd u hu => hI.accHist d hd u hu
  | decideOk i b t' h hb =>
    have hcomp := hI.compOk i b (.ok t') h
    have hnf : ¬ ∃ d ∈ S.decs, d.txn = i := by
      intro hex
      have := (hI.finIff i).mpr hex
      rw [h] at this; cases this
    have hacc : accepted (⟨i, b, .ok t'⟩ : Dec) = true := rfl
    have hC : (committedReqs req S.decs).length = S.rev := committedReqs_length req S hI.bases
    have hC' : committedReqs req (S.decs ++ [(⟨i, b, .ok t'⟩ : Dec)]) =
        committedReqs req S.decs ++ [req i] := by
      simp [committedReqs, filter_snoc_acc _ _ hacc]
    refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
    · show ((S.decs ++ [(⟨i, b, .ok t'⟩ : Dec)]).filter accepted).map Dec.base =
        List.range (S.rev + 1)
      rw [filter_snoc_acc _ _ hacc, List.map_append, hI.bases, List.range_succ]
      simp [hb]
    · intro d hd
      rcases List.mem_append.mp hd with hd | hd
      · obtain ⟨h1, h2⟩ := hI.decOk d hd
        have hne : d.base ≠ b + 1 := by omega
        refine ⟨by show d.base ≤ S.rev + 1; omega, ?_⟩
        show d.res = verdict K (if d.base = b + 1 then some t' else S.hist d.base)
          (req d.txn).1 (req d.txn).2
        rw [if_neg hne]; exact h2
      · rw [List.mem_singleton] at hd
        subst hd
        refine ⟨by show b ≤ S.rev + 1; omega, ?_⟩
        show Except.ok t' = verdict K (if b = b + 1 then some t' else S.hist b) (req i).1 (req i).2
        rw [if_neg (by omega)]; exact hcomp.2
    · intro k hk
      have hk2 : k ≤ S.rev + 1 := hk
      show (if k = b + 1 then some t' else S.hist k) =
        serialRun K w0 ((committedReqs req (S.decs ++ [(⟨i, b, .ok t'⟩ : Dec)])).take k)
      rw [hC']
      by_cases hk' : k ≤ S.rev
      · have hkb : k ≠ b + 1 := by omega
        rw [if_neg hkb, List.take_append_of_le_length (hC ▸ hk')]
        exact hI.serial k hk'
      · have hk1 : k = S.rev + 1 := by omega
        subst hk1
        rw [if_pos (show S.rev + 1 = b + 1 by omega), List.take_of_length_le (by simp [hC])]
        have hT : (committedReqs req S.decs).take S.rev = committedReqs req S.decs :=
          List.take_of_length_le (Nat.le_of_eq hC)
        have hsr := hI.serial S.rev (Nat.le_refl _)
        rw [hT] at hsr
        have hv : verdict K (S.hist S.rev) (req i).1 (req i).2 = .ok t' := by
          rw [← hb]; exact hcomp.2.symm
        rw [serialRun_append, ← hsr]
        simp [serialRun, serialStep, hv]
    · intro j b' v' hj
      have hj' : upd S.ph i .finished j = .loaded b' v' := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        obtain ⟨h1, h2⟩ := hI.loadOk j b' v' hj'
        refine ⟨?_, by show b' ≤ S.rev + 1; omega⟩
        show v' = (if b' = b + 1 then some t' else S.hist b')
        rw [if_neg (by omega)]; exact h1
    · intro j b' res hj
      have hj' : upd S.ph i .finished j = .computed b' res := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        obtain ⟨h1, h2⟩ := hI.compOk j b' res hj'
        refine ⟨by show b' ≤ S.rev + 1; omega, ?_⟩
        show res = verdict K (if b' = b + 1 then some t' else S.hist b') (req j).1 (req j).2
        rw [if_neg (by omega)]; exact h2
    · intro j
      show upd S.ph i .finished j = _ ↔ _
      by_cases hji : j = i
      · subst hji
        rw [upd_self]
        exact iff_of_true rfl ⟨⟨j, b, .ok t'⟩, List.mem_append_right _ (List.mem_singleton.mpr rfl), rfl⟩
      · rw [upd_ne _ _ _ _ hji, hI.finIff j]
        constructor
        · rintro ⟨d, hd, hdj⟩; exact ⟨d, List.mem_append_left _ hd, hdj⟩
        · rintro ⟨d, hd, hdj⟩
          rcases List.mem_append.mp hd with hd | hd
          · exact ⟨d, hd, hdj⟩
          · rw [List.mem_singleton] at hd
            subst hd
            exact absurd hdj.symm hji
    · show ((S.decs ++ [(⟨i, b, .ok t'⟩ : Dec)]).map Dec.txn).Nodup
      simp only [List.map_append, List.map_cons, List.map_nil]
      rw [List.nodup_append]
      refine ⟨hI.nodup, by simp, ?_⟩
      intro a ha c hc
      rw [List.mem_singleton] at hc
      subst hc
      obtain ⟨d, hd, hda⟩ := List.mem_map.mp ha
      intro hac
      exact hnf ⟨d, hd, hda.trans hac⟩
    · intro d hd u hu
      rcases List.mem_append.mp hd with hd | hd
      · have hlt := mem_accepted_base hI.bases hd (accepted_of_ok hu)
        have hne : d.base + 1 ≠ b + 1 := by omega
        show (if d.base + 1 = b + 1 then some t' else S.hist (d.base + 1)) = some u
        rw [if_neg hne]; exact hI.accHist d hd u hu
      · rw [List.mem_singleton] at hd
        subst hd
        have hut : t' = u := Except.ok.inj hu
        subst hut
        show (if b + 1 = b + 1 then some t' else S.hist (b + 1)) = some t'
        rw [if_pos rfl]
  | decideErr i b e h hb =>
    have hcomp := hI.compOk i b (.error e) h
    have hnf : ¬ ∃ d ∈ S.decs, d.txn = i := by
      intro hex
      have := (hI.finIff i).mpr hex
      rw [h] at this; cases this
    have hrej : accepted (⟨i, b, .error e⟩ : Dec) = false := rfl
    have hC' : committedReqs req (S.decs ++ [(⟨i, b, .error e⟩ : Dec)]) =
        committedReqs req S.decs := by
      simp [committedReqs, filter_snoc_rej _ _ hrej]
    refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
    · show ((S.decs ++ [(⟨i, b, .error e⟩ : Dec)]).filter accepted).map Dec.base = List.range S.rev
      rw [filter_snoc_rej _ _ hrej]; exact hI.bases
    · intro d hd
      rcases List.mem_append.mp hd with hd | hd
      · exact hI.decOk d hd
      · rw [List.mem_singleton] at hd
        subst hd
        refine ⟨by show b ≤ S.rev; omega, ?_⟩
        show Except.error e = verdict K (S.hist b) (req i).1 (req i).2
        exact hcomp.2
    · intro k hk
      show S.hist k = serialRun K w0 ((committedReqs req (S.decs ++ [(⟨i, b, .error e⟩ : Dec)])).take k)
      rw [hC']; exact hI.serial k hk
    · intro j b' v' hj
      have hj' : upd S.ph i .finished j = .loaded b' v' := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.loadOk j b' v' hj'
    · intro j b' res hj
      have hj' : upd S.ph i .finished j = .computed b' res := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.compOk j b' res hj'
    · intro j
      show upd S.ph i .finished j = _ ↔ _
      by_cases hji : j = i
      · subst hji
        rw [upd_self]
        exact iff_of_true rfl ⟨⟨j, b, .error e⟩, List.mem_append_right _ (List.mem_singleton.mpr rfl), rfl⟩
      · rw [upd_ne _ _ _ _ hji, hI.finIff j]
        constructor
        · rintro ⟨d, hd, hdj⟩; exact ⟨d, List.mem_append_left _ hd, hdj⟩
        · rintro ⟨d, hd, hdj⟩
          rcases List.mem_append.mp hd with hd | hd
          · exact ⟨d, hd, hdj⟩
          · rw [List.mem_singleton] at hd
            subst hd
            exact absurd hdj.symm hji
    · show ((S.decs ++ [(⟨i, b, .error e⟩ : Dec)]).map Dec.txn).Nodup
      simp only [List.map_append, List.map_cons, List.map_nil]
      rw [List.nodup_append]
      refine ⟨hI.nodup, by simp, ?_⟩
      intro a ha c hc
      rw [List.mem_singleton] at hc
      subst hc
      obtain ⟨d, hd, hda⟩ := List.mem_map.mp ha
      intro hac
      exact hnf ⟨d, hd, hda.trans hac⟩
    · intro d hd u hu
      rcases List.mem_append.mp hd with hd | hd
      · exact hI.accHist d hd u hu
      · rw [List.mem_singleton] at hd
        subst hd
        exact absurd hu (by simp)
  | conflict i b res h hb =>
    refine ⟨hI.bases, fun d hd => hI.decOk d hd, hI.serial, ?_, ?_, ?_, hI.nodup, ?_⟩
    · intro j b' v' hj
      have hj' : upd S.ph i .idle j = .loaded b' v' := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.loadOk j b' v' hj'
    · intro j b' res' hj
      have hj' : upd S.ph i .idle j = .computed b' res' := hj
      by_cases hji : j = i
      · subst hji; rw [upd_self] at hj'; cases hj'
      · rw [upd_ne _ _ _ _ hji] at hj'
        exact hI.compOk j b' res' hj'
    · intro j
      show upd S.ph i .idle j = _ ↔ _
      by_cases hji : j = i
      · subst hji
        rw [upd_self]
        constructor
        · intro hf; cases hf
        · rintro ⟨d, hd, hdj⟩
          have := (hI.finIff j).mpr ⟨d, hd, hdj⟩
          rw [h] at this; cases this
      · rw [upd_ne _ _ _ _ hji]
        exact hI.finIff j
    · exact fun d hd u hu => hI.accHist d hd u hu

theorem tinv_of_reach {K : Kernel} {req : TxnId → PrincipalId × Intent} {w0 : Option Tenant}
    {S : TState} (h : TReachable K req w0 S) : TInv K req w0 S := by
  obtain ⟨S0, hi, hs⟩ := h
  induction hs with
  | refl => exact tinv_init K req w0 _ hi
  | tail _ hstep ih => exact tinv_step ih hstep

/-! ## The serial equivalence and the no-effect lemmas -/

/-- Serial equivalence. In every reachable state: each revision is the serial execution of the
    committed transactions before it; every decision's result is the kernel's verdict on the serial
    state at its revision (the state after the previous commit); a committed result is the tenant
    at the next revision; and a transaction is decided at most once. -/
theorem serial_equivalence {K : Kernel} {req : TxnId → PrincipalId × Intent} {w0 : Option Tenant}
    {S : TState} (h : TReachable K req w0 S) :
    (∀ k, k ≤ S.rev → S.hist k = serialRun K w0 ((committedReqs req S.decs).take k)) ∧
    (∀ d ∈ S.decs, d.base ≤ S.rev ∧
      d.res = verdict K (serialRun K w0 ((committedReqs req S.decs).take d.base))
        (req d.txn).1 (req d.txn).2) ∧
    (∀ d ∈ S.decs, ∀ u, d.res = .ok u → S.hist (d.base + 1) = some u) ∧
    (S.decs.map Dec.txn).Nodup := by
  have hI := tinv_of_reach h
  refine ⟨hI.serial, fun d hd => ?_, hI.accHist, hI.nodup⟩
  obtain ⟨hb, hres⟩ := hI.decOk d hd
  refine ⟨hb, ?_⟩
  rw [← hI.serial d.base hb]
  exact hres

/-- A conflicting attempt leaves the tenant, its history and the decisions unchanged. -/
theorem conflict_no_effect (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant)
    (S : TState) (i : TxnId) (b : Nat) (res : Except Err Tenant)
    (h : S.ph i = .computed b res) (hb : b ≠ S.rev) :
    ∃ S', TNext K req w0 S S' ∧ S'.rev = S.rev ∧ S'.hist = S.hist ∧ S'.decs = S.decs :=
  ⟨_, TNext.conflict S i b res h hb, rfl, rfl, rfl⟩

/-- A refused decision leaves the tenant's history and revision unchanged, and the accepted
    decisions unchanged: it is recorded, and changes nothing else. -/
theorem refusal_no_effect (K : Kernel) (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant)
    (S : TState) (i : TxnId) (b : Nat) (e : Err)
    (h : S.ph i = .computed b (.error e)) (hb : b = S.rev) :
    ∃ S', TNext K req w0 S S' ∧ S'.rev = S.rev ∧ S'.hist = S.hist ∧
      S'.decs = S.decs ++ [⟨i, b, .error e⟩] ∧
      S'.decs.filter accepted = S.decs.filter accepted :=
  ⟨_, TNext.decideErr S i b e h hb, rfl, rfl, rfl,
    filter_snoc_rej _ _ rfl⟩

/-! ## Refinement of the protocol's store step -/

/-- The protocol's store as the finer model sees it. Tenant 0 is the one tenant of this model: its
    history and revision are the model's, and its events and decisions are the model's. Every other
    tenant keeps its initial state, which the store step never changes. -/
def storeOf (Wd : TenantId → Option Tenant) (S : TState) : StoreState :=
  { hist := fun x k => if x = 0 then S.hist k else Wd x
    rev := fun x => if x = 0 then S.rev else 0
    evs := S.decs.filterMap (fun d => match d.res with
      | .ok t => some (Event.mk 0 d.txn t)
      | .error _ => none)
    decided := fun r => (S.decs.find? (fun d => d.txn == r)).map (fun d => outcomeOf d.res) }

/-- A protocol state with the given packets and the store of the model. -/
def mkP (Wd : TenantId → Option Tenant) (S : TState) (net : Packet → Prop) : PState :=
  { net := net, txns := fun _ => none, noSession := fun _ => false, attempt := fun _ => 0,
    store := storeOf Wd S }

theorem storeState_eq' (a b : StoreState) (h1 : a.hist = b.hist) (h2 : a.rev = b.rev)
    (h3 : a.evs = b.evs) (h4 : a.decided = b.decided) : a = b := by
  cases a; cases b
  dsimp only at h1 h2 h3 h4
  subst h1; subst h2; subst h3; subst h4
  rfl

/-- Looking up a transaction that is not among the decisions, after a new decision for another
    transaction, finds the same as before, and finds the new decision for its own transaction. -/
theorem find_snoc (ds : List Dec) (d : Dec) (r : TxnId) (hn : ∀ e ∈ ds, e.txn ≠ d.txn) :
    (ds ++ [d]).find? (fun e => e.txn == r) =
      if d.txn = r then some d else ds.find? (fun e => e.txn == r) := by
  by_cases h : d.txn = r
  · have hnone : ds.find? (fun e => e.txn == r) = none := by
      apply List.find?_eq_none.mpr
      intro e he
      simp only [beq_iff_eq]
      intro heq
      exact hn e he (heq.trans h.symm)
    simp [List.find?_append, hnone, h]
  · have hnone : [d].find? (fun e => e.txn == r) = none := by
      simp [h]
    simp [List.find?_append, hnone, h]

theorem decideOk_refines_store (K : Kernel) (C : Config) (Wd : TenantId → Option Tenant)
    (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant) (S : TState)
    (hI : TInv K req w0 S) (i : TxnId) (b : Nat) (t' : Tenant)
    (h : S.ph i = .computed b (.ok t')) (hb : b = S.rev)
    (s : PState) (hs : s.store = storeOf Wd S)
    (hnet : s.net (.commit i 0 0 (req i).1 (req i).2 b (.ok t'))) :
    ∃ s', PNext K C s s' ∧ s'.store = storeOf Wd
      { rev := S.rev + 1
        hist := fun k => if k = b + 1 then some t' else S.hist k
        decs := S.decs ++ [⟨i, b, .ok t'⟩]
        ph := upd S.ph i .finished } := by
  have hnf : ∀ d ∈ S.decs, d.txn ≠ i := by
    intro d hd hdi
    have := (hI.finIff i).mpr ⟨d, hd, hdi⟩
    rw [h] at this; cases this
  have hfind : S.decs.find? (fun d => d.txn == i) = none := by
    apply List.find?_eq_none.mpr
    intro d hd
    simp only [beq_iff_eq]
    exact hnf d hd
  have hd : s.store.decided i = none := by
    rw [hs]; simp [storeOf, hfind]
  have hbb : b = s.store.rev 0 := by
    rw [hs]; simp [storeOf, hb]
  refine ⟨_, PNext.storeAccept s i 0 0 (req i).1 (req i).2 b (.ok t') t' hnet hd hbb rfl, ?_⟩
  apply storeState_eq'
  · funext x k
    by_cases hx : x = 0 <;> by_cases hk : k = b + 1 <;> simp [hx, hk, hs, storeOf]
  · funext x
    by_cases hx : x = 0 <;> simp [hx, hs, storeOf, hb]
  · simp [hs, storeOf, List.filterMap_append]
  · funext r
    rw [hs]
    simp only [storeOf]
    rw [find_snoc _ _ _ hnf]
    by_cases hr : i = r
    · subst hr; simp [outcomeOf]
    · simp [hr, Ne.symm hr, outcomeOf]

theorem decideErr_refines_store (K : Kernel) (C : Config) (Wd : TenantId → Option Tenant)
    (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant) (S : TState)
    (hI : TInv K req w0 S) (i : TxnId) (b : Nat) (e : Err)
    (h : S.ph i = .computed b (.error e)) (hb : b = S.rev)
    (s : PState) (hs : s.store = storeOf Wd S)
    (hnet : s.net (.commit i 0 0 (req i).1 (req i).2 b (.error e))) :
    ∃ s', PNext K C s s' ∧ s'.store = storeOf Wd
      { S with decs := S.decs ++ [⟨i, b, .error e⟩], ph := upd S.ph i .finished } := by
  have hnf : ∀ d ∈ S.decs, d.txn ≠ i := by
    intro d hd hdi
    have := (hI.finIff i).mpr ⟨d, hd, hdi⟩
    rw [h] at this; cases this
  have hfind : S.decs.find? (fun d => d.txn == i) = none := by
    apply List.find?_eq_none.mpr
    intro d hd
    simp only [beq_iff_eq]
    exact hnf d hd
  have hd : s.store.decided i = none := by
    rw [hs]; simp [storeOf, hfind]
  refine ⟨_, PNext.storeRefuse s i 0 0 (req i).1 (req i).2 b (.error e) e hnet hd
    (by rw [hs]; simp [storeOf, hb]) rfl, ?_⟩
  apply storeState_eq'
  · funext x k
    rw [hs]; simp [storeOf]
  · funext x
    rw [hs]; simp [storeOf]
  · rw [hs]; simp [storeOf, List.filterMap_append]
  · funext r
    rw [hs]
    simp only [storeOf]
    rw [find_snoc _ _ _ hnf]
    by_cases hr : i = r
    · subst hr; simp [outcomeOf]
    · simp [hr, Ne.symm hr, outcomeOf]

theorem conflict_refines_store (K : Kernel) (C : Config) (Wd : TenantId → Option Tenant)
    (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant) (S : TState)
    (hI : TInv K req w0 S) (i : TxnId) (b : Nat) (res : Except Err Tenant)
    (h : S.ph i = .computed b res) (hb : b ≠ S.rev)
    (s : PState) (hs : s.store = storeOf Wd S)
    (hnet : s.net (.commit i 0 0 (req i).1 (req i).2 b res)) :
    ∃ s', PNext K C s s' ∧ s'.store = storeOf Wd { S with ph := upd S.ph i .idle } := by
  have hnf : ∀ d ∈ S.decs, d.txn ≠ i := by
    intro d hd hdi
    have := (hI.finIff i).mpr ⟨d, hd, hdi⟩
    rw [h] at this; cases this
  have hfind : S.decs.find? (fun d => d.txn == i) = none := by
    apply List.find?_eq_none.mpr
    intro d hd
    simp only [beq_iff_eq]
    exact hnf d hd
  have hd : s.store.decided i = none := by
    rw [hs]; simp [storeOf, hfind]
  refine ⟨_, PNext.storeConflict s i 0 0 (req i).1 (req i).2 b res hnet hd
    (by rw [hs]; simpa [storeOf] using hb), ?_⟩
  exact hs

/-- Every step of the transactions is a step of the protocol's store, or none: a load and a compute
    change no store state, a decision is the store's accept or refusal, and a conflict is the
    store's conflict. `hnet` says each computed transaction's commit has been sent, which the
    backend's commit step does. -/
theorem step_refines_store (K : Kernel) (C : Config) (Wd : TenantId → Option Tenant)
    (req : TxnId → PrincipalId × Intent) (w0 : Option Tenant) {S S' : TState}
    (hI : TInv K req w0 S) (h : TNext K req w0 S S') (s : PState)
    (hs : s.store = storeOf Wd S)
    (hnet : ∀ i b res, S.ph i = .computed b res → s.net (.commit i 0 0 (req i).1 (req i).2 b res)) :
    ∃ s', Star (PNext K C) s s' ∧ s'.store = storeOf Wd S' := by
  cases h with
  | load i hi => exact ⟨s, Star.refl s, hs⟩
  | compute i b v hi => exact ⟨s, Star.refl s, hs⟩
  | decideOk i b t' h hb =>
    obtain ⟨s', hst, hs'⟩ := decideOk_refines_store K C Wd req w0 S hI i b t' h hb s hs
      (hnet i b (.ok t') h)
    exact ⟨s', Star.tail (Star.refl s) hst, hs'⟩
  | decideErr i b e h hb =>
    obtain ⟨s', hst, hs'⟩ := decideErr_refines_store K C Wd req w0 S hI i b e h hb s hs
      (hnet i b (.error e) h)
    exact ⟨s', Star.tail (Star.refl s) hst, hs'⟩
  | conflict i b res h hb =>
    obtain ⟨s', hst, hs'⟩ := conflict_refines_store K C Wd req w0 S hI i b res h hb s hs
      (hnet i b res h)
    exact ⟨s', Star.tail (Star.refl s) hst, hs'⟩

end Strata.Reduction
