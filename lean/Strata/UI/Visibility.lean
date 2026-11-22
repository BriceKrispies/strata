/-
Who sees what of a composition (docs/design-system.md, "Visibility" and "Role-based views").

Every element of a composition (the app, a screen, a region, a widget, a field rendering, an action,
a navigation entry, a draft) is a node with two audiences:

- its **declared** audience, written in the composition: anyone, every signed-in person, or the
  holders of any of a list of roles;
- its **data** audience, derived from the tenant's policies and never written by hand: for a widget
  bound to a section, the roles holding a Read policy or permission on the object type it shows;
  for an action, the invokers of its operation (`audOf`).

The server serves a viewer only the nodes the viewer sees under both audiences, below nodes the
viewer sees (`render`); the browser draws what it is served and filters nothing itself.

Proved:

- **soundness** (`render_sound`): every label in what a viewer is served comes from a node the viewer
  sees under both audiences, all of whose ancestors the viewer also sees;
- **no trace** (`hidden_leaves_no_trace`): a node the viewer does not see changes nothing in what
  the viewer is served, wherever it sits among its siblings, so its existence, a placeholder for it,
  the label of a region it would fill and the number of hidden nodes are not disclosed;
- **actions follow the server's own check** (`audOf_sees`): the audience derived from an operation's
  invokers admits a viewer exactly when the invoke check of the kernel
  (`crates/kernel/src/operations.rs`, stage 1) does not refuse that viewer;
- **data audiences follow policies** (`dataAud_sees`): a signed-in viewer sees the data audience of
  an object type exactly when one of the viewer's roles has a Read grant on it;
- **no empty screen by construction** (`content_check_nonempty`, `checked_screen_renders_content`):
  a screen that passes the content check, served to any viewer who sees it, shows at least one of
  its widgets; so an admin panel whose widgets only an admin may read cannot be declared for every
  signed-in person, and a member is never served it.

Roles are numbers and a viewer is anonymous or signed in with a list of roles; the roles of a
viewer's teams and run-as roles are assumed already in that list (`rules::roles_of`). No `sorry`, no
added axioms.
-/

namespace Strata.UI.Visibility

/-- A viewer: anonymous, or signed in with roles. -/
inductive Viewer where
  | anon
  | member (roles : List Nat)

/-- An audience. -/
inductive Aud where
  | anyone
  | signedIn
  | roles (rs : List Nat)

/-- Whether a viewer is in an audience. -/
def sees : Viewer → Aud → Bool
  | _, .anyone => true
  | .anon, _ => false
  | .member _, .signedIn => true
  | .member ps, .roles rs => rs.any (fun r => ps.contains r)

/-- A node of a composition: a label, its declared and data audiences, its children. -/
inductive Node where
  | mk (label : Nat) (aud : Aud) (data : Aud) (kids : List Node)

/-- The label of a node. -/
def Node.label : Node → Nat
  | .mk l _ _ _ => l

/-- The children of a node. -/
def Node.kids : Node → List Node
  | .mk _ _ _ ks => ks

/-- Whether a viewer sees a node, under both its audiences. -/
def visible (v : Viewer) : Node → Bool
  | .mk _ a d _ => sees v a && sees v d

/-- What a viewer is served: labels only, in a tree. -/
inductive Out where
  | mk (label : Nat) (kids : List Out)

mutual
  /-- The served form of a node, or none when the viewer does not see it. -/
  def render (v : Viewer) : Node → Option Out
    | .mk l a d ks => if sees v a && sees v d then some (.mk l (renderAll v ks)) else none
  /-- The served forms of the nodes the viewer sees, in order. -/
  def renderAll (v : Viewer) : List Node → List Out
    | [] => []
    | n :: ns =>
      match render v n with
      | some o => o :: renderAll v ns
      | none => renderAll v ns
end

mutual
  /-- Every label of a served tree. -/
  def labels : Out → List Nat
    | .mk l ks => l :: labelsAll ks
  /-- Every label of a list of served trees. -/
  def labelsAll : List Out → List Nat
    | [] => []
    | o :: os => labels o ++ labelsAll os
end

/-- A label is reached in a node through a chain of nodes the viewer sees. -/
inductive Served (v : Viewer) : Node → Nat → Prop where
  | self (n : Node) : visible v n = true → Served v n n.label
  | child (n k : Node) (l : Nat) : visible v n = true → k ∈ n.kids → Served v k l → Served v n l

mutual
  /-- Verifies: DS-007 -/
  theorem render_sound (v : Viewer) : ∀ (n : Node) (o : Out) (l : Nat),
      render v n = some o → l ∈ labels o → Served v n l
    | .mk lab a d ks, o, l, h, hl => by
      unfold render at h
      by_cases hv : (sees v a && sees v d) = true
      · rw [if_pos hv] at h
        cases h
        have hvis : visible v (.mk lab a d ks) = true := by simpa [visible] using hv
        simp only [labels, List.mem_cons] at hl
        rcases hl with rfl | hl
        · exact Served.self _ hvis
        · obtain ⟨k, hk, hs⟩ := renderAll_sound v ks l hl
          exact Served.child _ k l hvis (by simpa [Node.kids] using hk) hs
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

theorem render_none (v : Viewer) (n : Node) (h : visible v n = false) : render v n = none := by
  cases n with
  | mk l a d ks =>
    unfold render
    simp only [visible] at h
    rw [if_neg (by simp [h])]

theorem renderAll_append (v : Viewer) (a b : List Node) :
    renderAll v (a ++ b) = renderAll v a ++ renderAll v b := by
  induction a with
  | nil => simp [renderAll]
  | cons n ns ih =>
    cases hr : render v n with
    | none => simp [renderAll, hr, ih]
    | some o => simp [renderAll, hr, ih]

/-- A node the viewer does not see leaves no trace in what the viewer is served.
    Verifies: DS-008 -/
theorem hidden_leaves_no_trace (v : Viewer) (a b : List Node) (h : Node)
    (hh : visible v h = false) : renderAll v (a ++ h :: b) = renderAll v (a ++ b) := by
  rw [renderAll_append, renderAll_append]
  simp [renderAll, render_none v h hh]

/-- Who may invoke an operation: every signed-in person, or the holders of these roles (D-6547). -/
structure Invokers where
  signedIn : Bool
  roles : List Nat

/-- The kernel's stage 1 of an invocation (`operations.rs`): refused unless a signed-in person whom
    the invokers name. -/
def invokeRefused (inv : Invokers) : Viewer → Bool
  | .anon => true
  | .member ps => !inv.signedIn && !(inv.roles.any (fun r => ps.contains r))

/-- The audience of an action, derived from the invokers of its operation. -/
def audOf (inv : Invokers) : Aud := if inv.signedIn then .signedIn else .roles inv.roles

/-- Verifies: DS-009 -/
theorem audOf_sees (inv : Invokers) (v : Viewer) : sees v (audOf inv) = !invokeRefused inv v := by
  cases v with
  | anon => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused]
  | member ps => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused]

/-- The data audience of an object type: the roles with a Read grant on it. -/
def dataAud (grants : List (Nat × Nat)) (obj : Nat) : Aud :=
  .roles ((grants.filter (fun g => g.2 == obj)).map (·.1))

/-- Verifies: DS-010 -/
theorem dataAud_sees (grants : List (Nat × Nat)) (obj : Nat) (ps : List Nat) :
    sees (.member ps) (dataAud grants obj) = true ↔ ∃ r ∈ ps, (r, obj) ∈ grants := by
  simp only [dataAud, sees, List.any_eq_true, List.mem_map, List.mem_filter, beq_iff_eq,
    List.contains_iff_mem]
  constructor
  · rintro ⟨r, ⟨⟨g, hg, rfl⟩, hr⟩⟩
    exact ⟨g.1, hr, by rcases g with ⟨x, y⟩; simp at hg; rcases hg with ⟨hg, rfl⟩; exact hg⟩
  · rintro ⟨r, hr, hg⟩
    exact ⟨r, ⟨⟨(r, obj), ⟨hg, rfl⟩, rfl⟩, hr⟩⟩

/-- An audience the anonymous viewer is in, every viewer is in. -/
theorem sees_of_anon (a : Aud) (v : Viewer) (h : sees .anon a = true) : sees v a = true := by
  cases a <;> simp [sees] at h <;> simp [sees]

/-- More roles see at least as much. -/
theorem sees_mono (a : Aud) (ps qs : List Nat) (hsub : ∀ r ∈ ps, r ∈ qs)
    (h : sees (.member ps) a = true) : sees (.member qs) a = true := by
  cases a with
  | anyone => rfl
  | signedIn => rfl
  | roles rs =>
    simp only [sees, List.any_eq_true, List.contains_iff_mem] at h ⊢
    obtain ⟨r, hr, hp⟩ := h
    exact ⟨r, hr, hsub r hp⟩

theorem visible_of_anon (n : Node) (v : Viewer) (h : visible .anon n = true) :
    visible v n = true := by
  cases n with
  | mk l a d ks =>
    simp only [visible, Bool.and_eq_true] at h ⊢
    exact ⟨sees_of_anon a v h.1, sees_of_anon d v h.2⟩

theorem visible_mono (n : Node) (ps qs : List Nat) (hsub : ∀ r ∈ ps, r ∈ qs)
    (h : visible (.member ps) n = true) : visible (.member qs) n = true := by
  cases n with
  | mk l a d ks =>
    simp only [visible, Bool.and_eq_true] at h ⊢
    exact ⟨sees_mono a ps qs hsub h.1, sees_mono d ps qs hsub h.2⟩

/-- The least viewers an audience admits: every viewer it admits has the roles of one of them. -/
def minimal : Aud → List Viewer
  | .anyone => [.anon]
  | .signedIn => [.member []]
  | .roles rs => rs.map (fun r => .member [r])

/-- The content check of a screen: each least viewer of its declared audience sees one of its
    widgets. -/
def contentCheck (n : Node) : Bool :=
  match n with
  | .mk _ a _ ks => (minimal a).all (fun v => ks.any (visible v))

/-- Verifies: DS-011 -/
theorem content_check_nonempty (n : Node) (v : Viewer) (hc : contentCheck n = true)
    (hv : visible v n = true) : ∃ k ∈ n.kids, visible v k = true := by
  cases n with
  | mk l a d ks =>
    simp only [contentCheck, List.all_eq_true, List.any_eq_true] at hc
    simp only [visible, Bool.and_eq_true] at hv
    simp only [Node.kids]
    cases a with
    | anyone =>
      obtain ⟨k, hk, hvk⟩ := hc .anon (by simp [minimal])
      exact ⟨k, hk, visible_of_anon k v hvk⟩
    | signedIn =>
      cases v with
      | anon => simp [sees] at hv
      | member ps =>
        obtain ⟨k, hk, hvk⟩ := hc (.member []) (by simp [minimal])
        exact ⟨k, hk, visible_mono k [] ps (by simp) hvk⟩
    | roles rs =>
      cases v with
      | anon => simp [sees] at hv
      | member ps =>
        have h1 := hv.1
        simp only [sees, List.any_eq_true, List.contains_iff_mem] at h1
        obtain ⟨r, hr, hp⟩ := h1
        obtain ⟨k, hk, hvk⟩ := hc (.member [r]) (by simp [minimal]; exact hr)
        exact ⟨k, hk, visible_mono k [r] ps (by simpa using hp) hvk⟩

theorem renderAll_ne_nil (v : Viewer) (ks : List Node) (k : Node) (hk : k ∈ ks)
    (hv : visible v k = true) : renderAll v ks ≠ [] := by
  induction ks with
  | nil => simp at hk
  | cons n ns ih =>
    unfold renderAll
    cases hr : render v n with
    | some o => simp
    | none =>
      simp only
      rcases List.mem_cons.1 hk with rfl | hk
      · cases k with
        | mk l a d kids =>
          unfold render at hr
          simp only [visible] at hv
          rw [if_pos hv] at hr
          cases hr
      · exact ih hk

/-- A checked screen served to a viewer who sees it shows at least one widget.
    Verifies: DS-011 -/
theorem checked_screen_renders_content (n : Node) (v : Viewer) (hc : contentCheck n = true)
    (hv : visible v n = true) : ∃ o, render v n = some o ∧ ∃ l ks, o = .mk l ks ∧ ks ≠ [] := by
  obtain ⟨k, hk, hvk⟩ := content_check_nonempty n v hc hv
  cases n with
  | mk l a d ks =>
    refine ⟨.mk l (renderAll v ks), ?_, l, renderAll v ks, rfl, ?_⟩
    · unfold render
      simp only [visible] at hv
      rw [if_pos hv]
    · exact renderAll_ne_nil v ks k (by simpa [Node.kids] using hk) hvk

end Strata.UI.Visibility
