/-
Membership, teams, invitations and tenant creation (slice 7): the decisions of the kernel's member,
team and invitation intents and of `CreateTenant`, as a total, computable function.

The state is `MTenant` (`Strata/ModelBase.lean`): the part of a tenant these intents see, which is its
members (who hold direct roles), its grants and policies, the roles its schema defines, its teams and
its invitations. A user holds its direct roles and the roles of each team it belongs to
(`MTenant.rolesOf`). The decisions read grants and policies through the tenant and effective roles
through `rolesOf`. Object types and records are not touched by these intents. `Strata/Model.lean` makes
them constructors of `Intent`, and `stepMember` is the decision `stepTenant` takes for them: the
tenant's membership read as an `MTenant`, decided, and written back with `Tenant.withM`.

Each refusal is given in the order of the kernel's `transition.rs`: the reserved principal ids first
(`refuse_reserved_principals`), then the tenant (`TenantExists` or `TenantNotFound`), then the intent's
own checks, in order: the permission to manage members, the existence of the team or invitation, the
role (`UnknownRole`, then `public`), the conferring of the roles (`require_conferrable`), and last
the owner rule (`require_an_owner_remains`), which is asked of the state after the change.

Abstractions, all listed in `docs/verification.md`: principal ids are numbers and the kernel's
`team:` prefix and automation ids are one predicate, `reservedId`; the kernel's
separation of duties, scoped roles, staging roles, agents and the sessions revoked by a removal are not
modelled; a role list is taken in the order given; an invitation's expiry is a number of milliseconds
measured from the `now` the intent carries (the time of the request); the tenant's name is checked for
being blank and not stored.
-/

import Strata.ModelBase

namespace Strata

/-- Fourteen days in milliseconds (ADM-1082). -/
def invitationLifetime : Nat := 14 * 24 * 60 * 60 * 1000

/-- The tenant's members table with user `p` given the roles `rs`: a user left with no roles is removed. -/
def setDirect (m : MTenant) (p : PrincipalId) (rs : List RoleId) : MTenant :=
  { m with members :=
      if rs.isEmpty then m.members.filter (fun e => e.1 != p)
      else (p, rs) :: m.members.filter (fun e => e.1 != p) }

/-- The team table with team `id` given the definition `d`, the first entry for it. -/
def putTeam (m : MTenant) (id : TeamId) (d : TeamDef) : MTenant :=
  { m with teams := (id, d) :: m.teams.filter (fun e => e.1 != id) }

/-- The invitation table with invitation `id` given the definition `d`, the first entry for it. -/
def putInv (m : MTenant) (id : InvitationId) (d : InvitationDef) : MTenant :=
  { m with invitations := (id, d) :: m.invitations.filter (fun e => e.1 != id) }

/-- Whether a principal may manage members (the kernel's `Manage` on `Members` with no record): the
    owner role, or a policy of a role it holds that names `manage` on the members resource and whose
    condition holds. Members are managed by no grant, since a manage action is never granted. -/
def manageMembers (m : MTenant) (p : Principal) : Bool :=
  (m.rolesOf p).contains ownerRole ||
  m.policies.any (fun pol => (m.rolesOf p).contains pol.role && pol.actions.contains .manage &&
    pol.resource == .members && condHolds (subjectEnv p none none) pol.condition)

/-- The principal holds a grant (the kernel's `missing_grants` is empty for it): the owner holds every
    grant, and any other holds it through a role it holds. -/
def heldGrant (m : MTenant) (p : Principal) (g : Grant) : Bool :=
  (m.rolesOf p).contains ownerRole ||
  m.grants.any (fun e => (m.rolesOf p).contains e.1 && e.2 == g)

/-- The principal holds the power of a policy: an action on a resource under a condition (the kernel's
    `holds_policy_power`). The owner holds every power; otherwise a role permission on the object type
    gives an action on it (never a manage action), or a policy of a role it holds names the action,
    reaches the resource, and is unconditional or carries the very same condition. -/
def powerHeld (m : MTenant) (p : Principal) (a : Action) (r : Resource) (cond : Option Expr) : Bool :=
  let roles := m.rolesOf p
  roles.contains ownerRole ||
  (match r with
   | .object ty => a != .manage && m.grants.any (fun g => roles.contains g.1 && g.2 == .object ty a)
   | _ => false) ||
  m.policies.any (fun pol => roles.contains pol.role && pol.actions.contains a &&
    pol.resource.covers r && (pol.condition.isNone || pol.condition == cond))

/-- Whether a principal may confer a role (the kernel's `may_confer`): only an owner confers `owner`; a
    role that grants nothing is free; otherwise the principal must hold every grant and every policy
    power the role has. -/
def mayConfer (m : MTenant) (p : Principal) (role : RoleId) : Bool :=
  if role == ownerRole then (m.rolesOf p).contains ownerRole
  else
    let grants := (m.grants.filter (fun g => g.1 == role)).map Prod.snd
    let powers := (m.policies.filter (fun pol => pol.role == role)).flatMap
      (fun pol => pol.actions.map (fun a => (a, pol.resource, pol.condition)))
    if grants.isEmpty && powers.isEmpty then true
    else grants.all (heldGrant m p) && powers.all (fun q => powerHeld m p q.1 q.2.1 q.2.2)

/-- The users that could hold the owner role: the members, and the members of each team. -/
def MTenant.ownerCandidates (m : MTenant) : List PrincipalId :=
  m.members.map Prod.fst ++ m.teams.flatMap (fun e => e.2.members)

/-- Some user holds the owner role, directly or through a team (the kernel's `effective_owners` is
    non-empty). -/
def MTenant.hasOwner (m : MTenant) : Bool :=
  m.ownerCandidates.any (fun q => (m.rolesOf (.user q)).contains ownerRole)

/-- Whitespace, for trimming and for a blank name: space, tab, line feed and carriage return. (The
    kernel trims Unicode whitespace; the model names the ASCII part, `docs/verification.md`.) -/
def isWs (c : Char) : Bool := c == ' ' || c == '\t' || c == '\n' || c == '\r'

/-- A string with its leading and trailing whitespace removed. -/
def trimWs (s : String) : String :=
  String.mk ((s.toList.dropWhile isWs).reverse.dropWhile isWs).reverse

/-- A string that is empty once trimmed (the kernel's `trim().is_empty()`). -/
def isBlank (s : String) : Bool := s.toList.all isWs

/-- The normalised form of an address: trimmed and lowercased (the kernel's `normalise_address`). -/
def normaliseAddress (s : String) : String := (trimWs s).toLower

/-- An intent of the membership layer, against a tenant that exists. -/
inductive MemberIntent where
  /-- Give a user a role directly. -/
  | assignRole (p : PrincipalId) (role : RoleId)
  /-- Take a role away from a user directly. -/
  | revokeRole (p : PrincipalId) (role : RoleId)
  /-- Define a team, or relabel it; a new team has no members and no roles. -/
  | defineTeam (team : TeamId) (label : String)
  /-- Give a team a role, which its members hold through it. -/
  | assignTeamRole (team : TeamId) (role : RoleId)
  /-- Take a role away from a team. -/
  | revokeTeamRole (team : TeamId) (role : RoleId)
  /-- Make a user a member of a team. -/
  | addTeamMember (team : TeamId) (p : PrincipalId)
  /-- Remove a user from a team. -/
  | removeTeamMember (team : TeamId) (p : PrincipalId)
  /-- Issue an invitation to an address for roles, expiring fourteen days after `now`, the time of the
      request (which the intent carries, as the kernel's context does). -/
  | createInvitation (inv : InvitationId) (address : String) (roles : List RoleId) (now : Nat)
  /-- Revoke an invitation that was neither redeemed nor revoked. -/
  | revokeInvitation (inv : InvitationId)
  /-- Redeem an invitation at time `now`, with the verified address of the caller, if it has one (both
      carried by the intent, as the kernel's context carries them). -/
  | redeemInvitation (inv : InvitationId) (now : Nat) (verified : Option String)
  deriving Repr

/-- The explicit environment of a membership decision: who acts, the time, and the verified address of
    the caller, if it has one. -/
structure MCtx where
  principal : Principal
  now : Nat := 0
  verified : Option String := none
  deriving Repr

/-- The environment of an intent's decision: the acting principal, and the time and verified address
    the intent carries (only an invitation's creation and redemption read them). -/
def MemberIntent.ctxOf (p : Principal) : MemberIntent → MCtx
  | .createInvitation _ _ _ n => { principal := p, now := n }
  | .redeemInvitation _ n v => { principal := p, now := n, verified := v }
  | _ => { principal := p }

/-- The user an intent names, if it names one: the user a role is assigned to or a team gains. -/
def MemberIntent.named : MemberIntent → Option PrincipalId
  | .assignRole p _ => some p
  | .addTeamMember _ p => some p
  | _ => none

/-- The principal ids the kernel reserves: the `team:` prefix and the automation ids. The model's ids
    are numbers, so these are the ids from `reservedFrom` up (the harness writes such an id `team:p<n>`).
    A reserved id is refused as the caller, as a tenant's owner and as a member named by an
    assignment or a team addition. -/
def reservedFrom : Nat := 1000000

/-- Whether a principal id is reserved. -/
def reservedId (p : PrincipalId) : Bool := decide (reservedFrom ≤ p)

/-- A caller who is a user with a reserved id. -/
def callerReserved (p : Principal) : Bool :=
  match p with
  | .user q => reservedId q
  | _ => false

/-- The first role of the list that the tenant does not define (`UnknownRole`), or that is `public`
    (`InvalidIntent`), in the order given. -/
def roleProblem (m : MTenant) : List RoleId → Option Err
  | [] => none
  | r :: rs =>
    if !m.roles.contains r then some .unknownRole
    else if r == publicRole then some .invalidIntent
    else roleProblem m rs

/-- `AssignRole` and `RevokeRole`: the permission, the role (`UnknownRole`, `public`), the conferring of
    it, and for a revoke that the owner remains. -/
def changeRole (m : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId) (grant : Bool) :
    Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else if !m.roles.contains role then .error .unknownRole
  else if role == publicRole then .error .invalidIntent
  else if !mayConfer m c.principal role then .error .unauthorized
  else
    let direct := m.directOf target
    let rs := if grant then direct.insert role else direct.erase role
    let m' := setDirect m target rs
    if !grant && (m.hasOwner && !m'.hasOwner) then .error .lastOwner else .ok m'

/-- `DefineTeam`: the permission and a label that is not blank. -/
def defineTeamStep (m : MTenant) (c : MCtx) (team : TeamId) (label : String) : Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else if isBlank label then .error .invalidIntent
  else
    let d : TeamDef := match m.teams.lookup team with
      | some d => { d with label := label }
      | none => { label := label }
    .ok (putTeam m team d)

/-- `AssignTeamRole` and `RevokeTeamRole`: the permission, a known team, the role, the conferring of it,
    and for a revoke that the owner remains. -/
def teamRoleStep (m : MTenant) (c : MCtx) (team : TeamId) (role : RoleId) (grant : Bool) :
    Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else match m.teams.lookup team with
  | none => .error .invalidIntent
  | some d =>
    if !m.roles.contains role then .error .unknownRole
    else if role == publicRole then .error .invalidIntent
    else if !mayConfer m c.principal role then .error .unauthorized
    else
      let rs := if grant then d.roles.insert role else d.roles.erase role
      let m' := putTeam m team { d with roles := rs }
      if !grant && (m.hasOwner && !m'.hasOwner) then .error .lastOwner else .ok m'

/-- `AddTeamMember` and `RemoveTeamMember`: the permission, a known team, the conferring of every role
    of the team, and for a removal that the owner remains. -/
def teamMemberStep (m : MTenant) (c : MCtx) (team : TeamId) (p : PrincipalId) (add : Bool) :
    Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else match m.teams.lookup team with
  | none => .error .invalidIntent
  | some d =>
    if !d.roles.all (mayConfer m c.principal) then .error .unauthorized
    else
      let ms := if add then d.members.insert p else d.members.erase p
      let m' := putTeam m team { d with members := ms }
      if !add && (m.hasOwner && !m'.hasOwner) then .error .lastOwner else .ok m'

/-- `CreateInvitation`: the permission, a person issuing it, an address with an `@` once normalised,
    roles that are not empty, an invitation id not yet used, each role known and not `public`, and the
    conferring of every role. -/
def createInvitationStep (m : MTenant) (c : MCtx) (inv : InvitationId) (address : String)
    (roles : List RoleId) : Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else match c.principal with
  | .user creator =>
    let addr := normaliseAddress address
    if !addr.contains '@' then .error .invalidIntent
    else if roles.isEmpty then .error .invalidIntent
    else if (m.invitations.lookup inv).isSome then .error .invalidIntent
    else match roleProblem m roles with
      | some e => .error e
      | none =>
        if !roles.all (mayConfer m c.principal) then .error .unauthorized
        else
          let d : InvitationDef := { address := addr, roles := roles, createdBy := creator, expiresAt := c.now + invitationLifetime }
          .ok (putInv m inv d)
  | _ => .error .invalidIntent

/-- `RevokeInvitation`: the permission, a person revoking it, a known invitation that is neither redeemed
    nor revoked, and the conferring of every role it offers. Expiry is not asked. -/
def revokeInvitationStep (m : MTenant) (c : MCtx) (inv : InvitationId) : Except Err MTenant :=
  if !manageMembers m c.principal then .error .unauthorized
  else match c.principal with
  | .user actor =>
    match m.invitations.lookup inv with
    | none => .error .invalidIntent
    | some o =>
      if o.redeemedBy.isSome then .error .invalidIntent
      else if o.revokedBy.isSome then .error .invalidIntent
      else if !o.roles.all (mayConfer m c.principal) then .error .unauthorized
      else .ok (putInv m inv { o with revokedBy := some actor })
  | _ => .error .invalidIntent

/-- `RedeemInvitation`: a signed-in person with the verified address the invitation names; the invitation
    is known, not redeemed, not revoked and not expired; the issuer may still manage members and confer
    every role it offers. The redeemer gains the roles, and the owner rule is asked. -/
def redeemInvitationStep (m : MTenant) (c : MCtx) (inv : InvitationId) : Except Err MTenant :=
  match c.principal with
  | .user redeemer =>
    match m.invitations.lookup inv with
    | none => .error .invalidIntent
    | some o =>
      match c.verified with
      | none => .error .unauthorized
      | some va =>
        if normaliseAddress va != o.address then .error .unauthorized
        else if o.redeemedBy.isSome then .error .invalidIntent
        else if o.revokedBy.isSome then .error .invalidIntent
        else if !(c.now < o.expiresAt) then .error .invalidIntent
        else
          let issuer := Principal.user o.createdBy
          if !manageMembers m issuer then .error .unauthorized
          else if !o.roles.all (mayConfer m issuer) then .error .unauthorized
          else
            let rs := o.roles.foldl (fun acc r => acc.insert r) (m.directOf redeemer)
            let m1 := setDirect m redeemer rs
            let m2 := putInv m1 inv { o with redeemedBy := some redeemer }
            if m.hasOwner && !m2.hasOwner then .error .lastOwner else .ok m2
  | _ => .error .unauthorized

/-- The decision of one membership intent in a tenant that exists. -/
def stepMember (m : MTenant) (c : MCtx) : MemberIntent → Except Err MTenant
  | .assignRole p r => changeRole m c p r true
  | .revokeRole p r => changeRole m c p r false
  | .defineTeam t l => defineTeamStep m c t l
  | .assignTeamRole t r => teamRoleStep m c t r true
  | .revokeTeamRole t r => teamRoleStep m c t r false
  | .addTeamMember t p => teamMemberStep m c t p true
  | .removeTeamMember t p => teamMemberStep m c t p false
  | .createInvitation i a rs _ => createInvitationStep m c i a rs
  | .revokeInvitation i => revokeInvitationStep m c i
  | .redeemInvitation i _ _ => redeemInvitationStep m c i

/-- The membership of a tenant a system `CreateTenant` makes: the owner holding `owner`, the built-in
    roles, no grants or policies, no teams and no invitations. -/
def genesisM (owner : PrincipalId) : MTenant :=
  { members := [(owner, [ownerRole])], grants := [], policies := [] }

/-- The tenant a system `CreateTenant` makes: no object types, the genesis membership, a record counter that
    starts at 1, and the revision and event sequence the creation's one event leaves (slice 9). -/
def genesisTenant (owner : PrincipalId) : Tenant :=
  { types := [], members := [(owner, [ownerRole])], grants := [], policies := [], nextId := 1,
    revision := 1, eventSeq := 1 }

/-- The decision of a membership intent in a tenant that exists, as `stepTenant` takes it: the tenant's
    membership is decided on and written back; its object types, records and counter are kept. -/
def stepMemberT (t : Tenant) (p : Principal) (j : MemberIntent) : Except Err Tenant :=
  (stepMember t.mt (j.ctxOf p) j).map t.withM

end Strata
