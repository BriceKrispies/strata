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

use std::collections::BTreeSet;
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
    let spec_path = spec_path.replace('\\', "/");
    let plan = load(text)?;
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
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            SYNTAX,
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
    if !diagnostics.is_empty() {
        return Err(crate::diagnostic::dedup(diagnostics));
    }
    Ok(Plan {
        system,
        units,
        links: links.into_iter().collect(),
        budgets,
    })
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
/// part in, as source or target.
fn networks_of(plan: &Plan, unit: &Unit) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (from, to) in &plan.links {
        if *from == unit.id || *to == unit.id {
            out.insert(zone_network(zone_of(plan, to)));
        }
    }
    out
}

fn compose(plan: &Plan) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "name: {}", q(&plan.system));
    out.push_str("services:\n");
    let mut used = BTreeSet::new();
    for unit in &plan.units {
        let networks = networks_of(plan, unit);
        let _ = writeln!(out, "  {}:", unit.id);
        let _ = writeln!(out, "    image: {}", q(&unit.image));
        let _ = writeln!(out, "    expose:\n      - \"{PORT}\"");
        let _ = writeln!(out, "    labels:");
        let _ = writeln!(out, "      strata.trust-zone: {}", q(&unit.zone));
        if networks.is_empty() {
            out.push_str("    network_mode: none\n");
        } else {
            out.push_str("    networks:\n");
            for n in &networks {
                let _ = writeln!(out, "      - {n}");
            }
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
    let _ = writeln!(out, "          ports:\n            - containerPort: {PORT}");
    // Service.
    out.push_str("---\n");
    let _ = writeln!(out, "apiVersion: v1\nkind: Service\nmetadata:");
    let _ = writeln!(out, "  name: {}\n  namespace: {ns}", unit.id);
    let _ = writeln!(out, "spec:\n  type: ClusterIP\n  selector:");
    let _ = writeln!(out, "    app.kubernetes.io/name: {}", unit.id);
    let _ = writeln!(
        out,
        "  ports:\n    - port: {PORT}\n      targetPort: {PORT}"
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
                "      ports:\n        - protocol: TCP\n          port: {PORT}"
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
