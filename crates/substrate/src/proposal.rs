//! Proposed schema changes, written as text.
//!
//! One change per line, in terms of the tenant's own object and field names:
//!
//! ```text
//! require post.excerpt
//! optional post.excerpt
//! unique post.title
//! field post.subtitle: string
//! remove field post.tags
//! constraint post.short_title: len(new.title) <= 60 :: A title has at most 60 characters
//! remove constraint post.excerpt_length
//! ```
//!
//! The text is turned into ordinary kernel `SchemaChange` values against the current schema.
//! It is a convenience for people; the kernel only ever sees the changes.

use crate::syntax;
use strata_kernel::{
    ConstraintDef, ConstraintId, FieldDef, FieldId, FieldType, ObjectTypeId, SchemaChange, TenantSchema,
};

fn target<'a>(schema: &'a TenantSchema, path: &str) -> Result<(ObjectTypeId, &'a strata_kernel::ObjectTypeDef, String), String> {
    let (object, name) = path.split_once('.').ok_or_else(|| format!("expected object.name, found {path:?}"))?;
    let id = ObjectTypeId::new(object).map_err(|e| e.to_string())?;
    let def = schema.objects.get(&id).ok_or_else(|| format!("there is no object type {object}"))?;
    Ok((id, def, name.to_string()))
}

fn existing_field(schema: &TenantSchema, path: &str) -> Result<(ObjectTypeId, FieldDef), String> {
    let (object, def, name) = target(schema, path)?;
    let field = FieldId::new(name.clone()).map_err(|e| e.to_string())?;
    let found = def.fields.get(&field).ok_or_else(|| format!("{object} has no field {name}"))?;
    Ok((object, found.clone()))
}

fn field_type(text: &str) -> Result<FieldType, String> {
    Ok(match text {
        "string" => FieldType::Str,
        "int" => FieldType::Int,
        "bool" => FieldType::Bool,
        "timestamp" => FieldType::Time,
        other => return Err(format!("unknown field type {other:?}; use string, int, bool or timestamp")),
    })
}

fn parse_line(schema: &TenantSchema, line: &str) -> Result<SchemaChange, String> {
    let (verb, rest) = line.split_once(' ').map_or((line, ""), |(v, r)| (v, r.trim()));
    match verb {
        "require" | "optional" | "unique" => {
            let (object, mut field) = existing_field(schema, rest)?;
            match verb {
                "require" => field.required = true,
                "optional" => field.required = false,
                _ => field.unique = true,
            }
            Ok(SchemaChange::DefineField { object, field })
        }
        "field" => {
            let (path, ty) = rest.split_once(':').ok_or("expected `field object.name: type`")?;
            let (object, _, name) = target(schema, path.trim())?;
            let id = FieldId::new(name.clone()).map_err(|e| e.to_string())?;
            let label = name.replace('_', " ");
            Ok(SchemaChange::DefineField {
                object,
                field: FieldDef { id, label, ty: field_type(ty.trim())?, required: false, unique: false, default: None },
            })
        }
        "constraint" => {
            let (path, rule) = rest.split_once(':').ok_or("expected `constraint object.name: expression :: message`")?;
            let (expr, message) = rule.split_once("::").map_or((rule, ""), |(e, m)| (e, m.trim()));
            let (object, _, name) = target(schema, path.trim())?;
            let expr = syntax::parse(expr.trim())?;
            Ok(SchemaChange::DefineConstraint {
                object,
                constraint: ConstraintDef {
                    id: ConstraintId::new(name).map_err(|e| e.to_string())?,
                    message: if message.is_empty() { expr.to_string() } else { message.to_string() },
                    expr,
                },
            })
        }
        "remove" => {
            let (kind, path) = rest.split_once(' ').ok_or("expected `remove field object.name` or `remove constraint object.name`")?;
            let (object, def, name) = target(schema, path.trim())?;
            match kind {
                "field" => {
                    let field = FieldId::new(name.clone()).map_err(|e| e.to_string())?;
                    if !def.fields.contains_key(&field) {
                        return Err(format!("{object} has no field {name}"));
                    }
                    Ok(SchemaChange::RemoveField { object, field })
                }
                "constraint" => {
                    let constraint = ConstraintId::new(name.clone()).map_err(|e| e.to_string())?;
                    if !def.constraints.contains_key(&constraint) {
                        return Err(format!("{object} has no constraint {name}"));
                    }
                    Ok(SchemaChange::RemoveConstraint { object, constraint })
                }
                other => Err(format!("cannot remove a {other}; use `field` or `constraint`")),
            }
        }
        other => Err(format!("unknown change {other:?}; use require, optional, unique, field, constraint or remove")),
    }
}

/// Parse a proposal. Blank lines and lines starting with `#` are ignored. The error names the
/// line it is about.
pub fn parse(schema: &TenantSchema, text: &str) -> Result<Vec<SchemaChange>, String> {
    let mut changes = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        changes.push(parse_line(schema, line).map_err(|problem| format!("line {}: {problem}", number + 1))?);
    }
    if changes.is_empty() {
        return Err("write at least one change, such as `require post.excerpt`".into());
    }
    if changes.len() > 20 {
        return Err("a proposal has at most 20 changes".into());
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_kernel::testkit::*;

    fn schema() -> std::sync::Arc<TenantSchema> {
        notes_world().tenants[&tenant_id("alpha")].schema.clone()
    }

    #[test]
    fn parses_each_kind_of_change() {
        let text = "# tighten notes\nrequire note.rank\n\nunique note.title\nfield note.subtitle: string\nconstraint note.short: len(new.title) <= 10 :: Keep it short\nremove constraint note.title_length\nremove field note.body\n";
        let changes = parse(&schema(), text).unwrap();
        assert_eq!(changes.len(), 6);
        assert!(matches!(&changes[0], SchemaChange::DefineField { field, .. } if field.required && field.ty == FieldType::Int));
        assert!(matches!(&changes[1], SchemaChange::DefineField { field, .. } if field.unique && field.required));
        assert!(matches!(&changes[3], SchemaChange::DefineConstraint { constraint, .. } if constraint.message == "Keep it short"));
        assert!(matches!(&changes[5], SchemaChange::RemoveField { .. }));
    }

    #[test]
    fn errors_name_the_line_and_the_problem() {
        let s = schema();
        assert_eq!(parse(&s, "require note.rank\nrequire note.nope").unwrap_err(), "line 2: note has no field nope");
        assert!(parse(&s, "require ghost.title").unwrap_err().contains("no object type ghost"));
        assert!(parse(&s, "constraint note.x: len(").unwrap_err().starts_with("line 1:"));
        assert!(parse(&s, "drop table note").unwrap_err().contains("unknown change"));
        assert!(parse(&s, "\n# nothing\n").unwrap_err().contains("at least one change"));
    }
}
