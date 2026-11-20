//! Reading tenant bundles from disk.
//!
//! A tenant directory holds `tenant.json`. Two conveniences are resolved here, before the pure
//! bundle parser sees the document:
//!
//! - `"template": "<relative path>"` merges the structural sections (roles, objects, workflows,
//!   policies, queries, views) of another JSON file into the bundle. The publication template
//!   is the blog application; a tenant instantiates it with its own members and records.
//! - A record field written as `{ "$file": "<relative path>" }` is replaced by that file's text,
//!   so articles live in Markdown files next to the bundle.
//!
//! `accounts.dev.json`, if present, lists development logins. It is returned separately and the
//! backend only uses it in development mode.

use serde::Deserialize;
use serde_json::Value as Json;
use std::fs;
use std::path::{Path, PathBuf};
use strata_substrate::bundle::Bundle;

/// A development login. Never part of the tenant's own data.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevAccount {
    pub login: String,
    pub password: String,
    pub principal: String,
    pub display_name: String,
    /// The address the account counts as having verified, for the invitations of the fixture.
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LoadedTenant {
    pub bundle: Bundle,
    pub dev_accounts: Vec<DevAccount>,
}

const TEMPLATE_SECTIONS: [&str; 6] = [
    "roles",
    "objects",
    "workflows",
    "policies",
    "queries",
    "views",
];

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_json(path: &Path) -> Result<Json, String> {
    serde_json::from_str(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// The file a bundle names by a relative path, refused when it is outside what the bundle may
/// read: its own directory, and, for a template only, a shared directory (its name starts with
/// `_`) directly under the tenants root, which is where templates live. Symbolic links and `..`
/// are resolved before the check.
fn confined(dir: &Path, relative: &str, shared_allowed: bool) -> Result<PathBuf, String> {
    let outside = || {
        format!(
            "{relative}: the path is outside the tenant directory {}",
            dir.display()
        )
    };
    let base = fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = fs::canonicalize(dir.join(relative)).map_err(|e| format!("{relative}: {e}"))?;
    if path.starts_with(&base) {
        return Ok(path);
    }
    if shared_allowed {
        if let Some(root) = base.parent() {
            let shared = path
                .strip_prefix(root)
                .ok()
                .and_then(|inside| inside.components().next())
                .is_some_and(|first| first.as_os_str().to_string_lossy().starts_with('_'));
            if shared {
                return Ok(path);
            }
        }
    }
    Err(outside())
}

/// Load one tenant directory.
pub fn load_tenant(dir: &Path) -> Result<LoadedTenant, String> {
    let mut doc = read_json(&dir.join("tenant.json"))?;
    let root = doc.as_object_mut().ok_or("tenant.json must be an object")?;

    if let Some(template) = root.remove("template") {
        let relative = template.as_str().ok_or("template must be a path")?;
        let template = read_json(&confined(dir, relative, true)?)?;
        for section in TEMPLATE_SECTIONS {
            if let (Some(value), false) = (template.get(section), root.contains_key(section)) {
                root.insert(section.to_string(), value.clone());
            }
        }
    }

    if let Some(Json::Array(records)) = root.get_mut("records") {
        for record in records {
            let Some(Json::Object(fields)) = record.get_mut("fields") else {
                continue;
            };
            for value in fields.values_mut() {
                let file = value
                    .as_object()
                    .filter(|o| o.len() == 1)
                    .and_then(|o| o.get("$file"))
                    .and_then(Json::as_str);
                if let Some(relative) = file {
                    *value = Json::String(
                        read(&confined(dir, relative, false)?)?
                            .replace("\r\n", "\n")
                            .trim()
                            .to_string(),
                    );
                }
            }
        }
    }

    let bundle =
        Bundle::from_json(&doc.to_string()).map_err(|e| format!("{}: {e}", dir.display()))?;
    let accounts_path = dir.join("accounts.dev.json");
    let dev_accounts = if accounts_path.exists() {
        serde_json::from_str(&read(&accounts_path)?)
            .map_err(|e| format!("{}: {e}", accounts_path.display()))?
    } else {
        Vec::new()
    };
    Ok(LoadedTenant {
        bundle,
        dev_accounts,
    })
}

fn tenant_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("tenant.json").is_file())
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('_'))
        })
        .collect();
    dirs.sort();
    Ok(dirs)
}

/// Load every tenant directory under `root`, in name order, stopping at the first that does not
/// load. Directories whose name starts with `_` hold shared material such as templates and are
/// skipped.
pub fn load_all(root: &Path) -> Result<Vec<LoadedTenant>, String> {
    tenant_dirs(root)?
        .iter()
        .map(|dir| load_tenant(dir))
        .collect()
}

/// Load every tenant directory under `root`, in name order, each on its own: a directory that
/// does not load is returned as its path with the reason, and the others are still loaded.
pub fn load_each(root: &Path) -> Result<Vec<(PathBuf, Result<LoadedTenant, String>)>, String> {
    Ok(tenant_dirs(root)?
        .into_iter()
        .map(|dir| {
            let loaded = load_tenant(&dir);
            (dir, loaded)
        })
        .collect())
}
