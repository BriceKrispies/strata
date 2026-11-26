/-
The executable kernel, stage 2: the delete.

Two facts carry the delete. `blockedX_eq`: asking the referrer index which records block the delete
gives the stage 1 answer, which scans every slot of every type. `deleteX_good`: clearing the
references through the cells the index lists, one record at a time, leaves exactly the tenant stage 1
leaves by visiting every slot, and keeps the indexes consistent.

The clearing is a fold of `XTenant.clearOne` over the listed cells. `ClearedFrom` says where the fold
has got to: the tenant is the starting one with the records of the cells so far cleared of their
references to the deleted record (`clearRef`), which is idempotent, so a cell listed twice is no
problem. Once the fold is done, every record that refers to the deleted record is in the list (the
index is complete), so every record is cleared that stage 1 clears.
-/

import Strata.Impl.XProofs

namespace Strata
namespace Impl

theorem clearRef_idem (schema : Schema) (T : TypeId) (id : RecordId) (r : Record) :
    clearRef schema T id (clearRef schema T id r) = clearRef schema T id r := by
  have key : ∀ r', recRefers schema T id r' = false → clearRef schema T id r' = r' := by
    intro r' h
    unfold clearRef
    simp [h]
  apply key
  unfold recRefers
  rw [List.any_eq_false]
  intro fv hfv
  simp [fieldRefers_clearRef schema T id r fv hfv]

theorem blockedX_eq (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (ty : TypeId) (id : RecordId) :
    blockedX core rx ty id = blockedI core ty id := by
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok
  dsimp only at hinv hnd hu hc hr
  unfold blockedX blockedI
  rw [Bool.eq_iff_iff, List.any_eq_true, List.any_eq_true]
  constructor
  · rintro ⟨c, hcm, hb⟩
    rw [hr] at hcm
    unfold ITenant.rkeys ITenant.cellRec at hcm
    unfold ITenant.cellRec at hb
    cases hT : core.typ? c.1 with
    | none => simp [hT] at hb
    | some o =>
      cases hg : getRec o.slots c.2 with
      | none => simp [hT, hg] at hb
      | some r =>
        simp only [hT, hg, Option.map_some] at hb
        refine ⟨(c.1, o), mem_of_lookup _ _ _ hT, ?_⟩
        rw [anyFrom_iff]
        exact ⟨c.2, r, lt_of_getRec _ _ _ hg, getRec_some _ _ _ hg, hb⟩
  · rintro ⟨e, he, hb⟩
    rw [anyFrom_iff] at hb
    obtain ⟨i, r, hi, hr', hp⟩ := hb
    have hT : core.typ? e.1 = some e.2 := lookup_of_mem_nodup _ e he hnd
    have hg : getRec e.2.slots i = some r := by unfold getRec; simp [hr']
    refine ⟨(e.1, i), ?_, ?_⟩
    · rw [hr]
      unfold ITenant.rkeys ITenant.cellRec
      simp only [hT, hg, Option.map_some]
      rw [mem_rkeysOf]
      simp only [blocks, Bool.and_eq_true, Bool.not_eq_true'] at hp
      obtain ⟨_, hany⟩ := hp
      simp only [recRefers, List.any_eq_true] at hany ⊢
      obtain ⟨fv, hfv, hf⟩ := hany
      simp only [Bool.and_eq_true] at hf
      exact ⟨fv, hfv, hf.1⟩
    · unfold ITenant.cellRec
      simp only [hT, hg, Option.map_some]
      exact hp

theorem typ?_putType_same (t : ITenant) (ty : TypeId) (o o' : IObj) (h : t.typ? ty = some o) :
    (t.putType ty o').typ? ty = some o' := by
  rw [typ?_putType]; simp [h]

theorem typ?_putType_other (t : ITenant) (ty T : TypeId) (o' : IObj) (h : T ≠ ty) :
    (t.putType ty o').typ? T = t.typ? T := by
  rw [typ?_putType]
  have : (T == ty) = false := by simp [h]
  simp [this]

/-- Where the clearing fold has got to. `y` is `y0` with the records of the cells of `S` cleared of
    their references to record `id` of type `ty`; the indexes of `y` are consistent. -/
structure ClearedFrom (ty : TypeId) (id : RecordId) (S : Cell → Prop) (y0 y : XTenant) : Prop where
  ok : y.Ok
  keys : y.core.types.map Prod.fst = y0.core.types.map Prod.fst
  nextId : y.core.nextId = y0.core.nextId
  members : y.core.members = y0.core.members
  grants : y.core.grants = y0.core.grants
  policies : y.core.policies = y0.core.policies
  limits : y.core.limits = y0.core.limits
  roles : y.core.roles = y0.core.roles
  teams : y.core.teams = y0.core.teams
  invitations : y.core.invitations = y0.core.invitations
  isSome : ∀ T, (y.core.typ? T).isSome = (y0.core.typ? T).isSome
  typ : ∀ T o0, y0.core.typ? T = some o0 → ∃ o, y.core.typ? T = some o ∧ o.schema = o0.schema ∧
    o.uniques = o0.uniques ∧ o.constraints = o0.constraints ∧ o.workflows = o0.workflows ∧ o.slots.size = o0.slots.size ∧
    ∀ i, (S (T, i) → getRec o.slots i = (getRec o0.slots i).map (clearRef o0.schema ty id)) ∧
      (¬ S (T, i) → getRec o.slots i = getRec o0.slots i)

theorem ClearedFrom.refl (ty : TypeId) (id : RecordId) (y : XTenant) (h : y.Ok) :
    ClearedFrom ty id (fun _ => False) y y :=
  ⟨h, rfl, rfl, rfl, rfl, rfl, rfl, rfl, rfl, rfl, fun _ => rfl,
    fun T o0 ho => ⟨o0, ho, rfl, rfl, rfl, rfl, rfl, fun i => ⟨fun h => h.elim, fun _ => rfl⟩⟩⟩

theorem clearOne_cleared (ty : TypeId) (id : RecordId) (S : Cell → Prop) (y0 y : XTenant)
    (c : Cell) (h : ClearedFrom ty id S y0 y) :
    ClearedFrom ty id (fun c' => S c' ∨ c' = c) y0 (y.clearOne ty id c) := by
  obtain ⟨core, ux, cx, rx⟩ := y
  have hok := h.ok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok
  dsimp only at hinv hnd hu hc hr
  -- the cases where nothing changes
  have unchanged : (core.typ? c.1 = none ∨ ∃ o, core.typ? c.1 = some o ∧ getRec o.slots c.2 = none) →
      ClearedFrom ty id (fun c' => S c' ∨ c' = c) y0 ⟨core, ux, cx, rx⟩ := by
    intro hcase
    refine ⟨h.ok, h.keys, h.nextId, h.members, h.grants, h.policies, h.limits, h.roles, h.teams, h.invitations, h.isSome, ?_⟩
    intro T o0 ho0
    obtain ⟨o, hT, hs, hu', hcn, hw, hsz, hi⟩ := h.typ T o0 ho0
    refine ⟨o, hT, hs, hu', hcn, hw, hsz, fun i => ⟨?_, ?_⟩⟩
    · rintro (hS | hcell)
      · exact (hi i).1 hS
      · -- the cell is `c`, which holds no record
        have hTc : T = c.1 := by have := congrArg Prod.fst hcell; simpa using this
        have hic : i = c.2 := by have := congrArg Prod.snd hcell; simpa using this
        subst hTc; subst hic
        rcases hcase with hn | ⟨o', ho', hg⟩
        · have : (core.typ? c.1) = some o := hT
          rw [hn] at this; cases this
        · have hoo : o' = o := by
            have : (core.typ? c.1) = some o := hT
            rw [ho'] at this; exact Option.some.inj this
          subst hoo
          by_cases hS : S (c.1, c.2)
          · have := (hi c.2).1 hS
            rw [hg] at this
            rw [hg]
            cases hg0 : getRec o0.slots c.2 with
            | none => simp
            | some r0 => rw [hg0] at this; simp at this
          · have := (hi c.2).2 hS
            rw [hg] at this
            rw [hg, ← this]
            simp
    · intro hn
      exact (hi i).2 (fun hS => hn (Or.inl hS))
  dsimp only [XTenant.clearOne]
  cases hty : core.typ? c.1 with
  | none => exact unchanged (Or.inl hty)
  | some o =>
    simp only [hty]
    cases hg : getRec o.slots c.2 with
    | none => exact unchanged (Or.inr ⟨o, hty, hg⟩)
    | some r =>
      simp only [hg]
      have hsz := hinv c.1 o hty
      have hlt := lt_of_getRec _ _ _ hg
      have hmod := ITenant.modifyType_eq core c.1
        (fun o' => { o' with slots := setSlot o'.slots c.2 (some (clearRef o.schema ty id r)) }) o hty
      have hrefl : ∀ i, i ≠ c.2 → getRec (setSlot o.slots c.2 (some (clearRef o.schema ty id r))) i =
          getRec o.slots i := by
        intro i hi; rw [getRec_setSlot _ _ _ hlt]; simp [hi]
      have hnewslot : getRec (setSlot o.slots c.2 (some (clearRef o.schema ty id r))) c.2 =
          some (clearRef o.schema ty id r) := by
        rw [getRec_setSlot _ _ _ hlt]; simp
      have hch := cc_putType core c.1 c.2 (some (clearRef o.schema ty id r)) o
        { o with slots := setSlot o.slots c.2 (some (clearRef o.schema ty id r)) } hty rfl rfl
        hrefl hnewslot
      have hnew := XTenant.ok_reindex ⟨core, ux, cx, rx⟩
        (core.putType c.1 { o with slots := setSlot o.slots c.2 (some (clearRef o.schema ty id r)) })
        c.1 c.2 o (some r) (some (clearRef o.schema ty id r)) h.ok hch hty hg
        (inv_putType core c.1 _ core.nextId (Nat.le_refl _) (by simp [size_setSlot, hsz]) hinv)
      show ClearedFrom ty id _ y0 (XTenant.reindex
        (core.modifyType c.1
          (fun o' => { o' with slots := setSlot o'.slots c.2 (some (clearRef o.schema ty id r)) }))
        ux cx rx c.1 c.2 o.schema o.uniques (some r) (some (clearRef o.schema ty id r)))
      rw [hmod]
      refine ⟨hnew, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
      · show (putTypeI core.types c.1 _).map Prod.fst = _
        rw [putTypeI_keys]; exact h.keys
      · exact h.nextId
      · exact h.members
      · exact h.grants
      · exact h.policies
      · exact h.limits
      · exact h.roles
      · exact h.teams
      · exact h.invitations
      · intro T
        by_cases hT : T = c.1
        · subst hT
          show ((core.putType c.1 _).typ? c.1).isSome = _
          rw [typ?_putType_same _ _ _ _ hty, ← h.isSome c.1]
          simp [hty]
        · show ((core.putType c.1 _).typ? T).isSome = _
          rw [typ?_putType_other _ _ _ _ hT]
          exact h.isSome T
      · intro T o0 ho0
        obtain ⟨oy, hT, hs, hu', hcn, hw, hsz', hi⟩ := h.typ T o0 ho0
        by_cases hTc : T = c.1
        · subst hTc
          have hoy : oy = o := by rw [hty] at hT; exact (Option.some.inj hT).symm
          subst hoy
          refine ⟨_, typ?_putType_same _ _ _ _ hty, hs, hu', hcn, hw, ?_, ?_⟩
          · simp [size_setSlot, hsz']
          · intro i
            by_cases hic : i = c.2
            · subst hic
              have hr' : getRec (setSlot oy.slots c.2 (some (clearRef oy.schema ty id r))) c.2 =
                  some (clearRef oy.schema ty id r) := hnewslot
              refine ⟨fun _ => ?_, fun hn => absurd (Or.inr rfl) hn⟩
              show getRec (setSlot oy.slots c.2 _) c.2 = _
              rw [hr']
              by_cases hS : S (c.1, c.2)
              · have := (hi c.2).1 hS
                rw [hg] at this
                cases hg0 : getRec o0.slots c.2 with
                | none => rw [hg0] at this; simp at this
                | some r0 =>
                  rw [hg0] at this
                  simp only [Option.map_some, Option.some.injEq] at this
                  rw [← hs] at this
                  rw [Option.map_some, ← hs, this, clearRef_idem]
              · have := (hi c.2).2 hS
                rw [hg] at this
                rw [← this, ← hs]
                rfl
            · have hne : ¬ ((c.1, i) = (c.1, c.2)) := by simp [hic]
              refine ⟨fun hS => ?_, fun hn => ?_⟩
              · rcases hS with hS | hS
                · show getRec (setSlot oy.slots c.2 _) i = _
                  rw [hrefl i hic]; exact (hi i).1 hS
                · exact absurd hS hne
              · show getRec (setSlot oy.slots c.2 _) i = _
                rw [hrefl i hic]
                exact (hi i).2 (fun hS => hn (Or.inl hS))
        · refine ⟨oy, ?_, hs, hu', hcn, hw, hsz', ?_⟩
          · show (core.putType c.1 _).typ? T = _
            rw [typ?_putType_other _ _ _ _ hTc]; exact hT
          · intro i
            have hne : ¬ ((T, i) = c) := by
              intro h'; apply hTc; have := congrArg Prod.fst h'; simpa using this
            refine ⟨fun hS => ?_, fun hn => ?_⟩
            · rcases hS with hS | hS
              · exact (hi i).1 hS
              · exact absurd hS hne
            · exact (hi i).2 (fun hS => hn (Or.inl hS))

theorem ClearedFrom.congr {ty : TypeId} {id : RecordId} {S S' : Cell → Prop} {y0 y : XTenant}
    (hS : ∀ c, S c ↔ S' c) (h : ClearedFrom ty id S y0 y) : ClearedFrom ty id S' y0 y := by
  refine ⟨h.ok, h.keys, h.nextId, h.members, h.grants, h.policies, h.limits, h.roles, h.teams, h.invitations, h.isSome, ?_⟩
  intro T o0 ho0
  obtain ⟨o, hT, hs, hu, hcn, hw, hsz, hi⟩ := h.typ T o0 ho0
  exact ⟨o, hT, hs, hu, hcn, hw, hsz, fun i => ⟨fun h' => (hi i).1 ((hS _).mpr h'),
    fun h' => (hi i).2 (fun h'' => h' ((hS _).mp h''))⟩⟩

/-- Clearing one referrer keeps the tenant's revision, event sequence and schema version (slice 9). -/
theorem clearOne_revision (ty : TypeId) (id : RecordId) (y : XTenant) (c : Cell) :
    (XTenant.clearOne ty id y c).core.revision = y.core.revision ∧
      (XTenant.clearOne ty id y c).core.eventSeq = y.core.eventSeq ∧
      (XTenant.clearOne ty id y c).core.schemaVersion = y.core.schemaVersion := by
  unfold XTenant.clearOne
  rcases y with ⟨core, ux, cx, rx⟩
  dsimp only
  split <;> (try split) <;> (try split) <;> simp [XTenant.reindex, ITenant.modifyType]

theorem foldl_clearOne_revision (ty : TypeId) (id : RecordId) (ds : List Cell) :
    ∀ y : XTenant, (ds.foldl (XTenant.clearOne ty id) y).core.revision = y.core.revision ∧
      (ds.foldl (XTenant.clearOne ty id) y).core.eventSeq = y.core.eventSeq ∧
      (ds.foldl (XTenant.clearOne ty id) y).core.schemaVersion = y.core.schemaVersion := by
  induction ds with
  | nil => intro y; exact ⟨rfl, rfl, rfl⟩
  | cons c ds ih =>
    intro y
    simp only [List.foldl_cons]
    obtain ⟨h1, h2, h3⟩ := ih (XTenant.clearOne ty id y c)
    obtain ⟨g1, g2, g3⟩ := clearOne_revision ty id y c
    exact ⟨h1.trans g1, h2.trans g2, h3.trans g3⟩

/-- Clearing one referrer keeps the settings of the tenant (slice 12). -/
theorem clearOne_settings (ty : TypeId) (id : RecordId) (y : XTenant) (c : Cell) :
    (XTenant.clearOne ty id y c).core.settingDefs = y.core.settingDefs ∧
      (XTenant.clearOne ty id y c).core.overrides = y.core.overrides ∧
      (XTenant.clearOne ty id y c).core.settingRevs = y.core.settingRevs := by
  unfold XTenant.clearOne
  rcases y with ⟨core, ux, cx, rx⟩
  dsimp only
  split <;> (try split) <;> (try split) <;> simp [XTenant.reindex, ITenant.modifyType]

theorem foldl_clearOne_settings (ty : TypeId) (id : RecordId) (ds : List Cell) :
    ∀ y : XTenant, (ds.foldl (XTenant.clearOne ty id) y).core.settingDefs = y.core.settingDefs ∧
      (ds.foldl (XTenant.clearOne ty id) y).core.overrides = y.core.overrides ∧
      (ds.foldl (XTenant.clearOne ty id) y).core.settingRevs = y.core.settingRevs := by
  induction ds with
  | nil => intro y; exact ⟨rfl, rfl, rfl⟩
  | cons c ds ih =>
    intro y
    simp only [List.foldl_cons]
    obtain ⟨h1, h2, h3⟩ := ih (XTenant.clearOne ty id y c)
    obtain ⟨g1, g2, g3⟩ := clearOne_settings ty id y c
    exact ⟨h1.trans g1, h2.trans g2, h3.trans g3⟩


theorem clearAll_cleared (ty : TypeId) (id : RecordId) (ds : List Cell) :
    ∀ (S : Cell → Prop) (y0 y : XTenant), ClearedFrom ty id S y0 y →
      ClearedFrom ty id (fun c => S c ∨ c ∈ ds) y0 (ds.foldl (XTenant.clearOne ty id) y) := by
  induction ds with
  | nil => intro S y0 y h; simpa using h
  | cons c ds ih =>
    intro S y0 y h
    have h1 := clearOne_cleared ty id S y0 y c h
    have h2 := ih _ y0 _ h1
    simp only [List.foldl_cons]
    refine ClearedFrom.congr (fun c' => ?_) h2
    simp only [List.mem_cons]
    constructor
    · rintro ((hS | hc) | hd)
      · exact Or.inl hS
      · exact Or.inr (Or.inl hc)
      · exact Or.inr (Or.inr hd)
    · rintro (hS | hc | hd)
      · exact Or.inl (Or.inl hS)
      · exact Or.inl (Or.inr hc)
      · exact Or.inr hd

theorem getRec_map (g : Record → Record) (a : Array (Option Record)) (i : Nat) :
    getRec (a.map (Option.map g)) i = (getRec a i).map g := by
  unfold getRec
  rw [Array.getElem?_map]
  cases a[i]? with
  | none => rfl
  | some o => cases o <;> rfl

theorem array_ext_getRec (a b : Array (Option Record)) (hs : a.size = b.size)
    (h : ∀ i, getRec a i = getRec b i) : a = b := by
  apply Array.ext hs
  intro i h1 h2
  have := h i
  rwa [getRec_of_lt _ _ h1, getRec_of_lt _ _ h2] at this

theorem list_ext_lookup (l1 l2 : List (TypeId × IObj)) (hnd : (l1.map Prod.fst).Nodup)
    (hk : l1.map Prod.fst = l2.map Prod.fst) (hl : ∀ T, l1.lookup T = l2.lookup T) : l1 = l2 := by
  induction l1 generalizing l2 with
  | nil => cases l2 with
    | nil => rfl
    | cons e es => simp at hk
  | cons e es ih =>
    obtain ⟨k, v⟩ := e
    cases l2 with
    | nil => simp at hk
    | cons e2 fs =>
      obtain ⟨k2, v2⟩ := e2
      simp only [List.map_cons, List.cons.injEq, List.nodup_cons] at hk hnd
      obtain ⟨hk1, hk2⟩ := hk
      subst hk1
      have hv : v = v2 := by
        have := hl k
        rw [lookup_cons_eq, lookup_cons_eq] at this
        exact Option.some.inj this
      subst hv
      have : es = fs := by
        apply ih fs hnd.2 hk2
        intro T
        by_cases hT : T = k
        · subst hT
          have h1 : es.lookup T = none := by
            cases hl' : es.lookup T with
            | none => rfl
            | some w => exact absurd (List.mem_map.mpr ⟨(T, w), mem_of_lookup _ _ _ hl', rfl⟩) hnd.1
          have h2 : fs.lookup T = none := by
            cases hl' : fs.lookup T with
            | none => rfl
            | some w =>
              exact absurd (hk2 ▸ List.mem_map.mpr ⟨(T, w), mem_of_lookup _ _ _ hl', rfl⟩) hnd.1
          rw [h1, h2]
        · have hb : (T == k) = false := by simp [hT]
          have := hl T
          rwa [lookup_cons_ne _ _ _ _ hb, lookup_cons_ne _ _ _ _ hb] at this
      rw [this]

theorem IObj.ext' (a b : IObj) (h1 : a.schema = b.schema) (h2 : a.uniques = b.uniques)
    (h3 : a.slots = b.slots) (h4 : a.constraints = b.constraints)
    (h5 : a.workflows = b.workflows) : a = b := by
  cases a; cases b; simp_all

theorem ITenant.ext' (a b : ITenant) (h1 : a.types = b.types) (h2 : a.members = b.members)
    (h3 : a.grants = b.grants) (h4 : a.policies = b.policies) (h5 : a.nextId = b.nextId)
    (h6 : a.limits = b.limits) (h7 : a.roles = b.roles) (h8 : a.teams = b.teams)
    (h9 : a.invitations = b.invitations) (h10 : a.revision = b.revision)
    (h11 : a.eventSeq = b.eventSeq) (h12 : a.schemaVersion = b.schemaVersion)
    (h13 : a.settingDefs = b.settingDefs) (h14 : a.overrides = b.overrides)
    (h15 : a.settingRevs = b.settingRevs) : a = b := by
  cases a; cases b; simp_all

theorem getRec_delObjI (ty : TypeId) (id : RecordId) (j : TypeId) (o : IObj) (i : RecordId)
    (hlt : j = ty → id < o.slots.size) :
    getRec (delObjI ty id j o).slots i =
      if j = ty ∧ i = id then none else (getRec o.slots i).map (clearRef o.schema ty id) := by
  unfold delObjI
  by_cases hj : j = ty
  · have hb : (j == ty) = true := by simp [hj]
    simp only [hb, if_true]
    rw [getRec_setSlot _ _ _ (by simp [hlt hj])]
    rw [getRec_map]
    by_cases hi : i = id <;> simp [hi, hj]
  · have hb : (j == ty) = false := by simp [hj]
    simp only [hb, Bool.false_eq_true, if_false]
    rw [getRec_map]
    simp [hj]

theorem deleteX_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (ty : TypeId) (id : RecordId) (o : IObj) (old : Record)
    (hty : core.typ? ty = some o) (hg : getRec o.slots id = some old) :
    (XTenant.deleteX core ux cx rx ty id o.schema o.uniques old).Ok ∧
    (XTenant.deleteX core ux cx rx ty id o.schema o.uniques old).core =
      { core with types := core.types.map (fun e => (e.1, delObjI ty id e.1 e.2)) } := by
  have hok' := hok
  obtain ⟨hinv, hnd, hu, hc, hr⟩ := hok'
  dsimp only at hinv hnd hu hc hr
  have hlt := lt_of_getRec _ _ _ hg
  have hsz := hinv ty o hty
  have hmod := ITenant.modifyType_eq core ty
    (fun o' => { o' with slots := setSlot o'.slots id none }) o hty
  have hch1 : CellChange core (core.putType ty { o with slots := setSlot o.slots id none }) ty id
      none := by
    refine cc_putType core ty id none o _ hty rfl rfl ?_ ?_
    · intro i hi; rw [getRec_setSlot _ _ _ hlt]; simp [hi]
    · rw [getRec_setSlot _ _ _ hlt]; simp
  have hok1 := XTenant.ok_reindex ⟨core, ux, cx, rx⟩
    (core.putType ty { o with slots := setSlot o.slots id none }) ty id o (some old) none hok hch1
    hty hg (inv_putType core ty _ core.nextId (Nat.le_refl _) (by simp [size_setSlot, hsz]) hinv)
  simp only [XTenant.deleteX]
  rw [hmod]
  generalize hx1 : XTenant.reindex (core.putType ty { o with slots := setSlot o.slots id none })
    ux cx rx ty id o.schema o.uniques (some old) none = x1 at hok1 ⊢
  have hcl := clearAll_cleared ty id (x1.ridx.get (ty, id)) (fun _ => False) x1 x1
    (ClearedFrom.refl ty id x1 hok1)
  generalize hy : (x1.ridx.get (ty, id)).foldl (XTenant.clearOne ty id) x1 = y at hcl ⊢
  have hx1core : x1.core = core.putType ty { o with slots := setSlot o.slots id none } := by
    rw [← hx1]; rfl
  refine ⟨hcl.ok, ?_⟩
  -- every record that refers to `(ty, id)` is among the listed cells
  have hcomplete : ∀ T o1 i r, x1.core.typ? T = some o1 → getRec o1.slots i = some r →
      (T, i) ∉ x1.ridx.get (ty, id) → clearRef o1.schema ty id r = r := by
    intro T o1 i r hT hgi hn
    have hinv1 := hok1.2.2.2.2
    rw [hinv1] at hn
    unfold ITenant.rkeys ITenant.cellRec at hn
    simp only [hT, hgi, Option.map_some] at hn
    rw [mem_rkeysOf] at hn
    have : recRefers o1.schema ty id r = false := by simpa using hn
    unfold clearRef
    simp [this]
  apply ITenant.ext'
  · -- the object types
    apply list_ext_lookup
    · rw [hcl.keys, hx1core]
      show ((putTypeI core.types ty _).map Prod.fst).Nodup
      rw [putTypeI_keys]; exact hnd
    · rw [hcl.keys, hx1core, List.map_map]
      show (putTypeI core.types ty _).map Prod.fst = _
      rw [putTypeI_keys]
      simp [Function.comp]
    · intro T
      have hl := lookup_map_entry core.types (fun j o => delObjI ty id j o) T
      show y.core.typ? T = _
      rw [hl]
      change y.core.typ? T = (core.typ? T).map (delObjI ty id T)
      cases hct : core.typ? T with
      | none =>
        have h1 := hcl.isSome T
        have h2 : (x1.core.typ? T).isSome = false := by
          rw [hx1core, typ?_putType]
          by_cases hT : (T == ty) = true
          · have : T = ty := by simpa using hT
            subst this; simp [hct] at hty
          · simp [hT, hct]
        rw [h2] at h1
        cases hyT : y.core.typ? T with
        | none => rfl
        | some w => simp [hyT] at h1
      | some oc =>
        -- the object of `T` before the fold
        have hx1T : ∃ o0, x1.core.typ? T = some o0 ∧ o0.schema = oc.schema ∧
            o0.uniques = oc.uniques ∧ o0.constraints = oc.constraints ∧ o0.workflows = oc.workflows ∧ o0.slots.size = oc.slots.size ∧
            ∀ i, getRec o0.slots i = if T = ty ∧ i = id then none else getRec oc.slots i := by
          by_cases hT : T = ty
          · subst hT
            have hoo : oc = o := by rw [hct] at hty; exact Option.some.inj hty
            subst hoo
            refine ⟨{ oc with slots := setSlot oc.slots id none }, ?_, rfl, rfl, rfl, rfl, ?_, ?_⟩
            · rw [hx1core]; exact typ?_putType_same _ _ _ _ hct
            · simp [size_setSlot]
            · intro i
              show getRec (setSlot oc.slots id none) i = _
              rw [getRec_setSlot _ _ _ hlt]
              by_cases hi : i = id <;> simp [hi]
          · refine ⟨oc, ?_, rfl, rfl, rfl, rfl, rfl, ?_⟩
            · rw [hx1core, typ?_putType_other _ _ _ _ hT]; exact hct
            · intro i; simp [hT]
        obtain ⟨o0, hT0, hs0, hu0, hcn0, hw0, hsz0, hget0⟩ := hx1T
        obtain ⟨oy, hTy, hs, hu', hcn, hw', hsz', hi⟩ := hcl.typ T o0 hT0
        rw [hTy, Option.map_some]
        congr 1
        have hlt' : T = ty → id < oc.slots.size := by
          intro hT; subst hT
          have : oc = o := by rw [hct] at hty; exact Option.some.inj hty
          subst this; exact hlt
        apply IObj.ext'
        · rw [hs, hs0]; rfl
        · rw [hu', hu0]; rfl
        · apply array_ext_getRec
          · rw [hsz', hsz0]
            unfold delObjI
            split <;> simp [size_setSlot]
          · intro i
            rw [getRec_delObjI ty id T oc i hlt']
            have hmap : getRec oy.slots i = (getRec o0.slots i).map (clearRef oc.schema ty id) := by
              by_cases hmem : (T, i) ∈ x1.ridx.get (ty, id)
              · have := ((hi i).1 (Or.inr hmem))
                rw [this, hs0]
              · rw [(hi i).2 (fun hh => hh.elim (fun f => f) (fun m => hmem m))]
                cases hgi : getRec o0.slots i with
                | none => rfl
                | some r =>
                  rw [Option.map_some, ← hs0, hcomplete T o0 i r hT0 hgi hmem]
            rw [hmap, hget0 i]
            by_cases hh : T = ty ∧ i = id
            · simp [hh]
            · simp [hh]
        · rw [hcn, hcn0]; rfl
        · rw [hw', hw0]; rfl
  · exact hcl.members.trans (by rw [hx1core]; rfl)
  · exact hcl.grants.trans (by rw [hx1core]; rfl)
  · exact hcl.policies.trans (by rw [hx1core]; rfl)
  · exact hcl.nextId.trans (by rw [hx1core]; rfl)
  · exact hcl.limits.trans (by rw [hx1core]; rfl)
  · exact hcl.roles.trans (by rw [hx1core]; rfl)
  · exact hcl.teams.trans (by rw [hx1core]; rfl)
  · exact hcl.invitations.trans (by rw [hx1core]; rfl)
  · rw [← hy, (foldl_clearOne_revision ty id _ x1).1, hx1core]; rfl
  · rw [← hy, (foldl_clearOne_revision ty id _ x1).2.1, hx1core]; rfl
  · rw [← hy, (foldl_clearOne_revision ty id _ x1).2.2, hx1core]; rfl
  · rw [← hy, (foldl_clearOne_settings ty id _ x1).1, hx1core]; rfl
  · rw [← hy, (foldl_clearOne_settings ty id _ x1).2.1, hx1core]; rfl
  · rw [← hy, (foldl_clearOne_settings ty id _ x1).2.2, hx1core]; rfl

theorem delete_good (core : ITenant) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (rx : MM Cell)
    (hok : XTenant.Ok ⟨core, ux, cx, rx⟩) (p : Principal) (ty : TypeId) (id : RecordId)
    (ev : Option Nat) :
    Good ⟨core, ux, cx, rx⟩ (stepParts core ux cx rx p (.delete ty id ev))
      (stepTenantI core p (.delete ty id ev)) := by
  simp only [stepParts, stepTenantI]
  cases hty : core.typ? ty with
  | none => exact good_error _ _
  | some o =>
    simp only [hty]
    cases hg : getRec o.slots id with
    | none => exact good_error _ _
    | some old =>
      simp only [hg]
      by_cases hca : canDeleteI core p ty old = true
      · simp only [hca, Bool.not_true, if_false, Bool.false_eq_true]
        by_cases hv : versionClash ev old = true
        · simp only [hv, if_true]
          exact good_error _ _
        · simp only [hv, if_false, Bool.false_eq_true]
          rw [blockedX_eq core ux cx rx hok ty id]
          by_cases hb : blockedI core ty id = true
          · simp only [hb, if_true]
            exact good_error _ _
          · simp only [hb, if_false, Bool.false_eq_true]
            by_cases hcb : clearBreaksI core ty id = true
            · simp only [hcb, if_true]
              exact good_error _ _
            · simp only [hcb, if_false, Bool.false_eq_true]
              obtain ⟨hok2, hcore⟩ := deleteX_good core ux cx rx hok ty id o old hty hg
              refine ⟨?_, fun e x' h => (by cases h), ?_⟩
              · simp only [Except.mapError, Except.map]
                rw [hcore]
              · intro y hy
                simp only [Except.ok.injEq] at hy
                subst hy
                exact hok2
      · have : canDeleteI core p ty old = false := by simpa using hca
        simp only [this, Bool.not_false, if_true]
        exact good_error _ _

end Impl
end Strata
