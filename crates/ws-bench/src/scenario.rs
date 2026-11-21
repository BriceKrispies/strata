//! A scenario: a product workload over the enterprise corpus, as data.
//!
//! Scenario files are JSONC (whole-line `//` comments, trailing commas) under
//! `tests/scenarios/`. Unknown keys are refused, so a misspelt field is an error and not a silent
//! default.

use crate::corpus::Actor;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    /// The corpus modules the scenario touches.
    pub modules: Vec<String>,
    /// Population scale S of the primary tenant, in the loader's unit.
    pub scale: u32,
    /// Further small tenants beside the primary one (the many-small-tenants cell).
    #[serde(default)]
    pub small_tenants: Option<SmallTenants>,
    /// Operations in the run.
    pub length: usize,
    /// Logical clients interleaved by the seed.
    #[serde(default = "one")]
    pub clients: u32,
    /// The Zipf exponent over a type's ids, for a mix entry that sets none.
    #[serde(default = "default_skew")]
    pub skew: f64,
    pub mix: Vec<MixEntry>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    /// What to look at in the report for this scenario.
    pub measures: Vec<String>,
    /// What would count against the design here.
    pub refutation: Refutation,
}

fn one() -> u32 {
    1
}

fn default_skew() -> f64 {
    1.0
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SmallTenants {
    pub count: u32,
    pub scale: u32,
    /// The Zipf exponent over tenants (the primary tenant is rank 0).
    #[serde(default = "default_skew")]
    pub skew: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpKind {
    /// One record by id (`Need::Record`).
    Get,
    /// Records by a field value (`Need::ByField`).
    ByField,
    /// A page of an object type (`Need::PageOf`).
    Page,
    /// A whole object type (`Need::AllOf`).
    Scan,
    /// The tenant header (no needs).
    Header,
    /// A named query through `strata_storage_api::query`.
    Query,
    /// A ledger read (`Need::Ledger`).
    LedgerRead,
    Create,
    Update,
    Delete,
    PostLedger,
}

impl OpKind {
    pub fn is_write(self) -> bool {
        matches!(
            self,
            OpKind::Create | OpKind::Update | OpKind::Delete | OpKind::PostLedger
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            OpKind::Get => "get",
            OpKind::ByField => "by-field",
            OpKind::Page => "page",
            OpKind::Scan => "scan",
            OpKind::Header => "header",
            OpKind::Query => "query",
            OpKind::LedgerRead => "ledger-read",
            OpKind::Create => "create",
            OpKind::Update => "update",
            OpKind::Delete => "delete",
            OpKind::PostLedger => "post-ledger",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Within {
    /// Draw from every id of the type.
    #[default]
    All,
    /// Draw from the type's hot set only.
    Hot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct MixEntry {
    pub op: OpKind,
    pub weight: u32,
    /// The corpus object type the operation is about (`content__article`). Absent for an
    /// operation that needs none (`header`, `ledger-read`, `post-ledger`).
    #[serde(default)]
    pub object: Option<String>,
    /// A Zipf exponent for this entry, overriding the scenario's.
    #[serde(default)]
    pub skew: Option<f64>,
    #[serde(default)]
    pub within: Within,
    #[serde(default)]
    pub actor: Actor,
    /// The corpus query a `query` runs (`service__queue_backlog`); the tenant builds its request.
    #[serde(default)]
    pub query: Option<String>,
    /// The page size of a `page`.
    #[serde(default)]
    pub page_size: Option<u32>,
    /// An update decided on the revision the client last saw, committed only while the tenant is
    /// still at it: a lost race is a conflict and a retry, not a silent overwrite.
    #[serde(default)]
    pub contended: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultKind {
    /// A write through the second handle (a commit the first handle's caches do not see).
    ForeignCommit,
    /// The tenant restored to the snapshot taken before the first operation, through the second
    /// handle, after a foreign commit that the snapshot does not hold.
    Restore,
    /// A data-subject erasure through the second handle.
    Erasure,
    /// An additive schema change through the second handle (a product upgrade).
    SchemaChange,
    /// A handle cycle: the adapter handle under the store under test is replaced by a fresh one on
    /// the same files, and the working set above it is kept (it must change nothing).
    Reopen,
    /// A process restart: the whole stack is dropped, another handle commits while the process is
    /// down, and a new stack is built over a fresh handle with an empty working set (a `stale`
    /// cache is the exception: it is persisted across the restart).
    Restart,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Fault {
    pub kind: FaultKind,
    /// The fault lands before the operation at this fraction of the run, 0 to 1.
    #[serde(default)]
    pub at: Option<f64>,
    /// Or: before every this-many-th operation (a background writer).
    #[serde(default)]
    pub every: Option<usize>,
    /// The object type a foreign commit writes to, when it should be a given one.
    #[serde(default)]
    pub object: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Refutation {
    /// The bullet of "What would refute the design" in `docs/working-set.md` this applies, or the
    /// coherence refutation of the coherence runs.
    pub criterion: String,
    /// The result in this scenario that would refute the design.
    pub result: String,
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Scenario, String> {
        let scenario: Scenario =
            serde_json::from_str(&strip_jsonc(text)).map_err(|e| format!("scenario: {e}"))?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn validate(&self) -> Result<(), String> {
        let fail = |what: &str| Err(format!("scenario {}: {what}", self.name));
        if self.name.is_empty() {
            return Err("scenario: a name is required".to_string());
        }
        if self.modules.is_empty() {
            return fail("modules is empty");
        }
        if self.length == 0 {
            return fail("length is 0");
        }
        if self.clients == 0 {
            return fail("clients is 0");
        }
        if self.mix.is_empty() || self.mix.iter().all(|m| m.weight == 0) {
            return fail("the mix has no weight");
        }
        if self.skew < 0.0 || self.mix.iter().any(|m| m.skew.is_some_and(|s| s < 0.0)) {
            return fail("a skew is negative");
        }
        for entry in &self.mix {
            let needs_object = matches!(
                entry.op,
                OpKind::Get
                    | OpKind::ByField
                    | OpKind::Page
                    | OpKind::Scan
                    | OpKind::Create
                    | OpKind::Update
                    | OpKind::Delete
            );
            if needs_object && entry.object.is_none() {
                return fail(&format!("{} needs an object", entry.op.name()));
            }
            if entry.op == OpKind::Query && entry.query.is_none() {
                return fail("query needs a query");
            }
        }
        for fault in &self.faults {
            match (fault.at, fault.every) {
                (Some(at), None) if (0.0..=1.0).contains(&at) => {}
                (None, Some(every)) if every > 0 => {}
                _ => return fail("a fault names `at` in 0..1 or a positive `every`, one of them"),
            }
        }
        if self.measures.is_empty() {
            return fail("measures is empty");
        }
        Ok(())
    }
}

/// Remove whole-line `//` comments and trailing commas.
pub fn strip_jsonc(text: &str) -> String {
    let kept: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let chars: Vec<char> = kept.chars().collect();
    let mut out = String::with_capacity(kept.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        }
        if c == ',' {
            let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}
