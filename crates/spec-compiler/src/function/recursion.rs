//! Structural recursion (Exegra WG-4957, `selfRecursion` in `src/core/function.js`): whether
//! every call a typed function makes of itself passes, at one parameter position shared by all
//! of them, a strict sub-value of that parameter, whose type is a union.
//!
//! A strict sub-value is a name bound by a `match` on the parameter (or on a sub-value of it) to
//! a field of one of its union variants, an item of a list such a name holds (`x in items`), an
//! item or value such a container gives (`values`, `get`, `find`, `filter`, `take`, `sortBy`),
//! or a field read from any of these; `Some(x)` and `Ok(x)` of a sub-value bind a value as strict
//! as the matched one, and a `let` passes a sub-value on. Each call then works on a strictly
//! smaller value than its caller, so the recursion ends on every finite value, and the generated
//! function is ordinary recursive Rust. Exegra bounds neither the depth nor the steps of an
//! evaluation, so the generated code counts neither: the two cannot disagree on a runaway,
//! because neither admits one.
//!
//! Mutual recursion (a cycle through two or more functions) is refused, as Exegra refuses it.

use super::syntax::{Expr, Kind, Pattern};
use super::typing::{unshared, Sig, Ty};

/// The operations whose result holds only items or values of their container.
const SUB_VALUE_OPS: &[&str] = &["values", "get", "find", "filter", "take", "sortBy"];

/// Where a name's value comes from: the parameter at `i`, and whether it is strictly smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Origin {
    i: usize,
    strict: bool,
}

type Env = Vec<(String, Option<Origin>)>;

fn get(env: &Env, name: &str) -> Option<Origin> {
    env.iter()
        .rev()
        .find(|(n, _)| n == name)
        .and_then(|(_, o)| *o)
}

fn bind(env: &Env, name: &str, o: Option<Origin>) -> Env {
    let mut e = env.clone();
    e.push((name.to_string(), o));
    e
}

fn strict(o: Option<Origin>) -> Option<Origin> {
    o.map(|o| Origin {
        i: o.i,
        strict: true,
    })
}

/// What a function's calls of itself are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfRecursion {
    /// Why they are not structural; `None` when they are (or there are none).
    pub problem: Option<String>,
}

/// The origin of an expression's value.
fn origin(e: &Expr, env: &Env) -> Option<Origin> {
    match &e.kind {
        Kind::Ref(path, _) => {
            let o = get(env, &path[0])?;
            if path.len() > 1 {
                strict(Some(o))
            } else {
                Some(o)
            }
        }
        Kind::Field(a, _) => strict(origin(a, env)),
        Kind::Op(op) if SUB_VALUE_OPS.contains(&op.name.as_str()) => {
            let src = match &op.binder {
                Some((_, src)) => src.as_ref(),
                None => op.args.first()?,
            };
            strict(origin(src, env))
        }
        _ => None,
    }
}

struct Walk<'a> {
    me: &'a str,
    /// For each call of itself, the parameter positions given a strict sub-value of themselves.
    found: Vec<Vec<usize>>,
}

impl Walk<'_> {
    fn walk(&mut self, e: &Expr, env: &Env) {
        match &e.kind {
            Kind::Let(name, value, body) => {
                self.walk(value, env);
                let o = origin(value, env);
                self.walk(body, &bind(env, name, o));
            }
            Kind::Match(scrutinee, arms) => {
                self.walk(scrutinee, env);
                let o = origin(scrutinee, env);
                let st = scrutinee.ty();
                let sub = match unshared(&st) {
                    _ if o.is_none() => None,
                    Ty::Union(_) => strict(o),
                    Ty::Option(_) | Ty::Result(..) => o,
                    _ => None,
                };
                for arm in arms {
                    let mut env = env.clone();
                    if let Pattern::Case {
                        bind: b, fields, ..
                    } = &arm.pat
                    {
                        if let Some(b) = b {
                            if b != "_" {
                                env = bind(&env, b, sub);
                            }
                        }
                        for (_, b) in fields.iter().flatten() {
                            if b != "_" {
                                env = bind(&env, b, sub);
                            }
                        }
                    }
                    self.walk(&arm.body, &env);
                }
            }
            Kind::Op(op) if op.binder.is_some() => {
                let (name, src) = op.binder.as_ref().expect("checked");
                self.walk(src, env);
                let o = origin(src, env);
                let mut inner = bind(env, name, strict(o));
                if let Some((acc, init)) = &op.acc {
                    self.walk(init, env);
                    inner = bind(&inner, acc, None);
                }
                for a in &op.args {
                    self.walk(a, &inner);
                }
            }
            Kind::Call(f, args) => {
                for a in args {
                    self.walk(a, env);
                }
                if f != self.me {
                    return;
                }
                let ok = args
                    .iter()
                    .enumerate()
                    .filter(|(j, a)| origin(a, env).is_some_and(|o| o.i == *j && o.strict))
                    .map(|(j, _)| j)
                    .collect();
                self.found.push(ok);
            }
            _ => super::syntax::each_child(e, &mut |c| self.walk(c, env)),
        }
    }
}

/// Whether the calls a typed function makes of itself are structural.
pub(crate) fn self_recursion(sig: &Sig, lets: &[(String, Expr)], body: &Expr) -> SelfRecursion {
    let mut w = Walk {
        me: &sig.id,
        found: Vec::new(),
    };
    let mut env: Env = sig
        .params
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.clone(), Some(Origin { i, strict: false })))
        .collect();
    for (name, e) in lets {
        w.walk(e, &env);
        let o = origin(e, &env);
        env = bind(&env, name, o);
    }
    w.walk(body, &env);
    if w.found.is_empty() {
        return SelfRecursion { problem: None };
    }
    let unions: Vec<usize> = sig
        .params
        .iter()
        .enumerate()
        .filter(|(_, (_, t))| matches!(unshared(t), Ty::Union(_)))
        .map(|(i, _)| i)
        .collect();
    let shared = unions
        .iter()
        .any(|i| w.found.iter().all(|ok| ok.contains(i)));
    let problem = if shared {
        None
    } else if unions.is_empty() {
        Some("no parameter is a union, so no call can be on a strict sub-value of one".to_string())
    } else {
        let names: Vec<&str> = unions.iter().map(|i| sig.params[*i].0.as_str()).collect();
        Some(format!(
            "no union parameter ({}) is given a strict sub-value of itself in every self-call: a field of a variant matched from it, or an item of a list in such a field",
            names.join(", ")
        ))
    };
    SelfRecursion { problem }
}
