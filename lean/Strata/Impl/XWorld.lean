/-
The executable kernel, stage 2: the indexed world.

`stepX` is the transition function of the indexed kernel: `stepI` (stage 1) with `stepTenantX` for the
tenant it addresses. The theorems compose the two refinements, indexed to stage 1
(`stepTenantX_good`) and stage 1 to the spec (`stepTenantI_refines`):

* `stepX_refines`: for every world whose indexes are consistent (`XWorld.Ok`), the indexed kernel's
  outcome, read through `absX`, is the spec's outcome for the abstracted world.
* `stepX_ok`: an accepted step keeps every tenant's indexes consistent, so the refinement applies to
  the next step too.
* `absX_xInitial`, `xInitial_ok`: the fixture world, started with no records and empty indexes,
  abstracts to the spec's fixture world, and is consistent.
-/

import Strata.Impl.XSchema
import Strata.Impl.World

namespace Strata
namespace Impl

/-- A creation is refused by a tenant that exists, as in stage 1. -/
theorem createTenant_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell) (p : Principal)
    (owner : PrincipalId) (name : String) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.createTenant owner name))
      (stepTenantI core p (.createTenant owner name)) := by
  simp only [stepParts, stepTenantI]
  exact good_error _ _

/-- A membership step changes no record, so the indexes stay consistent. -/
theorem member_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (j : MemberIntent) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.member j))
      (stepTenantI core p (.member j)) := by
  simp only [stepParts, stepTenantI]
  cases hs : stepMember core.mt (j.ctxOf p) j with
  | error e => simp only [Except.map]; exact good_error _ _
  | ok m =>
    simp only [Except.map]
    refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    exact hok


/-- A setting write changes only the settings of the tenant, so its result is the tenant with those replaced. -/
theorem stepTenantI_writeSetting_shape {core c' : ITenant} {p : Principal} {n : String}
    {lvl : SettingLevel} {team : Option TeamId} {v : Option Val} {ev : Nat}
    (h : stepTenantI core p (.writeSetting n lvl team v ev) = .ok c') :
    ∃ ov rv, c' = { core with overrides := ov, settingRevs := rv } := by
  simp only [stepTenantI] at h
  cases hs : writeSettingT core.abs p n lvl team v ev with
  | error e => simp [hs, Except.map] at h
  | ok s =>
    simp only [hs, Except.map, Except.ok.injEq] at h
    obtain ⟨ov, rv, rfl⟩ := writeSettingT_ok_shape hs
    exact ⟨ov, rv, h.symm⟩

/-- A posting changes only the ledger of the tenant, so its result is the tenant with the ledger replaced (slice 13). -/
theorem stepTenantI_postLedger_shape {core c' : ITenant} {p : Principal} {now : Int}
    {ps : List LedgerPosting} (h : stepTenantI core p (.postLedger now ps) = .ok c') :
    ∃ L, c' = { core with ledger := L } := by
  simp only [stepTenantI] at h
  cases hs : postLedgerT core.abs p now ps with
  | error e => simp [hs, Except.map] at h
  | ok s =>
    simp only [hs, Except.map, Except.ok.injEq] at h
    obtain ⟨L, rfl⟩ := postLedgerT_ok_shape hs
    exact ⟨L, h.symm⟩

/-- A transaction changes only the ledger of the tenant (slice 13). -/
theorem stepTenantI_postLedgerTx_shape {core c' : ITenant} {p : Principal} {now : Int}
    {tx : LedgerTx} (h : stepTenantI core p (.postLedgerTx now tx) = .ok c') :
    ∃ L, c' = { core with ledger := L } := by
  simp only [stepTenantI] at h
  cases hs : postLedgerTxT core.abs p now tx with
  | error e => simp [hs, Except.map] at h
  | ok s =>
    simp only [hs, Except.map, Except.ok.injEq] at h
    obtain ⟨L, rfl⟩ := postLedgerTxT_ok_shape hs
    exact ⟨L, h.symm⟩

/-- An expiry changes only the ledger of the tenant, so its result is the tenant with the ledger replaced (slice 13). -/
theorem stepTenantI_expireLedger_shape {core c' : ITenant} {p : Principal} {now : Int} {u : Nat} {cut : Int}
    (h : stepTenantI core p (.expireLedger now u cut) = .ok c') :
    ∃ L, c' = { core with ledger := L } := by
  simp only [stepTenantI] at h
  cases hs : expireLedgerT core.abs p now u cut with
  | error e => simp [hs, Except.map] at h
  | ok s =>
    simp only [hs, Except.map, Except.ok.injEq] at h
    obtain ⟨L, rfl⟩ := expireLedgerT_ok_shape hs
    exact ⟨L, h.symm⟩

/-- A ledger write changes no record, so the indexes stay consistent. -/
theorem postLedger_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (now : Int) (ps : List LedgerPosting) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.postLedger now ps))
      (stepTenantI core p (.postLedger now ps)) := by
  simp only [stepParts]
  cases hs : stepTenantI core p (.postLedger now ps) with
  | error e => exact good_error _ _
  | ok c' =>
    obtain ⟨L, rfl⟩ := stepTenantI_postLedger_shape hs
    refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    exact hok

/-- A transaction changes no record, so the indexes stay consistent. -/
theorem postLedgerTx_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (now : Int) (tx : LedgerTx) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.postLedgerTx now tx))
      (stepTenantI core p (.postLedgerTx now tx)) := by
  simp only [stepParts]
  cases hs : stepTenantI core p (.postLedgerTx now tx) with
  | error e => exact good_error _ _
  | ok c' =>
    obtain ⟨L, rfl⟩ := stepTenantI_postLedgerTx_shape hs
    refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    exact hok

/-- An expiry changes no record, so the indexes stay consistent. -/
theorem expireLedger_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (now : Int) (u : Nat) (cut : Int) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.expireLedger now u cut))
      (stepTenantI core p (.expireLedger now u cut)) := by
  simp only [stepParts]
  cases hs : stepTenantI core p (.expireLedger now u cut) with
  | error e => exact good_error _ _
  | ok c' =>
    obtain ⟨L, rfl⟩ := stepTenantI_expireLedger_shape hs
    refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    exact hok

/-- A setting write changes no record, so the indexes stay consistent. -/
theorem setting_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (n : String) (lvl : SettingLevel)
    (team : Option TeamId) (v : Option Val) (ev : Nat) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.writeSetting n lvl team v ev))
      (stepTenantI core p (.writeSetting n lvl team v ev)) := by
  simp only [stepParts]
  cases hs : stepTenantI core p (.writeSetting n lvl team v ev) with
  | error e => exact good_error _ _
  | ok c' =>
    obtain ⟨ov, rv, rfl⟩ := stepTenantI_writeSetting_shape hs
    refine ⟨rfl, fun e x' h => (by cases h), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    exact hok

/-- The indexes built from every record are consistent. -/
theorem XTenant.rebuild_ok (core : ITenant) (hinv : core.Inv) (hnd : (core.types.map Prod.fst).Nodup) :
    (XTenant.rebuild core).Ok := by
  have hcell : ∀ (c : Cell) {K : Type} (kf : TypeId → IObj → Record → List K) (k : K),
      k ∈ core.keysBy kf c → c ∈ core.allCells := by
    intro c K kf k hk
    unfold ITenant.keysBy ITenant.cellRec at hk
    cases hT : core.typ? c.1 with
    | none => simp [hT] at hk
    | some o =>
      simp only [hT] at hk
      cases hg : getRec o.slots c.2 with
      | none => simp [hg] at hk
      | some r =>
        have hlt := lt_of_getRec _ _ _ hg
        unfold ITenant.allCells
        apply List.mem_flatMap.mpr
        exact ⟨(c.1, o), sc_lookup_mem_gen _ _ _ hT, (mem_cellsOf c.1 o.slots.size c).mpr ⟨rfl, hlt⟩⟩
  refine ⟨hinv, hnd, ?_, ?_, ?_⟩
  · apply MM.inv_addCells (∅ : MM (TypeId × FieldId × Val)) core.allCells core.ukeys (fun _ => []) core.ukeys
    · intro k c; simp [MM.get]
    · intro c k
      constructor
      · intro hk; exact Or.inr ⟨hcell c (fun S o r => ukeysOf S o.schema r) k hk, hk⟩
      · rintro (h | ⟨-, h⟩)
        · simp at h
        · exact h
  · apply MM.inv_addCells (∅ : MM (TypeId × List FieldId × List Val)) core.allCells core.ckeys (fun _ => [])
      core.ckeys
    · intro k c; simp [MM.get]
    · intro c k
      constructor
      · intro hk; exact Or.inr ⟨hcell c (fun S o r => ckeysOf S o.uniques r) k hk, hk⟩
      · rintro (h | ⟨-, h⟩)
        · simp at h
        · exact h
  · apply MM.inv_addCells (∅ : MM Cell) core.allCells core.rkeys (fun _ => []) core.rkeys
    · intro k c; simp [MM.get]
    · intro c k
      constructor
      · intro hk; exact Or.inr ⟨hcell c (fun S o r => rkeysOf o.schema r) k hk, hk⟩
      · rintro (h | ⟨-, h⟩)
        · simp at h
        · exact h

/-- A schema change in general: the indexed kernel takes stage 1's tenant and builds the indexes again. -/
theorem changeSchema_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ev : Option Nat) (strict : Bool)
    (cs : List Change) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.changeSchema ev strict cs))
      (stepTenantI core p (.changeSchema ev strict cs)) := by
  simp only [stepParts]
  cases h : stepTenantI core p (.changeSchema ev strict cs) with
  | error e => exact good_error _ _
  | ok core' =>
    refine ⟨rfl, fun e x' h' => (by cases h'), ?_⟩
    intro y hy
    simp only [Except.ok.injEq] at hy
    subst hy
    have hinv' : core'.Inv := stepTenantI_inv core p _ core' hok.1 h
    have hs := stepTenantI_refines core p (.changeSchema ev strict cs) hok.1
    rw [h] at hs
    simp only [Except.map] at hs
    obtain ⟨types1, hr, -, -, ht', -, -⟩ := changeSchema_ctx hs.symm
    have hk0 : (core.abs.types.map Prod.fst).Nodup := by
      have : core.abs.types.map Prod.fst = core.types.map Prod.fst := by
        unfold ITenant.abs
        rw [List.map_map]
        rfl
      rw [this]; exact hok.2.1
    have hkeys : (core'.abs.types.map Prod.fst).Nodup := by
      rw [show core'.abs.types = rewriteTypes core.abs.types (cs.flatMap Change.touched) types1 from
        congrArg Tenant.types ht', sc_keys_rewriteTypes]
      exact hr.keys hk0
    have hmap : core'.abs.types.map Prod.fst = core'.types.map Prod.fst := by
      unfold ITenant.abs
      rw [List.map_map]
      rfl
    apply XTenant.rebuild_ok core' hinv'
    rw [← hmap]
    exact hkeys

/-- The indexed outcome of one intent against one tenant agrees with stage 1's, and keeps the
    indexes consistent. -/
theorem stepTenantXL_good (x : XTenant) (hok : x.Ok) (p : Principal) (i : Intent) :
    Good x (stepTenantXL x p i) (stepTenantI x.core p i) := by
  obtain ⟨core, ux, cx, rx⟩ := x
  cases i with
  | create ty fs => exact create_good core ux cx rx hok p ty fs
  | update ty id fs ev => exact update_good core ux cx rx hok p ty id fs ev
  | delete ty id ev => exact delete_good core ux cx rx hok p ty id ev
  | setRequired ty f strict => exact setRequired_good core ux cx rx hok p ty f strict
  | setUnique ty f strict => exact setUnique_good core ux cx rx hok p ty f strict
  | defineUnique ty fs => exact defineUnique_good core ux cx rx hok p ty fs
  | createTenant owner name => exact createTenant_good core ux cx rx p owner name
  | member j => exact member_good core ux cx rx hok p j
  | writeSetting n lvl team v ev => exact setting_good core ux cx rx hok p n lvl team v ev
  | postLedger now ps => exact postLedger_good core ux cx rx hok p now ps
  | expireLedger now u c => exact expireLedger_good core ux cx rx hok p now u c
  | postLedgerTx now tx => exact postLedgerTx_good core ux cx rx hok p now tx
  | changeSchema ev strict cs => exact changeSchema_good core ux cx rx hok p ev strict cs

/-- The indexed outcome (refusals without the tenant handed back) agrees with stage 1's, and an
    accepted indexed tenant is consistent. -/
theorem stepTenantX_good (x : XTenant) (hok : x.Ok) (p : Principal) (i : Intent) :
    (stepTenantX x p i).map XTenant.core = stepTenantI x.core p i ∧
      ∀ y, stepTenantX x p i = .ok y → y.Ok := by
  have h := stepTenantXL_good x hok p i
  refine ⟨h.1, fun y hy => h.2.2 y ?_⟩
  unfold stepTenantX at hy
  cases hL : stepTenantXL x p i with
  | error e => rw [hL] at hy; cases hy
  | ok z =>
    rw [hL] at hy
    have hz : z = y := by simpa [Except.mapError] using hy
    rw [hz]

/-- The spec's tenant an indexed tenant stands for. -/
def XTenant.abs (x : XTenant) : Tenant := x.core.abs

/-- One intent against one tenant: the indexed kernel's outcome, read through the abstraction, is
    the spec's. -/
theorem stepTenantX_refines (x : XTenant) (hok : x.Ok) (p : Principal) (i : Intent) :
    (stepTenantX x p i).map XTenant.abs = stepTenant x.abs p i := by
  have hg := (stepTenantX_good x hok p i).1
  have hs := stepTenantI_refines x.core p i hok.1
  unfold XTenant.abs
  rw [← hs, ← hg]
  cases stepTenantX x p i <;> rfl

/-- An accepted indexed step keeps the indexes consistent. -/
theorem stepTenantX_ok (x : XTenant) (hok : x.Ok) (p : Principal) (i : Intent) (y : XTenant)
    (h : stepTenantX x p i = .ok y) : y.Ok :=
  (stepTenantX_good x hok p i).2 y h

/-- The whole indexed state: each tenant id has a tenant or does not. -/
abbrev XWorld := TenantId → Option XTenant

/-- The spec's world for an indexed world. -/
def absX (w : XWorld) : World := fun x => (w x).map XTenant.abs

/-- Every tenant of the world has consistent indexes. -/
def XWorld.Ok (w : XWorld) : Prop := ∀ x t, w x = some t → t.Ok

/-- A tenant with no records and empty indexes. -/
def XTenant.ofCore (t : ITenant) : XTenant := ⟨t, ∅, ∅, ∅⟩

theorem ofCore_ok (t : ITenant) (hinv : t.Inv) (hnd : (t.types.map Prod.fst).Nodup)
    (hempty : ∀ ty o, t.typ? ty = some o → o.slots = #[]) : (XTenant.ofCore t).Ok := by
  have hk : ∀ {K : Type} (kf : TypeId → IObj → Record → List K) (c : Cell), t.keysBy kf c = [] := by
    intro K kf c
    unfold ITenant.keysBy ITenant.cellRec
    cases hT : t.typ? c.1 with
    | none => rfl
    | some o => simp [hempty _ _ hT, getRec]
  refine ⟨hinv, hnd, ?_, ?_, ?_⟩
  · intro k c
    have e : t.ukeys c = [] := hk (fun S o r => ukeysOf S o.schema r) c
    simp [MM.get, XTenant.ofCore, e]
  · intro k c
    have e : t.ckeys c = [] := hk (fun S o r => ckeysOf S o.uniques r) c
    simp [MM.get, XTenant.ofCore, e]
  · intro k c
    have e : t.rkeys c = [] := hk (fun S o r => rkeysOf o.schema r) c
    simp [MM.get, XTenant.ofCore, e]

/-- The indexed tenant a `CreateTenant` makes: the genesis tenant with no records and empty indexes. -/
def createdTenantX (p : Principal) (owner : PrincipalId) (name : String) : Except Err XTenant :=
  (createdTenantI p owner name).map XTenant.ofCore

/-- The genesis tenant has consistent (empty) indexes. -/
theorem genesis_ok (owner : PrincipalId) : (XTenant.ofCore (ITenant.genesis owner)).Ok :=
  ofCore_ok _ (genesis_inv owner) (by simp [ITenant.genesis])
    (fun ty o h => by simp [ITenant.typ?, ITenant.genesis] at h)

/-- The tenant a `CreateTenant` makes, as the indexed kernel holds it, is the spec's. -/
theorem createdTenantX_abs (p : Principal) (owner : PrincipalId) (name : String) :
    (createdTenantX p owner name).map XTenant.abs = createdTenant p owner name := by
  rw [← createdTenantI_abs]
  unfold createdTenantX
  cases createdTenantI p owner name <;> rfl

/-- An accepted creation makes a consistent indexed tenant. -/
theorem createdTenantX_ok (p : Principal) (owner : PrincipalId) (name : String) (t : XTenant)
    (h : createdTenantX p owner name = .ok t) : t.Ok := by
  unfold createdTenantX createdTenantI at h
  repeat' split at h
  all_goals first | (cases h; done) | (simp only [Except.map, Except.ok.injEq] at h; subst h; exact genesis_ok owner)

/-- The indexed kernel's transition function: one intent, in one context. -/
def stepX (w : XWorld) (i : Intent) (c : Ctx) : Except Err XWorld :=
  if reservedRefused c.principal i then .error .invalidIntent
  else
  match w c.tenant with
  | none =>
    match i with
    | .createTenant owner name =>
      match createdTenantX c.principal owner name with
      | .error e => .error e
      | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)
    | _ => .error .noTenant
  | some t =>
    match stepTenantX t c.principal i with
    | .error e => .error e
    | .ok t' => .ok (fun x => if x = c.tenant then some t' else w x)

/-- One intent against the world: the indexed kernel refines the spec. -/
theorem stepX_refines (w : XWorld) (i : Intent) (c : Ctx) (hw : w.Ok) :
    (stepX w i c).map absX = step (absX w) i c := by
  unfold stepX step
  by_cases hr : reservedRefused c.principal i = true
  · simp [hr, Except.map]
  · simp only [hr, if_false, Bool.false_eq_true]
    cases hc : w c.tenant with
    | none =>
      have hc' : absX w c.tenant = none := by simp [absX, hc]
      simp only [hc, hc']
      cases i with
      | createTenant owner name =>
        dsimp only
        have h1 := createdTenantX_abs c.principal owner name
        cases h2 : createdTenantX c.principal owner name with
        | error e =>
          rw [h2] at h1
          simp only [Except.map] at h1
          rw [← h1]
          rfl
        | ok t' =>
          rw [h2] at h1
          simp only [Except.map] at h1
          rw [← h1]
          simp only [Except.map]
          congr 1
          funext x
          by_cases hx : x = c.tenant
          · simp [absX, hx]
          · simp [absX, hx]
      | _ => rfl
    | some t =>
      have hs := stepTenantX_refines t (hw c.tenant t hc) c.principal i
      simp only [absX, hc, Option.map_some]
      cases h' : stepTenantX t c.principal i with
      | error e =>
        rw [h'] at hs
        simp only [Except.map] at hs
        rw [← hs]
        simp [Except.map]
      | ok t' =>
        rw [h'] at hs
        simp only [Except.map] at hs
        rw [← hs]
        simp only [Except.map, absX]
        congr 1
        funext x
        by_cases hx : x = c.tenant
        · simp [absX, hx]
        · simp [absX, hx]

/-- An accepted step keeps the indexes of every tenant consistent. -/
theorem stepX_ok (w : XWorld) (i : Intent) (c : Ctx) (w' : XWorld) (hw : w.Ok)
    (h : stepX w i c = .ok w') : w'.Ok := by
  unfold stepX at h
  by_cases hr : reservedRefused c.principal i = true
  · simp [hr] at h
  · simp only [hr, if_false, Bool.false_eq_true] at h
    cases hc : w c.tenant with
    | none =>
      simp only [hc] at h
      cases i with
      | createTenant owner name =>
        cases h2 : createdTenantX c.principal owner name with
        | error e => simp [h2] at h
        | ok t'' =>
          simp only [h2, Except.ok.injEq] at h
          subst h
          intro x s hx
          by_cases hxc : x = c.tenant
          · simp [hxc] at hx
            subst hx
            exact createdTenantX_ok _ _ _ _ h2
          · simp only [hxc, if_false] at hx
            exact hw x s hx
      | _ => simp at h
    | some t =>
      rw [hc] at h
      cases h' : stepTenantX t c.principal i with
      | error e => simp [h'] at h
      | ok t'' =>
        simp only [h', Except.ok.injEq] at h
        subst h
        intro x s hx
        by_cases hxc : x = c.tenant
        · subst hxc
          simp at hx
          subst hx
          exact stepTenantX_ok t (hw c.tenant t hc) c.principal i t'' h'
        · simp only [hxc, if_false] at hx
          exact hw x s hx

/-- The indexed start state for a fixture tenant. -/
def xInitial : XWorld := fun x => (implInitial x).map XTenant.ofCore

theorem xInitial_ok : xInitial.Ok := by
  intro x t h
  unfold xInitial implInitial at h
  cases hw : initialWorld x with
  | none => simp [hw] at h
  | some s =>
    simp only [hw, Option.map_some, Option.some.injEq] at h
    subst h
    refine ofCore_ok _ (ofEmpty_inv s) ?_ ?_
    · unfold initialWorld initialTenant at hw
      split at hw
      · cases hw; decide
      · split at hw
        · cases hw; decide
        · cases hw
    · intro ty o ho
      unfold ITenant.typ? ITenant.ofEmpty at ho
      have := lookup_map_entry s.types (fun _ o => IObj.ofEmpty o) ty
      rw [this] at ho
      cases hl : s.types.lookup ty with
      | none => simp [hl] at ho
      | some o2 =>
        simp only [hl, Option.map_some, Option.some.injEq] at ho
        subst ho
        rfl

theorem absX_xInitial : absX xInitial = initialWorld := by
  funext x
  have := congrFun absW_implInitial x
  unfold absX xInitial absW at *
  cases h : implInitial x with
  | none => simp [h] at this ⊢; exact this
  | some t => simp [h, XTenant.ofCore, XTenant.abs] at this ⊢; exact this

end Impl
end Strata
