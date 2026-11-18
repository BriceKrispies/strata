/-
The abstract model as a program. Reads scenarios from the file named on the command line,
runs each through `Strata.step`, and prints every outcome and the final state.

The Rust kernel is run on the same scenarios and must print the same text. See
`crates/verification/src/lean.rs` for the other half and the file format.
-/

import Strata.Model

open Strata

/-- The two tenants every scenario starts from: one required string field, one optional
    integer, one optional boolean; an owner, an editor and two writers each. -/
def initialTenant (n : Nat) : Tenant :=
  { schema := [(1, { ty := .str, required := true }), (2, { ty := .int, required := false }),
               (3, { ty := .bool, required := false })],
    owner := n * 100,
    editors := [n * 100 + 1],
    writers := [n * 100 + 2, n * 100 + 3],
    records := [],
    nextId := 1 }

def initialWorld : World := fun x =>
  if x = 1 then some (initialTenant 1) else if x = 2 then some (initialTenant 2) else none

/-- The string without its first `n` characters. -/
def dropChars (s : String) (n : Nat) : String :=
  String.ofList (s.toList.drop n)

/-- The string without trailing spaces or carriage returns. -/
def trimEnd (s : String) : String :=
  String.ofList (s.toList.reverse.dropWhile (fun c => c == ' ' || c == '')).reverse

def parseVal (s : String) : Option Val :=
  if s = "n" then some .null
  else if s.startsWith "i:" then (dropChars s 2).toInt?.map Val.int
  else if s.startsWith "s:" then some (.str (dropChars s 2))
  else if s = "b:1" then some (.bool true)
  else if s = "b:0" then some (.bool false)
  else none

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
    | "update" :: id :: fields => pure (c, .update (← id.toNat?) (← parseFields fields))
    | ["delete", id] => pure (c, .delete (← id.toNat?))
    | ["require", f, "strict"] => pure (c, .setRequired (← f.toNat?) true)
    | ["require", f, "lax"] => pure (c, .setRequired (← f.toNat?) false)
    | _ => none
  | _ => none

def showVal : Val → String
  | .null => "n"
  | .int i => s!"i:{i}"
  | .str s => s!"s:{s}"
  | .bool true => "b:1"
  | .bool false => "b:0"

def showErr : Err → String
  | .noTenant => "no_tenant"
  | .unauthorized => "unauthorized"
  | .notFound => "not_found"
  | .invalid => "invalid"
  | .migrationRejected => "migration_rejected"

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
    trimEnd s!"record {q.1} owner={q.2.owner} {" ".intercalate fields}"
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
