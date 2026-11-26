/-
The fixture the scenario runs start from: two tenants, each with an owner, an editor and two
writers, and two object types. The spec executable (`Main.lean`) and the executable kernel
(`Strata/Impl/World.lean`) both start from `initialWorld`, so the fixture is part of the library
and its facts are audited.
-/

import Strata.Model
import Strata.Reads

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

/-- The constraint of the part type (WG-5664): a part names an item, through field `1` or through field
    `2`, a set-null reference, or has a number (field `4`), or was made by `p100` (field `5`, whose
    default is whether the caller is `p100`). A delete that clears the only reference of a part that
    has none of the others is refused. -/
def partConstraints : List Expr :=
  [.or (.or (.or (.not (.isNull (.field .record 1))) (.not (.isNull (.field .record 2))))
            (.not (.isNull (.field .record 4))))
       (.cmp .eq (.field .record 5) (.lit (.bool true)))]

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

/-- The third object type, "ticket" (id `4`), which carries the fixture's workflow (slice 8): `1` the
    state, a string that a new ticket starts `open` (its default); `2` an optional integer, the votes the
    guards read; `3` an optional string, the note that a transition writes. The id `3` is left to the
    scenario that names an unknown type. -/
def ticketSchema : Schema :=
  [(1, { ty := .str, required := false, default := .lit (.str "open") }),
   (2, { ty := .int, required := false }),
   (3, { ty := .str, required := false })]

/-- The workflow of the tickets (slice 8), over field `1`: an open ticket is shipped when it has a vote
    (`closed`, the note `shipped`); a closed ticket may be reopened; an open ticket with no votes is
    archived and its votes are set to `0`; a closed ticket is archived with its note set; an open ticket
    is escalated with nine votes, which a writer may not write (the record it would be is over the writers'
    limit). -/
def ticketWorkflow : Workflow :=
  { id := "ticket_flow", field := 1, initial := ["open"],
    transitions :=
      [ { name := "ship", source := "open", target := "closed",
          guard := some (.cmp .ge (.field .new 2) (.lit (.int 1))),
          set := [(3, .lit (.str "shipped"))] },
        { name := "reopen", source := "closed", target := "open" },
        { name := "archive", source := "open", target := "archived", guard := some (.isNull (.field .old 2)),
          set := [(2, .lit (.int 0))] },
        { name := "archive_closed", source := "closed", target := "archived",
          set := [(3, .lit (.str "archived"))] },
        { name := "escalate", source := "open", target := "escalated", set := [(2, .lit (.int 9))] } ] }

/-- The role of an editor in the fixture (the kernel's `editor`). -/
def editorRole : RoleId := 2

/-- The role of a writer in the fixture (the kernel's `writer`). -/
def writerRole : RoleId := 3

/-- A writer may change or delete a record it created: `record.$created_by == principal`. -/
def ownRecord : Expr := .cmp .eq (.createdBy .record) .principal

/-- The policies of the fixture, for each of its object types (slice 6). An editor may do everything
    to the records of each type; a writer may read and create them and change or delete the ones it
    created. The public role, which every user holds too, may create a part only when there is no
    user, that is for an anonymous caller (`isNull principal`); the system holds no role. -/
def fixturePolicies : List Policy :=
  [ { role := editorRole, resource := .object 1, actions := [.read, .create, .update, .delete] },
    { role := writerRole, resource := .object 1, actions := [.read, .create] },
    { role := writerRole, resource := .object 1, actions := [.update, .delete], condition := some ownRecord },
    { role := editorRole, resource := .object 2, actions := [.read, .create, .update, .delete] },
    { role := writerRole, resource := .object 2, actions := [.read, .create] },
    { role := writerRole, resource := .object 2, actions := [.update, .delete], condition := some ownRecord },
    { role := publicRole, resource := .object 2, actions := [.create],
      condition := some (.isNull .principal) },
    -- Slice 7: an editor may manage the tenant's members (`editors_manage_members`), so a manager who
    -- is not the owner is in the fixture: it may confer the roles whose powers it holds.
    { role := editorRole, resource := .members, actions := [.manage] },
    -- Slice 8: the tickets. An editor may do everything to them; a writer may read and create them and
    -- change or delete the ones it created whose votes are at most five (so an assignment that sets nine
    -- votes makes the update one the writer may not make, which the authorisation of the result refuses).
    { role := editorRole, resource := .object 4, actions := [.read, .create, .update, .delete] },
    { role := writerRole, resource := .object 4, actions := [.read, .create] },
    { role := writerRole, resource := .object 4, actions := [.update, .delete],
      condition := some (.and ownRecord (.cmp .le (.field .new 2) (.lit (.int 5)))) } ]

/-- The object types every scenario starts from, each tenant with an owner, an editor and two writers.
    Type `4` (the tickets, with the workflow) is part of the fixture too. The size limits are the kernel's. -/
def initialTenant (n : Nat) : Tenant :=
  { types := [(1, { schema := itemSchema, constraints := itemConstraints }),
               (2, { schema := partSchema, constraints := partConstraints }),
               (4, { schema := ticketSchema, workflows := [ticketWorkflow] })],
    members := [(n * 100, [ownerRole]), (n * 100 + 1, [editorRole]),
                (n * 100 + 2, [writerRole]), (n * 100 + 3, [writerRole])],
    grants := [],
    policies := fixturePolicies,
    nextId := 1,
    limits := Limits.kernel,
    roles := [ownerRole, publicRole, editorRole, writerRole],
    -- The kernel's fixture installs the tenant by six accepted steps: its creation, two schema changes
    -- (one event each, the schema version raised by each), and three role assignments (one event each).
    revision := 6, eventSeq := 6, schemaVersion := 3 }

/-- The fields an `update` line names for each object type: the item's fields, or the part's.
    A line that does not name one of them gives it `null`, so it replaces the record (`patch`
    patches). -/
def fieldsOf (ty : TypeId) : List FieldId :=
  if ty = 1 then [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
  else if ty = 2 then [1, 2, 3, 4, 5]
  else if ty = 4 then [1, 2, 3]
  else []

def initialWorld : World := fun x =>
  if x = 1 then some (initialTenant 1) else if x = 2 then some (initialTenant 2) else none

/-- Every object type of every tenant of the fixture world starts with no records. -/
theorem initialWorld_records (x : TenantId) (t : Tenant) (h : initialWorld x = some t) :
    ∀ e ∈ t.types, e.2.records = [] := by
  unfold initialWorld initialTenant at h
  split at h
  · cases h; intro e he; simp at he; rcases he with rfl | rfl | rfl <;> rfl
  · split at h
    · cases h; intro e he; simp at he; rcases he with rfl | rfl | rfl <;> rfl
    · cases h


/-! ## The read declarations of the fixture (slice 10)

The kernel holds a field's read rule in its definition and a named query in the tenant's schema; the
model holds them beside the write model, in a `ReadSpec` (see the slice 10 review note). The kernel's
fixture (`crates/verification/src/lean.rs`) declares exactly these, in tenants `t1` and `t2`; a tenant
that a scenario creates (tenant `3`) declares none. Every query is one the kernel plans as a scan: it
orders, and either has no filter or orders by more than one key. -/

/-- The read rules of the fixture. An item's field `10` is shown only when its boolean field `3` is
    `true`; a part's field `4` only to `p100`; a ticket's note (field `3`) only once it has a vote. -/
def fixtureReadRules : FieldRules :=
  [(1, 10, .cmp .eq (.field .record 3) (.lit (.bool true))),
   (2, 4, .cmp .eq .principal (.lit (.str "p100"))),
   (4, 3, .cmp .ge (.field .record 2) (.lit (.int 1)))]

/-- The named queries of the fixture. `items_by_rank` lists the items by field `2` descending;
    `items_not_elm` those not named `elm`, by field `3` then field `2` descending, three to a page,
    and declares a parameter `tag` that its filter does not read; `parts_ranked` the parts by field
    `4`; `tickets_open` the open tickets by field `2` then field `3`, two to a page. -/
def fixtureQueries : List (QueryId × QueryDef) :=
  [("items_by_rank",
    { ty := 1, order := [(2, true)], declared := [2, 3],
      aggregates :=
        [("count", { kind := .count, scanBound := 10000 }),
         ("bounded_count", { kind := .count, scanBound := 2 }),
         ("sum_rank", { kind := .sum, field := some 2, scanBound := 10000 }),
         ("min_rank", { kind := .minimum, field := some 2, scanBound := 10000 }),
         ("max_rank", { kind := .maximum, field := some 2, scanBound := 10000 }),
         ("top_rank", { kind := .top, field := some 2, n := some 2, descending := true, scanBound := 10000 }),
         ("count_by_flag", { kind := .count, groupBy := some 3, scanBound := 10000 })] }),
   ("items_not_elm",
    { ty := 1
      params := ["tag"]
      filter := some (.cmp .ne (.field .record 1) (.lit (.str "elm")))
      order := [(3, false), (2, true)]
      limit := some 3
      declared := [2, 3]
      aggregates :=
        [("count", { kind := .count, scanBound := 10000 }),
         ("sum_rank", { kind := .sum, field := some 2, scanBound := 10000 })] }),
   ("parts_ranked",
    { ty := 2, order := [(4, false)], declared := [4],
      aggregates :=
        [("count", { kind := .count, scanBound := 10000 }),
         ("sum", { kind := .sum, field := some 4, scanBound := 10000 }),
         ("max", { kind := .maximum, field := some 4, scanBound := 10000 })] }),
   ("tickets_open",
    { ty := 4
      filter := some (.cmp .eq (.field .record 1) (.lit (.str "open")))
      order := [(2, false), (3, false)]
      limit := some 2
      declared := [2, 3]
      aggregates :=
        [("count", { kind := .count, scanBound := 10000 }),
         ("sum_votes", { kind := .sum, field := some 2, scanBound := 10000 }),
         ("max_note", { kind := .maximum, field := some 3, scanBound := 10000 })] })]

/-- The searchable fields of the fixture's types: an item's string fields `1`, `7` (a list of strings), `9`
    and `10` (`10` has a read rule); a ticket's state `1` and note `3` (which has a read rule). A part has
    no text field. -/
def fixtureSearchable : List (TypeId × List FieldId) :=
  [(1, [1, 7, 9, 10]), (4, [1, 3])]

/-- The read declarations of the fixture's tenants `1` and `2`. -/
def fixtureReadSpec : ReadSpec :=
  { rules := fixtureReadRules, queries := fixtureQueries, searchable := fixtureSearchable }

/-- The read declarations of tenant `x`: the fixture's for the two tenants that start with it, none
    for a tenant a scenario creates. -/
def readSpecOf (x : TenantId) : ReadSpec :=
  if x = 1 ∨ x = 2 then fixtureReadSpec else {}

end Strata
