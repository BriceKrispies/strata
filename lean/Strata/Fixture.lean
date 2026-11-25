/-
The fixture the scenario runs start from: two tenants, each with an owner, an editor and two
writers, and two object types. The spec executable (`Main.lean`) and the executable kernel
(`Strata/Impl/World.lean`) both start from `initialWorld`, so the fixture is part of the library
and its facts are audited.
-/

import Strata.Model

namespace Strata

/-- The object type every `op` without a `type` names (id `1`, the "item" type). Its fields:
    `0` an optional list of references to items that restricts the delete of the item;
    `1` a required string; `2` an optional integer; `3` an optional boolean;
    `4` an optional item reference cleared when its target is deleted (`set-null`);
    `5` an optional item reference that restricts the delete (`restrict`);
    `6` an optional integer whose default is the constant `7`;
    `7` an optional list of strings;
    `8` an optional list of item references that loses an element when its target is deleted;
    `10` an optional string that is unique (slice 3);
    `11` an optional reference to a part that restricts the delete of the part;
    `9` an optional string whose default is the caller's principal (slice 5);
    `12` an optional reference to a part cleared when the part is deleted (`set-null`). -/
def itemSchema : Schema :=
  [(0, { ty := .list (.ref 1 .restrict), required := false }),
   (1, { ty := .str, required := true }), (2, { ty := .int, required := false }),
   (3, { ty := .bool, required := false }),
   (4, { ty := .ref 1 .setNull, required := false }),
   (5, { ty := .ref 1 .restrict, required := false }),
   (6, { ty := .int, required := false, default := .lit (.int 7) }),
   (7, { ty := .list .str, required := false }),
   (8, { ty := .list (.ref 1 .setNull), required := false }),
   (9, { ty := .str, required := false, default := .principal }),
   (10, { ty := .str, required := false, unique := true }),
   (11, { ty := .ref 2 .restrict, required := false }),
   (12, { ty := .ref 2 .setNull, required := false })]

/-- The constraints of the item type, in id order (slice 5). The first, on field `2`, is violated by a
    negative integer; the second, on field `1`, by the name `forbidden`. Both are statically well typed
    (the kernel's schema check refuses a comparison of two kinds), so neither can fail to evaluate. -/
def itemConstraints : List Expr :=
  [.or (.isNull (.field .record 2)) (.cmp .ge (.field .record 2) (.lit (.int 0))),
   .cmp .ne (.field .record 1) (.lit (.str "forbidden"))]

/-- The second object type, "part" (id `2`), whose references point into items and back:
    `1` a reference to an item that restricts its delete; `2` a reference to an item cleared when
    it is deleted (`set-null`); `3` a list of item references, each cleared (`set-null`);
    `4` an optional integer; `5` an optional boolean whose default is whether the caller is `p100`
    (an expression default). -/
def partSchema : Schema :=
  [(1, { ty := .ref 1 .restrict, required := false }),
   (2, { ty := .ref 1 .setNull, required := false }),
   (3, { ty := .list (.ref 1 .setNull), required := false }),
   (4, { ty := .int, required := false }),
   (5, { ty := .bool, required := false,
         default := .cmp .eq .principal (.lit (.str "p100")) })]

/-- The two object types every scenario starts from, each tenant with an owner, an editor and two
    writers. The size limits are the kernel's. -/
def initialTenant (n : Nat) : Tenant :=
  { types := [(1, { schema := itemSchema, constraints := itemConstraints }),
               (2, { schema := partSchema })],
    owner := n * 100,
    editors := [n * 100 + 1],
    writers := [n * 100 + 2, n * 100 + 3],
    nextId := 1,
    limits := Limits.kernel }

/-- The fields an `update` line names for each object type: the item's fields, or the part's.
    A line that does not name one of them gives it `null`, so it replaces the record (`patch`
    patches). -/
def fieldsOf (ty : TypeId) : List FieldId :=
  if ty = 1 then [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
  else if ty = 2 then [1, 2, 3, 4, 5]
  else []

def initialWorld : World := fun x =>
  if x = 1 then some (initialTenant 1) else if x = 2 then some (initialTenant 2) else none

/-- Every object type of every tenant of the fixture world starts with no records. -/
theorem initialWorld_records (x : TenantId) (t : Tenant) (h : initialWorld x = some t) :
    ∀ e ∈ t.types, e.2.records = [] := by
  unfold initialWorld initialTenant at h
  split at h
  · cases h; intro e he; simp at he; rcases he with rfl | rfl <;> rfl
  · split at h
    · cases h; intro e he; simp at he; rcases he with rfl | rfl <;> rfl
    · cases h

end Strata
