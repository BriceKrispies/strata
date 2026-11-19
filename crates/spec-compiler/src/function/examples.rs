//! The examples of a function as unit tests of the generated function, one test per example.
//!
//! An example's values are JSON in the form Exegra's examples write them (`function-value.js`):
//! bool, number and string as themselves; an enum value by its name; an id or scalar as its
//! representation; a struct as an object of its fields (an absent or null optional field is
//! `None`); a list as an array; an option as null or the value; a map as an object (integer keys
//! as decimal strings); a union in serde's external form whatever its wire encoding; a result as
//! `{"Ok": ..}` or `{"Err": ..}`. Each is written as a Rust value expression of the generated
//! type, so the tests do not depend on the types' serde encodings.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::Value;

use super::rust::{by_ref, rust_ty, snake_ident, type_path};
use super::typing::{int_kind, struct_fields, unsupported, variants, FieldInfo, Refusal, Sig, Ty};
use super::{Example, Function, Outcome};
use crate::ir::Spec;
use crate::naming;

fn show(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 60 {
        format!(
            "{}...",
            &s[..s.char_indices().nth(57).map_or(s.len(), |(i, _)| i)]
        )
    } else {
        s
    }
}

fn not_a(t: &Ty, v: &Value) -> Refusal {
    unsupported(format!("the example value {} is not {}", show(v), t.text()))
}

/// A value expression of type `t` for the example value `v`.
pub(crate) fn literal(spec: &Spec, t: &Ty, v: &Value) -> Result<TokenStream, Refusal> {
    Ok(match t {
        Ty::Bool => match v {
            Value::Bool(b) => quote! { #b },
            _ => return Err(not_a(t, v)),
        },
        Ty::Str => match v {
            Value::String(s) => {
                let lit = Literal::string(s);
                quote! { #lit.to_string() }
            }
            _ => return Err(not_a(t, v)),
        },
        Ty::Int(_) => {
            let n = v
                .as_i64()
                .map(i128::from)
                .or_else(|| v.as_u64().map(i128::from))
                .ok_or_else(|| not_a(t, v))?;
            let (lo, hi) = b_range(t);
            if n < lo || n > hi {
                return Err(not_a(t, v));
            }
            let lit: TokenStream = n
                .unsigned_abs()
                .to_string()
                .parse()
                .map_err(|_| not_a(t, v))?;
            if n < 0 {
                quote! { -#lit }
            } else {
                lit
            }
        }
        Ty::Alias(id, repr) => {
            let inner = literal(spec, repr, v)?;
            let p = type_path(id);
            quote! { #p(#inner) }
        }
        Ty::Enum(id) => {
            let Value::String(s) = v else {
                return Err(not_a(t, v));
            };
            if !super::typing::enum_values(spec, id).contains(s) {
                return Err(not_a(t, v));
            }
            let p = type_path(id);
            let value = naming::ident(&naming::pascal(s).unwrap_or_default());
            quote! { #p::#value }
        }
        Ty::Struct(id) => {
            let Value::Object(obj) = v else {
                return Err(not_a(t, v));
            };
            let fields = struct_fields(spec, id);
            if let Some(k) = obj.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
                return Err(unsupported(format!(
                    "the example gives {id} the field {k}, which it does not have"
                )));
            }
            let inits = field_inits(spec, id, &fields, obj)?;
            let p = type_path(id);
            quote! { #p { #(#inits),* } }
        }
        Ty::Union(id) => {
            let vs = variants(spec, id);
            let p = type_path(id);
            match v {
                Value::String(name) => {
                    let Some(var) = vs.iter().find(|x| &x.name == name && x.fields.is_empty())
                    else {
                        return Err(not_a(t, v));
                    };
                    let vi = naming::ident(&naming::pascal(&var.name).unwrap_or_default());
                    quote! { #p::#vi }
                }
                Value::Object(obj) if obj.len() == 1 => {
                    let (name, content) = obj.iter().next().expect("one entry");
                    let Some(var) = vs.iter().find(|x| &x.name == name && !x.fields.is_empty())
                    else {
                        return Err(not_a(t, v));
                    };
                    let vi = naming::ident(&naming::pascal(&var.name).unwrap_or_default());
                    if var.newtype {
                        let f = &var.fields[0];
                        let inner = field_value(spec, f, content)?;
                        quote! { #p::#vi(#inner) }
                    } else {
                        let Value::Object(fobj) = content else {
                            return Err(not_a(t, v));
                        };
                        let inits = field_inits(spec, id, &var.fields, fobj)?;
                        quote! { #p::#vi { #(#inits),* } }
                    }
                }
                _ => return Err(not_a(t, v)),
            }
        }
        Ty::List(of) => {
            let Value::Array(items) = v else {
                return Err(not_a(t, v));
            };
            if items.is_empty() {
                quote! { Vec::new() }
            } else {
                let items: Vec<TokenStream> = items
                    .iter()
                    .map(|x| literal(spec, of, x))
                    .collect::<Result<_, _>>()?;
                quote! { vec![#(#items),*] }
            }
        }
        Ty::Option(of) => match v {
            Value::Null => quote! { None },
            _ => {
                let inner = literal(spec, of, v)?;
                quote! { Some(#inner) }
            }
        },
        Ty::Map(k, val) => {
            let Value::Object(obj) = v else {
                return Err(not_a(t, v));
            };
            if obj.is_empty() {
                quote! { std::collections::BTreeMap::new() }
            } else {
                let mut entries = Vec::new();
                for (key, x) in obj {
                    let key_json = if int_kind(k).is_some() {
                        key.parse::<i64>()
                            .map(Value::from)
                            .or_else(|_| key.parse::<u64>().map(Value::from))
                            .map_err(|_| not_a(k, &Value::String(key.clone())))?
                    } else {
                        Value::String(key.clone())
                    };
                    let kt = literal(spec, k, &key_json)?;
                    let vt = literal(spec, val, x)?;
                    entries.push(quote! { (#kt, #vt) });
                }
                quote! { std::collections::BTreeMap::from([#(#entries),*]) }
            }
        }
        Ty::Result(o, e) => match v {
            Value::Object(obj) if obj.len() == 1 && obj.contains_key("Ok") => {
                let inner = literal(spec, o, &obj["Ok"])?;
                quote! { Ok(#inner) }
            }
            Value::Object(obj) if obj.len() == 1 && obj.contains_key("Err") => {
                let inner = literal(spec, e, &obj["Err"])?;
                quote! { Err(#inner) }
            }
            _ => return Err(not_a(t, v)),
        },
        Ty::Number | Ty::Unknown | Ty::Never => {
            return Err(unsupported(
                "an example value of a type that cannot be told",
            ))
        }
    })
}

fn b_range(t: &Ty) -> (i128, i128) {
    int_kind(t)
        .and_then(|b| b.integer_range())
        .unwrap_or((0, -1))
}

/// A field's value: `Some(..)` for an optional field, boxed where the type holds it boxed.
fn field_value(spec: &Spec, f: &FieldInfo, v: &Value) -> Result<TokenStream, Refusal> {
    let read = f.read(spec)?;
    let value = literal(spec, &read, v)?;
    if !f.boxed {
        return Ok(value);
    }
    if v.is_null() {
        return Ok(value);
    }
    Ok(match read {
        Ty::Option(_) => quote! { #value.map(Box::new) },
        _ => quote! { Box::new(#value) },
    })
}

fn field_inits(
    spec: &Spec,
    owner: &str,
    fields: &[FieldInfo],
    obj: &serde_json::Map<String, Value>,
) -> Result<Vec<TokenStream>, Refusal> {
    let mut out = Vec::new();
    for f in fields {
        let fid = snake_ident(&f.name);
        match obj.get(&f.name) {
            None | Some(Value::Null) => {
                if !f.may_be_absent(spec) {
                    return Err(unsupported(format!(
                        "the example leaves out the field {} of {owner}, which is required",
                        f.name
                    )));
                }
                out.push(quote! { #fid: None });
            }
            Some(v) => {
                let value = field_value(spec, f, v)?;
                out.push(quote! { #fid: #value });
            }
        }
    }
    Ok(out)
}

/// The tests of one function's examples.
pub(crate) fn tests(spec: &Spec, f: &Function, sig: &Sig) -> Result<Vec<TokenStream>, Refusal> {
    let name = snake_ident(&f.id);
    let base = naming::snake(&f.id).unwrap_or_default();
    let mut out = Vec::new();
    for (i, ex) in f.examples.iter().enumerate() {
        out.push(test(spec, f, sig, &name, &base, i + 1, ex)?);
    }
    Ok(out)
}

fn test(
    spec: &Spec,
    f: &Function,
    sig: &Sig,
    name: &proc_macro2::Ident,
    base: &str,
    n: usize,
    ex: &Example,
) -> Result<TokenStream, Refusal> {
    let test_name = format_ident!("{base}_example_{n}");
    if let Some(k) = ex
        .args
        .keys()
        .find(|k| !sig.params.iter().any(|(p, _)| p == *k))
    {
        return Err(unsupported(format!(
            "example {n} names {k}, which is not a parameter"
        )));
    }
    let mut lets = Vec::new();
    let mut args = Vec::new();
    for (pname, pt) in &sig.params {
        let v = ex.args.get(pname).cloned().unwrap_or(Value::Null);
        let value = literal(spec, pt, &v).map_err(|r| Refusal {
            code: r.code,
            message: format!("example {n}, argument {pname}: {}", r.message),
        })?;
        let ident = snake_ident(pname);
        let ty = rust_ty(pt)?;
        lets.push(quote! { let #ident: #ty = #value; });
        if by_ref(pt) {
            args.push(quote! { &#ident });
        } else {
            args.push(quote! { #ident });
        }
    }
    let call = quote! { #name(#(#args),*) };
    let check = match &ex.outcome {
        Outcome::Result(v) => {
            let want = literal(spec, &sig.returns, v).map_err(|r| Refusal {
                code: r.code,
                message: format!("example {n}, result: {}", r.message),
            })?;
            let result = sig.errors.is_some() || sig.faultable;
            match (result, &sig.returns, v) {
                (false, Ty::Bool, Value::Bool(true)) => quote! { assert!(#call); },
                (false, Ty::Bool, Value::Bool(false)) => quote! { assert!(!#call); },
                (false, _, _) => quote! { assert_eq!(#call, #want); },
                (true, _, _) => quote! { assert_eq!(#call, Ok(#want)); },
            }
        }
        Outcome::Error(v) => {
            let Some(errors) = &sig.errors else {
                return Err(unsupported(format!(
                    "example {n} expects an error, but {} declares no errors",
                    f.id
                )));
            };
            let want = literal(spec, &Ty::Union(errors.clone()), v).map_err(|r| Refusal {
                code: r.code,
                message: format!("example {n}, error: {}", r.message),
            })?;
            if sig.faultable {
                quote! { assert_eq!(#call, Err(Failure::Error(#want))); }
            } else {
                quote! { assert_eq!(#call, Err(#want)); }
            }
        }
    };
    let doc = match &ex.description {
        Some(d) => crate::render::Doc::new(d).tokens(),
        None => {
            let line = format!(" Example {n} of `{}`.", f.id);
            quote! { #[doc = #line] }
        }
    };
    Ok(quote! {
        #doc
        #[test]
        fn #test_name() {
            #(#lets)*
            #check
        }
    })
}
