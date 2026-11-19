//! The typed, owned form of the parts of a specification specc reads.
//!
//! Every entity keeps its specification id for diagnostics, and every list keeps declaration
//! order, which is the order of the generated code. Properties that the readiness checks
//! require are still optional here, so that loading never fails on them and the checks can
//! name every gap at once.

use serde_json::{Number, Value};

/// A loaded specification.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    /// The specification's path as given on the command line, with forward slashes.
    pub path: String,
    /// The `system` section.
    pub system: System,
    /// The `components` section.
    pub components: Vec<Component>,
    /// The `types` section.
    pub types: Vec<TypeDecl>,
    /// The `records` section.
    pub records: Vec<Record>,
    /// The `contracts` section.
    pub contracts: Vec<Contract>,
    /// The `events` section.
    pub events: Vec<Event>,
    /// The behaviors with `level: "implementation"`, in declaration order. Model-level
    /// behaviors are not loaded: nothing is generated from them.
    pub machines: Vec<Behavior>,
    /// The cross-cutting constructs: policies, validations, retentions, quotas, retries, audits.
    pub constructs: crate::constructs::Constructs,
    /// `system.telemetry` and the sensitive types: what the telemetry backend needs.
    pub telemetry: Telemetry,
}

/// `system.telemetry` and the types a log line may never hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Telemetry {
    /// The services that write a structured log, in declaration order.
    pub services: Vec<TelemetryService>,
    /// `(type id, kind)` of every declared type marked `sensitive`, or with a struct field
    /// marked `sensitive`.
    pub sensitive_types: Vec<(String, String)>,
}

/// A service of `system.telemetry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryService {
    /// The service name written on every line.
    pub id: String,
    /// Where the deployment keeps its log, relative to its working directory.
    pub log: Option<String>,
}

/// `event.logging` of an event that is logged (`logged: true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logging {
    /// The service whose log holds the event.
    pub service: String,
    /// `debug`, `info`, `warn` or `error`.
    pub level: String,
    /// The line's `event` member: `name`, or the event id.
    pub name: String,
    /// The payload field written as the line's `tenant`.
    pub tenant: Option<String>,
    /// The allowlisted payload fields, in order.
    pub fields: Vec<LoggedField>,
}

/// One allowlisted payload field of a logged event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedField {
    /// The payload field.
    pub field: String,
    /// The member name it is written under (`as`, or the field name).
    pub written_as: String,
}

impl Spec {
    /// The declared type with this id.
    pub fn type_decl(&self, id: &str) -> Option<&TypeDecl> {
        self.types.iter().find(|t| t.id == id)
    }

    /// The record with this id.
    pub fn record(&self, id: &str) -> Option<&Record> {
        self.records.iter().find(|r| r.id == id)
    }

    /// The component with this id.
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }

    /// The event with this id.
    pub fn event(&self, id: &str) -> Option<&Event> {
        self.events.iter().find(|e| e.id == id)
    }
}

/// The `system` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct System {
    /// The system id.
    pub id: String,
    /// The system name.
    pub name: String,
    /// `system.generation.target`, when the specification declares it ready to generate from.
    pub generation_target: Option<String>,
    /// `system.generation.wireCase`: how contract payloads name their JSON members; kebab-case
    /// (the names as written) when unstated.
    pub wire_case: crate::wire::WireCase,
}

/// A component; only what the contract and wiring backends name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// The component id.
    pub id: String,
    /// The component name.
    pub name: String,
    /// The `to` of every relationship of kind `calls` from this component, in the order of the
    /// relationships section, each once.
    pub calls: Vec<String>,
}

/// A `type` entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecl {
    /// The type id, which other entities use as a type name.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The description, if any.
    pub description: Option<String>,
    /// What kind of type it is.
    pub kind: TypeKind,
}

/// The four kinds of declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    /// An identifier over a built-in representation.
    Id {
        /// The representation, which must be built-in.
        repr: Option<String>,
    },
    /// A scalar over a built-in representation.
    Scalar {
        /// The representation, which must be built-in.
        repr: Option<String>,
    },
    /// An enumeration of string values.
    Enum {
        /// The values, in declaration order.
        values: Vec<String>,
    },
    /// A struct of named fields.
    Struct {
        /// The fields, in declaration order.
        fields: Vec<StructField>,
    },
}

impl TypeKind {
    /// The kind as the specification writes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Id { .. } => "id",
            Self::Scalar { .. } => "scalar",
            Self::Enum { .. } => "enum",
            Self::Struct { .. } => "struct",
        }
    }
}

/// A field of a struct type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructField {
    /// The field name.
    pub name: String,
    /// The type name.
    pub type_name: String,
    /// True when the field may be left out.
    pub optional: bool,
}

/// A `record` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The record id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The owning component.
    pub owner: Option<String>,
    /// The names of the key fields; `None` when the record states no key.
    pub key: Option<Vec<String>>,
    /// The fields, in declaration order.
    pub fields: Vec<RecordField>,
    /// Sets of field names whose combined values are unique.
    pub unique: Vec<Vec<String>>,
    /// Named indexes.
    pub indexes: Vec<Index>,
    /// The schema version, as written.
    pub schema_version: Option<Number>,
    /// The retention text.
    pub retention: Option<String>,
    /// The description, if any.
    pub description: Option<String>,
}

impl Record {
    /// The field with this name.
    pub fn field(&self, name: &str) -> Option<&RecordField> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// A field of a record.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordField {
    /// The field name.
    pub name: String,
    /// The type name.
    pub type_name: String,
    /// True when a record may leave the field unset.
    pub optional: bool,
    /// The constraints on its value.
    pub constraints: Constraints,
    /// The record whose key the field holds, or `principal`.
    pub refers_to: Option<String>,
    /// What deleting the referred record does.
    pub on_delete: Option<OnDelete>,
    /// The personal-data class, if the field is personal data.
    pub personal: Option<String>,
    /// The description, if any.
    pub description: Option<String>,
}

/// The constraints on a record field, as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Constraints {
    /// The least allowed value.
    pub min: Option<Number>,
    /// The greatest allowed value.
    pub max: Option<Number>,
    /// The greatest allowed length in characters.
    pub max_length: Option<Number>,
    /// A regular expression the value must match.
    pub pattern: Option<String>,
}

impl Constraints {
    /// True when no constraint is stated.
    pub fn is_empty(&self) -> bool {
        self.min.is_none()
            && self.max.is_none()
            && self.max_length.is_none()
            && self.pattern.is_none()
    }
}

/// What deleting a referred record does to the referring one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDelete {
    /// Deleting the referred record is refused.
    Restrict,
    /// The referring record is deleted too.
    Cascade,
    /// The referring field is cleared.
    SetNull,
}

impl OnDelete {
    /// Reads the specification's value.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "restrict" => Some(Self::Restrict),
            "cascade" => Some(Self::Cascade),
            "set-null" => Some(Self::SetNull),
            _ => None,
        }
    }

    /// The value as the specification writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restrict => "restrict",
            Self::Cascade => "cascade",
            Self::SetNull => "set-null",
        }
    }
}

/// A named index of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// The index name.
    pub name: String,
    /// The indexed field names, in order.
    pub fields: Vec<String>,
}

/// A `contract` entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    /// The contract id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The inputs, in declaration order.
    pub inputs: Vec<ContractField>,
    /// The outputs, in declaration order.
    pub outputs: Vec<ContractField>,
    /// The error conditions, in declaration order.
    pub errors: Vec<ContractError>,
    /// The preconditions.
    pub preconditions: Vec<String>,
    /// The postconditions.
    pub postconditions: Vec<String>,
    /// The stable code, such as `CT-012` or `H-012`.
    pub reference: Option<String>,
    /// Whether the code is generated or hand-written.
    pub realisation: Option<Realisation>,
    /// The implementing component.
    pub implemented_by: Option<String>,
    /// Who may call it.
    pub auth: Option<String>,
    /// Whether repeating a call has the effect of one call.
    pub idempotent: Option<bool>,
    /// How long a caller waits, as written.
    pub timeout_ms: Option<Number>,
    /// `binding.method`, as written.
    pub binding_method: Option<String>,
    /// `binding.path`, as written: the route the contract is served at.
    pub binding_path: Option<String>,
    /// `binding.case`: the contract's own wire case, overriding the system's.
    pub binding_case: Option<crate::wire::WireCase>,
}

/// An input or output of a contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractField {
    /// The field name.
    pub name: String,
    /// The type name, if stated.
    pub type_name: Option<String>,
    /// True when it may be left out.
    pub optional: bool,
    /// The description, if any.
    pub description: Option<String>,
}

/// An error condition of a contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    /// The error name.
    pub name: String,
    /// When it happens.
    pub condition: Option<String>,
    /// The payload type, when it carries one.
    pub type_name: Option<String>,
}

/// How a contract is realised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realisation {
    /// Generated from the contract.
    Generated,
    /// A hole: written by hand against the postconditions.
    Hand,
}

impl Realisation {
    /// Reads the specification's value.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "generated" => Some(Self::Generated),
            "hand" => Some(Self::Hand),
            _ => None,
        }
    }
}

/// An `event` entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The event id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The payload fields, in declaration order.
    pub payload: Vec<PayloadField>,
    /// `delivery`: `at-most-once` or `at-least-once`, as written.
    pub delivery: Option<String>,
    /// `elapsed`: the event stands for elapsed time and is fired by a timer.
    pub elapsed: bool,
    /// `contract`: the contract whose message the event is.
    pub contract: Option<String>,
    /// `role`: which message of its contract the event is (`request`, `response`, `error`).
    pub role: Option<String>,
    /// `domain`: a fact the running system records.
    pub domain: bool,
    /// `logging`, when the event is logged; `None` when it is not or does not say.
    pub logging: Option<Logging>,
}

/// A payload field of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadField {
    /// The field name.
    pub name: String,
    /// The type name, if stated.
    pub type_name: Option<String>,
    /// True when the field may be left out.
    pub optional: bool,
    /// `sensitive`: the kind of sensitive data the field holds, which is never logged.
    pub sensitive: Option<String>,
}

/// A `behavior` entity with `level: "implementation"`.
#[derive(Debug, Clone, PartialEq)]
pub struct Behavior {
    /// The behavior id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The description, if any.
    pub description: Option<String>,
    /// The initial state id.
    pub initial: Option<String>,
    /// The variables, in declaration order.
    pub variables: Vec<Variable>,
    /// The states, in declaration order.
    pub states: Vec<State>,
    /// The transitions, in declaration order (the order they are tried in).
    pub transitions: Vec<Transition>,
    /// `storage: {record, key}`.
    pub storage: Option<Storage>,
    /// `instances`, as written.
    pub instances: Option<Instances>,
    /// `transaction.isolation`, as written.
    pub isolation: Option<String>,
    /// The time units of the scenarios that run the behavior, distinct, in specification order
    /// (Exegra's `machineTimeUnit`): a format-2 scenario's `timeUnit`, by default `us`; a format-1
    /// scenario only when it names one. Bare-number delays are in this unit.
    pub time_units: Vec<String>,
}

/// Nanoseconds per unit of a scenario clock or of a duration string.
fn unit_ns(unit: &str) -> Option<u128> {
    Some(match unit {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "min" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        _ => return None,
    })
}

impl Behavior {
    /// The unit bare-number delays and `time` are in: the one unit of the scenarios that run the
    /// behavior, `ms` when none constrains it, or the units found when they disagree.
    pub fn time_unit(&self) -> Result<&str, &[String]> {
        match self.time_units.as_slice() {
            [] => Ok("ms"),
            [one] => Ok(one),
            many => Err(many),
        }
    }

    /// A fixed delay (`after`) in whole milliseconds, the unit of generated timers: a bare number
    /// in the behavior's time unit, or a duration string `"<decimal> <unit>"`. `None` when it is
    /// neither, is not a whole number of milliseconds, or is 2^53 ms or more.
    pub fn delay_ms(&self, after: &serde_json::Value) -> Option<u64> {
        let ns: u128 = match after {
            serde_json::Value::Number(n) => {
                let unit = self.time_unit().ok()?;
                u128::from(n.as_u64()?).checked_mul(unit_ns(unit)?)?
            }
            serde_json::Value::String(s) => {
                let s = s.trim();
                let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
                let (num, unit) = (&s[..split], s[split..].trim());
                let (int, frac) = num.split_once('.').unwrap_or((num, ""));
                if int.is_empty()
                    || !int.bytes().all(|b| b.is_ascii_digit())
                    || !frac.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                let digits: u128 = format!("{int}{frac}").parse().ok()?;
                let scale = 10u128.checked_pow(u32::try_from(frac.len()).ok()?)?;
                let total = digits.checked_mul(unit_ns(unit)?)?;
                if total % scale != 0 {
                    return None;
                }
                total / scale
            }
            _ => return None,
        };
        if ns % 1_000_000 != 0 {
            return None;
        }
        let ms = ns / 1_000_000;
        (ms < 1 << 53).then(|| u64::try_from(ms).ok()).flatten()
    }
}

/// A variable of a behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    /// The variable name.
    pub name: String,
    /// The representation type name.
    pub repr: Option<String>,
    /// The initial value, a JSON literal.
    pub initial: Option<Value>,
}

/// A state of a behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The state id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// True when the state is terminal.
    pub terminal: bool,
    /// The events the state deliberately ignores.
    pub ignores: Vec<String>,
}

/// A transition of a behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// The transition id.
    pub id: String,
    /// The source state.
    pub from: String,
    /// The target state.
    pub to: String,
    /// The event it reacts to.
    pub on: String,
    /// The guard expression, if any.
    pub guard: Option<String>,
    /// True when the transition is the explicit fall-through of its group.
    pub otherwise: bool,
    /// True when the transition fires only under a fault.
    pub fault_only: bool,
    /// The effects, in order.
    pub effects: Vec<Effect>,
}

/// An effect of a transition.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `{set, to}`: `to` is an expression when a string, a literal otherwise.
    Set {
        /// The variable assigned.
        variable: String,
        /// The value.
        to: Value,
    },
    /// `{emit, to?, target?, after?, payload?, via?, size?, fanout?}`.
    Emit {
        /// The emitted event.
        event: String,
        /// The receiver outside the modelled behaviors.
        to: Option<String>,
        /// The simulation addressing (`reply`), as written.
        target: Option<String>,
        /// The delay, as written.
        after: Option<Value>,
        /// The payload entries, in order.
        payload: Vec<(String, Value)>,
        /// The link properties present (`via`, `size`, `fanout`), which the runtime cannot
        /// express.
        link: Vec<String>,
    },
    /// `{timer, after, emit, payload?}`.
    Timer {
        /// The timer's name.
        name: String,
        /// The delay, as written.
        after: Option<Value>,
        /// The event the timer fires.
        event: String,
        /// The payload entries, in order.
        payload: Vec<(String, Value)>,
    },
    /// `{cancel}`.
    Cancel {
        /// The timer's name.
        name: String,
    },
    /// Any other effect (`use`, `acquire`, `release`, a sampled `set`, ...), by the property
    /// that names its kind.
    Other {
        /// The kind, such as `acquire`.
        kind: String,
    },
}

/// `storage: {record, key, fields, state, stateValues, creates}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Storage {
    /// The record id.
    pub record: Option<String>,
    /// The key field.
    pub key: Option<String>,
    /// `fields`: the record field each listed variable is held in (`None` when not a string),
    /// in the order written. A variable not listed is held in the field of its own name.
    pub fields: Vec<(String, Option<String>)>,
    /// `state`: the record field holding the state, when given (default `state`).
    pub state: Option<String>,
    /// `stateValues`: the stored value of each listed state (`None` when not a string).
    pub state_values: Option<Vec<(String, Option<String>)>>,
    /// `creates`: true when an event for a key with no record creates the record.
    pub creates: bool,
}

/// `instances`, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instances {
    /// The payload field that keys the instances.
    pub key: Option<String>,
    /// The record each instance is stored in.
    pub stored_in: Option<String>,
    /// True when the instances are counted (`count`) rather than keyed.
    pub counted: bool,
}

#[cfg(test)]
mod delay_tests {
    use super::Behavior;
    use serde_json::json;

    fn behavior(units: &[&str]) -> Behavior {
        Behavior {
            id: "m".to_string(),
            name: "M".to_string(),
            description: None,
            initial: None,
            variables: Vec::new(),
            states: Vec::new(),
            transitions: Vec::new(),
            storage: None,
            instances: None,
            isolation: None,
            time_units: units.iter().map(|u| (*u).to_string()).collect(),
        }
    }

    #[test]
    fn a_bare_delay_is_in_the_scenario_unit() {
        assert_eq!(behavior(&[]).delay_ms(&json!(5000)), Some(5000));
        assert_eq!(behavior(&["ms"]).delay_ms(&json!(5000)), Some(5000));
        assert_eq!(behavior(&["us"]).delay_ms(&json!(5000)), Some(5));
        assert_eq!(behavior(&["s"]).delay_ms(&json!(2)), Some(2000));
        // 5000 us is 5 ms; 2500 us is not a whole number of milliseconds.
        assert_eq!(behavior(&["us"]).delay_ms(&json!(2500)), None);
        // Scenarios that disagree leave a bare number without a unit.
        assert_eq!(behavior(&["ms", "us"]).delay_ms(&json!(5)), None);
    }

    #[test]
    fn a_duration_string_carries_its_unit() {
        let b = behavior(&["us"]);
        assert_eq!(b.delay_ms(&json!("1.5 s")), Some(1500));
        assert_eq!(b.delay_ms(&json!("30 ms")), Some(30));
        assert_eq!(b.delay_ms(&json!("2 min")), Some(120_000));
        assert_eq!(b.delay_ms(&json!("2500 us")), None);
        assert_eq!(b.delay_ms(&json!("soon")), None);
    }
}
