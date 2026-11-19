//! Loading: JSONC text to the typed IR.
//!
//! Only the sections specc reads are loaded (`system`, `components`, `types`, `records`,
//! `contracts`, `events`, the behaviors with `level: "implementation"`, and the `calls`
//! relationships, which become [`Component::calls`]); unknown sections and
//! properties are left to Exegra's own validation. A value
//! of the wrong JSON type is a `specc.spec-shape` diagnostic, and loading carries on so every
//! problem is reported at once.

use serde_json::{Map, Number, Value};

use crate::diagnostic::{Diagnostic, SPEC_ENTITY};
use crate::ir::{
    Auth, Behavior, Component, Constraints, Contract, ContractError, ContractField, Effect,
    ErrorBody, ErrorWire, Event, Exposure, Index, Injection, Instances, LoggedField, Logging,
    OnDelete, PayloadField, Realisation, Record, RecordField, Route, Scheme, Spec, State, Storage,
    StructField, System, Telemetry, TelemetryService, TenantScope,
    Transition, TypeDecl, TypeKind, Variable,
};
use crate::jsonc;

/// The code of a text that is not JSONC.
pub const SYNTAX: &str = "specc.spec-syntax";
/// The code of a value with the wrong shape.
pub const SHAPE: &str = "specc.spec-shape";

/// Loads a specification. `path` is the path to print in headers and diagnostics.
pub fn load(path: &str, text: &str) -> Result<Spec, Vec<Diagnostic>> {
    load_with_client_machines(path, text).map(|(spec, _)| spec)
}

/// Loads a specification and, apart from it, the implementation-level machines whose `subject`
/// is a component of category `client`. Those are not in `Spec::machines`: the browser runs
/// them, and [`crate::web`] generates them as JavaScript instead of the Rust backend.
pub fn load_with_client_machines(
    path: &str,
    text: &str,
) -> Result<(Spec, Vec<Behavior>), Vec<Diagnostic>> {
    let value = jsonc::parse(text).map_err(|e| {
        vec![Diagnostic::new(
            SYNTAX,
            SPEC_ENTITY,
            format!("the specification is not valid JSONC: {e}"),
        )]
    })?;
    let mut loader = Loader::default();
    let spec = loader.spec(path, &value);
    if loader.diagnostics.is_empty() {
        if let Some(spec) = spec {
            return Ok((spec, loader.client_machines));
        }
    }
    Err(loader.diagnostics)
}

#[derive(Default)]
pub(crate) struct Loader {
    diagnostics: Vec<Diagnostic>,
    client_machines: Vec<Behavior>,
}

pub(crate) type Obj = Map<String, Value>;

impl Loader {
    pub(crate) fn shape(&mut self, entity: &str, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(SHAPE, entity, message.into()));
    }

    fn spec(&mut self, path: &str, value: &Value) -> Option<Spec> {
        let Some(top) = value.as_object() else {
            self.shape(SPEC_ENTITY, "the specification is not a JSON object");
            return None;
        };
        let system = match top.get("system") {
            Some(Value::Object(obj)) => self.system(obj),
            Some(_) => {
                self.shape(SPEC_ENTITY, "\"system\" must be an object");
                None
            }
            None => {
                self.shape(SPEC_ENTITY, "the specification has no \"system\"");
                None
            }
        };
        let mut components: Vec<Component> = self
            .section(top, "components")
            .into_iter()
            .filter_map(|(entity, obj)| self.component(&entity, obj))
            .collect();
        for (entity, obj) in self.section(top, "relationships") {
            if self.opt_str(obj, "kind", &entity).as_deref() != Some("calls") {
                continue;
            }
            let from = self.opt_str(obj, "from", &entity);
            let to = self.opt_str(obj, "to", &entity);
            if let (Some(from), Some(to)) = (from, to) {
                if let Some(c) = components.iter_mut().find(|c| c.id == from) {
                    if !c.calls.contains(&to) {
                        c.calls.push(to);
                    }
                }
            }
        }
        // Components that are declared not-implemented: what a behavior's or contract's
        // realisation defaults from.
        let not_implemented: Vec<&str> = top
            .get("components")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| {
                c.get("implementation")
                    .and_then(|i| i.get("status"))
                    .and_then(Value::as_str)
                    == Some("not-implemented")
            })
            .filter_map(|c| c.get("id").and_then(Value::as_str))
            .collect();
        let types = self
            .section(top, "types")
            .into_iter()
            .filter_map(|(entity, obj)| self.type_decl(&entity, obj))
            .collect();
        let records = self
            .section(top, "records")
            .into_iter()
            .filter_map(|(entity, obj)| self.record(&entity, obj))
            .collect();
        let contracts = self
            .section(top, "contracts")
            .into_iter()
            .filter_map(|(entity, obj)| self.contract(&entity, obj, &not_implemented))
            .collect();
        let events = self
            .section(top, "events")
            .into_iter()
            .filter_map(|(entity, obj)| self.event(&entity, obj))
            .collect();
        let client_components: Vec<&str> = top
            .get("components")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("category").and_then(Value::as_str) == Some("client"))
            .filter_map(|c| c.get("id").and_then(Value::as_str))
            .collect();
        let mut machines = Vec::new();
        let mut declared = 0usize;
        for (entity, obj) in self.section(top, "behaviors") {
            if obj.get("level").and_then(Value::as_str) != Some("implementation") {
                continue;
            }
            let client = obj
                .get("subject")
                .and_then(Value::as_str)
                .is_some_and(|s| client_components.contains(&s));
            if let Some(mut b) = self.behavior(&entity, obj) {
                b.time_units = scenario_time_units(top, &entity);
                b.target = self.realisation_target(
                    obj,
                    "realisation",
                    &entity,
                    obj.get("subject")
                        .and_then(Value::as_str)
                        .is_some_and(|s| not_implemented.contains(&s)),
                );
                b.declared = declared;
                declared += 1;
                if client {
                    self.client_machines.push(b);
                } else {
                    machines.push(b);
                }
            }
        }
        // Scenario routes to implementation machines, each distinct one once.
        for scenario in top
            .get("scenarios")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let sid = scenario.get("id").and_then(Value::as_str).unwrap_or("");
            // The events its steps inject, into the machines it runs.
            let run: Vec<&str> = scenario
                .get("behaviors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for step in scenario
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(event) = step.get("event").and_then(Value::as_str) else {
                    continue;
                };
                for b in machines.iter_mut().filter(|b| run.contains(&b.id.as_str())) {
                    if !b.injected.iter().any(|i| i.event == event) {
                        b.injected.push(Injection {
                            event: event.to_string(),
                            scenario: sid.to_string(),
                        });
                    }
                }
            }
            for route in scenario
                .get("routes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let text = |k: &str| route.get(k).and_then(Value::as_str).map(str::to_string);
                let (Some(event), Some(behavior), Some(by)) =
                    (text("event"), text("behavior"), text("by"))
                else {
                    continue;
                };
                let field = text("field");
                if let Some(b) = machines.iter_mut().find(|b| b.id == behavior) {
                    if !b
                        .routes
                        .iter()
                        .any(|r| r.event == event && r.by == by && r.field == field)
                    {
                        b.routes.push(Route {
                            event,
                            by,
                            field,
                            scenario: sid.to_string(),
                        });
                    }
                }
            }
        }
        let (mut target_machines, mut shipped) = (Vec::new(), Vec::new());
        for b in machines {
            if b.target {
                target_machines.push(b);
            } else {
                shipped.push(b);
            }
        }
        let (memory_machines, machines): (Vec<_>, Vec<_>) =
            shipped.into_iter().partition(|b| b.memory);
        let constructs = self.constructs(top);
        let telemetry = self.telemetry(top);
        Some(Spec {
            path: path.replace('\\', "/"),
            system: system?,
            components,
            types,
            records,
            contracts,
            events,
            machines,
            memory_machines,
            target_machines,
            constructs,
            telemetry,
        })
    }

    /// `system.telemetry.services` and the types marked sensitive (directly or by a struct
    /// field). Exegra checks their shape; values of the wrong JSON type are skipped here.
    fn telemetry(&mut self, top: &Obj) -> Telemetry {
        let mut services = Vec::new();
        let listed = top
            .get("system")
            .and_then(|s| s.get("telemetry"))
            .and_then(|t| t.get("services"))
            .and_then(Value::as_array);
        for s in listed.into_iter().flatten().filter_map(Value::as_object) {
            if let Some(id) = s.get("id").and_then(Value::as_str) {
                services.push(TelemetryService {
                    id: id.to_string(),
                    log: s.get("log").and_then(Value::as_str).map(str::to_string),
                });
            }
        }
        let mut sensitive_types = Vec::new();
        let types = top.get("types").and_then(Value::as_array);
        for t in types.into_iter().flatten().filter_map(Value::as_object) {
            let Some(id) = t.get("id").and_then(Value::as_str) else {
                continue;
            };
            let field_mark = t
                .get("fields")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|f| f.get("sensitive").and_then(Value::as_str));
            if let Some(kind) = t.get("sensitive").and_then(Value::as_str).or(field_mark) {
                sensitive_types.push((id.to_string(), kind.to_string()));
            }
        }
        Telemetry {
            services,
            sensitive_types,
        }
    }

    /// `event.logging` when the event is logged.
    fn logging(&mut self, obj: &Obj, id: &str) -> Option<Logging> {
        let l = match obj.get("logging") {
            None | Some(Value::Null) => return None,
            Some(Value::Object(l)) => l,
            Some(_) => {
                self.shape(id, "\"logging\" must be an object");
                return None;
            }
        };
        if self.opt_bool(l, "logged", id) != Some(true) {
            return None;
        }
        let service = self.req_str(l, "service", id);
        let level = self.req_str(l, "level", id);
        let name = self
            .opt_str(l, "name", id)
            .unwrap_or_else(|| id.to_string());
        let tenant = self.opt_str(l, "tenant", id);
        let mut fields = Vec::new();
        for f in self.obj_list(l, "fields", id) {
            if let Some(field) = self.req_str(f, "field", id) {
                let written_as = self.opt_str(f, "as", id).unwrap_or_else(|| field.clone());
                fields.push(LoggedField { field, written_as });
            }
        }
        Some(Logging {
            service: service?,
            level: level?,
            name,
            tenant,
            fields,
        })
    }

    /// The entities of a section, each with the name diagnostics use for it: its id, or
    /// `<section>[<index>]` when it has none.
    pub(crate) fn section<'a>(&mut self, top: &'a Obj, name: &str) -> Vec<(String, &'a Obj)> {
        let items = match top.get(name) {
            None => return Vec::new(),
            Some(Value::Array(items)) => items,
            Some(_) => {
                self.shape(SPEC_ENTITY, format!("section \"{name}\" must be an array"));
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let fallback = format!("{name}[{i}]");
            let Some(obj) = item.as_object() else {
                self.shape(&fallback, "an entity must be an object");
                continue;
            };
            match obj.get("id") {
                Some(Value::String(id)) => out.push((id.clone(), obj)),
                _ => self.shape(&fallback, "an entity needs a string \"id\""),
            }
        }
        out
    }

    pub(crate) fn opt_str(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<String> {
        match obj.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                self.shape(entity, format!("\"{key}\" must be a string"));
                None
            }
        }
    }

    /// `storage.key`: one field name, or the list of the key fields of a composite key.
    fn key_fields(&mut self, obj: &Obj, entity: &str) -> Option<Vec<String>> {
        match obj.get("key") {
            Some(Value::Array(_)) => self
                .opt_str_list(obj, "key", entity)
                .filter(|k| !k.is_empty()),
            _ => self.opt_str(obj, "key", entity).map(|k| vec![k]),
        }
    }

    pub(crate) fn req_str(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<String> {
        if obj.get(key).is_none() {
            self.shape(entity, format!("\"{key}\" is required"));
            return None;
        }
        self.opt_str(obj, key, entity)
    }

    fn opt_bool(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<bool> {
        match obj.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => {
                self.shape(entity, format!("\"{key}\" must be true or false"));
                None
            }
        }
    }

    pub(crate) fn opt_number(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<Number> {
        match obj.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(n.clone()),
            Some(_) => {
                self.shape(entity, format!("\"{key}\" must be a number"));
                None
            }
        }
    }

    pub(crate) fn opt_str_list(
        &mut self,
        obj: &Obj,
        key: &str,
        entity: &str,
    ) -> Option<Vec<String>> {
        let value = obj.get(key).filter(|v| !v.is_null())?;
        let Some(items) = value.as_array() else {
            self.shape(entity, format!("\"{key}\" must be a list of strings"));
            return None;
        };
        let mut out = Vec::new();
        for item in items {
            match item.as_str() {
                Some(s) => out.push(s.to_string()),
                None => {
                    self.shape(entity, format!("\"{key}\" must be a list of strings"));
                    return None;
                }
            }
        }
        Some(out)
    }

    pub(crate) fn obj_list<'a>(&mut self, obj: &'a Obj, key: &str, entity: &str) -> Vec<&'a Obj> {
        let Some(value) = obj.get(key).filter(|v| !v.is_null()) else {
            return Vec::new();
        };
        let Some(items) = value.as_array() else {
            self.shape(entity, format!("\"{key}\" must be a list of objects"));
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in items {
            match item.as_object() {
                Some(o) => out.push(o),
                None => self.shape(entity, format!("every item of \"{key}\" must be an object")),
            }
        }
        out
    }

    fn system(&mut self, obj: &Obj) -> Option<System> {
        let id = self.req_str(obj, "id", SPEC_ENTITY);
        let entity = id.clone().unwrap_or_else(|| SPEC_ENTITY.to_string());
        let name = self.req_str(obj, "name", &entity);
        let (generation_target, wire_case, error_wire) = match obj.get("generation") {
            None | Some(Value::Null) => (None, None, ErrorWire::default()),
            Some(Value::Object(g)) => (
                self.opt_str(g, "target", &entity),
                self.wire_case(g, "wireCase", &entity),
                self.error_wire(g, &entity),
            ),
            Some(_) => {
                self.shape(&entity, "\"generation\" must be an object");
                (None, None, ErrorWire::default())
            }
        };
        Some(System {
            id: id?,
            name: name?,
            generation_target,
            wire_case: wire_case.unwrap_or_default(),
            error_wire,
        })
    }

    /// An HTTP status under `key`, if stated: an integer from 400 to 599.
    fn status(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<u16> {
        let n = self.opt_number(obj, key, entity)?;
        match n.as_u64().filter(|s| (400..=599).contains(s)) {
            Some(s) => Some(s as u16),
            None => {
                self.shape(
                    entity,
                    format!("\"{key}\" is {n}; a status is an integer from 400 to 599"),
                );
                None
            }
        }
    }

    /// A map of strings under `key`, in declaration order.
    fn str_map(&mut self, obj: &Obj, key: &str, entity: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        match obj.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, v) in m {
                    match v.as_str() {
                        Some(v) => out.push((k.clone(), v.to_string())),
                        None => self.shape(entity, format!("\"{key}.{k}\" must be a string")),
                    }
                }
            }
            Some(_) => self.shape(entity, format!("\"{key}\" must be an object")),
        }
        out
    }

    /// The error kinds, default status or kind and error body of `system.generation`.
    fn error_wire(&mut self, g: &Obj, entity: &str) -> ErrorWire {
        let mut kinds = Vec::new();
        match g.get("errorKinds") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for k in m.keys() {
                    if let Some(s) = self.status(m, k, entity) {
                        kinds.push((k.clone(), s));
                    }
                }
            }
            Some(_) => self.shape(entity, "\"errorKinds\" must be an object"),
        }
        let default_status = self.status(g, "errorStatus", entity);
        let default_kind = self.opt_str(g, "errorKind", entity);
        let body = match g.get("errorBody") {
            None | Some(Value::Null) => None,
            Some(Value::Object(b)) => {
                let type_name = self.req_str(b, "type", entity);
                let code = self.req_str(b, "code", entity);
                let message = self.req_str(b, "message", entity);
                let correlation = self.opt_str(b, "correlation", entity);
                let runtime = self.str_map(b, "runtime", entity);
                match (type_name, code, message) {
                    (Some(type_name), Some(code), Some(message)) => Some(ErrorBody {
                        type_name,
                        code,
                        message,
                        correlation,
                        runtime,
                    }),
                    _ => None,
                }
            }
            Some(_) => {
                self.shape(entity, "\"errorBody\" must be an object");
                None
            }
        };
        ErrorWire {
            kinds,
            default_status,
            default_kind,
            body,
        }
    }

    /// A wire case (`snake_case`, `kebab-case` or `camelCase`) under `key`, if stated.
    fn wire_case(&mut self, obj: &Obj, key: &str, entity: &str) -> Option<crate::wire::WireCase> {
        let text = self.opt_str(obj, key, entity)?;
        let parsed = crate::wire::WireCase::parse(&text);
        if parsed.is_none() {
            self.shape(
                entity,
                format!("\"{key}\" is \"{text}\"; it must be snake_case, kebab-case or camelCase"),
            );
        }
        parsed
    }

    fn component(&mut self, id: &str, obj: &Obj) -> Option<Component> {
        let name = self.req_str(obj, "name", id)?;
        Some(Component {
            id: id.to_string(),
            name,
            calls: Vec::new(),
        })
    }

    fn type_decl(&mut self, id: &str, obj: &Obj) -> Option<TypeDecl> {
        let name = self.req_str(obj, "name", id);
        let description = self.opt_str(obj, "description", id);
        let kind_text = self.req_str(obj, "kind", id);
        let repr = self.opt_str(obj, "repr", id);
        let kind = match kind_text.as_deref() {
            None => None,
            Some("id") => Some(TypeKind::Id { repr }),
            Some("scalar") => Some(TypeKind::Scalar { repr }),
            Some("enum") => Some(TypeKind::Enum {
                values: self.opt_str_list(obj, "values", id).unwrap_or_default(),
            }),
            Some("struct") => {
                let mut fields = Vec::new();
                for f in self.obj_list(obj, "fields", id) {
                    let name = self.req_str(f, "name", id);
                    let type_name = self.req_str(f, "type", id);
                    let optional = self.opt_bool(f, "optional", id).unwrap_or(false);
                    if let (Some(name), Some(type_name)) = (name, type_name) {
                        fields.push(StructField {
                            name,
                            type_name,
                            optional,
                        });
                    }
                }
                Some(TypeKind::Struct { fields })
            }
            Some(other) => {
                self.shape(
                    id,
                    format!("\"kind\" is \"{other}\"; it must be id, scalar, enum or struct"),
                );
                None
            }
        };
        Some(TypeDecl {
            id: id.to_string(),
            name: name?,
            description,
            kind: kind?,
        })
    }

    fn record(&mut self, id: &str, obj: &Obj) -> Option<Record> {
        let name = self.req_str(obj, "name", id);
        let owner = self.opt_str(obj, "owner", id);
        let key = self.opt_str_list(obj, "key", id);
        if obj.get("fields").is_none() {
            self.shape(id, "\"fields\" is required");
        }
        let mut fields = Vec::new();
        for f in self.obj_list(obj, "fields", id) {
            if let Some(field) = self.record_field(id, f) {
                fields.push(field);
            }
        }
        let mut unique = Vec::new();
        match obj.get("unique") {
            None | Some(Value::Null) => {}
            Some(Value::Array(sets)) => {
                for set in sets {
                    match set
                        .as_array()
                        .and_then(|s| s.iter().map(|n| n.as_str().map(str::to_string)).collect())
                    {
                        Some(names) => unique.push(names),
                        None => self.shape(id, "\"unique\" must be a list of lists of field names"),
                    }
                }
            }
            Some(_) => self.shape(id, "\"unique\" must be a list of lists of field names"),
        }
        let mut indexes = Vec::new();
        for ix in self.obj_list(obj, "indexes", id) {
            let name = self.req_str(ix, "name", id);
            let fields = self.opt_str_list(ix, "fields", id);
            if fields.is_none() && ix.get("fields").is_none() {
                self.shape(id, "every index needs \"fields\"");
            }
            if let (Some(name), Some(fields)) = (name, fields) {
                indexes.push(Index { name, fields });
            }
        }
        let schema_version = self.opt_number(obj, "schemaVersion", id);
        let retention = self.opt_str(obj, "retention", id);
        let description = self.opt_str(obj, "description", id);
        Some(Record {
            id: id.to_string(),
            name: name?,
            owner,
            key,
            fields,
            unique,
            indexes,
            schema_version,
            retention,
            description,
        })
    }

    fn record_field(&mut self, id: &str, f: &Obj) -> Option<RecordField> {
        let name = self.req_str(f, "name", id);
        let type_name = self.req_str(f, "type", id);
        let optional = self.opt_bool(f, "optional", id).unwrap_or(false);
        let constraints = match f.get("constraints") {
            None | Some(Value::Null) => Constraints::default(),
            Some(Value::Object(c)) => Constraints {
                min: self.opt_number(c, "min", id),
                max: self.opt_number(c, "max", id),
                max_length: self.opt_number(c, "maxLength", id),
                pattern: self.opt_str(c, "pattern", id),
            },
            Some(_) => {
                self.shape(id, "\"constraints\" must be an object");
                Constraints::default()
            }
        };
        let refers_to = self.opt_str(f, "refersTo", id);
        let on_delete = match self.opt_str(f, "onDelete", id) {
            None => None,
            Some(text) => {
                let parsed = OnDelete::parse(&text);
                if parsed.is_none() {
                    self.shape(
                        id,
                        format!(
                            "\"onDelete\" is \"{text}\"; it must be restrict, cascade or set-null"
                        ),
                    );
                }
                parsed
            }
        };
        let personal = self.opt_str(f, "personal", id);
        let description = self.opt_str(f, "description", id);
        Some(RecordField {
            name: name?,
            type_name: type_name?,
            optional,
            constraints,
            refers_to,
            on_delete,
            personal,
            description,
        })
    }

    fn contract_fields(&mut self, id: &str, obj: &Obj, key: &str) -> Vec<ContractField> {
        let mut out = Vec::new();
        for f in self.obj_list(obj, key, id) {
            let name = self.req_str(f, "name", id);
            let type_name = self.opt_str(f, "type", id);
            let optional = self.opt_bool(f, "optional", id).unwrap_or(false);
            let description = self.opt_str(f, "description", id);
            if let Some(name) = name {
                out.push(ContractField {
                    name,
                    type_name,
                    optional,
                    description,
                });
            }
        }
        out
    }

    fn contract(&mut self, id: &str, obj: &Obj, not_implemented: &[&str]) -> Option<Contract> {
        let name = self.req_str(obj, "name", id);
        let description = self.opt_str(obj, "description", id);
        let inputs = self.contract_fields(id, obj, "inputs");
        let outputs = self.contract_fields(id, obj, "outputs");
        let mut errors = Vec::new();
        for e in self.obj_list(obj, "errors", id) {
            let name = self.req_str(e, "name", id);
            let condition = self.opt_str(e, "condition", id);
            let type_name = self.opt_str(e, "type", id);
            let status = self.status(e, "status", id);
            let kind = self.opt_str(e, "kind", id);
            if let Some(name) = name {
                errors.push(ContractError {
                    name,
                    condition,
                    type_name,
                    status,
                    kind,
                });
            }
        }
        let preconditions = self
            .opt_str_list(obj, "preconditions", id)
            .unwrap_or_default();
        let postconditions = self
            .opt_str_list(obj, "postconditions", id)
            .unwrap_or_default();
        let reference = self.opt_str(obj, "ref", id);
        let realisation = match self.opt_str(obj, "realisation", id) {
            None => None,
            Some(text) => {
                let parsed = Realisation::parse(&text);
                if parsed.is_none() {
                    self.shape(
                        id,
                        format!("\"realisation\" is \"{text}\"; it must be generated or hand"),
                    );
                }
                parsed
            }
        };
        let implemented_by = self.opt_str(obj, "implementedBy", id);
        let target = self.realisation_target(
            obj,
            "deployment",
            id,
            implemented_by
                .as_deref()
                .is_some_and(|c| not_implemented.contains(&c)),
        );
        let (auth, auth_prose) = match obj.get("auth") {
            None | Some(Value::Null) => (None, None),
            Some(Value::String(text)) => (None, Some(text.clone())),
            Some(Value::Object(a)) => (self.auth(a, id), None),
            Some(_) => {
                self.shape(id, "\"auth\" must be an object {scheme, ...} or a sentence");
                (None, None)
            }
        };
        let exposure = match self.opt_str(obj, "exposure", id) {
            None => None,
            Some(text) => {
                let parsed = Exposure::parse(&text);
                if parsed.is_none() {
                    self.shape(
                        id,
                        format!("\"exposure\" is \"{text}\"; it must be public, internal or local"),
                    );
                }
                parsed
            }
        };
        let read_only = self.opt_bool(obj, "readOnly", id).unwrap_or(false);
        let tenant_scope = match self.opt_str(obj, "tenantScope", id) {
            None => None,
            Some(text) => {
                let parsed = TenantScope::parse(&text);
                if parsed.is_none() {
                    self.shape(
                        id,
                        format!(
                            "\"tenantScope\" is \"{text}\"; it must be request, none or platform"
                        ),
                    );
                }
                parsed
            }
        };
        let development = self.opt_str(obj, "development", id);
        let idempotent = self.opt_bool(obj, "idempotent", id);
        let timeout_ms = self.opt_number(obj, "timeoutMs", id);
        let (binding_method, binding_path, binding_case) = match obj.get("binding") {
            None | Some(Value::Null) => (None, None, None),
            Some(Value::Object(b)) => (
                self.opt_str(b, "method", id),
                self.opt_str(b, "path", id),
                self.wire_case(b, "case", id),
            ),
            Some(_) => {
                self.shape(id, "\"binding\" must be an object");
                (None, None, None)
            }
        };
        Some(Contract {
            id: id.to_string(),
            name: name?,
            description,
            inputs,
            outputs,
            errors,
            preconditions,
            postconditions,
            reference,
            realisation,
            implemented_by,
            auth,
            auth_prose,
            exposure,
            read_only,
            tenant_scope,
            development,
            idempotent,
            timeout_ms,
            binding_method,
            binding_path,
            binding_case,
            target,
        })
    }

    /// True when the declared `shipped` or `target` value under `key` is `target`; when the
    /// entity does not declare one, `default_target` (its component is not implemented).
    fn realisation_target(&mut self, obj: &Obj, key: &str, id: &str, default_target: bool) -> bool {
        match self.opt_str(obj, key, id).as_deref() {
            None => default_target,
            Some("shipped") => false,
            Some("target") => true,
            Some(other) => {
                self.shape(
                    id,
                    format!("\"{key}\" is \"{other}\"; it must be shipped or target"),
                );
                default_target
            }
        }

    /// A structured auth: its scheme and the members the format gives it. An unknown scheme is a
    /// shape error and no auth; Exegra checks which members apply to which scheme.
    fn auth(&mut self, a: &Obj, id: &str) -> Option<Auth> {
        let scheme = match self.req_str(a, "scheme", id) {
            None => None,
            Some(text) => {
                let parsed = Scheme::parse(&text);
                if parsed.is_none() {
                    self.shape(
                        id,
                        format!("the auth scheme \"{text}\" is not one of anonymous, session, service, provisioning-token, signature, signed-address, delegation, in-process"),
                    );
                }
                parsed
        };
        let roles = self.opt_str_list(a, "roles", id).unwrap_or_default();
        let policies = self.opt_str_list(a, "policies", id).unwrap_or_default();
        let tenant_policies = self.opt_bool(a, "tenantPolicies", id).unwrap_or(false);
        let csrf = self.opt_bool(a, "csrf", id).unwrap_or(false);
        let origin = self.opt_bool(a, "origin", id).unwrap_or(false);
        let step_up = self.opt_str(a, "stepUp", id);
        let secret = self.opt_str(a, "secret", id);
        let justification = self.opt_str(a, "justification", id);
        let description = self.opt_str(a, "description", id);
        Some(Auth {
            scheme: scheme?,
            roles,
            policies,
            tenant_policies,
            csrf,
            origin,
            step_up,
            secret,
            justification,
            description,
        })
    }

    fn event(&mut self, id: &str, obj: &Obj) -> Option<Event> {
        let name = self.req_str(obj, "name", id);
        let description = self.opt_str(obj, "description", id);
        let mut payload = Vec::new();
        for f in self.obj_list(obj, "payload", id) {
            let fname = self.req_str(f, "name", id);
            let type_name = self.opt_str(f, "type", id);
            let optional = self.opt_bool(f, "optional", id).unwrap_or(false);
            let sensitive = self.opt_str(f, "sensitive", id);
            if let Some(name) = fname {
                payload.push(PayloadField {
                    name,
                    type_name,
                    optional,
                    sensitive,
                });
            }
        }
        let delivery = self.opt_str(obj, "delivery", id);
        let elapsed = self.opt_bool(obj, "elapsed", id).unwrap_or(false);
        let contract = self.opt_str(obj, "contract", id);
        let role = self.opt_str(obj, "role", id);
        let domain = self.opt_bool(obj, "domain", id).unwrap_or(false);
        let logging = self.logging(obj, id);
        Some(Event {
            id: id.to_string(),
            name: name?,
            description,
            payload,
            delivery,
            elapsed,
            contract,
            role,
            domain,
            logging,
        })
    }

    fn behavior(&mut self, id: &str, obj: &Obj) -> Option<Behavior> {
        let name = self.req_str(obj, "name", id);
        let description = self.opt_str(obj, "description", id);
        let initial = self.opt_str(obj, "initial", id);
        let mut variables = Vec::new();
        for v in self.obj_list(obj, "variables", id) {
            let vname = self.req_str(v, "name", id);
            let repr = self.opt_str(v, "repr", id);
            let initial = v.get("initial").cloned();
            if let Some(name) = vname {
                variables.push(Variable {
                    name,
                    repr,
                    initial,
                });
            }
        }
        let mut states = Vec::new();
        for st in self.obj_list(obj, "states", id) {
            let sid = self.req_str(st, "id", id);
            let sname = self.opt_str(st, "name", id);
            let terminal = self.opt_bool(st, "terminal", id).unwrap_or(false);
            let ignores = self.opt_str_list(st, "ignores", id).unwrap_or_default();
            if let Some(sid) = sid {
                states.push(State {
                    name: sname.unwrap_or_else(|| sid.clone()),
                    id: sid,
                    terminal,
                    ignores,
                });
            }
        }
        let mut transitions = Vec::new();
        for t in self.obj_list(obj, "transitions", id) {
            if let Some(t) = self.transition(id, t) {
                transitions.push(t);
            }
        }
        let memory = match obj.get("storage") {
            Some(Value::Object(st)) => {
                match self.opt_str(st, "durability", id).as_deref() {
                    None | Some("record") => false,
                    Some("memory") => true,
                    Some(other) => {
                        self.shape(
                        id,
                        format!("\"storage.durability\" is \"{other}\"; it must be record or memory"),
                    );
                        false
                    }
                }
            }
            _ => false,
        };
        let storage = match obj.get("storage") {
            // A memory machine has no record, key, fields, state or transaction: whatever the
            // object holds besides `durability` is Exegra's to refuse.
            _ if memory => None,
            None | Some(Value::Null) => None,
            Some(Value::Object(st)) => {
                let string_map = |this: &mut Self, key: &str| match st.get(key) {
                    None | Some(Value::Null) => None,
                    Some(Value::Object(m)) => Some(
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                            .collect::<Vec<_>>(),
                    ),
                    Some(_) => {
                        this.shape(id, format!("\"storage.{key}\" must be an object"));
                        None
                    }
                };
                let fields = string_map(self, "fields").unwrap_or_default();
                let state_values = string_map(self, "stateValues");
                Some(Storage {
                    record: self.opt_str(st, "record", id),
                    key: self.key_fields(st, id),
                    fields,
                    state: self.opt_str(st, "state", id),
                    state_values,
                    creates: self.opt_bool(st, "creates", id).unwrap_or(false),
                })
            }
            Some(_) => {
                self.shape(id, "\"storage\" must be an object");
                None
            }
        };
        let instances = match obj.get("instances") {
            _ if memory => None,
            None | Some(Value::Null) => None,
            Some(Value::Object(i)) => Some(Instances {
                key: self.opt_str(i, "key", id).map(|k| vec![k]),
                stored_in: self.opt_str(i, "storedIn", id),
                counted: i.contains_key("count"),
                max: i.get("max").and_then(Value::as_u64),
            }),
            Some(_) => {
                self.shape(id, "\"instances\" must be an object");
                None
            }
        };
        let isolation = match obj.get("transaction") {
            _ if memory => None,
            None | Some(Value::Null) => None,
            Some(Value::Object(t)) => self.opt_str(t, "isolation", id),
            Some(_) => {
                self.shape(id, "\"transaction\" must be an object");
                None
            }
        };
        Some(Behavior {
            id: id.to_string(),
            name: name?,
            description,
            initial,
            variables,
            states,
            transitions,
            storage,
            instances,
            isolation,
            time_units: Vec::new(),
            routes: Vec::new(),
            injected: Vec::new(),
            target: false,
            memory,
            declared: 0,
        })
    }

    fn transition(&mut self, behavior: &str, t: &Obj) -> Option<Transition> {
        let id = self.req_str(t, "id", behavior);
        let at = id.clone().unwrap_or_else(|| behavior.to_string());
        let from = self.req_str(t, "from", &at);
        let to = self.req_str(t, "to", &at);
        let on = self.req_str(t, "on", &at);
        let guard = self.opt_str(t, "guard", &at);
        let otherwise = self.opt_bool(t, "otherwise", &at).unwrap_or(false);
        let fault_only = self.opt_bool(t, "faultOnly", &at).unwrap_or(false);
        let mut effects = Vec::new();
        for e in self.obj_list(t, "effects", &at) {
            if let Some(effect) = self.effect(&at, e) {
                effects.push(effect);
            }
        }
        Some(Transition {
            id: id?,
            from: from?,
            to: to?,
            on: on?,
            guard,
            otherwise,
            fault_only,
            effects,
        })
    }

    fn payload_entries(&mut self, obj: &Obj, entity: &str) -> Vec<(String, Value)> {
        match obj.get("payload") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(p)) => p.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Some(_) => {
                self.shape(entity, "an effect's \"payload\" must be an object");
                Vec::new()
            }
        }
    }

    fn effect(&mut self, entity: &str, e: &Obj) -> Option<Effect> {
        if e.contains_key("set") {
            let variable = self.req_str(e, "set", entity)?;
            return match e.get("to") {
                Some(Value::Object(_)) => Some(Effect::Other {
                    kind: "set with a sampled value".to_string(),
                }),
                Some(to) => Some(Effect::Set {
                    variable,
                    to: to.clone(),
                }),
                None => {
                    self.shape(entity, format!("the set of \"{variable}\" has no \"to\""));
                    None
                }
            };
        }
        if e.contains_key("timer") {
            let name = self.req_str(e, "timer", entity);
            let event = self.req_str(e, "emit", entity);
            let payload = self.payload_entries(e, entity);
            return Some(Effect::Timer {
                name: name?,
                after: e.get("after").cloned(),
                event: event?,
                payload,
            });
        }
        if e.contains_key("cancel") {
            return Some(Effect::Cancel {
                name: self.req_str(e, "cancel", entity)?,
            });
        }
        if e.contains_key("emit") {
            let event = self.req_str(e, "emit", entity);
            let to = self.opt_str(e, "to", entity);
            let target = self.opt_str(e, "target", entity);
            let payload = self.payload_entries(e, entity);
            let link = ["via", "size", "fanout"]
                .into_iter()
                .filter(|k| {
                    e.get(*k)
                        .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
                })
                .map(str::to_string)
                .collect();
            return Some(Effect::Emit {
                event: event?,
                to,
                target,
                after: e.get("after").cloned(),
                payload,
                link,
            });
        }
        let kind = ["use", "acquire", "release"]
            .into_iter()
            .find(|k| e.contains_key(*k))
            .map_or_else(
                || {
                    e.keys()
                        .next()
                        .cloned()
                        .unwrap_or_else(|| "empty".to_string())
                },
                str::to_string,
            );
        Some(Effect::Other { kind })
    }
}

/// The distinct time units of the scenarios that run behavior `id`, in specification order, as
/// Exegra's `machineTimeUnit` (`src/core/sim-input.js`) finds them: a scenario that needs format 2
/// (`needsFormat2`) counts in its `timeUnit`, by default `us`; a format-1 scenario only when it
/// names a `timeUnit`.
fn scenario_time_units(top: &serde_json::Map<String, Value>, id: &str) -> Vec<String> {
    let list = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
    let behaviors = list(top.get("behaviors"));
    let behavior = |bid: &str| {
        behaviors
            .iter()
            .find(|b| b.get("id").and_then(Value::as_str) == Some(bid))
    };
    let format2 = |s: &Value| {
        if !list(s.get("workloads")).is_empty()
            || s.get("seed").is_some()
            || s.get("horizon").is_some()
        {
            return true;
        }
        list(s.get("behaviors"))
            .iter()
            .filter_map(Value::as_str)
            .any(|bid| {
                behavior(bid).is_some_and(|b| {
                    b.get("instances").is_some_and(|i| !i.is_null())
                        || list(b.get("transitions")).iter().any(|t| {
                            list(t.get("effects")).iter().any(|fx| {
                                ["use", "acquire", "release", "timer", "cancel", "via"]
                                    .iter()
                                    .any(|k| fx.get(*k).is_some())
                            })
                        })
                })
            })
    };
    let mut units: Vec<String> = Vec::new();
    for s in list(top.get("scenarios")) {
        if !list(s.get("behaviors"))
            .iter()
            .any(|b| b.as_str() == Some(id))
        {
            continue;
        }
        let unit = match s.get("timeUnit").and_then(Value::as_str) {
            Some(u) => u.to_string(),
            None if format2(&s) => "us".to_string(),
            None => continue,
        };
        if !units.contains(&unit) {
            units.push(unit);
        }
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"{ "exegra": "0.1", "system": { "id": "s", "name": "S" } }"#;

    #[test]
    fn a_minimal_specification_loads() {
        let spec = load("specs/a.jsonc", MINIMAL).unwrap();
        assert_eq!(spec.system.id, "s");
        assert!(spec.types.is_empty() && spec.records.is_empty() && spec.contracts.is_empty());
    }

    #[test]
    fn backslashes_in_the_path_become_forward_slashes() {
        let spec = load("specs\\a.jsonc", MINIMAL).unwrap();
        assert_eq!(spec.path, "specs/a.jsonc");
    }

    #[test]
    fn invalid_jsonc_is_a_syntax_diagnostic() {
        let err = load("a", "{ \"system\": ").unwrap_err();
        assert_eq!(err[0].code, SYNTAX);
    }

    #[test]
    fn wrong_shapes_are_all_reported() {
        let text = r#"{ "system": { "id": "s", "name": "S" },
            "types": [ { "id": "t", "name": "T", "kind": "tuple" }, 3 ],
            "records": { } }"#;
        let err = load("a", text).unwrap_err();
        assert!(err.iter().all(|d| d.code == SHAPE));
        assert!(err.iter().any(|d| d.entity == "t"));
        assert!(err.iter().any(|d| d.entity == "types[1]"));
        assert!(err.iter().any(|d| d.message.contains("\"records\"")));
    }

    #[test]
    fn unknown_on_delete_is_a_shape_diagnostic() {
        let text = r#"{ "system": { "id": "s", "name": "S" },
            "records": [ { "id": "r", "name": "R", "owner": "c", "key": ["a"],
              "fields": [ { "name": "a", "type": "u32", "refersTo": "r", "onDelete": "explode" } ] } ] }"#;
        let err = load("a", text).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].entity, "r");
    }
}
