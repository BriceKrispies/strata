/-
The transition of the segment intents (slice 16, WG-5672): the kernel's segment decisions as a step against the world, with
the context of the model (`Ctx`), so that it lives after `Strata.Model`. The decisions it runs are in `Strata/Segments.lean`.
-/

import Strata.Model
import Strata.Segments

namespace Strata

/-- The outcome of an accepted segment intent: the world after it, the entry and exit events it appends to the log (each
    with its position in the log), and the effects of the triggers on them. -/
structure SegTransition where
  world : World
  segEvents : List (Nat × SegEvent)
  effects : List Effect

/-- An accepted definition of a segment is the tenant with its segments replaced, and nothing else. -/
theorem segDefine_shape {t t' : Tenant} {p : Principal} {id : SegmentId} {name : String} {subject : TypeId}
    {pf : FieldId} {predicate : Expr} {refresh : Option Nat}
    (h : segDefine t p id name subject pf predicate refresh = .ok t') :
    ∃ s, t' = { t with segments := s } := by
  unfold segDefine at h
  cases hd : segDefineSegs t p id name subject pf predicate refresh with
  | error e => rw [hd] at h; cases h
  | ok s => rw [hd] at h; exact ⟨s, (Except.ok.inj h).symm⟩

/-- An accepted removal of a segment is the tenant with its segments and memberships replaced, and nothing else. -/
theorem segRemove_shape {t t' : Tenant} {p : Principal} {id : SegmentId} (h : segRemove t p id = .ok t') :
    ∃ s m, t' = { t with segments := s, memberships := m } := by
  unfold segRemove at h
  split at h
  · cases h
  · split at h
    · cases h
    · exact ⟨_, _, (Except.ok.inj h).symm⟩

/-- The commit of decided memberships and events keeps every field of the tenant but its memberships and its log. -/
theorem segCommit_shape (t : Tenant) (ms : List Membership) (new : List SegEvent) :
    (segCommit t ms new).1 = { t with memberships := ms, segLog := t.segLog ++ new } := rfl

/-- The transition of a segment intent against the world; the tenant must exist. A definition and a removal write no
    event; a rebuild writes the entries and exits of the persons it decides. -/
def segTransition (w : World) (i : SegIntent) (c : Ctx) : Except Err SegTransition :=
  match w c.tenant with
  | none => .error .noTenant
  | some t =>
    let done (t' : Tenant) (new : List (Nat × SegEvent)) (effs : List Effect) : SegTransition :=
      { world := fun x => if x = c.tenant then some t' else w x, segEvents := new, effects := effs }
    match i with
    | .define id name subject pf predicate refresh =>
      (segDefine t c.principal id name subject pf predicate refresh).map (fun t' => done t' [] [])
    | .remove id =>
      (segRemove t c.principal id).map (fun t' => done t' [] [])
    | .rebuild id =>
      let (ms, new) := segRebuildPass t c.principal id c.hop
      let (t', effs) := segCommit t ms new
      .ok (done t' (numberFrom t.segLog.length new) effs)

end Strata
