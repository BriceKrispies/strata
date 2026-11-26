//! Search over the records of one tenant (SRH-150 to SRH-152, SRH-1590, D-6585, D-6586).
//!
//! **What is searched.** A record type declares the fields its search index holds, with the
//! `searchable` member of its definition ([`SchemaChange::SetSearchable`]). A field is searchable
//! when it is listed there, has a text type (a string, or a list of strings), and either is not
//! personal or is personal with a declaration that says `searchable` (D-6585). Nothing else is
//! ever indexed ([`indexed_fields`]). The declaration is checked when it is made ([`problems`]).
//!
//! **Tokens.** [`tokens_of`] is the one definition of how text becomes tokens, used by the
//! indexes of every store and by the query, so a store cannot disagree with another about what a
//! word is. The text is split at every maximal run of characters that are not alphanumeric
//! (`char::is_alphanumeric`: Unicode letters, digits and numeric characters); each word is
//! lowercased with the Unicode lowercase mapping (`str::to_lowercase`) and cut to
//! [`MAX_TOKEN_CHARS`] characters; a token that repeats is kept once; a field holds at most
//! [`MAX_FIELD_TOKENS`] tokens, the first in reading order. This is a lowercase mapping, not full
//! case folding (`ß` is not `ss`), and text is not normalised (a composed `é` and `e` followed by
//! a combining accent are different words). A list of strings is the text of its items, each a
//! separate piece.
//!
//! **Matching.** A query is split into tokens in the same way. A query token matches a field when
//! it is the start of a token of the field (so a word typed in part finds the word). A *field*
//! matches when every query token matches it, and a record matches when at least one field of it
//! matches. Tokens of two fields never combine: a record whose first word is in a field the
//! reader may read and whose second word is in a field the reader may not read does not match,
//! because the decision to return a record then rests on the hidden field.
//!
//! **Who decides.** The store holds an index and answers candidates: records and the fields
//! that matched, in ascending record id, for one record type ([`Probe`], [`Candidate`]). It
//! authorizes nothing. The kernel decides every candidate against the state it loads for the
//! reader ([`decide`]): the Read policies ([`visible_in`]), then the fields the reader may read
//! ([`redact`], D-6586), and keeps the candidate only if at least one matched field is one the
//! reader may read and the query still matches it in the record as the reader receives it. The
//! hit carries the record without the fields the reader may not read and the readable fields
//! that matched, never a hidden one. An index that is stale or wrong can therefore lose a hit but
//! cannot reveal one.
//!
//! **Fields the reader is never allowed to read** are not probed at all: a field whose read rule
//! does not look at the record (a role, the principal, the time) and is false for the reader is
//! left out of the plan, so no candidate and no page boundary depends on its content. A rule
//! that looks at the record cannot be decided before the record is loaded; a candidate that
//! matches only on such a field is dropped when it is decided, and counts towards the bound
//! below.
//!
//! **Order and paging.** Hits come in ascending order of object type id, then record id: there
//! is no relevance score. A request examines at most [`MAX_CANDIDATES`] candidates, whatever the
//! reader may read of them. A page ends when it holds the requested number of hits or the bound
//! is reached, and then carries a cursor that names the last candidate examined ([`SearchCursor`]);
//! a request with that cursor resumes after it, so what was inserted or deleted in between cannot
//! make a record appear twice or be skipped.
//!
//! The loop is written once, over a [`Source`] (a store that answers candidates and loads state),
//! so the same code runs against the memory store, the SQLite adapter and a plain snapshot.

use crate::aggregates::walk;
use crate::expr::{self, Env, Expr};
use crate::intent::{Context, DomainError, DomainErrorExt, ErrorKind};
use crate::limits;
use crate::read::{redact, visible_in};
use crate::rules::{roles_of, Lookup};
use crate::schema::{FieldType, ObjectTypeDef, TenantSchema};
use crate::state::{Need, Principal, Record, RecordExt, State, TenantState};
use crate::types::{
    FieldId, FieldTypeExt, ObjectTypeId, RecordId, SchemaChange, SchemaVersion, StateRevision,
    Value,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The longest token, in characters; a longer word is cut.
pub const MAX_TOKEN_CHARS: usize = 64;
/// The most distinct tokens one field holds in the index: the first in reading order.
pub const MAX_FIELD_TOKENS: usize = 1_000;
/// The most fields one record type declares searchable.
pub const MAX_SEARCHABLE_FIELDS: usize = 20;
/// The longest query, in bytes.
pub const MAX_QUERY_BYTES: usize = 200;
/// The most distinct tokens in a query.
pub const MAX_QUERY_TOKENS: usize = 8;
/// The candidates one probe of the store asks for.
pub const CANDIDATE_PAGE: u32 = limits::MAX_PAGE_SIZE;
/// The most candidates one request examines, whatever the reader may read of them.
pub const MAX_CANDIDATES: u32 = 5 * CANDIDATE_PAGE;
/// How many times a decision may ask for more state before it is given up.
const MAX_ROUNDS: usize = 8;

// ---------------------------------------------------------------------------------------------
// Declaration
// ---------------------------------------------------------------------------------------------

/// The type of a field is one that has text to search: a string or a list of strings.
pub fn text_like(ty: &FieldType) -> bool {
    match ty {
        FieldType::Str => true,
        FieldType::List(inner) => matches!(**inner, FieldType::Str),
        _ => false,
    }
}

/// The fields of an object type that the search index holds: those it lists as searchable, that
/// exist, have a text type and are not personal unless their personal declaration says
/// `searchable` (D-6585).
pub fn indexed_fields(def: &ObjectTypeDef) -> BTreeSet<FieldId> {
    def.searchable
        .iter()
        .flatten()
        .filter(|id| {
            def.fields.get(*id).is_some_and(|field| {
                text_like(&field.ty)
                    && field
                        .personal
                        .as_ref()
                        .is_none_or(|personal| personal.searchable)
            })
        })
        .cloned()
        .collect()
}

/// Whether `change` is one this module applies.
pub fn is_search(change: &SchemaChange) -> bool {
    matches!(
        change,
        SchemaChange::SetSearchable { .. } | SchemaChange::ClearSearchable { .. }
    )
}

/// Apply [`SchemaChange::SetSearchable`] or [`SchemaChange::ClearSearchable`] to a working copy
/// and describe it. Setting replaces the declaration there; clearing one that is not declared, and
/// changing an object type that does not exist, is refused. What the declaration names is checked
/// with the whole schema ([`problems`]).
pub fn apply(schema: &mut TenantSchema, change: &SchemaChange) -> Result<String, String> {
    match change {
        SchemaChange::SetSearchable { object, fields } => {
            if fields.is_empty() {
                return Err(format!(
                    "{object}: a searchable declaration names a field; clear it to index none"
                ));
            }
            schema
                .objects
                .get_mut(object)
                .ok_or_else(|| format!("no object type {object}"))?
                .searchable = Some(fields.clone());
            Ok(format!("set searchable fields of {object}"))
        }
        SchemaChange::ClearSearchable { object } => {
            schema
                .objects
                .get_mut(object)
                .ok_or_else(|| format!("no object type {object}"))?
                .searchable
                .take()
                .ok_or_else(|| format!("{object} declares no searchable fields"))?;
            Ok(format!("clear searchable fields of {object}"))
        }
        other => Err(format!("{other:?} is not a change of searchable fields")),
    }
}

/// The problems of the searchable declarations of a schema, pushed to `problems`: a field the type
/// lacks, a field that is not text, a personal field whose declaration is not searchable, a field
/// named twice, and too many fields.
pub fn problems(schema: &TenantSchema, problems: &mut Vec<String>) {
    for object in schema.objects.values() {
        let Some(listed) = &object.searchable else {
            continue;
        };
        let at = |what: &str| format!("{}: {what}", object.id);
        if listed.is_empty() {
            problems.push(at("a searchable declaration names a field"));
        }
        if listed.len() > MAX_SEARCHABLE_FIELDS {
            problems.push(format!(
                "limit: {}",
                at(&format!(
                    "{} searchable fields, more than the maximum of {MAX_SEARCHABLE_FIELDS}",
                    listed.len()
                ))
            ));
        }
        let mut seen = BTreeSet::new();
        for id in listed {
            if !seen.insert(id) {
                problems.push(at(&format!("searchable field {id} is named twice")));
            }
            match object.fields.get(id) {
                None => problems.push(at(&format!("searchable field {id} is not a field"))),
                Some(def) => {
                    if !text_like(&def.ty) {
                        problems.push(at(&format!(
                            "searchable field {id} of type {} is not text",
                            def.ty.name()
                        )));
                    }
                    if def.personal.as_ref().is_some_and(|p| !p.searchable) {
                        problems.push(at(&format!(
                            "searchable field {id} is personal and its declaration is not \
                             searchable (D-6585)"
                        )));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------------------------

/// The tokens of a text, in order of first occurrence, each once. See the module documentation
/// for the rule; the index and the query both use this function.
pub fn tokens_of(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let token: String = word.to_lowercase().chars().take(MAX_TOKEN_CHARS).collect();
        if seen.insert(token.clone()) {
            out.push(token);
            if out.len() == MAX_FIELD_TOKENS {
                break;
            }
        }
    }
    out
}

/// The tokens of the value of a field: those of a string, those of the items of a list of
/// strings, and none for a value of another type.
pub fn field_tokens(value: &Value) -> Vec<String> {
    match value {
        Value::Str(text) => tokens_of(text),
        Value::List(items) => {
            let pieces: Vec<&str> = items
                .iter()
                .filter_map(|item| match item {
                    Value::Str(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            tokens_of(&pieces.join("\n"))
        }
        _ => Vec::new(),
    }
}

/// The index entries of a record: for each indexed field of its type that holds tokens, the
/// field and its tokens, in field order. This is what a store writes in the commit of the record.
pub fn entries_of(def: &ObjectTypeDef, record: &Record) -> Vec<(FieldId, Vec<String>)> {
    indexed_fields(def)
        .into_iter()
        .filter_map(|field| {
            let tokens = field_tokens(record.get(&field));
            (!tokens.is_empty()).then_some((field, tokens))
        })
        .collect()
}

/// Whether every query token is the start of a token in `field_tokens`.
fn all_match(query: &[String], field_tokens: &[String]) -> bool {
    query
        .iter()
        .all(|q| field_tokens.iter().any(|t| t.starts_with(q.as_str())))
}

/// The fields among `fields` (and indexed) of `record` that match the query tokens.
pub fn matching_fields(
    def: &ObjectTypeDef,
    record: &Record,
    fields: &[FieldId],
    query: &[String],
) -> Vec<FieldId> {
    let indexed = indexed_fields(def);
    fields
        .iter()
        .filter(|field| indexed.contains(*field))
        .filter(|field| all_match(query, &field_tokens(record.get(field))))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Requests, plans, candidates
// ---------------------------------------------------------------------------------------------

/// Where the next page of a search starts: after the candidate `id` of `object`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SearchCursor {
    pub object: ObjectTypeId,
    pub id: RecordId,
}

impl SearchCursor {
    /// The cursor as the browser holds it: `<id>~<object type>`. Opaque to the client.
    pub fn encode(&self) -> String {
        format!("{}~{}", self.id.0, self.object)
    }

    /// The cursor an [`SearchCursor::encode`] text stands for; `None` for any other text.
    pub fn decode(text: &str) -> Option<SearchCursor> {
        let (id, object) = text.split_once('~')?;
        if id.is_empty() || id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(SearchCursor {
            object: ObjectTypeId::new(object).ok()?,
            id: RecordId(id.parse().ok()?),
        })
    }
}

/// A search request: the query text, the object types to search (all that have a searchable
/// field when empty), the page size and the cursor of the page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub objects: Vec<ObjectTypeId>,
    #[serde(default)]
    pub page_size: Option<u32>,
    #[serde(default)]
    pub cursor: Option<SearchCursor>,
}

/// What a request comes to for one reader: the query tokens, the object types in ascending order
/// each with the fields to probe, the page size and the position to resume after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub tokens: Vec<String>,
    pub objects: Vec<(ObjectTypeId, Vec<FieldId>)>,
    pub page_size: u32,
    pub after: Option<SearchCursor>,
}

/// One probe of a store's index: the records of one object type with an id above `after`, in
/// ascending id order, at most `limit`, whose `fields` match every token of the query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub object: ObjectTypeId,
    pub fields: Vec<FieldId>,
    pub tokens: Vec<String>,
    pub after: Option<RecordId>,
    pub limit: u32,
}

/// A record the index says matches, with the fields it matched in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: RecordId,
    pub fields: Vec<FieldId>,
}

/// The records of `records` (in ascending id order) that a probe selects: the reference
/// definition of an index probe, which the memory store answers with and the index of the SQLite
/// adapter answers the same as.
pub fn candidates_in<'a>(
    records: impl Iterator<Item = &'a Arc<Record>>,
    def: &ObjectTypeDef,
    probe: &Probe,
) -> Vec<Candidate> {
    records
        .filter(|record| probe.after.is_none_or(|after| record.id > after))
        .filter_map(|record| {
            let fields = matching_fields(def, record, &probe.fields, &probe.tokens);
            (!fields.is_empty()).then_some(Candidate {
                id: record.id,
                fields,
            })
        })
        .take(probe.limit as usize)
        .collect()
}

fn refuse(message: impl Into<String>) -> DomainError {
    DomainError::new(ErrorKind::InvalidIntent, message)
}

/// Whether a read rule can be decided without the record: it names no field of the record and
/// follows no reference, so it depends on the reader and the time alone.
fn record_independent(rule: &Expr) -> bool {
    let mut touches = false;
    walk(rule, &mut |node| {
        if matches!(node, Expr::Var(..) | Expr::Through(..)) {
            touches = true;
        }
    });
    !touches
}

/// Plan a request for the reader of `ctx`: refuse a query that has no token, too many tokens or
/// too many bytes, a page size outside 1 to [`limits::MAX_PAGE_SIZE`], and an object type that
/// does not exist; list the object types to search and, of each, the searchable fields the reader
/// is not statically barred from reading.
pub fn plan(
    tenant: &TenantState,
    request: &SearchRequest,
    ctx: &Context,
) -> Result<Plan, DomainError> {
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(refuse(format!(
            "a query is at most {MAX_QUERY_BYTES} bytes"
        )));
    }
    let tokens = tokens_of(&request.query);
    if tokens.is_empty() {
        return Err(refuse("a query has at least one word"));
    }
    if tokens.len() > MAX_QUERY_TOKENS {
        return Err(refuse(format!(
            "a query has at most {MAX_QUERY_TOKENS} different words"
        )));
    }
    let page_size = request.page_size.unwrap_or(limits::DEFAULT_PAGE_SIZE);
    if page_size == 0 || page_size > limits::MAX_PAGE_SIZE {
        return Err(refuse(format!(
            "a page is 1 to {} hits",
            limits::MAX_PAGE_SIZE
        )));
    }
    let schema = &tenant.schema;
    let wanted: BTreeSet<&ObjectTypeId> = request.objects.iter().collect();
    for object in &wanted {
        if !schema.objects.contains_key(*object) {
            return Err(DomainError::new(
                ErrorKind::UnknownObject,
                format!("no object type {object}"),
            ));
        }
    }
    let roles = roles_of(tenant, &ctx.principal);
    let no_params = BTreeMap::new();
    let barred = |rule: &Expr| -> bool {
        if matches!(ctx.principal, Principal::System) || !record_independent(rule) {
            return false;
        }
        let env = Env {
            record: None,
            old: None,
            new: None,
            params: &no_params,
            now: ctx.now,
            principal: &ctx.principal,
            roles: &roles,
            lookup: None,
        };
        !expr::holds(rule, &env).unwrap_or(false)
    };
    let mut objects = Vec::new();
    for (id, def) in &schema.objects {
        if !wanted.is_empty() && !wanted.contains(id) {
            continue;
        }
        let fields: Vec<FieldId> = indexed_fields(def)
            .into_iter()
            .filter(|field| {
                !def.fields
                    .get(field)
                    .and_then(|f| f.read.as_ref())
                    .is_some_and(&barred)
            })
            .collect();
        if !fields.is_empty() {
            objects.push((id.clone(), fields));
        }
    }
    Ok(Plan {
        tokens,
        objects,
        page_size,
        after: request.cursor.clone(),
    })
}

// ---------------------------------------------------------------------------------------------
// Deciding candidates
// ---------------------------------------------------------------------------------------------

/// A record found by a search, as the reader receives it, and the fields that matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    /// The object type of the record.
    pub object: ObjectTypeId,
    /// The record without the fields the reader may not read.
    pub record: Arc<Record>,
    /// The fields that matched, each one the reader may read.
    pub matched: Vec<FieldId>,
}

/// The answer to a search: the hits of the page in order, the cursor of the page after (absent
/// when the candidates ended), how many candidates were examined, and the revision and schema
/// version of the snapshot the page was decided on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub next: Option<SearchCursor>,
    pub examined: u32,
    pub revision: StateRevision,
    pub schema_version: SchemaVersion,
}

/// What [`decide`] made of a run of candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    pub hits: Vec<SearchHit>,
    /// How many candidates were examined: all of them, or up to and including the one that
    /// filled the page.
    pub consumed: usize,
}

/// Decide candidates of `object`, in order, for the reader of `ctx`, stopping with the candidate
/// that makes the `room`-th hit. A candidate is a hit when the record exists, the reader may read
/// it, and at least one field it matched in is a field the reader may read and still matches the
/// query in the record as the reader receives it. `InsufficientState` names a record the snapshot
/// lacks.
pub fn decide(
    state: &State,
    plan: &Plan,
    object: &ObjectTypeId,
    candidates: &[Candidate],
    room: usize,
    ctx: &Context,
) -> Result<Decided, DomainError> {
    if state.tenant_id != ctx.tenant {
        return Err(DomainError::new(
            ErrorKind::TenantMismatch,
            "snapshot is of another tenant",
        ));
    }
    let tenant = state
        .tenant
        .as_deref()
        .filter(|tenant| tenant.id == ctx.tenant)
        .ok_or_else(|| DomainError::new(ErrorKind::TenantNotFound, "no such tenant"))?;
    let lookup = Lookup {
        tenant,
        coverage: &state.coverage,
    };
    let def = tenant.schema.objects.get(object).ok_or_else(|| {
        DomainError::new(ErrorKind::UnknownObject, format!("no object type {object}"))
    })?;
    let roles = roles_of(tenant, &ctx.principal);
    let probed: BTreeSet<&FieldId> = plan
        .objects
        .iter()
        .filter(|(id, _)| id == object)
        .flat_map(|(_, fields)| fields)
        .collect();
    let mut hits = Vec::new();
    let mut consumed = 0;
    for candidate in candidates {
        if hits.len() >= room {
            break;
        }
        consumed += 1;
        let Some(record) = lookup.record(object, candidate.id)? else {
            continue;
        };
        if !visible_in(lookup, &ctx.principal, record, ctx.now)? {
            continue;
        }
        let shown = redact(lookup, &ctx.principal, &roles, ctx.now, record)?;
        let named: Vec<FieldId> = candidate
            .fields
            .iter()
            .filter(|field| probed.contains(field) && shown.fields.contains_key(*field))
            .cloned()
            .collect();
        let matched = matching_fields(def, &shown, &named, &plan.tokens);
        if !matched.is_empty() {
            hits.push(SearchHit {
                object: object.clone(),
                record: shown,
                matched,
            });
        }
    }
    Ok(Decided { hits, consumed })
}

// ---------------------------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------------------------

/// What a search runs against: a store that answers the probes of its index and loads the state a
/// decision needs. `domain` makes the error of the source from a refusal of the kernel.
pub trait Source {
    type Error;
    fn domain(&self, error: DomainError) -> Self::Error;
    /// The candidates of a probe, in ascending record id.
    fn candidates(&mut self, probe: &Probe) -> Result<Vec<Candidate>, Self::Error>;
    /// A snapshot that holds at least the state `needs` names; with no needs, the header of the
    /// tenant.
    fn load(&mut self, needs: &[Need]) -> Result<State, Self::Error>;
}

/// The cursor of a candidate.
fn position(object: &ObjectTypeId, id: RecordId) -> SearchCursor {
    SearchCursor {
        object: object.clone(),
        id,
    }
}

/// Decide a run of candidates, loading the state it needs and asking again for each record a
/// policy or a read rule follows into that the snapshot lacks.
fn decide_loaded<S: Source>(
    source: &mut S,
    plan: &Plan,
    object: &ObjectTypeId,
    candidates: &[Candidate],
    room: usize,
    ctx: &Context,
) -> Result<Decided, S::Error> {
    let mut needs: Vec<Need> = candidates
        .iter()
        .map(|candidate| Need::Record {
            object: object.clone(),
            id: candidate.id,
        })
        .collect();
    for _ in 0..MAX_ROUNDS {
        let state = source.load(&needs)?;
        match decide(&state, plan, object, candidates, room, ctx) {
            Err(DomainError {
                kind: ErrorKind::InsufficientState,
                missing: Some(need),
                ..
            }) => {
                // A rule that follows a reference reaches a record per row: load the whole type
                // it points into, so one more round answers every row.
                let more = match need {
                    Need::Record { object, .. } => Need::AllOf(object),
                    other => other,
                };
                if needs.contains(&more) {
                    return Err(source.domain(DomainError::new(
                        ErrorKind::InsufficientState,
                        "the state a search needs could not be loaded",
                    )));
                }
                needs.push(more);
            }
            other => return other.map_err(|error| source.domain(error)),
        }
    }
    Err(source.domain(DomainError::new(
        ErrorKind::LimitExceeded,
        "a search kept asking for more state than it was given",
    )))
}

/// Run a search for the reader of `ctx` against a source.
///
/// Object types are searched in ascending order, each in ascending record id, [`CANDIDATE_PAGE`]
/// candidates at a time, until the page holds its hits, the candidates end, or
/// [`MAX_CANDIDATES`] have been examined.
pub fn run<S: Source>(
    source: &mut S,
    request: &SearchRequest,
    ctx: &Context,
) -> Result<SearchResult, S::Error> {
    let header = source.load(&[])?;
    if header.tenant_id != ctx.tenant {
        return Err(source.domain(DomainError::new(
            ErrorKind::TenantMismatch,
            "snapshot is of another tenant",
        )));
    }
    let Some(tenant) = header.tenant.as_deref().filter(|t| t.id == ctx.tenant) else {
        return Err(source.domain(DomainError::new(
            ErrorKind::TenantNotFound,
            "no such tenant",
        )));
    };
    let plan = plan(tenant, request, ctx).map_err(|error| source.domain(error))?;
    let (revision, schema_version) = (tenant.revision, tenant.schema.version);

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut examined: u32 = 0;
    let mut next: Option<SearchCursor> = None;
    let mut last: Option<SearchCursor> = None;
    'objects: for (object, fields) in &plan.objects {
        let mut after = match &plan.after {
            Some(cursor) if cursor.object > *object => continue,
            Some(cursor) if cursor.object == *object => Some(cursor.id),
            _ => None,
        };
        loop {
            if examined >= MAX_CANDIDATES {
                next = last.clone();
                break 'objects;
            }
            let limit = CANDIDATE_PAGE.min(MAX_CANDIDATES - examined);
            let probe = Probe {
                object: object.clone(),
                fields: fields.clone(),
                tokens: plan.tokens.clone(),
                after,
                limit,
            };
            let found = source.candidates(&probe)?;
            if found.is_empty() {
                break;
            }
            let room = plan.page_size as usize - hits.len();
            let decided = decide_loaded(source, &plan, object, &found, room, ctx)?;
            examined += decided.consumed as u32;
            hits.extend(decided.hits);
            let consumed_last = found[decided.consumed.max(1) - 1].id;
            last = Some(position(object, consumed_last));
            if hits.len() >= plan.page_size as usize {
                next = last.clone();
                break 'objects;
            }
            if (found.len() as u32) < limit {
                break;
            }
            after = Some(consumed_last);
        }
    }
    Ok(SearchResult {
        hits,
        next,
        examined,
        revision,
        schema_version,
    })
}
