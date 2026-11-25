/-
The abstract model as a program. Reads scenarios from the file named on the command line,
runs each through `Strata.step`, and prints every outcome and the final state.

The Rust kernel is run on the same scenarios and must print the same text. See
`crates/verification/src/lean.rs` for the other half and the file format.
-/

import Strata.Model

open Strata

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

/-- The string without its first `n` characters. -/
def dropChars (s : String) (n : Nat) : String :=
  String.ofList (s.toList.drop n)

/-- The string without trailing spaces or carriage returns. -/
def trimEnd (s : String) : String :=
  String.ofList (s.toList.reverse.dropWhile (fun c => c == ' ' || c == '
')).reverse

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

/-- An `update` line replaces the record: it is the patch that names every field of the fixture
    schema, the ones the line leaves out as `null`. -/
def replacement (fs : Fields) : Fields :=
  fs ++ (fixtureFields.filter (fun f => !fs.any (fun fv => fv.1 == f))).map (fun f => (f, .null))

def parseField (s : String) : Option (FieldId × Val) :=
  match s.splitOn "=" with
  | [f, v] => do
    let f ← f.toNat?
    let v ← parseVal v
    pure (f, v)
  | _ => none

def parseFields (tokens : List String) : Option Fields :=
  tokens.mapM parseField

def parseOp (tokens : List String) : Option (Ctx × Intent) :=
  match tokens with
  | "op" :: t :: p :: rest => do
    let c : Ctx := { tenant := ← t.toNat?, principal := ← p.toNat? }
    match rest with
    | "create" :: fields => pure (c, .create (← parseFields fields))
    | "update" :: id :: rest => do
      let (ev, fields) ← parseExpected rest
      pure (c, .update (← id.toNat?) (replacement (← parseFields fields)) ev)
    | "patch" :: id :: rest => do
      let (ev, fields) ← parseExpected rest
      pure (c, .update (← id.toNat?) (← parseFields fields) ev)
    | "delete" :: id :: rest => do
      let (ev, more) ← parseExpected rest
      if more.isEmpty then pure (c, .delete (← id.toNat?) ev) else none
    | ["require", f, "strict"] => pure (c, .setRequired (← f.toNat?) true)
    | ["require", f, "lax"] => pure (c, .setRequired (← f.toNat?) false)
    | ["unique", f, "strict"] => pure (c, .setUnique (← f.toNat?) true)
    | ["unique", f, "lax"] => pure (c, .setUnique (← f.toNat?) false)
    | "composite" :: fs => pure (c, .defineUnique (← fs.mapM String.toNat?))
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

def showErr : Err → String
  | .noTenant => "no_tenant"
  | .unauthorized => "unauthorized"
  | .notFound => "not_found"
  | .invalid => "invalid"
  | .migrationRejected => "migration_rejected"
  | .danglingReference => "dangling_reference"
  | .stillReferenced => "still_referenced"
  | .versionConflict => "version_conflict"
  | .limitExceeded => "limit_exceeded"
  | .uniqueViolated => "unique_violated"
  | .invalidSchema => "invalid_schema"

/-- Insertion sort on a key; the lists here are a handful of elements. -/
def sortBy {α : Type} (key : α → Nat) : List α → List α
  | [] => []
  | x :: xs =>
    let rest := sortBy key xs
    (rest.filter (fun y => key y < key x)) ++ [x] ++ (rest.filter (fun y => ¬ key y < key x))

def showTenant (id : Nat) (t : Tenant) : List String :=
  let required := (sortBy (fun (fd : FieldId × FieldDef) => fd.1) (t.schema.filter (fun fd => fd.2.required))).map
    (fun fd => toString fd.1)
  let header := s!"tenant {id} next={t.nextId} required={",".intercalate required}"
  let records := (sortBy (fun (q : RecordId × Record) => q.1) t.records).map fun q =>
    let fields := (sortBy (fun (fv : FieldId × Val) => fv.1) q.2.fields).map fun fv => s!"{fv.1}={showVal fv.2}"
    trimEnd s!"record {q.1} owner={q.2.owner} v={q.2.version} {" ".intercalate fields}"
  header :: records

def showWorld (w : World) : List String :=
  [1, 2].flatMap fun id =>
    match w id with
    | some t => showTenant id t
    | none => [s!"tenant {id} absent"]

partial def runLines (lines : List String) (w : World) (out : Array String) : Array String :=
  match lines with
  | [] => out
  | line :: rest =>
    let tokens := (line.splitOn " ").filter (· ≠ "")
    match tokens with
    | [] => runLines rest w out
    | "scenario" :: name => runLines rest initialWorld (out.push s!"scenario {" ".intercalate name}")
    | ["end"] => runLines rest initialWorld ((out.push "state" ++ (showWorld w).toArray).push "end")
    | _ =>
      match parseOp tokens with
      | none => runLines rest w (out.push s!"unparsed {line}")
      | some (c, i) =>
        match step w i c with
        | .ok w' => runLines rest w' (out.push "ok")
        | .error e => runLines rest w (out.push s!"err {showErr e}")

def main (args : List String) : IO UInt32 := do
  match args with
  | [path] =>
    let text ← IO.FS.readFile path
    let lines := (text.splitOn "\n").map trimEnd
    for line in runLines lines initialWorld #[] do
      IO.println line
    pure 0
  | _ =>
    IO.eprintln "usage: strata_model <scenarios file>"
    pure 2
