//! The generated functions land in the pure crate and keep it pure: the dependency test of
//! crates/model-core, run on the functions fixture's pure crate, whose functions module holds the
//! generated functions and their examples as tests. Functions add no dependency.
//!
//! A source check in the style of `crates/runtime/tests/dependencies.rs`: it reads the manifest's
//! dependency sections, and scans the sources for paths into crates that do I/O. The crate may
//! depend on serde, serde_json, sha2 and regex only.

use std::path::{Path, PathBuf};

/// The dependencies the pure crate may have.
const ALLOWED: &[&str] = &["serde", "serde_json", "sha2", "regex"];

/// Crates that must never appear, by package name: the runtime and what it brings in.
const FORBIDDEN: &[&str] = &[
    "strata-runtime",
    "strata-model",
    "tokio",
    "axum",
    "reqwest",
    "rusqlite",
];

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
        files.len() >= 5,
        "lib.rs, prelude.rs, types.rs, rule_language.rs and functions.rs: {files:?}"
    );
    assert!(
        files.iter().any(|f| f.ends_with("functions.rs")),
        "the functions module is in the pure crate: {files:?}"
    );
    let mut patterns: Vec<String> = FORBIDDEN
        .iter()
        .map(|name| format!("{}::", name.replace('-', "_")))
        .collect();
    patterns.extend(
        [
            "std::fs",
            "std::net",
            "std::env",
            "std::process",
            "std::time",
            "std::thread",
            "uuid::",
        ]
        .map(str::to_string),
    );
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
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
