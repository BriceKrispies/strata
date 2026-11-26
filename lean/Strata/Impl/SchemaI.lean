/-
The executable kernel, stage 1: the schema change in general (slice 11).

`changeSchemaI` is the spec's `changeSchemaT` (`Strata/SchemaChange.lean`) on slots. The changes apply to the object
types as the spec's do (a field change touches the schema alone, so the slots stay where they are); the validity
rules, the impact rule and the violation check are the spec's functions, taken on the abstraction of the object types
(as stage 1's other schema changes take `schemaChangeRefused`); the records of the touched types are rewritten
slot by slot. `changeSchemaI_refines` says the result, read through the abstraction, is the spec's.
-/

import Strata.Impl.Tenant
import Strata.SchemaChangeProofs

namespace Strata
namespace Impl

/-- One change on the object types of an implementation tenant (the spec's `applyChange`). -/
def applyChangeI (types : List (TypeId × IObj)) : Change → Except Err (List (TypeId × IObj))
  | .defineField ty f d => match types.lookup ty with
    | none => .error .invalidSchema
    | some o => .ok (putTypeI types ty { o with schema := setField o.schema f d })
  | .removeField ty f => match types.lookup ty with
    | none => .error .invalidSchema
    | some o =>
      if o.schema.any (fun fd => fd.1 == f) then
        .ok (putTypeI types ty { o with schema := o.schema.filter (fun fd => !(fd.1 == f)) })
      else .error .invalidSchema
  | .defineType ty =>
    if types.any (fun e => e.1 == ty) then .ok types
    else .ok (types ++ [(ty, { schema := [] })])
  | .removeType ty _ => match types.lookup ty with
    | none => .error .invalidSchema
    | some _ => .ok (types.filter (fun e => !(e.1 == ty)))

/-- The changes in order, then the validity predicate on the abstraction (the spec's `applyChanges`). -/
def applyChangesI (valid : List (TypeId × ObjType) → Bool) (types : List (TypeId × IObj)) :
    List Change → Except Err (List (TypeId × IObj))
  | [] => if valid (types.map absEntry) then .ok types else .error .invalidSchema
  | c :: cs => (applyChangeI types c) >>= fun types' => applyChangesI valid types' cs

/-- The records of the touched types that existed, rewritten slot by slot (the spec's `rewriteTypes`). -/
def rewriteTypesI (orig : List (TypeId × IObj)) (touched : List TypeId) (types : List (TypeId × IObj)) :
    List (TypeId × IObj) :=
  types.map (fun e =>
    if touched.contains e.1 && orig.any (fun e0 => e0.1 == e.1) then
      (e.1, { e.2 with slots := e.2.slots.map (Option.map (stripRecord e.2.schema)) })
    else e)

/-- A schema change of an implementation tenant: the spec's `changeSchemaT` on slots. `canMgr` is the permission to
    manage the schema, taken by the caller on the tenant's header. The schema version is left to the commit. -/
def changeSchemaI (valid : List (TypeId × ObjType) → Bool) (canMgr : Bool) (t : ITenant) (strict : Bool)
    (expected : Option Nat) (cs : List Change) : Except Err ITenant :=
  if !canMgr then .error .unauthorized
  else if expected.any (fun v => v != t.schemaVersion) then .error .schemaConflict
  else if cs.isEmpty then .error .invalidIntent
  else if maxChanges < cs.length then .error .limitExceeded
  else
    match applyChangesI valid t.types cs with
    | .error e => .error e
    | .ok types1 =>
      if impactRefused t.policies (t.types.map absEntry) (types1.map absEntry) cs then .error .invalidSchema
      else
        let types2 := rewriteTypesI t.types (cs.flatMap Change.touched) types1
        if changeRefused t.limits (changesScope (types2.map absEntry) cs) (recordsIn (types2.map absEntry))
            strict then .error .migrationRejected
        else if removalsRefused (t.types.map absEntry) cs then .error .migrationRejected
        else .ok { t with types := types2 }

/-! ## Refinement -/

theorem ascend_map_opt (g : Record → Record) (L : List (Option Record)) (b : Nat) :
    ascend b (L.map (Option.map g)) = (ascend b L).map (fun q => (q.1, g q.2)) := by
  induction L generalizing b with
  | nil => simp [ascend]
  | cons o os ih =>
    simp only [List.map_cons, ascend, ih, List.map_append]
    congr 1
    cases o <;> simp [cellOf]

theorem absSlots_map_opt (g : Record → Record) (a : Array (Option Record)) :
    absSlots (a.map (Option.map g)) = (absSlots a).map (fun q => (q.1, g q.2)) := by
  unfold absSlots
  rw [Array.toList_map]
  exact ascend_map_opt g a.toList 0

theorem any_map_absEntry (l : List (TypeId × IObj)) (x : TypeId) :
    (l.map absEntry).any (fun e0 => e0.1 == x) = l.any (fun e0 => e0.1 == x) := by
  rw [List.any_map]; rfl

theorem abs_policies' (t : ITenant) : t.abs.policies = t.policies := rfl
theorem abs_limits' (t : ITenant) : t.abs.limits = t.limits := rfl
theorem abs_schemaVersion' (t : ITenant) : t.abs.schemaVersion = t.schemaVersion := rfl

theorem lookup_map_absEntry' (l : List (TypeId × IObj)) (ty : TypeId) :
    (l.map absEntry).lookup ty = (l.lookup ty).map IObj.abs := lookup_map_abs l ty

theorem applyChangeI_abs (types : List (TypeId × IObj)) (c : Change) :
    (applyChangeI types c).map (List.map absEntry) = applyChange (types.map absEntry) c := by
  cases c with
  | defineField ty f d =>
    simp only [applyChangeI, applyChange, lookup_map_absEntry']
    cases types.lookup ty with
    | none => rfl
    | some o =>
      simp only [Option.map_some, Except.map]
      rw [putTypeI_abs]
      rfl
  | removeField ty f =>
    simp only [applyChangeI, applyChange, lookup_map_absEntry']
    cases types.lookup ty with
    | none => rfl
    | some o =>
      simp only [Option.map_some]
      by_cases h : o.schema.any (fun fd => fd.1 == f) = true
      · simp only [IObj.abs, h, if_true, Except.map]
        rw [putTypeI_abs]
        rfl
      · simp [IObj.abs, h, Except.map]
  | defineType ty =>
    simp only [applyChangeI, applyChange, any_map_absEntry]
    by_cases h : types.any (fun e => e.1 == ty) = true
    · simp only [h, if_true, Except.map]
    · simp only [h, Bool.false_eq_true, if_false, Except.map, List.map_append, List.map_cons, List.map_nil]
      rfl
  | removeType ty del =>
    simp only [applyChangeI, applyChange, lookup_map_absEntry']
    cases types.lookup ty with
    | none => rfl
    | some o =>
      simp only [Option.map_some, Except.map]
      rw [List.filter_map]
      rfl

theorem applyChangesI_abs (valid : List (TypeId × ObjType) → Bool) (cs : List Change) :
    ∀ (types : List (TypeId × IObj)),
      (applyChangesI valid types cs).map (List.map absEntry) = applyChanges valid (types.map absEntry) cs := by
  induction cs with
  | nil =>
    intro types
    simp only [applyChangesI, applyChanges]
    split <;> rfl
  | cons c cs ih =>
    intro types
    simp only [applyChangesI, applyChanges, bind, Except.bind]
    have h := applyChangeI_abs types c
    cases h1 : applyChangeI types c with
    | error e =>
      rw [h1] at h
      simp only [Except.map] at h
      rw [← h]; rfl
    | ok t1 =>
      rw [h1] at h
      simp only [Except.map, Except.ok.injEq] at h
      rw [← h]
      exact ih t1

theorem rewriteTypesI_abs (orig : List (TypeId × IObj)) (touched : List TypeId)
    (types : List (TypeId × IObj)) :
    (rewriteTypesI orig touched types).map absEntry =
      rewriteTypes (orig.map absEntry) touched (types.map absEntry) := by
  unfold rewriteTypesI rewriteTypes
  rw [List.map_map, List.map_map]
  apply List.map_congr_left
  intro e _
  simp only [Function.comp, absEntry, any_map_absEntry]
  by_cases h : (touched.contains e.1 && orig.any (fun e0 => e0.1 == e.1)) = true
  · simp only [h, if_true, IObj.abs]
    rw [absSlots_map_opt]
  · simp only [h, Bool.false_eq_true, if_false]

theorem abs_types' (t : ITenant) : t.abs.types = t.types.map absEntry := rfl

/-- The tenant the spec's schema change leaves, as the abstraction of the implementation's, up to the schema version,
    which the commit sets. -/
theorem changeSchemaI_refines (valid : List (TypeId × ObjType) → Bool) (t : ITenant) (p : Principal)
    (strict : Bool) (ev : Option Nat) (cs : List Change) (canMgr : Bool) (hm : canMgr = canManage t.abs p) :
    (changeSchemaI valid canMgr t strict ev cs).map ITenant.abs =
      (changeSchemaT valid t.abs p strict ev cs).map (fun t' => { t' with schemaVersion := t.abs.schemaVersion }) := by
  subst hm
  have happ := applyChangesI_abs valid cs t.types
  unfold changeSchemaI changeSchemaT
  simp only [abs_policies', abs_limits', abs_schemaVersion', abs_types', rewriteTypesI_abs]
  rw [← happ]
  by_cases h1 : (!canManage t.abs p) = true
  · simp [h1, Except.map]
  · simp only [h1, Bool.false_eq_true, if_false]
    by_cases h2 : ev.any (fun v => v != t.schemaVersion) = true
    · simp [h2, Except.map]
    · simp only [h2, Bool.false_eq_true, if_false]
      by_cases h3 : cs.isEmpty = true
      · simp [h3, Except.map]
      · simp only [h3, Bool.false_eq_true, if_false]
        by_cases h4 : maxChanges < cs.length
        · simp [h4, Except.map]
        · simp only [h4, if_false]
          cases h5 : applyChangesI valid t.types cs with
          | error e => simp [Except.map]
          | ok types1 =>
            simp only [Except.map]
            by_cases h6 : impactRefused t.policies (t.types.map absEntry) (types1.map absEntry) cs = true
            · simp [h6, Except.map]
            · simp only [h6, Bool.false_eq_true, if_false]
              by_cases h7 : changeRefused t.limits (changesScope (rewriteTypes (t.types.map absEntry)
                  (cs.flatMap Change.touched) (types1.map absEntry)) cs)
                  (recordsIn (rewriteTypes (t.types.map absEntry) (cs.flatMap Change.touched)
                    (types1.map absEntry))) strict = true
              · simp [h7, Except.map]
              · simp only [h7, Bool.false_eq_true, if_false]
                by_cases h8 : removalsRefused (t.types.map absEntry) cs = true
                · simp [h8, Except.map]
                · simp only [h8, Bool.false_eq_true, if_false, Except.map]
                  unfold ITenant.abs
                  simp only [rewriteTypesI_abs]


/-! ## The slot invariant -/

theorem lk_filter {α : Type} (l : List (TypeId × α)) (ty k : TypeId) :
    (l.filter (fun e => !(e.1 == ty))).lookup k = if k = ty then none else l.lookup k := by
  induction l with
  | nil => simp [List.lookup]
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    by_cases hj : j = ty
    · subst hj
      by_cases hk : k = j
      · subst hk; simpa [List.filter_cons] using ih
      · have hb : (k == j) = false := by simpa using hk
        simp [List.filter_cons, ih, List.lookup, hb, hk]
    · have hb : (j == ty) = false := by simpa using hj
      by_cases hk : k = ty
      · subst hk
        have hkj : (k == j) = false := by
          have : k ≠ j := fun e => hj e.symm
          simpa using this
        simp [List.filter_cons, hb, List.lookup, hkj, ih]
      · by_cases hkj : k = j
        · subst hkj; simp [List.filter_cons, hb, List.lookup, hk]
        · have hkb : (k == j) = false := by simpa using hkj
          simp [List.filter_cons, hb, List.lookup, hkb, ih, hk]

theorem lk_append_fresh {α : Type} (l : List (TypeId × α)) (ty : TypeId) (o : α) (k : TypeId)
    (hf : l.lookup ty = none) :
    (l ++ [(ty, o)]).lookup k = if k = ty then some o else l.lookup k := by
  induction l with
  | nil =>
    by_cases hk : k = ty
    · subst hk; simp [List.lookup]
    · have : (k == ty) = false := by simpa using hk
      simp [List.lookup, this, hk]
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    have hj : j ≠ ty := by
      intro e; subst e; simp [List.lookup] at hf
    have hf' : es.lookup ty = none := by
      have : (ty == j) = false := by
        have : ty ≠ j := fun e => hj e.symm
        simpa using this
      simpa [List.lookup, this] using hf
    by_cases hk : k = j
    · subst hk
      have : k ≠ ty := hj
      simp [List.lookup, this]
    · have hb : (k == j) = false := by simpa using hk
      simp only [List.cons_append, List.lookup, hb, ih hf']

/-- Every type of the list that a lookup finds has a slot array within `n`. -/
def LkBelow (n : Nat) (l : List (TypeId × IObj)) : Prop := ∀ T o, l.lookup T = some o → o.slots.size ≤ n

theorem lkBelow_putTypeI {n : Nat} {l : List (TypeId × IObj)} (hl : LkBelow n l) (ty : TypeId) (o o' : IObj)
    (ho : l.lookup ty = some o) (hs : o'.slots = o.slots) : LkBelow n (putTypeI l ty o') := by
  intro T q hq
  rw [lookup_putTypeI] at hq
  by_cases hT : (T == ty) = true
  · simp only [hT, if_true] at hq
    split at hq
    · simp only [Option.some.injEq] at hq
      subst hq
      rw [hs]
      exact hl ty o ho
    · cases hq
  · simp only [hT, Bool.false_eq_true, if_false] at hq
    exact hl T q hq

theorem applyChangeI_lk (n : Nat) (types r : List (TypeId × IObj)) (c : Change)
    (hall : LkBelow n types) (h : applyChangeI types c = .ok r) : LkBelow n r := by
  cases c with
  | defineField ty f d =>
    simp only [applyChangeI] at h
    cases hl : types.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl, Except.ok.injEq] at h
      subst h
      exact lkBelow_putTypeI hall ty o _ hl rfl
  | removeField ty f =>
    simp only [applyChangeI] at h
    cases hl : types.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl] at h
      split at h
      · simp only [Except.ok.injEq] at h
        subst h
        exact lkBelow_putTypeI hall ty o _ hl rfl
      · simp at h
  | defineType ty =>
    simp only [applyChangeI] at h
    split at h
    · simp only [Except.ok.injEq] at h
      subst h; exact hall
    · rename_i hany
      simp only [Except.ok.injEq] at h
      subst h
      have hnone : types.lookup ty = none := by
        by_cases hx : types.lookup ty = none
        · exact hx
        · exfalso
          obtain ⟨o, ho⟩ := Option.ne_none_iff_exists'.mp hx
          apply hany
          exact List.any_eq_true.mpr ⟨(ty, o), sc_lookup_mem_gen types ty o ho, by simp⟩
      intro T q hq
      rw [lk_append_fresh types ty _ T hnone] at hq
      by_cases hT : T = ty
      · rw [if_pos hT] at hq
        cases hq
        simp
      · rw [if_neg hT] at hq
        exact hall T q hq
  | removeType ty del =>
    simp only [applyChangeI] at h
    cases hl : types.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl, Except.ok.injEq] at h
      subst h
      intro T q hq
      rw [lk_filter] at hq
      by_cases hT : T = ty
      · rw [if_pos hT] at hq; cases hq
      · rw [if_neg hT] at hq
        exact hall T q hq

theorem applyChangesI_lk (valid : List (TypeId × ObjType) → Bool) (n : Nat) (cs : List Change) :
    ∀ (types r : List (TypeId × IObj)), LkBelow n types →
      applyChangesI valid types cs = .ok r → LkBelow n r := by
  induction cs with
  | nil =>
    intro types r hall h
    simp only [applyChangesI] at h
    split at h
    · simp only [Except.ok.injEq] at h; subst h; exact hall
    · simp at h
  | cons c cs ih =>
    intro types r hall h
    simp only [applyChangesI] at h
    cases hc : applyChangeI types c with
    | error e => rw [hc] at h; cases h
    | ok t1 =>
      rw [hc] at h
      exact ih t1 r (applyChangeI_lk n types t1 c hall hc) h

theorem rewriteTypesI_eq (orig : List (TypeId × IObj)) (touched : List TypeId) (types : List (TypeId × IObj)) :
    rewriteTypesI orig touched types = types.map (fun e => (e.1,
      if touched.contains e.1 && orig.any (fun e0 => e0.1 == e.1) then
        { e.2 with slots := e.2.slots.map (Option.map (stripRecord e.2.schema)) } else e.2)) := by
  unfold rewriteTypesI
  apply List.map_congr_left
  intro e _
  split <;> rfl

theorem lkBelow_rewriteTypesI {n : Nat} (orig : List (TypeId × IObj)) (touched : List TypeId)
    {types : List (TypeId × IObj)} (h : LkBelow n types) : LkBelow n (rewriteTypesI orig touched types) := by
  intro T q hq
  rw [rewriteTypesI_eq] at hq
  have := lookup_map_entry types (fun T o => if touched.contains T && orig.any (fun e0 => e0.1 == T) then
        { o with slots := o.slots.map (Option.map (stripRecord o.schema)) } else o) T
  rw [this] at hq
  cases hl : types.lookup T with
  | none => rw [hl] at hq; simp at hq
  | some o =>
    rw [hl] at hq
    simp only [Option.map_some, Option.some.injEq] at hq
    have ho := h T o hl
    split at hq
    · subst hq; simpa using ho
    · subst hq; exact ho

/-- An accepted schema change keeps every slot array within the counter. -/
theorem changeSchemaI_inv (valid : List (TypeId × ObjType) → Bool) (canMgr : Bool) (t t' : ITenant)
    (strict : Bool) (ev : Option Nat) (cs : List Change) (hinv : t.Inv)
    (h : changeSchemaI valid canMgr t strict ev cs = .ok t') : t'.Inv := by
  unfold changeSchemaI at h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  split at h
  · cases h
  rename_i types1 hap
  split at h
  · cases h
  dsimp only at h
  split at h
  · cases h
  split at h
  · cases h
  simp only [Except.ok.injEq] at h
  subst h
  have h1 : LkBelow t.nextId t.types := fun T o ho => hinv T o ho
  exact lkBelow_rewriteTypesI _ _ (applyChangesI_lk valid t.nextId cs t.types types1 h1 hap)

end Impl
end Strata
