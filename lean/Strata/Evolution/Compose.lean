/-
Composition of rewrites (theorem 7).

A dependent that is several versions behind catches up by the rewrites of the changes in
between. The theorem says that doing so step by step on the successive schemas and data gives
what the dependent gave at the start, and that the composed rewrite is one substitution of the
reads, so catching up at once is the same as catching up step by step.
-/

import Strata.Evolution.Lens

namespace Strata.Evolution

/-- **Composition of rewrites (theorem 7).** Let `c₁` then `c₂` be accepted changes that are not
    breaking (adaptable ones, or compatible ones whose rewrite is the identity), `c₂` accepted on
    the schema `c₁` produced. A dependent of the first schema, rewritten by `c₁` and then by
    `c₂`, is in the final schema, is valid there, and evaluates on the data migrated by `c₁` and
    then `c₂` to what it evaluated to on the original data. Rewrites compose: a dependent that
    is two versions behind catches up one step at a time and loses nothing.

    Verifies: MIG-606. -/
theorem rewrite_compose (c₁ c₂ : Change) (s : Schema) (data : Data) (hwf : WF s)
    (hacc₁ : accepted c₁ s) (hnb₁ : classify c₁ ≠ .breaking)
    (hacc₂ : accepted c₂ (apply c₁ s)) (hnb₂ : classify c₂ ≠ .breaking)
    (d : Dep) (hd : d ∈ s.deps) :
    rewrite c₂ (rewrite c₁ d) ∈ (apply c₂ (apply c₁ s)).deps ∧
      ValidDep (apply c₂ (apply c₁ s)) (rewrite c₂ (rewrite c₁ d)) ∧
      evalDep (apply c₂ (apply c₁ s)) (migrate c₂ (migrate c₁ data)) (rewrite c₂ (rewrite c₁ d)) =
        evalDep s data d := by
  obtain ⟨hm₁, hv₁, he₁⟩ := rewrite_correct_nb c₁ s data hwf.deps hacc₁.1 hnb₁ d hd
  have hwf₁ := closure c₁ s hwf hacc₁
  obtain ⟨hm₂, hv₂, he₂⟩ :=
    rewrite_correct_nb c₂ (apply c₁ s) (migrate c₁ data) hwf₁.deps hacc₂.1 hnb₂ _ hm₁
  exact ⟨hm₂, hv₂, he₂.trans he₁⟩

/-- Rewriting twice is one substitution of the reads: the composed rewrite of a reader reads
    what the composition of the two substitutions reads. -/
theorem rewriteReader_compose (c₁ c₂ : Change) (r : Reader) :
    rewriteReader c₂ (rewriteReader c₁ r) =
      { r with reads := r.reads.map (fun e => (e.subst c₁).subst c₂) } := by
  simp [rewriteReader, List.map_map, Function.comp_def]

/-! ## A chain of changes -/

/-- The schema after a list of changes, applied in order. -/
def applyChain : List Change → Schema → Schema
  | [], s => s
  | c :: cs, s => applyChain cs (apply c s)

def migrateChain : List Change → Data → Data
  | [], d => d
  | c :: cs, d => migrateChain cs (migrate c d)

def rewriteChain : List Change → Dep → Dep
  | [], d => d
  | c :: cs, d => rewriteChain cs (rewrite c d)

/-- Each change of the chain is accepted on the schema the previous ones produced and is not
    breaking. -/
def AcceptedChain : List Change → Schema → Prop
  | [], _ => True
  | c :: cs, s => accepted c s ∧ classify c ≠ .breaking ∧ AcceptedChain cs (apply c s)

/-- **Composition over a chain.** A dependent several versions behind, rewritten by each change of
    an accepted chain of non-breaking changes in turn, is in the final schema, valid there, and
    evaluates on the chain's migrated data to what it evaluated to at the start. -/
theorem rewrite_chain (cs : List Change) :
    ∀ (s : Schema) (data : Data), WF s → AcceptedChain cs s → ∀ d ∈ s.deps,
      rewriteChain cs d ∈ (applyChain cs s).deps ∧
      ValidDep (applyChain cs s) (rewriteChain cs d) ∧
      evalDep (applyChain cs s) (migrateChain cs data) (rewriteChain cs d) = evalDep s data d := by
  induction cs with
  | nil =>
    intro s data hwf _ d hd
    exact ⟨hd, hwf.deps d hd, rfl⟩
  | cons c cs ih =>
    intro s data hwf hch d hd
    obtain ⟨hacc, hnb, hrest⟩ := hch
    obtain ⟨hm, hv, he⟩ := rewrite_correct_nb c s data hwf.deps hacc.1 hnb d hd
    obtain ⟨hm', hv', he'⟩ := ih (apply c s) (migrate c data) (closure c s hwf hacc) hrest _ hm
    exact ⟨hm', hv', he'.trans he⟩

end Strata.Evolution
