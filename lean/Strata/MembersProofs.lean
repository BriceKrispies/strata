/-
Theorems of the membership layer (slice 7, `Strata/Members.lean`).

What they say, in the order of the file:
- Effective roles: a user holds `public`, its direct roles, and the roles of its teams, and nothing else.
  The system holds none; an anonymous caller holds `public`.
- The owner rule: in every tenant that has an owner, every accepted step leaves one (given that team ids
  are distinct, which every accepted step keeps), and a tenant that is created has one.
- Conferring: an accepted assignment, team role grant, team membership or invitation redemption gives a
  principal no role that it could not hand out: the conferrer, or for a redemption the issuer, could
  confer it in the state before.
- Invitations: an accepted creation expires fourteen days after its time; an accepted redemption was
  made to the one verified address, before expiry, unredeemed, unrevoked, with the issuer still
  authorised, and afterwards the invitation is redeemed, so no later redemption is accepted; a revoked
  invitation is never redeemed.
- Reserved principal ids are refused before anything else.
- Creating a tenant: only the system, only when the tenant does not exist, with a name that is not blank;
  the tenant it makes has the owner, the built-in roles, no teams and no invitations.

Every statement is about the decisions of `Strata/Members.lean`, with no hypothesis beyond those named.
-/

import Strata.Model
import Strata.SchemaChangeInv

namespace Strata

/-! ## Lookups in the member table and the team table -/

/-- A lookup in a table with the entries of key `p` removed finds nothing under `p`. -/
theorem lookup_filter_self {β : Type} (l : List (PrincipalId × β)) (p : PrincipalId) :
    (l.filter (fun e => e.1 != p)).lookup p = none := by
  induction l with
  | nil => rfl
  | cons e l ih =>
    rcases e with ⟨k, b⟩
    by_cases hk : k = p
    · subst hk
      simp [List.filter_cons, ih]
    · have hkp : (k != p) = true := by simpa using hk
      have hpk : (p == k) = false := by simpa using Ne.symm hk
      simp [List.filter_cons, hkp, List.lookup_cons, hpk, ih]

/-- Removing the entries of key `p` leaves a lookup of any other key unchanged. -/
theorem lookup_filter_ne {β : Type} (l : List (PrincipalId × β)) (p q : PrincipalId) (hq : q ≠ p) :
    (l.filter (fun e => e.1 != p)).lookup q = l.lookup q := by
  induction l with
  | nil => rfl
  | cons e l ih =>
    rcases e with ⟨k, b⟩
    by_cases hk : k = p
    · subst hk
      have hqk : (q == k) = false := by simpa using hq
      simp [List.filter_cons, List.lookup_cons, hqk, ih]
    · have hkp : (k != p) = true := by simpa using hk
      simp [List.filter_cons, hkp, List.lookup_cons, ih]

/-- A key found by a lookup is a key of the table. -/
theorem lookup_some_key {β : Type} (l : List (PrincipalId × β)) (q : PrincipalId) (v : β)
    (h : l.lookup q = some v) : q ∈ l.map Prod.fst := by
  obtain ⟨l₁, l₂, hl, _⟩ := List.lookup_eq_some_iff.mp h
  rw [hl]
  simp

/-- The pair a lookup finds is an entry of the table. -/
theorem mem_of_lookup_some {β : Type} (l : List (PrincipalId × β)) (q : PrincipalId) (v : β)
    (h : l.lookup q = some v) : (q, v) ∈ l := by
  obtain ⟨l₁, l₂, hl, _⟩ := List.lookup_eq_some_iff.mp h
  rw [hl]
  simp

/-- Two entries of a table with distinct keys that have the same key are the same entry. -/
theorem eq_of_key_nodup {β : Type} : ∀ (l : List (TeamId × β)), (l.map Prod.fst).Nodup →
    ∀ a ∈ l, ∀ b ∈ l, a.1 = b.1 → a = b
  | [], _, a, ha, _, _, _ => by simp at ha
  | x :: l, hn, a, ha, b, hb, hk => by
    rw [List.map_cons, List.nodup_cons] at hn
    rcases List.mem_cons.mp ha with ha1 | ha1 <;> rcases List.mem_cons.mp hb with hb1 | hb1
    · subst ha1
      subst hb1
      rfl
    · subst ha1
      exfalso
      apply hn.1
      rw [hk]
      exact List.mem_map_of_mem hb1
    · subst hb1
      exfalso
      apply hn.1
      rw [← hk]
      exact List.mem_map_of_mem ha1
    · exact eq_of_key_nodup l hn.2 a ha1 b hb1 hk

/-! ## Effective roles -/

/-- The roles a user holds through its teams are those of each team it is a member of. -/
theorem mem_teamRoles (m : MTenant) (q : PrincipalId) (r : RoleId) :
    r ∈ m.teamRoles q ↔ ∃ e ∈ m.teams, q ∈ e.2.members ∧ r ∈ e.2.roles := by
  unfold MTenant.teamRoles
  simp only [List.mem_flatMap, List.mem_filter, List.contains_iff_mem, and_assoc]

/-- A user holds `public`, its direct roles and the roles of its teams, and nothing else. -/
theorem effective_roles (m : MTenant) (q : PrincipalId) (r : RoleId) :
    r ∈ m.rolesOf (.user q) ↔
      r = publicRole ∨ r ∈ m.directOf q ∨ ∃ e ∈ m.teams, q ∈ e.2.members ∧ r ∈ e.2.roles := by
  simp only [MTenant.rolesOf, List.mem_cons, List.mem_append, mem_teamRoles]

/-- The system holds no role. -/
theorem system_holds_no_role (m : MTenant) : m.rolesOf .system = [] := rfl

/-- An anonymous caller holds `public` and nothing else. -/
theorem anonymous_holds_public (m : MTenant) : m.rolesOf .anonymous = [publicRole] := rfl

/-- The system may not manage members: it holds no role, so no policy reaches it. -/
theorem system_cannot_manage_members (m : MTenant) : manageMembers m .system = false := by
  unfold manageMembers
  simp [system_holds_no_role]

/-- The direct roles of a user, after its entry in the member table is replaced. -/
theorem directOf_setDirect (m : MTenant) (p q : PrincipalId) (rs : List RoleId) :
    (setDirect m p rs).directOf q = if q = p then rs else m.directOf q := by
  unfold setDirect MTenant.directOf
  by_cases hq : q = p
  · subst hq
    by_cases hr : rs.isEmpty
    · rw [List.isEmpty_iff] at hr
      subst hr
      simp [lookup_filter_self]
    · simp [hr, List.lookup_cons]
  · have hqp : (q == p) = false := by simpa using hq
    by_cases hr : rs.isEmpty <;>
      simp [hr, hq, hqp, lookup_filter_ne _ p q hq, List.lookup_cons]

/-- Giving a user roles that keep every role it held keeps those of every principal. -/
theorem rolesOf_mono_setDirect (m : MTenant) (p : PrincipalId) (rs : List RoleId)
    (h : ∀ r ∈ m.directOf p, r ∈ rs) :
    ∀ q r, r ∈ m.rolesOf (.user q) → r ∈ (setDirect m p rs).rolesOf (.user q) := by
  intro q r hr
  rw [effective_roles] at hr ⊢
  rcases hr with hr | hr | hr
  · exact Or.inl hr
  · right; left
    rw [directOf_setDirect]
    by_cases hq : q = p
    · subst hq
      simp only [if_true]
      exact h r hr
    · simp only [hq, if_false]
      exact hr
  · exact Or.inr (Or.inr hr)

/-! ## The owner rule -/

/-- Some user holds the owner role exactly when the owner role is among the roles of a user. -/
theorem hasOwner_iff (m : MTenant) : m.hasOwner = true ↔ ∃ q, ownerRole ∈ m.rolesOf (.user q) := by
  constructor
  · intro h
    unfold MTenant.hasOwner at h
    obtain ⟨q, _, hq⟩ := List.any_eq_true.mp h
    exact ⟨q, by simpa using hq⟩
  · rintro ⟨q, hq⟩
    unfold MTenant.hasOwner
    apply List.any_eq_true.mpr
    refine ⟨q, ?_, by simpa using hq⟩
    rw [effective_roles] at hq
    unfold MTenant.ownerCandidates
    rcases hq with h | h | ⟨e, he, hm, _⟩
    · simp [ownerRole, publicRole] at h
    · apply List.mem_append_left
      unfold MTenant.directOf at h
      cases hl : m.members.lookup q with
      | none => rw [hl] at h; simp at h
      | some v => exact List.mem_map.mpr ⟨(q, v), mem_of_lookup_some _ q v hl, rfl⟩
    · apply List.mem_append_right
      exact List.mem_flatMap.mpr ⟨e, he, hm⟩

/-- If every role a user holds stays held, a tenant with an owner keeps one. -/
theorem hasOwner_of_mono (m m' : MTenant)
    (h : ∀ q r, r ∈ m.rolesOf (.user q) → r ∈ m'.rolesOf (.user q)) (ho : m.hasOwner = true) :
    m'.hasOwner = true := by
  obtain ⟨q, hq⟩ := (hasOwner_iff m).mp ho
  exact (hasOwner_iff m').mpr ⟨q, h q ownerRole hq⟩

/-- A tenant that is created has an owner. -/
theorem genesis_hasOwner (o : PrincipalId) : (genesisM o).hasOwner = true := by
  rw [hasOwner_iff]
  refine ⟨o, ?_⟩
  simp [effective_roles, MTenant.directOf, genesisM, ownerRole]

/-- The team table after the entry of team `T` is replaced by `d'`: a user's team roles are those of the
    teams it is in, and of `T` as `d'` gives it. -/
theorem mem_teamRoles_putTeam (m : MTenant) (T : TeamId) (d : TeamDef) (q : PrincipalId) (r : RoleId) :
    r ∈ (putTeam m T d).teamRoles q ↔
      (q ∈ d.members ∧ r ∈ d.roles) ∨ ∃ e ∈ m.teams, e.1 ≠ T ∧ q ∈ e.2.members ∧ r ∈ e.2.roles := by
  rw [mem_teamRoles]
  unfold putTeam
  constructor
  · rintro ⟨e, he, hq, hr⟩
    rw [List.mem_cons, List.mem_filter] at he
    rcases he with rfl | ⟨he, hne⟩
    · exact Or.inl ⟨hq, hr⟩
    · exact Or.inr ⟨e, he, by simpa using hne, hq, hr⟩
  · rintro (⟨hq, hr⟩ | ⟨e, he, hne, hq, hr⟩)
    · exact ⟨(T, d), List.mem_cons_self, hq, hr⟩
    · exact ⟨e, List.mem_cons_of_mem _ (List.mem_filter.mpr ⟨he, by simpa using hne⟩), hq, hr⟩

/-- Replacing a team by one with at least its members and at least its roles keeps every team role a user
    held (given the team's entry is in the table, and team ids are distinct). -/
theorem teamRoles_putTeam_mono (m : MTenant) (T : TeamId) (d d' : TeamDef)
    (hn : (m.teams.map Prod.fst).Nodup) (hd : (T, d) ∈ m.teams)
    (hm : ∀ p, p ∈ d.members → p ∈ d'.members) (hr : ∀ x ∈ d.roles, x ∈ d'.roles)
    (q : PrincipalId) (x : RoleId) (hx : x ∈ m.teamRoles q) :
    x ∈ (putTeam m T d').teamRoles q := by
  rw [mem_teamRoles] at hx
  obtain ⟨e, he, hq, hxe⟩ := hx
  rw [mem_teamRoles_putTeam]
  by_cases hk : e.1 = T
  · have hee : e = (T, d) := eq_of_key_nodup m.teams hn e he (T, d) hd (by simpa using hk)
    subst hee
    exact Or.inl ⟨hm q hq, hr x hxe⟩
  · exact Or.inr ⟨e, he, hk, hq, hxe⟩

/-- Team ids are distinct: each team id occurs once in the table. -/
def MTenant.TeamsNodup (m : MTenant) : Prop := (m.teams.map Prod.fst).Nodup

/-- Replacing the entry of a team keeps team ids distinct. -/
theorem putTeam_teamsNodup (m : MTenant) (T : TeamId) (d : TeamDef) (hn : m.TeamsNodup) :
    (putTeam m T d).TeamsNodup := by
  unfold MTenant.TeamsNodup at hn ⊢
  unfold putTeam
  simp only [List.map_cons]
  rw [List.nodup_cons]
  constructor
  · intro hmem
    obtain ⟨e, he, hk⟩ := List.mem_map.mp hmem
    rw [List.mem_filter] at he
    have : e.1 ≠ T := by simpa using he.2
    exact this hk
  · exact List.Sublist.nodup ((List.filter_sublist).map Prod.fst) hn

/-- The entry a lookup finds for a key of a table with distinct keys is the entry given. -/
theorem lookup_of_mem_nodup {β : Type} : ∀ (l : List (TeamId × β)), (l.map Prod.fst).Nodup →
    ∀ e ∈ l, l.lookup e.1 = some e.2
  | [], _, e, he => by simp at he
  | ⟨k, b⟩ :: l, hn, e, he => by
    rw [List.map_cons, List.nodup_cons] at hn
    rcases List.mem_cons.mp he with he1 | he1
    · subst he1
      simp
    · have hne : k ≠ e.1 := fun hk => hn.1 (by
        show k ∈ l.map Prod.fst
        rw [hk]
        exact List.mem_map_of_mem he1)
      have hne' : (e.1 == k) = false := by simpa using Ne.symm hne
      rw [List.lookup_cons, hne']
      exact lookup_of_mem_nodup l hn.2 e he1

/-- Replacing a team by one that keeps the members and roles of each entry for its key keeps every
    team role a user held. -/
theorem putTeam_mono (m : MTenant) (T : TeamId) (d' : TeamDef) (hn : m.TeamsNodup)
    (hT : ∀ e ∈ m.teams, e.1 = T → (∀ p ∈ e.2.members, p ∈ d'.members) ∧ ∀ x ∈ e.2.roles, x ∈ d'.roles) :
    ∀ q x, x ∈ m.rolesOf (.user q) → x ∈ (putTeam m T d').rolesOf (.user q) := by
  intro q x hx
  rw [effective_roles] at hx ⊢
  rcases hx with hx | hx | ⟨e, he, hq, hxe⟩
  · exact Or.inl hx
  · exact Or.inr (Or.inl hx)
  · right; right
    apply (mem_teamRoles _ _ _).mp
    rw [mem_teamRoles_putTeam]
    by_cases hk : e.1 = T
    · exact Or.inl ⟨(hT e he hk).1 q hq, (hT e he hk).2 x hxe⟩
    · exact Or.inr ⟨e, he, hk, hq, hxe⟩

/-- The member table and the invitations do not change the effective roles of anyone. -/
theorem rolesOf_putInv (m : MTenant) (i : InvitationId) (d : InvitationDef) (p : Principal) :
    (putInv m i d).rolesOf p = m.rolesOf p := by
  cases p <;> rfl

/-- The roles of a list folded into a set of roles, each of which is kept. -/
theorem mem_foldl_insert (roles : List RoleId) : ∀ (D : List RoleId) (r : RoleId), r ∈ D →
    r ∈ roles.foldl (fun acc x => acc.insert x) D := by
  induction roles with
  | nil => intro D r hr; exact hr
  | cons x xs ih =>
    intro D r hr
    exact ih (D.insert x) r (List.mem_insert_iff.mpr (Or.inr hr))

/-- A role of the list folded into a set of roles is one of the set or one of the list. -/
theorem mem_of_mem_foldl_insert (roles : List RoleId) : ∀ (D : List RoleId) (r : RoleId),
    r ∈ roles.foldl (fun acc x => acc.insert x) D → r ∈ D ∨ r ∈ roles := by
  induction roles with
  | nil => intro D r hr; exact Or.inl hr
  | cons x xs ih =>
    intro D r hr
    rcases ih (D.insert x) r hr with h | h
    · rcases List.mem_insert_iff.mp h with h1 | h1
      · exact Or.inr (by simp [h1])
      · exact Or.inl h1
    · exact Or.inr (List.mem_cons_of_mem _ h)

/-- The team ids of a member table with an entry replaced stay distinct. -/
theorem stepMember_teamsNodup_of_teams {m m' : MTenant} (hn : m.TeamsNodup) (h : m'.teams = m.teams) :
    m'.TeamsNodup := by
  unfold MTenant.TeamsNodup at hn ⊢
  rw [h]
  exact hn

/-! ## The owner rule, step by step -/

/-- Giving a role directly never removes an owner. -/
theorem directOf_grow_mono (m : MTenant) (target : PrincipalId) (rs : List RoleId)
    (h : ∀ r ∈ m.directOf target, r ∈ rs) :
    ∀ q x, x ∈ m.rolesOf (.user q) → x ∈ (setDirect m target rs).rolesOf (.user q) :=
  rolesOf_mono_setDirect m target rs h

/-- An accepted assignment or revocation of a role directly keeps an owner, when it is a grant, and a
    revocation keeps one only when the tenant keeps one. -/
theorem changeRole_owner (m m' : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId)
    (grant : Bool) (h : changeRole m c target role grant = .ok m') (ho : m.hasOwner = true) :
    m'.hasOwner = true := by
  cases grant
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals try cases h
    all_goals first
      | (simp_all; done)
      | exact hasOwner_of_mono _ _ (directOf_grow_mono _ _ _ (fun r hr => List.mem_insert_iff.mpr (Or.inr hr))) ho
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals try cases h
    all_goals first
      | exact hasOwner_of_mono _ _ (directOf_grow_mono _ _ _ (fun r hr => List.mem_insert_iff.mpr (Or.inr hr))) ho
      | (simp_all; done)


/-- A team entry the lookup finds is the entry given, when team ids are distinct: the team's roles and
    members are those of the entry it replaces. -/
theorem putTeam_mono_some (m : MTenant) (T : TeamId) (d d' : TeamDef) (hn : m.TeamsNodup)
    (hl : m.teams.lookup T = some d) (hm : ∀ p ∈ d.members, p ∈ d'.members)
    (hr : ∀ x ∈ d.roles, x ∈ d'.roles) :
    ∀ q x, x ∈ m.rolesOf (.user q) → x ∈ (putTeam m T d').rolesOf (.user q) := by
  apply putTeam_mono m T d' hn
  intro e he hk
  have h1 := lookup_of_mem_nodup m.teams hn e he
  have h2 : e.2 = d := by
    rw [hk, hl] at h1
    exact (Option.some.inj h1).symm
  rw [h2]
  exact ⟨hm, hr⟩

/-- Adding a team that no entry names keeps every team role a user held. -/
theorem putTeam_mono_none (m : MTenant) (T : TeamId) (d' : TeamDef) (hn : m.TeamsNodup)
    (hl : m.teams.lookup T = none) :
    ∀ q x, x ∈ m.rolesOf (.user q) → x ∈ (putTeam m T d').rolesOf (.user q) := by
  apply putTeam_mono m T d' hn
  intro e he hk
  have h1 := lookup_of_mem_nodup m.teams hn e he
  rw [hk, hl] at h1
  cases h1

/-- Defining a team, or relabelling it, keeps an owner. -/
theorem defineTeam_owner (m m' : MTenant) (c : MCtx) (team : TeamId) (label : String)
    (hn : m.TeamsNodup) (h : defineTeamStep m c team label = .ok m') (ho : m.hasOwner = true) :
    m'.hasOwner = true := by
  unfold defineTeamStep at h
  cases hl : m.teams.lookup team with
  | none =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact hasOwner_of_mono _ _ (putTeam_mono_none m team _ hn hl) ho
  | some d0 =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact hasOwner_of_mono _ _ (putTeam_mono_some m team d0 _ hn hl (fun p hp => hp)
      (fun x hx => hx)) ho

/-- Assigning or revoking a team role keeps an owner (a revocation only when one remains). -/
theorem teamRole_owner (m m' : MTenant) (c : MCtx) (team : TeamId) (role : RoleId) (grant : Bool)
    (hn : m.TeamsNodup) (h : teamRoleStep m c team role grant = .ok m') (ho : m.hasOwner = true) :
    m'.hasOwner = true := by
  cases grant
  · unfold teamRoleStep at h
    cases hl : m.teams.lookup team with
    | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
    | some d =>
      simp only [hl] at h
      repeat' split at h
      all_goals try cases h
      all_goals first
        | (simp_all; done)
        | exact hasOwner_of_mono _ _ (putTeam_mono_some m team d _ hn hl (fun p hp => hp)
            (fun x hx => hx)) ho
  · unfold teamRoleStep at h
    cases hl : m.teams.lookup team with
    | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
    | some d =>
      simp only [hl] at h
      repeat' split at h
      all_goals try cases h
      all_goals first
        | exact hasOwner_of_mono _ _ (putTeam_mono_some m team d _ hn hl (fun p hp => hp)
            (fun x hx => List.mem_insert_iff.mpr (Or.inr hx))) ho
        | (simp_all; done)

/-- Adding or removing a team member keeps an owner (a removal only when one remains). -/
theorem teamMember_owner (m m' : MTenant) (c : MCtx) (team : TeamId) (p : PrincipalId) (add : Bool)
    (hn : m.TeamsNodup) (h : teamMemberStep m c team p add = .ok m') (ho : m.hasOwner = true) :
    m'.hasOwner = true := by
  cases add
  · unfold teamMemberStep at h
    cases hl : m.teams.lookup team with
    | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
    | some d =>
      simp only [hl] at h
      repeat' split at h
      all_goals try cases h
      all_goals first
        | (simp_all; done)
        | exact hasOwner_of_mono _ _ (putTeam_mono_some m team d _ hn hl
            (fun p' hp => hp) (fun x hx => hx)) ho
  · unfold teamMemberStep at h
    cases hl : m.teams.lookup team with
    | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
    | some d =>
      simp only [hl] at h
      repeat' split at h
      all_goals try cases h
      all_goals first
        | exact hasOwner_of_mono _ _ (putTeam_mono_some m team d _ hn hl
            (fun p' hp => List.mem_insert_iff.mpr (Or.inr hp)) (fun x hx => hx)) ho
        | (simp_all; done)

/-- Issuing or revoking an invitation changes no role, so it keeps an owner. -/
theorem putInv_owner (m m' : MTenant) (h : ∀ q, m'.rolesOf (.user q) = m.rolesOf (.user q))
    (ho : m.hasOwner = true) : m'.hasOwner = true :=
  hasOwner_of_mono _ _ (fun q x hx => by rw [h q]; exact hx) ho

theorem createInvitation_owner (m m' : MTenant) (c : MCtx) (inv : InvitationId) (address : String)
    (roles : List RoleId) (h : createInvitationStep m c inv address roles = .ok m')
    (ho : m.hasOwner = true) : m'.hasOwner = true := by
  unfold createInvitationStep at h
  dsimp only at h
  repeat' split at h
  all_goals try cases h
  all_goals exact putInv_owner _ _ (fun q => by simp only [rolesOf_putInv]) ho

theorem revokeInvitation_owner (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : revokeInvitationStep m c inv = .ok m') (ho : m.hasOwner = true) : m'.hasOwner = true := by
  unfold revokeInvitationStep at h
  repeat' split at h
  all_goals try cases h
  all_goals exact putInv_owner _ _ (fun q => by simp only [rolesOf_putInv]) ho

theorem redeemInvitation_owner (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') (ho : m.hasOwner = true) : m'.hasOwner = true := by
  unfold redeemInvitationStep at h
  cases hl : m.invitations.lookup inv with
  | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
  | some o =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact hasOwner_of_mono _ _ (fun q x hx => by
      rw [rolesOf_putInv]
      exact rolesOf_mono_setDirect m _ _ (fun r hr => mem_foldl_insert _ _ r hr) q x hx) ho

/-- Every step keeps the owner of a tenant that has one. -/
theorem stepMember_owner_kept (m m' : MTenant) (c : MCtx) (i : MemberIntent) (hn : m.TeamsNodup)
    (h : stepMember m c i = .ok m') (ho : m.hasOwner = true) : m'.hasOwner = true := by
  cases i with
  | assignRole p r => exact changeRole_owner m m' c p r true h ho
  | revokeRole p r => exact changeRole_owner m m' c p r false h ho
  | defineTeam t l => exact defineTeam_owner m m' c t l hn h ho
  | assignTeamRole t r => exact teamRole_owner m m' c t r true hn h ho
  | revokeTeamRole t r => exact teamRole_owner m m' c t r false hn h ho
  | addTeamMember t p => exact teamMember_owner m m' c t p true hn h ho
  | removeTeamMember t p => exact teamMember_owner m m' c t p false hn h ho
  | createInvitation i a rs _ => exact createInvitation_owner m m' c i a rs h ho
  | revokeInvitation i => exact revokeInvitation_owner m m' c i h ho
  | redeemInvitation i _ _ => exact redeemInvitation_owner m m' c i h ho

/-! ## Team ids stay distinct -/

theorem changeRole_nodup (m m' : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId)
    (grant : Bool) (hn : m.TeamsNodup) (h : changeRole m c target role grant = .ok m') :
    m'.TeamsNodup := by
  cases grant
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals try cases h
    all_goals exact stepMember_teamsNodup_of_teams hn rfl
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals try cases h
    all_goals exact stepMember_teamsNodup_of_teams hn rfl

theorem defineTeam_nodup (m m' : MTenant) (c : MCtx) (team : TeamId) (label : String)
    (hn : m.TeamsNodup) (h : defineTeamStep m c team label = .ok m') : m'.TeamsNodup := by
  unfold defineTeamStep at h
  cases hl : m.teams.lookup team with
  | none =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn
  | some d0 =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn

theorem teamRole_nodup (m m' : MTenant) (c : MCtx) (team : TeamId) (role : RoleId) (grant : Bool)
    (hn : m.TeamsNodup) (h : teamRoleStep m c team role grant = .ok m') : m'.TeamsNodup := by
  unfold teamRoleStep at h
  cases hl : m.teams.lookup team with
  | none =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn
  | some d =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn

theorem teamMember_nodup (m m' : MTenant) (c : MCtx) (team : TeamId) (p : PrincipalId) (add : Bool)
    (hn : m.TeamsNodup) (h : teamMemberStep m c team p add = .ok m') : m'.TeamsNodup := by
  unfold teamMemberStep at h
  cases hl : m.teams.lookup team with
  | none =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn
  | some d =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact putTeam_teamsNodup m _ _ hn

theorem createInvitation_nodup (m m' : MTenant) (c : MCtx) (inv : InvitationId) (address : String)
    (roles : List RoleId) (hn : m.TeamsNodup) (h : createInvitationStep m c inv address roles = .ok m') :
    m'.TeamsNodup := by
  unfold createInvitationStep at h
  dsimp only at h
  repeat' split at h
  all_goals try cases h
  all_goals exact stepMember_teamsNodup_of_teams hn rfl

theorem revokeInvitation_nodup (m m' : MTenant) (c : MCtx) (inv : InvitationId) (hn : m.TeamsNodup)
    (h : revokeInvitationStep m c inv = .ok m') : m'.TeamsNodup := by
  unfold revokeInvitationStep at h
  repeat' split at h
  all_goals try cases h
  all_goals exact stepMember_teamsNodup_of_teams hn rfl

theorem redeemInvitation_nodup (m m' : MTenant) (c : MCtx) (inv : InvitationId) (hn : m.TeamsNodup)
    (h : redeemInvitationStep m c inv = .ok m') : m'.TeamsNodup := by
  unfold redeemInvitationStep at h
  cases hl : m.invitations.lookup inv with
  | none =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
  | some o =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals exact stepMember_teamsNodup_of_teams hn rfl

/-- Every step keeps team ids distinct. -/
theorem stepMember_teamsNodup (m m' : MTenant) (c : MCtx) (i : MemberIntent) (hn : m.TeamsNodup)
    (h : stepMember m c i = .ok m') : m'.TeamsNodup := by
  cases i with
  | assignRole p r => exact changeRole_nodup m m' c p r true hn h
  | revokeRole p r => exact changeRole_nodup m m' c p r false hn h
  | defineTeam t l => exact defineTeam_nodup m m' c t l hn h
  | assignTeamRole t r => exact teamRole_nodup m m' c t r true hn h
  | revokeTeamRole t r => exact teamRole_nodup m m' c t r false hn h
  | addTeamMember t p => exact teamMember_nodup m m' c t p true hn h
  | removeTeamMember t p => exact teamMember_nodup m m' c t p false hn h
  | createInvitation i a rs _ => exact createInvitation_nodup m m' c i a rs hn h
  | revokeInvitation i => exact revokeInvitation_nodup m m' c i hn h
  | redeemInvitation i _ _ => exact redeemInvitation_nodup m m' c i hn h


/-! ## Conferring -/

/-- An accepted direct grant gives the role directly to its target, and the conferrer may confer it. -/
theorem changeRole_grant_form (m m' : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId)
    (h : changeRole m c target role true = .ok m') :
    m' = setDirect m target ((m.directOf target).insert role) ∧
      mayConfer m c.principal role = true := by
  constructor
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals try cases h
    all_goals first | rfl | simp_all
  · unfold changeRole at h
    dsimp only at h
    repeat' split at h
    all_goals (try cases h)
    all_goals simp_all

/-- Assigning a role directly gives no principal a role that the conferrer could not confer. -/
theorem assign_only_conferrable (m m' : MTenant) (c : MCtx) (target : PrincipalId) (role : RoleId)
    (h : stepMember m c (.assignRole target role) = .ok m') :
    ∀ q x, x ∈ m'.rolesOf (.user q) → x ∈ m.rolesOf (.user q) ∨ mayConfer m c.principal x = true := by
  obtain ⟨hm', hconf⟩ := changeRole_grant_form m m' c target role h
  subst hm'
  intro q x hx
  rw [effective_roles] at hx
  rcases hx with hx | hx | ⟨e, he, hq, hxe⟩
  · exact Or.inl ((effective_roles m q x).mpr (Or.inl hx))
  · rw [directOf_setDirect] at hx
    by_cases hqt : q = target
    · subst hqt
      simp only [if_true] at hx
      rcases List.mem_insert_iff.mp hx with hx1 | hx1
      · subst hx1
        exact Or.inr hconf
      · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx1)))
    · simp only [hqt, if_false] at hx
      exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx)))
  · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr ⟨e, he, hq, hxe⟩)))

/-- An accepted grant of a team role gives the role to the team's members, and the conferrer may confer it. -/
theorem teamRole_grant_form (m m' : MTenant) (c : MCtx) (team : TeamId) (role : RoleId)
    (h : teamRoleStep m c team role true = .ok m') :
    ∃ d, m.teams.lookup team = some d ∧ m' = putTeam m team { d with roles := d.roles.insert role } ∧
      mayConfer m c.principal role = true := by
  unfold teamRoleStep at h
  cases hl : m.teams.lookup team with
  | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
  | some d =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals first | (refine ⟨d, rfl, rfl, ?_⟩; simp_all) | simp_all

/-- Granting a team role gives no principal a role that the conferrer could not confer. -/
theorem teamRole_only_conferrable (m m' : MTenant) (c : MCtx) (team : TeamId) (role : RoleId)
    (hn : m.TeamsNodup) (h : stepMember m c (.assignTeamRole team role) = .ok m') :
    ∀ q x, x ∈ m'.rolesOf (.user q) → x ∈ m.rolesOf (.user q) ∨ mayConfer m c.principal x = true := by
  obtain ⟨d, hl, hm', hconf⟩ := teamRole_grant_form m m' c team role h
  subst hm'
  intro q x hx
  rw [effective_roles] at hx
  rcases hx with hx | hx | ⟨e, he, hq, hxe⟩
  · exact Or.inl ((effective_roles m q x).mpr (Or.inl hx))
  · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx)))
  · have hx' : x ∈ (putTeam m team _).teamRoles q := (mem_teamRoles _ _ _).mpr ⟨e, he, hq, hxe⟩
    rw [mem_teamRoles_putTeam] at hx'
    rcases hx' with ⟨hq', hx'⟩ | ⟨e', he', _, hq'', hx''⟩
    · have hx2 := List.mem_insert_iff.mp hx'
      rcases hx2 with hx3 | hx3
      · subst hx3
        exact Or.inr hconf
      · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr
          ⟨(team, d), mem_of_lookup_some m.teams team d hl, hq', hx3⟩)))
    · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr ⟨e', he', hq'', hx''⟩)))

/-- An accepted addition of a team member gives it the team's roles, which the conferrer may confer. -/
theorem teamMember_add_form (m m' : MTenant) (c : MCtx) (team : TeamId) (p : PrincipalId)
    (h : teamMemberStep m c team p true = .ok m') :
    ∃ d, m.teams.lookup team = some d ∧ m' = putTeam m team { d with members := d.members.insert p } ∧
      ∀ x ∈ d.roles, mayConfer m c.principal x = true := by
  unfold teamMemberStep at h
  cases hl : m.teams.lookup team with
  | none => simp only [hl] at h; repeat' split at h; all_goals try cases h
  | some d =>
    simp only [hl] at h
    repeat' split at h
    all_goals try cases h
    all_goals first | (refine ⟨d, rfl, rfl, ?_⟩; simp_all) | simp_all

/-- Adding a team member gives it no role that the conferrer could not confer. -/
theorem teamMember_only_conferrable (m m' : MTenant) (c : MCtx) (team : TeamId) (p : PrincipalId)
    (h : stepMember m c (.addTeamMember team p) = .ok m') :
    ∀ q x, x ∈ m'.rolesOf (.user q) → x ∈ m.rolesOf (.user q) ∨ mayConfer m c.principal x = true := by
  obtain ⟨d, hl, hm', hconf⟩ := teamMember_add_form m m' c team p h
  subst hm'
  intro q x hx
  rw [effective_roles] at hx
  rcases hx with hx | hx | ⟨e, he, hq, hxe⟩
  · exact Or.inl ((effective_roles m q x).mpr (Or.inl hx))
  · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx)))
  · have hx' : x ∈ (putTeam m team _).teamRoles q := (mem_teamRoles _ _ _).mpr ⟨e, he, hq, hxe⟩
    rw [mem_teamRoles_putTeam] at hx'
    rcases hx' with ⟨hq', hx''⟩ | ⟨e', he', _, hq'', hx'''⟩
    · rcases List.mem_insert_iff.mp hq' with hq1 | hq1
      · subst hq1
        exact Or.inr (hconf x hx'')
      · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr
          ⟨(team, d), mem_of_lookup_some m.teams team d hl, hq1, hx''⟩)))
    · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr ⟨e', he', hq'', hx'''⟩)))

/-- An accepted redemption: the invitation names the redeemer and the caller's issuer still holds the
    authority it stands on, and the redeemer gains the roles offered. -/
theorem redeem_ok_form (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') :
    ∃ redeemer o, c.principal = .user redeemer ∧ m.invitations.lookup inv = some o ∧
      m' = putInv (setDirect m redeemer (o.roles.foldl (fun acc r => acc.insert r) (m.directOf redeemer)))
        inv { o with redeemedBy := some redeemer } ∧
      o.redeemedBy = none ∧ o.revokedBy = none ∧ c.now < o.expiresAt ∧
      manageMembers m (.user o.createdBy) = true ∧
      (∀ x ∈ o.roles, mayConfer m (.user o.createdBy) x = true) := by
  unfold redeemInvitationStep at h
  cases hp : c.principal with
  | system => simp only [hp] at h; cases h
  | anonymous => simp only [hp] at h; cases h
  | automation _ => simp only [hp] at h; cases h
  | user redeemer =>
    simp only [hp] at h
    cases hl : m.invitations.lookup inv with
    | none => simp only [hl] at h; cases h
    | some o =>
      simp only [hl] at h
      refine ⟨redeemer, o, rfl, rfl, ?_⟩
      repeat' split at h
      all_goals try cases h
      all_goals (try rfl)
      all_goals simp_all

/-- Redeeming an invitation gives the redeemer no role that the issuer could not confer, and the issuer
    is the one the invitation names. -/
theorem redeem_only_conferrable (m m' : MTenant) (c : MCtx) (inv : InvitationId) (now : Nat) (verified : Option String)
    (h : stepMember m c (.redeemInvitation inv now verified) = .ok m') :
    ∃ o, m.invitations.lookup inv = some o ∧
      ∀ q x, x ∈ m'.rolesOf (.user q) → x ∈ m.rolesOf (.user q) ∨
        mayConfer m (.user o.createdBy) x = true := by
  obtain ⟨redeemer, o, _, hl, hm', _, _, _, _, hconf⟩ := redeem_ok_form m m' c inv h
  refine ⟨o, hl, ?_⟩
  intro q x hx
  subst hm'
  rw [rolesOf_putInv, effective_roles] at hx
  rcases hx with hx | hx | ⟨e, he, hq, hxe⟩
  · exact Or.inl ((effective_roles m q x).mpr (Or.inl hx))
  · rw [directOf_setDirect] at hx
    by_cases hqr : q = redeemer
    · subst hqr
      simp only [if_true] at hx
      rcases mem_of_mem_foldl_insert o.roles _ x hx with hx1 | hx1
      · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx1)))
      · exact Or.inr (hconf x hx1)
    · simp only [hqr, if_false] at hx
      exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inl hx)))
  · exact Or.inl ((effective_roles m q x).mpr (Or.inr (Or.inr ⟨e, he, hq, hxe⟩)))

/-! ## Invitations -/

/-- The table after an entry for `i` is put has that entry under `i`. -/
theorem lookup_putInv_self (m : MTenant) (i : InvitationId) (d : InvitationDef) :
    (putInv m i d).invitations.lookup i = some d := by
  unfold putInv
  simp

/-- An accepted creation records the invitation for the issuer, expiring fourteen days after the time. -/
theorem create_invitation_expiry (m m' : MTenant) (c : MCtx) (inv : InvitationId) (address : String)
    (roles : List RoleId) (h : createInvitationStep m c inv address roles = .ok m') :
    ∃ creator, c.principal = .user creator ∧
      ∃ d, m'.invitations.lookup inv = some d ∧ d.expiresAt = c.now + invitationLifetime ∧
        d.createdBy = creator := by
  unfold createInvitationStep at h
  cases hp : c.principal with
  | system => simp only [hp] at h; repeat' split at h; all_goals cases h
  | anonymous => simp only [hp] at h; repeat' split at h; all_goals cases h
  | automation _ => simp only [hp] at h; repeat' split at h; all_goals cases h
  | user creator =>
    simp only [hp] at h
    refine ⟨creator, rfl, ?_⟩
    repeat' split at h
    all_goals try cases h
    all_goals exact ⟨_, lookup_putInv_self _ _ _, rfl, rfl⟩

/-- An accepted redemption leaves the invitation redeemed by the redeemer. -/
theorem redeem_sets_redeemed (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') :
    ∃ redeemer o', m'.invitations.lookup inv = some o' ∧ o'.redeemedBy = some redeemer := by
  obtain ⟨redeemer, o, _, _, hm', _, _, _, _, _⟩ := redeem_ok_form m m' c inv h
  subst hm'
  exact ⟨redeemer, _, lookup_putInv_self _ _ _, rfl⟩

/-- An invitation that is redeemed or revoked is refused any redemption, whoever asks. -/
theorem redeem_refused_of_closed (m : MTenant) (c : MCtx) (inv : InvitationId) (o : InvitationDef)
    (hl : m.invitations.lookup inv = some o) (hc : (o.redeemedBy.isSome || o.revokedBy.isSome) = true) :
    ∃ e, redeemInvitationStep m c inv = .error e := by
  unfold redeemInvitationStep
  repeat' split
  all_goals first
    | exact ⟨_, rfl⟩
    | (exfalso; simp_all)
    | (simp_all)

/-- A redeemed invitation is never redeemed again. -/
theorem redeem_twice_refused (m' : MTenant) (c' : MCtx) (inv : InvitationId) (o : InvitationDef)
    (hl : m'.invitations.lookup inv = some o) (hr : o.redeemedBy.isSome = true) :
    ∃ e, redeemInvitationStep m' c' inv = .error e :=
  redeem_refused_of_closed m' c' inv o hl (by simp [hr])

/-- A revoked invitation is never redeemed. -/
theorem revoked_not_redeemable (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : revokeInvitationStep m c inv = .ok m') (c' : MCtx) :
    ∃ e, redeemInvitationStep m' c' inv = .error e := by
  unfold revokeInvitationStep at h
  cases hp : c.principal with
  | system => simp only [hp] at h; repeat' split at h; all_goals cases h
  | anonymous => simp only [hp] at h; repeat' split at h; all_goals cases h
  | automation _ => simp only [hp] at h; repeat' split at h; all_goals cases h
  | user actor =>
    simp only [hp] at h
    cases hl : m.invitations.lookup inv with
    | none => simp only [hl] at h; repeat' split at h; all_goals cases h
    | some o =>
      simp only [hl] at h
      repeat' split at h
      all_goals try cases h
      all_goals (subst_vars; exact redeem_refused_of_closed _ c' inv _ (lookup_putInv_self _ _ _) (by simp))

/-- An accepted redemption is made only before expiry: the invitation was not past its time. -/
theorem redeem_not_expired (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') :
    ∃ o, m.invitations.lookup inv = some o ∧ c.now < o.expiresAt := by
  obtain ⟨_, o, _, hl, _, _, _, hexp, _, _⟩ := redeem_ok_form m m' c inv h
  exact ⟨o, hl, hexp⟩

/-- An accepted redemption is of an invitation that is neither redeemed nor revoked. -/
theorem redeem_not_closed (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') :
    ∃ o, m.invitations.lookup inv = some o ∧ o.redeemedBy = none ∧ o.revokedBy = none := by
  obtain ⟨_, o, _, hl, _, hr, hv, _, _, _⟩ := redeem_ok_form m m' c inv h
  exact ⟨o, hl, hr, hv⟩

/-- An accepted redemption is made while the issuer may still manage members and confer each role. -/
theorem redeem_issuer_authority (m m' : MTenant) (c : MCtx) (inv : InvitationId)
    (h : redeemInvitationStep m c inv = .ok m') :
    ∃ o, m.invitations.lookup inv = some o ∧ manageMembers m (.user o.createdBy) = true ∧
      ∀ x ∈ o.roles, mayConfer m (.user o.createdBy) x = true := by
  obtain ⟨_, o, _, hl, _, _, _, _, hman, hconf⟩ := redeem_ok_form m m' c inv h
  exact ⟨o, hl, hman, hconf⟩

/-- Every membership intent but a redemption is accepted only from a principal who may manage members. -/
theorem stepMember_manage (m m' : MTenant) (c : MCtx) (j : MemberIntent)
    (hj : ∀ inv now verified, j ≠ .redeemInvitation inv now verified)
    (h : stepMember m c j = .ok m') : manageMembers m c.principal = true := by
  cases hm : manageMembers m c.principal with
  | true => rfl
  | false =>
    exfalso
    cases j with
    | redeemInvitation inv now verified => exact hj inv now verified rfl
    | _ =>
      simp [stepMember, changeRole, defineTeamStep, teamRoleStep, teamMemberStep,
        createInvitationStep, revokeInvitationStep, hm] at h

/-! ## Reserved principal ids -/

/-- A step that names a reserved principal, or whose caller is one, is refused before anything else:
    whatever the intent, and whether or not the tenant exists. -/
theorem reserved_refused (w : World) (c : Ctx) (i : Intent)
    (h : reservedRefused c.principal i = true) : step w i c = .error .invalidIntent := by
  unfold step
  simp [h]

/-- Assigning a reserved principal a role is refused. -/
theorem assign_reserved_refused (w : World) (c : Ctx) (p : PrincipalId) (r : RoleId)
    (h : reservedId p = true) : step w (.member (.assignRole p r)) c = .error .invalidIntent := by
  apply reserved_refused
  simp [reservedRefused, Intent.named, MemberIntent.named, h]

/-- Adding a reserved principal to a team is refused. -/
theorem add_member_reserved_refused (w : World) (c : Ctx) (team : TeamId) (p : PrincipalId)
    (h : reservedId p = true) :
    step w (.member (.addTeamMember team p)) c = .error .invalidIntent := by
  apply reserved_refused
  simp [reservedRefused, Intent.named, MemberIntent.named, h]

/-- Creating a tenant for a reserved owner is refused. -/
theorem create_reserved_refused (w : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (h : reservedId owner = true) : step w (.createTenant owner name) c = .error .invalidIntent := by
  apply reserved_refused
  simp [reservedRefused, Intent.named, h]

/-- A caller with a reserved id is refused, whatever the intent. -/
theorem reserved_caller_refused (w : World) (c : Ctx) (q : PrincipalId) (i : Intent)
    (hp : c.principal = .user q) (h : reservedId q = true) : step w i c = .error .invalidIntent := by
  apply reserved_refused
  simp [reservedRefused, callerReserved, hp, h]

/-! ## Creating a tenant -/

/-- An accepted creation makes the genesis tenant under the named owner, when the tenant did not exist,
    by the system, with a name that is not blank. -/
theorem create_genesis (w w' : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (h : step w (.createTenant owner name) c = .ok w') :
    w c.tenant = none ∧ c.principal = .system ∧ isBlank name = false ∧
      w' c.tenant = some (genesisTenant owner) := by
  unfold step at h
  split at h
  · cases h
  · cases hw : w c.tenant with
    | some t => simp [hw, stepTenant] at h
    | none =>
      simp only [hw] at h
      cases hc : createdTenant c.principal owner name with
      | error e => simp [hc] at h
      | ok t' =>
        simp only [hc, Except.ok.injEq] at h
        subst h
        unfold createdTenant at hc
        split at hc
        · cases hc
        · split at hc
          · cases hc
          · rename_i hp hb
            simp only [Except.ok.injEq] at hc
            subst hc
            exact ⟨rfl, by simpa using hp, by simpa using hb, by simp⟩

/-- A tenant that exists is not created again. -/
theorem create_existing_refused (w : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (hr : reservedRefused c.principal (.createTenant owner name) = false)
    (hs : (w c.tenant).isSome = true) :
    step w (.createTenant owner name) c = .error .tenantExists := by
  obtain ⟨m, hm⟩ := Option.isSome_iff_exists.mp hs
  unfold step
  simp [hr, hm, stepTenant]

/-- Only the system creates a tenant. -/
theorem create_not_system_refused (w : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (hr : reservedRefused c.principal (.createTenant owner name) = false)
    (hw : w c.tenant = none) (hp : c.principal ≠ .system) :
    step w (.createTenant owner name) c = .error .unauthorized := by
  unfold step
  simp [hr, hw, hp, createdTenant]

/-- A blank name is refused. -/
theorem create_blank_refused (w : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (hr : reservedRefused c.principal (.createTenant owner name) = false)
    (hw : w c.tenant = none) (hp : c.principal = .system) (hb : isBlank name = true) :
    step w (.createTenant owner name) c = .error .invalidIntent := by
  unfold step
  simp [hr, hw, hp, createdTenant, hb]

/-- The tenant a creation makes has an owner and distinct team ids. -/
theorem genesis_owner_and_nodup (o : PrincipalId) :
    (genesisTenant o).mt.hasOwner = true ∧ (genesisTenant o).mt.TeamsNodup := by
  have : (genesisTenant o).mt = genesisM o := rfl
  rw [this]
  exact ⟨genesis_hasOwner o, by simp [MTenant.TeamsNodup, genesisM]⟩

/-! ## The world -/

/-- An accepted step leaves every tenant but the addressed one as it was. -/
theorem step_other (w w' : World) (i : Intent) (c : Ctx) (h : step w i c = .ok w')
    (x : TenantId) (hx : x ≠ c.tenant) : w' x = w x := by
  unfold step at h
  split at h
  · cases h
  · repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; simp [hx])

/-- An accepted creation makes the world the old one with the new genesis tenant at the context's id. -/
theorem create_world_form (w w' : World) (c : Ctx) (owner : PrincipalId) (name : String)
    (h : step w (.createTenant owner name) c = .ok w') :
    w' = fun x => if x = c.tenant then some (genesisTenant owner) else w x := by
  obtain ⟨hw, hp, hb, _⟩ := create_genesis w w' c owner name h
  unfold step at h
  split at h
  · cases h
  · simp only [hw] at h
    have hc : createdTenant c.principal owner name = .ok (genesisTenant owner) := by
      simp [createdTenant, hp, hb]
    simp only [hc, Except.ok.injEq] at h
    exact h.symm

/-- An accepted membership intent in a tenant that exists changes that tenant by a membership step, and
    no other. -/
theorem step_member_form (w w' : World) (c : Ctx) (j : MemberIntent)
    (h : step w (.member j) c = .ok w') :
    ∃ t m', w c.tenant = some t ∧ stepMember t.mt (j.ctxOf c.principal) j = .ok m' ∧
      w' = fun x => if x = c.tenant then some (t.withM m') else w x := by
  unfold step at h
  split at h
  · cases h
  · cases hw : w c.tenant with
    | none => simp [hw] at h
    | some t =>
      simp only [hw] at h
      cases hs : stepMember t.mt (j.ctxOf c.principal) j with
      | error e => simp [stepTenant, stepMemberT, hs, Except.map] at h
      | ok m' =>
        simp only [stepTenant, stepMemberT, hs, Except.map] at h
        cases h
        exact ⟨t, m', rfl, hs, rfl⟩

/-- A step of a record or schema intent leaves the membership of the tenant as it was: the members,
    grants, policies, roles, teams and invitations. -/
theorem stepTenant_data_mt (t t' : Tenant) (p : Principal) (i : Intent)
    (hc : ∀ owner name, i ≠ .createTenant owner name) (hm : ∀ j, i ≠ .member j)
    (h : stepTenant t p i = .ok t') : t'.mt = t.mt := by
  cases i with
  | createTenant owner name => exact absurd rfl (hc owner name)
  | member j => exact absurd rfl (hm j)
  | writeSetting n lvl team v ev => obtain ⟨ov, rv, rfl⟩ := writeSettingT_ok_shape h; rfl
  | postLedger now ps => obtain ⟨L, rfl⟩ := postLedgerT_ok_shape h; rfl
  | postLedgerTx now tx => obtain ⟨L, rfl⟩ := postLedgerTxT_ok_shape h; rfl
  | expireLedger now u c => obtain ⟨L, rfl⟩ := expireLedgerT_ok_shape h; rfl
  | invokeOperation now op ins key => exact invokeOperationT_mt h
  | changeSchema ev strict cs =>
    obtain ⟨types1, -, -, ht', -, -⟩ := stepTenant_changeSchema_parts h
    subst ht'; rfl
  | create ty fs =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)
  | update ty id fs ev =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)
  | delete ty id ev =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)
  | setRequired ty f strict =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)
  | setUnique ty f strict =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)
  | defineUnique ty fs =>
    simp only [stepTenant] at h
    repeat' split at h
    all_goals try (cases h; done)
    all_goals (simp only [Except.ok.injEq] at h; subst h; rfl)

/-- The tenant a membership decision writes back has the membership it decided on. -/
theorem withM_mt (t : Tenant) (m : MTenant) : (t.withM m).mt = m := rfl

/-- An accepted step of any intent that is not a creation or a membership intent keeps the membership
    of the tenant it addresses. -/
theorem step_data_mt (w w' : World) (c : Ctx) (i : Intent)
    (hc : ∀ owner name, i ≠ .createTenant owner name) (hm : ∀ j, i ≠ .member j)
    (h : step w i c = .ok w') (t : Tenant) (ht : w c.tenant = some t) :
    ∃ t', w' c.tenant = some t' ∧ t'.mt = t.mt := by
  unfold step at h
  split at h
  · cases h
  · simp only [ht] at h
    cases hs : stepTenant t c.principal i with
    | error e => simp [hs] at h
    | ok t' =>
      simp only [hs] at h
      cases h
      exact ⟨t', by simp, stepTenant_data_mt t t' _ i hc hm hs⟩

/-- An accepted step of an intent that is not a creation addresses a tenant that exists. -/
theorem step_exists (w w' : World) (c : Ctx) (i : Intent)
    (hc : ∀ owner name, i ≠ .createTenant owner name) (h : step w i c = .ok w') :
    ∃ t, w c.tenant = some t := by
  unfold step at h
  split at h
  · cases h
  · cases hw : w c.tenant with
    | some t => exact ⟨t, rfl⟩
    | none =>
      simp only [hw] at h
      cases i with
      | createTenant owner name => exact absurd rfl (hc owner name)
      | _ => simp at h

/-- An accepted step keeps an owner in every tenant that had one, whichever tenant it addresses and
    whatever the intent. -/
theorem owner_kept_world (w w' : World) (c : Ctx) (i : Intent)
    (hn : ∀ x t, w x = some t → t.mt.TeamsNodup) (h : step w i c = .ok w') :
    ∀ x t, w x = some t → t.mt.hasOwner = true → ∃ t', w' x = some t' ∧ t'.mt.hasOwner = true := by
  intro x t hx ho
  by_cases hxc : x = c.tenant
  · subst hxc
    by_cases hcr : ∃ owner name, i = .createTenant owner name
    · obtain ⟨owner, name, rfl⟩ := hcr
      obtain ⟨hnone, _⟩ := create_genesis w w' c owner name h
      rw [hnone] at hx; cases hx
    · by_cases hmb : ∃ j, i = .member j
      · obtain ⟨j, rfl⟩ := hmb
        obtain ⟨t0, m', hmt, hs, hwf⟩ := step_member_form w w' c j h
        have : t0 = t := Option.some.inj (by rw [← hmt, hx])
        subst this
        refine ⟨t0.withM m', ?_, ?_⟩
        · rw [hwf]; simp
        · rw [withM_mt]
          exact stepMember_owner_kept t0.mt m' _ j (hn _ _ hmt) hs ho
      · have hc : ∀ owner name, i ≠ .createTenant owner name := fun o n e => hcr ⟨o, n, e⟩
        have hm : ∀ j, i ≠ .member j := fun j e => hmb ⟨j, e⟩
        obtain ⟨t', h1, h2⟩ := step_data_mt w w' c i hc hm h t hx
        exact ⟨t', h1, by rw [h2]; exact ho⟩
  · exact ⟨t, by rw [step_other w w' i c h x hxc]; exact hx, ho⟩

/-- An accepted step keeps team ids distinct in every tenant. -/
theorem step_nodup (w w' : World) (c : Ctx) (i : Intent)
    (hn : ∀ x t, w x = some t → t.mt.TeamsNodup) (h : step w i c = .ok w') :
    ∀ x t, w' x = some t → t.mt.TeamsNodup := by
  intro x t hx
  by_cases hxc : x = c.tenant
  · subst hxc
    by_cases hcr : ∃ owner name, i = .createTenant owner name
    · obtain ⟨owner, name, rfl⟩ := hcr
      obtain ⟨_, _, _, hg⟩ := create_genesis w w' c owner name h
      rw [hg] at hx
      cases hx
      exact (genesis_owner_and_nodup owner).2
    · by_cases hmb : ∃ j, i = .member j
      · obtain ⟨j, rfl⟩ := hmb
        obtain ⟨t0, m', hmt, hs, hwf⟩ := step_member_form w w' c j h
        rw [hwf] at hx
        simp only [if_true] at hx
        cases hx
        rw [withM_mt]
        exact stepMember_teamsNodup t0.mt m' _ j (hn _ _ hmt) hs
      · have hc : ∀ owner name, i ≠ .createTenant owner name := fun o n e => hcr ⟨o, n, e⟩
        have hm : ∀ j, i ≠ .member j := fun j e => hmb ⟨j, e⟩
        obtain ⟨t0, ht0⟩ := step_exists w w' c i hc h
        obtain ⟨t', h1, h2⟩ := step_data_mt w w' c i hc hm h t0 ht0
        rw [h1] at hx
        cases hx
        have := hn _ _ ht0
        unfold MTenant.TeamsNodup at this ⊢
        rw [h2]; exact this
  · rw [step_other w w' i c h x hxc] at hx
    exact hn x t hx

end Strata
