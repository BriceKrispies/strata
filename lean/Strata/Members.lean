/-
Membership, teams, invitations and tenant creation (slice 7): the decisions of the kernel's member,
team and invitation intents and of `CreateTenant`, as a total, computable function.

The state is `MTenant`: a tenant of the earlier slices (`base`, whose members hold direct roles), the
roles its schema defines, its teams and its invitations. A user holds its direct roles and the roles of
each team it belongs to (`MTenant.rolesOf`). The decisions read the grants and policies of the tenant
through `base`, and they read effective roles through `rolesOf`. Object types and records are not
touched by these intents; `Tenant` and `step` (`Strata/Model.lean`) are unchanged.

Each refusal is given in the order of the kernel's `transition.rs`: the reserved principal ids first
(`refuse_reserved_principals`), then the tenant (`TenantExists` or `TenantNotFound`), then the intent's
own checks, in order: the permission to manage members, the existence of the team or invitation, the
role (`UnknownRole`, then `public`), the conferring of the roles (`require_conferrable`), and last
the owner rule (`require_an_owner_remains`), which is asked of the state after the change.

Abstractions, all listed in `docs/verification.md`: principal ids are numbers and the kernel's
`team:` prefix and automation ids are one predicate, `reserved`, given to the step; the kernel's
separation of duties, scoped roles, staging roles, agents and the sessions revoked by a removal are not
modelled; a role list is taken in the order given; an invitation's expiry is a number of milliseconds
measured from the context's `now`; the tenant's name is checked for being blank and not stored.
-/

import Strata.Model

namespace Strata

/-- A team, by id (the kernel's `TeamId`). -/
abbrev TeamId := Nat

/-- An invitation, by id (the kernel's `InvitationId`). -/
abbrev InvitationId := Nat

/-- A team: its label, the members it has and the roles those members hold through it. -/
structure TeamDef where
  label : String
  members : List PrincipalId := []
  roles : List RoleId := []
  deriving Repr

/-- An invitation: the normalised address it names, the roles it offers, the user who issued it, the
    time it expires (the issue time plus fourteen days, in milliseconds), and who redeemed it or
    revoked it, if anyone. -/
structure InvitationDef where
  address : String
  roles : List RoleId
  createdBy : PrincipalId
  expiresAt : Nat
  redeemedBy : Option PrincipalId := none
  revokedBy : Option PrincipalId := none
  deriving Repr

/-- A tenant with its membership: the tenant of the earlier slices, the roles its schema defines (the
    built-in `owner` and `public` included), its teams and its invitations. -/
structure MTenant where
  base : Tenant
  roles : List RoleId := [ownerRole, publicRole]
  teams : List (TeamId × TeamDef) := []
  invitations : List (InvitationId × InvitationDef) := []
  deriving Repr

/-- The world of the membership layer: each tenant id has a tenant with its membership or does not. -/
abbrev MWorld := TenantId → Option MTenant

/-- Fourteen days in milliseconds (ADM-1082). -/
def invitationLifetime : Nat := 14 * 24 * 60 * 60 * 1000

/-- The direct roles of a user: the roles its tenant's members table gives it (none if absent). -/
def MTenant.directOf (m : MTenant) (p : PrincipalId) : List RoleId :=
  (m.base.members.lookup p).getD []

/-- The roles a user holds through the teams it belongs to. -/
def MTenant.teamRoles (m : MTenant) (p : PrincipalId) : List RoleId :=
  (m.teams.filter (fun e => e.2.members.contains p)).flatMap (fun e => e.2.roles)

/-- The effective roles of a principal (the kernel's `roles_of`): none for the system, `public` for an
    anonymous caller, and for a user `public`, its direct roles and the roles of its teams. -/
def MTenant.rolesOf (m : MTenant) : Principal → List RoleId
  | .system => []
  | .anonymous => [publicRole]
  | .user p => publicRole :: (m.directOf p ++ m.teamRoles p)

/-- The tenant's members table with user `p` given the roles `rs`: a user left with no roles is removed. -/
def setDirect (m : MTenant) (p : PrincipalId) (rs : List RoleId) : MTenant :=
  { m with base := { m.base with members :=
      if rs.isEmpty then m.base.members.filter (fun e => e.1 != p)
      else (p, rs) :: m.base.members.filter (fun e => e.1 != p) } }

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
  m.base.policies.any (fun pol => (m.rolesOf p).contains pol.role && pol.actions.contains .manage &&
    pol.resource == .members && condHolds (subjectEnv p none none) pol.condition)

/-- The principal holds a grant (the kernel's `missing_grants` is empty for it): the owner holds every
    grant, and any other holds it through a role it holds. -/
def heldGrant (m : MTenant) (p : Principal) (g : Grant) : Bool :=
  (m.rolesOf p).contains ownerRole ||
  m.base.grants.any (fun e => (m.rolesOf p).contains e.1 && e.2 == g)

/-- The principal holds the power of a policy: an action on a resource under a condition (the kernel's
    `holds_policy_power`). The owner holds every power; otherwise a role permission on the object type
    gives an action on it (never a manage action), or a policy of a role it holds names the action,
    reaches the resource, and is unconditional or carries the very same condition. -/
def powerHeld (m : MTenant) (p : Principal) (a : Action) (r : Resource) (cond : Option Expr) : Bool :=
  let roles := m.rolesOf p
  roles.contains ownerRole ||
  (match r with
   | .object ty => a != .manage && m.base.grants.any (fun g => roles.contains g.1 && g.2 == .object ty a)
   | _ => false) ||
  m.base.policies.any (fun pol => roles.contains pol.role && pol.actions.contains a &&
    pol.resource.covers r && (pol.condition.isNone || pol.condition == cond))

/-- Whether a principal may confer a role (the kernel's `may_confer`): only an owner confers `owner`; a
    role that grants nothing is free; otherwise the principal must hold every grant and every policy
    power the role has. -/
def mayConfer (m : MTenant) (p : Principal) (role : RoleId) : Bool :=
  if role == ownerRole then (m.rolesOf p).contains ownerRole
  else
    let grants := (m.base.grants.filter (fun g => g.1 == role)).map Prod.snd
    let powers := (m.base.policies.filter (fun pol => pol.role == role)).flatMap
      (fun pol => pol.actions.map (fun a => (a, pol.resource, pol.condition)))
    if grants.isEmpty && powers.isEmpty then true
    else grants.all (heldGrant m p) && powers.all (fun q => powerHeld m p q.1 q.2.1 q.2.2)

/-- The users that could hold the owner role: the members, and the members of each team. -/
def MTenant.ownerCandidates (m : MTenant) : List PrincipalId :=
  m.base.members.map Prod.fst ++ m.teams.flatMap (fun e => e.2.members)

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

/-- The membership refusals, named by the kernel's error kinds. -/
inductive MErr where
  | unauthorized
  | invalidIntent
  | unknownRole
  | lastOwner
  | tenantExists
  | noTenant
  deriving DecidableEq, Repr

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
  /-- Issue an invitation to an address for roles, expiring fourteen days after the context's time. -/
  | createInvitation (inv : InvitationId) (address : String) (roles : List RoleId)
  /-- Revoke an invitation that was neither redeemed nor revoked. -/
  | revokeInvitation (inv : InvitationId)
  /-- Redeem an invitation with the verified address of the caller. -/
  | redeemInvitation (inv : InvitationId)
  deriving Repr

/-- An intent of the membership layer: create a tenant (the system only), or an intent in a tenant. -/
inductive MIntent where
  | createTenant (owner : PrincipalId) (name : String)
  | member (i : MemberIntent)
  deriving Repr

/-- The explicit environment of an intent: the tenant it addresses, who acts, the time, and the verified
    address of the caller, if it has one. -/
structure MCtx where
  tenant : TenantId
  principal : Principal
  now : Nat
  verified : Option String := none
  deriving Repr

/-- The first role of the list that the tenant does not define (`UnknownRole`), or that is `public`
    (`InvalidIntent`), in the order given. -/
def roleProblem (m : MTenant) : List RoleId → Option MErr
  | [] => none
  | r :: rs =>
    if !m.roles.contains r then some .unknownRole
    else if r == publicRole then some .invalidIntent
    else roleProblem m rs

/-- The refusal for a caller or a named user whose id is reserved (the kernel's
    `refuse_reserved_principals`): a caller who is a user with a reserved id, or a named principal of
    the intent that is reserved. -/
def callerReserved (reserved : PrincipalId → Bool) (c : MCtx) : Bool :=
  match c.principal with
  | .user p => reserved p
  | _ => false

def reservedRefused (reserved : PrincipalId → Bool) (c : MCtx) : MIntent → Bool
  | .createTenant owner _ => reserved owner || callerReserved reserved c
  | .member (.assignRole p _) => reserved p || callerReserved reserved c
  | .member (.addTeamMember _ p) => reserved p || callerReserved reserved c
  | _ => callerReserved reserved c

/-- `AssignRole` and `RevokeRole`: the permission, the role (`UnknownRole`, `public`), the conferring of
    it, and for a revoke that the owner remains. -/
def changeRole (m : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId) (grant : Bool) :
    Except MErr MTenant :=
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
def defineTeamStep (m : MTenant) (c : MCtx) (team : TeamId) (label : String) : Except MErr MTenant :=
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
    Except MErr MTenant :=
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
    Except MErr MTenant :=
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
    (roles : List RoleId) : Except MErr MTenant :=
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
def revokeInvitationStep (m : MTenant) (c : MCtx) (inv : InvitationId) : Except MErr MTenant :=
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
def redeemInvitationStep (m : MTenant) (c : MCtx) (inv : InvitationId) : Except MErr MTenant :=
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
def stepMember (m : MTenant) (c : MCtx) : MemberIntent → Except MErr MTenant
  | .assignRole p r => changeRole m c p r true
  | .revokeRole p r => changeRole m c p r false
  | .defineTeam t l => defineTeamStep m c t l
  | .assignTeamRole t r => teamRoleStep m c t r true
  | .revokeTeamRole t r => teamRoleStep m c t r false
  | .addTeamMember t p => teamMemberStep m c t p true
  | .removeTeamMember t p => teamMemberStep m c t p false
  | .createInvitation i a rs => createInvitationStep m c i a rs
  | .revokeInvitation i => revokeInvitationStep m c i
  | .redeemInvitation i => redeemInvitationStep m c i

/-- The tenant a system `CreateTenant` makes: no object types, the owner holding `owner`, the
    built-in roles, no teams and no invitations. Its record counter starts at 1. -/
def genesisTenant (owner : PrincipalId) : MTenant :=
  { base := { types := [], members := [(owner, [ownerRole])], grants := [], policies := [], nextId := 1 } }

/-- The transition of the membership layer: one intent, in one context, against the world. The reserved
    principal ids are refused first; then a create is refused when the tenant exists, and otherwise made
    by the system only, with a name that is not blank; any other intent needs the tenant to exist. -/
def stepWorld (reserved : PrincipalId → Bool) (w : MWorld) (c : MCtx) (i : MIntent) :
    Except MErr MWorld :=
  if reservedRefused reserved c i then .error .invalidIntent
  else match i with
  | .createTenant owner name =>
    match w c.tenant with
    | some _ => .error .tenantExists
    | none =>
      if c.principal != .system then .error .unauthorized
      else if isBlank name then .error .invalidIntent
      else .ok (fun x => if x = c.tenant then some (genesisTenant owner) else w x)
  | .member j =>
    match w c.tenant with
    | none => .error .noTenant
    | some m =>
      match stepMember m c j with
      | .error e => .error e
      | .ok m' => .ok (fun x => if x = c.tenant then some m' else w x)

end Strata
