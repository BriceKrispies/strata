/-
The executable kernel, stage 1: the write checks on slots.

`problemI` is the spec's `problem` (`Strata/Model.lean`) with the record set read from the slot
array. Every function that does not look at the other records (the limits, the types, the value
of one field, `agreeOn`, `Val.keyed`) is the spec's own, called unchanged. Only the four places
that read the record set are rewritten: a unique value's clash (`clashAtI`), a reference's target
(`hasI`, through `Val.resolvesI`), and the composite clash (inside `compositeProblemI`). Each
rewrite is proved equal to the spec's version on the abstraction (`absSlots`), so the write checks
give the spec's refusal for every input.
-/

import Strata.Impl.Store

namespace Strata
namespace Impl

/-- The clash of a keyed value `v` in field `f`: some other record (not the one with id `self`)
    holds it. -/
def clashAtI (a : Array (Option Record)) (self : RecordId) (f : FieldId) (v : Val) : Bool :=
  v.keyed && anyFrom a (fun q => q.1 != self && q.2.fields.get f == v) a.size

/-- A list element names no record, or names one that exists. -/
def Elem.resolvesI (a : Array (Option Record)) : Elem → Bool
  | .ref id => hasI a id
  | _ => true

/-- A value names only records that exist. -/
def Val.resolvesI (a : Array (Option Record)) : Val → Bool
  | .ref id => hasI a id
  | .list vs => vs.all (Elem.resolvesI a)
  | _ => true

/-- The spec's `valueProblem`, reading the record set from the slots. -/
def valueProblemI (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (f : FieldId) (d : FieldDef) (v : Val) : Option Err :=
  if v.overLimit L then some .limitExceeded
  else if d.unique && decide (L.maxUnique < v.size) then some .limitExceeded
  else if !v.hasType d.ty then some .invalid
  else if uniq && d.unique && clashAtI a self f v then some .uniqueViolated
  else if !Val.resolvesI a v then some .danglingReference
  else none

/-- The spec's `fieldProblem`, reading the record set from the slots. -/
def fieldProblemI (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (fs : Fields) (fd : FieldId × FieldDef) : Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblemI L uniq a self fd.1 fd.2)

/-- The spec's `compositeProblem`, reading the record set from the slots. -/
def compositeProblemI (uniq : Bool) (a : Array (Option Record)) (self : RecordId) (fs : Fields)
    (us : List (List FieldId)) : Option Err :=
  if uniq && us.any (fun u => anyFrom a (fun q => q.1 != self && agreeOn u fs q.2.fields) a.size)
  then some .uniqueViolated
  else none

/-- The spec's `problem`, reading the record set from the slots. -/
def problemI (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (a : Array (Option Record)) (self : RecordId) (r : Record) : Option Err :=
  if decide (L.maxRecord < r.size) then some .limitExceeded
  else if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    match schema.findSome? (fieldProblemI L uniq a self r.fields) with
    | some e => some e
    | none => compositeProblemI uniq a self r.fields us
  else some .invalid

/-! ## Agreement with the spec on the abstraction -/

theorem elem_resolves_eq (a : Array (Option Record)) :
    Elem.resolves (absSlots a) = Elem.resolvesI a := by
  funext e
  cases e <;> simp [Elem.resolves, Elem.resolvesI, has_absSlots]

theorem val_resolves_eq (a : Array (Option Record)) (v : Val) :
    Val.resolves (absSlots a) v = Val.resolvesI a v := by
  cases v <;> simp [Val.resolves, Val.resolvesI, elem_resolves_eq, has_absSlots]

theorem clashAt_eq (a : Array (Option Record)) (self : RecordId) (f : FieldId) (v : Val) :
    clashAt (absSlots a) self f v = clashAtI a self f v := by
  unfold clashAt clashAtI
  rw [any_absSlots]

theorem valueProblem_eq (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (f : FieldId) (d : FieldDef) (v : Val) :
    valueProblem L uniq (absSlots a) self f d v = valueProblemI L uniq a self f d v := by
  unfold valueProblem valueProblemI
  simp only [clashAt_eq, val_resolves_eq]

theorem valueProblem_eq' (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (f : FieldId) (d : FieldDef) :
    valueProblem L uniq (absSlots a) self f d = valueProblemI L uniq a self f d :=
  funext (valueProblem_eq L uniq a self f d)

theorem fieldProblem_eq (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (fs : Fields) (fd : FieldId × FieldDef) :
    fieldProblem L uniq (absSlots a) self fs fd = fieldProblemI L uniq a self fs fd := by
  unfold fieldProblem fieldProblemI
  simp only [valueProblem_eq']

theorem fieldProblem_eq' (L : Limits) (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (fs : Fields) :
    fieldProblem L uniq (absSlots a) self fs = fieldProblemI L uniq a self fs :=
  funext (fieldProblem_eq L uniq a self fs)

theorem compositeProblem_eq (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (fs : Fields) (us : List (List FieldId)) :
    compositeProblem uniq (absSlots a) self fs us = compositeProblemI uniq a self fs us := by
  unfold compositeProblem compositeProblemI
  simp only [any_absSlots]

theorem problem_eq (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (a : Array (Option Record)) (self : RecordId) (r : Record) :
    problem L schema us uniq (absSlots a) self r = problemI L schema us uniq a self r := by
  unfold problem problemI
  simp only [fieldProblem_eq', compositeProblem_eq]
  try rfl

end Impl
end Strata
