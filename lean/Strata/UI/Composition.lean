/-
Compositions and wireframe fidelity (docs/design-system.md, "Screens as compositions",
"Wireframes and fidelity" and "Tenant-arranged layouts").

A screen (a published body, a draft body, or a product screen as a tenant overlay leaves it) is a
template and a list of widgets. A widget has an id, unique in its screen, by which an overlay
addresses it; a pattern of the catalogue; the region it is placed in; and a binding:

- `placeholder`: a wireframe placeholder, drawn from its sample shape, naming nothing;
- `static`: a pattern that shows no data (a block of prose, a list of links);
- `bound b`: bound to `b`. What `b` is is abstract here. In the implementation a binding names a
  section of the screen's view, a form, an action or a platform source *together with the shape
  fingerprint* of what it names (for a section: its object type and the type of every field the
  widget reads). So a field whose type changes, or a section whose shape changes, is a binding that
  stops existing and a new one that starts: the theorems below cover shape changes because the
  fingerprint is part of `b`.

The catalogue says which patterns a template allows in each region, which patterns are static, and
which bindings a pattern accepts (the binding's kind against the pattern's `binds`, and the field
names its props read against the shape).

- **Valid**: ids are distinct; every widget is allowed where it is; a static pattern is `static`; a
  data pattern is a placeholder or bound to a binding the pattern accepts and the schema defines.
- **Publishable**: valid with no placeholder at all, whatever the pattern.

Proved:

- the kernel's check is exactly validity (`check_iff`); a checked composition names only bindings
  the schema defines and its patterns accept (`checked_bindings_resolve`);
- a publishable composition has every data widget bound to something that exists and is accepted
  (`publishable_bound`), and no placeholder (`publishable_no_placeholder`);
- binding a placeholder keeps validity and leaves one placeholder fewer (`bindAt_valid`,
  `bindAt_placeholders`); refinement one widget at a time is valid at every step and ends
  publishable (`refine_valid`, `refine_publishable`);
- the incremental rule, with the schema *and the catalogue* changing: unchanged screens stay valid
  when the change removes no binding they name and narrows no catalogue entry they use, and changed
  screens are checked against the new schema and catalogue (`incremental_valid`); without the
  removal guard a valid screen breaks (`unguarded_removal_breaks`);
- orphaning instead of refusing: removing from a screen exactly the widgets that fail under the new
  catalogue and schema leaves a valid screen with distinct ids (`orphan_valid`), removes only those
  (`orphan_removes_only_failing`), and removes nothing from a screen that still passes (`orphan_keeps_valid`).

Not modelled: the content of props beyond what `accepts` decides, overlay operations other than
dropping a widget, and the screens' visibility (Visibility.lean). No `sorry`, no added axioms.
-/

namespace Strata.UI.Composition

/-- A widget's binding. -/
inductive Bind (B : Type) where
  | placeholder
  | static
  | bound (b : B)

/-- A widget: its id, its pattern, its region, its binding. -/
structure Widget (P R B : Type) where
  id : Nat
  pattern : P
  region : R
  bind : Bind B

/-- What the catalogue says about templates and patterns. -/
structure Catalogue (T P R B : Type) where
  allowed : T → R → P → Bool
  isStatic : P → Bool
  accepts : P → B → Bool

/-- A screen: its template and its widgets. -/
structure Screen (T P R B : Type) where
  template : T
  widgets : List (Widget P R B)

variable {T P R B : Type}

/-- The binding of one widget is right for its pattern and exists. -/
def bindOk (cat : Catalogue T P R B) (has : B → Bool) (p : P) : Bind B → Bool
  | .placeholder => !cat.isStatic p
  | .static => cat.isStatic p
  | .bound b => !cat.isStatic p && cat.accepts p b && has b

/-- One widget is allowed where it is, and its binding is right. -/
def widgetCheck (cat : Catalogue T P R B) (has : B → Bool) (t : T) (w : Widget P R B) : Bool :=
  cat.allowed t w.region w.pattern && bindOk cat has w.pattern w.bind

/-- A screen is valid: distinct ids, and every widget passes. -/
def Valid (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) : Prop :=
  (s.widgets.map (·.id)).Nodup ∧ ∀ w ∈ s.widgets, widgetCheck cat has s.template w = true

/-- The kernel's check of a screen. -/
def check (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) : Bool :=
  decide ((s.widgets.map (·.id)).Nodup) && s.widgets.all (widgetCheck cat has s.template)

theorem bound_ok {cat : Catalogue T P R B} {has : B → Bool} {t : T} {w : Widget P R B} {b : B}
    (h : widgetCheck cat has t w = true) (hb : w.bind = .bound b) :
    cat.allowed t w.region w.pattern = true ∧ cat.isStatic w.pattern = false ∧
      cat.accepts w.pattern b = true ∧ has b = true := by
  unfold widgetCheck bindOk at h
  rw [hb] at h
  simp only [Bool.and_eq_true, Bool.not_eq_true'] at h
  exact ⟨h.1, h.2.1.1, h.2.1.2, h.2.2⟩

theorem ok_of_bound {cat : Catalogue T P R B} {has : B → Bool} {t : T} {w : Widget P R B} {b : B}
    (hb : w.bind = .bound b) (ha : cat.allowed t w.region w.pattern = true)
    (hs : cat.isStatic w.pattern = false) (hc : cat.accepts w.pattern b = true)
    (hh : has b = true) : widgetCheck cat has t w = true := by
  unfold widgetCheck bindOk
  rw [hb]
  simp [ha, hs, hc, hh]

theorem placeholder_ok {cat : Catalogue T P R B} {has : B → Bool} {t : T} {w : Widget P R B}
    (h : widgetCheck cat has t w = true) (hb : w.bind = .placeholder) :
    cat.allowed t w.region w.pattern = true ∧ cat.isStatic w.pattern = false := by
  unfold widgetCheck bindOk at h
  rw [hb] at h
  simp only [Bool.and_eq_true, Bool.not_eq_true'] at h
  exact h

/-- A widget that passes under one catalogue and schema passes under another that agrees on it. -/
theorem wc_transfer {cat cat' : Catalogue T P R B} {has has' : B → Bool} {t : T} {w : Widget P R B}
    (ok : widgetCheck cat has t w = true)
    (ha : cat'.allowed t w.region w.pattern = cat.allowed t w.region w.pattern)
    (hs : cat'.isStatic w.pattern = cat.isStatic w.pattern)
    (hb : ∀ b, w.bind = .bound b → cat'.accepts w.pattern b = cat.accepts w.pattern b ∧ has' b = true) :
    widgetCheck cat' has' t w = true := by
  cases h : w.bind with
  | placeholder =>
    have := placeholder_ok ok h
    unfold widgetCheck bindOk; rw [h, ha, hs]; simp [this.1, this.2]
  | static =>
    unfold widgetCheck bindOk at ok ⊢; rw [h] at ok ⊢; rw [ha, hs]; exact ok
  | bound b =>
    obtain ⟨h1, h2, h3, h4⟩ := bound_ok ok h
    obtain ⟨hacc, hhas⟩ := hb b h
    exact ok_of_bound h (ha.trans h1) (hs.trans h2) (hacc.trans h3) hhas

/-- Verifies: DS-001 -/
theorem check_iff (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) :
    check cat has s = true ↔ Valid cat has s := by
  unfold check Valid
  simp [List.all_eq_true]

/-- A composition that passes the check names only bindings the schema defines and its patterns
    accept.
    Verifies: DS-001 -/
theorem checked_bindings_resolve (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (h : check cat has s = true) :
    ∀ w ∈ s.widgets, ∀ b, w.bind = .bound b → has b = true ∧ cat.accepts w.pattern b = true := by
  intro w hw b hb
  have := bound_ok (((check_iff cat has s).1 h).2 w hw) hb
  exact ⟨this.2.2.2, this.2.2.1⟩

/-- Whether a binding is a placeholder. -/
def Bind.isPlaceholder : Bind B → Bool
  | .placeholder => true
  | _ => false

/-- Publishable: valid, with no placeholder at all. -/
def Publishable (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) : Prop :=
  Valid cat has s ∧ ∀ w ∈ s.widgets, w.bind.isPlaceholder = false

/-- Verifies: DS-002 -/
theorem publishable_no_placeholder (cat : Catalogue T P R B) (has : B → Bool)
    (s : Screen T P R B) (h : Publishable cat has s) : ∀ w ∈ s.widgets, w.bind ≠ .placeholder := by
  intro w hw he
  have := h.2 w hw
  rw [he] at this
  cases this

/-- Verifies: DS-002 -/
theorem publishable_bound (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (h : Publishable cat has s) :
    ∀ w ∈ s.widgets, cat.isStatic w.pattern = false →
      ∃ b, w.bind = .bound b ∧ has b = true ∧ cat.accepts w.pattern b = true := by
  intro w hw hs
  have hc := h.1.2 w hw
  have hp := h.2 w hw
  cases hb : w.bind with
  | placeholder => rw [hb] at hp; cases hp
  | static =>
    unfold widgetCheck bindOk at hc; rw [hb] at hc
    simp [hs] at hc
  | bound b =>
    have := bound_ok hc hb
    exact ⟨b, rfl, this.2.2.2, this.2.2.1⟩

/-- Bind the widget with id `i` to `b`, when it is a placeholder. -/
def bindWidget (i : Nat) (b : B) (w : Widget P R B) : Widget P R B :=
  if w.id = i ∧ w.bind.isPlaceholder = true then { w with bind := .bound b } else w

/-- Bind the placeholder with id `i` of a screen to `b`. -/
def bindAt (i : Nat) (b : B) (s : Screen T P R B) : Screen T P R B :=
  { s with widgets := s.widgets.map (bindWidget i b) }

theorem bindWidget_id (i : Nat) (b : B) (w : Widget P R B) : (bindWidget i b w).id = w.id := by
  unfold bindWidget; split <;> rfl

/-- Binding a placeholder to a binding its pattern accepts and the schema defines keeps validity.
    Verifies: DS-003 -/
theorem bindAt_valid (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) (i : Nat)
    (b : B) (hv : Valid cat has s)
    (hb : ∀ w ∈ s.widgets, w.id = i → cat.accepts w.pattern b = true ∧ has b = true) :
    Valid cat has (bindAt i b s) := by
  refine ⟨?_, ?_⟩
  · have : (s.widgets.map (bindWidget i b)).map (·.id) = s.widgets.map (·.id) := by
      simp [List.map_map, Function.comp_def, bindWidget_id]
    simpa [bindAt, this] using hv.1
  · intro w hw
    simp only [bindAt, List.mem_map] at hw
    obtain ⟨v, hv', rfl⟩ := hw
    have ok := hv.2 v hv'
    unfold bindWidget
    split
    · rename_i hc
      have hph := hc.2
      cases hvb : v.bind with
      | placeholder =>
        have ok' := placeholder_ok ok hvb
        obtain ⟨ha, hn⟩ := hb v hv' hc.1
        exact ok_of_bound (w := { v with bind := .bound b }) rfl ok'.1 ok'.2 ha hn
      | static => rw [hvb] at hph; cases hph
      | bound c => rw [hvb] at hph; cases hph
    · exact ok

/-- The placeholders of a list of widgets. -/
def placeholders (ws : List (Widget P R B)) : Nat := (ws.filter (·.bind.isPlaceholder)).length

/-- Binding the placeholder with id `i`, when ids are distinct, leaves one placeholder fewer.
    Verifies: DS-003 -/
theorem bindAt_placeholders (i : Nat) (b : B) :
    ∀ (ws : List (Widget P R B)), (ws.map (·.id)).Nodup →
      (∃ w ∈ ws, w.id = i ∧ w.bind.isPlaceholder = true) →
      placeholders (ws.map (bindWidget i b)) + 1 = placeholders ws
  | [], _, ⟨w, hw, _⟩ => by simp at hw
  | v :: vs, hnd, hex => by
    simp only [List.map_cons, List.nodup_cons, List.mem_map, not_exists, not_and] at hnd
    obtain ⟨hnot, hnd'⟩ := hnd
    by_cases hvi : v.id = i
    · have hvp : v.bind.isPlaceholder = true := by
        obtain ⟨w, hw, hi, hp⟩ := hex
        rcases List.mem_cons.1 hw with rfl | hw'
        · exact hp
        · exact absurd (hi.trans hvi.symm) (hnot w hw')
      have hrest : vs.map (bindWidget i b) = vs := by
        have h' : ∀ x ∈ vs, bindWidget i b x = id x := by
          intro x hx
          have : x.id ≠ i := fun h => hnot x hx (h.trans hvi.symm)
          simp [bindWidget, this]
        exact (List.map_congr_left h').trans (List.map_id vs)
      have hv : bindWidget i b v = { v with bind := .bound b } := by
        simp [bindWidget, hvi, hvp]
      unfold placeholders
      simp only [List.map_cons, hv, hrest, List.filter_cons]
      rw [if_pos hvp]
      simp [Bind.isPlaceholder]
    · have hex' : ∃ w ∈ vs, w.id = i ∧ w.bind.isPlaceholder = true := by
        obtain ⟨w, hw, hi, hp⟩ := hex
        rcases List.mem_cons.1 hw with rfl | hw'
        · exact absurd hi hvi
        · exact ⟨w, hw', hi, hp⟩
      have ih := bindAt_placeholders i b vs hnd' hex'
      have hv : bindWidget i b v = v := by simp [bindWidget, hvi]
      unfold placeholders at ih ⊢
      simp only [List.map_cons, hv, List.filter_cons]
      split <;> simp_all

/-- Refinement of the first `k` widgets: each placeholder resolved, the rest as they were. -/
def refine (resolve : Widget P R B → Option B) (k : Nat) (ws : List (Widget P R B)) :
    List (Widget P R B) :=
  (ws.take k).map (fun w => match w.bind, resolve w with
    | .placeholder, some b => { w with bind := .bound b }
    | _, _ => w) ++ ws.drop k

theorem refine_ids (resolve : Widget P R B → Option B) (k : Nat) (ws : List (Widget P R B)) :
    (refine resolve k ws).map (·.id) = ws.map (·.id) := by
  unfold refine
  rw [List.map_append, List.map_map]
  conv => rhs; rw [← List.take_append_drop k ws, List.map_append]
  congr 1
  apply List.map_congr_left
  intro w _
  simp only [Function.comp_apply]
  split <;> rfl

/-- Every step of refinement is valid, when each resolution is accepted and exists.
    Verifies: DS-003 -/
theorem refine_valid (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (resolve : Widget P R B → Option B) (hv : Valid cat has s)
    (hr : ∀ w ∈ s.widgets, ∀ b, resolve w = some b → cat.accepts w.pattern b = true ∧ has b = true)
    (k : Nat) : Valid cat has { s with widgets := refine resolve k s.widgets } := by
  refine ⟨by simpa [refine_ids] using hv.1, ?_⟩
  intro w hw
  simp only [refine, List.mem_append, List.mem_map] at hw
  rcases hw with ⟨v, hv', rfl⟩ | hw
  · have hvs : v ∈ s.widgets := List.mem_of_mem_take hv'
    have ok := hv.2 v hvs
    split
    · rename_i b hvb hres
      have ok' := placeholder_ok ok hvb
      obtain ⟨ha, hn⟩ := hr v hvs b hres
      exact ok_of_bound (w := { v with bind := .bound b }) rfl ok'.1 ok'.2 ha hn
    · exact ok
  · exact hv.2 w (List.mem_of_mem_drop hw)

/-- Refinement of every widget ends publishable, when every placeholder is resolved.
    Verifies: DS-003 -/
theorem refine_publishable (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (resolve : Widget P R B → Option B) (hv : Valid cat has s)
    (hr : ∀ w ∈ s.widgets, ∀ b, resolve w = some b → cat.accepts w.pattern b = true ∧ has b = true)
    (hall : ∀ w ∈ s.widgets, w.bind.isPlaceholder = true → (resolve w).isSome = true) :
    Publishable cat has { s with widgets := refine resolve s.widgets.length s.widgets } := by
  refine ⟨refine_valid cat has s resolve hv hr _, ?_⟩
  intro w hw
  simp only [refine, List.take_length, List.drop_length, List.append_nil, List.mem_map] at hw
  obtain ⟨v, hv', rfl⟩ := hw
  cases hvb : v.bind with
  | placeholder =>
    have hs := hall v hv' (by simp [hvb, Bind.isPlaceholder])
    cases hres : resolve v with
    | none => rw [hres] at hs; cases hs
    | some b => simp [hvb, hres, Bind.isPlaceholder]
  | static => simp [hvb, Bind.isPlaceholder]
  | bound c => simp [hvb, Bind.isPlaceholder]

/-- A screen names `b` when one of its widgets is bound to it. -/
def Names (s : Screen T P R B) (b : B) : Prop := ∃ w ∈ s.widgets, w.bind = .bound b

/-- The incremental rule, with the schema and the catalogue changing.
    Verifies: DS-004 -/
theorem incremental_valid (cat cat' : Catalogue T P R B) (has has' : B → Bool)
    (unchanged changed : List (Screen T P R B))
    (hu : ∀ s ∈ unchanged, Valid cat has s)
    (hkeep : ∀ b, has b = true → has' b = false → ∀ s ∈ unchanged, ¬ Names s b)
    (hcat : ∀ s ∈ unchanged, ∀ w ∈ s.widgets,
      cat'.allowed s.template w.region w.pattern = cat.allowed s.template w.region w.pattern ∧
      cat'.isStatic w.pattern = cat.isStatic w.pattern ∧
      ∀ b, w.bind = .bound b → cat'.accepts w.pattern b = cat.accepts w.pattern b)
    (hc : ∀ s ∈ changed, check cat' has' s = true) :
    ∀ s ∈ unchanged ++ changed, Valid cat' has' s := by
  intro s hs
  rcases List.mem_append.1 hs with hs | hs
  · refine ⟨(hu s hs).1, ?_⟩
    intro w hw
    have ok := (hu s hs).2 w hw
    obtain ⟨ha, hst, hacc⟩ := hcat s hs w hw
    refine wc_transfer ok ha hst ?_
    intro b hb
    refine ⟨hacc b hb, ?_⟩
    have hold := (bound_ok ok hb).2.2.2
    cases h' : has' b with
    | true => rfl
    | false => exact absurd ⟨w, hw, hb⟩ (hkeep b hold h' s hs)
  · exact (check_iff cat' has' s).1 (hc s hs)

/-- Without the removal guard, a valid screen stops being valid. -/
theorem unguarded_removal_breaks :
    ∃ (cat : Catalogue Unit Unit Unit Unit) (has has' : Unit → Bool) (s : Screen Unit Unit Unit Unit),
      Valid cat has s ∧ ¬ Valid cat has' s := by
  refine ⟨⟨fun _ _ _ => true, fun _ => false, fun _ _ => true⟩, fun _ => true, fun _ => false,
    ⟨(), [⟨0, (), (), .bound ()⟩]⟩, ?_, ?_⟩
  · exact ⟨by simp, by simp [widgetCheck, bindOk]⟩
  · intro h
    have := h.2 ⟨0, (), (), .bound ()⟩ (by simp)
    simp [widgetCheck, bindOk] at this

/-- Orphan the widgets that no longer pass under the new catalogue and schema: a binding that stopped
    existing or changed shape, a pattern the catalogue no longer allows where it is, a binding kind it
    no longer accepts. -/
def orphan (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B) : Screen T P R B :=
  { s with widgets := s.widgets.filter (widgetCheck cat has s.template) }

/-- Orphaning keeps a screen valid under any new catalogue and schema: what is left passes the check,
    and ids stay distinct.
    Verifies: DS-013 -/
theorem orphan_valid (cat cat' : Catalogue T P R B) (has has' : B → Bool) (s : Screen T P R B)
    (hv : Valid cat has s) : Valid cat' has' (orphan cat' has' s) := by
  refine ⟨List.Nodup.sublist (List.Sublist.map _ List.filter_sublist) hv.1, ?_⟩
  intro w hw
  simp only [orphan, List.mem_filter] at hw
  exact hw.2

/-- Orphaning removes only widgets that fail under the new catalogue and schema.
    Verifies: DS-013 -/
theorem orphan_removes_only_failing (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (w : Widget P R B) (hw : w ∈ s.widgets) (hgone : w ∉ (orphan cat has s).widgets) :
    widgetCheck cat has s.template w = false := by
  simp only [orphan, List.mem_filter, not_and] at hgone
  cases h : widgetCheck cat has s.template w
  · rfl
  · exact absurd h (hgone hw)

/-- Orphaning removes nothing from a screen that still passes.
    Verifies: DS-013 -/
theorem orphan_keeps_valid (cat : Catalogue T P R B) (has : B → Bool) (s : Screen T P R B)
    (hv : Valid cat has s) : orphan cat has s = s := by
  cases s with
  | mk t ws =>
    simp only [orphan, Screen.mk.injEq, true_and]
    exact List.filter_eq_self.mpr (fun w hw => hv.2 w hw)

end Strata.UI.Composition
