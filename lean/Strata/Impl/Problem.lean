/-
The executable kernel, stage 1: the write checks on slots.

`problemI` is the spec's `problem` (`Strata/Model.lean`) with the record sets read from slots: the
slots of the type being written, for a unique value's clash and the composite clash, and the slots
of every type (`E`), for the reference targets. Every function that does not look at the other
records (the limits, the types, the value of one field, `agreeOn`, `Val.keyed`) is the spec's own,
called unchanged. Each rewrite is proved equal to the spec's version on the abstraction, so the
write checks give the spec's refusal for every input.
-/

import Strata.Impl.Tenant

namespace Strata
namespace Impl

/-- The clash of a keyed value `v` in field `f`: some other record (not the one with id `self`)
    holds it. -/
def clashAtI (a : Array (Option Record)) (self : RecordId) (f : FieldId) (v : Val) : Bool :=
  v.keyed && anyFrom a (fun q => q.1 != self && q.2.fields.get f == v) a.size

/-- A reference of type `ty` names a record that exists in the type it targets. -/
def refHasI (E : TypeId → Array (Option Record)) (ty : Ty) (id : RecordId) : Bool :=
  match ty.target? with
  | some t => hasI (E t) id
  | none => true

/-- A list element names no record, or names one that exists in the target type. -/
def Elem.resolvesI (E : TypeId → Array (Option Record)) (ty : Ty) : Elem → Bool
  | .ref id => refHasI E ty id
  | _ => true

/-- A value names only records that exist in its target type. -/
def Val.resolvesI (E : TypeId → Array (Option Record)) (ty : Ty) : Val → Bool
  | .ref id => refHasI E ty id
  | .list vs => vs.all (Elem.resolvesI E ty)
  | _ => true

/-- The spec's `valueProblem`, reading the record sets from the slots. -/
def valueProblemI (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (a : Array (Option Record)) (self : RecordId) (f : FieldId) (d : FieldDef) (v : Val) :
    Option Err :=
  if v.overLimit L then some .limitExceeded
  else if d.unique && decide (L.maxUnique < v.size) then some .limitExceeded
  else if !v.hasType d.ty then some .invalid
  else if uniq && d.unique && clashAtI a self f v then some .uniqueViolated
  else if !Val.resolvesI E d.ty v then some .danglingReference
  else none

/-- The spec's `fieldProblem`, reading the record sets from the slots. -/
def fieldProblemI (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (a : Array (Option Record)) (self : RecordId) (fs : Fields) (fd : FieldId × FieldDef) :
    Option Err :=
  if fd.2.required && (fs.valuesOf fd.1).isEmpty then some .invalid
  else (fs.valuesOf fd.1).findSome? (valueProblemI L uniq E a self fd.1 fd.2)

/-- The spec's `compositeProblem`, reading the records from the slots. -/
def compositeProblemI (uniq : Bool) (a : Array (Option Record)) (self : RecordId) (fs : Fields)
    (us : List (List FieldId)) : Option Err :=
  if uniq && us.any (fun u => anyFrom a (fun q => q.1 != self && agreeOn u fs q.2.fields) a.size)
  then some .uniqueViolated
  else none

/-- The spec's `problem`, reading the record sets from the slots. -/
def problemI (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (E : TypeId → Array (Option Record)) (a : Array (Option Record)) (self : RecordId)
    (r : Record) : Option Err :=
  if decide (L.maxRecord < r.size) then some .limitExceeded
  else if r.fields.all (fun fv => (schema.lookup fv.1).isSome) then
    match schema.findSome? (fieldProblemI L uniq E a self r.fields) with
    | some e => some e
    | none => compositeProblemI uniq a self r.fields us
  else some .invalid

/-! ## Agreement with the spec on the abstraction -/

/-- The environment of slots as the spec's environment of lists. -/
def envOf (E : TypeId → Array (Option Record)) : Env := fun T => absSlots (E T)

theorem refHas_eq (E : TypeId → Array (Option Record)) (ty : Ty) (id : RecordId) :
    refHas (envOf E) ty id = refHasI E ty id := by
  unfold refHas refHasI envOf
  cases ty.target? <;> simp [has_absSlots]

theorem elem_resolves_eq (E : TypeId → Array (Option Record)) (ty : Ty) (e : Elem) :
    Elem.resolves (envOf E) ty e = Elem.resolvesI E ty e := by
  cases e <;> simp [Elem.resolves, Elem.resolvesI, refHas_eq]

theorem val_resolves_eq (E : TypeId → Array (Option Record)) (ty : Ty) (v : Val) :
    Val.resolves (envOf E) ty v = Val.resolvesI E ty v := by
  cases v with
  | list vs =>
    have h : Elem.resolves (envOf E) ty = Elem.resolvesI E ty := funext (elem_resolves_eq E ty)
    simp [Val.resolves, Val.resolvesI, h]
  | _ => simp [Val.resolves, Val.resolvesI, refHas_eq]

theorem clashAt_eq (a : Array (Option Record)) (self : RecordId) (f : FieldId) (v : Val) :
    clashAt (absSlots a) self f v = clashAtI a self f v := by
  unfold clashAt clashAtI
  rw [any_absSlots]

theorem valueProblem_eq (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (a : Array (Option Record)) (self : RecordId) (f : FieldId) (d : FieldDef) (v : Val) :
    valueProblem L uniq (envOf E) (absSlots a) self f d v = valueProblemI L uniq E a self f d v := by
  unfold valueProblem valueProblemI
  simp only [clashAt_eq, val_resolves_eq]

theorem fieldProblem_eq (L : Limits) (uniq : Bool) (E : TypeId → Array (Option Record))
    (a : Array (Option Record)) (self : RecordId) (fs : Fields) (fd : FieldId × FieldDef) :
    fieldProblem L uniq (envOf E) (absSlots a) self fs fd = fieldProblemI L uniq E a self fs fd := by
  unfold fieldProblem fieldProblemI
  have : valueProblem L uniq (envOf E) (absSlots a) self fd.1 fd.2 =
      valueProblemI L uniq E a self fd.1 fd.2 :=
    funext (valueProblem_eq L uniq E a self fd.1 fd.2)
  rw [this]

theorem compositeProblem_eq (uniq : Bool) (a : Array (Option Record)) (self : RecordId)
    (fs : Fields) (us : List (List FieldId)) :
    compositeProblem uniq (absSlots a) self fs us = compositeProblemI uniq a self fs us := by
  unfold compositeProblem compositeProblemI
  simp only [any_absSlots]

theorem problem_eq (L : Limits) (schema : Schema) (us : List (List FieldId)) (uniq : Bool)
    (E : TypeId → Array (Option Record)) (a : Array (Option Record)) (self : RecordId)
    (r : Record) :
    problem L schema us uniq (envOf E) (absSlots a) self r = problemI L schema us uniq E a self r := by
  unfold problem problemI
  have h : fieldProblem L uniq (envOf E) (absSlots a) self r.fields =
      fieldProblemI L uniq E a self r.fields := funext (fieldProblem_eq L uniq E a self r.fields)
  rw [h, compositeProblem_eq]
  rfl

end Impl
end Strata
