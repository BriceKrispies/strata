/-
The fixture the scenario runs start from: two tenants, each with an owner, an editor and two
writers, and the fixture schema. The spec executable (`Main.lean`) and the executable kernel
(`Strata/Impl/World.lean`) both start from `initialWorld`, so the fixture is part of the library
and its facts are audited.
-/

import Strata.Model

namespace Strata

/-- The two tenants every scenario starts from, each with an owner, an editor and two writers.
    Their fields, in field id order as the kernel's schema is:
    `0` an optional list of references that restricts the delete of an element's target;
    `1` a required string; `2` an optional integer; `3` an optional boolean;
    `4` an optional reference cleared when its target is deleted (`set-null`);
    `5` an optional reference that restricts the delete (`restrict`);
    `6` an optional integer whose default is the constant `7`;
    `7` an optional list of strings;
    `8` an optional list of references that loses an element when its target is deleted
    (`set-null`);
    `10` an optional string that is unique (slice 3).
    The size limits are the kernel's. -/
def initialTenant (n : Nat) : Tenant :=
  { schema := [(0, { ty := .list (.ref .restrict), required := false }),
               (1, { ty := .str, required := true }), (2, { ty := .int, required := false }),
               (3, { ty := .bool, required := false }),
               (4, { ty := .ref .setNull, required := false }),
               (5, { ty := .ref .restrict, required := false }),
               (6, { ty := .int, required := false, default := .int 7 }),
               (7, { ty := .list .str, required := false }),
               (8, { ty := .list (.ref .setNull), required := false }),
               (10, { ty := .str, required := false, unique := true })],
    owner := n * 100,
    editors := [n * 100 + 1],
    writers := [n * 100 + 2, n * 100 + 3],
    records := [],
    nextId := 1,
    limits := Limits.kernel }

/-- The fields of the fixture schema. An `update` line names each of them, as `null` when the line
    does not, so it replaces the record (`patch` patches). -/
def fixtureFields : List FieldId := [0, 1, 2, 3, 4, 5, 6, 7, 8, 10]

def initialWorld : World := fun x =>
  if x = 1 then some (initialTenant 1) else if x = 2 then some (initialTenant 2) else none
/-- Every tenant of the fixture world starts with no records. -/
theorem initialWorld_records (x : TenantId) (t : Tenant) (h : initialWorld x = some t) :
    t.records = [] := by
  unfold initialWorld initialTenant at h
  split at h
  · cases h; rfl
  · split at h
    · cases h; rfl
    · cases h

end Strata
