//! Plays a scenario against a store and measures it.
//!
//! One thread, logical clients: the seed decides which client acts and what it does, so the
//! operation sequence and every answer are a function of the seed alone. Wall-clock time is
//! measured around the call to the store under test and nothing else; the reference answers and
//! the coherence comparisons are outside it.
//!
//! **Coherence.** For every read, the answer through the store under test is compared with the
//! answer of the reference (the unwrapped second handle) at the same generation: the generation
//! is read before and after the reference answer and a pair that straddles a change is counted as
//! unmatched, not compared. For every intent, the decision the store under test commits (delta,
//! events, output) is compared with the decision the kernel reaches on the reference's complete
//! state. Reads are also compared in a probe straight after each fault.

use crate::corpus::{Actor, CorpusLoader, CorpusTenant, Probe, TenantRequest};
use crate::generation::{Generation, GenerationView};
use crate::modes::{build, Built, CountingHook, Mode};
use crate::report::*;
use crate::rng::{hash_text, Rng, ZipfCache};
use crate::scenario::{Fault, FaultKind, MixEntry, OpKind, Scenario, Within};
use crate::stores::{Opened, StoreSpec};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use strata_kernel::ext::*;
use strata_kernel::{
    Context, DeltaOp, Intent, Need, ObjectTypeId, ReadRequest, RecordId, State, StateRevision,
    TenantId, TenantSchema, Timestamp,
};
use strata_storage_api::{
    execute, execute_at, partial_state, query, ExecuteError, Store, StoreError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoherenceLevel {
    Off,
    /// Compare reads.
    Reads,
    /// Compare reads and the decision of every intent.
    Full,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub store: StoreSpec,
    pub mode: Mode,
    pub seed: u64,
    pub coherence: CoherenceLevel,
    /// How many divergences to keep in detail.
    pub max_divergences: usize,
    /// Count the size of the records the store returns (costs time inside the counting layer).
    pub count_bytes: bool,
    /// The storage holds the primary tenant already: address it, do not build it.
    pub reuse: bool,
    /// The byte budget of `--mode full`'s working set.
    pub budget_bytes: u64,
}

/// The budget of `--mode full` when none is given: 64 MiB.
pub const DEFAULT_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

impl Config {
    pub fn new(store: StoreSpec, mode: Mode, seed: u64) -> Config {
        Config {
            store,
            mode,
            seed,
            coherence: CoherenceLevel::Full,
            max_divergences: 5,
            count_bytes: false,
            reuse: false,
            budget_bytes: DEFAULT_BUDGET_BYTES,
        }
    }
}

/// What a client remembers: the revision it last saw of each tenant.
#[derive(Default)]
struct Client {
    seen: BTreeMap<usize, u64>,
}

enum Body {
    Read(Vec<Need>),
    Query(ReadRequest),
    Write {
        intent: Intent,
        actor: Actor,
        contended: bool,
        object: Option<ObjectTypeId>,
    },
}

struct Plan {
    kind: OpKind,
    /// What the operation is about, for the sequence digest and the divergence detail.
    label: String,
    body: Body,
}

#[derive(Default)]
struct SchemaCache {
    last: Option<(Arc<TenantSchema>, String)>,
}

impl SchemaCache {
    fn hash(&mut self, schema: &Arc<TenantSchema>) -> String {
        if let Some((held, hash)) = &self.last {
            if Arc::ptr_eq(held, schema) {
                return hash.clone();
            }
        }
        let hash = hex(&Sha256::digest(
            serde_json::to_vec(schema.as_ref()).unwrap_or_default(),
        ));
        self.last = Some((schema.clone(), hash.clone()));
        hash
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest_of(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))[..16].to_string()
}

struct Runner<'a> {
    scenario: &'a Scenario,
    cfg: &'a Config,
    gens: &'a dyn GenerationView,
    // Dropped before `opened`, so that no handle is open when a private directory is removed.
    built: Built,
    tenants: Vec<Box<dyn CorpusTenant>>,
    opened: Opened,
    live: Vec<BTreeMap<ObjectTypeId, Vec<RecordId>>>,
    rng: Rng,
    zipf: ZipfCache,
    schemas: SchemaCache,
    clients: Vec<Client>,
    clock: i64,
    counter: u64,
    snapshot: Option<u64>,
    erased: Vec<Probe>,
    sequence: Sha256,
    answers: Sha256,
    report: Report,
    samples: Vec<u64>,
    samples_by_kind: BTreeMap<String, Vec<u64>>,
    decodes: Option<u64>,
    timed: std::time::Duration,
    aliases: BTreeMap<String, String>,
    /// The object type or query the operation in hand is about: which module's actor acts.
    subject: Option<String>,
    /// The object type each foreign-commit fault writes to, by the fault's index.
    fault_targets: BTreeMap<usize, ObjectTypeId>,
}

/// Run `scenario` under `cfg`, populating through `loader`.
pub fn run(
    scenario: &Scenario,
    cfg: &Config,
    loader: &dyn CorpusLoader,
    gens: &dyn GenerationView,
) -> Result<Report, String> {
    scenario.validate()?;
    let wall = Instant::now();
    let opened = cfg
        .store
        .open(cfg.seed ^ hash_text(&scenario.name))
        .map_err(|e| format!("open store: {e}"))?;

    let populate_started = Instant::now();
    let mut tenants: Vec<Box<dyn CorpusTenant>> = Vec::new();
    let primary = TenantRequest {
        name: "corpus".to_string(),
        scale: scenario.scale,
        seed: cfg.seed,
        small: false,
    };
    tenants.push(if cfg.reuse {
        loader.reuse(opened.reference.clone(), &primary)?
    } else {
        loader.populate(opened.reference.clone(), &primary)?
    });
    if let Some(small) = &scenario.small_tenants {
        for i in 0..small.count {
            tenants.push(loader.populate(
                opened.reference.clone(),
                &TenantRequest {
                    name: format!("small{i}"),
                    scale: small.scale,
                    seed: cfg.seed ^ (u64::from(i) + 1),
                    small: true,
                },
            )?);
        }
    }
    let populate_ms = populate_started.elapsed().as_secs_f64() * 1e3;

    let live = tenants
        .iter()
        .map(|t| {
            t.catalogue()
                .objects
                .iter()
                .map(|(o, ids)| (o.clone(), ids.ids.clone()))
                .collect()
        })
        .collect();
    let counting = Arc::new(CountingHook::with_bytes(cfg.count_bytes));
    let built = build(
        opened.primary.clone(),
        cfg.mode,
        counting,
        cfg.budget_bytes,
        None,
    );
    let mut runner = Runner {
        scenario,
        cfg,
        gens,
        built,
        tenants,
        opened,
        live,
        rng: Rng(cfg.seed ^ hash_text(&scenario.name)),
        zipf: ZipfCache::default(),
        schemas: SchemaCache::default(),
        clients: (0..scenario.clients).map(|_| Client::default()).collect(),
        clock: 1_000_000,
        counter: 0,
        snapshot: None,
        erased: Vec::new(),
        sequence: Sha256::new(),
        answers: Sha256::new(),
        report: Report::default(),
        samples: Vec::new(),
        samples_by_kind: BTreeMap::new(),
        decodes: cfg.store.schema_decodes().map(|_| 0),
        timed: std::time::Duration::ZERO,
        aliases: BTreeMap::new(),
        subject: None,
        fault_targets: BTreeMap::new(),
    };
    runner.play()?;
    let mut report = runner.finish(wall.elapsed().as_secs_f64() * 1e3, populate_ms);
    report.tenants = scenario
        .small_tenants
        .as_ref()
        .map_or(0, |s| s.count as usize)
        + 1;
    Ok(report)
}

impl Runner<'_> {
    fn play(&mut self) -> Result<(), String> {
        let length = self.scenario.length;
        if self
            .scenario
            .faults
            .iter()
            .any(|f| f.kind == FaultKind::Restore)
        {
            let tenant = self.tenants[0].tenant().clone();
            let info = self
                .opened
                .reference
                .create_snapshot(&tenant, "ws-bench", Timestamp(self.clock))
                .map_err(|e| format!("snapshot: {e}"))?;
            self.snapshot = Some(info.id);
        }
        self.resolve_fault_targets()?;
        let total: u64 = self.scenario.mix.iter().map(|m| u64::from(m.weight)).sum();
        for i in 0..length {
            let due: Vec<(usize, Fault)> = self
                .scenario
                .faults
                .iter()
                .enumerate()
                .filter(|(_, f)| match (f.at, f.every) {
                    (Some(at), _) => ((at * length as f64) as usize).min(length - 1) == i,
                    (_, Some(every)) => i > 0 && i % every == 0,
                    _ => false,
                })
                .map(|(idx, f)| (idx, f.clone()))
                .collect();
            for (idx, fault) in &due {
                self.apply_fault(*idx, fault, i as u64)?;
            }
            let client = self.rng.below(self.clients.len() as u64) as usize;
            let mut at = self.rng.below(total);
            let entry = self
                .scenario
                .mix
                .iter()
                .find(|m| {
                    if at < u64::from(m.weight) {
                        true
                    } else {
                        at -= u64::from(m.weight);
                        false
                    }
                })
                .cloned()
                .unwrap_or_else(|| self.scenario.mix[0].clone());
            let tenant = self.pick_tenant();
            self.counter += 1;
            self.clock += 10;
            let plan = self.plan(tenant, &entry);
            let kind = entry.op.name().to_string();
            let counts = self.report.ops.entry(kind.clone()).or_default();
            counts.count += 1;
            let Some(plan) = plan else {
                counts.skipped += 1;
                self.sequence
                    .update(format!("{i}|{client}|{kind}|{tenant}|skipped\n").as_bytes());
                continue;
            };
            self.sequence
                .update(format!("{i}|{client}|{kind}|{tenant}|{}\n", plan.label).as_bytes());
            self.execute_plan(i as u64, client, tenant, &entry, plan);
            if !self.erased.is_empty() && i % 10 == 0 {
                self.sweep_erased(i as u64);
            }
        }
        Ok(())
    }

    fn pick_tenant(&mut self) -> usize {
        match &self.scenario.small_tenants {
            Some(small) if self.tenants.len() > 1 => {
                let zipf = self.zipf.get(self.tenants.len(), small.skew);
                zipf.sample(&mut self.rng)
            }
            _ => 0,
        }
    }

    fn resolve(&mut self, tenant: usize, name: &str) -> Option<ObjectTypeId> {
        let (object, aliased) = self.tenants[tenant].catalogue().resolve(name)?;
        if aliased {
            self.aliases
                .insert(name.to_string(), object.as_str().to_string());
        }
        Some(object)
    }

    /// A rank among `n`, by the entry's skew. The table is built for `n` rounded up to a multiple
    /// of 64, so that a population that grows or shrinks by a record does not build a table per
    /// operation; a rank past the end is clamped.
    fn rank(&mut self, n: usize, entry: &MixEntry) -> usize {
        if n == 0 {
            return 0;
        }
        let skew = entry.skew.unwrap_or(self.scenario.skew);
        let table = if n > 64 { n.div_ceil(64) * 64 } else { n };
        self.zipf.get(table, skew).sample(&mut self.rng).min(n - 1)
    }

    fn pick_id(
        &mut self,
        tenant: usize,
        object: &ObjectTypeId,
        entry: &MixEntry,
    ) -> Option<RecordId> {
        let len = self.live[tenant].get(object)?.len();
        if len == 0 {
            return None;
        }
        let hot = self.tenants[tenant]
            .catalogue()
            .objects
            .get(object)
            .map_or(len, |o| o.hot.max(1));
        let pool = match entry.within {
            Within::All => len,
            Within::Hot => hot.min(len),
        };
        let rank = self.rank(pool, entry);
        let ids = self.live[tenant].get(object)?;
        if entry.op == OpKind::Delete {
            // A delete takes from the cold end: the hot set is what the reads are about.
            Some(ids[len - 1 - rank])
        } else {
            Some(ids[rank])
        }
    }

    fn plan(&mut self, tenant: usize, entry: &MixEntry) -> Option<Plan> {
        let n = self.counter;
        let object = match &entry.object {
            Some(name) => Some(self.resolve(tenant, name)?),
            None => None,
        };
        let label_of = |object: &Option<ObjectTypeId>, id: Option<RecordId>| {
            format!(
                "{}#{}",
                object.as_ref().map_or("-", |o| o.as_str()),
                id.map_or(0, |i| i.0)
            )
        };
        let kind = entry.op;
        match kind {
            OpKind::Get => {
                let object_id = object.clone()?;
                let id = self.pick_id(tenant, &object_id, entry)?;
                Some(Plan {
                    kind,
                    label: label_of(&object, Some(id)),
                    body: Body::Read(vec![Need::Record {
                        object: object_id,
                        id,
                    }]),
                })
            }
            OpKind::ByField => {
                let object_id = object.clone()?;
                let len = self.tenants[tenant]
                    .catalogue()
                    .objects
                    .get(&object_id)?
                    .lookup
                    .as_ref()?
                    .values
                    .len();
                let rank = self.rank(len, entry);
                let found = self.tenants[tenant].catalogue().objects.get(&object_id)?;
                let lookup = found.lookup.as_ref()?;
                Some(Plan {
                    kind,
                    label: label_of(&object, Some(found.ids[rank])),
                    body: Body::Read(vec![Need::ByField {
                        object: object_id,
                        field: lookup.field.clone(),
                        value: lookup.values[rank].clone(),
                    }]),
                })
            }
            OpKind::Page => {
                let object_id = object.clone()?;
                let id = self.pick_id(tenant, &object_id, entry)?;
                let after = (id.0 % 2 == 1).then_some(id);
                Some(Plan {
                    kind,
                    label: label_of(&object, after),
                    body: Body::Read(vec![Need::PageOf {
                        object: object_id,
                        after,
                        limit: entry.page_size.unwrap_or(20),
                    }]),
                })
            }
            OpKind::Scan => Some(Plan {
                kind,
                label: label_of(&object, None),
                body: Body::Read(vec![Need::AllOf(object.clone()?)]),
            }),
            OpKind::Header => Some(Plan {
                kind,
                label: "header".to_string(),
                body: Body::Read(Vec::new()),
            }),
            OpKind::Query => {
                let request = self.tenants[tenant].named_query(entry.query.as_deref()?, n)?;
                Some(Plan {
                    kind,
                    label: format!("query#{}", digest_of(&format!("{request:?}"))),
                    body: Body::Query(request),
                })
            }
            OpKind::LedgerRead => {
                let need = self.tenants[tenant].ledger_need(n)?;
                Some(Plan {
                    kind,
                    label: format!("ledger#{}", digest_of(&format!("{need:?}"))),
                    body: Body::Read(vec![need]),
                })
            }
            OpKind::Create => {
                let object_id = object.clone()?;
                let fields = self.tenants[tenant].create_fields(&object_id, n)?;
                Some(Plan {
                    kind,
                    label: label_of(&object, None),
                    body: Body::Write {
                        intent: Intent::CreateRecord {
                            object: object_id,
                            fields,
                        },
                        actor: entry.actor,
                        contended: false,
                        object,
                    },
                })
            }
            OpKind::Update => {
                let object_id = object.clone()?;
                let id = self.pick_id(tenant, &object_id, entry)?;
                let fields = self.tenants[tenant].update_fields(&object_id, id, n)?;
                Some(Plan {
                    kind,
                    label: label_of(&object, Some(id)),
                    body: Body::Write {
                        intent: Intent::UpdateRecord {
                            object: object_id,
                            id,
                            fields,
                            expected_version: None,
                        },
                        actor: entry.actor,
                        contended: entry.contended,
                        object,
                    },
                })
            }
            OpKind::Delete => {
                let object_id = object.clone()?;
                let id = self.pick_id(tenant, &object_id, entry)?;
                Some(Plan {
                    kind,
                    label: label_of(&object, Some(id)),
                    body: Body::Write {
                        intent: Intent::DeleteRecord {
                            object: object_id,
                            id,
                            expected_version: None,
                        },
                        actor: entry.actor,
                        contended: false,
                        object,
                    },
                })
            }
            OpKind::PostLedger => {
                let intent = self.tenants[tenant].ledger_intent(n)?;
                Some(Plan {
                    kind,
                    label: format!("post#{}", digest_of(&format!("{intent:?}"))),
                    body: Body::Write {
                        intent,
                        actor: entry.actor,
                        contended: false,
                        object: None,
                    },
                })
            }
        }
    }

    fn ctx(&self, tenant: usize, actor: Actor) -> Context {
        let tid = self.tenants[tenant].tenant().clone();
        Context {
            tenant: tid,
            principal: self.tenants[tenant].actor(actor, self.subject.as_deref()),
            session: None,
            now: Timestamp(self.clock),
            correlation: format!("ws-bench-{}", self.counter),
            verified_address: None,
            idempotency_key: None,
            language: None,
            hop: None,
            agent: None,
        }
    }

    /// Run `call` against the store under test, timed, counting the schema decodes it makes.
    fn timed<T>(&mut self, call: impl FnOnce(&dyn Store) -> T) -> (T, u64) {
        let store = self.built.store.clone();
        let decodes_before = self.cfg.store.schema_decodes();
        let started = Instant::now();
        let out = call(store.as_ref());
        let elapsed = started.elapsed();
        if let (Some(before), Some(after), Some(total)) = (
            decodes_before,
            self.cfg.store.schema_decodes(),
            self.decodes.as_mut(),
        ) {
            *total += after.saturating_sub(before);
        }
        self.timed += elapsed;
        (out, elapsed.as_nanos() as u64)
    }

    fn record_latency(&mut self, kind: OpKind, ns: u64) {
        self.samples.push(ns);
        self.samples_by_kind
            .entry(kind.name().to_string())
            .or_default()
            .push(ns);
    }

    fn generation(&self, tenant: &TenantId) -> Option<Generation> {
        self.gens
            .generation(self.opened.reference.as_ref(), tenant)
            .ok()
            .flatten()
    }

    fn execute_plan(
        &mut self,
        op: u64,
        client: usize,
        tenant: usize,
        entry: &MixEntry,
        plan: Plan,
    ) {
        let kind = plan.kind;
        self.subject = entry.object.clone().or_else(|| entry.query.clone());
        match plan.body {
            Body::Read(needs) => {
                self.do_read(op, client, tenant, kind, &plan.label, entry.actor, needs)
            }
            Body::Query(request) => {
                self.do_query(op, client, tenant, kind, &plan.label, entry.actor, request)
            }
            Body::Write {
                intent,
                actor,
                contended,
                object,
            } => self.do_write(
                op,
                client,
                tenant,
                kind,
                &plan.label,
                actor,
                contended,
                object,
                intent,
            ),
        }
    }

    fn note_outcome(&mut self, kind: OpKind, outcome: &str, refusal: Option<String>) {
        let counts = self.report.ops.entry(kind.name().to_string()).or_default();
        match outcome {
            "ok" => counts.ok += 1,
            "domain" => counts.domain_errors += 1,
            _ => counts.store_errors += 1,
        }
        if let Some(kind_name) = refusal {
            *counts.refused.entry(kind_name.clone()).or_default() += 1;
            *self.report.refusals.entry(kind_name).or_default() += 1;
        }
    }

    fn diverge(
        &mut self,
        op: u64,
        kind: &str,
        tenant: &TenantId,
        what: &str,
        under_test: String,
        reference: String,
        after_fault: Option<&str>,
    ) {
        let generation = self.generation(tenant).map(|g| g.counter);
        let coherence = &mut self.report.coherence;
        coherence.divergences += 1;
        if after_fault.is_some() {
            coherence.post_fault_divergences += 1;
        }
        if coherence.first.len() < self.cfg.max_divergences {
            coherence.first.push(Divergence {
                op,
                kind: kind.to_string(),
                tenant: tenant.to_string(),
                what: what.to_string(),
                under_test,
                reference,
                generation,
                after_fault: after_fault.map(str::to_string),
            });
        }
    }

    /// The digest and a one-line summary of what a load answers for `needs`.
    fn state_answer(
        &mut self,
        result: &Result<State, StoreError>,
        needs: &[Need],
    ) -> (String, String) {
        let state = match result {
            Ok(state) => state,
            Err(e) => {
                return (
                    format!("err:{}", digest_of(&e.to_string())),
                    format!("error {e}"),
                )
            }
        };
        let Some(tenant) = state.tenant.as_deref() else {
            return ("absent".to_string(), "no tenant".to_string());
        };
        let cut;
        let thin: &strata_kernel::TenantState = if needs.is_empty() {
            tenant
        } else {
            cut = partial_state(tenant, needs);
            cut.tenant.as_deref().unwrap_or(tenant)
        };
        let schema_hash = self.schemas.hash(&thin.schema);
        let body = serde_json::to_string(&(
            thin.revision,
            &thin.name,
            thin.next_record,
            thin.event_seq,
            &schema_hash,
            &thin.members,
            &thin.setting_values,
            if needs.is_empty() {
                None
            } else {
                Some(&thin.records)
            },
            if needs.is_empty() {
                None
            } else {
                thin.ledger.as_ref()
            },
        ))
        .unwrap_or_default();
        let records: usize = thin.records.values().map(|r| r.len()).sum();
        (
            digest_of(&body),
            format!(
                "revision {} schema v{} records {records}",
                thin.revision.0, thin.schema.version
            ),
        )
    }

    fn check(
        &mut self,
        op: u64,
        kind: &str,
        tenant: &TenantId,
        what: &str,
        pair: Option<(
            (String, String),
            (String, String),
            Option<Generation>,
            Option<Generation>,
        )>,
        after_fault: Option<&str>,
    ) {
        let Some((test, reference, g0, g1)) = pair else {
            return;
        };
        if g0 != g1 {
            self.report.coherence.unmatched += 1;
            return;
        }
        self.report.coherence.checked += 1;
        if after_fault.is_some() {
            self.report.coherence.post_fault_checked += 1;
        }
        if test.0 != reference.0 {
            self.diverge(
                op,
                kind,
                tenant,
                what,
                format!("{} [{}]", test.0, test.1),
                format!("{} [{}]", reference.0, reference.1),
                after_fault,
            );
        }
    }

    /// One load through the store under test (timed when `timed`), and the reference's.
    #[allow(clippy::too_many_arguments)]
    fn read_pair(
        &mut self,
        op: u64,
        client: usize,
        tenant: usize,
        kind: OpKind,
        label: &str,
        needs: &[Need],
        timed: bool,
        after_fault: Option<&str>,
    ) {
        let tid = self.tenants[tenant].tenant().clone();
        let compare = self.cfg.coherence != CoherenceLevel::Off;
        let (test, ns) = if timed {
            self.timed(|store| store.load(&tid, needs))
        } else {
            (self.built.store.load(&tid, needs), 0)
        };
        if timed {
            self.record_latency(kind, ns);
        }
        let test_answer = self.state_answer(&test, needs);
        if let Ok(state) = &test {
            if let Some(t) = state.tenant.as_deref() {
                self.clients[client].seen.insert(tenant, t.revision.0);
            }
        }
        if timed {
            self.answers.update(test_answer.0.as_bytes());
            let outcome = if test.is_ok() { "ok" } else { "store" };
            self.note_outcome(kind, outcome, None);
        }
        if compare {
            let g0 = self.generation(&tid);
            let reference = self.opened.reference.load(&tid, needs);
            let reference_answer = self.state_answer(&reference, needs);
            let g1 = self.generation(&tid);
            self.check(
                op,
                kind.name(),
                &tid,
                label,
                Some((test_answer, reference_answer, g0, g1)),
                after_fault,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn do_read(
        &mut self,
        op: u64,
        client: usize,
        tenant: usize,
        kind: OpKind,
        label: &str,
        _actor: Actor,
        needs: Vec<Need>,
    ) {
        self.read_pair(op, client, tenant, kind, label, &needs, true, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn do_query(
        &mut self,
        op: u64,
        client: usize,
        tenant: usize,
        kind: OpKind,
        label: &str,
        actor: Actor,
        request: ReadRequest,
    ) {
        let ctx = self.ctx(tenant, actor);
        let answer = |result: &Result<strata_kernel::ReadResult, ExecuteError>| match result {
            Ok(read) => {
                let body = serde_json::to_string(&(
                    &read.rows,
                    read.revision,
                    read.schema_version,
                    &read.next,
                ))
                .unwrap_or_default();
                (
                    digest_of(&body),
                    format!("revision {} rows {}", read.revision.0, read.rows.len()),
                )
            }
            Err(e) => (
                format!("err:{}", digest_of(&e.to_string())),
                format!("error {e}"),
            ),
        };
        let (test, ns) = self.timed(|store| query(store, &request, &ctx));
        self.record_latency(kind, ns);
        let test_answer = answer(&test);
        if let Ok(read) = &test {
            self.clients[client].seen.insert(tenant, read.revision.0);
        }
        self.answers.update(test_answer.0.as_bytes());
        let outcome = match &test {
            Ok(_) => "ok",
            Err(ExecuteError::Domain(_)) => "domain",
            Err(_) => "store",
        };
        let refusal = match &test {
            Err(ExecuteError::Domain(e)) => Some(format!("{:?}", e.kind)),
            _ => None,
        };
        self.note_outcome(kind, outcome, refusal);
        if self.cfg.coherence != CoherenceLevel::Off {
            let tid = ctx.tenant.clone();
            let g0 = self.generation(&tid);
            let reference = query(self.opened.reference.as_ref(), &request, &ctx);
            let reference_answer = answer(&reference);
            let g1 = self.generation(&tid);
            self.check(
                op,
                kind.name(),
                &tid,
                label,
                Some((test_answer, reference_answer, g0, g1)),
                None,
            );
        }
    }

    /// The decision of a transition, as text: what the commit would write.
    fn decision(result: &Result<strata_kernel::Transition, strata_kernel::DomainError>) -> String {
        match result {
            Ok(t) => digest_of(&format!("{:?}", (&t.delta.ops, &t.events, &t.output))),
            Err(e) => format!("domain:{}", digest_of(&e.describe())),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn do_write(
        &mut self,
        op: u64,
        client: usize,
        tenant: usize,
        kind: OpKind,
        label: &str,
        actor: Actor,
        contended: bool,
        object: Option<ObjectTypeId>,
        intent: Intent,
    ) {
        let ctx = self.ctx(tenant, actor);
        let tid = ctx.tenant.clone();
        // The decision the kernel reaches on the reference's complete state, before the write.
        let expected = (self.cfg.coherence == CoherenceLevel::Full).then(|| {
            let g0 = self.generation(&tid);
            let decided = crate::decide::dry_run(self.opened.reference.as_ref(), &intent, &ctx);
            (decided, g0, self.generation(&tid))
        });
        let at = if contended {
            self.clients[client].seen.get(&tenant).copied()
        } else {
            None
        };
        let mut retries = 0u64;
        let (mut result, mut ns) = self.timed(|store| match at {
            Some(at) => execute_at(store, &intent, &ctx, None, StateRevision(at)),
            None => execute(store, &intent, &ctx),
        });
        if at.is_some()
            && matches!(
                &result,
                Err(ExecuteError::Store(StoreError::Conflict { .. }))
            )
        {
            self.report.conflicts.contended_conflicts += 1;
            retries += 1;
            let (again, more) = self.timed(|store| execute(store, &intent, &ctx));
            result = again;
            ns += more;
        }
        self.record_latency(kind, ns);
        let answer = match &result {
            Ok(done) => {
                retries += done.conflicts as u64;
                self.clients[client]
                    .seen
                    .insert(tenant, done.receipt.revision.0);
                self.after_write(tenant, &intent, object.as_ref(), &done.transition.delta.ops);
                Self::decision(&Ok(done.transition.clone()))
            }
            Err(ExecuteError::Domain(e)) => format!("domain:{}", digest_of(&e.describe())),
            Err(ExecuteError::Store(e)) => format!("store:{}", digest_of(&e.to_string())),
        };
        self.report.conflicts.retries += retries;
        self.answers.update(answer.as_bytes());
        let outcome = match &result {
            Ok(_) => "ok",
            Err(ExecuteError::Domain(_)) => "domain",
            Err(_) => "store",
        };
        let refusal = match &result {
            Err(ExecuteError::Domain(e)) => Some(format!("{:?}", e.kind)),
            _ => None,
        };
        self.note_outcome(kind, outcome, refusal);
        if let Some((decided, g0, g1)) = expected {
            if outcome != "store" {
                let reference = match &decided {
                    Ok(transition) => Self::decision(transition),
                    Err(e) => format!("store:{}", digest_of(e)),
                };
                self.check(
                    op,
                    kind.name(),
                    &tid,
                    label,
                    Some(((answer, String::new()), (reference, String::new()), g0, g1)),
                    None,
                );
            }
        }
    }

    /// Keep the live ids in step with what a write did.
    fn after_write(
        &mut self,
        tenant: usize,
        intent: &Intent,
        object: Option<&ObjectTypeId>,
        ops: &[DeltaOp],
    ) {
        let Some(object) = object else {
            return;
        };
        match intent {
            Intent::CreateRecord { .. } => {
                for op in ops {
                    if let DeltaOp::PutRecord(record) = op {
                        if &record.object == object {
                            self.live[tenant]
                                .entry(object.clone())
                                .or_default()
                                .push(record.id);
                        }
                    }
                }
            }
            Intent::DeleteRecord { id, .. } => {
                if let Some(ids) = self.live[tenant].get_mut(object) {
                    ids.retain(|live| live != id);
                }
            }
            _ => {}
        }
    }

    /// Re-read which ids exist (after a restore or an erasure through the other handle).
    fn resync_live(&mut self) {
        let tid = self.tenants[0].tenant().clone();
        let Ok(state) = self.opened.reference.load_complete(&tid) else {
            return;
        };
        let Some(tenant) = state.tenant else {
            return;
        };
        let known: Vec<ObjectTypeId> = self.live[0].keys().cloned().collect();
        for object in known {
            let present: std::collections::BTreeSet<RecordId> = tenant
                .records
                .get(&object)
                .map(|r| r.keys().copied().collect())
                .unwrap_or_default();
            if let Some(ids) = self.live[0].get_mut(&object) {
                ids.retain(|id| present.contains(id));
            }
        }
    }

    fn revision(&self) -> Option<u64> {
        let tid = self.tenants[0].tenant();
        self.opened
            .reference
            .tenant_summary(tid)
            .ok()
            .flatten()
            .map(|s| s.revision.0)
    }

    fn schema_version(&self) -> Option<u64> {
        let tid = self.tenants[0].tenant();
        let state = self.opened.reference.load(tid, &[]).ok()?;
        Some(state.tenant.as_deref()?.schema.version.0)
    }

    /// Choose, before the first operation, the object type each foreign commit writes to: the
    /// fault's own if the tenant can update it, else one the mix reads, then one the mix writes,
    /// then any of the scenario's modules, then any in the catalogue. A fault with nothing to
    /// write to is an error of the scenario, raised here and not in the middle of a run.
    fn resolve_fault_targets(&mut self) -> Result<(), String> {
        let faults = self.scenario.faults.clone();
        let mix = self.scenario.mix.clone();
        let modules = self.scenario.modules.clone();
        for (idx, fault) in faults.iter().enumerate() {
            if !matches!(
                fault.kind,
                FaultKind::ForeignCommit | FaultKind::Restore | FaultKind::Restart
            ) {
                continue;
            }
            let t = fault.tenant.unwrap_or(0);
            let mut names: Vec<String> = fault.object.iter().cloned().collect();
            for reading in [true, false] {
                for entry in &mix {
                    if let (Some(object), true) = (&entry.object, entry.op.is_write() != reading) {
                        names.push(object.clone());
                    }
                }
            }
            let catalogue: Vec<String> = self.tenants[t]
                .catalogue()
                .objects
                .keys()
                .map(|o| o.to_string())
                .collect();
            names.extend(
                catalogue
                    .iter()
                    .filter(|o| {
                        o.split("__")
                            .next()
                            .is_some_and(|m| modules.iter().any(|x| x == m))
                    })
                    .cloned(),
            );
            names.extend(catalogue);
            let mut found = None;
            for name in &names {
                let Some(object) = self.resolve(t, name) else {
                    continue;
                };
                let Some(id) = self.live[t]
                    .get(&object)
                    .and_then(|ids| ids.first().copied())
                else {
                    continue;
                };
                if self.tenants[t].update_fields(&object, id, 0).is_some() {
                    found = Some(object);
                    break;
                }
            }
            match found {
                Some(object) => {
                    self.fault_targets.insert(idx, object);
                }
                None => {
                    return Err(format!(
                        "scenario {}: the {:?} fault {idx} has no object type tenant {t} can update; tried {} names",
                        self.scenario.name,
                        fault.kind,
                        names.len()
                    ))
                }
            }
        }
        Ok(())
    }

    fn apply_fault(&mut self, idx: usize, fault: &Fault, op: u64) -> Result<(), String> {
        let tid = self.tenants[0].tenant().clone();
        let name = format!("{:?}", fault.kind);
        self.counter += 1;
        self.clock += 10;
        self.subject = None;
        let mut decodes_before = None;
        let (outcome, effective) = match fault.kind {
            FaultKind::ForeignCommit => self.foreign_commit(idx, fault),
            FaultKind::Restore => {
                let (marker, marked) = self.foreign_commit(idx, fault);
                let before = self.revision();
                let (restored, done) = match self.snapshot {
                    Some(id) => match self.opened.reference.restore_snapshot(&tid, id, None) {
                        Ok(()) => ("restored".to_string(), true),
                        Err(e) => (format!("restore failed: {e}"), false),
                    },
                    None => ("no snapshot".to_string(), false),
                };
                let after = self.revision();
                self.resync_live();
                let went_back = matches!((before, after), (Some(b), Some(a)) if a < b);
                (
                    format!("{marker}; {restored}; revision {before:?} -> {after:?}"),
                    marked && done && went_back,
                )
            }
            FaultKind::Erasure => {
                let n = self.counter;
                match self.tenants[0].erasure(n) {
                    None => (
                        "the tenant offers no erasure (no data subject with probes)".to_string(),
                        false,
                    ),
                    Some(erasure) => self.erase(erasure),
                }
            }
            FaultKind::SchemaChange => {
                let n = self.counter;
                let before = self.schema_version();
                decodes_before = self.built.wrapper_stats(self.cfg.mode).decodes;
                // The tenant's own path first (a product upgrade); a stand-in tenant changes its
                // schema directly.
                match self.tenants[0].evolve(n) {
                    Some(Ok(what)) => {
                        let after = self.schema_version();
                        (
                            format!("{what}; schema version {before:?} -> {after:?}"),
                            matches!((before, after), (Some(b), Some(a)) if a > b),
                        )
                    }
                    Some(Err(e)) => (format!("upgrade refused: {e}"), false),
                    None => match self.tenants[0].schema_change(n) {
                        None => ("the tenant offers no schema change".to_string(), false),
                        Some(changes) => {
                            let ctx = self.ctx(0, Actor::Owner);
                            let done = execute(
                                self.opened.reference.as_ref(),
                                &Intent::ChangeSchema {
                                    changes,
                                    expected_version: None,
                                    existing: strata_kernel::ExistingData::Reject,
                                },
                                &ctx,
                            );
                            let after = self.schema_version();
                            match done {
                                Ok(_) => (
                                    format!("schema version {before:?} -> {after:?}"),
                                    matches!((before, after), (Some(b), Some(a)) if a > b),
                                ),
                                Err(e) => (format!("refused: {e}"), false),
                            }
                        }
                    },
                }
            }
            FaultKind::Reopen => {
                self.opened
                    .reopen_primary()
                    .map_err(|e| format!("reopen: {e}"))?;
                self.built.swap_handle(self.opened.primary.clone());
                (
                    "adapter handle replaced under the working set".to_string(),
                    true,
                )
            }
            FaultKind::Restart => {
                // The stack is down: nothing reads through it while another handle commits.
                let stale = self.built.stale.clone();
                let (marker, marked) = self.foreign_commit(idx, fault);
                self.opened
                    .reopen_primary()
                    .map_err(|e| format!("restart: {e}"))?;
                let counting = self.built.counting.clone();
                self.built = build(
                    self.opened.primary.clone(),
                    self.cfg.mode,
                    counting,
                    self.cfg.budget_bytes,
                    stale,
                );
                let stats = self.built.wrapper_stats(self.cfg.mode);
                let cold = self.cfg.mode == Mode::Stale
                    || stats.placeholder
                    || stats.hits + stats.misses == 0;
                (
                    format!("stack rebuilt over a fresh handle; while down: {marker}"),
                    marked && cold,
                )
            }
        };
        self.report.faults.push(FaultRecord {
            kind: name.clone(),
            at_op: op,
            outcome,
            effective,
        });
        // A restart is not warmed by a probe: the first operations after it are the measurement,
        // and they are compared like any other.
        if fault.kind != FaultKind::Restart {
            self.probe_after(fault.tenant.unwrap_or(0), &name, op);
        }
        // A resident header must be decoded again after a schema change: the probe above read
        // the header through the store under test.
        if fault.kind == FaultKind::SchemaChange && self.cfg.coherence != CoherenceLevel::Off {
            let after = self.built.wrapper_stats(self.cfg.mode).decodes;
            if let (Some(b), Some(a), Some(record)) =
                (decodes_before, after, self.report.faults.last_mut())
            {
                record
                    .outcome
                    .push_str(&format!("; header decodes {b} -> {a}"));
                if a <= b {
                    record.effective = false;
                }
            }
        }
        Ok(())
    }

    /// Erase the subject through the reference handle, batch by batch, and check that it took
    /// effect: records changed, and no probe still holds its plaintext on the reference.
    fn erase(&mut self, erasure: crate::corpus::Erasure) -> (String, bool) {
        if erasure.probes.is_empty() {
            return ("the subject has no probe records".to_string(), false);
        }
        // The subject records are being read when the request lands: read each through the store
        // under test, as the workload would have, so that whatever it keeps resident holds them.
        let subject_tenant = self.tenants[0].tenant().clone();
        for probe in &erasure.probes {
            let need = [Need::Record {
                object: probe.object.clone(),
                id: probe.id,
            }];
            let _ = self.built.store.load(&subject_tenant, &need);
        }
        let ctx = self.ctx(0, Actor::Owner);
        let (mut object, mut after) = (None, None);
        let mut changed = 0usize;
        let mut batches = 0usize;
        let mut failure = None;
        // One decision erases a batch; repeat while its output says there is more.
        for _ in 0..50 {
            let intent = Intent::EraseSubject {
                subject: erasure.subject.clone(),
                resume_object: object.take(),
                resume_after: after.take(),
                held_objects: Vec::new(),
                addressed_invitations: Vec::new(),
            };
            match execute(self.opened.reference.as_ref(), &intent, &ctx) {
                Ok(executed) => {
                    batches += 1;
                    match executed.transition.output {
                        strata_kernel::Output::Erased {
                            changed: done,
                            more,
                            next_object,
                            next_after,
                            ..
                        } => {
                            changed += done.len();
                            if more {
                                object = next_object;
                                after = next_after;
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        let tid = self.tenants[0].tenant().clone();
        let mut remaining = 0usize;
        for probe in &erasure.probes {
            let need = vec![Need::Record {
                object: probe.object.clone(),
                id: probe.id,
            }];
            if let Ok(state) = self.opened.reference.load(&tid, &need) {
                let held = state
                    .tenant
                    .as_deref()
                    .and_then(|t| t.record(&probe.object, probe.id))
                    .map(|r| r.get(&probe.field).clone());
                if held.as_ref() == Some(&probe.plaintext) {
                    remaining += 1;
                }
            }
        }
        let probes = erasure.probes.len();
        self.erased.extend(erasure.probes);
        self.resync_live();
        match failure {
            Some(e) => (format!("refused: {e}"), false),
            None => (
                format!(
                    "erased in {batches} decision(s): {changed} record(s) changed, \
                     {remaining} of {probes} probe(s) still hold the plaintext"
                ),
                changed > 0 && remaining == 0,
            ),
        }
    }

    /// An update of one of the hottest records of the fault target through the reference
    /// handle; effective when the tenant generation moved.
    fn foreign_commit(&mut self, idx: usize, fault: &Fault) -> (String, bool) {
        let t = fault.tenant.unwrap_or(0);
        let Some(object) = self.fault_targets.get(&idx).cloned() else {
            return ("no target object".to_string(), false);
        };
        let width = self.live[t].get(&object).map_or(0, |ids| ids.len().min(3));
        if width == 0 {
            return (format!("no records of {object}"), false);
        }
        let id = self.live[t][&object][self.rng.below(width as u64) as usize];
        let Some(fields) = self.tenants[t].update_fields(&object, id, self.counter) else {
            return (format!("no update field for {object}"), false);
        };
        let tid = self.tenants[t].tenant().clone();
        let ctx = self.ctx(t, Actor::Owner);
        let before = self.generation(&tid);
        let done = execute(
            self.opened.reference.as_ref(),
            &Intent::UpdateRecord {
                object: object.clone(),
                id,
                fields,
                expected_version: None,
            },
            &ctx,
        );
        let after = self.generation(&tid);
        match done {
            Ok(done) => (
                format!(
                    "foreign commit to {object} in {tid} at revision {}",
                    done.receipt.revision.0
                ),
                before != after,
            ),
            Err(e) => (format!("foreign commit to {object} refused: {e}"), false),
        }
    }

    /// Read the header and the hottest records straight after a fault, through both handles.
    fn probe_after(&mut self, t: usize, fault: &str, op: u64) {
        if self.cfg.coherence == CoherenceLevel::Off {
            return;
        }
        let mut probes: Vec<Vec<Need>> = vec![Vec::new()];
        let names: Vec<String> = self
            .scenario
            .mix
            .iter()
            .filter_map(|m| m.object.clone())
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        for name in names {
            let Some(object) = self.resolve(t, &name) else {
                continue;
            };
            if !seen.insert(object.clone()) {
                continue;
            }
            for id in self.live[t].get(&object).into_iter().flatten().take(4) {
                probes.push(vec![Need::Record {
                    object: object.clone(),
                    id: *id,
                }]);
            }
        }
        // After an erasure: every record of the erased subject, through the store under test.
        if fault == "Erasure" {
            for probe in &self.erased {
                probes.push(vec![Need::Record {
                    object: probe.object.clone(),
                    id: probe.id,
                }]);
            }
        }
        for needs in probes {
            let label = format!("probe after {fault}");
            self.read_pair(op, 0, t, OpKind::Header, &label, &needs, false, Some(fault));
        }
        self.sweep_erased(op);
    }

    /// Look for the plaintext of an erased subject in what the store under test answers.
    fn sweep_erased(&mut self, op: u64) {
        let tid = self.tenants[0].tenant().clone();
        for probe in self.erased.clone() {
            let need = vec![Need::Record {
                object: probe.object.clone(),
                id: probe.id,
            }];
            let Ok(state) = self.built.store.load(&tid, &need) else {
                continue;
            };
            self.report.coherence.erasure_probes_read += 1;
            let held = state
                .tenant
                .as_deref()
                .and_then(|t| t.record(&probe.object, probe.id))
                .map(|record| record.get(&probe.field).clone());
            // A leak is plaintext the store under test serves that the reference no longer holds.
            let reference_held = self
                .opened
                .reference
                .load(&tid, &need)
                .ok()
                .and_then(|state| {
                    state
                        .tenant
                        .as_deref()
                        .and_then(|t| t.record(&probe.object, probe.id))
                        .map(|record| record.get(&probe.field).clone())
                });
            if held.as_ref() == Some(&probe.plaintext)
                && reference_held.as_ref() != Some(&probe.plaintext)
            {
                self.report.coherence.plaintext_leaks += 1;
                self.diverge(
                    op,
                    "erasure-probe",
                    &tid,
                    &format!("{}#{}.{}", probe.object, probe.id.0, probe.field),
                    "the subject's plaintext".to_string(),
                    "erased".to_string(),
                    Some("Erasure"),
                );
            }
        }
    }

    fn finish(mut self, wall_ms: f64, populate_ms: f64) -> Report {
        let scenario = self.scenario;
        let mut report = std::mem::take(&mut self.report);
        report.faults_ineffective = report.faults.iter().filter(|f| !f.effective).count() as u64;
        report.scenario = scenario.name.clone();
        report.store = self.cfg.store.name();
        report.mode = self.cfg.mode.name().to_string();
        report.seed = self.cfg.seed;
        report.scale = scenario.scale;
        report.scale_name = self.tenants[0].label();
        report.unmapped = self.tenants[0].unmapped();
        report.length = scenario.length;
        report.clients = scenario.clients;
        report.modules = scenario.modules.clone();
        report.measures = scenario.measures.clone();
        report.refutation = Some(scenario.refutation.clone());
        report.determinism = Determinism {
            sequence_digest: hex(&self.sequence.clone().finalize()),
            answer_digest: hex(&self.answers.clone().finalize()),
        };
        report.store_side = StoreSide {
            counts: self.built.counting.counts(),
            schema_decodes: self.decodes,
        };
        report.wrapper = Wrapper::of(self.built.wrapper_stats(self.cfg.mode));
        report.object_aliases = std::mem::take(&mut self.aliases);
        let mut overall = std::mem::take(&mut self.samples);
        let timed_ms = self.timed.as_secs_f64() * 1e3;
        report.timing = Timing {
            wall_ms,
            timed_ms,
            throughput_ops_per_s: if timed_ms > 0.0 {
                overall.len() as f64 / (timed_ms / 1e3)
            } else {
                0.0
            },
            overall: Latency::of(&mut overall),
            by_kind: std::mem::take(&mut self.samples_by_kind)
                .into_iter()
                .map(|(kind, mut samples)| (kind, Latency::of(&mut samples)))
                .collect(),
            populate_ms,
        };
        report
    }
}
