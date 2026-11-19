//! Policies (`policies` section) to the `policies` module.
//!
//! Every contract that policies govern, or that states a `policyDefault`, gets
//! `authorize_<contract>(caller, request[, record]) -> Decision`, which is exactly Exegra's rule:
//! a matching deny refuses; otherwise a matching allow permits; otherwise the contract's
//! `policyDefault` decides.
//!
//! A policy matches when the caller is of its principal kind (and has its role, when it names
//! one) and its condition is true. A condition that overflows does not match as an allow and
//! matches as a deny, so a failure never grants. A policy with a `resource` reads the record the
//! caller passes: without a record an allow does not match and a deny does. Only contract
//! policies are generated; a policy on a transition is refused until the machine backend can
//! name the event types its condition reads.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;

use super::view::{self, ExprScope};
use super::Backend;
use crate::constructs::{Policy, Verdict};
use crate::diagnostic::Diagnostic;
use crate::ir::{Contract, Spec};
use crate::naming;
use crate::render::{self, Namespace};

/// Code of a policy that does not name exactly one of `contract` and `transition`.
pub const TARGET: &str = "specc.policy-target";
/// Code of a policy on a transition.
pub const TRANSITION_UNSUPPORTED: &str = "specc.policy-transition-unsupported";
/// Code of a policy on a contract that is not declared.
pub const CONTRACT_UNKNOWN: &str = "specc.policy-contract-unknown";
/// Code of a policy whose `resource` is not a record.
pub const RESOURCE_UNKNOWN: &str = "specc.policy-resource-unknown";
/// Code of a contract whose policies name different resources.
pub const RESOURCE_AMBIGUOUS: &str = "specc.policy-resource-ambiguous";
/// Code of a condition that does not translate.
pub const EXPRESSION: &str = "specc.policy-expression";
/// Code of a contract that policies govern without a `policyDefault`.
pub const DEFAULT_MISSING: &str = "ready.policy-default-missing";

/// The policies backend.
pub struct PoliciesBackend;

/// The policies of a contract, in declaration order.
fn policies_of<'a>(spec: &'a Spec, contract: &str) -> Vec<&'a Policy> {
    spec.constructs
        .policies
        .iter()
        .filter(|p| p.contract.as_deref() == Some(contract))
        .collect()
}

/// The record the policies of a contract read, when they name one.
fn resource_of(policies: &[&Policy]) -> Option<String> {
    policies.iter().find_map(|p| p.resource.clone())
}

/// The contracts that get an `authorize` function.
fn governed(spec: &Spec) -> Vec<&Contract> {
    spec.contracts
        .iter()
        .filter(|c| {
            spec.constructs.policy_default(&c.id).is_some() || !policies_of(spec, &c.id).is_empty()
        })
        .collect()
}

fn scope_for(spec: &Spec, contract: &Contract, resource: Option<&str>) -> ExprScope {
    let mut scope = ExprScope::new();
    scope.principal(&quote! { caller });
    scope.head(
        "request",
        view::contract_entries(spec, &contract.inputs, &quote! { request }),
    );
    if let Some(record) = resource.and_then(|r| spec.record(r)) {
        scope.head(
            "record",
            view::record_entries(spec, record, &quote! { record }),
        );
    }
    scope
}

impl Backend for PoliciesBackend {
    fn module(&self) -> &'static str {
        "policies"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module policies");
        for p in &spec.constructs.policies {
            let what = format!("policy \"{}\"", p.id);
            if let Some(name) = render::checked_name(&mut out, &p.id, &what, naming::snake(&p.id)) {
                names.claim(&mut out, &p.id, &what, &format!("policy_{name}"));
            }
            match (&p.contract, &p.transition) {
                (Some(_), None) => {}
                (None, Some(t)) => out.push(Diagnostic::new(
                    TRANSITION_UNSUPPORTED,
                    &p.id,
                    format!("Policy \"{}\" governs transition \"{t}\"; policies on transitions are not generated yet.", p.id),
                )),
                _ => out.push(Diagnostic::new(
                    TARGET,
                    &p.id,
                    format!("Policy \"{}\" must name exactly one of \"contract\" and \"transition\".", p.id),
                )),
            }
            if let Some(r) = &p.resource {
                if spec.record(r).is_none() {
                    out.push(Diagnostic::new(
                        RESOURCE_UNKNOWN,
                        &p.id,
                        format!(
                            "The resource \"{r}\" of policy \"{}\" is not a record.",
                            p.id
                        ),
                    ));
                }
            }
            let Some(contract_id) = &p.contract else {
                continue;
            };
            let Some(contract) = spec.contracts.iter().find(|c| &c.id == contract_id) else {
                out.push(Diagnostic::new(
                    CONTRACT_UNKNOWN,
                    &p.id,
                    format!(
                        "Policy \"{}\" governs \"{contract_id}\", which is not a contract.",
                        p.id
                    ),
                ));
                continue;
            };
            if let Some(condition) = &p.condition {
                let scope = scope_for(spec, contract, p.resource.as_deref());
                if let Err(why) = view::boolean(&scope, condition) {
                    out.push(Diagnostic::new(
                        EXPRESSION,
                        &p.id,
                        format!(
                            "The condition of policy \"{}\" cannot be generated: {why}.",
                            p.id
                        ),
                    ));
                }
            }
        }
        for c in governed(spec) {
            let what = format!("contract \"{}\"", c.id);
            if let Some(name) = render::checked_name(&mut out, &c.id, &what, naming::snake(&c.id)) {
                names.claim(&mut out, &c.id, &what, &format!("authorize_{name}"));
            }
            let policies = policies_of(spec, &c.id);
            let resources: BTreeSet<&str> = policies
                .iter()
                .filter_map(|p| p.resource.as_deref())
                .collect();
            if resources.len() > 1 {
                out.push(Diagnostic::new(
                    RESOURCE_AMBIGUOUS,
                    &c.id,
                    format!(
                        "The policies of contract \"{}\" read different records ({}); one authorize function takes one record.",
                        c.id,
                        resources.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                ));
            }
            if !policies.is_empty() && spec.constructs.policy_default(&c.id).is_none() {
                out.push(Diagnostic::new(
                    DEFAULT_MISSING,
                    &c.id,
                    format!(
                        "Contract \"{}\" is governed by policies but does not say what they decide when none matches (\"policyDefault\").",
                        c.id
                    ),
                ));
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let contracts = governed(spec);
        let imports = if contracts.is_empty() {
            TokenStream::new()
        } else {
            quote! { use crate::support::{Caller, Decision}; }
        };
        let items: Vec<TokenStream> = contracts.iter().map(|c| contract_items(spec, c)).collect();
        quote! {
            #![doc = " The authorization policies of the specification: one function per governed contract that decides whether a caller may make a request."]
            #imports
            #(#items)*
        }
    }
}

/// The function of one policy, and the call that runs it.
struct Compiled {
    function: TokenStream,
    /// The arguments, after `caller` and `request`, the function takes.
    has_record: bool,
    ident: proc_macro2::Ident,
}

fn compile_policy(spec: &Spec, contract: &Contract, p: &Policy) -> Compiled {
    let name = naming::ident(&format!(
        "policy_{}",
        naming::snake(&p.id).unwrap_or_default()
    ));
    let request_ty = view::request_path(&contract.id);
    let scope = scope_for(spec, contract, p.resource.as_deref());
    let condition = p
        .condition
        .as_deref()
        .and_then(|c| view::boolean(&scope, c).ok());
    let used = condition
        .as_ref()
        .map(|t| t.heads.clone())
        .unwrap_or_default();
    let request = if used.contains("request") {
        quote! { request }
    } else {
        quote! { _request }
    };
    let record_param = p.resource.as_deref().map(|r| {
        let ty = view::record_path(r);
        let ident = if used.contains("record") {
            quote! { record }
        } else {
            quote! { _record }
        };
        quote! { , #ident: &#ty }
    });
    let mut checks = Vec::new();
    if let Some(kind) = &p.principal {
        checks.push(quote! {
            if caller.kind != #kind {
                return Ok(false);
            }
        });
    }
    if let Some(role) = &p.role {
        checks.push(quote! {
            if caller.role.as_deref() != Some(#role) {
                return Ok(false);
            }
        });
    }
    let result = match condition {
        Some(t) => {
            let tokens = t.tokens;
            quote! { Ok(#tokens) }
        }
        None => quote! { Ok(true) },
    };
    let mut doc = format!(
        " Policy `{}`: {} for ",
        p.id,
        match p.effect {
            Some(Verdict::Deny) => "deny",
            _ => "allow",
        }
    );
    doc.push_str(&match (&p.principal, &p.role) {
        (Some(k), Some(r)) => format!("a `{k}` with role `{r}`"),
        (Some(k), None) => format!("a `{k}`"),
        _ => "any principal".to_string(),
    });
    doc.push('.');
    let function = quote! {
        #[doc = #doc]
        fn #name(caller: &Caller, #request: &#request_ty #record_param) -> Result<bool, crate::support::Overflow> {
            #(#checks)*
            #result
        }
    };
    Compiled {
        function,
        has_record: p.resource.is_some(),
        ident: name,
    }
}

fn contract_items(spec: &Spec, c: &Contract) -> TokenStream {
    let policies = policies_of(spec, &c.id);
    let compiled: Vec<(&Policy, Compiled)> = policies
        .iter()
        .map(|p| (*p, compile_policy(spec, c, p)))
        .collect();
    let resource = resource_of(&policies);
    let function = naming::ident(&format!(
        "authorize_{}",
        naming::snake(&c.id).unwrap_or_default()
    ));
    let request_ty = view::request_path(&c.id);
    let caller = if compiled.is_empty() {
        quote! { _caller }
    } else {
        quote! { caller }
    };
    let request = if compiled.is_empty() {
        quote! { _request }
    } else {
        quote! { request }
    };
    let record_param = resource.as_deref().map(|r| {
        let ty = view::record_path(r);
        quote! { , record: Option<&#ty> }
    });

    let call = |p: &Compiled| {
        let ident = &p.ident;
        if p.has_record {
            quote! { record.map(|record| #ident(caller, request, record)) }
        } else {
            quote! { #ident(caller, request) }
        }
    };
    let mut denies = Vec::new();
    let mut allows = Vec::new();
    for (p, compiled) in &compiled {
        let result = call(compiled);
        let (permit, refuse) = if compiled.has_record {
            (quote! { Some(Ok(true)) }, quote! { Some(Ok(false)) })
        } else {
            (quote! { Ok(true) }, quote! { Ok(false) })
        };
        match p.effect {
            Some(Verdict::Deny) => denies.push(quote! {
                if !matches!(#result, #refuse) {
                    return Decision::Deny;
                }
            }),
            _ => allows.push(quote! {
                if matches!(#result, #permit) {
                    return Decision::Allow;
                }
            }),
        }
    }
    let default = match spec.constructs.policy_default(&c.id) {
        Some(Verdict::Allow) => quote! { Decision::Allow },
        _ => quote! { Decision::Deny },
    };
    let default_text = match spec.constructs.policy_default(&c.id) {
        Some(Verdict::Allow) => "allow",
        _ => "deny",
    };
    let mut doc = render::Doc::new(&format!(
        "Whether `caller` may make `request` to contract `{}`: {}.",
        c.id, c.name
    ));
    doc.paragraph("A matching deny policy refuses; otherwise a matching allow policy permits.");
    doc.paragraph(&format!(
        "Otherwise the contract's policy default decides: {default_text}."
    ));
    if resource.is_some() {
        doc.paragraph("A policy that reads the record does not match an allow, and does match a deny, when no record is given.");
    }
    let doc = doc.tokens();
    let functions = compiled.iter().map(|(_, c)| &c.function);
    quote! {
        #(#functions)*

        #doc
        pub fn #function(#caller: &Caller, #request: &#request_ty #record_param) -> Decision {
            #(#denies)*
            #(#allows)*
            #default
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    fn spec(extra: &str) -> Spec {
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S" }},
                "records": [ {{ "id": "rec", "name": "R", "owner": "svc", "key": ["id"], "schemaVersion": 1,
                    "fields": [ {{ "name": "id", "type": "u32" }}, {{ "name": "owner", "type": "string" }} ] }} ],
                "contracts": [ {{ "id": "do-it", "name": "Do it", "realisation": "generated", "policyDefault": "deny",
                    "inputs": [ {{ "name": "count", "type": "u32" }} ] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    fn emitted(spec: &Spec) -> String {
        PoliciesBackend.emit(spec).to_string()
    }

    #[test]
    fn a_governed_contract_gets_an_authorize_function() {
        let s = spec(
            r#""policies": [
              { "id": "ops-allow", "subject": { "principal": "operator" }, "contract": "do-it", "effect": "allow", "condition": "request.count < 10" },
              { "id": "ban", "subject": { "principal": "member", "role": "guest" }, "contract": "do-it", "effect": "deny" } ]"#,
        );
        assert!(PoliciesBackend.check(&s).is_empty());
        let text = emitted(&s);
        assert!(text.contains("pub fn authorize_do_it"));
        assert!(text.contains("fn policy_ops_allow"));
        assert!(text.contains("Decision :: Deny"));
    }

    #[test]
    fn policies_without_a_default_are_refused() {
        let mut s = spec(
            r#""policies": [ { "id": "p", "subject": { "principal": "member" }, "contract": "do-it", "effect": "allow" } ]"#,
        );
        s.constructs.policy_defaults.clear();
        let codes: Vec<String> = PoliciesBackend
            .check(&s)
            .into_iter()
            .map(|d| d.code)
            .collect();
        assert_eq!(codes, [DEFAULT_MISSING]);
    }

    #[test]
    fn what_cannot_be_generated_is_refused() {
        let s = spec(
            r#""policies": [
              { "id": "t", "subject": { "principal": "member" }, "transition": "go", "effect": "allow" },
              { "id": "u", "subject": { "principal": "member" }, "contract": "nope", "effect": "allow" },
              { "id": "bad", "subject": { "principal": "member" }, "contract": "do-it", "effect": "allow", "condition": "request.missing == 1" },
              { "id": "r1", "subject": { "principal": "member" }, "contract": "do-it", "resource": "rec", "effect": "allow" },
              { "id": "r2", "subject": { "principal": "member" }, "contract": "do-it", "resource": "ghost", "effect": "allow" } ]"#,
        );
        let codes: Vec<String> = PoliciesBackend
            .check(&s)
            .into_iter()
            .map(|d| d.code)
            .collect();
        for code in [
            TRANSITION_UNSUPPORTED,
            CONTRACT_UNKNOWN,
            EXPRESSION,
            RESOURCE_UNKNOWN,
        ] {
            assert!(codes.iter().any(|c| c == code), "{code} in {codes:?}");
        }
    }
}
