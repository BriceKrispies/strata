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

/-- The kernel's errors: nineteen tags, no payload. `unknownObject` is tag `11`,
    `constraintViolated` `12` and `evaluation` `13`; slice 7 adds `invalidIntent` `14`,
    `unknownRole` `15`, `lastOwner` `16` and `tenantExists` `17`; slice 8 adds `workflowViolation` `18`; slice 11 adds `schemaConflict` `19`. -/
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
  | .constraintViolated => [tag 12]
  | .evaluation => [tag 13]
  | .invalidIntent => [tag 14]
  | .unknownRole => [tag 15]
  | .lastOwner => [tag 16]
  | .tenantExists => [tag 17]
  | .workflowViolation => [tag 18]
  | .schemaConflict => [tag 19]
  | .selfAction => [tag 20]

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
    | 12 => some (.constraintViolated, r)
    | 13 => some (.evaluation, r)
    | 14 => some (.invalidIntent, r)
    | 15 => some (.unknownRole, r)
    | 16 => some (.lastOwner, r)
    | 17 => some (.tenantExists, r)
    | 18 => some (.workflowViolation, r)
    | 19 => some (.schemaConflict, r)
    | 20 => some (.selfAction, r)
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
          b.toNat = 10 ∨ b.toNat = 11 ∨ b.toNat = 12 ∨ b.toNat = 13 ∨ b.toNat = 14 ∨
          b.toNat = 15 ∨ b.toNat = 16 ∨ b.toNat = 17 ∨ b.toNat = 18 ∨ b.toNat = 19 ∨ b.toNat = 20 ∨
          21 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk |
          hk | hk | hk | hk | hk
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
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 12 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 13 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 14 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 15 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 16 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 17 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 18 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 19 hk]
      · simp only [decErr, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encErr, byte_eq_tag b 20 hk]
      · simp only [decErr] at h
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

/-! ## Expressions (slice 5)

An expression is encoded in prefix order: a tag byte, then its parts. The tags are `0` for a
literal (its value follows), `1` for a field (its scope byte, then the field id), `2` for the
principal, `3` for a comparison (its operator byte, then its two operands), `4` and `5` for `and`
and `or` (two operands each), `6` for `not`, `7` for `isNull` (one operand) and `8` for `$created_by` (its scope byte). A scope is `0` for
the record, `1` for old, `2` for new; a comparison operator is `0`..`5` for `eq`, `ne`, `lt`, `le`,
`gt`, `ge`.

A decoder takes a fuel: `decExprN n` spends one unit of fuel on each nested expression it reads,
and `decExpr` takes the length of its input as the fuel. Every node is at least one byte, so the
length of an expression's encoding bounds its node count, and that fuel is enough. The round trip
is stated for any fuel that covers the nodes (`decExprN_encExpr`); canonicity needs no fuel
(`decExprN_canon`). -/

def encScope : Scope → Bytes
  | .record => [tag 0]
  | .old => [tag 1]
  | .new => [tag 2]

def decScope : Dec Scope := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.record, r)
    | 1 => some (.old, r)
    | 2 => some (.new, r)
    | _ => none

theorem lawful_scope : Lawful encScope decScope := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encScope, decScope, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decScope] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ 3 ≤ b.toNat by omega)
        with hk | hk | hk | hk
      · simp only [decScope, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encScope, byte_eq_tag b 0 hk]
      · simp only [decScope, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encScope, byte_eq_tag b 1 hk]
      · simp only [decScope, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encScope, byte_eq_tag b 2 hk]
      · simp only [decScope] at h
        repeat' split at h
        all_goals first | omega | simp at h

def encCmpOp : CmpOp → Bytes
  | .eq => [tag 0]
  | .ne => [tag 1]
  | .lt => [tag 2]
  | .le => [tag 3]
  | .gt => [tag 4]
  | .ge => [tag 5]

def decCmpOp : Dec CmpOp := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.eq, r)
    | 1 => some (.ne, r)
    | 2 => some (.lt, r)
    | 3 => some (.le, r)
    | 4 => some (.gt, r)
    | 5 => some (.ge, r)
    | _ => none

theorem lawful_cmpOp : Lawful encCmpOp decCmpOp := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encCmpOp, decCmpOp, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decCmpOp] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ 6 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk | hk
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 0 hk]
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 1 hk]
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 2 hk]
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 3 hk]
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 4 hk]
      · simp only [decCmpOp, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encCmpOp, byte_eq_tag b 5 hk]
      · simp only [decCmpOp] at h
        repeat' split at h
        all_goals first | omega | simp at h

/-- The number of nodes of an expression. Each node is at least one byte. -/
def _root_.Strata.Expr.nodes : Expr → Nat
  | .lit _ => 1
  | .field _ _ => 1
  | .principal => 1
  | .cmp _ a b => 1 + a.nodes + b.nodes
  | .and a b => 1 + a.nodes + b.nodes
  | .or a b => 1 + a.nodes + b.nodes
  | .not a => 1 + a.nodes
  | .isNull a => 1 + a.nodes
  | .createdBy _ => 1

def encExpr : Expr → Bytes
  | .lit v => tag 0 :: encVal v
  | .field s f => tag 1 :: (encScope s ++ encNat f)
  | .principal => [tag 2]
  | .cmp op a b => tag 3 :: (encCmpOp op ++ (encExpr a ++ encExpr b))
  | .and a b => tag 4 :: (encExpr a ++ encExpr b)
  | .or a b => tag 5 :: (encExpr a ++ encExpr b)
  | .not a => tag 6 :: encExpr a
  | .isNull a => tag 7 :: encExpr a
  | .createdBy s => tag 8 :: encScope s

/-- The decoder with fuel `n`: each nested expression read spends one unit of fuel. -/
def decExprN : Nat → Dec Expr
  | 0, _ => none
  | n + 1, bs =>
    match bs with
    | [] => none
    | b :: r =>
      match b.toNat with
      | 0 => match decVal r with
        | some (v, r') => some (.lit v, r')
        | none => none
      | 1 => match decScope r with
        | some (s, r1) => match decNat r1 with
          | some (f, r2) => some (.field s f, r2)
          | none => none
        | none => none
      | 2 => some (.principal, r)
      | 3 => match decCmpOp r with
        | some (op, r1) => match decExprN n r1 with
          | some (a, r2) => match decExprN n r2 with
            | some (b', r3) => some (.cmp op a b', r3)
            | none => none
          | none => none
        | none => none
      | 4 => match decExprN n r with
        | some (a, r1) => match decExprN n r1 with
          | some (b', r2) => some (.and a b', r2)
          | none => none
        | none => none
      | 5 => match decExprN n r with
        | some (a, r1) => match decExprN n r1 with
          | some (b', r2) => some (.or a b', r2)
          | none => none
        | none => none
      | 6 => match decExprN n r with
        | some (a, r') => some (.not a, r')
        | none => none
      | 7 => match decExprN n r with
        | some (a, r') => some (.isNull a, r')
        | none => none
      | 8 => match decScope r with
        | some (s, r') => some (.createdBy s, r')
        | none => none
      | _ => none

def decExpr : Dec Expr := fun bs => decExprN bs.length bs

/-- An expression's encoding is at least as long as its node count. -/
theorem nodes_le_length : ∀ (e : Expr), e.nodes ≤ (encExpr e).length
  | .lit v => by
    simp only [Strata.Expr.nodes, encExpr, List.length_cons]
    omega
  | .field s f => by
    simp only [Strata.Expr.nodes, encExpr, List.length_cons, List.length_append]
    omega
  | .principal => by simp [Expr.nodes, encExpr]
  | .cmp op a b => by
    have ha := nodes_le_length a
    have hb := nodes_le_length b
    simp only [Strata.Expr.nodes, encExpr, List.length_cons, List.length_append, encCmpOp]
    omega
  | .and a b => by
    have ha := nodes_le_length a
    have hb := nodes_le_length b
    simp only [Strata.Expr.nodes, encExpr, List.length_cons, List.length_append]
    omega
  | .or a b => by
    have ha := nodes_le_length a
    have hb := nodes_le_length b
    simp only [Strata.Expr.nodes, encExpr, List.length_cons, List.length_append]
    omega
  | .not a => by
    have ha := nodes_le_length a
    simp only [Strata.Expr.nodes, encExpr, List.length_cons]
    omega
  | .isNull a => by
    have ha := nodes_le_length a
    simp only [Strata.Expr.nodes, encExpr, List.length_cons]
    omega
  | .createdBy s => by
    cases s <;> simp [Strata.Expr.nodes, encExpr, encScope]

/-- The round trip, for any fuel that covers the nodes of the expression. -/
theorem decExprN_encExpr : ∀ (e : Expr) (rest : Bytes) (n : Nat), e.nodes ≤ n →
    decExprN n (encExpr e ++ rest) = some (e, rest)
  | .lit v, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    simp [encExpr, decExprN, tag, lawful_val.1 v rest]
  | .field s f, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    simp [encExpr, decExprN, tag, lawful_scope.1 s, lawful_nat.1 f rest]
  | .principal, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    simp [encExpr, decExprN, tag]
  | .cmp op a b, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    have hab : a.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have hbb : b.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have ha := decExprN_encExpr a (encExpr b ++ rest) m hab
    have hb := decExprN_encExpr b rest m hbb
    simp [encExpr, decExprN, tag, lawful_cmpOp.1 op, ha, hb]
  | .and a b, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    have hab : a.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have hbb : b.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have ha := decExprN_encExpr a (encExpr b ++ rest) m hab
    have hb := decExprN_encExpr b rest m hbb
    simp [encExpr, decExprN, tag, ha, hb]
  | .or a b, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    have hab : a.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have hbb : b.nodes ≤ m := by simp [Strata.Expr.nodes] at hn; omega
    have ha := decExprN_encExpr a (encExpr b ++ rest) m hab
    have hb := decExprN_encExpr b rest m hbb
    simp [encExpr, decExprN, tag, ha, hb]
  | .not a, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    have ha := decExprN_encExpr a rest m (by simp [Strata.Expr.nodes] at hn; omega)
    simp [encExpr, decExprN, tag, ha]
  | .isNull a, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    have ha := decExprN_encExpr a rest m (by simp [Strata.Expr.nodes] at hn; omega)
    simp [encExpr, decExprN, tag, ha]
  | .createdBy s, rest, n, hn => by
    obtain ⟨m, rfl⟩ : ∃ m, n = m + 1 := ⟨n - 1, by simp [Strata.Expr.nodes] at hn; omega⟩
    simp [encExpr, decExprN, tag, lawful_scope.1 s]

/-- Every decoding with fuel `n` is canonical. -/
theorem decExprN_canon : ∀ (n : Nat) (bs : Bytes) (e : Expr) (rest : Bytes),
    decExprN n bs = some (e, rest) → bs = encExpr e ++ rest := by
  intro n
  induction n with
  | zero => intro bs e rest h; simp [decExprN] at h
  | succ m ih =>
    intro bs e rest h
    cases bs with
    | nil => simp [decExprN] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ b.toNat = 6 ∨ b.toNat = 7 ∨ b.toNat = 8 ∨ 9 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk | hk | hk | hk | hk | hk
      · simp only [decExprN, hk] at h
        cases hv : decVal r with
        | none => simp [hv] at h
        | some p =>
          obtain ⟨v, r'⟩ := p
          simp only [hv, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_val.2 r v r' hv
          simp [encExpr, byte_eq_tag b 0 hk, hr]
      · simp only [decExprN, hk] at h
        cases hs : decScope r with
        | none => simp [hs] at h
        | some p =>
          obtain ⟨s, r1⟩ := p
          simp only [hs] at h
          cases hf : decNat r1 with
          | none => simp [hf] at h
          | some q =>
            obtain ⟨f, r2⟩ := q
            simp only [hf, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have hs' := lawful_scope.2 r s r1 hs
            have hf' := lawful_nat.2 r1 f r2 hf
            simp [encExpr, byte_eq_tag b 1 hk, hs', hf']
      · simp only [decExprN, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encExpr, byte_eq_tag b 2 hk]
      · simp only [decExprN, hk] at h
        cases hc : decCmpOp r with
        | none => simp [hc] at h
        | some p =>
          obtain ⟨op, r1⟩ := p
          simp only [hc] at h
          cases ha : decExprN m r1 with
          | none => simp [ha] at h
          | some q =>
            obtain ⟨a, r2⟩ := q
            simp only [ha] at h
            cases hb : decExprN m r2 with
            | none => simp [hb] at h
            | some s =>
              obtain ⟨b', r3⟩ := s
              simp only [hb, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have hc' := lawful_cmpOp.2 r op r1 hc
              have ha' := ih r1 a r2 ha
              have hb' := ih r2 b' r3 hb
              simp only [encExpr, byte_eq_tag b 3 hk, hc', ha', hb', List.append_assoc]
              simp
      · simp only [decExprN, hk] at h
        cases ha : decExprN m r with
        | none => simp [ha] at h
        | some q =>
          obtain ⟨a, r1⟩ := q
          simp only [ha] at h
          cases hb : decExprN m r1 with
          | none => simp [hb] at h
          | some s =>
            obtain ⟨b', r2⟩ := s
            simp only [hb, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have ha' := ih r a r1 ha
            have hb' := ih r1 b' r2 hb
            simp only [encExpr, byte_eq_tag b 4 hk, ha', hb', List.append_assoc]
            simp
      · simp only [decExprN, hk] at h
        cases ha : decExprN m r with
        | none => simp [ha] at h
        | some q =>
          obtain ⟨a, r1⟩ := q
          simp only [ha] at h
          cases hb : decExprN m r1 with
          | none => simp [hb] at h
          | some s =>
            obtain ⟨b', r2⟩ := s
            simp only [hb, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have ha' := ih r a r1 ha
            have hb' := ih r1 b' r2 hb
            simp only [encExpr, byte_eq_tag b 5 hk, ha', hb', List.append_assoc]
            simp
      · simp only [decExprN, hk] at h
        cases ha : decExprN m r with
        | none => simp [ha] at h
        | some q =>
          obtain ⟨a, r'⟩ := q
          simp only [ha, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have ha' := ih r a r' ha
          simp [encExpr, byte_eq_tag b 6 hk, ha']
      · simp only [decExprN, hk] at h
        cases ha : decExprN m r with
        | none => simp [ha] at h
        | some q =>
          obtain ⟨a, r'⟩ := q
          simp only [ha, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have ha' := ih r a r' ha
          simp [encExpr, byte_eq_tag b 7 hk, ha']
      · simp only [decExprN, hk] at h
        cases hs : decScope r with
        | none => simp [hs] at h
        | some p =>
          obtain ⟨s, r'⟩ := p
          simp only [hs, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hs' := lawful_scope.2 r s r' hs
          simp [encExpr, byte_eq_tag b 8 hk, hs']
      · simp only [decExprN] at h
        repeat' split at h
        all_goals first | omega | simp at h

theorem lawful_expr : Lawful encExpr decExpr := by
  refine ⟨?_, ?_⟩
  · intro e rest
    unfold decExpr
    apply decExprN_encExpr e rest
    have h1 := nodes_le_length e
    simp only [List.length_append]
    omega
  · intro bs e rest h
    exact decExprN_canon _ bs e rest h

/-! ## Principals, roles and policies (slice 6)

An action is one byte: `0` read, `1` create, `2` update, `3` delete, `4` manage. A principal is a tag
then its id for a user (`0`), or the tag alone for the system (`1`) and an anonymous caller (`2`). A
resource is `0` and its object type for an object type, `1` for every object type and `2` for the
schema. A grant is `0` and a capability id, or `1`, an object type and an action. A policy is its
role, its resource, its actions and its optional condition (`encOpt`). -/

def encAction : Action → Bytes
  | .read => [tag 0]
  | .create => [tag 1]
  | .update => [tag 2]
  | .delete => [tag 3]
  | .manage => [tag 4]

def decAction : Dec Action := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.read, r)
    | 1 => some (.create, r)
    | 2 => some (.update, r)
    | 3 => some (.delete, r)
    | 4 => some (.manage, r)
    | _ => none

theorem lawful_action : Lawful encAction decAction := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encAction, decAction, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decAction] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          5 ≤ b.toNat by omega) with hk | hk | hk | hk | hk | hk
      · simp only [decAction, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encAction, byte_eq_tag b 0 hk]
      · simp only [decAction, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encAction, byte_eq_tag b 1 hk]
      · simp only [decAction, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encAction, byte_eq_tag b 2 hk]
      · simp only [decAction, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encAction, byte_eq_tag b 3 hk]
      · simp only [decAction, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encAction, byte_eq_tag b 4 hk]
      · simp only [decAction] at h
        repeat' split at h
        all_goals first | omega | simp at h

def encPrincipal : Principal → Bytes
  | .user p => tag 0 :: encNat p
  | .system => [tag 1]
  | .anonymous => [tag 2]

def decPrincipal : Dec Principal := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (p, r') => some (.user p, r')
      | none => none
    | 1 => some (.system, r)
    | 2 => some (.anonymous, r)
    | _ => none

theorem lawful_principal : Lawful encPrincipal decPrincipal := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | user p => simp [encPrincipal, decPrincipal, tag, lawful_nat.1 p rest]
    | system => simp [encPrincipal, decPrincipal, tag]
    | anonymous => simp [encPrincipal, decPrincipal, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decPrincipal] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ 3 ≤ b.toNat by omega)
        with hk | hk | hk | hk
      · simp only [decPrincipal, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨p, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r p r' hn
          simp [encPrincipal, byte_eq_tag b 0 hk, hr]
      · simp only [decPrincipal, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encPrincipal, byte_eq_tag b 1 hk]
      · simp only [decPrincipal, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encPrincipal, byte_eq_tag b 2 hk]
      · simp only [decPrincipal] at h
        repeat' split at h
        all_goals first | omega | simp at h

def encResource : Resource → Bytes
  | .object ty => tag 0 :: encNat ty
  | .anyObject => [tag 1]
  | .schema => [tag 2]
  | .members => [tag 3]

def decResource : Dec Resource := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (ty, r') => some (.object ty, r')
      | none => none
    | 1 => some (.anyObject, r)
    | 2 => some (.schema, r)
    | 3 => some (.members, r)
    | _ => none

theorem lawful_resource : Lawful encResource decResource := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | object ty => simp [encResource, decResource, tag, lawful_nat.1 ty rest]
    | anyObject => simp [encResource, decResource, tag]
    | schema => simp [encResource, decResource, tag]
    | members => simp [encResource, decResource, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decResource] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ 4 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk
      · simp only [decResource, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨ty, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r ty r' hn
          simp [encResource, byte_eq_tag b 0 hk, hr]
      · simp only [decResource, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encResource, byte_eq_tag b 1 hk]
      · simp only [decResource, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encResource, byte_eq_tag b 2 hk]
      · simp only [decResource, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encResource, byte_eq_tag b 3 hk]
      · simp only [decResource] at h
        repeat' split at h
        all_goals first | omega | simp at h

def encGrant : Grant → Bytes
  | .capability c => tag 0 :: encNat c
  | .object ty a => tag 1 :: (encNat ty ++ encAction a)

def decGrant : Dec Grant := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (c, r') => some (.capability c, r')
      | none => none
    | 1 => match decNat r with
      | some (ty, r1) => match decAction r1 with
        | some (a, r2) => some (.object ty a, r2)
        | none => none
      | none => none
    | _ => none

theorem lawful_grant : Lawful encGrant decGrant := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | capability c => simp [encGrant, decGrant, tag, lawful_nat.1 c rest]
    | object ty a =>
      simp [encGrant, decGrant, tag, lawful_nat.1 ty (encAction a ++ rest),
        lawful_action.1 a rest, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decGrant] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ 2 ≤ b.toNat by omega) with hk | hk | hk
      · simp only [decGrant, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨c, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r c r' hn
          simp [encGrant, byte_eq_tag b 0 hk, hr]
      · simp only [decGrant, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨ty, r1⟩ := q
          simp only [hn] at h
          cases ha : decAction r1 with
          | none => simp [ha] at h
          | some q2 =>
            obtain ⟨a, r2⟩ := q2
            simp only [ha, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have hr := lawful_nat.2 r ty r1 hn
            have ha' := lawful_action.2 r1 a r2 ha
            simp [encGrant, byte_eq_tag b 1 hk, hr, ha', List.append_assoc]
      · simp only [decGrant] at h
        repeat' split at h
        all_goals first | omega | simp at h

/-- A policy: its role, its resource, its actions, its condition. -/
def encPolicy : Policy → Bytes :=
  encIso (encPair encNat (encPair encResource (encPair (encList encAction) (encOpt encExpr))))
    (fun pl => (pl.role, pl.resource, pl.actions, pl.condition))

def decPolicy : Dec Policy :=
  decIso (decPair decNat (decPair decResource (decPair (decList decAction) (decOpt decExpr))))
    (fun p => { role := p.1, resource := p.2.1, actions := p.2.2.1, condition := p.2.2.2 })

theorem lawful_policy : Lawful encPolicy decPolicy :=
  lawful_iso (lawful_pair lawful_nat (lawful_pair lawful_resource
      (lawful_pair (lawful_list lawful_action) (lawful_opt lawful_expr))))
    (fun pl => by cases pl; rfl) (fun _ => rfl)

/-- A member: a user, then its roles. -/
def encMember : PrincipalId × List RoleId → Bytes := encPair encNat (encList encNat)

def decMember : Dec (PrincipalId × List RoleId) := decPair decNat (decList decNat)

theorem lawful_member : Lawful encMember decMember :=
  lawful_pair lawful_nat (lawful_list lawful_nat)

/-- A grant entry: a role, then what it grants. -/
def encGrantEntry : RoleId × Grant → Bytes := encPair encNat encGrant

def decGrantEntry : Dec (RoleId × Grant) := decPair decNat decGrant

theorem lawful_grantEntry : Lawful encGrantEntry decGrantEntry :=
  lawful_pair lawful_nat lawful_grant

/-! ## Fields, limits, records, schemas, tenants

Each structure is encoded as the nested product of its fields, in declaration order, through
`encIso`; the product's codec is `encPair` / `decPair`. -/

/-- A field definition: type, required, unique, default. -/
def encFieldDef : FieldDef → Bytes :=
  encIso (encPair encTy (encPair encBool (encPair encBool encExpr)))
    (fun fd => (fd.ty, fd.required, fd.unique, fd.default))

def decFieldDef : Dec FieldDef :=
  decIso (decPair decTy (decPair decBool (decPair decBool decExpr)))
    (fun p => { ty := p.1, required := p.2.1, unique := p.2.2.1, default := p.2.2.2 })

theorem lawful_fieldDef : Lawful encFieldDef decFieldDef :=
  lawful_iso (lawful_pair lawful_ty (lawful_pair lawful_bool (lawful_pair lawful_bool lawful_expr)))
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
  encIso (encPair (encOpt encNat) (encPair encNat (encList encFieldVal)))
    (fun r => (r.owner, r.version, r.fields))

def decRecord : Dec Record :=
  decIso (decPair (decOpt decNat) (decPair decNat (decList decFieldVal)))
    (fun p => { owner := p.1, version := p.2.1, fields := p.2.2 })

theorem lawful_record : Lawful encRecord decRecord :=
  lawful_iso (lawful_pair (lawful_opt lawful_nat) (lawful_pair lawful_nat (lawful_list lawful_fieldVal)))
    (fun r => by cases r; rfl) (fun _ => rfl)

/-- A record of the tenant: the record id, then the record. -/
def encRecordEntry : RecordId × Record → Bytes := encPair encNat encRecord

def decRecordEntry : Dec (RecordId × Record) := decPair decNat decRecord

theorem lawful_recordEntry : Lawful encRecordEntry decRecordEntry :=
  lawful_pair lawful_nat lawful_record

/-- A field assigned by a transition: the field id, then the expression that gives its value. -/
def encFieldExpr : FieldId × Expr → Bytes := encPair encNat encExpr

def decFieldExpr : Dec (FieldId × Expr) := decPair decNat decExpr

theorem lawful_fieldExpr : Lawful encFieldExpr decFieldExpr :=
  lawful_pair lawful_nat lawful_expr

/-- A transition of a workflow (slice 8, its name slice 9): its name, its source state, its target state, its
    guard (an option), then its assignments. -/
def encWfTransition : WfTransition → Bytes :=
  encIso (encPair encStr (encPair encStr (encPair encStr (encPair (encOpt encExpr) (encList encFieldExpr)))))
    (fun tr => (tr.name, tr.source, tr.target, tr.guard, tr.set))

def decWfTransition : Dec WfTransition :=
  decIso (decPair decStr (decPair decStr (decPair decStr (decPair (decOpt decExpr) (decList decFieldExpr)))))
    (fun p => { name := p.1, source := p.2.1, target := p.2.2.1, guard := p.2.2.2.1, set := p.2.2.2.2 })

theorem lawful_wfTransition : Lawful encWfTransition decWfTransition :=
  lawful_iso (lawful_pair lawful_str (lawful_pair lawful_str (lawful_pair lawful_str
      (lawful_pair (lawful_opt lawful_expr) (lawful_list lawful_fieldExpr)))))
    (fun tr => by cases tr; rfl) (fun _ => rfl)

/-- A workflow (slice 8, its id slice 9): its id, its field, its initial states, then its transitions. -/
def encWorkflow : Workflow → Bytes :=
  encIso (encPair encStr (encPair encNat (encPair (encList encStr) (encList encWfTransition))))
    (fun w => (w.id, w.field, w.initial, w.transitions))

def decWorkflow : Dec Workflow :=
  decIso (decPair decStr (decPair decNat (decPair (decList decStr) (decList decWfTransition))))
    (fun p => { id := p.1, field := p.2.1, initial := p.2.2.1, transitions := p.2.2.2 })

theorem lawful_workflow : Lawful encWorkflow decWorkflow :=
  lawful_iso (lawful_pair lawful_str (lawful_pair lawful_nat (lawful_pair (lawful_list lawful_str)
      (lawful_list lawful_wfTransition))))
    (fun w => by cases w; rfl) (fun _ => rfl)

/-- An object type: its schema, its composite constraints, its records, its constraints, then its
    workflows (slice 8). -/
def encObjType : ObjType → Bytes :=
  encIso (encPair (encList encSchemaEntry)
    (encPair (encList (encList encNat))
      (encPair (encList encRecordEntry) (encPair (encList encExpr) (encList encWorkflow)))))
    (fun o => (o.schema, o.uniques, o.records, o.constraints, o.workflows))

def decObjType : Dec ObjType :=
  decIso (decPair (decList decSchemaEntry)
      (decPair (decList (decList decNat))
        (decPair (decList decRecordEntry) (decPair (decList decExpr) (decList decWorkflow)))))
    (fun p => { schema := p.1, uniques := p.2.1, records := p.2.2.1, constraints := p.2.2.2.1,
                workflows := p.2.2.2.2 })

theorem lawful_objType : Lawful encObjType decObjType :=
  lawful_iso (lawful_pair (lawful_list lawful_schemaEntry)
      (lawful_pair (lawful_list (lawful_list lawful_nat))
        (lawful_pair (lawful_list lawful_recordEntry)
          (lawful_pair (lawful_list lawful_expr) (lawful_list lawful_workflow)))))
    (fun o => by cases o; rfl) (fun _ => rfl)

/-- A type of the tenant: its id, then the object type. -/
def encTypeEntry : TypeId × ObjType → Bytes := encPair encNat encObjType

def decTypeEntry : Dec (TypeId × ObjType) := decPair decNat decObjType

theorem lawful_typeEntry : Lawful encTypeEntry decTypeEntry :=
  lawful_pair lawful_nat lawful_objType

/-- A team (slice 7): its label, its members, then the roles it holds. -/
def encTeamDef : TeamDef → Bytes :=
  encIso (encPair encStr (encPair (encList encNat) (encList encNat)))
    (fun d => (d.label, d.members, d.roles))

def decTeamDef : Dec TeamDef :=
  decIso (decPair decStr (decPair (decList decNat) (decList decNat)))
    (fun p => { label := p.1, members := p.2.1, roles := p.2.2 })

theorem lawful_teamDef : Lawful encTeamDef decTeamDef :=
  lawful_iso (lawful_pair lawful_str (lawful_pair (lawful_list lawful_nat) (lawful_list lawful_nat)))
    (fun d => by cases d; rfl) (fun _ => rfl)

/-- A team of the tenant: its id, then the team. -/
def encTeamEntry : TeamId × TeamDef → Bytes := encPair encNat encTeamDef

def decTeamEntry : Dec (TeamId × TeamDef) := decPair decNat decTeamDef

theorem lawful_teamEntry : Lawful encTeamEntry decTeamEntry :=
  lawful_pair lawful_nat lawful_teamDef

/-- An invitation (slice 7): its address, the roles it offers, who issued it, when it expires, then who
    redeemed it and who revoked it, if anyone. -/
def encInvitationDef : InvitationDef → Bytes :=
  encIso (encPair encStr (encPair (encList encNat) (encPair encNat (encPair encNat
    (encPair (encOpt encNat) (encOpt encNat))))))
    (fun d => (d.address, d.roles, d.createdBy, d.expiresAt, d.redeemedBy, d.revokedBy))

def decInvitationDef : Dec InvitationDef :=
  decIso (decPair decStr (decPair (decList decNat) (decPair decNat (decPair decNat
    (decPair (decOpt decNat) (decOpt decNat))))))
    (fun p => { address := p.1, roles := p.2.1, createdBy := p.2.2.1, expiresAt := p.2.2.2.1,
                redeemedBy := p.2.2.2.2.1, revokedBy := p.2.2.2.2.2 })

theorem lawful_invitationDef : Lawful encInvitationDef decInvitationDef :=
  lawful_iso (lawful_pair lawful_str (lawful_pair (lawful_list lawful_nat)
    (lawful_pair lawful_nat (lawful_pair lawful_nat
      (lawful_pair (lawful_opt lawful_nat) (lawful_opt lawful_nat))))))
    (fun d => by cases d; rfl) (fun _ => rfl)

/-- An invitation of the tenant: its id, then the invitation. -/
def encInvitationEntry : InvitationId × InvitationDef → Bytes := encPair encNat encInvitationDef

def decInvitationEntry : Dec (InvitationId × InvitationDef) := decPair decNat decInvitationDef

theorem lawful_invitationEntry : Lawful encInvitationEntry decInvitationEntry :=
  lawful_pair lawful_nat lawful_invitationDef

/-! ## Settings (slice 12)

A level, a type and a subject are one tag byte, or a tag and a number; a definition and an override are
nested products of their fields, as the tenant's. -/

/-- A setting level: `platform` 0, `tenant` 1, `team` 2, `person` 3, one byte. -/
def encLevel : SettingLevel → Bytes
  | .platform => [tag 0]
  | .tenant => [tag 1]
  | .team => [tag 2]
  | .person => [tag 3]

def decLevel : Dec SettingLevel := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.platform, r)
    | 1 => some (.tenant, r)
    | 2 => some (.team, r)
    | 3 => some (.person, r)
    | _ => none

theorem lawful_level : Lawful encLevel decLevel := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encLevel, decLevel, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decLevel] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ 4 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk
      · simp only [decLevel, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLevel, byte_eq_tag b 0 hk]
      · simp only [decLevel, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLevel, byte_eq_tag b 1 hk]
      · simp only [decLevel, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLevel, byte_eq_tag b 2 hk]
      · simp only [decLevel, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLevel, byte_eq_tag b 3 hk]
      · simp only [decLevel] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A setting's type: the flag (tag 0), the whole number (tag 1), the text (tag 2), one byte. -/
def encSettingType : SettingType → Bytes
  | .bool => [tag 0]
  | .int => [tag 1]
  | .str => [tag 2]

def decSettingType : Dec SettingType := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.bool, r)
    | 1 => some (.int, r)
    | 2 => some (.str, r)
    | _ => none

theorem lawful_settingType : Lawful encSettingType decSettingType := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encSettingType, decSettingType, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decSettingType] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ 3 ≤ b.toNat by omega)
        with hk | hk | hk | hk
      · simp only [decSettingType, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encSettingType, byte_eq_tag b 0 hk]
      · simp only [decSettingType, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encSettingType, byte_eq_tag b 1 hk]
      · simp only [decSettingType, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encSettingType, byte_eq_tag b 2 hk]
      · simp only [decSettingType] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A subject: the tenant (tag 0), a team (tag 1, its number), a person (tag 2, its number). -/
def encSubject : SettingSubject → Bytes
  | .tenant => [tag 0]
  | .team t => tag 1 :: encNat t
  | .person p => tag 2 :: encNat p

def decSubject : Dec SettingSubject := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.tenant, r)
    | 1 => match decNat r with
      | some (t, r') => some (.team t, r')
      | none => none
    | 2 => match decNat r with
      | some (p, r') => some (.person p, r')
      | none => none
    | _ => none

theorem lawful_subject : Lawful encSubject decSubject := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | tenant => simp [encSubject, decSubject, tag]
    | team t => simp [encSubject, decSubject, tag, lawful_nat.1 t rest]
    | person p => simp [encSubject, decSubject, tag, lawful_nat.1 p rest]
  · intro bs x rest h
    cases bs with
    | nil => simp [decSubject] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ 3 ≤ b.toNat by omega)
        with hk | hk | hk | hk
      · simp only [decSubject, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encSubject, byte_eq_tag b 0 hk]
      · simp only [decSubject, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨t, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r t r' hn
          simp [encSubject, byte_eq_tag b 1 hk, hr]
      · simp only [decSubject, hk] at h
        cases hn : decNat r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨p, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := lawful_nat.2 r p r' hn
          simp [encSubject, byte_eq_tag b 2 hk, hr]
      · simp only [decSubject] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A setting's allowed values: a closed set (tag 0), or a range of whole numbers (tag 1). -/
def encAllowed : SettingAllowed → Bytes
  | .closed vs => tag 0 :: encList encVal vs
  | .range lo hi => tag 1 :: (encInt lo ++ encInt hi)

def decAllowed : Dec SettingAllowed := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decList decVal r with
      | some (vs, r') => some (.closed vs, r')
      | none => none
    | 1 => match decInt r with
      | some (lo, r1) => match decInt r1 with
        | some (hi, r2) => some (.range lo hi, r2)
        | none => none
      | none => none
    | _ => none

theorem lawful_allowed : Lawful encAllowed decAllowed := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | closed vs => simp [encAllowed, decAllowed, tag, (lawful_list lawful_val).1]
    | range lo hi => simp [encAllowed, decAllowed, tag, lawful_int.1 lo, lawful_int.1 hi, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decAllowed] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ 2 ≤ b.toNat by omega) with hk | hk | hk
      · simp only [decAllowed, hk] at h
        cases hn : decList decVal r with
        | none => simp [hn] at h
        | some q =>
          obtain ⟨vs, r'⟩ := q
          simp only [hn, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have hr := (lawful_list lawful_val).2 r vs r' hn
          simp [encAllowed, byte_eq_tag b 0 hk, hr]
      · simp only [decAllowed, hk] at h
        cases hl : decInt r with
        | none => simp [hl] at h
        | some q =>
          obtain ⟨lo, r1⟩ := q
          simp only [hl] at h
          cases hh : decInt r1 with
          | none => simp [hh] at h
          | some q2 =>
            obtain ⟨hi, r2⟩ := q2
            simp only [hh, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have h1 := lawful_int.2 r lo r1 hl
            have h2 := lawful_int.2 r1 hi r2 hh
            simp [encAllowed, byte_eq_tag b 1 hk, h1, h2, List.append_assoc]
      · simp only [decAllowed] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A setting definition: its name, its type, the levels it opens, its lock, its default and what it allows. -/
def encSettingDef : SettingDef → Bytes :=
  encIso (encPair encStr (encPair encSettingType (encPair (encList encLevel)
      (encPair (encOpt encLevel) (encPair encVal (encOpt encAllowed)))))) (fun d =>
    (d.name, d.ty, d.levels, d.locked, d.default, d.allowed))

def decSettingDef : Dec SettingDef :=
  decIso (decPair decStr (decPair decSettingType (decPair (decList decLevel)
      (decPair (decOpt decLevel) (decPair decVal (decOpt decAllowed)))))) (fun p =>
    { name := p.1, ty := p.2.1, levels := p.2.2.1, locked := p.2.2.2.1, default := p.2.2.2.2.1,
      allowed := p.2.2.2.2.2 })

theorem lawful_settingDef : Lawful encSettingDef decSettingDef :=
  lawful_iso (lawful_pair lawful_str (lawful_pair lawful_settingType (lawful_pair (lawful_list lawful_level)
      (lawful_pair (lawful_opt lawful_level) (lawful_pair lawful_val (lawful_opt lawful_allowed))))))
    (fun d => by cases d; rfl) (fun _ => rfl)

/-- An override: its setting, its subject, its value and its revision. -/
def encOverride : SettingOverride → Bytes :=
  encIso (encPair encStr (encPair encSubject (encPair encVal encNat)))
    (fun o => (o.name, o.subject, o.value, o.revision))

def decOverride : Dec SettingOverride :=
  decIso (decPair decStr (decPair decSubject (decPair decVal decNat)))
    (fun p => { name := p.1, subject := p.2.1, value := p.2.2.1, revision := p.2.2.2 })

theorem lawful_override : Lawful encOverride decOverride :=
  lawful_iso (lawful_pair lawful_str (lawful_pair lawful_subject (lawful_pair lawful_val lawful_nat)))
    (fun o => by cases o; rfl) (fun _ => rfl)

/-- A setting's revision at the tenant: its name and its revision. -/
def encSettingRev : String × Nat → Bytes := encPair encStr encNat

def decSettingRev : Dec (String × Nat) := decPair decStr decNat

theorem lawful_settingRev : Lawful encSettingRev decSettingRev :=
  lawful_pair lawful_str lawful_nat

/-! ## The ledger (slice 13)

A period is one tag byte; an owner and an atom of a policy's condition are a tag and their payload; the records
are nested products of their fields, as the tenant's, with the options and lists of the codec's combinators. The
ledger's state is the last component of a tenant, after its settings. -/

/-- The period codec: one tag byte, `hour` 0 to `month` 3. -/
def encLedgerPeriod : LedgerPeriod → Bytes
  | .hour => [tag 0]
  | .day => [tag 1]
  | .week => [tag 2]
  | .month => [tag 3]

def decLedgerPeriod : Dec LedgerPeriod := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.hour, r)
    | 1 => some (.day, r)
    | 2 => some (.week, r)
    | 3 => some (.month, r)
    | _ => none

theorem lawful_ledgerPeriod : Lawful encLedgerPeriod decLedgerPeriod := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x <;> simp [encLedgerPeriod, decLedgerPeriod, tag]
  · intro bs x rest h
    cases bs with
    | nil => simp [decLedgerPeriod] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ 4 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk
      · simp only [decLedgerPeriod, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLedgerPeriod, byte_eq_tag b 0 hk]
      · simp only [decLedgerPeriod, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLedgerPeriod, byte_eq_tag b 1 hk]
      · simp only [decLedgerPeriod, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLedgerPeriod, byte_eq_tag b 2 hk]
      · simp only [decLedgerPeriod, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLedgerPeriod, byte_eq_tag b 3 hk]
      · simp only [decLedgerPeriod] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- An owner: a person (tag 0) or a pool (tag 1), each followed by its number. -/
def encLedgerOwner : LedgerOwner → Bytes
  | .person p => tag 0 :: encNat p
  | .pool n => tag 1 :: encNat n

def decLedgerOwner : Dec LedgerOwner := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => match decNat r with
      | some (p, r1) => some (.person p, r1)
      | none => none
    | 1 => match decNat r with
      | some (n, r1) => some (.pool n, r1)
      | none => none
    | _ => none

theorem lawful_ledgerOwner : Lawful encLedgerOwner decLedgerOwner := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | person p => simp [encLedgerOwner, decLedgerOwner, tag, lawful_nat.1, List.append_assoc]
    | pool n => simp [encLedgerOwner, decLedgerOwner, tag, lawful_nat.1, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decLedgerOwner] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ 2 ≤ b.toNat by omega) with hk | hk | hk
      · simp only [decLedgerOwner, hk] at h
        cases hd : decNat r with
        | none => simp [hd] at h
        | some p1 =>
          obtain ⟨p, r1⟩ := p1
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have e := lawful_nat.2 r p r1 hd
          simp [encLedgerOwner, byte_eq_tag b 0 hk, e, List.append_assoc]
      · simp only [decLedgerOwner, hk] at h
        cases hd : decNat r with
        | none => simp [hd] at h
        | some p1 =>
          obtain ⟨n, r1⟩ := p1
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have e := lawful_nat.2 r n r1 hd
          simp [encLedgerOwner, byte_eq_tag b 1 hk, e, List.append_assoc]
      · simp only [decLedgerOwner] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- An atom of a policy's condition: the owner is the reader (tag 0), the unit is a number (tag 1), the scope is
    a text (tag 2). -/
def encLedgerAtom : LedgerAtom → Bytes
  | .ownerIsCaller => [tag 0]
  | .unitIs n => tag 1 :: encNat n
  | .scopeIs s => tag 2 :: encStr s

def decLedgerAtom : Dec LedgerAtom := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => some (.ownerIsCaller, r)
    | 1 => match decNat r with
      | some (n, r1) => some (.unitIs n, r1)
      | none => none
    | 2 => match decStr r with
      | some (s, r1) => some (.scopeIs s, r1)
      | none => none
    | _ => none

theorem lawful_ledgerAtom : Lawful encLedgerAtom decLedgerAtom := by
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | ownerIsCaller => simp [encLedgerAtom, decLedgerAtom, tag]
    | unitIs n => simp [encLedgerAtom, decLedgerAtom, tag, lawful_nat.1, List.append_assoc]
    | scopeIs s => simp [encLedgerAtom, decLedgerAtom, tag, lawful_str.1, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decLedgerAtom] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ 3 ≤ b.toNat by omega) with hk | hk | hk | hk
      · simp only [decLedgerAtom, hk, Option.some.injEq, Prod.mk.injEq] at h
        obtain ⟨rfl, rfl⟩ := h
        simp [encLedgerAtom, byte_eq_tag b 0 hk]
      · simp only [decLedgerAtom, hk] at h
        cases hd : decNat r with
        | none => simp [hd] at h
        | some p1 =>
          obtain ⟨n, r1⟩ := p1
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have e := lawful_nat.2 r n r1 hd
          simp [encLedgerAtom, byte_eq_tag b 1 hk, e, List.append_assoc]
      · simp only [decLedgerAtom, hk] at h
        cases hd : decStr r with
        | none => simp [hd] at h
        | some p1 =>
          obtain ⟨s, r1⟩ := p1
          simp only [hd, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have e := lawful_str.2 r s r1 hd
          simp [encLedgerAtom, byte_eq_tag b 2 hk, e, List.append_assoc]
      · simp only [decLedgerAtom] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- An expiry: its period and its number of periods. -/
def encLedgerExpiry : LedgerExpiry → Bytes :=
  encIso (encPair encLedgerPeriod encNat) (fun e => (e.period, e.periods))

def decLedgerExpiry : Dec LedgerExpiry :=
  decIso (decPair decLedgerPeriod decNat) (fun p => { period := p.1, periods := p.2 })

theorem lawful_ledgerExpiry : Lawful encLedgerExpiry decLedgerExpiry :=
  lawful_iso (lawful_pair lawful_ledgerPeriod lawful_nat) (fun e => by cases e; rfl) (fun _ => rfl)

/-- A unit declaration: its number and its expiry. -/
def encLedgerUnitDecl : LedgerUnitDecl → Bytes :=
  encIso (encPair encNat (encOpt encLedgerExpiry)) (fun u => (u.unit, u.expiry))

def decLedgerUnitDecl : Dec LedgerUnitDecl :=
  decIso (decPair decNat (decOpt decLedgerExpiry)) (fun p => { unit := p.1, expiry := p.2 })

theorem lawful_ledgerUnitDecl : Lawful encLedgerUnitDecl decLedgerUnitDecl :=
  lawful_iso (lawful_pair lawful_nat (lawful_opt lawful_ledgerExpiry)) (fun u => by cases u; rfl) (fun _ => rfl)

/-- A limit: its unit, its owner, its scope, the floor, the cap, the cap per period with its period, and whether the
    cap counts additions only. -/
def encLedgerLimit : LedgerLimit → Bytes :=
  encIso (encPair encNat (encPair (encOpt encLedgerOwner) (encPair encStr (encPair (encOpt encInt)
      (encPair (encOpt encInt) (encPair (encOpt (encPair encNat encLedgerPeriod)) (encOpt encBool)))))))
    (fun l => (l.unit, l.owner, l.scope, l.floor, l.cap, l.periodCap, l.additionsOnly))

def decLedgerLimit : Dec LedgerLimit :=
  decIso (decPair decNat (decPair (decOpt decLedgerOwner) (decPair decStr (decPair (decOpt decInt)
      (decPair (decOpt decInt) (decPair (decOpt (decPair decNat decLedgerPeriod)) (decOpt decBool)))))))
    (fun p =>
      { unit := p.1, owner := p.2.1, scope := p.2.2.1, floor := p.2.2.2.1, cap := p.2.2.2.2.1,
        periodCap := p.2.2.2.2.2.1, additionsOnly := p.2.2.2.2.2.2 })

theorem lawful_ledgerLimit : Lawful encLedgerLimit decLedgerLimit :=
  lawful_iso (lawful_pair lawful_nat (lawful_pair (lawful_opt lawful_ledgerOwner) (lawful_pair lawful_str
      (lawful_pair (lawful_opt lawful_int) (lawful_pair (lawful_opt lawful_int)
        (lawful_pair (lawful_opt (lawful_pair lawful_nat lawful_ledgerPeriod)) (lawful_opt lawful_bool)))))))
    (fun l => by cases l; rfl) (fun _ => rfl)

/-- The declarations of a ledger: its units, its pools and its unit limits. -/
def encLedgerDecls : LedgerDecls → Bytes :=
  encIso (encPair (encList encLedgerUnitDecl) (encPair (encList encNat) (encList encLedgerLimit)))
    (fun d => (d.units, d.pools, d.limits))

def decLedgerDecls : Dec LedgerDecls :=
  decIso (decPair (decList decLedgerUnitDecl) (decPair (decList decNat) (decList decLedgerLimit)))
    (fun p => { units := p.1, pools := p.2.1, limits := p.2.2 })

theorem lawful_ledgerDecls : Lawful encLedgerDecls decLedgerDecls :=
  lawful_iso (lawful_pair (lawful_list lawful_ledgerUnitDecl) (lawful_pair (lawful_list lawful_nat)
      (lawful_list lawful_ledgerLimit)))
    (fun d => by cases d; rfl) (fun _ => rfl)

/-- A bucket of expiring credit: its period's start and the credit left in it. -/
def encLedgerBucket : LedgerBucket → Bytes :=
  encIso (encPair encInt encNat) (fun b => (b.periodStart, b.remaining))

def decLedgerBucket : Dec LedgerBucket :=
  decIso (decPair decInt decNat) (fun p => { periodStart := p.1, remaining := p.2 })

theorem lawful_ledgerBucket : Lawful encLedgerBucket decLedgerBucket :=
  lawful_iso (lawful_pair lawful_int lawful_nat) (fun b => by cases b; rfl) (fun _ => rfl)

/-- The credits of one period of a header: the period, the start of the period and the credits. -/
def encLedgerPeriodTotal : LedgerPeriodTotal → Bytes :=
  encIso (encPair encLedgerPeriod (encPair encInt encNat)) (fun x => (x.period, x.start, x.credits))

def decLedgerPeriodTotal : Dec LedgerPeriodTotal :=
  decIso (decPair decLedgerPeriod (decPair decInt decNat)) (fun p =>
    { period := p.1, start := p.2.1, credits := p.2.2 })

theorem lawful_ledgerPeriodTotal : Lawful encLedgerPeriodTotal decLedgerPeriodTotal :=
  lawful_iso (lawful_pair lawful_ledgerPeriod (lawful_pair lawful_int lawful_nat))
    (fun x => by cases x; rfl) (fun _ => rfl)

/-- An account's header: its owner, unit and scope, its balance, its last sequence number, the credits of its
    periods and its buckets. -/
def encLedgerAccount : LedgerAccount → Bytes :=
  encIso (encPair encLedgerOwner (encPair encNat (encPair encStr (encPair encInt (encPair encNat
      (encPair (encList encLedgerPeriodTotal) (encList encLedgerBucket))))))) (fun a =>
    (a.owner, a.unit, a.scope, a.balance, a.lastSeq, a.periods, a.buckets))

def decLedgerAccount : Dec LedgerAccount :=
  decIso (decPair decLedgerOwner (decPair decNat (decPair decStr (decPair decInt (decPair decNat
      (decPair (decList decLedgerPeriodTotal) (decList decLedgerBucket))))))) (fun p =>
    { owner := p.1, unit := p.2.1, scope := p.2.2.1, balance := p.2.2.2.1, lastSeq := p.2.2.2.2.1,
      periods := p.2.2.2.2.2.1, buckets := p.2.2.2.2.2.2 })

theorem lawful_ledgerAccount : Lawful encLedgerAccount decLedgerAccount :=
  lawful_iso (lawful_pair lawful_ledgerOwner (lawful_pair lawful_nat (lawful_pair lawful_str
      (lawful_pair lawful_int (lawful_pair lawful_nat (lawful_pair (lawful_list lawful_ledgerPeriodTotal)
        (lawful_list lawful_ledgerBucket)))))))
    (fun a => by cases a; rfl) (fun _ => rfl)

/-- A journal entry: its account (owner, unit, scope), its number, its amount, its key, its reason, the principal
    that caused it, its time and the entry it corrects. -/
def encLedgerEntry : LedgerEntry → Bytes :=
  encIso (encPair encLedgerOwner (encPair encNat (encPair encStr (encPair encNat (encPair encInt (encPair encStr (encPair encStr (encPair (encOpt encNat) (encPair encInt ((encOpt encNat))))))))))) (fun e =>
    (e.owner, e.unit, e.scope, e.sequence, e.amount, e.key, e.reason, e.causedBy, e.time, e.corrects))

def decLedgerEntry : Dec LedgerEntry :=
  decIso (decPair decLedgerOwner (decPair decNat (decPair decStr (decPair decNat (decPair decInt (decPair decStr (decPair decStr (decPair (decOpt decNat) (decPair decInt ((decOpt decNat))))))))))) (fun p =>
    { owner := p.1, unit := p.2.1, scope := p.2.2.1, sequence := p.2.2.2.1, amount := p.2.2.2.2.1,
      key := p.2.2.2.2.2.1, reason := p.2.2.2.2.2.2.1, causedBy := p.2.2.2.2.2.2.2.1,
      time := p.2.2.2.2.2.2.2.2.1, corrects := p.2.2.2.2.2.2.2.2.2 })

theorem lawful_ledgerEntry : Lawful encLedgerEntry decLedgerEntry :=
  lawful_iso (lawful_pair lawful_ledgerOwner (lawful_pair lawful_nat (lawful_pair lawful_str (lawful_pair lawful_nat (lawful_pair lawful_int (lawful_pair lawful_str (lawful_pair lawful_str (lawful_pair (lawful_opt lawful_nat) (lawful_pair lawful_int ((lawful_opt lawful_nat)))))))))))
    (fun e => by cases e; rfl) (fun _ => rfl)

/-- The ledger's state: its accounts, its journal and its account limits. -/
def encLedgerState : LedgerState → Bytes :=
  encIso (encPair (encList encLedgerAccount) (encPair (encList encLedgerEntry) (encList encLedgerLimit)))
    (fun s => (s.accounts, s.journal, s.limits))

def decLedgerState : Dec LedgerState :=
  decIso (decPair (decList decLedgerAccount) (decPair (decList decLedgerEntry) (decList decLedgerLimit)))
    (fun p => { accounts := p.1, journal := p.2.1, limits := p.2.2 })

theorem lawful_ledgerState : Lawful encLedgerState decLedgerState :=
  lawful_iso (lawful_pair (lawful_list lawful_ledgerAccount) (lawful_pair (lawful_list lawful_ledgerEntry)
      (lawful_list lawful_ledgerLimit)))
    (fun s => by cases s; rfl) (fun _ => rfl)

/-- A policy over the ledger: its role, its unit (or none) and its atoms. -/
def encLedgerPolicy : LedgerPolicy → Bytes :=
  encIso (encPair encNat (encPair (encOpt encNat) (encList encLedgerAtom))) (fun l => (l.role, l.unit, l.conds))

def decLedgerPolicy : Dec LedgerPolicy :=
  decIso (decPair decNat (decPair (decOpt decNat) (decList decLedgerAtom))) (fun p =>
    { role := p.1, unit := p.2.1, conds := p.2.2 })

theorem lawful_ledgerPolicy : Lawful encLedgerPolicy decLedgerPolicy :=
  lawful_iso (lawful_pair lawful_nat (lawful_pair (lawful_opt lawful_nat) (lawful_list lawful_ledgerAtom)))
    (fun l => by cases l; rfl) (fun _ => rfl)

/-- A posting of a `PostLedger` intent: its owner, unit, amount, key, reason, the entry it corrects and its scope. -/
def encLedgerPosting : LedgerPosting → Bytes :=
  encIso (encPair encLedgerOwner (encPair encNat (encPair encInt (encPair encStr (encPair encStr
      (encPair (encOpt encNat) encStr))))))
    (fun q => (q.owner, q.unit, q.amount, q.key, q.reason, q.corrects, q.scope))

def decLedgerPosting : Dec LedgerPosting :=
  decIso (decPair decLedgerOwner (decPair decNat (decPair decInt (decPair decStr (decPair decStr
      (decPair (decOpt decNat) decStr)))))) (fun p =>
    { owner := p.1, unit := p.2.1, amount := p.2.2.1, key := p.2.2.2.1, reason := p.2.2.2.2.1,
      corrects := p.2.2.2.2.2.1, scope := p.2.2.2.2.2.2 })

theorem lawful_ledgerPosting : Lawful encLedgerPosting decLedgerPosting :=
  lawful_iso (lawful_pair lawful_ledgerOwner (lawful_pair lawful_nat (lawful_pair lawful_int (lawful_pair lawful_str
      (lawful_pair lawful_str (lawful_pair (lawful_opt lawful_nat) lawful_str))))))
    (fun q => by cases q; rfl) (fun _ => rfl)

/-- The tenant's fields as one nested product: its object types, members, grants, policies, next id,
    limits, the roles its schema defines (slice 7), its teams and its invitations, then (slice 9) its
    revision, its event sequence and its schema version. -/
def encTenantBody : List (TypeId × ObjType) × List (PrincipalId × List RoleId) ×
    List (RoleId × Grant) × List Policy × Nat × Limits × List RoleId ×
    List (TeamId × TeamDef) × List (InvitationId × InvitationDef) × Nat × Nat × Nat ×
    List SettingDef × List SettingOverride × List (String × Nat) ×
    Option LedgerDecls × LedgerState × List LedgerPolicy → Bytes :=
  encPair (encList encTypeEntry) (encPair (encList encMember) (encPair (encList encGrantEntry) (encPair (encList encPolicy) (encPair encNat (encPair encLimits (encPair (encList encNat) (encPair (encList encTeamEntry) (encPair (encList encInvitationEntry) (encPair encNat (encPair encNat (encPair encNat (encPair (encList encSettingDef) (encPair (encList encOverride) (encPair (encList encSettingRev) (encPair (encOpt encLedgerDecls) (encPair encLedgerState ((encList encLedgerPolicy))))))))))))))))))

def decTenantBody :
    Dec (List (TypeId × ObjType) × List (PrincipalId × List RoleId) ×
      List (RoleId × Grant) × List Policy × Nat × Limits × List RoleId ×
      List (TeamId × TeamDef) × List (InvitationId × InvitationDef) × Nat × Nat × Nat ×
      List SettingDef × List SettingOverride × List (String × Nat) ×
      Option LedgerDecls × LedgerState × List LedgerPolicy) :=
  decPair (decList decTypeEntry) (decPair (decList decMember) (decPair (decList decGrantEntry) (decPair (decList decPolicy) (decPair decNat (decPair decLimits (decPair (decList decNat) (decPair (decList decTeamEntry) (decPair (decList decInvitationEntry) (decPair decNat (decPair decNat (decPair decNat (decPair (decList decSettingDef) (decPair (decList decOverride) (decPair (decList decSettingRev) (decPair (decOpt decLedgerDecls) (decPair decLedgerState ((decList decLedgerPolicy))))))))))))))))))

theorem lawful_tenantBody : Lawful encTenantBody decTenantBody :=
  lawful_pair (lawful_list lawful_typeEntry) (lawful_pair (lawful_list lawful_member) (lawful_pair (lawful_list lawful_grantEntry) (lawful_pair (lawful_list lawful_policy) (lawful_pair lawful_nat (lawful_pair lawful_limits (lawful_pair (lawful_list lawful_nat) (lawful_pair (lawful_list lawful_teamEntry) (lawful_pair (lawful_list lawful_invitationEntry) (lawful_pair lawful_nat (lawful_pair lawful_nat (lawful_pair lawful_nat (lawful_pair (lawful_list lawful_settingDef) (lawful_pair (lawful_list lawful_override) (lawful_pair (lawful_list lawful_settingRev) (lawful_pair (lawful_opt lawful_ledgerDecls) (lawful_pair lawful_ledgerState ((lawful_list lawful_ledgerPolicy))))))))))))))))))

def encTenant : Tenant → Bytes :=
  encIso encTenantBody
    (fun t => (t.types, t.members, t.grants, t.policies, t.nextId, t.limits, t.roles, t.teams,
      t.invitations, t.revision, t.eventSeq, t.schemaVersion, t.settingDefs, t.overrides, t.settingRevs,
      t.ledgerDecls, t.ledger, t.ledgerPolicies))

def decTenant : Dec Tenant :=
  decIso decTenantBody fun p =>
    { types := p.1, members := p.2.1, grants := p.2.2.1, policies := p.2.2.2.1, nextId := p.2.2.2.2.1,
      limits := p.2.2.2.2.2.1, roles := p.2.2.2.2.2.2.1, teams := p.2.2.2.2.2.2.2.1,
      invitations := p.2.2.2.2.2.2.2.2.1, revision := p.2.2.2.2.2.2.2.2.2.1,
      eventSeq := p.2.2.2.2.2.2.2.2.2.2.1, schemaVersion := p.2.2.2.2.2.2.2.2.2.2.2.1,
      settingDefs := p.2.2.2.2.2.2.2.2.2.2.2.2.1, overrides := p.2.2.2.2.2.2.2.2.2.2.2.2.2.1,
      settingRevs := p.2.2.2.2.2.2.2.2.2.2.2.2.2.2.1,
      ledgerDecls := p.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2.1,
      ledger := p.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2.1,
      ledgerPolicies := p.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2.2 }

theorem lawful_tenant : Lawful encTenant decTenant :=
  lawful_iso lawful_tenantBody (fun t => by cases t; rfl) (fun _ => rfl)

/-! ## Intents, contexts and results -/

/-- The payload of a constructor is decoded and mapped into the sum. -/
theorem dec_ctor_enc {α β : Type} {e : β → Bytes} {d : Dec β} (h : Lawful e d) (f : β → α)
    (x : β) (rest : Bytes) : decIso d f (e x ++ rest) = some (f x, rest) := by
  unfold decIso
  rw [h.1 x rest]

/-- Whatever a constructor's decoder reads is the encoding of its payload. -/
theorem dec_ctor_inv {α β : Type} {e : β → Bytes} {d : Dec β} (h : Lawful e d) (f : β → α)
    (bs : Bytes) (y : α) (rest : Bytes) (hd : decIso d f bs = some (y, rest)) :
    ∃ x, y = f x ∧ e x ++ rest = bs := by
  unfold decIso at hd
  cases hx : d bs with
  | none => simp [hx] at hd
  | some p =>
    obtain ⟨x, r⟩ := p
    simp only [hx, Option.some.injEq, Prod.mk.injEq] at hd
    obtain ⟨rfl, rfl⟩ := hd
    exact ⟨x, rfl, (h.2 bs x r hx).symm⟩

/-- A membership intent (slice 7): tags `0` assign a role, `1` revoke a role, `2` define a team, `3`
    give a team a role, `4` take a team's role away, `5` add a team member, `6` remove one, `7` create an
    invitation, `8` revoke one, `9` redeem one; each is followed by its fields in declaration order. -/
def encMemberIntent : MemberIntent → Bytes
  | .assignRole p r => tag 0 :: encPair encNat encNat (p, r)
  | .revokeRole p r => tag 1 :: encPair encNat encNat (p, r)
  | .defineTeam t l => tag 2 :: encPair encNat encStr (t, l)
  | .assignTeamRole t r => tag 3 :: encPair encNat encNat (t, r)
  | .revokeTeamRole t r => tag 4 :: encPair encNat encNat (t, r)
  | .addTeamMember t p => tag 5 :: encPair encNat encNat (t, p)
  | .removeTeamMember t p => tag 6 :: encPair encNat encNat (t, p)
  | .createInvitation i a rs n =>
    tag 7 :: encPair encNat (encPair encStr (encPair (encList encNat) encNat)) (i, a, rs, n)
  | .revokeInvitation i => tag 8 :: encNat i
  | .redeemInvitation i n v =>
    tag 9 :: encPair encNat (encPair encNat (encOpt encStr)) (i, n, v)

def decMemberIntent : Dec MemberIntent := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => decIso (decPair decNat decNat) (fun x => MemberIntent.assignRole x.1 x.2) r
    | 1 => decIso (decPair decNat decNat) (fun x => MemberIntent.revokeRole x.1 x.2) r
    | 2 => decIso (decPair decNat decStr) (fun x => MemberIntent.defineTeam x.1 x.2) r
    | 3 => decIso (decPair decNat decNat) (fun x => MemberIntent.assignTeamRole x.1 x.2) r
    | 4 => decIso (decPair decNat decNat) (fun x => MemberIntent.revokeTeamRole x.1 x.2) r
    | 5 => decIso (decPair decNat decNat) (fun x => MemberIntent.addTeamMember x.1 x.2) r
    | 6 => decIso (decPair decNat decNat) (fun x => MemberIntent.removeTeamMember x.1 x.2) r
    | 7 => decIso (decPair decNat (decPair decStr (decPair (decList decNat) decNat)))
        (fun x => MemberIntent.createInvitation x.1 x.2.1 x.2.2.1 x.2.2.2) r
    | 8 => decIso decNat (fun x => MemberIntent.revokeInvitation x) r
    | 9 => decIso (decPair decNat (decPair decNat (decOpt decStr)))
        (fun x => MemberIntent.redeemInvitation x.1 x.2.1 x.2.2) r
    | _ => none

theorem lawful_memberIntent : Lawful encMemberIntent decMemberIntent := by
  have hnn := lawful_pair lawful_nat lawful_nat
  have hns := lawful_pair lawful_nat lawful_str
  have hci := lawful_pair lawful_nat (lawful_pair lawful_str
    (lawful_pair (lawful_list lawful_nat) lawful_nat))
  have hri := lawful_pair lawful_nat (lawful_pair lawful_nat (lawful_opt lawful_str))
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | assignRole p r => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | revokeRole p r => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | defineTeam t l => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hns, List.append_assoc]
    | assignTeamRole t r => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | revokeTeamRole t r => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | addTeamMember t p => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | removeTeamMember t p => simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hnn, List.append_assoc]
    | createInvitation i a rs n =>
      simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hci, List.append_assoc]
    | revokeInvitation i =>
      simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc lawful_nat, List.append_assoc]
    | redeemInvitation i n v =>
      simp [encMemberIntent, decMemberIntent, tag, dec_ctor_enc hri, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decMemberIntent] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ b.toNat = 6 ∨ b.toNat = 7 ∨ b.toNat = 8 ∨ b.toNat = 9 ∨ 10 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 0 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 1 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hns _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 2 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 3 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 4 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 5 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 6 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hci _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 7 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv lawful_nat _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 8 hk, ← hy]
      · simp only [decMemberIntent, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hri _ r x rest h
        simp [encMemberIntent, byte_eq_tag b 9 hk, ← hy]
      · simp only [decMemberIntent] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- A change of a schema change in general (slice 11): tag `0` defines a field (its object type, its id, its
    definition), `1` removes a field, `2` defines an object type, `3` removes one (and says whether it deletes its
    records). -/
def encChange : Change → Bytes
  | .defineField ty f d => tag 0 :: encPair encNat (encPair encNat encFieldDef) (ty, f, d)
  | .removeField ty f => tag 1 :: encPair encNat encNat (ty, f)
  | .defineType ty => tag 2 :: encNat ty
  | .removeType ty del => tag 3 :: encPair encNat encBool (ty, del)

def decChange : Dec Change := fun bs =>
  match bs with
  | [] => none
  | b :: r =>
    match b.toNat with
    | 0 => decIso (decPair decNat (decPair decNat decFieldDef))
        (fun x => Change.defineField x.1 x.2.1 x.2.2) r
    | 1 => decIso (decPair decNat decNat) (fun x => Change.removeField x.1 x.2) r
    | 2 => decIso decNat (fun x => Change.defineType x) r
    | 3 => decIso (decPair decNat decBool) (fun x => Change.removeType x.1 x.2) r
    | _ => none

theorem lawful_change : Lawful encChange decChange := by
  have hdf := lawful_pair lawful_nat (lawful_pair lawful_nat lawful_fieldDef)
  have hnn := lawful_pair lawful_nat lawful_nat
  have hnb := lawful_pair lawful_nat lawful_bool
  refine ⟨?_, ?_⟩
  · intro x rest
    cases x with
    | defineField ty f d => simp [encChange, decChange, tag, dec_ctor_enc hdf, List.append_assoc]
    | removeField ty f => simp [encChange, decChange, tag, dec_ctor_enc hnn, List.append_assoc]
    | defineType ty => simp [encChange, decChange, tag, dec_ctor_enc lawful_nat, List.append_assoc]
    | removeType ty del => simp [encChange, decChange, tag, dec_ctor_enc hnb, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decChange] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ 4 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk
      · simp only [decChange, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hdf _ r x rest h
        simp [encChange, byte_eq_tag b 0 hk, ← hy]
      · simp only [decChange, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnn _ r x rest h
        simp [encChange, byte_eq_tag b 1 hk, ← hy]
      · simp only [decChange, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv lawful_nat _ r x rest h
        simp [encChange, byte_eq_tag b 2 hk, ← hy]
      · simp only [decChange, hk] at h
        obtain ⟨y, rfl, hy⟩ := dec_ctor_inv hnb _ r x rest h
        simp [encChange, byte_eq_tag b 3 hk, ← hy]
      · simp only [decChange] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- An intent: tag `0` create, `1` update, `2` delete, `3` set required, `4` set unique, `5`
    define a composite unique constraint. Each names its object type first, right after the tag,
    as a natural. Slice 7: `6` creates a tenant (its owner, then its name) and `7` is a membership
    intent (`encMemberIntent`). Slice 11: `8` is a schema change in general (the version it expects, whether it is
    strict, its changes). -/
def encIntent : Intent → Bytes
  | .create ty fs => tag 0 :: (encNat ty ++ encList encFieldVal fs)
  | .update ty id fs ev => tag 1 :: (encNat ty ++ encNat id ++ encList encFieldVal fs ++ encOpt encNat ev)
  | .delete ty id ev => tag 2 :: (encNat ty ++ encNat id ++ encOpt encNat ev)
  | .setRequired ty f s => tag 3 :: (encNat ty ++ encNat f ++ encBool s)
  | .setUnique ty f s => tag 4 :: (encNat ty ++ encNat f ++ encBool s)
  | .defineUnique ty fs => tag 5 :: (encNat ty ++ encList encNat fs)
  | .createTenant owner name => tag 6 :: (encNat owner ++ encStr name)
  | .member j => tag 7 :: encMemberIntent j
  | .changeSchema ev s cs => tag 8 :: (encOpt encNat ev ++ encBool s ++ encList encChange cs)
  | .writeSetting n lvl team v ev =>
    tag 9 :: (encStr n ++ encLevel lvl ++ encOpt encNat team ++ encOpt encVal v ++ encNat ev)
  | .postLedger now ps => tag 10 :: (encInt now ++ encList encLedgerPosting ps)
  | .expireLedger now u c => tag 11 :: (encInt now ++ encNat u ++ encInt c)

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
    | 6 => match decNat r with
      | some (owner, r1) => match decStr r1 with
        | some (name, r2) => some (.createTenant owner name, r2)
        | none => none
      | none => none
    | 7 => match decMemberIntent r with
      | some (j, r1) => some (.member j, r1)
      | none => none
    | 8 => match decOpt decNat r with
      | some (ev, r1) => match decBool r1 with
        | some (s, r2) => match decList decChange r2 with
          | some (cs, r3) => some (.changeSchema ev s cs, r3)
          | none => none
        | none => none
      | none => none
    | 9 => match decStr r with
      | some (n, r1) => match decLevel r1 with
        | some (lvl, r2) => match decOpt decNat r2 with
          | some (team, r3) => match decOpt decVal r3 with
            | some (v, r4) => match decNat r4 with
              | some (ev, r5) => some (.writeSetting n lvl team v ev, r5)
              | none => none
            | none => none
          | none => none
        | none => none
      | none => none
    | 10 => match decInt r with
      | some (now, r1) => match decList decLedgerPosting r1 with
        | some (ps, r2) => some (.postLedger now ps, r2)
        | none => none
      | none => none
    | 11 => match decInt r with
      | some (now, r1) => match decNat r1 with
        | some (u, r2) => match decInt r2 with
          | some (c, r3) => some (.expireLedger now u c, r3)
          | none => none
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
    | createTenant owner name =>
      simp [encIntent, decIntent, tag, lawful_nat.1, lawful_str.1, List.append_assoc]
    | member j =>
      simp [encIntent, decIntent, tag, lawful_memberIntent.1]
    | changeSchema ev s cs =>
      simp [encIntent, decIntent, tag, (lawful_opt lawful_nat).1, lawful_bool.1,
        (lawful_list lawful_change).1, List.append_assoc]
    | writeSetting n lvl team v ev =>
      simp [encIntent, decIntent, tag, lawful_str.1, lawful_level.1, (lawful_opt lawful_nat).1,
        (lawful_opt lawful_val).1, lawful_nat.1, List.append_assoc]
    | postLedger now ps =>
      simp [encIntent, decIntent, tag, lawful_int.1, (lawful_list lawful_ledgerPosting).1, List.append_assoc]
    | expireLedger now u c =>
      simp [encIntent, decIntent, tag, lawful_int.1, lawful_nat.1, List.append_assoc]
  · intro bs x rest h
    cases bs with
    | nil => simp [decIntent] at h
    | cons b r =>
      rcases (show b.toNat = 0 ∨ b.toNat = 1 ∨ b.toNat = 2 ∨ b.toNat = 3 ∨ b.toNat = 4 ∨
          b.toNat = 5 ∨ b.toNat = 6 ∨ b.toNat = 7 ∨ b.toNat = 8 ∨ b.toNat = 9 ∨ b.toNat = 10 ∨
          b.toNat = 11 ∨ 12 ≤ b.toNat by omega)
        with hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk | hk
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
      · simp only [decIntent, hk] at h
        cases hd1 : decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨owner, r1⟩ := p1
          cases hd2 : decStr r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨name, r2⟩ := p2
            simp only [hd1, hd2, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have e1 := lawful_nat.2 r owner r1 hd1
            have e2 := lawful_str.2 r1 name r2 hd2
            simp [encIntent, byte_eq_tag b 6 hk, e1, e2, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decMemberIntent r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨j, r1⟩ := p1
          simp only [hd1, Option.some.injEq, Prod.mk.injEq] at h
          obtain ⟨rfl, rfl⟩ := h
          have e1 := lawful_memberIntent.2 r j r1 hd1
          simp [encIntent, byte_eq_tag b 7 hk, e1]
      · simp only [decIntent, hk] at h
        cases hd1 : decOpt decNat r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨ev, r1⟩ := p1
          cases hd2 : decBool r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨s, r2⟩ := p2
            cases hd3 : decList decChange r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨cs, r3⟩ := p3
              simp only [hd1, hd2, hd3, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have e1 := (lawful_opt lawful_nat).2 r ev r1 hd1
              have e2 := lawful_bool.2 r1 s r2 hd2
              have e3 := (lawful_list lawful_change).2 r2 cs r3 hd3
              simp [encIntent, byte_eq_tag b 8 hk, e1, e2, e3, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decStr r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨n, r1⟩ := p1
          cases hd2 : decLevel r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨lvl, r2⟩ := p2
            cases hd3 : decOpt decNat r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨team, r3⟩ := p3
              cases hd4 : decOpt decVal r3 with
              | none => simp [hd1, hd2, hd3, hd4] at h
              | some p4 =>
                obtain ⟨v, r4⟩ := p4
                cases hd5 : decNat r4 with
                | none => simp [hd1, hd2, hd3, hd4, hd5] at h
                | some p5 =>
                  obtain ⟨ev, r5⟩ := p5
                  simp only [hd1, hd2, hd3, hd4, hd5, Option.some.injEq, Prod.mk.injEq] at h
                  obtain ⟨rfl, rfl⟩ := h
                  have e1 := lawful_str.2 r n r1 hd1
                  have e2 := lawful_level.2 r1 lvl r2 hd2
                  have e3 := (lawful_opt lawful_nat).2 r2 team r3 hd3
                  have e4 := (lawful_opt lawful_val).2 r3 v r4 hd4
                  have e5 := lawful_nat.2 r4 ev r5 hd5
                  simp [encIntent, byte_eq_tag b 9 hk, e1, e2, e3, e4, e5, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decInt r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨now, r1⟩ := p1
          cases hd2 : decList decLedgerPosting r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨ps, r2⟩ := p2
            simp only [hd1, hd2, Option.some.injEq, Prod.mk.injEq] at h
            obtain ⟨rfl, rfl⟩ := h
            have e1 := lawful_int.2 r now r1 hd1
            have e2 := (lawful_list lawful_ledgerPosting).2 r1 ps r2 hd2
            simp [encIntent, byte_eq_tag b 10 hk, e1, e2, List.append_assoc]
      · simp only [decIntent, hk] at h
        cases hd1 : decInt r with
        | none => simp [hd1] at h
        | some p1 =>
          obtain ⟨now, r1⟩ := p1
          cases hd2 : decNat r1 with
          | none => simp [hd1, hd2] at h
          | some p2 =>
            obtain ⟨u, r2⟩ := p2
            cases hd3 : decInt r2 with
            | none => simp [hd1, hd2, hd3] at h
            | some p3 =>
              obtain ⟨c, r3⟩ := p3
              simp only [hd1, hd2, hd3, Option.some.injEq, Prod.mk.injEq] at h
              obtain ⟨rfl, rfl⟩ := h
              have e1 := lawful_int.2 r now r1 hd1
              have e2 := lawful_nat.2 r1 u r2 hd2
              have e3 := lawful_int.2 r2 c r3 hd3
              simp [encIntent, byte_eq_tag b 11 hk, e1, e2, e3, List.append_assoc]
      · simp only [decIntent] at h
        exfalso
        repeat' split at h
        all_goals first | omega | simp at h

/-- The context of a step: the tenant and the acting principal. -/
def encCtx : Ctx → Bytes := encIso (encPair encNat encPrincipal) (fun c => (c.tenant, c.principal))

def decCtx : Dec Ctx :=
  decIso (decPair decNat decPrincipal) (fun p => { tenant := p.1, principal := p.2 })

theorem lawful_ctx : Lawful encCtx decCtx :=
  lawful_iso (lawful_pair lawful_nat lawful_principal) (fun c => by cases c; rfl) (fun _ => rfl)

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
