//! K1: the kernel and the substrate stay pure.
//!
//! A source scan, not a proof: it fails when either crate names an ambient capability (file
//! system, network, clock, randomness, environment, process, threads), interior mutability,
//! global state or `unsafe`, or gains a dependency outside its allowance.

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

/// The code of a line: without a trailing `//` comment, and nothing at all for doc comments.
fn code_of(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return "";
    }
    line.split("//").next().unwrap_or("")
}

fn violations(crate_dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    sources(&root().join(crate_dir).join("src"), &mut files);
    assert!(!files.is_empty());
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        // Test modules may do what they like; the scan covers what ships.
        let shipped = text.split("#[cfg(test)]").next().unwrap_or("");
        for (number, line) in shipped.lines().enumerate() {
            let code = code_of(line);
            for (token, what) in FORBIDDEN {
                if code.contains(token) {
                    found.push(format!(
                        "{}:{}: `{token}` ({what})",
                        file.display(),
                        number + 1
                    ));
                }
            }
        }
    }
    found
}

fn dependencies(crate_dir: &str) -> Vec<String> {
    let manifest = std::fs::read_to_string(root().join(crate_dir).join("Cargo.toml")).unwrap();
    let section = manifest
        .split("[dependencies]")
        .nth(1)
        .unwrap_or("")
        .split("\n[")
        .next()
        .unwrap_or("");
    section
        .lines()
        .filter_map(|line| line.split(['.', '=', ' ']).next())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// Verifies: KRN-012
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
fn the_kernel_depends_only_on_serialisation_and_hashing() {
    assert_eq!(dependencies("kernel"), ["serde", "serde_json", "sha2"]);
    assert_eq!(
        dependencies("substrate"),
        ["strata-kernel", "serde", "serde_json"]
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
}
