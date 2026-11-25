/-
The distributed protocol of the backend (IronFleet's middle layer, WG-5655).

Hosts are abstract state machines that talk only through packets. The network is the set of
packets sent so far: nothing is removed, so a packet may be received any number of times, which
covers delay, drop, duplication and reordering. Nothing is tampered with.

Hosts and what they do:
- the BFF forwards a client's request, with its session token, to the backend and relays the
  backend's reply to the client (it keeps no state);
- the backend runs one transaction per request, concurrently with the others. A transaction loads
  a snapshot of its tenant at some revision, computes the kernel's verdict on it, and sends a
  commit guarded by that revision. A conflict sends it back to load again. Each load and snapshot
  carries the transaction's attempt number (the count of its conflicts so far), and the backend
  accepts only the snapshot of its current attempt: a snapshot read for an earlier attempt is never
  consumed;
- the store keeps each tenant's history of states (index = revision), decides each request once,
  and commits a transaction only when the tenant is still at the revision it was based on;
- the outbox publisher sends each committed event to the subscribers whose principal may read it.

Each host step reads at most one packet and then sends its packets. There is no clock, so the
reduction obligation (receives, then at most one time-dependent input, then sends) holds by
construction; the general argument that this suffices is C4.

`absState` maps a protocol state to a system state (`Spec.lean`). `Good` is the invariant over the
set of sent packets and the host states that the refinement needs.
-/

import Strata.System.Spec

namespace Strata.System

/-- Every message any host sends. -/
inductive Packet where
  /-- client to BFF -/
  | req (rid : RequestId) (tok : Token) (i : Intent)
  /-- BFF to backend -/
  | fwd (rid : RequestId) (tok : Token) (i : Intent)
  /-- backend to BFF -/
  | reply (rid : RequestId) (o : Outcome)
  /-- BFF to client: the observable reply -/
  | clientReply (rid : RequestId) (o : Outcome)
  /-- backend to store: load the tenant -/
  | load (rid : RequestId) (t : TenantId) (a : Nat)
  /-- store to backend: the tenant's state at revision `base` -/
  | snap (rid : RequestId) (t : TenantId) (v : Option Tenant) (base : Nat) (a : Nat)
  /-- backend to store: the verdict computed on the snapshot at revision `base` -/
  | commit (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent)
      (base : Nat) (res : Except Err Tenant)
  /-- store to backend: the request's decision -/
  | done (rid : RequestId) (o : Outcome)
  /-- store to backend: the commit was based on a revision that is no longer current -/
  | conflict (rid : RequestId) (base : Nat)
  /-- store to publisher: an event was committed -/
  | committed (e : Event)
  /-- publisher to subscriber: the observable frame -/
  | frame (sid : SubId) (e : Event)

/-- The phase of a backend transaction. -/
inductive Phase where
  | loading
  | ready (v : Option Tenant) (base : Nat)
  | committing (v : Option Tenant) (base : Nat)
  | finished (o : Outcome)

/-- One backend transaction: the request's session, the intent, and where it is. -/
structure Txn where
  tok : Token
  tenant : TenantId
  principal : PrincipalId
  intent : Intent
  phase : Phase

/-- The store: tenant states by revision, the committed events, and the decision made for each
    request. -/
structure StoreState where
  hist : TenantId → Nat → Option Tenant
  rev : TenantId → Nat
  evs : List Event
  decided : RequestId → Option Outcome

/-- A protocol state: the packets sent, the backend's transactions, the requests answered without
    a session, and the store. -/
structure PState where
  net : Packet → Prop
  txns : RequestId → Option Txn
  noSession : RequestId → Bool
  store : StoreState
  /-- the attempt number of each request's transaction: it counts the conflicts so far -/
  attempt : RequestId → Nat

/-- Packets the protocol sends without changing any invariant's subject. -/
def Inert : Packet → Prop
  | .req .. | .fwd .. | .clientReply .. | .load .. | .conflict .. | .frame .. => True
  | _ => False

def sendOne (s : PState) (p : Packet) : PState :=
  { s with net := fun x => s.net x ∨ x = p }

def updAtt (f : RequestId → Nat) (rid : RequestId) (n : Nat) : RequestId → Nat :=
  fun r => if r = rid then n else f r

def updTxn (f : RequestId → Option Txn) (rid : RequestId) (x : Txn) : RequestId → Option Txn :=
  fun r => if r = rid then some x else f r

theorem updTxn_self (f : RequestId → Option Txn) (rid : RequestId) (x : Txn) :
    updTxn f rid x rid = some x := by
  simp [updTxn]

theorem updTxn_ne (f : RequestId → Option Txn) (rid : RequestId) (x : Txn) (r : RequestId)
    (h : r ≠ rid) : updTxn f rid x r = f r := by
  simp [updTxn, h]

/-- The initial state: nothing sent, no transactions, every tenant in `w0` at revision 0. -/
def PInit (w0 : World) (s : PState) : Prop :=
  (∀ p, ¬ s.net p) ∧ (∀ r, s.txns r = none) ∧ (∀ r, s.noSession r = false) ∧
  (∀ t k, s.store.hist t k = w0 t) ∧ (∀ t, s.store.rev t = 0) ∧ s.store.evs = [] ∧
  (∀ r, s.store.decided r = none) ∧ (∀ r, s.attempt r = 0)


/-- One step of the protocol: exactly one host acts on at most one packet and sends its packets.
    The store's steps are the decisions on the network: a commit is applied only when its request
    is undecided and its revision is still current, and otherwise answered without a change. -/
inductive PNext (K : Kernel) (C : Config) : PState → PState → Prop
  /-- A client sends a request. Its content is arbitrary. -/
  | clientReq (s : PState) (rid : RequestId) (tok : Token) (i : Intent) :
      PNext K C s (sendOne s (.req rid tok i))
  /-- The BFF forwards a request with its token. -/
  | bffForward (s : PState) (rid : RequestId) (tok : Token) (i : Intent)
      (h : s.net (.req rid tok i)) :
      PNext K C s (sendOne s (.fwd rid tok i))
  /-- The BFF relays the backend's reply to the client. -/
  | bffRelay (s : PState) (rid : RequestId) (o : Outcome) (h : s.net (.reply rid o)) :
      PNext K C s (sendOne s (.clientReply rid o))
  /-- The backend starts a transaction for a request with a session, and asks the store for the
      tenant. A repeated forward changes nothing (the request already has a transaction). -/
  | begin (s : PState) (rid : RequestId) (tok : Token) (i : Intent) (t : TenantId)
      (p : PrincipalId) (h : s.net (.fwd rid tok i)) (hx : s.txns rid = none)
      (hn : s.noSession rid = false) (hs : C.sessions tok = some (t, p)) :
      PNext K C s { sendOne s (.load rid t (s.attempt rid)) with
        txns := updTxn s.txns rid ⟨tok, t, p, i, .loading⟩ }
  /-- The backend answers a request whose token has no session. -/
  | noSession (s : PState) (rid : RequestId) (tok : Token) (i : Intent)
      (h : s.net (.fwd rid tok i)) (hx : s.txns rid = none) (hn : s.noSession rid = false)
      (hs : C.sessions tok = none) :
      PNext K C s { sendOne s (.reply rid .noSession) with
        noSession := fun r => if r = rid then true else s.noSession r }
  /-- A transaction receives a snapshot of its tenant. -/
  | snap (s : PState) (rid : RequestId) (t : TenantId) (v : Option Tenant) (base : Nat) (a : Nat)
      (x : Txn) (h : s.net (.snap rid t v base a)) (hx : s.txns rid = some x)
      (ht : x.tenant = t) (hl : x.phase = .loading) (hat : s.attempt rid = a) :
      PNext K C s { s with txns := updTxn s.txns rid { x with phase := .ready v base } }
  /-- A transaction computes the kernel's verdict on its snapshot and sends the commit. -/
  | commit (s : PState) (rid : RequestId) (x : Txn) (v : Option Tenant) (base : Nat)
      (hx : s.txns rid = some x) (hp : x.phase = .ready v base) :
      PNext K C s { sendOne s (.commit rid x.tok x.tenant x.principal x.intent base
          (verdict K v x.principal x.intent)) with
        txns := updTxn s.txns rid { x with phase := .committing v base } }
  /-- The store refused a commit based on an old revision; the transaction loads again. -/
  | conflict (s : PState) (rid : RequestId) (base : Nat) (x : Txn) (v : Option Tenant)
      (h : s.net (.conflict rid base)) (hx : s.txns rid = some x)
      (hp : x.phase = .committing v base) :
      PNext K C s { sendOne s (.load rid x.tenant (s.attempt rid + 1)) with
        txns := updTxn s.txns rid { x with phase := .loading },
        attempt := updAtt s.attempt rid (s.attempt rid + 1) }
  /-- The store decided the request: the backend replies. -/
  | done (s : PState) (rid : RequestId) (o : Outcome) (x : Txn)
      (h : s.net (.done rid o)) (hx : s.txns rid = some x) :
      PNext K C s { sendOne s (.reply rid o) with
        txns := updTxn s.txns rid { x with phase := .finished o } }
  /-- The store answers a load with the tenant's current state and revision. -/
  | load (s : PState) (rid : RequestId) (t : TenantId) (a : Nat) (h : s.net (.load rid t a)) :
      PNext K C s (sendOne s (.snap rid t (s.store.hist t (s.store.rev t)) (s.store.rev t) a))
  /-- A commit for a request the store has decided: the decision is sent again, nothing changes. -/
  | storeDecided (s : PState) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (o : Outcome)
      (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = some o) :
      PNext K C s (sendOne s (.done rid o))
  /-- A commit for an undecided request whose revision is no longer current: a conflict. -/
  | storeConflict (s : PState) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant)
      (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = none)
      (hb : base ≠ s.store.rev t) :
      PNext K C s (sendOne s (.conflict rid base))
  /-- A commit of an accepted verdict on the current revision: the tenant moves to revision
      `base + 1`, the event is written to the outbox, and the request is decided. -/
  | storeAccept (s : PState) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (t' : Tenant)
      (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = none)
      (hb : base = s.store.rev t) (hr : res = .ok t') :
      PNext K C s
        { net := fun x => s.net x ∨ x = .done rid .accepted ∨ x = .committed (Event.mk t rid t')
          txns := s.txns
          noSession := s.noSession
          attempt := s.attempt
          store :=
            { hist := fun x k => if x = t ∧ k = base + 1 then some t' else s.store.hist x k
              rev := fun x => if x = t then base + 1 else s.store.rev x
              evs := s.store.evs ++ [Event.mk t rid t']
              decided := fun r => if r = rid then some .accepted else s.store.decided r } }
  /-- A commit of a refusal on the current revision: nothing changes but the decision. -/
  | storeRefuse (s : PState) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (e : Err)
      (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = none)
      (hb : base = s.store.rev t) (hr : res = .error e) :
      PNext K C s
        { net := fun x => s.net x ∨ x = .done rid (.refused e)
          txns := s.txns
          noSession := s.noSession
          attempt := s.attempt
          store := { s.store with
            decided := fun r => if r = rid then some (.refused e) else s.store.decided r } }
  /-- The publisher sends a committed event to a subscriber whose principal may read it. -/
  | publish (s : PState) (sid : SubId) (e : Event) (t : TenantId) (p : PrincipalId)
      (h : s.net (.committed e)) (hs : C.subs sid = some (t, p)) (ht : e.tenant = t)
      (hr : K.readable e.post p) :
      PNext K C s (sendOne s (.frame sid e))

/-- What the system specification sees of a protocol state. The replies are the store's
    decisions, and the answers without a session; the frames are the frames sent. -/
def absState (s : PState) : SysState :=
  { world := fun t => s.store.hist t (s.store.rev t)
    log := s.store.evs
    replies := fun r => match s.store.decided r with
      | some o => some o
      | none => if s.noSession r then some .noSession else none
    frames := fun f => s.net (.frame f.1 f.2) }

/-- The invariant of reachable protocol states, over the packets sent and the host states. -/
theorem ex_snap_ex {s s' : PState} {rid : RequestId} {t : TenantId} {v : Option Tenant}
    {b : Nat} (h : ∃ a, s.net (.snap rid t v b a)) (hm : ∀ p, s.net p → s'.net p) :
    ∃ a, s'.net (.snap rid t v b a) := by
  obtain ⟨a, ha⟩ := h
  exact ⟨a, hm _ ha⟩

structure Good (K : Kernel) (C : Config) (s : PState) : Prop where
  /-- A snapshot is the tenant's state at the revision it names, and that revision exists. -/
  snapHist : ∀ rid t v base a, s.net (.snap rid t v base a) →
    base ≤ s.store.rev t ∧ s.store.hist t base = v
  /-- A transaction in the ready or committing phase holds a snapshot the store sent. -/
  snapPhase : ∀ rid x v base, s.txns rid = some x →
    (x.phase = .ready v base ∨ x.phase = .committing v base) →
    ∃ a, s.net (.snap rid x.tenant v base a)
  /-- A transaction's token is a session of its tenant and principal. -/
  sessionTxn : ∀ rid x, s.txns rid = some x → C.sessions x.tok = some (x.tenant, x.principal)
  /-- A commit belongs to a transaction, with the same token, tenant, principal and intent. -/
  commitTxn : ∀ rid tok t p i base res, s.net (.commit rid tok t p i base res) →
    ∃ x, s.txns rid = some x ∧ x.tok = tok ∧ x.tenant = t ∧ x.principal = p ∧ x.intent = i
  /-- A commit's result is the kernel's verdict on the snapshot it was computed from. -/
  commitCorrect : ∀ rid tok t p i base res, s.net (.commit rid tok t p i base res) →
    ∃ v a, s.net (.snap rid t v base a) ∧ res = verdict K v p i
  /-- A done packet is the store's decision. -/
  doneDecided : ∀ rid o, s.net (.done rid o) → s.store.decided rid = some o
  /-- A reply is the store's decision, or an answer for a request without a session. -/
  replyDecided : ∀ rid o, s.net (.reply rid o) →
    s.store.decided rid = some o ∨ (o = .noSession ∧ s.noSession rid = true)
  /-- A committed packet names an event of the outbox. -/
  committedIn : ∀ e, s.net (.committed e) → e ∈ s.store.evs
  /-- A request without a session has no transaction. -/
  noSessionTxn : ∀ r, s.noSession r = true → s.txns r = none
  /-- A decided request has a transaction. -/
  decidedTxn : ∀ r o, s.store.decided r = some o → ∃ x, s.txns r = some x

theorem good_inert {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s) (p : Packet)
    (hp : Inert p) (hnet : ∀ x, s'.net x ↔ (s.net x ∨ x = p)) (htx : s'.txns = s.txns)
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t v b _ h
    rcases (hnet _).mp h with h | h
    · rw [hst]; exact hg.snapHist r t v b _ h
    · subst h; simp [Inert] at hp
  · intro r x v b hx hph
    rw [htx] at hx
    exact ex_snap_ex (hg.snapPhase r x v b hx hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r x hx
    rw [htx] at hx
    exact hg.sessionTxn r x hx
  · intro r tok t p' i b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨x, hx, h1, h2, h3, h4⟩ := hg.commitTxn r tok t p' i b res h
      exact ⟨x, by rw [htx]; exact hx, h1, h2, h3, h4⟩
    · subst h; simp [Inert] at hp
  · intro r tok t p' i b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨v, _, hv, he⟩ := hg.commitCorrect r tok t p' i b res h
      exact ⟨v, _, (hnet _).mpr (Or.inl hv), he⟩
    · subst h; simp [Inert] at hp
  · intro r o h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.doneDecided r o h
    · subst h; simp [Inert] at hp
  · intro r o h
    rcases (hnet _).mp h with h | h
    · rcases hg.replyDecided r o h with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · subst h; simp [Inert] at hp
  · intro e h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.committedIn e h
    · subst h; simp [Inert] at hp
  · intro r h
    rw [hns] at h
    rw [htx]
    exact hg.noSessionTxn r h
  · intro r o h
    rw [hst] at h
    obtain ⟨x, hx⟩ := hg.decidedTxn r o h
    exact ⟨x, by rw [htx]; exact hx⟩



theorem good_begin {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (tok : Token) (i : Intent) (t : TenantId) (p : PrincipalId)
    (hx : s.txns rid = none) (hn : s.noSession rid = false)
    (hs : C.sessions tok = some (t, p))
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .load rid t (s.attempt rid)))
    (htx : s'.txns = updTxn s.txns rid ⟨tok, t, p, i, .loading⟩)
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v b _ h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.snapHist r t' v b _ h
    · cases h
  · intro r y v b hy hph
    rw [htx] at hy
    by_cases hr : r = rid
    · rw [hr, updTxn_self] at hy
      rw [← Option.some.inj hy] at hph
      simp at hph
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact ex_snap_ex (hg.snapPhase r y v b hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    by_cases hr : r = rid
    · rw [hr, updTxn_self] at hy
      rw [← Option.some.inj hy]
      exact hs
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok' t' p' i' b res h
      by_cases hr : r = rid
      · rw [hr, hx] at hy
        cases hy
      · exact ⟨y, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hy, h1, h2, h3, h4⟩
    · cases h
  · intro r tok' t' p' i' b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨v, _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b res h
      exact ⟨v, _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h
  · intro r o h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.doneDecided r o h
    · cases h
  · intro r o h
    rcases (hnet _).mp h with h | h
    · rcases hg.replyDecided r o h with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h
  · intro e h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.committedIn e h
    · cases h
  · intro r h
    rw [hns] at h
    by_cases hr : r = rid
    · rw [hr, hn] at h
      cases h
    · rw [htx, updTxn_ne _ _ _ _ hr]
      exact hg.noSessionTxn r h
  · intro r o h
    rw [hst] at h
    obtain ⟨x, hx'⟩ := hg.decidedTxn r o h
    by_cases hr : r = rid
    · subst hr
      exact ⟨_, by rw [htx]; exact updTxn_self _ _ _⟩
    · exact ⟨x, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hx'⟩

theorem good_noSession {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (hx : s.txns rid = none) (hn : s.noSession rid = false)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .reply rid .noSession))
    (htx : s'.txns = s.txns)
    (hns : ∀ r, s'.noSession r = if r = rid then true else s.noSession r)
    (hst : s'.store = s.store) : Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t v b _ h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.snapHist r t v b _ h
    · cases h
  · intro r y v b hy hph
    rw [htx] at hy
    exact ex_snap_ex (hg.snapPhase r y v b hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    exact hg.sessionTxn r y hy
  · intro r tok t p' i' b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok t p' i' b res h
      exact ⟨y, by rw [htx]; exact hy, h1, h2, h3, h4⟩
    · cases h
  · intro r tok t p' i' b res h
    rcases (hnet _).mp h with h | h
    · obtain ⟨v, _, hv, he⟩ := hg.commitCorrect r tok t p' i' b res h
      exact ⟨v, _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h
  · intro r o h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.doneDecided r o h
    · cases h
  · intro r o h
    rcases (hnet _).mp h with h | h
    · rcases hg.replyDecided r o h with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · refine Or.inr ⟨h1, ?_⟩
        rw [hns]
        by_cases hr : r = rid
        · rw [hr, hn] at h2
          cases h2
        · rw [if_neg hr]
          exact h2
    · rw [Packet.reply.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      refine Or.inr ⟨rfl, ?_⟩
      rw [hns]
      simp
  · intro e h
    rw [hst]
    rcases (hnet _).mp h with h | h
    · exact hg.committedIn e h
    · cases h
  · intro r h
    rw [hns] at h
    by_cases hr : r = rid
    · subst hr
      rw [htx]
      exact hx
    · rw [if_neg hr] at h
      rw [htx]
      exact hg.noSessionTxn r h
  · intro r o h
    rw [hst] at h
    obtain ⟨x, hx'⟩ := hg.decidedTxn r o h
    exact ⟨x, by rw [htx]; exact hx'⟩



theorem upd_fields (x : Txn) (ph : Phase) :
    ({ x with phase := ph } : Txn).tok = x.tok ∧ ({ x with phase := ph } : Txn).tenant = x.tenant ∧
    ({ x with phase := ph } : Txn).principal = x.principal ∧
    ({ x with phase := ph } : Txn).intent = x.intent :=
  ⟨rfl, rfl, rfl, rfl⟩

/-- A transaction that keeps its session fields and changes its phase keeps every commit that
    named it. -/
theorem commit_transfer {s s' : PState} {rid : RequestId} {x y : Txn}
    (hx : s.txns rid = some x)
    (hf : y.tok = x.tok ∧ y.tenant = x.tenant ∧ y.principal = x.principal ∧ y.intent = x.intent)
    (htx : s'.txns = updTxn s.txns rid y) {r : RequestId} {tok : Token} {t : TenantId}
    {p : PrincipalId} {i : Intent}
    (hz : ∃ z, s.txns r = some z ∧ z.tok = tok ∧ z.tenant = t ∧ z.principal = p ∧ z.intent = i) :
    ∃ w, s'.txns r = some w ∧ w.tok = tok ∧ w.tenant = t ∧ w.principal = p ∧ w.intent = i := by
  obtain ⟨z, hzr, h1, h2, h3, h4⟩ := hz
  by_cases hr : r = rid
  · subst hr
    rw [hx] at hzr
    have hzx := Option.some.inj hzr
    refine ⟨y, by rw [htx, updTxn_self], ?_, ?_, ?_, ?_⟩
    · rw [hf.1, hzx]; exact h1
    · rw [hf.2.1, hzx]; exact h2
    · rw [hf.2.2.1, hzx]; exact h3
    · rw [hf.2.2.2, hzx]; exact h4
  · exact ⟨z, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hzr, h1, h2, h3, h4⟩

theorem good_snap {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (t : TenantId) (v : Option Tenant) (base : Nat) (x : Txn)
    (a : Nat) (h : s.net (.snap rid t v base a)) (hx : s.txns rid = some x) (ht : x.tenant = t)
    (hnet : ∀ y, s'.net y ↔ s.net y)
    (htx : s'.txns = updTxn s.txns rid { x with phase := .ready v base })
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  subst ht
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    exact hg.snapHist r t' v' b' _ ((hnet _).mp h')
  · intro r y v' b' hy hph
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      have hye := Option.some.inj hy
      subst hye
      rcases hph with hph | hph
      · simp at hph
        obtain ⟨rfl, rfl⟩ := hph
        exact ⟨a, (hnet _).mpr h⟩
      · simp at hph
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr hp)
  · intro r y hy
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      rw [← Option.some.inj hy]
      exact hg.sessionTxn _ x hx
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res h'
    exact commit_transfer hx (upd_fields x _) htx (hg.commitTxn r tok' t' p' i' b' res ((hnet _).mp h'))
  · intro r tok' t' p' i' b' res h'
    obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res ((hnet _).mp h')
    exact ⟨v'', _, (hnet _).mpr hv, he⟩
  · intro r o h'
    rw [hst]
    exact hg.doneDecided r o ((hnet _).mp h')
  · intro r o h'
    rcases hg.replyDecided r o ((hnet _).mp h') with h1 | ⟨h1, h2⟩
    · exact Or.inl (by rw [hst]; exact h1)
    · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
  · intro e h'
    rw [hst]
    exact hg.committedIn e ((hnet _).mp h')
  · intro r h'
    rw [hns] at h'
    rw [htx]
    by_cases hr : r = rid
    · subst hr
      have h0 := hg.noSessionTxn _ h'
      rw [hx] at h0
      cases h0
    · rw [updTxn_ne _ _ _ _ hr]
      exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hst] at h'
    obtain ⟨z, hz⟩ := hg.decidedTxn r o h'
    by_cases hr : r = rid
    · subst hr
      exact ⟨_, by rw [htx]; exact updTxn_self _ _ _⟩
    · exact ⟨z, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hz⟩

theorem good_commit {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (x : Txn) (v : Option Tenant) (base : Nat)
    (hx : s.txns rid = some x) (hp : x.phase = .ready v base)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨
      y = .commit rid x.tok x.tenant x.principal x.intent base (verdict K v x.principal x.intent)))
    (htx : s'.txns = updTxn s.txns rid { x with phase := .committing v base })
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  obtain ⟨a0, hsnap⟩ := hg.snapPhase rid x v base hx (Or.inl hp)
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r t' v' b' _ h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      have hye := Option.some.inj hy
      subst hye
      rcases hph with hph | hph
      · simp at hph
      · simp at hph
        obtain ⟨rfl, rfl⟩ := hph
        exact ⟨a0, (hnet _).mpr (Or.inl hsnap)⟩
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      rw [← Option.some.inj hy]
      exact hg.sessionTxn _ x hx
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · exact commit_transfer hx (upd_fields x _) htx (hg.commitTxn r tok' t' p' i' b' res h')
    · cases h'
      exact ⟨{ x with phase := .committing v base }, by rw [htx, updTxn_self], rfl, rfl, rfl, rfl⟩
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h'
      exact ⟨v, _, (hnet _).mpr (Or.inl hsnap), rfl⟩
  · intro r o h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.doneDecided r o h'
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o h' with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
  · intro e h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    by_cases hr : r = rid
    · subst hr
      have h0 := hg.noSessionTxn _ h'
      rw [hx] at h0
      cases h0
    · rw [updTxn_ne _ _ _ _ hr]
      exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hst] at h'
    obtain ⟨z, hz⟩ := hg.decidedTxn r o h'
    by_cases hr : r = rid
    · subst hr
      exact ⟨_, by rw [htx]; exact updTxn_self _ _ _⟩
    · exact ⟨z, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hz⟩

theorem good_conflict {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (base : Nat) (x : Txn) (v : Option Tenant)
    (h : s.net (.conflict rid base)) (hx : s.txns rid = some x)
    (hp : x.phase = .committing v base)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .load rid x.tenant (s.attempt rid + 1)))
    (htx : s'.txns = updTxn s.txns rid { x with phase := .loading })
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r t' v' b' _ h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      have hye := Option.some.inj hy
      subst hye
      rcases hph with hph | hph <;> simp at hph
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      rw [← Option.some.inj hy]
      exact hg.sessionTxn _ x hx
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · exact commit_transfer hx (upd_fields x _) htx (hg.commitTxn r tok' t' p' i' b' res h')
    · cases h'
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h'
  · intro r o h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.doneDecided r o h'
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o h' with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
  · intro e h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    by_cases hr : r = rid
    · subst hr
      have h0 := hg.noSessionTxn _ h'
      rw [hx] at h0
      cases h0
    · rw [updTxn_ne _ _ _ _ hr]
      exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hst] at h'
    obtain ⟨z, hz⟩ := hg.decidedTxn r o h'
    by_cases hr : r = rid
    · subst hr
      exact ⟨_, by rw [htx]; exact updTxn_self _ _ _⟩
    · exact ⟨z, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hz⟩

theorem good_done {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (o : Outcome) (x : Txn)
    (h : s.net (.done rid o)) (hx : s.txns rid = some x)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .reply rid o))
    (htx : s'.txns = updTxn s.txns rid { x with phase := .finished o })
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) : Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r t' v' b' _ h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      have hye := Option.some.inj hy
      subst hye
      rcases hph with hph | hph <;> simp at hph
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    by_cases hr : r = rid
    · subst hr
      rw [updTxn_self] at hy
      rw [← Option.some.inj hy]
      exact hg.sessionTxn _ x hx
    · rw [updTxn_ne _ _ _ _ hr] at hy
      exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · exact commit_transfer hx (upd_fields x _) htx (hg.commitTxn r tok' t' p' i' b' res h')
    · cases h'
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h'
  · intro r o' h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.doneDecided r o' h'
    · cases h'
  · intro r o' h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o' h' with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
      exact Or.inl (by rw [hst]; exact hg.doneDecided _ _ h)
  · intro e h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    by_cases hr : r = rid
    · subst hr
      have h0 := hg.noSessionTxn _ h'
      rw [hx] at h0
      cases h0
    · rw [updTxn_ne _ _ _ _ hr]
      exact hg.noSessionTxn r h'
  · intro r o' h'
    rw [hst] at h'
    obtain ⟨z, hz⟩ := hg.decidedTxn r o' h'
    by_cases hr : r = rid
    · subst hr
      exact ⟨_, by rw [htx]; exact updTxn_self _ _ _⟩
    · exact ⟨z, by rw [htx, updTxn_ne _ _ _ _ hr]; exact hz⟩

theorem good_load {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (t : TenantId) (a : Nat) (h : s.net (.load rid t a))
    (hnet : ∀ y, s'.net y ↔
      (s.net y ∨ y = .snap rid t (s.store.hist t (s.store.rev t)) (s.store.rev t) a))
    (htx : s'.txns = s.txns) (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) :
    Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r t' v' b' _ h'
    · cases h'
      exact ⟨Nat.le_refl _, rfl⟩
  · intro r y v' b' hy hph
    rw [htx] at hy
    exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok' t' p' i' b' res h'
      exact ⟨y, by rw [htx]; exact hy, h1, h2, h3, h4⟩
    · cases h'
  · intro r tok' t' p' i' b' res h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h'
  · intro r o h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.doneDecided r o h'
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o h' with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
  · intro e h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hst] at h'
    obtain ⟨y, hy⟩ := hg.decidedTxn r o h'
    exact ⟨y, by rw [htx]; exact hy⟩

theorem good_storeDecided {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent)
    (base : Nat) (res : Except Err Tenant) (o : Outcome)
    (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = some o)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .done rid o))
    (htx : s'.txns = s.txns) (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) :
    Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t' v' b' _ h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r t' v' b' _ h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok' t' p' i' b' res' h'
      exact ⟨y, by rw [htx]; exact hy, h1, h2, h3, h4⟩
    · cases h'
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv, he⟩ := hg.commitCorrect r tok' t' p' i' b' res' h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv), he⟩
    · cases h'
  · intro r o' h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.doneDecided r o' h'
    · cases h'
      exact hd
  · intro r o' h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o' h' with h1 | ⟨h1, h2⟩
      · exact Or.inl (by rw [hst]; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
  · intro e h'
    rw [hst]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    exact hg.noSessionTxn r h'
  · intro r o' h'
    rw [hst] at h'
    obtain ⟨y, hy⟩ := hg.decidedTxn r o' h'
    exact ⟨y, by rw [htx]; exact hy⟩


theorem good_storeAccept {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent)
    (base : Nat) (res : Except Err Tenant) (t1 : Tenant)
    (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = none)
    (hb : base = s.store.rev t) (hr : res = .ok t1)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .done rid .accepted ∨ y = .committed (Event.mk t rid t1)))
    (htx : s'.txns = s.txns) (hns : s'.noSession = s.noSession)
    (hhist : ∀ x k, s'.store.hist x k =
      if x = t ∧ k = base + 1 then some t1 else s.store.hist x k)
    (hrev : ∀ x, s'.store.rev x = if x = t then base + 1 else s.store.rev x)
    (hevs : s'.store.evs = s.store.evs ++ [Event.mk t rid t1])
    (hdec : ∀ r, s'.store.decided r = if r = rid then some .accepted else s.store.decided r) :
    Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r x v' k _ h'
    rcases (hnet _).mp h' with h' | h' | h'
    · obtain ⟨h1, h2⟩ := hg.snapHist r x v' k _ h'
      by_cases hx : x = t
      · have hk : k ≤ base := by rw [hx] at h1; omega
        have hne : ¬ (x = t ∧ k = base + 1) := fun hc => by have := hc.2; omega
        rw [hrev, hhist, if_pos hx, if_neg hne]
        exact ⟨by omega, h2⟩
      · have hne : ¬ (x = t ∧ k = base + 1) := fun hc => hx hc.1
        rw [hrev, hhist, if_neg hx, if_neg hne]
        exact ⟨h1, h2⟩
    · cases h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h' | h'
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok' t' p' i' b' res' h'
      exact ⟨y, by rw [htx]; exact hy, h1, h2, h3, h4⟩
    · cases h'
    · cases h'
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h' | h'
    · obtain ⟨v'', _, hv', he⟩ := hg.commitCorrect r tok' t' p' i' b' res' h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv'), he⟩
    · cases h'
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h' | h'
    · have h1 := hg.doneDecided r o h'
      by_cases hr' : r = rid
      · rw [hr', hd] at h1
        cases h1
      · rw [hdec, if_neg hr']
        exact h1
    · cases h'
      rw [hdec, if_pos rfl]
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h' | h'
    · rcases hg.replyDecided r o h' with h1 | ⟨h1, h2⟩
      · by_cases hr' : r = rid
        · rw [hr', hd] at h1
          cases h1
        · exact Or.inl (by rw [hdec, if_neg hr']; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
    · cases h'
  · intro e h'
    rw [hevs]
    rcases (hnet _).mp h' with h' | h' | h'
    · exact List.mem_append.mpr (Or.inl (hg.committedIn e h'))
    · cases h'
    · cases h'
      exact List.mem_append.mpr (Or.inr (List.mem_singleton.mpr rfl))
  · intro r h'
    rw [hns] at h'
    rw [htx]
    exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hdec] at h'
    by_cases hr' : r = rid
    · obtain ⟨y, hy, -⟩ := hg.commitTxn rid tok t p i base res h
      exact ⟨y, by rw [htx, hr']; exact hy⟩
    · rw [if_neg hr'] at h'
      obtain ⟨y, hy⟩ := hg.decidedTxn r o h'
      exact ⟨y, by rw [htx]; exact hy⟩

theorem good_storeRefuse {K : Kernel} {C : Config} {s s' : PState} (hg : Good K C s)
    (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent)
    (base : Nat) (res : Except Err Tenant) (e : Err)
    (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = none)
    (hb : base = s.store.rev t) (hr : res = .error e)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .done rid (.refused e)))
    (htx : s'.txns = s.txns) (hns : s'.noSession = s.noSession)
    (hdec : ∀ r, s'.store.decided r =
      if r = rid then some (.refused e) else s.store.decided r)
    (hhist : ∀ x k, s'.store.hist x k = s.store.hist x k)
    (hrev : ∀ x, s'.store.rev x = s.store.rev x)
    (hevs : s'.store.evs = s.store.evs) :
    Good K C s' := by
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r x v' k _ h'
    rw [hrev, hhist]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.snapHist r x v' k _ h'
    · cases h'
  · intro r y v' b' hy hph
    rw [htx] at hy
    exact ex_snap_ex (hg.snapPhase r y v' b' hy hph) (fun p hp => (hnet p).mpr (Or.inl hp))
  · intro r y hy
    rw [htx] at hy
    exact hg.sessionTxn r y hy
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨y, hy, h1, h2, h3, h4⟩ := hg.commitTxn r tok' t' p' i' b' res' h'
      exact ⟨y, by rw [htx]; exact hy, h1, h2, h3, h4⟩
    · cases h'
  · intro r tok' t' p' i' b' res' h'
    rcases (hnet _).mp h' with h' | h'
    · obtain ⟨v'', _, hv', he⟩ := hg.commitCorrect r tok' t' p' i' b' res' h'
      exact ⟨v'', _, (hnet _).mpr (Or.inl hv'), he⟩
    · cases h'
  · intro r o h'
    rcases (hnet _).mp h' with h' | h'
    · have h1 := hg.doneDecided r o h'
      by_cases hr' : r = rid
      · rw [hr', hd] at h1
        cases h1
      · rw [hdec, if_neg hr']
        exact h1
    · cases h'
      rw [hdec, if_pos rfl]
  · intro r o h'
    rcases (hnet _).mp h' with h' | h'
    · rcases hg.replyDecided r o h' with h1 | ⟨h1, h2⟩
      · by_cases hr' : r = rid
        · rw [hr', hd] at h1
          cases h1
        · exact Or.inl (by rw [hdec, if_neg hr']; exact h1)
      · exact Or.inr ⟨h1, by rw [hns]; exact h2⟩
    · cases h'
  · intro e' h'
    rw [hevs]
    rcases (hnet _).mp h' with h' | h'
    · exact hg.committedIn e' h'
    · cases h'
  · intro r h'
    rw [hns] at h'
    rw [htx]
    exact hg.noSessionTxn r h'
  · intro r o h'
    rw [hdec] at h'
    by_cases hr' : r = rid
    · obtain ⟨y, hy, -⟩ := hg.commitTxn rid tok t p i base res h
      exact ⟨y, by rw [htx, hr']; exact hy⟩
    · rw [if_neg hr'] at h'
      obtain ⟨y, hy⟩ := hg.decidedTxn r o h'
      exact ⟨y, by rw [htx]; exact hy⟩

theorem good_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) : Good K C s' := by
  cases h with
  | clientReq rid tok i => exact good_inert hg (.req rid tok i) trivial (fun _ => Iff.rfl) rfl rfl rfl
  | bffForward rid tok i h =>
    exact good_inert hg (.fwd rid tok i) trivial (fun _ => Iff.rfl) rfl rfl rfl
  | bffRelay rid o h => exact good_inert hg (.clientReply rid o) trivial (fun _ => Iff.rfl) rfl rfl rfl
  | begin rid tok i t p h hx hn hs =>
    exact good_begin hg rid tok i t p hx hn hs (fun _ => Iff.rfl) rfl rfl rfl
  | noSession rid tok i h hx hn hs =>
    exact good_noSession hg rid hx hn (fun _ => Iff.rfl) rfl (fun _ => rfl) rfl
  | snap rid t v base a x h hx ht hl hat =>
    exact good_snap hg rid t v base x a h hx ht (fun _ => Iff.rfl) rfl rfl rfl
  | commit rid x v base hx hp =>
    exact good_commit hg rid x v base hx hp (fun _ => Iff.rfl) rfl rfl rfl
  | conflict rid base x v h hx hp =>
    exact good_conflict hg rid base x v h hx hp (fun _ => Iff.rfl) rfl rfl rfl
  | done rid o x h hx => exact good_done hg rid o x h hx (fun _ => Iff.rfl) rfl rfl rfl
  | load rid t a h => exact good_load hg rid t a h (fun _ => Iff.rfl) rfl rfl rfl
  | storeDecided rid tok t p i base res o h hd =>
    exact good_storeDecided hg rid tok t p i base res o h hd (fun _ => Iff.rfl) rfl rfl rfl
  | storeConflict rid tok t p i base res h hd hb =>
    exact good_inert hg (.conflict rid base) trivial (fun _ => Iff.rfl) rfl rfl rfl
  | storeAccept rid tok t p i base res t1 h hd hb hr =>
    exact good_storeAccept hg rid tok t p i base res t1 h hd hb hr (fun _ => Iff.rfl) rfl rfl
      (fun _ _ => rfl) (fun _ => rfl) rfl (fun _ => rfl)
  | storeRefuse rid tok t p i base res e h hd hb hr =>
    exact good_storeRefuse hg rid tok t p i base res e h hd hb hr (fun _ => Iff.rfl) rfl rfl
      (fun _ => rfl) (fun _ _ => rfl) (fun _ => rfl) rfl
  | publish sid e t p h hs ht hr =>
    exact good_inert hg (.frame sid e) trivial (fun _ => Iff.rfl) rfl rfl rfl

theorem good_init {K : Kernel} {C : Config} {w0 : World} {s : PState} (hi : PInit w0 s) :
    Good K C s := by
  obtain ⟨hn, hT, hNS, -, -, -, hD⟩ := hi
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro r t v b _ h
    exact absurd h (hn _)
  · intro r x v b hx _
    rw [hT r] at hx
    cases hx
  · intro r x hx
    rw [hT r] at hx
    cases hx
  · intro r tok t p i b res h
    exact absurd h (hn _)
  · intro r tok t p i b res h
    exact absurd h (hn _)
  · intro r o h
    exact absurd h (hn _)
  · intro r o h
    exact absurd h (hn _)
  · intro e h
    exact absurd h (hn _)
  · intro r h
    rw [hNS r] at h
    cases h
  · intro r o h
    rw [hD.1 r] at h
    cases h



/-- The reflexive, transitive closure of a relation, with both ends free. -/
inductive Star {α : Type} (r : α → α → Prop) : α → α → Prop
  | refl (a : α) : Star r a a
  | tail {a b c : α} : Star r a b → r b c → Star r a c

/-- A protocol state is reachable from an initial state. -/
def ProtoReachable (K : Kernel) (C : Config) (w0 : World) (s : PState) : Prop :=
  ∃ s0, PInit w0 s0 ∧ Star (PNext K C) s0 s

theorem steps_star {α : Type} {r : α → α → Prop} {a b c : α} (h1 : Steps r a b)
    (h2 : Star r b c) : Steps r a c := by
  induction h2 with
  | refl => exact h1
  | tail _ hbc ih => exact Steps.tail ih hbc

theorem star_one {α : Type} {r : α → α → Prop} {a b : α} (h : r a b) : Star r a b :=
  Star.tail (Star.refl a) h

/-- Packets that are not frames. -/
def notFrame : Packet → Prop
  | .frame _ _ => False
  | _ => True

theorem notFrame_ne (p : Packet) (hp : notFrame p) (sid : SubId) (e : Event) :
    p ≠ .frame sid e := by
  intro h
  subst h
  exact hp

theorem frame_iff_send (s : PState) (p : Packet) (hp : notFrame p) (sid : SubId) (e : Event) :
    (sendOne s p).net (.frame sid e) ↔ s.net (.frame sid e) := by
  constructor
  · intro h
    rcases h with h | hx
    · exact h
    · exact absurd hx.symm (notFrame_ne p hp sid e)
  · intro h
    exact Or.inl h

theorem frame_eq_iff (f : SubId × Event) (sid : SubId) (e : Event) :
    Packet.frame f.1 f.2 = Packet.frame sid e ↔ f = (sid, e) := by
  constructor
  · intro h
    obtain ⟨h1, h2⟩ := Packet.frame.inj h
    exact Prod.ext h1 h2
  · intro h
    subst h
    rfl

theorem SysState.ext' {a b : SysState} (h1 : a.world = b.world) (h2 : a.log = b.log)
    (h3 : a.replies = b.replies) (h4 : a.frames = b.frames) : a = b := by
  cases a
  cases b
  simp only at h1 h2 h3 h4
  subst h1 h2 h3 h4
  rfl

/-- A protocol state that differs from another only in packets that are not frames, and in
    nothing else the specification sees, has the same abstraction. -/
theorem abs_eq_of (s s' : PState) (hst : s'.store = s.store) (hns : s'.noSession = s.noSession)
    (hfr : ∀ sid e, s'.net (.frame sid e) ↔ s.net (.frame sid e)) :
    absState s' = absState s := by
  apply SysState.ext'
  · simp only [absState, hst]
  · simp only [absState, hst]
  · funext r
    simp only [absState, hst, hns]
  · funext f
    apply propext
    exact hfr f.1 f.2

theorem abs_init {w0 : World} {s : PState} (hi : PInit w0 s) : SysInit w0 (absState s) := by
  obtain ⟨hn, hT, hNS, hH, hR, hE, hD⟩ := hi
  refine ⟨?_, ?_, ?_, ?_⟩
  · funext t
    show s.store.hist t (s.store.rev t) = w0 t
    rw [hR t]
    exact hH t 0
  · show s.store.evs = []
    exact hE
  · intro r
    show (match s.store.decided r with
      | some o => some o
      | none => if s.noSession r then some Outcome.noSession else none) = none
    rw [hD.1 r]
    simp [hNS r]
  · intro f
    exact hn _

theorem net_mono {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s') :
    ∀ x, s.net x → s'.net x := by
  intro x hx
  cases h <;> first | exact Or.inl hx | exact hx



/-- The frames of a post-state that adds one packet, which is not a frame, are the frames of the
    state. -/
theorem frames_eq_of (s s' : PState) (p : Packet) (hp : notFrame p)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = p)) (sid : SubId) (e : Event) :
    s'.net (.frame sid e) ↔ s.net (.frame sid e) := by
  rw [hnet]
  constructor
  · intro h
    rcases h with h | h
    · exact h
    · exact absurd h.symm (notFrame_ne p hp sid e)
  · intro h
    exact Or.inl h

theorem star_eq_abs {K : Kernel} {C : Config} {s s' : PState} (hst : s'.store = s.store)
    (hns : s'.noSession = s.noSession)
    (hfr : ∀ sid e, s'.net (.frame sid e) ↔ s.net (.frame sid e)) :
    Star (SpecNext K C) (absState s) (absState s') := by
  rw [abs_eq_of s s' hst hns hfr]
  exact Star.refl _

theorem star_noSession {K : Kernel} {C : Config} (s s' : PState) (rid : RequestId) (tok : Token)
    (hs : C.sessions tok = none) (hr0 : (absState s).replies rid = none)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .reply rid .noSession))
    (hns : ∀ r, s'.noSession r = if r = rid then true else s.noSession r)
    (hst : s'.store = s.store) (hd : s.store.decided rid = none) :
    Star (SpecNext K C) (absState s) (absState s') := by
  have hab : absState s' = { absState s with
      replies := fun r => if r = rid then some .noSession else (absState s).replies r } := by
    apply SysState.ext'
    · funext x
      show s'.store.hist x (s'.store.rev x) = s.store.hist x (s.store.rev x)
      rw [hst]
    · show s'.store.evs = s.store.evs
      rw [hst]
    · funext r
      by_cases hr' : r = rid
      · subst hr'
        simp [absState, hst, hns, hd]
      · simp [absState, hst, hns, hr']
    · funext f
      apply propext
      exact frames_eq_of s s' (.reply rid .noSession) trivial hnet f.1 f.2
  rw [hab]
  exact star_one (SpecNext.noSession (absState s) rid tok hs hr0)

theorem star_accept {K : Kernel} {C : Config} (s s' : PState) (rid : RequestId) (tok : Token)
    (t : TenantId) (p : PrincipalId) (i : Intent) (t1 : Tenant) (base : Nat)
    (hsess : C.sessions tok = some (t, p)) (hr0 : (absState s).replies rid = none)
    (hvw : verdict K ((absState s).world t) p i = .ok t1)
    (hnet : ∀ y, s'.net y ↔
      (s.net y ∨ y = .done rid .accepted ∨ y = .committed (Event.mk t rid t1)))
    (hns : s'.noSession = s.noSession)
    (hhist : ∀ x k, s'.store.hist x k =
      if x = t ∧ k = base + 1 then some t1 else s.store.hist x k)
    (hrev : ∀ x, s'.store.rev x = if x = t then base + 1 else s.store.rev x)
    (hevs : s'.store.evs = s.store.evs ++ [Event.mk t rid t1])
    (hdec : ∀ r, s'.store.decided r = if r = rid then some .accepted else s.store.decided r)
    (hb : base = s.store.rev t) :
    Star (SpecNext K C) (absState s) (absState s') := by
  have hab : absState s' = serveState K (absState s) rid t p i := by
    apply SysState.ext'
    · funext x
      show s'.store.hist x (s'.store.rev x) = (serveState K (absState s) rid t p i).world x
      simp only [serveState, hvw]
      by_cases hx : x = t
      · rw [if_pos hx]
        have h1 : s'.store.rev x = base + 1 := by rw [hrev, if_pos hx]
        rw [h1, hhist, if_pos ⟨hx, rfl⟩]
      · rw [if_neg hx]
        have h1 : s'.store.rev x = s.store.rev x := by rw [hrev, if_neg hx]
        have hne : ¬ (x = t ∧ s.store.rev x = base + 1) := fun hc => hx hc.1
        rw [h1, hhist, if_neg hne]
        rfl
    · show s'.store.evs = (serveState K (absState s) rid t p i).log
      simp only [serveState, hvw]
      rw [hevs]
      rfl
    · funext r
      show (match s'.store.decided r with
        | some o => some o
        | none => if s'.noSession r then some Outcome.noSession else none) =
        (if r = rid then some (outcomeOf (verdict K ((absState s).world t) p i))
          else (absState s).replies r)
      rw [hvw]
      by_cases hr' : r = rid
      · subst hr'
        rw [hdec, if_pos rfl, if_pos rfl]
        rfl
      · rw [hdec, if_neg hr', if_neg hr', hns]
        rfl
    · funext f
      apply propext
      show s'.net (.frame f.1 f.2) ↔ s.net (.frame f.1 f.2)
      rw [hnet]
      constructor
      · rintro (h | h | h)
        · exact h
        · cases h
        · cases h
      · intro h
        exact Or.inl h
  rw [hab]
  exact star_one (SpecNext.serve (absState s) rid tok t p i hsess hr0)

theorem star_refuse {K : Kernel} {C : Config} (s s' : PState) (rid : RequestId) (tok : Token)
    (t : TenantId) (p : PrincipalId) (i : Intent) (e : Err) (base : Nat)
    (hsess : C.sessions tok = some (t, p)) (hr0 : (absState s).replies rid = none)
    (hvw : verdict K ((absState s).world t) p i = .error e)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .done rid (.refused e)))
    (hns : s'.noSession = s.noSession)
    (hdec : ∀ r, s'.store.decided r = if r = rid then some (.refused e) else s.store.decided r)
    (hhist : ∀ x k, s'.store.hist x k = s.store.hist x k)
    (hrev : ∀ x, s'.store.rev x = s.store.rev x)
    (hevs : s'.store.evs = s.store.evs) :
    Star (SpecNext K C) (absState s) (absState s') := by
  have hab : absState s' = serveState K (absState s) rid t p i := by
    apply SysState.ext'
    · funext x
      show s'.store.hist x (s'.store.rev x) = (serveState K (absState s) rid t p i).world x
      simp only [serveState, hvw]
      rw [hrev, hhist]
      rfl
    · show s'.store.evs = (serveState K (absState s) rid t p i).log
      simp only [serveState, hvw]
      exact hevs
    · funext r
      show (match s'.store.decided r with
        | some o => some o
        | none => if s'.noSession r then some Outcome.noSession else none) =
        (if r = rid then some (outcomeOf (verdict K ((absState s).world t) p i))
          else (absState s).replies r)
      rw [hvw]
      by_cases hr' : r = rid
      · subst hr'
        rw [hdec, if_pos rfl, if_pos rfl]
        rfl
      · rw [hdec, if_neg hr', if_neg hr', hns]
        rfl
    · funext f
      apply propext
      show s'.net (.frame f.1 f.2) ↔ s.net (.frame f.1 f.2)
      rw [hnet]
      constructor
      · rintro (h | h)
        · exact h
        · cases h
      · intro h
        exact Or.inl h
  rw [hab]
  exact star_one (SpecNext.serve (absState s) rid tok t p i hsess hr0)

theorem star_frame {K : Kernel} {C : Config} (s s' : PState) (sid : SubId) (e : Event)
    (t : TenantId) (p : PrincipalId)
    (hs : C.subs sid = some (t, p)) (he : e ∈ (absState s).log) (ht : e.tenant = t)
    (hr : K.readable e.post p)
    (hnet : ∀ y, s'.net y ↔ (s.net y ∨ y = .frame sid e))
    (hns : s'.noSession = s.noSession) (hst : s'.store = s.store) :
    Star (SpecNext K C) (absState s) (absState s') := by
  have hab : absState s' = { absState s with
      frames := fun f => (absState s).frames f ∨ f = (sid, e) } := by
    apply SysState.ext'
    · funext x
      show s'.store.hist x (s'.store.rev x) = s.store.hist x (s.store.rev x)
      rw [hst]
    · show s'.store.evs = s.store.evs
      rw [hst]
    · funext r
      show (match s'.store.decided r with
        | some o => some o
        | none => if s'.noSession r then some Outcome.noSession else none) =
        (match s.store.decided r with
        | some o => some o
        | none => if s.noSession r then some Outcome.noSession else none)
      rw [hst, hns]
    · funext f
      apply propext
      show s'.net (.frame f.1 f.2) ↔ (s.net (.frame f.1 f.2) ∨ f = (sid, e))
      rw [hnet, frame_eq_iff]
  rw [hab]
  exact star_one (SpecNext.frame (absState s) sid e t p hs he ht hr)

/-- Every protocol step maps to zero steps of the specification, or to the one specification step
    that it linearises: a store decision answers a request, and a publication sends a frame. -/
theorem refine_star {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) : Star (SpecNext K C) (absState s) (absState s') := by
  cases h with
  | clientReq rid tok i =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.req rid tok i) trivial (fun _ => Iff.rfl))
  | bffForward rid tok i h =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.fwd rid tok i) trivial (fun _ => Iff.rfl))
  | bffRelay rid o h =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.clientReply rid o) trivial (fun _ => Iff.rfl))
  | begin rid tok i t p h hx hn hs =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.load rid t (s.attempt rid)) trivial (fun _ => Iff.rfl))
  | noSession rid tok i h hx hn hs =>
    have hd : s.store.decided rid = none := by
      cases hdd : s.store.decided rid with
      | none => rfl
      | some o =>
        obtain ⟨y, hy⟩ := hg.decidedTxn rid o hdd
        rw [hx] at hy
        cases hy
    have hr0 : (absState s).replies rid = none := by
      show (match s.store.decided rid with
        | some o => some o
        | none => if s.noSession rid then some Outcome.noSession else none) = none
      rw [hd, hn]
      rfl
    exact star_noSession s _ rid tok hs hr0 (fun _ => Iff.rfl) (fun _ => rfl) rfl hd
  | snap rid t v base x h hx ht hl =>
    exact star_eq_abs rfl rfl (fun sid e => Iff.rfl)
  | commit rid x v base hx hp =>
    exact star_eq_abs rfl rfl
      (frames_eq_of _ _ (.commit rid x.tok x.tenant x.principal x.intent base
        (verdict K v x.principal x.intent)) trivial (fun _ => Iff.rfl))
  | conflict rid base x v h hx hp =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.load rid x.tenant (s.attempt rid + 1)) trivial (fun _ => Iff.rfl))
  | done rid o x h hx =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.reply rid o) trivial (fun _ => Iff.rfl))
  | load rid t a h =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _
      (.snap rid t (s.store.hist t (s.store.rev t)) (s.store.rev t) a) trivial (fun _ => Iff.rfl))
  | storeDecided rid tok t p i base res o h hd =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.done rid o) trivial (fun _ => Iff.rfl))
  | storeConflict rid tok t p i base res h hd hb =>
    exact star_eq_abs rfl rfl (frames_eq_of _ _ (.conflict rid base) trivial (fun _ => Iff.rfl))
  | storeAccept rid tok t p i base res t1 h hd hb hr =>
    obtain ⟨x, hxs, h1, h2, h3, -⟩ := hg.commitTxn rid tok t p i base res h
    have hsess : C.sessions tok = some (t, p) := by
      have := hg.sessionTxn rid x hxs
      rw [h1, h2, h3] at this
      exact this
    have hns' : s.noSession rid = false := by
      cases hb' : s.noSession rid
      · rfl
      · have := hg.noSessionTxn rid hb'
        rw [hxs] at this
        cases this
    obtain ⟨v, _, hsnap, hv⟩ := hg.commitCorrect rid tok t p i base res h
    have hvh := hg.snapHist rid t v base _ hsnap
    have hvw : verdict K ((absState s).world t) p i = .ok t1 := by
      show verdict K (s.store.hist t (s.store.rev t)) p i = _
      rw [← hb, hvh.2]
      exact hv.symm.trans hr
    have hr0 : (absState s).replies rid = none := by
      show (match s.store.decided rid with
        | some o => some o
        | none => if s.noSession rid then some Outcome.noSession else none) = none
      rw [hd, hns']
      rfl
    exact star_accept s _ rid tok t p i t1 base hsess hr0 hvw (fun _ => Iff.rfl) rfl
      (fun _ _ => rfl) (fun _ => rfl) rfl (fun _ => rfl) hb
  | storeRefuse rid tok t p i base res e h hd hb hr =>
    obtain ⟨x, hxs, h1, h2, h3, -⟩ := hg.commitTxn rid tok t p i base res h
    have hsess : C.sessions tok = some (t, p) := by
      have := hg.sessionTxn rid x hxs
      rw [h1, h2, h3] at this
      exact this
    have hns' : s.noSession rid = false := by
      cases hb' : s.noSession rid
      · rfl
      · have := hg.noSessionTxn rid hb'
        rw [hxs] at this
        cases this
    obtain ⟨v, _, hsnap, hv⟩ := hg.commitCorrect rid tok t p i base res h
    have hvh := hg.snapHist rid t v base _ hsnap
    have hvw : verdict K ((absState s).world t) p i = .error e := by
      show verdict K (s.store.hist t (s.store.rev t)) p i = _
      rw [← hb, hvh.2]
      exact hv.symm.trans hr
    have hr0 : (absState s).replies rid = none := by
      show (match s.store.decided rid with
        | some o => some o
        | none => if s.noSession rid then some Outcome.noSession else none) = none
      rw [hd, hns']
      rfl
    exact star_refuse s _ rid tok t p i e base hsess hr0 hvw (fun _ => Iff.rfl) rfl
      (fun _ => rfl) (fun _ _ => rfl) (fun _ => rfl) rfl
  | publish sid e t p h hs ht hr =>
    exact star_frame s _ sid e t p hs (hg.committedIn e h) ht hr (fun _ => Iff.rfl) rfl rfl


/-- Every reply the BFF relays to a client was sent by the backend to the BFF. -/
theorem relayed_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (hr : ∀ rid o, s.net (.clientReply rid o) → s.net (.reply rid o)) :
    ∀ rid o, s'.net (.clientReply rid o) → s'.net (.reply rid o) := by
  intro rid o hc
  cases h
  case bffRelay r o' h0 =>
    rcases hc with hc | hc
    · exact Or.inl (hr _ _ hc)
    · cases hc
      exact Or.inl h0
  case storeAccept rid tok t p i base res t1 hh hd hb hr' =>
    rcases hc with hc | hc | hc
    · exact Or.inl (hr _ _ hc)
    · cases hc
    · cases hc
  case storeRefuse rid tok t p i base res e hh hd hb hr' =>
    rcases hc with hc | hc
    · exact Or.inl (hr _ _ hc)
    · cases hc
  case snap rid t v base x hh hx ht hl =>
    exact hr _ _ hc
  all_goals
    rcases hc with hc | hc
    · exact Or.inl (hr _ _ hc)
    · cases hc

/-- Every reachable state satisfies the invariant, the specification is reached, and every relayed
    reply was sent. -/
theorem protocol_refines {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) :
    Good K C s ∧ SpecReachable K C w0 (absState s) ∧
      (∀ rid o, s.net (.clientReply rid o) → s.net (.reply rid o)) := by
  obtain ⟨s0, hi, hr⟩ := h
  have key : ∀ s', Star (PNext K C) s0 s' → Good K C s' ∧
      Steps (SpecNext K C) (absState s0) (absState s') ∧
      (∀ rid o, s'.net (.clientReply rid o) → s'.net (.reply rid o)) := by
    intro s' hs'
    induction hs' with
    | refl =>
      refine ⟨good_init hi, Steps.refl, ?_⟩
      intro rid o hc
      exact absurd hc (hi.1 _)
    | tail _ hstep ih =>
      obtain ⟨hg, hsp, hrl⟩ := ih
      exact ⟨good_step hstep hg, steps_star hsp (refine_star hstep hg),
        relayed_step hstep hrl⟩
  obtain ⟨hg, hsp, hrl⟩ := key s hr
  exact ⟨hg, ⟨absState s0, abs_init hi, hsp⟩, hrl⟩

/-- The replies a client observes are the store's decisions, and the answers to requests without a
    session. -/
theorem observed_reply_is_decision {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) (rid : RequestId) (o : Outcome)
    (hc : s.net (.clientReply rid o)) :
    s.store.decided rid = some o ∨ (o = .noSession ∧ s.noSession rid = true) := by
  obtain ⟨hg, -, hrl⟩ := protocol_refines h
  exact hg.replyDecided rid o (hrl rid o hc)

/-- What a client observes is what the specification says: the reply of each request is the
    specification's, and each frame is a frame of the specification. -/
theorem observed_matches_spec {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) :
    (∀ rid o, s.net (.clientReply rid o) → (absState s).replies rid = some o) ∧
    (∀ sid e, s.net (.frame sid e) → (absState s).frames (sid, e)) := by
  obtain ⟨hg, -, hrl⟩ := protocol_refines h
  constructor
  · intro rid o hc
    rcases hg.replyDecided rid o (hrl rid o hc) with h1 | ⟨h1, h2⟩
    · show (match s.store.decided rid with
        | some o => some o
        | none => if s.noSession rid then some Outcome.noSession else none) = some o
      rw [h1]
    · have hdn : s.store.decided rid = none := by
        cases hdd : s.store.decided rid with
        | none => rfl
        | some o' =>
          obtain ⟨y, hy⟩ := hg.decidedTxn rid o' hdd
          rw [hg.noSessionTxn rid h2] at hy
          cases hy
      show (match s.store.decided rid with
        | some o => some o
        | none => if s.noSession rid then some Outcome.noSession else none) = some o
      rw [hdn, h2, h1]
      rfl
  · intro sid e hc
    exact hc

/-- A subscriber observes an event only if it was committed, its subscription names the tenant,
    and its principal may read the tenant after the change. -/
theorem frames_readable {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) (sid : SubId) (e : Event)
    (hc : s.net (.frame sid e)) :
    e ∈ s.store.evs ∧ ∃ t p, C.subs sid = some (t, p) ∧ e.tenant = t ∧ K.readable e.post p := by
  obtain ⟨hg, hsp, -⟩ := protocol_refines h
  exact spec_frames_readable K C w0 (absState s) hsp (sid, e) hc

/-- Duplicate deliveries do not create duplicate commits: a request has at most one committed
    event, in every reachable state. -/
theorem commit_once {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) :
    ∀ e1 ∈ s.store.evs, ∀ e2 ∈ s.store.evs, e1.rid = e2.rid → e1 = e2 := by
  obtain ⟨hg, hsp, -⟩ := protocol_refines h
  exact (spec_log_unique K C w0 (absState s) hsp).2

/-- A commit names the kernel's verdict on the tenant at the revision it was based on, and that
    revision is no later than the current one. -/
theorem commit_is_verdict {K : Kernel} {C : Config} {w0 : World} {s : PState}
    (h : ProtoReachable K C w0 s) (rid : RequestId) (tok : Token) (t : TenantId)
    (p : PrincipalId) (i : Intent) (base : Nat) (res : Except Err Tenant)
    (hc : s.net (.commit rid tok t p i base res)) :
    base ≤ s.store.rev t ∧ res = verdict K (s.store.hist t base) p i := by
  obtain ⟨hg, -, -⟩ := protocol_refines h
  obtain ⟨v, _, hsnap, hv⟩ := hg.commitCorrect rid tok t p i base res hc
  obtain ⟨hb, hvv⟩ := hg.snapHist rid t v base _ hsnap
  exact ⟨hb, by rw [hvv]; exact hv⟩

/-- One protocol step moves each tenant's revision by nothing or by one, and only a commit that
    the store applies moves it. -/
theorem rev_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (x : TenantId) :
    s'.store.rev x = s.store.rev x ∨ s'.store.rev x = s.store.rev x + 1 := by
  cases h
  case storeAccept rid tok t p i base res t1 hh hd hb hr =>
    by_cases hxt : x = t
    · right
      simp [hxt, hb]
    · left
      simp [hxt]
  all_goals exact Or.inl rfl

/-- A commit for a request the store has already decided changes nothing in the store. -/
theorem duplicate_commit_inert {K : Kernel} {C : Config} (s : PState) (rid : RequestId)
    (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent) (base : Nat)
    (res : Except Err Tenant) (o : Outcome)
    (h : s.net (.commit rid tok t p i base res)) (hd : s.store.decided rid = some o) :
    PNext K C s (sendOne s (.done rid o)) ∧ (sendOne s (.done rid o)).store = s.store :=
  ⟨PNext.storeDecided s rid tok t p i base res o h hd, rfl⟩

/-! ## A non-vacuity witness -/

/-- One tenant, owned by principal 7, with no records. -/
def wTenant : Tenant :=
  { types := [(1, { schema := [] })], owner := 7, editors := [], writers := [], nextId := 0 }

/-- The kernel: the step of the kernel's transition function, and a principal reads the tenant when
    it may create records in it. -/
def wKernel : Kernel :=
  { step := stepTenant, readable := fun t p => canCreate t p = true }

/-- Token 0 belongs to principal 9, a stranger to the tenant. -/
def wConfig : Config :=
  { sessions := fun tok => if tok = 0 then some (0, 9) else none, subs := fun _ => none }

def wWorld : World := fun t => if t = 0 then some wTenant else none

def wInit : PState :=
  { net := fun _ => False
    txns := fun _ => none
    noSession := fun _ => false
    attempt := fun _ => 0
    store :=
      { hist := fun t _ => wWorld t
        rev := fun _ => 0
        evs := []
        decided := fun _ => none } }

theorem wInit_pinit : PInit wWorld wInit :=
  ⟨fun _ h => h, fun _ => rfl, fun _ => rfl, fun _ _ => rfl, fun _ => rfl, rfl, fun _ => rfl, fun _ => rfl⟩
theorem refusal_reachable : ∃ s, ProtoReachable wKernel wConfig wWorld s ∧
    s.net (.clientReply 1 (.refused .unauthorized)) :=
  ⟨_, ⟨wInit, wInit_pinit, Star.tail (Star.tail (Star.tail (Star.tail (Star.tail (Star.tail
      (Star.tail (Star.tail (Star.tail (Star.refl wInit)
        (PNext.clientReq _ 1 0 (.create 1 [])))
        (PNext.bffForward _ 1 0 (.create 1 []) (Or.inr rfl)))
        (PNext.begin _ 1 0 (.create 1 []) 0 9 (Or.inr rfl) rfl rfl rfl))
        (PNext.load _ 1 0 0 (Or.inr rfl)))
        (PNext.snap _ 1 _ _ 0 _ _ (Or.inr rfl) rfl rfl rfl rfl))
        (PNext.commit _ 1 _ _ 0 rfl rfl))
        (PNext.storeRefuse _ 1 0 0 9 (.create 1 []) 0 _ .unauthorized (Or.inr rfl) rfl rfl rfl))
        (PNext.done _ 1 (.refused .unauthorized) _ (Or.inr rfl) rfl))
        (PNext.bffRelay _ 1 (.refused .unauthorized) (Or.inr rfl))⟩,
    Or.inr rfl⟩

end Strata.System
