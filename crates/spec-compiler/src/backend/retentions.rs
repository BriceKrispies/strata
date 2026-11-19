//! Retentions (`retentions` section) to the `retentions` module.
//!
//! The module holds the `RETENTIONS` table that a runtime job executes (what is kept for how long,
//! and what happens then) and, for every record with an anonymising retention, an
//! `anonymise(&mut self)` that sets the listed fields to their cleared value: `None` for an
//! optional field, an empty text, bytes or list, zero, `false` or the nil `uuid`. A field whose
//! type has no cleared value (an enum or a struct) is refused.

use proc_macro2::TokenStream;
use quote::quote;

use super::view;
use super::Backend;
use crate::constructs::{Retention, Then};
use crate::diagnostic::Diagnostic;
use crate::ir::Spec;
use crate::naming;
use crate::render::{Namespace, Uses};
use crate::resolve::TypeRef;

/// Code of a retention whose record is not declared.
pub const RECORD_UNKNOWN: &str = "specc.retention-record-unknown";
/// Code of a retention that does not state exactly one of `keepFor` and `until`.
pub const PERIOD: &str = "specc.retention-period";
/// Code of a `keepFor` that is not a usable duration.
pub const DURATION: &str = "specc.retention-duration";
/// Code of an anonymisation without fields, or fields on another action.
pub const FIELDS: &str = "specc.retention-fields";
/// Code of an anonymised field the record does not have.
pub const FIELD_UNKNOWN: &str = "specc.retention-field-unknown";
/// Code of an anonymised field whose type has no cleared value.
pub const FIELD_CLEAR: &str = "specc.retention-field-clear";
/// Code of a record with more than one retention.
pub const DUPLICATE: &str = "specc.retention-duplicate";
/// Code of an archive target that is not a component.
pub const ARCHIVE_TARGET_UNKNOWN: &str = "specc.retention-archive-target-unknown";
/// Code of an archiving retention without `archiveTo`.
pub const ARCHIVE_TARGET_MISSING: &str = "ready.retention-archive-target-missing";

/// The retentions backend.
pub struct RetentionsBackend;

impl Backend for RetentionsBackend {
    fn module(&self) -> &'static str {
        "retentions"
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module retentions");
        for n in ["Period", "Action", "Retention", "RETENTIONS", "for_record"] {
            names.claim(&mut out, "", "a type or item of the module", n);
        }
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for t in &spec.constructs.retentions {
            let what = format!("retention \"{}\"", t.id);
            match naming::upper_snake(&t.id) {
                Ok(name) => names.claim(&mut out, &t.id, &what, &name),
                Err(reason) => out.push(Diagnostic::new(
                    crate::render::NAME_INVALID,
                    &t.id,
                    format!("{what} cannot become a Rust name: {reason}."),
                )),
            }
            check_retention(spec, t, &mut out);
            if let Some(record) = &t.record {
                if let Some((_, first)) = seen.iter().find(|(r, _)| r == record) {
                    out.push(Diagnostic::new(
                        DUPLICATE,
                        &t.id,
                        format!("Record \"{record}\" already has retention \"{first}\"; a record has at most one."),
                    ));
                } else {
                    seen.push((record, &t.id));
                }
            }
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut uses = Uses::default();
        let consts: Vec<TokenStream> = spec
            .constructs
            .retentions
            .iter()
            .map(retention_const)
            .collect();
        let names: Vec<_> = spec
            .constructs
            .retentions
            .iter()
            .map(|t| view::upper_ident(&t.id))
            .collect();
        let impls: Vec<TokenStream> = spec
            .constructs
            .retentions
            .iter()
            .filter(|t| t.then == Then::Anonymise)
            .filter_map(|t| anonymise_impl(spec, t, &mut uses))
            .collect();
        let crate_uses = uses.import();
        quote! {
            #![doc = " How long the specification's records are kept, and what happens when the keeping ends."]
            use std::time::Duration;

            #crate_uses

            /// When the keeping of a record ends.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Period {
                /// This long after the record was created.
                KeepFor(Duration),
                /// When the event with this id occurs for the record.
                Until(&'static str),
            }

            /// What happens to a record when its keeping ends.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub enum Action {
                /// The record is deleted.
                Delete,
                /// The listed fields are cleared; see the record's `anonymise`.
                Anonymise,
                /// The record is moved to another component.
                Archive,
            }

            /// The retention of one record.
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
            pub struct Retention {
                /// The retention's id in the specification.
                pub id: &'static str,
                /// The id of the record it applies to.
                pub record: &'static str,
                /// When the keeping ends.
                pub period: Period,
                /// What happens then.
                pub then: Action,
                /// The fields an anonymisation clears, by their specification names.
                pub fields: &'static [&'static str],
                /// The id of the component an archive goes to.
                pub archive_to: Option<&'static str>,
            }

            #(#consts)*

            /// Every retention, in declaration order.
            pub const RETENTIONS: &[Retention] = &[#(#names),*];

            /// The retention of a record, by the record's id.
            pub fn for_record(record: &str) -> Option<&'static Retention> {
                RETENTIONS.iter().find(|r| r.record == record)
            }

            #(#impls)*
        }
    }
}

fn check_retention(spec: &Spec, t: &Retention, out: &mut Vec<Diagnostic>) {
    let record = match &t.record {
        Some(r) => match spec.record(r) {
            Some(record) => Some(record),
            None => {
                out.push(Diagnostic::new(
                    RECORD_UNKNOWN,
                    &t.id,
                    format!(
                        "Retention \"{}\" is about \"{r}\", which is not a record.",
                        t.id
                    ),
                ));
                None
            }
        },
        None => {
            out.push(Diagnostic::new(
                RECORD_UNKNOWN,
                &t.id,
                format!("Retention \"{}\" names no record.", t.id),
            ));
            None
        }
    };
    match (&t.keep_for, &t.until) {
        (Some(text), None) => {
            if let Err(why) = view::duration_ns(text) {
                out.push(Diagnostic::new(
                    DURATION,
                    &t.id,
                    format!(
                        "The \"keepFor\" of retention \"{}\" cannot be generated: {why}.",
                        t.id
                    ),
                ));
            }
        }
        (None, Some(_)) => {}
        _ => out.push(Diagnostic::new(
            PERIOD,
            &t.id,
            format!(
                "Retention \"{}\" must state exactly one of \"keepFor\" and \"until\".",
                t.id
            ),
        )),
    }
    if t.then == Then::Anonymise {
        if t.fields.is_empty() {
            out.push(Diagnostic::new(
                FIELDS,
                &t.id,
                format!(
                    "Retention \"{}\" anonymises, so it names the fields it clears.",
                    t.id
                ),
            ));
        }
        if let Some(record) = record {
            for name in &t.fields {
                match record.field(name) {
                    None => out.push(Diagnostic::new(
                        FIELD_UNKNOWN,
                        &t.id,
                        format!("Record \"{}\" has no field \"{name}\".", record.id),
                    )),
                    Some(f) => {
                        let clearable = f.optional
                            || view::zero_value(
                                spec,
                                &TypeRef::parse(&f.type_name),
                                &mut Uses::default(),
                            )
                            .is_some();
                        if !clearable {
                            out.push(Diagnostic::new(
                                FIELD_CLEAR,
                                &t.id,
                                format!(
                                    "Field \"{name}\" of record \"{}\" has type \"{}\", which has no cleared value; only an optional field, text, bytes, a list, a number, a bool or a uuid can be cleared.",
                                    record.id, f.type_name
                                ),
                            ));
                        }
                    }
                }
            }
        }
    } else if !t.fields.is_empty() {
        out.push(Diagnostic::new(
            FIELDS,
            &t.id,
            format!(
                "Retention \"{}\" does not anonymise, so it takes no \"fields\".",
                t.id
            ),
        ));
    }
    if t.then == Then::Archive {
        match &t.archive_to {
            None => out.push(Diagnostic::new(
                ARCHIVE_TARGET_MISSING,
                &t.id,
                format!(
                    "Retention \"{}\" archives but names no component in \"archiveTo\".",
                    t.id
                ),
            )),
            Some(c) if spec.component(c).is_none() => out.push(Diagnostic::new(
                ARCHIVE_TARGET_UNKNOWN,
                &t.id,
                format!(
                    "Retention \"{}\" archives to \"{c}\", which is not a component.",
                    t.id
                ),
            )),
            Some(_) => {}
        }
    } else if t.archive_to.is_some() {
        out.push(Diagnostic::new(
            ARCHIVE_TARGET_UNKNOWN,
            &t.id,
            format!(
                "Retention \"{}\" does not archive, so it takes no \"archiveTo\".",
                t.id
            ),
        ));
    }
}

fn retention_const(t: &Retention) -> TokenStream {
    let name = view::upper_ident(&t.id);
    let id = &t.id;
    let record = t.record.clone().unwrap_or_default();
    let period = match (&t.keep_for, &t.until) {
        (Some(text), _) => {
            let duration = view::duration_tokens(view::duration_ns(text).unwrap_or(1));
            quote! { Period::KeepFor(#duration) }
        }
        (None, Some(event)) => quote! { Period::Until(#event) },
        (None, None) => quote! { Period::Until("") },
    };
    let then = match t.then {
        Then::Delete => quote! { Action::Delete },
        Then::Anonymise => quote! { Action::Anonymise },
        Then::Archive => quote! { Action::Archive },
    };
    let fields = &t.fields;
    let archive_to = match &t.archive_to {
        Some(c) => quote! { Some(#c) },
        None => quote! { None },
    };
    let doc = format!(" Retention `{}` of record `{}`.", t.id, record);
    quote! {
        #[doc = #doc]
        pub const #name: Retention = Retention {
            id: #id,
            record: #record,
            period: #period,
            then: #then,
            fields: &[#(#fields),*],
            archive_to: #archive_to,
        };
    }
}

fn anonymise_impl(spec: &Spec, t: &Retention, uses: &mut Uses) -> Option<TokenStream> {
    let record = spec.record(t.record.as_deref()?)?;
    let mut statements = Vec::new();
    for name in &t.fields {
        let f = record.field(name)?;
        let ident = view::snake_ident(name);
        let value = if f.optional {
            quote! { None }
        } else {
            view::zero_value(spec, &TypeRef::parse(&f.type_name), uses)?
        };
        statements.push(quote! { self.#ident = #value; });
    }
    let owner = view::record_path(&record.id);
    let doc = format!(
        " Clears the fields retention `{}` clears: {}.",
        t.id,
        t.fields
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(quote! {
        impl #owner {
            #[doc = #doc]
            pub fn anonymise(&mut self) {
                #(#statements)*
            }
        }
    })
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
                "components": [ {{ "id": "vault", "name": "Vault" }} ],
                "records": [ {{ "id": "rec", "name": "R", "owner": "svc", "key": ["id"], "schemaVersion": 1,
                    "fields": [ {{ "name": "id", "type": "u32" }}, {{ "name": "email", "type": "string" }},
                                {{ "name": "nick", "type": "string", "optional": true }}, {{ "name": "state", "type": "mode" }} ] }} ],
                "types": [ {{ "id": "mode", "name": "Mode", "kind": "enum", "values": ["a"] }} ],
                {extra} }}"#
            ),
        )
        .unwrap()
    }

    fn codes(s: &Spec) -> Vec<String> {
        RetentionsBackend
            .check(s)
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn an_anonymising_retention_gets_an_anonymise_method() {
        let s = spec(
            r#""retentions": [ { "id": "r-anon", "record": "rec", "keepFor": "30 d", "then": "anonymise", "fields": ["email", "nick"] } ]"#,
        );
        assert!(codes(&s).is_empty());
        let text = RetentionsBackend.emit(&s).to_string();
        assert!(text.contains("pub fn anonymise (& mut self)"));
        assert!(text.contains("self . email = String :: new ()"));
        assert!(text.contains("self . nick = None"));
        assert!(text.contains("from_secs (2592000)"));
    }

    #[test]
    fn retentions_that_cannot_be_generated_are_refused() {
        let s = spec(
            r#""retentions": [
              { "id": "a", "record": "rec", "keepFor": "1 d", "then": "anonymise", "fields": ["state"] },
              { "id": "b", "record": "rec", "keepFor": "1 d", "until": "closed", "then": "delete" },
              { "id": "c", "record": "ghost", "keepFor": "soon", "then": "archive" },
              { "id": "d", "record": "rec", "until": "closed", "then": "archive", "archiveTo": "nowhere" } ]"#,
        );
        let got = codes(&s);
        for code in [
            FIELD_CLEAR,
            DUPLICATE,
            PERIOD,
            RECORD_UNKNOWN,
            DURATION,
            ARCHIVE_TARGET_MISSING,
            ARCHIVE_TARGET_UNKNOWN,
        ] {
            assert!(got.iter().any(|c| c == code), "{code} in {got:?}");
        }
    }
}
