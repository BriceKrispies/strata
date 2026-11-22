/-
What the browser keeps at rest, under whose key, and when it stops being readable
(docs/frontend.md, "Privacy at rest").

Everything a principal's partition holds at rest (view results, the write queue, the hot-set
manifest) is encrypted under a data key derived from a **key share** the server holds for that
principal on that device. The server gives the share only to a session of that principal. The
browser holds the derived key as a non-extractable WebCrypto key in the worker's memory, and also
in IndexedDB only when the person has chosen to keep their work available offline on that device.

The attacker model is deliberately strong: **any code of the origin may use any key the browser
holds** (a non-extractable key cannot be exported, but it can be used). So separation between
principals is not left to the code that chooses which partition to open; it rests on which keys
are on the device (`Opens`).

Proved:

- **one principal's data never opens for another** (`no_cross_principal`, `served_own`): a
  ciphertext opens only with its owner's key, and the browser only ever holds a principal's key
  after that principal's session fetched it;
- **after sign-out, no cached plaintext is readable, ever again** (`signout_erases`): sign-out
  deletes the share at the server and the key in the browser, key ids are never reused, so every
  ciphertext the principal left behind stays unreadable in every later state, whoever signs in,
  whatever the code does, whether or not deleting the ciphertexts themselves completed;
- **session expiry locks** (`expire_locks`, `unlock_needs_own_session`): after expiry nothing of
  the principal opens until a session of that same principal fetches the share again (the share
  is kept, so work queued offline survives re-authentication);
- **revocation elsewhere reaches a device at its next reload** (`revoke_reload_erases`): a share
  deleted at the server (sign-out of all sessions, the principal removed, its erasure) makes the
  device's ciphertexts unreadable once its in-memory key is gone, unless the person kept an
  offline key on that device, whose window is the bound stated in the design.

Assumptions, all of them:

P1. Encryption is ideal: a ciphertext opens only with the key it was made with (AES-GCM with a
    random nonce, authenticated).
P2. The server gives a principal's share only to a session of that principal, and draws every new
    share id fresh (`next`). Deleting a share at the server is final.
P3. Deleting a key from the browser (memory, and IndexedDB record of an offline key) removes it.
    Disk forensics of deleted IndexedDB records are outside the model; the design's answer is that
    an offline key is persisted only on a device the person marks as theirs.

No `sorry`, no added axioms.
-/

import Strata.Model

namespace Strata.Client.Privacy

open Strata

/-- Key ids are plain numbers, so arithmetic tactics see them. -/
abbrev KeyId := Nat

/-- A ciphertext at rest. `owner` is ghost bookkeeping for the statements; opening reads only
    `key` (P1). -/
structure Sealed (α : Type) where
  owner : PrincipalId
  key : Nat
  plain : α

structure St (α : Type) where
  /-- Server: the key share of each principal for this device. -/
  shares : PrincipalId → Option Nat
  /-- Server: the next fresh share id. -/
  next : Nat
  /-- Device: who the session cookie belongs to. -/
  session : Option PrincipalId
  /-- Device: the keys the browser holds (worker memory, and IndexedDB when offline is kept). -/
  keys : PrincipalId → Option Nat
  /-- Device: whether the person kept an offline key on this device (it survives a reload). -/
  offline : PrincipalId → Bool
  /-- Device: every ciphertext at rest, of every partition. -/
  disk : List (Sealed α)
  /-- Device: the worker's memory hot set, by owner. -/
  memory : List (PrincipalId × α)

def upd {β : Type} (f : PrincipalId → β) (p : PrincipalId) (b : β) : PrincipalId → β :=
  fun q => if q = p then b else f q

@[simp] theorem upd_same {β : Type} (f : PrincipalId → β) (p : PrincipalId) (b : β) :
    upd f p b p = b := by simp [upd]

theorem upd_other {β : Type} (f : PrincipalId → β) (p q : PrincipalId) (b : β) (h : q ≠ p) :
    upd f p b q = f q := by simp [upd, h]

inductive Op (α : Type) where
  /-- A successful authentication; it ends the session the browser held before (IDN-011). -/
  | signIn (p : PrincipalId)
  /-- The session's principal asks for its share and derives its key. -/
  | fetchKey
  /-- The session writes a value to its partition (a view result, a queued write). -/
  | write (v : α)
  /-- The person keeps their work available offline on this device. -/
  | keepOffline
  /-- Sign-out on this device. Ciphertexts are left in place: the statements assume deleting
      them did not happen (a crash, a full disk). -/
  | signOut
  /-- The session expires (seen by the browser, or the server's answer says so). -/
  | expire
  /-- The server deletes a principal's share without this device's knowledge: sign-out of all
      sessions from another device, the principal removed, its erasure. -/
  | revoke (p : PrincipalId)
  /-- The page reloads, the worker restarts, the browser crashes: memory is gone, an offline key
      stays. -/
  | reload

/-- The session ends on the device: its key and hot set go, its share stays at the server. -/
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
      match s.shares p with
      | some k => { s with keys := upd s.keys p (some k) }
      | none => { s with shares := upd s.shares p (some s.next), keys := upd s.keys p (some s.next),
                         next := s.next + 1 }
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
    | some p => { endSession s with shares := upd s.shares p none }
    | none => s
  | .expire => endSession s
  | .revoke p => { s with shares := upd s.shares p none }
  | .reload => { s with memory := [], keys := fun q => if s.offline q then s.keys q else none }

def run {α : Type} : St α → List (Op α) → St α
  | s, [] => s
  | s, op :: ops => run (step s op) ops

/-- A ciphertext opens on the device: some key the browser holds is its key. Any code of the
    origin may try any key, so this is what can be read, not what the application chooses to read. -/
def Opens {α : Type} (s : St α) (c : Sealed α) : Prop := ∃ q, s.keys q = some c.key

/-- What a read in a session of `q` can return: its hot set, and ciphertexts it can open. -/
def Served {α : Type} (s : St α) (q : PrincipalId) (x : α) : Prop :=
  (q, x) ∈ s.memory ∨ ∃ c ∈ s.disk, s.keys q = some c.key ∧ c.plain = x

/-! ## The invariant -/

structure Inv {α : Type} (s : St α) : Prop where
  keys_lt : ∀ q k, s.keys q = some k → k < s.next
  shares_lt : ∀ q k, s.shares q = some k → k < s.next
  disk_lt : ∀ c ∈ s.disk, c.key < s.next
  disk_keys : ∀ c ∈ s.disk, ∀ q, s.keys q = some c.key → c.owner = q
  disk_shares : ∀ c ∈ s.disk, ∀ q, s.shares q = some c.key → c.owner = q
  keys_shares : ∀ p q k, s.keys p = some k → s.shares q = some k → p = q
  shares_inj : ∀ p q k, s.shares p = some k → s.shares q = some k → p = q
  keys_inj : ∀ p q k, s.keys p = some k → s.keys q = some k → p = q

def init {α : Type} : St α :=
  { shares := fun _ => none, next := 0, session := none, keys := fun _ => none,
    offline := fun _ => false, disk := [], memory := [] }

theorem inv_init {α : Type} : Inv (init : St α) := by
  constructor <;> intros <;> simp_all [init]

/-- Removing keys from the browser keeps the invariant. -/
theorem inv_fewer_keys {α : Type} (s : St α) (keys' : PrincipalId → Option Nat)
    (hk : ∀ q k, keys' q = some k → s.keys q = some k) (h : Inv s) (mem : List (PrincipalId × α))
    (sess : Option PrincipalId) (off : PrincipalId → Bool) :
    Inv { s with keys := keys', memory := mem, session := sess, offline := off } := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
  exact ⟨fun q k hq => h1 q k (hk q k hq), h2, h3, fun c hc q hq => h4 c hc q (hk q _ hq), h5,
    fun p q k hp hq => h6 p q k (hk p k hp) hq, h7, fun p q k hp hq => h8 p q k (hk p k hp) (hk q k hq)⟩

theorem upd_none_sub (keys : PrincipalId → Option Nat) (p : PrincipalId) :
    ∀ q k, upd keys p none q = some k → keys q = some k := by
  intro q k h
  by_cases hq : q = p
  · subst hq; simp at h
  · rwa [upd_other _ _ _ _ hq] at h

theorem endSession_inv {α : Type} (s : St α) (h : Inv s) : Inv (endSession s) := by
  unfold endSession
  cases hs : s.session with
  | none => exact h
  | some p => exact inv_fewer_keys s _ (upd_none_sub s.keys p) h _ _ _

theorem endSession_shares {α : Type} (s : St α) : (endSession s).shares = s.shares := by
  unfold endSession; cases s.session <;> rfl

theorem endSession_next {α : Type} (s : St α) : (endSession s).next = s.next := by
  unfold endSession; cases s.session <;> rfl

theorem endSession_disk {α : Type} (s : St α) : (endSession s).disk = s.disk := by
  unfold endSession; cases s.session <;> rfl

theorem endSession_keys_sub {α : Type} (s : St α) :
    ∀ q k, (endSession s).keys q = some k → s.keys q = some k := by
  unfold endSession
  cases s.session with
  | none => intro q k h; exact h
  | some p => exact upd_none_sub s.keys p

/-- Removing a share at the server keeps the invariant. -/
theorem inv_drop_share {α : Type} (s : St α) (p : PrincipalId) (h : Inv s) :
    Inv { s with shares := upd s.shares p none } := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
  have hs := upd_none_sub s.shares p
  exact ⟨h1, fun q k hq => h2 q k (hs q k hq), h3, h4, fun c hc q hq => h5 c hc q (hs q _ hq),
    fun a b k ha hb => h6 a b k ha (hs b k hb), fun a b k ha hb => h7 a b k (hs a k ha) (hs b k hb), h8⟩

theorem inv_setKey {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (k : Nat)
    (hsh : s.shares p = some k) (sess : Option PrincipalId) :
    Inv ({ shares := s.shares, next := s.next, session := sess, keys := upd s.keys p (some k),
           offline := s.offline, disk := s.disk, memory := s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
  constructor <;> dsimp only
  · intro q k' hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; subst hq; exact h2 _ _ hsh
    · simp only [upd_other _ _ _ _ hqp] at hq; exact h1 q k' hq
  · exact h2
  · exact h3
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; exact h5 c hc q (by rw [hsh, hq])
    · simp only [upd_other _ _ _ _ hqp] at hq; exact h4 c hc q hq
  · exact h5
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha; exact h7 a b k hsh hb
    · simp only [upd_other _ _ _ _ hap] at ha; exact h6 a b k' ha hb
  · exact h7
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · simp only [upd_other _ _ _ _ hbp] at hb; exact (h6 b a k hb hsh).symm
    · simp only [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; subst hb; first | exact h6 a _ _ ha hsh | simp_all
      · simp only [upd_other _ _ _ _ hbp] at hb; exact h8 a b k' ha hb

theorem inv_fresh {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (sess : Option PrincipalId) :
    Inv ({ shares := upd s.shares p (some s.next), next := s.next + 1, session := sess,
           keys := upd s.keys p (some s.next), offline := s.offline, disk := s.disk,
           memory := s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
  constructor <;> dsimp only
  · intro q k' hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · simp only [upd_other _ _ _ _ hqp] at hq; have := h1 q k' hq; omega
  · intro q k' hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; omega
    · simp only [upd_other _ _ _ _ hqp] at hq; have := h2 q k' hq; omega
  · intro c hc; have := h3 c hc; omega
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; have := h3 c hc; omega
    · simp only [upd_other _ _ _ _ hqp] at hq; exact h4 c hc q hq
  · intro c hc q hq
    by_cases hqp : q = p
    · subst hqp; simp at hq; have := h3 c hc; omega
    · simp only [upd_other _ _ _ _ hqp] at hq; exact h5 c hc q hq
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · simp only [upd_other _ _ _ _ hbp] at hb; have := h2 b _ hb; omega
    · simp only [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; have := h1 a _ ha; omega
      · simp only [upd_other _ _ _ _ hbp] at hb; exact h6 a b k' ha hb
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · simp only [upd_other _ _ _ _ hbp] at hb; have := h2 b _ hb; omega
    · simp only [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; have := h2 a _ ha; omega
      · simp only [upd_other _ _ _ _ hbp] at hb; exact h7 a b k' ha hb
  · intro a b k' ha hb
    by_cases hap : a = p
    · subst hap; simp at ha; subst ha
      by_cases hbp : b = a
      · exact hbp.symm
      · simp only [upd_other _ _ _ _ hbp] at hb; have := h1 b _ hb; omega
    · simp only [upd_other _ _ _ _ hap] at ha
      by_cases hbp : b = p
      · subst hbp; simp at hb; have := h1 a _ ha; omega
      · simp only [upd_other _ _ _ _ hbp] at hb; exact h8 a b k' ha hb

theorem inv_write {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (k : Nat) (v : α)
    (hk : s.keys p = some k) (sess : Option PrincipalId) :
    Inv ({ shares := s.shares, next := s.next, session := sess, keys := s.keys,
           offline := s.offline, disk := ⟨p, k, v⟩ :: s.disk, memory := (p, v) :: s.memory } : St α) := by
  obtain ⟨h1, h2, h3, h4, h5, h6, h7, h8⟩ := h
  constructor <;> dsimp only
  · exact h1
  · exact h2
  · intro c hc
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h1 p k hk
    · exact h3 c hc
  · intro c hc q hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h8 p q k hk hq
    · exact h4 c hc q hq
  · intro c hc q hq
    rcases List.mem_cons.mp hc with hc | hc
    · subst hc; exact h6 p q k hk hq
    · exact h5 c hc q hq
  · exact h6
  · exact h7
  · exact h8

theorem step_inv {α : Type} (s : St α) (op : Op α) (h : Inv s) : Inv (step s op) := by
  cases op with
  | signIn p =>
    exact inv_fewer_keys (endSession s) _ (fun _ _ hq => hq) (endSession_inv s h) _ _ _
  | fetchKey =>
    simp only [step]
    cases hs : s.session with
    | none => exact h
    | some p =>
      simp only
      cases hsh : s.shares p with
      | some k => exact inv_setKey s h p k hsh _
      | none => exact inv_fresh s h p _
  | write v =>
    simp only [step]
    cases hs : s.session with
    | none => exact h
    | some p =>
      simp only
      cases hk : s.keys p with
      | none => exact h
      | some k => exact inv_write s h p k v hk _
  | keepOffline =>
    simp only [step]
    cases s.session with
    | none => exact h
    | some p => exact inv_fewer_keys s s.keys (fun _ _ hq => hq) h _ _ _
  | signOut =>
    simp only [step]
    cases hs : s.session with
    | none => exact h
    | some p =>
      have he := endSession_inv s h
      have := inv_drop_share (endSession s) p he
      simpa [endSession_shares] using this
  | expire => exact endSession_inv s h
  | revoke p => exact inv_drop_share s p h
  | reload =>
    exact inv_fewer_keys s _ (fun q k hq => by
      split at hq
      · exact hq
      · cases hq) h _ _ _

theorem run_inv {α : Type} (ops : List (Op α)) : ∀ s : St α, Inv s → Inv (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih => intro s h; exact ih _ (step_inv s op h)

theorem reachable_inv {α : Type} (ops : List (Op α)) : Inv (run (init : St α) ops) :=
  run_inv ops _ inv_init

/-! ## Separation between principals -/

/-- **A ciphertext opens only with its owner's key**: whatever code runs, a key the browser holds
    for `q` opens only `q`'s data.
    Verifies: FE-004 -/
theorem no_cross_principal {α : Type} (s : St α) (h : Inv s) (c : Sealed α) (hc : c ∈ s.disk)
    (q : PrincipalId) (hq : s.keys q = some c.key) : c.owner = q :=
  h.disk_keys c hc q hq

/-- The hot set only ever holds values written by their owner. -/
def MemOk {α : Type} (s : St α) : Prop := ∀ e ∈ s.memory, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2

theorem step_memOk {α : Type} (s : St α) (op : Op α) (h : MemOk s) : MemOk (step s op) := by
  have hf : ∀ (p : PrincipalId) (m : List (PrincipalId × α)), (∀ e ∈ m, e ∈ s.memory) →
      ∀ e ∈ m, ∃ c ∈ s.disk, c.owner = e.1 ∧ c.plain = e.2 := fun _ m hm e he => h e (hm e he)
  cases op with
  | signIn p =>
    simp only [step, endSession]
    cases s.session with
    | none => exact h
    | some q => exact hf q _ (fun e he => (List.mem_filter.mp he).1)
  | fetchKey =>
    simp only [step]
    cases s.session with
    | none => exact h
    | some p =>
      simp only
      cases s.shares p <;> exact h
  | write v =>
    simp only [step]
    cases s.session with
    | none => exact h
    | some p =>
      simp only
      cases hk : s.keys p with
      | none => exact h
      | some k =>
        intro e he
        rcases List.mem_cons.mp he with he | he
        · subst he; exact ⟨⟨p, k, v⟩, List.mem_cons_self .., rfl, rfl⟩
        · obtain ⟨c, hc, h1, h2⟩ := h e he
          exact ⟨c, List.mem_cons_of_mem _ hc, h1, h2⟩
  | keepOffline =>
    simp only [step]
    cases s.session with
    | none => exact h
    | some p => exact h
  | signOut =>
    simp only [step, endSession]
    cases s.session with
    | none => exact h
    | some q => exact hf q _ (fun e he => (List.mem_filter.mp he).1)
  | expire =>
    simp only [step, endSession]
    cases s.session with
    | none => exact h
    | some q => exact hf q _ (fun e he => (List.mem_filter.mp he).1)
  | revoke p => exact h
  | reload => intro e he; cases he

theorem run_memOk {α : Type} (ops : List (Op α)) : ∀ s : St α, MemOk s → MemOk (run s ops) := by
  induction ops with
  | nil => intro s h; exact h
  | cons op ops ih => intro s h; exact ih _ (step_memOk s op h)

/-- **What a session of `q` can be served was written by `q`**: from the hot set or from disk,
    nothing of another principal.
    Verifies: FE-004 -/
theorem served_own {α : Type} (s : St α) (h : Inv s) (hm : MemOk s) (q : PrincipalId) (x : α)
    (hx : Served s q x) : ∃ c ∈ s.disk, c.owner = q ∧ c.plain = x := by
  rcases hx with hx | ⟨c, hc, hk, hp⟩
  · exact hm _ hx
  · exact ⟨c, hc, h.disk_keys c hc q hk, hp⟩

/-! ## Sign-out, expiry and revocation -/

/-- A key id that nothing holds and nothing can be given again. -/
def Dead {α : Type} (s : St α) (k : Nat) : Prop :=
  k < s.next ∧ (∀ q, s.keys q ≠ some k) ∧ (∀ q, s.shares q ≠ some k)

theorem dead_step {α : Type} (s : St α) (op : Op α) (k : Nat) (hd : Dead s k) :
    Dead (step s op) k := by
  obtain ⟨hlt, hkeys, hshares⟩ := hd
  have sub : ∀ (keys' : PrincipalId → Option Nat), (∀ q k', keys' q = some k' → s.keys q = some k') →
      ∀ q, keys' q ≠ some k := fun keys' hs q hq => hkeys q (hs q k hq)
  have subS : ∀ (sh : PrincipalId → Option Nat), (∀ q k', sh q = some k' → s.shares q = some k') →
      ∀ q, sh q ≠ some k := fun sh hs q hq => hshares q (hs q k hq)
  cases op with
  | signIn p =>
    refine ⟨by simpa [step, endSession_next] using hlt, ?_, ?_⟩
    · exact sub _ (fun q k' hq => endSession_keys_sub s q k' hq)
    · simpa [step, endSession_shares] using hshares
  | fetchKey =>
    simp only [step]
    cases hs : s.session with
    | none => exact ⟨hlt, hkeys, hshares⟩
    | some p =>
      simp only
      cases hsh : s.shares p with
      | some k0 =>
        refine ⟨hlt, fun q hq => ?_, hshares⟩
        dsimp only at hq
        by_cases hqp : q = p
        · subst hqp; simp at hq; subst hq; exact hshares q hsh
        · simp only [upd_other _ _ _ _ hqp] at hq; exact hkeys q hq
      | none =>
        refine ⟨by dsimp only; omega, fun q hq => ?_, fun q hq => ?_⟩
        · dsimp only at hq
          by_cases hqp : q = p
          · subst hqp; simp at hq; omega
          · simp only [upd_other _ _ _ _ hqp] at hq; exact hkeys q hq
        · dsimp only at hq
          by_cases hqp : q = p
          · subst hqp; simp at hq; omega
          · simp only [upd_other _ _ _ _ hqp] at hq; exact hshares q hq
  | write v =>
    simp only [step]
    cases s.session with
    | none => exact ⟨hlt, hkeys, hshares⟩
    | some p =>
      simp only
      cases s.keys p with
      | none => exact ⟨hlt, hkeys, hshares⟩
      | some k0 => exact ⟨hlt, hkeys, hshares⟩
  | keepOffline =>
    simp only [step]
    cases s.session with
    | none => exact ⟨hlt, hkeys, hshares⟩
    | some p => exact ⟨hlt, hkeys, hshares⟩
  | signOut =>
    cases hs : s.session with
    | none => simp only [step, hs]; exact ⟨hlt, hkeys, hshares⟩
    | some p =>
      simp only [step, hs]
      refine ⟨by simpa [endSession_next] using hlt, ?_, ?_⟩
      · exact sub _ (fun q k' hq => endSession_keys_sub s q k' hq)
      · exact subS _ (fun q k' hq => upd_none_sub s.shares p q k' hq)
  | expire =>
    exact ⟨by simpa [step, endSession_next] using hlt, sub _ (fun q k' hq => endSession_keys_sub s q k' hq),
      by simpa [step, endSession_shares] using hshares⟩
  | revoke p =>
    exact ⟨hlt, hkeys, subS _ (upd_none_sub s.shares p)⟩
  | reload =>
    refine ⟨hlt, sub _ (fun q k' hq => ?_), hshares⟩
    simp only [step] at hq
    split at hq
    · exact hq
    · cases hq

theorem dead_run {α : Type} (ops : List (Op α)) : ∀ (s : St α) (k : Nat), Dead s k → Dead (run s ops) k := by
  induction ops with
  | nil => intro s k h; exact h
  | cons op ops ih => intro s k h; exact ih _ k (dead_step s op k h)

theorem dead_not_opens {α : Type} (s : St α) (c : Sealed α) (hd : Dead s c.key) : ¬ Opens s c := by
  rintro ⟨q, hq⟩
  exact hd.2.1 q hq

/-- **After sign-out, no cached plaintext is readable, ever again.** Every ciphertext the principal
    left at rest stays unopenable in every state reachable afterwards by any operations (another
    principal signing in, the same principal signing in again, reloads, any code using any key),
    even though sign-out deleted none of the ciphertexts.
    Verifies: FE-005 -/
theorem signout_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) (ops : List (Op α)) :
    ¬ Opens (run (step s .signOut) ops) c := by
  apply dead_not_opens
  apply dead_run
  have hlt := h.disk_lt c hc
  refine ⟨?_, fun q hq => ?_, fun q hq => ?_⟩
  · simp only [step, hs, endSession_next]; exact hlt
  · simp only [step, hs, endSession] at hq
    by_cases hqp : q = p
    · subst hqp; simp at hq
    · simp only [upd_other _ _ _ _ hqp] at hq
      exact hqp ((h.disk_keys c hc q hq).symm.trans ho)
  · simp only [step, hs, endSession] at hq
    by_cases hqp : q = p
    · subst hqp; simp at hq
    · simp only [upd_other _ _ _ _ hqp] at hq
      exact hqp ((h.disk_shares c hc q hq).symm.trans ho)

/-- **After the session expires, nothing of its principal opens** in the browser.
    Verifies: FE-006 -/
theorem expire_locks {α : Type} (s : St α) (h : Inv s) (p : PrincipalId) (hs : s.session = some p)
    (c : Sealed α) (hc : c ∈ s.disk) (ho : c.owner = p) : ¬ Opens (step s .expire) c := by
  rintro ⟨q, hq⟩
  simp only [step, endSession, hs] at hq
  by_cases hqp : q = p
  · subst hqp; simp at hq
  · simp only [upd_other _ _ _ _ hqp] at hq
    exact hqp ((h.disk_keys c hc q hq).symm.trans ho)

/-- **Only a session of `p` puts `p`'s key back**: an operation that gives the browser a key for
    `p` when it held none is a share fetch by a session of `p`.
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
        cases hsq : s.shares q <;> simp [step, hs, hsq, upd_other _ _ _ _ hqp, hnone] at hsome
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
      exact endNone
  | expire => exact absurd endNone (by simpa [step] using hsome)
  | revoke q => exact absurd hnone (by simpa [step] using hsome)
  | reload => exfalso; simp [step, hnone] at hsome

/-- **A share deleted at the server reaches the device at its next reload**: once the share is gone
    and no offline key was kept, a reload leaves every ciphertext of the principal unopenable,
    in that state and every later one. -/
theorem revoke_reload_erases {α : Type} (s : St α) (h : Inv s) (p : PrincipalId)
    (hsh : s.shares p = none) (hoff : s.offline p = false) (c : Sealed α) (hc : c ∈ s.disk)
    (ho : c.owner = p) (ops : List (Op α)) : ¬ Opens (run (step s .reload) ops) c := by
  apply dead_not_opens
  apply dead_run
  refine ⟨h.disk_lt c hc, fun q hq => ?_, fun q hq => ?_⟩
  · simp only [step] at hq
    have hqp : q = p := (h.disk_keys c hc q (by split at hq <;> first | exact hq | cases hq)).symm.trans ho
    subst hqp
    simp [hoff] at hq
  · have hqp : q = p := (h.disk_shares c hc q hq).symm.trans ho
    subst hqp
    simp [step, hsh] at hq

/-- With an offline key kept, the same reload does not erase: the window the design bounds. -/
theorem offline_key_survives_reload {α : Type} (s : St α) (p : PrincipalId) (k : Nat)
    (hk : s.keys p = some k) (hoff : s.offline p = true) : (step s .reload).keys p = some k := by
  simp [step, hoff, hk]

end Strata.Client.Privacy
