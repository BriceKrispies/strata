//! The enterprise corpus (`strata-corpus`) as the harness's tenant (WG-5398).
//!
//! [`EnterpriseLoader`] installs and populates the corpus (or addresses a populated store it is
//! handed) and returns an [`EnterpriseTenant`]. What a scenario operation maps to, in corpus
//! terms:
//!
//! - **ids, hot sets**: the corpus catalogue, hot ids first. For a reused store the catalogue is
//!   not persisted, so it is derived from the data: ids in creation order, the hot set the most
//!   referenced records of the type, the hot people the most common first names.
//! - **by-field**: the first unique text field of the type (a slug, a SKU code, an email).
//! - **create**: a record cloned from an existing one of the type with its unique fields made
//!   fresh. **update**: one plain text (or, failing that, integer) field of the type, not a
//!   workflow field and not personal. Both are tried against the kernel's decision when the tenant
//!   is opened; a type the kernel refuses is left without the operation and the operation is
//!   reported as skipped.
//! - **named queries**: the corpus query with parameters taken from values the data holds; one the
//!   store refuses at opening is dropped.
//! - **ledger**: a posting to the `commerce__stock` unit's pool and a ranked read of the unit.
//! - **erasure**: the data subject is a hot first name (the corpus declares each personal field as
//!   its own subject, and a name is a valid principal id). The probes are the records holding that
//!   name in a personal first-name field, found before the erasure.
//! - **schema change**: an additive optional field on a corpus object type, through
//!   `change-schema` (not a product re-install).
//!
//! Actors: the corpus has one principal, the owner, so `owner`, `editor` and `reader` all act as
//! it; `system` is the system principal.

use crate::corpus::{
    Actor, Catalogue, CorpusLoader, CorpusTenant, Erasure, Lookup, NotesLoader, ObjectIds, Probe,
    TenantRequest,
};
use crate::decide::dry_run;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
use strata_corpus::{
    build_with, compile_corpus, Catalogue as CorpusCatalogue, CorpusPaths, HotPerson, LoadOptions,
    PopulateOptions, Scale, TypeEntry,
};
use strata_kernel::ext::{DomainErrorExt, RecordExt, TenantStateExt};
use strata_kernel::{
    Context, FieldDef, FieldId, FieldType, Intent, LedgerAmount, LedgerOrder, LedgerPoolName,
    LedgerPostingRequest, LedgerScope, LedgerUnit, Need, ObjectTypeDef, ObjectTypeId, Principal,
    PrincipalId, ReadRequest, RecordId, SchemaChange, TenantId, TenantState, Timestamp, Value,
};
use strata_storage_api::{query, Store};

const TENANT: &str = "enterprise";
const OWNER: &str = "user:owner";
/// The clock of the opening trials: after everything the population wrote.
const TRIAL_NOW: i64 = 1_790_000_000_000;

pub struct EnterpriseLoader {
    pub scale: Scale,
}

impl CorpusLoader for EnterpriseLoader {
    fn populate(
        &self,
        store: Arc<dyn Store>,
        request: &TenantRequest,
    ) -> Result<Box<dyn CorpusTenant>, String> {
        if request.small {
            return NotesLoader.populate(store, request);
        }
        let products = compile_corpus(&CorpusPaths::default()).map_err(|e| e.to_string())?;
        let options = LoadOptions {
            replace: true,
            ..LoadOptions::default()
        };
        let populate = PopulateOptions::new(self.scale, request.seed);
        let mut finished = 0u64;
        let built = build_with(
            store.as_ref(),
            &products,
            &options,
            &populate,
            &mut |progress| {
                if progress.done == progress.total {
                    finished += 1;
                    eprintln!(
                        "ws-bench: built {} ({} records), {} commits so far",
                        progress.object, progress.total, progress.commits
                    );
                }
            },
        )
        .map_err(|e| e.to_string())?;
        EnterpriseTenant::open(store, Some(built.catalogue), self.scale)
    }

    fn reuse(
        &self,
        store: Arc<dyn Store>,
        request: &TenantRequest,
    ) -> Result<Box<dyn CorpusTenant>, String> {
        if request.small {
            return NotesLoader.populate(store, request);
        }
        EnterpriseTenant::open(store, None, self.scale)
    }
}

struct CreatePlan {
    template: BTreeMap<FieldId, Value>,
    uniques: Vec<(FieldId, bool)>,
}

impl CreatePlan {
    fn fields(&self, n: u64) -> BTreeMap<FieldId, Value> {
        let mut fields = self.template.clone();
        for (field, int) in &self.uniques {
            let fresh = match (fields.get(field), int) {
                (_, true) => Value::Int(2_000_000_000 + n as i64),
                (Some(Value::Str(old)), false) => Value::Str(format!("{old}-w{n}")),
                _ => Value::Str(format!("w{n}")),
            };
            fields.insert(field.clone(), fresh);
        }
        fields
    }
}

struct UpdateField {
    field: FieldId,
    int: bool,
}

pub struct EnterpriseTenant {
    store: Arc<dyn Store>,
    tenant: TenantId,
    owner: PrincipalId,
    label: String,
    catalogue: Catalogue,
    corpus: CorpusCatalogue,
    creates: BTreeMap<ObjectTypeId, CreatePlan>,
    updates: BTreeMap<ObjectTypeId, UpdateField>,
    queries: BTreeMap<String, Vec<ReadRequest>>,
    /// The ledger units a posting is accepted for, with their pools.
    ledger: Vec<(LedgerUnit, LedgerPoolName)>,
    /// The member that acts as an editor or a reader of a module.
    actors: BTreeMap<(String, Actor), PrincipalId>,
    upgrade: Option<ObjectTypeId>,
    /// Compiled bundles of the products an upgrade needs, by product id, compiled on first use.
    products: std::sync::Mutex<BTreeMap<String, strata_corpus::CompiledProduct>>,
    /// What the opening found it could not map, for the log.
    pub unmapped: Vec<String>,
}

fn ctx(tenant: &TenantId, principal: Principal) -> Context {
    Context {
        tenant: tenant.clone(),
        principal,
        session: None,
        now: Timestamp(TRIAL_NOW),
        correlation: "ws-bench-open".to_string(),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

fn accepted(store: &dyn Store, intent: &Intent, context: &Context) -> bool {
    matches!(dry_run(store, intent, context), Ok(Ok(_)))
}

/// A catalogue read off the data, for a store that was populated by another process.
fn derive(state: &TenantState) -> CorpusCatalogue {
    let mut catalogue = CorpusCatalogue {
        tenant: state.id.to_string(),
        owner: OWNER.to_string(),
        scale: "reused".to_string(),
        ..CorpusCatalogue::default()
    };
    let mut referenced: BTreeMap<(ObjectTypeId, u64), u64> = BTreeMap::new();
    for (object, records) in &state.records {
        let Some(def) = state.schema.objects.get(object) else {
            continue;
        };
        for record in records.values() {
            for (field, def_field) in &def.fields {
                if let FieldType::Ref { target, .. } = &def_field.ty {
                    if let Value::Ref(id) = record.get(field) {
                        *referenced.entry((target.clone(), id.0)).or_default() += 1;
                    }
                }
            }
        }
    }
    for (principal, roles) in &state.members {
        for role in roles.iter().filter(|r| r.as_str().contains("__")) {
            catalogue
                .members
                .insert(role.to_string(), principal.to_string());
        }
    }
    let mut first_names: BTreeMap<String, u64> = BTreeMap::new();
    for (object, def) in &state.schema.objects {
        let Some(records) = state.records.get(object) else {
            continue;
        };
        let mut ids: Vec<u64> = records.keys().map(|id| id.0).collect();
        ids.sort_unstable();
        let mut by_use = ids.clone();
        by_use.sort_by_key(|id| {
            std::cmp::Reverse(referenced.get(&(object.clone(), *id)).copied().unwrap_or(0))
        });
        let hot: Vec<u64> = by_use
            .iter()
            .copied()
            .filter(|id| referenced.contains_key(&(object.clone(), *id)))
            .take((ids.len() / 20).max(1))
            .collect();
        let personal: Vec<String> = def
            .fields
            .values()
            .filter(|f| f.personal.is_some())
            .map(|f| f.id.to_string())
            .collect();
        for field in def
            .fields
            .values()
            .filter(|f| f.personal.is_some() && f.id.as_str().contains("first"))
        {
            for record in records.values() {
                if let Value::Str(name) = record.get(&field.id) {
                    *first_names.entry(name.clone()).or_default() += 1;
                }
            }
        }
        let module = object.as_str().split("__").next().unwrap_or("").to_string();
        catalogue.records += ids.len() as u64;
        catalogue.types.insert(
            object.to_string(),
            TypeEntry {
                module,
                hot: if hot.is_empty() {
                    ids.iter().copied().take(3).collect()
                } else {
                    hot
                },
                ids,
                personal,
                ..TypeEntry::default()
            },
        );
    }
    let mut names: Vec<(String, u64)> = first_names.into_iter().collect();
    names.sort_by_key(|(name, count)| (std::cmp::Reverse(*count), name.clone()));
    catalogue.hot_people = names
        .into_iter()
        .take(8)
        .map(|(name, _)| HotPerson {
            name,
            email: String::new(),
        })
        .collect();
    catalogue
}

impl EnterpriseTenant {
    pub fn open(
        store: Arc<dyn Store>,
        built: Option<CorpusCatalogue>,
        scale: Scale,
    ) -> Result<Box<dyn CorpusTenant>, String> {
        let tenant = TenantId::new(TENANT).map_err(|e| format!("{e:?}"))?;
        let owner = PrincipalId::new(OWNER).map_err(|e| format!("{e:?}"))?;
        let state = store.load_complete(&tenant).map_err(|e| e.to_string())?;
        let ts = state
            .tenant
            .ok_or_else(|| format!("the store holds no tenant {TENANT}"))?;
        let corpus = built.unwrap_or_else(|| derive(&ts));
        let as_owner = ctx(&tenant, Principal::User(owner.clone()));
        let mut unmapped = Vec::new();

        let mut objects = BTreeMap::new();
        let mut creates = BTreeMap::new();
        let mut updates = BTreeMap::new();
        for (name, entry) in &corpus.types {
            if entry.ids.is_empty() {
                continue;
            }
            let Ok(object) = ObjectTypeId::new(name.clone()) else {
                continue;
            };
            let Some(def) = ts.schema.objects.get(&object) else {
                continue;
            };
            let records = ts.records.get(&object);
            // Hottest first, then the rest in creation order.
            let mut ids: Vec<u64> = entry.hot.clone();
            let hot_set: HashSet<u64> = entry.hot.iter().copied().collect();
            ids.extend(entry.ids.iter().copied().filter(|id| !hot_set.contains(id)));
            let lookup = def
                .fields
                .values()
                .find(|f| f.unique && f.ty == FieldType::Str)
                .map(|field| Lookup {
                    field: field.id.clone(),
                    values: ids
                        .iter()
                        .map(|id| {
                            records
                                .and_then(|r| r.get(&RecordId(*id)))
                                .map_or(Value::Null, |record| record.get(&field.id).clone())
                        })
                        .collect(),
                });
            // A create plan from the first template record the kernel accepts a copy of.
            let templates = [ids.first(), ids.get(entry.hot.len()), ids.last()];
            for id in templates.into_iter().flatten() {
                let Some(record) = records.and_then(|r| r.get(&RecordId(*id))) else {
                    continue;
                };
                let Some(uniques) = unique_fields(def) else {
                    break;
                };
                let plan = CreatePlan {
                    template: record.fields.clone(),
                    uniques,
                };
                let intent = Intent::CreateRecord {
                    object: object.clone(),
                    fields: plan.fields(0),
                };
                if accepted(store.as_ref(), &intent, &as_owner) {
                    creates.insert(object.clone(), plan);
                    break;
                }
            }
            if !creates.contains_key(&object) {
                unmapped.push(format!("create {object}"));
            }
            // An update field the kernel accepts.
            let target = RecordId(ids[0]);
            for candidate in update_candidates(&ts, def) {
                let intent = Intent::UpdateRecord {
                    object: object.clone(),
                    id: target,
                    fields: BTreeMap::from([(
                        candidate.field.clone(),
                        update_value(&candidate, 0),
                    )]),
                    expected_version: None,
                };
                if accepted(store.as_ref(), &intent, &as_owner) {
                    updates.insert(object.clone(), candidate);
                    break;
                }
            }
            if !updates.contains_key(&object) {
                unmapped.push(format!("update {object}"));
            }
            objects.insert(
                object,
                ObjectIds {
                    hot: entry.hot.len().max(1),
                    ids: ids.into_iter().map(RecordId).collect(),
                    lookup,
                },
            );
        }

        // The members of each module: an editor is the member the kernel lets update most of the
        // module's types, a reader the one it lets update fewest.
        let mut by_module: BTreeMap<String, Vec<PrincipalId>> = BTreeMap::new();
        for (role, principal) in &corpus.members {
            if let (Some(module), Ok(principal)) =
                (role.split("__").next(), PrincipalId::new(principal.clone()))
            {
                by_module
                    .entry(module.to_string())
                    .or_default()
                    .push(principal);
            }
        }
        let mut actors = BTreeMap::new();
        for (module, members) in &by_module {
            let mut scored: Vec<(usize, &PrincipalId)> = Vec::new();
            for member in members {
                let as_member = ctx(&tenant, Principal::User(member.clone()));
                let mut allowed = 0;
                for (object, field) in &updates {
                    let in_module = object.as_str().split("__").next() == Some(module.as_str());
                    let Some(first) = objects.get(object).and_then(|o: &ObjectIds| o.ids.first())
                    else {
                        continue;
                    };
                    if !in_module {
                        continue;
                    }
                    let intent = Intent::UpdateRecord {
                        object: object.clone(),
                        id: *first,
                        fields: BTreeMap::from([(field.field.clone(), update_value(field, 0))]),
                        expected_version: None,
                    };
                    if accepted(store.as_ref(), &intent, &as_member) {
                        allowed += 1;
                    }
                }
                scored.push((allowed, member));
            }
            // Ties go to the first member for an editor and the last for a reader.
            if let Some((_, editor)) = scored.iter().max_by_key(|(n, _)| *n).map(|(n, m)| (*n, *m))
            {
                actors.insert((module.clone(), Actor::Editor), editor.clone());
            }
            if let Some((_, reader)) = scored
                .iter()
                .rev()
                .min_by_key(|(n, _)| *n)
                .map(|(n, m)| (*n, *m))
            {
                actors.insert((module.clone(), Actor::Reader), reader.clone());
            }
        }
        if actors.is_empty() {
            unmapped.push(
                "actors: the store holds no module members, so every operation acts as the owner"
                    .to_string(),
            );
        }

        // Named queries, with parameters the data holds.
        let mut queries: BTreeMap<String, Vec<ReadRequest>> = BTreeMap::new();
        for (qid, def) in &ts.schema.queries {
            let object_def = ts.schema.objects.get(&def.object);
            let mut variants: Vec<BTreeMap<String, Value>> = vec![BTreeMap::new()];
            let mut mappable = true;
            for param in &def.params {
                let values = object_def
                    .zip(FieldId::new(param.clone()).ok())
                    .filter(|(o, f)| o.fields.contains_key(f))
                    .map(|(_, field)| distinct_values(&ts, &def.object, &field, 6))
                    .unwrap_or_default();
                if values.is_empty() {
                    mappable = false;
                    break;
                }
                variants = variants
                    .into_iter()
                    .flat_map(|base| {
                        values.iter().map(move |value| {
                            let mut with = base.clone();
                            with.insert(param.clone(), value.clone());
                            with
                        })
                    })
                    .collect();
            }
            if !mappable {
                unmapped.push(format!("query {qid} (no values for its parameters)"));
                continue;
            }
            let kept: Vec<ReadRequest> = variants
                .into_iter()
                .take(8)
                .map(|params| ReadRequest::Named {
                    query: qid.clone(),
                    params,
                    page_size: Some(def.limit.unwrap_or(50).min(100)),
                    cursor: None,
                })
                .filter(|request| query(store.as_ref(), request, &as_owner).is_ok())
                .collect();
            if kept.is_empty() {
                unmapped.push(format!("query {qid} (the store refuses it)"));
            } else {
                queries.insert(qid.to_string(), kept);
            }
        }

        // The ledger: a posting to the stock pool of commerce and to the budget pool of
        // automation. A direct posting needs the capability `ledger-post`, which no corpus role
        // grants and only the owner holds, so the owner posts.
        let mut ledger = Vec::new();
        for (unit, pool) in [
            ("commerce__stock", "commerce__stock_pool"),
            ("automation__api_calls", "automation__budget_pool"),
        ] {
            let (Ok(unit), Ok(pool)) = (LedgerUnit::new(unit), LedgerPoolName::new(pool)) else {
                continue;
            };
            let probe = posting(&unit, &pool, 0);
            match dry_run(store.as_ref(), &probe, &as_owner) {
                Ok(Ok(_)) => ledger.push((unit, pool)),
                Ok(Err(refused)) => unmapped.push(format!(
                    "ledger postings to {} are refused for the owner: {}",
                    unit.0,
                    refused.describe()
                )),
                Err(e) => unmapped.push(format!("ledger postings to {}: {e}", unit.0)),
            }
        }

        let upgrade = ObjectTypeId::new("content__article")
            .ok()
            .filter(|o| objects.contains_key(o))
            .or_else(|| objects.keys().next().cloned());
        let label = scale.name().to_string();
        if !unmapped.is_empty() {
            eprintln!(
                "ws-bench: {} operation(s) have no mapping on this tenant, for example: {}",
                unmapped.len(),
                unmapped
                    .iter()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        Ok(Box::new(EnterpriseTenant {
            store,
            tenant,
            owner,
            label,
            catalogue: Catalogue { objects },
            corpus,
            creates,
            updates,
            queries,
            ledger,
            actors,
            upgrade,
            products: std::sync::Mutex::new(BTreeMap::new()),
            unmapped,
        }))
    }
}

fn unique_fields(def: &ObjectTypeDef) -> Option<Vec<(FieldId, bool)>> {
    let mut uniques = Vec::new();
    for field in def.fields.values().filter(|f| f.unique) {
        match field.ty {
            FieldType::Str => uniques.push((field.id.clone(), false)),
            FieldType::Int => uniques.push((field.id.clone(), true)),
            // A unique reference or time has no fresh value to make: no create plan.
            _ => return None,
        }
    }
    Some(uniques)
}

fn update_candidates(ts: &TenantState, def: &ObjectTypeDef) -> Vec<UpdateField> {
    let workflow_fields: BTreeSet<&FieldId> = ts
        .schema
        .workflows
        .values()
        .filter(|w| w.object == def.id)
        .map(|w| &w.field)
        .collect();
    let plain = |f: &&FieldDef| {
        !f.unique
            && f.personal.is_none()
            && f.read.is_none()
            && !workflow_fields.contains(&f.id)
            && !f.id.as_str().contains("status")
            && !f.id.as_str().contains("state")
    };
    let liked = |f: &FieldDef| {
        ["title", "name", "summary", "note", "description", "label"]
            .iter()
            .any(|w| f.id.as_str().contains(w))
    };
    let mut strs: Vec<&FieldDef> = def
        .fields
        .values()
        .filter(plain)
        .filter(|f| f.ty == FieldType::Str)
        .collect();
    strs.sort_by_key(|f| !liked(f));
    let ints = def
        .fields
        .values()
        .filter(plain)
        .filter(|f| f.ty == FieldType::Int);
    strs.into_iter()
        .map(|f| UpdateField {
            field: f.id.clone(),
            int: false,
        })
        .chain(ints.map(|f| UpdateField {
            field: f.id.clone(),
            int: true,
        }))
        .collect()
}

fn update_value(field: &UpdateField, n: u64) -> Value {
    if field.int {
        Value::Int((n % 1000) as i64)
    } else {
        Value::Str(format!("ws {n}"))
    }
}

fn distinct_values(
    ts: &TenantState,
    object: &ObjectTypeId,
    field: &FieldId,
    most: usize,
) -> Vec<Value> {
    let mut seen = Vec::new();
    for record in ts.records_of(object) {
        let value = record.get(field);
        if matches!(value, Value::Null) || seen.contains(value) {
            continue;
        }
        seen.push(value.clone());
        if seen.len() >= most {
            break;
        }
    }
    seen
}

fn posting(unit: &LedgerUnit, pool: &LedgerPoolName, n: u64) -> Intent {
    let amount = if n % 4 == 3 { -1 } else { (n % 5 + 1) as i64 };
    Intent::PostLedger {
        postings: vec![LedgerPostingRequest {
            owner: strata_kernel::LedgerOwner::Pool { pool: pool.clone() },
            unit: unit.clone(),
            amount: LedgerAmount(amount),
            idempotency_key: format!("ws-bench-{n}"),
            reason: "ws-bench stock posting".to_string(),
            corrects: None,
            scope: None,
        }],
    }
}

impl CorpusTenant for EnterpriseTenant {
    fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    fn unmapped(&self) -> Vec<String> {
        self.unmapped.clone()
    }

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    fn actor(&self, who: Actor, subject: Option<&str>) -> Principal {
        match who {
            Actor::System => Principal::System,
            Actor::Owner => Principal::User(self.owner.clone()),
            Actor::Editor | Actor::Reader => subject
                .and_then(|s| s.split("__").next())
                .and_then(|module| self.actors.get(&(module.to_string(), who)))
                .map_or_else(
                    || Principal::User(self.owner.clone()),
                    |member| Principal::User(member.clone()),
                ),
        }
    }

    fn create_fields(&self, object: &ObjectTypeId, n: u64) -> Option<BTreeMap<FieldId, Value>> {
        self.creates.get(object).map(|plan| plan.fields(n))
    }

    fn update_fields(
        &self,
        object: &ObjectTypeId,
        _id: RecordId,
        n: u64,
    ) -> Option<BTreeMap<FieldId, Value>> {
        self.updates
            .get(object)
            .map(|field| BTreeMap::from([(field.field.clone(), update_value(field, n))]))
    }

    fn named_query(&self, query: &str, n: u64) -> Option<ReadRequest> {
        let variants = self.queries.get(query)?;
        variants.get(n as usize % variants.len()).cloned()
    }

    fn ledger_need(&self, n: u64) -> Option<Need> {
        let (unit, _) = self.ledger.get(n as usize % self.ledger.len().max(1))?;
        Some(Need::Ledger(strata_kernel::LedgerNeed::Ranked {
            unit: unit.clone(),
            scope: LedgerScope::new("").ok()?,
            order: LedgerOrder::GreatestFirst,
            offset: 0,
            limit: 20,
        }))
    }

    fn ledger_intent(&self, n: u64) -> Option<Intent> {
        let (unit, pool) = self.ledger.get(n as usize % self.ledger.len().max(1))?;
        Some(posting(unit, pool, n))
    }

    fn erasure(&self, n: u64) -> Option<Erasure> {
        let people = &self.corpus.hot_people;
        if people.is_empty() {
            return None;
        }
        let name = &people[n as usize % people.len()].name;
        let first = name.split_whitespace().next()?.to_string();
        let subject = PrincipalId::new(first.clone()).ok()?;
        let mut probes = Vec::new();
        for (object_name, entry) in &self.corpus.types {
            let object = ObjectTypeId::new(object_name.clone()).ok()?;
            for field in entry
                .personal
                .iter()
                .filter(|f| f.contains("first") || f.contains("given"))
            {
                let field = FieldId::new(field.clone()).ok()?;
                let state = self
                    .store
                    .load(
                        &self.tenant,
                        &[Need::ByField {
                            object: object.clone(),
                            field: field.clone(),
                            value: Value::Str(first.clone()),
                        }],
                    )
                    .ok()?;
                for record in state.tenant.as_deref()?.records_of(&object) {
                    if probes.len() < 200 {
                        probes.push(Probe {
                            object: object.clone(),
                            id: record.id,
                            field: field.clone(),
                            plaintext: Value::Str(first.clone()),
                        });
                    }
                }
            }
        }
        (!probes.is_empty()).then_some(Erasure { subject, probes })
    }

    /// A product upgrade: the product that owns the upgrade object type is re-installed at a later
    /// version of its bundle with one more optional field on that type, through the install path
    /// the loader uses (plan, one change-schema intent, ownership record).
    fn evolve(&self, n: u64) -> Option<Result<String, String>> {
        let object = self.upgrade.clone()?;
        let module = object.as_str().split("__").next()?.to_string();
        let product = {
            let mut cache = self.products.lock().ok()?;
            if !cache.contains_key(&module) {
                match strata_corpus::compile_product(&CorpusPaths::default(), &module) {
                    Ok(compiled) => {
                        cache.insert(module.clone(), compiled);
                    }
                    Err(e) => return Some(Err(e.to_string())),
                }
            }
            cache.get(&module)?.clone()
        };
        let field = FieldDef {
            id: FieldId::new(format!("ws_upgrade_{n}")).ok()?,
            label: format!("Upgrade {n}"),
            ty: FieldType::Str,
            required: false,
            unique: false,
            default: None,
            read: None,
            personal: None,
        };
        // Later than any version the file declares, and rising with every upgrade of the run.
        let version = format!("99.0.{n}");
        Some(
            strata_corpus::upgrade_product_with_field(
                self.store.as_ref(),
                &self.tenant,
                &self.owner,
                TRIAL_NOW + n as i64 * 10,
                &product,
                &object,
                field,
                &version,
            )
            .map(|schema| {
                format!("product {module} upgraded to {version} (schema version {schema})")
            })
            .map_err(|problems| problems.join("; ")),
        )
    }

    fn schema_change(&self, n: u64) -> Option<Vec<SchemaChange>> {
        let object = self.upgrade.clone()?;
        Some(vec![SchemaChange::DefineField {
            object,
            field: FieldDef {
                id: FieldId::new(format!("ws_upgrade_{n}")).ok()?,
                label: format!("Upgrade {n}"),
                ty: FieldType::Str,
                required: false,
                unique: false,
                default: None,
                read: None,
                personal: None,
            },
        }])
    }
}
