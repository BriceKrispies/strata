/-
What the browser keeps at rest, under whose key, and when it stops being readable
(docs/frontend.md, "Privacy at rest").

Everything a principal's partition holds at rest (view results, the write queue, drafts) is
encrypted under a key derived from a **key share** the identity service holds for one enrolled
device of that principal. The server issues the device id at enrolment and gives a device's share
only to a session of the principal it was enrolled for. The browser keeps the device id in the
partition and the derived key in the worker's memory (and in IndexedDB only when the person keeps
their work available offline).

What this protects against, and what it does not:

- **the next user of the device**, and **someone holding the disk after the key is gone**: whoever
  uses the browser later can only use the keys and device ids still in it (`Opens`), and the model
  lets any operation open any ciphertext with any key the browser holds;
- **not code running in the origin while the person is signed in** (cross-site scripting, a
  malicious extension): it can read the share as it arrives, or the plaintext, and copy them out.
  The model has no exfiltration and claims nothing about it.

Proved:

- **one principal's data never opens for another** (`no_cross_principal`, `served_own`);
- **sign-out, online or offline, leaves nothing readable, ever again** (`signout_erases`,
  `offline_signout_erases`): online sign-out deletes the share at the server; offline sign-out
  deletes the browser's key and the device id, so the share, still at the server, can never be
  fetched for this browser again. Neither depends on deleting the ciphertexts, nor on a marker
  delivered later;
- **expiry locks, and only the same principal unlocks** (`expire_locks`, `unlock_needs_own_session`,
  `expire_then_same_principal_unlocks`): the device id and share are kept, so work queued offline
  survives re-authentication;
- **revocation reaches the device at its next reload** (`revoke_device_erases`,
  `revoke_all_erases`), unless the person kept an offline key, which survives a reload
  (`offline_key_survives_reload`): for a kept key no bound is enforced by the model, and the
  design states none.

Assumptions, all of them:

P1. Encryption is ideal: a ciphertext opens only with the key it was made with.
P2. The server gives a device's share only to a session of the principal it was enrolled for,
    draws share and device ids fresh, and never brings a deleted share back (a restore of the
    identity database re-applies the deletions recorded after its point).
P3. Deleting a key or a device id from the browser removes it; disk forensics of deleted
    IndexedDB records, and code reading the browser's state while the person is signed in, are
    outside the model.

No `sorry`, no added axioms.
-/

import Strata.Model

namespace Strata.Client.Privacy

open Strata

/-- A ciphertext at rest. `owner` is ghost bookkeeping for the statements; opening reads only
    `key` (P1). -/
structure Sealed (α : Type) where
  owner : PrincipalId
  key : Nat
  plain : α

structure St (α : Type) where
  /-- Server: the share of each enrolled device id, with the principal it was enrolled for. -/
  shares : Nat → Option (PrincipalId × Nat)
  /-- Server: the next fresh share id and device id. -/
  next : Nat
  nextDev : Nat
  /-- Device: who the session cookie belongs to. -/
  session : Option PrincipalId
  /-- Device: the device id each partition holds. -/
  dev : PrincipalId → Option Nat
  /-- Device: the keys the browser holds (worker memory, and IndexedDB when offline is kept). -/
  keys : PrincipalId → Option Nat
  offline : PrincipalId → Bool
  disk : List (Sealed α)
  memory : List (PrincipalId × α)

def upd {β : Type} (f : PrincipalId → β) (p : PrincipalId) (b : β) : PrincipalId → β :=
  fun q => if q = p then b else f q

@[simp] theorem upd_same {β : Type} (f : PrincipalId → β) (p : PrincipalId) (b : β) :
    upd f p b p = b := by simp [upd]

theorem upd_other {β : Type} (f : PrincipalId → β) (p q : PrincipalId) (b : β) (h : q ≠ p) :
    upd f p b q = f q := by simp [upd, h]

def setD (f : Nat → Option (PrincipalId × Nat)) (d : Nat) (v : Option (PrincipalId × Nat)) :
    Nat → Option (PrincipalId × Nat) := fun d' => if d' = d then v else f d'

inductive Op (α : Type) where
  /-- A successful authentication; it ends the session the browser held before (IDN-011). -/
  | signIn (p : PrincipalId)
  /-- The session's principal asks for its device's share (enrolling the device first if its
      partition holds no device id) and derives its key. -/
  | fetchKey
  | write (v : α)
  | keepOffline
  /-- Sign-out with a connection: the server deletes this device's share. -/
  | signOut
  /-- Sign-out without a connection: the browser deletes its key and the device id. The server
      learns of it later, or never. -/
  | signOutOffline
  | expire
  /-- The server deletes one device's share (the person revokes a lost device). -/
  | revokeDev (d : Nat)
  /-- The server deletes every share of a principal (sign out everywhere, removal, erasure). -/
  | revokeAll (p : PrincipalId)
  | reload

/-- The session ends on the device: its key and hot set go, the device id and share stay. -/
def endSession {α : Type} (s : St α) : St α :=
  match s.session with
  | some p => { s with session := none, keys := upd s.keys p none, offline := upd s.offline p false,
                       memory := s.memory.filter (fun e => e.1 != p) }
  | none => s

def step {α : Type} (s : St α) : Op α → St α
  | .signIn p => { endSession s with session := some p }
  | .fetchKey =>
    match s.session with
    | some p =>
      match s.dev p with
      | some d =>
        match s.shares d with
        | some (q, k) => if q = p then { s with keys := upd s.keys p (some k) } else s
        | none => { s with dev := upd s.dev p none }
      | none =>
        { s with shares := setD s.shares s.nextDev (some (p, s.next)), dev := upd s.dev p (some s.nextDev),
                 keys := upd s.keys p (some s.next), next := s.next + 1, nextDev := s.nextDev + 1 }
    | none => s
  | .write v =>
    match s.session with
    | some p =>
      match s.keys p with
      | some k => { s with disk := ⟨p, k, v⟩ :: s.disk, memory := (p, v) :: s.memory }
      | none => s
    | none => s
  | .keepOffline =>
    match s.session with
    | some p => { s with offline := upd s.offline p true }
    | none => s
  | .signOut =>
    match s.session with
    | some p =>
      match s.dev p with
      | some d => { endSession s with shares := setD s.shares d none, dev := upd s.dev p none }
      | none => { endSession s with dev := upd s.dev p none }
    | none => s
  | .signOutOffline =>
    match s.session with
    | some p => { endSession s with dev := upd s.dev p none }
    | none => s
  | .expire => endSession s
  | .revokeDev d => { s with shares := setD s.shares d none }
  | .revokeAll p => { s with shares := fun d => match s.shares d with
                        | some (q, k) => if q = p then none else some (q, k)
                        | none => none }
  | .reload => { s with memory := [], keys := fun q => if s.offline q then s.keys q else none }

def run {α : Type} : St α → List (Op α) → St α
  | s, [] => s
  | s, op :: ops => run (step s op) ops

/-- A ciphertext opens on the device: some key the browser holds is its key. -/
def Opens {α : Type} (s : St α) (c : Sealed α) : Prop := ∃ q, s.keys q = some c.key

/-- What a read in a session of `q` can return: its hot set, and ciphertexts it can open. -/
def Served {α : Type} (s : St α) (q : PrincipalId) (x : α) : Prop :=
  (q, x) ∈ s.memory ∨ ∃ c ∈ s.disk, s.keys q = some c.key ∧ c.plain = x

/-! ## The invariant -/

structure Inv {α : Type} (s : St α) : Prop where
  keys_lt : ∀ q k, s.keys q = some k → k < s.next
  shares_lt : ∀ d q k, s.shares d = some (q, k) → k < s.next
  shares_dev_lt : ∀ d q k, s.shares d = some (q, k) → d < s.nextDev
  dev_lt : ∀ q d, s.dev q = some d → d < s.nextDev
  disk_lt : ∀ c ∈ s.disk, c.key < s.next
  disk_keys : ∀ c ∈ s.disk, ∀ q, s.keys q = some c.key → c.owner = q
  disk_shares : ∀ c ∈ s.disk, ∀ d q, s.shares d = some (q, c.key) → c.owner = q
  keys_shares : ∀ p d q k, s.keys p = some k → s.shares d = some (q, k) → p = q
  shares_key_inj : ∀ d d' p q k, s.shares d = some (p, k) → s.shares d' = some (q, k) → d = d'
  keys_inj : ∀ p q k, s.keys p = some k → s.keys q = some k → p = q
  /-- A partition's device id names a share of its own principal, if any. -/
  dev_own : ∀ q d r k, s.dev q = some d → s.shares d = some (r, k) → r = q

def init {α : Type} : St α :=
  { shares := fun _ => none, next := 0, nextDev := 0, session := none, dev := fun _ => none,
    keys := fun _ => none, offline := fun _ => false, disk := [], memory := [] }

theorem inv_init {α : Type} : Inv (init : St α) := by
  constructor <;> intros <;> simp_all [init]

/-- Every field of the invariant is kept when keys, shares and device ids only shrink. -/
theorem inv_shrink {α : Type} (s : St α) (h : Inv s) (keys' : PrincipalId → Option Nat)
    (shares' : Nat → Option (PrincipalId × Nat)) (dev' : PrincipalId → Option Nat)
    (hk : ∀ q k, keys' q = some k → s.keys q = some k)
    (hs : ∀ d v, shares' d = some v → s.shares d = some v)
    (hd : ∀ q d, dev' q = some d → s.dev q = some d)
    (mem : List (PrincipalId × α)) (sess : Option PrincipalId) (off : PrincipalId → Bool) :
    Inv { s with keys := keys', shares := shares', dev := dev', memory := mem, session := sess,
                 offline := off } := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
  constructor <;> dsimp only
  · exact fun q k hq => h1 q k (hk q k hq)
  · exact fun d q k hq => h2 d q k (hs d _ hq)
  · exact fun d q k hq => h3 d q k (hs d _ hq)
  · exact fun q d hq => h4 q d (hd q d hq)
  · exact h5
  · exact fun c hc q hq => h6 c hc q (hk q _ hq)
  · exact fun c hc d q hq => h7 c hc d q (hs d _ hq)
  · exact fun p d q k hp hq => h8 p d q k (hk p k hp) (hs d _ hq)
  · exact fun d d' p q k hp hq => h9 d d' p q k (hs d _ hp) (hs d' _ hq)
  · exact fun p q k hp hq => h10 p q k (hk p k hp) (hk q k hq)
  · exact fun q d r k hq hr => h11 q d r k (hd q d hq) (hs d _ hr)

theorem upd_none_sub {β : Type} (f : PrincipalId → Option β) (p : PrincipalId) :
    ∀ q b, upd f p none q = some b → f q = some b := by
  intro q b h
  by_cases hq : q = p
  · subst hq; simp at h
  · rwa [upd_other _ _ _ _ hq] at h

theorem setD_none_sub (f : Nat → Option (PrincipalId × Nat)) (d : Nat) :
    ∀ d' v, setD f d none d' = some v → f d' = some v := by
  intro d' v h
  simp only [setD] at h
  split at h
  · cases h
  · exact h

theorem endSession_keys_sub {α : Type} (s : St α) :
    ∀ q k, (endSession s).keys q = some k → s.keys q = some k := by
  unfold endSession
  cases s.session with
  | none => intro q k h; exact h
  | some p => exact upd_none_sub s.keys p

theorem endSession_eq {α : Type} (s : St α) :
    (endSession s).shares = s.shares ∧ (endSession s).dev = s.dev ∧ (endSession s).next = s.next ∧
    (endSession s).nextDev = s.nextDev ∧ (endSession s).disk = s.disk := by
  unfold endSession; cases s.session <;> exact ⟨rfl, rfl, rfl, rfl, rfl⟩

/-- The session ends, and shares and device ids may shrink: the invariant is kept. -/
theorem inv_endSession {α : Type} (s : St α) (h : Inv s) (shares' : Nat → Option (PrincipalId × Nat))
    (dev' : PrincipalId → Option Nat) (hs : ∀ d v, shares' d = some v → s.shares d = some v)
    (hd : ∀ q d, dev' q = some d → s.dev q = some d) (sess : Option PrincipalId) :
    Inv ({ shares := shares', next := (endSession s).next, nextDev := (endSession s).nextDev,
           session := sess, dev := dev', keys := (endSession s).keys, offline := (endSession s).offline,
           disk := (endSession s).disk, memory := (endSession s).memory } : St α) := by
  obtain ⟨_, _, e3, e4, e5⟩ := endSession_eq s
  rw [e3, e4, e5]
  exact inv_shrink s h (endSession s).keys shares' dev' (endSession_keys_sub s) hs hd
    (endSession s).memory sess (endSession s).offline

theorem inv_endSession0 {α : Type} (s : St α) (h : Inv s) : Inv (endSession s) := by
  unfold endSession
  cases s.session with
  | none => exact h
  | some p =>
    exact inv_shrink s h (upd s.keys p none) s.shares s.dev (upd_none_sub s.keys p) (fun _ _ x => x)
      (fun _ _ x => x) _ _ _

theorem endSession_idem {α : Type} (s : St α) : endSession (endSession s) = endSession s := by
  unfold endSession
  cases h : s.session <;> simp [h]

theorem inv_setKey {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (d k : Nat)
    (hdev : s.dev p = some d) (hsh : s.shares d = some (p, k)) (sess : Option PrincipalId) :
    Inv ({ shares := s.shares, next := s.next, nextDev := s.nextDev, session := sess, dev := s.dev,
           keys := upd s.keys p (some k), offline := s.offline, disk := s.disk, memory := s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
  constructor <;> dsimp only
  · intro q k' hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; subst hq; exact h2 _ _ _ hsh
    · rw [upd_other _ _ _ _ hqp] at hq; exact h1 q k' hq
  · exact h2
  · exact h3
  · exact h4
  · exact h5
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; exact h7 c hc d q (by rw [hsh, hq])
    · rw [upd_other _ _ _ _ hqp] at hq; exact h6 c hc q hq
  · exact h7
  · intro a d' q k' ha hq
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      have := h9 d d' a q k hsh hq; subst this
      rw [hsh] at hq; simp at hq; first | exact hq | exact hq.1
    · rw [upd_other _ _ _ _ hap] at ha; exact h8 a d' q k' ha hq
  · exact h9
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · rw [upd_other _ _ _ _ hbp] at hb; exact (h8 b d a k hb hsh).symm
    · rw [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; subst hb; exact h8 a d b _ ha hsh
      · rw [upd_other _ _ _ _ hbp] at hb; exact h10 a b k' ha hb
  · exact h11

theorem inv_enrol {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (sess : Option PrincipalId) :
    Inv ({ shares := setD s.shares s.nextDev (some (p, s.next)), next := s.next + 1, nextDev := s.nextDev + 1,
           session := sess, dev := upd s.dev p (some s.nextDev), keys := upd s.keys p (some s.next),
           offline := s.offline, disk := s.disk, memory := s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
  constructor <;> dsimp only
  · intro q k hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · rw [upd_other _ _ _ _ hqp] at hq; have := h1 q k hq; omega
  · intro d q k hq
    simp only [setD] at hq
    split at hq
    · simp at hq; omega
    · have := h2 d q k hq; omega
  · intro d q k hq
    simp only [setD] at hq
    split at hq
    · omega
    · have := h3 d q k hq; omega
  · intro q d hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · rw [upd_other _ _ _ _ hqp] at hq; have := h4 q d hq; omega
  · intro c hc; have := h5 c hc; omega
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; have := h5 c hc; omega
    · rw [upd_other _ _ _ _ hqp] at hq; exact h6 c hc q hq
  · intro c hc d q hq
    simp only [setD] at hq
    split at hq
    · simp at hq; have := h5 c hc; omega
    · exact h7 c hc d q hq
  · intro a d q k ha hq
    simp only [setD] at hq
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      split at hq
      · simp at hq; first | exact hq.1.symm | exact hq.symm | exact hq.1 | exact hq
      · have := h2 d q _ hq; omega
    · rw [upd_other _ _ _ _ hap] at ha
      split at hq
      · simp at hq; have := h1 a k ha; omega
      · exact h8 a d q k ha hq
  · intro d d' a b k ha hb
    simp only [setD] at ha hb
    split at ha <;> split at hb
    · omega
    · simp at ha; have := h2 d' b k hb; omega
    · simp at hb; have := h2 d a k ha; omega
    · exact h9 d d' a b k ha hb
  · intro a b k ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · rw [upd_other _ _ _ _ hbp] at hb; have := h1 b _ hb; omega
    · rw [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; have := h1 a _ ha; omega
      · rw [upd_other _ _ _ _ hbp] at hb; exact h10 a b k ha hb
  · intro q d r k hq hr
    simp only [setD] at hr
    by_cases hqp : q = p
    · subst hqp; simp at hq; subst hq
      split at hr
      · simp at hr; first | exact hr.1.symm | exact hr.symm | exact hr.1 | exact hr
      · omega
    · rw [upd_other _ _ _ _ hqp] at hq
      split at hr
      · have := h4 q d hq; omega
      · exact h11 q d r k hq hr

theorem inv_write {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (k : Nat) (v : α)
    (hk : s.keys p = some k) (sess : Option PrincipalId) :
    Inv ({ shares := s.shares, next := s.next, nextDev := s.nextDev, session := sess, dev := s.dev,
           keys := s.keys, offline := s.offline, disk := ⟨p, k, v⟩ :: s.disk,
           memory := (p, v) :: s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11⟩ := h
  constructor <;> dsimp only
  · exact h1
  · exact h2
  · exact h3
  · exact h4
  · intro c hc
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h1 p k hk
    · exact h5 c hc
  · intro c hc q hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h10 p q k hk hq
    · exact h6 c hc q hq
  · intro c hc d q hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h8 p d q k hk hq
    · exact h7 c hc d q hq
  · exact h8
  · exact h9
  · exact h10
  · exact h11

theorem step_inv {α : Type} (s : St α) (op : Op α) (h : Inv s) : Inv (step s op) := by
  have keep : ∀ d v, s.shares d = some v → s.shares d = some v := fun _ _ x => x
  have keepD : ∀ q d, s.dev q = some d → s.dev q = some d := fun _ _ x => x
  obtain ⟨e1, e2, _, _, _⟩ := endSession_eq s
  have esh : ∀ d v, (endSession s).shares d = some v → s.shares d = some v := fun d v hv => by
    rw [e1] at hv; exact hv
  have edv : ∀ q d, (endSession s).dev q = some d → s.dev q = some d := fun q d hv => by
    rw [e2] at hv; exact hv
  cases op with
  | signIn p => exact inv_endSession s h _ _ esh edv (some p)
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some d =>
        cases hsh : s.shares d with
        | some qk =>
          obtain ⟨q, k⟩ := qk
          by_cases hq : q = p
          · subst hq; simp only [step, hs, hd, hsh, ↓reduceIte]; exact inv_setKey s h q d k hd hsh _
          · simp only [step, hs, hd, hsh, hq, ↓reduceIte]; exact h
        | none =>
          simp only [step, hs, hd, hsh]
          exact inv_shrink s h s.keys s.shares (upd s.dev p none) (fun _ _ x => x) keep
            (upd_none_sub s.dev p) s.memory _ s.offline
      | none => simp only [step, hs, hd]; exact inv_enrol s h p _
  | write v =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hk : s.keys p with
      | none => simp only [step, hs, hk]; exact h
      | some k => simp only [step, hs, hk]; exact inv_write s h p k v hk _
  | keepOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      simp only [step, hs]
      exact inv_shrink s h s.keys s.shares s.dev (fun _ _ x => x) keep keepD _ _ _
  | signOut =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some d =>
        simp only [step, hs, hd]
        exact inv_endSession s h _ _ (setD_none_sub s.shares d) (upd_none_sub s.dev p) _
      | none =>
        simp only [step, hs, hd]
        exact inv_endSession s h _ _ esh (upd_none_sub s.dev p) _
  | signOutOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      simp only [step, hs]
      exact inv_endSession s h _ _ esh (upd_none_sub s.dev p) _
  | expire => exact inv_endSession0 s h
  | revokeDev d =>
    exact inv_shrink s h s.keys _ s.dev (fun _ _ x => x) (setD_none_sub s.shares d) keepD _ _ _
  | revokeAll p =>
    refine inv_shrink s h s.keys _ s.dev (fun _ _ x => x) (fun d v hv => ?_) keepD _ _ _
    cases hc : s.shares d with
    | none => simp [hc] at hv
    | some qk =>
      obtain ⟨q, k⟩ := qk
      by_cases hq : q = p
      · simp [hc, hq] at hv
      · simp [hc, hq] at hv; rw [← hv]
  | reload =>
    exact inv_shrink s h _ s.shares s.dev (fun q k hq => by
      by_cases ho : s.offline q
      · simp [ho] at hq; exact hq
      · simp [ho] at hq) keep keepD _ _ _

theorem run_inv {α : Type} (ops : List (Op α)) : ∀ s : St α, Inv s → Inv (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih => intro s h; exact ih _ (step_inv s op h)

theorem reachable_inv {α : Type} (ops : List (Op α)) : Inv (run (init : St α) ops) :=
  run_inv ops _ inv_init

/-! ## Separation between principals -/

/-- **A ciphertext opens only with its owner's key.**
    Verifies: FE-004 -/
theorem no_cross_principal {α : Type} (s : St α) (h : Inv s) (c : Sealed α) (hc : c ∈ s.disk)
    (q : PrincipalId) (hq : s.keys q = some c.key) : c.owner = q :=
  h.disk_keys c hc q hq

/-- The hot set only ever holds values written by their owner. -/
def MemOk {α : Type} (s : St α) : Prop := ∀ e ∈ s.memory, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2

theorem step_memOk {α : Type} (s : St α) (op : Op α) (h : MemOk s) : MemOk (step s op) := by
  have es : ∀ e ∈ (endSession s).memory, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2 := by
    unfold endSession
    cases s.session with
    | none => exact h
    | some q => exact fun e he => h e (List.mem_filter.mp he).1
  have ed := (endSession_eq s).2.2.2.2
  have esm : ∀ (t : St α), t.memory = (endSession s).memory → t.disk = s.disk → MemOk t :=
    fun t hm hd e he => by rw [hd]; rw [hm] at he; exact es e he
  cases op with
  | signIn p => exact esm _ rfl ed
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some d =>
        cases hsh : s.shares d with
        | some qk =>
          obtain ⟨q, k⟩ := qk
          by_cases hq : q = p
          · subst hq; simp only [step, hs, hd, hsh, ↓reduceIte]; exact h
          · simp only [step, hs, hd, hsh, hq, ↓reduceIte]; exact h
        | none => simp only [step, hs, hd, hsh]; exact h
      | none => simp only [step, hs, hd]; exact h
  | write v =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hk : s.keys p with
      | none => simp only [step, hs, hk]; exact h
      | some k =>
        simp only [step, hs, hk]
        intro e he
        rcases List.mem_cons.mp he with he | he
        · subst he; exact ⟨⟨p, k, v⟩, List.mem_cons_self .., rfl, rfl⟩
        · obtain ⟨c, hc, h1, h2⟩ := h e he
          exact ⟨c, List.mem_cons_of_mem _ hc, h1, h2⟩
  | keepOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p => simp only [step, hs]; exact h
  | signOut =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some d => simp only [step, hs, hd]; exact esm _ rfl ed
      | none => simp only [step, hs, hd]; exact esm _ rfl ed
  | signOutOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p => simp only [step, hs]; exact esm _ rfl ed
  | expire => exact esm _ rfl ed
  | revokeDev d => exact h
  | revokeAll p => exact h
  | reload => intro e he; cases he

theorem run_memOk {α : Type} (ops : List (Op α)) : ∀ s : St α, MemOk s → MemOk (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih => intro s h; exact ih _ (step_memOk s op h)

/-- **What a session of `q` can be served was written by `q`.**
    Verifies: FE-004 -/
theorem served_own {α : Type} (s : St α) (h : Inv s) (hm : MemOk s) (q : PrincipalId) (x : α)
    (hx : Served s q x) : ∃ c ∈ s.disk, c.owner = q ∧ c.plain = x := by
  rcases hx with hx | ⟨c, hc, hk, hp⟩
  · exact hm _ hx
  · exact ⟨c, hc, h.disk_keys c hc q hk, hp⟩

/-! ## Sign-out, expiry and revocation -/

/-- A key id that nothing holds and that no device id the browser holds can fetch again. -/
def Dead {α : Type} (s : St α) (k : Nat) : Prop :=
  k < s.next ∧ (∀ q, s.keys q ≠ some k) ∧ (∀ d q, s.shares d = some (q, k) → ∀ q', s.dev q' ≠ some d)

theorem dead_shrink {α : Type} (s : St α) (k : Nat) (hd : Dead s k) (s' : St α) (hn : s'.next = s.next)
    (hk : ∀ q k, s'.keys q = some k → s.keys q = some k)
    (hs : ∀ d v, s'.shares d = some v → s.shares d = some v)
    (hdv : ∀ q d, s'.dev q = some d → s.dev q = some d) : Dead s' k := by
  obtain ⟨h1, h2, h3⟩ := hd
  exact ⟨by rw [hn]; exact h1, fun q hq => h2 q (hk q k hq),
    fun d q hq q' hq' => h3 d q (hs d _ hq) q' (hdv q' d hq')⟩

theorem dead_step {α : Type} (s : St α) (op : Op α) (k : Nat) (hi : Inv s) (hd : Dead s k) :
    Dead (step s op) k := by
  obtain ⟨e1, e2, e3, e4, e5⟩ := endSession_eq s
  have keep : ∀ d v, s.shares d = some v → s.shares d = some v := fun _ _ x => x
  have keepD : ∀ q d, s.dev q = some d → s.dev q = some d := fun _ _ x => x
  have esh : ∀ d v, (endSession s).shares d = some v → s.shares d = some v := fun d v hv => by
    rw [e1] at hv; exact hv
  have edv : ∀ q d, (endSession s).dev q = some d → s.dev q = some d := fun q d hv => by
    rw [e2] at hv; exact hv
  cases op with
  | signIn p => exact dead_shrink s k hd _ e3 (endSession_keys_sub s) esh edv
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      cases hdv : s.dev p with
      | some d =>
        cases hsh : s.shares d with
        | some qk =>
          obtain ⟨q, k0⟩ := qk
          by_cases hq : q = p
          · subst hq
            simp only [step, hs, hdv, hsh, ↓reduceIte]
            obtain ⟨h1, h2, h3⟩ := hd
            refine ⟨h1, fun q' hq' => ?_, h3⟩
            by_cases hqp : q' = q
            · subst hqp; simp at hq'; subst hq'; exact h3 d q' hsh q' hdv
            · simp only [upd_other _ _ _ _ hqp] at hq'; exact h2 q' hq'
          · simp only [step, hs, hdv, hsh, hq, ↓reduceIte]; exact hd
        | none =>
          simp only [step, hs, hdv, hsh]
          exact dead_shrink s k hd _ rfl (fun _ _ x => x) keep (upd_none_sub s.dev p)
      | none =>
        simp only [step, hs, hdv]
        obtain ⟨h1, h2, h3⟩ := hd
        refine ⟨by dsimp only; omega, fun q' hq' => ?_, fun d q hq q' hq' => ?_⟩
        · by_cases hqp : q' = p
          · subst hqp; simp at hq'; omega
          · simp only [upd_other _ _ _ _ hqp] at hq'; exact h2 q' hq'
        · simp only [setD] at hq
          split at hq
          · simp at hq; omega
          · have hlt := hi.shares_dev_lt d q k hq
            by_cases hqp : q' = p
            · subst hqp; simp at hq'; omega
            · simp only [upd_other _ _ _ _ hqp] at hq'; exact h3 d q hq q' hq'
  | write v =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      cases hk : s.keys p with
      | none => simp only [step, hs, hk]; exact hd
      | some k' => simp only [step, hs, hk]; exact dead_shrink s k hd _ rfl (fun _ _ x => x) keep keepD
  | keepOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p => simp only [step, hs]; exact dead_shrink s k hd _ rfl (fun _ _ x => x) keep keepD
  | signOut =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      cases hdv : s.dev p with
      | some d =>
        simp only [step, hs, hdv]
        exact dead_shrink s k hd _ e3 (endSession_keys_sub s) (setD_none_sub s.shares d) (upd_none_sub s.dev p)
      | none =>
        simp only [step, hs, hdv]
        exact dead_shrink s k hd _ e3 (endSession_keys_sub s) esh (upd_none_sub s.dev p)
  | signOutOffline =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      simp only [step, hs]
      exact dead_shrink s k hd _ e3 (endSession_keys_sub s) esh (upd_none_sub s.dev p)
  | expire => exact dead_shrink s k hd _ e3 (endSession_keys_sub s) esh edv
  | revokeDev d =>
    exact dead_shrink s k hd _ rfl (fun _ _ x => x) (setD_none_sub s.shares d) keepD
  | revokeAll p =>
    refine dead_shrink s k hd _ rfl (fun _ _ x => x) (fun d v hv => ?_) keepD
    cases hc : s.shares d with
    | none => simp [step, hc] at hv
    | some qk =>
      obtain ⟨q, k'⟩ := qk
      by_cases hq : q = p
      · simp [step, hc, hq] at hv
      · simp [step, hc, hq] at hv; rw [← hv]
  | reload =>
    refine dead_shrink s k hd _ rfl (fun q k' hq => ?_) keep keepD
    by_cases ho : s.offline q
    · simp [step, ho] at hq; exact hq
    · simp [step, ho] at hq

theorem dead_run {α : Type} (ops : List (Op α)) :
    ∀ (s : St α) (k : Nat), Inv s → Dead s k → Dead (run s ops) k := by
  induction ops with
  | nil => intro s k _ h; exact h
  | cons op ops ih => intro s k hi h; exact ih _ k (step_inv s op hi) (dead_step s op k hi h)

theorem dead_not_opens {α : Type} (s : St α) (c : Sealed α) (hd : Dead s c.key) : ¬ Opens s c := by
  rintro ⟨q, hq⟩
  exact hd.2.1 q hq

/-- After the browser drops `p`'s key and device id, every ciphertext of `p` has a dead key. -/
theorem dead_after_drop {α : Type} (s s' : St α) (h : Inv s) (p : PrincipalId) (c : Sealed α)
    (hc : c ∈ s.disk) (ho : c.owner = p) (hn : s'.next = s.next)
    (hk : ∀ q k, s'.keys q = some k → s.keys q = some k) (hkp : s'.keys p = none)
    (hs : ∀ d v, s'.shares d = some v → s.shares d = some v)
    (hdv : ∀ q d, s'.dev q = some d → s.dev q = some d) (hdp : s'.dev p = none) : Dead s' c.key := by
  refine ⟨by rw [hn]; exact h.disk_lt c hc, fun q hq => ?_, fun d q hq q' hq' => ?_⟩
  · have := h.disk_keys c hc q (hk q _ hq)
    rw [ho] at this; subst this; rw [hkp] at hq; cases hq
  · have h1 := h.disk_shares c hc d q (hs d _ hq)
    have h2 := h.dev_own q' d q c.key (hdv q' d hq') (hs d _ hq)
    rw [ho] at h1; subst h1; subst h2; rw [hdp] at hq'; cases hq'

/-- **After sign-out with a connection, nothing the principal left at rest opens, ever again.**
    Verifies: FE-005 -/
theorem signout_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOut) ops) c := by
  apply dead_not_opens
  apply dead_run ops _ _ (step_inv s _ h)
  obtain ⟨e1, e2, e3, e4, e5⟩ := endSession_eq s
  have hk : ∀ q k, (endSession s).keys q = some k → s.keys q = some k := endSession_keys_sub s
  have hkp : (endSession s).keys p = none := by simp [endSession, hs]
  cases hd : s.dev p with
  | some d =>
    simp only [step, hs, hd]
    exact dead_after_drop s _ h p c hc ho e3 hk hkp (setD_none_sub s.shares d) (upd_none_sub s.dev p) (by simp)
  | none =>
    simp only [step, hs, hd]
    exact dead_after_drop s _ h p c hc ho e3 hk hkp (by intro d v hv; simpa [e1] using hv)
      (upd_none_sub s.dev p) (by simp)

/-- **After sign-out without a connection, nothing the principal left at rest opens, ever again**:
    the share stays at the server, but no device id the browser holds can fetch it.
    Verifies: FE-005 -/
theorem offline_signout_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId)
    (hs : s.session = some p) (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOutOffline) ops) c := by
  apply dead_not_opens
  apply dead_run ops _ _ (step_inv s _ h)
  obtain ⟨e1, e2, e3, e4, e5⟩ := endSession_eq s
  have hkp : (endSession s).keys p = none := by simp [endSession, hs]
  simp only [step, hs]
  exact dead_after_drop s _ h p c hc ho e3 (endSession_keys_sub s) hkp
    (by intro d v hv; simpa [e1] using hv) (upd_none_sub s.dev p) (by simp)

/-- **After the session expires, nothing of its principal opens** in the browser.
    Verifies: FE-006 -/
theorem expire_locks {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) : ¬ Opens (step s .expire) c := by
  rintro ⟨q, hq⟩
  simp only [step, endSession, hs] at hq
  by_cases hqp : q = p
  · subst hqp; simp at hq
  · rw [upd_other _ _ _ _ hqp] at hq
    exact hqp ((h.disk_keys c hc q hq).symm.trans ho)

/-- **Only a session of `p` puts `p`'s key back.**
    Verifies: FE-006 -/
theorem unlock_needs_own_session {α : Type} (s : St α) (op : Op α) (p : PrincipalId)
    (hnone : s.keys p = none) (hsome : (step s op).keys p ≠ none) :
    op = .fetchKey ∧ s.session = some p := by
  have endNone : (endSession s).keys p = none := by
    cases h : (endSession s).keys p with
    | none => rfl
    | some k => have := endSession_keys_sub s p k h; rw [hnone] at this; cases this
  cases op with
  | signIn q => exact absurd endNone (by simpa [step] using hsome)
  | fetchKey =>
    refine ⟨rfl, ?_⟩
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q =>
      by_cases hqp : p = q
      · rw [hqp]
      · exfalso
        cases hd : s.dev q with
        | none => simp [step, hs, hd, upd_other _ _ _ _ hqp, hnone] at hsome
        | some d =>
          cases hsh : s.shares d with
          | none => simp [step, hs, hd, hsh, hnone] at hsome
          | some qk =>
            obtain ⟨r, k⟩ := qk
            by_cases hr : r = q
            · simp [step, hs, hd, hsh, hr, upd_other _ _ _ _ hqp, hnone] at hsome
            · simp [step, hs, hd, hsh, hr, hnone] at hsome
  | write v =>
    exfalso
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q => cases hk : s.keys q <;> simp [step, hs, hk, hnone] at hsome
  | keepOffline =>
    exfalso
    cases hs : s.session <;> simp [step, hs, hnone] at hsome
  | signOut =>
    exfalso
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q =>
      apply hsome
      simp only [step, hs]
      cases s.dev q <;> exact endNone
  | signOutOffline =>
    exfalso
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q => apply hsome; simp only [step, hs]; exact endNone
  | expire => exact absurd endNone (by simpa [step] using hsome)
  | revokeDev d => exact absurd hnone (by simpa [step] using hsome)
  | revokeAll q => exact absurd hnone (by simpa [step] using hsome)
  | reload => exfalso; simp [step, hnone] at hsome

/-- Expiry keeps the device id and share: the same principal signing in again unlocks its data
    (the queue survives re-authentication). -/
theorem expire_then_same_principal_unlocks {α : Type} (s : St α) (p : PrincipalId) (d k : Nat)
    (hd : s.dev p = some d) (hsh : s.shares d = some (p, k)) :
    (run s [.expire, .signIn p, .fetchKey]).keys p = some k := by
  have e := endSession_eq s
  simp [run, step, endSession_idem, e.1, e.2.1, hd, hsh]

/-- **A device revoked at the server is unreadable from its next reload** (no offline key kept). -/
theorem revoke_device_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (d : Nat)
    (c : Sealed α) (hc : c ∈ s.disk) (hsh : s.shares d = some (p, c.key)) (hoff : s.offline p = false)
    (ops : List (Op α)) : ¬ Opens (run s (.revokeDev d :: .reload :: ops)) c := by
  have ho : c.owner = p := h.disk_shares c hc d p hsh
  apply dead_not_opens
  show Dead (run (step (step s (.revokeDev d)) .reload) ops) c.key
  apply dead_run ops _ _ (step_inv _ _ (step_inv s _ h))
  refine ⟨h.disk_lt c hc, fun q hq => ?_, fun d' q hq q' _ => ?_⟩
  · by_cases hoffq : s.offline q
    · simp [step, hoffq] at hq
      have := h.disk_keys c hc q hq
      rw [ho] at this; subst this; rw [hoff] at hoffq; cases hoffq
    · simp [step, hoffq] at hq
  · simp only [step, setD] at hq
    by_cases hdd : d' = d
    · simp [hdd] at hq
    · simp [hdd] at hq
      exact hdd (h.shares_key_inj d' d q p c.key hq hsh)

/-- **Signing out everywhere (or removing or erasing the principal) makes every device's copies
    unreadable from its next reload** (no offline key kept). -/
theorem revoke_all_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (c : Sealed α)
    (hc : c ∈ s.disk) (ho : c.owner = p) (hoff : s.offline p = false) (ops : List (Op α)) :
    ¬ Opens (run s (.revokeAll p :: .reload :: ops)) c := by
  apply dead_not_opens
  show Dead (run (step (step s (.revokeAll p)) .reload) ops) c.key
  apply dead_run ops _ _ (step_inv _ _ (step_inv s _ h))
  refine ⟨h.disk_lt c hc, fun q hq => ?_, fun d q hq q' _ => ?_⟩
  · by_cases hoffq : s.offline q
    · simp [step, hoffq] at hq
      have := h.disk_keys c hc q hq
      rw [ho] at this; subst this; rw [hoff] at hoffq; cases hoffq
    · simp [step, hoffq] at hq
  · cases hc' : s.shares d with
    | none => simp [step, hc'] at hq
    | some rk =>
      obtain ⟨r, k⟩ := rk
      by_cases hr : r = p
      · simp [step, hc', hr] at hq
      · simp [step, hc', hr] at hq
        obtain ⟨_, h2⟩ := hq
        have := h.disk_shares c hc d r (by rw [hc', h2])
        rw [ho] at this; exact hr this.symm

/-- With an offline key kept, a reload does not erase: the model enforces no bound on it. -/
theorem offline_key_survives_reload {α : Type} (s : St α) (p : PrincipalId) (k : Nat)
    (hk : s.keys p = some k) (hoff : s.offline p = true) : (step s .reload).keys p = some k := by
  simp [step, hoff, hk]

end Strata.Client.Privacy
