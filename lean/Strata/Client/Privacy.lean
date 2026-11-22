/-
What the browser keeps at rest, under whose key, and when it stops being readable
(docs/frontend.md, "Privacy at rest").

Everything a principal's partition holds at rest (view results, the write queue, drafts) is
encrypted under a key derived from a **key share** the identity service holds for one enrolled
device. At enrolment the service issues a device id and a **device secret**; it keeps the share and
a hash of the secret, and gives the share only to a session of the principal the device was enrolled
for **that presents the secret**. The device id is not a credential: it may be listed and known to
other sessions of the same principal (`fetchById` lets any of them name any id). The browser keeps
the id and secret in the partition and the derived key in the worker's memory (and in IndexedDB
only when a realm allows keeping work available offline and the person opts in).

What this protects against, and what it does not:

- **the next user of the device**, and **someone holding a copy of the disk after the key and secret
  are gone**, even one with a session of the same principal elsewhere: they can only present
  secrets and use keys still in some browser (`Opens`), and the model lets any operation open any
  ciphertext with any key the browser holds;
- **not code running in the origin while the person is signed in** (cross-site scripting, a
  malicious extension): it can read the secret, the share or the plaintext and copy them out.

Proved:

- **one principal's data never opens for another** (`no_cross_principal`, `served_own`);
- **sign-out, online or offline, leaves nothing the principal left at rest openable by any key
  or secret the browser then or later holds** (`signout_erases`, `offline_signout_erases`):
  online sign-out deletes the share; offline sign-out deletes the key, the id and the secret, so the
  share still at the server can never be fetched again, by this browser or by a session that knows
  the id;
- **expiry locks, and only the same principal unlocks** (`expire_locks`, `unlock_needs_own_session`,
  `expire_then_same_principal_unlocks`);
- **revocation reaches the device at its next reload** (`revoke_device_erases`, `revoke_all_erases`),
  unless the person kept an offline key, which survives a reload (`offline_key_survives_reload`);
- **rotation loses no record and no update** (`rot_no_loss`, `rot_no_lost_update`,
  `rot_finish_retires`): every change is computed outside a transaction and verified in one short
  transaction, an append or rewrite against the partition's current key id and a re-encryption
  against the version it read; without the key-id check a stale tab's append outlives the old key
  (`rot_stale_append_breaks`), and without the compare-and-swap a re-encryption loses an update
  (`rot_blind_write_loses_update`).

`fetchById` always presents the secret of the partition the session holds: it shows that knowing a
device id gives nothing without that device's secret, and says nothing about an attacker who holds
some other device's secret.

Assumptions, all of them:

P1. Encryption is ideal: a ciphertext opens only with the key it was made with.
P2. The server gives a share only to a session of its principal presenting its secret, draws ids,
    secrets and shares fresh, and never brings a deleted share back (a restore of the identity
    database re-applies the deletions recorded after its point).
P3. Deleting a key, an id or a secret from the browser removes it; a secret is not guessable;
    disk forensics of deleted IndexedDB records and code reading the browser's state while the
    person is signed in are outside the model.

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
  /-- Server: for each enrolled device id, its principal, its share and (the hash of) its secret. -/
  shares : Nat → Option (PrincipalId × Nat × Nat)
  /-- Server: fresh share ids, device ids and secrets. -/
  next : Nat
  nextDev : Nat
  nextSec : Nat
  session : Option PrincipalId
  /-- Device: the device id and secret each partition holds. -/
  dev : PrincipalId → Option (Nat × Nat)
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

def setD (f : Nat → Option (PrincipalId × Nat × Nat)) (d : Nat) (v : Option (PrincipalId × Nat × Nat)) :
    Nat → Option (PrincipalId × Nat × Nat) := fun d' => if d' = d then v else f d'

inductive Op (α : Type) where
  | signIn (p : PrincipalId)
  /-- The session asks for its own device's share, presenting the partition's secret (enrolling
      the device first if the partition holds none). -/
  | fetchKey
  /-- The session names any device id it knows (ids are listed), presenting the partition's secret. -/
  | fetchById (d : Nat)
  | write (v : α)
  | keepOffline
  | signOut
  | signOutOffline
  | expire
  | revokeDev (d : Nat)
  | revokeAll (p : PrincipalId)
  | reload

def endSession {α : Type} (s : St α) : St α :=
  match s.session with
  | some p => { s with session := none, keys := upd s.keys p none, offline := upd s.offline p false,
                       memory := s.memory.filter (fun e => e.1 != p) }
  | none => s

/-- Ask for the share at device id `d`, presenting secret `σ`, for principal `p`. -/
def fetchAt {α : Type} (s : St α) (p : PrincipalId) (d σ : Nat) : St α :=
  match s.shares d with
  | some (q, k, σ') => if q = p ∧ σ' = σ then { s with keys := upd s.keys p (some k) } else s
  | none => s

def step {α : Type} (s : St α) : Op α → St α
  | .signIn p => { endSession s with session := some p }
  | .fetchKey =>
    match s.session with
    | some p =>
      match s.dev p with
      | some (d, σ) =>
        match s.shares d with
        | some _ => fetchAt s p d σ
        | none => { s with dev := upd s.dev p none }
      | none =>
        { s with shares := setD s.shares s.nextDev (some (p, s.next, s.nextSec)),
                 dev := upd s.dev p (some (s.nextDev, s.nextSec)), keys := upd s.keys p (some s.next),
                 next := s.next + 1, nextDev := s.nextDev + 1, nextSec := s.nextSec + 1 }
    | none => s
  | .fetchById d =>
    match s.session with
    | some p =>
      match s.dev p with
      | some (_, σ) => fetchAt s p d σ
      | none => s
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
      | some (d, _) => { endSession s with shares := setD s.shares d none, dev := upd s.dev p none }
      | none => { endSession s with dev := upd s.dev p none }
    | none => s
  | .signOutOffline =>
    match s.session with
    | some p => { endSession s with dev := upd s.dev p none }
    | none => s
  | .expire => endSession s
  | .revokeDev d => { s with shares := setD s.shares d none }
  | .revokeAll p => { s with shares := fun d => match s.shares d with
                        | some (q, k, σ) => if q = p then none else some (q, k, σ)
                        | none => none }
  | .reload => { s with memory := [], keys := fun q => if s.offline q then s.keys q else none }

def run {α : Type} : St α → List (Op α) → St α
  | s, [] => s
  | s, op :: ops => run (step s op) ops

def Opens {α : Type} (s : St α) (c : Sealed α) : Prop := ∃ q, s.keys q = some c.key

def Served {α : Type} (s : St α) (q : PrincipalId) (x : α) : Prop :=
  (q, x) ∈ s.memory ∨ ∃ c ∈ s.disk, s.keys q = some c.key ∧ c.plain = x

/-! ## The invariant -/

structure Inv {α : Type} (s : St α) : Prop where
  keys_lt : ∀ q k, s.keys q = some k → k < s.next
  shares_lt : ∀ d q k σ, s.shares d = some (q, k, σ) → k < s.next
  shares_dev_lt : ∀ d q k σ, s.shares d = some (q, k, σ) → d < s.nextDev
  shares_sec_lt : ∀ d q k σ, s.shares d = some (q, k, σ) → σ < s.nextSec
  dev_lt : ∀ q d σ, s.dev q = some (d, σ) → σ < s.nextSec
  disk_lt : ∀ c ∈ s.disk, c.key < s.next
  disk_keys : ∀ c ∈ s.disk, ∀ q, s.keys q = some c.key → c.owner = q
  disk_shares : ∀ c ∈ s.disk, ∀ d q σ, s.shares d = some (q, c.key, σ) → c.owner = q
  keys_shares : ∀ p d q k σ, s.keys p = some k → s.shares d = some (q, k, σ) → p = q
  shares_key_inj : ∀ d d' p q k σ σ', s.shares d = some (p, k, σ) → s.shares d' = some (q, k, σ') → d = d'
  keys_inj : ∀ p q k, s.keys p = some k → s.keys q = some k → p = q
  /-- A secret a partition holds is the secret of a share of its own principal, if any. -/
  dev_sec : ∀ q d σ d' r k, s.dev q = some (d, σ) → s.shares d' = some (r, k, σ) → r = q

def init {α : Type} : St α :=
  { shares := fun _ => none, next := 0, nextDev := 0, nextSec := 0, session := none,
    dev := fun _ => none, keys := fun _ => none, offline := fun _ => false, disk := [], memory := [] }

theorem inv_init {α : Type} : Inv (init : St α) := by
  constructor <;> intros <;> simp_all [init]

theorem inv_shrink {α : Type} (s : St α) (h : Inv s) (keys' : PrincipalId → Option Nat)
    (shares' : Nat → Option (PrincipalId × Nat × Nat)) (dev' : PrincipalId → Option (Nat × Nat))
    (hk : ∀ q k, keys' q = some k → s.keys q = some k)
    (hs : ∀ d v, shares' d = some v → s.shares d = some v)
    (hd : ∀ q v, dev' q = some v → s.dev q = some v)
    (mem : List (PrincipalId × α)) (sess : Option PrincipalId) (off : PrincipalId → Bool) :
    Inv { s with keys := keys', shares := shares', dev := dev', memory := mem, session := sess,
                 offline := off } := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12⟩ := h
  constructor <;> dsimp only
  · exact fun q k hq => h1 q k (hk q k hq)
  · exact fun d q k σ hq => h2 d q k σ (hs d _ hq)
  · exact fun d q k σ hq => h3 d q k σ (hs d _ hq)
  · exact fun d q k σ hq => h4 d q k σ (hs d _ hq)
  · exact fun q d σ hq => h5 q d σ (hd q _ hq)
  · exact h6
  · exact fun c hc q hq => h7 c hc q (hk q _ hq)
  · exact fun c hc d q σ hq => h8 c hc d q σ (hs d _ hq)
  · exact fun p d q k σ hp hq => h9 p d q k σ (hk p k hp) (hs d _ hq)
  · exact fun d d' p q k σ σ' hp hq => h10 d d' p q k σ σ' (hs d _ hp) (hs d' _ hq)
  · exact fun p q k hp hq => h11 p q k (hk p k hp) (hk q k hq)
  · exact fun q d σ d' r k hq hr => h12 q d σ d' r k (hd q _ hq) (hs d' _ hr)

theorem upd_none_sub {β : Type} (f : PrincipalId → Option β) (p : PrincipalId) :
    ∀ q b, upd f p none q = some b → f q = some b := by
  intro q b h
  by_cases hq : q = p
  · subst hq; simp at h
  · rwa [upd_other _ _ _ _ hq] at h

theorem setD_none_sub (f : Nat → Option (PrincipalId × Nat × Nat)) (d : Nat) :
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
    (endSession s).nextDev = s.nextDev ∧ (endSession s).nextSec = s.nextSec ∧ (endSession s).disk = s.disk := by
  unfold endSession; cases s.session <;> exact ⟨rfl, rfl, rfl, rfl, rfl, rfl⟩

theorem inv_endSession {α : Type} (s : St α) (h : Inv s) (shares' : Nat → Option (PrincipalId × Nat × Nat))
    (dev' : PrincipalId → Option (Nat × Nat)) (hs : ∀ d v, shares' d = some v → s.shares d = some v)
    (hd : ∀ q v, dev' q = some v → s.dev q = some v) (sess : Option PrincipalId) :
    Inv ({ shares := shares', next := (endSession s).next, nextDev := (endSession s).nextDev,
           nextSec := (endSession s).nextSec, session := sess, dev := dev', keys := (endSession s).keys,
           offline := (endSession s).offline, disk := (endSession s).disk,
           memory := (endSession s).memory } : St α) := by
  obtain ⟨_, _, e3, e4, e5, e6⟩ := endSession_eq s
  rw [e3, e4, e5, e6]
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

theorem inv_setKey {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (d k σ : Nat)
    (hsh : s.shares d = some (p, k, σ)) :
    Inv { s with keys := upd s.keys p (some k) } := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12⟩ := h
  constructor <;> dsimp only
  · intro q k' hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; subst hq; exact h2 _ _ _ _ hsh
    · rw [upd_other _ _ _ _ hqp] at hq; exact h1 q k' hq
  · exact h2
  · exact h3
  · exact h4
  · exact h5
  · exact h6
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; exact h8 c hc d q σ (by rw [hsh, hq])
    · rw [upd_other _ _ _ _ hqp] at hq; exact h7 c hc q hq
  · exact h8
  · intro a d' q k' σ' ha hq
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      have := h10 d d' a q k σ σ' hsh hq; subst this
      rw [hsh] at hq; simp at hq; exact hq.1
    · rw [upd_other _ _ _ _ hap] at ha; exact h9 a d' q k' σ' ha hq
  · exact h10
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · rw [upd_other _ _ _ _ hbp] at hb; exact (h9 b d a k σ hb hsh).symm
    · rw [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; subst hb; exact h9 a d b _ σ ha hsh
      · rw [upd_other _ _ _ _ hbp] at hb; exact h11 a b k' ha hb
  · exact h12

theorem fetchAt_inv {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (d σ : Nat) :
    Inv (fetchAt s p d σ) := by
  unfold fetchAt
  cases hsh : s.shares d with
  | none => exact h
  | some v =>
    obtain ⟨q, k, σ'⟩ := v
    simp only
    by_cases hc : q = p ∧ σ' = σ
    · obtain ⟨rfl, rfl⟩ := hc
      simp only [and_self, ↓reduceIte]
      exact inv_setKey s h q d k σ' hsh
    · simp only [hc, ↓reduceIte]; exact h

theorem inv_enrol {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (sess : Option PrincipalId) :
    Inv ({ shares := setD s.shares s.nextDev (some (p, s.next, s.nextSec)), next := s.next + 1,
           nextDev := s.nextDev + 1, nextSec := s.nextSec + 1, session := sess,
           dev := upd s.dev p (some (s.nextDev, s.nextSec)), keys := upd s.keys p (some s.next),
           offline := s.offline, disk := s.disk, memory := s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12⟩ := h
  constructor <;> dsimp only
  · intro q k hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · rw [upd_other _ _ _ _ hqp] at hq; have := h1 q k hq; omega
  · intro d q k σ hq
    simp only [setD] at hq
    split at hq
    · simp at hq; omega
    · have := h2 d q k σ hq; omega
  · intro d q k σ hq
    simp only [setD] at hq
    split at hq
    · omega
    · have := h3 d q k σ hq; omega
  · intro d q k σ hq
    simp only [setD] at hq
    split at hq
    · simp at hq; omega
    · have := h4 d q k σ hq; omega
  · intro q d σ hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · rw [upd_other _ _ _ _ hqp] at hq; have := h5 q d σ hq; omega
  · intro c hc; have := h6 c hc; omega
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; have := h6 c hc; omega
    · rw [upd_other _ _ _ _ hqp] at hq; exact h7 c hc q hq
  · intro c hc d q σ hq
    simp only [setD] at hq
    split at hq
    · simp at hq; have := h6 c hc; omega
    · exact h8 c hc d q σ hq
  · intro a d q k σ ha hq
    simp only [setD] at hq
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      split at hq
      · simp at hq; first | exact hq.1 | exact hq.1.symm
      · have := h2 d q _ σ hq; omega
    · rw [upd_other _ _ _ _ hap] at ha
      split at hq
      · simp at hq; have := h1 a k ha; omega
      · exact h9 a d q k σ ha hq
  · intro d d' a b k σ σ' ha hb
    simp only [setD] at ha hb
    split at ha <;> split at hb
    · omega
    · simp at ha; have := h2 d' b k σ' hb; omega
    · simp at hb; have := h2 d a k σ ha; omega
    · exact h10 d d' a b k σ σ' ha hb
  · intro a b k ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · rw [upd_other _ _ _ _ hbp] at hb; have := h1 b _ hb; omega
    · rw [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; have := h1 a _ ha; omega
      · rw [upd_other _ _ _ _ hbp] at hb; exact h11 a b k ha hb
  · intro q d σ d' r k hq hr
    simp only [setD] at hr
    by_cases hqp : q = p
    · subst hqp; simp at hq; obtain ⟨rfl, rfl⟩ := hq
      split at hr
      · simp at hr; exact hr.1.symm
      · have := h4 d' r k _ hr; omega
    · rw [upd_other _ _ _ _ hqp] at hq
      split at hr
      · simp at hr; have := h5 q d σ hq; omega
      · exact h12 q d σ d' r k hq hr

theorem inv_write {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (k : Nat) (v : α)
    (hk : s.keys p = some k) (sess : Option PrincipalId) :
    Inv ({ shares := s.shares, next := s.next, nextDev := s.nextDev, nextSec := s.nextSec,
           session := sess, dev := s.dev, keys := s.keys, offline := s.offline,
           disk := ⟨p, k, v⟩ :: s.disk, memory := (p, v) :: s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, h11, h12⟩ := h
  constructor <;> dsimp only
  · exact h1
  · exact h2
  · exact h3
  · exact h4
  · exact h5
  · intro c hc
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h1 p k hk
    · exact h6 c hc
  · intro c hc q hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h11 p q k hk hq
    · exact h7 c hc q hq
  · intro c hc d q σ hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h9 p d q k σ hk hq
    · exact h8 c hc d q σ hq
  · exact h9
  · exact h10
  · exact h11
  · exact h12

theorem step_inv {α : Type} (s : St α) (op : Op α) (h : Inv s) : Inv (step s op) := by
  have keep : ∀ d v, s.shares d = some v → s.shares d = some v := fun _ _ x => x
  have keepD : ∀ q v, s.dev q = some v → s.dev q = some v := fun _ _ x => x
  obtain ⟨e1, e2, _, _, _, _⟩ := endSession_eq s
  have esh : ∀ d v, (endSession s).shares d = some v → s.shares d = some v := fun d v hv => by
    rw [e1] at hv; exact hv
  have edv : ∀ q v, (endSession s).dev q = some v → s.dev q = some v := fun q v hv => by
    rw [e2] at hv; exact hv
  cases op with
  | signIn p => exact inv_endSession s h _ _ esh edv (some p)
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some dσ =>
        obtain ⟨d, σ⟩ := dσ
        cases hsh : s.shares d with
        | some v => simp only [step, hs, hd, hsh]; exact fetchAt_inv s h p d σ
        | none =>
          simp only [step, hs, hd, hsh]
          exact inv_shrink s h s.keys s.shares (upd s.dev p none) (fun _ _ x => x) keep
            (upd_none_sub s.dev p) s.memory _ s.offline
      | none => simp only [step, hs, hd]; exact inv_enrol s h p _
  | fetchById d =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some dσ => obtain ⟨d0, σ⟩ := dσ; simp only [step, hs, hd]; exact fetchAt_inv s h p d σ
      | none => simp only [step, hs, hd]; exact h
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
      | some dσ =>
        obtain ⟨d, σ⟩ := dσ
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
      obtain ⟨q, k, σ⟩ := qk
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

def MemOk {α : Type} (s : St α) : Prop := ∀ e ∈ s.memory, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2

theorem fetchAt_mem {α : Type} (s : St α) (p : PrincipalId) (d σ : Nat) :
    (fetchAt s p d σ).memory = s.memory ∧ (fetchAt s p d σ).disk = s.disk := by
  unfold fetchAt
  cases s.shares d with
  | none => exact ⟨rfl, rfl⟩
  | some v => obtain ⟨q, k, σ'⟩ := v; simp only; split <;> exact ⟨rfl, rfl⟩

theorem step_memOk {α : Type} (s : St α) (op : Op α) (h : MemOk s) : MemOk (step s op) := by
  have es : ∀ e ∈ (endSession s).memory, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2 := by
    unfold endSession
    cases s.session with
    | none => exact h
    | some q => exact fun e he => h e (List.mem_filter.mp he).1
  have ed := (endSession_eq s).2.2.2.2.2
  have esm : ∀ (t : St α), t.memory = (endSession s).memory → t.disk = s.disk → MemOk t :=
    fun t hm hd e he => by rw [hd]; rw [hm] at he; exact es e he
  have fm : ∀ p d σ, MemOk (fetchAt s p d σ) := fun p d σ e he => by
    obtain ⟨h1, h2⟩ := fetchAt_mem s p d σ
    rw [h2]; rw [h1] at he; exact h e he
  cases op with
  | signIn p => exact esm _ rfl ed
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some dσ =>
        obtain ⟨d, σ⟩ := dσ
        cases hsh : s.shares d with
        | some v => simp only [step, hs, hd, hsh]; exact fm p d σ
        | none => simp only [step, hs, hd, hsh]; exact h
      | none => simp only [step, hs, hd]; exact h
  | fetchById d =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact h
    | some p =>
      cases hd : s.dev p with
      | some dσ => obtain ⟨d0, σ⟩ := dσ; simp only [step, hs, hd]; exact fm p d σ
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
      | some dσ => obtain ⟨d, σ⟩ := dσ; simp only [step, hs, hd]; exact esm _ rfl ed
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

/-- A key id that no browser key holds and that no secret the browser holds can fetch again. -/
def Dead {α : Type} (s : St α) (k : Nat) : Prop :=
  k < s.next ∧ (∀ q, s.keys q ≠ some k) ∧
  (∀ d q σ, s.shares d = some (q, k, σ) → ∀ q' d', s.dev q' ≠ some (d', σ))

theorem dead_shrink {α : Type} (s : St α) (k : Nat) (hd : Dead s k) (s' : St α) (hn : s'.next = s.next)
    (hk : ∀ q k, s'.keys q = some k → s.keys q = some k)
    (hs : ∀ d v, s'.shares d = some v → s.shares d = some v)
    (hdv : ∀ q v, s'.dev q = some v → s.dev q = some v) : Dead s' k := by
  obtain ⟨h1, h2, h3⟩ := hd
  exact ⟨by rw [hn]; exact h1, fun q hq => h2 q (hk q k hq),
    fun d q σ hq q' d' hq' => h3 d q σ (hs d _ hq) q' d' (hdv q' _ hq')⟩

theorem fetchAt_dead {α : Type} (s : St α) (p : PrincipalId) (d0 d σ k : Nat) (hd : Dead s k)
    (hdev : s.dev p = some (d0, σ)) : Dead (fetchAt s p d σ) k := by
  unfold fetchAt
  cases hsh : s.shares d with
  | none => exact hd
  | some v =>
    obtain ⟨q, k0, σ'⟩ := v
    simp only
    by_cases hc : q = p ∧ σ' = σ
    · obtain ⟨rfl, rfl⟩ := hc
      simp only [and_self, ↓reduceIte]
      obtain ⟨h1, h2, h3⟩ := hd
      refine ⟨h1, fun q' hq' => ?_, h3⟩
      dsimp only at hq'
      by_cases hqp : q' = q
      · subst hqp; simp at hq'; subst hq'; exact h3 d q' σ' hsh q' d0 hdev
      · rw [upd_other _ _ _ _ hqp] at hq'; exact h2 q' hq'
    · simp only [hc, ↓reduceIte]; exact hd

theorem dead_step {α : Type} (s : St α) (op : Op α) (k : Nat) (hi : Inv s) (hd : Dead s k) :
    Dead (step s op) k := by
  obtain ⟨e1, e2, e3, _, _, _⟩ := endSession_eq s
  have keep : ∀ d v, s.shares d = some v → s.shares d = some v := fun _ _ x => x
  have keepD : ∀ q v, s.dev q = some v → s.dev q = some v := fun _ _ x => x
  have esh : ∀ d v, (endSession s).shares d = some v → s.shares d = some v := fun d v hv => by
    rw [e1] at hv; exact hv
  have edv : ∀ q v, (endSession s).dev q = some v → s.dev q = some v := fun q v hv => by
    rw [e2] at hv; exact hv
  cases op with
  | signIn p => exact dead_shrink s k hd _ e3 (endSession_keys_sub s) esh edv
  | fetchKey =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      cases hdv : s.dev p with
      | some dσ =>
        obtain ⟨d, σ⟩ := dσ
        cases hsh : s.shares d with
        | some v => simp only [step, hs, hdv, hsh]; exact fetchAt_dead s p d d σ k hd hdv
        | none =>
          simp only [step, hs, hdv, hsh]
          exact dead_shrink s k hd _ rfl (fun _ _ x => x) keep (upd_none_sub s.dev p)
      | none =>
        simp only [step, hs, hdv]
        obtain ⟨h1, h2, h3⟩ := hd
        refine ⟨by dsimp only; omega, fun q' hq' => ?_, fun d q σ hq q' d' hq' => ?_⟩
        · by_cases hqp : q' = p
          · subst hqp; simp at hq'; omega
          · simp only [upd_other _ _ _ _ hqp] at hq'; exact h2 q' hq'
        · simp only [setD] at hq
          split at hq
          · simp at hq; omega
          · have hlt := hi.shares_sec_lt d q k σ hq
            by_cases hqp : q' = p
            · subst hqp; simp at hq'; omega
            · simp only [upd_other _ _ _ _ hqp] at hq'; exact h3 d q σ hq q' d' hq'
  | fetchById d =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact hd
    | some p =>
      cases hdv : s.dev p with
      | some dσ => obtain ⟨d0, σ⟩ := dσ; simp only [step, hs, hdv]; exact fetchAt_dead s p d0 d σ k hd hdv
      | none => simp only [step, hs, hdv]; exact hd
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
      | some dσ =>
        obtain ⟨d, σ⟩ := dσ
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
      obtain ⟨q, k', σ⟩ := qk
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

/-- After the browser drops `p`'s key, device id and secret, every ciphertext of `p` has a dead key. -/
theorem dead_after_drop {α : Type} (s s' : St α) (h : Inv s) (p : PrincipalId) (c : Sealed α)
    (hc : c ∈ s.disk) (ho : c.owner = p) (hn : s'.next = s.next)
    (hk : ∀ q k, s'.keys q = some k → s.keys q = some k) (hkp : s'.keys p = none)
    (hs : ∀ d v, s'.shares d = some v → s.shares d = some v)
    (hdv : ∀ q v, s'.dev q = some v → s.dev q = some v) (hdp : s'.dev p = none) : Dead s' c.key := by
  refine ⟨by rw [hn]; exact h.disk_lt c hc, fun q hq => ?_, fun d q σ hq q' d' hq' => ?_⟩
  · have := h.disk_keys c hc q (hk q _ hq)
    rw [ho] at this; subst this; rw [hkp] at hq; cases hq
  · have h1 := h.disk_shares c hc d q σ (hs d _ hq)
    have h2 := h.dev_sec q' d' σ d q c.key (hdv q' _ hq') (hs d _ hq)
    rw [ho] at h1; subst h1; subst h2; rw [hdp] at hq'; cases hq'

/-- **After sign-out with a connection, nothing the principal left at rest opens, ever again.**
    Verifies: FE-005 -/
theorem signout_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOut) ops) c := by
  apply dead_not_opens
  apply dead_run ops _ _ (step_inv s _ h)
  obtain ⟨e1, e2, e3, _, _, _⟩ := endSession_eq s
  have hk : ∀ q k, (endSession s).keys q = some k → s.keys q = some k := endSession_keys_sub s
  have hkp : (endSession s).keys p = none := by simp [endSession, hs]
  cases hd : s.dev p with
  | some dσ =>
    obtain ⟨d, σ⟩ := dσ
    simp only [step, hs, hd]
    exact dead_after_drop s _ h p c hc ho e3 hk hkp (setD_none_sub s.shares d) (upd_none_sub s.dev p) (by simp)
  | none =>
    simp only [step, hs, hd]
    exact dead_after_drop s _ h p c hc ho e3 hk hkp (by intro d v hv; simpa [e1] using hv)
      (upd_none_sub s.dev p) (by simp)

/-- **After sign-out without a connection, nothing the principal left at rest opens, ever again**:
    the share stays at the server, but no secret any browser state holds can fetch it, whatever
    device id a session of the principal names.
    Verifies: FE-005 -/
theorem offline_signout_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId)
    (hs : s.session = some p) (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOutOffline) ops) c := by
  apply dead_not_opens
  apply dead_run ops _ _ (step_inv s _ h)
  obtain ⟨e1, e2, e3, _, _, _⟩ := endSession_eq s
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

theorem fetchAt_keys_other {α : Type} (s : St α) (p q : PrincipalId) (d σ : Nat) (h : q ≠ p) :
    (fetchAt s p d σ).keys q = s.keys q := by
  unfold fetchAt
  cases s.shares d with
  | none => rfl
  | some v => obtain ⟨r, k, σ'⟩ := v; simp only; split <;> simp [upd_other _ _ _ _ h]

/-- **Only a session of `p` puts `p`'s key back.**
    Verifies: FE-006 -/
theorem unlock_needs_own_session {α : Type} (s : St α) (op : Op α) (p : PrincipalId)
    (hnone : s.keys p = none) (hsome : (step s op).keys p ≠ none) :
    (op = .fetchKey ∨ ∃ d, op = .fetchById d) ∧ s.session = some p := by
  have endNone : (endSession s).keys p = none := by
    cases h : (endSession s).keys p with
    | none => rfl
    | some k => have := endSession_keys_sub s p k h; rw [hnone] at this; cases this
  cases op with
  | signIn q => exact absurd endNone (by simpa [step] using hsome)
  | fetchKey =>
    refine ⟨Or.inl rfl, ?_⟩
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q =>
      by_cases hqp : p = q
      · rw [hqp]
      · exfalso
        cases hd : s.dev q with
        | none => simp [step, hs, hd, upd_other _ _ _ _ hqp, hnone] at hsome
        | some dσ =>
          obtain ⟨d, σ⟩ := dσ
          cases hsh : s.shares d with
          | none => simp [step, hs, hd, hsh, hnone] at hsome
          | some v =>
            simp only [step, hs, hd, hsh] at hsome
            rw [fetchAt_keys_other s q p d σ hqp, hnone] at hsome; exact hsome rfl
  | fetchById d =>
    refine ⟨Or.inr ⟨d, rfl⟩, ?_⟩
    cases hs : s.session with
    | none => simp [step, hs, hnone] at hsome
    | some q =>
      by_cases hqp : p = q
      · rw [hqp]
      · exfalso
        cases hd : s.dev q with
        | none => simp [step, hs, hd, hnone] at hsome
        | some dσ =>
          obtain ⟨d0, σ⟩ := dσ
          simp only [step, hs, hd] at hsome
          rw [fetchAt_keys_other s q p d σ hqp, hnone] at hsome; exact hsome rfl
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

/-- Expiry keeps the device id, secret and share: the same principal signing in again unlocks its
    data (the queue survives re-authentication). -/
theorem expire_then_same_principal_unlocks {α : Type} (s : St α) (p : PrincipalId) (d σ k : Nat)
    (hd : s.dev p = some (d, σ)) (hsh : s.shares d = some (p, k, σ)) :
    (run s [.expire, .signIn p, .fetchKey]).keys p = some k := by
  have e := endSession_eq s
  simp [run, step, endSession_idem, e.1, e.2.1, hd, hsh, fetchAt]

/-- **A device revoked at the server is unreadable from its next reload** (no offline key kept). -/
theorem revoke_device_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (d σ : Nat)
    (c : Sealed α) (hc : c ∈ s.disk) (hsh : s.shares d = some (p, c.key, σ)) (hoff : s.offline p = false)
    (ops : List (Op α)) : ¬ Opens (run s (.revokeDev d :: .reload :: ops)) c := by
  have ho : c.owner = p := h.disk_shares c hc d p σ hsh
  apply dead_not_opens
  show Dead (run (step (step s (.revokeDev d)) .reload) ops) c.key
  apply dead_run ops _ _ (step_inv _ _ (step_inv s _ h))
  refine ⟨h.disk_lt c hc, fun q hq => ?_, fun d' q σ' hq q' d'' _ => ?_⟩
  · by_cases hoffq : s.offline q
    · simp [step, hoffq] at hq
      have := h.disk_keys c hc q hq
      rw [ho] at this; subst this; rw [hoff] at hoffq; cases hoffq
    · simp [step, hoffq] at hq
  · simp only [step, setD] at hq
    by_cases hdd : d' = d
    · simp [hdd] at hq
    · simp [hdd] at hq
      exact hdd (h.shares_key_inj d' d q p c.key σ' σ hq hsh)

/-- **Signing out everywhere (or removing or erasing the principal) makes every device's copies
    unreadable from its next reload** (no offline key kept). -/
theorem revoke_all_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (c : Sealed α)
    (hc : c ∈ s.disk) (ho : c.owner = p) (hoff : s.offline p = false) (ops : List (Op α)) :
    ¬ Opens (run s (.revokeAll p :: .reload :: ops)) c := by
  apply dead_not_opens
  show Dead (run (step (step s (.revokeAll p)) .reload) ops) c.key
  apply dead_run ops _ _ (step_inv _ _ (step_inv s _ h))
  refine ⟨h.disk_lt c hc, fun q hq => ?_, fun d q σ hq q' _ _ => ?_⟩
  · by_cases hoffq : s.offline q
    · simp [step, hoffq] at hq
      have := h.disk_keys c hc q hq
      rw [ho] at this; subst this; rw [hoff] at hoffq; cases hoffq
    · simp [step, hoffq] at hq
  · cases hc' : s.shares d with
    | none => simp [step, hc'] at hq
    | some rk =>
      obtain ⟨r, k, σ'⟩ := rk
      by_cases hr : r = p
      · simp [step, hc', hr] at hq
      · simp [step, hc', hr] at hq
        obtain ⟨_, h2, h3⟩ := hq
        have := h.disk_shares c hc d r σ' (by rw [hc', h2])
        rw [ho] at this; exact hr this.symm

/-- With an offline key kept, a reload does not erase: the model enforces no bound on it. -/
theorem offline_key_survives_reload {α : Type} (s : St α) (p : PrincipalId) (k : Nat)
    (hk : s.keys p = some k) (hoff : s.offline p = true) : (step s .reload).keys p = some k := by
  simp [step, hoff, hk]

/-! ## Rotation

A partition's records are encrypted under the old share's key or the new one's. WebCrypto work
cannot run inside an IndexedDB transaction (the transaction commits when it goes idle), so every
change is computed outside a transaction and then **verified and written in one short
transaction**: an append or rewrite names the key id it encrypted under and is refused unless that
is the partition's current key id; a re-encryption names the version of the record it read and is
refused unless the record is still at that version (compare-and-swap). The order: the new share is
made durable (`begin`); the current key id switches to the new key (`switch`); each old record is
read (`rread`, outside a transaction) and written back under the new key (`rwrite`, the
compare-and-swap); the old share is deleted only when the current key is new and no record is left
under the old one (`finish`). A crash can stop the schedule anywhere, other tabs append and rewrite
at any time, and a tab may still hold the old key id when it writes. -/

structure RRec where
  newKey : Bool
  val : Nat
  ver : Nat
  deriving DecidableEq

structure Rot where
  recs : List RRec
  oldShare : Bool
  newShare : Bool
  /-- The partition's current key id: `true` once writers must use the new key. -/
  cur : Bool
  /-- Reads the rotating tab made outside a transaction: slot, value, version. -/
  reads : List (Nat × Nat × Nat)

inductive ROp where
  | begin
  | switch
  /-- Another tab appends a value it encrypted under key id `kw`. -/
  | append (kw : Bool) (v : Nat)
  /-- Another tab rewrites slot `i` with a value it encrypted under key id `kw`. -/
  | write (i : Nat) (kw : Bool) (v : Nat)
  /-- The rotating tab reads slot `i`, outside a transaction. -/
  | rread (i : Nat)
  /-- The rotating tab writes back its `j`-th read under the new key, if the record is unchanged. -/
  | rwrite (j : Nat)
  | finish

def rstep (r : Rot) : ROp → Rot
  | .begin => { r with newShare := true }
  | .switch => if r.newShare then { r with cur := true } else r
  | .append kw v => if kw = r.cur then { r with recs := r.recs ++ [⟨kw, v, 0⟩] } else r
  | .write i kw v =>
    match r.recs[i]? with
    | some old => if kw = r.cur then { r with recs := r.recs.set i ⟨kw, v, old.ver + 1⟩ } else r
    | none => r
  | .rread i =>
    match r.recs[i]? with
    | some old => { r with reads := r.reads ++ [(i, old.val, old.ver)] }
    | none => r
  | .rwrite j =>
    match r.reads[j]? with
    | some (i, v, ver) =>
      match r.recs[i]? with
      | some old => if r.newShare ∧ old.ver = ver then { r with recs := r.recs.set i ⟨true, v, ver + 1⟩ } else r
      | none => r
    | none => r
  | .finish => if r.cur && r.recs.all (fun x => x.newKey) then { r with oldShare := false } else r

def rrun : Rot → List ROp → Rot
  | r, [] => r
  | r, op :: ops => rrun (rstep r op) ops

/-- Every record's key is held, and the current key is held. -/
def ROk (r : Rot) : Prop :=
  (∀ x ∈ r.recs, (x.newKey = true → r.newShare = true) ∧ (x.newKey = false → r.oldShare = true)) ∧
  (r.cur = true → r.newShare = true) ∧ (r.cur = false → r.oldShare = true)

/-- A read made outside a transaction names a slot that exists, and still describes its record
    whenever the record is at the version read. -/
def ReadOk (r : Rot) : Prop :=
  ∀ p ∈ r.reads, p.1 < r.recs.length ∧
    ∀ x, r.recs[p.1]? = some x → p.2.2 ≤ x.ver ∧ (x.ver = p.2.2 → x.val = p.2.1)

theorem rstep_ok (r : Rot) (op : ROp) (h : ROk r) : ROk (rstep r op) := by
  obtain ⟨hd, hw1, hw2⟩ := h
  have keyOk : ∀ kw, kw = r.cur → (kw = true → r.newShare = true) ∧ (kw = false → r.oldShare = true) :=
    fun kw hk => by subst hk; exact ⟨hw1, hw2⟩
  cases op with
  | begin => exact ⟨fun x hx => ⟨fun _ => rfl, (hd x hx).2⟩, fun _ => rfl, hw2⟩
  | switch =>
    simp only [rstep]
    split
    · rename_i hn; exact ⟨hd, fun _ => hn, fun h => by cases h⟩
    · exact ⟨hd, hw1, hw2⟩
  | append kw v =>
    simp only [rstep]
    split
    · rename_i hk
      refine ⟨fun x hx => ?_, hw1, hw2⟩
      rcases List.mem_append.mp hx with hx | hx
      · exact hd x hx
      · simp at hx; subst hx; exact keyOk kw hk
    · exact ⟨hd, hw1, hw2⟩
  | write i kw v =>
    simp only [rstep]
    split
    · split
      · rename_i hk
        refine ⟨fun x hx => ?_, hw1, hw2⟩
        rcases List.mem_or_eq_of_mem_set hx with hx | hx
        · exact hd x hx
        · subst hx; exact keyOk kw hk
      · exact ⟨hd, hw1, hw2⟩
    · exact ⟨hd, hw1, hw2⟩
  | rread i =>
    simp only [rstep]
    split
    · exact ⟨hd, hw1, hw2⟩
    · exact ⟨hd, hw1, hw2⟩
  | rwrite j =>
    simp only [rstep]
    split
    · split
      · split
        · rename_i hc
          refine ⟨fun x hx => ?_, hw1, hw2⟩
          rcases List.mem_or_eq_of_mem_set hx with hx | hx
          · exact hd x hx
          · subst hx; exact ⟨fun _ => hc.1, fun h => by cases h⟩
        · exact ⟨hd, hw1, hw2⟩
      · exact ⟨hd, hw1, hw2⟩
    · exact ⟨hd, hw1, hw2⟩
  | finish =>
    simp only [rstep]
    split
    · rename_i hc
      simp only [Bool.and_eq_true] at hc
      obtain ⟨hcur, hall⟩ := hc
      refine ⟨fun x hx => ⟨(hd x hx).1, fun hf => ?_⟩, hw1, fun hc' => ?_⟩
      · have := List.all_eq_true.mp hall x hx; rw [hf] at this; cases this
      · rw [hcur] at hc'; cases hc'
    · exact ⟨hd, hw1, hw2⟩

theorem get_lt {α : Type} (l : List α) (i : Nat) (a : α) (h : l[i]? = some a) : i < l.length :=
  (List.getElem?_eq_some_iff.mp h).1

theorem get_mem {α : Type} (l : List α) (i : Nat) (a : α) (h : l[i]? = some a) : a ∈ l := by
  obtain ⟨hi, he⟩ := List.getElem?_eq_some_iff.mp h
  rw [← he]; exact List.getElem_mem hi

theorem set_get (l : List RRec) (i j : Nat) (a x : RRec) (h : (l.set i a)[j]? = some x) :
    (i = j ∧ x = a) ∨ (i ≠ j ∧ l[j]? = some x) := by
  rw [List.getElem?_set] at h
  by_cases hij : i = j
  · subst hij
    left
    simp only [↓reduceIte] at h
    split at h
    · simp at h; exact ⟨rfl, h.symm⟩
    · simp at h
  · right
    simp only [hij, ↓reduceIte] at h
    exact ⟨hij, h⟩

/-- Rewriting slot `i` with a higher version keeps every read valid. -/
theorem reads_set (r : Rot) (h : ReadOk r) (i : Nat) (old a : RRec) (hold : r.recs[i]? = some old)
    (hv : old.ver < a.ver) : ReadOk { r with recs := r.recs.set i a } := by
  intro p hp
  obtain ⟨hlt, hx⟩ := h p hp
  refine ⟨by simp only [List.length_set]; exact hlt, fun x hgx => ?_⟩
  rcases set_get r.recs i p.1 a x hgx with ⟨hij, rfl⟩ | ⟨_, hx'⟩
  · subst hij
    have := (hx old hold).1
    exact ⟨by omega, fun h' => by omega⟩
  · exact hx x hx'

theorem rstep_reads (r : Rot) (op : ROp) (h : ReadOk r) : ReadOk (rstep r op) := by
  cases op with
  | begin => exact h
  | switch => simp only [rstep]; split <;> exact h
  | append kw v =>
    simp only [rstep]
    split
    · intro p hp
      obtain ⟨hlt, hx⟩ := h p hp
      refine ⟨by simp only [List.length_append]; omega, fun x hgx => ?_⟩
      rw [List.getElem?_append_left hlt] at hgx
      exact hx x hgx
    · exact h
  | write i kw v =>
    simp only [rstep]
    split
    · rename_i old hold
      split
      · exact reads_set r h i old _ hold (by simp only; omega)
      · exact h
    · exact h
  | rread i =>
    simp only [rstep]
    split
    · rename_i old hold
      intro p hp
      rcases List.mem_append.mp hp with hp | hp
      · exact h p hp
      · simp at hp
        subst hp
        refine ⟨get_lt r.recs i old hold, fun x hgx => ?_⟩
        rw [hold] at hgx; cases hgx
        exact ⟨Nat.le_refl _, fun _ => rfl⟩
    · exact h
  | rwrite j =>
    simp only [rstep]
    split
    · split
      · rename_i old hold
        split
        · rename_i hc
          exact reads_set r h _ old _ hold (by simp only; omega)
        · exact h
      · exact h
    · exact h
  | finish => simp only [rstep]; split <;> exact h

/-- **Rotation loses no record**: at every point of every schedule (a crash stops it anywhere,
    other tabs append and rewrite under whatever key id they hold), every record's key is held.
    Verifies: FE-013 -/
theorem rot_no_loss (ops : List ROp) : ∀ r, ROk r → ROk (rrun r ops) := by
  induction ops with
  | nil => intro r h; exact h
  | cons op ops ih => intro r h; exact ih _ (rstep_ok r op h)

theorem rot_reads (ops : List ROp) : ∀ r, ReadOk r → ReadOk (rrun r ops) := by
  induction ops with
  | nil => intro r h; exact h
  | cons op ops ih => intro r h; exact ih _ (rstep_reads r op h)

/-- **A re-encryption never loses an update**: writing back a read changes no record's value,
    whatever other tabs wrote since the read.
    Verifies: FE-013 -/
theorem rot_no_lost_update (r : Rot) (h : ReadOk r) (j i : Nat) (x : RRec)
    (hx : (rstep r (.rwrite j)).recs[i]? = some x) : ∃ y, r.recs[i]? = some y ∧ x.val = y.val := by
  simp only [rstep] at hx
  split at hx
  · rename_i i0 v ver hread
    split at hx
    · rename_i old hold
      split at hx
      · rename_i hc
        rcases set_get r.recs i0 i _ x hx with ⟨hij, rfl⟩ | ⟨_, hx'⟩
        · subst hij
          have hmem := get_mem r.reads j _ hread
          have := ((h _ hmem).2 old hold).2 hc.2
          exact ⟨old, hold, this.symm⟩
        · exact ⟨x, hx', rfl⟩
      · exact ⟨x, hx, rfl⟩
    · exact ⟨x, hx, rfl⟩
  · exact ⟨x, hx, rfl⟩

/-- **Finishing retires the old key**: once `finish` takes effect, the current key is new, no record
    is under the old key, and the old share is deleted.
    Verifies: FE-013 -/
theorem rot_finish_retires (r : Rot) (hc : r.cur = true) (hall : r.recs.all (fun x => x.newKey) = true) :
    (rstep r .finish).oldShare = false ∧ ∀ x ∈ (rstep r .finish).recs, x.newKey = true := by
  simp only [rstep, hc, hall, Bool.and_self, ↓reduceIte]
  refine ⟨trivial, fun x hx => ?_⟩
  have := List.all_eq_true.mp hall x hx
  simpa using this

/-- **Without the key-id check, a stale tab breaks rotation**: after `begin`, `switch` and `finish`,
    a tab that still holds the old key id appends, and its record's key is gone. -/
def rotStaleAppend (r : Rot) (v : Nat) : Rot := { r with recs := r.recs ++ [⟨false, v, 0⟩] }

def rot0 : Rot := { recs := [], oldShare := true, newShare := false, cur := false, reads := [] }

theorem rot_stale_append_breaks :
    ¬ ROk (rotStaleAppend (rrun rot0 [.begin, .switch, .finish]) 7) := by
  intro h
  have := (h.1 ⟨false, 7, 0⟩ (by simp [rotStaleAppend, rrun, rstep, rot0])).2 rfl
  simp [rrun, rstep, rot0, rotStaleAppend] at this

/-- With the check, the same stale append is refused. -/
theorem rot_stale_append_refused :
    (rstep (rrun rot0 [.begin, .switch, .finish]) (.append false 7)).recs = [] := by
  decide

/-- **Without compare-and-swap, a re-encryption loses an update**: the rotating tab reads 1, another
    tab rewrites it to 2 under the new key, and the blind write-back restores 1. -/
def rwriteBlind (r : Rot) (i v ver : Nat) : Rot := { r with recs := r.recs.set i ⟨true, v, ver + 1⟩ }

def rot1 : Rot := { recs := [⟨false, 1, 0⟩], oldShare := true, newShare := false, cur := false, reads := [] }

theorem rot_blind_write_loses_update :
    (rwriteBlind (rrun rot1 [.begin, .switch, .rread 0, .write 0 true 2]) 0 1 0).recs[0]?.map (·.val) = some 1 ∧
    (rrun rot1 [.begin, .switch, .rread 0, .write 0 true 2]).recs[0]?.map (·.val) = some 2 := by
  decide

/-- With compare-and-swap the write-back is refused and the update stays. -/
theorem rot_cas_keeps_update :
    (rrun rot1 [.begin, .switch, .rread 0, .write 0 true 2, .rwrite 0]).recs[0]?.map (·.val) = some 2 := by
  decide

end Strata.Client.Privacy
