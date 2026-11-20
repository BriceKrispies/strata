//! The cross-cutting constructs of a specification: policies, validations, retentions, quotas,
//! retries and audits, as typed values, and their loading.
//!
//! Each construct keeps what the specification wrote; the backends interpret it and refuse what
//! they cannot express. Properties that the readiness checks require stay optional here so that
//! loading never fails on them. A value of the wrong JSON type, or a keyword outside its set, is a
//! `specc.spec-shape` diagnostic.

use serde_json::{Number, Value};

use crate::load::{Loader, Obj};

/// What a contract's policies decide when none of them matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The request is permitted.
    Allow,
    /// The request is refused.
    Deny,
}

/// The constructs of a specification, each list in declaration order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constructs {
    /// The `policyDefault` of every contract that states one, by contract id.
    pub policy_defaults: Vec<(String, Verdict)>,
    /// The `policies` section.
    pub policies: Vec<Policy>,
    /// The `validations` section.
    pub validations: Vec<Validation>,
    /// The `retentions` section.
    pub retentions: Vec<Retention>,
    /// The `quotas` section.
    pub quotas: Vec<Quota>,
    /// The `retries` section.
    pub retries: Vec<Retry>,
    /// The `audits` section.
    pub audits: Vec<Audit>,
    /// The capability model: capabilities, plans, roles, grants, token scopes, delegations and
    /// feature flags.
    pub access: crate::ir::AccessModel,
}

impl Constructs {
    /// The `policyDefault` of a contract.
    pub fn policy_default(&self, contract: &str) -> Option<Verdict> {
        self.policy_defaults
            .iter()
            .find(|(id, _)| id == contract)
            .map(|(_, v)| *v)
    }
}

/// A `policy` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// The policy id.
    pub id: String,
    /// The principal kind the policy is about (`subject.principal`).
    pub principal: Option<String>,
    /// The role the principal must have (`subject.role`).
    pub role: Option<String>,
    /// The governed contract.
    pub contract: Option<String>,
    /// The governed transition.
    pub transition: Option<String>,
    /// The record the condition reads as `record.<field>`.
    pub resource: Option<String>,
    /// The condition, in the guard language.
    pub condition: Option<String>,
    /// What a matching policy decides.
    pub effect: Option<Verdict>,
}

/// The rule of a validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Present, and not empty when a text, bytes or a list.
    Required,
    /// A length limit.
    MaxLength,
    /// A regular expression the whole value matches.
    Pattern,
    /// A numeric range.
    Range,
    /// A set of allowed values.
    OneOf,
    /// A boolean expression.
    Custom,
}

/// What a length limit counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Unicode scalar values.
    Characters,
    /// UTF-8 bytes.
    Bytes,
}

/// A `validation` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    /// The validation id.
    pub id: String,
    /// The contract whose input is validated.
    pub contract: Option<String>,
    /// The validated contract input.
    pub input: Option<String>,
    /// The record whose field is validated.
    pub record: Option<String>,
    /// The validated record field.
    pub field: Option<String>,
    /// The rule.
    pub rule: Rule,
    /// The limit of `maxLength`.
    pub max_length: Option<Number>,
    /// What `maxLength` counts.
    pub unit: Option<Unit>,
    /// The regular expression of `pattern`.
    pub pattern: Option<String>,
    /// The least value of `range`.
    pub min: Option<Number>,
    /// The greatest value of `range`.
    pub max: Option<Number>,
    /// The allowed values of `oneOf`.
    pub values: Vec<Value>,
    /// The expression of `custom`.
    pub expr: Option<String>,
    /// What a refusal says.
    pub message: Option<String>,
}

/// What a retention does when the keeping ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// Delete the record.
    Delete,
    /// Clear the listed fields.
    Anonymise,
    /// Move the record to another component.
    Archive,
}

/// A `retention` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Retention {
    /// The retention id.
    pub id: String,
    /// The record it applies to.
    pub record: Option<String>,
    /// How long a record is kept, as written.
    pub keep_for: Option<String>,
    /// The event that ends the keeping.
    pub until: Option<String>,
    /// What happens then.
    pub then: Then,
    /// The fields an anonymisation clears.
    pub fields: Vec<String>,
    /// The component an archive goes to.
    pub archive_to: Option<String>,
}

/// Whom a quota counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaScope {
    /// Per tenant.
    Tenant,
    /// Per principal.
    Principal,
    /// Per source address.
    SourceAddress,
}

/// What happens to an excess call or event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnExceed {
    /// It is refused.
    Refuse,
    /// It waits.
    Queue,
    /// It is slowed down.
    Throttle,
}

/// A `quota` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    /// The quota id.
    pub id: String,
    /// Whom it counts.
    pub scope: QuotaScope,
    /// The limited contract.
    pub contract: Option<String>,
    /// The limited event.
    pub event: Option<String>,
    /// The most calls or events per window, as written.
    pub limit: Option<Number>,
    /// The window, as written.
    pub window: Option<String>,
    /// What happens to an excess.
    pub on_exceed: OnExceed,
    /// The contract error a refusal is.
    pub error: Option<String>,
    /// The component that keeps the counters.
    pub held_by: Option<String>,
}

/// How the wait between attempts grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffKind {
    /// Every wait is the base.
    Fixed,
    /// Each wait doubles, up to the cap.
    Exponential,
}

/// A `retry` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Retry {
    /// The retry id.
    pub id: String,
    /// The repeated contract.
    pub contract: Option<String>,
    /// The repeated event.
    pub event: Option<String>,
    /// The most attempts, the first included, as written.
    pub attempts: Option<Number>,
    /// How the wait grows.
    pub backoff: Option<BackoffKind>,
    /// The first wait, as written.
    pub base: Option<String>,
    /// The longest wait, as written.
    pub cap: Option<String>,
    /// The input or payload field that identifies a repeat.
    pub idempotency_key: Option<String>,
    /// How long one attempt may take, as written.
    pub timeout: Option<String>,
}

/// How an audit log keeps its entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Immutability {
    /// Entries are only added.
    AppendOnly,
    /// Entries are added, each holding the hash of the one before.
    ChainedHash,
}

/// One captured field of an audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// The record field written.
    pub field: String,
    /// The expression that gives its value.
    pub from: String,
}

/// An `audit` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Audit {
    /// The audit id.
    pub id: String,
    /// The audited contract.
    pub contract: Option<String>,
    /// The audited transition.
    pub transition: Option<String>,
    /// The audited event.
    pub event: Option<String>,
    /// The record written.
    pub record: Option<String>,
    /// The captured fields, in order.
    pub capture: Vec<Capture>,
    /// How entries are kept.
    pub immutability: Immutability,
    /// The field that holds the previous entry's hash.
    pub chain_field: Option<String>,
}

const VERDICTS: &[(&str, Verdict)] = &[("allow", Verdict::Allow), ("deny", Verdict::Deny)];

impl Loader {
    /// Loads the six construct sections and the contracts' `policyDefault`.
    pub(crate) fn constructs(&mut self, top: &Obj) -> Constructs {
        let mut out = Constructs::default();
        // The contracts section was already read, and its shape problems reported, by the
        // contract loader; only the well-formed entries are looked at here.
        if let Some(Value::Array(items)) = top.get("contracts") {
            for item in items {
                let (Some(obj), Some(id)) =
                    (item.as_object(), item.get("id").and_then(Value::as_str))
                else {
                    continue;
                };
                if let Some(v) = self.keyword(obj, "policyDefault", id, VERDICTS) {
                    out.policy_defaults.push((id.to_string(), v));
                }
            }
        }
        for (id, o) in self.section(top, "policies") {
            let (principal, role) = match o.get("subject") {
                Some(Value::Object(s)) => (
                    self.req_str(s, "principal", &id),
                    self.opt_str(s, "role", &id),
                ),
                _ => {
                    self.shape(&id, "\"subject\" must be an object with a \"principal\"");
                    (None, None)
                }
            };
            let effect = self.required_keyword(o, "effect", &id, VERDICTS);
            out.policies.push(Policy {
                contract: self.opt_str(o, "contract", &id),
                transition: self.opt_str(o, "transition", &id),
                resource: self.opt_str(o, "resource", &id),
                condition: self.opt_str(o, "condition", &id),
                id,
                principal,
                role,
                effect,
            });
        }
        for (id, o) in self.section(top, "validations") {
            let Some(rule) = self.required_keyword(
                o,
                "rule",
                &id,
                &[
                    ("required", Rule::Required),
                    ("maxLength", Rule::MaxLength),
                    ("pattern", Rule::Pattern),
                    ("range", Rule::Range),
                    ("oneOf", Rule::OneOf),
                    ("custom", Rule::Custom),
                ],
            ) else {
                continue;
            };
            let values = match o.get("values") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items.clone(),
                Some(_) => {
                    self.shape(&id, "\"values\" must be a list");
                    Vec::new()
                }
            };
            out.validations.push(Validation {
                contract: self.opt_str(o, "contract", &id),
                input: self.opt_str(o, "input", &id),
                record: self.opt_str(o, "record", &id),
                field: self.opt_str(o, "field", &id),
                max_length: self.opt_number(o, "maxLength", &id),
                unit: self.keyword(
                    o,
                    "unit",
                    &id,
                    &[("characters", Unit::Characters), ("bytes", Unit::Bytes)],
                ),
                pattern: self.opt_str(o, "pattern", &id),
                min: self.opt_number(o, "min", &id),
                max: self.opt_number(o, "max", &id),
                expr: self.opt_str(o, "expr", &id),
                message: self.req_str(o, "message", &id),
                id,
                rule,
                values,
            });
        }
        for (id, o) in self.section(top, "retentions") {
            let Some(then) = self.required_keyword(
                o,
                "then",
                &id,
                &[
                    ("delete", Then::Delete),
                    ("anonymise", Then::Anonymise),
                    ("archive", Then::Archive),
                ],
            ) else {
                continue;
            };
            out.retentions.push(Retention {
                record: self.opt_str(o, "record", &id),
                keep_for: self.opt_str(o, "keepFor", &id),
                until: self.opt_str(o, "until", &id),
                fields: self.opt_str_list(o, "fields", &id).unwrap_or_default(),
                archive_to: self.opt_str(o, "archiveTo", &id),
                id,
                then,
            });
        }
        for (id, o) in self.section(top, "quotas") {
            let scope = self.required_keyword(
                o,
                "scope",
                &id,
                &[
                    ("tenant", QuotaScope::Tenant),
                    ("principal", QuotaScope::Principal),
                    ("source-address", QuotaScope::SourceAddress),
                ],
            );
            let on_exceed = self.required_keyword(
                o,
                "onExceed",
                &id,
                &[
                    ("refuse", OnExceed::Refuse),
                    ("queue", OnExceed::Queue),
                    ("throttle", OnExceed::Throttle),
                ],
            );
            let (Some(scope), Some(on_exceed)) = (scope, on_exceed) else {
                continue;
            };
            out.quotas.push(Quota {
                contract: self.opt_str(o, "contract", &id),
                event: self.opt_str(o, "event", &id),
                limit: self.opt_number(o, "limit", &id),
                window: self.opt_str(o, "window", &id),
                error: self.opt_str(o, "error", &id),
                held_by: self.opt_str(o, "heldBy", &id),
                id,
                scope,
                on_exceed,
            });
        }
        for (id, o) in self.section(top, "retries") {
            let (mut backoff, mut base, mut cap) = (None, None, None);
            match o.get("backoff") {
                Some(Value::Object(b)) => {
                    backoff = self.required_keyword(
                        b,
                        "kind",
                        &id,
                        &[
                            ("fixed", BackoffKind::Fixed),
                            ("exponential", BackoffKind::Exponential),
                        ],
                    );
                    base = self.req_str(b, "base", &id);
                    cap = self.opt_str(b, "cap", &id);
                }
                _ => self.shape(&id, "\"backoff\" must be an object"),
            }
            out.retries.push(Retry {
                contract: self.opt_str(o, "contract", &id),
                event: self.opt_str(o, "event", &id),
                attempts: self.opt_number(o, "attempts", &id),
                idempotency_key: self.opt_str(o, "idempotencyKey", &id),
                timeout: self.opt_str(o, "timeout", &id),
                id,
                backoff,
                base,
                cap,
            });
        }
        for (id, o) in self.section(top, "audits") {
            let Some(immutability) = self.required_keyword(
                o,
                "immutability",
                &id,
                &[
                    ("append-only", Immutability::AppendOnly),
                    ("chained-hash", Immutability::ChainedHash),
                ],
            ) else {
                continue;
            };
            let mut capture = Vec::new();
            for c in self.obj_list(o, "capture", &id) {
                let field = self.req_str(c, "field", &id);
                let from = self.req_str(c, "from", &id);
                if let (Some(field), Some(from)) = (field, from) {
                    capture.push(Capture { field, from });
                }
            }
            out.audits.push(Audit {
                contract: self.opt_str(o, "contract", &id),
                transition: self.opt_str(o, "transition", &id),
                event: self.opt_str(o, "event", &id),
                record: self.opt_str(o, "record", &id),
                chain_field: self.opt_str(o, "chainField", &id),
                id,
                capture,
                immutability,
            });
        }
        out
    }

    /// An optional property whose text is one of `table`'s keywords.
    fn keyword<T: Copy>(
        &mut self,
        obj: &Obj,
        key: &str,
        entity: &str,
        table: &[(&str, T)],
    ) -> Option<T> {
        let text = self.opt_str(obj, key, entity)?;
        let found = table.iter().find(|(k, _)| *k == text).map(|(_, v)| *v);
        if found.is_none() {
            let allowed: Vec<&str> = table.iter().map(|(k, _)| *k).collect();
            self.shape(
                entity,
                format!("\"{key}\" is \"{text}\"; it must be {}", allowed.join(", ")),
            );
        }
        found
    }

    /// A required property whose text is one of `table`'s keywords.
    fn required_keyword<T: Copy>(
        &mut self,
        obj: &Obj,
        key: &str,
        entity: &str,
        table: &[(&str, T)],
    ) -> Option<T> {
        if obj.get(key).is_none() {
            self.shape(entity, format!("\"{key}\" is required"));
            return None;
        }
        self.keyword(obj, key, entity, table)
    }
}

#[cfg(test)]
mod tests {
    use crate::load::load;

    use super::*;

    const SPEC: &str = r#"{ "system": { "id": "s", "name": "S" },
        "contracts": [ { "id": "c", "name": "C", "policyDefault": "deny" } ],
        "policies": [ { "id": "p", "subject": { "principal": "member", "role": "admin" }, "contract": "c", "effect": "allow" } ],
        "validations": [ { "id": "v", "contract": "c", "input": "x", "rule": "maxLength", "maxLength": 3, "unit": "bytes", "message": "m" } ],
        "retentions": [ { "id": "t", "record": "r", "keepFor": "1 d", "then": "anonymise", "fields": ["a"] } ],
        "quotas": [ { "id": "q", "scope": "tenant", "contract": "c", "limit": 5, "window": "1 min", "onExceed": "refuse", "error": "e" } ],
        "retries": [ { "id": "y", "contract": "c", "attempts": 3, "backoff": { "kind": "exponential", "base": "1 s", "cap": "9 s" } } ],
        "audits": [ { "id": "a", "contract": "c", "record": "r", "capture": [ { "field": "f", "from": "time" } ], "immutability": "append-only" } ] }"#;

    #[test]
    fn every_construct_loads() {
        let spec = load("s", SPEC).unwrap();
        let c = &spec.constructs;
        assert_eq!(c.policy_default("c"), Some(Verdict::Deny));
        assert_eq!(c.policies[0].role.as_deref(), Some("admin"));
        assert_eq!(c.validations[0].unit, Some(Unit::Bytes));
        assert_eq!(c.retentions[0].fields, ["a"]);
        assert_eq!(c.quotas[0].on_exceed, OnExceed::Refuse);
        assert_eq!(c.retries[0].backoff, Some(BackoffKind::Exponential));
        assert_eq!(c.audits[0].capture[0].from, "time");
    }

    #[test]
    fn unknown_keywords_are_shape_diagnostics() {
        let text = r#"{ "system": { "id": "s", "name": "S" },
            "policies": [ { "id": "p", "subject": { "principal": "m" }, "contract": "c", "effect": "maybe" } ] }"#;
        let err = load("s", text).unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.entity == "p" && d.code == crate::load::SHAPE));
    }
}
