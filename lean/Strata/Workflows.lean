/-
Workflows (slice 8): the theorems about the workflow decisions of the model.

A workflow names a string field of an object type with its initial states and its transitions. The
model's decisions are in `Strata/ModelBase.lean` (`fireOne`, `fireAll`, `initialsOk`, `statesOk`) and
`Strata/Model.lean` (the create and update branches of `stepTenant`). This file proves that

- a create starts every workflow field in an initial state, and is refused when it does not;
- an accepted update that changes a workflow field by its patch took a declared transition from the
  stored state to the new one, whose guard held on the candidate the step saw, and an update that has
  no such transition is refused;
- the assignments of a fired transition are written, and a record the workflows changed is authorised
  again;
- every accepted step keeps every stored record's workflow field one of its states, and holds a state
  there (a field that is absent or `null` is not one).

The rule that makes the first two hold is the kernel's definition-time refusal (`WfDefined`): a
transition of a workflow may not assign the field of another workflow of its type, and may assign the field of
its own workflow only a string literal that is one of the workflow's states (a `null` or a computed value is
refused). The kernel's schema check refuses such a definition (`TenantSchema::problems`, one workflow), so every
object type the kernel holds satisfies it; the model has no definition step, so the theorems take it as the
hypothesis `WfDefined` (for the object type) or `Tenant.WfDefs` / `World.WfDefs` (for the state). A changed workflow
field then holds the target of its transition, or the state named by the transition's own assignment of it.

The statements that describe a write as the patched record keep their hypothesis `Tenant.Plain`
(`Strata/WorkflowLemmas.lean`), because an assignment changes the record written.
-/
import Strata.Model
import Strata.References
import Strata.Uniqueness
import Strata.StepParts
import Strata.Writes
import Strata.WorkflowLemmas

namespace Strata

/-- The definition-time rule of the kernel (`TenantSchema::problems`, the workflow loop), for the workflows `ws` of a
    type: their ids are distinct; a transition of a workflow assigns no field of another workflow of the type; a
    transition's assignments are distinct fields; and a transition assigns the field of its own workflow only a
    string literal that is one of the workflow's states. Under this rule a write changes a workflow field only by
    the transition that takes it, and the value the field then holds is the target of that transition or a state
    its own assignment names. -/
def WfDefined (ws : List Workflow) : Prop :=
  (ws.map (fun w => w.id)).Nodup ∧
    ∀ w ∈ ws, ∀ tr ∈ w.transitions, (tr.set.map Prod.fst).Nodup ∧
      ∀ fe ∈ tr.set, (∀ v ∈ ws, v.id ≠ w.id → fe.1 ≠ v.field) ∧
        (fe.1 = w.field → ∃ s, fe.2 = .lit (.str s) ∧ s ∈ w.states)

theorem wfDefined_tail {w : Workflow} {ws : List Workflow} (h : WfDefined (w :: ws)) :
    WfDefined ws := by
  obtain ⟨hnd, hr⟩ := h
  refine ⟨(List.nodup_cons.mp hnd).2, fun w' hw' tr htr => ?_⟩
  obtain ⟨hn, hfe⟩ := hr w' (List.mem_cons_of_mem _ hw') tr htr
  exact ⟨hn, fun fe hm => ⟨fun v hv hne => (hfe fe hm).1 v (List.mem_cons_of_mem _ hv) hne, (hfe fe hm).2⟩⟩

/-- The ids of the workflows of a definition rule are distinct, so a workflow of the tail differs from the head. -/
theorem id_ne_of_nodup {w : Workflow} {ws : List Workflow} (h : ((w :: ws).map fun v => v.id).Nodup)
    {w' : Workflow} (hw' : w' ∈ ws) : w'.id ≠ w.id := by
  intro heq
  apply (List.nodup_cons.mp h).1
  show w.id ∈ List.map (fun v => v.id) ws
  rw [← heq]
  exact List.mem_map_of_mem (f := fun v => v.id) hw'

/-- Under the rule, a transition of the head workflow assigns no field of a workflow of the tail. -/
theorem wfDefined_cross {w0 : Workflow} {ws : List Workflow} (h : WfDefined (w0 :: ws)) {w : Workflow}
    (hw : w ∈ ws) {tr : WfTransition} (htr : tr ∈ w0.transitions) {fe : FieldId × Expr} (hfe : fe ∈ tr.set) :
    fe.1 ≠ w.field :=
  ((h.2 w0 List.mem_cons_self tr htr).2 fe hfe).1 w (List.mem_cons_of_mem _ hw) (id_ne_of_nodup h.1 hw)

/-- Under the rule, a transition of a workflow of the tail assigns no field of the head workflow. -/
theorem wfDefined_foreign {w0 : Workflow} {ws : List Workflow} (h : WfDefined (w0 :: ws)) {w : Workflow}
    (hw : w ∈ ws) {tr : WfTransition} (htr : tr ∈ w.transitions) {fe : FieldId × Expr} (hfe : fe ∈ tr.set) :
    fe.1 ≠ w0.field :=
  ((h.2 w (List.mem_cons_of_mem _ hw) tr htr).2 fe hfe).1 w0 List.mem_cons_self (id_ne_of_nodup h.1 hw).symm

/-- Every object type of the tenant is defined by the rule (the kernel refuses a definition that breaks it). -/
def Tenant.WfDefs (t : Tenant) : Prop :=
  ∀ T o, t.typ? T = some o → WfDefined o.workflows

/-- Every tenant of the world is defined by the rule. -/
def World.WfDefs (w : World) : Prop :=
  ∀ x t, w x = some t → t.WfDefs

/-! ## Fields: reading after a write -/

theorem wf_lookup_filter_none (fs : Fields) (f : FieldId) :
    (fs.filter (fun g => g.1 != f)).lookup f = none := by
  rw [List.lookup_eq_none_iff]
  intro p hp
  have h2 : (p.1 != f) = true := (List.mem_filter.mp hp).2
  exact bne_iff_ne.mpr (Ne.symm (bne_iff_ne.mp h2))

theorem lookup_filter_of_ne (es : Fields) (f g : FieldId) (hne : g ≠ f) :
    (es.filter (fun x => x.1 != g)).lookup f = es.lookup f := by
  induction es with
  | nil => rfl
  | cons e es ih =>
    by_cases he : e.1 = g
    · rw [List.filter_cons_of_neg (by simp [he]), ih]
      have hfe : (f == e.1) = false := by simp [he, Ne.symm hne]
      rw [List.lookup_cons, hfe]
    · rw [List.filter_cons_of_pos (by simpa using he), List.lookup_cons, List.lookup_cons, ih]

theorem get_eq_null_of_lookup_none {fs : Fields} {f : FieldId} (h : fs.lookup f = none) :
    fs.get f = .null := by
  unfold Fields.get
  rw [h]

/-- Setting a field to a value, the field reads that value (a `null` removes it, and reads `null`). -/
theorem get_set_self (fs : Fields) (f : FieldId) (u : Val) : (Fields.set fs (f, u)).get f = u := by
  by_cases hu : u = .null
  · subst hu
    simp [Fields.set, Fields.get, wf_lookup_filter_none]
  · simp [Fields.set, Fields.get, hu, List.lookup]

/-- Setting another field leaves this one as it was. -/
theorem get_set_ne (fs : Fields) (f g : FieldId) (u : Val) (hne : g ≠ f) :
    (Fields.set fs (g, u)).get f = fs.get f := by
  by_cases hu : u = .null
  · subst hu
    simp [Fields.set, Fields.get, lookup_filter_of_ne fs f g hne]
  · have hfg : (f == g) = false := by simp [Ne.symm hne]
    simp [Fields.set, Fields.get, hu, List.lookup, hfg, lookup_filter_of_ne fs f g hne]

/-- Assignments to fields other than `f` leave the value of `f`. -/
theorem applySets_get_other (f : FieldId) : ∀ (as : List (FieldId × Val)) (r : Record),
    (∀ fv ∈ as, fv.1 ≠ f) → (applySets r as).fields.get f = r.fields.get f
  | [], _, _ => rfl
  | fv :: as, r, h => by
    show (applySets { r with fields := Fields.set r.fields fv } as).fields.get f = r.fields.get f
    rw [applySets_get_other f as _ (fun x hx => h x (List.mem_cons_of_mem _ hx))]
    exact get_set_ne _ f fv.1 fv.2 (h fv List.mem_cons_self)

/-- Assignments give each assigned field the value it was assigned, when no field is assigned twice. -/
theorem applySets_get_mem : ∀ (as : List (FieldId × Val)) (r : Record),
    (as.map Prod.fst).Nodup → ∀ fv ∈ as, (applySets r as).fields.get fv.1 = fv.2
  | [], _, _, fv, h => by simp at h
  | g :: as, r, hnd, fv, h => by
    show (applySets { r with fields := Fields.set r.fields g } as).fields.get fv.1 = fv.2
    have hnd' := List.nodup_cons.mp hnd
    rcases List.mem_cons.mp h with rfl | htail
    · rw [applySets_get_other fv.1 as _ ?_]
      · exact get_set_self _ _ _
      · intro x hx hxe
        apply hnd'.1
        rw [List.mem_map]
        exact ⟨x, hx, hxe⟩
    · exact applySets_get_mem as _ hnd'.2 fv htail

/-! ## The evaluation of a transition's assignments -/

/-- The search for a transition gives a transition of the list whose source and target are the states it
    searched for and whose guard holds. -/
theorem findTransition_spec (env : ExprEnv) (was becomes : Val) :
    ∀ (trs : List WfTransition) (tr : WfTransition), findTransition env was becomes trs = some tr →
      tr ∈ trs ∧ was = .str tr.source ∧ becomes = .str tr.target ∧ guardHolds env tr.guard = true
  | [], tr, h => by simp [findTransition] at h
  | t :: ts, tr, h => by
    unfold findTransition at h
    split at h
    · rename_i hc
      cases h
      simp only [Bool.and_eq_true, beq_iff_eq] at hc
      exact ⟨List.mem_cons_self, hc.1.1, hc.1.2, hc.2⟩
    · obtain ⟨h1, h2, h3, h4⟩ := findTransition_spec env was becomes ts tr h
      exact ⟨List.mem_cons_of_mem _ h1, h2, h3, h4⟩

/-- The assignments of an accepted evaluation have the fields of the transition, in the same order. -/
theorem evalSets_keys (env : ExprEnv) : ∀ (set : List (FieldId × Expr)) (as : List (FieldId × Val)),
    evalSets env set = .ok as → as.map Prod.fst = set.map Prod.fst
  | [], as, h => by
    have h' : (Except.ok ([] : List (FieldId × Val)) : Except Unit (List (FieldId × Val))) =
        Except.ok as := by simpa [evalSets] using h
    have hh := Except.ok.inj h'
    subst hh
    rfl
  | (g, e) :: es, as, h => by
    unfold evalSets at h
    cases he : evalExpr env e with
    | error u => simp [he] at h
    | ok v =>
      cases hes : evalSets env es with
      | error u => simp [he, hes] at h
      | ok vs =>
        simp only [he, hes, Except.ok.injEq] at h
        subst h
        simp [evalSets_keys env es vs hes]

/-- The values of an accepted evaluation of assignments: each assignment has its value in the list. -/
theorem evalSets_mem (env : ExprEnv) : ∀ (set : List (FieldId × Expr)) (as : List (FieldId × Val)),
    evalSets env set = .ok as → ∀ fe ∈ set, ∃ v, evalExpr env fe.2 = .ok v ∧ (fe.1, v) ∈ as
  | [], as, h, fe, hf => by simp at hf
  | (g, e) :: es, as, h, fe, hf => by
    unfold evalSets at h
    cases he : evalExpr env e with
    | error u => simp [he] at h
    | ok v =>
      cases hes : evalSets env es with
      | error u => simp [he, hes] at h
      | ok vs =>
        simp only [he, hes, Except.ok.injEq] at h
        subst h
        rcases List.mem_cons.mp hf with rfl | htail
        · exact ⟨v, he, List.mem_cons_self⟩
        · obtain ⟨v', hv', hm⟩ := evalSets_mem env es vs hes fe htail
          exact ⟨v', hv', List.mem_cons_of_mem _ hm⟩

/-! ## One workflow -/

/-- A workflow that changes the state of its field takes a declared transition from the old state to the
    new one whose guard holds, on the environment the step reads; its assignments are then written, each
    with the value its expression has (when the assigned fields are distinct). -/
theorem fireOne_declared {p : Principal} {old cand : Record} {w : Workflow} {r : Record}
    (h : fireOne p old cand w = .ok r) (hch : old.fields.get w.field ≠ cand.fields.get w.field) :
    ∃ tr ∈ w.transitions, old.fields.get w.field = .str tr.source ∧
      cand.fields.get w.field = .str tr.target ∧
      guardHolds { record := some old, old := some old, new := some cand, principal := p.userId? }
        tr.guard = true ∧
      ((tr.set.map Prod.fst).Nodup → ∀ fe ∈ tr.set, ∃ v,
        evalExpr { record := some old, old := some old, new := some cand, principal := p.userId? }
          fe.2 = .ok v ∧ r.fields.get fe.1 = v) := by
  unfold fireOne at h
  dsimp only at h
  have hbe : ¬ (old.fields.get w.field == cand.fields.get w.field) = true := by
    intro hb; exact hch (beq_iff_eq.mp hb)
  rw [if_neg hbe] at h
  split at h
  · cases h
  · rename_i tr hT
    obtain ⟨hmem, hs, ht, hg⟩ := findTransition_spec _ _ _ _ _ hT
    refine ⟨tr, hmem, hs, ht, hg, fun hnd fe hfe => ?_⟩
    split at h
    · cases h
    · rename_i as has
      have hr : r = applySets cand as := (Except.ok.inj h).symm
      obtain ⟨v, hv, hm⟩ := evalSets_mem _ tr.set as has fe hfe
      refine ⟨v, hv, ?_⟩
      rw [hr]
      have hk : (as.map Prod.fst).Nodup := by
        rw [evalSets_keys _ tr.set as has]
        exact hnd
      exact applySets_get_mem as cand hk (fe.1, v) hm

/-- A workflow leaves the field `f` as it is when none of its assignments names `f`. -/
theorem fireOne_keeps {p : Principal} {old cand : Record} {w : Workflow} {r : Record} {f : FieldId}
    (hf : ∀ tr ∈ w.transitions, ∀ fe ∈ tr.set, fe.1 ≠ f)
    (h : fireOne p old cand w = .ok r) : r.fields.get f = cand.fields.get f := by
  unfold fireOne at h
  dsimp only at h
  split at h
  · cases h; rfl
  · split at h
    · cases h
    · rename_i tr hT
      split at h
      · cases h
      · rename_i as has
        have hr := Except.ok.inj h
        rw [← hr]
        apply applySets_get_other f as cand
        intro fv hfv
        have hk := evalSets_keys _ tr.set as has
        have hmem : fv.1 ∈ tr.set.map Prod.fst := by
          rw [← hk]
          exact List.mem_map_of_mem hfv
        obtain ⟨fe, hfe, hfeq⟩ := List.mem_map.mp hmem
        rw [← hfeq]
        exact hf tr (findTransition_mem _ _ _ _ _ hT) fe hfe

/-! ## Every workflow of a write -/

/-- A workflow leaves the candidate as it is when the field it watches has the same value on both. -/
theorem fireOne_unchanged {p : Principal} {old cand : Record} {w : Workflow}
    (h : old.fields.get w.field = cand.fields.get w.field) : fireOne p old cand w = .ok cand := by
  simp [fireOne, h]

/-- One workflow that changes its field: a transition takes the field from its source to its target, and the
    field then holds the target, or the state that the transition's own assignment of the field gives (a string
    literal, by the rule). -/
theorem fireOne_field {p : Principal} {old cand : Record} {w : Workflow} {r : Record}
    (hW : ∀ tr ∈ w.transitions, (tr.set.map Prod.fst).Nodup ∧
      ∀ fe ∈ tr.set, fe.1 = w.field → ∃ s, fe.2 = .lit (.str s))
    (h : fireOne p old cand w = .ok r) (hch : old.fields.get w.field ≠ cand.fields.get w.field) :
    ∃ tr ∈ w.transitions, old.fields.get w.field = .str tr.source ∧ cand.fields.get w.field = .str tr.target ∧
      guardHolds { record := some old, old := some old, new := some cand, principal := p.userId? }
        tr.guard = true ∧
      ((∀ fe ∈ tr.set, fe.1 ≠ w.field) ∧ r.fields.get w.field = .str tr.target ∨
        ∃ s, r.fields.get w.field = .str s ∧ ∃ fe ∈ tr.set, fe.1 = w.field ∧ fe.2 = .lit (.str s)) := by
  unfold fireOne at h
  dsimp only at h
  have hbe : ¬ (old.fields.get w.field == cand.fields.get w.field) = true := by
    intro hb; exact hch (beq_iff_eq.mp hb)
  rw [if_neg hbe] at h
  split at h
  · cases h
  · rename_i tr hT
    obtain ⟨hmem, hs, ht, hg⟩ := findTransition_spec _ _ _ _ _ hT
    refine ⟨tr, hmem, hs, ht, hg, ?_⟩
    split at h
    · cases h
    · rename_i as has
      have hr : r = applySets cand as := (Except.ok.inj h).symm
      subst hr
      have hk := evalSets_keys _ tr.set as has
      have hknd : (as.map Prod.fst).Nodup := by
        rw [hk]; exact (hW tr hmem).1
      by_cases hown : ∃ fe ∈ tr.set, fe.1 = w.field
      · obtain ⟨fe, hfe, hfeq⟩ := hown
        obtain ⟨s, hlit⟩ := (hW tr hmem).2 fe hfe hfeq
        obtain ⟨v, hv, hm⟩ := evalSets_mem _ tr.set as has fe hfe
        have hvs : v = .str s := by
          rw [hlit] at hv
          simp [evalExpr] at hv
          exact hv.symm
        have hget := applySets_get_mem as cand hknd (fe.1, v) hm
        right
        refine ⟨s, ?_, fe, hfe, hfeq, hlit⟩
        rw [← hfeq, ← hvs]
        exact hget
      · left
        refine ⟨fun fe hfe hf => hown ⟨fe, hfe, hf⟩, ?_⟩
        have hnot : ∀ fv ∈ as, fv.1 ≠ w.field := by
          intro fv hfv
          have hm : fv.1 ∈ tr.set.map Prod.fst := by
            rw [← hk]; exact List.mem_map_of_mem (f := Prod.fst) hfv
          obtain ⟨fe, hfe, hfeq⟩ := List.mem_map.mp hm
          rw [← hfeq]
          exact fun hf => hown ⟨fe, hfe, hf⟩
        rw [applySets_get_other w.field as cand hnot, ht]

/-- The workflows leave a field as it is when no transition of any of them assigns it. -/
theorem fireAll_keeps (p : Principal) (old : Record) (f : FieldId) :
    ∀ (ws : List Workflow) (cand r : Record), (∀ w ∈ ws, ∀ tr ∈ w.transitions, ∀ fe ∈ tr.set, fe.1 ≠ f) →
      fireAll p old cand ws = .ok r → r.fields.get f = cand.fields.get f
  | [], cand, r, _, h => by
    unfold fireAll at h
    cases h
    rfl
  | w :: ws, cand, r, hf, h => by
    unfold fireAll at h
    split at h
    · cases h
    · rename_i c1 hc1
      have hk1 : c1.fields.get f = cand.fields.get f :=
        fireOne_keeps (f := f) (hf w List.mem_cons_self) hc1
      have hk2 := fireAll_keeps p old f ws c1 r (fun w' hw' => hf w' (List.mem_cons_of_mem _ hw')) h
      rw [hk2, hk1]

/-- **The workflows of an accepted write leave each field with a stated final value.** Under the rule, for every
    workflow of the write: a field the patch left alone keeps its value; a field it changed took a declared
    transition, and the record then holds either that transition's target (when the transition assigns no field of
    the workflow) or the state the transition's own assignment of the field names. -/
theorem fireAll_final (p : Principal) (old : Record) :
    ∀ (ws : List Workflow) (cand r : Record), WfDefined ws → fireAll p old cand ws = .ok r →
      ∀ w ∈ ws,
        (old.fields.get w.field = cand.fields.get w.field →
          r.fields.get w.field = old.fields.get w.field) ∧
        (old.fields.get w.field ≠ cand.fields.get w.field →
          ∃ c tr, tr ∈ w.transitions ∧ c.fields.get w.field = cand.fields.get w.field ∧
            old.fields.get w.field = .str tr.source ∧ cand.fields.get w.field = .str tr.target ∧
            guardHolds { record := some old, old := some old, new := some c, principal := p.userId? }
              tr.guard = true ∧
            ((∀ fe ∈ tr.set, fe.1 ≠ w.field) ∧ r.fields.get w.field = .str tr.target ∨
              ∃ s, r.fields.get w.field = .str s ∧ ∃ fe ∈ tr.set, fe.1 = w.field ∧
                fe.2 = .lit (.str s))) := by
  intro ws
  induction ws with
  | nil => intro cand r _ _ w hw; simp at hw
  | cons w0 ws' ih =>
    intro cand r hd h w hw
    unfold fireAll at h
    split at h
    · cases h
    · rename_i c1 hc1
      have hkeepTail : ∀ u ∈ ws', c1.fields.get u.field = cand.fields.get u.field := fun u hu =>
        fireOne_keeps (fun tr htr fe hfe => wfDefined_cross hd hu htr hfe) hc1
      have hrh : r.fields.get w0.field = c1.fields.get w0.field :=
        fireAll_keeps p old w0.field ws' c1 r (fun w' hw' tr htr fe hfe => wfDefined_foreign hd hw' htr hfe) h
      have hW : ∀ tr ∈ w0.transitions, (tr.set.map Prod.fst).Nodup ∧
          ∀ fe ∈ tr.set, fe.1 = w0.field → ∃ s, fe.2 = .lit (.str s) :=
        fun tr htr => ⟨(hd.2 w0 List.mem_cons_self tr htr).1, fun fe hfe hf => by
          obtain ⟨s, hs, -⟩ := ((hd.2 w0 List.mem_cons_self tr htr).2 fe hfe).2 hf
          exact ⟨s, hs⟩⟩
      rcases List.mem_cons.mp hw with hu0 | hu'
      · rw [hu0]
        constructor
        · intro heq
          have hc : c1 = cand := Except.ok.inj (hc1.symm.trans (fireOne_unchanged heq))
          rw [hrh, hc, heq]
        · intro hch
          obtain ⟨tr, htr, hs, ht, hg, hr⟩ := fireOne_field hW hc1 hch
          refine ⟨cand, tr, htr, rfl, hs, ht, hg, ?_⟩
          rw [hrh]
          exact hr
      · obtain ⟨i1, i2⟩ := ih c1 r (wfDefined_tail hd) h w hu'
        rw [hkeepTail w hu'] at i1 i2
        exact ⟨i1, i2⟩

/-- **Every workflow field of an accepted write holds a string**, when the stored record did: a field the patch left
    alone keeps its string, and a field it changed holds the target or the literal state of the transition. -/
theorem fireAll_present {p : Principal} {old cand : Record} {ws : List Workflow} {r : Record}
    (hd : WfDefined ws) (hF : fireAll p old cand ws = .ok r)
    (hold : ∀ w ∈ ws, ∃ s, old.fields.get w.field = .str s) :
    ∀ w ∈ ws, ∃ s, r.fields.get w.field = .str s := by
  intro w hw
  obtain ⟨h1, h2⟩ := fireAll_final p old ws cand r hd hF w hw
  by_cases heq : old.fields.get w.field = cand.fields.get w.field
  · obtain ⟨s, hs⟩ := hold w hw
    exact ⟨s, by rw [h1 heq, hs]⟩
  · obtain ⟨-, tr, -, -, -, ht, -, hr⟩ := h2 heq
    rcases hr with ⟨-, hr⟩ | ⟨s, hr, -⟩
    · exact ⟨tr.target, hr⟩
    · exact ⟨s, hr⟩

/-! ## Creates -/

/-- A workflow field read off a record that starts in an initial state holds the state. -/
theorem wfInitialOk_spec {w : Workflow} {r : Record} (h : wfInitialOk w r = true) :
    ∃ s, r.fields.get w.field = .str s ∧ s ∈ w.initial := by
  unfold wfInitialOk at h
  cases hv : r.fields.get w.field with
  | null => simp [hv] at h
  | int i => simp [hv] at h
  | bool b => simp [hv] at h
  | ref x => simp [hv] at h
  | list vs => simp [hv] at h
  | str s => exact ⟨s, rfl, by simpa [hv] using h⟩

/-- An accepted create writes a record that starts in an initial state of every workflow of its type. -/
theorem create_ok_initials {t t' : Tenant} {p : Principal} {ty : TypeId} {fs : Fields}
    {o : ObjType} {r : Record} (ho : t.typ? ty = some o) (hN : newRecord o.schema p fs = .ok r)
    (h : stepTenant t p (.create ty fs) = .ok t') : initialsOk o.workflows r = true := by
  cases hi : initialsOk o.workflows r with
  | true => rfl
  | false =>
    simp only [stepTenant, ho, hN] at h
    repeat' split at h
    all_goals simp_all

/-- **Creates start in an initial state.** An accepted create writes a record whose workflow field holds one
    of the initial states of every workflow of its type. -/
theorem create_workflow_initial (t t' : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (h : stepTenant t p (.create ty fs) = .ok t') :
    ∃ r, newRecord o.schema p fs = .ok r ∧
      ∀ w ∈ o.workflows, ∃ s, r.fields.get w.field = .str s ∧ s ∈ w.initial := by
  obtain ⟨r, hN, -, -, -⟩ := create_ok_parts ho h
  have hi := create_ok_initials ho hN h
  unfold initialsOk at hi
  exact ⟨r, hN, fun w hw => wfInitialOk_spec (List.all_eq_true.mp hi w hw)⟩

/-- **A create that does not start in an initial state is refused** with `workflowViolation`, when the
    caller may create the record. -/
theorem create_non_initial_refused (t : Tenant) (p : Principal) (ty : TypeId) (fs : Fields)
    (o : ObjType) (ho : t.typ? ty = some o) (r : Record)
    (hN : newRecord o.schema p fs = .ok r) (hc : canCreate t p ty r = true)
    (hi : initialsOk o.workflows r = false) :
    stepTenant t p (.create ty fs) = .error .workflowViolation := by
  simp [stepTenant, ho, hN, hc, canCreate_attempt _ _ _ _ hc, hi]

/-! ## Updates -/

/-- **The record an accepted update writes is the patched record as the workflows leave it**, and a record
    the workflows changed is authorised again: the update may write only what the actor may write. -/
theorem update_writes_fired {t t' : Tenant} {p : Principal} {ty : TypeId} {id : RecordId}
    {fs : Fields} {ev : Option Nat} {o : ObjType} {old : Record} (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old)
    (h : stepTenant t p (.update ty id fs ev) = .ok t') :
    ∃ r, fireAll p old (patched old fs) o.workflows = .ok r ∧ t'.recordAt ty id = some r ∧
      (sameFields (patched old fs).fields r.fields = false → canUpdate t p ty old r = true) := by
  obtain ⟨r, hF, hdis, -, -, -, ht⟩ := update_ok_parts_gen ho hl h
  refine ⟨r, hF, ?_, fun hs => ?_⟩
  · subst ht
    unfold Tenant.recordAt
    rw [typ?_putType_self t ty o _ ho]
    show List.lookup id (o.records.map (fun q => if q.1 == id then (q.1, r) else q)) = some r
    rw [lookup_map_replace, hl]
    rfl
  · rcases hdis with hd | hc
    · rw [hs] at hd
      cases hd
    · exact hc

/-- **A changed workflow field follows a declared transition, and the record holds its state.** Under the rule, an
    accepted update that changes a workflow field by its patch took a transition of that workflow from the stored
    state to the patched one, whose guard held on the candidate the step read; the record it writes then holds the
    transition's target there, or the state that the transition's own assignment of the field names. -/
theorem update_changed_fired {t t' : Tenant} {p : Principal} {ty : TypeId} {id : RecordId}
    {fs : Fields} {ev : Option Nat} {o : ObjType} {old : Record} (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (hind : WfDefined o.workflows)
    (h : stepTenant t p (.update ty id fs ev) = .ok t') {w : Workflow} (hw : w ∈ o.workflows)
    (hch : old.fields.get w.field ≠ (patched old fs).fields.get w.field) :
    ∃ r, t'.recordAt ty id = some r ∧ ∃ c tr, tr ∈ w.transitions ∧
      c.fields.get w.field = (patched old fs).fields.get w.field ∧
      old.fields.get w.field = .str tr.source ∧ (patched old fs).fields.get w.field = .str tr.target ∧
      guardHolds { record := some old, old := some old, new := some c, principal := p.userId? }
        tr.guard = true ∧
      ((∀ fe ∈ tr.set, fe.1 ≠ w.field) ∧ r.fields.get w.field = .str tr.target ∨
        ∃ s, r.fields.get w.field = .str s ∧ ∃ fe ∈ tr.set, fe.1 = w.field ∧ fe.2 = .lit (.str s)) := by
  obtain ⟨r, hF, hrec, -⟩ := update_writes_fired ho hl h
  obtain ⟨c, tr, htr, hc, hs, ht, hg, hr⟩ :=
    (fireAll_final p old o.workflows (patched old fs) r hind hF w hw).2 hch
  exact ⟨r, hrec, c, tr, htr, hc, hs, ht, hg, hr⟩

/-- **An update that changes a workflow field without a declared transition is refused**, under the definition
    rule: when no transition of the workflow from the stored state to the patched one exists, no step is accepted. -/
theorem update_undeclared_refused {t : Tenant} {p : Principal} {ty : TypeId} {id : RecordId}
    {fs : Fields} {ev : Option Nat} {o : ObjType} {old : Record} (ho : t.typ? ty = some o)
    (hl : o.records.lookup id = some old) (hind : WfDefined o.workflows) {w : Workflow}
    (hw : w ∈ o.workflows)
    (hch : old.fields.get w.field ≠ (patched old fs).fields.get w.field)
    (hnone : ∀ tr ∈ w.transitions, ¬ (old.fields.get w.field = .str tr.source ∧
      (patched old fs).fields.get w.field = .str tr.target)) :
    ∀ t', stepTenant t p (.update ty id fs ev) ≠ .ok t' := by
  intro t' h'
  obtain ⟨-, -, c, tr, htr, -, hs, ht, -, -⟩ := update_changed_fired ho hl hind h' hw hch
  exact hnone tr htr ⟨hs, ht⟩

/-! ## Every accepted step keeps the states -/

/-- The states hold of a record after the references to a deleted record are cleared: a string is never
    changed by a clearing, and a value that is cleared is a reference or a list, not a string. -/
theorem str_of_clearFields {schema : Schema} {T : TypeId} {id : RecordId} {fs : Fields} {f : FieldId}
    {s : String} (hn : (fs.map Prod.fst).Nodup)
    (h : Fields.get (clearFields schema T id fs) f = .str s) : Fields.get fs f = .str s := by
  rw [get_clearFields_eq schema T id f fs hn] at h
  split at h
  · rename_i hfr
    exfalso
    have href : (Fields.get fs f).refersTo id = true := by
      unfold fieldRefers at hfr
      split at hfr
      · rename_i d _
        simp only [Bool.and_eq_true] at hfr
        exact hfr.2
      · cases hfr
    cases hv : Fields.get fs f with
    | null => simp [hv, Val.refersTo] at href
    | int i => simp [hv, Val.refersTo] at href
    | bool b => simp [hv, Val.refersTo] at href
    | str x => simp [hv, Val.refersTo] at href
    | ref x =>
      rw [hv] at h
      simp only [Val.dropRef] at h
      split at h <;> simp at h
    | list vs =>
      rw [hv] at h
      simp [Val.dropRef] at h
  · exact h

theorem statesOk_of_str {ws : List Workflow} {r : Record} (w : Workflow) (hw : w ∈ ws)
    (h : statesOk ws r = true) (s : String) (hs : r.fields.get w.field = .str s) : s ∈ w.states := by
  unfold statesOk at h
  rw [List.all_eq_true] at h
  have hw' := h w hw
  unfold wfStateOk at hw'
  rw [hs] at hw'
  simpa using hw'

theorem wfStateOk_of_get {w : Workflow} {r : Record}
    (h : ∀ s, r.fields.get w.field = .str s → s ∈ w.states) : wfStateOk w r = true := by
  unfold wfStateOk
  cases hv : r.fields.get w.field with
  | null => rfl
  | int _ => rfl
  | bool _ => rfl
  | ref _ => rfl
  | list _ => rfl
  | str s => simpa using h s hv

/-- A clearing of the references to a deleted record keeps every stored workflow field a state. -/
theorem clearRef_statesOk {ws : List Workflow} {schema : Schema} {T : TypeId} {id : RecordId}
    {r : Record} (hn : (r.fields.map Prod.fst).Nodup) (h : statesOk ws r = true) :
    statesOk ws (clearRef schema T id r) = true := by
  unfold statesOk
  rw [List.all_eq_true]
  intro w hw
  have hw0 : wfStateOk w r = true := by
    have := h
    unfold statesOk at this
    rw [List.all_eq_true] at this
    exact this w hw
  unfold clearRef
  split
  · apply wfStateOk_of_get
    intro s hs
    have hs' : Fields.get (clearFields schema T id r.fields) w.field = .str s := hs
    have hr := str_of_clearFields hn hs'
    unfold wfStateOk at hw0
    rw [hr] at hw0
    simpa using hw0
  · exact hw0

/-- A clearing of the references to a deleted record leaves a string field a string. -/
theorem get_clearFields_str {schema : Schema} {T : TypeId} {id : RecordId} {fs : Fields} {f : FieldId}
    {s : String} (hn : (fs.map Prod.fst).Nodup) (h : Fields.get fs f = .str s) :
    Fields.get (clearFields schema T id fs) f = .str s := by
  rw [get_clearFields_eq schema T id f fs hn, h]
  have hnr : fieldRefers schema T id (f, .str s) = false := by
    unfold fieldRefers
    cases schema.lookup f <;> simp [Val.refersTo]
  simp [hnr]

/-- A clearing of the references to a deleted record keeps every workflow field holding a state. -/
theorem clearRef_present {ws : List Workflow} {schema : Schema} {T : TypeId} {id : RecordId}
    {r : Record} (hn : (r.fields.map Prod.fst).Nodup) (h : ∀ w ∈ ws, ∃ s, r.fields.get w.field = .str s) :
    ∀ w ∈ ws, ∃ s, (clearRef schema T id r).fields.get w.field = .str s := by
  intro w hw
  obtain ⟨s, hs⟩ := h w hw
  refine ⟨s, ?_⟩
  unfold clearRef
  split
  · exact get_clearFields_str hn hs
  · exact hs

/-- Replacing one type's object type with one that has the same records and workflows keeps the states. -/
theorem putType_statesOk {t : Tenant} {ty : TypeId} {o o' : ObjType} (hT : t.typ? ty = some o)
    (hs : t.StatesOk) (hr : o'.records = o.records) (hw : o'.workflows = o.workflows) :
    (t.putType ty o').StatesOk := by
  intro T o'' hT'' q hq
  by_cases hTy : T = ty
  · subst hTy
    change (t.putType T _).typ? T = _ at hT''
    rw [typ?_putType_self t T o _ hT] at hT''
    cases hT''
    rw [hr] at hq
    rw [hw]
    exact hs T o hT q hq
  · change (t.putType ty _).typ? T = _ at hT''
    rw [typ?_putType_other t ty T _ hTy] at hT''
    exact hs T o'' hT'' q hq

/-- **Every accepted step keeps the workflow states.** An accepted step of a tenant that holds the states
    leaves it holding them; the distinct field names of the stored records are the one assumption the
    clearing of a delete needs. -/
theorem stepTenant_preserves_states (t t' : Tenant) (p : Principal) (i : Intent)
    (hd : t.Distinct) (hs : t.StatesOk) (hwd : t.WfDefs) (h : stepTenant t p i = .ok t') :
    t'.StatesOk := by
  cases i with
  | createTenant owner name => simp [stepTenant] at h
  | changeSchema ev strict cs => exact changeSchema_states hs h
  | member j =>
    obtain ⟨m', rfl⟩ := stepTenant_member_eq h
    exact hs
  | create ty fs =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      obtain ⟨r, hN, -, -, ht⟩ := create_ok_parts hT h
      have hi := create_ok_initials hT hN h
      have hi' := hi
      unfold initialsOk at hi'
      subst ht
      intro T o'' hT'' q hq
      by_cases hTy : T = ty
      · subst hTy
        change (t.putType T _).typ? T = _ at hT''
        rw [typ?_putType_self t T o _ hT] at hT''
        cases hT''
        rcases List.mem_cons.mp (show q ∈ (t.nextId, r) :: o.records from hq) with rfl | hq'
        · refine ⟨statesOk_of_initials hi, fun w hw => ?_⟩
          obtain ⟨s, hs1, -⟩ := wfInitialOk_spec (List.all_eq_true.mp hi' w hw)
          exact ⟨s, hs1⟩
        · exact hs T o hT q hq'
      · change (t.putType ty _).typ? T = _ at hT''
        rw [typ?_putType_other t ty T _ hTy] at hT''
        exact hs T o'' hT'' q hq
  | update ty id fs ev =>
    cases hT : t.typ? ty with
    | none => simp [stepTenant, hT] at h
    | some o =>
      cases hL : o.records.lookup id with
      | none => simp [stepTenant, hT, hL, missingRecord] at h
      | some old =>
        obtain ⟨r, hF, -, -, hst, -, ht⟩ := update_ok_parts_gen hT hL h
        subst ht
        -- The stored record held a state in each workflow field; the write leaves one there.
        have hold : ∀ w ∈ o.workflows, ∃ s, old.fields.get w.field = .str s :=
          (hs ty o hT (id, old) (lookup_mem hL)).2
        have hpres := fireAll_present (hwd ty o hT) hF hold
        intro T o'' hT'' q hq
        by_cases hTy : T = ty
        · subst hTy
          change (t.putType T _).typ? T = _ at hT''
          rw [typ?_putType_self t T o _ hT] at hT''
          cases hT''
          obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp
            (show q ∈ o.records.map (fun q => if q.1 == id then (q.1, r) else q) from hq)
          by_cases hid : (q₀.1 == id) = true
          · simp only [hid, if_true] at hmap
            rw [← hmap]
            exact ⟨hst, hpres⟩
          · simp only [hid, if_false] at hmap
            rw [← hmap]
            exact hs T o hT q₀ hq₀
        · change (t.putType ty _).typ? T = _ at hT''
          rw [typ?_putType_other t ty T _ hTy] at hT''
          exact hs T o'' hT'' q hq
  | delete ty id ev =>
    have ht := stepTenant_delete_eq h
    subst ht
    intro T o'' hT'' q hq
    rw [typ?_afterDelete] at hT''
    cases hT : t.typ? T with
    | none => simp [hT] at hT''
    | some o =>
      simp only [hT, Option.map_some] at hT''
      have ho'' : delObj ty id T o = o'' := Option.some.inj hT''
      subst ho''
      have hq' : q ∈ delClear o.schema ty id (if T == ty then o.records.filter (fun q => q.1 != id)
        else o.records) := hq
      obtain ⟨q₀, hq₀, hmap⟩ := List.mem_map.mp hq'
      have hmem : q₀ ∈ o.records := by
        split at hq₀
        · exact (List.mem_filter.mp hq₀).1
        · exact hq₀
      have hnd : (q₀.2.fields.map Prod.fst).Nodup := (hd.2 T o hT).2 q₀ hmem
      rw [← hmap]
      show statesOk o.workflows (clearRef o.schema ty id q₀.2) = true ∧
        ∀ w ∈ o.workflows, ∃ s, (clearRef o.schema ty id q₀.2).fields.get w.field = .str s
      obtain ⟨hs1, hs2⟩ := hs T o hT q₀ hmem
      exact ⟨clearRef_statesOk hnd hs1, clearRef_present hnd hs2⟩
  | setRequired ty f strict =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · cases h
          exact putType_statesOk hT hs rfl rfl
  | setUnique ty f strict =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · rename_i d _
          split at h
          · cases h
          · split at h
            · cases h
            · cases h
              exact putType_statesOk hT hs rfl rfl
  | defineUnique ty fs =>
    simp only [stepTenant] at h
    split at h
    · cases h
    · cases hT : t.typ? ty with
      | none => simp [hT] at h
      | some o =>
        simp only [hT] at h
        split at h
        · cases h
        · split at h
          · cases h
          · split at h
            · cases h
            · cases h
              exact putType_statesOk hT hs rfl rfl

/-- **Every accepted step keeps the workflow states of every tenant of a well-formed world.** A creation of
    a tenant makes one with no object types, so it holds no record. -/
theorem step_preserves_states (w w' : World) (i : Intent) (c : Ctx) (hwf : w.WF) (hs : w.StatesOk)
    (hwd : w.WfDefs) (h : step w i c = .ok w') : w'.StatesOk := by
  rcases step_cases h with ⟨t, t', ht, hst, hw'⟩ | ⟨owner, name, -, hnone, hw'⟩
  · intro x t'' hx
    rw [hw'] at hx
    dsimp only at hx
    by_cases hxc : x = c.tenant
    · rw [if_pos hxc] at hx
      cases hx
      subst hxc
      exact stepTenant_preserves_states t t' c.principal i (hwf c.tenant t ht).2.2.2.1
        (hs c.tenant t ht) (hwd c.tenant t ht) hst
    · rw [if_neg hxc] at hx
      exact hs x t'' hx
  · intro x t'' hx
    rw [hw'] at hx
    dsimp only at hx
    by_cases hxc : x = c.tenant
    · rw [if_pos hxc] at hx
      cases hx
      unfold Tenant.StatesOk
      intro T o hT
      simp [genesisTenant, Tenant.typ?] at hT
    · rw [if_neg hxc] at hx
      exact hs x t'' hx

end Strata
