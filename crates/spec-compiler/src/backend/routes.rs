//! Contracts on the wire (the `routes` module): one `strata_runtime::Contract` per contract a
//! component implements, the route of each contract on its own, and per implementing component the
//! routes of all its contracts and a typed client.
//!
//! For every contract with an `implementedBy` component (a facade has none, and is never served):
//!
//! - a unit type named after the contract implements `strata_runtime::Contract`, with
//!   `SPEC = ContractSpec { id, path, access, timeout, idempotent }`: `id` is the contract's `ref`
//!   (its id when it has none), `access` its exposure and structured auth as a
//!   `strata_runtime::Access` (scheme, roles, tenant policies, CSRF, origin, step-up, secret,
//!   tenant scope, development condition), `timeout` its `timeoutMs`
//!   ([`DEFAULT_TIMEOUT_MS`] when it states none) and `idempotent` its `idempotent` (false when it
//!   states none). Its error type implements `strata_runtime::ContractError`: each error travels
//!   with its status, which is its own `status`, else the status of its `kind`
//!   (`system.generation.errorKinds`), else the default `errorKind` or `errorStatus`. An error
//!   with no status is refused with `ready.error-status-missing`; nothing defaults to 422 unless
//!   the specification says so.
//! - **The error body.** Without `system.generation.errorBody` the contract's `Body` is the
//!   runtime's own (`strata_runtime::RuntimeBody`). With it, the module has `ErrorBody`, which
//!   answers every failure with the stated struct type: an error whose payload is that type is
//!   answered with the payload, its code set to the error's kind; any other error with a body of
//!   its kind whose message is the error's name; a runtime failure with a body of the kind
//!   `errorBody.runtime` gives it, at that kind's status. The correlation field, when the body
//!   names one, is set to the caller's correlation id. A client reads a body back as the contract
//!   error of its kind when exactly one error of the contract has that kind (and no payload or the
//!   body as payload), and otherwise as a runtime failure. Each error then needs a kind
//!   (`ready.error-kind-missing`).
//! - `<Contract>Handler`, the trait a service implements to serve this contract alone (one
//!   method, `handle`); every implementation of the component's trait implements it.
//! - `<Contract>Hooks`, what the route needs from the deployment beyond the handler: the record a
//!   policy with a `resource` reads (`<contract>_resource`), the payload of a refusal error that
//!   carries one (`<quota>_exceeded`) and the audit log (`append_<audit>`), with the shared
//!   [`AUDIT_CLOCK`], [`QUOTA_COUNTER`] and [`QUOTA_WAIT`] traits as supertraits where the
//!   contract has audits, quotas or a queueing or throttling quota. A hooks trait with no method
//!   of its own is implemented for every type that has its supertraits, `()` included when it
//!   has none.
//! - `serve_<contract>(routes, handler, hooks)`, which adds the contract's route to a
//!   `strata_runtime::Routes`. Per call, after the runtime's authorizer and body decoding: the
//!   request's `validate()` when the contract has validations (a broken rule answers
//!   `invalid_request`); the contract's policies through `policies::authorize_<contract>` with the
//!   caller the authorizer established (a deny answers `forbidden`); every quota on the contract
//!   through `count` (a refusing quota answers with its contract error); the handler; on success,
//!   every audit of the contract appended through `append_<audit>`. A failing hook answers
//!   `unavailable` before the call and `internal` after it.
//! - **The path rule.** The contract is served at `binding.path` when the contract states one,
//!   and otherwise at `/<implementedBy>/<contract id>`: the implementing component is the
//!   boundary the contract is served at. A path is `/` followed by segments of ASCII letters,
//!   digits, `-`, `_`, `.` and `~`, separated by single `/`, without a trailing `/`; anything else
//!   (such as a path parameter) is refused with [`PATH_INVALID`], because every contract is a
//!   `POST` of a JSON body. A `binding.method` other than `POST` is refused with
//!   [`METHOD_UNSUPPORTED`], and two contracts at one path with [`PATH_DUPLICATE`].
//!
//! For every implementing component `<C>`, built from the pieces above:
//!
//! - `<C>Hooks`, every hooks trait of its contracts at once, implemented for every type that
//!   implements them all;
//! - `<c>_routes(routes, service, hooks)`, which serves every contract of the component with an
//!   implementation of the component's trait;
//! - `<C>Client`, a typed client with one method per contract over `strata_runtime::Client`;
//!   a contract with a retry policy is called under it.
//!
//! **Exposure and auth.** A served contract states its `exposure` and its `auth` as
//! `{scheme, ...}`; one that does not is refused (`contract.exposure`,
//! `contract.auth-structured`), as is a sentence for an auth: nothing an authorizer would have to
//! read as words is generated. The in-process scheme is exactly the scheme of local contracts, and
//! a signature or signed address names its secret.
//!
//! **The authorizer.** `SpecAuthorizer<H>` is the `strata_runtime::Authorizer` generated from
//! the schemes: one arm per scheme, with no fallback, so a scheme the generator does not know
//! does not compile, and nothing is admitted that the contract's `Access` does not state. A local
//! contract is refused on any route. Per scheme it establishes the principal: anonymous nothing,
//! session through the hole `AuthHoles::session` (the runtime cannot know the deployment's session
//! store), service by comparing the service credential in constant time, a provisioning token or
//! a delegation (the bearer token) through their holes, a signature or signed address through the
//! hole that verifies it with the named secret. Then, generated: the development condition, the
//! origin of a browser call against the origins of the site it addresses (`AuthHoles::site`), the
//! request token against the session's or, for a visitor, `AuthHoles::visitor_token`, the roles
//! (any one admits), the step-up age, and for tenant scope `request` that a principal's tenant
//! is the site's. Every hole has a default that establishes nothing, so a deployment that fills no
//! hole admits only anonymous calls that ask for no browser check.
//!
//! **Capabilities.** Last, for a contract whose auth `requires` capabilities, the one check
//! `permitted` (generated by [`access_model`]): the tenant's plan entitles each capability or it
//! is core, no flag gating it is off for the tenant (`AuthHoles::entitlement` gives the tenant's
//! plan and flags), the caller holds it (`AuthHoles::holding` gives a principal's platform roles,
//! tenant roles, teams and grants; anonymous callers, services and external parties hold what
//! their platform roles confer), and a delegation the caller acts under passes it on. The module
//! also has the operation catalogue (`OPERATIONS`, `catalogue`), marked per principal with the
//! same check.
//!
//! **The gateway.** `GATEWAY_ROUTES` lists the routes through the gateway: the public contracts
//! and no other. `strata_runtime::Routes::gateway` serves only public contracts, so an internal
//! contract has no route facing the internet.
//!
//! The caller policies and audits read is built from the runtime's `Caller` the authorizer
//! established: its subject is the principal's id, and its attributes `kind`, `tenant` and `role`
//! the rest.

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::{access_model, view, Backend};
use crate::constructs::OnExceed;
use crate::diagnostic::Diagnostic;
use crate::ir::{
    Auth, Component, Contract, ErrorBody, Exposure, Scheme, Spec, TenantScope, TypeKind,
};
use crate::naming;
use crate::render::{self, Doc, Namespace, Scope, Uses};
use crate::resolve::TypeRef;

/// Code of a contract path that cannot be served.
pub const PATH_INVALID: &str = "specc.wiring-path-invalid";
/// Code of two contracts served at the same path.
pub const PATH_DUPLICATE: &str = "specc.wiring-path-duplicate";
/// Code of a `binding.method` other than `POST`.
pub const METHOD_UNSUPPORTED: &str = "specc.wiring-method-unsupported";
/// Code of an error of a served contract with no status (Exegra's readiness code).
pub const ERROR_STATUS_MISSING: &str = "ready.error-status-missing";
/// Code of an error of a served contract with no kind under an error body (Exegra's readiness
/// code).
pub const ERROR_KIND_MISSING: &str = "ready.error-kind-missing";
/// Code of an error kind that `system.generation.errorKinds` does not declare (Exegra's code).
pub const ERROR_KIND_UNKNOWN: &str = "contract.error-kind-unknown";
/// Code of a served contract whose auth is missing, prose or malformed (Exegra's code).
pub const AUTH_STRUCTURED: &str = "contract.auth-structured";
/// Code of a served contract that states no exposure, or whose scheme does not fit it (Exegra's
/// code).
pub const EXPOSURE: &str = "contract.exposure";
/// Code of an error body that cannot be generated: its type, fields or runtime mapping (Exegra's
/// `generation.error-body-*` rules, under one code).
pub const ERROR_BODY_INVALID: &str = "specc.error-body-invalid";

/// The name of the shared hooks trait giving the time audits record.
pub const AUDIT_CLOCK: &str = "AuditClock";
/// The name of the shared hooks trait counting calls against quotas.
pub const QUOTA_COUNTER: &str = "QuotaCounter";
/// The name of the shared hooks trait holding calls a queueing or throttling quota delays.
pub const QUOTA_WAIT: &str = "QuotaWait";

/// The failures the runtime reports itself, by their names in `errorBody.runtime`, with the
/// `strata_runtime::FailureCode` variant of each.
pub const RUNTIME_FAILURES: &[(&str, &str)] = &[
    ("unauthenticated", "Unauthenticated"),
    ("forbidden", "Forbidden"),
    ("invalid-request", "InvalidRequest"),
    ("timeout", "Timeout"),
    ("unavailable", "Unavailable"),
    ("internal", "Internal"),
    (IDEMPOTENCY_CONFLICT, "IdempotencyConflict"),
];

/// The runtime failure of an idempotency key reused with a different request. Unlike the others
/// the specification need not map it in `errorBody.runtime`: it is always answered as the error
/// kind `conflict` unless the mapping says otherwise, so the specification only has to declare that
/// kind (`system.generation.errorKinds`, 409 in Strata).
pub const IDEMPOTENCY_CONFLICT: &str = "idempotency-conflict";

/// The error kind the failures without a mapping in `errorBody.runtime` are answered as.
const IDEMPOTENCY_CONFLICT_KIND: &str = "conflict";

/// The timeout of a contract that states no `timeoutMs`, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// The routes backend.
pub struct RoutesBackend;

/// True when the generated crate depends on `strata-runtime`: it has an implementation-level
/// machine or a contract a component implements.
pub fn uses_runtime(spec: &Spec) -> bool {
    !spec.machines.is_empty()
        || !spec.memory_machines.is_empty()
        || !spec.target_machines.is_empty()
        || !served(spec).is_empty()
}

/// The components that implement at least one contract, in declaration order.
pub fn implementing_components(spec: &Spec) -> Vec<&Component> {
    spec.components
        .iter()
        .filter(|component| !contracts_of(spec, &component.id).is_empty())
        .collect()
}

/// The contracts a component implements, in declaration order.
pub fn contracts_of<'a>(spec: &'a Spec, component: &str) -> Vec<&'a Contract> {
    spec.contracts
        .iter()
        .filter(|c| c.served_by() == Some(component))
        .collect()
}

/// The contracts served by some declared component, in declaration order.
fn served(spec: &Spec) -> Vec<&Contract> {
    spec.contracts
        .iter()
        .filter(|c| c.served_by().is_some_and(|id| spec.component(id).is_some()))
        .collect()
}

/// The routes through the gateway: the served contracts whose exposure is public, in
/// declaration order. No internal or local contract is ever in it.
pub fn gateway_table(spec: &Spec) -> Vec<&Contract> {
    served(spec)
        .into_iter()
        .filter(|c| c.exposure == Some(Exposure::Public))
        .collect()
}

/// The step-up age of a structured auth, in milliseconds, when it states one that is a duration.
fn step_up_ms(auth: &Auth) -> Option<Result<u64, String>> {
    auth.step_up
        .as_deref()
        .map(|text| view::duration_ns(text).map(|ns| ns.div_ceil(1_000_000)))
}

/// The path a served contract is at, by the path rule in the module documentation.
pub fn contract_path(c: &Contract) -> String {
    match &c.binding_path {
        Some(path) => path.clone(),
        None => format!(
            "/{}/{}",
            c.implemented_by.as_deref().unwrap_or_default(),
            c.id
        ),
    }
}

/// True when `path` can be a route: `/` and segments of `[A-Za-z0-9._~-]` joined by `/`.
fn is_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty()
        && rest.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_.~".contains(ch))
        })
}

/// What the route of one contract uses beyond the handler.
struct Needs<'a> {
    validates: bool,
    governed: bool,
    resource: Option<String>,
    quotas: Vec<&'a crate::constructs::Quota>,
    audits: Vec<&'a crate::constructs::Audit>,
    retry: Option<&'a crate::constructs::Retry>,
}

impl Needs<'_> {
    fn principal(&self) -> bool {
        self.governed || !self.quotas.is_empty() || !self.audits.is_empty()
    }

    fn waits(&self) -> bool {
        self.quotas
            .iter()
            .any(|q| matches!(q.on_exceed, OnExceed::Queue | OnExceed::Throttle))
    }
}

fn needs<'a>(spec: &'a Spec, c: &Contract) -> Needs<'a> {
    let k = &spec.constructs;
    let policies: Vec<_> = k
        .policies
        .iter()
        .filter(|p| p.contract.as_deref() == Some(c.id.as_str()))
        .collect();
    Needs {
        validates: k
            .validations
            .iter()
            .any(|v| v.contract.as_deref() == Some(c.id.as_str()) && v.input.is_some()),
        governed: k.policy_default(&c.id).is_some() || !policies.is_empty(),
        resource: policies.iter().find_map(|p| p.resource.clone()),
        quotas: k
            .quotas
            .iter()
            .filter(|q| q.contract.as_deref() == Some(c.id.as_str()))
            .collect(),
        audits: k
            .audits
            .iter()
            .filter(|a| a.contract.as_deref() == Some(c.id.as_str()))
            .collect(),
        retry: k
            .retries
            .iter()
            .find(|r| r.contract.as_deref() == Some(c.id.as_str())),
    }
}

/// The error of contract `c` named `name`, with its payload type when it carries one.
fn error_of<'a>(c: &'a Contract, name: &str) -> Option<&'a crate::ir::ContractError> {
    c.errors.iter().find(|e| e.name == name)
}

fn snake(name: &str) -> String {
    naming::snake(name).unwrap_or_default()
}

fn pascal(name: &str) -> String {
    naming::pascal(name).unwrap_or_default()
}

/// The struct fields of the error body's type, when it is a declared struct.
fn body_fields<'a>(spec: &'a Spec, body: &ErrorBody) -> Option<&'a [crate::ir::StructField]> {
    match &spec.type_decl(&body.type_name)?.kind {
        TypeKind::Struct { fields } => Some(fields),
        _ => None,
    }
}

/// The values of the enum the error body's code field has, when it is one.
fn code_values<'a>(spec: &'a Spec, body: &ErrorBody) -> Option<&'a [String]> {
    let field = body_fields(spec, body)?
        .iter()
        .find(|f| f.name == body.code)?;
    match &spec.type_decl(&field.type_name)?.kind {
        TypeKind::Enum { values } => Some(values),
        _ => None,
    }
}

/// Everything wrong with the error body that would stop the routes from being generated.
fn check_body(spec: &Spec, body: &ErrorBody) -> Vec<String> {
    let mut problems = Vec::new();
    let wire = &spec.system.error_wire;
    let Some(fields) = body_fields(spec, body) else {
        problems.push(format!(
            "its type \"{}\" is not a declared struct",
            body.type_name
        ));
        return problems;
    };
    let roles = [
        ("code", Some(body.code.as_str())),
        ("message", Some(body.message.as_str())),
        ("correlation", body.correlation.as_deref()),
    ];
    for (role, name) in roles {
        let Some(name) = name else { continue };
        match fields.iter().find(|f| f.name == name) {
            None => problems.push(format!(
                "its {role} field \"{name}\" is not a field of \"{}\"",
                body.type_name
            )),
            Some(f) if f.optional => {
                problems.push(format!("its {role} field \"{name}\" is optional"))
            }
            Some(f) if role != "code" && f.type_name != "string" => problems.push(format!(
                "its {role} field \"{name}\" has type \"{}\", not string",
                f.type_name
            )),
            Some(_) => {}
        }
    }
    match code_values(spec, body) {
        None => problems.push(format!(
            "its code field \"{}\" is not of an enum type",
            body.code
        )),
        Some(values) => {
            for (kind, _) in &wire.kinds {
                if !values.contains(kind) {
                    problems.push(format!(
                        "the values of its code field lack the error kind \"{kind}\""
                    ));
                }
            }
        }
    }
    for f in fields {
        let named = f.name == body.code
            || f.name == body.message
            || body.correlation.as_deref() == Some(f.name.as_str());
        if !named && !f.optional {
            problems.push(format!(
                "field \"{}\" of \"{}\" is neither its code, message nor correlation, and is not optional",
                f.name, body.type_name
            ));
        }
    }
    for (failure, _) in RUNTIME_FAILURES {
        match body.runtime.iter().find(|(k, _)| k == failure) {
            None if *failure == IDEMPOTENCY_CONFLICT => {
                if wire.kind_status(IDEMPOTENCY_CONFLICT_KIND).is_none() {
                    problems.push(format!(
                        "it does not declare the error kind \"{IDEMPOTENCY_CONFLICT_KIND}\" an idempotency key reused with another request is answered as"
                    ));
                }
            }
            None => problems.push(format!(
                "its runtime mapping does not name the runtime failure \"{failure}\""
            )),
            Some((_, kind)) if wire.kind_status(kind).is_none() => problems.push(format!(
                "the runtime failure \"{failure}\" maps to \"{kind}\", which is not an error kind"
            )),
            Some(_) => {}
        }
    }
    problems
}

/// What stops the access of served contract `c` from being generated: a missing exposure, an
/// auth that is missing, a sentence or incomplete, a scheme that does not fit the exposure.
fn check_access(c: &Contract) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(exposure) = c.exposure else {
        out.push(Diagnostic::new(
            EXPOSURE,
            &c.id,
            format!(
                "Contract \"{}\" states no exposure (public, internal or local), so nothing says where it may be served.",
                c.id
            ),
        ));
        return out;
    };
    let Some(auth) = &c.auth else {
        let why = match &c.auth_prose {
            Some(text) => format!("states its auth as a sentence (\"{text}\")"),
            None => "states no auth".to_string(),
        };
        out.push(Diagnostic::new(
            AUTH_STRUCTURED,
            &c.id,
            format!(
                "Contract \"{}\" {why}; the authorizer is generated from a structured auth {{scheme, ...}} and admits nothing unstated.",
                c.id
            ),
        ));
        return out;
    };
    if (auth.scheme == Scheme::InProcess) != (exposure == Exposure::Local) {
        out.push(Diagnostic::new(
            EXPOSURE,
            &c.id,
            format!(
                "Contract \"{}\" is {} with the scheme {}; the in-process scheme is exactly the scheme of local contracts.",
                c.id,
                match exposure {
                    Exposure::Public => "public",
                    Exposure::Internal => "internal",
                    Exposure::Local => "local",
                },
                auth.scheme.name()
            ),
        ));
    }
    if matches!(auth.scheme, Scheme::Signature | Scheme::SignedAddress) && auth.secret.is_none() {
        out.push(Diagnostic::new(
            AUTH_STRUCTURED,
            &c.id,
            format!(
                "Contract \"{}\" is verified by {} but names no secret.",
                c.id,
                auth.scheme.name()
            ),
        ));
    }
    if let Some(Err(why)) = step_up_ms(auth) {
        out.push(Diagnostic::new(
            AUTH_STRUCTURED,
            &c.id,
            format!(
                "The step-up age of contract \"{}\" is not a duration: {why}.",
                c.id
            ),
        ));
    }
    out
}

impl Backend for RoutesBackend {
    fn module(&self) -> &'static str {
        "routes"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !served(spec).is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let wire = &spec.system.error_wire;
        let mut module = Namespace::new("module routes");
        for helper in [
            "principal",
            "refusal",
            "call_retry",
            "error_body",
            "runtime_body",
            "runtime_code",
            "authorize_access",
            "GATEWAY_ROUTES",
        ]
        .into_iter()
        .chain(access_model::HELPERS.iter().copied())
        {
            module.claim(&mut out, "", "a helper of the module", helper);
        }
        for fixed in [
            AUDIT_CLOCK,
            QUOTA_COUNTER,
            QUOTA_WAIT,
            "ErrorBody",
            "BodyError",
            "AuthHoles",
            "SpecAuthorizer",
        ]
        .into_iter()
        .chain(access_model::TYPES.iter().copied())
        {
            module.claim_type(&mut out, "", "a trait or type of the module", fixed);
        }
        if let Some(kind) = &wire.default_kind {
            if wire.kind_status(kind).is_none() {
                out.push(Diagnostic::new(
                    ERROR_KIND_UNKNOWN,
                    &spec.system.id,
                    format!("system.generation.errorKind is \"{kind}\", which errorKinds does not declare."),
                ));
            }
        }
        let served = served(spec);
        if let Some(body) = &wire.body {
            if !served.is_empty() {
                for problem in check_body(spec, body) {
                    out.push(Diagnostic::new(
                        ERROR_BODY_INVALID,
                        &spec.system.id,
                        format!(
                            "The error body of system.generation cannot be generated: {problem}."
                        ),
                    ));
                }
            }
        }
        out.extend(access_model::check(spec, &served));
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        for c in &served {
            out.extend(check_access(c));
            let what = format!("contract \"{}\"", c.id);
            if let Ok(name) = naming::pascal(&c.id) {
                module.claim_type(&mut out, &c.id, &format!("the wire type of {what}"), &name);
                module.claim_type(
                    &mut out,
                    &c.id,
                    &format!("the handler of {what}"),
                    &format!("{name}Handler"),
                );
                module.claim_type(
                    &mut out,
                    &c.id,
                    &format!("the hooks of {what}"),
                    &format!("{name}Hooks"),
                );
            }
            if let Ok(name) = naming::snake(&c.id) {
                module.claim(
                    &mut out,
                    &c.id,
                    &format!("the route of {what}"),
                    &format!("serve_{name}"),
                );
                module.claim(
                    &mut out,
                    &c.id,
                    &format!("the answer of {what}"),
                    &format!("answer_{name}"),
                );
            }
            for e in &c.errors {
                if let Some(kind) = &e.kind {
                    if wire.kind_status(kind).is_none() {
                        out.push(Diagnostic::new(
                            ERROR_KIND_UNKNOWN,
                            &c.id,
                            format!(
                                "The error \"{}\" of contract \"{}\" has kind \"{kind}\", which system.generation.errorKinds does not declare.",
                                e.name, c.id
                            ),
                        ));
                        continue;
                    }
                }
                if wire.body.is_some() {
                    if wire.kind_of(e).is_none() {
                        out.push(Diagnostic::new(
                            ERROR_KIND_MISSING,
                            &c.id,
                            format!(
                                "The error \"{}\" of contract \"{}\" states no \"kind\" and system.generation states no errorKind, so the error body cannot say what kind of failure it is.",
                                e.name, c.id
                            ),
                        ));
                    }
                } else if wire.status_of(e).is_none() {
                    out.push(Diagnostic::new(
                        ERROR_STATUS_MISSING,
                        &c.id,
                        format!(
                            "The error \"{}\" of contract \"{}\" states no \"status\" or \"kind\", and system.generation states no errorStatus or errorKind, so nothing says which HTTP status it is answered with.",
                            e.name, c.id
                        ),
                    ));
                }
            }
            if let Some(method) = &c.binding_method {
                if !method.eq_ignore_ascii_case("post") {
                    out.push(Diagnostic::new(
                        METHOD_UNSUPPORTED,
                        &c.id,
                        format!(
                            "Contract \"{}\" is bound to method {method}; contracts are served as POST with a JSON body.",
                            c.id
                        ),
                    ));
                }
            }
            let path = contract_path(c);
            if !is_route(&path) {
                out.push(Diagnostic::new(
                    PATH_INVALID,
                    &c.id,
                    format!(
                        "Contract \"{}\" would be served at \"{path}\", which is not a path of plain segments (/ and letters, digits, -, _, . and ~, without parameters or a trailing /).",
                        c.id
                    ),
                ));
            }
            if let Some(previous) = paths.insert(path.clone(), c.id.clone()) {
                out.push(Diagnostic::new(
                    PATH_DUPLICATE,
                    &c.id,
                    format!(
                        "Contracts \"{previous}\" and \"{}\" would both be served at \"{path}\".",
                        c.id
                    ),
                ));
            }
        }
        for component in implementing_components(spec) {
            let what = format!("component \"{}\"", component.id);
            let (Ok(type_name), Ok(fn_name)) =
                (naming::pascal(&component.id), naming::snake(&component.id))
            else {
                continue;
            };
            module.claim_type(
                &mut out,
                &component.id,
                &format!("the hooks of {what}"),
                &format!("{type_name}Hooks"),
            );
            module.claim_type(
                &mut out,
                &component.id,
                &format!("the client of {what}"),
                &format!("{type_name}Client"),
            );
            module.claim(
                &mut out,
                &component.id,
                &format!("the routes of {what}"),
                &format!("{fn_name}_routes"),
            );
            // One type implements every hooks trait of the component, so their methods share
            // one namespace.
            let mut hooks = Namespace::new(format!("the hooks of {what}"));
            for fixed in ["now", "count", "wait"] {
                hooks.claim(&mut out, &component.id, "a hook", fixed);
            }
            let mut methods = Namespace::new(format!("the client of {what}"));
            for fixed in ["new", "client"] {
                methods.claim(&mut out, &component.id, "a method of the client", fixed);
            }
            for c in contracts_of(spec, &component.id) {
                let n = needs(spec, c);
                let cwhat = format!("contract \"{}\"", c.id);
                if let Ok(name) = naming::snake(&c.id) {
                    methods.claim(&mut out, &c.id, &cwhat, &name);
                    if n.resource.is_some() {
                        hooks.claim(
                            &mut out,
                            &c.id,
                            &format!("the resource of {cwhat}"),
                            &format!("{name}_resource"),
                        );
                    }
                }
                for q in &n.quotas {
                    if let Ok(name) = naming::snake(&q.id) {
                        hooks.claim(
                            &mut out,
                            &q.id,
                            &format!("the refusal of quota \"{}\"", q.id),
                            &format!("{name}_exceeded"),
                        );
                    }
                }
                for a in &n.audits {
                    if let Ok(name) = naming::snake(&a.id) {
                        hooks.claim(
                            &mut out,
                            &a.id,
                            &format!("the log of audit \"{}\"", a.id),
                            &format!("append_{name}"),
                        );
                    }
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let body = spec
            .system
            .error_wire
            .body
            .as_ref()
            .map(|b| BodyNames::new(spec, b, &mut uses));
        let served = served(spec);
        let wire: Vec<TokenStream> = served
            .iter()
            .map(|c| wire_type(spec, c, body.as_ref()))
            .collect();
        let mut used = Used::default();
        let contracts: Vec<TokenStream> = served
            .iter()
            .map(|c| contract_items(spec, c, &mut uses, &mut used))
            .collect();
        let components: Vec<TokenStream> = implementing_components(spec)
            .into_iter()
            .map(|component| component_items(spec, component))
            .collect();
        let principal = if used.principal {
            quote! {
                /// The caller the specification's policies and audits read: the runtime caller's
                /// subject is the principal's id, its attributes `kind`, `tenant` and `role` the rest.
                fn principal(caller: &strata_runtime::Caller) -> crate::support::Caller {
                    let attribute = |name: &str| caller.attributes.get(name).cloned();
                    crate::support::Caller {
                        kind: attribute("kind").unwrap_or_default(),
                        id: caller.subject.clone().unwrap_or_default(),
                        tenant: attribute("tenant"),
                        role: attribute("role"),
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        let refusal = if used.refusal {
            quote! {
                /// A refusal of the runtime rather than of the contract.
                fn refusal<E>(code: strata_runtime::FailureCode, message: impl Into<String>) -> strata_runtime::Failure<E> {
                    strata_runtime::Failure::Runtime { code, message: message.into() }
                }
            }
        } else {
            TokenStream::new()
        };
        let call_retry = if used.retry {
            quote! {
                /// The runtime's form of a retry policy of the specification.
                fn call_retry(policy: &crate::retries::RetryPolicy) -> strata_runtime::CallRetry {
                    strata_runtime::CallRetry {
                        attempts: policy.attempts,
                        backoff: match policy.backoff {
                            crate::retries::Backoff::Fixed { base } => strata_runtime::CallBackoff::Fixed(base),
                            crate::retries::Backoff::Exponential { base, cap } => {
                                strata_runtime::CallBackoff::Exponential { base, cap }
                            }
                        },
                        timeout: policy.timeout,
                    }
                }
            }
        } else {
            TokenStream::new()
        };
        let fixed_hooks = fixed_hook_traits(&used);
        let body_items = body
            .as_ref()
            .map(|b| body_items(spec, b))
            .unwrap_or_default();
        let crate_uses = uses.import();
        let authorizer = authorizer_items();
        let capabilities = access_model::items(spec);
        let catalogue = access_model::catalogue_items(spec, &served);
        let gateway = gateway_items(spec);
        quote! {
            #![doc = " The contracts on the wire: one `strata_runtime::Contract` per contract a component implements, the route of each contract with the handler and hooks it needs, and per implementing component the routes of all its contracts and a typed client."]
            #![doc = ""]
            #![doc = " A contract is served at its `binding.path` when the specification states one, otherwise at `/<implementing component>/<contract id>`, always as a POST of a JSON body."]
            #![doc = ""]
            #![doc = " Who may call is decided by `SpecAuthorizer`, generated from the auth schemes; `GATEWAY_ROUTES` lists the public contracts, the only ones routed through the gateway."]
            #crate_uses
            #authorizer
            #capabilities
            #catalogue
            #gateway
            #fixed_hooks
            #body_items
            #(#wire)*
            #(#contracts)*
            #(#components)*
            #principal
            #refusal
            #call_retry
        }
    }
}

/// Which module-level helpers and shared hooks traits the contracts use.
#[derive(Default)]
struct Used {
    principal: bool,
    refusal: bool,
    retry: bool,
    clock: bool,
    counter: bool,
    wait: bool,
}

fn fixed_hook_traits(used: &Used) -> TokenStream {
    let clock = naming::ident(AUDIT_CLOCK);
    let counter = naming::ident(QUOTA_COUNTER);
    let wait = naming::ident(QUOTA_WAIT);
    let mut out = TokenStream::new();
    if used.clock {
        out.extend(quote! {
            /// The time audits record, shared by the hooks of every contract with an audit.
            pub trait #clock: Send + Sync + 'static {
                /// The time audits record. The system clock unless overridden.
                fn now(&self) -> crate::prelude::Timestamp {
                    crate::prelude::Timestamp(strata_runtime::Clock::now(&strata_runtime::SystemClock).0)
                }
            }
        });
    }
    if used.counter {
        out.extend(quote! {
            /// The quota counters, shared by the hooks of every contract with a quota.
            pub trait #counter: Send + Sync + 'static {
                /// Counts one call against `quota` in the scope of `caller` (`quota.scope`) and stores
                /// the counter, deciding with `quota.check(counter, now)`. An `Err` answers `unavailable`.
                fn count(
                    &self,
                    quota: &'static crate::quotas::Quota,
                    caller: &crate::support::Caller,
                ) -> impl std::future::Future<Output = Result<crate::quotas::QuotaOutcome, String>> + Send;
            }
        });
    }
    if used.wait {
        out.extend(quote! {
            /// Holding calls, shared by the hooks of every contract with a queueing or throttling
            /// quota.
            pub trait #wait: Send + Sync + 'static {
                /// Holds a call that a queueing or throttling `quota` let through only after
                /// `retry_after`; the call goes ahead when it returns.
                fn wait(
                    &self,
                    quota: &'static crate::quotas::Quota,
                    retry_after: std::time::Duration,
                ) -> impl std::future::Future<Output = ()> + Send;
            }
        });
    }
    out
}

/// The Rust names the error body's items use.
struct BodyNames {
    /// The body type, as the module names it.
    ty: TokenStream,
    /// The enum type of its code field.
    code_ty: TokenStream,
    /// The code field.
    code: proc_macro2::Ident,
    /// The message field.
    message: proc_macro2::Ident,
    /// The correlation field, if the body names one.
    correlation: Option<proc_macro2::Ident>,
    /// The specification's name of the body type.
    type_name: String,
}

impl BodyNames {
    fn new(spec: &Spec, body: &ErrorBody, uses: &mut Uses) -> Self {
        let code_type = body_fields(spec, body)
            .and_then(|fields| fields.iter().find(|f| f.name == body.code))
            .map(|f| f.type_name.clone())
            .unwrap_or_default();
        BodyNames {
            ty: render::rust_type(spec, &TypeRef::parse(&body.type_name), Scope::Other, uses),
            code_ty: render::rust_type(spec, &TypeRef::parse(&code_type), Scope::Other, uses),
            code: view::snake_ident(&body.code),
            message: view::snake_ident(&body.message),
            correlation: body.correlation.as_deref().map(view::snake_ident),
            type_name: body.type_name.clone(),
        }
    }

    /// The code enum's value for `kind`.
    fn kind(&self, kind: &str) -> TokenStream {
        let ty = &self.code_ty;
        let variant = view::pascal_ident(kind);
        quote! { #ty::#variant }
    }
}

/// The error body's module items: `ErrorBody`, `BodyError`, and the helpers making and reading
/// bodies.
fn body_items(spec: &Spec, names: &BodyNames) -> TokenStream {
    let wire = &spec.system.error_wire;
    let Some(body) = &wire.body else {
        return TokenStream::new();
    };
    let BodyNames {
        ty,
        code_ty,
        code,
        message,
        correlation,
        type_name,
    } = names;
    let fields: Vec<TokenStream> = body_fields(spec, body)
        .unwrap_or_default()
        .iter()
        .map(|f| {
            let name = view::snake_ident(&f.name);
            if f.name == body.code && name == "code" {
                quote! { code }
            } else if f.name == body.code {
                quote! { #name: code }
            } else if f.name == body.message {
                quote! { #name: message.into() }
            } else if body.correlation.as_deref() == Some(f.name.as_str()) {
                quote! { #name: String::new() }
            } else {
                quote! { #name: None }
            }
        })
        .collect();
    let kind_of = |failure: &str| {
        body.runtime
            .iter()
            .find(|(k, _)| k == failure)
            .map(|(_, kind)| kind.clone())
            .unwrap_or_else(|| {
                if failure == IDEMPOTENCY_CONFLICT {
                    IDEMPOTENCY_CONFLICT_KIND.to_string()
                } else {
                    String::new()
                }
            })
    };
    let runtime_arms: Vec<TokenStream> = RUNTIME_FAILURES
        .iter()
        .map(|(failure, variant)| {
            let variant = naming::ident(variant);
            let kind = kind_of(failure);
            let status = Literal::u16_unsuffixed(wire.kind_status(&kind).unwrap_or(500));
            let value = names.kind(&kind);
            quote! {
                strata_runtime::FailureCode::#variant => (#status, error_body(#value, message))
            }
        })
        .collect();
    let code_arms: Vec<TokenStream> = code_values(spec, body)
        .unwrap_or_default()
        .iter()
        .map(|value| {
            // The runtime failure of the same name when it maps to this kind, else the first
            // that does, else internal.
            let failure = RUNTIME_FAILURES
                .iter()
                .find(|(failure, _)| failure == value && kind_of(failure) == *value)
                .or_else(|| {
                    RUNTIME_FAILURES
                        .iter()
                        .find(|(failure, _)| kind_of(failure) == *value)
                })
                .map_or("Internal", |(_, variant)| *variant);
            let failure = naming::ident(failure);
            let value = names.kind(value);
            quote! { #value => strata_runtime::FailureCode::#failure }
        })
        .collect();
    let set_correlation = match correlation {
        Some(field) => quote! { body.#field = correlation.to_string(); },
        None => quote! { let _ = correlation; },
    };
    let body_doc = format!(
        " The error body every failed call is answered with (`system.generation.errorBody`): type `{type_name}`, its code the kind of the failure."
    );
    let _ = (code, message);
    quote! {
        #[doc = #body_doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct ErrorBody;

        /// A contract error as the error body carries it.
        pub trait BodyError: Sized {
            /// The body this error is answered with, before the correlation id is set.
            fn body(&self) -> #ty;
            /// The error a body stands for, when it is one of this contract's: the only error of
            /// the body's kind, with no payload or the body as its payload.
            fn from_body(body: #ty) -> Option<Self>;
        }

        /// A body of kind `code` with `message` and every optional field left out.
        fn error_body(code: #code_ty, message: impl Into<String>) -> #ty {
            #ty { #(#fields),* }
        }

        /// The status and body of a failure the runtime reports itself.
        fn runtime_body(code: strata_runtime::FailureCode, message: String) -> (u16, #ty) {
            match code {
                #(#runtime_arms,)*
            }
        }

        /// The runtime failure a body that is no contract error is read back as.
        fn runtime_code(code: &#code_ty) -> strata_runtime::FailureCode {
            match code {
                #(#code_arms,)*
            }
        }

        impl<E> strata_runtime::FailureBody<E> for ErrorBody
        where
            E: BodyError + strata_runtime::ContractError,
        {
            fn encode(
                failure: strata_runtime::Failure<&E>,
                correlation: &str,
            ) -> serde_json::Result<(u16, Vec<u8>)> {
                let (status, mut body) = match failure {
                    strata_runtime::Failure::Contract { error } => (error.status(), error.body()),
                    strata_runtime::Failure::Runtime { code, message } => runtime_body(code, message),
                };
                #set_correlation
                Ok((status, serde_json::to_vec(&body)?))
            }

            fn decode(_status: u16, bytes: &[u8]) -> Option<strata_runtime::Failure<E>> {
                let body: #ty = serde_json::from_slice(bytes).ok()?;
                let code = runtime_code(&body.#code);
                let message = body.#message.clone();
                Some(match E::from_body(body) {
                    Some(error) => strata_runtime::Failure::Contract { error },
                    None => strata_runtime::Failure::Runtime { code, message },
                })
            }
        }
    }
}

/// The unit type of a served contract, its `Contract` and `ContractError` impls, and with an
/// error body its `BodyError` impl.
fn wire_type(spec: &Spec, c: &Contract, body: Option<&BodyNames>) -> TokenStream {
    let wire = &spec.system.error_wire;
    let name = view::pascal_ident(&c.id);
    let request = view::request_path(&c.id);
    let response = view::response_path(&c.id);
    let error_name = naming::ident(&format!("{}Error", pascal(&c.id)));
    let id = c.reference.clone().unwrap_or_else(|| c.id.clone());
    let path = contract_path(c);
    let access = access_tokens(c);
    let timeout = Literal::u64_unsuffixed(
        c.timeout_ms
            .as_ref()
            .and_then(serde_json::Number::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS),
    );
    let idempotent = c.idempotent.unwrap_or(false);
    let doc = format!(
        " Contract `{}` ({}) on the wire: `POST {path}`.",
        c.id, c.name
    );
    // The statuses, each with the variants that travel with it, in order of first appearance.
    let mut statuses: Vec<(u16, Vec<TokenStream>)> = Vec::new();
    for e in &c.errors {
        let status = wire.status_of(e).unwrap_or(500);
        let variant = view::pascal_ident(&e.name);
        let pattern = quote! { Self::#variant { .. } };
        match statuses.iter_mut().find(|(s, _)| *s == status) {
            Some((_, patterns)) => patterns.push(pattern),
            None => statuses.push((status, vec![pattern])),
        }
    }
    // An error enum with no variants has no value to answer for.
    let status_body = match statuses.as_slice() {
        [] => quote! { match *self {} },
        [(status, _)] => {
            let status = Literal::u16_unsuffixed(*status);
            quote! { #status }
        }
        _ => {
            let arms = statuses.iter().map(|(status, patterns)| {
                let status = Literal::u16_unsuffixed(*status);
                quote! { #(#patterns)|* => #status }
            });
            quote! { match self { #(#arms,)* } }
        }
    };
    let body_ty = match body {
        Some(_) => quote! { ErrorBody },
        None => quote! { strata_runtime::RuntimeBody },
    };
    let body_impl = body
        .map(|names| body_error_impl(spec, c, names))
        .unwrap_or_default();
    quote! {
        #[doc = #doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct #name;

        impl strata_runtime::Contract for #name {
            const SPEC: strata_runtime::ContractSpec = strata_runtime::ContractSpec {
                id: #id,
                path: #path,
                access: #access,
                timeout: std::time::Duration::from_millis(#timeout),
                idempotent: #idempotent,
            };
            type Request = #request;
            type Response = #response;
            type Error = crate::contracts::#error_name;
            type Body = #body_ty;
        }

        impl strata_runtime::ContractError for crate::contracts::#error_name {
            fn status(&self) -> u16 {
                #status_body
            }
        }

        #body_impl
    }
}

/// The `strata_runtime::Access` of served contract `c`. `check_access` refuses a contract without
/// an exposure or a structured auth before anything is emitted; should one get here, it is given
/// the access no route serves (local, in-process), so nothing unstated is ever admitted.
pub(super) fn access_tokens(c: &Contract) -> TokenStream {
    let (exposure, auth) = match (c.exposure, &c.auth) {
        (Some(exposure), Some(auth)) => (exposure, auth),
        _ => return quote! { strata_runtime::Access::LOCAL },
    };
    let exposure = match exposure {
        Exposure::Public => quote! { strata_runtime::Exposure::Public },
        Exposure::Internal => quote! { strata_runtime::Exposure::Internal },
        Exposure::Local => quote! { strata_runtime::Exposure::Local },
    };
    let scheme = match auth.scheme {
        Scheme::Anonymous => quote! { strata_runtime::Scheme::Anonymous },
        Scheme::Session => quote! { strata_runtime::Scheme::Session },
        Scheme::Service => quote! { strata_runtime::Scheme::Service },
        Scheme::ProvisioningToken => quote! { strata_runtime::Scheme::ProvisioningToken },
        Scheme::Signature => quote! { strata_runtime::Scheme::Signature },
        Scheme::SignedAddress => quote! { strata_runtime::Scheme::SignedAddress },
        Scheme::Delegation => quote! { strata_runtime::Scheme::Delegation },
        Scheme::InProcess => quote! { strata_runtime::Scheme::InProcess },
    };
    let roles = &auth.roles;
    let requires = &auth.requires;
    let tenant_policies = auth.tenant_policies;
    let csrf = auth.csrf;
    let origin = auth.origin;
    let step_up = match step_up_ms(auth) {
        Some(Ok(ms)) => {
            let ms = Literal::u64_unsuffixed(ms);
            quote! { Some(std::time::Duration::from_millis(#ms)) }
        }
        _ => quote! { None },
    };
    let secret = match &auth.secret {
        Some(name) => quote! { Some(#name) },
        None => quote! { None },
    };
    let tenant_scope = match c.tenant_scope {
        Some(TenantScope::Request) => quote! { Some(strata_runtime::TenantScope::Request) },
        Some(TenantScope::None) => quote! { Some(strata_runtime::TenantScope::None) },
        Some(TenantScope::Platform) => quote! { Some(strata_runtime::TenantScope::Platform) },
        None => quote! { None },
    };
    let development = match &c.development {
        Some(name) => quote! { Some(#name) },
        None => quote! { None },
    };
    quote! {
        strata_runtime::Access {
            exposure: #exposure,
            scheme: #scheme,
            roles: &[#(#roles),*],
            requires: &[#(#requires),*],
            tenant_policies: #tenant_policies,
            csrf: #csrf,
            origin: #origin,
            step_up: #step_up,
            secret: #secret,
            tenant_scope: #tenant_scope,
            development: #development,
        }
    }
}

/// The routes through the gateway: one `strata_runtime::GatewayRoute` per public contract.
fn gateway_items(spec: &Spec) -> TokenStream {
    let routes: Vec<TokenStream> = gateway_table(spec)
        .into_iter()
        .map(|c| {
            let path = contract_path(c);
            let id = c.reference.clone().unwrap_or_else(|| c.id.clone());
            let component = c.implemented_by.clone().unwrap_or_default();
            quote! {
                strata_runtime::GatewayRoute { path: #path, contract: #id, component: #component }
            }
        })
        .collect();
    quote! {
        /// The routes through the gateway: every public contract a component serves, and only
        /// those. An internal or local contract is never routed through the gateway.
        pub const GATEWAY_ROUTES: &[strata_runtime::GatewayRoute] = &[#(#routes),*];
    }
}

/// The authorizer generated from the auth schemes: the holes a deployment fills, and
/// `SpecAuthorizer`, which checks everything a contract's `Access` states.
fn authorizer_items() -> TokenStream {
    quote! {
        /// What the generated authorizer needs from the deployment: what no specification
        /// states, such as the session store, the sites and the secrets. Every method has a
        /// default that establishes nothing, so a scheme whose hole the deployment does not fill
        /// is refused.
        pub trait AuthHoles: Send + Sync + 'static {
            /// The site the request addresses, from its host: its tenant and the origins its pages
            /// are served from. `None` for a host that serves no site.
            fn site(&self, request: &strata_runtime::Presented<'_>) -> Option<strata_runtime::Site> {
                let _ = request;
                None
            }

            /// The principal of the live session the request presents (its cookie or header),
            /// looked up in the deployment's session store, with the session's request token.
            /// `Ok(None)` when it presents none or one that is not live; an `Err` answers
            /// `unavailable`.
            fn session(
                &self,
                request: &strata_runtime::Presented<'_>,
            ) -> impl std::future::Future<Output = Result<Option<strata_runtime::Principal>, String>> + Send {
                let _ = request;
                async { Ok(None) }
            }

            /// Whether `token` is a request token the deployment issued to a visitor of `site`,
            /// for an anonymous call that asks for one.
            fn visitor_token(&self, site: &strata_runtime::Site, token: &str) -> bool {
                let _ = (site, token);
                false
            }

            /// The header the service credential is presented in.
            fn service_header(&self) -> &'static str {
                "x-service-token"
            }

            /// The service credential, or `None` when this deployment admits no service.
            fn service_secret(&self) -> Option<String> {
                None
            }

            /// The principal a provisioning token (the bearer token) stands for: the provisioning
            /// principal of one realm, with its tenant. `Ok(None)` when the token is not one.
            fn provisioning(
                &self,
                token: &str,
            ) -> impl std::future::Future<Output = Result<Option<strata_runtime::Principal>, String>> + Send {
                let _ = token;
                async { Ok(None) }
            }

            /// The principal a delegation token (the bearer token) acts for. `Ok(None)` when the
            /// token is not a live delegation.
            fn delegation(
                &self,
                token: &str,
            ) -> impl std::future::Future<Output = Result<Option<strata_runtime::Principal>, String>> + Send {
                let _ = token;
                async { Ok(None) }
            }

            /// Who signed the request with the secret named `secret`, when its signature verifies.
            /// `Ok(None)` when it does not.
            fn signature(
                &self,
                secret: &'static str,
                request: &strata_runtime::Presented<'_>,
            ) -> impl std::future::Future<Output = Result<Option<strata_runtime::Principal>, String>> + Send {
                let _ = (secret, request);
                async { Ok(None) }
            }

            /// Whether the request's target is an address signed with the secret named `secret`
            /// and still valid.
            fn signed_address(&self, secret: &'static str, request: &strata_runtime::Presented<'_>) -> bool {
                let _ = (secret, request);
                false
            }

            /// Whether the development condition named `condition` holds in this deployment.
            fn development(&self, condition: &'static str) -> bool {
                let _ = condition;
                false
            }

            /// The plan and the flag settings of `tenant`, for a call requiring a capability that
            /// is not core or that a flag gates. `Ok(None)` when the tenant is not known, which
            /// entitles it to the core capabilities only, with every flag at its default.
            fn entitlement(
                &self,
                tenant: &str,
            ) -> impl std::future::Future<Output = Result<Option<Entitlement>, String>> + Send {
                let _ = tenant;
                async { Ok(None) }
            }

            /// What `principal`, established by a session or a delegation, holds: its platform
            /// roles, its tenant roles and its teams with their roles' permissions (tenant data),
            /// its live grants and, under a delegation, the delegation and its token's scopes.
            /// The default holds nothing, so every capability is refused.
            fn holding(
                &self,
                principal: &strata_runtime::Principal,
            ) -> impl std::future::Future<Output = Result<Holding, String>> + Send {
                let _ = principal;
                async { Ok(Holding::default()) }
            }

            /// The time now, in milliseconds since the Unix epoch, against which a step-up age is
            /// judged. The system clock unless overridden.
            fn now_ms(&self) -> i64 {
                strata_runtime::Clock::now(&strata_runtime::SystemClock).0
            }
        }

        /// The authorizer generated from the specification's auth schemes. It admits a call only
        /// when everything the contract's `Access` states holds, and refuses a local contract on
        /// any route.
        #[derive(Debug, Clone, Default)]
        pub struct SpecAuthorizer<H: AuthHoles> {
            /// The deployment's holes.
            pub holes: H,
        }

        impl<H: AuthHoles> SpecAuthorizer<H> {
            /// An authorizer over `holes`.
            pub fn new(holes: H) -> Self {
                Self { holes }
            }
        }

        impl<H: AuthHoles> strata_runtime::Authorizer for SpecAuthorizer<H> {
            async fn authorize(
                &self,
                access: &strata_runtime::Access,
                request: &strata_runtime::Presented<'_>,
            ) -> Result<strata_runtime::Caller, strata_runtime::Refusal> {
                authorize_access(&self.holes, access, request).await
            }
        }

        /// The checks of one call, in order: exposure, development condition, the scheme's
        /// credential, origin, request token, roles, step-up, tenant scope, and the capabilities
        /// the contract requires ([`permitted`]: plan entitlement, flag rollout, the principal's
        /// permission, delegation bounds).
        async fn authorize_access<H: AuthHoles>(
            holes: &H,
            access: &strata_runtime::Access,
            request: &strata_runtime::Presented<'_>,
        ) -> Result<strata_runtime::Caller, strata_runtime::Refusal> {
            use strata_runtime::{Exposure, FailureCode, Scheme, TenantScope};
            let refuse = |code: FailureCode, message: &str| strata_runtime::Refusal {
                code,
                message: message.to_string(),
            };
            let correlation = request
                .header(strata_runtime::routes::CORRELATION)
                .unwrap_or("")
                .to_string();
            if access.exposure == Exposure::Local {
                return Err(refuse(FailureCode::Forbidden, "a local contract is not served on a route"));
            }
            if let Some(condition) = access.development {
                if !holes.development(condition) {
                    return Err(refuse(FailureCode::Forbidden, "this is not available here"));
                }
            }
            let site = holes.site(request);
            let unauthenticated = |message: &str| refuse(FailureCode::Unauthenticated, message);
            let principal: Option<strata_runtime::Principal> = match access.scheme {
                Scheme::Anonymous => None,
                Scheme::Session => Some(
                    holes
                        .session(request)
                        .await
                        .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?
                        .ok_or_else(|| unauthenticated("sign in to do this"))?,
                ),
                Scheme::Service => {
                    let given = request.header(holes.service_header());
                    match (holes.service_secret(), given) {
                        (Some(secret), Some(given)) if strata_runtime::routes::same_secret(given, &secret) => {
                            Some(strata_runtime::Principal {
                                subject: "service".to_string(),
                                kind: "service".to_string(),
                                ..strata_runtime::Principal::default()
                            })
                        }
                        _ => return Err(unauthenticated("the service credential is missing or wrong")),
                    }
                }
                Scheme::ProvisioningToken => {
                    let token = request
                        .bearer()
                        .ok_or_else(|| unauthenticated("a provisioning token is required"))?;
                    Some(
                        holes
                            .provisioning(token)
                            .await
                            .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?
                            .ok_or_else(|| unauthenticated("the provisioning token is not valid"))?,
                    )
                }
                Scheme::Delegation => {
                    let token = request
                        .bearer()
                        .ok_or_else(|| unauthenticated("a delegation token is required"))?;
                    Some(
                        holes
                            .delegation(token)
                            .await
                            .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?
                            .ok_or_else(|| unauthenticated("the delegation token is not valid"))?,
                    )
                }
                Scheme::Signature => {
                    let secret = access
                        .secret
                        .ok_or_else(|| refuse(FailureCode::Forbidden, "no secret verifies this call"))?;
                    Some(
                        holes
                            .signature(secret, request)
                            .await
                            .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?
                            .ok_or_else(|| unauthenticated("the signature is missing or does not verify"))?,
                    )
                }
                Scheme::SignedAddress => {
                    let secret = access
                        .secret
                        .ok_or_else(|| refuse(FailureCode::Forbidden, "no secret verifies this address"))?;
                    if !holes.signed_address(secret, request) {
                        return Err(refuse(FailureCode::Forbidden, "the address is not signed or has expired"));
                    }
                    None
                }
                Scheme::InProcess => {
                    return Err(refuse(FailureCode::Forbidden, "an in-process contract is not served on a route"));
                }
            };
            if access.origin {
                let allowed = match (&site, request.header("origin")) {
                    (Some(site), Some(origin)) => site.origins.iter().any(|known| known == origin),
                    _ => false,
                };
                if !allowed {
                    return Err(refuse(FailureCode::Forbidden, "this request did not come from the site"));
                }
            }
            if access.csrf {
                let given = request.header(strata_runtime::CSRF_TOKEN).unwrap_or("");
                let valid = !given.is_empty()
                    && match &principal {
                        Some(principal) => principal
                            .csrf_token
                            .as_deref()
                            .is_some_and(|expected| strata_runtime::routes::same_secret(given, expected)),
                        None => site.as_ref().is_some_and(|site| holes.visitor_token(site, given)),
                    };
                if !valid {
                    return Err(refuse(FailureCode::Forbidden, "the request token is missing or stale"));
                }
            }
            let role = match &principal {
                Some(principal) => access
                    .roles
                    .iter()
                    .find(|role| principal.roles.iter().any(|held| held == *role))
                    .map(|role| role.to_string())
                    .or_else(|| principal.roles.first().cloned()),
                None => None,
            };
            if !access.roles.is_empty() {
                let held = principal.as_ref().is_some_and(|principal| {
                    access.roles.iter().any(|role| principal.roles.iter().any(|held| held == role))
                });
                if !held {
                    return Err(match principal {
                        Some(_) => refuse(FailureCode::Forbidden, "the caller holds no role this needs"),
                        None => unauthenticated("sign in to do this"),
                    });
                }
            }
            if let Some(age) = access.step_up {
                let age = i64::try_from(age.as_millis()).unwrap_or(i64::MAX);
                let now = holes.now_ms();
                let fresh = principal
                    .as_ref()
                    .and_then(|principal| principal.authenticated_at_ms)
                    .is_some_and(|at| at <= now && now.saturating_sub(at) <= age);
                if !fresh {
                    return Err(unauthenticated("prove who you are again to do this"));
                }
            }
            if access.tenant_scope == Some(TenantScope::Request) {
                let Some(site) = &site else {
                    return Err(refuse(FailureCode::Forbidden, "the request addresses no site"));
                };
                let own = principal.as_ref().and_then(|principal| principal.tenant.as_deref());
                if own.is_some_and(|tenant| tenant != site.tenant) {
                    return Err(refuse(FailureCode::Forbidden, "the caller belongs to another tenant"));
                }
            }
            if !access.requires.is_empty() {
                let tenant = principal
                    .as_ref()
                    .and_then(|principal| principal.tenant.clone())
                    .or_else(|| site.as_ref().map(|site| site.tenant.clone()));
                let holding = match (&principal, access.scheme) {
                    (Some(principal), Scheme::Session | Scheme::Delegation) => Some(
                        holes
                            .holding(principal)
                            .await
                            .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?,
                    ),
                    _ => None,
                };
                if access.scheme == Scheme::Delegation
                    && !holding.as_ref().is_some_and(|holding| holding.acting_for.is_some())
                {
                    return Err(refuse(
                        FailureCode::Forbidden,
                        CapabilityRefusal::BeyondDelegation("").message(),
                    ));
                }
                let asker = match (&holding, access.scheme) {
                    (Some(holding), _) => Asker::Principal(holding),
                    (None, Scheme::Service) => Asker::Service,
                    (None, Scheme::Signature | Scheme::ProvisioningToken) => Asker::External,
                    (None, _) => Asker::Anonymous,
                };
                let needs_tenant = access.requires.iter().any(|required| {
                    CAPABILITIES
                        .iter()
                        .find(|capability| capability.id == *required)
                        .is_none_or(|capability| !capability.core || !capability.flags.is_empty())
                });
                let entitlement = match (&tenant, needs_tenant) {
                    (Some(tenant), true) => holes
                        .entitlement(tenant)
                        .await
                        .map_err(|why| refuse(FailureCode::Unavailable, why.as_str()))?,
                    _ => None,
                };
                permitted(access.requires, asker, tenant.as_deref(), entitlement.as_ref(), holes.now_ms())
                    .map_err(|refusal| refuse(FailureCode::Forbidden, refusal.message()))?;
            }
            let mut attributes = std::collections::BTreeMap::new();
            let tenant = principal
                .as_ref()
                .and_then(|principal| principal.tenant.clone())
                .or_else(|| site.as_ref().map(|site| site.tenant.clone()));
            if let Some(tenant) = tenant {
                attributes.insert("tenant".to_string(), tenant);
            }
            if let Some(role) = role {
                attributes.insert("role".to_string(), role);
            }
            if let Some(principal) = &principal {
                attributes.insert("kind".to_string(), principal.kind.clone());
            }
            Ok(strata_runtime::Caller {
                subject: principal.map(|principal| principal.subject),
                correlation,
                attributes,
            })
        }
    }
}

/// How contract `c`'s errors become bodies of the error body and are read back.
fn body_error_impl(spec: &Spec, c: &Contract, names: &BodyNames) -> TokenStream {
    let wire = &spec.system.error_wire;
    let error_name = naming::ident(&format!("{}Error", pascal(&c.id)));
    let code = &names.code;
    let mut body_arms = Vec::new();
    for e in &c.errors {
        let variant = view::pascal_ident(&e.name);
        let kind = names.kind(wire.kind_of(e).unwrap_or_default());
        let message = &e.name;
        body_arms.push(match &e.type_name {
            Some(t) if *t == names.type_name => quote! {
                Self::#variant(body) => {
                    let mut body = body.clone();
                    body.#code = #kind;
                    body
                }
            },
            Some(_) => quote! { Self::#variant(_) => error_body(#kind, #message) },
            None => quote! { Self::#variant => error_body(#kind, #message) },
        });
    }
    let body_fn = if body_arms.is_empty() {
        quote! { match *self {} }
    } else {
        quote! { match self { #(#body_arms,)* } }
    };
    // Read back only the errors that are alone with their kind and can be rebuilt from a body.
    let mut from_arms = Vec::new();
    for e in &c.errors {
        let Some(kind) = wire.kind_of(e) else {
            continue;
        };
        if c.errors
            .iter()
            .filter(|o| wire.kind_of(o) == Some(kind))
            .count()
            != 1
        {
            continue;
        }
        let variant = view::pascal_ident(&e.name);
        let value = names.kind(kind);
        match &e.type_name {
            Some(t) if *t == names.type_name => {
                from_arms.push(quote! { #value => Some(Self::#variant(body)) })
            }
            None => from_arms.push(quote! { #value => Some(Self::#variant) }),
            Some(_) => {}
        }
    }
    let all =
        code_values(spec, wire.body.as_ref().expect("an error body")).map_or(0, <[String]>::len);
    let from_fn = if from_arms.is_empty() {
        quote! {
            let _ = body;
            None
        }
    } else if from_arms.len() == all {
        quote! { match body.#code { #(#from_arms,)* } }
    } else {
        quote! { match body.#code { #(#from_arms,)* _ => None, } }
    };
    let ty = &names.ty;
    quote! {
        impl BodyError for crate::contracts::#error_name {
            fn body(&self) -> #ty {
                #body_fn
            }

            fn from_body(body: #ty) -> Option<Self> {
                #from_fn
            }
        }
    }
}

/// The handler trait, the hooks trait, the private answer and the public route of one contract.
fn contract_items(spec: &Spec, c: &Contract, uses: &mut Uses, used: &mut Used) -> TokenStream {
    let n = needs(spec, c);
    let base = pascal(&c.id);
    let wire = view::pascal_ident(&c.id);
    let handler_name = naming::ident(&format!("{base}Handler"));
    let hooks_name = naming::ident(&format!("{base}Hooks"));
    let serve_fn = naming::ident(&format!("serve_{}", snake(&c.id)));
    let answer_fn = naming::ident(&format!("answer_{}", snake(&c.id)));
    let method = naming::ident(&snake(&c.id));
    let component = c.implemented_by.as_deref().unwrap_or_default();
    let trait_name = view::pascal_ident(component);
    let request = view::request_path(&c.id);
    let response = view::response_path(&c.id);
    let error = naming::ident(&format!("{base}Error"));
    let error = quote! { crate::contracts::#error };

    used.principal |= n.principal();
    used.retry |= n.retry.is_some();
    used.refusal |= n.validates
        || n.governed
        || n.resource.is_some()
        || !n.quotas.is_empty()
        || !n.audits.is_empty();

    let mut supertraits = vec![quote! { Send }, quote! { Sync }, quote! { 'static }];
    let mut hook_methods = Vec::new();
    let mut steps = Vec::new();
    if n.validates {
        steps.push(quote! {
            if let Err(broken) = request.validate() {
                return Err(refusal(strata_runtime::FailureCode::InvalidRequest, broken.to_string()));
            }
        });
    }
    if n.principal() {
        steps.push(quote! { let principal = principal(&caller); });
    }
    if n.governed {
        let authorize = naming::ident(&format!("authorize_{}", snake(&c.id)));
        let denied = format!("the policies of contract {} refuse this caller", c.id);
        let decision = match &n.resource {
            Some(record) => {
                let hook = naming::ident(&format!("{}_resource", snake(&c.id)));
                let record_ty = view::record_path(record);
                let doc = format!(
                    " The record of `{record}` the policies of contract `{}` read for `request`, or `None` when there is none. An `Err` answers `unavailable`.",
                    c.id
                );
                hook_methods.push(quote! {
                    #[doc = #doc]
                    fn #hook(&self, request: &#request) -> impl std::future::Future<Output = Result<Option<#record_ty>, String>> + Send;
                });
                quote! {
                    {
                        let record = hooks
                            .#hook(&request)
                            .await
                            .map_err(|why| refusal(strata_runtime::FailureCode::Unavailable, why))?;
                        crate::policies::#authorize(&principal, &request, record.as_ref())
                    }
                }
            }
            None => quote! { crate::policies::#authorize(&principal, &request) },
        };
        steps.push(quote! {
            if !#decision.is_allowed() {
                return Err(refusal(strata_runtime::FailureCode::Forbidden, #denied));
            }
        });
    }
    if !n.quotas.is_empty() {
        used.counter = true;
        supertraits.push({
            let t = naming::ident(QUOTA_COUNTER);
            quote! { #t }
        });
    }
    if n.waits() {
        used.wait = true;
        supertraits.push({
            let t = naming::ident(QUOTA_WAIT);
            quote! { #t }
        });
    }
    for q in &n.quotas {
        let quota = view::upper_ident(&q.id);
        let mut waits = true;
        let exceeded = match q.on_exceed {
            OnExceed::Queue | OnExceed::Throttle => {
                quote! { hooks.wait(&crate::quotas::#quota, retry_after).await; }
            }
            OnExceed::Refuse => match q.error.as_deref().and_then(|e| error_of(c, e)) {
                Some(e) => {
                    let variant = view::pascal_ident(&e.name);
                    match &e.type_name {
                        None => {
                            waits = false;
                            quote! {
                                return Err(strata_runtime::Failure::Contract { error: #error::#variant });
                            }
                        }
                        Some(t) => {
                            let hook = naming::ident(&format!("{}_exceeded", snake(&q.id)));
                            let ty =
                                render::rust_type(spec, &TypeRef::parse(t), Scope::Other, uses);
                            let doc = format!(
                                " The payload of error `{}` of contract `{}` when quota `{}` refuses a call, `retry_after` before the window ends.",
                                e.name, c.id, q.id
                            );
                            hook_methods.push(quote! {
                                #[doc = #doc]
                                fn #hook(&self, retry_after: std::time::Duration) -> #ty;
                            });
                            quote! {
                                return Err(strata_runtime::Failure::Contract {
                                    error: #error::#variant(hooks.#hook(retry_after)),
                                });
                            }
                        }
                    }
                }
                None => {
                    waits = false;
                    let message = format!("quota {} is exceeded", q.id);
                    quote! {
                        return Err(refusal(strata_runtime::FailureCode::Unavailable, #message));
                    }
                }
            },
        };
        let pattern = if waits {
            quote! { crate::quotas::QuotaOutcome::Exceeded { retry_after, .. } }
        } else {
            quote! { crate::quotas::QuotaOutcome::Exceeded { .. } }
        };
        steps.push(quote! {
            match hooks
                .count(&crate::quotas::#quota, &principal)
                .await
                .map_err(|why| refusal(strata_runtime::FailureCode::Unavailable, why))?
            {
                crate::quotas::QuotaOutcome::Allowed(_) => {}
                #pattern => {
                    #exceeded
                }
            }
        });
    }
    let call = if n.audits.is_empty() {
        quote! {
            handler.handle(request).await.map_err(|error| strata_runtime::Failure::Contract { error })
        }
    } else {
        used.clock = true;
        supertraits.push({
            let t = naming::ident(AUDIT_CLOCK);
            quote! { #t }
        });
        let mut appends = Vec::new();
        for a in &n.audits {
            let audit_fn = naming::ident(&format!("audit_{}", snake(&a.id)));
            let entry = naming::ident(&format!("{}Entry", pascal(&a.id)));
            let hook = naming::ident(&format!("append_{}", snake(&a.id)));
            let doc = format!(
                " Appends an entry of audit `{}` (record `{}`) after a successful call of contract `{}`. An `Err` answers `internal`.",
                a.id,
                a.record.clone().unwrap_or_default(),
                c.id
            );
            hook_methods.push(quote! {
                #[doc = #doc]
                fn #hook(&self, entry: crate::audits::#entry) -> impl std::future::Future<Output = Result<(), String>> + Send;
            });
            appends.push(quote! {
                let entry = crate::audits::#audit_fn(&principal, &audited, &response, hooks.now())
                    .map_err(|why| refusal(strata_runtime::FailureCode::Internal, why.to_string()))?;
                hooks
                    .#hook(entry)
                    .await
                    .map_err(|why| refusal(strata_runtime::FailureCode::Internal, why))?;
            });
        }
        quote! {
            let audited = request.clone();
            let response = handler.handle(request).await.map_err(|error| strata_runtime::Failure::Contract { error })?;
            #(#appends)*
            Ok(response)
        }
    };
    let uses_hooks = n.resource.is_some() || !n.quotas.is_empty() || !n.audits.is_empty();
    let caller_param = if n.principal() {
        quote! { caller }
    } else {
        quote! { _caller }
    };
    let hooks_param = if uses_hooks {
        quote! { hooks }
    } else {
        quote! { _hooks }
    };

    let handler_doc = format!(
        " What serves contract `{}` ({}) on its own. Every implementation of the trait of component `{component}` implements it.",
        c.id, c.name
    );
    let handle_doc = format!(" Answers one call of contract `{}`.", c.id);
    let hooks_doc = format!(
        " What the route of contract `{}` needs from the deployment beyond its handler.",
        c.id
    );
    let blanket = if hook_methods.is_empty() {
        let doc = format!(
            " The route of contract `{}` needs no hook of its own.",
            c.id
        );
        quote! {
            #[doc = #doc]
            impl<T: #(#supertraits)+*> #hooks_name for T {}
        }
    } else {
        TokenStream::new()
    };
    let answer_doc = format!(" Answers one call of contract `{}`.", c.id);
    let serve_doc = format!(
        " Adds the route of contract `{}` to `routes`, answered by `handler` after the checks the specification states for it.",
        c.id
    );
    quote! {
        #[doc = #handler_doc]
        pub trait #handler_name: Send + Sync + 'static {
            #[doc = #handle_doc]
            fn handle(
                &self,
                request: #request,
            ) -> impl std::future::Future<Output = Result<#response, #error>> + Send;
        }

        impl<T: crate::contracts::#trait_name + Send + Sync + 'static> #handler_name for T {
            fn handle(
                &self,
                request: #request,
            ) -> impl std::future::Future<Output = Result<#response, #error>> + Send {
                <T as crate::contracts::#trait_name>::#method(self, request)
            }
        }

        #[doc = #hooks_doc]
        pub trait #hooks_name: #(#supertraits)+* {
            #(#hook_methods)*
        }

        #blanket

        #[doc = #answer_doc]
        async fn #answer_fn<S: #handler_name, H: #hooks_name>(
            handler: &S,
            #hooks_param: &H,
            #caller_param: strata_runtime::Caller,
            request: #request,
        ) -> Result<#response, strata_runtime::Failure<#error>> {
            #(#steps)*
            #call
        }

        #[doc = #serve_doc]
        pub fn #serve_fn<A, S, H>(
            routes: strata_runtime::Routes<A>,
            handler: std::sync::Arc<S>,
            hooks: std::sync::Arc<H>,
        ) -> strata_runtime::Routes<A>
        where
            A: strata_runtime::Authorizer,
            S: #handler_name,
            H: #hooks_name,
        {
            routes.serve_checked::<#wire, _, _>(move |caller: strata_runtime::Caller, request: #request| {
                let (handler, hooks) = (handler.clone(), hooks.clone());
                async move { #answer_fn(&*handler, &*hooks, caller, request).await }
            })
        }
    }
}

/// The hooks trait, the routes function and the client of one component, built from the pieces
/// of its contracts.
fn component_items(spec: &Spec, component: &Component) -> TokenStream {
    let type_name = pascal(&component.id);
    let hooks_name = naming::ident(&format!("{type_name}Hooks"));
    let client_name = naming::ident(&format!("{type_name}Client"));
    let routes_fn = naming::ident(&format!("{}_routes", snake(&component.id)));
    let trait_name = view::pascal_ident(&component.id);
    let contracts = contracts_of(spec, &component.id);

    let contract_hooks: Vec<proc_macro2::Ident> = contracts
        .iter()
        .map(|c| naming::ident(&format!("{}Hooks", pascal(&c.id))))
        .collect();
    let serve_fns: Vec<proc_macro2::Ident> = contracts
        .iter()
        .map(|c| naming::ident(&format!("serve_{}", snake(&c.id))))
        .collect();
    let mut registrations = Vec::new();
    for (i, serve) in serve_fns.iter().enumerate() {
        if i + 1 == serve_fns.len() {
            registrations.push(quote! { #serve(routes, service, hooks) });
        } else {
            registrations
                .push(quote! { let routes = #serve(routes, service.clone(), hooks.clone()); });
        }
    }

    let mut client_methods = Vec::new();
    for c in &contracts {
        let n = needs(spec, c);
        let wire = view::pascal_ident(&c.id);
        let method = naming::ident(&snake(&c.id));
        let request = view::request_path(&c.id);
        let response = view::response_path(&c.id);
        let error = naming::ident(&format!("{}Error", pascal(&c.id)));
        let call_doc = format!(" Calls contract `{}`: {}.", c.id, c.name);
        let body = match n.retry {
            Some(r) => {
                let policy = view::upper_ident(&r.id);
                quote! {
                    self.client
                        .call_retrying::<#wire>(request, "", &call_retry(&crate::retries::#policy))
                        .await
                }
            }
            None => quote! { self.client.call::<#wire>(request).await },
        };
        client_methods.push(quote! {
            #[doc = #call_doc]
            pub async fn #method(
                &self,
                request: &#request,
            ) -> Result<#response, strata_runtime::CallError<crate::contracts::#error>> {
                #body
            }
        });
    }

    let hooks_doc = Doc::new(&format!(
        "What the routes of component `{}` ({}) need from the deployment beyond the implementation of its contracts: the hooks of every one of them.",
        component.id, component.name
    ))
    .tokens();
    let routes_doc = format!(
        " Adds the route of every contract component `{}` implements to `routes`, each answered by `service` after the checks the specification states for it.",
        component.id
    );
    let client_doc = format!(
        " A typed client for the contracts component `{}` ({}) implements.",
        component.id, component.name
    );
    quote! {
        #hooks_doc
        pub trait #hooks_name: #(#contract_hooks)+* {}

        impl<T: #(#contract_hooks)+*> #hooks_name for T {}

        #[doc = #routes_doc]
        pub fn #routes_fn<A, S, H>(
            routes: strata_runtime::Routes<A>,
            service: std::sync::Arc<S>,
            hooks: std::sync::Arc<H>,
        ) -> strata_runtime::Routes<A>
        where
            A: strata_runtime::Authorizer,
            S: crate::contracts::#trait_name + Send + Sync + 'static,
            H: #hooks_name,
        {
            #(#registrations)*
        }

        #[doc = #client_doc]
        #[derive(Debug, Clone)]
        pub struct #client_name<K: strata_runtime::Credentials = strata_runtime::NoCredentials> {
            client: strata_runtime::Client<K>,
        }

        impl<K: strata_runtime::Credentials> #client_name<K> {
            /// A client calling through `client`, which holds the component's address and the
            /// caller's credentials.
            pub fn new(client: strata_runtime::Client<K>) -> Self {
                Self { client }
            }

            /// The runtime client underneath, for calls with a correlation id or another retry
            /// policy.
            pub fn client(&self) -> &strata_runtime::Client<K> {
                &self.client
            }

            #(#client_methods)*
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load;

    fn spec(contracts: &str) -> Spec {
        spec_with("", "", contracts)
    }

    /// The contracts with the access every test but the access tests takes for granted: internal,
    /// with the service credential.
    const SERVICE: &str =
        r#""implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "service" }"#;

    fn spec_with(generation: &str, types: &str, contracts: &str) -> Spec {
        let contracts = if contracts.contains("\"exposure\"") {
            contracts.to_string()
        } else {
            contracts.replace(r#""implementedBy": "svc""#, SERVICE)
        };
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S", "generation": {{ "target": "rust" {generation} }} }},
                "components": [ {{ "id": "svc", "name": "Service" }} ],
                "types": [ {types} ],
                "contracts": [ {contracts} ] }}"#
            ),
        )
        .unwrap()
    }

    fn codes(s: &Spec) -> Vec<String> {
        RoutesBackend.check(s).into_iter().map(|d| d.code).collect()
    }

    #[test]
    fn the_path_is_the_binding_or_the_component_and_id() {
        let s = spec(
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc", "binding": { "path": "/api/a" } },
               { "id": "b", "name": "B", "realisation": "generated", "implementedBy": "svc" }"#,
        );
        assert_eq!(contract_path(&s.contracts[0]), "/api/a");
        assert_eq!(contract_path(&s.contracts[1]), "/svc/b");
        assert!(codes(&s).is_empty());
    }

    #[test]
    fn unservable_paths_and_methods_are_refused() {
        let s = spec(
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc", "binding": { "path": "/api/{id}" } },
               { "id": "b", "name": "B", "realisation": "generated", "implementedBy": "svc", "binding": { "method": "GET", "path": "/svc/c" } },
               { "id": "c", "name": "C", "realisation": "generated", "implementedBy": "svc" }"#,
        );
        assert_eq!(
            codes(&s),
            [PATH_INVALID, METHOD_UNSUPPORTED, PATH_DUPLICATE]
        );
    }

    #[test]
    fn routes_are_plain_segments() {
        assert!(is_route("/a/b-c_d.e~f"));
        assert!(!is_route("/"));
        assert!(!is_route("a"));
        assert!(!is_route("/a/"));
        assert!(!is_route("/a//b"));
        assert!(!is_route("/a/:id"));
    }

    const KINDS: &str = r#", "errorKinds": { "conflict": 409, "invalid": 422, "internal": 500, "unavailable": 503, "forbidden": 403, "unauthenticated": 401 }"#;

    const SERVED: &str = r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc",
        "errors": [ { "name": "busy", "kind": "conflict" }, { "name": "odd", "status": 418 }, { "name": "plain" } ] }"#;

    #[test]
    fn an_error_takes_its_status_its_kind_or_the_default() {
        let s = spec_with(&format!(r#"{KINDS}, "errorStatus": 422"#), "", SERVED);
        assert!(codes(&s).is_empty());
        let wire = &s.system.error_wire;
        let statuses: Vec<_> = s.contracts[0]
            .errors
            .iter()
            .map(|e| wire.status_of(e))
            .collect();
        assert_eq!(statuses, [Some(409), Some(418), Some(422)]);
    }

    /// Negative fixture of `ready.error-status-missing`: a served error with no status, no kind
    /// and no default.
    #[test]
    fn a_served_error_without_a_status_is_refused() {
        let s = spec_with(KINDS, "", SERVED);
        assert_eq!(codes(&s), [ERROR_STATUS_MISSING]);
    }

    #[test]
    fn an_undeclared_error_kind_is_refused() {
        let s = spec_with(
            KINDS,
            "",
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc",
                "errors": [ { "name": "gone", "kind": "gone" } ] }"#,
        );
        assert_eq!(codes(&s), [ERROR_KIND_UNKNOWN]);
    }

    const BODY_TYPES: &str = r#"{ "id": "code", "name": "Code", "kind": "enum", "values": ["conflict", "invalid", "internal", "unavailable", "forbidden", "unauthenticated"] },
        { "id": "reply", "name": "Reply", "kind": "struct", "fields": [
            { "name": "code", "type": "code" }, { "name": "message", "type": "string" },
            { "name": "detail", "type": "string", "optional": true }, { "name": "correlation", "type": "string" } ] }"#;

    const BODY: &str = r#", "errorBody": { "type": "reply", "code": "code", "message": "message", "correlation": "correlation",
        "runtime": { "unauthenticated": "unauthenticated", "forbidden": "forbidden", "invalid-request": "invalid", "timeout": "unavailable", "unavailable": "unavailable", "internal": "internal" } }"#;

    const KINDED: &str = r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc",
        "errors": [ { "name": "busy", "kind": "conflict", "type": "reply" }, { "name": "bad", "kind": "invalid" } ] }"#;

    #[test]
    fn an_error_body_over_a_struct_of_the_kinds_is_accepted() {
        let s = spec_with(&format!("{KINDS}{BODY}"), BODY_TYPES, KINDED);
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
    }

    /// Negative fixture of `specc.error-body-invalid`: an error body whose type has a required
    /// field the body does not name, which a body made for a runtime failure could not fill.
    #[test]
    fn an_error_body_with_an_unnamed_required_field_is_refused() {
        let types = BODY_TYPES.replace(
            r#"{ "name": "detail", "type": "string", "optional": true }"#,
            r#"{ "name": "detail", "type": "string" }"#,
        );
        let s = spec_with(&format!("{KINDS}{BODY}"), &types, KINDED);
        assert_eq!(codes(&s), [ERROR_BODY_INVALID]);
    }

    #[test]
    fn under_an_error_body_an_error_without_a_kind_is_refused() {
        let s = spec_with(
            &format!("{KINDS}{BODY}"),
            BODY_TYPES,
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc",
                "errors": [ { "name": "odd", "status": 418 } ] }"#,
        );
        assert_eq!(codes(&s), [ERROR_KIND_MISSING]);
    }

    /// Removes the whitespace of emitted code, so it can be searched whatever the formatting.
    fn squeezed(tokens: TokenStream) -> String {
        tokens
            .to_string()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    /// Negative fixtures of `contract.exposure` and `contract.auth-structured`: a served contract
    /// without an exposure, with its auth as a sentence, with no auth, with the in-process scheme
    /// on an internal contract, and with a signature that names no secret.
    #[test]
    fn a_served_contract_without_exposure_or_structured_auth_is_refused() {
        let s = spec(
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc", "auth": { "scheme": "service" } },
               { "id": "b", "name": "B", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": "Any operator." },
               { "id": "c", "name": "C", "realisation": "generated", "implementedBy": "svc", "exposure": "internal" },
               { "id": "d", "name": "D", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "in-process" } },
               { "id": "e", "name": "E", "realisation": "generated", "implementedBy": "svc", "exposure": "public", "auth": { "scheme": "signature" } }"#,
        );
        assert_eq!(
            codes(&s),
            [
                EXPOSURE,
                AUTH_STRUCTURED,
                AUTH_STRUCTURED,
                EXPOSURE,
                AUTH_STRUCTURED
            ]
        );
    }

    /// Once the specification states capabilities, a served contract reachable from outside its
    /// process requires one (`capability.required`) and names only declared ones
    /// (`capability.unknown`); a local contract requires none. The required capabilities are
    /// data in the contract's `Access`, the tables are generated from the capability model, and
    /// the authorizer calls the one capability check.
    #[test]
    fn capabilities_are_required_declared_and_checked_in_one_place() {
        const MODEL: &str = r#""capabilities": [ { "id": "read", "core": true }, { "id": "premium" } ],
            "plans": [ { "id": "pro", "capabilities": ["premium"] } ],
            "roles": [ { "id": "visitor", "level": "platform", "holders": ["anonymous"], "capabilities": ["read"] },
                       { "id": "editor", "level": "tenant", "holders": ["person"], "capabilities": ["premium"] } ],
            "featureFlags": [ { "id": "premium-flag", "gates": ["premium"] } ],"#;
        let with_model = |contracts: &str| {
            load(
                "s",
                &format!(
                    r#"{{ "system": {{ "id": "s", "name": "S", "generation": {{ "target": "rust" }} }},
                    "components": [ {{ "id": "svc", "name": "Service" }} ], {MODEL}
                    "contracts": [ {contracts} ] }}"#
                ),
            )
            .unwrap()
        };
        let refused = with_model(
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "service" } },
               { "id": "b", "name": "B", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "service", "requires": ["write"] } },
               { "id": "c", "name": "C", "realisation": "generated", "implementedBy": "svc", "exposure": "local", "auth": { "scheme": "in-process" } }"#,
        );
        assert_eq!(
            codes(&refused),
            [
                access_model::CAPABILITY_REQUIRED,
                access_model::CAPABILITY_UNKNOWN
            ]
        );
        let ready = with_model(
            r#"{ "id": "a", "name": "A", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "service", "requires": ["read", "premium"] } }"#,
        );
        assert!(codes(&ready).is_empty(), "{:?}", codes(&ready));
        let text = squeezed(RoutesBackend.emit(&ready));
        assert!(
            text.contains(r#"roles:&[],requires:&["read","premium"],"#),
            "{text}"
        );
        assert!(
            text.contains(r#"CapabilitySpec{id:"premium",core:false,flags:&["premium-flag"]}"#),
            "{text}"
        );
        assert!(
            text.contains(r#"pubconstPLANS:&[(&str,&[&str])]=&[("pro",&["premium"])];"#),
            "{text}"
        );
        assert!(
            text.contains(r#"pubconstANONYMOUS_CAPABILITIES:&[&str]=&["read"];"#),
            "{text}"
        );
        assert!(
            text.contains(r#"pubconstPLATFORM_ROLES:&[(&str,&[&str])]=&[("visitor",&["read"])];"#),
            "tenant roles are tenant data: {text}"
        );
        assert!(text.contains("permitted(access.requires,asker,"), "{text}");
        assert!(text.contains(r#"input_schema:"{\""#), "{text}");
        assert!(text.contains(r#"\"additionalProperties\":false"#), "{text}");
    }

    const EXPOSED: &str = r#"{ "id": "pub", "name": "Pub", "realisation": "generated", "implementedBy": "svc", "exposure": "public", "readOnly": true, "tenantScope": "request", "auth": { "scheme": "anonymous" } },
        { "id": "int", "name": "Int", "realisation": "generated", "implementedBy": "svc", "exposure": "internal", "auth": { "scheme": "service" } },
        { "id": "loc", "name": "Loc", "realisation": "generated", "implementedBy": "svc", "exposure": "local", "auth": { "scheme": "in-process" } },
        { "id": "own", "name": "Own", "realisation": "generated", "implementedBy": "svc", "exposure": "public", "tenantScope": "request", "development": "DEV",
          "auth": { "scheme": "session", "roles": ["owner"], "csrf": true, "origin": true, "stepUp": "15 min" } }"#;

    #[test]
    fn only_public_contracts_are_routed_through_the_gateway() {
        let s = spec(EXPOSED);
        assert!(codes(&s).is_empty(), "{:?}", codes(&s));
        let ids: Vec<&str> = gateway_table(&s).iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["pub", "own"]);
        let text = squeezed(RoutesBackend.emit(&s));
        let table = text
            .split("GATEWAY_ROUTES")
            .nth(1)
            .and_then(|rest| rest.split("];").next())
            .unwrap_or_default()
            .to_string();
        assert!(table.contains(r#"path:"/svc/pub""#), "{table}");
        assert!(table.contains(r#"path:"/svc/own""#), "{table}");
        assert!(!table.contains("/svc/int"), "{table}");
        assert!(!table.contains("/svc/loc"), "{table}");
    }

    /// The access of each contract is data in its `ContractSpec`, and the authorizer is a match
    /// over the schemes with an arm for each and no fallback: nothing reads the auth as words.
    #[test]
    fn the_authorizer_is_generated_from_the_schemes_without_string_matching() {
        let s = spec(EXPOSED);
        let text = squeezed(RoutesBackend.emit(&s));
        assert!(text.contains("access:strata_runtime::Access{exposure:strata_runtime::Exposure::Internal,scheme:strata_runtime::Scheme::Service,roles:&[],"), "{text}");
        assert!(text.contains(r#"scheme:strata_runtime::Scheme::Session,roles:&["owner"],requires:&[],tenant_policies:false,csrf:true,origin:true,step_up:Some(std::time::Duration::from_millis(900000)),secret:None,tenant_scope:Some(strata_runtime::TenantScope::Request),development:Some("DEV"),"#), "{text}");
        assert!(
            !text.contains("auth:\""),
            "no ContractSpec carries words: {text}"
        );
        let authorize = text
            .split("asyncfnauthorize_access")
            .nth(1)
            .unwrap_or_default();
        for scheme in Scheme::ALL {
            let variant = match scheme {
                Scheme::Anonymous => "Anonymous",
                Scheme::Session => "Session",
                Scheme::Service => "Service",
                Scheme::ProvisioningToken => "ProvisioningToken",
                Scheme::Signature => "Signature",
                Scheme::SignedAddress => "SignedAddress",
                Scheme::Delegation => "Delegation",
                Scheme::InProcess => "InProcess",
            };
            assert!(
                authorize.contains(&format!("Scheme::{variant}=>")),
                "{variant}"
            );
        }
        assert!(
            !authorize.contains("_=>Some"),
            "no scheme falls through to an admission"
        );
        assert!(text.contains("pubtraitAuthHoles"));
        assert!(text.contains("pubstructSpecAuthorizer"));
    }
}
