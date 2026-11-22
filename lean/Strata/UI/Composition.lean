/-
Compositions and wireframe fidelity (docs/design-system.md, "Screens as compositions" and
"Wireframes").

A screen is a template and a list of widgets. A widget is a pattern of the catalogue placed in a
region of the template, with a binding to the data it shows (a section of a view, a form, an action)
or none: a widget with no binding is a wireframe placeholder, drawn from the sample shape it
declares. The catalogue says which patterns a template allows in each region, and which patterns
need data.

- **Valid**: every widget is allowed where it is placed, and every binding it has names something
  the tenant schema defines. A placeholder is valid; that is what lets a wireframe exist before the
  data model does.
- **Full fidelity**: every widget whose pattern needs data has a binding.

Proved:

- the check the kernel runs is exactly validity (`check_iff`), so a composition that passes it names
  only bindings the schema defines (`checked_bindings_resolve`);
- a valid composition at full fidelity has every data widget bound to something that exists
  (`full_valid_bound`);
- binding a placeholder to something that exists keeps the composition valid (`bindAt_valid`) and
  leaves one placeholder fewer (`bindAt_unbound`);
- refinement one widget at a time is valid at every step and ends at full fidelity
  (`refine_valid`, `refine_full`);
- the incremental rule: a schema change that removes nothing an unchanged screen names, applied with
  every changed screen checked against the new schema, leaves every screen valid
  (`incremental_valid`); without the removal guard it fails (`unguarded_removal_breaks`).

The model abstracts what a binding is (a type `B`) and what the schema defines (`has : B → Bool`).
No `sorry`, no added axioms.
-/

namespace Strata.UI.Composition

/-- A widget: a pattern placed in a region, bound or a placeholder. -/
structure Widget (P R B : Type) where
  pattern : P
  region : R
  bind : Option B

/-- What the catalogue says about templates and patterns. -/
structure Catalogue (T P R : Type) where
  allowed : T → R → P → Bool
  needsData : P → Bool

/-- A screen: its template and its widgets. -/
structure Screen (T P R B : Type) where
  template : T
  widgets : List (Widget P R B)

variable {T P R B : Type}

/-- One widget is allowed where it is, and its binding, if any, exists. -/
def WidgetOk (cat : Catalogue T P R) (has : B → Bool) (t : T) (w : Widget P R B) : Prop :=
  cat.allowed t w.region w.pattern = true ∧ ∀ b, w.bind = some b → has b = true

/-- A screen is valid when every widget is. -/
def Valid (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B) : Prop :=
  ∀ w ∈ s.widgets, WidgetOk cat has s.template w

/-- The kernel's check of one widget. -/
def widgetCheck (cat : Catalogue T P R) (has : B → Bool) (t : T) (w : Widget P R B) : Bool :=
  cat.allowed t w.region w.pattern && (match w.bind with | none => true | some b => has b)

/-- The kernel's check of a screen. -/
def check (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B) : Bool :=
  s.widgets.all (widgetCheck cat has s.template)

theorem widgetCheck_iff (cat : Catalogue T P R) (has : B → Bool) (t : T) (w : Widget P R B) :
    widgetCheck cat has t w = true ↔ WidgetOk cat has t w := by
  unfold widgetCheck WidgetOk
  cases hb : w.bind with
  | none => simp
  | some b => simp

/-- Verifies: DS-001 -/
theorem check_iff (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B) :
    check cat has s = true ↔ Valid cat has s := by
  unfold check Valid
  simp only [List.all_eq_true]
  exact ⟨fun h w hw => (widgetCheck_iff cat has _ w).1 (h w hw),
         fun h w hw => (widgetCheck_iff cat has _ w).2 (h w hw)⟩

/-- A composition that passes the check names only bindings the schema defines.
    Verifies: DS-001 -/
theorem checked_bindings_resolve (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B)
    (h : check cat has s = true) : ∀ w ∈ s.widgets, ∀ b, w.bind = some b → has b = true :=
  fun w hw => ((check_iff cat has s).1 h w hw).2

/-- Full fidelity: every widget whose pattern needs data is bound. -/
def Full (cat : Catalogue T P R) (s : Screen T P R B) : Prop :=
  ∀ w ∈ s.widgets, cat.needsData w.pattern = true → w.bind.isSome = true

/-- Verifies: DS-002 -/
theorem full_valid_bound (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B)
    (hv : Valid cat has s) (hf : Full cat s) :
    ∀ w ∈ s.widgets, cat.needsData w.pattern = true → ∃ b, w.bind = some b ∧ has b = true := by
  intro w hw hn
  have hs := hf w hw hn
  cases hb : w.bind with
  | none => rw [hb] at hs; cases hs
  | some b => exact ⟨b, rfl, (hv w hw).2 b hb⟩

/-- Bind the `i`-th widget of a list to `b`. -/
def setBind (b : B) : Nat → List (Widget P R B) → List (Widget P R B)
  | _, [] => []
  | 0, w :: ws => { w with bind := some b } :: ws
  | i + 1, w :: ws => w :: setBind b i ws

/-- Bind the `i`-th widget of a screen to `b`. -/
def bindAt (b : B) (i : Nat) (s : Screen T P R B) : Screen T P R B :=
  { s with widgets := setBind b i s.widgets }

theorem mem_setBind (b : B) : ∀ (i : Nat) (ws : List (Widget P R B)) (w : Widget P R B),
    w ∈ setBind b i ws → w ∈ ws ∨ ∃ v ∈ ws, w = { v with bind := some b }
  | _, [], _, h => by simp [setBind] at h
  | 0, v :: vs, w, h => by
    simp only [setBind, List.mem_cons] at h
    rcases h with h | h
    · exact Or.inr ⟨v, by simp, h⟩
    · exact Or.inl (by simp [h])
  | i + 1, v :: vs, w, h => by
    simp only [setBind, List.mem_cons] at h
    rcases h with h | h
    · exact Or.inl (by simp [h])
    · rcases mem_setBind b i vs w h with h' | ⟨u, hu, rfl⟩
      · exact Or.inl (by simp [h'])
      · exact Or.inr ⟨u, by simp [hu], rfl⟩

/-- Binding a placeholder to something the schema defines keeps the composition valid.
    Verifies: DS-003 -/
theorem bindAt_valid (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B) (b : B)
    (i : Nat) (hv : Valid cat has s) (hb : has b = true) : Valid cat has (bindAt b i s) := by
  intro w hw
  rcases mem_setBind b i s.widgets w hw with h | ⟨v, hv', rfl⟩
  · exact hv w h
  · refine ⟨(hv v hv').1, ?_⟩
    intro c hc
    simp at hc
    exact hc ▸ hb

/-- The placeholders that still need data. -/
def unbound (cat : Catalogue T P R) (ws : List (Widget P R B)) : Nat :=
  (ws.filter (fun w => cat.needsData w.pattern && w.bind.isNone)).length

/-- Binding a placeholder that needs data leaves one fewer.
    Verifies: DS-003 -/
theorem bindAt_unbound (cat : Catalogue T P R) (b : B) :
    ∀ (i : Nat) (ws : List (Widget P R B)) (w : Widget P R B), ws[i]? = some w →
      cat.needsData w.pattern = true → w.bind = none →
      unbound cat (setBind b i ws) + 1 = unbound cat ws
  | _, [], _, h, _, _ => by simp at h
  | 0, v :: vs, w, h, hn, hb => by
    simp at h
    subst h
    simp [unbound, setBind, List.filter, hn, hb]
  | i + 1, v :: vs, w, h, hn, hb => by
    simp at h
    have ih := bindAt_unbound cat b i vs w (by simpa using h) hn hb
    unfold unbound at ih ⊢
    simp only [setBind, List.filter]
    split <;> simp_all

/-- Refinement one widget at a time: the first `k` widgets resolved, the rest as they were. -/
def refine (resolve : Widget P R B → Option B) (k : Nat) (ws : List (Widget P R B)) :
    List (Widget P R B) :=
  (ws.take k).map (fun w => match w.bind, resolve w with
    | none, some b => { w with bind := some b }
    | _, _ => w) ++ ws.drop k

/-- Every step of refinement is valid, when what each placeholder is bound to exists.
    Verifies: DS-003 -/
theorem refine_valid (cat : Catalogue T P R) (has : B → Bool) (s : Screen T P R B)
    (resolve : Widget P R B → Option B) (hv : Valid cat has s)
    (hr : ∀ w ∈ s.widgets, ∀ b, resolve w = some b → has b = true) (k : Nat) :
    Valid cat has { s with widgets := refine resolve k s.widgets } := by
  intro w hw
  simp only [refine, List.mem_append, List.mem_map] at hw
  rcases hw with ⟨v, hv', rfl⟩ | hw
  · have hvs : v ∈ s.widgets := List.mem_of_mem_take hv'
    have ok := hv v hvs
    cases hb : v.bind with
    | none =>
      cases hres : resolve v with
      | none => simp only [hb, hres]; exact ok
      | some c =>
        simp only [hb, hres]
        refine ⟨ok.1, ?_⟩
        intro d hd
        simp at hd
        exact hd ▸ hr v hvs c hres
    | some c => simp only [hb]; exact ok
  · exact hv w (List.mem_of_mem_drop hw)

/-- Refinement of every widget ends at full fidelity, when every placeholder that needs data is
    resolved.
    Verifies: DS-003 -/
theorem refine_full (cat : Catalogue T P R) (s : Screen T P R B)
    (resolve : Widget P R B → Option B)
    (hr : ∀ w ∈ s.widgets, cat.needsData w.pattern = true → w.bind = none →
      (resolve w).isSome = true) :
    Full cat { s with widgets := refine resolve s.widgets.length s.widgets } := by
  intro w hw hn
  simp only [refine, List.take_length, List.drop_length, List.append_nil, List.mem_map] at hw
  rcases hw with ⟨v, hv, rfl⟩
  cases hb : v.bind with
  | none =>
    have hs := hr v hv
    cases hres : resolve v with
    | none =>
      simp only [hb, hres] at hn ⊢
      have := hs hn hb
      rw [hres] at this
      cases this
    | some c => simp [hb, hres]
  | some c => simp [hb]

/-- A screen names `b` when one of its widgets is bound to it. -/
def Names (s : Screen T P R B) (b : B) : Prop := ∃ w ∈ s.widgets, w.bind = some b

/-- The incremental rule: unchanged screens stay valid when the change removes nothing they name,
    and changed screens are checked against the new schema.
    Verifies: DS-004 -/
theorem incremental_valid (cat : Catalogue T P R) (has has' : B → Bool)
    (unchanged changed : List (Screen T P R B))
    (hu : ∀ s ∈ unchanged, Valid cat has s)
    (hkeep : ∀ b, has b = true → has' b = false → ∀ s ∈ unchanged, ¬ Names s b)
    (hc : ∀ s ∈ changed, check cat has' s = true) :
    ∀ s ∈ unchanged ++ changed, Valid cat has' s := by
  intro s hs
  rcases List.mem_append.1 hs with hs | hs
  · intro w hw
    refine ⟨(hu s hs w hw).1, ?_⟩
    intro b hb
    have hold := (hu s hs w hw).2 b hb
    cases h' : has' b with
    | true => rfl
    | false => exact absurd ⟨w, hw, hb⟩ (hkeep b hold h' s hs)
  · exact (check_iff cat has' s).1 (hc s hs)

/-- Without the removal guard, a valid screen stops being valid. -/
theorem unguarded_removal_breaks :
    ∃ (cat : Catalogue Unit Unit Unit) (has has' : Unit → Bool) (s : Screen Unit Unit Unit Unit),
      Valid cat has s ∧ ¬ Valid cat has' s := by
  refine ⟨⟨fun _ _ _ => true, fun _ => true⟩, fun _ => true, fun _ => false,
    ⟨(), [⟨(), (), some ()⟩]⟩, ?_, ?_⟩
  · intro w hw
    simp at hw
    subst hw
    exact ⟨rfl, fun _ _ => rfl⟩
  · intro h
    have := (h ⟨(), (), some ()⟩ (by simp)).2 () rfl
    cases this

end Strata.UI.Composition
