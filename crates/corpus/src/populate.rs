//! The population generator: records for every object type of the loaded tenant, from a seed.
//!
//! The generator reads the object types from the tenant's schema, so it follows whatever the
//! corpus declares and needs no table per field. What it knows beyond the schema is a share per
//! object type ([`crate::scale::SHARES`]) and the meaning of common field names (an email, a
//! slug, a price, a body of text), which it reads from the names.
//!
//! Every record goes in as a `create-record` intent through `execute`: the kernel decides it
//! (references, unique values, workflows, required fields, limits), the store commits it, and the
//! journal, the events and the automation jobs of the commit are written as in a deployment. A
//! reference that points at a type created later (the current revision of an article, the
//! featured tile of a dashboard) is set by an `update-record` once the target exists.
//!
//! The kernel has no multi-intent commit, so a batch here is a run of intents issued back to back
//! with a progress report after it; each record is one commit and one journal entry. A faster path
//! would have to be a change to the store contract.
//!
//! Realism, each of it a function of the seed and nothing else:
//!
//! - **References** follow the schema: a required reference always points at an existing record,
//!   an optional one is set for most records, a self reference (a parent category, a line
//!   manager) points at an earlier record of the same type.
//! - **Skew**: the target of a reference is drawn from a Zipf distribution over a shuffled order
//!   of the target's records, so a few products, contacts, queues and articles are referenced by
//!   a large share of the rows and most have few or none ([`crate::scale::skew_of`]).
//! - **People** come from one shared population: the same hot authors, customers and
//!   respondents recur across modules, and an object with a unique personal field (a contact, an
//!   employee) takes each person once.
//! - **Sizes** are skewed: most records carry short text, one in six carries several times
//!   more, and one in thirty-three carries tens of times more, up to the field and record limits.

use crate::catalogue::{Catalogue, HotPerson, RefEdge, TypeEntry};
use crate::load::{run, LoadedCorpus};
use crate::rng::{Rng, Zipf};
use crate::scale::{share_of, skew_of, Scale};
use crate::text::{self, Person};
use crate::CorpusError;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use strata_kernel::limits::{MAX_RECORD_BYTES, MAX_STRING_BYTES};
use strata_kernel::{
    Context, FieldId, FieldType, Intent, ObjectTypeId, Output, PersonalKind, Principal, RecordId,
    TenantSchema, TenantSchemaExt, Timestamp, Value,
};
use strata_storage_api::Store;

/// The time the population's data is relative to (2026-05-28T00:00:00Z), in milliseconds.
const NOW: i64 = 1_780_000_000_000;
const DAY: i64 = 86_400_000;
/// The commits of the population start on the second of January 2026, a millisecond apart times
/// [`STEP`].
const START: i64 = 1_767_312_000_000;
const STEP: i64 = 50;

/// What to populate.
#[derive(Clone, Debug)]
pub struct PopulateOptions {
    pub scale: Scale,
    pub seed: u64,
    /// Records between two progress reports.
    pub batch: usize,
    /// Skip a record the kernel refuses and carry on, counting it in
    /// [`Catalogue::rejections`], instead of stopping at the first.
    pub lenient: bool,
}

impl PopulateOptions {
    pub fn new(scale: Scale, seed: u64) -> Self {
        PopulateOptions {
            scale,
            seed,
            batch: 1000,
            lenient: false,
        }
    }
}

/// A progress report: the object type being filled, how many of its records are done and how many
/// it will have.
#[derive(Clone, Debug)]
pub struct Progress {
    pub object: String,
    pub done: u64,
    pub total: u64,
    pub commits: u64,
}

struct FieldPlan {
    id: FieldId,
    name: String,
    ty: FieldType,
    required: bool,
    unique: bool,
    has_default: bool,
    computed: bool,
    personal: Option<PersonalKind>,
    /// The initial states of the workflow on this field, when it has one.
    initial: Option<Vec<String>>,
}

struct TypePlan {
    index: usize,
    id: ObjectTypeId,
    key: String,
    module: String,
    fields: Vec<FieldPlan>,
    count: u64,
    /// An object with a unique personal field takes each person of the shared pool once.
    unique_person: bool,
}

/// The records of a finished type, in the order references to it draw them.
struct Pool {
    ids: Vec<u64>,
    /// Popularity rank to index in `ids`.
    order: Vec<u32>,
    zipf: Zipf,
}

impl Pool {
    fn new(ids: Vec<u64>, skew: f64, rng: &mut Rng) -> Pool {
        let mut order: Vec<u32> = (0..ids.len() as u32).collect();
        rng.shuffle(&mut order);
        let zipf = Zipf::new(ids.len(), skew);
        Pool { ids, order, zipf }
    }

    fn pick(&self, rng: &mut Rng) -> Option<u64> {
        if self.ids.is_empty() {
            return None;
        }
        let rank = self.zipf.sample(rng);
        Some(self.ids[self.order[rank] as usize])
    }

    fn hot(&self, n: usize) -> Vec<u64> {
        self.order
            .iter()
            .take(n)
            .map(|&i| self.ids[i as usize])
            .collect()
    }
}

/// Per record state while its fields are drawn.
struct RecordState {
    n: u64,
    person: Option<usize>,
    /// 1, 6 or 60: how much text this record carries.
    factor: usize,
    bytes: usize,
}

struct Populator<'a> {
    store: &'a dyn Store,
    tenant: strata_kernel::TenantId,
    principal: Principal,
    clock: i64,
    commits: u64,
    people: Vec<Person>,
    people_zipf: Zipf,
    pools: BTreeMap<String, Pool>,
}

fn long_text(name: &str) -> Option<(usize, usize)> {
    const BIG: &[&str] = &[
        "body",
        "content",
        "markdown",
        "html",
        "payload",
        "definition",
        "template",
        "answers",
        "config",
        "transcript",
    ];
    const SMALL: &[&str] = &[
        "description",
        "notes",
        "summary",
        "message",
        "comment",
        "reason",
        "text",
        "details",
        "rationale",
        "instructions",
        "excerpt",
        "abstract",
        "feedback",
        "resolution",
    ];
    if BIG.iter().any(|w| name.contains(w)) {
        Some((400, 2500))
    } else if SMALL.iter().any(|w| name.contains(w)) {
        Some((60, 300))
    } else {
        None
    }
}

fn is_future(name: &str) -> bool {
    [
        "due",
        "expires",
        "expiry",
        "until",
        "deadline",
        "embargo",
        "scheduled",
        "next",
        "valid_to",
        "ends",
        "renewal",
        "release_at",
        "publish_at",
    ]
    .iter()
    .any(|w| name.contains(w))
}

impl Populator<'_> {
    fn commit(&mut self, intent: Intent) -> Result<Output, String> {
        self.clock += STEP;
        let ctx = Context {
            tenant: self.tenant.clone(),
            principal: self.principal.clone(),
            session: None,
            now: Timestamp(self.clock),
            correlation: format!("corpus:{}", self.commits),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
        };
        self.commits += 1;
        run(self.store, &ctx, &intent)
    }

    fn person_of(&mut self, plan: &TypePlan, rec: &mut RecordState, rng: &mut Rng) -> usize {
        if let Some(person) = rec.person {
            return person;
        }
        let person = if plan.unique_person {
            rec.n as usize % self.people.len()
        } else {
            self.people_zipf.sample(rng)
        };
        rec.person = Some(person);
        person
    }

    fn personal(
        &mut self,
        kind: &PersonalKind,
        name: &str,
        plan: &TypePlan,
        rec: &mut RecordState,
        rng: &mut Rng,
    ) -> String {
        let at = self.person_of(plan, rec, rng);
        let person = &self.people[at];
        match kind {
            PersonalKind::Name => {
                if name.contains("first") || name.contains("given") {
                    person.first.clone()
                } else if name.contains("last") || name.contains("family") || name.contains("sur") {
                    person.last.clone()
                } else {
                    person.full()
                }
            }
            PersonalKind::EmailAddress => person.email.clone(),
            PersonalKind::PhoneNumber => person.phone.clone(),
            PersonalKind::PostalAddress => format!(
                "{} {} Street, {}",
                rng.range(1, 240),
                rng.pick(text::WORDS),
                rng.pick(text::CITIES)
            ),
            PersonalKind::Identifier => text::hex(rng, 12),
            _ => text::paragraph(rng, 40),
        }
    }

    /// A text value for a field with no personal declaration, read from the field's name.
    fn text_value(
        &mut self,
        plan: &TypePlan,
        fp: &FieldPlan,
        rec: &mut RecordState,
        rng: &mut Rng,
    ) -> String {
        let name = fp.name.as_str();
        let n = rec.n;
        if name.contains("email") {
            let at = self.person_of(plan, rec, rng);
            return self.people[at].email.clone();
        }
        if name.contains("phone") || name.contains("mobile") {
            let at = self.person_of(plan, rec, rng);
            return self.people[at].phone.clone();
        }
        if fp.unique {
            let word = |rng: &mut Rng| *rng.pick(text::WORDS);
            let value = if name.contains("slug") {
                format!("{}-{}-{n}", word(rng), word(rng))
            } else if name.contains("route") {
                format!("/{}/{}-{n}", word(rng), word(rng))
            } else if name.contains("checksum") || name.contains("hash") {
                format!("{}{n:08x}", text::hex(rng, 56))
            } else if name.contains("code") || name.contains("number") || name.contains("reference")
            {
                format!(
                    "{}-{n:06}",
                    &plan.module[..plan.module.len().min(3)].to_uppercase()
                )
            } else {
                format!("{}-{n}", text::title(rng).replace(' ', "-"))
            };
            return value;
        }
        if let Some((lo, hi)) = long_text(name) {
            let base = rng.range(lo as i64, hi as i64) as usize;
            let room = MAX_RECORD_BYTES
                .saturating_sub(rec.bytes)
                .saturating_sub(MAX_RECORD_BYTES / 4);
            let want = (base * rec.factor)
                .min(MAX_STRING_BYTES / 2)
                .min(room)
                .max(16);
            return text::paragraph(rng, want);
        }
        if name.contains("url")
            || name.contains("link")
            || name.contains("endpoint")
            || name.ends_with("uri")
        {
            return format!(
                "https://{}.example.com/{}",
                rng.pick(text::WORDS),
                text::hex(rng, 8)
            );
        }
        if name.contains("currency") {
            return rng.pick(text::CURRENCIES).to_string();
        }
        if name.contains("locale") || name.contains("language") {
            return rng.pick(text::LOCALES).to_string();
        }
        if name.contains("region") || name.contains("territory") {
            return rng.pick(text::REGIONS).to_string();
        }
        if name.contains("country") {
            return rng.pick(text::COUNTRIES).to_string();
        }
        if name.contains("city") {
            return rng.pick(text::CITIES).to_string();
        }
        if name.contains("status")
            || name.contains("state")
            || name.contains("stage")
            || name.contains("priority")
            || name.contains("channel")
            || name.contains("outcome")
            || name.contains("kind")
            || name.contains("type")
            || name.contains("level")
            || name.contains("purpose")
            || name.contains("direction")
            || name.contains("source")
            || name.contains("method")
            || name.contains("band")
            || name.contains("category")
        {
            return rng.pick(text::categories(name)).to_string();
        }
        if name.contains("slug") {
            return format!("{}-{}-{n}", rng.pick(text::WORDS), rng.pick(text::WORDS));
        }
        if name.contains("name")
            || name.contains("title")
            || name.contains("label")
            || name.contains("subject")
        {
            return text::title(rng);
        }
        if name.ends_with("_id")
            || name.contains("reference")
            || name.contains("code")
            || name.contains("number")
            || name.contains("key")
            || name.contains("token")
            || name.contains("checksum")
            || name.contains("hash")
        {
            return text::hex(rng, 16);
        }
        text::phrase(rng, 2)
    }

    fn int_value(&mut self, fp: &FieldPlan, rec: &RecordState, rng: &mut Rng) -> i64 {
        let name = fp.name.as_str();
        let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
        if fp.unique {
            return rec.n as i64 + 1;
        }
        if has(&[
            "amount", "price", "total", "cost", "budget", "value", "revenue", "fee",
        ]) {
            rng.skewed(500, 5_000_000)
        } else if has(&[
            "quantity", "qty", "count", "units", "items", "attempts", "seats",
        ]) {
            rng.skewed(1, 40)
        } else if has(&["size", "bytes", "length", "weight"]) {
            rng.skewed(1_000, 50_000_000)
        } else if has(&["width", "height", "pixels"]) {
            rng.range(64, 4096)
        } else if has(&["duration", "seconds", "minutes", "ttl", "timeout", "_ms"]) {
            rng.skewed(1, 86_400)
        } else if has(&["percent", "pct", "score", "rating", "rate"]) {
            rng.range(0, 100)
        } else if has(&[
            "priority", "rank", "order", "position", "level", "sort", "sequence",
        ]) {
            rng.range(0, 10)
        } else if has(&["version", "revision", "generation"]) {
            rng.range(1, 20)
        } else if has(&[
            "limit",
            "max",
            "threshold",
            "quota",
            "cap",
            "warn",
            "breach",
        ]) {
            rng.range(10, 10_000)
        } else {
            rng.skewed(0, 1000)
        }
    }

    fn time_value(&mut self, fp: &FieldPlan, rng: &mut Rng) -> i64 {
        if is_future(&fp.name) {
            NOW + rng.skewed(3_600_000, 180 * DAY)
        } else {
            NOW - rng.skewed(60_000, 540 * DAY)
        }
    }

    /// The value of one field, or `None` to leave it out.
    #[allow(clippy::too_many_arguments)]
    fn value(
        &mut self,
        plan: &TypePlan,
        fp: &FieldPlan,
        ty: &FieldType,
        rec: &mut RecordState,
        rng: &mut Rng,
        field_index: usize,
        back: &mut BackIndex,
        own_ids: &[u64],
    ) -> Option<Value> {
        match ty {
            FieldType::Str => {
                if let Some(kind) = fp.personal {
                    let text = self.personal(&kind, &fp.name, plan, rec, rng);
                    return Some(Value::Str(text));
                }
                Some(Value::Str(self.text_value(plan, fp, rec, rng)))
            }
            FieldType::Int => Some(Value::Int(self.int_value(fp, rec, rng))),
            FieldType::Bool => {
                let name = fp.name.as_str();
                let p = if [
                    "active",
                    "enabled",
                    "published",
                    "granted",
                    "verified",
                    "default",
                ]
                .iter()
                .any(|w| name.contains(w))
                {
                    0.8
                } else if ["do_not", "deleted", "archived", "blocked", "opt_out"]
                    .iter()
                    .any(|w| name.contains(w))
                {
                    0.1
                } else {
                    0.3
                };
                Some(Value::Bool(rng.chance(p)))
            }
            FieldType::Time => Some(Value::Time(self.time_value(fp, rng))),
            FieldType::Language => Some(Value::Str(rng.pick(text::LOCALES).to_string())),
            FieldType::File { .. } => Some(Value::Str(text::hex(rng, 64))),
            FieldType::List(inner) => {
                let len = rng.range(1, 4) as usize;
                let mut items = Vec::new();
                for _ in 0..len {
                    if let Some(item) =
                        self.value(plan, fp, inner, rec, rng, field_index, back, own_ids)
                    {
                        items.push(item);
                    }
                }
                Some(Value::List(items))
            }
            FieldType::Ref { target, .. } => {
                let target = target.to_string();
                if target == plan.key {
                    // A self reference points at an earlier record, mostly a hot one near the start.
                    if own_ids.is_empty() || (!fp.required && !rng.chance(0.65)) {
                        return None;
                    }
                    let at = rng.skewed(0, own_ids.len() as i64 - 1) as usize;
                    return Some(Value::Ref(RecordId(own_ids[at])));
                }
                match self.pools.get(&target) {
                    Some(pool) => {
                        let id = pool.pick(rng)?;
                        back.note(plan.index, field_index, id);
                        Some(Value::Ref(RecordId(id)))
                    }
                    // The target is created later: an update sets it afterwards.
                    None => None,
                }
            }
        }
    }
}

/// For the references the update pass resolves: which records of a type point at each record of
/// the type they point to (revisions by article, stages by request), so that "the current
/// revision" is one of the article's own.
#[derive(Default)]
struct BackIndex {
    /// (type index, field index) -> target id -> the ids of this type that point at it, in order.
    wanted: BTreeSet<(usize, usize)>,
    found: HashMap<(usize, usize), HashMap<u64, Vec<u64>>>,
    /// The id of the record being created is not known until it commits; the entries wait here.
    pending: Vec<(usize, usize, u64)>,
}

impl BackIndex {
    fn note(&mut self, ti: usize, fi: usize, target: u64) {
        if self.wanted.contains(&(ti, fi)) {
            self.pending.push((ti, fi, target));
        }
    }

    fn settle(&mut self, id: u64) {
        for (ti, fi, target) in self.pending.drain(..) {
            self.found
                .entry((ti, fi))
                .or_default()
                .entry(target)
                .or_default()
                .push(id);
        }
    }
}

fn plan_types(schema: &TenantSchema, scale: Scale) -> Vec<TypePlan> {
    let shares: f64 = schema.objects.keys().map(|k| share_of(k.as_str())).sum();
    let mut plans = Vec::new();
    for (index, (id, def)) in schema.objects.iter().enumerate() {
        let key = id.to_string();
        let module = key.split("__").next().unwrap_or("").to_string();
        let workflows: Vec<_> = schema.workflows_of(id).collect();
        let mut fields = Vec::new();
        for (fid, fd) in &def.fields {
            let initial = workflows
                .iter()
                .find(|w| &w.field == fid)
                .map(|w| w.initial.iter().cloned().collect::<Vec<_>>());
            fields.push(FieldPlan {
                id: fid.clone(),
                name: fid.to_string(),
                ty: fd.ty.clone(),
                required: fd.required,
                unique: fd.unique,
                has_default: fd.default.is_some(),
                computed: fd.read.is_some(),
                personal: fd.personal.as_ref().map(|p| p.kind),
                initial,
            });
        }
        let unique_person = fields.iter().any(|f| f.unique && f.personal.is_some());
        let share = share_of(&key);
        let count = ((scale.target() as f64 * share / shares).round() as u64).max(scale.floor());
        plans.push(TypePlan {
            index,
            id: id.clone(),
            key,
            module,
            fields,
            count,
            unique_person,
        });
    }
    plans
}

/// The order types are created in: a type after every type it must reference. An optional
/// reference to a type that comes later is set by the update pass.
fn creation_order(plans: &[TypePlan]) -> Result<Vec<usize>, CorpusError> {
    let index: HashMap<&str, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.as_str(), i))
        .collect();
    let mut needs: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); plans.len()];
    for (i, plan) in plans.iter().enumerate() {
        for fp in &plan.fields {
            if let FieldType::Ref { target, .. } = &fp.ty {
                if fp.required {
                    if let Some(&t) = index.get(target.as_str()) {
                        if t != i {
                            needs[i].insert(t);
                        }
                    }
                }
            }
        }
    }
    let mut order = Vec::new();
    let mut placed = vec![false; plans.len()];
    while order.len() < plans.len() {
        let next = (0..plans.len()).find(|&i| !placed[i] && needs[i].iter().all(|&t| placed[t]));
        match next {
            Some(i) => {
                placed[i] = true;
                order.push(i);
            }
            None => {
                return Err(CorpusError::new(
                    "the object types require each other through required references",
                ))
            }
        }
    }
    Ok(order)
}

/// Populate the loaded tenant. See the module.
pub fn populate(
    store: &dyn Store,
    loaded: &LoadedCorpus,
    options: &PopulateOptions,
) -> Result<Catalogue, CorpusError> {
    populate_with(store, loaded, options, &mut |_| {})
}

/// [`populate`] with a progress report every [`PopulateOptions::batch`] records.
pub fn populate_with(
    store: &dyn Store,
    loaded: &LoadedCorpus,
    options: &PopulateOptions,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Catalogue, CorpusError> {
    let state = store
        .load(&loaded.tenant, &[])
        .map_err(CorpusError::from_display)?;
    let held = state
        .tenant
        .ok_or_else(|| CorpusError::new("the tenant does not exist"))?;
    let schema = held.schema.clone();
    let plans = plan_types(&schema, options.scale);
    let order = creation_order(&plans)?;

    // The shared population of people, large enough for the biggest type that takes each person
    // once.
    let unique_max = plans
        .iter()
        .filter(|p| p.unique_person)
        .map(|p| p.count)
        .max()
        .unwrap_or(0);
    let people_n = (unique_max as usize + unique_max as usize / 10).max(200);
    let mut people_rng = Rng::derive(options.seed, "people");
    let people = text::people(&mut people_rng, people_n);
    let people_zipf = Zipf::new(people.len(), 1.0);

    let mut gen = Populator {
        store,
        tenant: loaded.tenant.clone(),
        principal: Principal::User(loaded.owner.clone()),
        clock: START.max(loaded.clock + 1),
        commits: 0,
        people,
        people_zipf,
        pools: BTreeMap::new(),
    };

    // Which references the update pass resolves, so that the back index is kept for them.
    let position: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    let index: HashMap<&str, usize> = plans
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.as_str(), i))
        .collect();
    let mut back = BackIndex::default();
    // (owner type, field index, target type)
    let mut deferred_fields: Vec<(usize, usize, usize)> = Vec::new();
    for (i, plan) in plans.iter().enumerate() {
        for (f, fp) in plan.fields.iter().enumerate() {
            if let FieldType::Ref { target, .. } = &fp.ty {
                if let Some(&t) = index.get(target.as_str()) {
                    if t != i && position[&t] > position[&i] && !fp.required {
                        deferred_fields.push((i, f, t));
                    }
                }
            }
        }
    }
    // The type that points back is the one created later; its references to the owner are the
    // ones to remember.
    for &(owner, _, target) in &deferred_fields {
        for (f, fp) in plans[target].fields.iter().enumerate() {
            if let FieldType::Ref { target: of, .. } = &fp.ty {
                if of.as_str() == plans[owner].key {
                    back.wanted.insert((target, f));
                }
            }
        }
    }

    let mut catalogue = Catalogue {
        tenant: loaded.tenant.to_string(),
        owner: loaded.owner.to_string(),
        scale: options.scale.name().to_string(),
        seed: options.seed,
        ..Catalogue::default()
    };

    for &ti in &order {
        let plan = &plans[ti];
        let mut rng = Rng::derive(options.seed, &format!("type:{}", plan.key));
        let mut entry = TypeEntry {
            module: plan.module.clone(),
            ..TypeEntry::default()
        };
        for fp in &plan.fields {
            if let FieldType::Ref { target, .. } = &fp.ty {
                entry.refs.push(RefEdge {
                    field: fp.name.clone(),
                    target: target.to_string(),
                    required: fp.required,
                });
            }
            if fp.personal.is_some() {
                entry.personal.push(fp.name.clone());
            }
        }
        for n in 0..plan.count {
            let roll = rng.unit();
            let factor = if roll < 0.03 {
                60
            } else if roll < 0.20 {
                6
            } else {
                1
            };
            let mut rec = RecordState {
                n,
                person: None,
                factor,
                bytes: 0,
            };
            let mut fields: BTreeMap<FieldId, Value> = BTreeMap::new();
            for (fi, fp) in plan.fields.iter().enumerate() {
                if fp.computed {
                    continue;
                }
                if let Some(initial) = &fp.initial {
                    let state = initial[rng.below(initial.len() as u64) as usize].clone();
                    rec.bytes += state.len();
                    fields.insert(fp.id.clone(), Value::Str(state));
                    continue;
                }
                if !fp.required {
                    let name = fp.name.as_str();
                    let p = if fp.has_default {
                        0.3
                    } else if long_text(name).is_some() || name.contains("phone") {
                        0.55
                    } else {
                        0.85
                    };
                    if !rng.chance(p) {
                        continue;
                    }
                }
                let own = entry.ids.as_slice();
                let ty = fp.ty.clone();
                if let Some(value) =
                    gen.value(plan, fp, &ty, &mut rec, &mut rng, fi, &mut back, own)
                {
                    rec.bytes += value_bytes(&value);
                    fields.insert(fp.id.clone(), value);
                }
            }
            let intent = Intent::CreateRecord {
                object: plan.id.clone(),
                fields,
            };
            match gen.commit(intent) {
                Ok(Output::Record { id, .. }) => {
                    let id = id.0;
                    back.settle(id);
                    entry.ids.push(id);
                    entry.bytes += rec.bytes as u64;
                    if rec.factor == 60 {
                        entry.large.push(id);
                    }
                    catalogue.records += 1;
                }
                Ok(other) => {
                    return Err(CorpusError::new(format!(
                        "{}: create-record answered {other:?}",
                        plan.key
                    )))
                }
                Err(error) => {
                    back.pending.clear();
                    if options.lenient {
                        let rejected = catalogue
                            .rejections
                            .entry(plan.key.clone())
                            .or_insert((0, error.clone()));
                        rejected.0 += 1;
                    } else {
                        return Err(CorpusError::new(format!(
                            "{}: record {n} of {} was refused: {error}",
                            plan.key, plan.count
                        )));
                    }
                }
            }
            if options.batch > 0 && (n + 1) % options.batch as u64 == 0 {
                progress(&Progress {
                    object: plan.key.clone(),
                    done: n + 1,
                    total: plan.count,
                    commits: gen.commits,
                });
            }
        }
        // The type is complete: references to it can be drawn, hottest first.
        let skew = skew_of(&plan.key);
        let pool = Pool::new(
            entry.ids.clone(),
            skew,
            &mut Rng::derive(options.seed, &format!("hot:{}", plan.key)),
        );
        let hot_n = ((entry.ids.len() as f64 * 0.02).ceil() as usize).clamp(1, 50);
        entry.hot = pool.hot(hot_n);
        gen.pools.insert(plan.key.clone(), pool);
        progress(&Progress {
            object: plan.key.clone(),
            done: plan.count,
            total: plan.count,
            commits: gen.commits,
        });
        catalogue.types.insert(plan.key.clone(), entry);
    }

    // The update pass: the optional references to types created after their owner.
    for &(owner, fi, target) in &deferred_fields {
        let plan = &plans[owner];
        let fp = &plan.fields[fi];
        let mut rng = Rng::derive(options.seed, &format!("update:{}:{}", plan.key, fp.name));
        let ids = catalogue.types[&plan.key].ids.clone();
        // The field of the target type that points back at this one, if it has one.
        let back_key = plans[target]
            .fields
            .iter()
            .position(
                |f| matches!(&f.ty, FieldType::Ref { target: of, .. } if of.as_str() == plan.key),
            )
            .map(|f| (target, f));
        for id in ids {
            if !rng.chance(0.85) {
                continue;
            }
            let chosen = back_key
                .and_then(|key| back.found.get(&key))
                .and_then(|by_owner| by_owner.get(&id))
                .and_then(|own| {
                    if own.is_empty() {
                        None
                    } else if rng.chance(0.8) {
                        own.last().copied()
                    } else {
                        Some(own[rng.below(own.len() as u64) as usize])
                    }
                })
                .or_else(|| {
                    gen.pools
                        .get(&plans[target].key)
                        .and_then(|p| p.pick(&mut rng))
                });
            let Some(value) = chosen else { continue };
            let mut set = BTreeMap::new();
            set.insert(fp.id.clone(), Value::Ref(RecordId(value)));
            let intent = Intent::UpdateRecord {
                object: plan.id.clone(),
                id: RecordId(id),
                fields: set,
                expected_version: None,
            };
            if let Err(error) = gen.commit(intent) {
                if options.lenient {
                    let rejected = catalogue
                        .rejections
                        .entry(format!("{}.{} (update)", plan.key, fp.name))
                        .or_insert((0, error.clone()));
                    rejected.0 += 1;
                } else {
                    return Err(CorpusError::new(format!(
                        "{}: setting {} on record {id} was refused: {error}",
                        plan.key, fp.name
                    )));
                }
            }
        }
    }

    catalogue.commits = gen.commits;
    catalogue.people = gen.people.len() as u64;
    catalogue.hot_people = gen
        .people
        .iter()
        .take(20)
        .map(|p| HotPerson {
            name: p.full(),
            email: p.email.clone(),
        })
        .collect();
    Ok(catalogue)
}

fn value_bytes(value: &Value) -> usize {
    match value {
        Value::Str(s) => s.len(),
        Value::List(items) => items.iter().map(value_bytes).sum::<usize>() + 2,
        _ => 8,
    }
}

/// How many records each object type of the tenant would get at `scale`.
pub fn planned_counts(
    store: &dyn Store,
    tenant: &strata_kernel::TenantId,
    scale: Scale,
) -> Result<BTreeMap<String, u64>, CorpusError> {
    let state = store.load(tenant, &[]).map_err(CorpusError::from_display)?;
    let held = state
        .tenant
        .ok_or_else(|| CorpusError::new("the tenant does not exist"))?;
    Ok(plan_types(&held.schema, scale)
        .into_iter()
        .map(|plan| (plan.key, plan.count))
        .collect())
}
