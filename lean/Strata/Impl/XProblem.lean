/-
The executable kernel, stage 2: the write checks by index lookup.

`problemX` is `problemI` (stage 1, `Strata/Impl/Problem.lean`) with the two scans of the type's slots
replaced by lookups: a unique value's clash is a lookup in the unique index (`clashX`), and a
composite constraint's clash a lookup in the composite index (`compClashX`). The reference check was
already a slot lookup per reference. Everything else is the spec's function, called unchanged.

`problemX_eq` says `problemX` and `problemI` agree on every tenant whose indexes are consistent
(`XTenant.Ok`), for every object type and every record. The agreement is for the schema entries and the
constraints of the type the write is checked against, which is the only place the checks are called.
-/

import Strata.Impl.XTenant

namespace Strata
namespace Impl

/-- A unique value clashes: it is keyed, and a cell other than `self` lists it. -/
def clashX (ux : MM (TypeId × FieldId × Val)) (S : TypeId) (self : RecordId) (f : FieldId)
    (v : Val) : Bool :=
  v.keyed && (ux.get (S, f, v)).any (fun c => c.2 != self)

/-- A composite constraint clashes: the record holds all its fields keyed, and a cell other than
    `self` lists those values. -/
def compClashX (cx : MM (TypeId × List FieldId × List Val)) (S : TypeId) (self : RecordId)
    (fs : Fields) (u : List FieldId) : Bool :=
  u.all (fun f => (fs.get f).keyed) && (cx.get (S, u, u.map fs.get)).any (fun c => c.2 != self)

/-- `valueProblemI` with the clash from the unique index. -/
def valueProblemX (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (ux : MM (TypeId × FieldId × Val)) (S : TypeId) (self : RecordId) (f : FieldId)
    (d : FieldDef) (v : Val) : Option Err :=
  if v.overLimit L then some .limitExceeded
  else if d.unique && decide (L.maxUnique < v.size) then some .limitExceeded
  else if !v.hasType d.ty then some .invalid
  else if uniq && d.unique && clashX ux S self f v then some .uniqueViolated
  else if !Val.resolvesI E d.ty v then some .danglingReference
  else none

/-- `fieldProblemI` with the clash from the unique index. -/
def fieldProblemX (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (ux : MM (TypeId × FieldId × Val)) (S : TypeId) (self : RecordId) (fs : Fields)
    (fd : FieldId × FieldDef) : Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblemX L uniq E ux S self fd.1 fd.2)

/-- `compositeProblemI` with the clash from the composite index. -/
def compositeProblemX (uniq : Bool) (cx : MM (TypeId × List FieldId × List Val)) (S : TypeId)
    (self : RecordId) (fs : Fields) (us : List (List FieldId)) : Option Err :=
  if uniq && us.any (compClashX cx S self fs) then some .uniqueViolated else none

/-- `problemI` with the clashes from the indexes. -/
def problemX (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (E : TypeId → Array (Option Record)) (ux : MM (TypeId × FieldId × Val))
    (cx : MM (TypeId × List FieldId × List Val)) (S : TypeId) (self : RecordId) (r : Record) :
    Option Err :=
  if decide (L.maxRecord < r.size) then some .limitExceeded
  else if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    match schema.findSome? (fieldProblemX L uniq E ux S self r.fields) with
    | some e => some e
    | none => compositeProblemX uniq cx S self r.fields us
  else some .invalid

/-! ## Agreement with the scans -/

theorem findSome_congr {α β : Type} (f g : α → Option β) (l : List α)
    (h : ∀ x ∈ l, f x = g x) : l.findSome? f = l.findSome? g := by
  induction l with
  | nil => rfl
  | cons x xs ih =>
    simp only [List.findSome?_cons]
    rw [h x (List.mem_cons_self), ih (fun y hy => h y (List.mem_cons_of_mem _ hy))]

theorem any_congr {α : Type} (f g : α → Bool) (l : List α) (h : ∀ x ∈ l, f x = g x) :
    l.any f = l.any g := by
  induction l with
  | nil => rfl
  | cons x xs ih =>
    simp only [List.any_cons]
    rw [h x (List.mem_cons_self), ih (fun y hy => h y (List.mem_cons_of_mem _ hy))]

/-- The unique index answers the clash question: for a field the schema declares unique and a keyed
    value, some other record of the type holds the value iff the index lists another cell. -/
theorem clashX_eq (x : XTenant) (hu : x.uidx.Inv x.core.ukeys) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (self : RecordId) (f : FieldId) (d : FieldDef)
    (hmem : (f, d) ∈ o.schema) (hd : d.unique = true) (v : Val) :
    clashAtI o.slots self f v = clashX x.uidx S self f v := by
  unfold clashAtI clashX
  by_cases hk : v.keyed = true
  · simp only [hk, Bool.true_and]
    rw [Bool.eq_iff_iff, anyFrom_iff, List.any_eq_true]
    constructor
    · rintro ⟨i, r, hi, hr, hp⟩
      have hg : getRec o.slots i = some r := by unfold getRec; simp [hr]
      simp only [Bool.and_eq_true, bne_iff_ne, ne_eq, beq_iff_eq] at hp
      refine ⟨(S, i), ?_, by simpa using hp.1⟩
      rw [hu]
      unfold ITenant.ukeys ITenant.cellRec
      simp only [ho, hg, Option.map_some]
      rw [mem_ukeysOf]
      refine ⟨rfl, (f, d), hmem, rfl, hd, ?_, hp.2⟩
      simp only [hp.2]; exact hk
    · rintro ⟨c, hc, hne⟩
      rw [hu] at hc
      unfold ITenant.ukeys ITenant.cellRec at hc
      cases hT : x.core.typ? c.1 with
      | none => simp [hT] at hc
      | some o' =>
        cases hg : getRec o'.slots c.2 with
        | none => simp [hT, hg] at hc
        | some r =>
          simp only [hT, hg, Option.map_some] at hc
          rw [mem_ukeysOf] at hc
          obtain ⟨h1, fd, hfd, h2, h3, h4, h5⟩ := hc
          simp only at h1 h2 h5
          subst h1
          rw [hT] at ho
          cases ho
          have hr : o.slots[c.2]? = some (some r) := getRec_some _ _ _ hg
          refine ⟨c.2, r, ?_, hr, ?_⟩
          · exact (Array.getElem?_eq_some_iff.mp hr).1
          · simp only [Bool.and_eq_true, bne_iff_ne, ne_eq, beq_iff_eq]
            exact ⟨by simpa using hne, by first | exact h5 | (rw [← h2]; exact h5) | (rw [h2]; exact h5)⟩
  · have : v.keyed = false := by simpa using hk
    simp [this]

theorem compClashX_eq (x : XTenant) (hc : x.cidx.Inv x.core.ckeys) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (self : RecordId) (fs : Fields) (u : List FieldId)
    (hu : u ∈ o.uniques) :
    anyFrom o.slots (fun q => q.1 != self && agreeOn u fs q.2.fields) o.slots.size =
      compClashX x.cidx S self fs u := by
  unfold compClashX
  by_cases hk : u.all (fun f => (fs.get f).keyed) = true
  · simp only [hk, Bool.true_and]
    rw [Bool.eq_iff_iff, anyFrom_iff, List.any_eq_true]
    have hagree : ∀ g : Fields, agreeOn u fs g = true ↔ u.map fs.get = u.map g.get := by
      intro g
      unfold agreeOn
      rw [List.map_inj_left]
      simp only [List.all_eq_true, Bool.and_eq_true, beq_iff_eq] at hk ⊢
      constructor
      · intro h f hf; exact (h f hf).2
      · intro h f hf; exact ⟨hk f hf, h f hf⟩
    constructor
    · rintro ⟨i, r, hi, hr, hp⟩
      have hg : getRec o.slots i = some r := by unfold getRec; simp [hr]
      simp only [Bool.and_eq_true, bne_iff_ne, ne_eq] at hp
      refine ⟨(S, i), ?_, by simpa using hp.1⟩
      rw [hc]
      unfold ITenant.ckeys ITenant.cellRec
      simp only [ho, hg, Option.map_some]
      rw [mem_ckeysOf]
      refine ⟨rfl, hu, ?_, ?_⟩
      · have := (hagree r.fields).mp hp.2
        simp only [List.all_eq_true] at hk ⊢
        intro f hf
        have h2 : fs.get f = r.fields.get f := by
          have := congrArg (fun l => l) this
          exact (List.map_inj_left.mp this) f hf
        rw [← h2]; exact hk f hf
      · exact (hagree r.fields).mp hp.2
    · rintro ⟨c, hcell, hne⟩
      rw [hc] at hcell
      unfold ITenant.ckeys ITenant.cellRec at hcell
      cases hT : x.core.typ? c.1 with
      | none => simp [hT] at hcell
      | some o' =>
        cases hg : getRec o'.slots c.2 with
        | none => simp [hT, hg] at hcell
        | some r =>
          simp only [hT, hg, Option.map_some] at hcell
          rw [mem_ckeysOf] at hcell
          obtain ⟨h1, h2, h3, h4⟩ := hcell
          simp only at h1 h2 h3 h4
          subst h1
          rw [hT] at ho
          cases ho
          have hr : o.slots[c.2]? = some (some r) := getRec_some _ _ _ hg
          refine ⟨c.2, r, ?_, hr, ?_⟩
          · exact (Array.getElem?_eq_some_iff.mp hr).1
          · simp only [Bool.and_eq_true, bne_iff_ne, ne_eq]
            refine ⟨by simpa using hne, (hagree r.fields).mpr ?_⟩
            exact h4
  · have : u.all (fun f => (fs.get f).keyed) = false := by simpa using hk
    simp only [this, Bool.false_and]
    rw [Bool.eq_false_iff]
    intro h
    obtain ⟨i, r, _, _, hp⟩ := (anyFrom_iff _ _ _).mp h
    simp only [Bool.and_eq_true] at hp
    have := hp.2
    unfold agreeOn at this
    simp only [List.all_eq_true, Bool.and_eq_true] at this
    simp only [List.all_eq_true] at hk this
    exact hk (fun f hf => (this f hf).1)

theorem valueProblemX_eq (x : XTenant) (hu : x.uidx.Inv x.core.ukeys) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (self : RecordId) (f : FieldId) (d : FieldDef) (hmem : (f, d) ∈ o.schema) (v : Val) :
    valueProblemX L uniq E x.uidx S self f d v = valueProblemI L uniq E o.slots self f d v := by
  unfold valueProblemX valueProblemI
  by_cases hd : d.unique = true
  · rw [clashX_eq x hu S o ho self f d hmem hd v]
  · have : d.unique = false := by simpa using hd
    simp [this]

theorem fieldProblemX_eq (x : XTenant) (hu : x.uidx.Inv x.core.ukeys) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (self : RecordId) (fs : Fields) (fd : FieldId × FieldDef) (hmem : fd ∈ o.schema) :
    fieldProblemX L uniq E x.uidx S self fs fd = fieldProblemI L uniq E o.slots self fs fd := by
  unfold fieldProblemX fieldProblemI
  have : valueProblemX L uniq E x.uidx S self fd.1 fd.2 = valueProblemI L uniq E o.slots self fd.1 fd.2 :=
    funext (valueProblemX_eq x hu S o ho L uniq E self fd.1 fd.2 hmem)
  rw [this]

theorem compositeProblemX_eq (x : XTenant) (hc : x.cidx.Inv x.core.ckeys) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (uniq : Bool) (self : RecordId) (fs : Fields) :
    compositeProblemX uniq x.cidx S self fs o.uniques =
      compositeProblemI uniq o.slots self fs o.uniques := by
  unfold compositeProblemX compositeProblemI
  rw [any_congr (compClashX x.cidx S self fs)
    (fun u => anyFrom o.slots (fun q => q.1 != self && agreeOn u fs q.2.fields) o.slots.size)
    o.uniques (fun u hu => (compClashX_eq x hc S o ho self fs u hu).symm)]

/-- On a tenant with consistent indexes the indexed write check is the stage 1 one, for the
    schema and the constraints of the type it is checked against. -/
theorem problemX_eq (x : XTenant) (hok : x.Ok) (S : TypeId) (o : IObj)
    (ho : x.core.typ? S = some o) (L : Limits) (uniq : Bool) (self : RecordId) (r : Record) :
    problemX L o.schema o.uniques uniq x.core.slotsOf x.uidx x.cidx S self r =
      problemI L o.schema o.uniques uniq x.core.slotsOf o.slots self r := by
  obtain ⟨_, _, hu, hc, _⟩ := hok
  unfold problemX problemI
  rw [findSome_congr (fieldProblemX L uniq x.core.slotsOf x.uidx S self r.fields)
    (fieldProblemI L uniq x.core.slotsOf o.slots self r.fields) o.schema
    (fun fd hfd => fieldProblemX_eq x hu S o ho L uniq _ self r.fields fd hfd),
    compositeProblemX_eq x hc S o ho uniq self r.fields]
  rfl

end Impl
end Strata
