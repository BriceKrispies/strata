//! Wire names: how generated code names the JSON members of what crosses a contract.
//!
//! The specification names fields in kebab-case. `system.generation.wireCase` (`snake_case`,
//! `kebab-case` or `camelCase`; kebab-case, the names as written, when unstated) says how they
//! are named on the wire, and a contract may override it with `binding.case`. The wire case
//! governs:
//!
//! - the members of a contract's request and response, and the tags of its errors;
//! - the members of every struct type those reach, through `list<T>`, `option<T>` and struct
//!   fields (transitively), and of an error's payload type;
//! - the payload members of an event that names the contract (`event.contract`), and the struct
//!   types those reach.
//!
//! Nothing else changes: records, machine variables and events that cross no contract keep the
//! specification's names. A struct type reached under two cases that name its fields differently
//! has no single wire form, and is refused ([`WIRE_CASE_CONFLICT`]).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::diagnostic::Diagnostic;
use crate::ir::{Contract, Event, Spec, StructField, TypeKind};
use crate::resolve::TypeRef;

/// Code of a struct type reached under two wire cases that name its fields differently.
pub const WIRE_CASE_CONFLICT: &str = "specc.wire-case-conflict";

/// How JSON members are named on the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WireCase {
    /// `snake_case`: words joined by `_`.
    Snake,
    /// `kebab-case`: the names exactly as the specification writes them.
    #[default]
    Kebab,
    /// `camelCase`: the first word in lower case, each later word capitalised.
    Camel,
}

impl WireCase {
    /// Reads the specification's value.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "snake_case" => Some(Self::Snake),
            "kebab-case" => Some(Self::Kebab),
            "camelCase" => Some(Self::Camel),
            _ => None,
        }
    }

    /// The value as the specification writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snake => "snake_case",
            Self::Kebab => "kebab-case",
            Self::Camel => "camelCase",
        }
    }

    /// The wire name of a specification name. Kebab-case keeps the name as written. The other
    /// cases split it into words at every character that is not an ASCII letter or digit and at
    /// every change from lower to upper case (as Rust names are made), then join them; a name
    /// with no such words is kept as written.
    pub fn name(self, spec_name: &str) -> String {
        if self == Self::Kebab {
            return spec_name.to_string();
        }
        let words = words(spec_name);
        if words.is_empty() {
            return spec_name.to_string();
        }
        match self {
            Self::Snake => words.join("_"),
            _ => {
                let mut out = words[0].clone();
                for w in &words[1..] {
                    let mut chars = w.chars();
                    if let Some(first) = chars.next() {
                        out.push(first.to_ascii_uppercase());
                        out.push_str(chars.as_str());
                    }
                }
                out
            }
        }
    }
}

/// Lower-case words of a name, split at non-alphanumeric characters and at lower-to-upper
/// changes. Unlike [`crate::naming::words`] it never fails: non-ASCII letters stay in their word.
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for run in name.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = run.chars().collect();
        let mut word = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 {
                let prev = chars[i - 1];
                let next = chars.get(i + 1).copied();
                let lower_to_upper =
                    (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && c.is_ascii_uppercase();
                let acronym_end = prev.is_ascii_uppercase()
                    && c.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase());
                if (lower_to_upper || acronym_end) && !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            word.push(c.to_ascii_lowercase());
        }
        if !word.is_empty() {
            out.push(word);
        }
    }
    out
}

/// The wire case of a contract: its `binding.case`, else the system's.
pub fn contract_case(spec: &Spec, c: &Contract) -> WireCase {
    c.binding_case.unwrap_or(spec.system.wire_case)
}

/// The wire case of an event's payload: its contract's when it names one, else kebab-case.
pub fn event_case(spec: &Spec, e: &Event) -> WireCase {
    e.contract
        .as_deref()
        .and_then(|id| spec.contracts.iter().find(|c| c.id == id))
        .map_or(WireCase::Kebab, |c| contract_case(spec, c))
}

/// Every type name a contract's payloads use, with the contract's case: its inputs, outputs and
/// error payloads, and the payloads of the events that name it.
fn roots(spec: &Spec) -> Vec<(String, WireCase, String)> {
    let mut out = Vec::new();
    for c in &spec.contracts {
        let case = contract_case(spec, c);
        let what = format!("contract \"{}\"", c.id);
        for t in c
            .inputs
            .iter()
            .chain(&c.outputs)
            .filter_map(|f| f.type_name.as_deref())
            .chain(c.errors.iter().filter_map(|e| e.type_name.as_deref()))
        {
            out.push((t.to_string(), case, what.clone()));
        }
    }
    for e in &spec.events {
        if e.contract.is_none() {
            continue;
        }
        let case = event_case(spec, e);
        let what = format!("event \"{}\"", e.id);
        for t in e.payload.iter().filter_map(|f| f.type_name.as_deref()) {
            out.push((t.to_string(), case, what.clone()));
        }
    }
    out
}

/// The struct types a contract payload reaches, each with the case of the first payload (in
/// declaration order) that reaches it, and the conflicts: a struct reached under a second case
/// that names one of its fields differently.
fn walk(spec: &Spec) -> (BTreeMap<String, (WireCase, String)>, Vec<Diagnostic>) {
    let mut cases: BTreeMap<String, (WireCase, String)> = BTreeMap::new();
    let mut out = Vec::new();
    for (root, case, what) in roots(spec) {
        let mut stack = vec![TypeRef::parse(&root)];
        while let Some(t) = stack.pop() {
            match t {
                TypeRef::List(inner)
                | TypeRef::Set(inner)
                | TypeRef::Option(inner)
                | TypeRef::Shared(inner) => stack.push(*inner),
                TypeRef::Map(key, value) => {
                    stack.push(*key);
                    stack.push(*value);
                }
                TypeRef::Builtin(_) => {}
                TypeRef::Declared(id) => {
                    let fields: Vec<&StructField> = match spec.type_decl(&id).map(|d| &d.kind) {
                        Some(TypeKind::Struct { fields }) => fields.iter().collect(),
                        Some(TypeKind::Union { variants, .. }) => {
                            variants.iter().flat_map(|v| &v.fields).collect()
                        }
                        _ => continue,
                    };
                    match cases.get(&id) {
                        Some((seen, _)) if *seen == case => continue,
                        Some((seen, first)) => {
                            let differs = fields
                                .iter()
                                .any(|f| seen.name(&f.name) != case.name(&f.name));
                            if differs {
                                out.push(Diagnostic::new(
                                    WIRE_CASE_CONFLICT,
                                    &id,
                                    format!(
                                        "Struct \"{id}\" crosses {first} in {} and {what} in {}, which name its fields differently on the wire; give the contracts one wire case (binding.case) or use separate types.",
                                        seen.as_str(),
                                        case.as_str()
                                    ),
                                ));
                            }
                            continue;
                        }
                        None => {
                            cases.insert(id.clone(), (case, what.clone()));
                        }
                    }
                    stack.extend(fields.iter().map(|f| TypeRef::parse(&f.type_name)));
                }
            }
        }
    }
    (cases, out)
}

/// The wire case of each struct type a contract payload reaches; a struct no contract reaches
/// is not in the map and keeps the specification's names.
pub fn type_cases(spec: &Spec) -> BTreeMap<String, WireCase> {
    walk(spec)
        .0
        .into_iter()
        .map(|(id, (case, _))| (id, case))
        .collect()
}

/// The refusals of wire naming: struct types with no single wire form.
pub fn check(spec: &Spec) -> Vec<Diagnostic> {
    let mut out = walk(spec).1;
    out.dedup();
    out
}

/// Converts JSON written with the specification's names (as Exegra's traces are) to the wire
/// form generated code serialises: the members of every struct a contract reaches in its wire
/// case, and the payload members of an event that names a contract in that contract's case.
pub struct WireValues<'a> {
    spec: &'a Spec,
    cases: &'a BTreeMap<String, WireCase>,
}

impl<'a> WireValues<'a> {
    /// The converter of one specification, whose [`type_cases`] are `cases`.
    pub fn new(spec: &'a Spec, cases: &'a BTreeMap<String, WireCase>) -> Self {
        Self { spec, cases }
    }

    /// A value of type `t`; members that are not fields of their struct are kept as they are.
    pub fn value(&self, t: &TypeRef, v: &Value) -> Value {
        match (t, v) {
            (TypeRef::Option(inner) | TypeRef::Shared(inner), v) => self.value(inner, v),
            (TypeRef::List(inner) | TypeRef::Set(inner), Value::Array(items)) => {
                Value::Array(items.iter().map(|i| self.value(inner, i)).collect())
            }
            (TypeRef::Map(_, value), Value::Object(map)) => Value::Object(
                map.iter()
                    .map(|(k, m)| (k.clone(), self.value(value, m)))
                    .collect(),
            ),
            (TypeRef::Declared(id), Value::Object(map)) => {
                let Some(TypeKind::Struct { fields }) = self.spec.type_decl(id).map(|d| &d.kind)
                else {
                    return v.clone();
                };
                let case = self.cases.get(id).copied().unwrap_or_default();
                let mut out = serde_json::Map::new();
                for (k, member) in map {
                    match fields.iter().find(|f| f.name == *k) {
                        Some(f) => out.insert(
                            case.name(k),
                            self.value(&TypeRef::parse(&f.type_name), member),
                        ),
                        None => out.insert(k.clone(), member.clone()),
                    };
                }
                Value::Object(out)
            }
            _ => v.clone(),
        }
    }

    /// The payload of event `event`.
    pub fn payload(&self, event: &str, payload: &Value) -> Value {
        let (Some(e), Value::Object(map)) = (self.spec.event(event), payload) else {
            return payload.clone();
        };
        let case = event_case(self.spec, e);
        let mut out = serde_json::Map::new();
        for (k, member) in map {
            match e.payload.iter().find(|f| f.name == *k) {
                Some(f) => out.insert(
                    case.name(k),
                    match f.type_name.as_deref() {
                        Some(t) => self.value(&TypeRef::parse(t), member),
                        None => member.clone(),
                    },
                ),
                None => out.insert(k.clone(), member.clone()),
            };
        }
        Value::Object(out)
    }

    /// The variables of machine `machine`: variable names stay, their values are converted.
    pub fn vars(&self, machine: &str, vars: &Value) -> Value {
        let (Some(b), Value::Object(map)) =
            (self.spec.machines.iter().find(|b| b.id == machine), vars)
        else {
            return vars.clone();
        };
        let mut out = serde_json::Map::new();
        for (k, member) in map {
            let t = b
                .variables
                .iter()
                .find(|v| v.name == *k)
                .and_then(|v| v.repr.as_deref());
            out.insert(
                k.clone(),
                match t {
                    Some(t) => self.value(&TypeRef::parse(t), member),
                    None => member.clone(),
                },
            );
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_case() {
        assert_eq!(WireCase::Snake.name("article-id"), "article_id");
        assert_eq!(WireCase::Snake.name("heartbeatMs"), "heartbeat_ms");
        assert_eq!(WireCase::Camel.name("cover-alt"), "coverAlt");
        assert_eq!(WireCase::Camel.name("depends_on"), "dependsOn");
        assert_eq!(WireCase::Kebab.name("depends_on"), "depends_on");
        assert_eq!(WireCase::Kebab.name("cover-alt"), "cover-alt");
        assert_eq!(WireCase::Snake.name("v2-id"), "v2_id");
        assert_eq!(WireCase::Snake.name("--"), "--");
    }

    #[test]
    fn cases_parse_as_the_specification_writes_them() {
        for c in [WireCase::Snake, WireCase::Kebab, WireCase::Camel] {
            assert_eq!(WireCase::parse(c.as_str()), Some(c));
        }
        assert_eq!(WireCase::parse("PascalCase"), None);
        assert_eq!(WireCase::default(), WireCase::Kebab);
    }
}
