/-
The canonical byte codec, part 2: the boundary types of the kernel.

Each type of `Strata.Model` that crosses the boundary gets an encoder and a decoder built from the
combinators of `Strata.Codec.Core`, and a theorem that the pair is `Lawful`. The boundary entry
points (`encodeBytes`, `decodeBytes`) move between `ByteArray` and the list form, and decode only
when the whole input is consumed, so `decode (encode x) = some x` and `decode b = some x` implies
`encode x = b` for every type here (`decodeBytes_encodeBytes`, `encodeBytes_of_decodeBytes`).

The format of each type, byte by byte, is in `docs/formal-verification.md` ("Boundary codec").
`World` is not encoded: it is a function from tenant ids to tenants. The boundary carries one
tenant (`Tenant`), the acting context (`Ctx`), the intent (`Intent`) and the step's result
(`Except Err Tenant`).

Proof shape for every dispatching decoder (one that reads a tag byte and then a payload): the
tag is case-split with `omega` into its constructor values, each case unfolds the decoder with
that value, the payload is read by its own `Lawful` canonicity lemma, and the remaining case (a
tag no constructor has) has no decoding.
-/
import Strata.Model
import Strata.Codec.Core

namespace Strata.Codec

open Strata

/-! ## Enumerations -/

def encOnDelete : OnDelete → Bytes
  | .restrict => [tag 0]
  | .setNull => [tag 1]

def decOnDelete : Dec OnDelete := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.restrict, r)
    | 1 => some (.setNull, r)
    | _ => none

theorem lawful_onDelete : Lawful encOnDelete decOnDelete := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encOnDelete, decOnDelete, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decOnDelete] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ 2 ≤ b.toNat by omega) with hk | hk | hk
      · simp only [decOnDelete, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encOnDelete, byte_eq_tag b 0 hk]
      · simp only [decOnDelete, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encOnDelete, byte_eq_tag b 1 hk]
      · simp only [decOnDelete] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- The kernel's errors: twelve tags, no payload. `unknownObject` is the last, tag `11`. -/
def encErr : Err → Bytes
  | .noTenant => [tag 0]
  | .unauthorized => [tag 1]
  | .notFound => [tag 2]
  | .invalid => [tag 3]
  | .migrationRejected => [tag 4]
  | .danglingReference => [tag 5]
  | .stillReferenced => [tag 6]
  | .versionConflict => [tag 7]
  | .limitExceeded => [tag 8]
  | .uniqueViolated => [tag 9]
  | .invalidSchema => [tag 10]
  | .unknownObject => [tag 11]

def decErr : Dec Err := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.noTenant, r)
    | 1 => some (.unauthorized, r)
    | 2 => some (.notFound, r)
    | 3 => some (.invalid, r)
    | 4 => some (.migrationRejected, r)
    | 5 => some (.danglingReference, r)
    | 6 => some (.stillReferenced, r)
    | 7 => some (.versionConflict, r)
    | 8 => some (.limitExceeded, r)
    | 9 => some (.uniqueViolated, r)
    | 10 => some (.invalidSchema, r)
    | 11 => some (.unknownObject, r)
    | _ => none

theorem lawful_err : Lawful encErr decErr := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encErr, decErr, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decErr] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ b.toNat = 6 ∨ b.toNat = 7 ∨ b.toNat = 8 ∨ b.toNat = 9 ∨
          b.toNat = 10 ∨ b.toNat = 11 ∨ 12 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 0 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 1 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 2 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 3 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 4 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 5 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 6 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 7 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 8 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 9 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 10 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 11 hk]
      · simp only [decErr] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-! ## Types -/

/-- A type: tags `0`..`2` for the scalars; `3` for a reference that restricts its delete and `4`
    for one that clears, each followed by the id of the object type it names; `5` for a list and
    its element type. -/
def encTy : Ty → Bytes
  | .int => [tag 0]
  | .str => [tag 1]
  | .bool => [tag 2]
  | .ref t .restrict => tag 3 :: encNat t
  | .ref t .setNull => tag 4 :: encNat t
  | .list e => tag 5 :: encTy e

def decTy : Bytes → Option (Ty × Bytes)
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.int, r)
    | 1 => some (.str, r)
    | 2 => some (.bool, r)
    | 3 => match decNat r with
      | some (t, r') => some (.ref t .restrict, r')
      | none => none
    | 4 => match decNat r with
      | some (t, r') => some (.ref t .setNull, r')
      | none => none
    | 5 => match decTy r with
      | some (e, r') => some (.list e, r')
      | none => none
    | _ => none

theorem decTy_encTy : ∀ (t : Ty) (rest : Bytes), decTy (encTy t ++ rest) = some (t, rest)
  | .int, rest => by simp [encTy, decTy, tag]
  | .str, rest => by simp [encTy, decTy, tag]
  | .bool, rest => by simp [encTy, decTy, tag]
  | .ref t .restrict, rest => by simp [encTy, decTy, tag, lawful_nat.1]
  | .ref t .setNull, rest => by simp [encTy, decTy, tag, lawful_nat.1]
  | .list e, rest => by
    have ih := decTy_encTy e rest
    simp [encTy, decTy, tag, ih]

theorem decTy_canon : ∀ (bs : Bytes) (t : Ty) (rest : Bytes),
    decTy bs = some (t, rest) → bs = encTy t ++ rest
  | [], t, rest, h => by simp [decTy] at h
  | b :: r, t, rest, h => by
    rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
        b.toNat = 5 ∨ 6 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk | hk
    · simp only [decTy, hk, Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      simp [encTy, byte_eq_tag b 0 hk]
    · simp only [decTy, hk, Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      simp [encTy, byte_eq_tag b 1 hk]
    · simp only [decTy, hk, Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, rfl⟩ := h
      simp [encTy, byte_eq_tag b 2 hk]
    · simp only [decTy, hk] at h
      cases hd : decNat r with
      | none => simp [hd] at h
      | some p =>
        obtain ⟨t', r'⟩ := p
        simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        have hr := lawful_nat.2 r t' r' hd
        simp [encTy, byte_eq_tag b 3 hk, hr]
    · simp only [decTy, hk] at h
      cases hd : decNat r with
      | none => simp [hd] at h
      | some p =>
        obtain ⟨t', r'⟩ := p
        simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        have hr := lawful_nat.2 r t' r' hd
        simp [encTy, byte_eq_tag b 4 hk, hr]
    · simp only [decTy, hk] at h
      cases hd : decTy r with
      | none => simp [hd] at h
      | some p =>
        obtain ⟨e, r'⟩ := p
        simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        have hr := decTy_canon r e r' hd
        simp [encTy, byte_eq_tag b 5 hk, hr]
    · simp only [decTy] at h
      exfalso
      repeat' split at h
      all_goals first | omega | simp at h

theorem lawful_ty : Lawful encTy decTy := ⟨decTy_encTy, decTy_canon⟩

/-! ## Elements and values -/

def encElem : Elem → Bytes
  | .null => [tag 0]
  | .int i => tag 1 :: encInt i
  | .str s => tag 2 :: encStr s
  | .bool b => tag 3 :: encBool b
  | .ref id => tag 4 :: encNat id

def decElem : Dec Elem := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.null, r)
    | 1 => match decInt r with
      | some (i, r') => some (.int i, r')
      | none => none
    | 2 => match decStr r with
      | some (s, r') => some (.str s, r')
      | none => none
    | 3 => match decBool r with
      | some (x, r') => some (.bool x, r')
      | none => none
    | 4 => match decNat r with
      | some (id, r') => some (.ref id, r')
      | none => none
    | _ => none

theorem lawful_elem : Lawful encElem decElem := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | null => simp [encElem, decElem, tag]
    | int i => simp [encElem, decElem, tag, lawful_int.1]
    | str s => simp [encElem, decElem, tag, lawful_str.1]
    | bool b => simp [encElem, decElem, tag, lawful_bool.1]
    | ref id => simp [encElem, decElem, tag, lawful_nat.1]
  · intro bs x rest h
    cases bs with
    | nil => simp [decElem] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          5 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk
      · simp only [decElem, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encElem, byte_eq_tag b 0 hk]
      · simp only [decElem, hk] at h
        cases hd : decInt r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨i, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_int.2 r i r' hd
          simp [encElem, byte_eq_tag b 1 hk, hr]
      · simp only [decElem, hk] at h
        cases hd : decStr r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨s, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_str.2 r s r' hd
          simp [encElem, byte_eq_tag b 2 hk, hr]
      · simp only [decElem, hk] at h
        cases hd : decBool r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨x', r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_bool.2 r x' r' hd
          simp [encElem, byte_eq_tag b 3 hk, hr]
      · simp only [decElem, hk] at h
        cases hd : decNat r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨id, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r id r' hd
          simp [encElem, byte_eq_tag b 4 hk, hr]
      · simp only [decElem] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A value: the tag `0` for null, `1`..`4` for the scalars and reference, `5` for a list. -/
def encVal : Val → Bytes
  | .null => [tag 0]
  | .int i => tag 1 :: encInt i
  | .str s => tag 2 :: encStr s
  | .bool b => tag 3 :: encBool b
  | .ref id => tag 4 :: encNat id
  | .list vs => tag 5 :: encList encElem vs

def decVal : Dec Val := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.null, r)
    | 1 => match decInt r with
      | some (i, r') => some (.int i, r')
      | none => none
    | 2 => match decStr r with
      | some (s, r') => some (.str s, r')
      | none => none
    | 3 => match decBool r with
      | some (x, r') => some (.bool x, r')
      | none => none
    | 4 => match decNat r with
      | some (id, r') => some (.ref id, r')
      | none => none
    | 5 => match decList decElem r with
      | some (vs, r') => some (.list vs, r')
      | none => none
    | _ => none

theorem lawful_val : Lawful encVal decVal := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | null => simp [encVal, decVal, tag]
    | int i => simp [encVal, decVal, tag, lawful_int.1]
    | str s => simp [encVal, decVal, tag, lawful_str.1]
    | bool b => simp [encVal, decVal, tag, lawful_bool.1]
    | ref id => simp [encVal, decVal, tag, lawful_nat.1]
    | list vs => simp [encVal, decVal, tag, (lawful_list lawful_elem).1]
  · intro bs x rest h
    cases bs with
    | nil => simp [decVal] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ 6 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk | hk
      · simp only [decVal, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encVal, byte_eq_tag b 0 hk]
      · simp only [decVal, hk] at h
        cases hd : decInt r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨i, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_int.2 r i r' hd
          simp [encVal, byte_eq_tag b 1 hk, hr]
      · simp only [decVal, hk] at h
        cases hd : decStr r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨s, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_str.2 r s r' hd
          simp [encVal, byte_eq_tag b 2 hk, hr]
      · simp only [decVal, hk] at h
        cases hd : decBool r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨x', r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_bool.2 r x' r' hd
          simp [encVal, byte_eq_tag b 3 hk, hr]
      · simp only [decVal, hk] at h
        cases hd : decNat r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨id, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r id r' hd
          simp [encVal, byte_eq_tag b 4 hk, hr]
      · simp only [decVal, hk] at h
        cases hd : decList decElem r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨vs, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := (lawful_list lawful_elem).2 r vs r' hd
          simp [encVal, byte_eq_tag b 5 hk, hr]
      · simp only [decVal] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-! ## Fields, limits, records, schemas, tenants

Each structure is encoded as the nested product of its fields, in declaration order, through
`encIso`; the product's codec is `encPair` / `decPair`. -/

/-- A field definition: type, required, unique, default. -/
def encFieldDef : FieldDef → Bytes :=
  encIso (encPair encTy (encPair encBool (encPair encBool encVal)))
    (fun fd => (fd.ty, fd.required, fd.unique, fd.default))

def decFieldDef : Dec FieldDef :=
  decIso (decPair decTy (decPair decBool (decPair decBool decVal)))
    (fun p => { ty := p.1, required := p.2.1, unique := p.2.2.1, default := p.2.2.2 })

theorem lawful_fieldDef : Lawful encFieldDef decFieldDef :=
  lawful_iso (lawful_pair lawful_ty (lawful_pair lawful_bool (lawful_pair lawful_bool lawful_val)))
    (fun fd => by cases fd; rfl) (fun _ => rfl)

/-- The limits: the string, list, unique and record bounds, as four naturals. -/
def encLimits : Limits → Bytes :=
  encIso (encPair encNat (encPair encNat (encPair encNat encNat)))
    (fun L => (L.maxString, L.maxList, L.maxUnique, L.maxRecord))

def decLimits : Dec Limits :=
  decIso (decPair decNat (decPair decNat (decPair decNat decNat)))
    (fun p => { maxString := p.1, maxList := p.2.1, maxUnique := p.2.2.1, maxRecord := p.2.2.2 })

theorem lawful_limits : Lawful encLimits decLimits :=
  lawful_iso (lawful_pair lawful_nat (lawful_pair lawful_nat (lawful_pair lawful_nat lawful_nat)))
    (fun L => by cases L; rfl) (fun _ => rfl)

/-- A field of a record: the field id, then its value. -/
def encFieldVal : FieldId × Val → Bytes := encPair encNat encVal

def decFieldVal : Dec (FieldId × Val) := decPair decNat decVal

theorem lawful_fieldVal : Lawful encFieldVal decFieldVal := lawful_pair lawful_nat lawful_val

/-- A schema entry: the field id, then its definition. -/
def encSchemaEntry : FieldId × FieldDef → Bytes := encPair encNat encFieldDef

def decSchemaEntry : Dec (FieldId × FieldDef) := decPair decNat decFieldDef

theorem lawful_schemaEntry : Lawful encSchemaEntry decSchemaEntry :=
  lawful_pair lawful_nat lawful_fieldDef

/-- A record: its owner, its version, then its fields. -/
def encRecord : Record → Bytes :=
  encIso (encPair encNat (encPair encNat (encList encFieldVal)))
    (fun r => (r.owner, r.version, r.fields))

def decRecord : Dec Record :=
  decIso (decPair decNat (decPair decNat (decList decFieldVal)))
    (fun p => { owner := p.1, version := p.2.1, fields := p.2.2 })

theorem lawful_record : Lawful encRecord decRecord :=
  lawful_iso (lawful_pair lawful_nat (lawful_pair lawful_nat (lawful_list lawful_fieldVal)))
    (fun r => by cases r; rfl) (fun _ => rfl)

/-- A record of the tenant: the record id, then the record. -/
def encRecordEntry : RecordId × Record → Bytes := encPair encNat encRecord

def decRecordEntry : Dec (RecordId × Record) := decPair decNat decRecord

theorem lawful_recordEntry : Lawful encRecordEntry decRecordEntry :=
  lawful_pair lawful_nat lawful_record

/-- An object type: its schema, its composite constraints, then its records. -/
def encObjType : ObjType → Bytes :=
  encIso (encPair (encList encSchemaEntry) (encPair (encList (encList encNat)) (encList encRecordEntry)))
    (fun o => (o.schema, o.uniques, o.records))

def decObjType : Dec ObjType :=
  decIso (decPair (decList decSchemaEntry)
      (decPair (decList (decList decNat)) (decList decRecordEntry)))
    (fun p => { schema := p.1, uniques := p.2.1, records := p.2.2 })

theorem lawful_objType : Lawful encObjType decObjType :=
  lawful_iso (lawful_pair (lawful_list lawful_schemaEntry)
      (lawful_pair (lawful_list (lawful_list lawful_nat)) (lawful_list lawful_recordEntry)))
    (fun o => by cases o; rfl) (fun _ => rfl)

/-- A type of the tenant: its id, then the object type. -/
def encTypeEntry : TypeId × ObjType → Bytes := encPair encNat encObjType

def decTypeEntry : Dec (TypeId × ObjType) := decPair decNat decObjType

theorem lawful_typeEntry : Lawful encTypeEntry decTypeEntry :=
  lawful_pair lawful_nat lawful_objType

/-- The tenant's fields as one nested product: its object types, owner, editors, writers, next id,
    limits. -/
def encTenantBody : List (TypeId × ObjType) × Nat × List Nat × List Nat × Nat × Limits → Bytes :=
  encPair (encList encTypeEntry)
    (encPair encNat
      (encPair (encList encNat)
        (encPair (encList encNat)
          (encPair encNat encLimits))))

def decTenantBody :
    Dec (List (TypeId × ObjType) × Nat × List Nat × List Nat × Nat × Limits) :=
  decPair (decList decTypeEntry)
    (decPair decNat
      (decPair (decList decNat)
        (decPair (decList decNat)
          (decPair decNat decLimits))))

theorem lawful_tenantBody : Lawful encTenantBody decTenantBody :=
  lawful_pair (lawful_list lawful_typeEntry)
    (lawful_pair lawful_nat
      (lawful_pair (lawful_list lawful_nat)
        (lawful_pair (lawful_list lawful_nat)
          (lawful_pair lawful_nat lawful_limits))))

def encTenant : Tenant → Bytes :=
  encIso encTenantBody
    (fun t => (t.types, t.owner, t.editors, t.writers, t.nextId, t.limits))

def decTenant : Dec Tenant :=
  decIso decTenantBody fun p =>
    ⟨p.1, p.2.1, p.2.2.1, p.2.2.2.1, p.2.2.2.2.1, p.2.2.2.2.2⟩

theorem lawful_tenant : Lawful encTenant decTenant :=
  lawful_iso lawful_tenantBody (fun t => by cases t; rfl) (fun _ => rfl)

/-! ## Intents, contexts and results -/

/-- An intent: tag `0` create, `1` update, `2` delete, `3` set required, `4` set unique, `5`
    define a composite unique constraint. Each names its object type first, right after the tag,
    as a natural. -/
def encIntent : Intent → Bytes
  | .create ty fs => tag 0 :: (encNat ty ++ encList encFieldVal fs)
  | .update ty id fs ev => tag 1 :: (encNat ty ++ encNat id ++ encList encFieldVal fs ++ encOpt encNat ev)
  | .delete ty id ev => tag 2 :: (encNat ty ++ encNat id ++ encOpt encNat ev)
  | .setRequired ty f s => tag 3 :: (encNat ty ++ encNat f ++ encBool s)
  | .setUnique ty f s => tag 4 :: (encNat ty ++ encNat f ++ encBool s)
  | .defineUnique ty fs => tag 5 :: (encNat ty ++ encList encNat fs)

def decIntent : Dec Intent := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (ty, r1) => match decList decFieldVal r1 with
        | some (fs, r2) => some (.create ty fs, r2)
        | none => none
      | none => none
    | 1 => match decNat r with
      | some (ty, r1) => match decNat r1 with
        | some (id, r2) => match decList decFieldVal r2 with
          | some (fs, r3) => match decOpt decNat r3 with
            | some (ev, r4) => some (.update ty id fs ev, r4)
            | none => none
          | none => none
        | none => none
      | none => none
    | 2 => match decNat r with
      | some (ty, r1) => match decNat r1 with
        | some (id, r2) => match decOpt decNat r2 with
          | some (ev, r3) => some (.delete ty id ev, r3)
          | none => none
        | none => none
      | none => none
    | 3 => match decNat r with
      | some (ty, r1) => match decNat r1 with
        | some (f, r2) => match decBool r2 with
          | some (s, r3) => some (.setRequired ty f s, r3)
          | none => none
        | none => none
      | none => none
    | 4 => match decNat r with
      | some (ty, r1) => match decNat r1 with
        | some (f, r2) => match decBool r2 with
          | some (s, r3) => some (.setUnique ty f s, r3)
          | none => none
        | none => none
      | none => none
    | 5 => match decNat r with
      | some (ty, r1) => match decList decNat r1 with
        | some (fs, r2) => some (.defineUnique ty fs, r2)
        | none => none
      | none => none
    | _ => none

theorem lawful_intent : Lawful encIntent decIntent := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | create ty fs =>
      simp [encIntent, decIntent, tag, lawful_nat.1, (lawful_list lawful_fieldVal).1, List.append_assoc]
    | update ty id fs ev =>
      simp [encIntent, decIntent, tag, lawful_nat.1, (lawful_list lawful_fieldVal).1,
        (lawful_opt lawful_nat).1, List.append_assoc]
    | delete ty id ev =>
      simp [encIntent, decIntent, tag, lawful_nat.1, (lawful_opt lawful_nat).1, List.append_assoc]
    | setRequired ty f s =>
      simp [encIntent, decIntent, tag, lawful_nat.1, lawful_bool.1, List.append_assoc]
    | setUnique ty f s =>
      simp [encIntent, decIntent, tag, lawful_nat.1, lawful_bool.1, List.append_assoc]
    | defineUnique ty fs =>
      simp [encIntent, decIntent, tag, lawful_nat.1, (lawful_list lawful_nat).1, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decIntent] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ 6 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk | hk
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decList decFieldVal r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨fs, r2⟩ := p2
            simp only [hd1, hd2, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have e1 := lawful_nat.2 r ty r1 hd1
            have e2 := (lawful_list lawful_fieldVal).2 r1 fs r2 hd2
            simp [encIntent, byte_eq_tag b 0 hk, e1, e2, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨id, r2⟩ := p2
            cases hd3 : decList decFieldVal r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨fs, r3⟩ := p3
              cases hd4 : decOpt decNat r3 with
              | none => simp [hd1, hd2, hd3, hd4] at h
              | some p4 =>
                obtain ⟨ev, r4⟩ := p4
                simp only [hd1, hd2, hd3, hd4, Option.some.injEq, Prod.mk.injEq] at h
                obtain ⟨rfl, rfl⟩ := h
                have e1 := lawful_nat.2 r ty r1 hd1
                have e2 := lawful_nat.2 r1 id r2 hd2
                have e3 := (lawful_list lawful_fieldVal).2 r2 fs r3 hd3
                have e4 := (lawful_opt lawful_nat).2 r3 ev r4 hd4
                simp [encIntent, byte_eq_tag b 1 hk, e1, e2, e3, e4, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨id, r2⟩ := p2
            cases hd3 : decOpt decNat r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨ev, r3⟩ := p3
              simp only [hd1, hd2, hd3, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have e1 := lawful_nat.2 r ty r1 hd1
              have e2 := lawful_nat.2 r1 id r2 hd2
              have e3 := (lawful_opt lawful_nat).2 r2 ev r3 hd3
              simp [encIntent, byte_eq_tag b 2 hk, e1, e2, e3, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨f, r2⟩ := p2
            cases hd3 : decBool r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨s, r3⟩ := p3
              simp only [hd1, hd2, hd3, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have e1 := lawful_nat.2 r ty r1 hd1
              have e2 := lawful_nat.2 r1 f r2 hd2
              have e3 := lawful_bool.2 r2 s r3 hd3
              simp [encIntent, byte_eq_tag b 3 hk, e1, e2, e3, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨f, r2⟩ := p2
            cases hd3 : decBool r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨s, r3⟩ := p3
              simp only [hd1, hd2, hd3, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have e1 := lawful_nat.2 r ty r1 hd1
              have e2 := lawful_nat.2 r1 f r2 hd2
              have e3 := lawful_bool.2 r2 s r3 hd3
              simp [encIntent, byte_eq_tag b 4 hk, e1, e2, e3, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ty, r1⟩ := p1
          cases hd2 : decList decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨fs, r2⟩ := p2
            simp only [hd1, hd2, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have e1 := lawful_nat.2 r ty r1 hd1
            have e2 := (lawful_list lawful_nat).2 r1 fs r2 hd2
            simp [encIntent, byte_eq_tag b 5 hk, e1, e2, List.append_assoc]
      · simp only [decIntent] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- The context of a step: the tenant and the acting principal. -/
def encCtx : Ctx → Bytes := encIso (encPair encNat encNat) (fun c => (c.tenant, c.principal))

def decCtx : Dec Ctx :=
  decIso (decPair decNat decNat) (fun p => { tenant := p.1, principal := p.2 })

theorem lawful_ctx : Lawful encCtx decCtx :=
  lawful_iso (lawful_pair lawful_nat lawful_nat) (fun c => by cases c; rfl) (fun _ => rfl)

/-- The result of a step: tag `0` a refusal, `1` the tenant after the change. -/
def encResult : Except Err Tenant → Bytes
  | .error e => tag 0 :: encErr e
  | .ok t => tag 1 :: encTenant t

def decResult : Dec (Except Err Tenant) := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decErr r with
      | some (e, r') => some (.error e, r')
      | none => none
    | 1 => match decTenant r with
      | some (t, r') => some (.ok t, r')
      | none => none
    | _ => none

theorem lawful_result : Lawful encResult decResult := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | error e => simp [encResult, decResult, tag, lawful_err.1]
    | ok t => simp [encResult, decResult, tag, lawful_tenant.1]
  · intro bs x rest h
    cases bs with
    | nil => simp [decResult] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ 2 ≤ b.toNat by omega) with hk | hk | hk
      · simp only [decResult, hk] at h
        cases hd : decErr r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨e, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_err.2 r e r' hd
          simp [encResult, byte_eq_tag b 0 hk, hr]
      · simp only [decResult, hk] at h
        cases hd : decTenant r with
        | none => simp [hd] at h
        | some p =>
          obtain ⟨t, r'⟩ := p
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_tenant.2 r t r' hd
          simp [encResult, byte_eq_tag b 1 hk, hr]
      · simp only [decResult] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-! ## The boundary entry points -/

/-- Canonical bytes of a value. -/
def encodeBytes {α : Type} (e : α → Bytes) (x : α) : ByteArray := ⟨(e x).toArray⟩

/-- The value of bytes that the decoder reads completely; `none` if it stops early or fails. -/
def decodeBytes {α : Type} (d : Dec α) (b : ByteArray) : Option α :=
  match d b.data.toList with
  | some (x, []) => some x
  | _ => none

theorem decodeBytes_encodeBytes {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d)
    (x : α) : decodeBytes d (encodeBytes e x) = some x := by
  unfold decodeBytes encodeBytes
  simp only [List.toList_toArray]
  have := h.1 x []
  simp only [List.append_nil] at this
  rw [this]

theorem encodeBytes_of_decodeBytes {α : Type} {e : α → Bytes} {d : Dec α} (h : Lawful e d)
    (b : ByteArray) (x : α) (hb : decodeBytes d b = some x) : encodeBytes e x = b := by
  unfold decodeBytes at hb
  cases hd : d b.data.toList with
  | none => simp [hd] at hb
  | some yr =>
    obtain ⟨y, r⟩ := yr
    cases r with
    | nil =>
      simp only [hd, Option.some.injEq] at hb
      subst hb
      have hc := h.2 b.data.toList y [] hd
      simp only [List.append_nil] at hc
      unfold encodeBytes
      rw [← hc]
    | cons c cs => simp [hd] at hb

end Strata.Codec
