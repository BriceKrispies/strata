import Strata.ModelBase

/-!
# Settings (slice 12)

The kernel's typed settings (`crates/kernel/src/settings.rs`: `write_setting`, `resolve_with`). A setting has a
definition, held by the tenant's schema, and any number of overrides, held by the tenant's state. The levels
from the least to the most specific are the platform, the tenant, a team and a person. A definition names the
levels that may hold an override and may be locked, which closes every level more specific than the lock. The
platform's own definitions (the kernel's `platform_definitions`) are known to every tenant.

Resolution (the kernel's `resolve_with`): the person's own override, then the override of the team of the person
whose identifier comes first in ascending byte order, then the tenant's, then the platform's value, then the
default; a level is consulted only where it is open. A write (`writeSettingT`) is decided in the kernel's check
order and sets or removes one override, raising the setting's revision by one.

Abstractions, listed in the slice 12 review note: the platform's values are a parameter of resolution (the
kernel's `resolve` holds none, and the control plane's writes are not modelled); timestamps, declared types and
the reference form of `allowed` are not modelled; an override's `set_by` and `set_at` are not held. A team's
identifier in the model is a number `n`, which the kernel writes as `team<n>`; the order of the teams is the order
of those identifiers as text, which is the order the kernel's team map has.
-/

namespace Strata

/-- The capability a write at the tenant level needs (the kernel's `setting-write-tenant`). The fixture grants
    no capability to a role, so only the owner writes at that level. -/
def capWriteTenant : CapabilityId := 1

/-- The capability a write at the team level needs (the kernel's `setting-write-team`). -/
def capWriteTeam : CapabilityId := 2

/-- The most overrides a tenant holds (the kernel's `MAX_SETTING_OVERRIDES`). -/
def maxSettingOverrides : Nat := 100000

/-- The definition the platform owns (SET-005, WG-5600): every tenant has `member_names`, a text setting that is
    overridden at the tenant level and takes one of three values. -/
def memberNamesDef : SettingDef :=
  { name := "member_names", ty := .str, levels := [.tenant], default := .str "members",
    allowed := some (.closed [.str "members", .str "everyone", .str "nobody"]) }

/-- The definitions the platform holds, shared by every tenant (the kernel's `platform_definitions`). -/
def platformDefs : List SettingDef := [memberNamesDef]

/-- The definition of a setting the tenant has: its own, else the platform's (the kernel's `definition`). -/
def Tenant.settingDef? (t : Tenant) (n : String) : Option SettingDef :=
  match t.settingDefs.find? (fun d => d.name == n) with
  | some d => some d
  | none => platformDefs.find? (fun d => d.name == n)

/-- A principal holds a capability when the owner role does, or a role it holds is granted it (the kernel's
    `holds_any` over the capability grant). -/
def Tenant.holdsCap (t : Tenant) (p : Principal) (c : CapabilityId) : Bool :=
  (t.rolesOf p).contains ownerRole ||
    t.grants.any (fun g => (t.rolesOf p).contains g.1 && decide (g.2 = .capability c))

/-- A level of a definition is open when the definition allows it and no lock closes it: a lock at level `k`
    leaves the levels up to `k` open (the kernel's `is_open`). -/
def SettingDef.isOpen (d : SettingDef) (l : SettingLevel) : Bool :=
  d.levels.contains l &&
    (match d.locked with
      | none => true
      | some k => decide (l.rank ≤ k.rank))

/-- The override of a setting at a subject, if there is one. -/
def Tenant.overrideOf? (t : Tenant) (n : String) (s : SettingSubject) : Option SettingOverride :=
  t.overrides.find? (fun o => o.name == n && decide (o.subject = s))

/-- The revision of a setting: zero until its first change, and never reused (the kernel's `tenant_revision`). -/
def Tenant.settingRev (t : Tenant) (n : String) : Nat :=
  (t.settingRevs.lookup n).getD 0

/-- The key of a team in the order the kernel uses: its identifier `team<n>`, compared as text. -/
def teamKey (tid : TeamId) : String := "team" ++ toString tid

/-- The teams a person belongs to, in the kernel's order (ascending byte order of their identifiers). -/
def Tenant.teamsOf (t : Tenant) (u : PrincipalId) : List TeamId :=
  ((t.teams.filter (fun e => e.2.members.contains u)).map (fun e => e.1)).mergeSort
    (fun a b => decide (teamKey a ≤ teamKey b))

/-- The override of a setting held by the first team of a person, in the kernel's order, that holds one. -/
def Tenant.teamOverride? (t : Tenant) (n : String) (u : PrincipalId) :
    Option (TeamId × SettingOverride) :=
  (t.teamsOf u).findSome? (fun tid => (t.overrideOf? n (.team tid)).map (fun o => (tid, o)))

/-! ## Resolution -/

/-- Where a resolved value came from. -/
inductive SettingSource where
  | person
  | team
  | tenant
  | platform
  | default
  deriving DecidableEq, Repr

/-- A resolved setting: the value, the level it came from (and the team, for a team's value), and the revision
    of the setting at the tenant. -/
structure SettingResolution where
  source : SettingSource
  team : Option TeamId
  value : Val
  tenantRevision : Nat
  deriving DecidableEq, Repr

/-- The platform's value of each setting, as the control plane holds it: `none` when there is none. -/
abbrev PlatformValues := String → Option Val

/-- The value the levels give a principal, the most specific open level first: the person's own override, the
    first team's, the tenant's, then the platform's (a value of the setting's type that no limit refuses). -/
def pickOverride (t : Tenant) (pv : PlatformValues) (d : SettingDef) (p : Principal) :
    Option (SettingSource × Option TeamId × Val) :=
  let personV : Option (SettingSource × Option TeamId × Val) :=
    match p with
    | .user u =>
      if d.isOpen .person then
        (t.overrideOf? d.name (.person u)).map (fun o => (.person, none, o.value))
      else none
    | _ => none
  let teamV : Option (SettingSource × Option TeamId × Val) :=
    match p with
    | .user u =>
      if d.isOpen .team then
        (t.teamOverride? d.name u).map (fun e => (.team, some e.1, e.2.value))
      else none
    | _ => none
  let tenantV : Option (SettingSource × Option TeamId × Val) :=
    if d.isOpen .tenant then
      (t.overrideOf? d.name .tenant).map (fun o => (.tenant, none, o.value))
    else none
  let platformV : Option (SettingSource × Option TeamId × Val) :=
    if d.isOpen .platform then
      match pv d.name with
      | some v => if d.ty.admits v && !Val.overLimit t.limits v then some (.platform, none, v) else none
      | none => none
    else none
  personV <|> teamV <|> tenantV <|> platformV

/-- A setting's resolution for a principal, given the platform's values. -/
def resolveDef (t : Tenant) (pv : PlatformValues) (p : Principal) (d : SettingDef) : SettingResolution :=
  let (src, tid, v) := (pickOverride t pv d p).getD (.default, none, d.default)
  { source := src, team := tid, value := v, tenantRevision := t.settingRev d.name }

/-- The resolution of a setting for a principal (the kernel's `resolve_with`). An undefined setting is
    `unknownObject`. -/
def resolveWith (t : Tenant) (pv : PlatformValues) (p : Principal) (n : String) :
    Except Err SettingResolution :=
  match t.settingDef? n with
  | none => .error .unknownObject
  | some d => .ok (resolveDef t pv p d)

/-- The resolution with no platform value held (the kernel's `resolve`). -/
def resolve (t : Tenant) (p : Principal) (n : String) : Except Err SettingResolution :=
  resolveWith t (fun _ => none) p n

/-! ## Writes -/

/-- The subject a write at a level names: the tenant, the caller, or the team it names. A team that is not the
    tenant's is `invalidIntent`, and so is a team named at another level, or a team level without one. -/
def subjectOf (t : Tenant) (u : PrincipalId) (lvl : SettingLevel) (team : Option TeamId) :
    Except Err SettingSubject :=
  match lvl, team with
  | .tenant, none => .ok .tenant
  | .person, none => .ok (.person u)
  | .team, some tid => if (t.teams.lookup tid).isSome then .ok (.team tid) else .error .invalidIntent
  | _, _ => .error .invalidIntent

/-- The subject an accepted write names, for its event. Where `subjectOf` accepts, this is its subject. -/
def settingSubjectOf (p : Principal) (lvl : SettingLevel) (team : Option TeamId) : SettingSubject :=
  match lvl, team, p with
  | .team, some tid, _ => .team tid
  | .person, _, .user u => .person u
  | _, _, _ => .tenant

/-- A value against what its definition allows beyond its type: a closed set (`invalidIntent` when the value is
    not in it), a range of whole numbers (`limitExceeded` outside it). -/
def allowedOk (allowed : Option SettingAllowed) (v : Val) : Except Err Unit :=
  match allowed with
  | none => .ok ()
  | some (.closed vs) => if vs.contains v then .ok () else .error .invalidIntent
  | some (.range lo hi) =>
    match v with
    | .int n => if lo ≤ n ∧ n ≤ hi then .ok () else .error .limitExceeded
    | _ => .error .limitExceeded

/-- The write of one override (the kernel's `write_setting`), in its check order: the caller is a person; the
    platform level is not written here; the tenant and team levels need their capability; the setting is
    defined; its level is open; the subject is named and exists; a value is not null, of the setting's type,
    within the value limits and allowed; the expected revision is the override's; a removal finds an override;
    a new override is within the tenant's maximum. The result is the tenant with the override set or removed and
    the setting's revision raised; the commit raises the tenant's revision and event sequence. -/
def writeSettingCore (t : Tenant) (p : Principal) (n : String) (lvl : SettingLevel) (team : Option TeamId)
    (v : Option Val) (expected : Nat) : Except Err Tenant :=
  match p with
  | .system => .error .unauthorized
  | .anonymous => .error .unauthorized
  | .user u =>
    if lvl == .platform then .error .invalidIntent
    else if lvl == .tenant && !t.holdsCap p capWriteTenant then .error .unauthorized
    else if lvl == .team && !t.holdsCap p capWriteTeam then .error .unauthorized
    else match t.settingDef? n with
    | none => .error .unknownObject
    | some d =>
      if !d.isOpen lvl then .error .invalidIntent
      else match subjectOf t u lvl team with
      | .error e => .error e
      | .ok subj =>
        let checked : Except Err Unit :=
          match v with
          | none => .ok ()
          | some x =>
            if !d.ty.admits x then .error .invalid
            else if Val.overLimit t.limits x then .error .limitExceeded
            else allowedOk d.allowed x
        match checked with
        | .error e => .error e
        | .ok _ =>
          let cur := t.overrideOf? n subj
          let held := (cur.map (fun o => o.revision)).getD 0
          if expected != held then .error .versionConflict
          else if v.isNone && cur.isNone then .error .invalidIntent
          else if v.isSome && cur.isNone && decide (t.overrides.length ≥ maxSettingOverrides) then
            .error .limitExceeded
          else
            let rev := t.settingRev n + 1
            let others := t.overrides.filter (fun o => !(o.name == n && decide (o.subject = subj)))
            let overrides' :=
              match v with
              | some x => others ++ [{ name := n, subject := subj, value := x, revision := rev }]
              | none => others
            .ok { t with overrides := overrides',
                         settingRevs := (t.settingRevs.filter (fun e => e.1 != n)) ++ [(n, rev)] }

end Strata
namespace Strata

/-- The part of a tenant a write decides on: its membership, its settings and its limits. A write reads no object
    type, counter, revision or sequence, so it decides the same on any tenant with these fields (slice 12). -/
def Tenant.decisionView (t : Tenant) : Tenant :=
  { types := [], members := t.members, grants := t.grants, policies := t.policies, nextId := 0,
    limits := t.limits, roles := t.roles, teams := t.teams, invitations := t.invitations,
    settingDefs := t.settingDefs, overrides := t.overrides, settingRevs := t.settingRevs }

/-- The write of one override on a tenant (the kernel's `write_setting`), decided on the tenant's decision view
    and applied to the tenant: its overrides and the revisions of its settings change, nothing else. -/
def writeSettingT (t : Tenant) (p : Principal) (n : String) (lvl : SettingLevel) (team : Option TeamId)
    (v : Option Val) (expected : Nat) : Except Err Tenant :=
  (writeSettingCore t.decisionView p n lvl team v expected).map
    (fun a => { t with overrides := a.overrides, settingRevs := a.settingRevs })

end Strata

namespace Strata

/-- An accepted write changes only the overrides and the revisions of the setting: every other field of the
    tenant is the one before. -/
theorem writeSettingT_ok_shape {t t' : Tenant} {p : Principal} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {expected : Nat}
    (h : writeSettingT t p n lvl team v expected = .ok t') :
    ∃ ov rv, t' = { t with overrides := ov, settingRevs := rv } := by
  unfold writeSettingT at h
  cases hc : writeSettingCore t.decisionView p n lvl team v expected with
  | error e => simp [hc, Except.map] at h
  | ok a =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    exact ⟨a.overrides, a.settingRevs, h.symm⟩

/-- An accepted write is by a signed-in person, and at the tenant or team level holds the capability that
    level needs: the condition `authorized` gives a write (`Model.lean`). -/
theorem writeSettingCore_ok_auth {t t' : Tenant} {p : Principal} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {expected : Nat}
    (h : writeSettingCore t p n lvl team v expected = .ok t') :
    (p.userId?.isSome && (lvl == .person || (lvl == .tenant && t.holdsCap p capWriteTenant)
      || (lvl == .team && t.holdsCap p capWriteTeam))) = true := by
  cases p with
  | system => simp [writeSettingCore] at h
  | anonymous => simp [writeSettingCore] at h
  | user u =>
    unfold writeSettingCore at h
    cases lvl <;> simp_all [Principal.userId?, subjectOf]
    all_goals repeat' split at h
    all_goals simp_all [Principal.userId?]

/-- An accepted write is by a signed-in person, with the capability its level needs (`authorized`). -/
theorem writeSettingT_ok_auth {t t' : Tenant} {p : Principal} {n : String} {lvl : SettingLevel}
    {team : Option TeamId} {v : Option Val} {expected : Nat}
    (h : writeSettingT t p n lvl team v expected = .ok t') :
    (p.userId?.isSome && (lvl == .person || (lvl == .tenant && t.holdsCap p capWriteTenant)
      || (lvl == .team && t.holdsCap p capWriteTeam))) = true := by
  unfold writeSettingT at h
  cases hc : writeSettingCore t.decisionView p n lvl team v expected with
  | error e => simp [hc, Except.map] at h
  | ok a => exact writeSettingCore_ok_auth (t := t.decisionView) (t' := a) hc

/-- The decision of a write does not depend on the object types, the counter, the revision or the sequence
    (slice 12): on a tenant with other ones it gives the same overrides, and the result differs only in those. -/
theorem writeSettingT_with (t : Tenant) (ts : List (TypeId × ObjType)) (nx rv es : Nat) (p : Principal)
    (n : String) (lvl : SettingLevel) (team : Option TeamId) (v : Option Val) (e : Nat) :
    writeSettingT { t with types := ts, nextId := nx, revision := rv, eventSeq := es } p n lvl team v e =
      (writeSettingT t p n lvl team v e).map
        (fun a => { a with types := ts, nextId := nx, revision := rv, eventSeq := es }) := by
  have hv : ({ t with types := ts, nextId := nx, revision := rv, eventSeq := es } : Tenant).decisionView =
      t.decisionView := rfl
  unfold writeSettingT
  rw [hv]
  cases hc : writeSettingCore t.decisionView p n lvl team v e with
  | error x => rfl
  | ok a => rfl

end Strata
