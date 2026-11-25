/-
The browser kernel's routing and session slice (docs/formal-verification.md, "Browser kernel: gap
analysis", slice 1).

`Allowed cat s m s' es` says which states and effects the kernel may produce from state `s` on message
`m`. It states outcomes: the post-state is given as the change it makes (a request is awaited, the
page refreshes, the person changes), and the effects are listed in order. It covers the messages of
routing (`init` with a bootstrapped session, `navigate`, `popstate`), the answers that decide the view
(`viewAnswer`, `viewFailure`), the session (`sessionAnswer`, `sessionEnded`) and the two session
changes a visitor starts (`submit "login"`, `action "logout"`) with their answers. It has no case for a
message of a later slice; a step the relation does not allow is not a step of the spec.

Requirements this slice states, and proves from the relation:

- a view model the page holds was read for the principal who is signed in now (`held_is_own`);
- a model of a screen that needs a signed-in person is held only while someone is signed in
  (`held_guarded_signed_in`);
- so the page shows a model only to the person it was read for (`page_shows_only_own`), and a visitor
  is shown no guarded screen (`visitor_shows_no_guarded_page`);
- a change of the signed-in person drops the held model (`session_change_drops_model`);
- a request is sent only for a request the state awaits (`requests_are_pending`).

Replies to requests of an earlier generation (made before the person changed), and replies to
superseded view requests, are dropped and change nothing (`dropFailure`, `dropViewAnswer`,
`dropViewFailure`, `dropSessionAnswer`). The relation covers the answers' outcomes and not the
liveness of a reply: a request that never answers stays awaited until its deadline, which the driver
decodes into a `failure`.

Slice 2 adds the cold start (`initCold`, `dropColdView`, `learnSession`). Not yet: the bootstrapped view (`init` takes the session only),
forms beyond the sign-in's validity, the live connection's frames and socket effects, toasts and
focus, the page builders and every message of a later slice. No unfinished proof and no added axiom.
-/

import Strata.Browser.Types

namespace Strata.Browser

/-- The request deadline the kernel gives every request, in milliseconds (`REQUEST_TIMEOUT_MS`). -/
def requestDeadlineMs : Nat := 30000

/-- The effects of asking for `k` under request `id`: the request and its deadline. -/
def fetch (id : Nat) (k : ReqKind) : List Effect :=
  [.http id k, .timer "request" id requestDeadlineMs]

/-- The navigation effects: the address is pushed unless it is the current one and no replace is asked;
the scroll always follows. -/
def navEffects (cur r : Route) (replace : Bool) : List Effect :=
  (if cur = r ∧ replace = false then [] else [.pushUrl r replace]) ++ [.scrollTop]

/-- Drop the request with this id from the pending list. -/
def dropId (ps : List Pending) (id : Nat) : List Pending :=
  ps.filter (fun p => p.id != id)

/-- Drop every view request from the pending list: a new view request supersedes them. -/
def dropViews (ps : List Pending) : List Pending :=
  ps.filter (fun p => p.kind != ReqKind.view)

/-- The request `id` is in flight, of kind `k`, in the current generation. -/
def pendingOf (s : State) (id : Nat) (k : ReqKind) : Prop :=
  ∃ p ∈ s.pending, p.id = id ∧ p.kind = k ∧ p.gen = s.generation

/-- A request with this id is in flight in the current generation, whatever its kind. -/
def liveReply (s : State) (id : Nat) : Prop :=
  ∃ p ∈ s.pending, p.id = id ∧ p.gen = s.generation

/-- The state once a view of `r` is asked for: the address moves, the held model stays (the page
refreshes), and the new view request is the one awaited. -/
def askView (s : State) (r : Route) : State :=
  { s with
    route := r
    nextId := s.nextId + 1
    pending := ⟨s.nextId, .view, s.generation⟩ :: dropViews s.pending
    view := { status := .loading, model := s.view.model, request := some s.nextId } }

/-- The state once the person changes (sign-in, sign-out, session end, or a session reply that names
another person): the generation moves on, every request of the old generation is forgotten, the held
model is dropped, the address is `r`, and a view of `r` is asked for. -/
def switched (s : State) (who : Option Principal) (r : Route) : State :=
  { route := r
    session := who
    generation := s.generation + 1
    pending := [⟨s.nextId, .view, s.generation + 1⟩]
    nextId := s.nextId + 1
    view := { status := .loading, model := none, request := some s.nextId } }

/-- The kernel's transitions for the routing and session slice. -/
inductive Allowed (cat : Catalogue) : State → Msg → State → List Effect → Prop
  /-- The first message of a page: the route, and the session the page was rendered for; a view of
  the route is asked for. -/
  | init {s : State} {r : Route} {who : Option Principal} :
      s.nextId = 1 →
      Allowed cat s (.init r who)
        { route := r, session := who, generation := s.generation
          pending := [⟨s.nextId, .view, s.generation⟩], nextId := s.nextId + 1
          view := { status := .loading, model := none, request := some s.nextId } }
        (fetch s.nextId .view)
  /-- The first message of a page that starts without a bootstrapped session: the session is read and
  the view of the route is asked for, both in flight; the session is not yet known. -/
  | initCold {s : State} {r : Route} :
      s.nextId = 1 →
      Allowed cat s (.initCold r)
        { route := r, session := none, generation := s.generation
          pending := [⟨s.nextId, .session, s.generation⟩, ⟨s.nextId + 1, .view, s.generation⟩]
          nextId := s.nextId + 2, sessionKnown := false
          view := { status := .loading, model := none, request := some (s.nextId + 1) } }
        (fetch s.nextId .session ++ fetch (s.nextId + 1) .view)
  /-- A view answer that arrives while the session is not known is not held: the person it was read
  for is not known to be the person signed in, so the view is dropped and read again once the session
  is known. -/
  | dropColdView {s : State} {id : Nat} {who : Option Principal} {scr : Screen} :
      s.sessionKnown = false →
      s.view.request = some id →
      Allowed cat s (.viewAnswer id who scr)
        { s with
          pending := dropId s.pending id
          view := { s.view with request := none } }
        []
  /-- The session read answers while the session is not known: the session is now known, and the view
  of the route is asked for again (the one dropped while the session was not known is not kept). -/
  | learnSession {s : State} {id : Nat} {who : Option Principal} :
      s.sessionKnown = false →
      pendingOf s id .session →
      Allowed cat s (.sessionAnswer id who)
        { route := s.route, session := who, generation := s.generation
          pending := [⟨s.nextId, .view, s.generation⟩], nextId := s.nextId + 1
          view := { status := .loading, model := none, request := some s.nextId } }
        (fetch s.nextId .view)
  /-- A navigation: the address moves; the page refreshes (the held model stays until the new view
  arrives). A visitor may ask for a guarded address: the gateway answers it, and the page shows that. -/
  | navigate {s : State} {r : Route} {replace : Bool} :
      Allowed cat s (.navigate r replace) (askView s r)
        (navEffects s.route r replace ++ fetch s.nextId .view)
  /-- Back or forward: the address moves; no push, no scroll. -/
  | popstate {s : State} {r : Route} :
      Allowed cat s (.popstate r) (askView s r) (fetch s.nextId .view)
  /-- The view that is awaited arrives, for the person it was read for. It is held, with the principal
  it answers for, which must be the person signed in now, and a screen that needs a person is held only
  when one is signed in. -/
  | viewAnswer {s : State} {id : Nat} {who : Option Principal} {scr : Screen} :
      s.view.request = some id →
      s.sessionKnown = true →
      who = s.session →
      (cat.needsSession scr = true → who.isSome = true) →
      Allowed cat s (.viewAnswer id who scr)
        { s with
          pending := dropId s.pending id
          view := { status := .ready, model := some ⟨scr, who⟩, request := none } }
        []
  /-- Reload is pressed: the view of the current address is asked for again; no push, no scroll. -/
  | reload {s : State} {arg : String} :
      Allowed cat s (.action "reload" arg) (askView s s.route) (fetch s.nextId .view)
  /-- A view that is no longer awaited (superseded by a later navigation, or dropped by a change of
  person) changes nothing. -/
  | dropViewAnswer {s : State} {id : Nat} {who : Option Principal} {scr : Screen} :
      s.view.request ≠ some id →
      Allowed cat s (.viewAnswer id who scr) s []
  /-- The awaited view was refused or failed: the page shows the refusal, and holds no model. -/
  | viewFailure {s : State} {id : Nat} {missing : Bool} :
      s.view.request = some id →
      Allowed cat s (.viewFailure id missing)
        { s with
          pending := dropId s.pending id
          view := { status := if missing then ViewStatus.missing else ViewStatus.failed
                    model := none, request := none } }
        []
  /-- A view that is no longer awaited, refused late, changes nothing. -/
  | dropViewFailure {s : State} {id : Nat} {missing : Bool} :
      s.view.request ≠ some id →
      Allowed cat s (.viewFailure id missing) s []
  /-- A request other than the view was refused or failed (or passed its deadline): it is no longer
  in flight. Its own screen's consequence, if any, is a later slice's. -/
  | failure {s : State} {id : Nat} :
      (∃ p ∈ s.pending, p.id = id ∧ p.kind ≠ ReqKind.view ∧ p.gen = s.generation) →
      Allowed cat s (.failure id) { s with pending := dropId s.pending id } []
  /-- A reply to a request that is not in flight in this generation changes nothing. -/
  | dropFailure {s : State} {id : Nat} :
      ¬ liveReply s id →
      Allowed cat s (.failure id) s []
  /-- The session read answers the same person: nothing changes but the request. -/
  | sessionSame {s : State} {id : Nat} {who : Option Principal} :
      s.sessionKnown = true →
      pendingOf s id .session →
      who = s.session →
      Allowed cat s (.sessionAnswer id who) { s with pending := dropId s.pending id } []
  /-- The session read names another person (or none): that is a change of person. -/
  | sessionChanged {s : State} {id : Nat} {who : Option Principal} :
      s.sessionKnown = true →
      pendingOf s id .session →
      who ≠ s.session →
      Allowed cat s (.sessionAnswer id who) (switched s who s.route) (fetch s.nextId .view)
  /-- A session read that is no longer in flight changes nothing. -/
  | dropSessionAnswer {s : State} {id : Nat} {who : Option Principal} :
      ¬ liveReply s id →
      Allowed cat s (.sessionAnswer id who) s []
  /-- A complete sign-in form is sent. Validity is the catalogue's; the form's own messages are a later
  slice. -/
  | signInBegins {s : State} {values : List (String × String)} :
      cat.validSignIn values = true →
      Allowed cat s (.submit "login" values)
        { s with
          nextId := s.nextId + 1
          pending := ⟨s.nextId, .signIn, s.generation⟩ :: s.pending }
        (fetch s.nextId .signIn)
  /-- A sign-in is accepted: the person signed in, and the page goes where the reply names. -/
  | signInAnswer {s : State} {id : Nat} {who : Principal} {target : Route} :
      pendingOf s id .signIn →
      Allowed cat s (.signInAnswer id who target) (switched s (some who) target)
        (navEffects s.route target false ++ fetch s.nextId .view)
  /-- Sign-out is pressed on a screen that offers it: the request is sent. -/
  | signOutBegins {s : State} {arg : String} :
      cat.signOutOn s.route = true →
      Allowed cat s (.action "logout" arg)
        { s with
          nextId := s.nextId + 1
          pending := ⟨s.nextId, .signOut, s.generation⟩ :: s.pending }
        (fetch s.nextId .signOut)
  /-- Sign-out is accepted: no person is signed in; the page goes where the reply names, replacing
  the address. -/
  | signOutAnswer {s : State} {id : Nat} {target : Route} :
      pendingOf s id .signOut →
      Allowed cat s (.signOutAnswer id target) (switched s none target)
        (navEffects s.route target true ++ fetch s.nextId .view)
  /-- The session ends (expiry, or a sign-out elsewhere) while a person is signed in and no sign-out of
  ours is in flight: no person is signed in. -/
  | sessionEnded {s : State} :
      s.session.isSome = true →
      ¬ (∃ p ∈ s.pending, p.kind = ReqKind.signOut) →
      Allowed cat s .sessionEnded (switched s none s.route) (fetch s.nextId .view)
  /-- The session ends while our own sign-out is in flight: the frame is ignored; the reply to the
  sign-out says what happened (the kernel's rule). -/
  | sessionEndedWhileSigningOut {s : State} :
      (∃ p ∈ s.pending, p.kind = ReqKind.signOut) →
      Allowed cat s .sessionEnded s []

/-- The states the kernel can reach from the initial state by allowed steps. -/
inductive Reachable (cat : Catalogue) : State → Prop
  | start : Reachable cat
      { route := root, session := none, generation := 0, pending := [], nextId := 1
        view := { status := .loading, model := none, request := none } }
  | step {s s' : State} {m : Msg} {es : List Effect} :
      Reachable cat s → Allowed cat s m s' es → Reachable cat s'

/-- The page shows a held model: the model is held and the page is loading it or has it. -/
def Shows (s : State) (h : Held) : Prop :=
  s.view.model = some h ∧ (s.view.status = ViewStatus.loading ∨ s.view.status = ViewStatus.ready)

/-! ## The theorems -/

/-- **A held view model was read for the person signed in now.** Every view the page holds answers
for the principal the kernel holds as signed in: a change of person drops the model, and a reply is
held only for the person it answers.
-/
theorem held_is_own (cat : Catalogue) {s : State} (hs : Reachable cat s) :
    ∀ h, s.view.model = some h → h.readFor = s.session := by
  induction hs with
  | start => intro h hh; simp at hh
  | step _ ha ih =>
    rename_i s₀ s₁ m es
    cases ha <;> simp_all [askView, switched, dropViews]

/-- **A model of a screen that needs a signed-in person is held only while someone is signed in.**
The gateway's refusal of such a view to a visitor is never a model; a change of person drops the model.
-/
theorem held_guarded_signed_in (cat : Catalogue) {s : State} (hs : Reachable cat s) :
    ∀ h, s.view.model = some h → cat.needsSession h.screen = true → s.session.isSome = true := by
  induction hs with
  | start => intro h hh; simp at hh
  | step _ ha ih =>
    rename_i s₀ s₁ m es
    cases ha <;> simp_all [askView, switched, dropViews]

/-- **The page shows a model only to the person it was read for.**
-/
theorem page_shows_only_own (cat : Catalogue) {s : State} (hs : Reachable cat s)
    (h : Held) (hshow : Shows s h) : h.readFor = s.session :=
  held_is_own cat hs h hshow.1

/-- **A visitor is shown no screen that needs a signed-in person.**
-/
theorem visitor_shows_no_guarded_page (cat : Catalogue) {s : State} (hs : Reachable cat s)
    (h : Held) (hshow : Shows s h) (hvisitor : s.session = none) :
    cat.needsSession h.screen = false := by
  have hg := held_guarded_signed_in cat hs h hshow.1
  cases hn : cat.needsSession h.screen with
  | false => rfl
  | true => simp [hn, hvisitor] at hg

/-- **A change of the person signed in drops the held model.** Whatever step changes the session,
the state it leads to holds no model, so no view read for the former person outlives the change.
-/
theorem session_change_drops_model (cat : Catalogue) {s s' : State} {m : Msg} {es : List Effect}
    (ha : Allowed cat s m s' es) (hch : s.session ≠ s'.session) :
    s'.view.model = none := by
  cases ha <;> simp_all [switched, askView, dropViews]

/-- **A request is sent only for a request the state awaits.** Every request effect of a step names a
request that is in flight in the state it leads to, in the generation it is in.
-/
theorem requests_are_pending (cat : Catalogue) {s s' : State} {m : Msg} {es : List Effect}
    (ha : Allowed cat s m s' es) :
    ∀ id k, Effect.http id k ∈ es →
      ∃ p ∈ s'.pending, p.id = id ∧ p.kind = k ∧ p.gen = s'.generation := by
  intro id k hmem
  cases ha <;> first
    | (simp_all [askView, switched, fetch, navEffects, dropViews]; done)
    | (simp only [fetch, List.mem_append, List.mem_cons, List.mem_nil_iff, or_false, Effect.http.injEq,
        reduceCtorEq] at hmem
       rcases hmem with ⟨rfl, rfl⟩ | ⟨rfl, rfl⟩ <;> simp_all)

/-! ### Cold start (slice 2) -/

/-- **A page holds a model only once its session is known, and then only for the person signed in.**
A page that starts without a bootstrapped session holds nothing until the session read answers. -/
theorem held_only_when_known (cat : Catalogue) {s : State} (hs : Reachable cat s) :
    ∀ h, s.view.model = some h → s.sessionKnown = true ∧ h.readFor = s.session := by
  induction hs with
  | start => intro h hh; simp at hh
  | step _ ha ih =>
    rename_i s₀ s₁ m es
    cases ha <;> simp_all [askView, switched, dropViews]

/-- **A view answer that arrives while the session is not known changes no model.** The cold page does
not hold the view; it keeps what it holds and reads the view again once the session is known. -/
theorem cold_answer_changes_no_model (cat : Catalogue) {s s' : State} {id : Nat} {who : Option Principal}
    {scr : Screen} {es : List Effect} (ha : Allowed cat s (.viewAnswer id who scr) s' es)
    (hk : s.sessionKnown = false) : s'.view.model = s.view.model := by
  cases ha <;> simp_all

/-- **A cold start awaits the session.** The state it leads to does not know the session, and a session
read is in flight. -/
theorem cold_start_awaits_session (cat : Catalogue) {s s' : State} {r : Route} {es : List Effect}
    (ha : Allowed cat s (.initCold r) s' es) :
    s'.sessionKnown = false ∧ ∃ p ∈ s'.pending, p.kind = ReqKind.session := by
  cases ha
  exact ⟨rfl, _, List.mem_cons_self, rfl⟩

/-! ## Non-vacuity -/

/-- A catalogue with one screen, which needs a signed-in person, for the witness below. -/
def sampleCatalogue : Catalogue :=
  { screenOf := fun _ => some 0
    needsSession := fun scr => scr == 0
    signOutOn := fun _ => true
    validSignIn := fun _ => true }

/-- The relation is not empty where the theorems look: a page that starts for a person, reads the
view of its screen and holds it is a reachable state, holding a model read for that person. -/
theorem signed_in_view_reachable :
    ∃ s, Reachable sampleCatalogue s ∧ s.view.model = some ⟨0, some 7⟩ :=
  ⟨_, Reachable.step (Reachable.step Reachable.start (Allowed.init (s := _) (r := root) (who := some 7) rfl))
      (Allowed.viewAnswer (s := _) (id := 1) (who := some 7) (scr := 0) rfl rfl rfl (by simp [sampleCatalogue])),
    rfl⟩

end Strata.Browser
