//! Client machines to JavaScript modules for the browser's frontend kernel.
//!
//! An implementation-level machine (`behaviors` with `level: "implementation"`) whose `subject`
//! is a component of category `client` runs in the browser, so it is not generated as Rust (the
//! loader keeps it out of `Spec::machines`, see [`crate::load::load_with_client_machines`]) but
//! as one ES module per machine, `<machine>.js`, beside a shared `machine-runtime.js`. Each
//! module is plain, dependency-free and pure, in the style of the kernel (`web/src/kernel`):
//! `// @ts-check` and JSDoc types, and exports
//!
//! - `initial(key)`: `{ state, vars }` of a new instance;
//! - `step(state, vars, event, now)`: `{ state, vars, emits, timers }`, or `{ ignored: true }`.
//!
//! `event` is `{ event: <event id>, payload }`, `now` is milliseconds since the Unix epoch (what
//! `time` reads), each emit is `{ to, delivery, event, payload }` and each timer command is
//! `{ set, afterMs, event, payload }` or `{ cancel }`; an emit with a delay (`after`) also carries
//! `afterMs`, which the kernel waits before delivering it; `event` in an emit or timer is the event
//! id. Nothing is stored: the machine's state and variables live in the kernel's state, so there
//! is no `to_record` or `from_record`.
//!
//! `step` has the semantics of the Rust backend (`backend/machines.rs`) and of Exegra
//! (`docs/semantics.md`):
//!
//! - among the transitions from the current state on the event, in declaration order, the first
//!   whose guard holds fires (an absent guard holds; `otherwise` is declarative and means the
//!   same); guards read the variables as they were before the step;
//! - its effects run in the order written, each on the variables as updated by the effects
//!   before it; the machine then moves to the target state. The returned `vars` is a new object;
//!   `vars` as passed is never changed;
//! - an event with no enabled transition (the state lists it in `ignores`, no guard holds, the
//!   state is terminal, or the machine does not receive the event at all) is `{ ignored: true }`;
//! - a checked integer operation out of range, or a division by zero, throws `StepError`
//!   (`machine-runtime.js`) and nothing is committed; a `state` that is not one of the machine's
//!   states throws it too.
//!
//! Integers are JavaScript numbers; `u64` and `i64` are limited to the safe-integer range (see
//! `strata-spec-expr`'s `js` module). Timestamps and durations are numbers of milliseconds, and
//! declared ids and scalars are their representation.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::Value;
use strata_spec_expr::{self as expr, ExprKind, JsBinding, JsScope, Ty};

use crate::backend::machines::{
    CROSS_BEHAVIOUR_READ, EFFECT_UNSUPPORTED, EMIT_AFTER, EMIT_LINK, EMIT_UNADDRESSED,
    EVENT_UNKNOWN, EXPRESSION, LITERAL, NO_EVENTS, PAYLOAD_FIELD, RESOURCE_EFFECT, STATE_UNKNOWN,
    TIMER_AFTER, VARIABLE_UNKNOWN,
};
use crate::diagnostic::{dedup, Diagnostic};
use crate::emit::{rust_header, GeneratedFile};
use crate::ir::{Behavior, Effect, Event, Spec, Transition, TypeKind};
use crate::load;
use crate::naming;
use crate::resolve::{self, Builtin, TypeRef};

/// Code of a machine id that cannot name a file or a set of type names.
pub const NAME: &str = "specc.web-name";

/// The file shared by every generated machine.
pub const RUNTIME_FILE: &str = "machine-runtime.js";

/// Generates the JavaScript files of the client machines of a specification (paths relative to
/// the output directory), or refuses with every diagnostic. A specification without client
/// machines gives no files. `spec_path` is the path written into the headers.
pub fn generate(spec_path: &str, text: &str) -> Result<Vec<GeneratedFile>, Vec<Diagnostic>> {
    let (spec, machines) = load::load_with_client_machines(spec_path, text)?;
    if machines.is_empty() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    let mut used = BTreeSet::new();
    for b in &machines {
        let valid = !b.id.is_empty()
            && b.id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let pascal = naming::pascal(&b.id);
        let (true, Ok(pascal)) = (valid, pascal) else {
            diagnostics.push(Diagnostic::new(
                NAME,
                &b.id,
                format!(
                    "Machine \"{}\" cannot name a file: use ASCII letters, digits, - and _.",
                    b.id
                ),
            ));
            continue;
        };
        if format!("{}.js", b.id) == RUNTIME_FILE || !used.insert(pascal.clone()) {
            diagnostics.push(Diagnostic::new(
                NAME,
                &b.id,
                format!(
                    "Machine \"{}\" collides with another file or type name of the generated modules.",
                    b.id
                ),
            ));
            continue;
        }
        let mut gen = Gen::new(&spec, b, pascal);
        match gen.machine() {
            Some(body) => files.push(GeneratedFile {
                path: format!("{}.js", b.id),
                contents: format!("{}\n{body}", rust_header(&spec.path)),
            }),
            None => diagnostics.extend(gen.out),
        }
    }
    if !diagnostics.is_empty() {
        return Err(dedup(diagnostics));
    }
    files.push(GeneratedFile {
        path: RUNTIME_FILE.to_string(),
        contents: format!("{}\n{RUNTIME}", rust_header(&spec.path)),
    });
    files.sort();
    Ok(files)
}

/// The contents of `machine-runtime.js` after its header.
const RUNTIME: &str = r#"// @ts-check
// What the generated machines share: the error a step throws.

export class StepError extends Error {
  /**
   * @param {'overflow' | 'unknown-state'} kind
   * @param {string} message
   */
  constructor(kind, message) {
    super(message);
    this.name = 'StepError';
    /** @type {'overflow' | 'unknown-state'} */
    this.kind = kind;
  }
}

/**
 * The refusal of a step whose arithmetic overflows or divides by zero.
 * @param {string} transition
 * @returns {never}
 */
export function overflow(transition) {
  throw new StepError('overflow', `transition ${transition} overflowed or divided by zero`);
}
"#;

// ---------------------------------------------------------------------------------------------
// Names and types.

/// True when `name` can be written as a JavaScript property without quotes.
fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// A single-quoted JavaScript string literal.
fn sq(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A property name in an object literal or type.
fn key(name: &str) -> String {
    if is_ident(name) {
        name.to_string()
    } else {
        sq(name)
    }
}

/// A member access on a value.
fn member(name: &str) -> String {
    if is_ident(name) {
        format!(".{name}")
    } else {
        format!("[{}]", sq(name))
    }
}

fn builtin_ty(b: Builtin) -> Ty {
    match b {
        Builtin::Bool => Ty::Bool,
        Builtin::U8 => Ty::U8,
        Builtin::U16 => Ty::U16,
        Builtin::U32 => Ty::U32,
        Builtin::U64 => Ty::U64,
        Builtin::I32 => Ty::I32,
        Builtin::I64 => Ty::I64,
        Builtin::F64 => Ty::F64,
        Builtin::String => Ty::String,
        Builtin::Bytes => Ty::Bytes,
        Builtin::Timestamp => Ty::Timestamp,
        Builtin::Duration => Ty::Duration,
        Builtin::Uuid => Ty::Uuid,
    }
}

/// The expression type of a type nested in an option or list, where nothing is unwrapped.
fn raw_ty(t: &TypeRef) -> Ty {
    match t {
        TypeRef::Builtin(b) => builtin_ty(*b),
        TypeRef::Declared(id) => Ty::Named(id.clone()),
        TypeRef::Option(inner) => Ty::Option(Box::new(raw_ty(inner))),
        TypeRef::List(inner) => Ty::List(Box::new(raw_ty(inner))),
    }
}

/// How expressions see a value of type `t`: timestamps and durations as their integers,
/// declared ids and scalars as their representation (as in the Rust backend).
fn expr_ty(spec: &Spec, t: &TypeRef) -> Ty {
    match t {
        TypeRef::Builtin(Builtin::Timestamp) => Ty::I64,
        TypeRef::Builtin(Builtin::Duration) => Ty::U64,
        TypeRef::Builtin(b) => builtin_ty(*b),
        TypeRef::Declared(id) => {
            let repr = spec.type_decl(id).and_then(|d| match &d.kind {
                TypeKind::Id { repr } | TypeKind::Scalar { repr } => {
                    repr.as_deref().and_then(Builtin::parse)
                }
                _ => None,
            });
            match repr {
                Some(b) => expr_ty(spec, &TypeRef::Builtin(b)),
                None => Ty::Named(id.clone()),
            }
        }
        TypeRef::Option(_) | TypeRef::List(_) => raw_ty(t),
    }
}

/// The JSDoc type of a value of type `t`; `nested` parenthesises unions.
fn js_type(spec: &Spec, t: &TypeRef, nested: bool, depth: usize) -> String {
    let wrap = |s: String| if nested { format!("({s})") } else { s };
    if depth > 16 {
        return "unknown".to_string();
    }
    match t {
        TypeRef::Builtin(b) => match b {
            Builtin::Bool => "boolean",
            Builtin::String | Builtin::Uuid => "string",
            Builtin::Bytes => "Uint8Array",
            _ => "number",
        }
        .to_string(),
        TypeRef::List(inner) => format!("{}[]", js_type(spec, inner, true, depth + 1)),
        TypeRef::Option(inner) => {
            wrap(format!("{} | null", js_type(spec, inner, false, depth + 1)))
        }
        TypeRef::Declared(id) => match spec.type_decl(id).map(|d| &d.kind) {
            Some(TypeKind::Id { repr } | TypeKind::Scalar { repr }) => {
                match repr.as_deref().and_then(Builtin::parse) {
                    Some(b) => js_type(spec, &TypeRef::Builtin(b), nested, depth + 1),
                    None => "unknown".to_string(),
                }
            }
            Some(TypeKind::Enum { values }) => {
                let union: Vec<String> = values.iter().map(|v| sq(v)).collect();
                if union.is_empty() {
                    "never".to_string()
                } else if union.len() == 1 {
                    union[0].clone()
                } else {
                    wrap(union.join(" | "))
                }
            }
            Some(TypeKind::Struct { fields }) => {
                let items: Vec<String> = fields
                    .iter()
                    .map(|f| {
                        let t = js_type(spec, &TypeRef::parse(&f.type_name), false, depth + 1);
                        format!("{}{}: {t}", key(&f.name), if f.optional { "?" } else { "" })
                    })
                    .collect();
                object_type(&items)
            }
            None => "unknown".to_string(),
        },
    }
}

fn object_type(items: &[String]) -> String {
    if items.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", items.join(", "))
    }
}

/// Every dotted reference in an expression.
fn references(e: &expr::Expr, out: &mut Vec<Vec<String>>) {
    match &e.kind {
        ExprKind::Lit(_) => {}
        ExprKind::Ref(path) => out.push(path.clone()),
        ExprKind::Un(_, a) => references(a, out),
        ExprKind::Bin(_, a, b) => {
            references(a, out);
            references(b, out);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// One machine.

/// A named value an expression can read: a variable or a payload field.
#[derive(Debug, Clone)]
struct Slot {
    name: String,
    type_name: String,
    optional: bool,
    /// How expressions see it.
    ty: Ty,
}

/// The scope of one transition: `vars` in a guard, `next` in the effects.
struct MachineScope {
    vars: Vec<Slot>,
    payload: Vec<Slot>,
    base: &'static str,
    transition: String,
    used_overflow: Rc<Cell<bool>>,
}

impl JsScope for MachineScope {
    fn resolve_js(&self, path: &[&str]) -> Option<JsBinding> {
        match path {
            ["time"] => Some(JsBinding {
                ty: Ty::I64,
                js: "now".to_string(),
            }),
            [name] => {
                let slot = self.vars.iter().find(|s| s.name == *name)?;
                Some(JsBinding {
                    ty: slot.ty.clone(),
                    js: format!("{}{}", self.base, member(name)),
                })
            }
            ["event", name] => {
                let slot = self.payload.iter().find(|s| s.name == *name)?;
                let read = format!("event.payload{}", member(name));
                if slot.optional {
                    // A field left out reads as the option's None.
                    return Some(JsBinding {
                        ty: Ty::Option(Box::new(raw_ty(&TypeRef::parse(&slot.type_name)))),
                        js: format!("({read} ?? null)"),
                    });
                }
                Some(JsBinding {
                    ty: slot.ty.clone(),
                    js: read,
                })
            }
            _ => None,
        }
    }

    fn overflow_error_js(&self) -> String {
        self.used_overflow.set(true);
        format!("overflow({})", sq(&self.transition))
    }
}

/// One event the machine receives or emits.
#[derive(Clone)]
struct EventInfo<'a> {
    event: &'a Event,
    fields: Vec<Slot>,
}

impl EventInfo<'_> {
    fn payload_type(&self, spec: &Spec) -> String {
        let items: Vec<String> = self
            .fields
            .iter()
            .map(|f| {
                format!(
                    "{}{}: {}",
                    key(&f.name),
                    if f.optional { "?" } else { "" },
                    js_type(spec, &TypeRef::parse(&f.type_name), false, 0)
                )
            })
            .collect();
        object_type(&items)
    }
}

struct Gen<'a> {
    spec: &'a Spec,
    b: &'a Behavior,
    /// The PascalCase prefix of the machine's type names.
    pascal: String,
    out: Vec<Diagnostic>,
    vars: Vec<Slot>,
    received: Vec<EventInfo<'a>>,
    emitted: Vec<EventInfo<'a>>,
    used_overflow: Rc<Cell<bool>>,
    has_cancel: bool,
    timer_events: BTreeSet<String>,
}

impl<'a> Gen<'a> {
    fn new(spec: &'a Spec, b: &'a Behavior, pascal: String) -> Self {
        Gen {
            spec,
            b,
            pascal,
            out: Vec::new(),
            vars: Vec::new(),
            received: Vec::new(),
            emitted: Vec::new(),
            used_overflow: Rc::default(),
            has_cancel: false,
            timer_events: BTreeSet::new(),
        }
    }

    fn refuse(&mut self, code: &str, message: String) {
        self.out.push(Diagnostic::new(code, &self.b.id, message));
    }

    fn ty_name(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.pascal)
    }

    /// The module, or `None` with diagnostics recorded.
    fn machine(&mut self) -> Option<String> {
        let b = self.b;
        let spec = self.spec;
        let errors_before = self.out.len();
        if b.states.is_empty() {
            self.refuse(
                STATE_UNKNOWN,
                format!("Machine \"{}\" has no states.", b.id),
            );
            return None;
        }
        let state_known = |id: &str| b.states.iter().any(|s| s.id == id);
        let initial = match &b.initial {
            Some(i) if state_known(i) => Some(i.clone()),
            Some(i) => {
                self.refuse(
                    STATE_UNKNOWN,
                    format!(
                        "The initial state \"{i}\" of machine \"{}\" is not one of its states.",
                        b.id
                    ),
                );
                None
            }
            None => {
                self.refuse(
                    STATE_UNKNOWN,
                    format!("Machine \"{}\" names no initial state.", b.id),
                );
                None
            }
        };

        // Variables.
        for v in &b.variables {
            let Some(repr) = &v.repr else {
                self.refuse(
                    "ready.repr-missing",
                    format!(
                        "Variable \"{}\" of machine \"{}\" has no repr.",
                        v.name, b.id
                    ),
                );
                continue;
            };
            if !resolve::resolves(spec, repr) {
                self.refuse(
                    "ready.repr-unresolved",
                    format!(
                        "Variable \"{}\" of machine \"{}\" has {}.",
                        v.name,
                        b.id,
                        resolve::unresolved_text(Some(repr))
                    ),
                );
                continue;
            }
            if self.vars.iter().any(|s| s.name == v.name) {
                self.refuse(
                    NAME,
                    format!(
                        "Machine \"{}\" declares variable \"{}\" twice.",
                        b.id, v.name
                    ),
                );
                continue;
            }
            self.vars.push(Slot {
                name: v.name.clone(),
                type_name: repr.clone(),
                optional: false,
                ty: expr_ty(spec, &TypeRef::parse(repr)),
            });
        }

        // Events: what the machine receives and emits, in the order of the events section.
        let mut received_ids = BTreeSet::new();
        let mut emitted_ids = BTreeSet::new();
        for t in &b.transitions {
            if t.fault_only {
                self.refuse(
                    "ready.fault-only",
                    format!("Transition \"{}\" of machine \"{}\" is faultOnly; production code has no faults to enable it.", t.id, b.id),
                );
            }
            received_ids.insert(t.on.as_str());
            for e in &t.effects {
                match e {
                    Effect::Emit { event, .. } => {
                        emitted_ids.insert(event.as_str());
                    }
                    Effect::Timer { event, .. } => {
                        received_ids.insert(event.as_str());
                    }
                    _ => {}
                }
            }
        }
        for s in &b.states {
            for e in &s.ignores {
                received_ids.insert(e.as_str());
            }
        }
        for id in received_ids.iter().chain(emitted_ids.iter()) {
            if spec.event(id).is_none() {
                self.refuse(
                    EVENT_UNKNOWN,
                    format!(
                        "Machine \"{}\" names event \"{id}\", which is not declared.",
                        b.id
                    ),
                );
            }
        }
        for e in &spec.events {
            let receives = received_ids.contains(e.id.as_str());
            let emits = emitted_ids.contains(e.id.as_str());
            if !receives && !emits {
                continue;
            }
            let Some(info) = self.event_info(e) else {
                continue;
            };
            if emits
                && !matches!(
                    e.delivery.as_deref(),
                    Some("at-most-once" | "at-least-once")
                )
            {
                self.refuse(
                    "ready.delivery-missing",
                    format!("Event \"{}\", which machine \"{}\" emits, declares no delivery (at-most-once or at-least-once).", e.id, b.id),
                );
            }
            if receives {
                self.received.push(info.clone());
            }
            if emits {
                self.emitted.push(info);
            }
        }
        if self.received.is_empty() && self.out.len() == errors_before {
            self.refuse(
                NO_EVENTS,
                format!(
                    "Machine \"{}\" receives no event, so it can never step.",
                    b.id
                ),
            );
        }
        for t in &b.transitions {
            for (state, what) in [(&t.from, "source"), (&t.to, "target")] {
                if !state_known(state) {
                    self.refuse(
                        STATE_UNKNOWN,
                        format!("The {what} state \"{state}\" of transition \"{}\" is not a state of machine \"{}\".", t.id, b.id),
                    );
                }
            }
        }

        // The initial variables.
        let mut initial_vars = Vec::new();
        for v in &b.variables {
            let Some(slot) = self.vars.iter().find(|s| s.name == v.name).cloned() else {
                continue;
            };
            let t = TypeRef::parse(&slot.type_name);
            let value = match &v.initial {
                Some(value) => self.literal(value, &t),
                None if matches!(t, TypeRef::Option(_)) => Ok("null".to_string()),
                None => Err("no initial value is given".to_string()),
            };
            match value {
                Ok(value) => initial_vars.push(format!("{}: {value}", key(&v.name))),
                Err(reason) => self.refuse(
                    LITERAL,
                    format!(
                        "The initial value of variable \"{}\" of machine \"{}\" cannot be written: {reason}.",
                        v.name, b.id
                    ),
                ),
            }
        }

        // The step: a switch on the state, then on the event.
        let mut state_cases: Vec<String> = Vec::new();
        let mut empty_states: Vec<&str> = Vec::new();
        for s in &b.states {
            let mut ons: Vec<&str> = Vec::new();
            if !s.terminal {
                for t in &b.transitions {
                    if t.from == s.id && !t.fault_only && !ons.contains(&t.on.as_str()) {
                        ons.push(&t.on);
                    }
                }
            }
            if ons.is_empty() {
                empty_states.push(&s.id);
                continue;
            }
            let mut lines = vec![format!("    case {}:", sq(&s.id))];
            lines.push("      switch (event.event) {".to_string());
            for on in ons {
                let group: Vec<&Transition> = b
                    .transitions
                    .iter()
                    .filter(|t| t.from == s.id && t.on == on && !t.fault_only)
                    .collect();
                let Some(code) = self.group(on, &group) else {
                    continue;
                };
                lines.push(format!("        case {}: {{", sq(on)));
                lines.extend(code.iter().map(|l| format!("          {l}")));
                lines.push("        }".to_string());
            }
            lines.push("        default:".to_string());
            lines.push("          break;".to_string());
            lines.push("      }".to_string());
            lines.push("      break;".to_string());
            state_cases.push(lines.join("\n"));
        }
        if !empty_states.is_empty() {
            let mut lines: Vec<String> = empty_states
                .iter()
                .map(|s| format!("    case {}:", sq(s)))
                .collect();
            lines.push("      break;".to_string());
            state_cases.push(lines.join("\n"));
        }

        if self.out.len() > errors_before {
            return None;
        }
        let initial = initial?;
        Some(self.assemble(&initial, &initial_vars, &state_cases))
    }

    fn event_info(&mut self, e: &'a Event) -> Option<EventInfo<'a>> {
        let b = self.b;
        let mut fields: Vec<Slot> = Vec::new();
        for f in &e.payload {
            let resolved = f
                .type_name
                .as_deref()
                .filter(|t| resolve::resolves(self.spec, t));
            let Some(type_name) = resolved else {
                self.refuse(
                    "ready.payload-type-unresolved",
                    format!(
                        "Payload field \"{}\" of event \"{}\", which machine \"{}\" uses, has {}.",
                        f.name,
                        e.id,
                        b.id,
                        resolve::unresolved_text(f.type_name.as_deref())
                    ),
                );
                continue;
            };
            if fields.iter().any(|s| s.name == f.name) {
                self.refuse(
                    NAME,
                    format!(
                        "Event \"{}\" declares payload field \"{}\" twice.",
                        e.id, f.name
                    ),
                );
                continue;
            }
            fields.push(Slot {
                name: f.name.clone(),
                type_name: type_name.to_string(),
                optional: f.optional,
                ty: expr_ty(self.spec, &TypeRef::parse(type_name)),
            });
        }
        Some(EventInfo { event: e, fields })
    }

    fn scope(&self, payload: &[Slot], transition: &str, base: &'static str) -> MachineScope {
        MachineScope {
            vars: self.vars.clone(),
            payload: payload.to_vec(),
            base,
            transition: transition.to_string(),
            used_overflow: self.used_overflow.clone(),
        }
    }

    /// Parses an expression and refuses references one machine's step cannot read.
    fn parse(&mut self, src: &str, what: &str) -> Option<expr::Expr> {
        let b = self.b;
        let parsed = match expr::parse(src) {
            Ok(e) => e,
            Err(e) => {
                self.refuse(
                    EXPRESSION,
                    format!(
                        "{what} of machine \"{}\" (`{src}`) does not parse: {}.",
                        b.id, e.message
                    ),
                );
                return None;
            }
        };
        let mut refs = Vec::new();
        references(&parsed, &mut refs);
        let mut ok = true;
        for r in refs {
            if r.len() >= 2 && r[0] != "event" {
                self.refuse(
                    CROSS_BEHAVIOUR_READ,
                    format!(
                        "{what} of machine \"{}\" reads `{}` in `{src}`: another behavior's variable, an aggregate or a resource, which one machine's step cannot read.",
                        b.id,
                        r.join(".")
                    ),
                );
                ok = false;
            }
        }
        ok.then_some(parsed)
    }

    /// An expression as JavaScript of the expected type.
    fn value(
        &mut self,
        src: &str,
        expected: &Ty,
        scope: &MachineScope,
        what: &str,
    ) -> Option<String> {
        let parsed = self.parse(src, what)?;
        match expr::to_js_as(&parsed, scope, expected) {
            Ok(js) => Some(js),
            Err(e) => {
                self.refuse(
                    EXPRESSION,
                    format!(
                        "{what} of machine \"{}\" (`{src}`) cannot be generated: {}.",
                        self.b.id, e.message
                    ),
                );
                None
            }
        }
    }

    /// A JSON value as a JavaScript value of type `t`.
    fn literal(&self, value: &Value, t: &TypeRef) -> Result<String, String> {
        let spec = self.spec;
        match (t, value) {
            (TypeRef::Option(_), Value::Null) => Ok("null".to_string()),
            (TypeRef::Option(inner), v) => self.literal(v, inner),
            (TypeRef::List(inner), Value::Array(items)) => {
                let items = items
                    .iter()
                    .map(|i| self.literal(i, inner))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", items.join(", ")))
            }
            (TypeRef::Builtin(Builtin::Bool), Value::Bool(v)) => Ok(v.to_string()),
            (TypeRef::Builtin(Builtin::String | Builtin::Uuid), Value::String(s)) => Ok(sq(s)),
            (TypeRef::Builtin(Builtin::F64), Value::Number(n)) => Ok(n.to_string()),
            (TypeRef::Builtin(b), Value::Number(n)) => {
                let (low, high) = b
                    .integer_range()
                    .ok_or_else(|| format!("{n} is not a literal of this type"))?;
                let v = n
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
                    .ok_or_else(|| format!("{n} is not an integer"))?;
                let safe = (1i128 << 53) - 1;
                let (low, high) = (low.max(-safe), high.min(safe));
                if v < low || v > high {
                    return Err(format!("{v} does not fit the type (JavaScript holds integers exactly up to 2^53 - 1)"));
                }
                Ok(v.to_string())
            }
            (TypeRef::Declared(id), v) => {
                let decl = spec
                    .type_decl(id)
                    .ok_or_else(|| format!("\"{id}\" is not declared"))?;
                match (&decl.kind, v) {
                    (TypeKind::Id { repr } | TypeKind::Scalar { repr }, v) => {
                        let repr = repr
                            .as_deref()
                            .and_then(Builtin::parse)
                            .ok_or_else(|| format!("\"{id}\" has no built-in repr"))?;
                        self.literal(v, &TypeRef::Builtin(repr))
                    }
                    (TypeKind::Enum { values }, Value::String(s)) if values.contains(s) => {
                        Ok(sq(s))
                    }
                    (TypeKind::Struct { fields }, Value::Object(map)) => {
                        if let Some(extra) =
                            map.keys().find(|k| !fields.iter().any(|f| f.name == **k))
                        {
                            return Err(format!("\"{extra}\" is not a field of \"{id}\""));
                        }
                        let mut items = Vec::new();
                        for f in fields {
                            match map.get(&f.name) {
                                Some(v) => items.push(format!(
                                    "{}: {}",
                                    key(&f.name),
                                    self.literal(v, &TypeRef::parse(&f.type_name))?
                                )),
                                None if f.optional => {}
                                None => return Err(format!("field \"{}\" is missing", f.name)),
                            }
                        }
                        Ok(if items.is_empty() {
                            "{}".to_string()
                        } else {
                            format!("{{ {} }}", items.join(", "))
                        })
                    }
                    _ => Err(format!("{v} is not a literal of type \"{id}\"")),
                }
            }
            (_, v) => Err(format!("{v} is not a literal of this type")),
        }
    }

    /// The payload object of an emit or timer, built from its payload entries; an optional
    /// field without a value is left out.
    fn payload(
        &mut self,
        info: &EventInfo<'_>,
        entries: &[(String, Value)],
        scope: &MachineScope,
        what: &str,
    ) -> Option<String> {
        let b = self.b;
        let mut ok = true;
        for (k, _) in entries {
            if !info.fields.iter().any(|f| f.name == *k) {
                self.refuse(
                    PAYLOAD_FIELD,
                    format!("{what} of machine \"{}\" gives payload field \"{k}\", which its event does not declare (or whose type does not resolve).", b.id),
                );
                ok = false;
            }
        }
        let mut items = Vec::new();
        for f in &info.fields {
            let entry = entries.iter().find(|(k, _)| *k == f.name).map(|(_, v)| v);
            let fwhat = format!("{what}, payload field \"{}\",", f.name);
            let value = match entry {
                None if f.optional => continue,
                None => {
                    self.refuse(
                        PAYLOAD_FIELD,
                        format!("{what} of machine \"{}\" gives no value for payload field \"{}\", which is required.", b.id, f.name),
                    );
                    None
                }
                Some(Value::Null) if f.optional => continue,
                Some(Value::String(src)) => self.value(src, &f.ty, scope, &fwhat),
                Some(literal) => match self.literal(literal, &TypeRef::parse(&f.type_name)) {
                    Ok(v) => Some(v),
                    Err(reason) => {
                        self.refuse(
                            LITERAL,
                            format!(
                                "{fwhat} of machine \"{}\" cannot be written: {reason}.",
                                b.id
                            ),
                        );
                        None
                    }
                },
            };
            match value {
                Some(v) => items.push(format!("{}: {v}", key(&f.name))),
                None => ok = false,
            }
        }
        ok.then(|| object_type_value(&items))
    }

    /// The code of one `(state, event)` group, to sit in a `case` block: the transitions in
    /// declaration order, each in an `if` on its guard, until one has none.
    fn group(&mut self, on: &str, group: &[&'a Transition]) -> Option<Vec<String>> {
        let info = self.received.iter().find(|e| e.event.id == on)?.clone();
        let mut lines = Vec::new();
        let mut unconditional = false;
        for t in group {
            let guard = match &t.guard {
                Some(g) => {
                    let what = format!("The guard of transition \"{}\"", t.id);
                    let parsed = self.parse(g, &what);
                    let scope = self.scope(&info.fields, &t.id, "vars");
                    let js = parsed.and_then(|p| match expr::to_js_as(&p, &scope, &Ty::Bool) {
                        Ok(js) => Some(js),
                        Err(e) => {
                            let message = format!(
                                "{what} of machine \"{}\" (`{g}`) cannot be generated: {}.",
                                self.b.id, e.message
                            );
                            self.out
                                .push(Diagnostic::new(EXPRESSION, &self.b.id, message));
                            None
                        }
                    });
                    Some(js?)
                }
                None => None,
            };
            let body = self.body(t, &info.fields)?;
            match guard {
                Some(guard) => {
                    lines.push(format!("if ({guard}) {{"));
                    lines.extend(body.iter().map(|l| format!("  {l}")));
                    lines.push("}".to_string());
                }
                None => {
                    // Declaration order: nothing after an unguarded transition is considered.
                    lines.extend(body);
                    unconditional = true;
                    break;
                }
            }
        }
        if !unconditional {
            lines.push("break;".to_string());
        }
        Some(lines)
    }

    /// One transition: its effects in order, then its target state.
    fn body(&mut self, t: &'a Transition, payload_fields: &[Slot]) -> Option<Vec<String>> {
        let b = self.b;
        let mut lines = vec![
            format!("// {}", t.id),
            "const next = { ...vars };".to_string(),
            format!("/** @type {{{}[]}} */", self.ty_name("Emit")),
            "const emits = [];".to_string(),
            format!("/** @type {{{}[]}} */", self.ty_name("Timer")),
            "const timers = [];".to_string(),
        ];
        let mut ok = true;
        for (i, e) in t.effects.iter().enumerate() {
            let what = format!("Effect {} of transition \"{}\"", i + 1, t.id);
            match e {
                Effect::Set { variable, to } => {
                    let Some(slot) = self.vars.iter().find(|v| v.name == *variable).cloned() else {
                        self.refuse(
                            VARIABLE_UNKNOWN,
                            format!("{what} of machine \"{}\" sets \"{variable}\", which is not one of its variables.", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    let value = match to {
                        Value::String(src) => {
                            let scope = self.scope(payload_fields, &t.id, "next");
                            self.value(src, &slot.ty, &scope, &what)
                        }
                        literal => match self.literal(literal, &TypeRef::parse(&slot.type_name)) {
                            Ok(v) => Some(v),
                            Err(reason) => {
                                self.refuse(
                                    LITERAL,
                                    format!("{what} of machine \"{}\" sets \"{variable}\" to a value that cannot be written: {reason}.", b.id),
                                );
                                None
                            }
                        },
                    };
                    match value {
                        Some(v) => lines.push(format!("next{} = {v};", member(variable))),
                        None => ok = false,
                    }
                }
                Effect::Emit {
                    event,
                    to,
                    target,
                    after,
                    payload,
                    link,
                } => {
                    if !link.is_empty() {
                        self.refuse(
                            EMIT_LINK,
                            format!("{what} of machine \"{}\" emits \"{event}\" over a link ({}); a client machine emits to its receiver directly.", b.id, link.join(", ")),
                        );
                        ok = false;
                    }
                    let delay = match after {
                        None | Some(Value::Null) => Some(0),
                        Some(Value::Number(n)) => n.as_u64().filter(|n| *n < 1 << 53),
                        Some(_) => None,
                    };
                    let Some(delay) = delay else {
                        self.refuse(
                            EMIT_AFTER,
                            format!("{what} of machine \"{}\" emits \"{event}\" after a delay that is not a whole number of milliseconds; generated code delays an emit by a fixed number of milliseconds only.", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    let Some(receiver) = to.clone() else {
                        let how = match target.as_deref() {
                            Some("reply") => "replies to the sender".to_string(),
                            Some(other) => format!("is addressed by target \"{other}\""),
                            None => "is addressed by scenario routes".to_string(),
                        };
                        self.refuse(
                            EMIT_UNADDRESSED,
                            format!("{what} of machine \"{}\" emits \"{event}\", which {how}; generated code addresses an emit only by its \"to\" (a component or contract id).", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    let Some(info) = self.emitted.iter().find(|e| e.event.id == *event).cloned()
                    else {
                        ok = false;
                        continue;
                    };
                    let delivery = match info.event.delivery.as_deref() {
                        Some("at-least-once") => "at-least-once",
                        _ => "at-most-once",
                    };
                    let scope = self.scope(payload_fields, &t.id, "next");
                    let Some(p) = self.payload(&info, payload, &scope, &what) else {
                        ok = false;
                        continue;
                    };
                    // A delayed emit carries `afterMs`: the kernel delivers it that long after the step.
                    let after_ms = if delay > 0 {
                        format!(", afterMs: {delay}")
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "emits.push({{ to: {}, delivery: {}, event: {}, payload: {p}{after_ms} }});",
                        sq(&receiver),
                        sq(delivery),
                        sq(event)
                    ));
                }
                Effect::Timer {
                    name,
                    after,
                    event,
                    payload,
                } => {
                    let Some(millis) = after.as_ref().and_then(Value::as_u64) else {
                        self.refuse(
                            TIMER_AFTER,
                            format!("{what} of machine \"{}\" sets timer \"{name}\" with an \"after\" that is not a whole number of milliseconds.", b.id),
                        );
                        ok = false;
                        continue;
                    };
                    if millis > (1u64 << 53) - 1 {
                        self.refuse(
                            TIMER_AFTER,
                            format!("{what} of machine \"{}\" sets timer \"{name}\" with an \"after\" beyond JavaScript's exact integers.", b.id),
                        );
                        ok = false;
                        continue;
                    }
                    let Some(info) = self.received.iter().find(|e| e.event.id == *event).cloned()
                    else {
                        ok = false;
                        continue;
                    };
                    let scope = self.scope(payload_fields, &t.id, "next");
                    let Some(p) = self.payload(&info, payload, &scope, &what) else {
                        ok = false;
                        continue;
                    };
                    self.timer_events.insert(event.clone());
                    lines.push(format!(
                        "timers.push({{ set: {}, afterMs: {millis}, event: {}, payload: {p} }});",
                        sq(name),
                        sq(event)
                    ));
                }
                Effect::Cancel { name } => {
                    self.has_cancel = true;
                    lines.push(format!("timers.push({{ cancel: {} }});", sq(name)));
                }
                Effect::Other { kind } => {
                    let code = if matches!(kind.as_str(), "use" | "acquire" | "release") {
                        RESOURCE_EFFECT
                    } else {
                        EFFECT_UNSUPPORTED
                    };
                    self.refuse(
                        code,
                        format!("{what} of machine \"{}\" is a \"{kind}\" effect, which a client machine cannot express.", b.id),
                    );
                    ok = false;
                }
            }
        }
        if !ok {
            return None;
        }
        lines.push(format!(
            "return {{ state: {}, vars: next, emits, timers }};",
            sq(&t.to)
        ));
        Some(lines)
    }

    fn assemble(
        &mut self,
        initial: &str,
        initial_vars: &[String],
        state_cases: &[String],
    ) -> String {
        let b = self.b;
        let spec = self.spec;
        let (state_t, vars_t, event_t, emit_t, timer_t, outcome_t) = (
            self.ty_name("State"),
            self.ty_name("Vars"),
            self.ty_name("Event"),
            self.ty_name("Emit"),
            self.ty_name("Timer"),
            self.ty_name("Outcome"),
        );
        let mut out = String::new();
        out.push_str("// @ts-check\n");
        out.push_str(&format!("// Machine `{}`: {}.\n", b.id, b.name));
        if let Some(d) = &b.description {
            for line in d.lines() {
                out.push_str(&format!("// {line}\n").replace("// \n", "//\n"));
            }
        }
        out.push_str(&format!(
            "// Specification behavior `{}`, an implementation-level machine run in the browser.\n",
            b.id
        ));
        out.push('\n');
        if self.used_overflow.get() {
            out.push_str("import { overflow, StepError } from './machine-runtime.js';\n\n");
        } else {
            out.push_str("import { StepError } from './machine-runtime.js';\n\n");
        }

        // Types.
        let states = vec![b
            .states
            .iter()
            .map(|s| sq(&s.id))
            .collect::<Vec<_>>()
            .join(" | ")];
        out.push_str(&typedef(
            &format!("The states of machine `{}`.", b.id),
            &state_t,
            &states,
        ));
        let vars_items: Vec<String> = self
            .vars
            .iter()
            .map(|v| {
                format!(
                    "{}: {}",
                    key(&v.name),
                    js_type(spec, &TypeRef::parse(&v.type_name), false, 0)
                )
            })
            .collect();
        out.push_str(&typedef(
            &format!("The variables of machine `{}`.", b.id),
            &vars_t,
            &[object_type(&vars_items)],
        ));
        let events: Vec<String> = self
            .received
            .iter()
            .map(|e| {
                let payload = e.payload_type(spec);
                if e.fields.is_empty() {
                    format!("{{ event: {}, payload?: {payload} }}", sq(&e.event.id))
                } else {
                    format!("{{ event: {}, payload: {payload} }}", sq(&e.event.id))
                }
            })
            .collect();
        out.push_str(&typedef(
            &format!(
                "The events machine `{}` receives, including those its timers fire.",
                b.id
            ),
            &event_t,
            &events,
        ));
        let emits: Vec<String> = self
            .emitted
            .iter()
            .map(|e| {
                let delivery = match e.event.delivery.as_deref() {
                    Some("at-least-once") => "at-least-once",
                    _ => "at-most-once",
                };
                format!(
                    "{{ to: string, delivery: {}, event: {}, payload: {}, afterMs?: number }}",
                    sq(delivery),
                    sq(&e.event.id),
                    e.payload_type(spec)
                )
            })
            .collect();
        out.push_str(&typedef(
            &format!("An event machine `{}` asks the kernel to deliver.", b.id),
            &emit_t,
            &emits,
        ));
        let mut timers: Vec<String> = self
            .received
            .iter()
            .filter(|e| self.timer_events.contains(&e.event.id))
            .map(|e| {
                format!(
                    "{{ set: string, afterMs: number, event: {}, payload: {} }}",
                    sq(&e.event.id),
                    e.payload_type(spec)
                )
            })
            .collect();
        if self.has_cancel {
            timers.push("{ cancel: string }".to_string());
        }
        out.push_str(&typedef(
            &format!(
                "A timer command of machine `{}`: set (replacing a timer of the same name) or cancel.",
                b.id
            ),
            &timer_t,
            &timers,
        ));
        out.push_str(&typedef(
            "What a step does: the new state and variables with what to emit and which timers to change, or nothing at all when the event is ignored.",
            &outcome_t,
            &[
                format!(
                    "{{ state: {state_t}, vars: {vars_t}, emits: {emit_t}[], timers: {timer_t}[] }}"
                ),
                "{ ignored: true }".to_string(),
            ],
        ));

        // initial.
        out.push_str(&format!(
            "/**\n * The state and variables of a new instance.\n * @param {{string}} [_key] The instance key; unused, the state lives in the kernel.\n * @returns {{{{ state: {state_t}, vars: {vars_t} }}}}\n */\n"
        ));
        out.push_str("export function initial(_key) {\n");
        out.push_str(&format!(
            "  return {{ state: {}, vars: {} }};\n}}\n\n",
            sq(initial),
            object_type_value(initial_vars)
        ));

        // step.
        out.push_str(&format!(
            "/**\n * Steps the machine on an event at `now`, milliseconds since the Unix epoch.\n * @param {{{state_t}}} state\n * @param {{{vars_t}}} vars\n * @param {{{event_t}}} event\n * @param {{number}} now\n * @returns {{{outcome_t}}}\n * @throws {{StepError}} when arithmetic overflows or divides by zero.\n */\n"
        ));
        out.push_str("export function step(state, vars, event, now) {\n");
        out.push_str("  switch (state) {\n");
        for c in state_cases {
            out.push_str(c);
            out.push('\n');
        }
        out.push_str("    default:\n");
        out.push_str(&format!(
            "      throw new StepError('unknown-state', `machine {} has no state ${{state}}`);\n",
            b.id
        ));
        out.push_str("  }\n");
        out.push_str("  return { ignored: true };\n}\n");
        out
    }
}

/// An object literal from its `key: value` items.
fn object_type_value(items: &[String]) -> String {
    if items.is_empty() {
        "{}".to_string()
    } else {
        format!("{{ {} }}", items.join(", "))
    }
}

/// A JSDoc `@typedef` of the union of `members`, with its description.
fn typedef(doc: &str, name: &str, members: &[String]) -> String {
    let union = match members {
        [] => "never".to_string(),
        [only] => only.clone(),
        many => many.join("\n *   | "),
    };
    format!("/**\n * {doc}\n * @typedef {{{union}}} {name}\n */\n\n")
}
