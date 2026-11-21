//! The pure crate stays pure: the kernel depends on it, and the kernel may not touch files, the
//! network, the clock, randomness or the environment (KRN-012).
//!
//! A source check in the style of `crates/runtime/tests/dependencies.rs`: it reads the manifest's
//! dependency sections, and scans the sources for paths into crates that do I/O. The crate may
//! depend on serde, serde_json, sha2 and regex, and on uuid (for uuid ids) with its `serde`
//! feature only: default features off and no `v4` or other generator feature, so nothing in it
//! can make a random or clock-based id.

use std::path::{Path, PathBuf};

/// The dependencies the pure crate may have.
const ALLOWED: &[&str] = &["serde", "serde_json", "sha2", "regex", "uuid"];

/// Crates that must never appear, by package name: the runtime and what it brings in, and crates
/// that do I/O, networking or randomness.
const FORBIDDEN: &[&str] = &[
    "strata-runtime",
    "strata-model",
    "tokio",
    "axum",
    "hyper",
    "mio",
    "reqwest",
    "rusqlite",
    "rand",
    "getrandom",
];

/// The only features the pure crate may turn on in uuid: decoding and encoding ids.
const UUID_FEATURES: &[&str] = &["serde"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The names declared in every dependency section of the manifest: `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` and their target-specific forms.
fn declared(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let (mut in_list, mut in_table) = (false, false);
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            let section = line.trim_matches(|c| c == '[' || c == ']');
            in_list = section.ends_with("dependencies");
            in_table = false;
            if let Some((head, name)) = section.rsplit_once('.') {
                if head.ends_with("dependencies") {
                    in_table = true;
                    names.push(name.trim_matches('"').to_string());
                }
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') || !(in_list || in_table) {
            continue;
        }
        if in_list {
            if let Some(name) = line.split(['.', '=', ' ']).next() {
                names.push(name.trim_matches('"').to_string());
            }
        }
        // A renamed dependency: `alias = { package = "tokio", ... }`.
        if let Some(rest) = line.split("package").nth(1) {
            if let Some(name) = rest.split('"').nth(1) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// The uuid dependency's line in the manifest: `uuid = { ... }` (or `uuid = "..."`), if any.
fn uuid_line(manifest: &str) -> Option<String> {
    manifest
        .lines()
        .map(str::trim)
        .find(|line| line.split(['=', ' ']).next() == Some("uuid"))
        .map(str::to_string)
}

/// Why a uuid dependency line is not pure, or `None` when it is: default features must be off
/// (they are `std`, which is harmless, but turning them off keeps the feature list exact) and
/// every feature listed must be in [`UUID_FEATURES`], so `v4` (randomness) and the clock-based
/// generators (`v1`, `v6`, `v7`) are refused.
fn uuid_problem(line: &str) -> Option<String> {
    if !line.contains("default-features = false") {
        return Some(format!("uuid must turn default features off: {line}"));
    }
    let features = line
        .split("features")
        .filter_map(|rest| {
            let list = rest.split('[').nth(1)?.split(']').next()?;
            Some(list.to_string())
        })
        .collect::<Vec<_>>()
        .join(",");
    let extra: Vec<&str> = features
        .split(',')
        .map(|f| f.trim().trim_matches('"'))
        .filter(|f| !f.is_empty() && !UUID_FEATURES.contains(f))
        .collect();
    if !extra.is_empty() {
        return Some(format!(
            "uuid may have only the features {UUID_FEATURES:?}, found {extra:?}: {line}"
        ));
    }
    None
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_pure_crate_declares_only_pure_dependencies() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let names = declared(&manifest);
    assert!(
        names.contains(&"serde".to_string()),
        "the manifest was read: {names:?}"
    );
    let forbidden: Vec<&String> = names
        .iter()
        .filter(|n| FORBIDDEN.contains(&n.as_str()) || n.starts_with("strata-"))
        .collect();
    assert!(
        forbidden.is_empty(),
        "the pure crate must not depend on {forbidden:?}"
    );
    let unexpected: Vec<&String> = names
        .iter()
        .filter(|n| !ALLOWED.contains(&n.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "the pure crate depends only on {ALLOWED:?}, found {unexpected:?}"
    );
}

#[test]
fn the_pure_crate_names_no_impure_crate_or_std_io_in_its_sources() {
    let mut files = Vec::new();
    sources(&root().join("src"), &mut files);
    assert!(
        files.len() >= 3,
        "lib.rs, prelude.rs and types.rs: {files:?}"
    );
    let mut patterns: Vec<String> = FORBIDDEN
        .iter()
        .map(|name| format!("{}::", name.replace('-', "_")))
        .collect();
    // A uuid is named (`uuid::Uuid`), but never made: no generator of the uuid crate.
    patterns.extend(
        [
            "std::fs",
            "std::net",
            "std::env",
            "std::io",
            "std::process",
            "std::time",
            "std::thread",
            "Uuid::new_v",
            "Uuid::now_v",
        ]
        .map(str::to_string),
    );
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        // A `#[cfg(test)]` item is not shipped: it is skipped to its closing brace, as the
        // kernel's purity scan does (crates/verification/tests/purity.rs).
        let mut skipping: Option<i64> = None;
        let mut marked = false;
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let depth_change = line.matches('{').count() as i64 - line.matches('}').count() as i64;
            if let Some(depth) = skipping {
                let depth = depth + depth_change;
                skipping = (depth > 0).then_some(depth);
                continue;
            }
            if line.trim() == "#[cfg(test)]" {
                marked = true;
                continue;
            }
            if marked {
                marked = false;
                if depth_change > 0 {
                    skipping = Some(depth_change);
                }
                continue;
            }
            for pattern in &patterns {
                if line.contains(pattern.as_str()) {
                    found.push(format!("{}:{}: {pattern}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "impure paths named:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_check_would_catch_a_dependency() {
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n\
                    tokio = { version = \"1\" }\n\n[dev-dependencies]\n\
                    alias = { package = \"rusqlite\", version = \"0.32\" }\n\n\
                    [target.'cfg(unix)'.dependencies.strata-runtime]\npath = \"../runtime\"\n";
    let names = declared(manifest);
    for name in ["tokio", "rusqlite", "strata-runtime"] {
        assert!(
            names.contains(&name.to_string()),
            "{name} missed in {names:?}"
        );
    }
    assert!(
        !names.contains(&"x".to_string()),
        "package fields are not dependencies"
    );
}

#[test]
fn uuid_is_declared_with_the_serde_feature_only() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    if let Some(line) = uuid_line(&manifest) {
        assert_eq!(uuid_problem(&line), None);
    }
}

#[test]
fn the_uuid_check_would_catch_a_generator_feature() {
    let pure = "uuid = { version = \"1\", default-features = false, features = [\"serde\"] }";
    assert_eq!(uuid_problem(pure), None);
    for line in [
        "uuid = { version = \"1\", default-features = false, features = [\"serde\", \"v4\"] }",
        "uuid = { version = \"1\", default-features = false, features = [\"v7\"] }",
        "uuid = { version = \"1\", features = [\"serde\"] }",
        "uuid = \"1\"",
    ] {
        assert!(uuid_problem(line).is_some(), "{line} was accepted");
    }
    let manifest = format!(
        "[dependencies]
serde = \"1\"
{pure}
"
    );
    assert_eq!(uuid_line(&manifest).as_deref(), Some(pure));
}
