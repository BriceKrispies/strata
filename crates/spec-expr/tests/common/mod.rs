#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::format_ident;
use std::collections::HashMap;
use strata_spec_expr::{Binding, Scope, Ty};

/// A scope of named references. The Rust expression of `a.b-c` is the identifier `a_b_c`.
pub struct TestScope {
    vars: HashMap<String, Ty>,
}

pub fn scope(vars: &[(&str, Ty)]) -> TestScope {
    TestScope {
        vars: vars
            .iter()
            .map(|(n, t)| ((*n).to_string(), t.clone()))
            .collect(),
    }
}

impl Scope for TestScope {
    fn resolve(&self, path: &[&str]) -> Option<Binding> {
        let key = path.join(".");
        let ty = self.vars.get(&key)?.clone();
        let ident = format_ident!("{}", key.replace(['.', '-'], "_"));
        Some(Binding {
            ty,
            tokens: quote::quote!(#ident),
        })
    }

    fn overflow_error(&self) -> TokenStream {
        "StepError::Overflow".parse().unwrap()
    }
}

/// The token text of Rust source, so that expectations are written as ordinary Rust.
pub fn tok(src: &str) -> String {
    src.parse::<TokenStream>().unwrap().to_string()
}

pub fn ty_of_ok(src: &str, s: &TestScope) -> Ty {
    let e = strata_spec_expr::parse(src).unwrap();
    strata_spec_expr::type_of(&e, s).unwrap()
}

pub fn rust(src: &str, s: &TestScope) -> String {
    let e = strata_spec_expr::parse(src).unwrap();
    strata_spec_expr::to_rust(&e, s).unwrap().to_string()
}

pub fn rust_as(src: &str, s: &TestScope, ty: &Ty) -> String {
    let e = strata_spec_expr::parse(src).unwrap();
    strata_spec_expr::to_rust_as(&e, s, ty).unwrap().to_string()
}

pub fn type_err(src: &str, s: &TestScope) -> strata_spec_expr::TypeError {
    let e = strata_spec_expr::parse(src).unwrap();
    strata_spec_expr::to_rust(&e, s).unwrap_err()
}
