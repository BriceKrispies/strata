import Strata.Reads

/-!
# Search (slice 10, second half): text search over the records of a tenant

The kernel's `search` (`crates/kernel/src/search.rs`). A request names a query text, the object types to
search (all of them when none is named) and a page size. The answer is the hits of the first page, in
object type order and then record id order, each with the record as the reader receives it and the
fields that matched, and a cursor for the next page.

The model reproduces the kernel's decisions, in the kernel's order:
- the query: at most 200 bytes, at least one word, at most 8 distinct words (`invalidIntent`);
- the page size: 1 to 200, the default 50 (`invalidIntent`);
- the object types named: each must exist (`unknownObject`);
- the candidates: for each type in order, the records whose searchable fields the reader is not
  statically barred from reading match every query word, as a prefix of a word of the field; at most
  1,000 candidates are examined in all, and a page ends when it holds its hits;
- a candidate is a hit when the reader may read the stored record and a matched field is one the reader
  may read (the redacted copy holds it) and still matches in that copy.

Abstractions, all listed in the slice 10 review note:
- The tokeniser is the ASCII one: a word is a maximal run of ASCII letters and digits, lowercased in
  ASCII. The kernel uses the Unicode alphanumerics and lowercase mapping; the model's words and the
  generator's agree only on ASCII text.
- Object types are searched in ascending type id, which is the kernel's order of the fixture's names.
- A search starts from the first candidate: the model takes no cursor, and the next cursor it returns is
  the kernel's (`examined` candidates, the last one decided).
- The searchable declarations are taken as given: their checks (text type, not personal, no duplicates)
  are the kernel's definition-time checks, not modelled.
-/

namespace Strata

/-- The longest query, in bytes (the kernel's `MAX_QUERY_BYTES`). -/
def maxQueryBytes : Nat := 200
/-- The most distinct words in a query (the kernel's `MAX_QUERY_TOKENS`). -/
def maxQueryTokens : Nat := 8
/-- The longest word, in characters; a longer one is cut (the kernel's `MAX_TOKEN_CHARS`). -/
def maxTokenChars : Nat := 64
/-- The most distinct tokens one field holds, the first in reading order (the kernel's `MAX_FIELD_TOKENS`). -/
def maxFieldTokens : Nat := 1000
/-- The candidates one probe asks for: the page bound (the kernel's `CANDIDATE_PAGE`). -/
def candidatePage : Nat := maxPage
/-- The most candidates one search examines (the kernel's `MAX_CANDIDATES`). -/
def maxCandidates : Nat := 5 * candidatePage

/-! ## Tokens -/

/-- The words of a text: the maximal runs of ASCII letters and digits, in reading order, each lowercased
    (ASCII) and cut to `maxTokenChars` characters. Repeats are kept; see `tokensOf`. -/
def alnumRuns : List Char → List Char → List (List Char)
  | cur, [] => if cur.isEmpty then [] else [cur.reverse]
  | cur, c :: cs =>
    if c.isAlphanum then alnumRuns (c :: cur) cs
    else (if cur.isEmpty then [] else [cur.reverse]) ++ alnumRuns [] cs

def words (s : String) : List String :=
  (alnumRuns [] s.toList).map (fun cs => String.mk ((cs.map Char.toLower).take maxTokenChars))

/-- Keep the first occurrence of each word, in order. -/
def dedupAux (seen : List String) : List String → List String
  | [] => []
  | x :: xs => if seen.contains x then dedupAux seen xs else x :: dedupAux (x :: seen) xs

/-- The tokens of a text: its distinct words, in order of first occurrence, at most `maxFieldTokens` of
    them (the kernel's `tokens_of`). -/
def tokensOf (s : String) : List String := (dedupAux [] (words s)).take maxFieldTokens

/-- The tokens of the value of a field: those of a string, those of the string items of a list (joined
    by newlines, as the kernel joins them), and none for any other value (the kernel's `field_tokens`). -/
def textTokens : Val → List String
  | .str s => tokensOf s
  | .list vs => tokensOf (String.intercalate "\n"
      (vs.filterMap (fun e => match e with
        | .str s => some s
        | _ => none)))
  | _ => []

/-- Whether every query word is the start of some token of the field (the kernel's `all_match`). -/
def allMatch (q ts : List String) : Bool :=
  q.all (fun x => ts.any (fun t => t.startsWith x))

/-- A field a search index holds has a text type: a string, or a list of strings. -/
def textTy : Ty → Bool
  | .str => true
  | .list .str => true
  | _ => false

/-- The fields of an object type that its search index holds: the ones its declaration lists, which exist
    and have a text type, in ascending order (the kernel's `indexed_fields`, without the personal fields,
    which the model does not have). -/
def searchedFields (spec : ReadSpec) (t : Tenant) (ty : TypeId) : List FieldId :=
  match t.typ? ty, spec.searchable.lookup ty with
  | some o, some listed =>
      (listed.filter (fun f => match o.schema.lookup f with
        | some d => textTy d.ty
        | none => false)).mergeSort (fun a b => decide (a ≤ b))
  | _, _ => []

/-! ## Who is barred from a field -/

/-- Whether an expression reads a field of a record (the kernel's `Var` and `Through` nodes, which this
    model reads through `field` and `createdBy`). -/
def exprReadsRecord : Expr → Bool
  | .lit _ => false
  | .field _ _ => true
  | .createdBy _ => true
  | .principal => false
  | .cmp _ a b => exprReadsRecord a || exprReadsRecord b
  | .and a b => exprReadsRecord a || exprReadsRecord b
  | .or a b => exprReadsRecord a || exprReadsRecord b
  | .not a => exprReadsRecord a
  | .isNull a => exprReadsRecord a
  | .param _ => false

/-- An empty record, for deciding a read rule that reads no record. -/
def emptyRecord : Record := { owner := none, version := 0, fields := [] }

/-- Whether a field is statically barred from the reader: its read rule does not look at the record and
    is not `true` for the reader, so the kernel probes no candidate on it. The system is never barred. -/
def barredField (rs : FieldRules) (p : Principal) (ty : TypeId) (f : FieldId) : Bool :=
  match p with
  | .system => false
  | _ => match rs.rule ty f with
    | none => false
    | some e => !exprReadsRecord e && !condHolds (readEnv p emptyRecord) (some e)

/-- The fields a search of an object type probes for the reader: its searchable fields not barred. -/
def probeFields (spec : ReadSpec) (t : Tenant) (p : Principal) (ty : TypeId) : List FieldId :=
  (searchedFields spec t ty).filter (fun f => !barredField spec.rules p ty f)

/-! ## Candidates and decisions -/

/-- The search request: the query text, the object types to search (all when empty) and the page size
    (the default when none). -/
structure SearchReq where
  query : String
  objects : List TypeId := []
  page : Option Nat := none

/-- A hit: the record as the reader receives it, with the fields that matched. -/
structure SearchHit where
  ty : TypeId
  id : RecordId
  record : Record
  matched : List FieldId
  deriving Repr

/-- A candidate: a stored record that matches the query in one of the probed fields, with those fields. -/
structure Cand where
  id : RecordId
  record : Record
  fields : List FieldId

/-- The probed fields of a stored record that match the query. -/
def matchedIn (q : List String) (fs : List FieldId) (r : Record) : List FieldId :=
  fs.filter (fun f => allMatch q (textTokens (Fields.get r.fields f)))

/-- The candidates of an object type, in ascending record id: the records that match the query in one of
    the probed fields of the stored record (the kernel's `candidates_in`). -/
def candidatesOf (src : Src) (ty : TypeId) (q : List String) (probes : List FieldId) : List Cand :=
  ((src.recs ty).mergeSort (fun a b => decide (a.1 ≤ b.1))).filterMap (fun e =>
    let fs := matchedIn q probes e.2
    if fs.isEmpty then none else some { id := e.1, record := e.2, fields := fs })

/-- Decide a candidate (the kernel's `decide`): a hit when the reader may read the stored record and a
    probed field that matched is one the reader may read and still matches in the redacted copy. -/
def decideCand (src : Src) (rs : FieldRules) (p : Principal) (ty : TypeId) (q : List String)
    (probes : List FieldId) (c : Cand) : Option SearchHit :=
  if !src.canRead p ty c.record then none
  else
    let shown := redact rs p ty c.record
    let named := c.fields.filter (fun f => probes.contains f && shown.fields.any (fun fv => fv.1 == f))
    let matched := matchedIn q named shown
    if matched.isEmpty then none
    else some { ty := ty, id := c.id, record := shown, matched := matched }

/-- Decide candidates in order with `room` hits still wanted: the hits, and how many candidates were
    examined (up to and including the one that fills the page). -/
def decideRun (ev : Cand → Option SearchHit) : Nat → List Cand → List SearchHit × Nat
  | _, [] => ([], 0)
  | room, c :: cs =>
    match ev c with
    | none => let r := decideRun ev room cs; (r.1, r.2 + 1)
    | some h =>
      if room ≤ 1 then ([h], 1)
      else let r := decideRun ev (room - 1) cs; (h :: r.1, r.2 + 1)

/-! ## The loop -/

/-- The state of a search as it runs: the hits so far, the candidates examined, the last candidate
    examined (its type and id), the cursor the answer carries, and whether the search has stopped. -/
structure SearchAcc where
  hits : List SearchHit := []
  examined : Nat := 0
  last : Option (TypeId × RecordId) := none
  next : Option (TypeId × RecordId) := none
  stop : Bool := false

/-- The probes of one object type (the kernel's loop over one object): each asks for the candidates
    left, up to the page bound and the candidates still allowed, decides them, and goes on from the last
    one examined while the probe came back full. `fuel` is the number of candidates plus one; every
    round examines at least one candidate, so the fuel is never the limit. -/
def probeType (size : Nat) (ty : TypeId) (ev : Cand → Option SearchHit) :
    Nat → SearchAcc → List Cand → SearchAcc
  | 0, acc, _ => acc
  | fuel + 1, acc, rest =>
    if acc.stop then acc
    else if maxCandidates ≤ acc.examined then { acc with next := acc.last, stop := true }
    else
      let limit := min candidatePage (maxCandidates - acc.examined)
      let found := rest.take limit
      if found.isEmpty then acc
      else
        let (hs, consumed) := decideRun ev (size - acc.hits.length) found
        let lastId := ((found.drop (consumed - 1)).head?).map (fun c => (ty, c.id))
        let acc' := { acc with hits := acc.hits ++ hs, examined := acc.examined + consumed,
                               last := lastId }
        if size ≤ acc'.hits.length then { acc' with next := acc'.last, stop := true }
        else if found.length < limit then acc'
        else probeType size ty ev fuel acc' (rest.drop consumed)

/-- The object types of a search, in order: each with its probed fields. -/
def searchTypes (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat) :
    List (TypeId × List FieldId) → SearchAcc → SearchAcc
  | [], acc => acc
  | (ty, probes) :: more, acc =>
    if acc.stop then acc
    else
      let cands := candidatesOf src ty q probes
      searchTypes src rs p q size more
        (probeType size ty (decideCand src rs p ty q probes) (cands.length + 1) acc cands)

/-- The answer to a search: the hits of the page in order, the cursor of the next page and the number of
    candidates examined. -/
structure SearchResult where
  hits : List SearchHit
  next : Option (TypeId × RecordId)
  examined : Nat
  deriving Repr

/-- The checks of a search, in the kernel's order, and what it searches: the query's words, the page size
    and the object types with their probed fields. -/
def searchPlan (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    Except ReadErr (List String × Nat × List (TypeId × List FieldId)) :=
  let q := tokensOf req.query
  if maxQueryBytes < req.query.utf8ByteSize then .error .invalidIntent
  else if q = [] then .error .invalidIntent
  else if maxQueryTokens < q.length then .error .invalidIntent
  else
    let size := req.page.getD defaultPage
    if size = 0 ∨ maxPage < size then .error .invalidIntent
    else if req.objects.any (fun ty => (t.typ? ty).isNone) then .error .unknownObject
    else
      let types := t.types.mergeSort (fun a b => decide (a.1 ≤ b.1))
      let plan := types.filterMap (fun e =>
        if req.objects.isEmpty ∨ req.objects.contains e.1 then
          let ps := probeFields spec t p e.1
          if ps.isEmpty then none else some (e.1, ps)
        else none)
      .ok (q, size, plan)

/-- The search of a plan over a source: the loop of the kernel's `run`, from the first candidate. -/
def searchOn (src : Src) (rs : FieldRules) (p : Principal) (q : List String) (size : Nat)
    (plan : List (TypeId × List FieldId)) : SearchResult :=
  let acc := searchTypes src rs p q size plan {}
  { hits := acc.hits, next := acc.next, examined := acc.examined }

/-- Answer a search for a principal (the kernel's `search`). Total and computable. -/
def search (t : Tenant) (spec : ReadSpec) (p : Principal) (req : SearchReq) :
    Except ReadErr SearchResult :=
  match searchPlan t spec p req with
  | .error e => .error e
  | .ok (q, size, plan) => .ok (searchOn (Src.ofTenant t) spec.rules p q size plan)

end Strata
