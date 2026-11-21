//! The examples of a function as unit tests of the generated function, one test per example.
//!
//! An example's values are JSON in the form Exegra's examples write them (`function-value.js`):
//! bool, number and string as themselves; an enum value by its name; an id or scalar as its
//! representation; a struct as an object of its fields (an absent or null optional field is
//! `None`); a list as an array; an option as null or the value; a map as an object (integer keys
//! as decimal strings); a union in serde's external form whatever its wire encoding; a result as
//! `{"Ok": ..}` or `{"Err": ..}`. Each is written as a Rust value expression of the generated
//! type, so the tests do not depend on the types' serde encodings.
//!
//! # Fixtures
//!
//! An argument, result or error may be a reference to an example fixture (Exegra WG-4963):
//! `{"fixture": id}` or `{"fixture": id, "with": {"a.b": value}}`. Each fixture a test uses is
//! written once, as `fn fixture_<id>() -> T` in the test module, and a use calls it; an override
//! is an assignment in code after the call (a struct field, a map key or a list index, through
//! options). A path that goes through a union or a boxed field cannot be assigned, so that use
//! writes the fixture's value with the overrides merged in as a literal instead, which is the
//! same value. A fixture nobody uses is not written.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::Value;

use super::rust::{by_ref, rust_ty, snake_ident, type_path};
use super::typing::{
    int_kind, invalid, struct_fields, ty_of_text, unshared, unsupported, variants, FieldInfo,
    Refusal, Sig, Ty,
};
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
        Ty::Uuid => {
            let u = uuid_value(v).ok_or_else(|| not_a(t, v))?;
            let lit = Literal::u128_suffixed(u);
            quote! { uuid::Uuid::from_u128(#lit) }
        }
        Ty::Float => {
            let x = v.as_f64().ok_or_else(|| not_a(t, v))?;
            if !x.is_finite() {
                return Err(not_a(t, v));
            }
            let lit = Literal::f64_unsuffixed(x.abs());
            if x.is_sign_negative() {
                quote! { -#lit }
            } else {
                quote! { #lit }
            }
        }
        // Exegra writes a shared<T> as a T.
        Ty::Shared(of) => {
            let inner = literal(spec, of, v)?;
            quote! { std::sync::Arc::new(#inner) }
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
                        // Exegra writes every variant with fields but a newtype as an object of
                        // its fields, a tuple variant too.
                        let Value::Object(fobj) = content else {
                            return Err(not_a(t, v));
                        };
                        if let Some(k) = fobj
                            .keys()
                            .find(|k| !var.fields.iter().any(|f| &f.name == *k))
                        {
                            return Err(unsupported(format!(
                                "the example gives variant {} of {id} the field {k}, which it does not have",
                                var.name
                            )));
                        }
                        if var.tuple {
                            let values = positional(spec, id, &var.fields, fobj)?;
                            quote! { #p::#vi(#(#values),*) }
                        } else {
                            let inits = field_inits(spec, id, &var.fields, fobj)?;
                            quote! { #p::#vi { #(#inits),* } }
                        }
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
        Ty::Set(of) => {
            let Value::Array(items) = v else {
                return Err(not_a(t, v));
            };
            // Exegra holds a set as the list written, and iterates it in that order; the
            // BTreeSet iterates in its own. They agree only on a list in that order without a
            // repeated item, so an example writes a set so.
            for (i, pair) in items.windows(2).enumerate() {
                if order(spec, of, &pair[0], &pair[1])? != std::cmp::Ordering::Less {
                    return Err(unsupported(format!(
                        "the example set {} does not have item {} after item {} in the order of {}; a set's items are written in that order, each once, as the generated BTreeSet holds them",
                        show(v),
                        i + 2,
                        i + 1,
                        of.text()
                    )));
                }
            }
            if items.is_empty() {
                quote! { std::collections::BTreeSet::new() }
            } else {
                let items: Vec<TokenStream> = items
                    .iter()
                    .map(|x| literal(spec, of, x))
                    .collect::<Result<_, _>>()?;
                quote! { std::collections::BTreeSet::from([#(#items),*]) }
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
                    let kt = map_key(spec, k, key)?;
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
        // Exegra writes the unit value as null.
        Ty::Unit => match v {
            Value::Null => quote! { () },
            _ => return Err(not_a(t, v)),
        },
        Ty::Number | Ty::Wide | Ty::Unknown | Ty::Never => {
            return Err(unsupported(
                "an example value of a type that cannot be told",
            ))
        }
    })
}

/// How two example values of type `t` compare in the order the generated type derives (`Ord`):
/// a bool false first, a string by its bytes, an integer by value, an id or scalar by its
/// representation, an enum value or a union variant by declaration order and then a variant's
/// fields, a struct by its fields in order, an option `None` first, a list or set item by item,
/// a map entry by entry in key order, a result `Ok` first.
fn order(spec: &Spec, t: &Ty, a: &Value, b: &Value) -> Result<std::cmp::Ordering, Refusal> {
    use std::cmp::Ordering;
    let bad = |v: &Value| not_a(t, v);
    Ok(match t {
        Ty::Bool => match (a, b) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Bool(_), _) => return Err(bad(b)),
            _ => return Err(bad(a)),
        },
        Ty::Str => match (a, b) {
            (Value::String(x), Value::String(y)) => x.as_bytes().cmp(y.as_bytes()),
            (Value::String(_), _) => return Err(bad(b)),
            _ => return Err(bad(a)),
        },
        Ty::Int(_) => {
            let n = |v: &Value| {
                v.as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
                    .ok_or_else(|| bad(v))
            };
            n(a)?.cmp(&n(b)?)
        }
        Ty::Alias(_, repr) | Ty::Shared(repr) => order(spec, repr, a, b)?,
        // Lowercase hyphenated text orders as the uuid's bytes do.
        Ty::Uuid => {
            let u = |v: &Value| uuid_value(v).ok_or_else(|| bad(v));
            u(a)?.cmp(&u(b)?)
        }
        Ty::Float => {
            return Err(unsupported(
                "an example set of f64 items, which have no total order",
            ))
        }
        Ty::Enum(id) => {
            let values = super::typing::enum_values(spec, id);
            let at = |v: &Value| match v {
                Value::String(s) => values.iter().position(|x| x == s).ok_or_else(|| bad(v)),
                _ => Err(bad(v)),
            };
            at(a)?.cmp(&at(b)?)
        }
        Ty::Struct(id) => {
            let (Value::Object(x), Value::Object(y)) = (a, b) else {
                return Err(bad(if a.is_object() { b } else { a }));
            };
            fields_order(spec, &struct_fields(spec, id), x, y)?
        }
        Ty::Union(id) => {
            let vs = variants(spec, id);
            let split = |v: &Value| -> Result<(usize, Option<Value>), Refusal> {
                let (name, content) = match v {
                    Value::String(s) => (s.clone(), None),
                    Value::Object(o) if o.len() == 1 => {
                        let (k, c) = o.iter().next().expect("one entry");
                        (k.clone(), Some(c.clone()))
                    }
                    _ => return Err(bad(v)),
                };
                let i = vs
                    .iter()
                    .position(|x| x.name == name)
                    .ok_or_else(|| bad(v))?;
                Ok((i, content))
            };
            let ((i, ca), (j, cb)) = (split(a)?, split(b)?);
            if i != j {
                return Ok(i.cmp(&j));
            }
            let var = &vs[i];
            match (ca, cb) {
                (None, None) => Ordering::Equal,
                (Some(x), Some(y)) if var.newtype => {
                    let ft = var.fields[0].read(spec)?;
                    order(spec, &ft, &x, &y)?
                }
                (Some(Value::Object(x)), Some(Value::Object(y))) => {
                    fields_order(spec, &var.fields, &x, &y)?
                }
                _ => return Err(bad(a)),
            }
        }
        Ty::List(of) | Ty::Set(of) => {
            let (Value::Array(x), Value::Array(y)) = (a, b) else {
                return Err(bad(if a.is_array() { b } else { a }));
            };
            for (p, q) in x.iter().zip(y) {
                let o = order(spec, of, p, q)?;
                if o != Ordering::Equal {
                    return Ok(o);
                }
            }
            x.len().cmp(&y.len())
        }
        Ty::Option(of) => match (a, b) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Less,
            (_, Value::Null) => Ordering::Greater,
            _ => order(spec, of, a, b)?,
        },
        Ty::Map(k, v) => {
            let (Value::Object(x), Value::Object(y)) = (a, b) else {
                return Err(bad(if a.is_object() { b } else { a }));
            };
            let entries =
                |m: &serde_json::Map<String, Value>| -> Result<Vec<(Value, Value)>, Refusal> {
                    let mut out = Vec::new();
                    for (key, value) in m {
                        let kj = if int_kind(k).is_some() {
                            key.parse::<i64>()
                                .map(Value::from)
                                .or_else(|_| key.parse::<u64>().map(Value::from))
                                .map_err(|_| not_a(k, &Value::String(key.clone())))?
                        } else {
                            Value::String(key.clone())
                        };
                        out.push((kj, value.clone()));
                    }
                    let mut failed = None;
                    out.sort_by(|p, q| match order(spec, k, &p.0, &q.0) {
                        Ok(o) => o,
                        Err(r) => {
                            if failed.is_none() {
                                failed = Some(r);
                            }
                            Ordering::Equal
                        }
                    });
                    match failed {
                        Some(r) => Err(r),
                        None => Ok(out),
                    }
                };
            let (ex, ey) = (entries(x)?, entries(y)?);
            for ((pk, pv), (qk, qv)) in ex.iter().zip(&ey) {
                let o = order(spec, k, pk, qk)?;
                if o != Ordering::Equal {
                    return Ok(o);
                }
                let o = order(spec, v, pv, qv)?;
                if o != Ordering::Equal {
                    return Ok(o);
                }
            }
            ex.len().cmp(&ey.len())
        }
        Ty::Result(o, e) => {
            let side = |v: &Value| -> Result<(bool, Value), Refusal> {
                match v {
                    Value::Object(m) if m.len() == 1 && m.contains_key("Ok") => {
                        Ok((false, m["Ok"].clone()))
                    }
                    Value::Object(m) if m.len() == 1 && m.contains_key("Err") => {
                        Ok((true, m["Err"].clone()))
                    }
                    _ => Err(bad(v)),
                }
            };
            let ((ea, x), (eb, y)) = (side(a)?, side(b)?);
            if ea != eb {
                return Ok(ea.cmp(&eb));
            }
            order(spec, if ea { e } else { o }, &x, &y)?
        }
        Ty::Unit => match (a, b) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => return Err(bad(b)),
            _ => return Err(bad(a)),
        },
        Ty::Number | Ty::Wide | Ty::Unknown | Ty::Never => {
            return Err(unsupported(
                "an example value of a type that cannot be told",
            ))
        }
    })
}

/// Two objects of the fields, field by field in declaration order (a field left out is null).
fn fields_order(
    spec: &Spec,
    fields: &[FieldInfo],
    x: &serde_json::Map<String, Value>,
    y: &serde_json::Map<String, Value>,
) -> Result<std::cmp::Ordering, Refusal> {
    for f in fields {
        let ft = f.read(spec)?;
        let p = x.get(&f.name).unwrap_or(&Value::Null);
        let q = y.get(&f.name).unwrap_or(&Value::Null);
        let o = order(spec, &ft, p, q)?;
        if o != std::cmp::Ordering::Equal {
            return Ok(o);
        }
    }
    Ok(std::cmp::Ordering::Equal)
}

/// A map key written as an object key: an integer key is a decimal string.
fn map_key(spec: &Spec, k: &Ty, key: &str) -> Result<TokenStream, Refusal> {
    let key_json = if int_kind(k).is_some() {
        key.parse::<i64>()
            .map(Value::from)
            .or_else(|_| key.parse::<u64>().map(Value::from))
            .map_err(|_| not_a(k, &Value::String(key.to_string())))?
    } else {
        Value::String(key.to_string())
    };
    literal(spec, k, &key_json)
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
    let values = positional(spec, owner, fields, obj)?;
    Ok(fields
        .iter()
        .zip(values)
        .map(|(f, value)| {
            let fid = snake_ident(&f.name);
            quote! { #fid: #value }
        })
        .collect())
}

/// The values of the fields in declaration order (`None` for one left out), as a tuple variant
/// takes them.
fn positional(
    spec: &Spec,
    owner: &str,
    fields: &[FieldInfo],
    obj: &serde_json::Map<String, Value>,
) -> Result<Vec<TokenStream>, Refusal> {
    let mut out = Vec::new();
    for f in fields {
        match obj.get(&f.name) {
            None | Some(Value::Null) => {
                if !f.may_be_absent(spec) {
                    return Err(unsupported(format!(
                        "the example leaves out the field {} of {owner}, which is required",
                        f.name
                    )));
                }
                out.push(quote! { None });
            }
            Some(v) => out.push(field_value(spec, f, v)?),
        }
    }
    Ok(out)
}

/// The tests of one function's examples.
pub(crate) fn tests(
    spec: &Spec,
    f: &Function,
    sig: &Sig,
    used: &mut BTreeSet<String>,
) -> Result<Vec<TokenStream>, Refusal> {
    let name = snake_ident(&f.id);
    let base = naming::snake(&f.id).unwrap_or_default();
    let mut out = Vec::new();
    for (i, ex) in f.examples.iter().enumerate() {
        out.push(test(spec, f, sig, &name, &base, i + 1, ex, used)?);
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
    used: &mut BTreeSet<String>,
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
        let value = value_expr(spec, pt, &v, used).map_err(|r| Refusal {
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
            let want = value_expr(spec, &sig.returns, v, used).map_err(|r| Refusal {
                code: r.code,
                message: format!("example {n}, result: {}", r.message),
            })?;
            let result = sig.errors.is_some() || sig.faultable;
            match (result, &sig.returns, v) {
                (false, Ty::Bool, Value::Bool(true)) => quote! { assert!(#call); },
                (false, Ty::Bool, Value::Bool(false)) => quote! { assert!(!#call); },
                // The expected value is bound with the return type first: an empty list compares
                // with a list of any item type, so `assert_eq!(f(..), Vec::new())` cannot infer it.
                (false, _, _) => {
                    let ret = rust_ty(&sig.returns)?;
                    quote! { let want_: #ret = #want; assert_eq!(#call, want_); }
                }
                (true, _, _) => {
                    let ret = rust_ty(&sig.returns)?;
                    quote! { let want_: #ret = #want; assert_eq!(#call, Ok(want_)); }
                }
            }
        }
        Outcome::Error(v) => {
            let Some(errors) = &sig.errors else {
                return Err(unsupported(format!(
                    "example {n} expects an error, but {} declares no errors",
                    f.id
                )));
            };
            let want =
                value_expr(spec, &Ty::Union(errors.clone()), v, used).map_err(|r| Refusal {
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
    // An example that builds a large value inline (hundreds of object types, say) has a frame that
    // a debug build makes larger than the default stack of a test thread, so it runs on a thread
    // with a stack of its own and hands its panic back to the test.
    let size = quote! { #(#lets)* #check }.to_string().len();
    if size > LARGE_EXAMPLE_CHARS {
        return Ok(quote! {
            #doc
            #[test]
            fn #test_name() {
                let run = std::thread::Builder::new()
                    .stack_size(#LARGE_EXAMPLE_STACK_BYTES)
                    .spawn(move || {
                        #(#lets)*
                        #check
                    })
                    .expect("a thread for the example");
                if let Err(panic) = run.join() {
                    std::panic::resume_unwind(panic);
                }
            }
        });
    }
    Ok(quote! {
        #doc
        #[test]
        fn #test_name() {
            #(#lets)*
            #check
        }
    })
}

/// The size, in characters of generated code, past which an example runs on a thread with a
/// stack of its own.
const LARGE_EXAMPLE_CHARS: usize = 100_000;

/// The stack of that thread: 256 MiB, which is address space the test only touches as it uses it.
const LARGE_EXAMPLE_STACK_BYTES: usize = 256 * 1024 * 1024;

/// A fixture reference: the fixture's id and its overrides.
struct FixtureRef<'a> {
    id: &'a str,
    with: Option<&'a serde_json::Map<String, Value>>,
}

/// The reference an example value is, if it is one: an object whose `fixture` is a string and
/// whose only other key is `with`.
fn fixture_ref(v: &Value) -> Result<Option<FixtureRef<'_>>, Refusal> {
    let Value::Object(obj) = v else {
        return Ok(None);
    };
    let Some(Value::String(id)) = obj.get("fixture") else {
        return Ok(None);
    };
    if obj.keys().any(|k| k != "fixture" && k != "with") {
        return Ok(None);
    }
    let with = match obj.get("with") {
        None => None,
        Some(Value::Object(w)) => Some(w),
        Some(_) => {
            return Err(invalid(format!(
                "\"with\" of the fixture {id} must be an object of path to value"
            )))
        }
    };
    Ok(Some(FixtureRef { id, with }))
}

/// The name of the function that gives a fixture's value.
pub(crate) fn fixture_fn(id: &str) -> proc_macro2::Ident {
    format_ident!("fixture_{}", naming::snake(id).unwrap_or_default())
}

/// A value expression of type `t` for an example value that is a literal or a fixture reference.
fn value_expr(
    spec: &Spec,
    t: &Ty,
    v: &Value,
    used: &mut BTreeSet<String>,
) -> Result<TokenStream, Refusal> {
    let Some(r) = fixture_ref(v)? else {
        return literal(spec, t, v);
    };
    let Some(fx) = spec.example_fixtures.iter().find(|f| f.id == r.id) else {
        return Err(invalid(format!(
            "the example refers to the fixture {}, which is not declared",
            r.id
        )));
    };
    let fty = ty_of_text(spec, &fx.type_name)?;
    // A fixture stands for a value of its type, an option of it, or (as Exegra reads
    // shared<T> as T) a shared one of either.
    let (wrap, inner) = match t {
        Ty::Option(of) if unshared(of) == unshared(&fty) => (true, of.as_ref()),
        _ => (false, t),
    };
    if unshared(inner) != unshared(&fty) {
        return Err(invalid(format!(
            "the fixture {} is a {}, but a {} stands here",
            fx.id,
            fty.text(),
            t.text()
        )));
    }
    let share = matches!(inner, Ty::Shared(_)) && !matches!(fty, Ty::Shared(_));
    if matches!(fty, Ty::Shared(_)) && !matches!(inner, Ty::Shared(_)) {
        return Err(unsupported(format!(
            "the fixture {} is a {}, written as an Arc, where a plain {} stands",
            fx.id,
            fty.text(),
            inner.text()
        )));
    }
    used.insert(fx.id.clone());
    let make = fixture_fn(&fx.id);
    let overrides: Vec<(&String, &Value)> = r.with.map(|w| w.iter().collect()).unwrap_or_default();
    let mut stmts = Vec::new();
    let mut in_code = true;
    for (path, value) in &overrides {
        let segs: Vec<&str> = path.split('.').collect();
        if segs.iter().any(|s| s.is_empty()) {
            return Err(invalid(format!(
                "the override {path} of the fixture {} has an empty step",
                fx.id
            )));
        }
        match assign(spec, &fty, &segs, value)? {
            Some(stmt) => stmts.push(stmt),
            None => {
                in_code = false;
                break;
            }
        }
    }
    let body = if overrides.is_empty() {
        quote! { #make() }
    } else if in_code {
        quote! { { let mut fixture = #make(); #(#stmts)* fixture } }
    } else {
        // A path through a union or a boxed field: the same value, written out.
        let mut merged = fx.value.clone();
        for (path, value) in &overrides {
            let segs: Vec<&str> = path.split('.').collect();
            json_set(&mut merged, &segs, value, path)?;
        }
        literal(spec, &fty, &merged)?
    };
    let body = if share {
        quote! { std::sync::Arc::new(#body) }
    } else {
        body
    };
    Ok(if wrap {
        quote! { Some(#body) }
    } else {
        body
    })
}

/// The statement that sets the value at `segs` of the local `fixture` (of type `t`), or `None`
/// when the path goes where an assignment cannot (a union, a boxed field).
fn assign(
    spec: &Spec,
    t: &Ty,
    segs: &[&str],
    value: &Value,
) -> Result<Option<TokenStream>, Refusal> {
    let mut place = quote! { fixture };
    let mut t = t.clone();
    for (i, seg) in segs.iter().enumerate() {
        let last = i + 1 == segs.len();
        while let Ty::Option(of) = t.clone() {
            place = quote! { #place.as_mut().expect("the fixture value is present") };
            t = *of;
        }
        match t.clone() {
            Ty::Struct(id) => {
                let fields = struct_fields(spec, &id);
                let Some(f) = fields.iter().find(|f| f.name == *seg) else {
                    return Err(invalid(format!("{id} has no field {seg}")));
                };
                let fid = snake_ident(&f.name);
                if last {
                    let v = field_value(spec, f, value)?;
                    return Ok(Some(quote! { #place.#fid = #v; }));
                }
                if f.boxed {
                    return Ok(None);
                }
                place = quote! { #place.#fid };
                t = f.read(spec)?;
            }
            Ty::Map(k, vt) => {
                let key = map_key(spec, &k, seg)?;
                if last {
                    let v = literal(spec, &vt, value)?;
                    return Ok(Some(quote! { #place.insert(#key, #v); }));
                }
                place = quote! { #place.get_mut(&#key).expect("the fixture has the key") };
                t = *vt;
            }
            Ty::List(of) => {
                let idx: usize = seg
                    .parse()
                    .map_err(|_| invalid(format!("{seg} is not a list index")))?;
                let index = Literal::usize_unsuffixed(idx);
                if last {
                    let v = literal(spec, &of, value)?;
                    return Ok(Some(quote! { #place[#index] = #v; }));
                }
                place = quote! { #place[#index] };
                t = *of;
            }
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Sets the value at `segs` of a JSON value: object keys (a struct field, a map key or the
/// name of a union variant) and array indexes.
fn json_set(v: &mut Value, segs: &[&str], value: &Value, path: &str) -> Result<(), Refusal> {
    let Some((seg, rest)) = segs.split_first() else {
        *v = value.clone();
        return Ok(());
    };
    let bad = || {
        invalid(format!(
            "the override {path} does not lead into the fixture's value"
        ))
    };
    match v {
        Value::Object(o) if rest.is_empty() => {
            o.insert((*seg).to_string(), value.clone());
            Ok(())
        }
        Value::Object(o) => json_set(o.get_mut(*seg).ok_or_else(bad)?, rest, value, path),
        Value::Array(a) => {
            let idx: usize = seg.parse().map_err(|_| bad())?;
            json_set(a.get_mut(idx).ok_or_else(bad)?, rest, value, path)
        }
        _ => Err(bad()),
    }
}

/// The fixture functions of the fixtures the tests use, in id order.
pub(crate) fn fixtures(
    spec: &Spec,
    used: &BTreeSet<String>,
) -> Result<Vec<TokenStream>, (String, Refusal)> {
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    let mut out = Vec::new();
    for id in used {
        let Some(fx) = spec.example_fixtures.iter().find(|f| &f.id == id) else {
            continue;
        };
        let fail = |r: Refusal| (id.clone(), r);
        let rust = naming::snake(id).unwrap_or_default();
        if let Some(other) = names.insert(rust.clone(), id) {
            return Err(fail(unsupported(format!(
                "the fixtures {other} and {id} both become the Rust function fixture_{rust}"
            ))));
        }
        let ty = ty_of_text(spec, &fx.type_name).map_err(fail)?;
        let rust_type = rust_ty(&ty).map_err(fail)?;
        let value = literal(spec, &ty, &fx.value).map_err(fail)?;
        let name = fixture_fn(id);
        let doc = match &fx.description {
            Some(d) => crate::render::Doc::new(d).tokens(),
            None => {
                let line = format!(" Example fixture `{id}`.");
                quote! { #[doc = #line] }
            }
        };
        out.push(quote! {
            #doc
            fn #name() -> #rust_type {
                #value
            }
        });
    }
    Ok(out)
}

/// A uuid as an example writes it: lowercase hyphenated text, as Exegra and the generated type's
/// `Display` both write it, so the two never disagree on how one prints or orders.
fn uuid_value(v: &Value) -> Option<u128> {
    let Value::String(s) = v else { return None };
    let b = s.as_bytes();
    if b.len() != 36 {
        return None;
    }
    let mut n: u128 = 0;
    for (i, c) in b.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if *c != b'-' {
                return None;
            }
            continue;
        }
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => return None,
        };
        n = (n << 4) | u128::from(d);
    }
    Some(n)
}
