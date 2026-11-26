/-
The executable segment intents (slice 16, WG-5672): `segTransition` on both executable stages. The spec decides the
definition, the removal or the rebuild on the abstraction of the stored tenant; the stored tenant takes the segments, the
memberships and the log of the spec's tenant, and nothing else changes (the records, the counters and the automations are
those the store holds). The refinement theorems carry the outcome to `segTransition`.
-/

import Strata.Impl.Transition
import Strata.SegmentsStep

namespace Strata
namespace Impl

/-- The stored tenant after a segment intent: the stored tenant with the segment rows of the spec's tenant. -/
def ITenant.withSeg (t : ITenant) (tf : Tenant) : ITenant :=
  { t with segments := tf.segments, memberships := tf.memberships, segLog := tf.segLog }

/-- The stored tenant with the segment rows of `tf` reads, through the abstraction, as `tf` when `tf` differs from the
    stored tenant only in those rows. -/
theorem ITenant.withSeg_abs (t : ITenant) (tf : Tenant)
    (h : ∃ s m l, tf = { t.abs with segments := s, memberships := m, segLog := l }) :
    (ITenant.withSeg t tf).abs = tf := by
  obtain ⟨s, m, l, rfl⟩ := h
  rfl

/-- The spec's segment intent, on a tenant that exists, leaves it with its segments, memberships and log replaced and
    nothing else changed (slice 16). -/
theorem segTransition_tenant {w : World} {i : SegIntent} {c : Ctx} {r : SegTransition} {t : Tenant}
    (h : segTransition w i c = .ok r) (hw : w c.tenant = some t) :
    ∃ s m l, r.world c.tenant = some { t with segments := s, memberships := m, segLog := l } := by
  unfold segTransition at h
  simp only [hw] at h
  cases i with
  | define id name subject pf predicate refresh =>
    dsimp only at h
    cases hd : segDefine t c.principal id name subject pf predicate refresh with
    | error e => simp [hd, Except.map] at h
    | ok t' =>
      obtain ⟨s, hs⟩ := segDefine_shape hd
      simp [hd, Except.map] at h
      subst h
      exact ⟨s, t.memberships, t.segLog, by simp [hs]⟩
  | remove id =>
    dsimp only at h
    cases hd : segRemove t c.principal id with
    | error e => simp [hd, Except.map] at h
    | ok t' =>
      obtain ⟨s, m, hs⟩ := segRemove_shape hd
      simp [hd, Except.map] at h
      subst h
      exact ⟨s, m, t.segLog, by simp [hs]⟩
  | rebuild id =>
    dsimp only at h
    rcases hr : segRebuildPass t c.principal id c.hop with ⟨ms, new⟩
    simp only [hr] at h
    cases h
    exact ⟨t.segments, ms, t.segLog ++ new, by simp [segCommit]⟩

/-- A segment intent leaves every other tenant as it was (slice 16). -/
theorem segTransition_other {w : World} {i : SegIntent} {c : Ctx} {r : SegTransition}
    (h : segTransition w i c = .ok r) (x : TenantId) (hx : x ≠ c.tenant) : r.world x = w x := by
  unfold segTransition at h
  rcases hw : w c.tenant with _ | t
  · rw [hw] at h
    cases h
  · rw [hw] at h
    dsimp only at h
    cases i with
    | define id name subject pf predicate refresh =>
      dsimp only at h
      cases hd : segDefine t c.principal id name subject pf predicate refresh with
      | error e => simp [hd, Except.map] at h
      | ok t' =>
        simp [hd, Except.map] at h
        subst h
        simp [hx]
    | remove id =>
      dsimp only at h
      cases hd : segRemove t c.principal id with
      | error e => simp [hd, Except.map] at h
      | ok t' =>
        simp [hd, Except.map] at h
        subst h
        simp [hx]
    | rebuild id =>
      dsimp only at h
      rcases hr : segRebuildPass t c.principal id c.hop with ⟨ms, new⟩
      simp only [hr] at h
      cases h
      simp [hx]

/-- The stage 1 segment intent: the spec's `segTransition` on the abstraction, and the stored tenant with the spec's
    segment rows. A tenant that does not exist is `noTenant`, as the spec says. -/
def transitionSegI (w : IWorld) (i : SegIntent) (c : Ctx) :
    Except Err (IWorld × List (Nat × SegEvent) × List Effect) :=
  match segTransition (absW w) i c with
  | .error e => .error e
  | .ok r =>
    match w c.tenant with
    | none => .error .noTenant
    | some t => .ok (fun x => if x = c.tenant then (r.world c.tenant).map (fun tf => ITenant.withSeg t tf) else w x,
        r.segEvents, r.effects)

/-- The stage 1 segment intent is the spec's, on the abstraction. -/
theorem transitionSegI_refines (w : IWorld) (i : SegIntent) (c : Ctx) :
    (transitionSegI w i c).map (fun p => (absW p.1, p.2.1, p.2.2)) =
      (segTransition (absW w) i c).map (fun r => (r.world, r.segEvents, r.effects)) := by
  unfold transitionSegI
  cases hs : segTransition (absW w) i c with
  | error e => rfl
  | ok r =>
    dsimp only
    cases hw : w c.tenant with
    | none =>
      have e1 : absW w c.tenant = none := by simp [absW, hw]
      unfold segTransition at hs
      simp only [e1] at hs
      cases hs
    | some t =>
      dsimp only
      have e1 : absW w c.tenant = some t.abs := by simp [absW, hw]
      obtain ⟨s, m, l, hr⟩ := segTransition_tenant hs e1
      have hcom : (ITenant.withSeg t { t.abs with segments := s, memberships := m, segLog := l }).abs =
          { t.abs with segments := s, memberships := m, segLog := l } :=
        ITenant.withSeg_abs t _ ⟨s, m, l, rfl⟩
      have hW : absW (fun x => if x = c.tenant then (r.world c.tenant).map (fun tf => ITenant.withSeg t tf)
          else w x) = r.world := by
        funext x
        by_cases hx : x = c.tenant
        · subst hx
          rw [hr]
          show ((if c.tenant = c.tenant then (some { t.abs with segments := s, memberships := m, segLog := l }).map
              (fun tf => ITenant.withSeg t tf) else w c.tenant).map ITenant.abs) = _
          rw [if_pos rfl]
          simp only [Option.map_some]
          rw [hcom]
        · rw [segTransition_other hs x hx]
          simp [absW, hx]
      simp only [Except.map, hW]

/-! ## The indexed stage -/

/-- The indexed tenant after a segment intent: its core with the segment rows of the spec's tenant. -/
def XTenant.withSeg (t : XTenant) (tf : Tenant) : XTenant :=
  { t with core := ITenant.withSeg t.core tf }

/-- The stage 2 segment intent: the spec's `segTransition` on the abstraction, and the stored tenant with the spec's segment
    rows. -/
def transitionSegX (w : XWorld) (i : SegIntent) (c : Ctx) :
    Except Err (XWorld × List (Nat × SegEvent) × List Effect) :=
  match segTransition (absX w) i c with
  | .error e => .error e
  | .ok r =>
    match w c.tenant with
    | none => .error .noTenant
    | some t => .ok (fun x => if x = c.tenant then (r.world c.tenant).map (fun tf => XTenant.withSeg t tf) else w x,
        r.segEvents, r.effects)

/-- The stage 2 segment intent is the spec's, on the abstraction. -/
theorem transitionSegX_refines (w : XWorld) (i : SegIntent) (c : Ctx) :
    (transitionSegX w i c).map (fun p => (absX p.1, p.2.1, p.2.2)) =
      (segTransition (absX w) i c).map (fun r => (r.world, r.segEvents, r.effects)) := by
  unfold transitionSegX
  cases hs : segTransition (absX w) i c with
  | error e => rfl
  | ok r =>
    dsimp only
    cases hw : w c.tenant with
    | none =>
      have e1 : absX w c.tenant = none := by simp [absX, hw]
      unfold segTransition at hs
      simp only [e1] at hs
      cases hs
    | some t =>
      dsimp only
      have e1 : absX w c.tenant = some t.abs := by simp [absX, hw]
      obtain ⟨s, m, l, hr⟩ := segTransition_tenant hs e1
      have hcom : (XTenant.withSeg t { t.abs with segments := s, memberships := m, segLog := l }).abs =
          { t.abs with segments := s, memberships := m, segLog := l } :=
        ITenant.withSeg_abs t.core _ ⟨s, m, l, rfl⟩
      have hW : absX (fun x => if x = c.tenant then (r.world c.tenant).map (fun tf => XTenant.withSeg t tf)
          else w x) = r.world := by
        funext x
        by_cases hx : x = c.tenant
        · subst hx
          rw [hr]
          show ((if c.tenant = c.tenant then (some { t.abs with segments := s, memberships := m, segLog := l }).map
              (fun tf => XTenant.withSeg t tf) else w c.tenant).map XTenant.abs) = _
          rw [if_pos rfl]
          simp only [Option.map_some]
          rw [hcom]
        · rw [segTransition_other hs x hx]
          simp [absX, hx]
      simp only [Except.map, hW]

end Impl
end Strata
