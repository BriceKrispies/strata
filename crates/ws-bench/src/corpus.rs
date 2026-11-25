//! What the harness needs from the enterprise corpus, and a stand-in that works today.
//!
//! The corpus loader and population generator (WG-5398) installs the 14 product files into a
//! tenant of a [`Store`] and fills it from a seed. The harness asks for exactly this much of it:
//!
//! - [`CorpusLoader::populate`]: build one tenant at a scale from a seed, and return it as a
//!   [`CorpusTenant`];
//! - the [`Catalogue`] of the tenant: the ids created per object type, hottest first, with the
//!   size of the hot set and (optionally) a field to look records up by;
//! - the product knowledge the harness cannot have: which fields to write to create or update a
//!   record of a type, a named query to read, a ledger posting, the data subject to erase and the
//!   plaintext that must not survive it, a schema change to make.
//!
//! Object types are named as the corpus names them (`content__article`, `commerce__order`). A
//! scenario that names a type the catalogue lacks is resolved to a type it has, deterministically,
//! and the report says so ([`Catalogue::resolve`]); that is how the stand-in below runs scenarios
//! written for the corpus.

use crate::rng::{hash_text, Rng};
use std::collections::BTreeMap;
use std::sync::Arc;
use strata_kernel::ext::{RecordExt, TenantStateExt};
use strata_kernel::testkit;
use strata_kernel::{
    Context, DataSubject, ExistingData, FieldDef, FieldId, FieldType, Intent, Need, ObjectTypeId,
    PersonalDeclaration, PersonalKind, Principal, PrincipalId, ReadRequest, RecordId,
    RetentionRule, RetentionStart, SchemaChange, TenantId, Timestamp, Value,
};
use strata_storage_api::{execute, Store};

/// How a tenant is asked for.
#[derive(Clone, Debug)]
pub struct TenantRequest {
    /// The tenant id.
    pub name: String,
    /// The population scale S, in the loader's own unit (the stand-in: notes per tenant).
    pub scale: u32,
    pub seed: u64,
    /// One of the small tenants beside the primary one: a platform-template tenant, not the
    /// corpus.
    pub small: bool,
}

/// Install and populate a tenant.
pub trait CorpusLoader {
    /// Create the tenant `request.name` on `store` (replacing one of that name) and fill it.
    fn populate(
        &self,
        store: Arc<dyn Store>,
        request: &TenantRequest,
    ) -> Result<Box<dyn CorpusTenant>, String>;

    /// Address a tenant that `store` holds already (a reused populated store) without building it.
    /// The default builds it.
    fn reuse(
        &self,
        store: Arc<dyn Store>,
        request: &TenantRequest,
    ) -> Result<Box<dyn CorpusTenant>, String> {
        self.populate(store, request)
    }
}

/// The records of one object type in the populated tenant.
#[derive(Clone, Debug, Default)]
pub struct ObjectIds {
    /// Every id created, hottest first: the harness draws ranks from this order with a Zipf skew.
    pub ids: Vec<RecordId>,
    /// How many of the first ids are the type's hot set.
    pub hot: usize,
    /// A field to look records up by, with the value each id holds (parallel to `ids`).
    pub lookup: Option<Lookup>,
}

#[derive(Clone, Debug)]
pub struct Lookup {
    pub field: FieldId,
    pub values: Vec<Value>,
}

#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub objects: BTreeMap<ObjectTypeId, ObjectIds>,
}

impl Catalogue {
    /// The catalogue's name for the type a scenario names: itself, or, when the catalogue has no
    /// such type, one chosen by the name's hash among those it has. `None` for an empty catalogue.
    pub fn resolve(&self, wanted: &str) -> Option<(ObjectTypeId, bool)> {
        if let Some((object, _)) = self.objects.iter().find(|(o, _)| o.as_str() == wanted) {
            return Some((object.clone(), false));
        }
        let at = (hash_text(wanted) % self.objects.len().max(1) as u64) as usize;
        self.objects.keys().nth(at).map(|o| (o.clone(), true))
    }
}

/// Who acts.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    serde::Deserialize,
    serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Actor {
    Owner,
    #[default]
    Editor,
    Reader,
    System,
}

/// A value of a subject's that must not be readable after the subject is erased.
#[derive(Clone, Debug)]
pub struct Probe {
    pub object: ObjectTypeId,
    pub id: RecordId,
    pub field: FieldId,
    pub plaintext: Value,
}

#[derive(Clone, Debug)]
pub struct Erasure {
    pub subject: PrincipalId,
    pub probes: Vec<Probe>,
}

/// A populated tenant and what the harness needs to drive it. `n` is a counter the harness
/// advances per call, for values that must differ (a slug, a title).
pub trait CorpusTenant: Send + Sync {
    fn tenant(&self) -> &TenantId;
    /// What the tenant is, for the report (`small`, `notes-60`).
    fn label(&self) -> String {
        String::new()
    }
    fn catalogue(&self) -> &Catalogue;
    /// Operations the tenant could not map onto its schema at opening, for the report.
    fn unmapped(&self) -> Vec<String> {
        Vec::new()
    }
    /// The principal that acts as `who` on `subject`, an object type or a query of the corpus
    /// (its module's member holding the matching roles).
    fn actor(&self, who: Actor, subject: Option<&str>) -> Principal;
    /// The fields to create a record of `object` with.
    fn create_fields(&self, object: &ObjectTypeId, n: u64) -> Option<BTreeMap<FieldId, Value>>;
    /// The fields to change on an existing record of `object`.
    fn update_fields(
        &self,
        object: &ObjectTypeId,
        id: RecordId,
        n: u64,
    ) -> Option<BTreeMap<FieldId, Value>>;
    /// The request that runs the corpus query `query` (`service__queue_backlog`): a queue, a
    /// segment, a dashboard.
    fn named_query(&self, query: &str, n: u64) -> Option<ReadRequest>;
    /// A ledger read (a stock or balance need).
    fn ledger_need(&self, n: u64) -> Option<Need>;
    /// A ledger posting.
    fn ledger_intent(&self, n: u64) -> Option<Intent>;
    /// A data subject whose erasure the tenant can show.
    fn erasure(&self, n: u64) -> Option<Erasure>;
    /// Evolve the tenant schema the way it is really evolved (a product upgrade), through the
    /// handle the tenant was populated through. `None` when the tenant has no such path; it is
    /// then changed with [`CorpusTenant::schema_change`]. The text says what was done.
    fn evolve(&self, _n: u64) -> Option<Result<String, String>> {
        None
    }
    /// An additive schema change, for a tenant that has no upgrade path.
    fn schema_change(&self, n: u64) -> Option<Vec<SchemaChange>>;
}

/// The stand-in loader: the kernel testkit's notes tenant (`person` and `note`), populated at a
/// scale. Enough to compile and unit-check the harness; the coordinator replaces it with the
/// corpus loader.
pub struct NotesLoader;

pub struct NotesTenant {
    id: TenantId,
    catalogue: Catalogue,
    store: Arc<dyn Store>,
    /// The person type carries a personal field (`nick`), so an erasure can be shown.
    personal: bool,
}

fn ctx_for(tenant: &TenantId, principal: Principal, now: i64) -> Context {
    Context {
        tenant: tenant.clone(),
        principal,
        session: None,
        now: Timestamp(now),
        correlation: format!("ws-bench-{now}"),
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

impl CorpusLoader for NotesLoader {
    fn populate(
        &self,
        store: Arc<dyn Store>,
        request: &TenantRequest,
    ) -> Result<Box<dyn CorpusTenant>, String> {
        let shared = store.clone();
        let store = store.as_ref();
        let id = TenantId::new(&request.name).map_err(|e| format!("tenant id: {e:?}"))?;
        store.reset_tenant(&id).map_err(|e| e.to_string())?;
        let mut now = 1_000;
        let mut step = |principal: Principal, intent: Intent| -> Result<(), String> {
            now += 1;
            execute(store, &intent, &ctx_for(&id, principal, now))
                .map(|_| ())
                .map_err(|e| format!("{e}"))
        };
        let owner = testkit::principal_id(&format!("{}:owner", request.name));
        step(
            Principal::System,
            Intent::CreateTenant {
                name: request.name.clone(),
                owner: owner.clone(),
            },
        )?;
        let owner_p = Principal::User(owner);
        step(
            owner_p.clone(),
            Intent::ChangeSchema {
                changes: testkit::notes_schema(),
                expected_version: None,
                existing: ExistingData::Reject,
            },
        )?;
        for (who, role) in [
            ("editor", "editor"),
            ("writer1", "writer"),
            ("writer2", "writer"),
        ] {
            step(
                owner_p.clone(),
                Intent::AssignRole {
                    principal: testkit::principal_id(&format!("{}:{who}", request.name)),
                    role: testkit::role(role),
                },
            )?;
        }
        // A personal field on the person type, so that an erasure has something to erase. If the
        // kernel refuses it the stand-in simply offers no erasure.
        let personal = step(
            owner_p.clone(),
            Intent::ChangeSchema {
                changes: vec![
                    SchemaChange::DefineField {
                        object: testkit::object("person"),
                        field: FieldDef {
                            personal: Some(PersonalDeclaration {
                                kind: PersonalKind::Name,
                                subject: DataSubject::Field {
                                    field: testkit::field("nick"),
                                },
                                searchable: true,
                            }),
                            normalise: None,
                            ..testkit::field_def("nick", FieldType::Str, false)
                        },
                    },
                    SchemaChange::SetRetention {
                        object: testkit::object("person"),
                        retention: RetentionRule {
                            days: 365,
                            from: RetentionStart::Created,
                        },
                    },
                ],
                expected_version: None,
                existing: ExistingData::Reject,
            },
        )
        .is_ok();
        let editor = testkit::user(&format!("{}:editor", request.name));
        let people = (request.scale / 10).max(1);
        for i in 0..people {
            let mut fields =
                BTreeMap::from([(testkit::field("name"), testkit::text(&format!("P{i}")))]);
            if personal {
                fields.insert(testkit::field("nick"), testkit::text(&format!("Nick{i}")));
            }
            step(
                editor.clone(),
                Intent::CreateRecord {
                    object: testkit::object("person"),
                    fields,
                },
            )?;
        }
        for i in 0..request.scale {
            step(
                editor.clone(),
                Intent::CreateRecord {
                    object: testkit::object("note"),
                    fields: BTreeMap::from([
                        (testkit::field("title"), testkit::text(&format!("T{i}"))),
                        (testkit::field("slug"), testkit::text(&format!("s{i}"))),
                        (testkit::field("rank"), Value::Int(i64::from(i % 97))),
                    ]),
                },
            )?;
        }
        let state = store.load_complete(&id).map_err(|e| e.to_string())?;
        let tenant = state.tenant.ok_or("the tenant was not created")?;
        let mut rng = Rng(request.seed ^ hash_text(&request.name));
        let mut objects = BTreeMap::new();
        for (object, records) in &tenant.records {
            let mut ids: Vec<RecordId> = records.keys().copied().collect();
            // The popularity order is drawn from the seed, not the creation order.
            for i in (1..ids.len()).rev() {
                ids.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let lookup = (object.as_str() == "note").then(|| Lookup {
                field: testkit::field("slug"),
                values: ids
                    .iter()
                    .map(|id| records[id].get(&testkit::field("slug")).clone())
                    .collect(),
            });
            let hot = (ids.len() / 10).max(1);
            objects.insert(object.clone(), ObjectIds { ids, hot, lookup });
        }
        Ok(Box::new(NotesTenant {
            id,
            catalogue: Catalogue { objects },
            store: shared,
            personal,
        }))
    }
}

impl CorpusTenant for NotesTenant {
    fn tenant(&self) -> &TenantId {
        &self.id
    }

    fn label(&self) -> String {
        "stub".to_string()
    }

    fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    fn actor(&self, who: Actor, _subject: Option<&str>) -> Principal {
        match who {
            Actor::Owner => testkit::user(&format!("{}:owner", self.id)),
            Actor::Editor => testkit::user(&format!("{}:editor", self.id)),
            Actor::Reader => Principal::Anonymous,
            Actor::System => Principal::System,
        }
    }

    fn create_fields(&self, object: &ObjectTypeId, n: u64) -> Option<BTreeMap<FieldId, Value>> {
        match object.as_str() {
            "note" => Some(BTreeMap::from([
                (testkit::field("title"), testkit::text(&format!("N{n}"))),
                (testkit::field("slug"), testkit::text(&format!("w{n}"))),
            ])),
            "person" => Some(BTreeMap::from([(
                testkit::field("name"),
                testkit::text(&format!("W{n}")),
            )])),
            _ => None,
        }
    }

    fn update_fields(
        &self,
        object: &ObjectTypeId,
        _id: RecordId,
        n: u64,
    ) -> Option<BTreeMap<FieldId, Value>> {
        match object.as_str() {
            "note" => Some(BTreeMap::from([
                (testkit::field("title"), testkit::text(&format!("U{n}"))),
                (testkit::field("rank"), Value::Int((n % 100) as i64)),
            ])),
            "person" => Some(BTreeMap::from([(
                testkit::field("name"),
                testkit::text(&format!("V{n}")),
            )])),
            _ => None,
        }
    }

    fn named_query(&self, _query: &str, _n: u64) -> Option<ReadRequest> {
        None
    }

    fn ledger_need(&self, _n: u64) -> Option<Need> {
        None
    }

    fn ledger_intent(&self, _n: u64) -> Option<Intent> {
        None
    }

    fn erasure(&self, n: u64) -> Option<Erasure> {
        if !self.personal {
            return None;
        }
        let person = testkit::object("person");
        let ids = &self.catalogue.objects.get(&person)?.ids;
        let id = *ids.get(n as usize % ids.len().max(1))?;
        let nick = testkit::field("nick");
        let state = self
            .store
            .load(
                &self.id,
                &[Need::Record {
                    object: person.clone(),
                    id,
                }],
            )
            .ok()?;
        let plaintext = state
            .tenant
            .as_deref()?
            .record(&person, id)?
            .get(&nick)
            .clone();
        let Value::Str(text) = &plaintext else {
            return None;
        };
        Some(Erasure {
            subject: PrincipalId::new(text.clone()).ok()?,
            probes: vec![Probe {
                object: person,
                id,
                field: nick,
                plaintext,
            }],
        })
    }

    fn schema_change(&self, n: u64) -> Option<Vec<SchemaChange>> {
        Some(vec![SchemaChange::DefineField {
            object: testkit::object("note"),
            field: testkit::field_def(&format!("extra{n}"), FieldType::Str, false),
        }])
    }
}
