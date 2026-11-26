/-
The abstract model as a program. Reads scenarios from the file named on the command line,
runs each through `Strata.transition`, and prints every outcome and the final state. An accepted
step prints `ok`, then `revision <n>` (the addressed tenant's revision after it), one `event` line per
domain event and one `effect publish <seq>` line per event (slice 9). Refusals print `err <kind>`.

With `--impl` before the file name, the same scenarios run through the executable kernel
(`Strata.Impl.transitionI`, the records held in slots) and print the same text, the state read back
through the abstraction. The differential harness compares both with the Rust kernel.

A `read` line (records or a named query, see `parseRead`) prints the answer of the kernel's read: `err <kind>`, or `ok`, `next <cursor>` and a `row` line per row. The three kernels (spec, `--impl`, `--indexed`) print the same.

The Rust kernel is run on the same scenarios and must print the same text. See
`crates/verification/src/lean.rs` for the other half and the file format.
-/

import Strata.Impl.World
import Strata.Impl.Transition
import Strata.Impl.Read
import Strata.Impl.Search
import Strata.Impl.Aggregates
import Strata.Bench

open Strata
open Strata.Impl


/-- The string without its first `n` characters. -/
def dropChars (s : String) (n : Nat) : String :=
  String.ofList (s.toList.drop n)

/-- The string without trailing spaces or carriage returns. -/
def trimEnd (s : String) : String :=
  String.ofList (s.toList.reverse.dropWhile (fun c => c == ' ' || c == '\r')).reverse

/-- `<count>:<rest>`, split at the first colon. -/
def splitCount (s : String) : Option (Nat × String) :=
  match s.splitOn ":" with
  | k :: rest@(_ :: _) => k.toNat?.map (fun n => (n, ":".intercalate rest))
  | _ => none

/-- `text` written `n` times. -/
def repeatStr (n : Nat) (text : String) : String :=
  String.join (List.replicate n text)

/-- A scalar or reference: `n`, `i:<int>`, `s:<text>`, `s*<count>:<text>` (the text repeated),
    `b:1`, `b:0`, `r:<id>`. -/
def parseElem (s : String) : Option Elem :=
  if s = "n" then some .null
  else if s.startsWith "i:" then (dropChars s 2).toInt?.map Elem.int
  else if s.startsWith "s*" then (splitCount (dropChars s 2)).map (fun p => .str (repeatStr p.1 p.2))
  else if s.startsWith "s:" then some (.str (dropChars s 2))
  else if s = "b:1" then some (.bool true)
  else if s = "b:0" then some (.bool false)
  else if s.startsWith "r:" then (dropChars s 2).toNat?.map Elem.ref
  else none

def Elem.toVal : Elem → Val
  | .null => .null
  | .int i => .int i
  | .str s => .str s
  | .bool b => .bool b
  | .ref id => .ref id

/-- A value: an element token, or a list: `l:` followed by element tokens separated by commas
    (`l:` alone is the empty list), or `l*<count>:<element>` (the element repeated). -/
def parseVal (s : String) : Option Val :=
  if s.startsWith "l*" then do
    let (n, e) ← splitCount (dropChars s 2)
    let e ← parseElem e
    pure (.list (List.replicate n e))
  else if s.startsWith "l:" then
    let body := dropChars s 2
    if body = "" then some (.list [])
    else ((body.splitOn ",").mapM parseElem).map Val.list
  else (parseElem s).map Elem.toVal

/-- An optional expected version, `v=<n>`, at the head of the tokens. -/
def parseExpected : List String → Option (Option Nat × List String)
  | t :: rest =>
    if t.startsWith "v=" then (dropChars t 2).toNat?.map (fun n => (some n, rest))
    else some (none, t :: rest)
  | [] => some (none, [])

/-- An `update` line replaces the record: it is the patch that names every field of its type, the
    ones the line leaves out as `null`. -/
def replacement (ty : TypeId) (fs : Fields) : Fields :=
  fs ++ ((fieldsOf ty).filter (fun f => !fs.any (fun fv => fv.1 == f))).map (fun f => (f, .null))

def parseField (s : String) : Option (FieldId × Val) :=
  match s.splitOn "=" with
  | [f, v] => do
    let f ← f.toNat?
    let v ← parseVal v
    pure (f, v)
  | _ => none

def parseFields (tokens : List String) : Option Fields :=
  tokens.mapM parseField

/-- A `type <n>` before the intent names the object type; without it the type is `1`. -/
def splitType : List String → Option (TypeId × List String)
  | "type" :: n :: rest => n.toNat?.map (fun k => (k, rest))
  | rest => some (1, rest)

/-- A principal token: a number is that user, `system` the system and `anon` an anonymous caller. -/
def parsePrincipal (s : String) : Option Principal :=
  if s = "system" then some .system
  else if s = "anon" then some .anonymous
  else (s.toNat?).map Principal.user

/-- A text token of the membership lines: `s:<text>`, where `_` is a space (so a blank name or label is
    `s:_`, and an address may have spaces around it). -/
def parseText (s : String) : Option String :=
  if s.startsWith "s:" then some (String.ofList ((dropChars s 2).toList.map (fun c => if c == '_' then ' ' else c)))
  else none

/-- The roles of an invitation: numbers separated by commas, or `-` for none. -/
def parseRoles (s : String) : Option (List RoleId) :=
  if s = "-" then some [] else (s.splitOn ",").mapM String.toNat?

/-- The time of a request: `@<milliseconds>`. -/
def parseNow (s : String) : Option Nat :=
  if s.startsWith "@" then (dropChars s 1).toNat? else none

/-- A field type token: `int`, `str`, `bool`, `ref:<type>:r` (a reference that restricts the delete of its target),
    `ref:<type>:s` (one that is cleared when its target is deleted) and `list:<type>`. -/
partial def parseTy (s : String) : Option Ty :=
  if s = "int" then some .int
  else if s = "str" then some .str
  else if s = "bool" then some .bool
  else if s.startsWith "list:" then (parseTy (dropChars s 5)).map Ty.list
  else if s.startsWith "ref:" then
    match (dropChars s 4).splitOn ":" with
    | [t, "r"] => t.toNat?.map (fun n => Ty.ref n .restrict)
    | [t, "s"] => t.toNat?.map (fun n => Ty.ref n .setNull)
    | _ => none
  else none

/-- The options of a field definition after its type: `required`, `unique`, `default=<value>`, in any order. -/
def parseFieldOpts : List String → FieldDef → Option FieldDef
  | [], d => some d
  | "required" :: rest, d => parseFieldOpts rest { d with required := true }
  | "unique" :: rest, d => parseFieldOpts rest { d with unique := true }
  | tok :: rest, d =>
    if tok.startsWith "default=" then
      (parseVal (dropChars tok 8)).bind (fun v => parseFieldOpts rest { d with default := .lit v })
    else none

/-- One change of a `schema` line: `define-field <type> <field> <ty> [required] [unique] [default=<v>]`,
    `remove-field <type> <field>`, `define-type <type>`, `remove-type <type> [delete]`. -/
def parseChange : List String → Option Change
  | "define-field" :: t :: f :: ty :: opts => do
    let d ← parseFieldOpts opts { ty := ← parseTy ty, required := false }
    pure (.defineField (← t.toNat?) (← f.toNat?) d)
  | ["remove-field", t, f] => do pure (.removeField (← t.toNat?) (← f.toNat?))
  | ["define-type", t] => do pure (.defineType (← t.toNat?))
  | ["remove-type", t] => do pure (.removeType (← t.toNat?) false)
  | ["remove-type", t, "delete"] => do pure (.removeType (← t.toNat?) true)
  | _ => none

/-- The changes of a `schema` line, separated by `;`. -/
def splitChanges (tokens : List String) : List (List String) :=
  (tokens.foldr (fun tok (acc : List (List String)) =>
    if tok = ";" then [] :: acc
    else match acc with
      | cur :: rest => (tok :: cur) :: rest
      | [] => [[tok]]) [[]])

/-- A schema change line: `schema [v=<n>] [strict|lax] <change> ; <change> ...` (strict unless it says `lax`). -/
def parseSchemaOp (tokens : List String) : Option Intent := do
  let (ev, rest) ← parseExpected tokens
  let (strict, rest) := match rest with
    | "strict" :: r => (true, r)
    | "lax" :: r => (false, r)
    | r => (true, r)
  let cs ← if rest.isEmpty then some [] else (splitChanges rest).mapM parseChange
  pure (.changeSchema ev strict cs)

/-- A membership or creation line: the intents of slice 7. -/
def parseMemberOp (rest : List String) : Option Intent :=
  match rest with
  | ["create-tenant", owner, name] => do pure (.createTenant (← owner.toNat?) (← parseText name))
  | ["assign", u, r] => do pure (.member (.assignRole (← u.toNat?) (← r.toNat?)))
  | ["revoke", u, r] => do pure (.member (.revokeRole (← u.toNat?) (← r.toNat?)))
  | ["team", t, l] => do pure (.member (.defineTeam (← t.toNat?) (← parseText l)))
  | ["team-assign", t, r] => do pure (.member (.assignTeamRole (← t.toNat?) (← r.toNat?)))
  | ["team-revoke", t, r] => do pure (.member (.revokeTeamRole (← t.toNat?) (← r.toNat?)))
  | ["team-add", t, u] => do pure (.member (.addTeamMember (← t.toNat?) (← u.toNat?)))
  | ["team-remove", t, u] => do pure (.member (.removeTeamMember (← t.toNat?) (← u.toNat?)))
  | ["invite", i, a, rs, n] =>
    do pure (.member (.createInvitation (← i.toNat?) (← parseText a) (← parseRoles rs) (← parseNow n)))
  | ["invite-revoke", i] => do pure (.member (.revokeInvitation (← i.toNat?)))
  | ["redeem", i, n] => do pure (.member (.redeemInvitation (← i.toNat?) (← parseNow n) none))
  | ["redeem", i, n, v] =>
    if v.startsWith "verified=" then
      do pure (.member (.redeemInvitation (← i.toNat?) (← parseNow n) (some (← parseText (dropChars v 9)))))
    else none
  | _ => none

/-- A settings level token (slice 12). -/
def parseSettingLevel : String → Option SettingLevel
  | "platform" => some .platform
  | "tenant" => some .tenant
  | "team" => some .team
  | "person" => some .person
  | _ => none

/-- The optional `team=<n>` token of a setting write. -/
def takeTeam : List String → Option (Option TeamId × List String)
  | tok :: rest =>
    if tok.startsWith "team=" then
      (do let t ← (dropChars tok 5).toNat?; pure (some t, rest))
    else some (none, tok :: rest)
  | [] => some (none, [])

/-- The optional `v=<n>` token of a setting write: the expected revision, zero when absent. -/
def takeRev : List String → Option (Nat × List String)
  | tok :: rest =>
    if tok.startsWith "v=" then
      (do let v ← (dropChars tok 2).toNat?; pure (v, rest))
    else some (0, tok :: rest)
  | [] => some (0, [])

/-- A setting write (slice 12): `setting <name> <level> [team=<n>] [v=<rev>] <value|remove>`. -/
def parseSettingOp (rest : List String) : Option Intent := do
  let "setting" :: name :: lvl :: more := rest | none
  let level ← parseSettingLevel lvl
  let (team, more) ← takeTeam more
  let (rev, more) ← takeRev more
  match more with
  | ["remove"] => pure (.writeSetting name level team none rev)
  | [v] => pure (.writeSetting name level team (some (← parseVal v)) rev)
  | _ => none

def parseOp (tokens : List String) : Option (Ctx × Intent) :=
  match tokens with
  | "op" :: t :: p :: rest0 => do
    let c : Ctx := { tenant := ← t.toNat?, principal := ← parsePrincipal p }
    let (ty, rest) ← splitType rest0
    match rest with
    | "create-tenant" :: _ => pure (c, ← parseMemberOp rest)
    | "assign" :: _ => pure (c, ← parseMemberOp rest)
    | "revoke" :: _ => pure (c, ← parseMemberOp rest)
    | "team" :: _ => pure (c, ← parseMemberOp rest)
    | "team-assign" :: _ => pure (c, ← parseMemberOp rest)
    | "team-revoke" :: _ => pure (c, ← parseMemberOp rest)
    | "team-add" :: _ => pure (c, ← parseMemberOp rest)
    | "team-remove" :: _ => pure (c, ← parseMemberOp rest)
    | "invite" :: _ => pure (c, ← parseMemberOp rest)
    | "invite-revoke" :: _ => pure (c, ← parseMemberOp rest)
    | "redeem" :: _ => pure (c, ← parseMemberOp rest)
    | "schema" :: more => pure (c, ← parseSchemaOp more)
    | "setting" :: _ => pure (c, ← parseSettingOp rest)
    | "create" :: fields => pure (c, .create ty (← parseFields fields))
    | "update" :: id :: rest => do
      let (ev, fields) ← parseExpected rest
      pure (c, .update ty (← id.toNat?) (replacement ty (← parseFields fields)) ev)
    | "patch" :: id :: rest => do
      let (ev, fields) ← parseExpected rest
      pure (c, .update ty (← id.toNat?) (← parseFields fields) ev)
    | "delete" :: id :: rest => do
      let (ev, more) ← parseExpected rest
      if more.isEmpty then pure (c, .delete ty (← id.toNat?) ev) else none
    | ["require", f, "strict"] => pure (c, .setRequired ty (← f.toNat?) true)
    | ["require", f, "lax"] => pure (c, .setRequired ty (← f.toNat?) false)
    | ["unique", f, "strict"] => pure (c, .setUnique ty (← f.toNat?) true)
    | ["unique", f, "lax"] => pure (c, .setUnique ty (← f.toNat?) false)
    | "composite" :: fs => pure (c, .defineUnique ty (← fs.mapM String.toNat?))
    | _ => none
  | _ => none

/-- A string of at least 100 characters that repeats one character prints as `s*<count>:<c>`;
    any other as `s:<text>`. -/
def showStr (s : String) : String :=
  match s.toList with
  | c :: cs => if decide (100 ≤ s.length) && cs.all (· == c) then s!"s*{s.length}:{c}" else s!"s:{s}"
  | [] => "s:"

def showElem : Elem → String
  | .null => "n"
  | .int i => s!"i:{i}"
  | .str s => showStr s
  | .bool true => "b:1"
  | .bool false => "b:0"
  | .ref id => s!"r:{id}"

/-- A list of at least 100 equal elements prints as `l*<count>:<element>`; any other as `l:`
    followed by its elements separated by commas. -/
def showVal : Val → String
  | .null => "n"
  | .int i => s!"i:{i}"
  | .str s => showStr s
  | .bool true => "b:1"
  | .bool false => "b:0"
  | .ref id => s!"r:{id}"
  | .list vs =>
    match vs with
    | e :: es =>
      if decide (100 ≤ vs.length) && es.all (· == e) then s!"l*{vs.length}:{showElem e}"
      else "l:" ++ ",".intercalate (vs.map showElem)
    | [] => "l:"

/-- A settings level as the printed forms name it (slice 12). -/
def showLevel : SettingLevel → String
  | .platform => "platform"
  | .tenant => "tenant"
  | .team => "team"
  | .person => "person"

/-- A setting's subject as the printed forms name it: the tenant, `team<n>` or `p<n>`. -/
def showSubject : SettingSubject → String
  | .tenant => "tenant"
  | .team t => s!"team{t}"
  | .person p => s!"p{p}"

def showErr : Err → String
  | .noTenant => "no_tenant"
  | .unauthorized => "unauthorized"
  | .notFound => "not_found"
  | .invalid => "invalid"
  | .migrationRejected => "migration_rejected"
  | .unknownObject => "unknown_object"
  | .danglingReference => "dangling_reference"
  | .stillReferenced => "still_referenced"
  | .versionConflict => "version_conflict"
  | .limitExceeded => "limit_exceeded"
  | .uniqueViolated => "unique_violated"
  | .invalidSchema => "invalid_schema"
  | .constraintViolated => "constraint_violated"
  | .evaluation => "evaluation"
  | .invalidIntent => "invalid_intent"
  | .unknownRole => "unknown_role"
  | .lastOwner => "last_owner"
  | .tenantExists => "tenant_exists"
  | .workflowViolation => "workflow_violation"
  | .schemaConflict => "schema_conflict"

/-- Insertion sort on a key; the lists here are a handful of elements. -/
def sortBy {α : Type} (key : α → Nat) : List α → List α
  | [] => []
  | x :: xs =>
    let rest := sortBy key xs
    (rest.filter (fun y => key y < key x)) ++ [x] ++ (rest.filter (fun y => ¬ key y < key x))

/-- The required fields of an object type, by id. -/
def requiredOf (o : ObjType) : String :=
  ",".intercalate ((sortBy (fun (fd : FieldId × FieldDef) => fd.1) (o.schema.filter (fun fd => fd.2.required))).map
    (fun fd => toString fd.1))

/-- The creator of a record as the scenario output names it: its user id, or `-` when no user created it. -/
def ownerText : Option PrincipalId → String
  | some n => toString n
  | none => "-"

/-- The records of an object type, one line each; `pre` marks the type on a non-default type. -/
def recordLines (pre : String) (o : ObjType) : List String :=
  (sortBy (fun (q : RecordId × Record) => q.1) o.records).map fun q =>
    let fields := (sortBy (fun (fv : FieldId × Val) => fv.1) q.2.fields).map fun fv => s!"{fv.1}={showVal fv.2}"
    trimEnd s!"{pre}record {q.1} owner={ownerText q.2.owner} v={q.2.version} {" ".intercalate fields}"

/-- A list of numbers, sorted, separated by commas; `-` for none. -/
def showNats (l : List Nat) : String :=
  if l.isEmpty then "-" else ",".intercalate ((sortBy id l).map toString)

/-- A text as the membership lines print it: `s:<text>` with each space written `_`. -/
def showText (s : String) : String :=
  "s:" ++ String.ofList (s.toList.map (fun c => if c == ' ' then '_' else c))

/-- A user or `-`. -/
def showUser : Option PrincipalId → String
  | some n => toString n
  | none => "-"

/-- The membership of a tenant, one line each: its roles, its members, its teams and its invitations,
    sorted by id, lists sorted. -/
def membershipLines (id : Nat) (t : Tenant) : List String :=
  [s!"roles {id} {showNats t.roles}"] ++
  (sortBy (fun (e : PrincipalId × List RoleId) => e.1) t.members).map
    (fun e => s!"member {id} {e.1} {showNats e.2}") ++
  (sortBy (fun (e : TeamId × TeamDef) => e.1) t.teams).map
    (fun e => s!"team {id} {e.1} {showText e.2.label} {showNats e.2.members} {showNats e.2.roles}") ++
  (sortBy (fun (e : InvitationId × InvitationDef) => e.1) t.invitations).map
    (fun e => s!"invitation {id} {e.1} {showText e.2.address} {showNats e.2.roles} by={e.2.createdBy} expires={e.2.expiresAt} redeemed={showUser e.2.redeemedBy} revoked={showUser e.2.revokedBy}")

/-- The key an override is listed under: its setting, its level and its subject, the tenant's subject being
    the tenant's id (the kernel's override key). -/
def overrideKey (id : Nat) (o : SettingOverride) : String :=
  let subj := match o.subject with
    | .tenant => toString id
    | s => showSubject s
  s!"{o.name}/{showLevel o.subject.level}/{subj}"

/-- The settings of a tenant that hold an override or a revision, in key order (slice 12). They are printed
    only when there are some. A setting's revision is one when it is defined (the kernel gives every definition the
    revision one), so a revision line shows only a revision above one: a setting that has been written. -/
def settingLines (id : Nat) (t : Tenant) : List String :=
  (t.overrides.mergeSort (fun a b => decide (overrideKey id a ≤ overrideKey id b))).map
    (fun o => s!"setting-override {id} {o.name} level={showLevel o.subject.level} subject={showSubject o.subject} value={showVal o.value} rev={o.revision}") ++
  ((t.settingRevs.mergeSort (fun a b => decide (a.1 ≤ b.1))).filter (fun e => e.2 > 1)).map
    (fun e => s!"setting-revision {id} {e.1} {e.2}")

/-- The tenant: its counter and the default type's required fields and records, then each other
    object type's required fields and records, marked with its id. Then its membership, when it is
    not the one the fixture starts a tenant with (always for tenant 3, which only a creation makes),
    then its setting overrides and revisions (slice 12). The tickets (type `4`, slice 8) are printed only
    while they hold a record, so the state of every scenario that does not create one is printed as before. -/
def showTenant (id : Nat) (t : Tenant) : List String :=
  let d : ObjType := (t.typ? 1).getD { schema := [] }
  let header := s!"tenant {id} next={t.nextId} required={requiredOf d}"
  let others := sortBy (fun (e : TypeId × ObjType) => e.1)
    (t.types.filter (fun e => e.1 != 1 && (e.1 != 4 || !e.2.records.isEmpty)))
  header :: recordLines "" d ++
    (others.flatMap fun e => s!"type {e.1} required={requiredOf e.2}" :: recordLines s!"type {e.1} " e.2) ++
    (if id == 3 || membershipLines id t != membershipLines id (initialTenant id) then membershipLines id t
     else []) ++
    settingLines id t

def showWorld (w : World) : List String :=
  [1, 2].flatMap (fun id =>
    match w id with
    | some t => showTenant id t
    | none => [s!"tenant {id} absent"]) ++
  (match w 3 with
    | some t => showTenant 3 t
    | none => [])


/-- The text of a record's stored form: its creator, its version and its fields. -/
def recordBody (r : Record) : String :=
  let fields := (sortBy (fun (fv : FieldId × Val) => fv.1) r.fields).map fun fv => s!"{fv.1}={showVal fv.2}"
  trimEnd s!"owner={ownerText r.owner} v={r.version} {" ".intercalate fields}"

def showPrincipal : Principal → String
  | .user n => toString n
  | .system => "system"
  | .anonymous => "anon"

def showSchemaLine : SchemaLine → String
  | .field ty f => s!"field:{ty}.{f}"
  | .unique ty fs => s!"unique:{ty}:{",".intercalate (fs.map toString)}"
  | .removeField ty f => s!"remove-field:{ty}.{f}"
  | .defineType ty => s!"define-type:{ty}"
  | .removeType ty => s!"remove-type:{ty}"

/-- The workflow transitions an update fired, by workflow id (the kernel's map is sorted by id); `-` for none. -/
def showFired (fs : List (String × String)) : String :=
  if fs.isEmpty then "-"
  else ",".intercalate ((fs.mergeSort (fun a b => decide (a.1 ≤ b.1))).map fun p => s!"{p.1}:{p.2}")

/-- One domain event as its line: the sequence number, the revision, the principal and the body. -/
def showEvent (e : DomainEvent) : String :=
  let body := match e.body with
    | .tenantCreated name => s!"tenant-created name={name}"
    | .schemaChanged v summary g =>
      let items := if summary.isEmpty then "-" else ";".intercalate (summary.map showSchemaLine)
      s!"schema-changed version={v} grandfathered={g} summary={items}"
    | .recordCreated ty id r => s!"record-created type={ty} id={id} {recordBody r}"
    | .recordUpdated ty id b a fired =>
      s!"record-updated type={ty} id={id} before=[{recordBody b}] after=[{recordBody a}] fired={showFired fired}"
    | .recordDeleted ty id b => s!"record-deleted type={ty} id={id} before=[{recordBody b}]"
    | .memberChanged who roles =>
      let name := match who with
        | .user n => toString n
        | .team t => s!"team:{t}"
      s!"member-changed who={name} roles={showNats roles}"
    | .settingChanged name subj =>
      s!"setting-changed setting={name} level={showLevel subj.level} subject={showSubject subj}"
  s!"event {e.seq} rev={e.revision} by={showPrincipal e.principal} {body}"


/-! ## Reads

A read line is `read <tenant> <principal> records <type> <ids..>` (an id is `n`, or `a-b` for every id
from `a` to `b`) or `read <tenant> <principal> query <name> [k=v ..] [page=<n>] [cursor=<c>]` (a cursor is
`o<n>`, an offset, or `a<id>`, a position after a record). It prints `err <kind>`, or `ok`, `next <cursor>`
(`-` for none) and one `row <id> <record>` line per row, in order. -/

/-- A token of record ids: `n`, or `a-b` for the ids from `a` to `b`. -/
def parseIds (s : String) : Option (List Nat) :=
  match s.splitOn "-" with
  | [a] => a.toNat?.map (fun n => [n])
  | [a, b] => do
    let a ← a.toNat?
    let b ← b.toNat?
    pure (List.range' a (b + 1 - a))
  | _ => none

/-- A cursor token: `o<n>` or `a<id>`. -/
def parseCursor (s : String) : Option Cursor :=
  if s.startsWith "o" then (dropChars s 1).toNat?.map Cursor.offset
  else if s.startsWith "a" then (dropChars s 1).toNat?.map Cursor.after
  else none

/-- The arguments of a named query read: parameters `k=v`, `page=<n>` and `cursor=<c>`. -/
def parseQueryArgs : List String → Option (List (String × Val) × Option Nat × Option Cursor)
  | [] => some ([], none, none)
  | tok :: rest => do
    let (ps, pg, cu) ← parseQueryArgs rest
    match tok.splitOn "=" with
    | k :: v =>
      let v := "=".intercalate v
      if k = "page" then pure (ps, some (← v.toNat?), cu)
      else if k = "cursor" then pure (ps, pg, some (← parseCursor v))
      else pure ((k, ← parseVal v) :: ps, pg, cu)
    | [] => none

/-- A read line: the tenant, the principal and the request. -/
def parseRead : List String → Option (TenantId × Principal × ReadReq)
  | "read" :: t :: p :: "records" :: ty :: ids => do
    let idss ← ids.mapM parseIds
    pure (← t.toNat?, ← parsePrincipal p, .records (← ty.toNat?) idss.flatten)
  | "read" :: t :: p :: "query" :: name :: rest => do
    let (ps, pg, cu) ← parseQueryArgs rest
    pure (← t.toNat?, ← parsePrincipal p, .named name ps pg cu)
  | _ => none

def showReadErr : ReadErr → String
  | .unknownObject => "unknown_object"
  | .unknownQuery => "unknown_query"
  | .invalidIntent => "invalid_intent"
  | .limitExceeded => "limit_exceeded"

def showCursor : Option Cursor → String
  | none => "-"
  | some (.offset n) => s!"o{n}"
  | some (.after id) => s!"a{id}"

/-- A setting read (slice 12): `read <tenant> <principal> setting <name>`. -/
def parseSettingRead : List String → Option (TenantId × Principal × String)
  | ["read", t, p, "setting", n] => do pure (← t.toNat?, ← parsePrincipal p, n)
  | _ => none

def showSource : SettingSource → String
  | .person => "person"
  | .team => "team"
  | .tenant => "tenant"
  | .platform => "platform"
  | .default => "default"

/-- The lines a setting read prints: `err <kind>`, or `ok`, the source, the team (`-` for none), the value and
    the setting's revision at the tenant. -/
def showSettingRead : Except Err SettingResolution → List String
  | .error e => [s!"err {showErr e}"]
  | .ok r => ["ok", s!"source {showSource r.source}",
      s!"team {match r.team with | some t => toString t | none => "-"}", s!"value {showVal r.value}",
      s!"tenant-rev {r.tenantRevision}"]

/-- The lines a read prints. -/
def showRead : Except ReadErr ReadResult → List String
  | .error e => [s!"err {showReadErr e}"]
  | .ok r => ["ok", s!"next {showCursor r.next}"] ++ r.rows.map (fun row => s!"row {row.1} {recordBody row.2}")

/-! ### Search and aggregates (slice 10, second half)

A search line is `read <tenant> <principal> search <type|*> <words..> [page=<n>]`: the words are the
tokens before the page, joined by spaces, and `*` searches every type. It prints `err <kind>`, or `ok`,
`examined <n>`, `next <type> <id>` (`-` for none) and one `hit <type> <id> matched=<fields> <record>` line
per hit, in order. An aggregate line is `read <tenant> <principal> aggregate <query> <aggregate>
[filter.<n>=<v> ..] [<param>=<v> ..]`: a `filter.<n>` token filters on field `n`, any other `k=v` is a
parameter. It prints `err <kind>`, with the rule for a limit, or `ok`, `scanned <n>`, and for each group
`group <key|-> <value|->` followed by its `top <id> <value>` rows. -/

/-- The words of a search line, and its page size when one is named last. -/
def parseSearch : List String → Option (TenantId × Principal × SearchReq)
  | "read" :: t :: p :: "search" :: ty :: rest => do
    let objects ← (if ty = "*" then some [] else (ty.toNat?).map (fun n => [n]))
    let (ws, page) ← match rest.getLast? with
      | some last =>
        if last.startsWith "page=" then
          (do let n ← (dropChars last 5).toNat?; pure (rest.dropLast, some n))
        else some (rest, none)
      | none => some (rest, none)
    pure (← t.toNat?, ← parsePrincipal p, { query := " ".intercalate ws, objects := objects, page := page })
  | _ => none

/-- The arguments of an aggregate line: `filter.<n>=<v>` filters on field `n`, `k=v` is a parameter. -/
def parseAggArgs : List String → Option (List (String × Val) × List (FieldId × Val))
  | [] => some ([], [])
  | tok :: rest => do
    let (ps, fs) ← parseAggArgs rest
    match tok.splitOn "=" with
    | k :: v =>
      let v := "=".intercalate v
      if k.startsWith "filter." then
        pure (ps, (← (dropChars k 7).toNat?, ← parseVal v) :: fs)
      else pure ((k, ← parseVal v) :: ps, fs)
    | [] => none

def parseAggregate : List String → Option (TenantId × Principal × AggReq)
  | "read" :: t :: p :: "aggregate" :: q :: a :: rest => do
    let (ps, fs) ← parseAggArgs rest
    pure (← t.toNat?, ← parsePrincipal p, { query := q, aggregate := a, params := ps, filter := fs })
  | _ => none

def showSearchNext : Option (TypeId × RecordId) → String
  | none => "-"
  | some (ty, id) => s!"{ty} {id}"

def showSearchHit (h : SearchHit) : String :=
  s!"hit {h.ty} {h.id} matched={",".intercalate (h.matched.map toString)} {recordBody h.record}"

/-- The lines a search prints. -/
def showSearch : Except ReadErr SearchResult → List String
  | .error e => [s!"err {showReadErr e}"]
  | .ok r => ["ok", s!"examined {r.examined}", s!"next {showSearchNext r.next}"] ++ r.hits.map showSearchHit

def showAggErr : AggErr → String
  | .unknownQuery => "err unknown_query"
  | .invalidIntent => "err invalid_intent"
  | .unknownObject => "err unknown_object"
  | .unknownField => "err unknown_field"
  | .limitExceeded .scanBound => "err limit_exceeded scan-bound-exceeded"
  | .limitExceeded .sumOverflow => "err limit_exceeded sum-overflow"
  | .limitExceeded .groupBound => "err limit_exceeded group-bound-exceeded"

def showOptVal : Option Val → String
  | none => "-"
  | some v => showVal v

/-- The lines an aggregate prints. -/
def showAgg : Except AggErr AggResult → List String
  | .error e => [showAggErr e]
  | .ok r => ["ok", s!"scanned {r.scanned}"] ++ r.groups.flatMap (fun g =>
      [s!"group {showOptVal g.key} {showOptVal g.value}"] ++ g.rows.map (fun row => s!"top {row.1} {showVal row.2}"))

/-- One kernel the scenario runner drives: its start state, its transition (the world after an accepted step,
    with the events it emits), the revision of a tenant, and how it prints its state. -/
structure Kernel (W : Type) where
  init : W
  step : W → Intent → Ctx → Except Err (W × List DomainEvent)
  revision : W → TenantId → Option Nat
  render : W → List String
  /-- A read on a tenant for a principal: none when the tenant does not exist. -/
  read : W → TenantId → Principal → ReadReq → Option (Except ReadErr ReadResult)
  /-- A search on a tenant for a principal (slice 10, second half). -/
  search : W → TenantId → Principal → SearchReq → Option (Except ReadErr SearchResult)
  /-- An aggregate of a named query on a tenant for a principal (slice 10, second half). -/
  aggregate : W → TenantId → Principal → AggReq → Option (Except AggErr AggResult)
  /-- The resolution of a setting for a principal on a tenant (slice 12). -/
  setting : W → TenantId → Principal → String → Option (Except Err SettingResolution)

/-- The spec's kernel: the model as written, with its transition. -/
def specKernel : Kernel World :=
  { init := initialWorld,
    step := fun w i c => (transition w i c).map (fun tr => (tr.world, tr.events)),
    revision := fun w x => (w x).map (fun t => t.revision),
    render := showWorld,
    read := fun w x p req => (w x).map (fun t => read t (readSpecOf x) p req),
    search := fun w x p req => (w x).map (fun t => search t (readSpecOf x) p req),
    aggregate := fun w x p req => (w x).map (fun t => aggregate t (readSpecOf x) p req),
    setting := fun w x p n => (w x).map (fun t => resolve t p n) }

/-- The executable kernel: the slots, printed through the abstraction. -/
def implKernel : Kernel IWorld :=
  { init := implInitial, step := fun w i c => transitionI w i c,
    revision := fun w x => (w x).map (fun t => t.revision), render := fun w => showWorld (absW w),
    read := fun w x p req => (w x).map (fun t => readI t (readSpecOf x) p req),
    search := fun w x p req => (w x).map (fun t => searchI t (readSpecOf x) p req),
    aggregate := fun w x p req => (w x).map (fun t => aggregateI t (readSpecOf x) p req),
    setting := fun w x p n => (w x).map (fun t => resolve t.abs p n) }

/-- The indexed kernel: slots and indexes, printed through the abstraction. -/
def xKernel : Kernel XWorld :=
  { init := xInitial, step := fun w i c => transitionX w i c,
    revision := fun w x => (w x).map (fun t => t.core.revision), render := fun w => showWorld (absX w),
    read := fun w x p req => (w x).map (fun t => readX t (readSpecOf x) p req),
    search := fun w x p req => (w x).map (fun t => searchX t (readSpecOf x) p req),
    aggregate := fun w x p req => (w x).map (fun t => aggregateX t (readSpecOf x) p req),
    setting := fun w x p n => (w x).map (fun t => resolve t.core.abs p n) }

partial def runLines {W : Type} (K : Kernel W) (lines : List String) (w : W) (out : Array String) :
    Array String :=
  match lines with
  | [] => out
  | line :: rest =>
    let tokens := (line.splitOn " ").filter (· ≠ "")
    match tokens with
    | [] => runLines K rest w out
    | "scenario" :: name => runLines K rest K.init (out.push s!"scenario {" ".intercalate name}")
    | ["end"] => runLines K rest K.init ((out.push "state" ++ (K.render w).toArray).push "end")
    | "read" :: _ =>
      let answered : Option (List String) :=
        match tokens with
        | _ :: _ :: _ :: "setting" :: _ => (parseSettingRead tokens).map (fun (x, p, n) =>
            match K.setting w x p n with
            | none => ["err no_tenant"]
            | some r => showSettingRead r)
        | _ :: _ :: _ :: "search" :: _ => (parseSearch tokens).map (fun (x, p, req) =>
            match K.search w x p req with
            | none => ["err no_tenant"]
            | some r => showSearch r)
        | _ :: _ :: _ :: "aggregate" :: _ => (parseAggregate tokens).map (fun (x, p, req) =>
            match K.aggregate w x p req with
            | none => ["err no_tenant"]
            | some r => showAgg r)
        | _ => (parseRead tokens).map (fun (x, p, req) =>
            match K.read w x p req with
            | none => ["err no_tenant"]
            | some r => showRead r)
      match answered with
      | none => runLines K rest w (out.push s!"unparsed {line}")
      | some lines => runLines K rest w (lines.foldl (fun a l => a.push l) out)
    | _ =>
      match parseOp tokens with
      | none => runLines K rest w (out.push s!"unparsed {line}")
      | some (c, i) =>
        match K.step w i c with
        | .ok (w', evs) =>
          -- An accepted step: `ok`, the tenant's revision after it, then each event and its `Publish` effect.
          let rev := (K.revision w' c.tenant).getD 0
          let lines := ["ok", s!"revision {rev}"] ++ evs.map showEvent ++
            evs.map (fun e => s!"effect publish {e.seq}")
          runLines K rest w' (lines.foldl (fun a l => a.push l) out)
        | .error e => runLines K rest w (out.push s!"err {showErr e}")

def runScenarios {W : Type} (K : Kernel W) (path : String) : IO Unit := do
  let text ← IO.FS.readFile path
  let lines := (text.splitOn "\n").map trimEnd
  for line in runLines K lines K.init #[] do
    IO.println line

def main (args : List String) : IO UInt32 := do
  match args with
  | [path] =>
    runScenarios specKernel path
    pure 0
  | ["--impl", path] =>
    runScenarios implKernel path
    pure 0
  | ["--indexed", path] =>
    runScenarios xKernel path
    pure 0
  | ["--bench", n] | ["--impl", "--bench", n] =>
    match n.toNat? with
    | some n =>
      let ok ← bench n 20000 20000
      pure (if ok then 0 else 1)
    | none =>
      IO.eprintln "usage: strata_model --bench <number of operations>"
      pure 2
  | ["--bench", n, specMax, stage1Max] =>
    match n.toNat?, specMax.toNat?, stage1Max.toNat? with
    | some n, some a, some b =>
      let ok ← bench n a b
      pure (if ok then 0 else 1)
    | _, _, _ =>
      IO.eprintln "usage: strata_model --bench <n> [<spec max> <stage 1 max>]"
      pure 2
  | _ =>
    IO.eprintln "usage: strata_model [--impl | --indexed] <scenarios file>
       strata_model --bench <n> [<spec max> <stage 1 max>]"
    pure 2
