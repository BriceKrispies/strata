//! What a population created, so that a scenario can address records without searching.

use serde_json::{json, Value as Json};
use std::collections::BTreeMap;

/// A reference field of an object type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefEdge {
    pub field: String,
    pub target: String,
    pub required: bool,
}

/// One object type's records.
#[derive(Clone, Debug, Default)]
pub struct TypeEntry {
    /// The module (product) that owns the object type.
    pub module: String,
    /// The ids of every record of the type, in creation order.
    pub ids: Vec<u64>,
    /// The records the reference draws favour, most popular first: the few hot authors, products,
    /// queues and contacts of the type. Draws of a reference to this type follow this order.
    pub hot: Vec<u64>,
    /// The records whose payload is far above the type's typical size.
    pub large: Vec<u64>,
    /// The bytes of field values the population wrote for the type.
    pub bytes: u64,
    pub refs: Vec<RefEdge>,
    /// The fields that hold personal data.
    pub personal: Vec<String>,
}

/// A person who recurs across modules: the hot authors, customers and respondents.
#[derive(Clone, Debug)]
pub struct HotPerson {
    pub name: String,
    pub email: String,
}

/// Everything a population created.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub tenant: String,
    pub owner: String,
    pub scale: String,
    pub seed: u64,
    /// Records created (one `create-record` commit each).
    pub records: u64,
    /// Every commit the population made: creations and the updates that set the references
    /// pointing at a type created later (the current revision of an article, the featured tile of
    /// a dashboard).
    pub commits: u64,
    pub types: BTreeMap<String, TypeEntry>,
    /// The people the shared pool draws favour most, most popular first.
    pub hot_people: Vec<HotPerson>,
    /// The size of the shared pool of people.
    pub people: u64,
    /// Records the kernel refused in a lenient run, by object type: how many, and the first
    /// reason. Empty when the run was strict, which stops at the first.
    pub rejections: BTreeMap<String, (u64, String)>,
}

/// `ids` as inclusive `[first, last]` ranges of consecutive numbers.
fn ranges(ids: &[u64]) -> Vec<[u64; 2]> {
    let mut out: Vec<[u64; 2]> = Vec::new();
    for &id in ids {
        match out.last_mut() {
            Some(last) if last[1] + 1 == id => last[1] = id,
            _ => out.push([id, id]),
        }
    }
    out
}

impl Catalogue {
    pub fn total_ids(&self) -> u64 {
        self.types.values().map(|t| t.ids.len() as u64).sum()
    }

    /// The ids of one object type, or none when it has none.
    pub fn ids(&self, object: &str) -> &[u64] {
        self.types.get(object).map_or(&[], |t| t.ids.as_slice())
    }

    /// The hot set of one object type, most popular first.
    pub fn hot(&self, object: &str) -> &[u64] {
        self.types.get(object).map_or(&[], |t| t.hot.as_slice())
    }

    /// The object types of one module.
    pub fn module_types(&self, module: &str) -> Vec<&str> {
        self.types
            .iter()
            .filter(|(_, t)| t.module == module)
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// The catalogue as JSON: ids as ranges, so a 130,000-record tenant stays a small document.
    pub fn to_json(&self) -> Json {
        let types: serde_json::Map<String, Json> = self
            .types
            .iter()
            .map(|(id, t)| {
                (
                    id.clone(),
                    json!({
                        "module": t.module,
                        "count": t.ids.len(),
                        "id_ranges": ranges(&t.ids),
                        "hot": t.hot,
                        "large": t.large,
                        "bytes": t.bytes,
                        "refs": t.refs.iter().map(|r| json!({
                            "field": r.field, "target": r.target, "required": r.required
                        })).collect::<Vec<_>>(),
                        "personal_fields": t.personal,
                    }),
                )
            })
            .collect();
        json!({
            "tenant": self.tenant,
            "owner": self.owner,
            "scale": self.scale,
            "seed": self.seed,
            "records": self.records,
            "commits": self.commits,
            "people": self.people,
            "rejections": self.rejections.iter().map(|(k, (n, why))| (k.clone(), json!({"count": n, "first": why}))).collect::<serde_json::Map<String, Json>>(),
            "hot_people": self.hot_people.iter().map(|p| json!({"name": p.name, "email": p.email})).collect::<Vec<_>>(),
            "types": types,
        })
    }
}
