//! Retries (`retries` section) to the `retries` module.
//!
//! Each retry is a `RetryPolicy` constant for a contract or an event: the attempts (the first
//! included), the backoff, the idempotency key and the time one attempt may take. The policy is
//! data plus one pure function, `RetryPolicy::wait_after`, which the caller's retry loop asks;
//! `for_contract` and `for_event` find the policy of a target. The runtime's client takes the
//! same numbers as its `CallRetry`.

use proc_macro2::{Literal, TokenStream};
use quote::quote;

use super::view;
use super::Backend;
use crate::constructs::{BackoffKind, Retry};
use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::naming;
use crate::render::Namespace;

/// Code of a retry that does not name exactly one of `contract` and `event`.
pub const TARGET: &str = "specc.retry-target";
/// Code of a retry on a contract that is not declared.
pub const CONTRACT_UNKNOWN: &str = "specc.retry-contract-unknown";
/// Code of an attempts count that is not a positive integer of 32 bits.
pub const ATTEMPTS: &str = "specc.retry-attempts";
/// Code of a duration that is not usable.
pub const DURATION: &str = "specc.retry-duration";
/// Code of a backoff that is incomplete or inconsistent.
pub const BACKOFF: &str = "specc.retry-backoff";
/// Code of an idempotency key that is not an input of the contract.
pub const KEY_UNKNOWN: &str = "specc.retry-idempotency-key-unknown";

/// The retries backend.
pub struct RetriesBackend;

impl Backend for RetriesBackend {
    fn module(&self) -> &'static str {
        "retries"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module retries");
        for n in [
            "Target",
            "Backoff",
            "RetryPolicy",
            "ALL",
            "for_contract",
            "for_event",
        ] {
            names.claim(&mut out, "", "an item of the module", n);
        }
        for y in &spec.constructs.retries {
            let what = format!("retry \"{}\"", y.id);
            match naming::upper_snake(&y.id) {
                Ok(name) => names.claim(&mut out, &y.id, &what, &name),
                Err(reason) => out.push(Diagnostic::new(
                    crate::render::NAME_INVALID,
                    &y.id,
                    format!("{what} cannot become a Rust name: {reason}."),
                )),
            }
            check_retry(spec, y, &mut out);
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let consts: Vec<TokenStream> = spec.constructs.retries.iter().map(retry_const).collect();
        let names: Vec<_> = spec
            .constructs
            .retries
            .iter()
            .map(|y| view::upper_ident(&y.id))
            .collect();
        quote! {
            #![doc = " How a contract call or an emitted event is repeated when an attempt fails."]
            use std::time::Duration;

            /// What a retry policy repeats.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Target {
                /// Calls of the contract with this id.
                Contract(&'static str),
                /// Emissions of the event with this id.
                Event(&'static str),
            }

            /// How long to wait between attempts.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Backoff {
                /// Every wait is `base`.
                Fixed {
                    /// The wait before every further attempt.
                    base: Duration,
                },
                /// The first wait is `base` and each later wait doubles, up to `cap`.
                Exponential {
                    /// The wait before the second attempt.
                    base: Duration,
                    /// The longest wait, if the specification limits it.
                    cap: Option<Duration>,
                },
            }

            /// When and how often an attempt is repeated.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct RetryPolicy {
                /// The policy's id in the specification.
                pub id: &'static str,
                /// What it repeats.
                pub target: Target,
                /// The most attempts, the first included.
                pub attempts: u32,
                /// How long to wait between attempts.
                pub backoff: Backoff,
                /// The input or payload field that identifies a repeat, by its specification name.
                pub idempotency_key: Option<&'static str>,
                /// How long one attempt may take.
                pub timeout: Option<Duration>,
            }

            impl RetryPolicy {
                /// The wait before the next attempt after `failures` attempts have failed, or
                /// `None` when no attempt is left (or none has failed).
                pub fn wait_after(&self, failures: u32) -> Option<Duration> {
                    if failures == 0 || failures >= self.attempts {
                        return None;
                    }
                    Some(match self.backoff {
                        Backoff::Fixed { base } => base,
                        Backoff::Exponential { base, cap } => {
                            let doublings = (failures - 1).min(31);
                            let wait = base.saturating_mul(1u32 << doublings);
                            cap.map_or(wait, |cap| wait.min(cap))
                        }
                    })
                }
            }

            #(#consts)*

            /// Every retry policy, in declaration order.
            pub const ALL: &[RetryPolicy] = &[#(#names),*];

            /// The retry policy of a contract, by the contract's id.
            pub fn for_contract(contract: &str) -> Option<&'static RetryPolicy> {
                ALL.iter()
                    .find(|p| matches!(p.target, Target::Contract(id) if id == contract))
            }

            /// The retry policy of an event, by the event's id.
            pub fn for_event(event: &str) -> Option<&'static RetryPolicy> {
                ALL.iter()
                    .find(|p| matches!(p.target, Target::Event(id) if id == event))
            }
        }
    }
}

fn check_retry(spec: &Spec, y: &Retry, out: &mut Vec<Diagnostic>) {
    match (&y.contract, &y.event) {
        (Some(c), None) => match spec.contracts.iter().find(|x| &x.id == c) {
            None => out.push(Diagnostic::new(
                CONTRACT_UNKNOWN,
                &y.id,
                format!(
                    "Retry \"{}\" repeats \"{c}\", which is not a contract.",
                    y.id
                ),
            )),
            Some(contract) => {
                if let Some(key) = &y.idempotency_key {
                    if !contract.inputs.iter().any(|f| &f.name == key) {
                        out.push(Diagnostic::new(
                            KEY_UNKNOWN,
                            &y.id,
                            format!("\"{key}\" is not an input of contract \"{c}\"."),
                        ));
                    }
                }
            }
        },
        (None, Some(_)) => {}
        _ => out.push(Diagnostic::new(
            TARGET,
            &y.id,
            format!(
                "Retry \"{}\" must name exactly one of \"contract\" and \"event\".",
                y.id
            ),
        )),
    }
    if y.attempts
        .as_ref()
        .and_then(serde_json::Number::as_u64)
        .is_none_or(|n| n < 1 || n > u64::from(u32::MAX))
    {
        out.push(Diagnostic::new(
            ATTEMPTS,
            &y.id,
            format!(
                "The attempts of retry \"{}\" must be a positive integer of at most {}.",
                y.id,
                u32::MAX
            ),
        ));
    }
    let duration = |what: &str, text: &Option<String>, out: &mut Vec<Diagnostic>| {
        let text = text.as_deref()?;
        match view::duration_ns(text) {
            Ok(ns) => Some(ns),
            Err(why) => {
                out.push(Diagnostic::new(
                    DURATION,
                    &y.id,
                    format!(
                        "The {what} of retry \"{}\" cannot be generated: {why}.",
                        y.id
                    ),
                ));
                None
            }
        }
    };
    let base = duration("backoff base", &y.base, out);
    let cap = duration("backoff cap", &y.cap, out);
    duration("timeout", &y.timeout, out);
    if y.base.is_none() {
        out.push(Diagnostic::new(
            BACKOFF,
            &y.id,
            format!("Retry \"{}\" states no backoff base.", y.id),
        ));
    }
    if y.cap.is_some() && y.backoff != Some(BackoffKind::Exponential) {
        out.push(Diagnostic::new(
            BACKOFF,
            &y.id,
            format!("Retry \"{}\" waits a fixed time, which has no cap.", y.id),
        ));
    }
    if let (Some(base), Some(cap)) = (base, cap) {
        if cap < base {
            out.push(Diagnostic::new(
                BACKOFF,
                &y.id,
                format!(
                    "The backoff cap of retry \"{}\" is shorter than its base.",
                    y.id
                ),
            ));
        }
    }
}

fn retry_const(y: &Retry) -> TokenStream {
    let name = view::upper_ident(&y.id);
    let id = &y.id;
    let target = match (&y.contract, &y.event) {
        (Some(c), _) => quote! { Target::Contract(#c) },
        (None, Some(e)) => quote! { Target::Event(#e) },
        (None, None) => quote! { Target::Event("") },
    };
    let attempts = Literal::u32_unsuffixed(
        y.attempts
            .as_ref()
            .and_then(serde_json::Number::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(1),
    );
    let duration = |text: &Option<String>| {
        text.as_deref()
            .and_then(|t| view::duration_ns(t).ok())
            .map(view::duration_tokens)
    };
    let base = duration(&y.base).unwrap_or_else(|| view::duration_tokens(1_000_000));
    let backoff = match y.backoff {
        Some(BackoffKind::Exponential) => {
            let cap = match duration(&y.cap) {
                Some(c) => quote! { Some(#c) },
                None => quote! { None },
            };
            quote! { Backoff::Exponential { base: #base, cap: #cap } }
        }
        _ => quote! { Backoff::Fixed { base: #base } },
    };
    let key = match &y.idempotency_key {
        Some(k) => quote! { Some(#k) },
        None => quote! { None },
    };
    let timeout = match duration(&y.timeout) {
        Some(t) => quote! { Some(#t) },
        None => quote! { None },
    };
    let doc = format!(" Retry policy `{}`.", y.id);
    quote! {
        #[doc = #doc]
        pub const #name: RetryPolicy = RetryPolicy {
            id: #id,
            target: #target,
            attempts: #attempts,
            backoff: #backoff,
            idempotency_key: #key,
            timeout: #timeout,
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
                    "inputs": [ {{ "name": "token", "type": "string" }} ] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    fn codes(s: &Spec) -> Vec<String> {
        RetriesBackend
            .check(s)
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn a_retry_is_a_policy_constant() {
        let s = spec(
            r#""retries": [
              { "id": "call-retry", "contract": "do-it", "attempts": 4, "backoff": { "kind": "exponential", "base": "100 ms", "cap": "2 s" }, "idempotencyKey": "token", "timeout": "5 s" },
              { "id": "emit-retry", "event": "sent", "attempts": 2, "backoff": { "kind": "fixed", "base": "1 s" } } ]"#,
        );
        assert!(codes(&s).is_empty());
        let text = RetriesBackend.emit(&s).to_string();
        assert!(text.contains("pub const CALL_RETRY : RetryPolicy"));
        assert!(text.contains("Backoff :: Exponential"));
        assert!(text.contains("Target :: Event (\"sent\")"));
        assert!(text.contains("from_millis (100)"));
    }

    #[test]
    fn retries_that_cannot_be_generated_are_refused() {
        let s = spec(
            r#""retries": [
              { "id": "a", "contract": "do-it", "attempts": 0, "backoff": { "kind": "fixed", "base": "1 s", "cap": "2 s" } },
              { "id": "b", "contract": "do-it", "attempts": 2, "backoff": { "kind": "exponential", "base": "5 s", "cap": "1 s" }, "idempotencyKey": "nope" },
              { "id": "c", "attempts": 2, "backoff": { "kind": "fixed", "base": "later" } },
              { "id": "d", "contract": "ghost", "attempts": 2, "backoff": { "kind": "fixed", "base": "1 s" } } ]"#,
        );
        let got = codes(&s);
        for code in [
            ATTEMPTS,
            BACKOFF,
            KEY_UNKNOWN,
            TARGET,
            DURATION,
            CONTRACT_UNKNOWN,
        ] {
            assert!(got.iter().any(|c| c == code), "{code} in {got:?}");
        }
    }
}
