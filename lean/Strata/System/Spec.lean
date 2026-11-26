/-
The system specification of the backend (IronFleet's top layer, WG-5655).

This is the trusted part of Layer C: a centralised machine that serves one request at a time, as
if the whole backend ran on one thread. A request names a session token; the token names a tenant
and a principal; the kernel's verdict on the intent, applied to the tenant's state at the moment
the request is served, is the reply. An accepted intent becomes an event, and a subscriber may
observe an event only if its principal may read the tenant after the change.

What clients can observe is the reply to each request and the frames sent to subscribers.
`SysState` keeps nothing else, so `SpecRelation` (in `Protocol.lean`) talks only about those.

The kernel enters as a parameter (`Kernel`), so nothing here depends on which kernel slices exist.
-/

import Strata.Model

namespace Strata.System

abbrev RequestId := Nat
abbrev Token := Nat
abbrev SubId := Nat

/-- The per-tenant kernel the system is parametrised by. `step` is one intent by one principal
    against one tenant state; `readable` says whether the principal may read the tenant's
    records in that state. -/
structure Kernel where
  step : Tenant → Principal → Intent → Except Err Tenant
  readable : Tenant → Principal → Prop

/-- The static configuration: which principal each session token belongs to, and which tenant and
    principal each subscriber reads for. -/
structure Config where
  sessions : Token → Option (TenantId × Principal)
  subs : SubId → Option (TenantId × Principal)

/-- A committed change: the tenant, the request that made it, and the tenant's state after it. -/
structure Event where
  tenant : TenantId
  rid : RequestId
  post : Tenant

/-- What a client is told about its request. -/
inductive Outcome where
  | accepted
  | refused (e : Err)
  | noSession
  deriving DecidableEq

/-- The kernel's verdict on an intent against a tenant state. No tenant is an error. -/
def verdict (K : Kernel) (t : Option Tenant) (p : Principal) (i : Intent) : Except Err Tenant :=
  match t with
  | none => .error .noTenant
  | some tn => K.step tn p i

def outcomeOf : Except Err Tenant → Outcome
  | .ok _ => .accepted
  | .error e => .refused e

/-- The state of the system: the tenants, the events committed so far, the reply given to each
    request, and the frames sent to each subscriber. -/
structure SysState where
  world : World
  log : List Event
  replies : RequestId → Option Outcome
  frames : SubId × Event → Prop

def SysInit (w0 : World) (s : SysState) : Prop :=
  s.world = w0 ∧ s.log = [] ∧ (∀ r, s.replies r = none) ∧ ∀ f, ¬ s.frames f

/-- Serve one request: the verdict of the intent on the tenant at its turn becomes the reply, and
    an accepted verdict becomes the tenant's new state and an event. -/
def serveState (K : Kernel) (s : SysState) (rid : RequestId) (t : TenantId) (p : Principal)
    (i : Intent) : SysState :=
  { world := fun x => match verdict K (s.world t) p i with
      | .ok t' => if x = t then some t' else s.world x
      | .error _ => s.world x
    log := match verdict K (s.world t) p i with
      | .ok t' => s.log ++ [Event.mk t rid t']
      | .error _ => s.log
    replies := fun r =>
      if r = rid then some (outcomeOf (verdict K (s.world t) p i)) else s.replies r
    frames := s.frames }

/-- Serving a request never forgets a committed event. -/
theorem serve_log_sub (K : Kernel) (s : SysState) (rid : RequestId) (t : TenantId)
    (p : Principal) (i : Intent) (e : Event) (he : e ∈ s.log) :
    e ∈ (serveState K s rid t p i).log := by
  show e ∈ (match verdict K (s.world t) p i with
    | .ok t' => s.log ++ [Event.mk t rid t']
    | .error _ => s.log)
  split
  · exact List.mem_append_left _ he
  · exact he

inductive SpecNext (K : Kernel) (C : Config) : SysState → SysState → Prop
  /-- A request with a session is served: one reply, the verdict at its turn. -/
  | serve (s : SysState) (rid : RequestId) (tok : Token) (t : TenantId) (p : Principal)
      (i : Intent) (hs : C.sessions tok = some (t, p)) (hr : s.replies rid = none) :
      SpecNext K C s (serveState K s rid t p i)
  /-- A request with no session is answered without touching any tenant. -/
  | noSession (s : SysState) (rid : RequestId) (tok : Token) (hs : C.sessions tok = none)
      (hr : s.replies rid = none) :
      SpecNext K C s { s with replies := fun r => if r = rid then some .noSession else s.replies r }
  /-- A subscriber observes a committed event of its tenant, if its principal may read the
      tenant after the change. -/
  | frame (s : SysState) (sid : SubId) (e : Event) (t : TenantId) (p : Principal)
      (hs : C.subs sid = some (t, p)) (he : e ∈ s.log) (ht : e.tenant = t)
      (hr : K.readable e.post p) :
      SpecNext K C s { s with frames := fun f => s.frames f ∨ f = (sid, e) }

/-- The reflexive, transitive closure of a relation, from a fixed start. -/
inductive Steps {α : Type} (r : α → α → Prop) (a : α) : α → Prop
  | refl : Steps r a a
  | tail {b c : α} : Steps r a b → r b c → Steps r a c

/-- The specification's reachable states. -/
def SpecReachable (K : Kernel) (C : Config) (w0 : World) (s : SysState) : Prop :=
  ∃ s0, SysInit w0 s0 ∧ Steps (SpecNext K C) s0 s

/-- Every observed frame is a frame of a committed event that its subscriber may read. -/
def FramesOk (K : Kernel) (C : Config) (s : SysState) : Prop :=
  ∀ f, s.frames f → f.2 ∈ s.log ∧
    ∃ t p, C.subs f.1 = some (t, p) ∧ f.2.tenant = t ∧ K.readable f.2.post p

theorem frames_ok_step (K : Kernel) (C : Config) (s s' : SysState)
    (h : SpecNext K C s s') (hI : FramesOk K C s) : FramesOk K C s' := by
  cases h with
  | serve rid tok t p i hs hr =>
    intro f hf
    exact ⟨serve_log_sub K s rid t p i f.2 (hI f hf).1, (hI f hf).2⟩
  | noSession rid tok hs hr => exact fun f hf => hI f hf
  | frame sid e t p hs he ht hr =>
    intro f hf
    rcases hf with hf | hf
    · exact hI f hf
    · subst hf
      exact ⟨he, t, p, hs, ht, hr⟩

/-- In every reachable state of the specification, each observed frame is a frame of a committed
    event that its subscriber's principal may read. -/
theorem spec_frames_readable (K : Kernel) (C : Config) (w0 : World) (s : SysState)
    (h : SpecReachable K C w0 s) : FramesOk K C s := by
  obtain ⟨s0, hi, hr⟩ := h
  induction hr with
  | refl => exact fun f hf => absurd hf (hi.2.2.2 f)
  | tail _ hstep ih => exact frames_ok_step K C _ _ hstep ih

/-- Every committed event belongs to a request that has been answered. -/
def LogAnswered (s : SysState) : Prop := ∀ e ∈ s.log, s.replies e.rid ≠ none

/-- A request commits at most one event. -/
def LogUnique (s : SysState) : Prop :=
  ∀ e1 ∈ s.log, ∀ e2 ∈ s.log, e1.rid = e2.rid → e1 = e2

/-- An event in a served state is an old event, or the one this request committed. -/
theorem serve_log_mem (K : Kernel) (s : SysState) (rid : RequestId) (t : TenantId)
    (p : Principal) (i : Intent) (e : Event)
    (he : e ∈ (serveState K s rid t p i).log) :
    e ∈ s.log ∨ ∃ t', verdict K (s.world t) p i = .ok t' ∧ e = Event.mk t rid t' := by
  have he' : e ∈ (match verdict K (s.world t) p i with
      | .ok t' => s.log ++ [Event.mk t rid t']
      | .error _ => s.log) := he
  split at he'
  · rename_i t' hv
    rcases List.mem_append.mp he' with h | h
    · exact Or.inl h
    · simp at h
      exact Or.inr ⟨t', hv, h⟩
  · exact Or.inl he'

theorem spec_answered_unique_step (K : Kernel) (C : Config) (s s' : SysState)
    (h : SpecNext K C s s') (hA : LogAnswered s) (hU : LogUnique s) :
    LogAnswered s' ∧ LogUnique s' := by
  cases h with
  | serve rid tok t p i hs hr =>
    have hrid : ∀ r, r ≠ rid → (serveState K s rid t p i).replies r = s.replies r := by
      intro r hne
      show (if r = rid then _ else s.replies r) = s.replies r
      simp [hne]
    have hreq : (serveState K s rid t p i).replies rid ≠ none := by
      show (if rid = rid then _ else s.replies rid) ≠ none
      simp
    refine ⟨?_, ?_⟩
    · intro e he
      rcases serve_log_mem K s rid t p i e he with h1 | ⟨t', _, rfl⟩
      · by_cases hx : e.rid = rid
        · rw [hx]; exact hreq
        · rw [hrid _ hx]; exact hA e h1
      · exact hreq
    · intro e1 he1 e2 he2 hrr
      rcases serve_log_mem K s rid t p i e1 he1 with h1 | ⟨t1, hv1, rfl⟩ <;>
        rcases serve_log_mem K s rid t p i e2 he2 with h2 | ⟨t2, hv2, rfl⟩
      · exact hU _ h1 _ h2 hrr
      · have hr1 : e1.rid = rid := hrr
        exact (hA e1 h1 (by rw [hr1]; exact hr)).elim
      · have hr2 : e2.rid = rid := hrr.symm
        exact (hA e2 h2 (by rw [hr2]; exact hr)).elim
      · rw [Except.ok.inj (hv1.symm.trans hv2)]
  | noSession rid tok hs hr =>
    refine ⟨?_, hU⟩
    intro e he
    show (if e.rid = rid then some Outcome.noSession else s.replies e.rid) ≠ none
    split
    · simp
    · exact hA e he
  | frame sid e t p hs he ht hr => exact ⟨hA, hU⟩

/-- In every reachable state each committed event answers a request, and a request commits at most
    one event. -/
theorem spec_log_unique (K : Kernel) (C : Config) (w0 : World) (s : SysState)
    (h : SpecReachable K C w0 s) : LogAnswered s ∧ LogUnique s := by
  obtain ⟨s0, hi, hr⟩ := h
  have hlog0 : s0.log = [] := hi.2.1
  have hinit : LogAnswered s0 ∧ LogUnique s0 := by
    refine ⟨fun e he => ?_, fun e1 he1 => ?_⟩
    · rw [hlog0] at he; exact absurd he (List.not_mem_nil)
    · rw [hlog0] at he1; exact absurd he1 (List.not_mem_nil)
  induction hr with
  | refl => exact hinit
  | tail _ hstep ih => exact spec_answered_unique_step K C _ _ hstep ih.1 ih.2

end Strata.System
