/-
Who sees what of a composition (docs/design-system.md, "Visibility" and "Role-based views").

A composition is a tree. Its inner nodes are **containers** (a shell, a screen, a region, a layout
container, a navigation group); its leaves are **elements** (a widget, a field rendering, an action,
a navigation entry, a link). Every node has a **declared** audience, written by the owner or the
agent in roles or audience groups: anyone, every signed-in person, the holders of one of a list of
roles, or the signed-in holders of one of a list of roles. An element also has:

- a **data audience**, derived from the tenant's policies: the roles with a Read grant on the object
  type it shows, or, for an action, the invokers of its operation (`audOf`). It only ever narrows what
  is served; it never decides who a screen is for;
- a **content audience** (for the content lint): the roles with an unconditional grant, or with a
  grant whose condition mentions `principal` (a row of one's own), as `audience.rs`'s `scan` finds;
- whether it **counts** as content (binds data or offers an action), and whether it is **admin**:
  marked by the author, or derived by the kernel (an action whose invokers name roles, or whose
  conditions test a role or negate anything).

A container may **need content**: then it is served only when an element that counts is served
inside it. Every principal holds the role `pub` (`roles_of`); automation principals draw no interface
and are not viewers.

Proved:

- **soundness** (`render_sound`): every label served comes from a node the viewer sees, inside
  containers the viewer sees;
- **no empty container** (`served_has_element`), and **no container that needs content without it**
  (`served_needs_content`);
- **no trace** (`hidden_leaves_no_trace`, `hidden_leaves_no_trace_in`): a node the viewer is not served
  changes nothing the viewer is served, wherever it is among its siblings, including the container;
- **admin by construction** (`admin_needs_role`): in a composition that passes the admin check (every
  admin element lies under, or is itself, a declared audience of roles none of which is `public` or a
  role every member holds by default), an admin element is served only to a viewer holding one of
  those roles; an anonymous visitor and a member holding only default roles are never served one
  (`admin_never_to_default_member`); an admin audience naming the default role is refused
  (`admin_default_role_refused`);
- **shells: first match wins** (`select_sees`, `select_first`); **screen variants: the first that
  serves the viewer something wins** (`selectServed_sound`, `selectServed_first`), so a viewer in an
  earlier variant's audience who may read none of its content falls through (`variant_falls_through`);
- actions follow the kernel's invoker check (`audOf_sees`); data audiences follow grants, the public
  role included (`dataAud_sees`, `public_grant_anon_sees`); content inside data (`contentAud_sub`);
- **the class part and its narrowing** (`narrow_class_eq_render`, `renderClass_shared`): the gateway's
  class part, built from declared audiences alone and narrowed per request by the viewer's data
  audiences, is exactly what rendering for that viewer gives, and two viewers who agree on every declared
  audience share the class part; signed-in status is one of the things that decide (`signed_in_decides`);
- the **content lint** (`checked_screen_has_content`) holds for what it reasons on; it is a lint that
  warns, not the guarantee that keeps admin screens from members (that is the admin check).

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

/-- An audience. Audience groups are expanded to their roles before this point. -/
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

/-- A node: a container (that may need content) with children, or an element. -/
inductive Node where
  | box (label : Nat) (aud : Aud) (needs : Bool) (kids : List Node)
  | elem (label : Nat) (aud data content : Aud) (counts admin : Bool)

/-- What a viewer is served. -/
inductive Out where
  | box (label : Nat) (kids : List Out)
  | elem (label : Nat) (counts admin : Bool)

mutual
  /-- The number of served elements that count. -/
  def counted : Out → Nat
    | .box _ ks => countedAll ks
    | .elem _ c _ => if c then 1 else 0
  /-- The number of served elements that count, in a list. -/
  def countedAll : List Out → Nat
    | [] => 0
    | o :: os => counted o + countedAll os
end

/-- A container's served form, from its children's: pruned when empty, or when it needs content and
    none of its served elements counts. -/
def boxOut (l : Nat) (needs : Bool) : List Out → Option Out
  | [] => none
  | o :: os => if needs = true ∧ countedAll (o :: os) = 0 then none else some (.box l (o :: os))

theorem boxOut_some {l : Nat} {needs : Bool} {os : List Out} {o : Out} (h : boxOut l needs os = some o) :
    o = .box l os ∧ os ≠ [] ∧ (needs = true → 0 < countedAll os) := by
  cases os with
  | nil => simp [boxOut] at h
  | cons x xs =>
    simp only [boxOut] at h
    split at h
    · cases h
    · rename_i hn
      cases h
      refine ⟨rfl, by simp, fun hneed => ?_⟩
      have : ¬ countedAll (x :: xs) = 0 := fun h0 => hn ⟨hneed, h0⟩
      omega

mutual
  /-- The served form of a node. -/
  def render (v : Viewer) : Node → Option Out
    | .box l a n ks => if sees v a then boxOut l n (renderAll v ks) else none
    | .elem l a d _ c adm => if sees v a && sees v d then some (.elem l c adm) else none
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
  | .box l _ _ _ => l
  | .elem l _ _ _ _ _ => l

/-- Whether a viewer sees a node itself (not its children). -/
def here (v : Viewer) : Node → Bool
  | .box _ a _ _ => sees v a
  | .elem _ a d _ _ _ => sees v a && sees v d

/-- The children of a node. -/
def Node.kids : Node → List Node
  | .box _ _ _ ks => ks
  | .elem _ _ _ _ _ _ => []

mutual
  /-- Every label of a served tree. -/
  def labels : Out → List Nat
    | .box l ks => l :: labelsAll ks
    | .elem l _ _ => [l]
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
    | .box lab a nd ks, o, l, h, hl => by
      simp only [render] at h
      by_cases ha : sees v a = true
      · rw [if_pos ha] at h
        have hh : here v (.box lab a nd ks) = true := by simpa [here] using ha
        obtain ⟨rfl, _, _⟩ := boxOut_some h
        simp only [labels, List.mem_cons] at hl
        rcases hl with rfl | hl
        · exact Served.self _ hh
        · obtain ⟨k, hk, hs⟩ := renderAll_sound v ks l hl
          exact Served.child _ k l hh (by simpa [Node.kids] using hk) hs
      · rw [if_neg ha] at h
        cases h
    | .elem lab a d c cnt adm, o, l, h, hl => by
      simp only [render] at h
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
    | .elem _ _ _ => 1
  /-- The number of elements in a list of served trees. -/
  def elemsAll : List Out → Nat
    | [] => 0
    | o :: os => elems o + elemsAll os
end

mutual
  theorem render_elems (v : Viewer) : ∀ (n : Node) (o : Out), render v n = some o → 0 < elems o
    | .box lab a nd ks, o, h => by
      simp only [render] at h
      by_cases ha : sees v a = true
      · rw [if_pos ha] at h
        obtain ⟨rfl, hne, _⟩ := boxOut_some h
        simp only [elems]
        exact renderAll_elems v ks hne
      · rw [if_neg ha] at h; cases h
    | .elem lab a d c cnt adm, o, h => by
      simp only [render] at h
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

/-- Every container served holds a served element.
    Verifies: DS-012 -/
theorem served_has_element (v : Viewer) (n : Node) (o : Out) (h : render v n = some o) :
    0 < elems o := render_elems v n o h

/-- A container that needs content is served only with a served element that counts.
    Verifies: DS-012 -/
theorem served_needs_content (v : Viewer) (l : Nat) (a : Aud) (ks : List Node) (o : Out)
    (h : render v (.box l a true ks) = some o) : 0 < counted o := by
  simp only [render] at h
  by_cases ha : sees v a = true
  · rw [if_pos ha] at h
    obtain ⟨rfl, _, hc⟩ := boxOut_some h
    simpa [counted] using hc rfl
  · rw [if_neg ha] at h; cases h

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

/-- A node the viewer is not served leaves no trace in the container around it, which is served, or
    pruned, exactly as if the node were not there.
    Verifies: DS-008 -/
theorem hidden_leaves_no_trace_in (v : Viewer) (l : Nat) (aud : Aud) (nd : Bool) (a b : List Node)
    (h : Node) (hh : render v h = none) :
    render v (.box l aud nd (a ++ h :: b)) = render v (.box l aud nd (a ++ b)) := by
  simp only [render, hidden_leaves_no_trace v a b h hh]

/-- A declared audience that holds only holders of named roles, none of them `public` or a role every
    member is given by default (`dflt`: the realm's default role, and any role the tenant marks as held
    by everyone). -/
def strict (dflt : List Nat) : Aud → Bool
  | .roles rs => !rs.isEmpty && rs.all (fun r => r != pub && !dflt.contains r)
  | .memberRoles rs => !rs.isEmpty && rs.all (fun r => r != pub && !dflt.contains r)
  | _ => false

/-- The viewer holds a role other than `public` and the default roles. -/
def HoldsRole (dflt : List Nat) (v : Viewer) : Prop := ∃ r, r ≠ pub ∧ r ∉ dflt ∧ r ∈ rolesOf v

theorem strict_roles (dflt rs : List Nat) (v : Viewer)
    (hs : (rs.all (fun r => r != pub && !dflt.contains r)) = true)
    (h : (rs.any (fun r => (rolesOf v).contains r)) = true) : HoldsRole dflt v := by
  simp only [List.all_eq_true, Bool.and_eq_true, bne_iff_ne, ne_eq, Bool.not_eq_true'] at hs
  simp only [List.any_eq_true, List.contains_iff_mem] at h
  obtain ⟨r, hr, hv⟩ := h
  refine ⟨r, (hs r hr).1, fun hd => ?_, hv⟩
  have hc : dflt.contains r = true := List.contains_iff_mem.mpr hd
  rw [(hs r hr).2] at hc
  cases hc

theorem strict_sees (dflt : List Nat) (a : Aud) (v : Viewer) (hs : strict dflt a = true)
    (h : sees v a = true) : HoldsRole dflt v := by
  cases a with
  | anyone => simp [strict] at hs
  | signedIn => simp [strict] at hs
  | roles rs =>
    simp only [strict, Bool.and_eq_true] at hs
    exact strict_roles dflt rs v hs.2 h
  | memberRoles rs =>
    simp only [strict, Bool.and_eq_true] at hs
    simp only [sees, Bool.and_eq_true] at h
    exact strict_roles dflt rs v hs.2 h.2

mutual
  /-- The admin check: every admin element lies under, or has, a strict declared audience. `g` says a
      strict audience is above. -/
  def adminOk (dflt : List Nat) (g : Bool) : Node → Bool
    | .box _ a _ ks => adminOkAll dflt (g || strict dflt a) ks
    | .elem _ a _ _ _ adm => !adm || g || strict dflt a
  /-- The admin check of a list of nodes. -/
  def adminOkAll (dflt : List Nat) (g : Bool) : List Node → Bool
    | [] => true
    | n :: ns => adminOk dflt g n && adminOkAll dflt g ns
end

mutual
  /-- The admin labels of a served tree. -/
  def adminLabels : Out → List Nat
    | .box _ ks => adminLabelsAll ks
    | .elem l _ adm => if adm then [l] else []
  /-- The admin labels of a list of served trees. -/
  def adminLabelsAll : List Out → List Nat
    | [] => []
    | o :: os => adminLabels o ++ adminLabelsAll os
end

mutual
  theorem admin_served (dflt : List Nat) (v : Viewer) : ∀ (g : Bool) (n : Node) (o : Out) (l : Nat),
      adminOk dflt g n = true → (g = true → HoldsRole dflt v) → render v n = some o →
      l ∈ adminLabels o → HoldsRole dflt v
    | g, .box lab a nd ks, o, l, hok, hg, h, hl => by
      simp only [render] at h
      by_cases ha : sees v a = true
      · rw [if_pos ha] at h
        obtain ⟨rfl, _, _⟩ := boxOut_some h
        simp only [adminOk] at hok
        simp only [adminLabels] at hl
        refine adminAll_served dflt v (g || strict dflt a) ks l hok ?_ hl
        intro hg'
        cases hgv : g
        · simp [hgv] at hg'
          exact strict_sees dflt a v hg' ha
        · exact hg hgv
      · rw [if_neg ha] at h; cases h
    | g, .elem lab a d c cnt adm, o, l, hok, hg, h, hl => by
      simp only [render] at h
      by_cases hv : (sees v a && sees v d) = true
      · rw [if_pos hv] at h
        cases h
        cases hadm : adm
        · simp [adminLabels, hadm] at hl
        · simp only [adminOk, hadm, Bool.not_true, Bool.false_or, Bool.or_eq_true] at hok
          rcases hok with hgt | hs
          · exact hg hgt
          · simp only [Bool.and_eq_true] at hv
            exact strict_sees dflt a v hs hv.1
      · rw [if_neg hv] at h; cases h
  theorem adminAll_served (dflt : List Nat) (v : Viewer) : ∀ (g : Bool) (ns : List Node) (l : Nat),
      adminOkAll dflt g ns = true → (g = true → HoldsRole dflt v) →
      l ∈ adminLabelsAll (renderAll v ns) → HoldsRole dflt v
    | g, [], l, _, _, h => by simp [renderAll, adminLabelsAll] at h
    | g, n :: ns, l, hok, hg, h => by
      simp only [adminOkAll, Bool.and_eq_true] at hok
      unfold renderAll at h
      cases hr : render v n with
      | none => rw [hr] at h; exact adminAll_served dflt v g ns l hok.2 hg h
      | some o =>
        rw [hr] at h
        simp only [adminLabelsAll, List.mem_append] at h
        rcases h with h | h
        · exact admin_served dflt v g n o l hok.1 hg hr h
        · exact adminAll_served dflt v g ns l hok.2 hg h
end

/-- In a composition that passes the admin check, an admin element is served only to a viewer who
    holds a role other than `public` and the default roles, named in a strict audience above or on it.
    Verifies: DS-011 -/
theorem admin_needs_role (dflt : List Nat) (v : Viewer) (n : Node) (o : Out) (l : Nat)
    (hok : adminOk dflt false n = true) (h : render v n = some o) (hl : l ∈ adminLabels o) :
    HoldsRole dflt v :=
  admin_served dflt v false n o l hok (by simp) h hl

/-- An anonymous visitor, and a member holding only the roles every member holds by default, are never
    served an admin element.
    Verifies: DS-011 -/
theorem admin_never_to_default_member (dflt : List Nat) (n : Node) (o : Out)
    (hok : adminOk dflt false n = true) :
    (render (.member dflt) n = some o → adminLabels o = []) ∧
    (render .anon n = some o → adminLabels o = []) := by
  constructor
  · intro h
    cases hl : adminLabels o with
    | nil => rfl
    | cons l ls =>
      obtain ⟨r, hr, hd, hv⟩ := admin_needs_role dflt _ n o l hok h (by simp [hl])
      simp only [rolesOf, List.mem_cons] at hv
      rcases hv with hv | hv
      · exact absurd hv hr
      · exact absurd hv hd
  · intro h
    cases hl : adminLabels o with
    | nil => rfl
    | cons l ls =>
      obtain ⟨r, hr, _, hv⟩ := admin_needs_role dflt _ n o l hok h (by simp [hl])
      simp [rolesOf] at hv
      exact absurd hv hr

/-- The first review's admin action, offered to every signed-in person but derived admin, fails the
    admin check under a box declared for every signed-in person. -/
theorem admin_action_for_members_refused :
    adminOk [] false (.box 1 .signedIn false [.elem 2 .signedIn .signedIn .signedIn true true]) = false := by
  simp [adminOk, adminOkAll, strict]

/-- The third review's case: an admin audience naming the default role (5) is not strict, so the admin
    check refuses it. -/
theorem admin_default_role_refused :
    adminOk [5] false (.box 1 (.roles [5]) false [.elem 2 .anyone .anyone .anyone true true]) = false := by
  simp [adminOk, adminOkAll, strict, pub]

/-- The first audience in order that holds the viewer, with what it selects (shells). -/
def select {α : Type} (v : Viewer) : List (Aud × α) → Option α
  | [] => none
  | (a, x) :: rest => if sees v a then some x else select v rest

/-- The shell selected has an audience that holds the viewer.
    Verifies: DS-014 -/
theorem select_sees {α : Type} (v : Viewer) :
    ∀ (vs : List (Aud × α)) (x : α), select v vs = some x → ∃ a, (a, x) ∈ vs ∧ sees v a = true
  | [], x, h => by simp [select] at h
  | (a, y) :: rest, x, h => by
    simp only [select] at h
    by_cases ha : sees v a = true
    · rw [if_pos ha] at h; cases h; exact ⟨a, by simp, ha⟩
    · rw [if_neg ha] at h
      obtain ⟨b, hb, hs⟩ := select_sees v rest x h
      exact ⟨b, List.mem_cons_of_mem _ hb, hs⟩

/-- No earlier shell's audience holds the viewer: the declared order decides, never a union.
    Verifies: DS-014 -/
theorem select_first {α : Type} (v : Viewer) (pre : List (Aud × α)) (a : Aud) (x : α)
    (rest : List (Aud × α)) (hpre : ∀ p ∈ pre, sees v p.1 = false) (ha : sees v a = true) :
    select v (pre ++ (a, x) :: rest) = some x := by
  induction pre with
  | nil => simp [select, ha]
  | cons p ps ih =>
    obtain ⟨b, y⟩ := p
    have hb : sees v b = false := hpre (b, y) (by simp)
    simp only [List.cons_append, select, hb]
    exact ih (fun q hq => hpre q (List.mem_cons_of_mem _ hq))

/-- Whether a variant serves the viewer anything: its audience holds the viewer and its screen
    renders. -/
def served (v : Viewer) (p : Aud × Node) : Option Out :=
  if sees v p.1 then render v p.2 else none

/-- Screen variants: the first in declared order that serves the viewer something. -/
def selectServed (v : Viewer) : List (Aud × Node) → Option Out
  | [] => none
  | p :: rest =>
    match served v p with
    | some o => some o
    | none => selectServed v rest

/-- What is served comes from a variant whose audience holds the viewer and whose screen renders.
    Verifies: DS-014 -/
theorem selectServed_sound (v : Viewer) :
    ∀ (vs : List (Aud × Node)) (o : Out), selectServed v vs = some o →
      ∃ p ∈ vs, sees v p.1 = true ∧ render v p.2 = some o
  | [], o, h => by simp [selectServed] at h
  | p :: rest, o, h => by
    simp only [selectServed] at h
    cases hp : served v p with
    | some o' =>
      rw [hp] at h
      cases h
      simp only [served] at hp
      by_cases ha : sees v p.1 = true
      · rw [if_pos ha] at hp; exact ⟨p, by simp, ha, hp⟩
      · rw [if_neg ha] at hp; cases hp
    | none =>
      rw [hp] at h
      obtain ⟨q, hq, hs, hr⟩ := selectServed_sound v rest o h
      exact ⟨q, List.mem_cons_of_mem _ hq, hs, hr⟩

/-- Earlier variants that serve the viewer nothing are passed over: a viewer in an earlier variant's
    audience who may read none of its content gets the next variant that serves them, not a page
    that is not there.
    Verifies: DS-014 -/
theorem selectServed_first (v : Viewer) (pre : List (Aud × Node)) (p : Aud × Node) (o : Out)
    (rest : List (Aud × Node)) (hpre : ∀ q ∈ pre, served v q = none) (hp : served v p = some o) :
    selectServed v (pre ++ p :: rest) = some o := by
  induction pre with
  | nil => simp [selectServed, hp]
  | cons q qs ih =>
    have hq : served v q = none := hpre q (by simp)
    simp only [List.cons_append, selectServed, hq]
    exact ih (fun r hr => hpre r (List.mem_cons_of_mem _ hr))

/-- The third review's case: a role-7 viewer whose first variant (for role 7) shows only an element
    readable by role 9 is served the second variant. -/
theorem variant_falls_through :
    selectServed (.member [7])
      [(.roles [7], .box 1 .anyone false [.elem 2 .anyone (.roles [9]) .anyone true false]),
       (.signedIn, .box 3 .anyone false [.elem 4 .anyone .anyone .anyone true false])] =
    some (.box 3 [.elem 4 true false]) := by
  simp [selectServed, served, render, renderAll, sees, rolesOf, pub, boxOut, Viewer.signedIn,
    countedAll, counted]


/-- Who may invoke an operation (D-6547). -/
structure Invokers where
  signedIn : Bool
  roles : List Nat

/-- Stage 1 of an invocation (`operations.rs`). -/
def invokeRefused (inv : Invokers) : Viewer → Bool
  | .anon => true
  | v@(.member _) => !inv.signedIn && !(inv.roles.any (fun r => (rolesOf v).contains r))

/-- The audience of an action, derived from its invokers. -/
def audOf (inv : Invokers) : Aud := if inv.signedIn then .signedIn else .memberRoles inv.roles

/-- Verifies: DS-009 -/
theorem audOf_sees (inv : Invokers) (v : Viewer) : sees v (audOf inv) = !invokeRefused inv v := by
  cases v with
  | anon => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused, Viewer.signedIn]
  | member ps => cases h : inv.signedIn <;> simp [audOf, h, sees, invokeRefused, Viewer.signedIn]

/-- A Read grant: a role, an object type, whether it has a condition, and whether its condition
    mentions `principal` (as `audience.rs`'s `scan` reports). -/
structure Grant where
  role : Nat
  obj : Nat
  conditional : Bool
  principal : Bool

/-- The data audience of an object type: every role with a Read grant on it. -/
def dataAud (gs : List Grant) (obj : Nat) : Aud :=
  .roles ((gs.filter (fun g => g.obj == obj)).map (·.role))

/-- The content audience: unconditional grants, and grants whose condition mentions `principal`. -/
def contentAud (gs : List Grant) (obj : Nat) : Aud :=
  .roles ((gs.filter (fun g => g.obj == obj && (!g.conditional || g.principal))).map (·.role))

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

/-- A grant to `public` puts the anonymous visitor in the data audience.
    Verifies: DS-010 -/
theorem public_grant_anon_sees (gs : List Grant) (obj : Nat) (c p : Bool)
    (h : ⟨pub, obj, c, p⟩ ∈ gs) : sees .anon (dataAud gs obj) = true :=
  (dataAud_sees gs obj .anon).2 ⟨_, h, rfl, by simp [rolesOf]⟩

/-- The content audience is inside the data audience.
    Verifies: DS-010 -/
theorem contentAud_sub (gs : List Grant) (obj : Nat) (v : Viewer)
    (h : sees v (contentAud gs obj) = true) : sees v (dataAud gs obj) = true := by
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

/-- The least viewers of an audience. -/
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
  /-- Whether a viewer reaches, through containers it sees, an element that counts and whose declared
      and content audiences hold it. -/
  def reach (v : Viewer) : Node → Bool
    | .box _ a _ ks => sees v a && reachAll v ks
    | .elem _ a _ c cnt _ => cnt && sees v a && sees v c
  /-- Whether a viewer reaches content in one of these nodes. -/
  def reachAll (v : Viewer) : List Node → Bool
    | [] => false
    | n :: ns => reach v n || reachAll v ns
end

mutual
  theorem reach_mono (m v : Viewer) (hle : Le m v) : ∀ n : Node, reach m n = true → reach v n = true
    | .box l a nd ks, h => by
      simp only [reach, Bool.and_eq_true] at h ⊢
      exact ⟨sees_mono a m v hle h.1, reachAll_mono m v hle ks h.2⟩
    | .elem l a d c cnt adm, h => by
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

/-- Content inside data, for every viewer. -/
def Sub (c d : Aud) : Prop := ∀ v, sees v c = true → sees v d = true

-- Every element's content audience lies inside its data audience.
mutual
  def WellFormed : Node → Prop
    | .box _ _ _ ks => WellFormedAll ks
    | .elem _ _ d c _ _ => Sub c d
  def WellFormedAll : List Node → Prop
    | [] => True
    | n :: ns => WellFormed n ∧ WellFormedAll ns
end

mutual
  theorem reach_served (v : Viewer) : ∀ n : Node, WellFormed n → reach v n = true →
      ∃ o, render v n = some o ∧ 0 < counted o
    | .box l a nd ks, hw, h => by
      simp only [reach, Bool.and_eq_true] at h
      obtain ⟨ha, hk⟩ := h
      have hall := reachAll_served v ks (by simpa [WellFormed] using hw) hk
      cases hr : renderAll v ks with
      | nil => rw [hr] at hall; simp [countedAll] at hall
      | cons o os =>
        rw [hr] at hall
        refine ⟨.box l (o :: os), ?_, by simpa [counted] using hall⟩
        simp only [render, if_pos ha, hr, boxOut]
        rw [if_neg (by omega)]
    | .elem l a d c cnt adm, hw, h => by
      simp only [reach, Bool.and_eq_true] at h
      obtain ⟨⟨hc, ha⟩, hcon⟩ := h
      have hd : sees v d = true := hw v hcon
      refine ⟨.elem l cnt adm, by simp [render, ha, hd], by simp [counted, hc]⟩
  theorem reachAll_served (v : Viewer) : ∀ ns : List Node, WellFormedAll ns →
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

/-- The content lint of a screen: every least viewer of its declared audience reaches content. -/
def contentCheck : Node → Bool
  | n@(.box _ a _ _) => (minimal a).all (fun m => reach m n)
  | .elem _ _ _ _ _ _ => true

/-- What the content lint establishes, for what it reasons on: a screen that passes it, served to a
    viewer in its audience, holds an element that counts in that viewer's content audience. -/
theorem checked_screen_has_content (l : Nat) (a : Aud) (nd : Bool) (ks : List Node) (v : Viewer)
    (hw : WellFormed (.box l a nd ks)) (hc : contentCheck (.box l a nd ks) = true)
    (hv : sees v a = true) : ∃ o, render v (.box l a nd ks) = some o ∧ 0 < counted o := by
  obtain ⟨m, hm, hle⟩ := minimal_le a v hv
  simp only [contentCheck, List.all_eq_true] at hc
  exact reach_served v _ hw (reach_mono m v hle _ (hc m hm))

/-- The first review's empty region: a public screen with a public region holding one admin-only
    widget renders nothing for the anonymous visitor. -/
theorem empty_region_pruned :
    render .anon (.box 1 .anyone false
      [.box 2 .anyone false [.elem 3 (.roles [7]) .anyone .anyone true false]]) = none := by
  simp [render, renderAll, sees, rolesOf, pub, boxOut]

/-- The second review's static element: a region that needs content, holding a static introduction
    and a widget the viewer may not see, is pruned rather than served with the introduction alone. -/
theorem static_does_not_keep_region :
    render (.member []) (.box 2 .signedIn true
      [.elem 3 .signedIn .anyone .anyone false false,
       .elem 4 (.roles [7]) .anyone .anyone true false]) = none := by
  simp [render, renderAll, sees, rolesOf, pub, boxOut, countedAll, counted, Viewer.signedIn]

/-! ### The class part and its per-request narrowing

The gateway builds one part per interface class from the declared audiences alone, keeping every data
audience in it and pruning nothing; per request it narrows that part by the viewer's data audiences and
prunes. These theorems say the narrowed class part is exactly what the viewer is served, and that two
viewers who agree on every declared audience of a composition share its class part. -/

/-- A class part: declared audiences applied, data audiences kept, nothing pruned. -/
inductive ClassOut where
  | box (label : Nat) (needs : Bool) (kids : List ClassOut)
  | elem (label : Nat) (data : Aud) (counts admin : Bool)

mutual
  /-- The class part of a node: what the declared audiences let through. -/
  def renderClass (v : Viewer) : Node → Option ClassOut
    | .box l a n ks => if sees v a then some (.box l n (renderClassAll v ks)) else none
    | .elem l a d _ c adm => if sees v a then some (.elem l d c adm) else none
  /-- The class parts of a list of nodes. -/
  def renderClassAll (v : Viewer) : List Node → List ClassOut
    | [] => []
    | n :: ns =>
      match renderClass v n with
      | some o => o :: renderClassAll v ns
      | none => renderClassAll v ns
end

mutual
  /-- Narrowing a class part by the viewer's data audiences, pruning as `render` does. -/
  def narrow (v : Viewer) : ClassOut → Option Out
    | .box l n ks => boxOut l n (narrowAll v ks)
    | .elem l d c adm => if sees v d then some (.elem l c adm) else none
  /-- Narrowing a list of class parts. -/
  def narrowAll (v : Viewer) : List ClassOut → List Out
    | [] => []
    | o :: os =>
      match narrow v o with
      | some x => x :: narrowAll v os
      | none => narrowAll v os
end

mutual
  /-- Narrowing the class part is exactly what the viewer is served.
      Verifies: DS-019 -/
  theorem narrow_class_eq_render (v : Viewer) : ∀ n : Node,
      (renderClass v n).bind (narrow v) = render v n
    | .box l a nd ks => by
      by_cases ha : sees v a = true
      · simp only [renderClass, render, ha, if_true, Option.bind_some, narrow]
        rw [narrowAll_class_eq_renderAll v ks]
      · simp only [renderClass, render]
        rw [if_neg ha, if_neg ha]
        rfl
    | .elem l a d c cnt adm => by
      by_cases ha : sees v a = true
      · simp only [renderClass, render, ha, if_true, Option.bind_some, narrow, Bool.true_and]
      · simp only [renderClass, render]
        rw [if_neg ha]
        have : (sees v a && sees v d) = false := by simp [Bool.not_eq_true] at ha; simp [ha]
        rw [this]
        rfl
  theorem narrowAll_class_eq_renderAll (v : Viewer) : ∀ ns : List Node,
      narrowAll v (renderClassAll v ns) = renderAll v ns
    | [] => by simp [renderClassAll, narrowAll, renderAll]
    | n :: ns => by
      have ih := narrowAll_class_eq_renderAll v ns
      have hn := narrow_class_eq_render v n
      cases hc : renderClass v n with
      | none =>
        rw [hc] at hn
        simp only [Option.bind_none] at hn
        simp only [renderClassAll, hc, renderAll, ← hn, ih]
      | some o =>
        rw [hc] at hn
        simp only [Option.bind_some] at hn
        simp only [renderClassAll, hc, narrowAll, renderAll, ← hn]
        cases narrow v o <;> simp [ih]
end

mutual
  /-- Every declared audience of a node. -/
  def declared : Node → List Aud
    | .box _ a _ ks => a :: declaredAll ks
    | .elem _ a _ _ _ _ => [a]
  /-- Every declared audience of a list of nodes. -/
  def declaredAll : List Node → List Aud
    | [] => []
    | n :: ns => declared n ++ declaredAll ns
end

mutual
  /-- Two viewers who agree on every declared audience of a node get the same class part: the class
      key need hold no more than what decides those audiences (deciding groups, signed-in status).
      Verifies: DS-020 -/
  theorem renderClass_shared (v w : Viewer) : ∀ n : Node,
      (∀ a ∈ declared n, sees v a = sees w a) → renderClass v n = renderClass w n
    | .box l a nd ks, h => by
      have ha : sees v a = sees w a := h a (by simp [declared])
      have hk : ∀ b ∈ declaredAll ks, sees v b = sees w b :=
        fun b hb => h b (by simp [declared, hb])
      simp only [renderClass, ha, renderClassAll_shared v w ks hk]
    | .elem l a d c cnt adm, h => by
      have ha : sees v a = sees w a := h a (by simp [declared])
      simp only [renderClass, ha]
  theorem renderClassAll_shared (v w : Viewer) : ∀ ns : List Node,
      (∀ a ∈ declaredAll ns, sees v a = sees w a) → renderClassAll v ns = renderClassAll w ns
    | [], _ => rfl
    | n :: ns, h => by
      have hn := renderClass_shared v w n (fun a ha => h a (by simp [declaredAll, ha]))
      have hs := renderClassAll_shared v w ns (fun a ha => h a (by simp [declaredAll, ha]))
      simp only [renderClassAll, hn, hs]
end

/-- Signed-in status decides a declared audience: an anonymous visitor and a member holding no group
    can differ on `signedIn`, so the class key must hold it. -/
theorem signed_in_decides : sees .anon .signedIn ≠ sees (.member []) .signedIn := by
  simp [sees, Viewer.signedIn]

end Strata.UI.Visibility
