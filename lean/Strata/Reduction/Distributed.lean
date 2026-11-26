/-
The distributed reduction (WG-5660, part (b)): IronFleet section 3.6, generic and machine-checked.

Fine-grained model. Each host step of an IronFleet-shaped protocol is split into its I/O events: a
packet received (`Io.recv`), at most one time-dependent input (`Io.tick`), and packets sent
(`Io.send`). An event `Ev` names its host and the number of that host's step it belongs to. A fine
execution is a list of events, in the order they happened, with events of different hosts
interleaved arbitrarily. Its validity has four parts:

- the reduction obligation: within each host step, the receives come first, then at most one
  time-dependent event, then the sends (`Obl`);
- each host's events are in step order (`HostOrd`);
- no packet is received before it is sent, counting the packets sent from the start (`NetOk`);
- each host's steps are run by the host's step relation, from its initial state, with the inputs
  and outputs the events record (`FineExec`).

The main theorem, `reduction`, says that every fine execution is equivalent to one in which each
host step is contiguous: a permutation of the same events, with the same events for each host in
the same order (so each host receives the same packets in the same order, sends them in the same
order, and runs through the same local states), no packet received before it is sent, and each
step's events in one block.

The proof is a commutation argument. Take the first event `x` at which some step is complete (a
send, or the last event of its step); its step's events can be moved to the front. Receives in
front of `x` are receives of packets from the start (nothing is sent before `x`), so they stay
valid, and moving the sends of the step to the front keeps every later receive valid. Induction on
the length of the trace removes one step at a time.

No unfinished proof and no added axiom.
-/

namespace Strata.Reduction

/-- An I/O event of a host step: a packet received, a time-dependent input, or a packet sent. -/
inductive Io (P T : Type) where
  | recv (p : P)
  | tick (t : T)
  | send (p : P)

/-- A fine-grained event: an I/O event of `host`, belonging to that host's step number `step`. -/
structure Ev (H P T : Type) where
  host : H
  step : Nat
  io : Io P T

variable {H P T : Type}

/-- The position of an event kind within a step: receives first, then the time-dependent input,
    then the sends. -/
def rank : Io P T → Nat
  | .recv _ => 0
  | .tick _ => 1
  | .send _ => 2

/-- Whether an event belongs to step `k` of host `h`. -/
def tagIs [DecidableEq H] (h : H) (k : Nat) (e : Ev H P T) : Bool :=
  decide (e.host = h ∧ e.step = k)

/-- The events of step `k` of host `h`, in the order of the trace. -/
def seg [DecidableEq H] (τ : List (Ev H P T)) (h : H) (k : Nat) : List (Ev H P T) :=
  τ.filter (tagIs h k)

/-- The events of host `h`, in the order of the trace. -/
def hostEvs [DecidableEq H] (τ : List (Ev H P T)) (h : H) : List (Ev H P T) :=
  τ.filter (fun e => decide (e.host = h))

/-- The packets a list of events receives, in order. -/
def recvsOf (l : List (Ev H P T)) : List P :=
  l.filterMap fun e => match e.io with
    | .recv p => some p
    | _ => none

/-- The packets a list of events sends, in order. -/
def sendsOf (l : List (Ev H P T)) : List P :=
  l.filterMap fun e => match e.io with
    | .send p => some p
    | _ => none

/-- The time-dependent inputs of a list of events, in order. -/
def ticksOf (l : List (Ev H P T)) : List T :=
  l.filterMap fun e => match e.io with
    | .tick t => some t
    | _ => none

/-- The reduction obligation for the events of one host step: receives, then at most one
    time-dependent event, then sends. -/
def Obl (l : List (Ev H P T)) : Prop :=
  l.Pairwise (fun a b => rank a.io ≤ rank b.io) ∧ (ticksOf l).length ≤ 1

/-- Each host's events are in step order. -/
def HostOrd [DecidableEq H] (τ : List (Ev H P T)) : Prop :=
  ∀ h, (hostEvs τ h).Pairwise (fun a b => a.step ≤ b.step)

/-- No packet is received before it is sent: `N` is the set of packets available at the start,
    and each send makes its packet available to the events after it. -/
def NetOk (N : P → Prop) : List (Ev H P T) → Prop
  | [] => True
  | e :: es => (∀ p, e.io = .recv p → N p) ∧ NetOk (fun q => N q ∨ e.io = .send q) es

/-- A fine-grained execution of a protocol with host step relation `HStep` from `init`, in which
    host `h` takes `N h` steps. -/
structure FineExec [DecidableEq H] {σ : Type}
    (HStep : H → σ → List P → Option T → σ → List P → Prop) (init : H → σ) (N : H → Nat)
    (τ : List (Ev H P T)) : Prop where
  /-- Every event belongs to a step that exists. -/
  bounded : ∀ e ∈ τ, e.step < N e.host
  /-- Each host's events are in step order. -/
  hostOrd : HostOrd τ
  /-- Every step satisfies the reduction obligation. -/
  obl : ∀ h k, Obl (seg τ h k)
  /-- No packet is received before it is sent. -/
  net : NetOk (fun _ => False) τ
  /-- Each host's steps are run by its step relation, from its initial state. -/
  run : ∃ st : H → Nat → σ, (∀ h, st h 0 = init h) ∧
    ∀ h k, k < N h → HStep h (st h k) (recvsOf (seg τ h k)) (ticksOf (seg τ h k)).head?
      (st h (k + 1)) (sendsOf (seg τ h k))

/-! ## Lists of events -/

theorem allRecvIn_netOk (N : P → Prop) :
    ∀ (l : List (Ev H P T)), (∀ y ∈ l, ∀ p, y.io = .recv p → N p) → NetOk N l
  | [], _ => trivial
  | e :: es, h => by
    refine ⟨fun p hp => h e (by simp) p hp, ?_⟩
    apply allRecvIn_netOk
    intro y hy p hp
    exact Or.inl (h y (List.mem_cons_of_mem _ hy) p hp)

theorem netOk_mono {N N' : P → Prop} (h : ∀ q, N q → N' q) :
    ∀ (l : List (Ev H P T)), NetOk N l → NetOk N' l
  | [], _ => trivial
  | e :: es, hN => by
    refine ⟨fun p hp => h p (hN.1 p hp), netOk_mono ?_ es hN.2⟩
    intro q hq
    exact hq.elim (fun h1 => Or.inl (h q h1)) (fun h2 => Or.inr h2)

theorem netOk_congr {N N' : P → Prop} (h : ∀ q, N q ↔ N' q) (l : List (Ev H P T)) :
    NetOk N l ↔ NetOk N' l :=
  ⟨netOk_mono (fun q hq => (h q).mp hq) l, netOk_mono (fun q hq => (h q).mpr hq) l⟩

theorem set_cons (N : P → Prop) (e : Ev H P T) (l : List (Ev H P T)) (q : P) :
    ((N q ∨ e.io = .send q) ∨ ∃ y ∈ l, y.io = .send q) ↔
      (N q ∨ ∃ y ∈ e :: l, y.io = .send q) := by
  constructor
  · rintro ((h | h) | ⟨y, hy, hy'⟩)
    · exact Or.inl h
    · exact Or.inr ⟨e, by simp, h⟩
    · exact Or.inr ⟨y, List.mem_cons_of_mem _ hy, hy'⟩
  · rintro (h | ⟨y, hy, hy'⟩)
    · exact Or.inl (Or.inl h)
    · rcases List.mem_cons.mp hy with rfl | hy
      · exact Or.inl (Or.inr hy')
      · exact Or.inr ⟨y, hy, hy'⟩

/-- A list's receives are available as in the list alone, with the sends of the events before. -/
theorem netOk_append (N : P → Prop) : ∀ (l1 l2 : List (Ev H P T)),
    NetOk N (l1 ++ l2) ↔
      NetOk N l1 ∧ NetOk (fun q => N q ∨ ∃ y ∈ l1, y.io = .send q) l2
  | [], l2 => by
    have hs : ∀ q, N q ↔ (N q ∨ ∃ y ∈ ([] : List (Ev H P T)), y.io = .send q) := by
      intro q
      constructor
      · exact Or.inl
      · rintro (h | ⟨y, hy, _⟩)
        · exact h
        · simp at hy
    simp only [List.nil_append, NetOk, true_and]
    exact netOk_congr hs l2
  | e :: l1, l2 => by
    have ih := netOk_append (fun q => N q ∨ e.io = .send q) l1 l2
    have hset : ∀ q, ((N q ∨ e.io = .send q) ∨ ∃ y ∈ l1, y.io = .send q) ↔
        (N q ∨ ∃ y ∈ e :: l1, y.io = .send q) := fun q => set_cons N e l1 q
    simp only [List.cons_append, NetOk]
    constructor
    · rintro ⟨h1, h2⟩
      obtain ⟨h2a, h2b⟩ := ih.mp h2
      exact ⟨⟨h1, h2a⟩, (netOk_congr (N := fun q => (N q ∨ e.io = .send q) ∨
        ∃ y ∈ l1, y.io = .send q) (N' := fun q => N q ∨ ∃ y ∈ e :: l1, y.io = .send q)
        hset l2).mp h2b⟩
    · rintro ⟨⟨h1, h2a⟩, h2b⟩
      exact ⟨h1, ih.mpr ⟨h2a, (netOk_congr (N := fun q => N q ∨ ∃ y ∈ e :: l1,
        y.io = .send q) (N' := fun q => (N q ∨ e.io = .send q) ∨ ∃ y ∈ l1, y.io = .send q)
        (fun q => (hset q).symm) l2).mp h2b⟩⟩

theorem netOk_prefix (N : P → Prop) (l1 l2 : List (Ev H P T)) (h : NetOk N (l1 ++ l2)) :
    NetOk N l1 :=
  ((netOk_append N l1 l2).mp h).1

/-- Receives of events that send nothing are available at the start. -/
theorem recv_of_nosend (N : P → Prop) : ∀ (A rest : List (Ev H P T)),
    NetOk N (A ++ rest) → (∀ y ∈ A, ∀ q, y.io ≠ .send q) →
      ∀ y ∈ A, ∀ p, y.io = .recv p → N p
  | [], _, _, _, y, hy, _, _ => by simp at hy
  | e :: A, rest, h, hs, y, hy, p, hp => by
    have h' : (∀ p, e.io = .recv p → N p) ∧
        NetOk (fun q => N q ∨ e.io = .send q) (A ++ rest) := h
    rcases List.mem_cons.mp hy with rfl | hy'
    · exact h'.1 p hp
    · have key := recv_of_nosend (fun q => N q ∨ e.io = .send q) A rest h'.2
        (fun y' hy'' q => hs y' (List.mem_cons_of_mem _ hy'') q) y hy' p hp
      exact key.elim id (fun h4 => absurd h4 (hs e (by simp) p))

/-! ## Splitting a trace at its first complete step -/

/-- An event is a candidate for the front of the trace when it is a send, or when no later event
    belongs to its step (it is the last event of its step). -/
def Cand [DecidableEq H] (x : Ev H P T) (after : List (Ev H P T)) : Prop :=
  (∃ q, x.io = .send q) ∨ ∀ z ∈ after, ¬ (z.host = x.host ∧ z.step = x.step)

/-- The first candidate of a nonempty trace: the events before it are not sends, and each of them
    has a later event of its own step, at or after the candidate. -/
theorem split_first [DecidableEq H] : ∀ (τ : List (Ev H P T)), τ ≠ [] →
    ∃ A x B, τ = A ++ x :: B ∧ Cand x B ∧
      ∀ y ∈ A, (∀ q, y.io ≠ .send q) ∧ ∃ z ∈ x :: B, z.host = y.host ∧ z.step = y.step
  | [], h => absurd rfl h
  | e :: es, _ => by
    by_cases hc : Cand e es
    · exact ⟨[], e, es, by simp, hc, fun y hy => by simp at hy⟩
    · have hns : ∀ q, e.io ≠ .send q := fun q hq => hc (Or.inl ⟨q, hq⟩)
      have hz : ∃ z ∈ es, z.host = e.host ∧ z.step = e.step := by
        apply Classical.byContradiction
        intro hcon
        exact hc (Or.inr (fun z hz hsame => hcon ⟨z, hz, hsame⟩))
      obtain ⟨z, hzmem, hzsame⟩ := hz
      obtain ⟨A', x, B, hes, hcx, hA'⟩ := split_first es (by
        rintro rfl
        simp at hzmem)
      refine ⟨e :: A', x, B, by rw [hes]; rfl, hcx, ?_⟩
      intro y hy
      rcases List.mem_cons.mp hy with rfl | hy'
      · refine ⟨hns, ?_⟩
        rw [hes] at hzmem
        rcases List.mem_append.mp hzmem with hzA | hzB
        · obtain ⟨_, z', hz', h1, h2⟩ := hA' z hzA
          exact ⟨z', hz', h1.trans hzsame.1, h2.trans hzsame.2⟩
        · exact ⟨z, hzB, hzsame⟩
      · exact hA' y hy'

/-! ## Steps and their removal -/

/-- The events of step `k` of host `h`, as the events of the host. -/
theorem seg_eq_hostEvs [DecidableEq H] (τ : List (Ev H P T)) (h : H) (k : Nat) :
    seg τ h k = (hostEvs τ h).filter (fun e => decide (e.step = k)) := by
  unfold seg hostEvs
  rw [List.filter_filter]
  apply List.filter_congr
  intro e _
  simp [tagIs, Bool.and_comm]

/-- Two different steps have no event in common. -/
theorem tagIs_unique [DecidableEq H] {h h0 : H} {k k0 : Nat} {e : Ev H P T}
    (hne : h ≠ h0 ∨ k ≠ k0) (ht : tagIs h k e = true) : tagIs h0 k0 e = false := by
  simp only [tagIs, decide_eq_true_eq] at ht
  simp only [tagIs]
  rw [decide_eq_false_iff_not]
  rintro ⟨h1, h2⟩
  rcases hne with hne | hne
  · exact hne (ht.1.symm.trans h1)
  · exact hne (ht.2.symm.trans h2)

/-- Removing a step's events keeps the receives of the others, with the packets of the removed
    events' sends available from the start. -/
theorem netOk_remove (P' : Ev H P T → Bool) (Sg : P → Prop) :
    ∀ (τ : List (Ev H P T)) (N : P → Prop), NetOk N τ →
      (∀ y ∈ τ, P' y = true → ∀ q, y.io = .send q → Sg q) →
      NetOk (fun q => N q ∨ Sg q) (τ.filter (fun y => !P' y))
  | [], _, _, _ => trivial
  | e :: es, N, h, hSg => by
    have h' : (∀ p, e.io = .recv p → N p) ∧
        NetOk (fun q => N q ∨ e.io = .send q) es := h
    have hSges : ∀ y ∈ es, P' y = true → ∀ q, y.io = .send q → Sg q :=
      fun y hy => hSg y (List.mem_cons_of_mem _ hy)
    by_cases hp : P' e = true
    · have hSge : ∀ q, e.io = .send q → Sg q := fun q hq => hSg e (by simp) hp q hq
      have ih := netOk_remove P' Sg es (fun q => N q ∨ e.io = .send q) h'.2 hSges
      have hf : (e :: es).filter (fun y => !P' y) = es.filter (fun y => !P' y) := by
        simp [List.filter_cons, hp]
      rw [hf]
      have hiff : ∀ q, (N q ∨ Sg q) ↔ ((N q ∨ e.io = .send q) ∨ Sg q) := by
        intro q
        constructor
        · rintro (h1 | h1)
          · exact Or.inl (Or.inl h1)
          · exact Or.inr h1
        · rintro ((h1 | h1) | h1)
          · exact Or.inl h1
          · exact Or.inr (hSge q h1)
          · exact Or.inr h1
      exact (netOk_congr hiff _).mpr ih
    · have hp' : P' e = false := by simpa using hp
      have ih := netOk_remove P' Sg es (fun q => N q ∨ e.io = .send q) h'.2 hSges
      have hf : (e :: es).filter (fun y => !P' y) = e :: es.filter (fun y => !P' y) := by
        simp [List.filter_cons, hp']
      rw [hf]
      refine ⟨fun p hp2 => Or.inl (h'.1 p hp2), ?_⟩
      have hiff : ∀ q, ((N q ∨ Sg q) ∨ e.io = .send q) ↔ ((N q ∨ e.io = .send q) ∨ Sg q) := by
        intro q; constructor
        · rintro ((h1 | h1) | h1)
          · exact Or.inl (Or.inl h1)
          · exact Or.inr h1
          · exact Or.inl (Or.inr h1)
        · rintro ((h1 | h1) | h1)
          · exact Or.inl (Or.inl h1)
          · exact Or.inr h1
          · exact Or.inl (Or.inr h1)
      exact (netOk_congr hiff _).mpr ih

/-! ## Host projections -/

theorem hostEvs_append [DecidableEq H] (l1 l2 : List (Ev H P T)) (h : H) :
    hostEvs (l1 ++ l2) h = hostEvs l1 h ++ hostEvs l2 h := by
  unfold hostEvs
  exact List.filter_append _ _

theorem hostEvs_filter [DecidableEq H] (τ : List (Ev H P T)) (p : Ev H P T → Bool) (h : H) :
    hostEvs (τ.filter p) h = (hostEvs τ h).filter p := by
  unfold hostEvs
  rw [List.filter_filter, List.filter_filter]
  congr 1
  funext a
  exact Bool.and_comm _ _

theorem hostEvs_of_all [DecidableEq H] (l : List (Ev H P T)) (h : H)
    (hl : ∀ e ∈ l, e.host = h) : hostEvs l h = l := by
  unfold hostEvs
  exact List.filter_eq_self.mpr (fun a ha => by simp [hl a ha])

theorem hostEvs_of_none [DecidableEq H] (l : List (Ev H P T)) (h : H)
    (hl : ∀ e ∈ l, e.host ≠ h) : hostEvs l h = [] := by
  unfold hostEvs
  exact List.filter_eq_nil_iff.mpr (fun a ha => by simp [hl a ha])

/-- A sorted list whose steps are all at or after `k0` splits into its `k0` steps, then the rest. -/
theorem sorted_split (k0 : Nat) : ∀ (L : List (Ev H P T)),
    L.Pairwise (fun a b => a.step ≤ b.step) → (∀ e ∈ L, k0 ≤ e.step) →
    L.filter (fun e => decide (e.step = k0)) ++ L.filter (fun e => !decide (e.step = k0)) = L
  | [], _, _ => rfl
  | e :: L', hp, hk => by
    have hp' := List.pairwise_cons.mp hp
    have ih := sorted_split k0 L' hp'.2 (fun y hy => hk y (List.mem_cons_of_mem _ hy))
    have he : k0 ≤ e.step := hk e (by simp)
    by_cases hs : e.step = k0
    · have h1 : (e :: L').filter (fun e => decide (e.step = k0)) =
          e :: L'.filter (fun e => decide (e.step = k0)) :=
        List.filter_cons_of_pos (by simp [hs])
      have h2 : (e :: L').filter (fun e => !decide (e.step = k0)) =
          L'.filter (fun e => !decide (e.step = k0)) :=
        List.filter_cons_of_neg (by simp [hs])
      rw [h1, h2, List.cons_append, ih]
    · have hL'eq : L'.filter (fun e => decide (e.step = k0)) = [] := by
        apply List.filter_eq_nil_iff.mpr
        intro a ha h
        have h3 := hp'.1 a ha
        have h4 : a.step = k0 := by simpa using h
        omega
      have hall : L'.filter (fun e => !decide (e.step = k0)) = L' := by
        apply List.filter_eq_self.mpr
        intro a ha
        have h3 := hp'.1 a ha
        show (!decide (a.step = k0)) = true
        simp only [Bool.not_eq_true', decide_eq_false_iff_not]
        omega
      have h1 : (e :: L').filter (fun e => decide (e.step = k0)) =
          L'.filter (fun e => decide (e.step = k0)) :=
        List.filter_cons_of_neg (by simp [hs])
      have h2 : (e :: L').filter (fun e => !decide (e.step = k0)) =
          e :: L'.filter (fun e => !decide (e.step = k0)) :=
        List.filter_cons_of_pos (by simp [hs])
      rw [h1, h2, hL'eq, hall]
      simp

/-- Splitting a trace at its first step: the host's projection is the step's events, then the
    rest, because no earlier step of that host exists. -/
theorem hostEvs_split [DecidableEq H] (τ : List (Ev H P T)) (h0 : H) (k0 : Nat)
    (hord : (hostEvs τ h0).Pairwise (fun a b => a.step ≤ b.step))
    (hge : ∀ e ∈ τ, e.host = h0 → k0 ≤ e.step) (h : H) :
    hostEvs (seg τ h0 k0 ++ τ.filter (fun e => !tagIs h0 k0 e)) h = hostEvs τ h := by
  rw [hostEvs_append, hostEvs_filter]
  by_cases hh : h = h0
  · subst hh
    have hA : hostEvs (seg τ h k0) h = seg τ h k0 := by
      apply hostEvs_of_all
      intro e he
      have hm := (List.mem_filter.mp he).2
      simp only [tagIs, decide_eq_true_eq] at hm
      exact hm.1
    have hge' : ∀ e ∈ hostEvs τ h, k0 ≤ e.step := by
      intro e he
      have hm := List.mem_filter.mp he
      exact hge e hm.1 (by simpa using hm.2)
    have hfilt : (hostEvs τ h).filter (fun e => !tagIs h k0 e) =
        (hostEvs τ h).filter (fun e => !decide (e.step = k0)) := by
      apply List.filter_congr
      intro e he
      have hm := List.mem_filter.mp he
      have hhe : e.host = h := by simpa using hm.2
      simp [tagIs, hhe]
    rw [hA, hfilt, seg_eq_hostEvs]
    exact sorted_split k0 (hostEvs τ h) hord hge'
  · have hA : hostEvs (seg τ h0 k0) h = [] := by
      apply hostEvs_of_none
      intro e he h'
      have hm := (List.mem_filter.mp he).2
      simp only [tagIs, decide_eq_true_eq] at hm
      exact hh (h'.symm.trans hm.1)
    rw [hA, List.nil_append]
    apply List.filter_eq_self.mpr
    intro e he
    have hhe : e.host = h := by simpa using (List.mem_filter.mp he).2
    show (!tagIs h0 k0 e) = true
    simp only [tagIs, Bool.not_eq_true', decide_eq_false_iff_not]
    intro hc
    exact hh (hhe.symm.trans hc.1)

/-- Removing a step leaves the other steps unchanged. -/
theorem seg_remove_ne [DecidableEq H] (τ : List (Ev H P T)) (h0 h : H) (k0 k : Nat)
    (hne : h ≠ h0 ∨ k ≠ k0) :
    seg (τ.filter (fun e => !tagIs h0 k0 e)) h k = seg τ h k := by
  unfold seg
  rw [List.filter_filter]
  apply List.filter_congr
  intro e _
  by_cases ht : tagIs h k e = true
  · have := tagIs_unique hne ht
    simp [this, ht]
  · simp [ht]

theorem seg_remove_self [DecidableEq H] (τ : List (Ev H P T)) (h0 : H) (k0 : Nat) :
    seg (τ.filter (fun e => !tagIs h0 k0 e)) h0 k0 = [] := by
  unfold seg
  rw [List.filter_filter]
  apply List.filter_eq_nil_iff.mpr
  intro a _
  simp

/-! ## The receives of the first step -/

/-- The receives of the first step are available from the start: the events before it send
    nothing, and the step's own events are the candidate and the events before it. -/
theorem seg_netOk [DecidableEq H] (N0 : P → Prop) (A B : List (Ev H P T)) (x : Ev H P T)
    (hnet : NetOk N0 (A ++ x :: B))
    (hA : ∀ y ∈ A, ∀ q, y.io ≠ .send q)
    (hobl : Obl (seg (A ++ x :: B) x.host x.step))
    (hcx : Cand x B) :
    NetOk N0 (seg (A ++ x :: B) x.host x.step) := by
  apply allRecvIn_netOk
  intro y hy p hp
  have hmem := List.mem_filter.mp hy
  have htag : tagIs x.host x.step y = true := hmem.2
  have hy' := hmem.1
  rcases List.mem_append.mp hy' with hyA | hyxB
  · exact recv_of_nosend N0 A (x :: B) hnet hA y hyA p hp
  · rcases List.mem_cons.mp hyxB with rfl | hyB
    · have h1 : NetOk N0 (A ++ [y] ++ B) := by simpa using hnet
      have h2 := ((netOk_append N0 (A ++ [y]) B).mp h1).1
      have h3 := ((netOk_append N0 A [y]).mp h2).2
      rcases h3.1 p hp with h5 | ⟨z, hz, hzs⟩
      · exact h5
      · exact absurd hzs (hA z hz p)
    · rcases hcx with ⟨q, hq⟩ | hall
      · have hfilt : seg (A ++ x :: B) x.host x.step =
            A.filter (tagIs x.host x.step) ++ x :: B.filter (tagIs x.host x.step) := by
          unfold seg
          rw [List.filter_append, List.filter_cons_of_pos (by simp [tagIs])]
        have hpw := hobl.1
        rw [hfilt] at hpw
        obtain ⟨_, hcons, _⟩ := List.pairwise_append.mp hpw
        have hr := (List.pairwise_cons.mp hcons).1 y
          (List.mem_filter.mpr ⟨hyB, htag⟩)
        rw [hq, hp] at hr
        simp [rank] at hr
      · simp only [tagIs, decide_eq_true_eq] at htag
        exact absurd htag (hall y hyB)

/-! ## The reordering -/

theorem ne_or_of_not_and {a b : Prop} (h : ¬(a ∧ b)) : ¬a ∨ ¬b := by
  by_cases ha : a
  · exact Or.inr (fun hb => h ⟨ha, hb⟩)
  · exact Or.inl ha

/-- Removing the events of one step leaves the step `q` unchanged, when the step is nonempty. -/
theorem seg_remove_of_nonempty [DecidableEq H] (τ : List (Ev H P T)) (h0 : H) (k0 : Nat)
    (q : H × Nat) (hne : seg (τ.filter (fun e => !tagIs h0 k0 e)) q.1 q.2 ≠ []) :
    seg (τ.filter (fun e => !tagIs h0 k0 e)) q.1 q.2 = seg τ q.1 q.2 := by
  by_cases hq : q.1 = h0 ∧ q.2 = k0
  · have h2 : seg (τ.filter (fun e => !tagIs h0 k0 e)) q.1 q.2 = [] := by
      rw [hq.1, hq.2]
      exact seg_remove_self τ h0 k0
    exact absurd h2 hne
  · exact seg_remove_ne τ h0 q.1 k0 q.2 (ne_or_of_not_and hq)

/-- `τ'` is `τ` with each host step made contiguous: the same events, the same events of each
    host in the same order, each step's events in one block, and the blocks listed in an order
    in which each host's steps increase. -/
def Reordered [DecidableEq H] (τ τ' : List (Ev H P T)) : Prop :=
  τ'.Perm τ ∧ (∀ h, hostEvs τ' h = hostEvs τ h) ∧
    (∀ h k, ∃ A B : List (Ev H P T), τ' = A ++ seg τ h k ++ B ∧
      (∀ e ∈ A, tagIs h k e = false) ∧ (∀ e ∈ B, tagIs h k e = false)) ∧
    (∃ bs : List (H × Nat), τ' = (bs.map (fun p => seg τ p.1 p.2)).flatten ∧
      bs.Pairwise (fun p q => p.1 = q.1 → p.2 < q.2) ∧ ∀ p ∈ bs, seg τ p.1 p.2 ≠ [])

theorem reordered_nil [DecidableEq H] : Reordered ([] : List (Ev H P T)) [] :=
  ⟨List.Perm.refl _, fun _ => rfl, fun h k => ⟨[], [], by simp [seg], by simp, by simp⟩,
    ⟨[], by simp, List.Pairwise.nil, by simp⟩⟩

/-- The core of the reduction. Every trace whose steps satisfy the obligation, whose hosts run in
    step order, and whose receives are available (from `N0`, or sent earlier) is reordered into one
    where each step is contiguous, with the same events for each host and every receive still
    after its send. -/
theorem reorder_core [DecidableEq H] (n : Nat) : ∀ (N0 : P → Prop) (τ : List (Ev H P T)),
    τ.length ≤ n → HostOrd τ → (∀ h k, Obl (seg τ h k)) → NetOk N0 τ →
    ∃ τ' : List (Ev H P T), Reordered τ τ' ∧ NetOk N0 τ' := by
  induction n with
  | zero =>
    intro N0 τ hlen _ _ _
    have hnil : τ = [] := List.eq_nil_of_length_eq_zero (Nat.le_zero.mp hlen)
    subst hnil
    exact ⟨[], reordered_nil, trivial⟩
  | succ n ih =>
    intro N0 τ hlen hord hobl hnet
    by_cases hnil : τ = []
    · subst hnil
      exact ⟨[], reordered_nil, trivial⟩
    obtain ⟨A, x, B, hτ, hcx, hA⟩ := split_first τ hnil
    subst hτ
    -- the first step is complete at `x`: every earlier event of its host is in an earlier step
    have hge : ∀ e ∈ A ++ x :: B, e.host = x.host → x.step ≤ e.step := by
      intro e he hh
      have hpw := hord x.host
      have hsplit : hostEvs (A ++ x :: B) x.host = hostEvs A x.host ++ (x :: hostEvs B x.host) := by
        rw [hostEvs_append]
        congr 1
        unfold hostEvs
        exact List.filter_cons_of_pos (by simp)
      rw [hsplit] at hpw
      obtain ⟨_, hcons, _⟩ := List.pairwise_append.mp hpw
      have hBle := (List.pairwise_cons.mp hcons).1
      rcases List.mem_append.mp he with heA | heXB
      · refine Classical.byContradiction (fun hlt => ?_)
        have hlt' : e.step < x.step := Nat.lt_of_not_le hlt
        obtain ⟨_, z, hz, hzh, hzs⟩ := hA e heA
        rcases List.mem_cons.mp hz with rfl | hzB
        · omega
        · have hzmem : z ∈ hostEvs B x.host := by
            unfold hostEvs
            exact List.mem_filter.mpr ⟨hzB, by simpa using (hzh.trans hh)⟩
          have := hBle z hzmem
          omega
      · rcases List.mem_cons.mp heXB with rfl | heB
        · exact Nat.le_refl _
        · have hemem : e ∈ hostEvs B x.host := by
            unfold hostEvs
            exact List.mem_filter.mpr ⟨heB, by simpa using hh⟩
          exact hBle e hemem
    have hxs : tagIs x.host x.step x = true := by simp [tagIs]
    have hxmem : x ∈ seg (A ++ x :: B) x.host x.step :=
      List.mem_filter.mpr ⟨by simp, hxs⟩
    have hPerm : (seg (A ++ x :: B) x.host x.step ++
        (A ++ x :: B).filter (fun e => !tagIs x.host x.step e)).Perm (A ++ x :: B) :=
      List.filter_append_perm _ _
    have hlenR : ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e)).length ≤ n := by
      have h1 := hPerm.length_eq
      have hlenpos : 0 < (seg (A ++ x :: B) x.host x.step).length :=
        List.length_pos_of_mem hxmem
      simp only [List.length_append, List.length_cons] at h1 hlen
      omega
    -- the rest of the trace, and its own hypotheses
    have hordR : HostOrd ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e)) := by
      intro h
      rw [hostEvs_filter]
      exact List.Pairwise.filter _ (hord h)
    have hSg : NetOk (fun q => N0 q ∨ ∃ y ∈ seg (A ++ x :: B) x.host x.step, y.io = .send q)
        ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e)) :=
      netOk_remove (tagIs x.host x.step)
        (fun q => ∃ y ∈ seg (A ++ x :: B) x.host x.step, y.io = .send q) (A ++ x :: B) N0 hnet
        (fun y hy hyt q hq => ⟨y, List.mem_filter.mpr ⟨hy, hyt⟩, hq⟩)
    have hoblR : ∀ h k, Obl (seg ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e)) h k) := by
      intro h k
      by_cases hc : h = x.host ∧ k = x.step
      · rw [hc.1, hc.2, seg_remove_self]
        exact ⟨List.Pairwise.nil, by simp [ticksOf]⟩
      · rw [seg_remove_ne _ _ _ _ _ (ne_or_of_not_and hc)]
        exact hobl h k
    obtain ⟨R', hRre, hRnet⟩ :=
      ih (fun q => N0 q ∨ ∃ y ∈ seg (A ++ x :: B) x.host x.step, y.io = .send q)
        ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e)) hlenR hordR hoblR hSg
    obtain ⟨hRp, hRh, hRc, bsR, hR_eq, hR_pair, hR_ne⟩ := hRre
    refine ⟨seg (A ++ x :: B) x.host x.step ++ R', ⟨?_, ?_, ?_, ?_⟩, ?_⟩
    · exact (List.Perm.append_left _ hRp).trans hPerm
    · intro h
      rw [hostEvs_append, hRh h, ← hostEvs_append]
      exact hostEvs_split (A ++ x :: B) x.host x.step (hord x.host) hge h
    · intro h k
      by_cases hc : h = x.host ∧ k = x.step
      · obtain ⟨rfl, rfl⟩ := hc
        refine ⟨[], R', by simp, by simp, ?_⟩
        intro e he
        have hemem := hRp.mem_iff.mp he
        have := (List.mem_filter.mp hemem).2
        simpa using this
      · have hne : h ≠ x.host ∨ k ≠ x.step := ne_or_of_not_and hc
        obtain ⟨A'', B'', hR'eq, hA'', hB''⟩ := hRc h k
        rw [seg_remove_ne _ _ _ _ _ hne] at hR'eq
        refine ⟨seg (A ++ x :: B) x.host x.step ++ A'', B'', ?_, ?_, hB''⟩
        · rw [hR'eq]
          simp [List.append_assoc]
        · intro e he
          rcases List.mem_append.mp he with he | he
          · have ht : tagIs x.host x.step e = true := (List.mem_filter.mp he).2
            have hne' : x.host ≠ h ∨ x.step ≠ k :=
              hne.elim (fun h1 => Or.inl (Ne.symm h1)) (fun h1 => Or.inr (Ne.symm h1))
            exact tagIs_unique hne' ht
          · exact hA'' e he
    · -- the block order: the first step, then the blocks of the rest
      have hmap : bsR.map (fun p => seg ((A ++ x :: B).filter (fun e => !tagIs x.host x.step e))
          p.1 p.2) = bsR.map (fun p => seg (A ++ x :: B) p.1 p.2) :=
        List.map_congr_left (fun q hq => seg_remove_of_nonempty _ x.host x.step q (hR_ne q hq))
      refine ⟨(x.host, x.step) :: bsR, ?_, ?_, ?_⟩
      · simp only [List.map_cons, List.flatten_cons]
        rw [hR_eq, hmap]
      · refine List.pairwise_cons.mpr ⟨fun q hq hh => ?_, hR_pair⟩
        have hne := hR_ne q hq
        obtain ⟨e, he⟩ := List.exists_mem_of_ne_nil _ hne
        have he' := List.mem_filter.mp he
        have heτ := (List.mem_filter.mp he'.1).1
        have htag := he'.2
        simp only [tagIs, decide_eq_true_eq] at htag
        have hle := hge e heτ (htag.1.trans hh.symm)
        have hne2 : q.2 ≠ x.step := by
          intro h2
          apply hne
          have hq2 : q = (x.host, x.step) := Prod.ext hh.symm h2
          rw [hq2]
          exact seg_remove_self _ _ _
        have hqe : q.2 = e.step := htag.2.symm
        omega
      · intro p hp
        rcases List.mem_cons.mp hp with rfl | hpR
        · exact List.ne_nil_of_mem hxmem
        · have h1 := hR_ne p hpR
          rw [seg_remove_of_nonempty _ x.host x.step p h1] at h1
          exact h1
    · exact (netOk_append N0 _ _).mpr ⟨seg_netOk N0 A B x hnet (fun y hy => (hA y hy).1)
        (hobl x.host x.step) hcx, hRnet⟩

/-- The reduction (IronFleet section 3.6, generic). Every fine-grained execution of a protocol in
    the IronFleet shape is equivalent to one in which each host step is contiguous: the same
    events, the same events of each host in the same order (so each host receives the same packets
    in the same order and sends the same packets in the same order, and each host runs through the
    same local states), no packet received before it is sent, and each step's events in one block.
    The reordered execution is a fine execution of the same protocol with the same host states. -/
theorem reduction [DecidableEq H] {σ : Type}
    {HStep : H → σ → List P → Option T → σ → List P → Prop} {init : H → σ} {N : H → Nat}
    {τ : List (Ev H P T)} (hE : FineExec HStep init N τ) :
    ∃ τ' : List (Ev H P T), τ'.Perm τ ∧ (∀ h, hostEvs τ' h = hostEvs τ h) ∧
      (∀ h k, ∃ A B : List (Ev H P T), τ' = A ++ seg τ h k ++ B ∧
        (∀ e ∈ A, tagIs h k e = false) ∧ (∀ e ∈ B, tagIs h k e = false)) ∧
      FineExec HStep init N τ' := by
  obtain ⟨τ', hR, hNet⟩ := reorder_core τ.length (fun _ => False) τ (Nat.le_refl _) hE.hostOrd
    hE.obl hE.net
  have hseg : ∀ h k, seg τ' h k = seg τ h k := by
    intro h k
    rw [seg_eq_hostEvs, seg_eq_hostEvs, hR.2.1 h]
  refine ⟨τ', hR.1, hR.2.1, hR.2.2.1, ?_⟩
  exact
    { bounded := fun e he => hE.bounded e (hR.1.mem_iff.mp he)
      hostOrd := fun h => by rw [hR.2.1 h]; exact hE.hostOrd h
      obl := fun h k => by rw [hseg]; exact hE.obl h k
      net := hNet
      run := by
        obtain ⟨st, h0, hs⟩ := hE.run
        exact ⟨st, h0, fun h k hk => by rw [hseg]; exact hs h k hk⟩ }

/-- A run of a fine execution is a run of any execution with the same events for each step: the
    host steps and their states are the same. -/
theorem run_congr {H P T σ : Type} [DecidableEq H] (HStep : H → σ → List P → Option T → σ → List P → Prop)
    (N : H → Nat) (τ τ' : List (Ev H P T)) (hseg : ∀ h k, seg τ' h k = seg τ h k) (st : H → Nat → σ) :
    (∀ h k, k < N h → HStep h (st h k) (recvsOf (seg τ h k)) (ticksOf (seg τ h k)).head?
      (st h (k + 1)) (sendsOf (seg τ h k))) ↔
    (∀ h k, k < N h → HStep h (st h k) (recvsOf (seg τ' h k)) (ticksOf (seg τ' h k)).head?
      (st h (k + 1)) (sendsOf (seg τ' h k))) := by
  simp only [hseg]

end Strata.Reduction
