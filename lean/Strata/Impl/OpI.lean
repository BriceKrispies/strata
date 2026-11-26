/-
The executable kernel, stage 1: an accepted operation invocation.

An invocation changes only the object types' records, the record counter and the ledger (the steps of
`Strata/Operations.lean`). The implementation does not replay the steps on slots: it asks the spec for the
tenant the invocation leaves and rebuilds the slot arrays from the records it holds (`rebuildI`). The facts
that make the rebuilt tenant read back as the spec's are proved here: the records of every type stay
strictly descending by id and below the counter through every step.
-/

import Strata.Uniqueness
import Strata.Impl.Tenant

namespace Strata
namespace Impl

/-- The slot array of size `n` whose slot `i` holds the record with id `i` of `rs`. -/
def slotsOfRecords (n : Nat) (rs : Records) : Array (Option Record) :=
  (Array.range n).map (fun i => rs.lookup i)

/-- The size of a slot array for the records `rs` under a counter `n`: `n`, unless a record's id is not below
    it (only a type the tenant lists twice, shadowed by its first entry, can hold such a record). -/
def slotBound (n : Nat) (rs : Records) : Nat := rs.foldl (fun m q => max m (q.1 + 1)) n

/-- The implementation object type for a spec one, its slots rebuilt for a counter of `n`. -/
def IObj.ofSpec (n : Nat) (o : ObjType) : IObj :=
  { schema := o.schema, uniques := o.uniques, slots := slotsOfRecords (slotBound n o.records) o.records,
    constraints := o.constraints, workflows := o.workflows }

/-- The implementation tenant `t` with the object types, the counter and the ledger of the spec tenant `t'`
    (what an accepted invocation changes); everything else is `t`'s. -/
def rebuildI (t : ITenant) (t' : Tenant) : ITenant :=
  { t with types := t'.types.map (fun e => (e.1, IObj.ofSpec t'.nextId e.2)),
           nextId := t'.nextId, ledger := t'.ledger }

/-- Records are strictly descending by id. -/
def RecDesc (rs : Records) : Prop := rs.Pairwise (fun a b => b.1 < a.1)

/-- Every object type entry's records are strictly descending by id. -/
def _root_.Strata.Tenant.Desc (t : Tenant) : Prop := ∀ e ∈ t.types, RecDesc e.2.records

theorem ids_ge_of_mem_ascend (b : Nat) (L : List (Option Record)) (q : RecordId × Record)
    (h : q ∈ ascend b L) : b ≤ q.1 := by
  obtain ⟨i, r, rfl, -⟩ := (mem_ascend q L b).mp h
  simp

theorem ascend_desc (L : List (Option Record)) : ∀ b, RecDesc (ascend b L) := by
  induction L with
  | nil => intro b; exact List.Pairwise.nil
  | cons o os ih =>
    intro b
    unfold RecDesc
    rw [ascend, List.pairwise_append]
    refine ⟨ih (b + 1), ?_, ?_⟩
    · cases o <;> simp [cellOf]
    · intro a ha c hc
      have h1 := ids_ge_of_mem_ascend _ _ _ ha
      cases o with
      | none => simp [cellOf] at hc
      | some x =>
        simp only [cellOf, List.mem_singleton] at hc
        subst hc
        exact h1

theorem absSlots_descending (a : Array (Option Record)) : RecDesc (absSlots a) :=
  ascend_desc _ 0

theorem absSlots_ids_lt (a : Array (Option Record)) (q : RecordId × Record) (h : q ∈ absSlots a) :
    q.1 < a.size := by
  obtain ⟨i, r, rfl, hi⟩ := (mem_ascend q a.toList 0).mp h
  have := (List.getElem?_eq_some_iff.mp hi).1
  simpa using this

theorem lookup_mem_rec (rs : Records) (k : RecordId) (r : Record) (h : rs.lookup k = some r) :
    (k, r) ∈ rs := by
  induction rs with
  | nil => simp [List.lookup] at h
  | cons x xs ih =>
    obtain ⟨a, y⟩ := x
    simp only [List.lookup] at h
    by_cases hk : (k == a) = true
    · simp only [hk] at h
      have hka : k = a := by simpa using hk
      subst hka
      cases h
      simp
    · simp only [Bool.not_eq_true] at hk
      simp only [hk] at h
      exact List.mem_cons_of_mem _ (ih h)

theorem lookup_eq_of_desc (l1 l2 : Records) (h1 : RecDesc l1) (h2 : RecDesc l2)
    (h : ∀ k, l1.lookup k = l2.lookup k) : l1 = l2 := by
  induction l1 generalizing l2 with
  | nil =>
    cases l2 with
    | nil => rfl
    | cons y t2 =>
      obtain ⟨b, y⟩ := y
      have := h b
      simp [List.lookup] at this
  | cons x t1 ih =>
    obtain ⟨a, x⟩ := x
    cases l2 with
    | nil =>
      have := h a
      simp [List.lookup] at this
    | cons y t2 =>
      obtain ⟨b, y⟩ := y
      unfold RecDesc at h1 h2
      rw [List.pairwise_cons] at h1 h2
      -- the head of a descending list has the greatest id
      have hab : a ≤ b := by
        have hl : ((a, x) :: t1).lookup a = some x := by simp [List.lookup]
        have hm := lookup_mem_rec ((b, y) :: t2) a x (by rw [← h a]; exact hl)
        rcases List.mem_cons.mp hm with he | hm'
        · exact Nat.le_of_eq (congrArg Prod.fst he)
        · exact Nat.le_of_lt (h2.1 _ hm')
      have hba : b ≤ a := by
        have hl : ((b, y) :: t2).lookup b = some y := by simp [List.lookup]
        have hm := lookup_mem_rec ((a, x) :: t1) b y (by rw [h b]; exact hl)
        rcases List.mem_cons.mp hm with he | hm'
        · exact Nat.le_of_eq (congrArg Prod.fst he)
        · exact Nat.le_of_lt (h1.1 _ hm')
      have hEq : a = b := Nat.le_antisymm hab hba
      subst hEq
      have hxy : x = y := by
        have hl := h a
        simpa [List.lookup] using hl
      subst hxy
      have ht : t1 = t2 := by
        apply ih t2 h1.2 h2.2
        intro k
        by_cases hk : k = a
        · subst hk
          have n1 : t1.lookup k = none := by
            cases hl : t1.lookup k with
            | none => rfl
            | some z => exact absurd (h1.1 _ (lookup_mem_rec t1 k z hl)) (by simp)
          have n2 : t2.lookup k = none := by
            cases hl : t2.lookup k with
            | none => rfl
            | some z => exact absurd (h2.1 _ (lookup_mem_rec t2 k z hl)) (by simp)
          rw [n1, n2]
        · have hl := h k
          have hb : (k == a) = false := by simpa using hk
          simpa [List.lookup, hb] using hl
      rw [ht]

theorem getRec_slotsOfRecords (n : Nat) (rs : Records) (k : Nat) :
    getRec (slotsOfRecords n rs) k = if k < n then rs.lookup k else none := by
  unfold getRec slotsOfRecords
  by_cases hk : k < n
  · simp [hk]
  · simp [hk]

/-- The slots rebuilt from the records read back as those records, when they are strictly descending by id
    and below the counter. -/
theorem absSlots_slotsOfRecords (n : Nat) (rs : Records) (hd : RecDesc rs) (hb : ∀ q ∈ rs, q.1 < n) :
    absSlots (slotsOfRecords n rs) = rs := by
  apply lookup_eq_of_desc _ _ (absSlots_descending _) hd
  intro k
  rw [lookup_absSlots, getRec_slotsOfRecords]
  by_cases hk : k < n
  · simp [hk]
  · simp only [hk, if_false]
    cases hl : rs.lookup k with
    | none => rfl
    | some z => exact absurd (hb _ (lookup_mem_rec rs k z hl)) hk


/-! ## The slot bound -/

theorem slotBound_ge (rs : Records) : ∀ n, n ≤ slotBound n rs := by
  induction rs with
  | nil => intro n; exact Nat.le_refl _
  | cons q rs ih =>
    intro n
    unfold slotBound at *
    simp only [List.foldl_cons]
    exact Nat.le_trans (Nat.le_max_left _ _) (ih _)

theorem lt_slotBound (rs : Records) : ∀ n (q : RecordId × Record), q ∈ rs → q.1 < slotBound n rs := by
  induction rs with
  | nil => intro n q h; simp at h
  | cons x rs ih =>
    intro n q h
    unfold slotBound at *
    simp only [List.foldl_cons]
    rcases List.mem_cons.mp h with rfl | h'
    · exact Nat.lt_of_lt_of_le (Nat.lt_of_lt_of_le (Nat.lt_succ_self _) (Nat.le_max_right _ _))
        (slotBound_ge rs _)
    · exact ih _ q h'

theorem slotBound_of_lt (rs : Records) : ∀ n, (∀ q ∈ rs, q.1 < n) → slotBound n rs = n := by
  induction rs with
  | nil => intro n _; rfl
  | cons x rs ih =>
    intro n h
    unfold slotBound at *
    simp only [List.foldl_cons]
    have h1 : max n (x.1 + 1) = n := Nat.max_eq_left (h x (List.mem_cons_self))
    rw [h1]
    exact ih n (fun q hq => h q (List.mem_cons_of_mem _ hq))

theorem IObj.abs_ofSpec (n : Nat) (o : ObjType) (hd : RecDesc o.records) :
    (IObj.ofSpec n o).abs = o := by
  unfold IObj.abs IObj.ofSpec
  simp only [absSlots_slotsOfRecords _ o.records hd (fun q hq => lt_slotBound o.records n q hq)]

/-! ## What a step keeps -/

theorem mem_putTypeList (l : List (TypeId × ObjType)) (ty : TypeId) (o : ObjType) (e : TypeId × ObjType)
    (h : e ∈ putTypeList l ty o) : e ∈ l ∨ e = (ty, o) := by
  induction l with
  | nil => simp [putTypeList] at h
  | cons x xs ih =>
    obtain ⟨k, p⟩ := x
    simp only [putTypeList] at h
    by_cases hk : (k == ty) = true
    · simp only [hk, if_true] at h
      rcases List.mem_cons.mp h with he | he
      · exact Or.inr he
      · exact Or.inl (List.mem_cons_of_mem _ he)
    · simp only [hk] at h
      rcases List.mem_cons.mp h with he | he
      · exact Or.inl (by rw [he]; exact List.mem_cons_self)
      · rcases ih he with h1 | h1
        · exact Or.inl (List.mem_cons_of_mem _ h1)
        · exact Or.inr h1

theorem recDesc_map_id (f : RecordId × Record → RecordId × Record) (hf : ∀ q, (f q).1 = q.1) (rs : Records)
    (h : RecDesc rs) : RecDesc (rs.map f) := by
  unfold RecDesc at *
  rw [List.pairwise_map]
  exact h.imp (fun {a b} hab => by rw [hf, hf]; exact hab)

theorem recDesc_filter (p : RecordId × Record → Bool) (rs : Records) (h : RecDesc rs) :
    RecDesc (rs.filter p) := List.Pairwise.filter p h

theorem delObj_facts (ty id j : TypeId) (o : ObjType) (hd : RecDesc o.records) :
    RecDesc (delObj ty id j o).records ∧ ∀ q ∈ (delObj ty id j o).records, ∃ q₀ ∈ o.records, q.1 = q₀.1 := by
  have hk : RecDesc (if j == ty then o.records.filter (fun q => q.1 != id) else o.records) := by
    split
    · exact recDesc_filter _ _ hd
    · exact hd
  have hmem : ∀ q ∈ (if j == ty then o.records.filter (fun q => q.1 != id) else o.records), q ∈ o.records := by
    intro q hq
    split at hq
    · exact (List.mem_filter.mp hq).1
    · exact hq
  refine ⟨?_, ?_⟩
  · exact recDesc_map_id (fun q => (q.1, clearRef o.schema ty id q.2)) (fun q => rfl) _ hk
  · intro q hq
    unfold delObj delClear at hq
    obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
    exact ⟨q₀, hmem q₀ hq₀, rfl⟩

theorem afterDelete_facts (t : Tenant) (ty id : TypeId) (hids : t.IdsBelow) (hdesc : t.Desc) :
    (t.afterDelete ty id).IdsBelow ∧ (t.afterDelete ty id).Desc ∧
      (t.afterDelete ty id).types.map Prod.fst = t.types.map Prod.fst := by
  refine ⟨?_, ?_, types_keys_afterDelete t ty id⟩
  · intro T o' hT' q hq
    rw [typ?_afterDelete] at hT'
    cases ho : t.typ? T with
    | none => rw [ho] at hT'; cases hT'
    | some o =>
      rw [ho] at hT'
      simp only [Option.map_some, Option.some.injEq] at hT'
      subst hT'
      have hmem : (T, o) ∈ t.types := lookup_mem ho
      obtain ⟨q₀, hq₀, he⟩ := (delObj_facts ty id T o (hdesc _ hmem)).2 q hq
      rw [he]
      exact hids T o ho q₀ hq₀
  · intro e he
    change e ∈ t.types.map (fun e => (e.1, delObj ty id e.1 e.2)) at he
    obtain ⟨e0, he0, rfl⟩ := List.mem_map.mp he
    exact (delObj_facts ty id e0.1 e0.2 (hdesc e0 he0)).1

/-- A step of an operation keeps every record id below the counter, the records of each type strictly
    descending by id, and the object types' keys. -/
theorem opStep_facts (c : OpCtx) (st st' : OpState) (s : OpStep) (hids : st.tenant.IdsBelow)
    (hdesc : st.tenant.Desc) (h : opStep c st s = .ok st') :
    st'.tenant.IdsBelow ∧ st'.tenant.Desc ∧ st'.tenant.types.map Prod.fst = st.tenant.types.map Prod.fst := by
  cases s with
  | create id ty fs =>
    cases hty : st.tenant.typ? ty with
    | none => simp [opStep, opCreate, hty] at h
    | some o =>
      obtain ⟨vals, r, hN, hP, -, ht⟩ := opCreate_ok_parts hty h
      rw [ht]
      refine ⟨?_, ?_, types_keys_putType _ _ _⟩
      · intro T o' hT' q hq
        change (st.tenant.putType ty { o with records := (st.tenant.nextId, r) :: o.records }).typ? T = some o' at hT'
        by_cases hTy : T = ty
        · subst hTy
          rw [typ?_putType_self st.tenant T o _ hty] at hT'
          cases hT'
          rcases List.mem_cons.mp hq with rfl | hq'
          · exact Nat.lt_succ_self _
          · exact Nat.lt_succ_of_lt (hids T o hty q hq')
        · rw [typ?_putType_other st.tenant ty T _ hTy] at hT'
          exact Nat.lt_succ_of_lt (hids T o' hT' q hq)
      · intro e he
        change e ∈ putTypeList st.tenant.types ty { o with records := (st.tenant.nextId, r) :: o.records } at he
        rcases mem_putTypeList _ _ _ _ he with he' | rfl
        · exact hdesc e he'
        · unfold RecDesc
          rw [List.pairwise_cons]
          exact ⟨fun q hq => hids ty o hty q hq, hdesc _ (lookup_mem hty)⟩
  | update id ty rec fs =>
    cases hty : st.tenant.typ? ty with
    | none => simp [opStep, opUpdate, hty] at h
    | some o =>
      cases hid : opTarget c st.params rec with
      | error e => simp [opStep, opUpdate, hty, hid] at h
      | ok id' =>
        cases hL : o.records.lookup id' with
        | none => simp [opStep, opUpdate, hty, hid, hL] at h
        | some old =>
          obtain ⟨vals, r, -, -, hP, -, -, ht⟩ := opUpdate_ok_parts_gen hty hid hL h
          rw [ht]
          have hf : ∀ q : RecordId × Record, ((fun q : RecordId × Record => if q.1 == id' then (q.1, r) else q) q).1 = q.1 := by
            intro q
            dsimp only
            split <;> rfl
          refine ⟨?_, ?_, types_keys_putType _ _ _⟩
          · intro T o' hT' q hq
            change (st.tenant.putType ty { o with records := o.records.map (fun q =>
              if q.1 == id' then (q.1, r) else q) }).typ? T = some o' at hT'
            by_cases hTy : T = ty
            · subst hTy
              rw [typ?_putType_self st.tenant T o _ hty] at hT'
              cases hT'
              obtain ⟨q₀, hq₀, rfl⟩ := List.mem_map.mp hq
              rw [hf]
              exact hids T o hty q₀ hq₀
            · rw [typ?_putType_other st.tenant ty T _ hTy] at hT'
              exact hids T o' hT' q hq
          · intro e he
            change e ∈ putTypeList st.tenant.types ty { o with records := o.records.map (fun q =>
              if q.1 == id' then (q.1, r) else q) } at he
            rcases mem_putTypeList _ _ _ _ he with he' | rfl
            · exact hdesc e he'
            · exact recDesc_map_id _ hf _ (hdesc _ (lookup_mem hty))
  | delete id ty rec =>
    simp only [opStep, opDelete] at h
    repeat' split at h
    all_goals (try (simp at h; done))
    all_goals (simp only [Except.ok.injEq] at h; rw [← h])
    all_goals exact afterDelete_facts _ _ _ hids hdesc
  | post id owner unit amt reason =>
    simp only [opStep, opPost] at h
    repeat' split at h
    all_goals (try (simp at h; done))
    all_goals (simp only [Except.ok.injEq] at h; rw [← h]; exact ⟨hids, hdesc, rfl⟩)

/-! ## What an invocation keeps, and the rebuilt tenant -/

/-- What an accepted invocation of operation `name` on `t` keeps: the ids below the counter, the records
    strictly descending, the object types' keys, and every other field of the tenant. -/
theorem invoke_facts {t t' : Tenant} {p : Principal} {now : Int} {name : String}
    {ins : List (String × Val)} {key : String} (hids : t.IdsBelow) (hdesc : t.Desc)
    (h : invokeOperationT t p now name ins key = .ok t') :
    t'.IdsBelow ∧ t'.Desc ∧ t'.types.map Prod.fst = t.types.map Prod.fst ∧
      t' = { t with types := t'.types, nextId := t'.nextId, ledger := t'.ledger } := by
  have key : (fun x : Tenant => x.IdsBelow ∧ x.Desc ∧ x.types.map Prod.fst = t.types.map Prod.fst ∧
      { t with types := x.types, nextId := x.nextId, ledger := x.ledger } = x) t' :=
    invokeOperationT_pres (fun x : Tenant => x.IdsBelow ∧ x.Desc ∧ x.types.map Prod.fst = t.types.map Prod.fst ∧
      { t with types := x.types, nextId := x.nextId, ledger := x.ledger } = x)
      (fun c s st st' hs hp => by
        obtain ⟨a1, a2, a3⟩ := opStep_facts c st st' s hp.1 hp.2.1 hs
        refine ⟨a1, a2, a3.trans hp.2.2.1, ?_⟩
        obtain ⟨ty, nid, L, hsh⟩ := opStep_shape c st st' s hs
        have e := hp.2.2.2
        rw [hsh]
        show { t with types := ty, nextId := nid, ledger := L } = { st.tenant with types := ty, nextId := nid, ledger := L }
        calc { t with types := ty, nextId := nid, ledger := L }
            = { { t with types := st.tenant.types, nextId := st.tenant.nextId, ledger := st.tenant.ledger } with
                types := ty, nextId := nid, ledger := L } := rfl
          _ = _ := by rw [e])
      ⟨hids, hdesc, rfl, rfl⟩ h
  exact ⟨key.1, key.2.1, key.2.2.1, key.2.2.2.symm⟩

/-- An implementation tenant satisfying its invariant stands for a spec tenant with ids below the counter. -/
theorem ITenant.abs_idsBelow (t : ITenant) (hinv : t.Inv) : t.abs.IdsBelow := by
  intro T o hT q hq
  have hT' : (t.types.map absEntry).lookup T = some o := hT
  rw [lookup_map_abs] at hT'
  cases ho : t.typ? T with
  | none => unfold ITenant.typ? at ho; rw [ho] at hT'; cases hT'
  | some io =>
    unfold ITenant.typ? at ho
    rw [ho] at hT'
    simp only [Option.map_some, Option.some.injEq] at hT'
    subst hT'
    have := absSlots_ids_lt io.slots q hq
    have h2 := hinv T io ho
    exact Nat.lt_of_lt_of_le this h2

/-- The records an implementation tenant reads back as are strictly descending, whatever its invariant. -/
theorem ITenant.abs_desc (t : ITenant) : t.abs.Desc := by
  intro e he
  change e ∈ t.types.map absEntry at he
  obtain ⟨e0, -, rfl⟩ := List.mem_map.mp he
  exact absSlots_descending e0.2.slots

/-- The tenant rebuilt from what an accepted invocation leaves reads back as it. -/
theorem rebuildI_abs (t : ITenant) (t' : Tenant) (p : Principal) (now : Int) (name : String)
    (ins : List (String × Val)) (key : String) (hinv : t.Inv)
    (h : invokeOperationT t.abs p now name ins key = .ok t') : (rebuildI t t').abs = t' := by
  obtain ⟨-, hd, -, hrest⟩ := invoke_facts (t.abs_idsBelow hinv) t.abs_desc h
  have ht : (t'.types.map (fun e => (e.1, IObj.ofSpec t'.nextId e.2))).map absEntry = t'.types := by
    rw [List.map_map]
    conv => rhs; rw [← List.map_id t'.types]
    apply List.map_congr_left
    intro e he
    obtain ⟨k, o⟩ := e
    simp only [Function.comp, absEntry, id]
    rw [IObj.abs_ofSpec _ _ (hd (k, o) he)]
  calc (rebuildI t t').abs
      = { t.abs with types := (t'.types.map (fun e => (e.1, IObj.ofSpec t'.nextId e.2))).map absEntry,
                     nextId := t'.nextId, ledger := t'.ledger } := rfl
    _ = { t.abs with types := t'.types, nextId := t'.nextId, ledger := t'.ledger } := by rw [ht]
    _ = t' := hrest.symm

/-- The rebuilt tenant keeps the slot invariant: each first entry's records are below the counter. -/
theorem rebuildI_inv (t : ITenant) (t' : Tenant) (hids : t'.IdsBelow) : (rebuildI t t').Inv := by
  intro T o hT
  have hT' : (t'.types.map (fun e => (e.1, IObj.ofSpec t'.nextId e.2))).lookup T = some o := hT
  rw [Impl.lookup_map_entry t'.types (fun _ o => IObj.ofSpec t'.nextId o) T] at hT'
  cases ho : t'.typ? T with
  | none => unfold Tenant.typ? at ho; rw [ho] at hT'; cases hT'
  | some o' =>
    unfold Tenant.typ? at ho
    rw [ho] at hT'
    simp only [Option.map_some, Option.some.injEq] at hT'
    subst hT'
    show (slotsOfRecords _ _).size ≤ t'.nextId
    rw [slotBound_of_lt _ _ (hids T o' ho)]
    simp [slotsOfRecords]

end Impl
end Strata
