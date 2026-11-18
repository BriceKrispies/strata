//! The tenant theme schema. A tenant chooses among named tokens; it never supplies CSS.
//!
//! The stylesheet defines what each token name means. Tenant data only selects names, and a
//! name outside the schema falls back to the default, so no tenant value ever reaches a
//! stylesheet or a style attribute.

use serde::Serialize;
use std::collections::BTreeMap;
use strata_kernel::{FieldId, Value};

pub const ACCENTS: [&str; 5] = ["moss", "clay", "ink", "ochre", "plum"];
pub const PAPERS: [&str; 3] = ["linen", "stone", "chalk"];

/// A resolved theme: every member is one of the names above.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Theme {
    pub accent: &'static str,
    pub paper: &'static str,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: ACCENTS[0],
            paper: PAPERS[0],
        }
    }
}

fn pick(allowed: &[&'static str], value: Option<&Value>) -> Option<&'static str> {
    let wanted = value?.as_str()?;
    allowed.iter().copied().find(|name| *name == wanted)
}

/// Resolve a theme from a settings record's fields (`accent`, `paper`). Returns the theme and
/// the names of any settings that were present but not in the schema.
pub fn resolve(fields: &BTreeMap<FieldId, Value>) -> (Theme, Vec<&'static str>) {
    let mut rejected = Vec::new();
    let mut choose = |name: &'static str, allowed: &[&'static str], fallback: &'static str| {
        let value = FieldId::new(name).ok().and_then(|id| fields.get(&id));
        match (pick(allowed, value), value) {
            (Some(token), _) => token,
            (None, Some(value)) if !value.is_null() => {
                rejected.push(name);
                fallback
            }
            _ => fallback,
        }
    };
    let theme = Theme {
        accent: choose("accent", &ACCENTS, ACCENTS[0]),
        paper: choose("paper", &PAPERS, PAPERS[0]),
    };
    (theme, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<FieldId, Value> {
        pairs
            .iter()
            .map(|(k, v)| (FieldId::new(*k).unwrap(), Value::Str((*v).to_string())))
            .collect()
    }

    #[test]
    fn known_tokens_are_selected_and_anything_else_falls_back() {
        assert_eq!(
            resolve(&fields(&[("accent", "clay"), ("paper", "stone")])).0,
            Theme {
                accent: "clay",
                paper: "stone"
            }
        );
        let (theme, rejected) = resolve(&fields(&[("accent", "red; background: url(//evil)")]));
        assert_eq!(theme, Theme::default());
        assert_eq!(rejected, vec!["accent"]);
    }
}
