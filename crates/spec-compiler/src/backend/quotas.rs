//! Quotas (`quotas` section) to the `quotas` module.
//!
//! Each quota is a `Quota` constant, and `Quota::check(counter, now)` is the pure decision a
//! runtime counter store applies: a fixed window that starts with the first counted call or
//! event, allows `limit` of them, and starts again when the window has passed. The caller keeps
//! the counter per scope (`tenant`, `principal` or `source-address`) and stores the counter the
//! outcome returns. A refused call is not counted. The error a refusal answers with is named in
//! `Quota::error`, the contract error the specification chose.

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::view;
use super::Backend;
use crate::constructs::{OnExceed, Quota, QuotaScope};
use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::naming;
use crate::render::Namespace;

/// Code of a quota that does not name exactly one of `contract` and `event`.
pub const TARGET: &str = "specc.quota-target";
/// Code of a quota on a contract that is not declared.
pub const CONTRACT_UNKNOWN: &str = "specc.quota-contract-unknown";
/// Code of a limit that is not a positive integer.
pub const LIMIT: &str = "specc.quota-limit";
/// Code of a window that is not a usable duration.
pub const WINDOW: &str = "specc.quota-window";
/// Code of a refusal error that is missing, unknown or misplaced.
pub const ERROR: &str = "specc.quota-error";

/// The quotas backend.
pub struct QuotasBackend;

impl Backend for QuotasBackend {
    fn module(&self) -> &'static str {
        "quotas"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module quotas");
        for n in [
            "Scope",
            "OnExceed",
            "Target",
            "Quota",
            "Counter",
            "QuotaOutcome",
            "QUOTAS",
            "for_contract",
            "for_event",
        ] {
            names.claim(&mut out, "", "an item of the module", n);
        }
        for q in &spec.constructs.quotas {
            let what = format!("quota \"{}\"", q.id);
            match naming::upper_snake(&q.id) {
                Ok(name) => names.claim(&mut out, &q.id, &what, &name),
                Err(reason) => out.push(Diagnostic::new(
                    crate::render::NAME_INVALID,
                    &q.id,
                    format!("{what} cannot become a Rust name: {reason}."),
                )),
            }
            check_quota(spec, q, &mut out);
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let consts: Vec<TokenStream> = spec.constructs.quotas.iter().map(quota_const).collect();
        let names: Vec<_> = spec
            .constructs
            .quotas
            .iter()
            .map(|q| view::upper_ident(&q.id))
            .collect();
        quote! {
            #![doc = " How often a scope may call a contract or cause an event, and the pure decision that applies it."]
            use std::time::Duration;

            use crate::prelude::Timestamp;

            /// Whom a quota counts.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Scope {
                /// Each tenant has its own count.
                Tenant,
                /// Each principal has its own count.
                Principal,
                /// Each source address has its own count.
                SourceAddress,
            }

            /// What happens to a call or event beyond the limit.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum OnExceed {
                /// It is refused.
                Refuse,
                /// It waits until the window has passed.
                Queue,
                /// It is slowed down.
                Throttle,
            }

            /// What a quota limits.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Target {
                /// Calls of the contract with this id.
                Contract(&'static str),
                /// Occurrences of the event with this id.
                Event(&'static str),
            }

            /// A quota: at most `limit` calls or events per scope in each `window`.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct Quota {
                /// The quota's id in the specification.
                pub id: &'static str,
                /// Whom it counts.
                pub scope: Scope,
                /// What it limits.
                pub target: Target,
                /// The most calls or events in one window.
                pub limit: u64,
                /// The length of the window.
                pub window: Duration,
                /// What happens beyond the limit.
                pub on_exceed: OnExceed,
                /// The contract error a refusal answers with, when the quota refuses calls.
                pub error: Option<&'static str>,
                /// The id of the component that keeps the counters.
                pub held_by: Option<&'static str>,
            }

            /// The state of one scope's count: when its window started and how many were counted.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct Counter {
                /// When the current window started.
                pub window_start: Timestamp,
                /// How many calls or events the window has counted.
                pub count: u64,
            }

            /// What counting one more call or event comes to.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum QuotaOutcome {
                /// Within the limit; store this counter.
                Allowed(Counter),
                /// Beyond the limit; nothing was counted.
                Exceeded {
                    /// How long until the window ends and counting starts again.
                    retry_after: Duration,
                    /// The unchanged counter.
                    counter: Counter,
                },
            }

            impl Quota {
                /// Counts one call or event at `now` against the scope's `counter` (`None` when
                /// the scope has none yet). The window is fixed: it starts at its first count.
                pub fn check(&self, counter: Option<Counter>, now: Timestamp) -> QuotaOutcome {
                    let window = i64::try_from(self.window.as_millis())
                        .unwrap_or(i64::MAX)
                        .max(1);
                    let current = match counter {
                        Some(c)
                            if now >= c.window_start
                                && now.0.saturating_sub(c.window_start.0) < window =>
                        {
                            c
                        }
                        _ => Counter {
                            window_start: now,
                            count: 0,
                        },
                    };
                    if current.count < self.limit {
                        QuotaOutcome::Allowed(Counter {
                            window_start: current.window_start,
                            count: current.count + 1,
                        })
                    } else {
                        let elapsed = now.0.saturating_sub(current.window_start.0);
                        let remaining = u64::try_from(window.saturating_sub(elapsed)).unwrap_or(0);
                        QuotaOutcome::Exceeded {
                            retry_after: Duration::from_millis(remaining),
                            counter: current,
                        }
                    }
                }
            }

            #(#consts)*

            /// Every quota, in declaration order.
            pub const QUOTAS: &[Quota] = &[#(#names),*];

            /// The quotas on a contract, by the contract's id.
            pub fn for_contract(contract: &str) -> impl Iterator<Item = &'static Quota> + '_ {
                QUOTAS
                    .iter()
                    .filter(move |q| matches!(q.target, Target::Contract(id) if id == contract))
            }

            /// The quotas on an event, by the event's id.
            pub fn for_event(event: &str) -> impl Iterator<Item = &'static Quota> + '_ {
                QUOTAS
                    .iter()
                    .filter(move |q| matches!(q.target, Target::Event(id) if id == event))
            }
        }
    }
}

fn check_quota(spec: &Spec, q: &Quota, out: &mut Vec<Diagnostic>) {
    let contract = match (&q.contract, &q.event) {
        (Some(c), None) => match spec.contracts.iter().find(|x| &x.id == c) {
            Some(contract) => Some(contract),
            None => {
                out.push(Diagnostic::new(
                    CONTRACT_UNKNOWN,
                    &q.id,
                    format!(
                        "Quota \"{}\" limits \"{c}\", which is not a contract.",
                        q.id
                    ),
                ));
                None
            }
        },
        (None, Some(_)) => None,
        _ => {
            out.push(Diagnostic::new(
                TARGET,
                &q.id,
                format!(
                    "Quota \"{}\" must name exactly one of \"contract\" and \"event\".",
                    q.id
                ),
            ));
            None
        }
    };
    if q.limit
        .as_ref()
        .and_then(serde_json::Number::as_u64)
        .is_none_or(|n| n < 1)
    {
        out.push(Diagnostic::new(
            LIMIT,
            &q.id,
            format!(
                "The limit of quota \"{}\" must be a positive integer.",
                q.id
            ),
        ));
    }
    match q.window.as_deref().map(view::duration_ns) {
        Some(Ok(_)) => {}
        Some(Err(why)) => out.push(Diagnostic::new(
            WINDOW,
            &q.id,
            format!(
                "The window of quota \"{}\" cannot be generated: {why}.",
                q.id
            ),
        )),
        None => out.push(Diagnostic::new(
            WINDOW,
            &q.id,
            format!("Quota \"{}\" states no window.", q.id),
        )),
    }
    let refuses_calls = q.on_exceed == OnExceed::Refuse && q.contract.is_some();
    match (&q.error, refuses_calls) {
        (None, true) => out.push(Diagnostic::new(
            ERROR,
            &q.id,
            format!(
                "Quota \"{}\" refuses calls, so it names the contract error the refusal is in \"error\".",
                q.id
            ),
        )),
        (Some(e), true) => {
            if contract.is_some_and(|c| !c.errors.iter().any(|x| &x.name == e)) {
                out.push(Diagnostic::new(
                    ERROR,
                    &q.id,
                    format!("Contract \"{}\" has no error \"{e}\".", q.contract.clone().unwrap_or_default()),
                ));
            }
        }
        (Some(_), false) => out.push(Diagnostic::new(
            ERROR,
            &q.id,
            format!("Quota \"{}\" takes an \"error\" only when it refuses calls of a contract.", q.id),
        )),
        (None, false) => {}
    }
}

fn quota_const(q: &Quota) -> TokenStream {
    let name = view::upper_ident(&q.id);
    let id = &q.id;
    let scope = match q.scope {
        QuotaScope::Tenant => quote! { Scope::Tenant },
        QuotaScope::Principal => quote! { Scope::Principal },
        QuotaScope::SourceAddress => quote! { Scope::SourceAddress },
    };
    let target = match (&q.contract, &q.event) {
        (Some(c), _) => quote! { Target::Contract(#c) },
        (None, Some(e)) => quote! { Target::Event(#e) },
        (None, None) => quote! { Target::Event("") },
    };
    let limit = Literal::u64_unsuffixed(
        q.limit
            .as_ref()
            .and_then(serde_json::Number::as_u64)
            .unwrap_or(1),
    );
    let window = view::duration_tokens(
        q.window
            .as_deref()
            .and_then(|w| view::duration_ns(w).ok())
            .unwrap_or(1_000_000),
    );
    let on_exceed = match q.on_exceed {
        OnExceed::Refuse => quote! { OnExceed::Refuse },
        OnExceed::Queue => quote! { OnExceed::Queue },
        OnExceed::Throttle => quote! { OnExceed::Throttle },
    };
    let option = |v: &Option<String>| match v {
        Some(s) => quote! { Some(#s) },
        None => quote! { None },
    };
    let error = option(&q.error);
    let held_by = option(&q.held_by);
    let doc = format!(" Quota `{}`.", q.id);
    quote! {
        #[doc = #doc]
        pub const #name: Quota = Quota {
            id: #id,
            scope: #scope,
            target: #target,
            limit: #limit,
            window: #window,
            on_exceed: #on_exceed,
            error: #error,
            held_by: #held_by,
        };
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
                "contracts": [ {{ "id": "do-it", "name": "Do it", "realisation": "generated",
                    "errors": [ {{ "name": "too-many" }} ] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    #[test]
    fn a_quota_is_a_constant_with_a_check() {
        let s = spec(
            r#""quotas": [ { "id": "per-tenant", "scope": "tenant", "contract": "do-it", "limit": 100, "window": "1 min", "onExceed": "refuse", "error": "too-many", "heldBy": "gate" },
                           { "id": "events", "scope": "principal", "event": "ev", "limit": 5, "window": "10 s", "onExceed": "throttle" } ]"#,
        );
        assert!(QuotasBackend.check(&s).is_empty());
        let text = QuotasBackend.emit(&s).to_string();
        assert!(text.contains("pub const PER_TENANT : Quota"));
        assert!(text.contains("Target :: Contract (\"do-it\")"));
        assert!(text.contains("error : Some (\"too-many\")"));
        assert!(text.contains("from_secs (60)"));
    }

    #[test]
    fn quotas_that_cannot_be_generated_are_refused() {
        let s = spec(
            r#""quotas": [
              { "id": "a", "scope": "tenant", "contract": "do-it", "limit": 0, "window": "soon", "onExceed": "refuse" },
              { "id": "b", "scope": "tenant", "contract": "do-it", "limit": 1, "window": "1 s", "onExceed": "refuse", "error": "nope" },
              { "id": "c", "scope": "tenant", "limit": 1, "window": "1 s", "onExceed": "queue" },
              { "id": "d", "scope": "tenant", "contract": "ghost", "limit": 1, "window": "1 s", "onExceed": "queue", "error": "x" } ]"#,
        );
        let got: Vec<String> = QuotasBackend
            .check(&s)
            .into_iter()
            .map(|d| d.code)
            .collect();
        for code in [LIMIT, WINDOW, ERROR, TARGET, CONTRACT_UNKNOWN] {
            assert!(got.iter().any(|c| c == code), "{code} in {got:?}");
        }
    }
}
