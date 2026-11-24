//! The tenant theme schema. A tenant chooses among named tokens; it never supplies CSS.
//!
//! The stylesheet defines what each token name means. Tenant data only selects names, and a
//! name outside the schema falls back to the default, so no tenant value ever reaches a
//! stylesheet or a style attribute.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use strata_kernel::ext::*;
use strata_kernel::{
    FieldId, Principal, SettingDef, SettingId, SettingLevel, SettingType, TenantState, Value,
};

pub const ACCENTS: [&str; 5] = ["moss", "clay", "ink", "ochre", "plum"];
pub const PAPERS: [&str; 3] = ["linen", "stone", "chalk"];
/// The design-system presets of specifications/ui-catalogue.jsonc. The first is the default.
pub const THEMES: [&str; 2] = ["fieldnotes", "neutral"];

/// The tenant setting that holds the design-system preset of a tenant that has no settings record
/// of its own to say (D-DS-15). The tenant document's `theme` defines it, with the preset as its
/// default, and the owner may change it at the tenant level.
pub const THEME_SETTING: &str = "ui_theme";

/// The definition of [`THEME_SETTING`] with `theme` as its value, or why the name is not a preset.
pub fn setting_def(theme: &str) -> Result<SettingDef, String> {
    let theme = THEMES
        .iter()
        .find(|name| **name == theme)
        .ok_or_else(|| format!("theme must be one of {THEMES:?}, got {theme:?}"))?;
    Ok(SettingDef {
        id: SettingId::new(THEME_SETTING).map_err(|e| e.to_string())?,
        label: "Design-system preset".to_string(),
        r#type: SettingType::new("string").map_err(|e| e.to_string())?,
        default: Value::Str((*theme).to_string()),
        levels: BTreeSet::from([SettingLevel::Tenant]),
        locked_by: None,
    })
}

/// The preset the tenant's own configuration names, or `None` when it names none or a name
/// outside [`THEMES`].
pub fn tenant_theme(tenant: &TenantState) -> Option<&'static str> {
    let id = SettingId::new(THEME_SETTING).ok()?;
    let found = strata_kernel::settings::resolve(tenant, &Principal::System, &id).ok()?;
    pick(&THEMES, Some(&found.value))
}

/// The preset a settings record names in its `theme` field, when it names one of [`THEMES`].
pub fn record_theme(fields: &BTreeMap<FieldId, Value>) -> Option<&'static str> {
    let id = FieldId::new("theme").ok()?;
    pick(&THEMES, fields.get(&id))
}

/// The preset of that name, when there is one.
pub fn known_theme(name: &str) -> Option<&'static str> {
    THEMES.iter().copied().find(|theme| *theme == name)
}

/// A resolved theme: every member is one of the names above.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Theme {
    pub accent: &'static str,
    pub paper: &'static str,
    /// The preset (`data-theme`). Left out of the view while it is the default, so a tenant that
    /// chooses nothing is served what it always was.
    #[serde(skip_serializing_if = "is_default_theme")]
    pub theme: &'static str,
}

fn is_default_theme(theme: &&str) -> bool {
    *theme == THEMES[0]
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            accent: ACCENTS[0],
            paper: PAPERS[0],
            theme: THEMES[0],
        }
    }
}

fn pick(allowed: &[&'static str], value: Option<&Value>) -> Option<&'static str> {
    let wanted = value?.as_str()?;
    allowed.iter().copied().find(|name| *name == wanted)
}

/// Resolve a theme from a settings record's fields (`accent`, `paper`, `theme`). Returns the theme and
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
        theme: choose("theme", &THEMES, THEMES[0]),
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
                paper: "stone",
                theme: "fieldnotes"
            }
        );
        assert_eq!(resolve(&fields(&[("theme", "neutral")])).0.theme, "neutral");
        let (theme, rejected) = resolve(&fields(&[("theme", "evil\"><script>")]));
        assert_eq!(theme, Theme::default());
        assert_eq!(rejected, vec!["theme"]);
        let (theme, rejected) = resolve(&fields(&[("accent", "red; background: url(//evil)")]));
        assert_eq!(theme, Theme::default());
        assert_eq!(rejected, vec!["accent"]);
    }
}
