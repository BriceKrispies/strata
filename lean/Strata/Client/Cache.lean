/-
The browser's cache of view results, against the server it copies (docs/frontend.md, "The cache
and its coherence").

The browser cannot read the store's generation the way a runtime can (lean/Strata/WorkingSet.lean,
`checkedRead_linearizes`). It learns generations from what the server sends it: a read reply
stamped with the one generation the whole reply was read at (the request-snapshot rule of the
working set, `request_snapshot`), a commit receipt stamped with the generation the commit made,
and notices on the socket, each saying which keys may differ between two generations. So the
client's knowledge lags, and the property it can have is relative to what it knows:

- **never staler than the last known generation**: an answer served as current is the server's
  value at the newest generation the client has heard of; anything older is served only labelled
  stale, with its stamp (`read_current_eq`, `read_stale_older`, `run_coh`);
- **read after a confirmed commit**: after the receipt of a commit, a read of the key it wrote
  returns the committed value, unless the client has by then heard of a later generation, in
  which case it returns the server's value there or a labelled stale answer (`read_after_commit`,
  `receipt_read`, `notice_keeps_current`).

A model of outcomes over abstract keys and values (`K`, `V`): the server is the value of every key
at every generation it has reached, and a commit defines the next generation without changing an
earlier one (assumption C1 below). The section "The working set's keys" instantiates it with the
working set's `Key`, `Payload` and `fetch`.

Assumptions, all of them:

C1. The server's history is immutable: a commit defines generation `gen + 1` and changes no earlier
    generation's values. This is W4 of the working set seen from outside: a changed state never
    shows a generation already seen.
C2. A notice `notice a b ks` is complete for the keys the client may hold: every such key whose
    value differs between generations `a` and `b` is in `ks`. This is the visible-notice rule
    (D13: who hears about an event is the kernel's read decision, evaluated before and after the
    change) plus a notice for every out-of-commit change that alters a visible value (a restore,
    an anonymisation). A notice that is not complete is ignored by the model's step (`step`), as a
    server would never send it; what happens when a real server breaks C2 is the next item.
C3. A key destruction (D-350) is not a change of the history; the section "Erasure" shows that the
    browser, which holds opened values, therefore needs an erasure epoch in its stamp.

No `sorry`, no added axioms. `Audit.lean` prints the axioms each theorem depends on.
-/

import Strata.WorkingSet

namespace Strata.Client.Cache

open Strata

/-! ## The server, as a client observes it -/

/-- The generation the server has reached, and the value of every key at every generation. Values
    at generations above `gen` mean nothing. -/
structure Server (K V : Type) where
  gen : Nat
  hist : Nat → K → V

/-- Any commit by anyone, or any other change to what a read returns: the next generation. -/
def Server.commit {K V : Type} (s : Server K V) (f : K → V) : Server K V :=
  { gen := s.gen + 1, hist := fun g => if g = s.gen + 1 then f else s.hist g }

theorem Server.commit_hist_old {K V : Type} (s : Server K V) (f : K → V) (g : Nat) (h : g ≤ s.gen) :
    (s.commit f).hist g = s.hist g := by
  have hne : ¬ g = s.gen + 1 := by omega
  simp only [Server.commit, hne, ↓reduceIte]

/-! ## The client's cache -/

structure Entry (V : Type) where
  val : V
  /-- The generation the server read the value at. -/
  gen : Nat

structure Cache (K V : Type) where
  /-- The newest generation the client has heard of. -/
  known : Nat
  entries : K → Option (Entry V)

/-- What a read gives the kernel. Only `current` may be drawn as the page; `stale` is drawn labelled
    with its age, and `miss` asks the server. -/
inductive Answer (V : Type) where
  | current (v : V)
  | stale (v : V) (gen : Nat)
  | miss

set_option linter.unusedSectionVars false

variable {K V : Type} [DecidableEq K]

def setK (f : K → Option (Entry V)) (k : K) (x : Option (Entry V)) : K → Option (Entry V) :=
  fun k' => if k' = k then x else f k'

def read (c : Cache K V) (k : K) : Answer V :=
  match c.entries k with
  | none => .miss
  | some e => if e.gen = c.known then .current e.val else .stale e.val e.gen

/-- Take in a value read at generation `g`. A reply never replaces an entry read at a later
    generation (a late reply loses: the client's guarded put); the client now knows the server
    reached `g`. -/
def put (c : Cache K V) (k : K) (v : V) (g : Nat) : Cache K V :=
  match c.entries k with
  | some e =>
    if g < e.gen then { c with known := max c.known g }
    else { known := max c.known g, entries := setK c.entries k (some ⟨v, g⟩) }
  | none => { known := max c.known g, entries := setK c.entries k (some ⟨v, g⟩) }

/-- A notice: between generations `a` and `b` only the keys `ks` may differ. Entries stamped `a`
    are carried to `b`, except the keys named, which are dropped (and read again). -/
def advance (c : Cache K V) (a b : Nat) (ks : List K) : Cache K V :=
  { known := max c.known b,
    entries := fun k =>
      match c.entries k with
      | some e => if e.gen = a then (if k ∈ ks then none else some ⟨e.val, b⟩) else some e
      | none => none }

/-! ## Operations -/

inductive Op (K V : Type) where
  /-- A commit or other change at the server, by anyone (this client included). -/
  | commit (f : K → V)
  /-- A read reply, or a commit receipt, for key `k` read at generation `g`. -/
  | reply (k : K) (g : Nat)
  /-- A notice on the socket: between `a` and `b` only `ks` may differ. -/
  | notice (a b : Nat) (ks : List K)
  /-- The connection drops, a frame is lost, the page goes offline: nothing the cache holds changes. -/
  | lose

/-- Whether a notice is one a server satisfying C2 could send. -/
def NoticeOk (s : Server K V) (a b : Nat) (ks : List K) : Prop :=
  a ≤ b ∧ b ≤ s.gen ∧ ∀ k, k ∉ ks → s.hist a k = s.hist b k

open Classical in
/-- One operation. A reply comes from the server, so it carries the server's value at its
    generation; a delivery the server could not have made is ignored. -/
noncomputable def step (sc : Server K V × Cache K V) : Op K V → Server K V × Cache K V
  | .commit f => (sc.1.commit f, sc.2)
  | .reply k g => if g ≤ sc.1.gen then (sc.1, put sc.2 k (sc.1.hist g k) g) else sc
  | .notice a b ks => if NoticeOk sc.1 a b ks then (sc.1, advance sc.2 a b ks) else sc
  | .lose => sc

noncomputable def run : Server K V × Cache K V → List (Op K V) → Server K V × Cache K V
  | sc, [] => sc
  | sc, op :: ops => run (step sc op) ops

/-! ## Coherence -/

/-- Every entry is the server's value at its own stamp, no entry claims a generation the client has
    not heard of, and the client has heard of nothing the server has not reached. -/
def Coh (s : Server K V) (c : Cache K V) : Prop :=
  c.known ≤ s.gen ∧ ∀ k e, c.entries k = some e → e.gen ≤ c.known ∧ e.val = s.hist e.gen k

theorem coh_empty (s : Server K V) : Coh s ⟨0, fun _ => none⟩ :=
  ⟨Nat.zero_le _, fun _ _ h => by cases h⟩

theorem put_coh (s : Server K V) (c : Cache K V) (k : K) (g : Nat) (hg : g ≤ s.gen) (h : Coh s c) :
    Coh s (put c k (s.hist g k) g) := by
  obtain ⟨hk, he⟩ := h
  have hmax : max c.known g ≤ s.gen := Nat.max_le.mpr ⟨hk, hg⟩
  have hnew : ∀ k' e', setK c.entries k (some ⟨s.hist g k, g⟩) k' = some e' →
      e'.gen ≤ max c.known g ∧ e'.val = s.hist e'.gen k' := by
    intro k' e' h'
    simp only [setK] at h'
    by_cases hkk : k' = k
    · simp only [hkk, ↓reduceIte, Option.some.injEq] at h'
      subst h'
      subst hkk
      exact ⟨Nat.le_max_right _ _, rfl⟩
    · simp only [hkk, ↓reduceIte] at h'
      obtain ⟨h1, h2⟩ := he k' e' h'
      exact ⟨Nat.le_trans h1 (Nat.le_max_left _ _), h2⟩
  unfold put
  cases hc : c.entries k with
  | none => exact ⟨hmax, hnew⟩
  | some e =>
    simp only
    split
    · exact ⟨hmax, fun k' e' h' => by
        obtain ⟨h1, h2⟩ := he k' e' h'
        exact ⟨Nat.le_trans h1 (Nat.le_max_left _ _), h2⟩⟩
    · exact ⟨hmax, hnew⟩

theorem advance_coh (s : Server K V) (c : Cache K V) (a b : Nat) (ks : List K)
    (hn : NoticeOk s a b ks) (h : Coh s c) : Coh s (advance c a b ks) := by
  obtain ⟨hab, hb, hsame⟩ := hn
  obtain ⟨hk, he⟩ := h
  refine ⟨Nat.max_le.mpr ⟨hk, hb⟩, fun k e h' => ?_⟩
  simp only [advance] at h'
  cases hc : c.entries k with
  | none => rw [hc] at h'; cases h'
  | some e0 =>
    rw [hc] at h'
    obtain ⟨h1, h2⟩ := he k e0 hc
    simp only at h'
    by_cases hg : e0.gen = a
    · simp only [hg, ↓reduceIte] at h'
      by_cases hm : k ∈ ks
      · simp only [hm, ↓reduceIte] at h'
        cases h'
      · simp only [hm, ↓reduceIte, Option.some.injEq] at h'
        subst h'
        refine ⟨Nat.le_max_right _ _, ?_⟩
        simp only
        rw [h2, hg]
        exact hsame k hm
    · simp only [hg, ↓reduceIte, Option.some.injEq] at h'
      subst h'
      exact ⟨Nat.le_trans h1 (Nat.le_max_left _ _), h2⟩

theorem commit_coh (s : Server K V) (c : Cache K V) (f : K → V) (h : Coh s c) :
    Coh (s.commit f) c := by
  obtain ⟨hk, he⟩ := h
  refine ⟨by simp only [Server.commit]; omega, fun k e h' => ?_⟩
  obtain ⟨h1, h2⟩ := he k e h'
  refine ⟨h1, ?_⟩
  rw [Server.commit_hist_old s f e.gen (by omega)]
  exact h2

theorem step_coh (sc : Server K V × Cache K V) (op : Op K V) (h : Coh sc.1 sc.2) :
    Coh (step sc op).1 (step sc op).2 := by
  cases op with
  | commit f => exact commit_coh _ _ f h
  | reply k g =>
    simp only [step]
    split
    · exact put_coh _ _ k g (by assumption) h
    · exact h
  | notice a b ks =>
    simp only [step]
    split
    · exact advance_coh _ _ a b ks (by assumption) h
    · exact h
  | lose => exact h

/-- **Coherence holds at every step of every schedule** of commits, replies, notices and losses,
    in any order.
    Verifies: FE-001, FE-009 -/
theorem run_coh (ops : List (Op K V)) :
    ∀ sc : Server K V × Cache K V, Coh sc.1 sc.2 → Coh (run sc ops).1 (run sc ops).2 := by
  induction ops with
  | nil => intro sc h; exact h
  | cons op ops ih => intro sc h; exact ih _ (step_coh sc op h)

/-! ## What a read returns -/

/-- **A current answer is the server's value at the last known generation.**
    Verifies: FE-001 -/
theorem read_current_eq (s : Server K V) (c : Cache K V) (k : K) (v : V) (h : Coh s c)
    (hr : read c k = .current v) : v = s.hist c.known k ∧ c.known ≤ s.gen := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hg : e.gen = c.known
    · simp only [hg, ↓reduceIte, Answer.current.injEq] at hr
      obtain ⟨_, h2⟩ := h.2 k e hc
      rw [← hr, h2, hg]
      exact ⟨rfl, h.1⟩
    · simp only [hg, ↓reduceIte] at hr
      cases hr

/-- **Never staler than the last known generation, unlabelled**: an answer served as current is
    stamped with the last known generation, and any other held entry is strictly older and is
    returned labelled with its stamp.
    Verifies: FE-009 -/
theorem read_stale_older (s : Server K V) (c : Cache K V) (k : K) (v : V) (g : Nat) (h : Coh s c)
    (hr : read c k = .stale v g) : g < c.known ∧ v = s.hist g k := by
  unfold read at hr
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hg : e.gen = c.known
    · simp only [hg, ↓reduceIte] at hr
      cases hr
    · simp only [hg, ↓reduceIte, Answer.stale.injEq] at hr
      obtain ⟨hv, hgg⟩ := hr
      obtain ⟨h1, h2⟩ := h.2 k e hc
      subst hv hgg
      exact ⟨Nat.lt_of_le_of_ne h1 hg, h2⟩

/-- A value put at a generation no older than anything the client knows is read back as current. -/
theorem put_read (s : Server K V) (c : Cache K V) (k : K) (v : V) (g : Nat) (h : Coh s c)
    (hg : c.known ≤ g) : read (put c k v g) k = .current v := by
  have hm : max c.known g = g := Nat.max_eq_right hg
  unfold put
  cases hc : c.entries k with
  | none => simp [read, setK, hm]
  | some e =>
    have he : e.gen ≤ g := Nat.le_trans (h.2 k e hc).1 hg
    have hlt : ¬ g < e.gen := by omega
    simp [hlt, read, setK, hm]

/-- **A read after a confirmed commit returns the committed value.** The commit makes generation
    `gen + 1` with `f`; its receipt is a reply for the written key at that generation.
    Verifies: FE-002 -/
theorem read_after_commit (s : Server K V) (c : Cache K V) (f : K → V) (k : K) (h : Coh s c) :
    read (run (s, c) [.commit f, .reply k (s.gen + 1)]).2 k = .current (f k) := by
  have hc : Coh (s.commit f) c := commit_coh s c f h
  have hf : (s.commit f).hist (s.gen + 1) k = f k := by simp [Server.commit]
  have hp := put_read (s.commit f) c k (f k) (s.gen + 1) hc (by have := h.1; omega)
  have hle : s.gen + 1 ≤ (s.commit f).gen := Nat.le_refl _
  simp only [run, step, hle, ↓reduceIte, hf]
  exact hp

/-- The receipt arriving after other traffic: if the client has heard of nothing newer than the
    commit's generation, the read is the committed value; whatever it has heard of, the read is
    the server's value at the newest generation it knows, or a labelled stale answer.
    Verifies: FE-002 -/
theorem receipt_read (s : Server K V) (c : Cache K V) (k : K) (g : Nat) (h : Coh s c)
    (hg : g ≤ s.gen) (hk : c.known ≤ g) :
    read (step (s, c) (.reply k g)).2 k = .current (s.hist g k) := by
  simp only [step, hg, ↓reduceIte]
  exact put_read s c k _ g h hk

/-- A notice that does not name a key keeps a current answer for it current. -/
theorem notice_keeps_current (s : Server K V) (c : Cache K V) (k : K) (v : V) (b : Nat)
    (ks : List K) (hn : NoticeOk s c.known b ks) (hk : k ∉ ks) (hr : read c k = .current v) :
    read (step (s, c) (.notice c.known b ks)).2 k = .current v := by
  simp only [step, hn, ↓reduceIte]
  have hab : max c.known b = b := Nat.max_eq_right hn.1
  unfold read at hr ⊢
  cases hc : c.entries k with
  | none => rw [hc] at hr; cases hr
  | some e =>
    rw [hc] at hr
    simp only at hr
    by_cases hg : e.gen = c.known
    · simp only [hg, ↓reduceIte, Answer.current.injEq] at hr
      subst hr
      simp [advance, hc, hg, hk, hab]
    · simp only [hg, ↓reduceIte] at hr
      cases hr

/-! ## The working set's keys

The model instantiated with the working set's keys and payloads: a server whose generation `g`
holds the world `worlds g`, so each key's value is the working set's `fetch`. A current answer is
then what the store would load at the last known generation. -/

/-- A server whose generations are the worlds `worlds 0, worlds 1, ...`, up to `gen`. -/
def ofWorlds (worlds : Nat → World) (gen : Nat) : Server WorkingSet.Key WorkingSet.Payload :=
  { gen := gen, hist := fun g k => WorkingSet.fetch (worlds g) k }

theorem read_current_fetch (worlds : Nat → World) (gen : Nat)
    (c : Cache WorkingSet.Key WorkingSet.Payload) (k : WorkingSet.Key) (v : WorkingSet.Payload)
    (h : Coh (ofWorlds worlds gen) c) (hr : read c k = .current v) :
    v = WorkingSet.fetch (worlds c.known) k :=
  (read_current_eq _ c k v h hr).1

/-! ## Erasure

The browser receives opened values: the gateway opens sealed fields before it answers. So the
browser cache is the working set's rejected placement, above sealing, and
`WorkingSet.above_sealing_serves_erased` applies to it unchanged: a key destruction raises no
generation, so an entry opened before it stays current by its generation and goes on being served.
The fix is a second number in the stamp: an erasure epoch the server raises at every key
destruction of the tenant, sent with every reply and notice. -/

open WorkingSet in
/-- **The browser cache stamped by generation alone serves erased values.** This is the working
    set's counterexample; the browser cache is `runAbove`. -/
theorem browser_generation_only_serves_erased (s : SealedStore) (k : Key)
    (hlive : s.keys (s.vals k).key = true) (hplain : (s.vals k).plain ≠ .record none) :
    runAbove [] s [.admitEntry k, .destroy (s.vals k).key, .read k] ≠
      runTruth s [.admitEntry k, .destroy (s.vals k).key, .read k] :=
  above_sealing_serves_erased s k hlive hplain

open WorkingSet

/-- The sealed store with an erasure epoch, raised by every key destruction. -/
structure EStore where
  base : SealedStore
  ep : Nat

/-- An opened entry with its stamp: key, generation, erasure epoch, opened value. -/
abbrev EEntry := Key × Nat × Nat × Payload

/-- The entry for `k` stamped with generation `g` and epoch `ep`, if held. -/
def eHit (c : List EEntry) (g ep : Nat) (k : Key) : Option Payload :=
  (c.find? (fun e => e.1 == k && e.2.1 == g && e.2.2.1 == ep)).map (fun e => e.2.2.2)

/-- The browser's placement, above sealing, with the epoch in the stamp. -/
def runEpoch : List EEntry → EStore → List SOp → List Payload
  | _, _, [] => []
  | c, s, .admitEntry k :: ops =>
    runEpoch ((k, s.base.gen, s.ep, openWith s.base.keys (s.base.vals k)) :: c) s ops
  | c, s, .write k v :: ops => runEpoch c { s with base := s.base.write k v } ops
  | c, s, .destroy n :: ops => runEpoch c { base := destroyKey s.base n, ep := s.ep + 1 } ops
  | c, s, .read k :: ops =>
    (eHit c s.base.gen s.ep k).getD (openWith s.base.keys (s.base.vals k)) :: runEpoch c s ops

def EOk (c : List EEntry) (s : EStore) : Prop :=
  ∀ e ∈ c, e.2.1 ≤ s.base.gen ∧ e.2.2.1 ≤ s.ep ∧
    (e.2.1 = s.base.gen → e.2.2.1 = s.ep → e.2.2.2 = openWith s.base.keys (s.base.vals e.1))

theorem eHit_spec (c : List EEntry) (g ep : Nat) (k : Key) (v : Payload)
    (h : eHit c g ep k = some v) : ∃ e ∈ c, e.1 = k ∧ e.2.1 = g ∧ e.2.2.1 = ep ∧ e.2.2.2 = v := by
  unfold eHit at h
  cases hf : c.find? (fun e => e.1 == k && e.2.1 == g && e.2.2.1 == ep) with
  | none => rw [hf] at h; cases h
  | some e =>
    rw [hf] at h
    simp only [Option.map_some, Option.some.injEq] at h
    have hp := List.find?_some hf
    simp only [Bool.and_eq_true, beq_iff_eq] at hp
    exact ⟨e, List.mem_of_find?_eq_some hf, hp.1.1, hp.1.2, hp.2, h⟩

/-- **With the erasure epoch in the stamp, the browser cache serves no erased value**: for every
    schedule of admissions, value writes, key destructions and reads, every read is the stored
    value opened with the keys live at that read.
    Verifies: FE-003 -/
theorem epoch_stamp_erasure_safe (ops : List SOp) :
    ∀ (c : List EEntry) (s : EStore), EOk c s → runEpoch c s ops = runTruth s.base ops := by
  induction ops with
  | nil => intro _ _ _; rfl
  | cons op ops ih =>
    intro c s hc
    cases op with
    | admitEntry k =>
      apply ih
      intro e he
      rcases List.mem_cons.mp he with h | h
      · subst h; exact ⟨Nat.le_refl _, Nat.le_refl _, fun _ _ => rfl⟩
      · exact hc e h
    | write k v =>
      apply ih
      intro e he
      obtain ⟨hle, hep, _⟩ := hc e he
      refine ⟨by simp [SealedStore.write]; omega, hep, fun h _ => ?_⟩
      simp [SealedStore.write] at h; omega
    | destroy n =>
      apply ih
      intro e he
      obtain ⟨hle, hep, _⟩ := hc e he
      refine ⟨by simpa [destroyKey] using hle, by simp only; omega, fun _ h => ?_⟩
      simp only at h; omega
    | read k =>
      simp only [runEpoch, runTruth]
      congr 1
      · cases hh : eHit c s.base.gen s.ep k with
        | none => rfl
        | some v =>
          obtain ⟨e, he, hk, hg, hep, hv⟩ := eHit_spec c s.base.gen s.ep k v hh
          have := (hc e he).2.2 hg hep
          rw [hv, hk] at this
          simp [this]
      · exact ih c s hc

/-- Purging at a new epoch: the entries kept are only those of the epoch now known, so no value
    opened before a key destruction stays at rest. -/
def purge (c : List EEntry) (ep : Nat) : List EEntry := c.filter (fun e => e.2.2.1 == ep)

theorem purge_leaves_current (c : List EEntry) (ep : Nat) : ∀ e ∈ purge c ep, e.2.2.1 = ep := by
  intro e he
  simpa [purge] using (List.mem_filter.mp he).2

end Strata.Client.Cache
