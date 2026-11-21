//! The runtime is generic: it depends on no crate that holds platform semantics.
//!
//! A source check, like `crates/verification/tests/purity.rs`, without `cargo metadata`: it reads
//! the manifest's dependency sections and scans every source file for paths into the forbidden
//! crates. Generated and platform code may depend on the runtime, never the other way round.

use std::path::{Path, PathBuf};

/// Crates the runtime must never depend on, by package name.
const FORBIDDEN: &[&str] = &[
    "strata-kernel",
    "strata-substrate",
    "strata-backend",
    "strata-bff",
    "strata-protocol",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The names declared in every dependency section of the manifest: `[dependencies]`,
/// `[dev-dependencies]`, `[build-dependencies]` and their target-specific forms.
fn declared(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    // In a section listing dependencies, and in a `[dependencies.name]` table.
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
        // A renamed dependency: `alias = { package = "strata-kernel", ... }`.
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
fn the_runtime_declares_no_platform_crate() {
    let manifest = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let names = declared(&manifest);
    assert!(
        names.contains(&"rusqlite".to_string()),
        "the manifest was read: {names:?}"
    );
    let found: Vec<&String> = names
        .iter()
        .filter(|name| FORBIDDEN.contains(&name.as_str()))
        .collect();
    assert!(found.is_empty(), "the runtime must not depend on {found:?}");
    // Nor on any other Strata crate: they all depend on the kernel, directly or not.
    let strata: Vec<&String> = names.iter().filter(|n| n.starts_with("strata-")).collect();
    assert!(
        strata.is_empty(),
        "the runtime depends on no Strata crate, found {strata:?}"
    );
}

#[test]
fn the_runtime_names_no_platform_crate_in_its_sources() {
    let mut files = Vec::new();
    sources(&root().join("src"), &mut files);
    sources(&root().join("tests"), &mut files);
    assert!(files.len() > 2);
    let mut found = Vec::new();
    for file in files {
        if file.file_name().is_some_and(|n| n == "dependencies.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for name in FORBIDDEN {
                let path = name.replace('-', "_");
                if line.contains(&format!("{path}::")) || line.contains(&format!("use {path}")) {
                    found.push(format!("{}:{}: {path}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "platform crates named:\n{}",
        found.join("\n")
    );
}
