//! Refusals of the contract rules of the audit wave (WG-5038), read from the specification text:
//!
//! - `contract.step-up-missing`, `contract.step-up-unknown`: a contract named in the list of
//!   operations that need a fresh authentication (decision D-6201, claim IDN-1202) states no
//!   `auth.stepUp`, or no `step-up-required` error of kind `forbidden`, or does not exist.
//! - `contract.tenant-mismatch-missing`: a public contract with an input of type `tenant-id` has no
//!   `tenant-mismatch` error (claim TEN-1140).
//! - `contract.convention-concurrency`, `-pagination`, `-idempotency`, `-error-kind`: a public
//!   contract breaks the contract conventions of decision D-6595 (claim ARC-1599), unless the
//!   checked-in list `contract_conventions_exempt.json` names it for that rule. An entry of that
//!   list that no longer matches a violation is refused as `contract.convention-exemption-stale`,
//!   so the list can only shrink.
//!
//! Each rule is in force once the specification holds the entity that states it: the decision
//! `d-6201`, the claim `TEN-1140`, the claim `ARC-1599`. A specification without it is not
//! refused, so the rules can land before the operations that add those entities are applied.
//!
//! CMS-1020 (a bound on a Markdown source) has no rule here: the specification declares no
//! Markdown field. A Markdown field is a field of a tenant record type in a product file, and the
//! bound is the parser's (`strata_substrate::markdown::parse`), so specc has nothing to refuse.

use serde_json::{Map, Value};

use crate::diagnostic::Diagnostic;

type Obj = Map<String, Value>;

/// Code of a contract in the step-up list without a step-up age or without the error.
pub const STEP_UP_MISSING: &str = "contract.step-up-missing";
/// Code of a contract the step-up list names that the specification does not have.
pub const STEP_UP_UNKNOWN: &str = "contract.step-up-unknown";
/// Code of a public contract with a `tenant-id` input and no `tenant-mismatch` error.
pub const TENANT_MISMATCH_MISSING: &str = "contract.tenant-mismatch-missing";
/// Code of a public contract that names its optimistic-concurrency input in a retired way.
pub const CONVENTION_CONCURRENCY: &str = "contract.convention-concurrency";
/// Code of a public contract with a pagination shape other than `cursor`, `limit` and `next`.
pub const CONVENTION_PAGINATION: &str = "contract.convention-pagination";
/// Code of a public contract with an idempotency input other than `idempotency-key`.
pub const CONVENTION_IDEMPOTENCY: &str = "contract.convention-idempotency";
/// Code of a public contract with an error outside the platform vocabulary.
pub const CONVENTION_ERROR_KIND: &str = "contract.convention-error-kind";
/// Code of an exemption that names a contract or rule that no longer breaks the convention.
pub const CONVENTION_EXEMPTION_STALE: &str = "contract.convention-exemption-stale";

/// The decision that holds the list of operations that need a fresh authentication.
const STEP_UP_DECISION: &str = "d-6201";
/// The error a contract of that list declares.
const STEP_UP_ERROR: &str = "step-up-required";
/// The claim that states the tenant-mismatch rule.
const TENANT_CLAIM: &str = "TEN-1140";
/// The error a public contract with a tenant input declares.
const TENANT_MISMATCH_ERROR: &str = "tenant-mismatch";
/// The claim that states the contract conventions.
const CONVENTIONS_CLAIM: &str = "ARC-1599";

/// The names an optimistic-concurrency input had before D-6595 chose `expected-version`.
const RETIRED_CONCURRENCY: &[&str] = &[
    "based-on-version",
    "expected-revision",
    "base-version",
    "base-revision",
    "if-match",
    "if-version",
    "expected-etag",
    "etag",
    "previous-version",
];
/// Names of a pagination input that D-6595 replaced by `cursor` and `limit`.
const RETIRED_PAGINATION_INPUTS: &[&str] = &["after", "offset", "page", "page-size", "per-page"];
/// Names of a pagination output that D-6595 replaced by `next`.
const RETIRED_PAGINATION_OUTPUTS: &[&str] = &["next-cursor", "next-page"];
/// Names an idempotency input could have, other than `idempotency-key`.
const RETIRED_IDEMPOTENCY: &[&str] = &[
    "idempotency-token",
    "idempotency_key",
    "idempotency",
    "request-id",
    "client-request-id",
    "dedupe-key",
    "dedup-key",
];
/// The error of an idempotency key reused with other input.
const IDEMPOTENCY_ERROR: &str = "idempotency-mismatch";
/// The error kinds of the platform vocabulary (D-6595).
const PLATFORM_KINDS: &[&str] = &[
    "unauthenticated",
    "forbidden",
    "not_found",
    "invalid",
    "conflict",
    "throttled",
    "unavailable",
    "internal",
];
/// Error names D-6595 retires: each becomes `invalid` or `forbidden`.
const RETIRED_ERROR_NAMES: &[&str] = &["rejected", "too-many", "field-not-readable"];

/// The rules of the conventions, as they are named in the exemption list.
pub const CONVENTION_RULES: &[&str] = &["concurrency", "pagination", "idempotency", "error-kind"];

/// A contract exempt from one convention rule, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exemption {
    /// The contract id.
    pub contract: String,
    /// One of [`CONVENTION_RULES`].
    pub rule: String,
    /// Why the contract is exempt, naming the task that removes the entry.
    pub reason: String,
}

/// The exemptions checked in beside this file.
pub fn exemptions() -> Result<Vec<Exemption>, String> {
    parse_exemptions(include_str!("contract_conventions_exempt.json"))
}

/// Reads an exemption list: `{ "exemptions": [ { "contract", "rule", "reason" } ] }`.
pub fn parse_exemptions(text: &str) -> Result<Vec<Exemption>, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = value
        .get("exemptions")
        .and_then(Value::as_array)
        .ok_or("the list has no \"exemptions\" array")?;
    let mut out = Vec::new();
    for (i, entry) in list.iter().enumerate() {
        let field = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .ok_or(format!("exemption {i} has no \"{key}\""))
        };
        let exemption = Exemption {
            contract: field("contract")?,
            rule: field("rule")?,
            reason: field("reason")?,
        };
        if !CONVENTION_RULES.contains(&exemption.rule.as_str()) {
            return Err(format!(
                "exemption {i} names the rule \"{}\"; the rules are {}",
                exemption.rule,
                CONVENTION_RULES.join(", ")
            ));
        }
        out.push(exemption);
    }
    Ok(out)
}

/// Every refusal of these rules in the specification `top`, with the checked-in exemptions.
pub(crate) fn check(top: &Obj) -> Vec<Diagnostic> {
    match exemptions() {
        Ok(list) => check_with(top, &list),
        Err(why) => vec![Diagnostic::new(
            CONVENTION_EXEMPTION_STALE,
            "contract_conventions_exempt.json",
            format!("the exemption list of the contract conventions cannot be read: {why}"),
        )],
    }
}

/// Like [`check`], with the exemptions given.
pub(crate) fn check_with(top: &Obj, exempt: &[Exemption]) -> Vec<Diagnostic> {
    let contracts: Vec<&Obj> = top
        .get("contracts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect();
    let mut out = Vec::new();
    if let Some(decision) = entity(top, "decisions", |o| {
        text(o, "id") == Some(STEP_UP_DECISION)
    }) {
        out.extend(step_up(&contracts, decision));
    }
    if has_claim(top, TENANT_CLAIM) {
        out.extend(tenant_mismatch(&contracts));
    }
    if has_claim(top, CONVENTIONS_CLAIM) {
        out.extend(conventions(&contracts, exempt));
    }
    out
}

fn text<'a>(o: &'a Obj, key: &str) -> Option<&'a str> {
    o.get(key).and_then(Value::as_str)
}

fn entity<'a>(top: &'a Obj, section: &str, pred: impl Fn(&Obj) -> bool) -> Option<&'a Obj> {
    top.get(section)
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|o| pred(o))
}

fn has_claim(top: &Obj, reference: &str) -> bool {
    entity(top, "claims", |o| text(o, "ref") == Some(reference)).is_some()
}

/// The names of the `inputs` or `outputs` of a contract.
fn field_names<'a>(contract: &'a Obj, key: &str) -> Vec<&'a str> {
    contract
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("name").and_then(Value::as_str))
        .collect()
}

fn errors(contract: &Obj) -> Vec<&Obj> {
    contract
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .collect()
}

fn has_error(contract: &Obj, name: &str) -> bool {
    errors(contract)
        .iter()
        .any(|e| text(e, "name") == Some(name))
}

fn is_public(contract: &Obj) -> bool {
    text(contract, "exposure") == Some("public")
}

fn id_of(contract: &Obj) -> &str {
    text(contract, "id").unwrap_or("")
}

/// The contract references (`CT-055`, `H-033`) a decision statement lists as operations, in the
/// order written. A reference followed by `..` opens a range of in-process operations the decision
/// says are not on the list, and is left out.
pub fn listed_references(statement: &str) -> Vec<String> {
    let bytes = statement.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let prefix = if boundary && statement[i..].starts_with("CT-") {
            Some(3)
        } else if boundary && statement[i..].starts_with("H-") {
            Some(2)
        } else {
            None
        };
        let Some(prefix) = prefix else {
            i += 1;
            continue;
        };
        let digits = bytes[i + prefix..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            i += prefix;
            continue;
        }
        let end = i + prefix + digits;
        if !statement[end..].starts_with("..") {
            out.push(statement[i..end].to_string());
        }
        i = end;
    }
    out
}

fn step_up(contracts: &[&Obj], decision: &Obj) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let statement = text(decision, "statement").unwrap_or("");
    for reference in listed_references(statement) {
        let Some(contract) = contracts
            .iter()
            .find(|c| text(c, "ref") == Some(reference.as_str()))
        else {
            out.push(Diagnostic::new(
                STEP_UP_UNKNOWN,
                &reference,
                format!(
                    "Decision D-6201 lists {reference} among the operations that need a fresh authentication, and the specification has no contract with that reference."
                ),
            ));
            continue;
        };
        let age = contract
            .get("auth")
            .and_then(Value::as_object)
            .and_then(|a| text(a, "stepUp"))
            .is_some_and(|s| !s.trim().is_empty());
        let error = errors(contract).iter().any(|e| {
            text(e, "name") == Some(STEP_UP_ERROR) && text(e, "kind") == Some("forbidden")
        });
        if age && error {
            continue;
        }
        let lacks = match (age, error) {
            (false, false) => "a step-up age in its authorisation (auth.stepUp) and a step-up-required error of kind forbidden",
            (false, true) => "a step-up age in its authorisation (auth.stepUp)",
            _ => "a step-up-required error of kind forbidden",
        };
        out.push(Diagnostic::new(
            STEP_UP_MISSING,
            id_of(contract),
            format!(
                "Contract \"{}\" ({reference}) is on the list of operations that need a fresh authentication (D-6201, IDN-1202) and states no {lacks}.",
                id_of(contract)
            ),
        ));
    }
    out
}

fn tenant_mismatch(contracts: &[&Obj]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for contract in contracts.iter().filter(|c| is_public(c)) {
        let tenant_inputs: Vec<&str> = contract
            .get("inputs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter(|f| text(f, "type") == Some("tenant-id"))
            .filter_map(|f| text(f, "name"))
            .collect();
        if tenant_inputs.is_empty() || has_error(contract, TENANT_MISMATCH_ERROR) {
            continue;
        }
        out.push(Diagnostic::new(
            TENANT_MISMATCH_MISSING,
            id_of(contract),
            format!(
                "Public contract \"{}\" takes the tenant-id input {} and declares no tenant-mismatch error; the gateway refuses a request whose tenant input differs from the tenant of the host and session (TEN-1140).",
                id_of(contract),
                tenant_inputs.join(", ")
            ),
        ));
    }
    out
}

/// What one public contract breaks, as (rule, message) pairs.
fn broken_conventions(contract: &Obj) -> Vec<(&'static str, String)> {
    let inputs = field_names(contract, "inputs");
    let outputs = field_names(contract, "outputs");
    let mut out = Vec::new();

    let retired: Vec<&str> = inputs
        .iter()
        .copied()
        .filter(|n| RETIRED_CONCURRENCY.contains(n))
        .collect();
    if !retired.is_empty() {
        out.push((
            "concurrency",
            format!(
                "names its optimistic-concurrency input {}; the input is expected-version (schema-version for a schema)",
                retired.join(", ")
            ),
        ));
    }

    let has_in = |n: &str| inputs.contains(&n);
    let has_out = |n: &str| outputs.contains(&n);
    let retired_in: Vec<&str> = inputs
        .iter()
        .copied()
        .filter(|n| RETIRED_PAGINATION_INPUTS.contains(n))
        .collect();
    let retired_out: Vec<&str> = outputs
        .iter()
        .copied()
        .filter(|n| RETIRED_PAGINATION_OUTPUTS.contains(n))
        .collect();
    let paginated = has_in("cursor")
        || has_out("next")
        || (has_in("after") && has_in("limit"))
        || retired_in.iter().any(|n| *n != "after")
        || !retired_out.is_empty();
    if paginated {
        let mut problems = Vec::new();
        if !has_in("cursor") {
            problems.push("no cursor input".to_string());
        }
        if !has_in("limit") {
            problems.push("no limit input".to_string());
        }
        if !has_out("next") {
            problems.push("no next output".to_string());
        }
        for name in retired_in.iter().chain(retired_out.iter()) {
            problems.push(format!("the retired name {name}"));
        }
        if !problems.is_empty() {
            out.push((
                "pagination",
                format!(
                    "is paginated and has {}; the shape is the inputs cursor and limit and the output next",
                    problems.join(", ")
                ),
            ));
        }
    }

    let aliases: Vec<&str> = inputs
        .iter()
        .copied()
        .filter(|n| RETIRED_IDEMPOTENCY.contains(n))
        .collect();
    if !aliases.is_empty() {
        out.push((
            "idempotency",
            format!(
                "names its idempotency input {}; the input is idempotency-key",
                aliases.join(", ")
            ),
        ));
    } else if has_error(contract, IDEMPOTENCY_ERROR) && !has_in("idempotency-key") {
        out.push((
            "idempotency",
            "declares an idempotency-mismatch error and takes no idempotency-key input".to_string(),
        ));
    }

    let mut kinds = Vec::new();
    for error in errors(contract) {
        let name = text(error, "name").unwrap_or("");
        match text(error, "kind") {
            None => kinds.push(format!("error {name} states no kind")),
            Some(kind) if !PLATFORM_KINDS.contains(&kind) => {
                kinds.push(format!(
                    "error {name} has the kind {kind}, which is not in the platform vocabulary"
                ));
            }
            Some(_) => {}
        }
        if RETIRED_ERROR_NAMES.contains(&name) {
            kinds.push(format!(
                "error {name} is retired; it becomes invalid or forbidden"
            ));
        }
    }
    if !kinds.is_empty() {
        out.push(("error-kind", kinds.join("; ")));
    }
    out
}

fn conventions(contracts: &[&Obj], exempt: &[Exemption]) -> Vec<Diagnostic> {
    let code_of = |rule: &str| match rule {
        "concurrency" => CONVENTION_CONCURRENCY,
        "pagination" => CONVENTION_PAGINATION,
        "idempotency" => CONVENTION_IDEMPOTENCY,
        _ => CONVENTION_ERROR_KIND,
    };
    let mut out = Vec::new();
    let mut used = vec![false; exempt.len()];
    for contract in contracts.iter().filter(|c| is_public(c)) {
        let id = id_of(contract);
        for (rule, message) in broken_conventions(contract) {
            let hit = exempt
                .iter()
                .position(|e| e.contract == id && e.rule == rule);
            if let Some(i) = hit {
                used[i] = true;
                continue;
            }
            out.push(Diagnostic::new(
                code_of(rule),
                id,
                format!("Public contract \"{id}\" {message} (D-6595, ARC-1599)."),
            ));
        }
    }
    for (e, used) in exempt.iter().zip(used) {
        if !used {
            out.push(Diagnostic::new(
                CONVENTION_EXEMPTION_STALE,
                &e.contract,
                format!(
                    "The exemption of \"{}\" from the {} convention is stale: no public contract with that id breaks it. Remove the entry from contract_conventions_exempt.json.",
                    e.contract, e.rule
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn top(extra: Value) -> Obj {
        let mut base = json!({ "contracts": [], "claims": [], "decisions": [] });
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        base.as_object().unwrap().clone()
    }

    fn codes(diagnostics: &[Diagnostic]) -> Vec<(String, String)> {
        diagnostics
            .iter()
            .map(|d| (d.code.clone(), d.entity.clone()))
            .collect()
    }

    fn pair(code: &str, entity: &str) -> (String, String) {
        (code.to_string(), entity.to_string())
    }

    fn convention_claim() -> Value {
        json!([{ "id": "arc-1599", "ref": "ARC-1599" }])
    }

    fn public(id: &str, inputs: Value, outputs: Value) -> Value {
        json!({ "id": id, "exposure": "public", "inputs": inputs, "outputs": outputs, "errors": [] })
    }

    #[test]
    fn the_embedded_exemption_list_is_well_formed() {
        let list = exemptions().expect("the list parses");
        for e in &list {
            assert!(CONVENTION_RULES.contains(&e.rule.as_str()), "{e:?}");
        }
        let mut keys: Vec<_> = list.iter().map(|e| (&e.contract, &e.rule)).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "an exemption is listed twice");
    }

    #[test]
    fn a_malformed_exemption_list_is_an_error() {
        assert!(parse_exemptions("{}").is_err());
        assert!(parse_exemptions(
            r#"{"exemptions":[{"contract":"a","rule":"nope","reason":"r"}]}"#
        )
        .is_err());
        assert!(
            parse_exemptions(r#"{"exemptions":[{"contract":"a","rule":"pagination"}]}"#).is_err()
        );
    }

    #[test]
    fn references_of_the_step_up_list_skip_ranges() {
        let statement = "passkeys (CT-055, H-033), a realm (CT-065) and the key service (CT-151..155) but not XCT-9 or CT-";
        assert_eq!(listed_references(statement), ["CT-055", "H-033", "CT-065"]);
    }

    #[test]
    fn nothing_is_refused_before_the_entities_that_state_the_rules() {
        let t = top(json!({ "contracts": [
            public("a", json!([{ "name": "tenant", "type": "tenant-id" }, { "name": "based-on-version" }]), json!([]))
        ] }));
        assert!(check_with(&t, &[]).is_empty());
    }

    #[test]
    fn a_contract_on_the_step_up_list_needs_an_age_and_the_error() {
        let ok_error = json!([{ "name": "step-up-required", "kind": "forbidden" }]);
        let t = top(json!({
            "decisions": [{ "id": "d-6201", "statement": "list: CT-001, CT-002, CT-003, CT-004, CT-005" }],
            "contracts": [
                { "id": "full", "ref": "CT-001", "auth": { "stepUp": "15 min" }, "errors": ok_error },
                { "id": "no-age", "ref": "CT-002", "auth": {}, "errors": ok_error },
                { "id": "no-error", "ref": "CT-003", "auth": { "stepUp": "15 min" }, "errors": [] },
                { "id": "wrong-kind", "ref": "CT-004", "auth": { "stepUp": "15 min" },
                  "errors": [{ "name": "step-up-required", "kind": "invalid" }] }
            ]
        }));
        assert_eq!(
            codes(&check_with(&t, &[])),
            [
                pair(STEP_UP_MISSING, "no-age"),
                pair(STEP_UP_MISSING, "no-error"),
                pair(STEP_UP_MISSING, "wrong-kind"),
                pair(STEP_UP_UNKNOWN, "CT-005"),
            ]
        );
    }

    #[test]
    fn a_public_tenant_input_needs_the_tenant_mismatch_error() {
        let t = top(json!({
            "claims": [{ "id": "ten-1140", "ref": "TEN-1140" }],
            "contracts": [
                public("bare", json!([{ "name": "tenant", "type": "tenant-id" }]), json!([])),
                { "id": "guarded", "exposure": "public", "inputs": [{ "name": "tenant", "type": "tenant-id" }],
                  "errors": [{ "name": "tenant-mismatch", "kind": "forbidden" }] },
                { "id": "internal", "exposure": "internal", "inputs": [{ "name": "tenant", "type": "tenant-id" }] },
                public("no-tenant", json!([{ "name": "host", "type": "host" }]), json!([]))
            ]
        }));
        assert_eq!(
            codes(&check_with(&t, &[])),
            [pair(TENANT_MISMATCH_MISSING, "bare")]
        );
    }

    #[test]
    fn a_public_contract_that_breaks_a_convention_is_refused_unless_exempt() {
        let t = top(json!({ "claims": convention_claim(), "contracts": [
            public("old-concurrency", json!([{ "name": "expected-revision" }]), json!([])),
            public("good-concurrency", json!([{ "name": "expected-version" }]), json!([])),
            public("old-page", json!([{ "name": "after" }, { "name": "limit" }]), json!([{ "name": "next" }])),
            public("short-page", json!([{ "name": "cursor" }]), json!([{ "name": "next" }])),
            public("good-page", json!([{ "name": "cursor" }, { "name": "limit" }]), json!([{ "name": "next" }])),
            public("stream", json!([{ "name": "after" }]), json!([{ "name": "stream" }])),
            public("alias", json!([{ "name": "request-id" }]), json!([])),
            { "id": "mismatch", "exposure": "public", "inputs": [],
              "errors": [{ "name": "idempotency-mismatch", "kind": "conflict" }] },
            { "id": "kinds", "exposure": "public", "inputs": [],
              "errors": [{ "name": "rejected", "kind": "invalid" }, { "name": "odd", "kind": "teapot" }, { "name": "bare" }] },
            { "id": "local-one", "exposure": "local", "inputs": [{ "name": "based-on-version" }] }
        ] }));
        assert_eq!(
            codes(&check_with(&t, &[])),
            [
                pair(CONVENTION_CONCURRENCY, "old-concurrency"),
                pair(CONVENTION_PAGINATION, "old-page"),
                pair(CONVENTION_PAGINATION, "short-page"),
                pair(CONVENTION_IDEMPOTENCY, "alias"),
                pair(CONVENTION_IDEMPOTENCY, "mismatch"),
                pair(CONVENTION_ERROR_KIND, "kinds"),
            ]
        );
        let exempt = |contract: &str, rule: &str| Exemption {
            contract: contract.to_string(),
            rule: rule.to_string(),
            reason: "test".to_string(),
        };
        let list = [
            exempt("old-concurrency", "concurrency"),
            exempt("old-page", "pagination"),
            exempt("short-page", "pagination"),
            exempt("alias", "idempotency"),
            exempt("mismatch", "idempotency"),
            exempt("kinds", "error-kind"),
        ];
        assert!(check_with(&t, &list).is_empty());
    }

    #[test]
    fn an_exemption_that_matches_no_violation_is_stale() {
        let t = top(json!({ "claims": convention_claim(), "contracts": [
            public("fixed", json!([{ "name": "expected-version" }]), json!([]))
        ] }));
        let list = [
            Exemption {
                contract: "fixed".into(),
                rule: "concurrency".into(),
                reason: "r".into(),
            },
            Exemption {
                contract: "gone".into(),
                rule: "pagination".into(),
                reason: "r".into(),
            },
        ];
        assert_eq!(
            codes(&check_with(&t, &list)),
            [
                pair(CONVENTION_EXEMPTION_STALE, "fixed"),
                pair(CONVENTION_EXEMPTION_STALE, "gone"),
            ]
        );
    }
}
