/-
The browser kernel's vocabulary: its state, its messages and its effects, stated abstractly
(docs/formal-verification.md, "Browser kernel: gap analysis").

The kernel is `update : State -> Message -> State x Effect list` in web/src/kernel/update.js, with
`view : State -> ViewModel` in view.js; the worker driver (web/src/worker/main.js) executes the
effects. Everything here is an outcome, not an algorithm: a route is a path and a query, a principal
is a number, a request is an identifier the kernel draws, and a payload that belongs to a later slice
is an opaque string. Which state a message leads to, and which effects it asks for, is the relation
`Allowed` in `Strata/Browser/Session.lean` (slice 1). Messages and effects of later slices are declared
here so that the vocabulary is complete; no relation covers them yet, and the gap analysis says which
slice adds each.

Nothing here is trusted to match the JavaScript: the relation is what the implementation is proved
against (slice B2), and where the JavaScript differs from it, the gap analysis says so.
-/

namespace Strata.Browser

/-- A signed-in person, as the kernel names them (the session's principal, opaque here). -/
abbrev Principal := Nat

/-- A screen: the platform's or a tenant's, as the generated route table names it (opaque here). -/
abbrev Screen := Nat

/-- An address as the kernel parses it: a path and its query parameters, in order. -/
structure Route where
  path : String
  query : List (String × String)
  deriving DecidableEq

/-- The address the page starts on when none is given. -/
def root : Route := ⟨"/", []⟩

/-- What the generated tables say about the screens: which screen an address opens, whether a screen
needs a signed-in person (`auth: session`), whether an address shows the sign-out action, and whether
a sign-in form is complete. The last two are the form and action facts slice 1 reads; the rest of the
generated tables enter in later slices. -/
structure Catalogue where
  screenOf : Route → Option Screen
  needsSession : Screen → Bool
  signOutOn : Route → Bool
  validSignIn : List (String × String) → Bool
  /-- The address a search with these values goes to (the search form's success navigation). -/
  searchTarget : List (String × String) → Route
  /-- The emailed-link request form is complete. -/
  validLink : List (String × String) → Bool
  /-- The invitation form (the password) is complete. -/
  validInvite : List (String × String) → Bool
  /-- The identity provider named by an action is one the site offers. -/
  validProvider : String → Bool
  /-- The second-factor form (a code) is complete. -/
  validSecondFactor : List (String × String) → Bool
  /-- The step-up form (a password or a code) is complete. -/
  validStepUp : List (String × String) → Bool

/-- The kind of a request the kernel has in flight. `session`, `view`, `signIn` and `signOut` are the
kinds slice 1 names. `personal` stands for every request whose reply belongs to the person signed in
when it was made (the kernel's `USER_KINDS` and `USER_KIND_PREFIXES`); `shared` for the rest, whose
reply is the same for everyone (the interface manifest and its parts, the providers list). -/
inductive ReqKind where
  | session
  | view
  | signIn
  | signOut
  | personal
  | shared
  deriving DecidableEq

/-- The status of the page's view: being read, read, not found, or failed. -/
inductive ViewStatus where
  | loading
  | ready
  | missing
  | failed
  deriving DecidableEq

/-- A view model the page holds: the screen it draws, and the principal the reply was read for
(`none` for a visitor). -/
structure Held where
  screen : Screen
  readFor : Option Principal
  deriving DecidableEq

/-- The page's view: its status, the model it holds (kept while a new one is read, as the page refreshes),
and the id of the view request it awaits. -/
structure ViewSlot where
  status : ViewStatus
  model : Option Held
  request : Option Nat

/-- A request in flight: its id, its kind, and the generation it was made in. -/
structure Pending where
  id : Nat
  kind : ReqKind
  gen : Nat

/-- The colour scheme the person chose: follow the device, or light, or dark. -/
inductive ThemeMode where
  | auto
  | light
  | dark
  deriving DecidableEq

/-- The mode the theme button moves to (`auto`, `light`, `dark`, then `auto` again). -/
def ThemeMode.next : ThemeMode → ThemeMode
  | .auto => .light
  | .light => .dark
  | .dark => .auto

/-- The value the choice is stored under (`strata.theme`). -/
def ThemeMode.key : ThemeMode → String
  | .auto => "auto"
  | .light => "light"
  | .dark => "dark"

/-- The live connection's state, as the page holds it: not yet open, opening, open, or waiting to try
again. -/
inductive WsState where
  | idle
  | connecting
  | opened
  | waiting
  deriving DecidableEq

/-- The delay before a reconnect after a close, by the number of closes before it (the machine's
back-off: 0.5, 1, 2, 4 seconds, then 8 seconds). -/
def backoffMs : Nat → Nat
  | 0 => 500
  | 1 => 1000
  | 2 => 2000
  | 3 => 4000
  | _ => 8000

/-- The kernel's state, for the routing and session slice. Later slices add their fields here:
forms, the live connection, the tenant interface, the Lab and the rest of the page state. -/
structure State where
  route : Route
  /-- The signed-in person, or `none` for a visitor. -/
  session : Option Principal
  /-- Counts the clearings of person-scoped state (sign-in, sign-out, session end). -/
  generation : Nat
  pending : List Pending
  /-- The next request id the kernel will draw. -/
  nextId : Nat
  view : ViewSlot
  /-- Whether the session is known. A page that starts cold does not know it until the session read
  answers; a page that starts with a bootstrapped session knows it from the start. -/
  sessionKnown : Bool := true
  /-- The sign-in form's values, as they were submitted or typed. -/
  loginValues : List (String × String) := []
  /-- Whether the sign-in form shows a refusal or an incomplete-form message. -/
  loginError : Bool := false
  /-- The colour scheme chosen, kept across a change of person (it is the browser's, not the person's). -/
  themeMode : ThemeMode := .auto
  /-- The device's colour scheme, as the page last heard it. -/
  systemDark : Bool := false
  /-- Whether the menu is open. -/
  menuOpen : Bool := false
  /-- The ids of the toasts on screen. -/
  toasts : List Nat := []
  /-- The live connection (`liveInit`, `liveStep` in `machines.js`). -/
  wsState : WsState := .idle
  /-- The last notice sequence number acknowledged (0 for none). -/
  wsLast : Nat := 0
  /-- Refused upgrades counted since the socket last opened. -/
  wsRefusals : Nat := 0
  /-- Live updates have given up after repeated refusals. -/
  wsStopped : Bool := false
  /-- The token of the running reconnect timer, if any. -/
  wsReconnect : Option Nat := none
  /-- The token of the running heartbeat timer, if any. -/
  wsHeartbeat : Option Nat := none
  /-- The sequence number of the last notice the connection has taken in (its frame counter). -/
  wsLastN : Nat := 0
  /-- Closes since the last welcome: the back-off of the next reconnect. -/
  wsAttempts : Nat := 0
  /-- Heartbeats sent without a pong since the last pong. -/
  wsMissed : Nat := 0
  /-- The heartbeat interval the welcome announced (10 seconds until the first welcome). -/
  wsHeartbeatMs : Nat := 10000
  /-- The emailed-link request was answered: the page says a link is on its way. -/
  linkSent : Bool := false
  /-- The first factor was right and a second factor is needed: the sign-in waits for the code. -/
  factorPrompt : Bool := false
  /-- A request was refused for want of a fresh authentication: the step-up prompt is shown. -/
  stepUp : Bool := false
  /-- The passkey ceremony in flight: the request id the browser's prompt answers under. -/
  ceremony : Option Nat := none

/-- Effects out of the kernel. `http` and `timer "request"` are the request and its deadline; the
other effects are the page's own (URL, scroll), the live connection, storage, the browser's passkey
prompt, and logs. -/
inductive Effect where
  | http (id : Nat) (kind : ReqKind)
  | timer (name : String) (token : Nat) (ms : Nat)
  | pushUrl (route : Route) (replace : Bool)
  | scrollTop
  | wsConnect (csrf : Option String)
  | wsSend (message : String)
  | wsClose
  | store (key value : String)
  | passkey (op : String) (id : Nat)
  | log (event detail : String)
  | navigateAway (href : String)

/-- Messages into the kernel. Replies of the server are decoded first: `viewAnswer`, `viewFailure`,
`sessionAnswer`, `signInAnswer`, `signOutAnswer` and `failure` are the decoded forms of the driver's
`http_result` (by the kind of the request it answers), and `sessionEnded` is the decoded session frame
of the socket. `timer "request"` is a request past its deadline, which the kernel answers as an
unreachable site (status 0). -/
inductive Msg where
  | init (route : Route) (who : Option Principal)
  | initCold (route : Route)
  | navigate (route : Route) (replace : Bool)
  | popstate (route : Route)
  | input (form name value : String)
  | submit (form : String) (values : List (String × String))
  | action (name arg : String)
  | systemTheme (dark : Bool)
  | viewAnswer (id : Nat) (who : Option Principal) (screen : Screen)
  | viewFailure (id : Nat) (missing : Bool)
  | failure (id : Nat)
  | sessionAnswer (id : Nat) (who : Option Principal)
  | signInAnswer (id : Nat) (who : Principal) (target : Route)
  | signOutAnswer (id : Nat) (target : Route)
  | sessionEnded
  | passkeyResult (id : Nat) (ok : Bool)
  | wsOpen
  | wsFrame (frame : String)
  | wsClosed (refused : Bool)
  | timer (name : String) (token : Nat)
  | dismiss (id : Nat)
  | sharedAnswer (id : Nat)
  /-- The decoded frames of the live connection: the welcome (with the server's head), a notice (its
  sequence number, and whether the page's live bindings read the view again for it), a resync, a pong. -/
  | welcome (head heartbeatMs : Nat)
  | frameEvent (n seq : Nat) (refresh : Bool)
  | resync (head : Nat)
  | pong (head : Nat)
  /-- The page resets the live connection (a sign-in or a sign-out, `ui-live-reset`). -/
  | liveReset
  /-- The answers of the sign-in methods: a link was requested (sent, whether or not the address has an
  account); an invitation was accepted, and the page goes to the reply's address; an identity provider's
  start answered with the address to send the browser to. -/
  | linkAnswer (id : Nat)
  | inviteAccepted (id : Nat) (target : Route)
  | oidcRedirect (id : Nat) (href : String)
  /-- The second factor is asked for: the first factor was right (`second_factor` in the sign-in's reply). -/
  | signInWaiting (id : Nat)
  /-- A request was refused for want of a fresh authentication (`step_up_required`). -/
  | stepUpRequired (id : Nat)
  /-- The step-up was confirmed (the reply to the step-up form). -/
  | stepUpAnswer (id : Nat)
  /-- A factor request (enrol, remove, new recovery codes) was answered. -/
  | factorsAnswer (id : Nat)
  /-- A request for the person's own lists (sessions, passkeys) was answered; `reload` says whether the
  answer changed the list, so that it is read again. -/
  | personalAnswer (id : Nat) (reload : Bool)
  /-- The passkey registration started: its challenge came back, and the browser's prompt is asked for. -/
  | personalChallenge (id : Nat)

/-- The desk's actions that make a request for the person signed in: the revocation of a session or of the
others, and the removal of a passkey (`sessions.js`, `passkeys.js`). -/
def deskRequests : List String :=
  ["sessions.revoke", "sessions.revoke_others", "passkeys.remove"]

/-- The factor actions that make a request for the signed-in person (`factors.*` in `factors.js`). -/
def factorsRequests : List String :=
  ["factors.enrol_start", "factors.remove_totp", "factors.new_codes"]

end Strata.Browser
