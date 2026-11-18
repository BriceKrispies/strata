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

fn violations_in(name: &str, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (number, code) in shipped_lines(text) {
        for (token, what) in FORBIDDEN {
            if code.contains(token) {
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
