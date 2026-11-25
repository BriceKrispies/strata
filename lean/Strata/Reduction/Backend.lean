/-
The backend as an IronFleet-shaped protocol, and the instantiation of the reduction (WG-5660, part
(b), the second half).

Hosts of the protocol in `Strata/System/Protocol.lean`: the client, the BFF, the backend, the store
and the outbox publisher. Each host owns part of the protocol's state. The backend owns the
transactions and the answers given without a session; the store owns the tenant's history and its
decisions; the other hosts own nothing. Each protocol step (`PNext`) changes the state of one host
only and reads only that host's state, the packets it receives and the static configuration, so it
is a host step in the IronFleet sense.

`RStep` is the host step relation, one constructor per protocol step, with the host's own state
before and after, the packets it receives (at most one) and the packets it sends. `FineExec` of
`RStep` is a fine-grained execution of the backend: every host step is split into its I/O events,
and events of different hosts interleave arbitrarily. The reduction (`Distributed.lean`) reorders
any such execution so that each host step is one block, and the blocks in order are protocol steps.

Main theorem: `backend_fine_refines`. Every fine-grained execution of the backend refines a
protocol behaviour (`ProtoReachable`), and so a behaviour of the system spec (`SpecReachable`,
through `protocol_refines`).

A host step that does nothing (no receive, no send, no change) is `RStep.idle`. It is a step of the
fine model and corresponds to no protocol step at all. The protocol has no such step, but it is
harmless: a host that does nothing in a step is not observable.

No unfinished proof and no added axiom.
-/

import Strata.System.Protocol
import Strata.Reduction.Distributed

namespace Strata.Reduction

open Strata Strata.System

/-- The hosts of the backend's protocol. -/
inductive Role where
  | client
  | bff
  | backend
  | store
  | publisher
  deriving DecidableEq

/-- The state one host owns. -/
structure Own where
  txns : RequestId → Option Txn
  noS : RequestId → Bool
  store : StoreState

/-- The owned state of every host at the start: the protocol's initial state. -/
def ownInit (w0 : World) : Own :=
  { txns := fun _ => none
    noS := fun _ => false
    store := { hist := fun t _ => w0 t, rev := fun _ => 0, evs := [], decided := fun _ => none } }

/-- One host step: the host `r` with owned state `s` receives `rs` (none, or one packet), takes no
    time-dependent input (`tk` is `none`), becomes `s'` and sends `ss`. -/
inductive RStep (K : Kernel) (C : Config) :
    Role → Own → List Packet → Option Unit → Own → List Packet → Prop
  /-- A host step with no receive, no send and no change. -/
  | idle (r : Role) (s : Own) : RStep K C r s [] none s []
  /-- The client sends a request. -/
  | clientReq (s : Own) (rid : RequestId) (tok : Token) (i : Intent) :
      RStep K C .client s [] none s [.req rid tok i]
  /-- The BFF forwards a request. -/
  | bffForward (s : Own) (rid : RequestId) (tok : Token) (i : Intent) :
      RStep K C .bff s [.req rid tok i] none s [.fwd rid tok i]
  /-- The BFF relays a reply. -/
  | bffRelay (s : Own) (rid : RequestId) (o : Outcome) :
      RStep K C .bff s [.reply rid o] none s [.clientReply rid o]
  /-- The backend starts a transaction for a request with a session. -/
  | begin (s : Own) (rid : RequestId) (tok : Token) (i : Intent) (t : TenantId) (p : PrincipalId)
      (hx : s.txns rid = none) (hn : s.noS rid = false) (hs : C.sessions tok = some (t, p)) :
      RStep K C .backend s [.fwd rid tok i] none
        { s with txns := updTxn s.txns rid ⟨tok, t, p, i, .loading⟩ } [.load rid t]
  /-- The backend answers a request whose token has no session. -/
  | noSession (s : Own) (rid : RequestId) (tok : Token) (i : Intent)
      (hx : s.txns rid = none) (hn : s.noS rid = false) (hs : C.sessions tok = none) :
      RStep K C .backend s [.fwd rid tok i] none
        { s with noS := fun r => if r = rid then true else s.noS r } [.reply rid .noSession]
  /-- The backend receives a snapshot. -/
  | snap (s : Own) (rid : RequestId) (t : TenantId) (v : Option Tenant) (base : Nat) (x : Txn)
      (hx : s.txns rid = some x) (ht : x.tenant = t) (hl : x.phase = .loading) :
      RStep K C .backend s [.snap rid t v base] none
        { s with txns := updTxn s.txns rid { x with phase := .ready v base } } []
  /-- The backend computes the verdict and sends the commit. -/
  | commit (s : Own) (rid : RequestId) (x : Txn) (v : Option Tenant) (base : Nat)
      (hx : s.txns rid = some x) (hp : x.phase = .ready v base) :
      RStep K C .backend s [] none
        { s with txns := updTxn s.txns rid { x with phase := .committing v base } }
        [.commit rid x.tok x.tenant x.principal x.intent base (verdict K v x.principal x.intent)]
  /-- The store refused a commit on an old revision: the backend loads again. -/
  | conflict (s : Own) (rid : RequestId) (base : Nat) (x : Txn) (v : Option Tenant)
      (hx : s.txns rid = some x) (hp : x.phase = .committing v base) :
      RStep K C .backend s [.conflict rid base] none
        { s with txns := updTxn s.txns rid { x with phase := .loading } } [.load rid x.tenant]
  /-- The store decided the request: the backend replies. -/
  | done (s : Own) (rid : RequestId) (o : Outcome) (x : Txn) (hx : s.txns rid = some x) :
      RStep K C .backend s [.done rid o] none
        { s with txns := updTxn s.txns rid { x with phase := .finished o } } [.reply rid o]
  /-- The store answers a load with the tenant's current state and revision. -/
  | load (s : Own) (rid : RequestId) (t : TenantId) :
      RStep K C .store s [.load rid t] none s
        [.snap rid t (s.store.hist t (s.store.rev t)) (s.store.rev t)]
  /-- A commit for a request the store has decided: the decision is sent again. -/
  | storeDecided (s : Own) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (o : Outcome)
      (hd : s.store.decided rid = some o) :
      RStep K C .store s [.commit rid tok t p i base res] none s [.done rid o]
  /-- A commit on an old revision: a conflict. -/
  | storeConflict (s : Own) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant)
      (hd : s.store.decided rid = none) (hb : base ≠ s.store.rev t) :
      RStep K C .store s [.commit rid tok t p i base res] none s [.conflict rid base]
  /-- A commit of an accepted verdict on the current revision: the tenant moves on. -/
  | storeAccept (s : Own) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (t' : Tenant)
      (hd : s.store.decided rid = none) (hb : base = s.store.rev t) (hr : res = .ok t') :
      RStep K C .store s [.commit rid tok t p i base res] none
        { s with store :=
            { hist := fun x k => if x = t ∧ k = base + 1 then some t' else s.store.hist x k
              rev := fun x => if x = t then base + 1 else s.store.rev x
              evs := s.store.evs ++ [Event.mk t rid t']
              decided := fun r => if r = rid then some .accepted else s.store.decided r } }
        [.done rid .accepted, .committed (Event.mk t rid t')]
  /-- A commit of a refusal on the current revision: only the decision changes. -/
  | storeRefuse (s : Own) (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId)
      (i : Intent) (base : Nat) (res : Except Err Tenant) (e : Err)
      (hd : s.store.decided rid = none) (hb : base = s.store.rev t) (hr : res = .error e) :
      RStep K C .store s [.commit rid tok t p i base res] none
        { s with store := { s.store with
            decided := fun r => if r = rid then some (.refused e) else s.store.decided r } }
        [.done rid (.refused e)]
  /-- The publisher sends a committed event to a subscriber whose principal may read it. -/
  | publish (s : Own) (sid : SubId) (e : Event) (t : TenantId) (p : PrincipalId)
      (hs : C.subs sid = some (t, p)) (ht : e.tenant = t) (hr : K.readable e.post p) :
      RStep K C .publisher s [.committed e] none s [.frame sid e]

/-- A host step has no time-dependent input. -/
theorem rstep_tk {K : Kernel} {C : Config} {r : Role} {s s' : Own} {rs ss : List Packet}
    {tk : Option Unit} (h : RStep K C r s rs tk s' ss) : tk = none := by
  cases h <;> rfl

/-- A host step that receives and sends nothing changes nothing. -/
theorem rstep_empty {K : Kernel} {C : Config} {r : Role} {s s' : Own}
    (h : RStep K C r s [] none s' []) : s' = s := by
  cases h
  · rfl
  all_goals simp_all

/-! ## Each host step is a protocol step -/

/-- The backend's host step realised by the protocol's steps: the protocol moves from `G` to `G'`
    with the backend's state `s` replaced by `s'` and the packets `ss` added to the network; the
    other hosts' state is untouched. -/
theorem bridge_backend {K : Kernel} {C : Config} {s s' : Own} {rs ss : List Packet}
    (h : RStep K C .backend s rs none s' ss) (G : PState)
    (hT : G.txns = s.txns) (hN : G.noSession = s.noS) (hnet : ∀ p ∈ rs, G.net p) :
    ∃ G', Star (PNext K C) G G' ∧ (∀ q, G'.net q ↔ (G.net q ∨ q ∈ ss)) ∧
      G'.txns = s'.txns ∧ G'.noSession = s'.noS ∧ G'.store = G.store := by
  cases h with
  | idle _ _ => exact ⟨G, Star.refl G, fun q => by simp, hT, hN, rfl⟩
  | begin s rid tok i t p hx hn hs =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.begin G rid tok i t p (hnet _ (by simp))
      (by rw [hT]; exact hx) (by rw [hN]; exact hn) hs), ?_, ?_, ?_, rfl⟩
    · intro q; simp [sendOne]
    · show updTxn G.txns rid ⟨tok, t, p, i, .loading⟩ = updTxn s.txns rid ⟨tok, t, p, i, .loading⟩
      rw [hT]
    · exact hN
  | noSession s rid tok i hx hn hs =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.noSession G rid tok i (hnet _ (by simp))
      (by rw [hT]; exact hx) (by rw [hN]; exact hn) hs), ?_, ?_, ?_, rfl⟩
    · intro q; simp [sendOne]
    · exact hT
    · show (fun r => if r = rid then true else G.noSession r) =
        (fun r => if r = rid then true else s.noS r)
      rw [hN]
  | snap s rid t v base x hx ht hl =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.snap G rid t v base x (hnet _ (by simp))
      (by rw [hT]; exact hx) ht hl), ?_, ?_, ?_, rfl⟩
    · intro q; simp
    · show updTxn G.txns rid { x with phase := .ready v base } =
        updTxn s.txns rid { x with phase := .ready v base }
      rw [hT]
    · exact hN
  | commit s rid x v base hx hp =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.commit G rid x v base (by rw [hT]; exact hx) hp),
      ?_, ?_, ?_, rfl⟩
    · intro q; simp [sendOne]
    · show updTxn G.txns rid { x with phase := .committing v base } =
        updTxn s.txns rid { x with phase := .committing v base }
      rw [hT]
    · exact hN
  | conflict s rid base x v hx hp =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.conflict G rid base x v (hnet _ (by simp))
      (by rw [hT]; exact hx) hp), ?_, ?_, ?_, rfl⟩
    · intro q; simp [sendOne]
    · show updTxn G.txns rid { x with phase := .loading } =
        updTxn s.txns rid { x with phase := .loading }
      rw [hT]
    · exact hN
  | done s rid o x hx =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.done G rid o x (hnet _ (by simp))
      (by rw [hT]; exact hx)), ?_, ?_, ?_, rfl⟩
    · intro q; simp [sendOne]
    · show updTxn G.txns rid { x with phase := .finished o } =
        updTxn s.txns rid { x with phase := .finished o }
      rw [hT]
    · exact hN

/-- The store's host step realised by the protocol's steps. -/
theorem bridge_store {K : Kernel} {C : Config} {s s' : Own} {rs ss : List Packet}
    (h : RStep K C .store s rs none s' ss) (G : PState)
    (hS : G.store = s.store) (hnet : ∀ p ∈ rs, G.net p) :
    ∃ G', Star (PNext K C) G G' ∧ (∀ q, G'.net q ↔ (G.net q ∨ q ∈ ss)) ∧
      G'.store = s'.store ∧ G'.txns = G.txns ∧ G'.noSession = G.noSession := by
  cases h with
  | idle _ _ => exact ⟨G, Star.refl G, fun q => by simp, hS, rfl, rfl⟩
  | load s rid t =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.load G rid t (hnet _ (by simp))), ?_, ?_, rfl, rfl⟩
    · intro q; simp [sendOne, hS]
    · show G.store = s.store
      exact hS
  | storeDecided s rid tok t p i base res o hd =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.storeDecided G rid tok t p i base res o
      (hnet _ (by simp)) (by rw [hS]; exact hd)), ?_, ?_, rfl, rfl⟩
    · intro q; simp [sendOne]
    · show G.store = s.store
      exact hS
  | storeConflict s rid tok t p i base res hd hb =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.storeConflict G rid tok t p i base res
      (hnet _ (by simp)) (by rw [hS]; exact hd) (by rw [hS]; exact hb)), ?_, ?_, rfl, rfl⟩
    · intro q; simp [sendOne]
    · show G.store = s.store
      exact hS
  | storeAccept s rid tok t p i base res t' hd hb hr =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.storeAccept G rid tok t p i base res t'
      (hnet _ (by simp)) (by rw [hS]; exact hd) (by rw [hS]; exact hb) hr), ?_, ?_, rfl, rfl⟩
    · intro q; simp
    · dsimp only
      rw [hS]
  | storeRefuse s rid tok t p i base res e hd hb hr =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.storeRefuse G rid tok t p i base res e
      (hnet _ (by simp)) (by rw [hS]; exact hd) (by rw [hS]; exact hb) hr), ?_, ?_, rfl, rfl⟩
    · intro q; simp [sendOne]
    · dsimp only
      rw [hS]

/-- A host step of a host that owns no state: the protocol's step, with the state of the other
    hosts untouched. -/
theorem bridge_other {K : Kernel} {C : Config} {r : Role} (hb : r ≠ .backend) (hst : r ≠ .store)
    {s s' : Own} {rs ss : List Packet}
    (h : RStep K C r s rs none s' ss) (G : PState) (hnet : ∀ p ∈ rs, G.net p) :
    ∃ G', Star (PNext K C) G G' ∧ (∀ q, G'.net q ↔ (G.net q ∨ q ∈ ss)) ∧
      G'.txns = G.txns ∧ G'.noSession = G.noSession ∧ G'.store = G.store := by
  cases h with
  | idle _ _ => exact ⟨G, Star.refl G, fun q => by simp, rfl, rfl, rfl⟩
  | clientReq s rid tok i =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.clientReq G rid tok i), ?_, rfl, rfl, rfl⟩
    intro q; simp [sendOne]
  | bffForward s rid tok i =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.bffForward G rid tok i (hnet _ (by simp))),
      ?_, rfl, rfl, rfl⟩
    intro q; simp [sendOne]
  | bffRelay s rid o =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.bffRelay G rid o (hnet _ (by simp))), ?_, rfl, rfl, rfl⟩
    intro q; simp [sendOne]
  | begin => exact absurd rfl hb
  | noSession => exact absurd rfl hb
  | snap => exact absurd rfl hb
  | commit => exact absurd rfl hb
  | conflict => exact absurd rfl hb
  | done => exact absurd rfl hb
  | load => exact absurd rfl hst
  | storeDecided => exact absurd rfl hst
  | storeConflict => exact absurd rfl hst
  | storeAccept => exact absurd rfl hst
  | storeRefuse => exact absurd rfl hst
  | publish s sid e t p hs ht hr =>
    refine ⟨_, Star.tail (Star.refl G) (PNext.publish G sid e t p (hnet _ (by simp)) hs ht hr),
      ?_, rfl, rfl, rfl⟩
    intro q; simp [sendOne]

/-! ## Helper lemmas -/

theorem sendIff {H P T : Type} (a : Ev H P T) (q : P) :
    (match a.io with | .send p => some p | _ => none) = some q ↔ a.io = .send q := by
  rcases a with ⟨h, k, io⟩
  cases io <;> simp

theorem recvIff {H P T : Type} (a : Ev H P T) (q : P) :
    (match a.io with | .recv p => some p | _ => none) = some q ↔ a.io = .recv q := by
  rcases a with ⟨h, k, io⟩
  cases io <;> simp

theorem mem_sendsOf {H P T : Type} (l : List (Ev H P T)) (q : P) :
    q ∈ sendsOf l ↔ ∃ y ∈ l, y.io = .send q := by
  unfold sendsOf
  rw [List.mem_filterMap]
  constructor
  · rintro ⟨a, ha, hf⟩
    exact ⟨a, ha, (sendIff a q).mp hf⟩
  · rintro ⟨a, ha, hio⟩
    exact ⟨a, ha, (sendIff a q).mpr hio⟩

theorem mem_recvsOf {H P T : Type} (l : List (Ev H P T)) (q : P) :
    q ∈ recvsOf l ↔ ∃ y ∈ l, y.io = .recv q := by
  unfold recvsOf
  rw [List.mem_filterMap]
  constructor
  · rintro ⟨a, ha, hf⟩
    exact ⟨a, ha, (recvIff a q).mp hf⟩
  · rintro ⟨a, ha, hio⟩
    exact ⟨a, ha, (recvIff a q).mpr hio⟩

/-- In a step whose events are in obligation order, each receive is available from the packets
    available before the step: the receives come before any send of the step. -/
theorem recv_of_obl {H P T : Type} : ∀ (l : List (Ev H P T)) (S : P → Prop),
    l.Pairwise (fun a b => rank a.io ≤ rank b.io) → NetOk S l →
      ∀ y ∈ l, ∀ p, y.io = .recv p → S p
  | [], _, _, _, y, hy, _, _ => by simp at hy
  | e :: es, S, hp, h, y, hy, p, hyp => by
    have h' : (∀ q, e.io = .recv q → S q) ∧
        NetOk (fun q => S q ∨ e.io = .send q) es := h
    have hp2 := List.pairwise_cons.mp hp
    rcases List.mem_cons.mp hy with rfl | hy'
    · exact h'.1 p hyp
    · have hr : rank e.io ≤ rank y.io := hp2.1 y hy'
      rw [hyp] at hr
      have hrecv_e : ∀ q, e.io ≠ .send q := by
        intro q hq
        rw [hq] at hr
        simp [rank] at hr
      have key := recv_of_obl es (fun q => S q ∨ e.io = .send q) hp2.2 h'.2 y hy' p hyp
      exact key.elim id (fun h4 => absurd h4 (hrecv_e p))

/-- A gap of empty steps leaves a host's state unchanged. -/
theorem eq_of_gap {α : Type} (f : Nat → α) (c k : Nat) (hc : c ≤ k)
    (hf : ∀ j, c ≤ j → j < k → f (j + 1) = f j) : f k = f c := by
  have key : ∀ d, (∀ j, c ≤ j → j < c + d → f (j + 1) = f j) → f (c + d) = f c := by
    intro d
    induction d with
    | zero => intro _; rfl
    | succ d ih =>
      intro hd
      have h1 := hd (c + d) (by omega) (by omega)
      rw [show c + (d + 1) = c + d + 1 from rfl, h1]
      exact ih (fun j hj1 hj2 => hd j hj1 (by omega))
  obtain ⟨d, rfl⟩ := Nat.exists_eq_add_of_le hc
  exact key d (fun j hj1 hj2 => hf j hj1 (by omega))

theorem star_trans {α : Type} {r : α → α → Prop} {a b c : α} (h1 : Star r a b)
    (h2 : Star r b c) : Star r a c := by
  induction h2 with
  | refl => exact h1
  | tail _ hstep ih => exact Star.tail ih hstep

/-- The protocol's initial state, for the world `w0`. -/
def initState (w0 : World) : PState :=
  { net := fun _ => False
    txns := fun _ => none
    noSession := fun _ => false
    store := { hist := fun t _ => w0 t, rev := fun _ => 0, evs := [], decided := fun _ => none } }

theorem initState_pinit (w0 : World) : PInit w0 (initState w0) :=
  ⟨fun _ h => h, fun _ => rfl, fun _ => rfl, fun _ _ => rfl, fun _ => rfl, rfl, fun _ => rfl⟩

/-- The owned states of the backend and the store, as the protocol state shows them. -/
def Glob (st : Role → Nat → Own) (c : Role → Nat) (G : PState) : Prop :=
  G.txns = (st .backend (c .backend)).txns ∧ G.noSession = (st .backend (c .backend)).noS ∧
    G.store = (st .store (c .store)).store

/-- The counters, with one host's counter set to `v`. -/
def cupd (c : Role → Nat) (r : Role) (v : Nat) : Role → Nat :=
  fun x => if x = r then v else c x

/-! ## One host step, in the protocol state -/

/-- The protocol moves through one host step of role `r`: the owned states of the protocol state
    follow the host's step, and its network gains the packets sent. -/
theorem step_role (K : Kernel) (C : Config) (st : Role → Nat → Own) (c : Role → Nat) (r : Role)
    (k : Nat) (G : PState) (hG : Glob st c G) (hst : st r (c r) = st r k)
    (rs ss : List Packet) (hR : RStep K C r (st r k) rs none (st r (k + 1)) ss)
    (hrecv : ∀ p ∈ rs, G.net p) :
    ∃ G', Star (PNext K C) G G' ∧ (∀ q, G'.net q ↔ (G.net q ∨ q ∈ ss)) ∧
      Glob st (cupd c r (k + 1)) G' := by
  cases r with
  | client =>
    obtain ⟨G', hs, hn, htx, hno, hst'⟩ := bridge_other (by decide) (by decide) hR G hrecv
    exact ⟨G', hs, hn, by
      refine ⟨?_, ?_, ?_⟩
      · rw [htx]; simpa [cupd] using hG.1
      · rw [hno]; simpa [cupd] using hG.2.1
      · rw [hst']; simpa [cupd] using hG.2.2⟩
  | bff =>
    obtain ⟨G', hs, hn, htx, hno, hst'⟩ := bridge_other (by decide) (by decide) hR G hrecv
    exact ⟨G', hs, hn, by
      refine ⟨?_, ?_, ?_⟩
      · rw [htx]; simpa [cupd] using hG.1
      · rw [hno]; simpa [cupd] using hG.2.1
      · rw [hst']; simpa [cupd] using hG.2.2⟩
  | publisher =>
    obtain ⟨G', hs, hn, htx, hno, hst'⟩ := bridge_other (by decide) (by decide) hR G hrecv
    exact ⟨G', hs, hn, by
      refine ⟨?_, ?_, ?_⟩
      · rw [htx]; simpa [cupd] using hG.1
      · rw [hno]; simpa [cupd] using hG.2.1
      · rw [hst']; simpa [cupd] using hG.2.2⟩
  | backend =>
    have hT : G.txns = (st .backend k).txns := by rw [hG.1, hst]
    have hN : G.noSession = (st .backend k).noS := by rw [hG.2.1, hst]
    obtain ⟨G', hs, hn, htx, hno, hsto⟩ := bridge_backend hR G hT hN hrecv
    refine ⟨G', hs, hn, ?_, ?_, ?_⟩
    · simp only [cupd, if_true]; exact htx
    · simp only [cupd, if_true]; exact hno
    · have : (cupd c .backend (k + 1)) .store = c .store := by simp [cupd]
      rw [hsto, this]; exact hG.2.2
  | store =>
    have hS : G.store = (st .store k).store := by rw [hG.2.2, hst]
    obtain ⟨G', hs, hn, hsto, htx, hno⟩ := bridge_store hR G hS hrecv
    refine ⟨G', hs, hn, ?_, ?_, ?_⟩
    · have : (cupd c .store (k + 1)) .backend = c .backend := by simp [cupd]
      rw [htx, this]; exact hG.1
    · have : (cupd c .store (k + 1)) .backend = c .backend := by simp [cupd]
      rw [hno, this]; exact hG.2.1
    · simp only [cupd, if_true]; exact hsto

/-! ## The block order -/

/-- The protocol's behaviour from the blocks of a reordered execution, in their order. -/
theorem bridge_run (K : Kernel) (C : Config) (w0 : World) (τ : List (Ev Role Packet Unit))
    (N : Role → Nat) (st : Role → Nat → Own) (hst0 : ∀ r, st r 0 = ownInit w0)
    (hrun : ∀ r k, k < N r → RStep K C r (st r k) (recvsOf (seg τ r k))
      (ticksOf (seg τ r k)).head? (st r (k + 1)) (sendsOf (seg τ r k)))
    (hobl : ∀ r k, Obl (seg τ r k)) (hbnd : ∀ e ∈ τ, e.step < N e.host)
    (bs : List (Role × Nat)) (hbs : bs.Pairwise (fun p q => p.1 = q.1 → p.2 < q.2))
    (hne : ∀ p ∈ bs, seg τ p.1 p.2 ≠ []) (hcomp : ∀ p : Role × Nat, seg τ p.1 p.2 ≠ [] → p ∈ bs)
    (hnetbs : NetOk (fun _ => False) (bs.map (fun p => seg τ p.1 p.2)).flatten) :
    ∃ G, Star (PNext K C) (initState w0) G := by
  have key : ∀ (rest pre : List (Role × Nat)) (G : PState) (c : Role → Nat),
      pre ++ rest = bs → (∀ p ∈ pre, p.2 < c p.1) → (∀ p ∈ rest, c p.1 ≤ p.2) →
      Glob st c G →
      (∀ q, G.net q ↔ ∃ y ∈ (pre.map (fun p => seg τ p.1 p.2)).flatten, y.io = .send q) →
      NetOk (fun q => ∃ y ∈ (pre.map (fun p => seg τ p.1 p.2)).flatten, y.io = .send q)
        (rest.map (fun p => seg τ p.1 p.2)).flatten →
      ∃ G', Star (PNext K C) G G' := by
    intro rest
    induction rest with
    | nil => intro pre G c _ _ _ _ _ _; exact ⟨G, Star.refl G⟩
    | cons hd rest' ih =>
      intro pre G c hpr hpre hrest hG hnetG hNet
      obtain ⟨r, k⟩ := hd
      have hmemb : (r, k) ∈ bs := by
        rw [← hpr]; exact List.mem_append_right _ (by simp)
      have hck : c r ≤ k := hrest (r, k) (by simp)
      have hblkne : seg τ r k ≠ [] := hne (r, k) hmemb
      obtain ⟨e0, he0⟩ := List.exists_mem_of_ne_nil _ hblkne
      have he0m := List.mem_filter.mp he0
      have he0t : tagIs r k e0 = true := he0m.2
      have he0' : e0.host = r ∧ e0.step = k := by simpa [tagIs] using he0t
      have hkN : k < N r := by
        have := hbnd e0 he0m.1
        rw [he0'.1, he0'.2] at this
        exact this
      -- no earlier step of this host is nonempty before `k`
      have hbs' := hbs
      rw [← hpr] at hbs'
      have hcons := (List.pairwise_append.mp hbs').2.1
      have hhead := (List.pairwise_cons.mp hcons).1
      have hgap : ∀ j, c r ≤ j → j < k → seg τ r j = [] := by
        intro j hj1 hj2
        refine Classical.byContradiction (fun hne' => ?_)
        have hmem := hcomp (r, j) hne'
        rw [← hpr] at hmem
        rcases List.mem_append.mp hmem with hpm | hrm
        · have := hpre (r, j) hpm
          simp at this
          omega
        · rcases List.mem_cons.mp hrm with heq | hrest'm
          · have hjk : j = k := congrArg Prod.snd heq
            omega
          · have := hhead (r, j) hrest'm rfl
            omega
      have hst_eq : st r (c r) = st r k := by
        refine (eq_of_gap (fun j => st r j) (c r) k hck ?_).symm
        intro j hj1 hj2
        have hz := hgap j hj1 hj2
        have hR := hrun r j (by omega)
        rw [hz] at hR
        simp only [recvsOf, ticksOf, sendsOf, List.filterMap_nil, List.head?_nil] at hR
        exact rstep_empty hR
      have hR := hrun r k hkN
      have htk : (ticksOf (seg τ r k)).head? = none := rstep_tk hR
      rw [htk] at hR
      have hsplit : (((r, k) :: rest').map (fun p => seg τ p.1 p.2)).flatten =
          seg τ r k ++ (rest'.map (fun p => seg τ p.1 p.2)).flatten := by
        simp [List.flatten_cons]
      have hNetR : NetOk (fun q => ∃ y ∈ (pre.map (fun p => seg τ p.1 p.2)).flatten,
          y.io = .send q) (seg τ r k) := by
        rw [hsplit] at hNet
        exact ((netOk_append _ _ _).mp hNet).1
      have hrecv : ∀ p ∈ recvsOf (seg τ r k), G.net p := by
        intro p hp
        obtain ⟨y, hy, hyp⟩ := (mem_recvsOf _ p).mp hp
        have hS := recv_of_obl (seg τ r k) _ (hobl r k).1 hNetR y hy p hyp
        exact (hnetG p).mpr hS
      obtain ⟨G', hstar, hnetG', hG'⟩ := step_role K C st c r k G hG hst_eq
        (recvsOf (seg τ r k)) (sendsOf (seg τ r k)) hR hrecv
      -- the continuation, on the rest of the blocks
      have hpr' : (pre ++ [(r, k)]) ++ rest' = bs := by
        rw [← hpr]; simp
      have hflat : ((pre ++ [(r, k)]).map (fun p => seg τ p.1 p.2)).flatten =
          (pre.map (fun p => seg τ p.1 p.2)).flatten ++ seg τ r k := by
        simp [List.map_append, List.flatten_append]
      have hpre' : ∀ p ∈ pre ++ [(r, k)], p.2 < cupd c r (k + 1) p.1 := by
        intro p hp
        rcases List.mem_append.mp hp with hpm | hpm
        · have h1 := hpre p hpm
          by_cases hpr1 : p.1 = r
          · have hk1 : cupd c r (k + 1) p.1 = k + 1 := by simp [cupd, hpr1]
            rw [hk1]
            have h2 : c p.1 ≤ k := by rw [hpr1]; exact hck
            omega
          · have h3 : cupd c r (k + 1) p.1 = c p.1 := by simp [cupd, hpr1]
            rw [h3]; exact h1
        · rw [List.mem_singleton.mp hpm]
          simp [cupd]
      have hrest' : ∀ p ∈ rest', cupd c r (k + 1) p.1 ≤ p.2 := by
        intro p hp
        by_cases hpr1 : p.1 = r
        · have h3 : cupd c r (k + 1) p.1 = k + 1 := by simp [cupd, hpr1]
          rw [h3]
          have h4 : k < p.2 := hhead p hp hpr1.symm
          omega
        · have h3 : cupd c r (k + 1) p.1 = c p.1 := by simp [cupd, hpr1]
          rw [h3]; exact hrest p (List.mem_cons_of_mem _ hp)
      have hnetG'' : ∀ q, G'.net q ↔ ∃ y ∈ ((pre ++ [(r, k)]).map
          (fun p => seg τ p.1 p.2)).flatten, y.io = .send q := by
        intro q
        rw [hnetG', hnetG q, hflat]
        constructor
        · rintro (⟨y, hy, hy'⟩ | h2)
          · exact ⟨y, List.mem_append_left _ hy, hy'⟩
          · obtain ⟨y, hy, hy'⟩ := (mem_sendsOf _ q).mp h2
            exact ⟨y, List.mem_append_right _ hy, hy'⟩
        · rintro ⟨y, hy, hy'⟩
          rcases List.mem_append.mp hy with h1 | h1
          · exact Or.inl ⟨y, h1, hy'⟩
          · exact Or.inr ((mem_sendsOf _ q).mpr ⟨y, h1, hy'⟩)
      have hNet'' : NetOk (fun q => ∃ y ∈ ((pre ++ [(r, k)]).map
          (fun p => seg τ p.1 p.2)).flatten, y.io = .send q) (rest'.map
          (fun p => seg τ p.1 p.2)).flatten := by
        have h2 := ((netOk_append _ _ _).mp hNet).2
        rw [hflat]
        refine (netOk_congr (fun q => ?_) _).mp h2
        constructor
        · rintro (⟨y, hy, hy'⟩ | ⟨y, hy, hy'⟩)
          · exact ⟨y, List.mem_append_left _ hy, hy'⟩
          · exact ⟨y, List.mem_append_right _ hy, hy'⟩
        · rintro ⟨y, hy, hy'⟩
          rcases List.mem_append.mp hy with h1 | h1
          · exact Or.inl ⟨y, h1, hy'⟩
          · exact Or.inr ⟨y, h1, hy'⟩
      obtain ⟨G'', hst2⟩ := ih (pre ++ [(r, k)]) G' (cupd c r (k + 1)) hpr' hpre' hrest' hG'
        hnetG'' hNet''
      exact ⟨G'', star_trans hstar hst2⟩
  have hG0 : Glob st (fun _ => 0) (initState w0) := by
    refine ⟨?_, ?_, ?_⟩
    · simp [hst0, ownInit, initState]
    · simp [hst0, ownInit, initState]
    · simp [hst0, ownInit, initState]
  have hnet0 : NetOk (fun q => ∃ y ∈ (([] : List (Ev Role Packet Unit)).map
      (fun p => seg τ p.1 p.2)).flatten, y.io = .send q) (bs.map
      (fun p => seg τ p.1 p.2)).flatten :=
    (netOk_congr (fun q => by simp) _).mp hnetbs
  obtain ⟨G, hG⟩ := key bs [] (initState w0) (fun _ => 0) (by simp) (by simp) (by simp) hG0
    (fun q => by simp [initState]) hnet0
  exact ⟨G, hG⟩

/-- Every fine-grained execution of the backend refines a protocol behaviour, and so a behaviour of
    the system spec. -/
theorem backend_fine_refines (K : Kernel) (C : Config) (w0 : World) (N : Role → Nat)
    (τ : List (Ev Role Packet Unit)) (hE : FineExec (RStep K C) (fun _ => ownInit w0) N τ) :
    ∃ s : PState, ProtoReachable K C w0 s ∧ SpecReachable K C w0 (absState s) := by
  obtain ⟨τ', hreord, hNet⟩ := reorder_core τ.length (fun _ => False) τ (Nat.le_refl _)
    hE.hostOrd hE.obl hE.net
  obtain ⟨hperm, -, -, bs, hbs_eq, hpair, hne⟩ := hreord
  have hcomp : ∀ p : Role × Nat, seg τ p.1 p.2 ≠ [] → p ∈ bs := by
    intro p hp
    obtain ⟨e, he⟩ := List.exists_mem_of_ne_nil _ hp
    have he' : e ∈ τ' := hperm.mem_iff.mpr (List.mem_filter.mp he).1
    rw [hbs_eq] at he'
    obtain ⟨l, hl, hel⟩ := List.mem_flatten.mp he'
    obtain ⟨q, hq, hql⟩ := List.mem_map.mp hl
    rw [← hql] at hel
    have hel' : e ∈ seg τ q.1 q.2 := hel
    have hq1 := (List.mem_filter.mp hel').2
    have hp1 := (List.mem_filter.mp he).2
    simp only [tagIs, decide_eq_true_eq] at hq1 hp1
    have hqp : q = p := Prod.ext (hq1.1.symm.trans hp1.1) (hq1.2.symm.trans hp1.2)
    rw [← hqp]; exact hq
  have hnetbs : NetOk (fun _ => False) (bs.map (fun p => seg τ p.1 p.2)).flatten := by
    rw [← hbs_eq]; exact hNet
  obtain ⟨st, hst0, hsrun⟩ := hE.run
  obtain ⟨G, hG⟩ := bridge_run K C w0 τ N st hst0 hsrun hE.obl hE.bounded bs hpair hne hcomp
    hnetbs
  exact ⟨G, ⟨initState w0, initState_pinit w0, hG⟩,
    (protocol_refines ⟨initState w0, initState_pinit w0, hG⟩).2.1⟩

end Strata.Reduction
