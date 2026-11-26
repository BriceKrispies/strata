/-
The schema change in general (slice 11): what an accepted `changeSchemaT` leaves, as lemmas about the object types it
writes. They are the cases of the state invariants (`Strata/References.lean`, `Strata/Uniqueness.lean`,
`Strata/Workflows.lean`, `Strata/Proofs.lean`) for `Intent.changeSchema`.
-/

import Strata.Model
import Strata.StepParts

namespace Strata

/-! ## Lookups in a list of object types -/

theorem sc_lookup_cons (j k : TypeId) (p : ObjType) (es : List (TypeId × ObjType)) :
    ((j, p) :: es).lookup k = if k = j then some p else es.lookup k := by
  by_cases h : k = j
  · subst h; simp [List.lookup]
  · have : (k == j) = false := by simpa using h
    simp [List.lookup, this, h]

theorem sc_lookup_putTypeList (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType) (k : TypeId) :
    (putTypeList l ty o).lookup k =
      if k = ty then (if l.lookup ty = none then none else some o) else l.lookup k := by
  induction l with
  | nil => simp [putTypeList, List.lookup]
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    by_cases hj : j = ty
    · subst hj
      by_cases hk : k = j
      · subst hk; simp [putTypeList, List.lookup]
      · have hb : (k == j) = false := by simpa using hk
        simp [putTypeList, List.lookup, hb, hk]
    · have hb : (j == ty) = false := by simpa using hj
      by_cases hk : k = ty
      · subst hk
        have hkj : (k == j) = false := by
          have : k ≠ j := fun e => hj e.symm
          simpa using this
        simp only [putTypeList, hb, Bool.false_eq_true, if_false, List.lookup, hkj, ih]
        try simp
      · by_cases hkj : k = j
        · subst hkj
          have : (k == k) = true := by simp
          simp [putTypeList, hb, List.lookup, hk]
        · have hkb : (k == j) = false := by simpa using hkj
          simp only [putTypeList, hb, Bool.false_eq_true, if_false, List.lookup, hkb, ih]
          simp [hk]

theorem sc_lookup_putTypeList_self (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType)
    (h : l.lookup ty ≠ none) : (putTypeList l ty o).lookup ty = some o := by
  rw [sc_lookup_putTypeList]; simp [h]

theorem sc_lookup_putTypeList_other (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType) (k : TypeId)
    (h : k ≠ ty) : (putTypeList l ty o).lookup k = l.lookup k := by
  rw [sc_lookup_putTypeList]; simp [h]

theorem sc_keys_putTypeList (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType) :
    (putTypeList l ty o).map Prod.fst = l.map Prod.fst := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    unfold putTypeList
    split
    · rename_i h
      have hk : k = ty := by simpa using h
      subst hk
      rfl
    · simp [ih]

theorem sc_lookup_filter (l : List (TypeId × ObjType)) (ty k : TypeId) :
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

theorem sc_lookup_append_fresh (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType) (k : TypeId)
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
      intro e; subst e; simp [sc_lookup_cons] at hf
    have hf' : es.lookup ty = none := by
      rw [sc_lookup_cons] at hf
      have : ty ≠ j := fun e => hj e.symm
      simpa [this] using hf
    simp only [List.cons_append, sc_lookup_cons, ih hf']
    by_cases hk : k = j
    · subst hk
      have : k ≠ ty := hj
      simp [this]
    · simp [hk]

theorem sc_lookup_none_of_keys (l : List (TypeId × ObjType)) (k : TypeId)
    (h : ¬ l.any (fun e => e.1 == k) = true) : l.lookup k = none := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    simp only [List.any_cons, Bool.or_eq_true, not_or] at h
    obtain ⟨h1, h2⟩ := h
    have hk : k ≠ j := by
      intro e; subst e; simp at h1
    rw [sc_lookup_cons]
    simp [hk, ih (by simpa using h2)]

theorem sc_any_of_lookup (l : List (TypeId × ObjType)) (k : TypeId) (o : ObjType)
    (h : l.lookup k = some o) : l.any (fun e => e.1 == k) = true := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons e es ih =>
    obtain ⟨j, p⟩ := e
    rw [sc_lookup_cons] at h
    by_cases hk : k = j
    · subst hk; simp
    · simp only [hk, if_false] at h
      simp [ih h]

theorem sc_keys_nodup_lookup_none (l : List (TypeId × ObjType)) (k : TypeId) :
    l.lookup k = none ↔ ¬ l.any (fun e => e.1 == k) = true := by
  constructor
  · intro h hany
    induction l with
    | nil => simp at hany
    | cons e' es ih =>
      obtain ⟨j, p⟩ := e'
      rw [sc_lookup_cons] at h
      by_cases hkj : k = j
      · simp [hkj] at h
      · simp only [hkj, if_false] at h
        have hj : (j == k) = false := by
          have : j ≠ k := fun e => hkj e.symm
          simpa using this
        simp only [List.any_cons, hj, Bool.false_or] at hany
        exact ih h hany
  · exact sc_lookup_none_of_keys l k


/-! ## Schemas -/

theorem sc_setField_keys_nodup (schema : Schema) (f : FieldId) (d : FieldDef)
    (h : (schema.map Prod.fst).Nodup) : ((setField schema f d).map Prod.fst).Nodup := by
  unfold setField
  by_cases hany : schema.any (fun fd => fd.1 == f) = true
  · rw [if_pos hany, List.map_map]
    have : (Prod.fst ∘ fun fd : FieldId × FieldDef => if fd.1 == f then (f, d) else fd) = Prod.fst := by
      funext fd
      by_cases hb : fd.1 = f
      · simp [hb]
      · simp [hb]
    rw [this]; exact h
  · rw [if_neg hany, List.map_append]
    simp only [List.map_cons, List.map_nil]
    refine List.nodup_append.mpr ⟨h, List.nodup_cons.mpr ⟨by simp, List.nodup_nil⟩, ?_⟩
    intro a ha b hb
    simp only [List.mem_singleton] at hb
    subst hb
    intro hab
    subst hab
    apply hany
    obtain ⟨fd, hfd, rfl⟩ := List.mem_map.mp ha
    exact List.any_eq_true.mpr ⟨fd, hfd, by simp⟩

theorem sc_filter_keys_nodup (schema : Schema) (p : FieldId × FieldDef → Bool)
    (h : (schema.map Prod.fst).Nodup) : ((schema.filter p).map Prod.fst).Nodup :=
  h.sublist (List.Sublist.map _ (List.filter_sublist))

theorem sc_filter_types_keys_nodup (l : List (TypeId × ObjType)) (p : TypeId × ObjType → Bool)
    (h : (l.map Prod.fst).Nodup) : ((l.filter p).map Prod.fst).Nodup :=
  h.sublist (List.Sublist.map _ (List.filter_sublist))

/-! ## What the changes do to the object types -/

/-- The object types a change removes. -/
def Change.removed : Change → List TypeId
  | .removeType X _ => [X]
  | _ => []

/-- What the changes done so far (naming `names`, touching `tch`, removing `rem`) leave, against the object types `orig`
    they started from: one entry per type; a type no change names is as it was; a type no change touches is as it was
    or holds no record; a type holding records has those of its original, and its composite constraints, constraints
    and workflows; a type no change removes keeps its records, and one that is gone was removed; each schema names each
    field once. -/
structure ChRel (orig cur : List (TypeId × ObjType)) (names tch rem : List TypeId) : Prop where
  keys : (orig.map Prod.fst).Nodup → (cur.map Prod.fst).Nodup
  same : ∀ T, T ∉ names → cur.lookup T = orig.lookup T
  kept : ∀ T o1, cur.lookup T = some o1 → T ∉ tch → orig.lookup T = some o1 ∨ o1.records = []
  shape : ∀ T o1, cur.lookup T = some o1 → o1.records = [] ∨
    ∃ o0, orig.lookup T = some o0 ∧ o1.records = o0.records ∧ o1.uniques = o0.uniques ∧
      o1.constraints = o0.constraints ∧ o1.workflows = o0.workflows
  sch : (∀ T o, orig.lookup T = some o → (o.schema.map Prod.fst).Nodup) →
    ∀ T o1, cur.lookup T = some o1 → (o1.schema.map Prod.fst).Nodup
  emptied : ∀ T o1 o0, cur.lookup T = some o1 → orig.lookup T = some o0 → T ∉ rem → o1.records = o0.records
  gone : ∀ T o0, orig.lookup T = some o0 → cur.lookup T = none → T ∈ rem

theorem chRel_start (orig : List (TypeId × ObjType)) : ChRel orig orig [] [] [] where
  keys := fun hk => hk
  same := fun _ _ => rfl
  kept := fun T o1 h _ => Or.inl h
  shape := fun T o1 h => Or.inr ⟨o1, h, rfl, rfl, rfl, rfl⟩
  sch := fun hs => hs
  emptied := fun T o1 o0 h1 h0 _ => by rw [h0] at h1; cases h1; rfl
  gone := fun T o0 h0 h1 => by rw [h0] at h1; cases h1

theorem chRel_putType {orig cur : List (TypeId × ObjType)} {names tch rem : List TypeId} (c : Change)
    (hr : ChRel orig cur names tch rem) (ty : TypeId) (o : ObjType) (hl : cur.lookup ty = some o)
    (hty : c.names = ty) (htch : ty ∈ c.touched) (hrem : c.removed = []) (o' : ObjType)
    (hrec : o'.records = o.records) (hu : o'.uniques = o.uniques) (hc : o'.constraints = o.constraints)
    (hw : o'.workflows = o.workflows)
    (hs : (∀ T o, orig.lookup T = some o → (o.schema.map Prod.fst).Nodup) → (o'.schema.map Prod.fst).Nodup) :
    ChRel orig (putTypeList cur ty o') (c.names :: names) (c.touched ++ tch) (c.removed ++ rem) := by
  have hne : cur.lookup ty ≠ none := by simp [hl]
  have lk : ∀ T, (putTypeList cur ty o').lookup T = if T = ty then some o' else cur.lookup T := by
    intro T
    by_cases hT : T = ty
    · subst hT; simp [sc_lookup_putTypeList_self cur T _ hne]
    · simp [sc_lookup_putTypeList_other cur ty _ T hT, hT]
  rw [hrem, List.nil_append]
  refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
  · intro hk; rw [sc_keys_putTypeList]; exact hr.keys hk
  · intro T hT
    have hTy : T ≠ ty := by
      intro e; apply hT; subst e; rw [hty]; exact List.mem_cons_self
    have hT' : T ∉ names := fun hm => hT (List.mem_cons_of_mem _ hm)
    rw [lk T, if_neg hTy]; exact hr.same T hT'
  · intro T o1 ho hT
    rw [lk T] at ho
    by_cases hTy : T = ty
    · exfalso; apply hT; subst hTy
      exact List.mem_append_left _ htch
    · rw [if_neg hTy] at ho
      have hT' : T ∉ tch := fun hm => hT (List.mem_append_right _ hm)
      exact hr.kept T o1 ho hT'
  · intro T o1 ho
    rw [lk T] at ho
    by_cases hTy : T = ty
    · subst hTy
      rw [if_pos rfl] at ho
      cases ho
      rcases hr.shape _ o hl with h1 | ⟨o0, h0, h2, h3, h4, h5⟩
      · exact Or.inl (by rw [hrec]; exact h1)
      · exact Or.inr ⟨o0, h0, by rw [hrec, h2], by rw [hu, h3], by rw [hc, h4], by rw [hw, h5]⟩
    · rw [if_neg hTy] at ho
      exact hr.shape T o1 ho
  · intro hs0 T o1 ho
    rw [lk T] at ho
    by_cases hTy : T = ty
    · rw [if_pos hTy] at ho
      cases ho
      exact hs hs0
    · rw [if_neg hTy] at ho
      exact hr.sch hs0 T o1 ho
  · intro T o1 o0 ho h0 hT
    rw [lk T] at ho
    by_cases hTy : T = ty
    · subst hTy
      rw [if_pos rfl] at ho
      cases ho
      rw [hrec]
      exact hr.emptied _ o o0 hl h0 hT
    · rw [if_neg hTy] at ho
      exact hr.emptied T o1 o0 ho h0 hT
  · intro T o0 h0 hn
    rw [lk T] at hn
    by_cases hTy : T = ty
    · rw [if_pos hTy] at hn; cases hn
    · rw [if_neg hTy] at hn
      exact hr.gone T o0 h0 hn

theorem chRel_step {orig cur cur' : List (TypeId × ObjType)} {names tch rem : List TypeId} (c : Change)
    (hr : ChRel orig cur names tch rem) (h : applyChange cur c = .ok cur') :
    ChRel orig cur' (c.names :: names) (c.touched ++ tch) (c.removed ++ rem) := by
  cases c with
  | defineField ty f d =>
    simp only [applyChange] at h
    cases hl : cur.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl, Except.ok.injEq] at h
      subst h
      exact chRel_putType _ hr ty o hl rfl (by simp [Change.touched]) rfl _ rfl rfl rfl rfl
        (fun hs0 => sc_setField_keys_nodup _ f d (hr.sch hs0 ty o hl))
  | removeField ty f =>
    simp only [applyChange] at h
    cases hl : cur.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl] at h
      split at h
      · simp only [Except.ok.injEq] at h
        subst h
        exact chRel_putType _ hr ty o hl rfl (by simp [Change.touched]) rfl _ rfl rfl rfl rfl
          (fun hs0 => sc_filter_keys_nodup _ _ (hr.sch hs0 ty o hl))
      · simp at h
  | defineType ty =>
    simp only [applyChange] at h
    by_cases hany : cur.any (fun e => e.1 == ty) = true
    · simp only [hany, if_true, Except.ok.injEq] at h
      subst h
      refine ⟨hr.keys, ?_, ?_, ?_, hr.sch, ?_, ?_⟩
      · intro T hT
        exact hr.same T (fun hm => hT (List.mem_cons_of_mem _ hm))
      · intro T o1 ho hT
        exact hr.kept T o1 ho (fun hm => hT (List.mem_append_right _ hm))
      · exact hr.shape
      · intro T o1 o0 ho h0 hT
        exact hr.emptied T o1 o0 ho h0 (fun hm => hT (by simpa [Change.removed] using hm))
      · intro T o0 h0 hn
        have := hr.gone T o0 h0 hn
        simpa [Change.removed] using this
    · simp only [hany, Bool.false_eq_true, if_false, Except.ok.injEq] at h
      subst h
      have hnone : cur.lookup ty = none := (sc_keys_nodup_lookup_none cur ty).mpr hany
      have lk : ∀ T, (cur ++ [(ty, ({ schema := [] } : ObjType))]).lookup T =
          if T = ty then some { schema := [] } else cur.lookup T := fun T =>
        sc_lookup_append_fresh cur ty _ T hnone
      refine ⟨?_, ?_, ?_, ?_, ?_, ?_, ?_⟩
      · intro hk
        rw [List.map_append]
        simp only [List.map_cons, List.map_nil]
        refine List.nodup_append.mpr ⟨hr.keys hk, List.nodup_cons.mpr ⟨by simp, List.nodup_nil⟩, ?_⟩
        intro a ha b hb
        simp only [List.mem_singleton] at hb
        subst hb
        intro hab
        subst hab
        apply hany
        obtain ⟨e, he, rfl⟩ := List.mem_map.mp ha
        exact List.any_eq_true.mpr ⟨e, he, by simp⟩
      · intro T hT
        have hTy : T ≠ ty := by
          intro e; apply hT; subst e; simp [Change.names]
        rw [lk T, if_neg hTy]
        exact hr.same T (fun hm => hT (List.mem_cons_of_mem _ hm))
      · intro T o1 ho hT
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho
          cases ho
          exact Or.inr rfl
        · rw [if_neg hTy] at ho
          exact hr.kept T o1 ho (fun hm => hT (List.mem_append_right _ hm))
      · intro T o1 ho
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho
          cases ho
          exact Or.inl rfl
        · rw [if_neg hTy] at ho
          exact hr.shape T o1 ho
      · intro hs0 T o1 ho
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho
          cases ho
          exact List.nodup_nil
        · rw [if_neg hTy] at ho
          exact hr.sch hs0 T o1 ho
      · intro T o1 o0 ho h0 hT
        rw [lk T] at ho
        by_cases hTy : T = ty
        · exfalso
          subst hTy
          have := hr.gone _ o0 h0 hnone
          exact hT (by simpa [Change.removed] using this)
        · rw [if_neg hTy] at ho
          exact hr.emptied T o1 o0 ho h0 (fun hm => hT (by simpa [Change.removed] using hm))
      · intro T o0 h0 hn
        rw [lk T] at hn
        by_cases hTy : T = ty
        · rw [if_pos hTy] at hn; cases hn
        · rw [if_neg hTy] at hn
          have := hr.gone T o0 h0 hn
          simpa [Change.removed] using this
  | removeType ty del =>
    simp only [applyChange] at h
    cases hl : cur.lookup ty with
    | none => simp [hl] at h
    | some o =>
      simp only [hl, Except.ok.injEq] at h
      subst h
      have lk : ∀ T, (cur.filter (fun e => !(e.1 == ty))).lookup T = if T = ty then none else cur.lookup T :=
        fun T => sc_lookup_filter cur ty T
      refine ⟨fun hk => sc_filter_types_keys_nodup _ _ (hr.keys hk), ?_, ?_, ?_, ?_, ?_, ?_⟩
      · intro T hT
        have hTy : T ≠ ty := by
          intro e; apply hT; subst e; simp [Change.names]
        rw [lk T, if_neg hTy]
        exact hr.same T (fun hm => hT (List.mem_cons_of_mem _ hm))
      · intro T o1 ho hT
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho; cases ho
        · rw [if_neg hTy] at ho
          exact hr.kept T o1 ho (fun hm => hT (List.mem_append_right _ hm))
      · intro T o1 ho
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho; cases ho
        · rw [if_neg hTy] at ho
          exact hr.shape T o1 ho
      · intro hs0 T o1 ho
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho; cases ho
        · rw [if_neg hTy] at ho
          exact hr.sch hs0 T o1 ho
      · intro T o1 o0 ho h0 hT
        rw [lk T] at ho
        by_cases hTy : T = ty
        · rw [if_pos hTy] at ho; cases ho
        · rw [if_neg hTy] at ho
          exact hr.emptied T o1 o0 ho h0 (fun hm => hT (List.mem_append_right _ hm))
      · intro T o0 h0 hn
        rw [lk T] at hn
        by_cases hTy : T = ty
        · subst hTy; simp [Change.removed]
        · rw [if_neg hTy] at hn
          exact List.mem_append_right _ (hr.gone T o0 h0 hn)

theorem ChRel.mono {orig cur : List (TypeId × ObjType)} {names tch rem names' tch' rem' : List TypeId}
    (hr : ChRel orig cur names tch rem) (hn : ∀ x, x ∈ names → x ∈ names') (ht : ∀ x, x ∈ tch → x ∈ tch')
    (hm : ∀ x, x ∈ rem → x ∈ rem') : ChRel orig cur names' tch' rem' where
  keys := hr.keys
  same := fun T hT => hr.same T (fun hh => hT (hn T hh))
  kept := fun T o1 ho hT => hr.kept T o1 ho (fun hh => hT (ht T hh))
  shape := hr.shape
  sch := hr.sch
  emptied := fun T o1 o0 h1 h0 hT => hr.emptied T o1 o0 h1 h0 (fun hh => hT (hm T hh))
  gone := fun T o0 h0 hn => hm T (hr.gone T o0 h0 hn)

/-- The whole list of changes: the object types it leaves keep the relation. -/
theorem chRel_applyChanges (valid : List (TypeId × ObjType) → Bool) (orig : List (TypeId × ObjType)) :
    ∀ (cs : List Change) (cur r : List (TypeId × ObjType)) (names tch rem : List TypeId),
      ChRel orig cur names tch rem → applyChanges valid cur cs = .ok r →
      ChRel orig r (cs.map Change.names ++ names) (cs.flatMap Change.touched ++ tch)
        (cs.flatMap Change.removed ++ rem) := by
  intro cs
  induction cs with
  | nil =>
    intro cur r names tch rem hr h
    simp only [applyChanges] at h
    split at h
    · simp only [Except.ok.injEq] at h
      subst h
      simpa using hr
    · simp at h
  | cons c cs ih =>
    intro cur r names tch rem hr h
    simp only [applyChanges] at h
    cases hc : applyChange cur c with
    | error e => rw [hc] at h; cases h
    | ok cur' =>
      rw [hc] at h
      have h' : applyChanges valid cur' cs = .ok r := h
      have hr' := chRel_step c hr hc
      have := ih cur' r _ _ _ hr' h'
      refine this.mono ?_ ?_ ?_
      · intro x hx
        simp only [List.mem_append, List.mem_cons] at hx
        simp only [List.map_cons, List.cons_append, List.mem_append, List.mem_cons]
        rcases hx with h | h | h <;> simp [h]
      · intro x hx
        simp only [List.mem_append] at hx
        simp only [List.flatMap_cons, List.mem_append]
        rcases hx with h | h | h <;> simp [h]
      · intro x hx
        simp only [List.mem_append] at hx
        simp only [List.flatMap_cons, List.mem_append]
        rcases hx with h | h | h <;> simp [h]

/-- A list of changes that applies leaves object types that the validity predicate accepts. -/
theorem sc_applyChanges_valid (valid : List (TypeId × ObjType) → Bool) :
    ∀ (types : List (TypeId × ObjType)) (cs : List Change) (r : List (TypeId × ObjType)),
      applyChanges valid types cs = .ok r → valid r = true := by
  intro types cs
  induction cs generalizing types with
  | nil =>
    intro r h
    simp only [applyChanges] at h
    split at h
    · simp only [Except.ok.injEq] at h
      subst h
      assumption
    · simp at h
  | cons c cs ih =>
    intro r h
    simp only [applyChanges] at h
    cases hc : applyChange types c with
    | error e => rw [hc] at h; cases h
    | ok types' =>
      rw [hc] at h
      exact ih types' r h

/-! ## The accepted change -/

/-- An accepted `changeSchemaT` passed every check, in its order. -/
theorem changeSchemaT_parts {valid : List (TypeId × ObjType) → Bool} {t t' : Tenant} {p : Principal}
    {strict : Bool} {ev : Option Nat} {cs : List Change}
    (h : changeSchemaT valid t p strict ev cs = .ok t') :
    ∃ types1, applyChanges valid t.types cs = .ok types1 ∧
      impactRefused t.policies t.types types1 cs = false ∧
      t' = { t with types := rewriteTypes t.types (cs.flatMap Change.touched) types1,
                    schemaVersion := t.schemaVersion + 1 } ∧
      changeRefused t.limits (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
        (recordsIn (rewriteTypes t.types (cs.flatMap Change.touched) types1)) strict = false ∧
      removalsRefused t.types cs = false := by
  unfold changeSchemaT at h
  split at h
  · simp at h
  · split at h
    · simp at h
    · split at h
      · simp at h
      · split at h
        · simp at h
        · split at h
          · simp at h
          · rename_i types1 hap
            split at h
            · simp at h
            · rename_i himp
              simp only [] at h
              split at h
              · simp at h
              · rename_i hch
                split at h
                · simp at h
                · rename_i hrem
                  simp only [Except.ok.injEq] at h
                  subst h
                  refine ⟨types1, hap, ?_, rfl, ?_, ?_⟩
                  · simpa using himp
                  · simpa using hch
                  · simpa using hrem

theorem stepTenant_changeSchema_parts {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') :
    ∃ types1, applyChanges (schemaRulesOk t.policies) t.types cs = .ok types1 ∧
      impactRefused t.policies t.types types1 cs = false ∧
      t' = { t with types := rewriteTypes t.types (cs.flatMap Change.touched) types1 } ∧
      changeRefused t.limits (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
        (recordsIn (rewriteTypes t.types (cs.flatMap Change.touched) types1)) strict = false ∧
      removalsRefused t.types cs = false := by
  simp only [stepTenant] at h
  cases hc : changeSchemaT (schemaRulesOk t.policies) t p strict ev cs with
  | error e => simp [hc, Except.map] at h
  | ok t1 =>
    simp only [hc, Except.map, Except.ok.injEq] at h
    subst h
    obtain ⟨types1, hap, himp, rfl, hch, hrem⟩ := changeSchemaT_parts hc
    exact ⟨types1, hap, himp, rfl, hch, hrem⟩

/-! ## Rewriting the records -/

/-- An object type after the rewrite of its records (`rewriteTypes`): the records of a touched type that existed
    lose the fields the type no longer declares. -/
def rwObj (orig : List (TypeId × ObjType)) (touched : List TypeId) (T : TypeId) (o : ObjType) : ObjType :=
  if touched.contains T && orig.any (fun e0 => e0.1 == T) then
    { o with records := o.records.map (fun q => (q.1, stripRecord o.schema q.2)) }
  else o

theorem rewriteTypes_eq (orig : List (TypeId × ObjType)) (touched : List TypeId)
    (types : List (TypeId × ObjType)) :
    rewriteTypes orig touched types = types.map (fun e => (e.1, rwObj orig touched e.1 e.2)) := by
  unfold rewriteTypes rwObj
  apply List.map_congr_left
  intro e _
  split <;> rfl

theorem sc_lookup_map_entry {α β : Type} (l : List (TypeId × α)) (g : TypeId → α → β) (T : TypeId) :
    (l.map (fun e => (e.1, g e.1 e.2))).lookup T = (l.lookup T).map (g T) := by
  induction l with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, p⟩ := e
    simp only [List.map_cons, List.lookup]
    by_cases hT : T = k
    · subst hT; simp
    · have : (T == k) = false := by simpa using hT
      simp [this, ih]

theorem sc_lookup_rewriteTypes (orig : List (TypeId × ObjType)) (touched : List TypeId)
    (types : List (TypeId × ObjType)) (T : TypeId) :
    (rewriteTypes orig touched types).lookup T = (types.lookup T).map (rwObj orig touched T) := by
  rw [rewriteTypes_eq, sc_lookup_map_entry]

theorem sc_keys_rewriteTypes (orig : List (TypeId × ObjType)) (touched : List TypeId)
    (types : List (TypeId × ObjType)) :
    (rewriteTypes orig touched types).map Prod.fst = types.map Prod.fst := by
  rw [rewriteTypes_eq, List.map_map]
  rfl

theorem rwObj_cases (orig : List (TypeId × ObjType)) (touched : List TypeId) (T : TypeId) (o : ObjType) :
    rwObj orig touched T o = o ∨
      (T ∈ touched ∧ rwObj orig touched T o =
        { o with records := o.records.map (fun q => (q.1, stripRecord o.schema q.2)) }) := by
  unfold rwObj
  by_cases hc : (touched.contains T && orig.any (fun e0 => e0.1 == T)) = true
  · right
    refine ⟨?_, by rw [if_pos hc]⟩
    have := (Bool.and_eq_true _ _).mp hc
    simpa using this.1
  · left; rw [if_neg hc]

/-- A stripped record has the fields of the schema, each once if the record's fields were distinct, with the values the
    record had. -/
theorem stripRecord_fields (schema : Schema) (r : Record) :
    (stripRecord schema r).fields = r.fields ∨
      (stripRecord schema r).fields = r.fields.filter (fun fv => schema.any (fun fd => fd.1 == fv.1)) := by
  unfold stripRecord
  split
  · exact Or.inl rfl
  · exact Or.inr rfl

theorem stripRecord_owner (schema : Schema) (r : Record) : (stripRecord schema r).owner = r.owner := by
  unfold stripRecord; split <;> rfl

theorem stripRecord_nodup (schema : Schema) (r : Record) (h : (r.fields.map Prod.fst).Nodup) :
    ((stripRecord schema r).fields.map Prod.fst).Nodup := by
  rcases stripRecord_fields schema r with e | e
  · rw [e]; exact h
  · rw [e]; exact h.sublist (List.Sublist.map _ List.filter_sublist)

theorem sc_lookup_filter_key (fs : Fields) (p : FieldId → Bool) (f : FieldId) (hp : p f = true) :
    (fs.filter (fun fv => p fv.1)).lookup f = fs.lookup f := by
  induction fs with
  | nil => rfl
  | cons e es ih =>
    obtain ⟨k, v⟩ := e
    by_cases hk : k = f
    · subst hk; simp [List.filter_cons, hp, List.lookup]
    · by_cases hpk : p k = true
      · have hb : (f == k) = false := by
          have : f ≠ k := fun e => hk e.symm
          simpa using this
        simp [List.filter_cons, hpk, List.lookup, hb, ih]
      · have hb : (f == k) = false := by
          have : f ≠ k := fun e => hk e.symm
          simpa using this
        simp [List.filter_cons, hpk, List.lookup, hb, ih]

/-- A declared field reads the same in a stripped record. -/
theorem stripRecord_get (schema : Schema) (r : Record) (f : FieldId) (hf : declaredIn schema f = true) :
    (stripRecord schema r).fields.get f = r.fields.get f := by
  rcases stripRecord_fields schema r with e | e
  · rw [e]
  · rw [e]
    unfold Fields.get
    have := sc_lookup_filter_key r.fields (fun g => schema.any (fun fd => fd.1 == g)) f hf
    rw [this]

/-- Evaluating an expression on a stored record reads only the fields it reads and the record's creator. -/
theorem evalExpr_storedEnv_congr (r r' : Record) (ho : r.owner = r'.owner) :
    ∀ e : Expr, (∀ f ∈ e.reads, r.fields.get f = r'.fields.get f) →
      evalExpr (storedEnv r) e = evalExpr (storedEnv r') e := by
  intro e
  induction e with
  | lit v => intro _; rfl
  | field s f =>
    intro h
    have hf := h f (by simp [Expr.reads])
    cases s <;> simp [evalExpr, storedEnv, ExprEnv.scoped, hf]
  | createdBy s =>
    intro _
    cases s <;> simp [evalExpr, storedEnv, ExprEnv.scoped, ho]
  | principal => intro _; rfl
  | cmp op a b iha ihb =>
    intro h
    simp only [evalExpr]
    rw [iha (fun f hf => h f (by simp [Expr.reads, hf])), ihb (fun f hf => h f (by simp [Expr.reads, hf]))]
  | and a b iha ihb =>
    intro h
    simp only [evalExpr]
    rw [iha (fun f hf => h f (by simp [Expr.reads, hf])), ihb (fun f hf => h f (by simp [Expr.reads, hf]))]
  | or a b iha ihb =>
    intro h
    simp only [evalExpr]
    rw [iha (fun f hf => h f (by simp [Expr.reads, hf])), ihb (fun f hf => h f (by simp [Expr.reads, hf]))]
  | not a iha =>
    intro h
    simp only [evalExpr]
    rw [iha (fun f hf => h f (by simp [Expr.reads, hf]))]
  | isNull a iha =>
    intro h
    simp only [evalExpr]
    rw [iha (fun f hf => h f (by simp [Expr.reads, hf]))]

theorem constraintProblem_storedEnv_congr (r r' : Record) (ho : r.owner = r'.owner) :
    ∀ cs : List Expr, (∀ c ∈ cs, ∀ f ∈ c.reads, r.fields.get f = r'.fields.get f) →
      constraintProblem (storedEnv r) cs = constraintProblem (storedEnv r') cs := by
  intro cs
  induction cs with
  | nil => intro _; rfl
  | cons c cs ih =>
    intro h
    simp only [constraintProblem]
    rw [evalExpr_storedEnv_congr r r' ho c (h c List.mem_cons_self)]
    rw [ih (fun d hd => h d (List.mem_cons_of_mem _ hd))]


/-! ## What the validity rules and the impact rule give -/

theorem sc_lookup_mem {l : List (TypeId × ObjType)} {a : TypeId} {b : ObjType}
    (h : l.lookup a = some b) : (a, b) ∈ l := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons x xs ih =>
    obtain ⟨k, v⟩ := x
    rw [sc_lookup_cons] at h
    by_cases hk : a = k
    · simp only [hk, if_true, Option.some.injEq] at h
      subst hk; subst h
      exact List.mem_cons_self
    · simp only [hk, if_false] at h
      exact List.mem_cons_of_mem _ (ih h)

theorem sc_declared_of_lookup (schema : Schema) (f : FieldId) (d : FieldDef)
    (h : schema.lookup f = some d) : declaredIn schema f = true := by
  induction schema with
  | nil => simp [List.lookup] at h
  | cons e es ih =>
    obtain ⟨k, v⟩ := e
    unfold declaredIn
    by_cases hk : f = k
    · subst hk; simp
    · have hb : (f == k) = false := by simpa using hk
      simp only [List.lookup, hb] at h
      have := ih h
      unfold declaredIn at this
      have hb' : (k == f) = false := by
        have : k ≠ f := fun e => hk e.symm
        simpa using this
      simp [hb', this]

/-- The rules of a valid schema, for one object type of it. -/
theorem schemaRulesOk_entry {ps : List Policy} {types : List (TypeId × ObjType)}
    (h : schemaRulesOk ps types = true) {T : TypeId} {o : ObjType} (hl : types.lookup T = some o) :
    (∀ fd ∈ o.schema, ∀ X, fd.2.ty.target? = some X → types.any (fun e' => e'.1 == X) = true) ∧
    (∀ c ∈ o.constraints, ∀ f ∈ c.reads, declaredIn o.schema f = true) ∧
    (∀ w ∈ o.workflows, declaredIn o.schema w.field = true) := by
  unfold schemaRulesOk at h
  have h1 := ((Bool.and_eq_true _ _).mp h).1
  have h2 := (List.all_eq_true.mp h1) (T, o) (sc_lookup_mem hl)
  simp only [Bool.and_eq_true] at h2
  obtain ⟨⟨⟨⟨⟨hA, -⟩, -⟩, -⟩, hE⟩, hF⟩ := h2
  refine ⟨?_, ?_, ?_⟩
  · intro fd hfd X hX
    have := List.all_eq_true.mp hA fd hfd
    simpa [hX] using this
  · intro c hc f hf
    have := List.all_eq_true.mp hE c hc
    unfold allDeclared at this
    exact List.all_eq_true.mp this f hf
  · intro w hw
    have := List.all_eq_true.mp hF w hw
    unfold workflowOk at this
    simp only [Bool.and_eq_true] at this
    have h3 := this.1
    cases hlk : o.schema.lookup w.field with
    | none => simp [hlk] at h3
    | some d => exact sc_declared_of_lookup _ _ d hlk

/-- A change list that removes an object type and leaves it defined again, while a type's field still names it, is
    refused: so an accepted one leaves no such field. -/
theorem impactRefused_removed {ps : List Policy} {types0 types1 : List (TypeId × ObjType)} {cs : List Change}
    (h : impactRefused ps types0 types1 cs = false) {X : TypeId} {d : Bool}
    (hc : Change.removeType X d ∈ cs) (hX : types1.any (fun e => e.1 == X) = true)
    {U : TypeId} {o0 o1 : ObjType} (h0 : types0.lookup U = some o0) (h1 : types1.lookup U = some o1)
    (fd : FieldId × FieldDef) (hfd : fd ∈ o0.schema) (ht : fd.2.ty.target? = some X)
    (hfd1 : fd ∈ o1.schema) : False := by
  have : impactRefused ps types0 types1 cs = true := by
    unfold impactRefused
    simp only [Bool.or_eq_true]
    right
    refine List.any_eq_true.mpr ⟨Change.removeType X d, hc, ?_⟩
    simp only [Bool.and_eq_true, Bool.or_eq_true]
    refine ⟨hX, Or.inl ?_⟩
    refine List.any_eq_true.mpr ⟨(U, o0), sc_lookup_mem h0, ?_⟩
    refine List.any_eq_true.mpr ⟨fd, hfd, ?_⟩
    simp only [Bool.and_eq_true, ht, h1, beq_self_eq_true]
    refine ⟨trivial, ?_⟩
    exact List.any_eq_true.mpr ⟨fd, hfd1, by simp [ht]⟩
  rw [h] at this
  cases this

/-! ## The invariants through an accepted change -/

theorem changeSchema_ctx {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool} {cs : List Change}
    (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') :
    ∃ types1, ChRel t.types types1 (cs.map Change.names) (cs.flatMap Change.touched)
        (cs.flatMap Change.removed) ∧
      schemaRulesOk t.policies types1 = true ∧
      impactRefused t.policies t.types types1 cs = false ∧
      t' = { t with types := rewriteTypes t.types (cs.flatMap Change.touched) types1 } ∧
      (∀ T o2, t'.typ? T = some o2 → ∃ o1, types1.lookup T = some o1 ∧
        rwObj t.types (cs.flatMap Change.touched) T o1 = o2) ∧
      (strict = true → schemaViolated t.limits (changesScope t'.types cs) = false ∧
        refsViolated (changesScope t'.types cs) (recordsIn t'.types) = false) := by
  obtain ⟨types1, hap, himp, ht', hch, -⟩ := stepTenant_changeSchema_parts h
  have hr0 := chRel_start t.types
  have hr := chRel_applyChanges (schemaRulesOk t.policies) t.types cs t.types types1 [] [] [] hr0 hap
  rw [List.append_nil, List.append_nil, List.append_nil] at hr
  refine ⟨types1, hr, sc_applyChanges_valid _ _ _ _ hap, himp, ht', ?_, ?_⟩
  · intro T o2 hT2
    have hT2' : (rewriteTypes t.types (cs.flatMap Change.touched) types1).lookup T = some o2 := by
      rw [ht'] at hT2; exact hT2
    rw [sc_lookup_rewriteTypes] at hT2'
    cases hl : types1.lookup T with
    | none => rw [hl] at hT2'; simp at hT2'
    | some o1 =>
      rw [hl] at hT2'
      exact ⟨o1, rfl, by simpa using hT2'⟩
  · intro hs
    rw [hs] at hch
    have hch' : (schemaViolated t.limits (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
        || refsViolated (changesScope (rewriteTypes t.types (cs.flatMap Change.touched) types1) cs)
          (recordsIn (rewriteTypes t.types (cs.flatMap Change.touched) types1))) = false := by
      simpa [changeRefused] using hch
    rw [Bool.or_eq_false_iff] at hch'
    rw [ht']
    exact hch'

theorem rwObj_record_view (orig : List (TypeId × ObjType)) (touched : List TypeId) (T : TypeId) (o : ObjType) :
    (rwObj orig touched T o).schema = o.schema ∧ (rwObj orig touched T o).uniques = o.uniques ∧
    (rwObj orig touched T o).constraints = o.constraints ∧ (rwObj orig touched T o).workflows = o.workflows ∧
    ∀ q ∈ (rwObj orig touched T o).records, ∃ q1 ∈ o.records, q.1 = q1.1 ∧
      (q.2 = q1.2 ∨ q.2 = stripRecord o.schema q1.2) := by
  unfold rwObj
  split
  · refine ⟨rfl, rfl, rfl, rfl, ?_⟩
    intro q hq
    obtain ⟨q1, hq1, rfl⟩ := List.mem_map.mp hq
    exact ⟨q1, hq1, rfl, Or.inr rfl⟩
  · exact ⟨rfl, rfl, rfl, rfl, fun q hq => ⟨q, hq, rfl, Or.inl rfl⟩⟩

theorem changeSchema_distinct {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (hd : t.Distinct) (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') :
    t'.Distinct := by
  obtain ⟨types1, hr, hv, himp, ht', hview, -⟩ := changeSchema_ctx h
  refine ⟨?_, ?_⟩
  · rw [ht']
    show ((rewriteTypes _ _ _).map Prod.fst).Nodup
    rw [sc_keys_rewriteTypes]; exact hr.keys hd.1
  · intro T o2 hT2
    obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
    obtain ⟨hs, -, -, -, hrec⟩ := rwObj_record_view t.types (cs.flatMap Change.touched) T o1
    refine ⟨by rw [hs]; exact hr.sch (fun T o h => (hd.2 T o h).1) T o1 hl1, ?_⟩
    intro q hq
    obtain ⟨q1, hq1, -, hq2⟩ := hrec q hq
    rcases hr.shape T o1 hl1 with he | ⟨o0, h0, h2, -, -, -⟩
    · rw [he] at hq1; cases hq1
    · have hq0 : q1 ∈ o0.records := by rw [← h2]; exact hq1
      have hn := (hd.2 T o0 h0).2 q1 hq0
      rcases hq2 with e | e
      · rw [e]; exact hn
      · rw [e]; exact stripRecord_nodup _ _ hn

theorem changeSchema_idsBelow {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (hids : t.IdsBelow)
    (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') : t'.IdsBelow := by
  obtain ⟨types1, hr, hv, himp, ht', hview, -⟩ := changeSchema_ctx h
  intro T o2 hT2 q hq
  obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
  obtain ⟨-, -, -, -, hrec⟩ := rwObj_record_view t.types (cs.flatMap Change.touched) T o1
  obtain ⟨q1, hq1, hid, -⟩ := hrec q hq
  rcases hr.shape T o1 hl1 with he | ⟨o0, h0, h2, -, -, -⟩
  · rw [he] at hq1; cases hq1
  · have hq0 : q1 ∈ o0.records := by rw [← h2]; exact hq1
    have := hids T o0 h0 q1 hq0
    rw [hid]
    rw [ht']
    exact this

theorem changeSchema_states {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (hs : t.StatesOk)
    (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') : t'.StatesOk := by
  obtain ⟨types1, hr, hv, himp, ht', hview, -⟩ := changeSchema_ctx h
  intro T o2 hT2 q hq
  obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
  obtain ⟨hsch, -, -, hwf, hrec⟩ := rwObj_record_view t.types (cs.flatMap Change.touched) T o1
  obtain ⟨q1, hq1, hid, hq2⟩ := hrec q hq
  rcases hr.shape T o1 hl1 with he | ⟨o0, h0, h2, h3, h4, h5⟩
  · rw [he] at hq1; cases hq1
  · have hq0 : q1 ∈ o0.records := by rw [← h2]; exact hq1
    obtain ⟨hs1, hs2⟩ := hs T o0 h0 q1 hq0
    have hdecl := (schemaRulesOk_entry hv hl1).2.2
    rw [hwf, h5]
    rcases hq2 with e | e
    · rw [e]; exact ⟨hs1, hs2⟩
    · rw [e]
      have hget : ∀ w ∈ o0.workflows, (stripRecord o1.schema q1.2).fields.get w.field = q1.2.fields.get w.field :=
        fun w hw => stripRecord_get _ _ _ (hdecl w (by rw [h5]; exact hw))
      refine ⟨?_, fun w hw => ?_⟩
      · unfold statesOk at hs1 ⊢
        apply List.all_eq_true.mpr
        intro w hw
        have := List.all_eq_true.mp hs1 w hw
        unfold wfStateOk at this ⊢
        rw [hget w hw]
        exact this
      · obtain ⟨s, hs3⟩ := hs2 w hw
        exact ⟨s, by rw [hget w hw]; exact hs3⟩

theorem changeSchema_satisfied {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (hsat : t.Satisfied)
    (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') : t'.Satisfied := by
  obtain ⟨types1, hr, hv, himp, ht', hview, -⟩ := changeSchema_ctx h
  intro T o2 hT2 q hq
  obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
  obtain ⟨hsch, -, hcon, -, hrec⟩ := rwObj_record_view t.types (cs.flatMap Change.touched) T o1
  obtain ⟨q1, hq1, hid, hq2⟩ := hrec q hq
  rcases hr.shape T o1 hl1 with he | ⟨o0, h0, h2, h3, h4, h5⟩
  · rw [he] at hq1; cases hq1
  · have hq0 : q1 ∈ o0.records := by rw [← h2]; exact hq1
    have hs1 := hsat T o0 h0 q1 hq0
    have hdecl := (schemaRulesOk_entry hv hl1).2.1
    rw [hcon, h4]
    rcases hq2 with e | e
    · rw [e]; exact hs1
    · rw [e]
      rw [constraintProblem_storedEnv_congr (stripRecord o1.schema q1.2) q1.2 (stripRecord_owner _ _)
        o0.constraints (fun c hc f hf => stripRecord_get _ _ _ (hdecl c (by rw [h4]; exact hc) f hf))]
      exact hs1


/-! ## Reference integrity and the checked tables -/

theorem sc_lookup_mem_gen {α β : Type} [BEq α] [LawfulBEq α] (l : List (α × β)) (a : α) (b : β)
    (h : l.lookup a = some b) : (a, b) ∈ l := by
  induction l with
  | nil => simp [List.lookup] at h
  | cons x xs ih =>
    obtain ⟨k, v⟩ := x
    by_cases hk : a = k
    · subst hk
      simp only [List.lookup, beq_self_eq_true, Option.some.injEq] at h
      subst h; exact List.mem_cons_self
    · have hb : (a == k) = false := by simpa using hk
      simp only [List.lookup, hb] at h
      exact List.mem_cons_of_mem _ (ih h)

theorem sc_resolves_mono {env env' : Env} {ty : Ty}
    (hcov : ∀ X x, ty.target? = some X → Records.has (env X) x = true → Records.has (env' X) x = true) :
    ∀ v : Val, v.resolves env ty = true → v.resolves env' ty = true := by
  intro v
  have hr : ∀ id, refHas env ty id = true → refHas env' ty id = true := by
    intro id h
    unfold refHas at h ⊢
    cases hX : ty.target? with
    | none => rfl
    | some X => rw [hX] at h; exact hcov X id hX h
  cases v with
  | null => intro _; rfl
  | int i => intro _; rfl
  | str s => intro _; rfl
  | bool b => intro _; rfl
  | ref id => intro h; exact hr id h
  | list vs =>
    intro h
    simp only [Val.resolves, List.all_eq_true] at h ⊢
    intro e he
    have := h e he
    cases e with
    | null => rfl
    | int i => rfl
    | str s => rfl
    | bool b => rfl
    | ref id => exact hr id this

theorem sc_refsOk_mono {env env' : Env} {schema : Schema} {r : Record} (h : refsOk env schema r = true)
    (hcov : ∀ fd ∈ schema, ∀ X x, fd.2.ty.target? = some X → Records.has (env X) x = true →
      Records.has (env' X) x = true) : refsOk env' schema r = true := by
  unfold refsOk at h ⊢
  apply List.all_eq_true.mpr
  intro fv hfv
  have := List.all_eq_true.mp h fv hfv
  cases hl : schema.lookup fv.1 with
  | none => rfl
  | some d =>
    rw [hl] at this
    have hmem : (fv.1, d) ∈ schema := sc_lookup_mem_gen schema fv.1 d hl
    exact sc_resolves_mono (fun X x hX hx => hcov (fv.1, d) hmem X x hX hx) fv.2 this

theorem has_rwObj_records (orig : List (TypeId × ObjType)) (touched : List TypeId) (X : TypeId) (o : ObjType)
    (x : RecordId) :
    Records.has (rwObj orig touched X o).records x = Records.has o.records x := by
  rcases rwObj_cases orig touched X o with e | ⟨-, e⟩
  · rw [e]
  · rw [e]
    unfold Records.has
    have h2 : (o.records.map (fun q => (q.1, stripRecord o.schema q.2))).lookup x =
        (o.records.lookup x).map (stripRecord o.schema) :=
      sc_lookup_map_entry o.records (fun _ r => stripRecord o.schema r) x
    show ((o.records.map (fun q => (q.1, stripRecord o.schema q.2))).lookup x).isSome = _
    rw [h2]
    simp

theorem changeSchema_refs {t t' : Tenant} {p : Principal} {ev : Option Nat} {strict : Bool}
    {cs : List Change} (hrefs : t.RefsOk) (hstrict : strict = true)
    (h : stepTenant t p (.changeSchema ev strict cs) = .ok t') : t'.RefsOk := by
  obtain ⟨types1, hr, hv, himp, ht', hview, hstr⟩ := changeSchema_ctx h
  obtain ⟨hsv, hrv⟩ := hstr hstrict
  intro T o2 hT2 q hq
  obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
  by_cases hTt : T ∈ cs.flatMap Change.touched
  · have hmem : (T, rwObj t.types (cs.flatMap Change.touched) T o1) ∈ changesScope t'.types cs := by
      unfold changesScope scopedTypes
      exact List.mem_filter.mpr ⟨sc_lookup_mem hT2, by simpa using hTt⟩
    have h1 := Bool.eq_false_iff.mpr (List.any_eq_false.mp hrv _ hmem)
    have h2 := List.any_eq_false.mp h1 q hq
    have : refsOk (recordsIn t'.types) (rwObj t.types (cs.flatMap Change.touched) T o1).schema q.2 = true := by
      simpa using h2
    exact this
  · have hrw : rwObj t.types (cs.flatMap Change.touched) T o1 = o1 := by
      rcases rwObj_cases t.types (cs.flatMap Change.touched) T o1 with e | ⟨hm, -⟩
      · exact e
      · exact absurd hm hTt
    rw [hrw] at hq ⊢
    rcases hr.kept T o1 hl1 hTt with h0 | he
    · have hrefsT := hrefs T o1 h0 q hq
      refine sc_refsOk_mono hrefsT ?_
      intro fd hfd X x hX hx
      -- the target type, in the tenant before the change
      cases hoX : t.typ? X with
      | none =>
        have : t.env X = [] := by unfold Tenant.env; rw [hoX]; rfl
        rw [this] at hx
        simp [Records.has, List.lookup] at hx
      | some oX =>
        have hany := (schemaRulesOk_entry hv hl1).1 fd hfd X hX
        cases hlX : types1.lookup X with
        | none => exact absurd hany ((sc_keys_nodup_lookup_none types1 X).mp hlX)
        | some oX1 =>
          by_cases hXr : X ∈ cs.flatMap Change.removed
          · exfalso
            obtain ⟨c, hc, hXc⟩ := List.mem_flatMap.mp hXr
            cases c with
            | removeType X' d =>
              have hXX : X = X' := by simpa [Change.removed] using hXc
              subst hXX
              exact impactRefused_removed himp hc hany h0 hl1 fd hfd hX hfd
            | defineField _ _ _ => simp [Change.removed] at hXc
            | removeField _ _ => simp [Change.removed] at hXc
            | defineType _ => simp [Change.removed] at hXc
          · have hrec := hr.emptied X oX1 oX hlX hoX hXr
            have henv : t'.env X = (rwObj t.types (cs.flatMap Change.touched) X oX1).records := by
              unfold Tenant.env Tenant.typ?
              rw [ht']
              show (((rewriteTypes _ _ _).lookup X).map ObjType.records).getD [] = _
              rw [sc_lookup_rewriteTypes, hlX]
              rfl
            have henv0 : t.env X = oX.records := by unfold Tenant.env; rw [hoX]; rfl
            rw [henv, has_rwObj_records, hrec, ← henv0]
            exact hx
    · rw [he] at hq; cases hq

theorem changeSchema_table {t t' : Tenant} {p : Principal} {ev : Option Nat} {cs : List Change}
    (h : stepTenant t p (.changeSchema ev true cs) = .ok t') :
    (∀ T o2, t'.typ? T = some o2 → T ∈ cs.flatMap Change.touched → tableViolated t.limits o2 = false) ∧
    (∀ T o2, t'.typ? T = some o2 → T ∉ cs.flatMap Change.touched → t.typ? T = some o2 ∨ o2.records = []) := by
  obtain ⟨types1, hr, hv, himp, ht', hview, hstr⟩ := changeSchema_ctx h
  obtain ⟨hsv, -⟩ := hstr rfl
  constructor
  · intro T o2 hT2 hTt
    have hmem : (T, o2) ∈ changesScope t'.types cs := by
      unfold changesScope scopedTypes
      exact List.mem_filter.mpr ⟨sc_lookup_mem hT2, by simpa using hTt⟩
    have := List.any_eq_false.mp hsv _ hmem
    simpa [tableViolated] using this
  · intro T o2 hT2 hTt
    obtain ⟨o1, hl1, rfl⟩ := hview T o2 hT2
    have hrw : rwObj t.types (cs.flatMap Change.touched) T o1 = o1 := by
      rcases rwObj_cases t.types (cs.flatMap Change.touched) T o1 with e | ⟨hm, -⟩
      · exact e
      · exact absurd hm hTt
    rw [hrw]
    exact hr.kept T o1 hl1 hTt


/-! ## The field a `setRequired` or `setUnique` declares when its type lacks it -/

theorem ensureField_lookup (s : Schema) (f k : FieldId) :
    (ensureField s f).lookup k = s.lookup k ∨
      (s.lookup k = none ∧ (ensureField s f).lookup k = some plainField) := by
  unfold ensureField
  by_cases hany : s.any (fun fd => fd.1 == f) = true
  · rw [if_pos hany]; exact Or.inl rfl
  · rw [if_neg hany]
    induction s with
    | nil =>
      by_cases hk : k = f
      · subst hk; right; simp [List.lookup]
      · left
        have : (k == f) = false := by simpa using hk
        simp [List.lookup, this]
    | cons e es ih =>
      obtain ⟨j, d⟩ := e
      have hjf : (j == f) = false := by
        simp only [List.any_cons, Bool.or_eq_true, not_or] at hany
        simpa using hany.1
      have hes : ¬ es.any (fun fd => fd.1 == f) = true := by
        simp only [List.any_cons, Bool.or_eq_true, not_or] at hany
        exact hany.2
      by_cases hk : k = j
      · subst hk; left; simp [List.lookup]
      · have hb : (k == j) = false := by simpa using hk
        have := ih hes
        simp only [List.cons_append, List.lookup, hb]
        exact this

theorem keys_ensureField (s : Schema) (f : FieldId) (h : (s.map Prod.fst).Nodup) :
    ((ensureField s f).map Prod.fst).Nodup := by
  unfold ensureField
  by_cases hany : s.any (fun fd => fd.1 == f) = true
  · rw [if_pos hany]; exact h
  · rw [if_neg hany, List.map_append]
    simp only [List.map_cons, List.map_nil]
    refine List.nodup_append.mpr ⟨h, List.nodup_cons.mpr ⟨by simp, List.nodup_nil⟩, ?_⟩
    intro a ha b hb
    simp only [List.mem_singleton] at hb
    subst hb
    intro hab
    subst hab
    apply hany
    obtain ⟨fd, hfd, rfl⟩ := List.mem_map.mp ha
    exact List.any_eq_true.mpr ⟨fd, hfd, by simp⟩

theorem ensureField_declared (s : Schema) (f : FieldId) : declaredIn (ensureField s f) f = true := by
  unfold ensureField declaredIn
  by_cases hany : s.any (fun fd => fd.1 == f) = true
  · rw [if_pos hany]; exact hany
  · rw [if_neg hany]
    simp [List.any_append]

/-- Every field the schema declared stays declared. -/
theorem ensureField_mem (s : Schema) (f : FieldId) (fd : FieldId × FieldDef) (h : fd ∈ s) :
    fd ∈ ensureField s f := by
  unfold ensureField
  split
  · exact h
  · exact List.mem_append_left _ h


theorem ensureField_eq_of_lookup (s : Schema) (f : FieldId) (d : FieldDef) (h : s.lookup f = some d) :
    ensureField s f = s := by
  unfold ensureField
  have hd : s.any (fun fd => fd.1 == f) = true := sc_declared_of_lookup s f d h
  rw [if_pos hd]


/-- A schema change decided on two tenants that agree on what it reads (the right to manage the schema, the schema
    version, the object types with their records, the policies and the limits) leaves the same object types, or is
    refused the same way. -/
theorem changeSchemaT_types_congr (valid : List (TypeId × ObjType) → Bool) (t1 t2 : Tenant) (p : Principal)
    (strict : Bool) (ev : Option Nat) (cs : List Change) (hm : canManage t1 p = canManage t2 p)
    (hv : t1.schemaVersion = t2.schemaVersion) (ht : t1.types = t2.types) (hp : t1.policies = t2.policies)
    (hl : t1.limits = t2.limits) :
    (changeSchemaT valid t1 p strict ev cs).map (fun t' => t'.types) =
      (changeSchemaT valid t2 p strict ev cs).map (fun t' => t'.types) := by
  unfold changeSchemaT
  rw [hm, hv, ht, hp, hl]
  repeat' (first | rfl | split)
  all_goals (dsimp only)
  repeat' (first | rfl | split)

end Strata
