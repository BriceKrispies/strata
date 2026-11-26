/-
The executable transitions (slice 9): the implementations of `Strata.transition` on both executable stages.

Each is its stage's `stepI` or `stepX`, with the addressed tenant committed (`commitI`, the revision, the event
sequence and the schema version raised), and the events of the step: the spec's derivation `stepEvents`, read
through the abstraction of the world before and after the commit. The refinement theorems carry the step
(`stepI_refines`, `stepX_refines`) to the transition: the events and the world are the spec's.
-/

import Strata.Impl.World
import Strata.Impl.XWorld

namespace Strata
namespace Impl

/-- The stage 1 world after an accepted step, with the addressed tenant committed (`commitI`). -/
def commitWorldI (w w' : IWorld) (c : Ctx) (i : Intent) : IWorld :=
  fun x => if x = c.tenant then
      match w c.tenant, w' c.tenant with
      | some t, some t' => some (commitI t t' c.principal i)
      | _, _ => w' x
    else w' x

theorem absW_commitWorldI (w w' : IWorld) (c : Ctx) (i : Intent) :
    absW (commitWorldI w w' c i) = commitWorld (absW w) (absW w') c i := by
  funext x
  unfold commitWorldI commitWorld absW
  by_cases hx : x = c.tenant
  · subst hx
    rcases hw : w c.tenant with _ | t <;> rcases hw' : w' c.tenant with _ | t' <;>
      simp [commitWorldI, commitWorld, absW, hw, hw', commitI_abs]
  · simp [hx]

/-- The stage 1 transition: the step with the tenant committed, and the events of the step. -/
def transitionI (w : IWorld) (i : Intent) (c : Ctx) : Except Err (IWorld × List DomainEvent) :=
  match stepI w i c with
  | .error e => .error e
  | .ok w' =>
    let w'' := commitWorldI w w' c i
    .ok (w'', stepEvents (absW w) (absW w'') c i)

/-- The stage 1 transition is the spec's transition, on the abstraction: the same world and the same events. -/
theorem transitionI_refines (w : IWorld) (i : Intent) (c : Ctx) (hw : w.Inv) :
    (transitionI w i c).map (fun p => (absW p.1, p.2)) =
      (transition (absW w) i c).map (fun tr => (tr.world, tr.events)) := by
  have h := stepI_refines w i c hw
  unfold transitionI transition
  cases hs : stepI w i c with
  | error e =>
    have hsp : step (absW w) i c = .error e := by rw [← h, hs]; rfl
    simp [hsp, Except.map]
  | ok w' =>
    have hsp : step (absW w) i c = .ok (absW w') := by rw [← h, hs]; rfl
    simp only [hsp, Except.map, absW_commitWorldI]

/-- The stage 2 world after an accepted step, with the addressed tenant committed: the commit is on the core
    of the indexed tenant, which the indexes do not depend on. -/
def commitX (t t' : XTenant) (p : Principal) (i : Intent) : XTenant :=
  { t' with core := commitI t.core t'.core p i }

def commitWorldX (w w' : XWorld) (c : Ctx) (i : Intent) : XWorld :=
  fun x => if x = c.tenant then
      match w c.tenant, w' c.tenant with
      | some t, some t' => some (commitX t t' c.principal i)
      | _, _ => w' x
    else w' x

theorem absX_commitWorldX (w w' : XWorld) (c : Ctx) (i : Intent) :
    absX (commitWorldX w w' c i) = commitWorld (absX w) (absX w') c i := by
  funext x
  unfold commitWorldX commitWorld absX
  by_cases hx : x = c.tenant
  · subst hx
    rcases hw : w c.tenant with _ | t <;> rcases hw' : w' c.tenant with _ | t' <;>
      simp [commitWorldX, commitWorld, absX, hw, hw', commitX, XTenant.abs, commitI_abs]
  · simp [hx]

/-- The stage 2 transition: the step with the tenant committed, and the events of the step. -/
def transitionX (w : XWorld) (i : Intent) (c : Ctx) : Except Err (XWorld × List DomainEvent) :=
  match stepX w i c with
  | .error e => .error e
  | .ok w' =>
    let w'' := commitWorldX w w' c i
    .ok (w'', stepEvents (absX w) (absX w'') c i)

/-- The stage 2 transition is the spec's transition, on the abstraction. -/
theorem transitionX_refines (w : XWorld) (i : Intent) (c : Ctx) (hw : w.Ok) :
    (transitionX w i c).map (fun p => (absX p.1, p.2)) =
      (transition (absX w) i c).map (fun tr => (tr.world, tr.events)) := by
  have h := stepX_refines w i c hw
  unfold transitionX transition
  cases hs : stepX w i c with
  | error e =>
    have hsp : step (absX w) i c = .error e := by rw [← h, hs]; rfl
    simp [hsp, Except.map]
  | ok w' =>
    have hsp : step (absX w) i c = .ok (absX w') := by rw [← h, hs]; rfl
    simp only [hsp, Except.map, absX_commitWorldX]

end Impl
end Strata
