//! Telemetry (`system.telemetry`, `event.logging`) to the `telemetry` module.
//!
//! For each logged event the module has a struct holding exactly what its line may hold (the
//! correlation id, the tenant field and the allowlisted payload fields) and one function that
//! writes the line through `strata_runtime::Telemetry`. Each service of `system.telemetry` gets a
//! `JsonLines` constant naming it. A line is `ts, level, service, event`, then `correlation`
//! (left out when empty), `tenant` and the allowlisted fields under the names they are written
//! as; an optional field that is unset is left out. A payload is never logged whole: the struct
//! has no other field to put it in.
//!
//! Implementation-level machines call these functions for the logged events they emit
//! (`Machine::log_emitted`, see [`emitted_arm`]); hand-written code calls them for the domain
//! events it produces.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::Backend;
use crate::diagnostic::Diagnostic;
use crate::ir::{Event, Logging, Spec, TypeKind};
use crate::naming;
use crate::render::{self, Doc, Namespace};
use crate::resolve::{Builtin, TypeRef};

/// Code of a logged event whose service `system.telemetry` does not declare.
pub const SERVICE_UNKNOWN: &str = "specc.telemetry-service-unknown";
/// Code of a logged field (or tenant field) that is not a payload field of its event.
pub const FIELD_UNKNOWN: &str = "specc.telemetry-field-unknown";
/// Code of a logged field that is not a scalar: a list, a struct, bytes or json.
pub const FIELD_TYPE: &str = "specc.telemetry-field-type";
/// Code of a logged field that is marked sensitive or of a sensitive type.
pub const FIELD_SENSITIVE: &str = "specc.telemetry-field-sensitive";
/// Code of a level that is not debug, info, warn or error.
pub const LEVEL: &str = "specc.telemetry-level";

/// The telemetry backend.
pub struct TelemetryBackend;

/// True when the crate has a telemetry module: a service is declared or an event is logged.
pub fn wanted(spec: &Spec) -> bool {
    !spec.telemetry.services.is_empty() || spec.events.iter().any(|e| e.logging.is_some())
}

/// How a logged value is passed to its logging function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repr {
    Bool,
    U32,
    U64,
    I64,
    F64,
    Str,
    Uuid,
}

impl Repr {
    fn param(self) -> TokenStream {
        match self {
            Repr::Bool => quote! { bool },
            Repr::U32 => quote! { u32 },
            Repr::U64 => quote! { u64 },
            Repr::I64 => quote! { i64 },
            Repr::F64 => quote! { f64 },
            Repr::Str => quote! { &'a str },
            Repr::Uuid => quote! { uuid::Uuid },
        }
    }
}

/// One member of a line after the correlation: its Rust field, the name it is written as, how it
/// is passed, whether it may be unset, and the payload field it comes from.
struct Member {
    field: String,
    written_as: String,
    rust: proc_macro2::Ident,
    repr: Repr,
    optional: bool,
    type_ref: TypeRef,
}

/// The representation of a scalar type, or `None` when a line cannot hold it.
fn repr_of(spec: &Spec, t: &TypeRef) -> Option<Repr> {
    match t {
        TypeRef::Builtin(b) => builtin_repr(*b),
        TypeRef::Declared(id) => match &spec.type_decl(id)?.kind {
            TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                builtin_repr(Builtin::parse(repr.as_deref()?)?)
            }
            TypeKind::Enum { .. } => Some(Repr::Str),
            TypeKind::Struct { .. } | TypeKind::Union { .. } => None,
        },
        TypeRef::List(_)
        | TypeRef::Set(_)
        | TypeRef::Option(_)
        | TypeRef::Map(..)
        | TypeRef::Shared(_) => None,
    }
}

fn builtin_repr(b: Builtin) -> Option<Repr> {
    Some(match b {
        Builtin::Bool => Repr::Bool,
        Builtin::U8 | Builtin::U16 | Builtin::U32 => Repr::U32,
        Builtin::U64 | Builtin::Duration => Repr::U64,
        Builtin::I32 | Builtin::I64 | Builtin::Timestamp => Repr::I64,
        Builtin::F64 => Repr::F64,
        Builtin::String => Repr::Str,
        Builtin::Uuid => Repr::Uuid,
        Builtin::Bytes | Builtin::Json => return None,
    })
}

/// The kind of sensitive data a type holds: a type marked sensitive (or with a marked struct
/// field), a struct with such a field, or list / option of one.
fn sensitivity(spec: &Spec, t: &TypeRef, depth: usize) -> Option<String> {
    if depth > 32 {
        return None;
    }
    match t {
        TypeRef::Builtin(_) => None,
        TypeRef::List(inner)
        | TypeRef::Set(inner)
        | TypeRef::Option(inner)
        | TypeRef::Shared(inner) => sensitivity(spec, inner, depth + 1),
        TypeRef::Map(key, value) => {
            sensitivity(spec, key, depth + 1).or_else(|| sensitivity(spec, value, depth + 1))
        }
        TypeRef::Declared(id) => {
            if let Some((_, kind)) = spec.telemetry.sensitive_types.iter().find(|(t, _)| t == id) {
                return Some(kind.clone());
            }
            match &spec.type_decl(id)?.kind {
                TypeKind::Struct { fields } => fields
                    .iter()
                    .find_map(|f| sensitivity(spec, &TypeRef::parse(&f.type_name), depth + 1)),
                TypeKind::Union { variants, .. } => variants
                    .iter()
                    .flat_map(|v| &v.fields)
                    .find_map(|f| sensitivity(spec, &TypeRef::parse(&f.type_name), depth + 1)),
                _ => None,
            }
        }
    }
}

/// A logged payload field split into optionality and the scalar it holds.
fn base(t: &TypeRef, optional: bool) -> (bool, TypeRef) {
    match t {
        TypeRef::Option(inner) => (true, (**inner).clone()),
        other => (optional, other.clone()),
    }
}

/// Checks one logged field of `e`, returning its member when it can be logged.
fn member(
    spec: &Spec,
    e: &Event,
    field: &str,
    written_as: &str,
    what: &str,
    out: &mut Vec<Diagnostic>,
) -> Option<Member> {
    let Some(p) = e.payload.iter().find(|p| p.name == field) else {
        out.push(Diagnostic::new(
            FIELD_UNKNOWN,
            &e.id,
            format!(
                "The {what} \"{field}\" of event \"{}\" is not one of its payload fields.",
                e.id
            ),
        ));
        return None;
    };
    let t = TypeRef::parse(p.type_name.as_deref().unwrap_or(""));
    let sensitive = p.sensitive.clone().or_else(|| sensitivity(spec, &t, 0));
    if let Some(kind) = sensitive {
        out.push(Diagnostic::new(
            FIELD_SENSITIVE,
            &e.id,
            format!(
                "The {what} \"{field}\" of event \"{}\" is sensitive ({kind}); sensitive data is never logged.",
                e.id
            ),
        ));
        return None;
    }
    let (optional, scalar) = base(&t, p.optional);
    let Some(repr) = repr_of(spec, &scalar) else {
        out.push(Diagnostic::new(
            FIELD_TYPE,
            &e.id,
            format!(
                "The {what} \"{field}\" of event \"{}\" has type \"{}\"; a log line holds only booleans, numbers and strings (ids, enums and uuids included), or an option of one.",
                e.id,
                p.type_name.as_deref().unwrap_or("")
            ),
        ));
        return None;
    };
    let rust = render::checked_name(out, &e.id, what, naming::snake(field))?;
    Some(Member {
        field: field.to_string(),
        written_as: written_as.to_string(),
        rust: naming::ident(&rust),
        repr,
        optional,
        type_ref: scalar,
    })
}

/// The members of a logged event's line after the correlation: the tenant, then the fields.
fn members(spec: &Spec, e: &Event, l: &Logging, out: &mut Vec<Diagnostic>) -> Vec<Member> {
    let mut names = Namespace::new(format!("the line of event \"{}\"", e.id));
    let mut ignored = Vec::new();
    names.claim(&mut ignored, &e.id, "the correlation", "correlation");
    let mut all = Vec::new();
    if let Some(t) = &l.tenant {
        if let Some(m) = member(spec, e, t, "tenant", "tenant field", out) {
            all.push(m);
        }
    }
    for f in &l.fields {
        if let Some(m) = member(spec, e, &f.field, &f.written_as, "logged field", out) {
            all.push(m);
        }
    }
    for m in &all {
        names.claim(
            out,
            &e.id,
            &format!("payload field \"{}\"", m.field),
            &m.rust.to_string(),
        );
    }
    all
}

fn level_tokens(level: &str) -> Option<TokenStream> {
    Some(match level {
        "debug" => quote! { Level::Debug },
        "info" => quote! { Level::Info },
        "warn" => quote! { Level::Warn },
        "error" => quote! { Level::Error },
        _ => return None,
    })
}

impl Backend for TelemetryBackend {
    fn module(&self) -> &'static str {
        "telemetry"
    }

    fn wanted(&self, spec: &Spec) -> bool {
        wanted(spec)
    }

    fn check(&self, spec: &Spec) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let mut names = Namespace::new("module telemetry");
        for s in &spec.telemetry.services {
            if let Some(n) = render::checked_name(
                &mut out,
                &s.id,
                &format!("telemetry service \"{}\"", s.id),
                naming::upper_snake(&s.id),
            ) {
                names.claim(
                    &mut out,
                    &s.id,
                    &format!("telemetry service \"{}\"", s.id),
                    &n,
                );
            }
        }
        for e in &spec.events {
            let Some(l) = &e.logging else { continue };
            let what = format!("event \"{}\"", e.id);
            if !spec.telemetry.services.iter().any(|s| s.id == l.service) {
                out.push(Diagnostic::new(
                    SERVICE_UNKNOWN,
                    &e.id,
                    format!(
                        "Event \"{}\" is logged by service \"{}\", which system.telemetry does not declare.",
                        e.id, l.service
                    ),
                ));
            }
            if level_tokens(&l.level).is_none() {
                out.push(Diagnostic::new(
                    LEVEL,
                    &e.id,
                    format!(
                        "Event \"{}\" is logged at level \"{}\"; a level is debug, info, warn or error.",
                        e.id, l.level
                    ),
                ));
            }
            if let Some(n) = render::checked_name(&mut out, &e.id, &what, naming::pascal(&e.id)) {
                names.claim_type(&mut out, &e.id, &what, &n);
            }
            if let Some(n) = render::checked_name(&mut out, &e.id, &what, naming::snake(&e.id)) {
                names.claim(&mut out, &e.id, &what, &n);
            }
            members(spec, e, l, &mut out);
        }
        out
    }

    fn emit(&self, spec: &Spec) -> TokenStream {
        let mut items = Vec::new();
        for s in &spec.telemetry.services {
            let name = naming::ident(&naming::upper_snake(&s.id).unwrap_or_default());
            let id = &s.id;
            let doc = match &s.log {
                Some(log) => format!(
                    " The log of service `{id}`: JSON lines on standard output, which the deployment keeps in `{log}`."
                ),
                None => format!(" The log of service `{id}`: JSON lines on standard output."),
            };
            items.push(quote! {
                #[doc = #doc]
                pub const #name: JsonLines = JsonLines { service: #id };
            });
        }
        for e in &spec.events {
            let Some(l) = &e.logging else { continue };
            let mut ignored = Vec::new();
            let all = members(spec, e, l, &mut ignored);
            let ty = naming::ident(&naming::pascal(&e.id).unwrap_or_default());
            let func = naming::ident(&naming::snake(&e.id).unwrap_or_default());
            let level = level_tokens(&l.level).unwrap_or_else(|| quote! { Level::Info });
            let log_name = &l.name;

            let mut doc = Doc::new(&format!("The log line of event `{}`: {}.", e.id, e.name));
            if let Some(d) = &e.description {
                doc.paragraph(d);
            }
            doc.paragraph(&format!(
                "Written by service `{}` at level `{}` as `{}`. It holds exactly these members; nothing else of the payload is ever logged.",
                l.service, l.level, l.name
            ));
            let doc = doc.tokens();
            let mut fields = vec![quote! {
                /// The correlation id that follows the request through every service; empty when unknown, and then not written.
                pub correlation: &'a str,
            }];
            let mut pushes = Vec::new();
            for m in &all {
                let rust = &m.rust;
                let param = m.repr.param();
                let ty_tokens = if m.optional {
                    quote! { Option<#param> }
                } else {
                    param
                };
                let fdoc = if m.written_as == m.field {
                    format!(" Payload field `{}`.", m.field)
                } else {
                    format!(
                        " Payload field `{}`, written as `{}`.",
                        m.field, m.written_as
                    )
                };
                fields.push(quote! {
                    #[doc = #fdoc]
                    pub #rust: #ty_tokens,
                });
                let key = &m.written_as;
                pushes.push(match (m.repr, m.optional) {
                    (Repr::Uuid, false) => {
                        let text =
                            format_ident!("{}_text", rust.to_string().trim_start_matches("r#"));
                        quote! {
                            let #text = line.#rust.to_string();
                            fields.push((#key, #text.as_str().into()));
                        }
                    }
                    (Repr::Uuid, true) => {
                        let text =
                            format_ident!("{}_text", rust.to_string().trim_start_matches("r#"));
                        quote! {
                            let #text = line.#rust.map(|value| value.to_string());
                            if let Some(value) = &#text {
                                fields.push((#key, value.as_str().into()));
                            }
                        }
                    }
                    (_, false) => quote! { fields.push((#key, line.#rust.into())); },
                    (_, true) => quote! {
                        if let Some(value) = line.#rust {
                            fields.push((#key, value.into()));
                        }
                    },
                });
            }
            let capacity = Literal::usize_unsuffixed(all.len() + 1);
            let fn_doc = format!(
                " Writes the line of event `{}` to `telemetry` (service `{}`, level `{}`, event `{}`).",
                e.id, l.service, l.level, l.name
            );
            items.push(quote! {
                #doc
                #[derive(Debug, Clone, Copy, PartialEq)]
                pub struct #ty<'a> {
                    #(#fields)*
                }

                #[doc = #fn_doc]
                pub fn #func(telemetry: &dyn Telemetry, line: &#ty<'_>) {
                    let mut fields: Vec<Field<'_>> = Vec::with_capacity(#capacity);
                    if !line.correlation.is_empty() {
                        fields.push(("correlation", line.correlation.into()));
                    }
                    #(#pushes)*
                    telemetry.event(#level, #log_name, &fields);
                }
            });
        }
        let uses_level = spec.events.iter().any(|e| e.logging.is_some());
        let imports = if uses_level {
            quote! { use strata_runtime::telemetry::{Field, JsonLines, Level, Telemetry}; }
        } else {
            quote! { use strata_runtime::telemetry::JsonLines; }
        };
        quote! {
            #![doc = " The structured log lines of the events the specification logs (`system.telemetry`, `event.logging`): one struct and one function per logged event, holding exactly the correlation id, the tenant and the allowlisted payload fields, written through `strata_runtime::Telemetry`, and one `JsonLines` constant per service. A line is `ts`, `level`, `service`, `event`, then `correlation` (when known), `tenant` and the allowlisted fields."]

            #imports

            #(#items)*
        }
    }
}

/// The pattern and the call of `Machine::log_emitted` that log emitted event `e`, whose payload
/// struct is bound as `payload` with the given Rust field names (payload field name, Rust
/// field), or `None` when the event is not logged. Machines report no correlation id.
pub fn emitted_arm(
    spec: &Spec,
    e: &Event,
    variant: &proc_macro2::Ident,
    payload_fields: &[(String, proc_macro2::Ident)],
) -> Option<(TokenStream, TokenStream)> {
    let l = e.logging.as_ref()?;
    let mut ignored = Vec::new();
    let all = members(spec, e, l, &mut ignored);
    let ty = naming::ident(&naming::pascal(&e.id).ok()?);
    let func = naming::ident(&naming::snake(&e.id).ok()?);
    let mut values = Vec::new();
    for m in &all {
        let (_, source) = payload_fields.iter().find(|(name, _)| *name == m.field)?;
        let p = e.payload.iter().find(|p| p.name == m.field)?;
        let declared_option = matches!(
            TypeRef::parse(p.type_name.as_deref().unwrap_or("")),
            TypeRef::Option(_)
        );
        let rust = &m.rust;
        let value = if m.optional {
            let conv = convert(spec, &m.type_ref, quote! { value }, true);
            // A payload field both optional and of type option<T> holds Option<Option<T>>.
            let flat = if p.optional && declared_option {
                quote! { .as_ref().and_then(|value| value.as_ref()) }
            } else {
                quote! { .as_ref() }
            };
            quote! { payload.#source #flat.map(|value| #conv) }
        } else {
            convert(spec, &m.type_ref, quote! { payload.#source }, false)
        };
        values.push(quote! { #rust: #value, });
    }
    let bind = if values.is_empty() {
        quote! { _ }
    } else {
        quote! { payload }
    };
    Some((
        quote! { Emitted::#variant(#bind) },
        quote! {
            crate::telemetry::#func(
                telemetry,
                &crate::telemetry::#ty { correlation: "", #(#values)* },
            )
        },
    ))
}

/// The expression that turns a typed payload value into its logged representation. `value` is a
/// place of the value's type, or (when `is_ref`) a reference to one.
fn convert(spec: &Spec, t: &TypeRef, value: TokenStream, is_ref: bool) -> TokenStream {
    let copy = |v: TokenStream| {
        if is_ref {
            quote! { *#v }
        } else {
            v
        }
    };
    match t {
        TypeRef::Builtin(b) => builtin_convert(*b, value, is_ref),
        TypeRef::Declared(id) => match spec.type_decl(id).map(|d| &d.kind) {
            Some(TypeKind::Id { repr }) | Some(TypeKind::Scalar { repr }) => {
                match repr.as_deref().and_then(Builtin::parse) {
                    Some(b) => builtin_convert(b, quote! { #value.0 }, false),
                    None => copy(value),
                }
            }
            Some(TypeKind::Enum { .. }) => quote! { #value.as_str() },
            _ => copy(value),
        },
        _ => copy(value),
    }
}

fn builtin_convert(b: Builtin, value: TokenStream, is_ref: bool) -> TokenStream {
    let copied = if is_ref {
        quote! { *#value }
    } else {
        value.clone()
    };
    match b {
        Builtin::String => quote! { #value.as_str() },
        Builtin::U8 | Builtin::U16 => quote! { u32::from(#copied) },
        Builtin::I32 => quote! { i64::from(#copied) },
        Builtin::Timestamp | Builtin::Duration => quote! { #value.0 },
        _ => copied,
    }
}
