//! Conversion between kernel values and plain JSON, directed by field types, plus timestamp
//! parsing. Clients speak plain JSON; the kernel speaks `Value`.

use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use strata_kernel::{FieldId, FieldType, ObjectTypeDef, RecordId, Value};

/// Days since the Unix epoch of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM[:SS]Z` (UTC) into milliseconds since the epoch.
pub fn parse_time(text: &str) -> Result<i64, String> {
    let bad = || format!("not a UTC timestamp: {text:?}");
    let number = |s: &str| s.parse::<i64>().map_err(|_| bad());
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time.strip_suffix('Z').ok_or_else(bad)?)),
        None => (text, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 {
        return Err(bad());
    }
    let (year, month, day) = (number(parts[0])?, number(parts[1])?, number(parts[2])?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    if let Some(time) = time {
        let parts: Vec<&str> = time.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            return Err(bad());
        }
        let (hour, minute) = (number(parts[0])?, number(parts[1])?);
        let second = if parts.len() == 3 {
            number(parts[2])?
        } else {
            0
        };
        if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
            return Err(bad());
        }
        seconds += hour * 3_600 + minute * 60 + second;
    }
    Ok(seconds * 1_000)
}

/// Turn plain JSON into a value of the given field type. `null` stays `null`.
///
/// `resolve` maps a symbolic reference such as `@ines` to a record id; pass a resolver that
/// returns `None` where symbolic references are not allowed.
pub fn coerce(
    ty: &FieldType,
    json: &Json,
    resolve: &dyn Fn(&str) -> Option<RecordId>,
) -> Result<Value, String> {
    let mismatch = || format!("expected {}, got {json}", ty.name());
    Ok(match (ty, json) {
        (_, Json::Null) => Value::Null,
        (FieldType::Str, Json::String(s)) => Value::Str(s.clone()),
        (FieldType::Int, Json::Number(n)) => Value::Int(n.as_i64().ok_or_else(mismatch)?),
        (FieldType::Bool, Json::Bool(b)) => Value::Bool(*b),
        (FieldType::Time, Json::Number(n)) => Value::Time(n.as_i64().ok_or_else(mismatch)?),
        (FieldType::Time, Json::String(s)) => Value::Time(parse_time(s)?),
        (FieldType::Ref { .. }, Json::Number(n)) => {
            Value::Ref(RecordId(n.as_u64().ok_or_else(mismatch)?))
        }
        (FieldType::Ref { .. }, Json::String(s)) => {
            let key = s.strip_prefix('@').ok_or_else(mismatch)?;
            Value::Ref(resolve(key).ok_or_else(|| format!("unknown record key @{key}"))?)
        }
        (FieldType::List(inner), Json::Array(items)) => Value::List(
            items
                .iter()
                .map(|item| coerce(inner, item, resolve))
                .collect::<Result<_, _>>()?,
        ),
        _ => return Err(mismatch()),
    })
}

/// Turn a plain JSON object into field values for an object type. Unknown fields are an error
/// here, so the caller gets the field name rather than a bare type complaint from the kernel.
pub fn coerce_fields(
    object: &ObjectTypeDef,
    fields: &serde_json::Map<String, Json>,
    resolve: &dyn Fn(&str) -> Option<RecordId>,
) -> Result<BTreeMap<FieldId, Value>, String> {
    let mut out = BTreeMap::new();
    for (name, json) in fields {
        let id = FieldId::new(name.clone()).map_err(|e| e.to_string())?;
        let def = object
            .fields
            .get(&id)
            .ok_or_else(|| format!("{} has no field {name}", object.id))?;
        out.insert(
            id,
            coerce(&def.ty, json, resolve).map_err(|e| format!("{name}: {e}"))?,
        );
    }
    Ok(out)
}

/// Turn a value into plain JSON. Timestamps become milliseconds, references become ids.
pub fn to_plain(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => json!(b),
        Value::Int(i) => json!(i),
        Value::Str(s) => json!(s),
        Value::Time(t) => json!(t),
        Value::Ref(id) => json!(id.0),
        Value::List(items) => Json::Array(items.iter().map(to_plain).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utc_timestamps() {
        assert_eq!(parse_time("1970-01-01"), Ok(0));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Ok(951_868_800_000));
        assert_eq!(parse_time("2026-11-18T09:30Z"), Ok(1_794_994_200_000));
        assert!(
            parse_time("2026-11-18T09:30").is_err(),
            "a time needs its Z"
        );
        assert!(parse_time("18/11/2026").is_err());
    }

    #[test]
    fn coerces_by_field_type() {
        let none = |_: &str| None;
        let list = FieldType::List(Box::new(FieldType::Str));
        assert_eq!(
            coerce(&list, &json!(["a", "b"]), &none),
            Ok(Value::List(vec![
                Value::Str("a".into()),
                Value::Str("b".into())
            ]))
        );
        assert!(coerce(&FieldType::Int, &json!("3"), &none).is_err());
        assert_eq!(
            coerce(&FieldType::Time, &json!("1970-01-02"), &none),
            Ok(Value::Time(86_400_000))
        );
        assert_eq!(
            coerce(&FieldType::Bool, &Json::Null, &none),
            Ok(Value::Null)
        );
    }
}
