//! K1: the kernel and the substrate stay pure.
//!
//! A source scan, not a proof: it fails when either crate names an ambient capability (file
//! system, network, clock, randomness, environment, process, threads), interior mutability,
//! global state or `unsafe`, or gains a dependency outside its allowance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FORBIDDEN: &[(&str, &str)] = &[
    ("std::fs", "file system"),
    ("std::net", "network"),
    ("std::io", "input and output"),
    ("std::env", "environment"),
    ("std::process", "processes"),
    ("std::thread", "threads"),
    ("std::time", "the clock"),
    ("SystemTime", "the clock"),
    ("Instant::", "the clock"),
    ("rand::", "randomness"),
    ("getrandom", "randomness"),
    ("RefCell", "interior mutability"),
    ("Cell<", "interior mutability"),
    ("Mutex", "shared mutable state"),
    ("RwLock", "shared mutable state"),
    ("Atomic", "shared mutable state"),
    ("static mut", "global mutable state"),
    ("thread_local", "global mutable state"),
    ("lazy_static", "global state"),
    ("OnceLock", "global state"),
    ("OnceCell", "global state"),
    ("unsafe", "unsafe code"),
    ("HashMap", "unordered iteration"),
    ("HashSet", "unordered iteration"),
    ("println!", "output"),
    ("eprintln!", "output"),
    ("dbg!", "output"),
    ("tokio", "an async runtime"),
    ("async fn", "an async runtime"),
    // Ambient facilities reached other than by a plain `std::x::y` path.
    ("RandomState", "randomness (hash seeds)"),
    ("env!", "environment, read at build time"),
    ("option_env!", "environment, read at build time"),
    ("include_str!", "file system, read at build time"),
    ("include_bytes!", "file system, read at build time"),
    ("include!", "file system, read at build time"),
    ("extern crate", "a crate outside the allowance"),
    ("std::{", "a grouped std import, which the scan cannot read"),
    ("std::os", "the operating system"),
    ("std::ffi", "the operating system"),
    ("std::path", "the file system"),
    ("std::alloc", "the allocator"),
    ("#[link", "foreign code"),
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
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

/// The code of a line: nothing at all for comment lines, and otherwise the line without its
/// string literal contents and without any trailing `//` comment. A `//` inside a string literal
/// is not a comment, so it must not cut the rest of the line out of the scan.
fn code_of(line: &str) -> String {
    if line.trim_start().starts_with("//") {
        return String::new();
    }
    let mut code = String::new();
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => {
                    in_string = false;
                    code.push('"');
                }
                _ => {}
            }
        } else if c == '\'' && chars.peek() == Some(&'"') {
            // The character literal '"' is not the start of a string.
            chars.next();
            chars.next();
        } else if c == '"' {
            in_string = true;
            code.push('"');
        } else if c == '/' && chars.peek() == Some(&'/') {
            break;
        } else {
            code.push(c);
        }
    }
    code
}

/// The lines that ship, with their numbers: everything except items marked `#[cfg(test)]`.
/// Such an item is skipped to the end of its braces (or its semicolon), so code that follows a
/// test module in the same file is still scanned.
fn shipped_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut marked = false;
    // Inside a test-only item: the brace depth, and whether its first brace was seen.
    let mut skipping: Option<(i64, bool)> = None;
    for (index, line) in text.lines().enumerate() {
        let code = code_of(line);
        let opens = code.matches('{').count() as i64;
        let closes = code.matches('}').count() as i64;
        if let Some((depth, opened)) = skipping {
            let (depth, opened) = (depth + opens - closes, opened || opens > 0);
            skipping = if (opened && depth <= 0) || (!opened && code.trim_end().ends_with(';')) {
                None
            } else {
                Some((depth, opened))
            };
            continue;
        }
        if code.trim() == "#[cfg(test)]" {
            marked = true;
            continue;
        }
        if marked {
            marked = false;
            let ends_here =
                (opens > 0 && opens <= closes) || (opens == 0 && code.trim_end().ends_with(';'));
            if !ends_here {
                skipping = Some((opens - closes, opens > 0));
            }
            continue;
        }
        out.push((index + 1, code));
    }
    out
}

/// Whether `code` names `token`: a token that starts with a letter must not be the tail of a longer
/// identifier, so `rand::` is not found in `Operand::` nor `Cell<` in `RefCell<`' own text twice.
fn names(code: &str, token: &str) -> bool {
    let word = token.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    code.match_indices(token).any(|(at, _)| {
        !word
            || !code[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn violations_in(name: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (number, code) in shipped_lines(text) {
        for (token, what) in FORBIDDEN {
            if names(&code, token) {
                found.push(format!("{name}:{number}: `{token}` ({what})"));
            }
        }
    }
    found
}

fn violations(crate_dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    sources(&root().join(crate_dir).join("src"), &mut files);
    assert!(!files.is_empty());
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        found.extend(violations_in(&file.display().to_string(), &text));
    }
    found
}

/// Every dependency table of a crate's manifest, by its header: `dependencies`,
/// `dev-dependencies`, `build-dependencies` and any target-specific one
/// (`target.'cfg(unix)'.dependencies` and the like), each with the names it lists. A
/// `[dependencies.name]` header lists `name` in `dependencies`.
fn dependency_tables(crate_dir: &str) -> BTreeMap<String, Vec<String>> {
    let manifest = std::fs::read_to_string(root().join(crate_dir).join("Cargo.toml")).unwrap();
    dependency_tables_of(&manifest)
}

fn dependency_tables_of(manifest: &str) -> BTreeMap<String, Vec<String>> {
    let mut tables: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']').trim_start_matches('[').trim();
            current = None;
            if header.ends_with("dependencies") {
                tables.entry(header.to_string()).or_default();
                current = Some(header.to_string());
            } else if let Some((table, name)) = header.rsplit_once(".dependencies.") {
                // `[dependencies.name]`, `[target.x.dependencies.name]`.
                let table = format!("{table}.dependencies");
                tables.entry(table).or_default().push(name.to_string());
            } else if let Some((table, name)) = header
                .split_once('.')
                .filter(|(table, _)| table.ends_with("dependencies"))
            {
                tables
                    .entry(table.to_string())
                    .or_default()
                    .push(name.to_string());
            }
        } else if let Some(table) = &current {
            if let Some(name) = line.split(['.', '=', ' ']).next().filter(|n| !n.is_empty()) {
                tables.get_mut(table).unwrap().push(name.to_string());
            }
        }
    }
    tables
}

fn dependencies(crate_dir: &str) -> Vec<String> {
    dependency_tables(crate_dir)
        .remove("dependencies")
        .unwrap_or_default()
}

/// Verifies: KRN-012
/// Inspects: crates/kernel/src
#[test]
fn the_kernel_names_no_ambient_capability() {
    let found = violations("kernel");
    assert!(
        found.is_empty(),
        "the kernel must stay pure:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_substrate_names_no_ambient_capability() {
    let found = violations("substrate");
    assert!(
        found.is_empty(),
        "the substrate must stay pure:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_kernel_depends_only_on_serialisation_hashing_and_its_generated_types() {
    // strata-model-core holds the kernel's data types, generated from the specification, and the
    // substrate names them too; its own dependency test (crates/model-core/tests/dependencies.rs)
    // keeps it pure (KRN-012).
    assert_eq!(
        dependencies("kernel"),
        [
            "serde",
            "serde_json",
            "sha2",
            "strata-model-core",
            // Unicode data and algorithms with no I/O, no clock and no global mutable state
            // (docs/decisions.md D22): normalisation on write and the emoji properties.
            "unicode-normalization",
            "unicode-properties",
        ]
    );
    assert_eq!(
        dependencies("substrate"),
        ["strata-kernel", "strata-model-core", "serde", "serde_json"]
    );
}

/// Build, dev and target-specific dependency tables are scanned as well: a build script or a
/// platform-specific dependency is as much a dependency as an ordinary one. The only other table
/// either crate has is its dev-dependencies.
#[test]
fn no_other_dependency_table_widens_the_allowance() {
    let names = |list: &[&str]| list.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let other = |dir: &str| {
        let mut tables = dependency_tables(dir);
        tables.remove("dependencies");
        tables
    };
    assert_eq!(
        other("kernel"),
        BTreeMap::from([(
            "dev-dependencies".to_string(),
            names(&["proptest", "strata-kernel"])
        )])
    );
    assert_eq!(
        other("substrate"),
        BTreeMap::from([("dev-dependencies".to_string(), names(&["strata-kernel"]))])
    );
}

#[test]
fn the_manifest_scan_sees_build_and_target_tables() {
    let manifest = "[dependencies]
serde.workspace = true

[build-dependencies]
cc = \"1\"

[target.'cfg(unix)'.dependencies]
libc = \"0.2\"

[dependencies.rand]
version = \"0.8\"
";
    let tables = dependency_tables_of(manifest);
    assert_eq!(tables["dependencies"], ["serde", "rand"]);
    assert_eq!(tables["build-dependencies"], ["cc"]);
    assert_eq!(tables["target.'cfg(unix)'.dependencies"], ["libc"]);
}

#[test]
fn only_test_items_are_left_out_of_the_scan() {
    let source = "fn shipped() {}
#[cfg(test)]
mod tests {
    fn helper() {
        let _ = std::fs::read(\"x\");
    }
}
fn also_shipped() {
    let _ = std::time::SystemTime::now();
}
#[cfg(test)]
use std::env;
fn last() {
    let _ = std::process::id();
}
";
    let found = violations_in("probe.rs", source);
    assert_eq!(
        found,
        [
            "probe.rs:9: `std::time` (the clock)",
            "probe.rs:9: `SystemTime` (the clock)",
            "probe.rs:14: `std::process` (processes)",
        ],
        "code after a test module is scanned, a test-only module and import are not"
    );
}

#[test]
fn the_scan_would_catch_a_violation() {
    assert_eq!(
        code_of("    let now = std::time::SystemTime::now(); // why"),
        "    let now = std::time::SystemTime::now(); "
    );
    assert_eq!(code_of("    /// reads no clock (std::time)"), "");
    assert!(FORBIDDEN
        .iter()
        .any(|(token, _)| "std::time::SystemTime::now()".contains(token)));
    // A `//` inside a string literal does not hide the rest of the line from the scan.
    assert!(
        code_of("let u = \"http://x\"; let t = std::time::Instant::now();").contains("std::time")
    );
    // A character literal that is a quotation mark does not open a string.
    assert!(code_of("if c == '\"' { std::fs::read(p); }").contains("std::fs"));
    // Grouped imports and build-time reads are visible as well.
    assert!(code_of("use std::{fs, io};").contains("std::{"));
    assert!(code_of("const HOME: &str = env!(\"HOME\");").contains("env!"));
    assert!(
        code_of("let s = std::collections::hash_map::RandomState::new();").contains("RandomState")
    );
}

/// Every workspace member, the generated crates and the specification compiler's fixture crates
/// included, inherits the workspace lints (`unsafe_code = "forbid"` among them). The generated
/// crates' manifests are written by specc, so the line is emitted there.
#[test]
fn every_workspace_member_inherits_the_workspace_lints() {
    let workspace = std::fs::read_to_string(root().join("..").join("Cargo.toml")).unwrap();
    let members: Vec<&str> = workspace
        .split("members = [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .expect("the workspace lists its members")
        .split('"')
        .skip(1)
        .step_by(2)
        .collect();
    assert!(members.len() > 20, "{members:?}");
    let mut without = Vec::new();
    for member in members {
        let manifest =
            std::fs::read_to_string(root().join("..").join(member).join("Cargo.toml")).unwrap();
        let inherits = manifest
            .split("[lints]")
            .nth(1)
            .and_then(|rest| rest.lines().find(|l| !l.trim().is_empty()))
            .is_some_and(|line| line.replace(' ', "") == "workspace=true");
        if !inherits {
            without.push(member);
        }
    }
    assert!(
        without.is_empty(),
        "these members do not inherit the workspace lints ([lints] workspace = true): {without:?}"
    );
}
