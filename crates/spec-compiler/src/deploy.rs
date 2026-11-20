//! The deployment output: `specc deploy` turns the components, relationships and budget
//! requirements of a specification into a local container stack, Kubernetes manifests and
//! Prometheus alerting rules.
//!
//! # What is a deployable unit
//!
//! A component is a unit when its `category` is `computational`, `storage`, `infrastructure`
//! or `transport`: something that runs and listens. `logical` components are libraries inside
//! other units, `external` ones are somebody else's systems and `client` ones run in a browser,
//! so none of them is deployed. The specification names no table that maps several components
//! onto one binary, and each component today names its own code in `implementation.notes`, so
//! every unit is one component with its own image, `strata/<id>:dev`. Two things are reported
//! as warnings, never refusals: units whose `implementation.status` is `not-implemented` (their
//! image does not exist yet), and units whose `implementation.notes` name the same code (they
//! would share a binary, but are emitted separately).
//!
//! # The profile
//!
//! The specification says which components exist, where they sit and what they connect to. It
//! does not say how a binary is started: its port, its environment, its health path, its
//! volumes, which ports reach the host. Those facts come from an optional deployment profile, a
//! JSONC file read next to the specification (`deploy.jsonc` in the specification's directory,
//! or the file named by `--profile`), keyed by unit id. Nothing in the generator names a
//! component: a unit without a profile entry is emitted with its image and the default port.
//!
//! ```jsonc
//! {
//!   "containerfile": "deploy/Containerfile",   // relative to the build context
//!   "context": "..",                           // build context, relative to the output directory
//!   "units": {
//!     "<unit id>": {
//!       "target": "<build target>",            // default: the unit id
//!       "port": 4101,                          // what the unit listens on
//!       "env": { "NAME": "value or ${VAR}" },  // compose interpolates ${VAR}
//!       "health": "/path",                     // HTTP GET on the unit's own port
//!       "volumes": ["name:/path"],             // named volumes are declared at the top level
//!       "publish": ["127.0.0.1:4100:4100"],    // ports that reach the host
//!       "shareNetworkWith": "<unit id>"        // join that unit's network namespace
//!     }
//!   }
//! }
//! ```
//!
//! # The local stack
//!
//! The compose file holds only the units that are implemented, because only those have an
//! image that can be built; the others are named in a comment. With a Containerfile in the
//! profile, a unit gets a `build` section. `depends_on` follows the connections (a unit starts
//! after the units it calls, waiting for health when the callee has a health path); an edge
//! that would close a cycle is left out. A unit that shares another's network namespace
//! publishes its ports on that unit, because the namespace is where ports live.
//!
//! # Connections
//!
//! A relationship of kind `calls`, `reads`, `writes`, `publishes`, `subscribes` or `data-flow`
//! between two units is a connection from `from` to `to`; other kinds (`observes`,
//! `depends-on`, `controls`) open no network path. Connections with an endpoint that is not a
//! unit are left out. The Kubernetes policy of a unit admits exactly the units that connect to
//! it. The local stack is coarser, because a compose network cannot name a pair: a connection
//! happens on the network of the zone of its target, and both ends join that network.
//!
//! # Monitoring
//!
//! Every requirement of kind `requirement` whose `evaluate.method` is `budget` becomes an
//! alerting rule. See [`crate::budget`] for how statements are read.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use serde_json::{Map, Value};

use crate::budget::{self, Budget};
use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::emit::{GeneratedFile, HEADER_MARKERS};
use crate::jsonc;

/// Code of a text that is not JSONC.
pub const SYNTAX: &str = "deploy.spec-syntax";
/// Code of a value with the wrong shape.
pub const SHAPE: &str = "deploy.spec-shape";
/// Code of an id or zone that cannot be a Kubernetes name.
pub const NAME_INVALID: &str = "deploy.name-invalid";
/// Code of a unit without a trust zone.
pub const ZONE_MISSING: &str = "deploy.zone-missing";
/// Code of a relationship that names a component the specification does not declare.
pub const ENDPOINT_UNKNOWN: &str = "deploy.relationship-endpoint-unknown";
/// Code of a deployment profile that is not valid JSONC or has the wrong shape.
pub const PROFILE: &str = "deploy.profile-shape";
/// Code of a deployment profile that names a unit the specification does not deploy.
pub const PROFILE_UNIT: &str = "deploy.profile-unit-unknown";
/// Code of an id used twice.
pub const DUPLICATE: &str = "deploy.duplicate-id";

/// The category of components that are deployed.
pub const UNIT_CATEGORIES: [&str; 4] = ["computational", "storage", "infrastructure", "transport"];
/// The relationship kinds that open a network path from `from` to `to`.
pub const CONNECTING_KINDS: [&str; 6] = [
    "calls",
    "reads",
    "writes",
    "publishes",
    "subscribes",
    "data-flow",
];
/// The port every unit listens on until the specification states one.
pub const PORT: u16 = 8080;

/// A deployable unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The component id, also the service name.
    pub id: String,
    /// The component's `ref`, such as `C-11`.
    pub reference: Option<String>,
    /// The trust zone.
    pub zone: String,
    /// The image.
    pub image: String,
    /// False when the component is `not-implemented`.
    pub implemented: bool,
    /// `implementation.notes`.
    pub notes: Option<String>,
    /// The build target.
    pub target: String,
    /// The port the unit listens on.
    pub port: u16,
    /// Environment, values as compose reads them.
    pub env: BTreeMap<String, String>,
    /// The HTTP path of the health check, on the unit's own port.
    pub health: Option<String>,
    /// Volume mounts, `source:target`.
    pub volumes: Vec<String>,
    /// Ports published to the host, in compose syntax.
    pub publish: Vec<String>,
    /// The unit whose network namespace this one joins.
    pub shares_network_with: Option<String>,
    /// True when the profile has an entry for the unit.
    pub configured: bool,
}

/// A budget requirement.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetRequirement {
    /// The `ref`, or the id when there is no ref.
    pub reference: String,
    /// The statement.
    pub statement: String,
    /// The boundary id, if stated.
    pub boundary: Option<String>,
    /// The boundary detail as written, if stated.
    pub detail: Option<String>,
    /// The statement read into a threshold.
    pub budget: Budget,
}

/// Everything the deployment files are made from.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The system id.
    pub system: String,
    /// The units in declaration order.
    pub units: Vec<Unit>,
    /// Connections `(from, to)` between units, sorted, without duplicates or self-loops.
    pub links: Vec<(String, String)>,
    /// The budget requirements in declaration order.
    pub budgets: Vec<BudgetRequirement>,
    /// The Containerfile units are built from, relative to the build context.
    pub containerfile: Option<String>,
    /// The build context, relative to the output directory.
    pub context: String,
    /// True when a profile was given: then only the units it configures are started.
    pub profiled: bool,
}

/// The generated files and the warnings that go with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployOutput {
    /// The files, sorted by path.
    pub files: Vec<GeneratedFile>,
    /// Gaps in the specification that did not stop generation.
    pub warnings: Vec<String>,
}

/// Generates the deployment files from a specification's text, or refuses with every
/// diagnostic. `spec_path` is the path written into the headers.
pub fn generate(spec_path: &str, text: &str) -> Result<DeployOutput, Vec<Diagnostic>> {
    generate_with_profile(spec_path, text, None)
}

/// Like [`generate`], with the text of a deployment profile (see the module documentation).
pub fn generate_with_profile(
    spec_path: &str,
    text: &str,
    profile: Option<&str>,
) -> Result<DeployOutput, Vec<Diagnostic>> {
    let spec_path = spec_path.replace('\\', "/");
    let plan = load_with_profile(text, profile)?;
    let mut files = vec![
        file("compose.yaml", &spec_path, compose(&plan)),
        file("kubernetes/namespaces.yaml", &spec_path, namespaces(&plan)),
        file("monitoring/budget-alerts.yaml", &spec_path, alerts(&plan)),
    ];
    for unit in &plan.units {
        files.push(file(
            &format!("kubernetes/{}/{}.yaml", unit.zone, unit.id),
            &spec_path,
            unit_manifests(&plan, unit),
        ));
    }
    files.sort();
    Ok(DeployOutput {
        files,
        warnings: warnings(&plan),
    })
}

fn file(path: &str, spec_path: &str, body: String) -> GeneratedFile {
    GeneratedFile {
        path: path.to_string(),
        contents: format!(
            "{} from {}. Do not edit; change the specification and regenerate.\n{body}",
            HEADER_MARKERS[1],
            crate::codetext::line_text(spec_path)
        ),
    }
}

// ---- loading ------------------------------------------------------------------------------

fn is_dns_label(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 50
        && bytes[0].is_ascii_lowercase() | bytes[0].is_ascii_digit()
        && bytes[bytes.len() - 1] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn string<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

/// Reads the plan out of the specification's text.
pub fn load(text: &str) -> Result<Plan, Vec<Diagnostic>> {
    load_with_profile(text, None)
}

/// Reads the plan out of the specification's text and an optional deployment profile.
pub fn load_with_profile(text: &str, profile: Option<&str>) -> Result<Plan, Vec<Diagnostic>> {
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            e.code(SYNTAX),
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let mut diagnostics = Vec::new();
    let Some(top) = value.as_object() else {
        return Err(vec![Diagnostic::new(
            SHAPE,
            SPEC_ENTITY,
            "the specification is not a JSON object",
        )]);
    };
    let system = top
        .get("system")
        .and_then(Value::as_object)
        .and_then(|s| string(s, "id"))
        .unwrap_or("strata")
        .to_string();
    let mut units = Vec::new();
    let mut component_ids = BTreeSet::new();
    for item in array(top, "components", &mut diagnostics) {
        let Some(obj) = item.as_object() else {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                "components",
                "a component is not an object",
            ));
            continue;
        };
        let Some(id) = string(obj, "id") else {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                "components",
                "a component has no string id",
            ));
            continue;
        };
        if !component_ids.insert(id.to_string()) {
            diagnostics.push(Diagnostic::new(
                DUPLICATE,
                id,
                "two components have this id.",
            ));
            continue;
        }
        let category = string(obj, "category").unwrap_or("");
        if !UNIT_CATEGORIES.contains(&category) {
            continue;
        }
        if !is_dns_label(id) {
            diagnostics.push(Diagnostic::new(
                NAME_INVALID,
                id,
                "a unit id must be lower-case letters, digits and hyphens, at most 50 characters, to be a service name.",
            ));
        }
        let zone = string(obj, "trustZone");
        match zone {
            None => diagnostics.push(Diagnostic::new(
                ZONE_MISSING,
                id,
                "the component is deployed but names no trustZone.",
            )),
            Some(z) if !is_dns_label(z) => diagnostics.push(Diagnostic::new(
                NAME_INVALID,
                id,
                format!("the trust zone \"{z}\" cannot be part of a namespace name."),
            )),
            Some(_) => {}
        }
        let implementation = obj.get("implementation").and_then(Value::as_object);
        let status = implementation.and_then(|i| string(i, "status"));
        let notes = implementation
            .and_then(|i| string(i, "notes"))
            .map(str::to_string);
        units.push(Unit {
            id: id.to_string(),
            reference: string(obj, "ref").map(str::to_string),
            zone: zone.unwrap_or("").to_string(),
            image: format!("strata/{id}:dev"),
            implemented: status != Some("not-implemented"),
            notes,
            target: id.to_string(),
            port: PORT,
            env: BTreeMap::new(),
            health: None,
            volumes: Vec::new(),
            publish: Vec::new(),
            shares_network_with: None,
            configured: false,
        });
    }
    let unit_ids: BTreeSet<&str> = units.iter().map(|u| u.id.as_str()).collect();
    let mut links = BTreeSet::new();
    for item in array(top, "relationships", &mut diagnostics) {
        let Some(obj) = item.as_object() else {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                "relationships",
                "a relationship is not an object",
            ));
            continue;
        };
        let id = string(obj, "id").unwrap_or("<relationship>");
        let kind = string(obj, "kind").unwrap_or("");
        if !CONNECTING_KINDS.contains(&kind) {
            continue;
        }
        let (Some(from), Some(to)) = (string(obj, "from"), string(obj, "to")) else {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                id,
                "a relationship needs a string from and a string to.",
            ));
            continue;
        };
        for end in [from, to] {
            if !component_ids.contains(end) {
                diagnostics.push(Diagnostic::new(
                    ENDPOINT_UNKNOWN,
                    id,
                    format!("\"{end}\" is not a declared component."),
                ));
            }
        }
        if from != to && unit_ids.contains(from) && unit_ids.contains(to) {
            links.insert((from.to_string(), to.to_string()));
        }
    }
    let mut budgets = Vec::new();
    for item in array(top, "claims", &mut diagnostics) {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let is_budget = string(obj, "kind") == Some("requirement")
            && obj
                .get("evaluate")
                .and_then(Value::as_object)
                .and_then(|e| string(e, "method"))
                == Some("budget");
        if !is_budget {
            continue;
        }
        let id = string(obj, "id").unwrap_or("<requirement>");
        let Some(statement) = string(obj, "statement") else {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                id,
                "a budget requirement has no statement.",
            ));
            continue;
        };
        budgets.push(BudgetRequirement {
            reference: string(obj, "ref").unwrap_or(id).to_string(),
            statement: statement.to_string(),
            boundary: string(obj, "boundary").map(str::to_string),
            detail: string(obj, "boundaryDetail").map(str::to_string),
            budget: budget::parse(statement),
        });
    }
    let (containerfile, context) = match profile {
        Some(text) => apply_profile(text, &mut units, &mut diagnostics),
        None => (None, "..".to_string()),
    };
    if !diagnostics.is_empty() {
        return Err(crate::diagnostic::dedup(diagnostics));
    }
    Ok(Plan {
        system,
        units,
        links: links.into_iter().collect(),
        budgets,
        containerfile,
        context,
        profiled: profile.is_some(),
    })
}

/// Reads the profile into the units. Returns the Containerfile and the build context.
fn apply_profile(
    text: &str,
    units: &mut [Unit],
    diagnostics: &mut Vec<Diagnostic>,
) -> (Option<String>, String) {
    let bad = |what: String| Diagnostic::new(PROFILE, "deploy profile", what);
    let value = match jsonc::parse(text) {
        Ok(v) => v,
        Err(e) => {
            diagnostics.push(bad(format!("the profile is not valid JSONC: {e}")));
            return (None, "..".to_string());
        }
    };
    let Some(top) = value.as_object() else {
        diagnostics.push(bad("the profile is not a JSON object".to_string()));
        return (None, "..".to_string());
    };
    let containerfile = string(top, "containerfile").map(str::to_string);
    let context = string(top, "context").unwrap_or("..").to_string();
    let Some(entries) = top.get("units").and_then(Value::as_object) else {
        return (containerfile, context);
    };
    let ids: BTreeSet<String> = units.iter().map(|u| u.id.clone()).collect();
    for (id, entry) in entries {
        let Some(unit) = units.iter_mut().find(|u| u.id == *id) else {
            diagnostics.push(Diagnostic::new(
                PROFILE_UNIT,
                id,
                "the profile configures a unit that the specification does not deploy.",
            ));
            continue;
        };
        let Some(obj) = entry.as_object() else {
            diagnostics.push(bad(format!("the entry of \"{id}\" is not an object")));
            continue;
        };
        unit.configured = true;
        if let Some(t) = string(obj, "target") {
            unit.target = t.to_string();
        }
        if let Some(p) = obj.get("port") {
            match p.as_u64().and_then(|n| u16::try_from(n).ok()) {
                Some(n) if n > 0 => unit.port = n,
                _ => diagnostics.push(bad(format!("\"{id}\": port is not a port number"))),
            }
        }
        if let Some(h) = string(obj, "health") {
            unit.health = Some(h.to_string());
        }
        if let Some(env) = obj.get("env") {
            match env.as_object() {
                Some(map) => {
                    for (k, v) in map {
                        match v.as_str() {
                            Some(v) => {
                                unit.env.insert(k.clone(), v.to_string());
                            }
                            None => {
                                diagnostics.push(bad(format!("\"{id}\": env {k} is not a string")))
                            }
                        }
                    }
                }
                None => diagnostics.push(bad(format!("\"{id}\": env is not an object"))),
            }
        }
        for (key, slot) in [
            ("volumes", &mut unit.volumes),
            ("publish", &mut unit.publish),
        ] {
            if let Some(list) = obj.get(key) {
                let items: Option<Vec<String>> = list
                    .as_array()
                    .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
                match items {
                    Some(items) => *slot = items,
                    None => {
                        diagnostics.push(bad(format!("\"{id}\": {key} is not a list of strings")))
                    }
                }
            }
        }
        if let Some(other) = string(obj, "shareNetworkWith") {
            if other == id || !ids.contains(other) {
                diagnostics.push(bad(format!(
                    "\"{id}\": shareNetworkWith \"{other}\" is not another deployed unit"
                )));
            } else {
                unit.shares_network_with = Some(other.to_string());
            }
        }
    }
    (containerfile, context)
}

fn array<'a>(
    top: &'a Map<String, Value>,
    key: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> &'a [Value] {
    match top.get(key) {
        None => &[],
        Some(Value::Array(items)) => items,
        Some(_) => {
            diagnostics.push(Diagnostic::new(
                SHAPE,
                SPEC_ENTITY,
                format!("\"{key}\" is not an array"),
            ));
            &[]
        }
    }
}

// ---- warnings -----------------------------------------------------------------------------

fn warnings(plan: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    let missing: Vec<&str> = plan
        .units
        .iter()
        .filter(|u| !u.implemented)
        .map(|u| u.id.as_str())
        .collect();
    if !missing.is_empty() {
        out.push(format!(
            "warning[deploy.not-implemented]: {} of {} units are not implemented, so their images do not exist yet: {}",
            missing.len(),
            plan.units.len(),
            missing.join(", ")
        ));
    }
    let mut seen: Vec<(&str, Vec<&str>)> = Vec::new();
    for unit in plan.units.iter().filter(|u| u.implemented) {
        let Some(notes) = unit.notes.as_deref() else {
            continue;
        };
        match seen.iter_mut().find(|(n, _)| *n == notes) {
            Some((_, ids)) => ids.push(&unit.id),
            None => seen.push((notes, vec![&unit.id])),
        }
    }
    for (notes, ids) in seen.iter().filter(|(_, ids)| ids.len() > 1) {
        out.push(format!(
            "warning[deploy.shared-binary]: units {} are all implemented by \"{notes}\"; the specification has no table mapping them to one binary, so each is emitted as its own unit",
            ids.join(", ")
        ));
    }
    out
}

// ---- YAML ---------------------------------------------------------------------------------

/// A YAML double-quoted scalar (JSON strings are valid YAML).
fn q(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

fn zone_network(zone: &str) -> String {
    format!("zone-{zone}")
}

fn namespace(zone: &str) -> String {
    format!("strata-{zone}")
}

fn zone_of<'a>(plan: &'a Plan, id: &str) -> &'a str {
    plan.units
        .iter()
        .find(|u| u.id == id)
        .map_or("", |u| u.zone.as_str())
}

/// The networks a unit joins: the zone network of the target of every connection it takes
/// part in between runnable units, as source or target.
fn networks_of(plan: &Plan, unit: &Unit, runnable_ids: &BTreeSet<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (from, to) in &plan.links {
        if !runnable_ids.contains(from.as_str()) || !runnable_ids.contains(to.as_str()) {
            continue;
        }
        if *from == unit.id || *to == unit.id {
            out.insert(zone_network(zone_of(plan, to)));
        }
    }
    out
}

/// The units the local stack starts: implemented ones, and when there is a profile, only those
/// it configures (an implemented component without an entry is a library inside another unit).
fn runnable(plan: &Plan) -> Vec<&Unit> {
    plan.units
        .iter()
        .filter(|u| u.implemented && (!plan.profiled || u.configured))
        .collect()
}

/// The units `unit` waits for: the runnable units it connects to, minus any edge that would
/// close a cycle (compose refuses a cycle). A unit that shares a network namespace always
/// waits for its owner.
fn dependencies<'a>(plan: &'a Plan, unit: &Unit, runnable_ids: &BTreeSet<&str>) -> Vec<&'a str> {
    let mut edges: Vec<(&str, &str)> = Vec::new();
    for (from, to) in &plan.links {
        if runnable_ids.contains(from.as_str()) && runnable_ids.contains(to.as_str()) {
            edges.push((from, to));
        }
    }
    for u in &plan.units {
        if let Some(owner) = &u.shares_network_with {
            if runnable_ids.contains(u.id.as_str()) && runnable_ids.contains(owner.as_str()) {
                edges.push((&u.id, owner));
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    let mut kept: Vec<(&str, &str)> = Vec::new();
    for (from, to) in edges {
        // Keep the edge unless `to` already waits, directly or not, for `from`.
        let mut seen = BTreeSet::new();
        let mut stack = vec![to];
        let mut cycle = false;
        while let Some(at) = stack.pop() {
            if at == from {
                cycle = true;
                break;
            }
            if seen.insert(at) {
                stack.extend(kept.iter().filter(|(f, _)| *f == at).map(|(_, t)| *t));
            }
        }
        if !cycle {
            kept.push((from, to));
        }
    }
    kept.iter()
        .filter(|(f, _)| *f == unit.id)
        .map(|(_, t)| *t)
        .collect()
}

fn compose(plan: &Plan) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "name: {}", q(&plan.system));
    let units = runnable(plan);
    let runnable_ids: BTreeSet<&str> = units.iter().map(|u| u.id.as_str()).collect();
    let names = |keep: &dyn Fn(&Unit) -> bool| -> Vec<&str> {
        plan.units
            .iter()
            .filter(|u| keep(u))
            .map(|u| u.id.as_str())
            .collect()
    };
    let unbuilt = names(&|u| !u.implemented);
    if !unbuilt.is_empty() {
        let _ = writeln!(
            out,
            "# Not in this stack, because they are not implemented and have no image: {}.",
            unbuilt.join(", ")
        );
    }
    let unstarted = names(&|u| u.implemented && !runnable_ids.contains(u.id.as_str()));
    if !unstarted.is_empty() {
        let _ = writeln!(
            out,
            "# Not in this stack, because the deployment profile does not say how to start them: {}.",
            unstarted.join(", ")
        );
    }
    out.push_str("services:\n");
    let mut used = BTreeSet::new();
    let mut volumes = BTreeSet::new();
    for unit in &units {
        let networks = networks_of(plan, unit, &runnable_ids);
        // Ports published for this unit and for every unit that shares its namespace.
        let publish: Vec<&String> = plan
            .units
            .iter()
            .filter(|u| {
                u.id == unit.id || u.shares_network_with.as_deref() == Some(unit.id.as_str())
            })
            .flat_map(|u| u.publish.iter())
            .collect();
        let _ = writeln!(out, "  {}:", unit.id);
        let _ = writeln!(out, "    image: {}", q(&unit.image));
        let _ = writeln!(
            out,
            "    container_name: {}",
            q(&format!("{}-{}", plan.system, unit.id))
        );
        if let Some(containerfile) = &plan.containerfile {
            let _ = writeln!(out, "    build:");
            let _ = writeln!(out, "      context: {}", q(&plan.context));
            let _ = writeln!(out, "      dockerfile: {}", q(containerfile));
            let _ = writeln!(out, "      target: {}", q(&unit.target));
        }
        if unit.shares_network_with.is_none() {
            let _ = writeln!(out, "    expose:\n      - \"{}\"", unit.port);
        }
        if !publish.is_empty() {
            out.push_str("    ports:\n");
            for p in &publish {
                let _ = writeln!(out, "      - {}", q(p));
            }
        }
        if !unit.env.is_empty() {
            out.push_str("    environment:\n");
            for (k, v) in &unit.env {
                let _ = writeln!(out, "      {k}: {}", q(v));
            }
        }
        if !unit.volumes.is_empty() {
            out.push_str("    volumes:\n");
            for v in &unit.volumes {
                let _ = writeln!(out, "      - {}", q(v));
                let source = v.split(':').next().unwrap_or("");
                if !source.is_empty() && !source.starts_with(['/', '.', '~']) {
                    volumes.insert(source.to_string());
                }
            }
        }
        if let Some(path) = &unit.health {
            let url = format!("http://127.0.0.1:{}{path}", unit.port);
            let _ = writeln!(
                out,
                "    healthcheck:\n      test: [\"CMD\", \"wget\", \"-q\", \"-O\", \"/dev/null\", {}]",
                q(&url)
            );
            out.push_str(
                "      interval: 5s\n      timeout: 3s\n      retries: 24\n      start_period: 5s\n",
            );
        }
        let waits = dependencies(plan, unit, &runnable_ids);
        if !waits.is_empty() {
            out.push_str("    depends_on:\n");
            for id in waits {
                let healthy = plan.units.iter().any(|u| u.id == id && u.health.is_some());
                let condition = if healthy {
                    "service_healthy"
                } else {
                    "service_started"
                };
                let _ = writeln!(out, "      {id}:\n        condition: {condition}");
            }
        }
        let _ = writeln!(out, "    labels:");
        let _ = writeln!(out, "      strata.trust-zone: {}", q(&unit.zone));
        if let Some(owner) = &unit.shares_network_with {
            let _ = writeln!(out, "    network_mode: {}", q(&format!("service:{owner}")));
        } else if !networks.is_empty() {
            out.push_str("    networks:\n");
            for n in &networks {
                let _ = writeln!(out, "      - {n}");
            }
        } else if publish.is_empty() {
            out.push_str("    network_mode: none\n");
        }
        used.extend(networks);
    }
    if used.is_empty() {
        out.push_str("networks: {}\n");
    } else {
        out.push_str("networks:\n");
        for n in &used {
            let _ = writeln!(out, "  {n}: {{}}");
        }
    }
    if !volumes.is_empty() {
        out.push_str("volumes:\n");
        for v in &volumes {
            let _ = writeln!(out, "  {v}: {{}}");
        }
    }
    out
}

/// The zones with at least one unit, in order of first use.
fn zones(plan: &Plan) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for unit in &plan.units {
        if !out.contains(&unit.zone.as_str()) {
            out.push(&unit.zone);
        }
    }
    out
}

fn namespaces(plan: &Plan) -> String {
    let mut out = String::new();
    let zones = zones(plan);
    if zones.is_empty() {
        out.push_str("# no unit has a trust zone\n");
    }
    for (i, zone) in zones.iter().enumerate() {
        if i > 0 {
            out.push_str("---\n");
        }
        let _ = writeln!(out, "apiVersion: v1");
        let _ = writeln!(out, "kind: Namespace");
        let _ = writeln!(out, "metadata:");
        let _ = writeln!(out, "  name: {}", namespace(zone));
        let _ = writeln!(out, "  labels:");
        let _ = writeln!(out, "    strata.dev/trust-zone: {}", q(zone));
    }
    out
}

fn unit_manifests(plan: &Plan, unit: &Unit) -> String {
    let ns = namespace(&unit.zone);
    let mut out = String::new();
    // Deployment.
    let _ = writeln!(out, "apiVersion: apps/v1\nkind: Deployment\nmetadata:");
    let _ = writeln!(out, "  name: {}\n  namespace: {ns}", unit.id);
    let _ = writeln!(out, "  labels:\n    app.kubernetes.io/name: {}", unit.id);
    let _ = writeln!(out, "    strata.dev/trust-zone: {}", q(&unit.zone));
    if let Some(r) = &unit.reference {
        let _ = writeln!(out, "    strata.dev/component-ref: {}", q(r));
    }
    let _ = writeln!(out, "spec:\n  replicas: 1\n  selector:\n    matchLabels:");
    let _ = writeln!(out, "      app.kubernetes.io/name: {}", unit.id);
    let _ = writeln!(out, "  template:\n    metadata:\n      labels:");
    let _ = writeln!(out, "        app.kubernetes.io/name: {}", unit.id);
    let _ = writeln!(out, "    spec:\n      containers:");
    let _ = writeln!(out, "        - name: {}", unit.id);
    let _ = writeln!(out, "          image: {}", q(&unit.image));
    let _ = writeln!(
        out,
        "          ports:\n            - containerPort: {}",
        unit.port
    );
    // Service.
    out.push_str("---\n");
    let _ = writeln!(out, "apiVersion: v1\nkind: Service\nmetadata:");
    let _ = writeln!(out, "  name: {}\n  namespace: {ns}", unit.id);
    let _ = writeln!(out, "spec:\n  type: ClusterIP\n  selector:");
    let _ = writeln!(out, "    app.kubernetes.io/name: {}", unit.id);
    let _ = writeln!(
        out,
        "  ports:\n    - port: {0}\n      targetPort: {0}",
        unit.port
    );
    // NetworkPolicy.
    out.push_str("---\n");
    let _ = writeln!(
        out,
        "apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:"
    );
    let _ = writeln!(out, "  name: {}-ingress\n  namespace: {ns}", unit.id);
    let _ = writeln!(out, "spec:\n  podSelector:\n    matchLabels:");
    let _ = writeln!(out, "      app.kubernetes.io/name: {}", unit.id);
    out.push_str("  policyTypes:\n    - Ingress\n");
    let sources: Vec<&String> = plan
        .links
        .iter()
        .filter(|(_, to)| *to == unit.id)
        .map(|(from, _)| from)
        .collect();
    if sources.is_empty() {
        out.push_str("  ingress: []\n");
    } else {
        out.push_str("  ingress:\n");
        for from in sources {
            let _ = writeln!(out, "    - from:");
            let _ = writeln!(out, "        - namespaceSelector:");
            let _ = writeln!(out, "            matchLabels:");
            let _ = writeln!(
                out,
                "              kubernetes.io/metadata.name: {}",
                namespace(zone_of(plan, from))
            );
            let _ = writeln!(out, "          podSelector:\n            matchLabels:");
            let _ = writeln!(out, "              app.kubernetes.io/name: {from}");
            let _ = writeln!(
                out,
                "      ports:\n        - protocol: TCP\n          port: {}",
                unit.port
            );
        }
    }
    out
}

// ---- alerts -------------------------------------------------------------------------------

/// Lower-case snake form of free text: runs of other characters become one underscore.
fn snake(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out.truncate(48);
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// The placeholder metric name: `strata_<boundary>_<detail>`, or `strata_<ref>` without a
/// boundary.
fn metric_base(req: &BudgetRequirement) -> String {
    match (&req.boundary, &req.detail) {
        (Some(b), Some(d)) => format!("strata_{}_{}", snake(b), snake(d)),
        (Some(b), None) => format!("strata_{}_{}", snake(b), snake(&req.reference)),
        _ => format!("strata_{}", snake(&req.reference)),
    }
}

/// A number as PromQL, without float noise.
fn num(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// The alert expression and, when the statement was read, a description of the threshold.
fn expression(req: &BudgetRequirement) -> (String, Option<String>) {
    let base = metric_base(req);
    match &req.budget {
        Budget::Latency { quantile, seconds } => (
            format!(
                "histogram_quantile({}, sum by (le) (rate({base}_duration_seconds_bucket[5m]))) > {}",
                num(*quantile),
                num(*seconds)
            ),
            Some(format!("p{} latency above {} s", num(quantile * 100.0), num(*seconds))),
        ),
        Budget::Ratio { violation, fraction } => (
            format!("{base}_ratio {} {}", violation.operator(), num(*fraction)),
            Some(format!("ratio {} {}", violation.operator(), num(*fraction))),
        ),
        Budget::Duration { violation, seconds } => (
            format!("{base}_seconds {} {}", violation.operator(), num(*seconds)),
            Some(format!("seconds {} {}", violation.operator(), num(*seconds))),
        ),
        Budget::Count { violation, value } => (
            format!("{base} {} {}", violation.operator(), num(*value)),
            Some(format!("value {} {}", violation.operator(), num(*value))),
        ),
        Budget::Unparsed => (base, None),
    }
}

fn alerts(plan: &Plan) -> String {
    let mut out = String::from("groups:\n");
    if plan.budgets.is_empty() {
        return "groups: []\n".to_string();
    }
    out.push_str("  - name: strata-budgets\n    rules:\n");
    for req in &plan.budgets {
        let (expr, threshold) = expression(req);
        let _ = writeln!(
            out,
            "      # PLACEHOLDER metric {}: derived from the boundary detail, not yet an instrument that exists.",
            metric_base(req)
        );
        if threshold.is_none() {
            let _ = writeln!(
                out,
                "      # The statement's target was not read: this expression has no threshold and never fires."
            );
        }
        let _ = writeln!(
            out,
            "      - alert: Budget_{}",
            snake(&req.reference).to_ascii_uppercase()
        );
        let _ = writeln!(out, "        expr: {}", q(&expr));
        let _ = writeln!(out, "        for: 5m");
        let _ = writeln!(out, "        labels:");
        let _ = writeln!(out, "          requirement: {}", q(&req.reference));
        if let Some(b) = &req.boundary {
            let _ = writeln!(out, "          boundary: {}", q(b));
        }
        let _ = writeln!(out, "          severity: warning");
        let _ = writeln!(out, "        annotations:");
        let _ = writeln!(out, "          summary: {}", q(&req.statement));
        let _ = writeln!(out, "          metric_placeholder: \"true\"");
        if let Some(t) = threshold {
            let _ = writeln!(out, "          threshold: {}", q(&t));
        } else {
            let _ = writeln!(out, "          threshold: \"unparsed\"");
        }
    }
    out
}
