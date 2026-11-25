/-
Leases, with a logical clock, for one tenant served by several processes.

Each process has a working set (stamped entries, as in `Strata.WorkingSet`), the generation it
last learned, and the lease it believes it holds (a lease number and a term end). The control
directory records the holder, the lease number and the term end. The tenant's storage records the
generation and the fence token: the lease number of the last acquisition.

- `acquire` succeeds only when the directory has no lease or its term ended at least the margin
  ago; it issues the next lease number, writes it as the fence token and rotates the generation.
- `renew` extends the holder's term while it is still valid and makes a generation read.
- `revoke` clears the directory record and the holder's lease (an acknowledged revocation; a
  revocation by timeout is the term running out).
- `write` is a counter-raising write: it presents the writer's lease number and a generation, and
  the store accepts it only if both match.
- `raw` is a write that bypasses the fence (raw access to the database). The guarantee is stated
  for schedules without it, and `raw_breaks_leased_read` shows why.
- `read` is served from a resident entry only while the process's own term has more than the
  margin left; otherwise it reads the store (a checked read).

Theorems: `write_refused_unless_holder` (a process the directory does not name as holder cannot
write, whatever generation it presents), `acquire_refused_while_serving` (a second would-be holder
cannot take the lease while the holder can serve), `cache_read_before_term`, `stepL_inv`,
`leased_reads_correct` (every read of every fenced schedule returns the store's value: the
`covered` condition of the main model, derived from fencing), `raw_breaks_leased_read`.
-/

import Strata.WorkingSet

namespace Strata.WorkingSet.Leases

open Strata Strata.WorkingSet

abbrev ProcId := Nat

structure Proc where
  cache : List Entry
  known : Nat
  /-- The lease this process believes it holds: lease number and term end. -/
  lease : Option (Nat × Nat)

structure LState where
  x : TenantId
  world : World
  gen : Nat
  /-- The fence token in the tenant's storage: the lease number of the last acquisition. -/
  token : Nat
  /-- The directory's record: holder, lease number, term end. -/
  dir : Option (ProcId × Nat × Nat)
  now : Nat
  /-- The next lease number the directory issues. -/
  next : Nat
  procs : ProcId → Proc

inductive LOp where
  | tick
  | acquire (p : ProcId) (term : Nat)
  | renew (p : ProcId) (term : Nat)
  | revoke
  | write (p : ProcId) (g : Nat) (t : Option Tenant)
  | raw (t : Option Tenant)
  | admitEntry (p : ProcId) (k : Key)
  | read (p : ProcId) (k : Key)

def LOp.fenced : LOp → Bool
  | .raw _ => false
  | _ => true

inductive LOut where
  | value (v : Payload) (fromCache : Bool)
  | accepted
  | refused
  | done

-- The drift margin.
variable (m : Nat)

def setP (s : LState) (p : ProcId) (q : Proc) : LState :=
  { s with procs := fun r => if r = p then q else s.procs r }

/-- A process serves from resident entries only while its own term has more than the margin left. -/
def serving (s : LState) (p : ProcId) : Bool :=
  match (s.procs p).lease with
  | some (_, e) => decide (s.now + m < e)
  | none => false

/-- The directory's lease is free to take: none, or its term ended at least the margin ago. -/
def free (s : LState) : Bool :=
  match s.dir with
  | none => true
  | some (_, _, e) => decide (e + m ≤ s.now)

/-- The process's entries, kept if they are of the current generation and dropped if not. -/
def kept (s : LState) (p : ProcId) : List Entry :=
  if (s.procs p).known = s.gen then (s.procs p).cache else []

def stepL (s : LState) : LOp → LState × LOut
  | .tick => ({ s with now := s.now + 1 }, .done)
  | .acquire p term =>
    if free m s then
      (setP { s with dir := some (p, s.next, s.now + term), token := s.next, gen := s.gen + 1,
                     next := s.next + 1 } p
        { cache := [], known := s.gen + 1, lease := some (s.next, s.now + term) }, .done)
    else (s, .done)
  | .renew p term =>
    match s.dir, (s.procs p).lease with
    | some (h, n, e), some (n', _) =>
      if h = p ∧ n = n' ∧ s.now + m < e then
        (setP { s with dir := some (p, n, s.now + term) } p
          { cache := kept s p, known := s.gen, lease := some (n, s.now + term) }, .done)
      else (s, .done)
    | _, _ => (s, .done)
  | .revoke =>
    match s.dir with
    | some (h, _, _) => (setP { s with dir := none } h { s.procs h with lease := none }, .done)
    | none => (s, .done)
  | .write p g t =>
    match (s.procs p).lease with
    | some (n, _) =>
      if n = s.token ∧ g = s.gen then
        (setP { s with world := upd s.world s.x t, gen := s.gen + 1 } p
          { s.procs p with cache := [], known := s.gen + 1 }, .accepted)
      else (s, .refused)
    | none => (s, .refused)
  | .raw t => ({ s with world := upd s.world s.x t, gen := s.gen + 1 }, .done)
  | .admitEntry p k =>
    (setP s p { s.procs p with cache := ⟨k, s.gen, fetch s.world k⟩ :: kept s p, known := s.gen },
      .done)
  | .read p k =>
    match (if serving m s p then hit (s.procs p).cache (fun _ => (s.procs p).known) k else none) with
    | some e => (s, .value e.val true)
    | none => (setP s p { s.procs p with cache := kept s p, known := s.gen },
        .value (fetch s.world k) false)

/-! ### Invariants -/

def SoundP (s : LState) : Prop :=
  ∀ p, ∀ e ∈ (s.procs p).cache, e.stamp ≤ s.gen ∧ (e.stamp = s.gen → e.val = fetch s.world e.key)

def DirTok (s : LState) : Prop := ∀ h n e, s.dir = some (h, n, e) → n = s.token

/-- Only the directory's holder holds the current lease number, and its own term is no later than
    the directory's. -/
def TokOk (s : LState) : Prop :=
  ∀ p n e, (s.procs p).lease = some (n, e) → n = s.token → ∃ e', s.dir = some (p, n, e') ∧ e ≤ e'

def Below (s : LState) : Prop :=
  s.token < s.next ∧ ∀ p n e, (s.procs p).lease = some (n, e) → n < s.next

/-- A serving process holds the current lease number and knows the current generation. -/
def ServeOk (s : LState) : Prop :=
  ∀ p, serving m s p = true → (∃ e, (s.procs p).lease = some (s.token, e)) ∧ (s.procs p).known = s.gen

def LInv (s : LState) : Prop := SoundP s ∧ DirTok s ∧ TokOk s ∧ Below s ∧ ServeOk m s

@[simp] theorem setP_same (s : LState) (p : ProcId) (q : Proc) : (setP s p q).procs p = q := by
  simp [setP]

@[simp] theorem setP_other (s : LState) (p r : ProcId) (q : Proc) (h : r ≠ p) :
    (setP s p q).procs r = s.procs r := by
  simp [setP, h]

theorem serving_setP_ne (s : LState) (p : ProcId) (q : Proc) (r : ProcId) (h : r ≠ p) :
    serving m (setP s p q) r = serving m s r := by
  unfold serving
  rw [setP_other s p r q h]
  rfl

theorem serving_setP_ck (s : LState) (p : ProcId) (c : List Entry) (k : Nat) :
    serving m (setP s p ⟨c, k, (s.procs p).lease⟩) p = serving m s p := by
  unfold serving
  rw [setP_same]
  rfl

theorem kept_sound (s : LState) (p : ProcId) (hs : SoundP s) :
    ∀ e ∈ kept s p, e.stamp ≤ s.gen ∧ (e.stamp = s.gen → e.val = fetch s.world e.key) := by
  intro e he
  unfold kept at he
  split at he
  · exact hs p e he
  · simp at he

theorem serving_of (s : LState) (p : ProcId) (n e : Nat) (h : (s.procs p).lease = some (n, e)) :
    serving m s p = decide (s.now + m < e) := by
  simp [serving, h]

/-- A serving process cannot coexist with a free directory lease. -/
theorem not_free_of_serving (s : LState) (p : ProcId) (hinv : LInv m s)
    (hsv : serving m s p = true) : free m s = false := by
  obtain ⟨⟨e, hl⟩, _⟩ := hinv.2.2.2.2 p hsv
  obtain ⟨e', hd, hle⟩ := hinv.2.2.1 p _ e hl rfl
  have hn : s.now + m < e := by simpa [serving_of m s p _ e hl] using hsv
  simp only [free, hd, decide_eq_false_iff_not]
  omega

/-- **A process the directory does not name as holder cannot write**, whatever generation it
    presents, the current one included. -/
theorem write_refused_unless_holder (s : LState) (p : ProcId) (hinv : LInv m s)
    (hnh : ∀ n e, s.dir ≠ some (p, n, e)) (g : Nat) (t : Option Tenant) :
    stepL m s (.write p g t) = (s, .refused) := by
  simp only [stepL]
  cases hl : (s.procs p).lease with
  | none => rfl
  | some ne =>
    obtain ⟨n, e⟩ := ne
    simp only []
    rw [ite_eq_right]
    rintro ⟨hn, _⟩
    obtain ⟨e', hd, _⟩ := hinv.2.2.1 p n e hl hn
    exact hnh n e' hd

/-- **A second would-be holder cannot take the lease** while the holder can still serve from its
    working set: the acquisition changes nothing. -/
theorem acquire_refused_while_serving (s : LState) (p q : ProcId) (term : Nat) (hinv : LInv m s)
    (hsv : serving m s p = true) : stepL m s (.acquire q term) = (s, .done) := by
  simp only [stepL, not_free_of_serving m s p hinv hsv, Bool.false_eq_true, ite_false]

/-- **A holder serves from resident entries only before its term end** (less the margin). -/
theorem cache_read_before_term (s : LState) (p : ProcId) (k : Key) (v : Payload)
    (h : (stepL m s (.read p k)).2 = .value v true) :
    ∃ n e, (s.procs p).lease = some (n, e) ∧ s.now + m < e := by
  simp only [stepL] at h
  split at h
  · next e he =>
    split at he
    · next hsv =>
      cases hl : (s.procs p).lease with
      | none => simp [serving, hl] at hsv
      | some ne =>
        obtain ⟨n, e'⟩ := ne
        exact ⟨n, e', rfl, by simpa [serving, hl] using hsv⟩
    · cases he
  · simp at h

/-- **Every read returns the store's value**, given the invariant. -/
theorem read_correct (s : LState) (p : ProcId) (k : Key) (hinv : LInv m s) (v : Payload)
    (b : Bool) (h : (stepL m s (.read p k)).2 = .value v b) : v = fetch s.world k := by
  simp only [stepL] at h
  split at h
  · next e he =>
    split at he
    · next hsv =>
      obtain ⟨hm, hk, hst⟩ := hit_stamp _ _ k e he
      have hkn := (hinv.2.2.2.2 p hsv).2
      simp only [LOut.value.injEq] at h
      rw [← h.1, ← hk]
      exact (hinv.1 p e hm).2 (hst.trans hkn)
    · cases he
  · simp only [LOut.value.injEq] at h
    exact h.1.symm

/-! ### The invariant is preserved by every fenced operation -/

theorem stepL_inv (s : LState) (op : LOp) (hf : op.fenced = true) (hinv : LInv m s) :
    LInv m (stepL m s op).1 := by
  obtain ⟨hs, hd, ht, hb, hv⟩ := hinv
  cases op with
  | tick =>
    refine ⟨hs, hd, ht, hb, fun p hsv => hv p ?_⟩
    have h1 : serving m { s with now := s.now + 1 } p = true := hsv
    unfold serving at h1 ⊢
    cases hl : (s.procs p).lease with
    | none => simp [hl] at h1
    | some ne =>
      obtain ⟨n, e⟩ := ne
      simp only [hl, decide_eq_true_eq] at h1 ⊢
      omega
  | acquire p term =>
    simp only [stepL]
    split
    · next hfree =>
      refine ⟨?_, ?_, ?_, ?_, ?_⟩
      · intro q e he
        by_cases hq : q = p
        · subst hq; simp at he
        · rw [setP_other _ _ _ _ hq] at he
          obtain ⟨hle, _⟩ := hs q e he
          exact ⟨by simp [setP]; omega, fun h => by simp [setP] at h; omega⟩
      · intro h n e hdir
        simp [setP] at hdir
        exact hdir.2.1.symm
      · intro q n e hl hn
        by_cases hq : q = p
        · subst hq
          simp at hl
          obtain ⟨rfl, rfl⟩ := hl
          exact ⟨_, rfl, Nat.le_refl _⟩
        · rw [setP_other _ _ _ _ hq] at hl
          have := hb.2 q n e hl
          simp [setP] at hn
          omega
      · refine ⟨by simp [setP], fun q n e hl => ?_⟩
        by_cases hq : q = p
        · subst hq; simp at hl; obtain ⟨rfl, _⟩ := hl; simp [setP]
        · rw [setP_other _ _ _ _ hq] at hl
          have := hb.2 q n e hl
          simp [setP]; omega
      · intro q hsv
        by_cases hq : q = p
        · subst hq; simp [setP]
        · exfalso
          have hsv0 : serving m s q = true := by
            cases hl : (s.procs q).lease with
            | none => simp [serving, setP, hq, hl] at hsv
            | some ne => obtain ⟨n, e⟩ := ne; simpa [serving, setP, hq, hl] using hsv
          have := not_free_of_serving m s q ⟨hs, hd, ht, hb, hv⟩ hsv0
          rw [this] at hfree; cases hfree
    · exact ⟨hs, hd, ht, hb, hv⟩
  | renew p term =>
    simp only [stepL]
    split
    · next h n e n' e0 hdir hl =>
      split
      · next hc =>
        obtain ⟨rfl, rfl, _⟩ := hc
        have hn : n = s.token := hd h n e hdir
        refine ⟨?_, ?_, ?_, ?_, ?_⟩
        · intro q e1 he1
          by_cases hq : q = h
          · subst hq; simp only [setP_same] at he1; exact kept_sound s q hs e1 he1
          · rw [setP_other _ _ _ _ hq] at he1; exact hs q e1 he1
        · intro h' n2 e2 hd2
          simp [setP] at hd2
          rw [← hd2.2.1]; exact hn
        · intro q n2 e2 hl2 hn2
          by_cases hq : q = h
          · subst hq; simp at hl2; obtain ⟨rfl, rfl⟩ := hl2; exact ⟨_, by simp [setP], Nat.le_refl _⟩
          · rw [setP_other _ _ _ _ hq] at hl2
            obtain ⟨e', hd', _⟩ := ht q n2 e2 hl2 hn2
            rw [hdir] at hd'
            simp at hd'
            exact absurd hd'.1.symm hq
        · refine ⟨hb.1, fun q n2 e2 hl2 => ?_⟩
          by_cases hq : q = h
          · subst hq; simp at hl2; obtain ⟨rfl, _⟩ := hl2; exact hb.2 q n e0 hl
          · rw [setP_other _ _ _ _ hq] at hl2; exact hb.2 q n2 e2 hl2
        · intro q hsv
          by_cases hq : q = h
          · subst hq; simp [setP, hn]
          · have hsv0 : serving m s q = true := by
              rw [serving_setP_ne m _ _ _ _ hq] at hsv; exact hsv
            obtain ⟨⟨e2, hl2⟩, hk⟩ := hv q hsv0
            exact ⟨⟨e2, by simp [setP, hq, hl2]⟩, by simp [setP, hq, hk]⟩
      · exact ⟨hs, hd, ht, hb, hv⟩
    · exact ⟨hs, hd, ht, hb, hv⟩
  | revoke =>
    simp only [stepL]
    split
    · next h n e hdir =>
      refine ⟨?_, fun _ _ _ hd2 => by simp [setP] at hd2, ?_, ?_, ?_⟩
      · intro q e1 he1
        by_cases hq : q = h
        · subst hq; simp only [setP_same] at he1; exact hs q e1 he1
        · rw [setP_other _ _ _ _ hq] at he1; exact hs q e1 he1
      · intro q n2 e2 hl2 hn2
        by_cases hq : q = h
        · subst hq; simp at hl2
        · rw [setP_other _ _ _ _ hq] at hl2
          obtain ⟨e', hd', _⟩ := ht q n2 e2 hl2 hn2
          rw [hdir] at hd'
          simp at hd'
          exact absurd hd'.1.symm hq
      · refine ⟨hb.1, fun q n2 e2 hl2 => ?_⟩
        by_cases hq : q = h
        · subst hq; simp at hl2
        · rw [setP_other _ _ _ _ hq] at hl2; exact hb.2 q n2 e2 hl2
      · intro q hsv
        by_cases hq : q = h
        · subst hq; simp [serving, setP] at hsv
        · have hsv0 : serving m s q = true := by
            rw [serving_setP_ne m _ _ _ _ hq] at hsv; exact hsv
          obtain ⟨⟨e2, hl2⟩, hk⟩ := hv q hsv0
          exact ⟨⟨e2, by simp [setP, hq, hl2]⟩, by simp [setP, hq, hk]⟩
    · exact ⟨hs, hd, ht, hb, hv⟩
  | write p g t =>
    simp only [stepL]
    split
    · next n e0 hl =>
      split
      · next hc =>
        obtain ⟨hn, _⟩ := hc
        refine ⟨?_, hd, ?_, ?_, ?_⟩
        · intro q e1 he1
          by_cases hq : q = p
          · subst hq; simp at he1
          · rw [setP_other _ _ _ _ hq] at he1
            obtain ⟨hle, _⟩ := hs q e1 he1
            exact ⟨by simp [setP]; omega, fun h => by simp [setP] at h; omega⟩
        · intro q n2 e2 hl2 hn2
          by_cases hq : q = p
          · subst hq; simp at hl2; exact ht q n2 e2 hl2 hn2
          · rw [setP_other _ _ _ _ hq] at hl2; exact ht q n2 e2 hl2 hn2
        · refine ⟨hb.1, fun q n2 e2 hl2 => ?_⟩
          by_cases hq : q = p
          · subst hq; simp at hl2; exact hb.2 q n2 e2 hl2
          · rw [setP_other _ _ _ _ hq] at hl2; exact hb.2 q n2 e2 hl2
        · intro q hsv
          by_cases hq : q = p
          · subst hq
            refine ⟨⟨e0, by simp [setP, hl, hn]⟩, by simp [setP]⟩
          · exfalso
            have hsv0 : serving m s q = true := by
              rw [serving_setP_ne m _ _ _ _ hq] at hsv; exact hsv
            obtain ⟨⟨e2, hl2⟩, _⟩ := hv q hsv0
            obtain ⟨_, hdq, _⟩ := ht q _ e2 hl2 rfl
            obtain ⟨_, hdp, _⟩ := ht p n e0 hl hn
            rw [hdq] at hdp
            simp at hdp
            exact hq hdp.1
      · exact ⟨hs, hd, ht, hb, hv⟩
    · exact ⟨hs, hd, ht, hb, hv⟩
  | raw t => simp [LOp.fenced] at hf
  | admitEntry p k =>
    simp only [stepL]
    refine ⟨?_, hd, ?_, ?_, ?_⟩
    · intro q e1 he1
      by_cases hq : q = p
      · subst hq
        simp only [setP_same, List.mem_cons] at he1
        rcases he1 with h1 | h1
        · subst h1; exact ⟨Nat.le_refl _, fun _ => rfl⟩
        · exact kept_sound s q hs e1 h1
      · rw [setP_other _ _ _ _ hq] at he1; exact hs q e1 he1
    · intro q n2 e2 hl2 hn2
      by_cases hq : q = p
      · subst hq; simp at hl2; exact ht q n2 e2 hl2 hn2
      · rw [setP_other _ _ _ _ hq] at hl2; exact ht q n2 e2 hl2 hn2
    · refine ⟨hb.1, fun q n2 e2 hl2 => ?_⟩
      by_cases hq : q = p
      · subst hq; simp at hl2; exact hb.2 q n2 e2 hl2
      · rw [setP_other _ _ _ _ hq] at hl2; exact hb.2 q n2 e2 hl2
    · intro q hsv
      by_cases hq : q = p
      · subst hq
        have hsv0 : serving m s q = true := by
          rw [serving_setP_ck] at hsv; exact hsv
        obtain ⟨⟨e2, hl2⟩, _⟩ := hv q hsv0
        exact ⟨⟨e2, by simp [setP, hl2]⟩, by simp [setP]⟩
      · have hsv0 : serving m s q = true := by
          rw [serving_setP_ne m _ _ _ _ hq] at hsv; exact hsv
        obtain ⟨⟨e2, hl2⟩, hk⟩ := hv q hsv0
        exact ⟨⟨e2, by simp [setP, hq, hl2]⟩, by simp [setP, hq, hk]⟩
  | read p k =>
    simp only [stepL]
    split
    · exact ⟨hs, hd, ht, hb, hv⟩
    · refine ⟨?_, hd, ?_, ?_, ?_⟩
      · intro q e1 he1
        by_cases hq : q = p
        · subst hq; simp only [setP_same] at he1; exact kept_sound s q hs e1 he1
        · rw [setP_other _ _ _ _ hq] at he1; exact hs q e1 he1
      · intro q n2 e2 hl2 hn2
        by_cases hq : q = p
        · subst hq; simp at hl2; exact ht q n2 e2 hl2 hn2
        · rw [setP_other _ _ _ _ hq] at hl2; exact ht q n2 e2 hl2 hn2
      · refine ⟨hb.1, fun q n2 e2 hl2 => ?_⟩
        by_cases hq : q = p
        · subst hq; simp at hl2; exact hb.2 q n2 e2 hl2
        · rw [setP_other _ _ _ _ hq] at hl2; exact hb.2 q n2 e2 hl2
      · intro q hsv
        by_cases hq : q = p
        · subst hq
          have hsv0 : serving m s q = true := by
            rw [serving_setP_ck] at hsv; exact hsv
          obtain ⟨⟨e2, hl2⟩, _⟩ := hv q hsv0
          exact ⟨⟨e2, by simp [setP, hl2]⟩, by simp [setP]⟩
        · have hsv0 : serving m s q = true := by
            rw [serving_setP_ne m _ _ _ _ hq] at hsv; exact hsv
          obtain ⟨⟨e2, hl2⟩, hk⟩ := hv q hsv0
          exact ⟨⟨e2, by simp [setP, hq, hl2]⟩, by simp [setP, hq, hk]⟩

/-! ### Schedules -/

/-- Every read of the schedule returns the store's value at that point. -/
def ReadsTrueL : LState → List LOp → Prop
  | _, [] => True
  | s, op :: ops =>
    (∀ p k v b, op = .read p k → (stepL m s op).2 = .value v b → v = fetch s.world k) ∧
    ReadsTrueL (stepL m s op).1 ops

/-- **Leased reads are fetch-correct whenever every counter-raising write is fenced**: in every
    schedule without a write that bypasses the fence, every read, from resident entries or not,
    returns the store's value. The main model's `covered` condition is this, derived. -/
theorem leased_reads_correct (ops : List LOp) (hf : ∀ op ∈ ops, op.fenced = true) :
    ∀ s, LInv m s → ReadsTrueL m s ops := by
  induction ops with
  | nil => intro _ _; trivial
  | cons op ops ih =>
    intro s hinv
    refine ⟨fun p k v b hop hr => ?_, ih (fun o ho => hf o (List.mem_cons_of_mem _ ho)) _
      (stepL_inv m s op (hf op List.mem_cons_self) hinv)⟩
    subst hop
    exact read_correct m s p k hinv v b hr

/-- A state with no lease and empty working sets satisfies the invariant. -/
theorem linv_init (x : TenantId) (w : World) (g n : Nat) (now : Nat) :
    LInv m { x := x, world := w, gen := g, token := n, dir := none, now := now, next := n + 1,
             procs := fun _ => { cache := [], known := g, lease := none } } := by
  refine ⟨fun _ e he => by simp at he, fun _ _ _ h => by simp at h, fun _ _ _ h => by simp at h,
    ⟨Nat.lt_succ_self _, fun _ _ _ h => by simp at h⟩, fun _ h => by simp [serving] at h⟩

def cexHeader : Header := { types := [], members := [(0, [ownerRole])], grants := [], policies := [] }

def cexTenantL : Tenant :=
  { types := [], members := [(0, [ownerRole])], grants := [], policies := [], nextId := 1 }

def cexProc : Proc :=
  { cache := [{ key := .header 1, stamp := 1, val := .header (some cexHeader) }],
    known := 1,
    lease := some (0, 100) }

/-- A holder of tenant 1's lease, with its header resident and current. -/
def cexL : LState :=
  { x := 1,
    world := fun y => if y = 1 then some cexTenantL else none,
    gen := 1,
    token := 0,
    dir := some (0, 0, 100),
    now := 0,
    next := 1,
    procs := fun q => if q = 0 then cexProc else { cache := [], known := 1, lease := none } }

/-- **Why the fence matters.** A write that bypasses it, followed by a read while the holder's term
    is valid, is served from the resident entry and returns the value from before the write. -/
theorem raw_breaks_leased_read :
    LInv 0 cexL ∧
    (stepL 0 (stepL 0 cexL (.raw none)).1 (.read 0 (.header 1))).2 ≠
      .value (fetch (upd cexL.world 1 none) (.header 1)) false ∧
    (stepL 0 (stepL 0 cexL (.raw none)).1 (.read 0 (.header 1))).2 =
      .value (.header (some cexHeader)) true := by
  refine ⟨⟨?_, ?_, ?_, ?_, ?_⟩, ?_, ?_⟩
  · intro p e he
    by_cases hp : p = 0
    · subst hp
      simp only [cexL, cexProc, ite_true, List.mem_singleton] at he
      subst he
      refine ⟨Nat.le_refl _, fun _ => ?_⟩
      simp [fetch, Tenant.header, cexL, cexTenantL, cexHeader]
    · simp [cexL, hp] at he
  · intro h n e hd; simp only [cexL, Option.some.injEq, Prod.mk.injEq] at hd ⊢; omega
  · intro p n e hl hn
    by_cases hp : p = 0
    · subst hp
      simp only [cexL, cexProc, ite_true, Option.some.injEq, Prod.mk.injEq] at hl hn ⊢
      exact ⟨100, by simp; omega⟩
    · simp [cexL, hp] at hl
  · refine ⟨by simp [cexL], fun p n e hl => ?_⟩
    by_cases hp : p = 0
    · subst hp
      simp only [cexL, cexProc, ite_true, Option.some.injEq, Prod.mk.injEq] at hl ⊢
      omega
    · simp [cexL, hp] at hl
  · intro p hsv
    by_cases hp : p = 0
    · subst hp; simp [cexL, cexProc]
    · simp [serving, cexL, hp] at hsv
  · simp [stepL, serving, cexL, cexProc, hit, fetch, upd, cexHeader]
  · simp [stepL, serving, cexL, cexProc, hit]

end Strata.WorkingSet.Leases
