/-
The executable transitions (slices 9, 15 and 16): the implementations of `Strata.transition` on both executable stages.

Each is its stage's `stepI` or `stepX` (the decision of the intent on the stored tenant), and then the spec's `afterStep`
on the abstraction of the decided tenant, and then the spec's segment pass (`segAfterRecords`): the events of the step, the
automation suspensions, the trigger effects and the segment log are the spec's, and the stored tenant takes the counters,
the schema version, the automations and the segment rows the spec gives it.
The refinement theorems carry the step to the transition: the world, the events and the effects are the spec's.
-/

import Strata.Impl.World
import Strata.Impl.XWorld

namespace Strata
namespace Impl

/-- The spec's tenant after the decision (`afterStep`) and the segment pass, as the stored tenant of stage 1: the decided
    tenant with the counters, the schema version, the automations and the segment rows that the spec's tenant holds. -/
def ITenant.committed (t' : ITenant) (tf : Tenant) : ITenant :=
  { t' with revision := tf.revision, eventSeq := tf.eventSeq, schemaVersion := tf.schemaVersion,
            automations := tf.automations, segments := tf.segments, memberships := tf.memberships,
            segLog := tf.segLog }

/-- The spec's `afterStep` keeps every field of the decided tenant except its automations, its counters and its schema
    version, which the suspensions and the commit may change. -/
theorem suspendLost_shape (t : Tenant) : ∃ a, (suspendLost t).1 = { t with automations := a } := by
  unfold suspendLost
  dsimp only
  by_cases h : (lostAuthority t).isEmpty = true
  · simp only [h, if_true]
    exact ⟨t.automations, rfl⟩
  · simp only [h, if_false, Bool.false_eq_true]
    exact ⟨_, rfl⟩

theorem suspend_shape (t' : Tenant) (b : Bool) :
    ∃ a, (if b then suspendLost t' else (t', [])).1 = { t' with automations := a } := by
  cases b
  · exact ⟨t'.automations, rfl⟩
  · exact suspendLost_shape t'

/-- The spec's `afterStep` keeps every field of the decided tenant except its automations, its counters and its schema
    version, which the suspensions and the commit may change. -/
theorem afterStep_shape (t t' : Tenant) (p : Principal) (i : Intent) (hop : Nat) :
    ∃ a rv es sv, (afterStep t t' p i hop).1 =
      { t' with automations := a, revision := rv, eventSeq := es, schemaVersion := sv } := by
  obtain ⟨a, ha⟩ := suspend_shape t' i.changesAuthority
  refine ⟨a, (afterStep t t' p i hop).1.revision, (afterStep t t' p i hop).1.eventSeq,
    (afterStep t t' p i hop).1.schemaVersion, ?_⟩
  unfold afterStep
  simp only [ha]
  rfl

/-- The stored tenant of stage 1 read through the abstraction is the spec's tenant after the decision and the segment pass. -/
theorem ITenant.committed_abs (t' : ITenant) (tf : Tenant)
    (h : ∃ a rv es sv ms l, tf = { t'.abs with automations := a, revision := rv, eventSeq := es, schemaVersion := sv, memberships := ms, segLog := l }) :
    (ITenant.committed t' tf).abs = tf := by
  obtain ⟨a, rv, es, sv, ms, l, rfl⟩ := h
  rfl

/-- The accepted step of the spec, on the abstraction of the decided tenant, has the counters, the automations and the
    segment rows its shape gives (slices 15 and 16). -/
theorem acceptedT_shape (t t' : Tenant) (p : Principal) (i : Intent) (hop : Nat) :
    ∃ a rv es sv ms l, (acceptedT t t' p i hop).1 =
      { t' with automations := a, revision := rv, eventSeq := es, schemaVersion := sv, memberships := ms, segLog := l } := by
  obtain ⟨a, rv, es, sv, hsh⟩ := afterStep_shape t t' p i hop
  obtain ⟨ms, l, hseg⟩ := segAfterRecords_shape (afterStep t t' p i hop).1 (afterStep t t' p i hop).2.1 p hop
  refine ⟨a, rv, es, sv, ms, l, ?_⟩
  dsimp only [acceptedT]
  rw [hseg, hsh]

/-- The stage 1 transition: the decision of the intent, then the spec's accepted step (`acceptedT`: `afterStep` and the
    segment pass) on the abstraction of the decided tenant. A creation of a tenant has no automations and no segments to
    decide on and emits its events only. -/
def transitionI (w : IWorld) (i : Intent) (c : Ctx) :
    Except Err (IWorld × List DomainEvent × List Effect) :=
  match stepI w i c with
  | .error e => .error e
  | .ok w' =>
    match w c.tenant, w' c.tenant with
    | some t, some t' =>
      let r := acceptedT t.abs t'.abs c.principal i c.hop
      .ok (fun x => if x = c.tenant then some (ITenant.committed t' r.1) else w' x, r.2.1, r.2.2)
    | _, _ =>
      let evs := stepEvents (absW w) (absW w') c i
      .ok (w', evs, evs.map (fun e => .publish e.seq))

/-- The stage 1 transition is the spec's transition, on the abstraction: the same world, events and effects. -/
theorem transitionI_refines (w : IWorld) (i : Intent) (c : Ctx) (hw : w.Inv) :
    (transitionI w i c).map (fun p => (absW p.1, p.2.1, p.2.2)) =
      (transition (absW w) i c).map (fun tr => (tr.world, tr.events, tr.effects)) := by
  have h := stepI_refines w i c hw
  unfold transitionI transition
  cases hs : stepI w i c with
  | error e =>
    have hsp : step (absW w) i c = .error e := by rw [← h, hs]; rfl
    simp [hsp, Except.map]
  | ok w' =>
    have hsp : step (absW w) i c = .ok (absW w') := by rw [← h, hs]; rfl
    simp only [hsp, Except.map]
    rcases hw1 : w c.tenant with _ | t <;> rcases hw2 : w' c.tenant with _ | t'
    · have e1 : absW w c.tenant = none := by simp [absW, hw1]
      have e2 : absW w' c.tenant = none := by simp [absW, hw2]
      have hcw : commitWorld (absW w) (absW w') c i = absW w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · have e1 : absW w c.tenant = none := by simp [absW, hw1]
      have e2 : absW w' c.tenant = some (ITenant.abs t') := by simp [absW, hw2]
      have hcw : commitWorld (absW w) (absW w') c i = absW w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · have e1 : absW w c.tenant = some (ITenant.abs t) := by simp [absW, hw1]
      have e2 : absW w' c.tenant = none := by simp [absW, hw2]
      have hcw : commitWorld (absW w) (absW w') c i = absW w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · simp only [absW, hw1, hw2, Option.map_some, Except.map]
      obtain ⟨a, rv, es, sv, ms, l, hsh⟩ := acceptedT_shape t.abs t'.abs c.principal i c.hop
      have hcom : (ITenant.committed t' (acceptedT t.abs t'.abs c.principal i c.hop).1).abs =
          (acceptedT t.abs t'.abs c.principal i c.hop).1 :=
        ITenant.committed_abs t' _ ⟨a, rv, es, sv, ms, l, hsh⟩
      have hW : absW (fun x => if x = c.tenant then
            some (ITenant.committed t' (acceptedT t.abs t'.abs c.principal i c.hop).1) else w' x) =
          fun x => if x = c.tenant then some (acceptedT t.abs t'.abs c.principal i c.hop).1 else absW w' x := by
        funext x
        by_cases hx : x = c.tenant
        · subst hx; simp [absW, hcom]
        · simp [absW, hx]
      simp [hW, absW]

/-- The stage 2 transition: the same, with the indexed tenant's core committed. -/
def XTenant.committed (t' : XTenant) (tf : Tenant) : XTenant :=
  { t' with core := ITenant.committed t'.core tf }

def transitionX (w : XWorld) (i : Intent) (c : Ctx) :
    Except Err (XWorld × List DomainEvent × List Effect) :=
  match stepX w i c with
  | .error e => .error e
  | .ok w' =>
    match w c.tenant, w' c.tenant with
    | some t, some t' =>
      let r := acceptedT t.abs t'.abs c.principal i c.hop
      .ok (fun x => if x = c.tenant then some (XTenant.committed t' r.1) else w' x, r.2.1, r.2.2)
    | _, _ =>
      let evs := stepEvents (absX w) (absX w') c i
      .ok (w', evs, evs.map (fun e => .publish e.seq))

/-- The stage 2 transition is the spec's transition, on the abstraction. -/
theorem transitionX_refines (w : XWorld) (i : Intent) (c : Ctx) (hw : w.Ok) :
    (transitionX w i c).map (fun p => (absX p.1, p.2.1, p.2.2)) =
      (transition (absX w) i c).map (fun tr => (tr.world, tr.events, tr.effects)) := by
  have h := stepX_refines w i c hw
  unfold transitionX transition
  cases hs : stepX w i c with
  | error e =>
    have hsp : step (absX w) i c = .error e := by rw [← h, hs]; rfl
    simp [hsp, Except.map]
  | ok w' =>
    have hsp : step (absX w) i c = .ok (absX w') := by rw [← h, hs]; rfl
    simp only [hsp, Except.map]
    rcases hw1 : w c.tenant with _ | t <;> rcases hw2 : w' c.tenant with _ | t'
    · have e1 : absX w c.tenant = none := by simp [absX, hw1]
      have e2 : absX w' c.tenant = none := by simp [absX, hw2]
      have hcw : commitWorld (absX w) (absX w') c i = absX w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · have e1 : absX w c.tenant = none := by simp [absX, hw1]
      have e2 : absX w' c.tenant = some (XTenant.abs t') := by simp [absX, hw2]
      have hcw : commitWorld (absX w) (absX w') c i = absX w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · have e1 : absX w c.tenant = some (XTenant.abs t) := by simp [absX, hw1]
      have e2 : absX w' c.tenant = none := by simp [absX, hw2]
      have hcw : commitWorld (absX w) (absX w') c i = absX w' := by
        funext x
        by_cases hx : x = c.tenant <;> simp [commitWorld, hx, e1, e2]
      simp [e1, e2, hcw]
    · simp only [absX, hw1, hw2, Option.map_some, Except.map]
      obtain ⟨a, rv, es, sv, ms, l, hsh⟩ := acceptedT_shape t.abs t'.abs c.principal i c.hop
      have hcom : (XTenant.committed t' (acceptedT t.abs t'.abs c.principal i c.hop).1).abs =
          (acceptedT t.abs t'.abs c.principal i c.hop).1 :=
        ITenant.committed_abs t'.core _ ⟨a, rv, es, sv, ms, l, hsh⟩
      have hW : absX (fun x => if x = c.tenant then
            some (XTenant.committed t' (acceptedT t.abs t'.abs c.principal i c.hop).1) else w' x) =
          fun x => if x = c.tenant then some (acceptedT t.abs t'.abs c.principal i c.hop).1 else absX w' x := by
        funext x
        by_cases hx : x = c.tenant
        · subst hx; simp [absX, hcom]
        · simp [absX, hx]
      simp [hW, absX]

end Impl
end Strata
