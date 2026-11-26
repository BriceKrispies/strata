/-
Roles and policies as data (slice 6): what the decision says, and what the fixture's roles do.

The decision is `Tenant.allows` (`Strata/Model.lean`), a Bool, and `Tenant.Permits` says the same as a
proposition: the owner role is held, or a grant of a held role names the action on the object type,
or a policy of a held role names the action on the resource and its condition is `true`. Everything
here is about that: default deny, which principals hold which roles, the system and the anonymous
caller, the permission an accepted step needs, the capabilities, and the fixture's roles.

The statements about the fixture are closed terms on tenant `1` of `Strata/Fixture.lean` and on a
small tenant of two items, so they are decided by evaluation. A general statement about the writer's
rule would need `Nat.repr` to be injective, which the library does not name, so the writer's rule is
checked on concrete records.
-/

import Strata.Proofs
import Strata.Fixture

namespace Strata

/-! ## Who holds which roles -/

/-- The system holds no role (the kernel's `roles_of`). -/
theorem rolesOf_system (t : Tenant) : t.rolesOf .system = [] := rfl

/-- An anonymous caller holds the built-in `public` role and nothing else. -/
theorem rolesOf_anonymous (t : Tenant) : t.rolesOf .anonymous = [publicRole] := rfl

/-- A user holds the built-in `public` role, its direct roles and the roles of the teams it belongs to
    (slice 7 added the teams: this was `publicRole :: (t.members.lookup p).getD []`, which
    `rolesOf_user_no_teams` states for a tenant with no teams). -/
theorem rolesOf_user (t : Tenant) (p : PrincipalId) :
    t.rolesOf (.user p) = publicRole :: ((t.members.lookup p).getD [] ++ t.mt.teamRoles p) := rfl

/-- In a tenant with no teams a user holds the built-in `public` role and its direct roles. -/
theorem rolesOf_user_no_teams (t : Tenant) (p : PrincipalId) (h : t.teams = []) :
    t.rolesOf (.user p) = publicRole :: (t.members.lookup p).getD [] := by
  simp [rolesOf_user, MTenant.teamRoles, Tenant.mt, h]

/-- The anonymous caller does not hold the owner role. -/
theorem anonymous_not_owner (t : Tenant) : ownerRole ∉ t.rolesOf .anonymous := by
  rw [rolesOf_anonymous, List.mem_singleton]
  decide

/-! ## Default deny -/

/-- The system is permitted nothing: it holds no role, so no grant or policy reaches it. -/
theorem system_never_permitted (t : Tenant) (a : Action) (r : Resource) (env : ExprEnv) :
    ¬ t.Permits .system a r env := by
  rintro (h | ⟨_, _, _, g, _, hrole, _⟩ | ⟨pol, _, hrole, _⟩)
  · rw [rolesOf_system] at h
    exact List.not_mem_nil h
  · rw [rolesOf_system] at hrole
    exact List.not_mem_nil hrole
  · rw [rolesOf_system] at hrole
    exact List.not_mem_nil hrole

/-- The anonymous caller is permitted only what a grant or a policy of the public role gives it: the
    owner role is not held, and the one role it holds is `public`. -/
theorem anonymous_permitted_only_public (t : Tenant) (a : Action) (r : Resource) (env : ExprEnv)
    (h : t.Permits .anonymous a r env) :
    (∃ ty, r = .object ty ∧ a ≠ .manage ∧ ∃ g ∈ t.grants, g.1 = publicRole ∧ g.2 = .object ty a) ∨
    (∃ pol ∈ t.policies, pol.role = publicRole ∧ a ∈ pol.actions ∧ pol.resource.covers r = true ∧
      condHolds env pol.condition = true) := by
  rcases h with h | ⟨ty, hr, ha, g, hg, hrole, hg2⟩ | ⟨pol, hp, hrole, ha, hc, hcond⟩
  · exact absurd h (anonymous_not_owner t)
  · left
    refine ⟨ty, hr, ha, g, hg, ?_, hg2⟩
    rw [rolesOf_anonymous, List.mem_singleton] at hrole
    exact hrole
  · right
    refine ⟨pol, hp, ?_, ha, hc, hcond⟩
    rw [rolesOf_anonymous, List.mem_singleton] at hrole
    exact hrole

/-- The owner role permits every action on every resource. -/
theorem owner_permits (t : Tenant) (p : PrincipalId) (a : Action) (r : Resource) (env : ExprEnv)
    (h : ownerRole ∈ t.rolesOf (.user p)) : t.Permits (.user p) a r env :=
  Or.inl h

/-- Default deny for the schema: a principal that no role of it permits may not manage the schema.
    (The step-level forms are `default_deny_create`, `default_deny_update` and `default_deny_delete`
    in `Strata/Proofs.lean`.) -/
theorem default_deny_manage (t : Tenant) (p : Principal)
    (h : ¬ t.Permits p .manage .schema (subjectEnv p none none)) : canManage t p = false :=
  default_deny _ _ _ _ _ h

/-- The system is refused every schema change, as it is refused every record intent. -/
theorem system_refused_schema (t : Tenant) (ty : TypeId) (f : FieldId) (s : Bool) :
    stepTenant t .system (.setRequired ty f s) = .error .unauthorized := by
  have hf : canManage t .system = false :=
    default_deny_manage t .system (system_never_permitted t .manage .schema _)
  simp [stepTenant, hf]

/-- The system is refused an accepted create: no role of it permits one. -/
theorem system_refused_create (t : Tenant) (ty : TypeId) (fs : Fields) (o : ObjType) (r : Record)
    (ho : t.typ? ty = some o) (hN : newRecord o.schema .system fs = .ok r) :
    stepTenant t .system (.create ty fs) = .error .unauthorized :=
  default_deny_create t .system ty fs o r ho hN (system_never_permitted t .create _ _)

/-- The system is refused an update of an existing record. -/
theorem system_refused_update (t : Tenant) (ty : TypeId) (id : RecordId) (fs : Fields)
    (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) :
    stepTenant t .system (.update ty id fs ev) = .error .unauthorized :=
  default_deny_update t .system ty id fs ev o old ho hl (system_never_permitted t .update _ _)

/-- The system is refused a delete of an existing record. -/
theorem system_refused_delete (t : Tenant) (ty : TypeId) (id : RecordId) (ev : Option Nat)
    (o : ObjType) (old : Record) (ho : t.typ? ty = some o) (hl : o.records.lookup id = some old) :
    stepTenant t .system (.delete ty id ev) = .error .unauthorized :=
  default_deny_delete t .system ty id ev o old ho hl (system_never_permitted t .delete _ _)

/-! ## What an accepted step needs -/

/-- An accepted create is permitted: a role the principal holds grants the create, or a policy of
    one of its roles allows it, for the record it writes. -/
theorem create_ok_permitted (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧
      t.Permits p .create (.object ty) (subjectEnv p none (some r)) := by
  cases hN : newRecord o.schema p fs with
  | error e => cases ha : canAttemptCreate t p ty <;> simp [stepTenant, ho, hN, ha] at h
  | ok r =>
    refine ⟨r, rfl, ?_⟩
    refine Classical.byContradiction fun hn => ?_
    rw [default_deny_create t p ty fs o r ho hN hn] at h
    cases h

/-- An accepted update is permitted for the record it writes. -/
theorem update_ok_permitted (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (fs : Fields) (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (h : stepTenant t p (.update ty id fs ev) = .ok t') :
    t.Permits p .update (.object ty) (subjectEnv p (some old) (some (patched old fs))) := by
  refine Classical.byContradiction fun hn => ?_
  rw [default_deny_update t p ty id fs ev o old ho hl hn] at h
  cases h

/-- An accepted delete is permitted for the record it removes. -/
theorem delete_ok_permitted (t t' : Tenant) (p : Principal) (ty : TypeId) (id : RecordId)
    (ev : Option Nat) (o : ObjType) (old : Record) (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (h : stepTenant t p (.delete ty id ev) = .ok t') :
    t.Permits p .delete (.object ty) (subjectEnv p (some old) (some old)) := by
  refine Classical.byContradiction fun hn => ?_
  rw [default_deny_delete t p ty id ev o old ho hl hn] at h
  cases h

/-- An accepted schema change is permitted to manage the schema. -/
theorem setRequired_ok_permitted (t t' : Tenant) (p : Principal) (ty : TypeId) (f : FieldId)
    (s : Bool) (h : stepTenant t p (.setRequired ty f s) = .ok t') :
    t.Permits p .manage .schema (subjectEnv p none none) := by
  refine Classical.byContradiction fun hn => ?_
  have hf := default_deny_manage t p hn
  simp [stepTenant, hf] at h

/-! ## Capabilities -/

/-- The principal holds the capability `c`: the owner role does, and so does a role it holds that
    grants it. -/
def Tenant.HoldsCapability (t : Tenant) (p : Principal) (c : CapabilityId) : Prop :=
  ownerRole ∈ t.rolesOf p ∨ ∃ g ∈ t.grants, g.1 ∈ t.rolesOf p ∧ g.2 = .capability c

/-- The owner role holds every capability. -/
theorem owner_holds_every_capability (t : Tenant) (p : Principal) (c : CapabilityId)
    (h : ownerRole ∈ t.rolesOf p) : t.HoldsCapability p c :=
  Or.inl h

/-- Without the owner role, a capability is held only through a grant of a role the principal holds. -/
theorem capability_held_by_grant (t : Tenant) (p : Principal) (c : CapabilityId)
    (h : t.HoldsCapability p c) (hn : ownerRole ∉ t.rolesOf p) :
    ∃ g ∈ t.grants, g.1 ∈ t.rolesOf p ∧ g.2 = .capability c :=
  h.resolve_left hn

/-! ## The fixture's roles

Tenant `n` of the fixture has the owner `n*100`, the editor `n*100+1` and the writers `n*100+2` and
`n*100+3`. The policies are `fixturePolicies` (`Strata/Fixture.lean`). -/

/-- The owner of the fixture may do every action on every resource, whatever the record. -/
theorem fixture_owner_permits (n : Nat) (a : Action) (r : Resource) (env : ExprEnv) :
    (initialTenant n).Permits (.user (n * 100)) a r env :=
  owner_permits _ _ a r env (by simp [initialTenant, Tenant.rolesOf, MTenant.rolesOf, MTenant.directOf, MTenant.teamRoles, Tenant.mt, ownerRole])

/-- The editor policy of the fixture on items (`editors_all`): no condition. -/
def editorPolicy1 : Policy :=
  { role := editorRole, resource := .object 1, actions := [.read, .create, .update, .delete] }

/-- The editor policy of the fixture on parts (`part_editors_all`): no condition. -/
def editorPolicy2 : Policy :=
  { role := editorRole, resource := .object 2, actions := [.read, .create, .update, .delete] }

/-- An editor of the fixture may read, create, update and delete the records of both object types. -/
theorem fixture_editor_permits (n : Nat) (a : Action) (ty : TypeId) (env : ExprEnv)
    (ha : a = .read ∨ a = .create ∨ a = .update ∨ a = .delete) (hty : ty = 1 ∨ ty = 2) :
    (initialTenant n).Permits (.user (n * 100 + 1)) a (.object ty) env := by
  have hbeq : (n * 100 + 1 == n * 100) = false := by simp
  have hmem : editorRole ∈ (initialTenant n).rolesOf (.user (n * 100 + 1)) := by
    simp [initialTenant, Tenant.rolesOf, MTenant.rolesOf, MTenant.directOf, MTenant.teamRoles, Tenant.mt, List.lookup, hbeq, editorRole]
  have hact : a ∈ [Action.read, .create, .update, .delete] := by
    rcases ha with rfl | rfl | rfl | rfl <;> simp
  rcases hty with rfl | rfl
  · exact Or.inr (Or.inr ⟨editorPolicy1, by simp [initialTenant, fixturePolicies, editorPolicy1],
      hmem, hact, rfl, rfl⟩)
  · exact Or.inr (Or.inr ⟨editorPolicy2, by simp [initialTenant, fixturePolicies, editorPolicy2],
      hmem, hact, rfl, rfl⟩)

/-! ### Tenant `1`, concretely

The principals of tenant `1` are the owner `100`, the editor `101`, the writers `102` and `103`, a
stranger `150`, and the anonymous caller. The decisions below are closed terms, decided by evaluation. -/

/-- A stranger may not create an item: it holds `public`, and the public policy names parts only. -/
theorem fixture_stranger_refused_item :
    stepTenant (initialTenant 1) (.user 150) (.create 1 []) = .error .unauthorized := rfl

/-- The anonymous caller may not create an item: no policy of `public` names items. -/
theorem fixture_anonymous_refused_item :
    stepTenant (initialTenant 1) .anonymous (.create 1 []) = .error .unauthorized := rfl

/-- The anonymous caller may create a part (with a number, as a part must have a link or a number): the public policy whose condition is `isNull(principal)`
    holds for it. -/
theorem fixture_anonymous_creates_part :
    ∃ t', stepTenant (initialTenant 1) .anonymous (.create 2 [(4, .int 1)]) = .ok t' := ⟨_, rfl⟩

/-- A stranger may not create a part: the condition of the public policy is false for a user. -/
theorem fixture_stranger_refused_part :
    stepTenant (initialTenant 1) (.user 150) (.create 2 []) = .error .unauthorized := rfl

/-- The editor may create an item, whatever the record. -/
theorem fixture_editor_creates_item :
    ∃ t', stepTenant (initialTenant 1) (.user 101) (.create 1 [(1, .str "x")]) = .ok t' := ⟨_, rfl⟩

/-- Tenant `1` with one item created by writer `102` (record `1`) and one by writer `103` (record
    `2`), each with field `1` named `a`. -/
def fixtureItems : Tenant :=
  { types := [(1, { schema := itemSchema, constraints := itemConstraints,
                    records := [(1, { owner := some 102, version := 1, fields := [(1, .str "a")] }),
                                (2, { owner := some 103, version := 1, fields := [(1, .str "a")] })] }),
              (2, { schema := partSchema, constraints := partConstraints })],
    members := (initialTenant 1).members, grants := [], policies := fixturePolicies, nextId := 3,
    limits := Limits.kernel }

/-- A writer may update the record it created: its condition `record.$created_by == principal` holds. -/
theorem fixture_writer_updates_own :
    ∃ t', stepTenant fixtureItems (.user 102) (.update 1 1 [(1, .str "b")] none) = .ok t' := ⟨_, rfl⟩

/-- A writer may not update a record another writer created: its condition is false. -/
theorem fixture_writer_refused_other :
    stepTenant fixtureItems (.user 102) (.update 1 2 [(1, .str "b")] none) = .error .unauthorized := rfl

/-- An editor may update a record any writer created. -/
theorem fixture_editor_updates_other :
    ∃ t', stepTenant fixtureItems (.user 101) (.update 1 2 [(1, .str "b")] none) = .ok t' := ⟨_, rfl⟩

/-- A writer that may not update records does not learn whether an id exists: a missing record is
    refused `unauthorized`, since the permission is asked first with no record to test. -/
theorem fixture_writer_missing_record_hidden :
    stepTenant fixtureItems (.user 102) (.update 1 9 [(1, .str "b")] none) = .error .unauthorized := rfl

/-- An editor learns that a missing record is missing. -/
theorem fixture_editor_missing_record :
    stepTenant fixtureItems (.user 101) (.update 1 9 [(1, .str "b")] none) = .error .notFound := rfl

/-- The system is refused a record intent on the fixture, which gives it no role. -/
theorem fixture_system_refused_item :
    stepTenant (initialTenant 1) .system (.create 1 []) = .error .unauthorized := rfl

end Strata
