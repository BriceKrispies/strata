/-
Who sees what of a composition (docs/design-system.md, "Visibility" and "Role-based views").

A composition is a tree. Its inner nodes are **containers** (a screen, a region, a layout container,
a navigation group); its leaves are **elements** (a widget, a field rendering, an action, a
navigation entry). Every node has a declared audience: anyone, every signed-in person, the holders
of one of a list of roles, or the signed-in holders of one of a list of roles. An element also has:

- a **data audience**, derived from the tenant's policies and never written: the roles with a Read
  grant on the object type it shows, with or without a condition (an upper bound of who may get a
  row), or, for an action, the invokers of its operation (`audOf`);
- a **content audience**, also derived: the roles with an *unconditional* Read grant, or every role
  with a grant when the author marks the widget own-scoped (it shows each person their own rows);
  a content audience is always inside the data audience;
- whether it **counts** as content: a widget that binds data or offers an action counts; a static
  block of prose or links does not.

Every principal holds the role `pub` (`roles_of` in `crates/kernel/src/rules.rs` gives `public` to
the anonymous principal and to every user), so a grant to `pub` makes data public. Automation
principals hold only their run-as roles; they draw no interface and are not viewers here.

The server serves a viewer the elements the viewer sees under their declared and data audiences,
inside containers the viewer sees, and **prunes a container left with nothing**, so no region,
heading or box is served without an element in it (`render`).

Proved:

- **soundness** (`render_sound`): every label served comes from a node the viewer sees, under
  containers the viewer sees;
- **no empty container** (`served_has_element`): every container served holds a served element;
- **no trace** (`hidden_leaves_no_trace`, `hidden_leaves_no_trace_in`): a node the viewer is not
  served changes nothing in what the viewer is served, wherever it is among its siblings, including
  the container around it, which is pruned exactly as if the node were not there;
- **actions follow the server's own check** (`audOf_sees`): the audience derived from an operation's
  invokers admits a viewer exactly when the kernel's invoke check (`operations.rs`, stage 1) does not
  refuse that viewer;
- **data audiences follow policies** (`dataAud_sees`), **public data is public**
  (`public_grant_anon_sees`), and content audiences are inside data audiences (`contentAud_sub`);
- **the content check** (`checked_screen_has_content`): a screen that passes it, served to any
  viewer it is served to, holds an element that counts and whose content audience holds that
  viewer. The check reasons on the least viewers of the screen's audience, which is sound because
  every audience here is monotone in the viewer's roles (`sees_mono`). A condition such as
  `not has_role(banned)` is not monotone; a grant with any condition is conditional, and
  conditional grants count only for own-scoped widgets, so the check never relies on one.

No `sorry`, no added axioms.
-/

namespace Strata.UI.Visibility

/-- The role every principal holds (`public`). -/
def pub : Nat := 0

/-- A viewer: anonymous, or signed in with roles. -/
inductive Viewer where
  | anon
  | member (roles : List Nat)

/-- The roles a viewer holds: always `pub`. -/
def rolesOf : Viewer → List Nat
  | .anon => [pub]
  | .member ps => pub :: ps

/-- Whether a viewer is signed in. -/
def Viewer.signedIn : Viewer → Bool
  | .anon => false
  | .member _ => true

/-- An audience. -/
inductive Aud where
  | anyone
  | signedIn
  | roles (rs : List Nat)
  | memberRoles (rs : List Nat)

/-- Whether a viewer is in an audience. -/
def sees (v : Viewer) : Aud → Bool
  | .anyone => true
  | .signedIn => v.signedIn
  | .roles rs => rs.any (fun r => (rolesOf v).contains r)
  | .memberRoles rs => v.signedIn && rs.any (fun r => (rolesOf v).contains r)

/-- A node: a container with children, or an element. -/
inductive Node where
  | box (label : Nat) (aud : Aud) (kids : List Node)
  | elem (label : Nat) (aud data content : Aud) (counts : Bool)

/-- What a viewer is served. -/
inductive Out where
  | box (label : Nat) (kids : List Out)
  | elem (label : Nat) (counts : Bool)

mutual
  /-- The served form of a node: an element the viewer sees, a container the viewer sees that
      holds something served; otherwise nothing. -/
  def render (v : Viewer) : Node → Option Out
    | .box l a ks =>
      if sees v a then
        match renderAll v ks with
        | [] => none
        | o :: os => some (.box l (o :: os))
      else none
    | .elem l a d _ c => if sees v a && sees v d then some (.elem l c) else none
  /-- The served forms of the nodes the viewer is served, in order. -/
  def renderAll (v : Viewer) : List Node → List Out
    | [] => []
    | n :: ns =>
      match render v n with
      | some o => o :: renderAll v ns
      | none => renderAll v ns
end

/-- The label of a node. -/
def Node.label : Node → Nat
  | .box l _ _ => l
  | .elem l _ _ _ _ => l

/-- Whether a viewer sees a node itself (not its children). -/
def here (v : Viewer) : Node → Bool
  | .box _ a _ => sees v a
  | .elem _ a d _ _ => sees v a && sees v d

/-- The children of a node. -/
def Node.kids : Node → List Node
  | .box _ _ ks => ks
  | .elem _ _ _ _ _ => []

mutual
  /-- Every label of a served tree. -/
  def labels : Out → List Nat
    | .box l ks => l :: labelsAll ks
    | .elem l _ => [l]
  /-- Every label of a list of served trees. -/
  def labelsAll : List Out → List Nat
    | [] => []
    | o :: os => labels o ++ labelsAll os
end

/-- A label is reached in a node through nodes the viewer sees. -/
inductive Served (v : Viewer) : Node → Nat → Prop where
  | self (n : Node) : here v n = true → Served v n n.label
  | child (n k : Node) (l : Nat) : here v n = true → k ∈ n.kids → Served v k l → Served v n l

mutual
  /-- Verifies: DS-007 -/
  theorem render_sound (v : Viewer) : ∀ (n : Node) (o : Out) (l : Nat),
      render v n = some o → l ∈ labels o → Served v n l
    | .box lab a ks, o, l, h, hl => by
      unfold render at h
      by_cases ha : sees v a = true
      · rw [if_pos ha] at h
        have hh : here v (.box lab a ks) = true := by simpa [here] using ha
        cases hr : renderAll v ks with
        | nil => rw [hr] at h; cases h
        | cons o' os =>
          rw [hr] at h
          cases h
          simp only [labels, List.mem_cons] at hl
          rcases hl with rfl | hl
          · exact Served.self _ hh
          · rw [← hr] at hl
            obtain ⟨k, hk, hs⟩ := renderAll_sound v ks l hl
            exact Served.child _ k l hh (by simpa [Node.kids] using hk) hs
      · rw [if_neg ha] at h
        cases h
    | .elem lab a d c cnt, o, l, h, hl => by
      unfold render at h
      by_cases hv : (sees v a && sees v d) = true
      · rw [if_pos hv] at h
        cases h
        simp only [labels, List.mem_singleton] at hl
        subst hl
        exact Served.self _ (by simpa [here] using hv)
      · rw [if_neg hv] at h
        cases h
  theorem renderAll_sound (v : Viewer) : ∀ (ns : List Node) (l : Nat),
      l ∈ labelsAll (renderAll v ns) → ∃ k ∈ ns, Served v k l
    | [], l, h => by simp [renderAll, labelsAll] at h
    | n :: ns, l, h => by
      unfold renderAll at h
      cases hr : render v n with
      | none =>
        rw [hr] at h
        obtain ⟨k, hk, hs⟩ := renderAll_sound v ns l h
        exact ⟨k, List.mem_cons_of_mem _ hk, hs⟩
      | some o =>
        rw [hr] at h
        simp only [labelsAll, List.mem_append] at h
        rcases h with h | h
        · exact ⟨n, List.mem_cons_self, render_sound v n o l hr h⟩
        · obtain ⟨k, hk, hs⟩ := renderAll_sound v ns l h
          exact ⟨k, List.mem_cons_of_mem _ hk, hs⟩
end

mutual
  /-- The number of elements in a served tree. -/
  def elems : Out → Nat
    | .box _ ks => elemsAll ks
    | .elem _ _ => 1
  /-- The number of elements in a list of served trees. -/
  def elemsAll : List Out → Nat
    | [] => 0
    | o :: os => elems o + elemsAll os
end

mutual
  theorem render_elems (v : Viewer) : ∀ (n : Node) (o : Out), render v n = some o → 0 < elems o
    | .box lab a ks, o, h => by
      unfold render at h
      by_cases ha : sees v a = true
      · rw [if_pos ha] at h
        cases hr : renderAll v ks with
        | nil => rw [hr] at h; cases h
        | cons o' os =>
          rw [hr] at h
          cases h
          have := renderAll_elems v ks
          rw [hr] at this
          simp only [elems]
          exact this (by simp)
      · rw [if_neg ha] at h; cases h
    | .elem lab a d c cnt, o, h => by
      unfold render at h
      by_cases hv : (sees v a && sees v d) = true
      · rw [if_pos hv] at h; cases h; simp [elems]
      · rw [if_neg hv] at h; cases h
  theorem renderAll_elems (v : Viewer) : ∀ (ns : List Node),
      renderAll v ns ≠ [] → 0 < elemsAll (renderAll v ns)
    | [], h => by simp [renderAll] at h
    | n :: ns, h => by
      unfold renderAll at h ⊢
      cases hr : render v n with
      | none => rw [hr] at h; exact renderAll_elems v ns h
      | some o =>
        simp only [elemsAll]
        have := render_elems v n o hr
        omega
end

/-- Every container served holds a served element: no region, heading or box is served empty.
    Verifies: DS-007 -/
theorem served_has_element (v : Viewer) (n : Node) (o : Out) (h : render v n = some o) :
    0 < elems o := render_elems v n o h

theorem renderAll_append (v : Viewer) (a b : List Node) :
    renderAll v (a ++ b) = renderAll v a ++ renderAll v b := by
  induction a with
  | nil => simp [renderAll]
  | cons n ns ih =>
    cases hr : render v n with
    | none => simp [renderAll, hr, ih]
    | some o => simp [renderAll, hr, ih]

/-- A node the viewer is not served leaves no trace among its siblings.
    Verifies: DS-008 -/
theorem hidden_leaves_no_trace (v : Viewer) (a b : List Node) (h : Node)
    (hh : render v h = none) : renderAll v (a ++ h :: b) = renderAll v (a ++ b) := by
  rw [renderAll_append, renderAll_append]
  simp [renderAll, hh]

/-- A node the viewer is not served leaves no trace in the container around it, which is served,
    or pruned, exactly as if the node were not there.
    Verifies: DS-008 -/
theorem hidden_leaves_no_trace_in (v : Viewer) (l : Nat) (aud : Aud) (a b : List Node) (h : Node)
    (hh : render v h = none) :
    render v (.box l aud (a ++ h :: b)) = render v (.box l aud (a ++ b)) := by
  simp only [render, hidden_leaves_no_trace v a b h hh]

/-- An element the viewer does not see is not served. -/
theorem render_elem_none (v : Viewer) (l : Nat) (a d c : Aud) (cnt : Bool)
    (h : (sees v a && sees v d) = false) : render v (.elem l a d c cnt) = none := by
  simp [render, h]

/-- Who may invoke an operation: every signed-in person, or the holders of these roles (D-6547). -/
structure Invokers where
  signedIn : Bool
  roles : List Nat

/-- Stage 1 of an invocation (`operations.rs`): refused unless the caller is a signed-in person
    whom the invokers name, the caller's roles including `public`. -/
def invokeRefused (inv : Invokers) : Viewer → Bool
  | .anon => true
  | v@(.member _) => !inv.signedIn && !(inv.roles.any (fun r => (rolesOf v).contains r))

/-- The audience of an action, derived from the invokers of its operation. -/
def audOf (inv : Invokers) : Aud := if inv.signedIn then .signedIn else .memberRoles inv.roles

/-- Verifies: DS-009 -/
theorem audOf_sees (inv : Invokers) (v : Viewer) : sees v (audOf inv) = !invokeRefused inv v := by
  cases v with
  | anon => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused, Viewer.signedIn]
  | member ps => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused, Viewer.signedIn]

/-- A Read grant: a role, an object type, and whether it has a condition. -/
structure Grant where
  role : Nat
  obj : Nat
  conditional : Bool

/-- The data audience of an object type: every role with a Read grant on it. -/
def dataAud (gs : List Grant) (obj : Nat) : Aud :=
  .roles ((gs.filter (fun g => g.obj == obj)).map (·.role))

/-- The content audience: the roles with an unconditional grant, or with any grant when the widget
    is own-scoped. -/
def contentAud (gs : List Grant) (obj : Nat) (own : Bool) : Aud :=
  .roles ((gs.filter (fun g => g.obj == obj && (own || !g.conditional))).map (·.role))

/-- Verifies: DS-010 -/
theorem dataAud_sees (gs : List Grant) (obj : Nat) (v : Viewer) :
    sees v (dataAud gs obj) = true ↔ ∃ g ∈ gs, g.obj = obj ∧ g.role ∈ rolesOf v := by
  simp only [dataAud, sees, List.any_eq_true, List.mem_map, List.mem_filter, beq_iff_eq,
    List.contains_iff_mem]
  constructor
  · rintro ⟨r, ⟨g, ⟨hg, ho⟩, rfl⟩, hr⟩
    exact ⟨g, hg, ho, hr⟩
  · rintro ⟨g, hg, ho, hr⟩
    exact ⟨g.role, ⟨g, ⟨hg, ho⟩, rfl⟩, hr⟩

/-- A grant to `public` makes the data audience hold the anonymous visitor.
    Verifies: DS-010 -/
theorem public_grant_anon_sees (gs : List Grant) (obj : Nat) (c : Bool)
    (h : ⟨pub, obj, c⟩ ∈ gs) : sees .anon (dataAud gs obj) = true :=
  (dataAud_sees gs obj .anon).2 ⟨_, h, rfl, by simp [rolesOf]⟩

/-- The content audience is inside the data audience.
    Verifies: DS-010 -/
theorem contentAud_sub (gs : List Grant) (obj : Nat) (own : Bool) (v : Viewer)
    (h : sees v (contentAud gs obj own) = true) : sees v (dataAud gs obj) = true := by
  simp only [contentAud, sees, List.any_eq_true, List.mem_map, List.mem_filter, Bool.and_eq_true,
    beq_iff_eq, List.contains_iff_mem] at h
  obtain ⟨r, ⟨g, ⟨hg, ho, _⟩, rfl⟩, hr⟩ := h
  exact (dataAud_sees gs obj v).2 ⟨g, hg, ho, hr⟩

/-- One viewer holds at least the roles of another, and is signed in if the other is. -/
def Le (m v : Viewer) : Prop := (∀ r ∈ rolesOf m, r ∈ rolesOf v) ∧ (m.signedIn = true → v.signedIn = true)

/-- Every audience is monotone in the viewer. -/
theorem sees_mono (a : Aud) (m v : Viewer) (hle : Le m v) (h : sees m a = true) : sees v a = true := by
  cases a with
  | anyone => rfl
  | signedIn => exact hle.2 h
  | roles rs =>
    simp only [sees, List.any_eq_true, List.contains_iff_mem] at h ⊢
    obtain ⟨r, hr, hm⟩ := h
    exact ⟨r, hr, hle.1 r hm⟩
  | memberRoles rs =>
    simp only [sees, Bool.and_eq_true, List.any_eq_true, List.contains_iff_mem] at h ⊢
    obtain ⟨hs, r, hr, hm⟩ := h
    exact ⟨hle.2 hs, r, hr, hle.1 r hm⟩

/-- The least viewers of an audience: every viewer in it holds at least one of them. -/
def minimal : Aud → List Viewer
  | .anyone => [.anon]
  | .signedIn => [.member []]
  | .roles rs => rs.map (fun r => if r = pub then .anon else .member [r])
  | .memberRoles rs => rs.map (fun r => .member [r])

theorem minimal_le (a : Aud) (v : Viewer) (h : sees v a = true) : ∃ m ∈ minimal a, Le m v := by
  cases a with
  | anyone =>
    refine ⟨.anon, by simp [minimal], ?_, by simp [Viewer.signedIn]⟩
    intro r hr
    simp [rolesOf] at hr
    subst hr
    cases v <;> simp [rolesOf]
  | signedIn =>
    cases v with
    | anon => simp [sees, Viewer.signedIn] at h
    | member ps =>
      refine ⟨.member [], by simp [minimal], ?_, fun _ => rfl⟩
      intro r hr
      simp [rolesOf] at hr
      simp [rolesOf, hr]
  | roles rs =>
    simp only [sees, List.any_eq_true, List.contains_iff_mem] at h
    obtain ⟨r, hr, hv⟩ := h
    by_cases hp : r = pub
    · refine ⟨.anon, ?_, ?_, by simp [Viewer.signedIn]⟩
      · simp only [minimal, List.mem_map]; exact ⟨r, hr, by simp [hp]⟩
      · intro x hx
        simp [rolesOf] at hx
        subst hx
        cases v <;> simp [rolesOf]
    · refine ⟨.member [r], ?_, ?_, ?_⟩
      · simp only [minimal, List.mem_map]; exact ⟨r, hr, by simp [hp]⟩
      · intro x hx
        simp [rolesOf] at hx
        rcases hx with rfl | rfl
        · cases v <;> simp [rolesOf]
        · exact hv
      · intro _
        cases v with
        | anon => simp [rolesOf] at hv; exact absurd hv hp
        | member _ => rfl
  | memberRoles rs =>
    simp only [sees, Bool.and_eq_true, List.any_eq_true, List.contains_iff_mem] at h
    obtain ⟨hs, r, hr, hv⟩ := h
    refine ⟨.member [r], ?_, ?_, fun _ => hs⟩
    · simp only [minimal, List.mem_map]; exact ⟨r, hr, rfl⟩
    · intro x hx
      simp [rolesOf] at hx
      rcases hx with rfl | rfl
      · cases v <;> simp [rolesOf]
      · exact hv

mutual
  /-- Whether a viewer reaches, through containers it sees, an element that counts and whose
      declared and content audiences hold it. -/
  def reach (v : Viewer) : Node → Bool
    | .box _ a ks => sees v a && reachAll v ks
    | .elem _ a _ c cnt => cnt && sees v a && sees v c
  /-- Whether a viewer reaches content in one of these nodes. -/
  def reachAll (v : Viewer) : List Node → Bool
    | [] => false
    | n :: ns => reach v n || reachAll v ns
end

mutual
  theorem reach_mono (m v : Viewer) (hle : Le m v) : ∀ n : Node, reach m n = true → reach v n = true
    | .box l a ks, h => by
      simp only [reach, Bool.and_eq_true] at h ⊢
      exact ⟨sees_mono a m v hle h.1, reachAll_mono m v hle ks h.2⟩
    | .elem l a d c cnt, h => by
      simp only [reach, Bool.and_eq_true] at h ⊢
      exact ⟨⟨h.1.1, sees_mono a m v hle h.1.2⟩, sees_mono c m v hle h.2⟩
  theorem reachAll_mono (m v : Viewer) (hle : Le m v) :
      ∀ ns : List Node, reachAll m ns = true → reachAll v ns = true
    | [], h => by simp [reachAll] at h
    | n :: ns, h => by
      simp only [reachAll, Bool.or_eq_true] at h ⊢
      rcases h with h | h
      · exact Or.inl (reach_mono m v hle n h)
      · exact Or.inr (reachAll_mono m v hle ns h)
end

mutual
  /-- The number of served elements that count. -/
  def counted : Out → Nat
    | .box _ ks => countedAll ks
    | .elem _ c => if c then 1 else 0
  /-- The number of served elements that count, in a list. -/
  def countedAll : List Out → Nat
    | [] => 0
    | o :: os => counted o + countedAll os
end

-- Every element's content audience lies inside its data audience.
mutual
  def WellFormed (sub : Aud → Aud → Prop) : Node → Prop
    | .box _ _ ks => WellFormedAll sub ks
    | .elem _ _ d c _ => sub c d
  def WellFormedAll (sub : Aud → Aud → Prop) : List Node → Prop
    | [] => True
    | n :: ns => WellFormed sub n ∧ WellFormedAll sub ns
end

/-- Content inside data, for every viewer. -/
def Sub (c d : Aud) : Prop := ∀ v, sees v c = true → sees v d = true

mutual
  theorem reach_served (v : Viewer) : ∀ n : Node, WellFormed Sub n → reach v n = true →
      ∃ o, render v n = some o ∧ 0 < counted o
    | .box l a ks, hw, h => by
      simp only [reach, Bool.and_eq_true] at h
      obtain ⟨ha, hk⟩ := h
      have hall := reachAll_served v ks (by simpa [WellFormed] using hw) hk
      cases hr : renderAll v ks with
      | nil => rw [hr] at hall; simp [countedAll] at hall
      | cons o os =>
        refine ⟨.box l (o :: os), ?_, ?_⟩
        · simp [render, ha, hr]
        · rw [hr] at hall; simpa [counted] using hall
    | .elem l a d c cnt, hw, h => by
      simp only [reach, Bool.and_eq_true] at h
      obtain ⟨⟨hc, ha⟩, hcon⟩ := h
      have hd : sees v d = true := hw v hcon
      refine ⟨.elem l cnt, by simp [render, ha, hd], by simp [counted, hc]⟩
  theorem reachAll_served (v : Viewer) : ∀ ns : List Node, WellFormedAll Sub ns →
      reachAll v ns = true → 0 < countedAll (renderAll v ns)
    | [], _, h => by simp [reachAll] at h
    | n :: ns, hw, h => by
      simp only [WellFormedAll] at hw
      simp only [reachAll, Bool.or_eq_true] at h
      unfold renderAll
      rcases h with h | h
      · obtain ⟨o, hr, hc⟩ := reach_served v n hw.1 h
        rw [hr]; simp only [countedAll]; omega
      · have := reachAll_served v ns hw.2 h
        cases hr : render v n with
        | none => exact this
        | some o => simp only [countedAll]; omega
end

/-- The content check of a screen: every least viewer of its declared audience reaches content. -/
def contentCheck : Node → Bool
  | n@(.box _ a _) => (minimal a).all (fun m => reach m n)
  | .elem _ _ _ _ _ => true

/-- A screen that passes the content check, served to a viewer in its audience, holds an element
    that counts and whose content audience holds that viewer.
    Verifies: DS-011 -/
theorem checked_screen_has_content (l : Nat) (a : Aud) (ks : List Node) (v : Viewer)
    (hw : WellFormed Sub (.box l a ks)) (hc : contentCheck (.box l a ks) = true)
    (hv : sees v a = true) : ∃ o, render v (.box l a ks) = some o ∧ 0 < counted o := by
  obtain ⟨m, hm, hle⟩ := minimal_le a v hv
  simp only [contentCheck, List.all_eq_true] at hc
  exact reach_served v _ hw (reach_mono m v hle _ (hc m hm))

/-- Without the pruning rule, an empty region's label would be served: the counterexample of the
    first review, a public screen with a public region holding one admin-only widget, renders
    nothing for the anonymous visitor. -/
theorem empty_region_pruned :
    render .anon (.box 1 .anyone [.box 2 .anyone [.elem 3 (.roles [7]) .anyone .anyone true]]) = none := by
  simp [render, renderAll, sees, rolesOf, pub]

/-- The second review's case: members (role 5) read only their own messages (a conditional grant),
    admins (role 7) read all of them. A moderation screen declared for members whose queue widget is
    not own-scoped fails the content check, so it cannot be declared for members. -/
theorem conditional_grant_does_not_count :
    let gs : List Grant := [⟨5, 1, true⟩, ⟨7, 1, false⟩]
    contentCheck (.box 1 (.roles [5])
      [.elem 2 .anyone (dataAud gs 1) (contentAud gs 1 false) true]) = false := by
  simp [contentCheck, minimal, reach, reachAll, sees, contentAud, rolesOf, pub]

end Strata.UI.Visibility
