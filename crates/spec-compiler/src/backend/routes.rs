//! Contracts on the wire (the `routes` module): one `strata_runtime::Contract` per contract a
//! component implements, per implementing component the routes that serve its trait and a typed
//! client.
//!
//! For every contract with an `implementedBy` component (a facade has none, and is never served):
//!
//! - a unit type named after the contract implements `strata_runtime::Contract`, with
//!   `SPEC = ContractSpec { id, path, auth, timeout, idempotent }`: `id` is the contract's `ref`
//!   (its id when it has none), `auth` its `auth` (`none` when it states none), `timeout` its
//!   `timeoutMs` ([`DEFAULT_TIMEOUT_MS`] when it states none) and `idempotent` its `idempotent`
//!   (false when it states none). Its error type implements `strata_runtime::ContractError`; a
//!   contract error travels with status [`CONTRACT_ERROR_STATUS`], since the specification
//!   states no status for it.
//! - **The path rule.** The contract is served at `binding.path` when the contract states one,
//!   and otherwise at `/<implementedBy>/<contract id>`: the implementing component is the
//!   boundary the contract is served at. A path is `/` followed by segments of ASCII letters,
//!   digits, `-`, `_`, `.` and `~`, separated by single `/`, without a trailing `/`; anything else
//!   (such as a path parameter) is refused with [`PATH_INVALID`], because every contract is a
//!   `POST` of a JSON body. A `binding.method` other than `POST` is refused with
//!   [`METHOD_UNSUPPORTED`], and two contracts at one path with [`PATH_DUPLICATE`].
//!
//! For every implementing component `<C>`:
//!
//! - `<C>Hooks`, the trait the deployment implements for what the routes need beyond the
//!   contract implementation: the record a policy with a `resource` reads
//!   (`<contract>_resource`), the quota counters (`count`, plus `<quota>_exceeded` for the
//!   payload of a refusal error that carries one and `wait` for a queueing or throttling quota),
//!   the audit log (`append_<audit>`) and the time audits record (`now`, the system clock unless
//!   overridden). Only what the component's contracts use is in it; when nothing is required,
//!   `()` implements it.
//! - `<c>_routes(routes, service, hooks)`, which adds one route per contract to a
//!   `strata_runtime::Routes`. Per call, after the runtime's authorizer and body decoding: the
//!   request's `validate()` when the contract has validations (a broken rule answers
//!   `invalid_request`); the contract's policies through `policies::authorize_<contract>` with
//!   the caller the authorizer established (a deny answers `forbidden`); every quota on the
//!   contract through `hooks.count` (a refusing quota answers with its contract error); the
//!   trait method; on success, every audit of the contract appended through
//!   `hooks.append_<audit>`. A failing hook answers `unavailable` before the call and `internal`
//!   after it.
//! - `<C>Client`, a typed client with one method per contract over `strata_runtime::Client`;
//!   a contract with a retry policy is called under it.
//!
//! The caller policies and audits read is built from the runtime's `Caller`: its subject is the
//! principal's id, and its attributes `kind`, `tenant` and `role` the rest.

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::{view, Backend};
use crate::constructs::OnExceed;
use crate::diagnostic::Diagnostic;
use crate::ir::{Component, Contract, Spec};
use crate::naming;
use crate::render::{self, Doc, Namespace, Scope, Uses};
use crate::resolve::TypeRef;

/// Code of a contract path that cannot be served.
pub const PATH_INVALID: &str = "specc.wiring-path-invalid";
/// Code of two contracts served at the same path.
pub const PATH_DUPLICATE: &str = "specc.wiring-path-duplicate";
/// Code of a `binding.method` other than `POST`.
pub const METHOD_UNSUPPORTED: &str = "specc.wiring-method-unsupported";

/// The timeout of a contract that states no `timeoutMs`, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// The HTTP status every contract error travels with.
pub const CONTRACT_ERROR_STATUS: u16 = 422;

/// The routes backend.
pub struct RoutesBackend;

/// True when the generated crate depends on `strata-runtime`: it has an implementation-level
/// machine or a contract a component implements.
pub fn uses_runtime(spec: &Spec) -> bool {
    !spec.machines.is_empty() || !served(spec).is_empty()
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

/// What the routes of one contract use beyond the trait method.
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

impl Backend for RoutesBackend {
    fn module(&self) -> &'static str {
        "routes"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        !served(spec).is_empty()
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut module = Namespace::new("module routes");
        for helper in ["principal", "refusal", "call_retry"] {
            module.claim(&mut out, "", "a helper of the module", helper);
        }
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        for c in served(spec) {
            let what = format!("contract \"{}\"", c.id);
            if let Ok(name) = naming::pascal(&c.id) {
                module.claim_type(&mut out, &c.id, &format!("the wire type of {what}"), &name);
            }
            if let Ok(name) = naming::snake(&c.id) {
                module.claim(
                    &mut out,
                    &c.id,
                    &format!("the route of {what}"),
                    &format!("serve_{name}"),
                );
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
        let wire: Vec<TokenStream> = served(spec).into_iter().map(wire_type).collect();
        let mut principal_used = false;
        let mut retry_used = false;
        let mut refusal_used = false;
        let components: Vec<TokenStream> = implementing_components(spec)
            .into_iter()
            .map(|component| {
                let tokens = component_items(spec, component, &mut uses);
                for c in contracts_of(spec, &component.id) {
                    let n = needs(spec, c);
                    principal_used |= n.principal();
                    retry_used |= n.retry.is_some();
                    refusal_used |= n.validates
                        || n.governed
                        || n.resource.is_some()
                        || !n.quotas.is_empty()
                        || !n.audits.is_empty();
                }
                tokens
            })
            .collect();
        let principal = if principal_used {
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
        let refusal = if refusal_used {
            quote! {
                /// A refusal of the runtime rather than of the contract.
                fn refusal<E>(code: strata_runtime::FailureCode, message: impl Into<String>) -> strata_runtime::Failure<E> {
                    strata_runtime::Failure::Runtime { code, message: message.into() }
                }
            }
        } else {
            TokenStream::new()
        };
        let call_retry = if retry_used {
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
        let crate_uses = uses.import();
        quote! {
            #![doc = " The contracts on the wire: one `strata_runtime::Contract` per contract a component implements, and per implementing component the routes that serve its trait, the hooks those routes need, and a typed client."]
            #![doc = ""]
            #![doc = " A contract is served at its `binding.path` when the specification states one, otherwise at `/<implementing component>/<contract id>`, always as a POST of a JSON body."]
            #crate_uses
            #(#wire)*
            #(#components)*
            #principal
            #refusal
            #call_retry
        }
    }
}

/// The unit type of a served contract and its `Contract` and `ContractError` impls.
fn wire_type(c: &Contract) -> TokenStream {
    let name = view::pascal_ident(&c.id);
    let request = view::request_path(&c.id);
    let response = view::response_path(&c.id);
    let error_name = naming::ident(&format!("{}Error", pascal(&c.id)));
    let id = c.reference.clone().unwrap_or_else(|| c.id.clone());
    let path = contract_path(c);
    let auth = c.auth.clone().unwrap_or_else(|| "none".to_string());
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
    let status = Literal::u16_unsuffixed(CONTRACT_ERROR_STATUS);
    // An error enum with no variants has no value to answer for.
    let body = if c.errors.is_empty() {
        quote! { match *self {} }
    } else {
        quote! { #status }
    };
    quote! {
        #[doc = #doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct #name;

        impl strata_runtime::Contract for #name {
            const SPEC: strata_runtime::ContractSpec = strata_runtime::ContractSpec {
                id: #id,
                path: #path,
                auth: #auth,
                timeout: std::time::Duration::from_millis(#timeout),
                idempotent: #idempotent,
            };
            type Request = #request;
            type Response = #response;
            type Error = crate::contracts::#error_name;
        }

        impl strata_runtime::ContractError for crate::contracts::#error_name {
            fn status(&self) -> u16 {
                #body
            }
        }
    }
}

/// The hooks trait, the private route functions, the routes function and the client of one
/// component.
fn component_items(spec: &Spec, component: &Component, uses: &mut Uses) -> TokenStream {
    let type_name = pascal(&component.id);
    let hooks_name = naming::ident(&format!("{type_name}Hooks"));
    let client_name = naming::ident(&format!("{type_name}Client"));
    let routes_fn = naming::ident(&format!("{}_routes", snake(&component.id)));
    let trait_name = view::pascal_ident(&component.id);
    let contracts = contracts_of(spec, &component.id);

    let mut hook_methods = Vec::new();
    let mut required = false;
    let mut wants_now = false;
    let mut wants_count = false;
    let mut wants_wait = false;
    let mut serve_fns = Vec::new();
    let mut registrations = Vec::new();
    let mut client_methods = Vec::new();

    for c in &contracts {
        let n = needs(spec, c);
        let wire = view::pascal_ident(&c.id);
        let method = naming::ident(&snake(&c.id));
        let serve_fn = naming::ident(&format!("serve_{}", snake(&c.id)));
        let request = view::request_path(&c.id);
        let response = view::response_path(&c.id);
        let error = naming::ident(&format!("{}Error", pascal(&c.id)));
        let error = quote! { crate::contracts::#error };
        let uses_hooks = n.resource.is_some() || !n.quotas.is_empty() || !n.audits.is_empty();

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
                    required = true;
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
        for q in &n.quotas {
            wants_count = true;
            required = true;
            let quota = view::upper_ident(&q.id);
            let mut waits = true;
            let exceeded = match q.on_exceed {
                OnExceed::Queue | OnExceed::Throttle => {
                    wants_wait = true;
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
                service.#method(request).await.map_err(|error| strata_runtime::Failure::Contract { error })
            }
        } else {
            wants_now = true;
            required = true;
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
                let response = service.#method(request).await.map_err(|error| strata_runtime::Failure::Contract { error })?;
                #(#appends)*
                Ok(response)
            }
        };
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
        let serve_doc = format!(" Answers one call of contract `{}`.", c.id);
        serve_fns.push(quote! {
            #[doc = #serve_doc]
            async fn #serve_fn<S: crate::contracts::#trait_name + Sync, H: #hooks_name>(
                service: &S,
                #hooks_param: &H,
                #caller_param: strata_runtime::Caller,
                request: #request,
            ) -> Result<#response, strata_runtime::Failure<#error>> {
                #(#steps)*
                #call
            }
        });
        registrations.push(quote! {
            .serve_checked::<#wire, _, _>({
                let (service, hooks) = (service.clone(), hooks.clone());
                move |caller: strata_runtime::Caller, request: #request| {
                    let (service, hooks) = (service.clone(), hooks.clone());
                    async move { #serve_fn(&*service, &*hooks, caller, request).await }
                }
            })
        });

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
            ) -> Result<#response, strata_runtime::CallError<#error>> {
                #body
            }
        });
    }

    let mut fixed_hooks = Vec::new();
    if wants_now {
        fixed_hooks.push(quote! {
            /// The time audits record. The system clock unless overridden.
            fn now(&self) -> crate::prelude::Timestamp {
                crate::prelude::Timestamp(strata_runtime::Clock::now(&strata_runtime::SystemClock).0)
            }
        });
    }
    if wants_count {
        fixed_hooks.push(quote! {
            /// Counts one call against `quota` in the scope of `caller` (`quota.scope`) and stores
            /// the counter, deciding with `quota.check(counter, now)`. An `Err` answers `unavailable`.
            fn count(
                &self,
                quota: &'static crate::quotas::Quota,
                caller: &crate::support::Caller,
            ) -> impl std::future::Future<Output = Result<crate::quotas::QuotaOutcome, String>> + Send;
        });
    }
    if wants_wait {
        fixed_hooks.push(quote! {
            /// Holds a call that a queueing or throttling `quota` let through only after
            /// `retry_after`; the call goes ahead when it returns.
            fn wait(
                &self,
                quota: &'static crate::quotas::Quota,
                retry_after: std::time::Duration,
            ) -> impl std::future::Future<Output = ()> + Send;
        });
    }
    let hooks_doc = Doc::new(&format!(
        "What the routes of component `{}` ({}) need from the deployment beyond the implementation of its contracts.",
        component.id, component.name
    ))
    .tokens();
    let unit_impl = if required {
        TokenStream::new()
    } else {
        let doc = format!(" Component `{}` needs no hooks.", component.id);
        quote! {
            #[doc = #doc]
            impl #hooks_name for () {}
        }
    };
    let routes_doc = format!(
        " Adds a route for every contract component `{}` implements to `routes`, each answered by `service` after the checks the specification states for it.",
        component.id
    );
    let client_doc = format!(
        " A typed client for the contracts component `{}` ({}) implements.",
        component.id, component.name
    );
    quote! {
        #hooks_doc
        pub trait #hooks_name: Send + Sync + 'static {
            #(#fixed_hooks)*
            #(#hook_methods)*
        }

        #unit_impl

        #(#serve_fns)*

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
            routes
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
        load(
            "s",
            &format!(
                r#"{{ "system": {{ "id": "s", "name": "S" }},
                "components": [ {{ "id": "svc", "name": "Service" }} ],
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
}
