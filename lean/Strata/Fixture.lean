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
    `12` an optional reference to a part cleared when the part is deleted (`set-null`). -/
def itemSchema : Schema :=
  [(0, { ty := .list (.ref 1 .restrict), required := false }),
   (1, { ty := .str, required := true }), (2, { ty := .int, required := false }),
   (3, { ty := .bool, required := false }),
   (4, { ty := .ref 1 .setNull, required := false }),
   (5, { ty := .ref 1 .restrict, required := false }),
   (6, { ty := .int, required := false, default := .int 7 }),
   (7, { ty := .list .str, required := false }),
   (8, { ty := .list (.ref 1 .setNull), required := false }),
   (10, { ty := .str, required := false, unique := true }),
   (11, { ty := .ref 2 .restrict, required := false }),
   (12, { ty := .ref 2 .setNull, required := false })]

/-- The second object type, "part" (id `2`), whose references point into items and back:
    `1` a reference to an item that restricts its delete; `2` a reference to an item cleared when
    it is deleted (`set-null`); `3` a list of item references, each cleared (`set-null`);
    `4` an optional integer. -/
def partSchema : Schema :=
  [(1, { ty := .ref 1 .restrict, required := false }),
   (2, { ty := .ref 1 .setNull, required := false }),
   (3, { ty := .list (.ref 1 .setNull), required := false }),
   (4, { ty := .int, required := false })]

/-- The two object types every scenario starts from, each tenant with an owner, an editor and two
    writers. The size limits are the kernel's. -/
def initialTenant (n : Nat) : Tenant :=
  { types := [(1, { schema := itemSchema }), (2, { schema := partSchema })],
    owner := n * 100,
    editors := [n * 100 + 1],
    writers := [n * 100 + 2, n * 100 + 3],
    nextId := 1,
    limits := Limits.kernel }

/-- The fields an `update` line names for each object type: the item's fields, or the part's.
    A line that does not name one of them gives it `null`, so it replaces the record (`patch`
    patches). -/
def fieldsOf (ty : TypeId) : List FieldId :=
  if ty = 1 then [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12]
  else if ty = 2 then [1, 2, 3, 4]
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
