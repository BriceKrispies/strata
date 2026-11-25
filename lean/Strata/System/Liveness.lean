/-
Backend liveness (IronFleet's liveness layer, WG-5656).

The fair protocol spec: the protocol's behaviours (`PInit` and `PNext` from `Protocol.lean`) with
the fairness the backend's hosts provide, stated as one weak-fairness assumption per host action
instance. Each action below is a rule of `PNext`, restricted
to one request id or one packet; the family of all instances is the host's fairness.

Theorems: `reply_leadsTo` (a request with a session gets a reply, under the bounded-workload
hypothesis), `publish_leadsTo` (a committed event is delivered to every subscriber that may read it),
and `decided_relayed_leadsTo` (a decided request's reply is relayed to the client). Every
assumption is weak fairness; the attempt tags in `Protocol.lean` are what make that suffice. See
`docs/formal-verification.md`, section "Backend liveness", for the assumptions in English.
`protoSpec_nonvacuous` exhibits a behaviour satisfying the fair spec, in which request 1 is accepted,
answered and published.
-/

import Strata.System.Protocol
import Strata.Temporal.Rules

namespace Strata.System

open Strata.Temporal

/-! ## Generic fairness lemmas -/

/-- Weak fairness from a point on: if `A` is enabled at `n`, and every step that does not take `A`
keeps it enabled, then `A` is taken at some step at or after `n`. -/
theorem fair_pers {A : PState → PState → Prop} {b : Behaviour PState} {n : Nat} (hw : weakFair A b)
    (hp : ∀ m, n ≤ m → enabled A (b m) → ¬ A (b m) (b (m+1)) → enabled A (b (m+1)))
    (he : enabled A (b n)) : ∃ j, n ≤ j ∧ A (b j) (b (j+1)) := by
  refine Classical.byContradiction fun hc => ?_
  have hc' : ∀ j, n ≤ j → ¬ A (b j) (b (j+1)) := fun j hj hA => hc ⟨j, hj, hA⟩
  have hall : ∀ d, enabled A (b (n + d)) := by
    intro d
    induction d with
    | zero => exact he
    | succ d ih => exact hp (n + d) (by omega) ih (hc' (n + d) (by omega))
  have hev : eventually (always (lift (enabled A))) b := by
    refine ⟨n, fun m => ?_⟩
    exact hall m
  obtain ⟨k, hk⟩ := (hw hev) n
  rw [act_suffix] at hk
  exact hc' (n + k) (by omega) hk

/-- Strong fairness from a point on: if `A` is enabled infinitely often, it is taken from any point
on. -/
theorem sfair_inf {A : PState → PState → Prop} {b : Behaviour PState} (hs : strongFair A b)
    (hinf : ∀ n, ∃ m, n ≤ m ∧ enabled A (b m)) (n : Nat) :
    ∃ j, n ≤ j ∧ A (b j) (b (j+1)) := by
  have hev : always (eventually (lift (enabled A))) b := by
    intro d
    obtain ⟨m, hm, he⟩ := hinf d
    refine ⟨m - d, ?_⟩
    show enabled A (b (d + (m - d)))
    have e : d + (m - d) = m := by omega
    rw [e]
    exact he
  obtain ⟨k, hk⟩ := (hs hev) n
  rw [act_suffix] at hk
  exact ⟨n + k, by omega, hk⟩

/-- A behaviour's states are monotone in the network: a packet sent stays sent. -/
theorem net_seq {b : Behaviour PState} {K : Kernel} {C : Config}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n) :
    ∀ m n, m ≤ n → ∀ p, (b m).net p → (b n).net p := by
  intro m n hmn p hp
  obtain ⟨d, rfl⟩ : ∃ d, n = m + d := ⟨n - m, by omega⟩
  clear hmn
  induction d with
  | zero => exact hp
  | succ d ih =>
    rcases hstep (m + d) with h | h
    · exact net_mono h p ih
    · show (b (m + d + 1)).net p
      rw [h]; exact ih

/-! ## The host actions, one instance each

Each action is one rule of `PNext`, restricted to the packet or request it names. `actX` relates
the state before and after the rule. The fairness of the backend's hosts is stated over these. -/

/-- The BFF forwards request `rid` (with token `tok` and intent `i`) once it has been sent. -/
def actFwd (rid : RequestId) (tok : Token) (i : Intent) (s s' : PState) : Prop :=
  s.net (.req rid tok i) ∧ s' = sendOne s (.fwd rid tok i)

/-- The backend starts a transaction for request `rid`, whose token has a session. -/
def actBegin (C : Config) (rid : RequestId) (tok : Token) (i : Intent) (s s' : PState) : Prop :=
  ∃ t p, C.sessions tok = some (t, p) ∧ s.net (.fwd rid tok i) ∧ s.txns rid = none ∧
    s.noSession rid = false ∧
    s' = { sendOne s (.load rid t (s.attempt rid)) with txns := updTxn s.txns rid ⟨tok, t, p, i, .loading⟩ }

/-- The BFF relays the backend's reply for `rid` to the client. -/
def actRelay (rid : RequestId) (o : Outcome) (s s' : PState) : Prop :=
  s.net (.reply rid o) ∧ s' = sendOne s (.clientReply rid o)

/-- The backend finishes the transaction for `rid` once the store has decided it. -/
def actDone (rid : RequestId) (o : Outcome) (s s' : PState) : Prop :=
  ∃ x, s.txns rid = some x ∧ s.net (.done rid o) ∧
    s' = { sendOne s (.reply rid o) with txns := updTxn s.txns rid { x with phase := .finished o } }

/-- The store answers a load for `rid` with the tenant's current state and revision. -/
def actLoad (rid : RequestId) (t : TenantId) (a : Nat) (s s' : PState) : Prop :=
  s.net (.load rid t a) ∧
    s' = sendOne s (.snap rid t (s.store.hist t (s.store.rev t)) (s.store.rev t) a)

/-- The backend computes the verdict on its snapshot and sends the commit. -/
def actCommit (K : Kernel) (rid : RequestId) (s s' : PState) : Prop :=
  ∃ x v b, s.txns rid = some x ∧ x.phase = .ready v b ∧
    s' = { sendOne s (.commit rid x.tok x.tenant x.principal x.intent b
            (verdict K v x.principal x.intent)) with
          txns := updTxn s.txns rid { x with phase := .committing v b } }

/-- The store refused a commit of `rid`; the backend loads again. -/
def actConflict (rid : RequestId) (s s' : PState) : Prop :=
  ∃ x v b, s.txns rid = some x ∧ x.phase = .committing v b ∧ s.net (.conflict rid b) ∧
    s' = { sendOne s (.load rid x.tenant (s.attempt rid + 1)) with
      txns := updTxn s.txns rid { x with phase := .loading },
      attempt := updAtt s.attempt rid (s.attempt rid + 1) }

/-- The backend takes a snapshot it was sent while it is loading its tenant. -/
def actSnap (rid : RequestId) (t : TenantId) (v : Option Tenant) (b a : Nat) (s s' : PState) : Prop :=
  ∃ x, s.txns rid = some x ∧ x.tenant = t ∧ x.phase = .loading ∧ s.attempt rid = a ∧
    s.net (.snap rid t v b a) ∧
    s' = { s with txns := updTxn s.txns rid { x with phase := .ready v b } }

/-- The publisher sends a committed event to a subscriber whose principal may read it. -/
def actPublish (K : Kernel) (C : Config) (sid : SubId) (e : Event) (s s' : PState) : Prop :=
  s.net (.committed e) ∧ (∃ t p, C.subs sid = some (t, p) ∧ e.tenant = t ∧ K.readable e.post p) ∧
    s' = sendOne s (.frame sid e)

/-- The store's decision on one commit packet, one rule per case: a decided request is answered
    again; an undecided commit on an old revision is a conflict; on the current revision it is
    accepted or refused. -/
inductive StoreStep (K : Kernel) (C : Config) (rid : RequestId) (tok : Token) (t : TenantId)
    (p : PrincipalId) (i : Intent) (base : Nat) (res : Except Err Tenant) :
    PState → PState → Prop
  | decided (s : PState) (o : Outcome) (h : s.net (.commit rid tok t p i base res))
      (hd : s.store.decided rid = some o) :
      StoreStep K C rid tok t p i base res s (sendOne s (.done rid o))
  | conflict (s : PState) (h : s.net (.commit rid tok t p i base res))
      (hd : s.store.decided rid = none) (hb : base ≠ s.store.rev t) :
      StoreStep K C rid tok t p i base res s (sendOne s (.conflict rid base))
  | accept (s : PState) (t' : Tenant) (h : s.net (.commit rid tok t p i base res))
      (hd : s.store.decided rid = none) (hb : base = s.store.rev t) (hr : res = .ok t') :
      StoreStep K C rid tok t p i base res s
        { net := fun x => s.net x ∨ x = .done rid .accepted ∨ x = .committed (Event.mk t rid t')
          txns := s.txns
          noSession := s.noSession
          attempt := s.attempt
          store :=
            { hist := fun x k => if x = t ∧ k = base + 1 then some t' else s.store.hist x k
              rev := fun x => if x = t then base + 1 else s.store.rev x
              evs := s.store.evs ++ [Event.mk t rid t']
              decided := fun r => if r = rid then some .accepted else s.store.decided r } }
  | refuse (s : PState) (e : Err) (h : s.net (.commit rid tok t p i base res))
      (hd : s.store.decided rid = none) (hb : base = s.store.rev t) (hr : res = .error e) :
      StoreStep K C rid tok t p i base res s
        { net := fun x => s.net x ∨ x = .done rid (.refused e)
          txns := s.txns
          noSession := s.noSession
          attempt := s.attempt
          store := { s.store with
            decided := fun r => if r = rid then some (.refused e) else s.store.decided r } }

/-- The fairness of the backend's hosts, one assumption per action instance. Each action is
    weakly fair. A snapshot is taken only in the attempt it was read for, so no stale snapshot can
    keep a transaction conflicting. -/
def protoFair (K : Kernel) (C : Config) : Temporal PState := fun b =>
  (∀ rid tok i, weakFair (actFwd rid tok i) b) ∧
  (∀ rid tok i, weakFair (actBegin C rid tok i) b) ∧
  (∀ rid o, weakFair (actRelay rid o) b) ∧
  (∀ rid o, weakFair (actDone rid o) b) ∧
  (∀ rid t a, weakFair (actLoad rid t a) b) ∧
  (∀ rid, weakFair (actCommit K rid) b) ∧
  (∀ rid, weakFair (actConflict rid) b) ∧
  (∀ rid tok t p i base res, weakFair (StoreStep K C rid tok t p i base res) b) ∧
  (∀ rid t v base a, weakFair (actSnap rid t v base a) b) ∧
  (∀ sid e, weakFair (actPublish K C sid e) b)

/-- The fair protocol spec: `Init ∧ □[Next] ∧ fairness`. -/
def protoSpec (K : Kernel) (C : Config) (w0 : World) : Temporal PState :=
  spec (PInit w0) (PNext K C) id (protoFair K C)

/-! ## Invariants used by the liveness proof

`LInv` holds in every state of a behaviour: the request ids are below the workload bound `N`,
the store's revision is at most the number of decided requests below `N`, and the bookkeeping
fields relate the backend's transactions, the store's decisions and the packets sent. -/

/-- The request id a packet names. -/
def ridOf : Packet → RequestId
  | .req r _ _ => r
  | .fwd r _ _ => r
  | .reply r _ => r
  | .clientReply r _ => r
  | .load r _ _ => r
  | .snap r _ _ _ _ => r
  | .commit r _ _ _ _ _ _ => r
  | .done r _ => r
  | .conflict r _ => r
  | .committed e => e.rid
  | .frame _ e => e.rid

/-- The number of decided requests among the ids below `n`. -/
def dcount (d : RequestId → Option Outcome) : Nat → Nat
  | 0 => 0
  | n + 1 => dcount d n + (if d n = none then 0 else 1)

structure LInv (K : Kernel) (C : Config) (N : Nat) (s : PState) : Prop where
  bndP : ∀ p, s.net p → ridOf p < N
  bndT : ∀ r, s.txns r ≠ none → r < N
  rev : ∀ t, s.store.rev t ≤ dcount s.store.decided N
  doneDec : ∀ r o, s.store.decided r = some o → s.net (.done r o)
  nsRep : ∀ r, s.noSession r = true → s.net (.reply r .noSession)
  evsCom : ∀ e, e ∈ s.store.evs → s.net (.committed e)
  txnFin : ∀ r x o, s.txns r = some x → x.phase = .finished o → s.store.decided r = some o
  txnLoad : ∀ r x, s.txns r = some x → s.net (.load r x.tenant (s.attempt r))
  loadRev : ∀ r t a, s.net (.load r t a) → a ≤ s.store.rev t
  snapAtt : ∀ r t v b a, s.net (.snap r t v b a) → a ≤ b
  attLe : ∀ r x v b, s.txns r = some x → (x.phase = .ready v b ∨ x.phase = .committing v b) →
    s.attempt r ≤ b
  attBound : ∀ r x, s.txns r = some x → s.attempt r ≤ s.store.rev x.tenant
  conflictLt : ∀ r b, s.net (.conflict r b) → ∃ x, s.txns r = some x ∧ b < s.store.rev x.tenant
  zeroNone : ∀ r, s.txns r = none → s.attempt r = 0
  txnCom : ∀ r x v b, s.txns r = some x → x.phase = .committing v b →
    s.net (.commit r x.tok x.tenant x.principal x.intent b (verdict K v x.principal x.intent))

theorem dcount_le (d : RequestId → Option Outcome) (N : Nat) : dcount d N ≤ N := by
  induction N with
  | zero => exact Nat.le_refl _
  | succ n ih =>
    show dcount d n + (if d n = none then 0 else 1) ≤ n + 1
    split <;> omega

theorem dcount_mono {d d' : RequestId → Option Outcome} (h : ∀ r, d r ≠ none → d' r ≠ none)
    (N : Nat) : dcount d N ≤ dcount d' N := by
  induction N with
  | zero => exact Nat.le_refl _
  | succ n ih =>
    show dcount d n + (if d n = none then 0 else 1) ≤ dcount d' n + (if d' n = none then 0 else 1)
    have hn := h n
    split <;> split <;> simp_all <;> omega

theorem dcount_upd {d d' : RequestId → Option Outcome} {r0 : RequestId}
    (h0 : d r0 = none) (h1 : d' r0 ≠ none) (hr : ∀ r, r ≠ r0 → d' r = d r) (N : Nat) :
    dcount d' N = dcount d N + (if r0 < N then 1 else 0) := by
  induction N with
  | zero => simp [dcount]
  | succ n ih =>
    show dcount d' n + (if d' n = none then 0 else 1) =
      dcount d n + (if d n = none then 0 else 1) + (if r0 < n + 1 then 1 else 0)
    rw [ih]
    by_cases hn : n = r0
    · subst hn
      simp [h0, h1]
    · rw [hr n hn]
      have e1 : (if r0 < n + 1 then 1 else 0) = (if r0 < n then 1 else 0) := by
        by_cases hlt : r0 < n
        · have h2 : r0 < n + 1 := Nat.lt_succ_of_lt hlt
          simp [hlt, h2]
        · have h2 : ¬ r0 < n + 1 := fun hh => hn (Nat.le_antisymm (Nat.lt_succ_iff.mp hh) (Nat.not_lt.mp hlt)).symm
          simp [hlt, h2]
      rw [e1]
      omega

/-- The bound on a transaction record updated at `rid`. -/
theorem bnd_upd {N : Nat} {f : RequestId → Option Txn} {rid : RequestId} {x : Txn}
    (hf : ∀ r, f r ≠ none → r < N) (hrid : rid < N) :
    ∀ r, updTxn f rid x r ≠ none → r < N := by
  intro r hr
  by_cases h : r = rid
  · subst h; exact hrid
  · rw [updTxn_ne _ _ _ _ h] at hr
    exact hf r hr

/-- A transaction record persists: once `r` has a transaction, it keeps one, with the same tenant. -/
theorem txn_keep_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (r : RequestId) (x : Txn) (hx : s.txns r = some x) :
    ∃ y, s'.txns r = some y ∧ y.tenant = x.tenant := by
  cases h with
  | clientReq rid tok i => exact ⟨x, hx, rfl⟩
  | bffForward rid tok i h1 => exact ⟨x, hx, rfl⟩
  | bffRelay rid o h1 => exact ⟨x, hx, rfl⟩
  | begin rid tok i t p' h1 hn0 hn hsn =>
    by_cases hrr : r = rid
    · subst hrr; rw [hn0] at hx; cases hx
    · exact ⟨x, by simpa [updTxn, hrr] using hx, rfl⟩
  | noSession rid tok i h1 hn0 hn hsn => exact ⟨x, hx, rfl⟩
  | snap rid t v base a x0 h1 hx0 ht hl hat =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx)
      subst e
      exact ⟨{ x0 with phase := .ready v base }, by simp [updTxn], rfl⟩
    · exact ⟨x, by simpa [updTxn, hrr] using hx, rfl⟩
  | commit rid x0 v base hx0 hph =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx)
      subst e
      exact ⟨{ x0 with phase := .committing v base }, by simp [updTxn], rfl⟩
    · exact ⟨x, by simpa [updTxn, hrr] using hx, rfl⟩
  | conflict rid base x0 v h1 hx0 hph =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx)
      subst e
      exact ⟨{ x0 with phase := .loading }, by simp [updTxn], rfl⟩
    · exact ⟨x, by simpa [updTxn, hrr] using hx, rfl⟩
  | done rid o x0 h1 hx0 =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx)
      subst e
      exact ⟨{ x0 with phase := .finished o }, by simp [updTxn], rfl⟩
    · exact ⟨x, by simpa [updTxn, hrr] using hx, rfl⟩
  | load rid t a h1 => exact ⟨x, hx, rfl⟩
  | storeDecided rid tok t p' i base res o h1 hd => exact ⟨x, hx, rfl⟩
  | storeConflict rid tok t p' i base res h1 hd hb => exact ⟨x, hx, rfl⟩
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact ⟨x, hx, rfl⟩
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact ⟨x, hx, rfl⟩
  | publish sid e t p' h1 hsub ht hr => exact ⟨x, hx, rfl⟩

theorem rev_mono_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s') :
    ∀ t, s.store.rev t ≤ s'.store.rev t := fun t => by
  rcases rev_step h t with e | e <;> omega

theorem updAtt_ne (f : RequestId → Nat) (rid : RequestId) (n : Nat) (r : RequestId) (h : r ≠ rid) :
    updAtt f rid n r = f r := by
  simp [updAtt, h]

theorem linv_bndP {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) (hreq : ∀ rid tok i, s'.net (.req rid tok i) → rid < N) :
    ∀ p, s'.net p → ridOf p < N := by
  have hs := hI.bndP
  intro p hp
  cases h with
  | clientReq rid tok i =>
    rcases hp with h1 | h1
    · exact hs p h1
    · subst h1; exact hreq rid tok i (Or.inr rfl)
  | bffForward rid tok i h1 =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.req rid tok i) h1
  | bffRelay rid o h1 =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.reply rid o) h1
  | begin rid tok i t p' h1 hx hn hsn =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.fwd rid tok i) h1
  | noSession rid tok i h1 hx hn hsn =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.fwd rid tok i) h1
  | snap rid t v base a x h1 hx ht hl hat => exact hs p hp
  | commit rid x v base hx hph =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hI.bndT rid (by rw [hx]; simp)
  | conflict rid base x v h1 hx hph =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hI.bndT rid (by rw [hx]; simp)
  | done rid o x h1 hx =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.done rid o) h1
  | load rid t a h1 =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.load rid t a) h1
  | storeDecided rid tok t p' i base res o h1 hd =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.commit rid tok t p' i base res) h1
  | storeConflict rid tok t p' i base res h1 hd hb =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.commit rid tok t p' i base res) h1
  | storeAccept rid tok t p' i base res t' h1 hd hb hr =>
    rcases hp with h2 | h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.commit rid tok t p' i base res) h1
    · subst h2; exact hs (.commit rid tok t p' i base res) h1
  | storeRefuse rid tok t p' i base res e h1 hd hb hr =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.commit rid tok t p' i base res) h1
  | publish sid e t p' h1 hsub ht hr =>
    rcases hp with h2 | h2
    · exact hs p h2
    · subst h2; exact hs (.committed e) h1

theorem linv_bndT {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r, s'.txns r ≠ none → r < N := by
  have ht := hI.bndT
  cases h with
  | clientReq rid tok i => exact ht
  | bffForward rid tok i h1 => exact ht
  | bffRelay rid o h1 => exact ht
  | begin rid tok i t p' h1 hx hn hsn => exact bnd_upd ht (hI.bndP _ h1)
  | noSession rid tok i h1 hx hn hsn => exact ht
  | snap rid t v base a x h1 hx ht' hl hat => exact bnd_upd ht (hI.bndT rid (by rw [hx]; simp))
  | commit rid x v base hx hph => exact bnd_upd ht (hI.bndT rid (by rw [hx]; simp))
  | conflict rid base x v h1 hx hph => exact bnd_upd ht (hI.bndT rid (by rw [hx]; simp))
  | done rid o x h1 hx => exact bnd_upd ht (hI.bndT rid (by rw [hx]; simp))
  | load rid t a h1 => exact ht
  | storeDecided rid tok t p' i base res o h1 hd => exact ht
  | storeConflict rid tok t p' i base res h1 hd hb => exact ht
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact ht
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact ht
  | publish sid e t p' h1 hsub ht' hr => exact ht

theorem linv_rev {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ t, s'.store.rev t ≤ dcount s'.store.decided N := by
  have hr := hI.rev
  cases h with
  | clientReq rid tok i => exact hr
  | bffForward rid tok i h1 => exact hr
  | bffRelay rid o h1 => exact hr
  | begin rid tok i t p' h1 hx hn hsn => exact hr
  | noSession rid tok i h1 hx hn hsn => exact hr
  | snap rid t v base a x h1 hx ht hl hat => exact hr
  | commit rid x v base hx hph => exact hr
  | conflict rid base x v h1 hx hph => exact hr
  | done rid o x h1 hx => exact hr
  | load rid t a h1 => exact hr
  | storeDecided rid tok t p' i base res o h1 hd => exact hr
  | storeConflict rid tok t p' i base res h1 hd hb => exact hr
  | publish sid e t p' h1 hsub ht hr2 => exact hI.rev
  | storeRefuse rid tok t p' i base res e h1 hd hb hr' =>
    intro t1
    refine Nat.le_trans (hI.rev t1) (dcount_mono ?_ N)
    intro r hne
    by_cases hrr : r = rid
    · subst hrr; simp
    · simp [hrr, hne]
  | storeAccept rid tok t p' i base res t' h1 hd hb hr' =>
    intro t1
    have hrid : rid < N := hI.bndP _ h1
    have hup := dcount_upd (d := s.store.decided)
      (d' := fun r => if r = rid then some Outcome.accepted else s.store.decided r) (r0 := rid)
      hd (by simp) (fun r hne => by simp [hne]) N
    show (if t1 = t then base + 1 else s.store.rev t1) ≤
      dcount (fun r => if r = rid then some Outcome.accepted else s.store.decided r) N
    rw [hup, if_pos hrid]
    have hr1 := hI.rev t1
    by_cases ht : t1 = t
    · subst ht
      rw [if_pos rfl]
      omega
    · rw [if_neg ht]
      omega

theorem linv_doneDec {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r o, s'.store.decided r = some o → s'.net (.done r o) := by
  have hd := hI.doneDec
  cases h with
  | clientReq rid tok i => exact fun r o h' => Or.inl (hd r o h')
  | bffForward rid tok i h1 => exact fun r o h' => Or.inl (hd r o h')
  | bffRelay rid o h1 => exact fun r o' h' => Or.inl (hd r o' h')
  | begin rid tok i t p' h1 hx hn hsn => exact fun r o h' => Or.inl (hd r o h')
  | noSession rid tok i h1 hx hn hsn => exact fun r o h' => Or.inl (hd r o h')
  | snap rid t v base a x h1 hx ht hl hat => exact fun r o h' => hd r o h'
  | commit rid x v base hx hph => exact fun r o h' => Or.inl (hd r o h')
  | conflict rid base x v h1 hx hph => exact fun r o h' => Or.inl (hd r o h')
  | done rid o x h1 hx => exact fun r o' h' => Or.inl (hd r o' h')
  | load rid t a h1 => exact fun r o h' => Or.inl (hd r o h')
  | storeDecided rid tok t p' i base res o h1 hdd => exact fun r o' h' => Or.inl (hd r o' h')
  | storeConflict rid tok t p' i base res h1 hdd hb => exact fun r o h' => Or.inl (hd r o h')
  | storeAccept rid tok t p' i base res t' h1 hdd hb hr =>
    intro r o h'
    by_cases hrr : r = rid
    · subst hrr
      simp at h'
      subst h'
      exact Or.inr (Or.inl rfl)
    · simp [hrr] at h'
      exact Or.inl (hd r o h')
  | storeRefuse rid tok t p' i base res e h1 hdd hb hr =>
    intro r o h'
    by_cases hrr : r = rid
    · subst hrr
      simp at h'
      subst h'
      exact Or.inr rfl
    · simp [hrr] at h'
      exact Or.inl (hd r o h')
  | publish sid e t p' h1 hsub ht hr => exact fun r o h' => Or.inl (hd r o h')

theorem linv_nsRep {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r, s'.noSession r = true → s'.net (.reply r .noSession) := by
  have hn := hI.nsRep
  cases h with
  | clientReq rid tok i => exact fun r hr => Or.inl (hn r hr)
  | bffForward rid tok i h1 => exact fun r hr => Or.inl (hn r hr)
  | bffRelay rid o h1 => exact fun r hr => Or.inl (hn r hr)
  | begin rid tok i t p' h1 hx hn' hsn => exact fun r hr => Or.inl (hn r hr)
  | noSession rid tok i h1 hx hn' hsn =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr
      exact Or.inr rfl
    · have hr' : s.noSession r = true := by simpa [hrr] using hr
      exact Or.inl (hn r hr')
  | snap rid t v base a x h1 hx ht hl hat => exact fun r hr => hn r hr
  | commit rid x v base hx hph => exact fun r hr => Or.inl (hn r hr)
  | conflict rid base x v h1 hx hph => exact fun r hr => Or.inl (hn r hr)
  | done rid o x h1 hx => exact fun r hr => Or.inl (hn r hr)
  | load rid t a h1 => exact fun r hr => Or.inl (hn r hr)
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r hr => Or.inl (hn r hr)
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r hr => Or.inl (hn r hr)
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact fun r hr' => Or.inl (hn r hr')
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact fun r hr' => Or.inl (hn r hr')
  | publish sid e t p' h1 hsub ht hr => exact fun r hr' => Or.inl (hn r hr')

theorem linv_evsCom {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ e, e ∈ s'.store.evs → s'.net (.committed e) := by
  have hev := hI.evsCom
  cases h with
  | clientReq rid tok i => exact fun e he => Or.inl (hev e he)
  | bffForward rid tok i h1 => exact fun e he => Or.inl (hev e he)
  | bffRelay rid o h1 => exact fun e he => Or.inl (hev e he)
  | begin rid tok i t p' h1 hx hn hsn => exact fun e he => Or.inl (hev e he)
  | noSession rid tok i h1 hx hn hsn => exact fun e he => Or.inl (hev e he)
  | snap rid t v base a x h1 hx ht hl hat => exact fun e he => hev e he
  | commit rid x v base hx hph => exact fun e he => Or.inl (hev e he)
  | conflict rid base x v h1 hx hph => exact fun e he => Or.inl (hev e he)
  | done rid o x h1 hx => exact fun e he => Or.inl (hev e he)
  | load rid t a h1 => exact fun e he => Or.inl (hev e he)
  | storeDecided rid tok t p' i base res o h1 hd => exact fun e he => Or.inl (hev e he)
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun e he => Or.inl (hev e he)
  | storeAccept rid tok t p' i base res t' h1 hd hb hr =>
    intro e he
    rcases List.mem_append.mp he with h2 | h2
    · exact Or.inl (hev e h2)
    · simp at h2
      subst h2
      exact Or.inr (Or.inr rfl)
  | storeRefuse rid tok t p' i base res e0 h1 hd hb hr => exact fun e he => Or.inl (hev e he)
  | publish sid e0 t p' h1 hsub ht hr => exact fun e he => Or.inl (hev e he)

theorem linv_txnFin {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) (hI : LInv K C N s) :
    ∀ r x o, s'.txns r = some x → x.phase = .finished o → s'.store.decided r = some o := by
  have hf := hI.txnFin
  cases h with
  | clientReq rid tok i => exact fun r x o hr hph => hf r x o hr hph
  | bffForward rid tok i h1 => exact fun r x o hr hph => hf r x o hr hph
  | bffRelay rid o h1 => exact fun r x o' hr hph => hf r x o' hr hph
  | begin rid tok i t p' h1 hx hn hsn =>
    intro r x' o' hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hf r x' o' hr hph
  | noSession rid tok i h1 hx hn hsn => exact fun r x o hr hph => hf r x o hr hph
  | snap rid t v base a x0 h1 hx ht hl hat =>
    intro r x' o' hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hf r x' o' hr hph
  | commit rid x0 v base hx hph0 =>
    intro r x' o' hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hf r x' o' hr hph
  | conflict rid base x0 v h1 hx hph0 =>
    intro r x' o' hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hf r x' o' hr hph
  | done rid o0 x0 h1 hx =>
    intro r x' o' hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
      subst hph
      exact hg.doneDecided _ _ h1
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hf r x' o' hr hph
  | load rid t a h1 => exact fun r x o hr hph => hf r x o hr hph
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r x o' hr hph => hf r x o' hr hph
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r x o hr hph => hf r x o hr hph
  | storeAccept rid tok t p' i base res t' h1 hd hb hr =>
    intro r x' o' hr' hph
    by_cases hrr : r = rid
    · subst hrr
      have := hf r x' o' hr' hph
      rw [hd] at this
      cases this
    · have hd' : s.store.decided r = some o' := hf r x' o' hr' hph
      show (if r = rid then some Outcome.accepted else s.store.decided r) = some o'
      simp [hrr, hd']
  | storeRefuse rid tok t p' i base res e h1 hd hb hr =>
    intro r x' o' hr' hph
    by_cases hrr : r = rid
    · subst hrr
      have := hf r x' o' hr' hph
      rw [hd] at this
      cases this
    · have hd' : s.store.decided r = some o' := hf r x' o' hr' hph
      show (if r = rid then some (Outcome.refused e) else s.store.decided r) = some o'
      simp [hrr, hd']
  | publish sid e t p' h1 hsub ht hr => exact fun r x o hr' hph => hf r x o hr' hph

theorem linv_txnLoad {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r x, s'.txns r = some x → s'.net (.load r x.tenant (s'.attempt r)) := by
  have hl := hI.txnLoad
  cases h with
  | clientReq rid tok i => exact fun r x hr => Or.inl (hl r x hr)
  | bffForward rid tok i h1 => exact fun r x hr => Or.inl (hl r x hr)
  | bffRelay rid o h1 => exact fun r x hr => Or.inl (hl r x hr)
  | begin rid tok i t p' h1 hx hn hsn =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Or.inr rfl
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hl r x' hr)
  | noSession rid tok i h1 hx hn hsn => exact fun r x hr => Or.inl (hl r x hr)
  | snap rid t v base a x0 h1 hx ht hl0 hat =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact hl _ x0 hx
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hl r x' hr
  | commit rid x0 v base hx hph =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Or.inl (hl _ x0 hx)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hl r x' hr)
  | conflict rid base x0 v h1 hx hph =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Or.inr (by simp [updAtt])
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      have := hl r x' hr
      exact Or.inl (by simpa [updAtt, hrr] using this)
  | done rid o x0 h1 hx =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Or.inl (hl _ x0 hx)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hl r x' hr)
  | load rid t a h1 => exact fun r x hr => Or.inl (hl r x hr)
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r x hr => Or.inl (hl r x hr)
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r x hr => Or.inl (hl r x hr)
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact fun r x hr' => Or.inl (hl r x hr')
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact fun r x hr' => Or.inl (hl r x hr')
  | publish sid e t p' h1 hsub ht hr => exact fun r x hr' => Or.inl (hl r x hr')

theorem linv_txnCom {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r x v b, s'.txns r = some x → x.phase = .committing v b →
      s'.net (.commit r x.tok x.tenant x.principal x.intent b (verdict K v x.principal x.intent)) := by
  have hc := hI.txnCom
  cases h with
  | clientReq rid tok i => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | bffForward rid tok i h1 => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | bffRelay rid o h1 => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | begin rid tok i t p' h1 hx hn hsn =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hc r x' v b hr hph)
  | noSession rid tok i h1 hx hn hsn => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | snap rid t v0 base a x0 h1 hx ht hl hat =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hc r x' v b hr hph
  | commit rid x0 v0 base hx hph0 =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
      obtain ⟨rfl, rfl⟩ := hph
      exact Or.inr rfl
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hc r x' v b hr hph)
  | conflict rid base x0 v0 h1 hx hph0 =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hc r x' v b hr hph)
  | done rid o x0 h1 hx =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Or.inl (hc r x' v b hr hph)
  | load rid t a h1 => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r x v b hr hph => Or.inl (hc r x v b hr hph)
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact fun r x v b hr' hph => Or.inl (hc r x v b hr' hph)
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact fun r x v b hr' hph => Or.inl (hc r x v b hr' hph)
  | publish sid e t p' h1 hsub ht hr => exact fun r x v b hr' hph => Or.inl (hc r x v b hr' hph)

theorem linv_loadRev {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) (hI : LInv K C N s) :
    ∀ r t a, s'.net (.load r t a) → a ≤ s'.store.rev t := by
  have hr := hI.loadRev
  have hm := rev_mono_step h
  cases h with
  | clientReq rid tok i =>
    intro r t a hp
    rcases hp with h1 | h1
    · exact hr r t a h1
    · cases h1
  | bffForward rid tok i h1 =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | bffRelay rid o h1 =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · obtain ⟨rfl, rfl, rfl⟩ := Packet.load.inj h2
      rw [hI.zeroNone _ hx]
      exact Nat.zero_le _
  | noSession rid tok i h1 hx hn hsn =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | snap rid t v base a x h1 hx ht hl hat => intro r t' a' hp; exact hr r t' a' hp
  | commit rid x v base hx hph =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | conflict rid base x v h1 hx hph =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · obtain ⟨rfl, rfl, rfl⟩ := Packet.load.inj h2
      have hA := hI.attLe _ x v base hx (Or.inr hph)
      obtain ⟨x', hx', hlt⟩ := hI.conflictLt _ base h1
      have e : x' = x := Option.some.inj (hx'.symm.trans hx)
      subst e
      exact Nat.succ_le_of_lt (Nat.lt_of_le_of_lt hA hlt)
  | done rid o x h1 hx =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | load rid t0 a0 h1 =>
    intro r t a hp
    rcases hp with h2 | h2
    · exact hr r t a h2
    · cases h2
  | storeDecided rid tok t p' i base res o h1 hd =>
    intro r t0 a hp
    rcases hp with h2 | h2
    · exact Nat.le_trans (hr r t0 a h2) (hm _)
    · cases h2
  | storeConflict rid tok t p' i base res h1 hd hb =>
    intro r t0 a hp
    rcases hp with h2 | h2
    · exact Nat.le_trans (hr r t0 a h2) (hm _)
    · cases h2
  | storeAccept rid tok t p' i base res t' h1 hd hb hr' =>
    intro r t0 a hp
    rcases hp with h2 | h2 | h2
    · exact Nat.le_trans (hr r t0 a h2) (hm _)
    · cases h2
    · cases h2
  | storeRefuse rid tok t p' i base res e h1 hd hb hr' =>
    intro r t0 a hp
    rcases hp with h2 | h2
    · exact Nat.le_trans (hr r t0 a h2) (hm _)
    · cases h2
  | publish sid e t p' h1 hsub ht hr' =>
    intro r t0 a hp
    rcases hp with h2 | h2
    · exact Nat.le_trans (hr r t0 a h2) (hm _)
    · cases h2

theorem linv_snapAtt {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r t v b a, s'.net (.snap r t v b a) → a ≤ b := by
  have hs := hI.snapAtt
  cases h with
  | clientReq rid tok i =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | bffForward rid tok i h1 =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | bffRelay rid o h1 =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | noSession rid tok i h1 hx hn hsn =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | snap rid t v base a x h1 hx ht hl hat => intro r t' v' b' a' hp; exact hs r t' v' b' a' hp
  | commit rid x v base hx hph =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | conflict rid base x v h1 hx hph =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | done rid o x h1 hx =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · cases h2
  | load rid t0 a0 h1 =>
    intro r t v b a hp
    rcases hp with h2 | h2
    · exact hs r t v b a h2
    · obtain ⟨rfl, rfl, rfl, rfl, rfl⟩ := Packet.snap.inj h2
      exact hI.loadRev _ _ _ h1
  | storeDecided rid tok t p' i base res o h1 hd =>
    intro r t0 v b a hp
    rcases hp with h2 | h2
    · exact hs r t0 v b a h2
    · cases h2
  | storeConflict rid tok t p' i base res h1 hd hb =>
    intro r t0 v b a hp
    rcases hp with h2 | h2
    · exact hs r t0 v b a h2
    · cases h2
  | storeAccept rid tok t p' i base res t' h1 hd hb hr' =>
    intro r t0 v b a hp
    rcases hp with h2 | h2 | h2
    · exact hs r t0 v b a h2
    · cases h2
    · cases h2
  | storeRefuse rid tok t p' i base res e h1 hd hb hr' =>
    intro r t0 v b a hp
    rcases hp with h2 | h2
    · exact hs r t0 v b a h2
    · cases h2
  | publish sid e t p' h1 hsub ht hr' =>
    intro r t0 v b a hp
    rcases hp with h2 | h2
    · exact hs r t0 v b a h2
    · cases h2

theorem linv_attLe {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r x v b, s'.txns r = some x →
      (x.phase = .ready v b ∨ x.phase = .committing v b) → s'.attempt r ≤ b := by
  have hA := hI.attLe
  cases h with
  | clientReq rid tok i => exact fun r x v b hr hph => hA r x v b hr hph
  | bffForward rid tok i h1 => exact fun r x v b hr hph => hA r x v b hr hph
  | bffRelay rid o h1 => exact fun r x v b hr hph => hA r x v b hr hph
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hA r x' v b hr hph
  | noSession rid tok i h1 hx hn hsn => exact fun r x v b hr hph => hA r x v b hr hph
  | snap rid t v0 base a x0 h1 hx ht hl hat =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      have hab := hI.snapAtt _ _ _ _ _ h1
      show s.attempt _ ≤ _
      rw [hat]
      rcases hph with hph | hph
      · simp only [Phase.ready.injEq] at hph
        obtain ⟨-, rfl⟩ := hph
        exact hab
      · cases hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hA r x' v b hr hph
  | commit rid x0 v0 base hx0 hph0 =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      rcases hph with hph | hph
      · cases hph
      · obtain ⟨hv, hb⟩ := Phase.committing.inj hph
        have hh := hA _ _ _ _ hx0 (Or.inl hph0)
        subst hv
        subst hb
        exact hh
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hA r x' v b hr hph
  | conflict rid base x0 v0 h1 hx0 hph0 =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      have hu := updAtt_ne s.attempt rid (s.attempt rid + 1) r hrr
      show updAtt s.attempt rid (s.attempt rid + 1) r ≤ _
      rw [hu]
      exact hA r x' v b hr hph
  | done rid o x0 h1 hx =>
    intro r x' v b hr hph
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      simp at hph
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact hA r x' v b hr hph
  | load rid t a h1 => exact fun r x v b hr hph => hA r x v b hr hph
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r x v b hr hph => hA r x v b hr hph
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r x v b hr hph => hA r x v b hr hph
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact fun r x v b hr' hph => hA r x v b hr' hph
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact fun r x v b hr' hph => hA r x v b hr' hph
  | publish sid e t p' h1 hsub ht hr => exact fun r x v b hr' hph => hA r x v b hr' hph

theorem linv_zeroNone {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r, s'.txns r = none → s'.attempt r = 0 := by
  have hz := hI.zeroNone
  cases h with
  | clientReq rid tok i => exact fun r hr => hz r hr
  | bffForward rid tok i h1 => exact fun r hr => hz r hr
  | bffRelay rid o h1 => exact fun r hr => hz r hr
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr; simp [updTxn] at hr
    · have hr' : s.txns r = none := by simpa [updTxn, hrr] using hr
      exact hz r hr'
  | noSession rid tok i h1 hx hn hsn => exact fun r hr => hz r hr
  | snap rid t v base a x h1 hx ht hl hat =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr; simp [updTxn] at hr
    · have hr' : s.txns r = none := by simpa [updTxn, hrr] using hr
      exact hz r hr'
  | commit rid x v base hx hph =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr; simp [updTxn] at hr
    · have hr' : s.txns r = none := by simpa [updTxn, hrr] using hr
      exact hz r hr'
  | conflict rid base x v h1 hx hph =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr; simp [updTxn] at hr
    · have hr' : s.txns r = none := by simpa [updTxn, hrr] using hr
      have := hz r hr'
      simpa [updAtt, hrr] using this
  | done rid o x h1 hx =>
    intro r hr
    by_cases hrr : r = rid
    · subst hrr; simp [updTxn] at hr
    · have hr' : s.txns r = none := by simpa [updTxn, hrr] using hr
      exact hz r hr'
  | load rid t a h1 => exact fun r hr => hz r hr
  | storeDecided rid tok t p' i base res o h1 hd => exact fun r hr => hz r hr
  | storeConflict rid tok t p' i base res h1 hd hb => exact fun r hr => hz r hr
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => exact fun r hr' => hz r hr'
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => exact fun r hr' => hz r hr'
  | publish sid e t p' h1 hsub ht hr => exact fun r hr' => hz r hr'

theorem linv_attBound {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hI : LInv K C N s) : ∀ r x, s'.txns r = some x → s'.attempt r ≤ s'.store.rev x.tenant := by
  have hB := hI.attBound
  have hm := rev_mono_step h
  have hA := hI.attLe
  have hC := hI.conflictLt
  have hz := hI.zeroNone
  cases h with
  | clientReq rid tok i => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | bffForward rid tok i h1 => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | bffRelay rid o h1 => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r x hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      show s.attempt r ≤ _
      rw [hz _ hx]
      exact Nat.zero_le _
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | noSession rid tok i h1 hx hn hsn => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | snap rid t v base a x0 h1 hx ht hl hat =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Nat.le_trans (hB _ _ hx) (hm _)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Nat.le_trans (hB r x' hr) (hm x'.tenant)
  | commit rid x0 v base hx hph =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Nat.le_trans (hB _ _ hx) (hm _)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Nat.le_trans (hB r x' hr) (hm x'.tenant)
  | conflict rid base x0 v h1 hx hph =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      obtain ⟨x2, hx2, hlt⟩ := hC _ base h1
      have e : x2 = x0 := Option.some.inj (hx2.symm.trans hx)
      subst e
      have hA' := hA _ x2 v base hx (Or.inr hph)
      simp only [updAtt, ↓reduceIte]
      exact Nat.succ_le_of_lt (Nat.lt_of_le_of_lt hA' hlt)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      have hu := updAtt_ne s.attempt rid (s.attempt rid + 1) r hrr
      show updAtt s.attempt rid (s.attempt rid + 1) r ≤ _
      rw [hu]
      exact Nat.le_trans (hB r x' hr) (hm x'.tenant)
  | done rid o x0 h1 hx =>
    intro r x' hr
    by_cases hrr : r = rid
    · subst hrr
      simp only [updTxn, ↓reduceIte, Option.some.injEq] at hr
      subst hr
      exact Nat.le_trans (hB _ _ hx) (hm _)
    · simp only [updTxn, hrr, ↓reduceIte] at hr
      exact Nat.le_trans (hB r x' hr) (hm x'.tenant)
  | load rid t a h1 => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | storeDecided rid tok t p' i base res o h1 hd => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | storeConflict rid tok t p' i base res h1 hd hb => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | storeAccept rid tok t p' i base res t' h1 hd hb hr' => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | storeRefuse rid tok t p' i base res e h1 hd hb hr' => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)
  | publish sid e t p' h1 hsub ht hr' => intro r x hr; exact Nat.le_trans (hB r x hr) (hm x.tenant)

theorem linv_conflictLt {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) (hI : LInv K C N s) :
    ∀ r b, s'.net (.conflict r b) → ∃ x, s'.txns r = some x ∧ b < s'.store.rev x.tenant := by
  have hC := hI.conflictLt
  have hk := fun r x hx => txn_keep_step h r x hx
  have hm := rev_mono_step h
  cases h with
  | storeConflict rid tok t p' i base res h1 hd hb =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx, hlt⟩ := hC r b h2
      exact ⟨x, hx, Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · obtain ⟨hr1, hb1⟩ := Packet.conflict.inj h2
      rw [hr1, hb1]
      obtain ⟨x, hx, -, hxt, -⟩ := hg.commitTxn _ _ _ _ _ _ _ h1
      obtain ⟨v, a, hsn, _⟩ := hg.commitCorrect _ _ _ _ _ _ _ h1
      have hbl : base ≤ s.store.rev t := (hg.snapHist _ _ _ _ _ hsn).1
      refine ⟨x, hx, ?_⟩
      show base < s.store.rev x.tenant
      rw [hxt]
      have : base ≠ s.store.rev t := hb
      omega
  | clientReq rid tok i =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx, hlt⟩ := hC r b h2
      exact ⟨x, hx, Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | bffForward rid tok i h1 =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx, hlt⟩ := hC r b h2
      exact ⟨x, hx, Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | bffRelay rid o h1 =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx, hlt⟩ := hC r b h2
      exact ⟨x, hx, Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | begin rid tok i t0 p' h1 hx hn hsn =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      obtain ⟨y, hy, hty⟩ := hk r x hx'
      exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | noSession rid tok i h1 hx hn hsn =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      obtain ⟨y, hy, hty⟩ := hk r x hx'
      exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | snap rid t v base a x h1 hx ht hl hat =>
    intro r b hp
    obtain ⟨x', hx', hlt⟩ := hC r b hp
    obtain ⟨y, hy, hty⟩ := hk r x' hx'
    exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x'.tenant)⟩
  | commit rid x v base hx hph =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x', hx', hlt⟩ := hC r b h2
      obtain ⟨y, hy, hty⟩ := hk r x' hx'
      exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x'.tenant)⟩
    · cases h2
  | conflict rid base x v h1 hx hph =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x', hx', hlt⟩ := hC r b h2
      obtain ⟨y, hy, hty⟩ := hk r x' hx'
      exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x'.tenant)⟩
    · cases h2
  | done rid o x h1 hx =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x', hx', hlt⟩ := hC r b h2
      obtain ⟨y, hy, hty⟩ := hk r x' hx'
      exact ⟨y, hy, by rw [hty]; exact Nat.lt_of_lt_of_le hlt (hm x'.tenant)⟩
    · cases h2
  | load rid t a h1 =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      exact ⟨x, hx', Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | storeDecided rid tok t p' i base res o h1 hd =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      exact ⟨x, hx', Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | storeAccept rid tok t p' i base res t' h1 hd hb hr' =>
    intro r b hp
    rcases hp with h2 | h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      exact ⟨x, hx', Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
    · cases h2
  | storeRefuse rid tok t p' i base res e h1 hd hb hr' =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      exact ⟨x, hx', Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2
  | publish sid e t p' h1 hsub ht hr' =>
    intro r b hp
    rcases hp with h2 | h2
    · obtain ⟨x, hx', hlt⟩ := hC r b h2
      exact ⟨x, hx', Nat.lt_of_lt_of_le hlt (hm x.tenant)⟩
    · cases h2

theorem linv_step {K : Kernel} {C : Config} {N : Nat} {s s' : PState} (h : PNext K C s s')
    (hg : Good K C s) (hI : LInv K C N s) (hreq : ∀ rid tok i, s'.net (.req rid tok i) → rid < N) :
    LInv K C N s' :=
  { bndP := linv_bndP h hI hreq
    bndT := linv_bndT h hI
    rev := linv_rev h hI
    doneDec := linv_doneDec h hI
    nsRep := linv_nsRep h hI
    evsCom := linv_evsCom h hI
    txnFin := linv_txnFin h hg hI
    txnLoad := linv_txnLoad h hI
    loadRev := linv_loadRev h hg hI
    snapAtt := linv_snapAtt h hI
    attLe := linv_attLe h hI
    attBound := linv_attBound h hI
    conflictLt := linv_conflictLt h hg hI
    zeroNone := linv_zeroNone h hI
    txnCom := linv_txnCom h hI }

theorem linv_init {K : Kernel} {C : Config} {N : Nat} {w0 : World} {s : PState}
    (hi : PInit w0 s) : LInv K C N s where
  bndP := fun p hp => absurd hp (hi.1 p)
  bndT := fun r hr => absurd (hi.2.1 r) hr
  rev := fun t => by rw [hi.2.2.2.2.1 t]; exact Nat.zero_le _
  doneDec := fun r o h => by rw [hi.2.2.2.2.2.2.1 r] at h; cases h
  nsRep := fun r h => by rw [hi.2.2.1 r] at h; cases h
  evsCom := fun e he => by rw [hi.2.2.2.2.2.1] at he; cases he
  txnFin := fun r x o hr _ => by rw [hi.2.1 r] at hr; cases hr
  txnLoad := fun r x hr => by rw [hi.2.1 r] at hr; cases hr
  loadRev := fun r t a hp => absurd hp (hi.1 _)
  snapAtt := fun r t v b a hp => absurd hp (hi.1 _)
  attLe := fun r x v b hr _ => by rw [hi.2.1 r] at hr; cases hr
  attBound := fun r x hr => by rw [hi.2.1 r] at hr; cases hr
  conflictLt := fun r b hp => absurd hp (hi.1 _)
  zeroNone := fun r _ => hi.2.2.2.2.2.2.2 r
  txnCom := fun r x v b hr _ => by rw [hi.2.1 r] at hr; cases hr

theorem seq_inv {K : Kernel} {C : Config} {w0 : World} {N : Nat} {b : Behaviour PState}
    (hinit : PInit w0 (b 0)) (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hN : ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N) :
    (∀ n, Good K C (b n)) ∧ (∀ n, LInv K C N (b n)) := by
  have key : ∀ n, Good K C (b n) ∧ LInv K C N (b n) := by
    intro n
    induction n with
    | zero => exact ⟨good_init hinit, linv_init hinit⟩
    | succ n ih =>
      rcases hstep n with h | h
      · exact ⟨good_step h ih.1, linv_step h ih.1 ih.2 (hN (n+1))⟩
      · rw [h]; exact ih
  exact ⟨fun n => (key n).1, fun n => (key n).2⟩

/-- The tenant of a transaction is the same at every later position. -/
theorem txn_chain {C : Config} {K : Kernel} {b : Behaviour PState} (r : RequestId)
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n) (n : Nat) (x : Txn)
    (hx : (b n).txns r = some x) : ∀ d, ∃ y, (b (n + d)).txns r = some y ∧ y.tenant = x.tenant := by
  intro d
  induction d with
  | zero => exact ⟨x, hx, rfl⟩
  | succ d ih =>
    obtain ⟨y, hy, hty⟩ := ih
    rcases hstep (n + d) with h | h
    · obtain ⟨z, hz, htz⟩ := txn_keep_step h r y hy
      exact ⟨z, by rw [show n + (d+1) = n + d + 1 by omega]; exact hz, htz.trans hty⟩
    · refine ⟨y, ?_, hty⟩
      rw [show n + (d+1) = n + d + 1 by omega, h]
      exact hy

/-- All transaction records of `r` at any two positions have the same tenant. -/
theorem txn_tenant_const {K : Kernel} {C : Config} {b : Behaviour PState} (r : RequestId)
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n) {n m : Nat} {x y : Txn}
    (hx : (b n).txns r = some x) (hy : (b m).txns r = some y) : x.tenant = y.tenant := by
  rcases Nat.le_total n m with hnm | hmn
  · obtain ⟨d, rfl⟩ : ∃ d, m = n + d := ⟨m - n, by omega⟩
    obtain ⟨z, hz, htz⟩ := txn_chain r hstep n x hx d
    rw [hz] at hy
    cases hy
    first | exact htz | exact htz.symm
  · obtain ⟨d, rfl⟩ : ∃ d, n = m + d := ⟨n - m, by omega⟩
    obtain ⟨z, hz, htz⟩ := txn_chain r hstep m y hy d
    rw [hz] at hx
    cases hx
    first | exact htz | exact htz.symm

/-- A commit packet is always met by one of the store's steps. -/
theorem store_enabled {K : Kernel} {C : Config} {rid : RequestId} {tok : Token} {t : TenantId}
    {p : PrincipalId} {i : Intent} {base : Nat} {res : Except Err Tenant} {s : PState}
    (hc : s.net (.commit rid tok t p i base res)) :
    ∃ s', StoreStep K C rid tok t p i base res s s' := by
  cases hd : s.store.decided rid with
  | some o => exact ⟨_, StoreStep.decided s o hc hd⟩
  | none =>
    by_cases hb : base = s.store.rev t
    · cases res with
      | ok t' => exact ⟨_, StoreStep.accept s t' hc hd hb rfl⟩
      | error e => exact ⟨_, StoreStep.refuse s e hc hd hb rfl⟩
    · exact ⟨_, StoreStep.conflict s hc hd hb⟩

/-- A store step on an undecided request that leaves it undecided is a conflict. -/
theorem store_conflict_only {K : Kernel} {C : Config} {rid : RequestId} {tok : Token}
    {t : TenantId} {p : PrincipalId} {i : Intent} {base : Nat} {res : Except Err Tenant}
    {s s' : PState} (h : StoreStep K C rid tok t p i base res s s')
    (h0 : s.store.decided rid = none) (h1 : s'.store.decided rid = none) :
    s'.net (.conflict rid base) := by
  cases h with
  | decided o hc hd => rw [h0] at hd; cases hd
  | conflict hc hd hb => exact Or.inr rfl
  | accept t2 hc hd hb hr => simp at h1
  | refuse e2 hc hd hb hr => simp at h1

/-- A ready transaction stays ready until its commit is taken. -/
theorem ready_keep {K : Kernel} {C : Config} {r : RequestId} {s s' : PState} (h : PNext K C s s')
    {x : Txn} {v : Option Tenant} {b : Nat} (hx : s.txns r = some x) (hph : x.phase = .ready v b)
    (hdone : ∀ o, ¬ s.net (.done r o)) (hnot : ¬ actCommit K r s s') :
    s'.txns r = s.txns r := by
  cases h with
  | clientReq rid tok i => rfl
  | bffForward rid tok i h1 => rfl
  | bffRelay rid o h1 => rfl
  | begin rid tok i t p' h1 hn0 hn hsn =>
    by_cases hrr : r = rid
    · subst hrr; rw [hn0] at hx; cases hx
    · simp [updTxn, hrr]
  | noSession rid tok i h1 hn0 hn hsn => rfl
  | snap rid t v0 base a x0 h1 hx0 ht hl hat =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      rw [hl] at hph; cases hph
    · simp [updTxn, hrr]
  | commit rid x0 v0 base hx0 hph0 =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      exact absurd ⟨_, _, _, hx0, hph0, rfl⟩ hnot
    · simp [updTxn, hrr]
  | conflict rid base x0 v0 h1 hx0 hph0 =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      rw [hph] at hph0; cases hph0
    · simp [updTxn, hrr]
  | done rid o x0 h1 hx0 =>
    by_cases hrr : r = rid
    · subst hrr; exact absurd h1 (hdone o)
    · simp [updTxn, hrr]
  | load rid t a h1 => rfl
  | storeDecided rid tok t p' i base res o h1 hd => rfl
  | storeConflict rid tok t p' i base res h1 hd hb => rfl
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => rfl
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => rfl
  | publish sid e t p' h1 hsub ht hr => rfl

/-- A committing transaction stays committing until its conflict is taken; a conflict changes it to
    loading. -/
theorem committing_keep {K : Kernel} {C : Config} {r : RequestId} {s s' : PState}
    (h : PNext K C s s') {x : Txn} {v : Option Tenant} {b : Nat} (hx : s.txns r = some x)
    (hph : x.phase = .committing v b)
    (hdone : ∀ o, ¬ s.net (.done r o)) (hnot : ¬ actConflict r s s') :
    s'.txns r = s.txns r := by
  cases h with
  | clientReq rid tok i => rfl
  | bffForward rid tok i h1 => rfl
  | bffRelay rid o h1 => rfl
  | begin rid tok i t p' h1 hn0 hn hsn =>
    by_cases hrr : r = rid
    · subst hrr; rw [hn0] at hx; cases hx
    · simp [updTxn, hrr]
  | noSession rid tok i h1 hn0 hn hsn => rfl
  | snap rid t v0 base a x0 h1 hx0 ht hl hat =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      rw [hl] at hph; cases hph
    · simp [updTxn, hrr]
  | commit rid x0 v0 base hx0 hph0 =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      rw [hph] at hph0; cases hph0
    · simp [updTxn, hrr]
  | conflict rid base x0 v0 h1 hx0 hph0 =>
    by_cases hrr : r = rid
    · subst hrr
      have e := Option.some.inj (hx0.symm.trans hx); subst e
      exact absurd ⟨_, _, _, hx0, hph0, h1, rfl⟩ hnot
    · simp [updTxn, hrr]
  | done rid o x0 h1 hx0 =>
    by_cases hrr : r = rid
    · subst hrr; exact absurd h1 (hdone o)
    · simp [updTxn, hrr]
  | load rid t a h1 => rfl
  | storeDecided rid tok t p' i base res o h1 hd => rfl
  | storeConflict rid tok t p' i base res h1 hd hb => rfl
  | storeAccept rid tok t p' i base res t' h1 hd hb hr => rfl
  | storeRefuse rid tok t p' i base res e h1 hd hb hr => rfl
  | publish sid e t p' h1 hsub ht hr => rfl

/-! ## The reply argument: lemmas along a fair behaviour -/

theorem storeStep_net {K : Kernel} {C : Config} {rid : RequestId} {tok : Token} {t : TenantId}
    {p : PrincipalId} {i : Intent} {base : Nat} {res : Except Err Tenant} {s s' : PState}
    (h : StoreStep K C rid tok t p i base res s s') : s.net (.commit rid tok t p i base res) := by
  cases h with
  | decided o hc hd => exact hc
  | conflict hc hd hb => exact hc
  | accept t2 hc hd hb hr => exact hc
  | refuse e2 hc hd hb hr => exact hc

theorem store_step_undecided {K : Kernel} {C : Config} {rid : RequestId} {tok : Token}
    {t : TenantId} {p : PrincipalId} {i : Intent} {base : Nat} {res : Except Err Tenant}
    {s s' : PState} (h : StoreStep K C rid tok t p i base res s s')
    (h0 : s.store.decided rid = none) (h1 : s'.store.decided rid = none) :
    base ≠ s.store.rev t := by
  cases h with
  | decided o hc hd => rw [h0] at hd; cases hd
  | conflict hc hd hb => exact hb
  | accept t2 hc hd hb hr => simp at h1
  | refuse e2 hc hd hb hr => simp at h1

theorem eventually_const_aux (f : Nat → Nat) (hm : ∀ n, f n ≤ f (n+1)) (N : Nat)
    (hb : ∀ n, f n ≤ N) : ∀ d n0, N - f n0 ≤ d → ∃ T, ∀ n, T ≤ n → f n = f T := by
  have mono : ∀ a d, f a ≤ f (a + d) := by
    intro a d
    induction d with
    | zero => exact Nat.le_refl _
    | succ d ih => exact Nat.le_trans ih (hm (a + d))
  have mono' : ∀ a b, a ≤ b → f a ≤ f b := by
    intro a b hab
    obtain ⟨d, rfl⟩ : ∃ d, b = a + d := ⟨b - a, by omega⟩
    exact mono a d
  intro d
  induction d with
  | zero =>
    intro n0 hd
    refine ⟨n0, fun n hn => ?_⟩
    have h1 := hb n
    have h2 := mono' n0 n hn
    have h3 := hb n0
    omega
  | succ d ih =>
    intro n0 hd
    by_cases hall : ∀ n, n0 ≤ n → f n = f n0
    · exact ⟨n0, hall⟩
    · have hex : ∃ n, n0 ≤ n ∧ f n ≠ f n0 := by
        refine Classical.byContradiction fun hc => hall (fun n hn => ?_)
        refine Classical.byContradiction fun hne => hc ⟨n, hn, hne⟩
      obtain ⟨n, hn, hne⟩ := hex
      have hlt : f n0 < f n := by
        have := mono' n0 n hn
        omega
      exact ih n (by have := hb n; omega)

/-- A bounded monotone sequence is eventually constant. -/
theorem eventually_const (f : Nat → Nat) (hm : ∀ n, f n ≤ f (n+1)) (N : Nat)
    (hb : ∀ n, f n ≤ N) : ∃ T, ∀ n, T ≤ n → f n = f T :=
  eventually_const_aux f hm N hb (N - f 0) 0 (Nat.le_refl _)

theorem txn_some_seq {K : Kernel} {C : Config} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n) (m : Nat) (r : RequestId)
    (x : Txn) (hx : (b m).txns r = some x) : ∃ y, (b (m+1)).txns r = some y := by
  rcases hstep m with h | h
  · obtain ⟨y, hy, _⟩ := txn_keep_step h r x hx
    exact ⟨y, hy⟩
  · rw [h]; exact ⟨x, hx⟩

/-- A relay is taken once its reply is in the network. -/
theorem relay_of_reply {K : Kernel} {C : Config} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hRel : ∀ rid o, weakFair (actRelay rid o) b) (rid : RequestId) (o : Outcome) (n : Nat)
    (h : (b n).net (.reply rid o)) : ∃ m, (b m).net (.clientReply rid o) := by
  have hp : ∀ m, n ≤ m → enabled (actRelay rid o) (b m) →
      ¬ actRelay rid o (b m) (b (m+1)) → enabled (actRelay rid o) (b (m+1)) := by
    intro m hm hen _
    obtain ⟨s', h1, _⟩ := hen
    exact ⟨sendOne (b (m+1)) (.clientReply rid o), net_seq hstep m (m+1) (by omega) _ h1, rfl⟩
  obtain ⟨j, _, hA⟩ := fair_pers (hRel rid o) hp
    ⟨sendOne (b n) (.clientReply rid o), h, rfl⟩
  exact ⟨j+1, by rw [hA.2]; exact Or.inr rfl⟩

/-- A finished transaction's reply is relayed. -/
theorem relay_of_done {K : Kernel} {C : Config} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hRel : ∀ rid o, weakFair (actRelay rid o) b)
    (hDone : ∀ rid o, weakFair (actDone rid o) b) (r : RequestId) (o : Outcome) (n : Nat)
    (hd : (b n).net (.done r o)) (x : Txn) (hx : (b n).txns r = some x) :
    ∃ m, (b m).net (.clientReply r o) := by
  have hp : ∀ m, n ≤ m → enabled (actDone r o) (b m) →
      ¬ actDone r o (b m) (b (m+1)) → enabled (actDone r o) (b (m+1)) := by
    intro m _ hen _
    obtain ⟨_, x', hx', hdn, _⟩ := hen
    obtain ⟨y, hy⟩ := txn_some_seq hstep m r x' hx'
    refine ⟨{ sendOne (b (m+1)) (.reply r o) with
        txns := updTxn (b (m+1)).txns r { y with phase := .finished o } }, y, hy, ?_, rfl⟩
    exact net_seq hstep m (m+1) (by omega) _ hdn
  obtain ⟨j, _, hA⟩ := fair_pers (hDone r o) hp
    ⟨{ sendOne (b n) (.reply r o) with txns := updTxn (b n).txns r { x with phase := .finished o } },
      x, hx, hd, rfl⟩
  obtain ⟨x2, _, _, hout⟩ := hA
  exact relay_of_reply hstep hRel r o (j+1) (by rw [hout]; exact Or.inr rfl)

/-- Under the hypothesis that no reply to `r` is ever sent, `r` is never answered without a session. -/
theorem nosession_false {K : Kernel} {C : Config} {N : Nat} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hRel : ∀ rid o, weakFair (actRelay rid o) b)
    (hLinv : ∀ n, LInv K C N (b n)) (r : RequestId)
    (H0 : ∀ m o, ¬ (b m).net (.clientReply r o)) (n : Nat) : (b n).noSession r = false := by
  cases h : (b n).noSession r with
  | false => rfl
  | true =>
    exfalso
    obtain ⟨m, hm⟩ := relay_of_reply hstep hRel r .noSession n ((hLinv n).nsRep r h)
    exact H0 m .noSession hm

/-- Under the hypothesis that no reply to `r` is ever sent, `r` is never decided. -/
theorem decided_none_of {K : Kernel} {C : Config} {N : Nat} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hRel : ∀ rid o, weakFair (actRelay rid o) b)
    (hDone : ∀ rid o, weakFair (actDone rid o) b)
    (hGood : ∀ n, Good K C (b n)) (hLinv : ∀ n, LInv K C N (b n)) (r : RequestId)
    (H0 : ∀ m o, ¬ (b m).net (.clientReply r o)) (n : Nat) : (b n).store.decided r = none := by
  cases h : (b n).store.decided r with
  | none => rfl
  | some o =>
    exfalso
    obtain ⟨x, hx⟩ := (hGood n).decidedTxn r o h
    obtain ⟨m, hm⟩ := relay_of_done hstep hRel hDone r o n ((hLinv n).doneDec r o h) x hx
    exact H0 m o hm

/-- Under the hypotheses of the reply argument, the request's transaction is eventually created. -/
theorem txn_exists {K : Kernel} {C : Config} {b : Behaviour PState}
    (hstep : ∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n)
    (hFwd : ∀ rid tok i, weakFair (actFwd rid tok i) b)
    (hBeg : ∀ rid tok i, weakFair (actBegin C rid tok i) b)
    (r : RequestId) (tok : Token) (i : Intent) (t : TenantId) (p : PrincipalId)
    (hs : C.sessions tok = some (t, p)) (k : Nat) (hreq : (b k).net (.req r tok i))
    (hnsF : ∀ n, (b n).noSession r = false) :
    ∃ n x, (b n).txns r = some x := by
  refine Classical.byContradiction fun hcon => ?_
  have hnone : ∀ n, (b n).txns r = none := by
    intro n
    cases h : (b n).txns r with
    | none => rfl
    | some x => exact absurd ⟨n, x, h⟩ hcon
  have hp1 : ∀ m, k ≤ m → enabled (actFwd r tok i) (b m) →
      ¬ actFwd r tok i (b m) (b (m+1)) → enabled (actFwd r tok i) (b (m+1)) := by
    intro m hm hen _
    obtain ⟨s', h1, _⟩ := hen
    exact ⟨sendOne (b (m+1)) (.fwd r tok i), net_seq hstep m (m+1) (by omega) _ h1, rfl⟩
  obtain ⟨j, hjk, hA⟩ := fair_pers (hFwd r tok i) hp1 ⟨sendOne (b k) (.fwd r tok i), hreq, rfl⟩
  have hfwd : (b (j+1)).net (.fwd r tok i) := by rw [hA.2]; exact Or.inr rfl
  have hp2 : ∀ m, j+1 ≤ m → enabled (actBegin C r tok i) (b m) →
      ¬ actBegin C r tok i (b m) (b (m+1)) → enabled (actBegin C r tok i) (b (m+1)) := by
    intro m hm _ _
    exact ⟨{ sendOne (b (m+1)) (.load r t ((b (m+1)).attempt r)) with
        txns := updTxn (b (m+1)).txns r ⟨tok, t, p, i, .loading⟩ }, t, p, hs,
      net_seq hstep (j+1) (m+1) (by omega) _ hfwd, hnone (m+1), hnsF (m+1), rfl⟩
  have hen0 : enabled (actBegin C r tok i) (b (j+1)) :=
    ⟨{ sendOne (b (j+1)) (.load r t ((b (j+1)).attempt r)) with
        txns := updTxn (b (j+1)).txns r ⟨tok, t, p, i, .loading⟩ }, t, p, hs, hfwd,
      hnone (j+1), hnsF (j+1), rfl⟩
  obtain ⟨j2, _, hB⟩ := fair_pers (hBeg r tok i) hp2 hen0
  obtain ⟨t2, p2, _, _, _, _, hout⟩ := hB
  have := hnone (j2+1)
  rw [hout] at this
  simp [updTxn] at this

/-- The attempt number never decreases. -/
theorem attempt_mono_step {K : Kernel} {C : Config} {s s' : PState} (h : PNext K C s s')
    (r : RequestId) : s.attempt r ≤ s'.attempt r := by
  cases h with
  | conflict rid base x v h1 hx hph =>
    by_cases hr : r = rid
    · subst hr; simp [updAtt]
    · exact Nat.le_of_eq (updAtt_ne s.attempt rid (s.attempt rid + 1) r hr).symm
  | _ => exact Nat.le_refl _

/-- A conflict step moves the attempt number on by one. -/
theorem conflict_inc (r : RequestId) (s s' : PState) (h : actConflict r s s') :
    s'.attempt r = s.attempt r + 1 := by
  obtain ⟨x, v, b, -, -, -, rfl⟩ := h
  simp [updAtt]

/-- The parts of the fair spec. -/
theorem protoSpec_parts {K : Kernel} {C : Config} {w0 : World} {b : Behaviour PState}
    (h : protoSpec K C w0 b) :
    PInit w0 (b 0) ∧ (∀ n, PNext K C (b n) (b (n+1)) ∨ b (n+1) = b n) ∧ protoFair K C b :=
  ⟨h.1, fun n => h.2.1 n, h.2.2⟩

/-- A request with a valid session is answered: no behaviour of the fair spec keeps it unanswered,
    under the bounded-workload hypothesis (every request id is below `N`). The argument, by
    contradiction. The attempt number of the request's transaction never decreases and is bounded
    by the revision, so it is eventually constant, from `T` on. From `T` on there is no conflict and
    no commit: a commit would be answered by the store as a conflict, which would move the attempt
    on. So the transaction is loading from `T` on, and the store answers its load with a snapshot
    of the current attempt, which the backend takes. That step makes the transaction ready and then
    committing, which the argument excludes. Only weak fairness is used. -/
theorem reply_core {K : Kernel} {C : Config} {w0 : World} {N : Nat} {b : Behaviour PState}
    (hspec : protoSpec K C w0 b) (hN : ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N)
    (r : RequestId) (tok : Token) (i : Intent) (t : TenantId) (p : PrincipalId)
    (hs : C.sessions tok = some (t, p)) (k : Nat) (hreq : (b k).net (.req r tok i)) :
    ∃ m o, (b m).net (.clientReply r o) := by
  obtain ⟨hinit, hstep, hF⟩ := protoSpec_parts hspec
  obtain ⟨hFwd, hBeg, hRel, hDone, hLoad, hCom, hConf, hSto, hSnap, _⟩ := hF
  obtain ⟨hGood, hLinv⟩ := seq_inv hinit hstep hN
  refine Classical.byContradiction fun hcon => ?_
  have H0 : ∀ m o, ¬ (b m).net (.clientReply r o) := fun m o h => hcon ⟨m, o, h⟩
  have hnsF := nosession_false hstep hRel hLinv r H0
  have hdecN := decided_none_of hstep hRel hDone hGood hLinv r H0
  have noDone : ∀ m o, ¬ (b m).net (.done r o) := fun m o h => by
    have := (hGood m).doneDecided r o h
    rw [hdecN m] at this
    cases this
  have noFin : ∀ m y o, (b m).txns r = some y → y.phase ≠ .finished o := fun m y o hy hph => by
    have := (hLinv m).txnFin r y o hy hph
    rw [hdecN m] at this
    cases this
  obtain ⟨n0, x0, hx0⟩ := txn_exists hstep hFwd hBeg r tok i t p hs k hreq hnsF
  have recAt : ∀ m, n0 ≤ m → ∃ y, (b m).txns r = some y := fun m hm => by
    obtain ⟨d, rfl⟩ : ∃ d, m = n0 + d := ⟨m - n0, by omega⟩
    obtain ⟨y, hy, _⟩ := txn_chain r hstep n0 x0 hx0 d
    exact ⟨y, hy⟩
  -- the attempt number is bounded by the revision, and never decreases
  have hbnd : ∀ d, (b (n0 + d)).attempt r ≤ N := by
    intro d
    obtain ⟨y, hy, _⟩ := txn_chain r hstep n0 x0 hx0 d
    exact Nat.le_trans ((hLinv _).attBound r y hy)
      (Nat.le_trans ((hLinv _).rev y.tenant) (dcount_le _ N))
  have hmono : ∀ d, (b (n0 + d)).attempt r ≤ (b (n0 + (d+1))).attempt r := by
    intro d
    rcases hstep (n0 + d) with h | h
    · exact attempt_mono_step h r
    · rw [show n0 + (d+1) = n0 + d + 1 by omega, h]; exact Nat.le_refl _
  obtain ⟨T', hT'⟩ := eventually_const (fun d => (b (n0 + d)).attempt r) hmono N hbnd
  have hAconst : ∀ m, n0 + T' ≤ m → (b m).attempt r = (b (n0 + T')).attempt r := by
    intro m hm
    have h2 : (b (n0 + (m - n0))).attempt r = (b (n0 + T')).attempt r := hT' (m - n0) (by omega)
    rw [show n0 + (m - n0) = m by omega] at h2
    exact h2
  obtain ⟨T, hnT, hT⟩ : ∃ T, n0 ≤ T ∧ ∀ m, T ≤ m → (b m).attempt r = (b T).attempt r :=
    ⟨n0 + T', by omega, hAconst⟩
  -- from `T` on there is no conflict (it would move the attempt on)
  have noConf : ∀ j, T ≤ j → ¬ actConflict r (b j) (b (j+1)) := by
    intro j hj hA
    have h1 := conflict_inc r _ _ hA
    have h2 := hT (j+1) (by omega)
    have h3 := hT j hj
    omega
  -- committing records persist from `m` on while there is no conflict
  have commit_frozen : ∀ m, T ≤ m → ∀ y v bb, (b m).txns r = some y → y.phase = .committing v bb →
      ∀ d, (b (m+d)).txns r = some y := by
    intro m hm y v bb hy hph d
    induction d with
    | zero => exact hy
    | succ d ih =>
      show (b (m+d+1)).txns r = some y
      rcases hstep (m+d) with h | h
      · have hk := committing_keep h ih hph (fun o => noDone (m+d) o) (noConf (m+d) (by omega))
        rw [hk]; exact ih
      · rw [h]; exact ih
  -- from `T` on there is no commit: the store would answer it with a conflict
  have noCommitting : ∀ m, T ≤ m → ∀ y v bb, (b m).txns r = some y →
      y.phase = .committing v bb → False := by
    intro m hm y v bb hy hph
    have hcp := (hLinv m).txnCom r y v bb hy hph
    have hpS : ∀ m', m ≤ m' →
        enabled (StoreStep K C r y.tok y.tenant y.principal y.intent bb (verdict K v y.principal y.intent))
          (b m') →
        ¬ StoreStep K C r y.tok y.tenant y.principal y.intent bb (verdict K v y.principal y.intent)
          (b m') (b (m'+1)) →
        enabled (StoreStep K C r y.tok y.tenant y.principal y.intent bb (verdict K v y.principal y.intent))
          (b (m'+1)) := by
      intro m' _ hen _
      obtain ⟨s', hs'⟩ := hen
      exact store_enabled (net_seq hstep m' (m'+1) (by omega) _ (storeStep_net hs'))
    obtain ⟨j, hj, hSJ⟩ := fair_pers (hSto r y.tok y.tenant y.principal y.intent bb
      (verdict K v y.principal y.intent)) hpS (store_enabled hcp)
    have hconf1 := store_conflict_only hSJ (hdecN j) (hdecN (j+1))
    have hpC : ∀ m', j+1 ≤ m' → enabled (actConflict r) (b m') →
        ¬ actConflict r (b m') (b (m'+1)) → enabled (actConflict r) (b (m'+1)) := by
      intro m' hm' _ _
      have hyk := commit_frozen m hm y v bb hy hph (m' + 1 - m)
      rw [show m + (m' + 1 - m) = m' + 1 by omega] at hyk
      have hconfm : (b (m'+1)).net (.conflict r bb) := net_seq hstep (j+1) (m'+1) (by omega) _ hconf1
      exact ⟨{ sendOne (b (m'+1)) (.load r y.tenant ((b (m'+1)).attempt r + 1)) with
        txns := updTxn (b (m'+1)).txns r { y with phase := .loading },
        attempt := updAtt (b (m'+1)).attempt r ((b (m'+1)).attempt r + 1) },
        y, v, bb, hyk, hph, hconfm, rfl⟩
    have he1 : enabled (actConflict r) (b (j+1)) := by
      have hyk := commit_frozen m hm y v bb hy hph (j + 1 - m)
      rw [show m + (j + 1 - m) = j + 1 by omega] at hyk
      exact ⟨{ sendOne (b (j+1)) (.load r y.tenant ((b (j+1)).attempt r + 1)) with
        txns := updTxn (b (j+1)).txns r { y with phase := .loading },
        attempt := updAtt (b (j+1)).attempt r ((b (j+1)).attempt r + 1) },
        y, v, bb, hyk, hph, hconf1, rfl⟩
    obtain ⟨j2, hj2, hC2⟩ := fair_pers (hConf r) hpC he1
    have h1 := conflict_inc r _ _ hC2
    have h2 := hT (j2+1) (by omega)
    have h3 := hT j2 (by omega)
    omega
  -- from `T` on there is no ready record: its commit would be a committing record
  have noReady : ∀ m, T ≤ m → ∀ y v bb, (b m).txns r = some y → y.phase = .ready v bb → False := by
    intro m hm y v bb hy hph
    have hpR : ∀ m', m ≤ m' → enabled (actCommit K r) (b m') →
        ¬ actCommit K r (b m') (b (m'+1)) → enabled (actCommit K r) (b (m'+1)) := by
      intro m' _ hen hnot
      rcases hstep m' with h | h
      · obtain ⟨_, y', v', b', hy', hph', _⟩ := hen
        have hk := ready_keep h hy' hph' (fun o => noDone m' o) hnot
        exact ⟨{ sendOne (b (m'+1)) (.commit r y'.tok y'.tenant y'.principal y'.intent b'
          (verdict K v' y'.principal y'.intent)) with
          txns := updTxn (b (m'+1)).txns r { y' with phase := .committing v' b' } },
          y', v', b', by rw [hk]; exact hy', hph', rfl⟩
      · rw [h]; exact hen
    have hen0 : enabled (actCommit K r) (b m) :=
      ⟨{ sendOne (b m) (.commit r y.tok y.tenant y.principal y.intent bb
          (verdict K v y.principal y.intent)) with
          txns := updTxn (b m).txns r { y with phase := .committing v bb } },
        y, v, bb, hy, hph, rfl⟩
    obtain ⟨j, hjm, hAc⟩ := fair_pers (hCom r) hpR hen0
    obtain ⟨y2, v2, b2, -, -, hout⟩ := hAc
    have hrec : (b (j+1)).txns r = some { y2 with phase := .committing v2 b2 } := by
      rw [hout]; simp [updTxn]
    exact noCommitting (j+1) (by omega) _ v2 b2 hrec rfl
  -- from `T` on the transaction is loading
  have loadingAt : ∀ m, T ≤ m → ∀ y, (b m).txns r = some y → y.phase = .loading := by
    intro m hm y hy
    cases hph : y.phase with
    | loading => rfl
    | ready v bb => exact (noReady m hm y v bb hy hph).elim
    | committing v bb => exact (noCommitting m hm y v bb hy hph).elim
    | finished o => exact (noFin m y o hy hph).elim
  -- the store answers the load of the final attempt, and the backend takes the snapshot
  obtain ⟨y0, hy0⟩ := recAt T (by omega)
  have hL0 := (hLinv T).txnLoad r y0 hy0
  have henL : enabled (actLoad r y0.tenant ((b T).attempt r)) (b T) :=
    ⟨sendOne (b T) (.snap r y0.tenant ((b T).store.hist y0.tenant ((b T).store.rev y0.tenant))
      ((b T).store.rev y0.tenant) ((b T).attempt r)), hL0, rfl⟩
  have hpL : ∀ m, T ≤ m → enabled (actLoad r y0.tenant ((b T).attempt r)) (b m) →
      ¬ actLoad r y0.tenant ((b T).attempt r) (b m) (b (m+1)) →
      enabled (actLoad r y0.tenant ((b T).attempt r)) (b (m+1)) := by
    intro m hm hen _
    obtain ⟨s', h1, _⟩ := hen
    exact ⟨sendOne (b (m+1)) (.snap r y0.tenant ((b (m+1)).store.hist y0.tenant ((b (m+1)).store.rev y0.tenant))
      ((b (m+1)).store.rev y0.tenant) ((b T).attempt r)), net_seq hstep m (m+1) (by omega) _ h1, rfl⟩
  obtain ⟨j, hjT, hLj⟩ := fair_pers (hLoad r y0.tenant ((b T).attempt r)) hpL henL
  have hsnapj : (b (j+1)).net (.snap r y0.tenant ((b j).store.hist y0.tenant ((b j).store.rev y0.tenant))
      ((b j).store.rev y0.tenant) ((b T).attempt r)) := by
    rw [hLj.2]
    exact Or.inr rfl
  obtain ⟨v, base, hsn⟩ : ∃ v base, (b (j+1)).net (.snap r y0.tenant v base ((b T).attempt r)) :=
    ⟨_, _, hsnapj⟩
  have hpS2 : ∀ m, j+1 ≤ m → enabled (actSnap r y0.tenant v base ((b T).attempt r)) (b m) →
      ¬ actSnap r y0.tenant v base ((b T).attempt r) (b m) (b (m+1)) →
      enabled (actSnap r y0.tenant v base ((b T).attempt r)) (b (m+1)) := by
    intro m hm _ _
    obtain ⟨z, hz⟩ := recAt (m+1) (by omega)
    have hzl := loadingAt (m+1) (by omega) z hz
    have hzt : y0.tenant = z.tenant := txn_tenant_const r hstep hy0 hz
    have hat : (b (m+1)).attempt r = (b T).attempt r := hT (m+1) (by omega)
    exact ⟨{ b (m+1) with txns := updTxn (b (m+1)).txns r { z with phase := .ready v base } },
      z, hz, hzt.symm, hzl, hat, net_seq hstep (j+1) (m+1) (by omega) _ hsn, rfl⟩
  have hen2 : enabled (actSnap r y0.tenant v base ((b T).attempt r)) (b (j+1)) := by
    obtain ⟨z, hz⟩ := recAt (j+1) (by omega)
    have hzl := loadingAt (j+1) (by omega) z hz
    have hzt : y0.tenant = z.tenant := txn_tenant_const r hstep hy0 hz
    have hat : (b (j+1)).attempt r = (b T).attempt r := hT (j+1) (by omega)
    exact ⟨{ b (j+1) with txns := updTxn (b (j+1)).txns r { z with phase := .ready v base } },
      z, hz, hzt.symm, hzl, hat, hsn, rfl⟩
  obtain ⟨j2, hj2, hS2⟩ := fair_pers (hSnap r y0.tenant v base ((b T).attempt r)) hpS2 hen2
  obtain ⟨x2, hx2, -, -, -, -, hout2⟩ := hS2
  have hrec2 : (b (j2+1)).txns r = some { x2 with phase := .ready v base } := by
    rw [hout2]; simp [updTxn]
  exact noReady (j2+1) (by omega) _ v base hrec2 rfl

/-- A request with a valid session is eventually answered. -/
theorem reply_leadsTo {K : Kernel} {C : Config} {w0 : World} {N : Nat} {b : Behaviour PState}
    (hspec : protoSpec K C w0 b) (hN : ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N)
    (r : RequestId) (tok : Token) (i : Intent) (t : TenantId) (p : PrincipalId)
    (hs : C.sessions tok = some (t, p)) :
    leadsTo (lift (fun s => s.net (.req r tok i)))
      (lift (fun s => ∃ o, s.net (.clientReply r o))) b := by
  intro k hk
  have hk' : (b k).net (.req r tok i) := hk
  obtain ⟨m, o, hm⟩ := reply_core hspec hN r tok i t p hs k hk'
  have hstep := (protoSpec_parts hspec).2.1
  have hm' : (b (max m k)).net (.clientReply r o) :=
    net_seq hstep m (max m k) (Nat.le_max_left _ _) _ hm
  refine ⟨max m k - k, o, ?_⟩
  show (b (k + (max m k - k))).net (.clientReply r o)
  have e : k + (max m k - k) = max m k := by omega
  rw [e]
  exact hm'

/-- A committed event is delivered as a frame to every subscriber whose principal may read it. -/
theorem publish_leadsTo {K : Kernel} {C : Config} {w0 : World} {N : Nat} {b : Behaviour PState}
    (hspec : protoSpec K C w0 b) (hN : ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N)
    (e : Event) (sid : SubId) (t : TenantId) (p : PrincipalId) (hsub : C.subs sid = some (t, p))
    (htn : e.tenant = t) (hread : K.readable e.post p) :
    leadsTo (lift (fun s => e ∈ s.store.evs)) (lift (fun s => s.net (.frame sid e))) b := by
  obtain ⟨hinit, hstep, hF⟩ := protoSpec_parts hspec
  obtain ⟨_, _, _, _, _, _, _, _, _, hPub⟩ := hF
  obtain ⟨_, hLinv⟩ := seq_inv hinit hstep hN
  intro k hk
  have hk' : e ∈ (b k).store.evs := hk
  have hcom : (b k).net (.committed e) := (hLinv k).evsCom e hk'
  have hp : ∀ m, k ≤ m → enabled (actPublish K C sid e) (b m) →
      ¬ actPublish K C sid e (b m) (b (m+1)) → enabled (actPublish K C sid e) (b (m+1)) := by
    intro m hm hen _
    obtain ⟨s', h1, _⟩ := hen
    exact ⟨sendOne (b (m+1)) (.frame sid e), net_seq hstep m (m+1) (by omega) _ h1,
      ⟨t, p, hsub, htn, hread⟩, rfl⟩
  obtain ⟨j, hj, hA⟩ := fair_pers (hPub sid e) hp
    ⟨sendOne (b k) (.frame sid e), hcom, ⟨t, p, hsub, htn, hread⟩, rfl⟩
  have hfr : (b (j+1)).net (.frame sid e) := by
    obtain ⟨_, _, hout⟩ := hA
    rw [hout]
    exact Or.inr rfl
  refine ⟨j + 1 - k, ?_⟩
  show (b (k + (j + 1 - k))).net (.frame sid e)
  have e' : k + (j + 1 - k) = j + 1 := by omega
  rw [e']
  exact hfr

/-- A decided request's reply is eventually relayed to the client. -/
theorem decided_relayed_leadsTo {K : Kernel} {C : Config} {w0 : World} {N : Nat}
    {b : Behaviour PState} (hspec : protoSpec K C w0 b)
    (hN : ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N)
    (r : RequestId) (o : Outcome) :
    leadsTo (lift (fun s => s.store.decided r = some o))
      (lift (fun s => s.net (.clientReply r o))) b := by
  obtain ⟨hinit, hstep, hF⟩ := protoSpec_parts hspec
  obtain ⟨_, _, hRel, hDone, _, _, _, _, _, _⟩ := hF
  obtain ⟨hGood, hLinv⟩ := seq_inv hinit hstep hN
  intro k hk
  have hk' : (b k).store.decided r = some o := hk
  obtain ⟨x, hx⟩ := (hGood k).decidedTxn r o hk'
  obtain ⟨m, hm⟩ := relay_of_done hstep hRel hDone r o k ((hLinv k).doneDec r o hk') x hx
  have hm' : (b (max m k)).net (.clientReply r o) :=
    net_seq hstep m (max m k) (Nat.le_max_left _ _) _ hm
  refine ⟨max m k - k, ?_⟩
  show (b (k + (max m k - k))).net (.clientReply r o)
  have e : k + (max m k - k) = max m k := by omega
  rw [e]
  exact hm'


/-! ## Non-vacuity: a fair run in which request 1 is accepted, answered and published

The accepting run written out state by state, then constant. In the last state every enabled host
action is a no-op, so the constant tail satisfies each fairness assumption. -/

/-- A packet already in the network does not change the state when sent again. -/
theorem sendOne_self (s : PState) (p : Packet) (h : s.net p) : sendOne s p = s := by
  have hf : (fun x => s.net x ∨ x = p) = s.net :=
    funext fun x => propext ⟨fun hx => hx.elim id (fun e => e ▸ h), fun hx => Or.inl hx⟩
  show ({ s with net := fun x => s.net x ∨ x = p } : PState) = s
  rw [hf]

/-- Updating a transaction record to the value it already has changes nothing. -/
theorem updTxn_same (f : RequestId → Option Txn) (rid : RequestId) (x : Txn) (h : f rid = some x) :
    updTxn f rid x = f := by
  funext r
  by_cases hr : r = rid
  · subst hr; simp [updTxn, h]
  · simp [updTxn, hr]

/-- A constant tail satisfies an action's weak fairness if every enabled instance of the action is
    a step that leaves the state unchanged. -/
theorem tail_always {A : PState → PState → Prop} {b : Behaviour PState} {S : PState} {M : Nat}
    (hS : ∀ m, M ≤ m → b m = S) (hself : enabled A S → A S S) (hen : enabled A S) :
    always (eventually (act A)) b := by
  intro j
  refine ⟨M, ?_⟩
  show A (b (j+M)) (b (j+M+1))
  rw [hS (j+M) (by omega), hS (j+M+1) (by omega)]
  exact hself hen

theorem tail_weak {A : PState → PState → Prop} {b : Behaviour PState} {S : PState} {M : Nat}
    (hS : ∀ m, M ≤ m → b m = S) (hself : enabled A S → A S S) : weakFair A b := by
  intro hev
  obtain ⟨n, hn⟩ := hev
  have h1 : enabled A (b (n + M)) := hn M
  rw [hS (n+M) (by omega)] at h1
  exact tail_always hS hself h1

/-- The tenant after the accepted create of request 1 (the kernel's verdict on the empty create). -/
def aT1 : Tenant :=
  { wTenant with types := [(1, { schema := [], records := [(0, { owner := 7, version := 1, fields := [] })] })], nextId := 1 }

theorem aT1_spec : stepTenant wTenant 7 (.create 1 []) = .ok aT1 := rfl

theorem aT1_owner : aT1.owner = 7 := rfl

theorem aT1_can : canCreate aT1 7 = true := rfl

/-- Token 0 belongs to principal 9 (refused); token 1 to principal 7 (the owner); subscriber 0 reads
    tenant 0 as principal 7. -/
def aConfig : Config :=
  { sessions := fun tok => if tok = 0 then some (0, 9) else if tok = 1 then some (0, 7) else none
    subs := fun sid => if sid = 0 then some (0, 7) else none }

/-- The committed event of request 1. -/
def aEv : Event := { tenant := 0, rid := 1, post := aT1 }

def a1 : PState := sendOne wInit (.req 1 1 (.create 1 []))
def a2 : PState := sendOne a1 (.fwd 1 1 (.create 1 []))
def a3 : PState := { sendOne a2 (.load 1 0 0) with
  txns := updTxn a2.txns 1 ⟨1, 0, 7, .create 1 [], .loading⟩ }
def a4 : PState := sendOne a3 (.snap 1 0 (wWorld 0) 0 0)
def a5 : PState := { a4 with txns := updTxn a4.txns 1 ⟨1, 0, 7, .create 1 [], .ready (wWorld 0) 0⟩ }
def a6 : PState := { sendOne a5 (.commit 1 1 0 7 (.create 1 []) 0
    (verdict wKernel (wWorld 0) 7 (.create 1 []))) with
  txns := updTxn a5.txns 1 ⟨1, 0, 7, .create 1 [], .committing (wWorld 0) 0⟩ }
def a7 : PState :=
  { net := fun x => a6.net x ∨ x = .done 1 .accepted ∨ x = .committed aEv
    txns := a6.txns
    noSession := a6.noSession
    attempt := a6.attempt
    store := { hist := fun x k => if x = 0 ∧ k = 0 + 1 then some aT1 else a6.store.hist x k
               rev := fun x => if x = 0 then 0 + 1 else a6.store.rev x
               evs := a6.store.evs ++ [aEv]
               decided := fun r => if r = 1 then some .accepted else a6.store.decided r } }
def a8 : PState := { sendOne a7 (.reply 1 .accepted) with
  txns := updTxn a7.txns 1 ⟨1, 0, 7, .create 1 [], .finished .accepted⟩ }
def a9 : PState := sendOne a8 (.clientReply 1 .accepted)
def a10 : PState := sendOne a9 (.snap 1 0 (some aT1) 1 0)
def a11 : PState := sendOne a10 (.frame 0 aEv)

theorem a1_step : PNext wKernel aConfig wInit a1 := PNext.clientReq wInit 1 1 (.create 1 [])

theorem a2_step : PNext wKernel aConfig a1 a2 := PNext.bffForward a1 1 1 (.create 1 []) (Or.inr rfl)

theorem a3_step : PNext wKernel aConfig a2 a3 :=
  PNext.begin a2 1 1 (.create 1 []) 0 7 (Or.inr rfl) rfl rfl rfl

theorem a4_step : PNext wKernel aConfig a3 a4 := PNext.load a3 1 0 0 (Or.inr rfl)

theorem a5_step : PNext wKernel aConfig a4 a5 :=
  PNext.snap a4 1 0 (wWorld 0) 0 0 ⟨1, 0, 7, .create 1 [], .loading⟩ (Or.inr rfl) rfl rfl rfl rfl

theorem a6_step : PNext wKernel aConfig a5 a6 :=
  PNext.commit a5 1 ⟨1, 0, 7, .create 1 [], .ready (wWorld 0) 0⟩ (wWorld 0) 0 rfl rfl

theorem a7_step : PNext wKernel aConfig a6 a7 :=
  PNext.storeAccept a6 1 1 0 7 (.create 1 []) 0 (verdict wKernel (wWorld 0) 7 (.create 1 [])) aT1
    (Or.inr rfl) rfl rfl aT1_spec

theorem a8_step : PNext wKernel aConfig a7 a8 :=
  PNext.done a7 1 .accepted ⟨1, 0, 7, .create 1 [], .committing (wWorld 0) 0⟩ (Or.inr (Or.inl rfl)) rfl

theorem a9_step : PNext wKernel aConfig a8 a9 := PNext.bffRelay a8 1 .accepted (Or.inr rfl)

theorem a9_load : a9.net (.load 1 0 0) := by
  simp [a9, a8, a7, a6, a5, a4, a3, a2, a1, sendOne, wInit]

theorem a10_step : PNext wKernel aConfig a9 a10 := PNext.load a9 1 0 0 a9_load

theorem a10_committed : a10.net (.committed aEv) := by
  simp [a10, a9, a8, a7, sendOne]

theorem a11_step : PNext wKernel aConfig a10 a11 :=
  PNext.publish a10 0 aEv 0 7 a10_committed rfl rfl rfl

/-- The accepted run, then its last state forever. -/
def atr : Nat → PState
  | 0 => wInit
  | 1 => a1
  | 2 => a2
  | 3 => a3
  | 4 => a4
  | 5 => a5
  | 6 => a6
  | 7 => a7
  | 8 => a8
  | 9 => a9
  | 10 => a10
  | _ => a11

theorem atr_step (n : Nat) : PNext wKernel aConfig (atr n) (atr (n+1)) ∨ atr (n+1) = atr n := by
  match n with
  | 0 => exact Or.inl a1_step
  | 1 => exact Or.inl a2_step
  | 2 => exact Or.inl a3_step
  | 3 => exact Or.inl a4_step
  | 4 => exact Or.inl a5_step
  | 5 => exact Or.inl a6_step
  | 6 => exact Or.inl a7_step
  | 7 => exact Or.inl a8_step
  | 8 => exact Or.inl a9_step
  | 9 => exact Or.inl a10_step
  | 10 => exact Or.inl a11_step
  | k+11 => exact Or.inr rfl

theorem atr_ge (m : Nat) (h : 11 ≤ m) : atr m = a11 := by
  match m, h with
  | 11, _ => rfl
  | k+12, _ => rfl

/-- The last state: its network. -/
theorem a11_net (p : Packet) : a11.net p ↔
    (p = .req 1 1 (.create 1 []) ∨ p = .fwd 1 1 (.create 1 []) ∨ p = .load 1 0 0 ∨
     p = .snap 1 0 (wWorld 0) 0 0 ∨
     p = .commit 1 1 0 7 (.create 1 []) 0 (verdict wKernel (wWorld 0) 7 (.create 1 [])) ∨
     p = .done 1 .accepted ∨ p = .committed aEv ∨ p = .reply 1 .accepted ∨
     p = .clientReply 1 .accepted ∨ p = .snap 1 0 (some aT1) 1 0 ∨ p = .frame 0 aEv) := by
  simp only [a11, a10, a9, a8, a7, a6, a5, a4, a3, a2, a1, sendOne, wInit, or_assoc, false_or]

theorem a11_txn1 : a11.txns 1 = some ⟨1, 0, 7, .create 1 [], .finished .accepted⟩ := rfl

theorem a11_txne (r : RequestId) (hr : r ≠ 1) : a11.txns r = none := by
  simp [a11, a10, a9, a8, a7, a6, a5, a4, a3, a2, a1, sendOne, wInit, updTxn, hr]

theorem a11_decided : a11.store.decided 1 = some .accepted := rfl

theorem a11_decided_rest (r : RequestId) (hr : r ≠ 1) : a11.store.decided r = none := by
  simp [a11, a10, a9, a8, a7, a6, a5, a4, a3, a2, a1, sendOne, wInit, hr]

theorem a11_hist : a11.store.hist 0 (a11.store.rev 0) = some aT1 := rfl

theorem a11_rev : a11.store.rev 0 = 1 := rfl

theorem a11_attempt (r : RequestId) : a11.attempt r = 0 := rfl

theorem a11_done : a11.net (.done 1 .accepted) := (a11_net _).2 (by simp)
theorem a11_reply : a11.net (.reply 1 .accepted) := (a11_net _).2 (by simp)
theorem a11_clientReply : a11.net (.clientReply 1 .accepted) := (a11_net _).2 (by simp)
theorem a11_commit : a11.net (.commit 1 1 0 7 (.create 1 []) 0 (verdict wKernel (wWorld 0) 7 (.create 1 []))) :=
  (a11_net _).2 (by simp)
theorem a11_fwd : a11.net (.fwd 1 1 (.create 1 [])) := (a11_net _).2 (by simp)
theorem a11_load : a11.net (.load 1 0 0) := (a11_net _).2 (by simp)
theorem a11_snap : a11.net (.snap 1 0 (some aT1) 1 0) := (a11_net _).2 (by simp)
theorem a11_frame : a11.net (.frame 0 aEv) := (a11_net _).2 (by simp)

/-- Each enabled instance of each host action is a step that leaves the last state unchanged. -/
theorem fwd_self (rid : RequestId) (tok : Token) (i : Intent)
    (hen : enabled (actFwd rid tok i) a11) : actFwd rid tok i a11 a11 := by
  obtain ⟨_, h1, _⟩ := hen
  have hh : rid = 1 ∧ tok = 1 ∧ i = .create 1 [] := by simpa using (a11_net _).1 h1
  obtain ⟨rfl, rfl, rfl⟩ := hh
  exact ⟨h1, (sendOne_self a11 _ a11_fwd).symm⟩

theorem begin_none (rid : RequestId) (tok : Token) (i : Intent) (C : Config)
    (hen : enabled (actBegin C rid tok i) a11) : False := by
  obtain ⟨_, t, p, _, h1, hnone, _, _⟩ := hen
  have hh : rid = 1 ∧ tok = 1 ∧ i = .create 1 [] := by simpa using (a11_net _).1 h1
  obtain ⟨rfl, _, _⟩ := hh
  rw [a11_txn1] at hnone
  cases hnone

theorem relay_self (rid : RequestId) (o : Outcome) (hen : enabled (actRelay rid o) a11) :
    actRelay rid o a11 a11 := by
  obtain ⟨_, h1, _⟩ := hen
  have hh : rid = 1 ∧ o = .accepted := by simpa using (a11_net _).1 h1
  obtain ⟨rfl, rfl⟩ := hh
  exact ⟨a11_reply, (sendOne_self a11 _ a11_clientReply).symm⟩

theorem done_self (rid : RequestId) (o : Outcome) (hen : enabled (actDone rid o) a11) :
    actDone rid o a11 a11 := by
  obtain ⟨_, x, hx, hdn, _⟩ := hen
  have hh : rid = 1 ∧ o = .accepted := by simpa using (a11_net _).1 hdn
  obtain ⟨rfl, rfl⟩ := hh
  have hx' : x = ⟨1, 0, 7, .create 1 [], .finished .accepted⟩ :=
    Option.some.inj (hx.symm.trans a11_txn1)
  subst hx'
  refine ⟨_, a11_txn1, a11_done, ?_⟩
  rw [sendOne_self a11 _ a11_reply]
  exact (congrArg (fun f => { a11 with txns := f }) (updTxn_same _ 1 _ a11_txn1)).symm

theorem load_self (rid : RequestId) (t : TenantId) (a : Nat) (hen : enabled (actLoad rid t a) a11) :
    actLoad rid t a a11 a11 := by
  obtain ⟨_, h1, _⟩ := hen
  have hh : rid = 1 ∧ t = 0 ∧ a = 0 := by simpa using (a11_net _).1 h1
  obtain ⟨rfl, rfl, rfl⟩ := hh
  exact ⟨a11_load, (sendOne_self a11 _ a11_snap).symm⟩

theorem commit_none (K : Kernel) (rid : RequestId) (hen : enabled (actCommit K rid) a11) : False := by
  obtain ⟨_, x, v, b, hx, hph, _⟩ := hen
  by_cases hr : rid = 1
  · subst hr
    rw [a11_txn1] at hx
    cases hx
    cases hph
  · rw [a11_txne rid hr] at hx
    cases hx

theorem conflict_none (rid : RequestId) (hen : enabled (actConflict rid) a11) : False := by
  obtain ⟨_, x, v, b, hx, hph, _⟩ := hen
  by_cases hr : rid = 1
  · subst hr
    rw [a11_txn1] at hx
    cases hx
    cases hph
  · rw [a11_txne rid hr] at hx
    cases hx

theorem snap_none (rid : RequestId) (t : TenantId) (v : Option Tenant) (b a : Nat)
    (hen : enabled (actSnap rid t v b a) a11) : False := by
  obtain ⟨_, x, hx, _, hph, _⟩ := hen
  by_cases hr : rid = 1
  · subst hr
    rw [a11_txn1] at hx
    cases hx
    cases hph
  · rw [a11_txne rid hr] at hx
    cases hx

theorem a11_committed : a11.net (.committed aEv) := (a11_net _).2 (by simp)

theorem publish_self (sid : SubId) (e : Event) (hen : enabled (actPublish wKernel aConfig sid e) a11) :
    actPublish wKernel aConfig sid e a11 a11 := by
  obtain ⟨_, hcom, ⟨t, p, hsub, htn, hr⟩, _⟩ := hen
  have he : e = aEv := by simpa using (a11_net _).1 hcom
  have hs0 : sid = 0 := Classical.byContradiction fun hne => by
    simp [aConfig, hne] at hsub
  subst he
  subst hs0
  exact ⟨a11_committed, ⟨0, 7, rfl, rfl, aT1_can⟩, (sendOne_self a11 _ a11_frame).symm⟩

theorem store_self (rid : RequestId) (tok : Token) (t : TenantId) (p : PrincipalId) (i : Intent)
    (base : Nat) (res : Except Err Tenant)
    (hen : enabled (StoreStep wKernel aConfig rid tok t p i base res) a11) :
    StoreStep wKernel aConfig rid tok t p i base res a11 a11 := by
  obtain ⟨_, hs⟩ := hen
  have hc : a11.net (.commit rid tok t p i base res) := storeStep_net hs
  have hh : rid = 1 ∧ tok = 1 ∧ t = 0 ∧ p = 7 ∧ i = .create 1 [] ∧ base = 0 ∧
      res = verdict wKernel (wWorld 0) 7 (.create 1 []) := by
    simpa using (a11_net _).1 hc
  obtain ⟨rfl, rfl, rfl, rfl, rfl, rfl, rfl⟩ := hh
  have hs' := StoreStep.decided (K := wKernel) (C := aConfig) a11 .accepted hc a11_decided
  rw [sendOne_self a11 _ a11_done] at hs'
  exact hs'

/-- The accepted run, then stuttering, satisfies the fair spec. Request 1 is accepted, its reply is
    delivered to the client, and the committed event is delivered as a frame to subscriber 0. -/
theorem protoSpec_nonvacuous : ∃ b : Behaviour PState, protoSpec wKernel aConfig wWorld b ∧
    (∃ k, (b k).net (.req 1 1 (.create 1 []))) ∧
    (∃ N, ∀ n rid tok i, (b n).net (.req rid tok i) → rid < N) ∧
    (b 11).net (.clientReply 1 .accepted) ∧ (b 11).net (.frame 0 aEv) := by
  have hS : ∀ m, 11 ≤ m → atr m = a11 := fun m hm => atr_ge m hm
  refine ⟨atr, ?_, ⟨1, Or.inr rfl⟩, ⟨2, ?_⟩, a11_clientReply, a11_frame⟩
  · unfold protoSpec
    refine ⟨wInit_pinit, fun n => ?_, ?_⟩
    · rcases atr_step n with h | h
      · exact Or.inl h
      · exact Or.inr (congrArg id h)
    · exact ⟨fun rid tok i => tail_weak hS (fwd_self rid tok i),
        fun rid tok i => tail_weak hS (fun hen => (begin_none rid tok i aConfig hen).elim),
        fun rid o => tail_weak hS (relay_self rid o),
        fun rid o => tail_weak hS (done_self rid o),
        fun rid t a => tail_weak hS (load_self rid t a),
        fun rid => tail_weak hS (fun hen => (commit_none wKernel rid hen).elim),
        fun rid => tail_weak hS (fun hen => (conflict_none rid hen).elim),
        fun rid tok t p i base res => tail_weak hS (store_self rid tok t p i base res),
        fun rid t v base a => tail_weak hS (fun hen => (snap_none rid t v base a hen).elim),
        fun sid e => tail_weak hS (publish_self sid e)⟩
  · intro n rid tok i hn
    have h9 : a11.net (.req rid tok i) := by
      by_cases hle : n ≤ 11
      · exact net_seq atr_step n 11 hle _ hn
      · rw [hS n (by omega)] at hn
        exact hn
    have key : rid = 1 ∧ tok = 1 ∧ i = .create 1 [] := by simpa using (a11_net _).1 h9
    obtain ⟨k1, -, -⟩ := key
    rw [k1]
    decide

end Strata.System
