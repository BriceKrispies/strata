//! The specification file as a manifest and parts (WG-5386), and the one way the Rust tools read it.
//!
//! `specifications/spec.jsonc` lists part files: `{ "specParts": ["intent.jsonc", ...] }`. Each part
//! is a JSONC object holding some top-level sections. [`read_spec`] resolves a manifest to the single
//! document a monolithic `spec.jsonc` would hold:
//!
//! * array sections are concatenated in manifest order (`claims` may be in two parts);
//! * a section that is not an array (`system`, `vocabulary`, `exegra`, ...) may be in one part only;
//! * an entity id that two parts give the same array section is refused;
//! * a missing part, a part that is itself a manifest, and a manifest with sections of its own are
//!   refused.
//!
//! Every refusal names the files. Exegra (`tools/spec-manifest.js`), Concordance
//! (`concordance-exegra/src/parts.rs`) and `scripts/spec-parts.mjs` implement the same rule.
//!
//! The assembled text is canonical, so every implementation produces the same bytes: `{`, one member
//! per line at two spaces in the order a key first appears, an array as `[`, its items one per line
//! at four spaces (each item's text unchanged, comments between items dropped) and `]`, any other
//! value unchanged after `"key": `, and `}` with a final newline. A file that is not a manifest is
//! returned as it is.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// The key that makes a file a manifest.
pub const MANIFEST_KEY: &str = "specParts";

/// The optional manifest member that says which part new sections and entities go to. Exegra's `apply`
/// reads it; resolving ignores it.
pub const PLACEMENT_KEY: &str = "specSections";

/// Why a specification could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecPartsError {
    /// A stable code: `read`, `syntax`, `manifest-shape`, `manifest-sections`, `missing-part`,
    /// `nested-manifest`, `duplicate-section`, `duplicate-id`.
    pub code: &'static str,
    /// What is wrong, naming the files.
    pub message: String,
}

impl fmt::Display for SpecPartsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SpecPartsError {}

fn err(code: &'static str, message: String) -> SpecPartsError {
    SpecPartsError { code, message }
}

/// A resolved specification.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The specification text: the assembled document, or the file's own text when it is no manifest.
    pub text: String,
    /// The files read: the manifest first, then the parts in order (just the file when no manifest).
    pub files: Vec<PathBuf>,
    /// Whether the file was a manifest.
    pub manifest: bool,
}

struct Member {
    key: String,
    start: usize,
    end: usize,
    items: Option<Vec<(usize, usize)>>,
}

struct Scanner<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    what: &'a str,
}

impl Scanner<'_> {
    fn fail(&self, message: &str) -> SpecPartsError {
        let line = self.b[..self.i.min(self.b.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count()
            + 1;
        err("syntax", format!("{}: {message} (line {line})", self.what))
    }

    fn skip(&mut self) -> Result<(), SpecPartsError> {
        loop {
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.b.get(self.i) == Some(&b'/') && self.b.get(self.i + 1) == Some(&b'/') {
                while self.i < self.b.len() && self.b[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.b.get(self.i) == Some(&b'/') && self.b.get(self.i + 1) == Some(&b'*') {
                match self.src[self.i + 2..].find("*/") {
                    Some(at) => self.i += 2 + at + 2,
                    None => return Err(self.fail("unterminated block comment")),
                }
            } else {
                return Ok(());
            }
        }
    }

    fn string(&mut self) -> Result<(usize, usize), SpecPartsError> {
        let start = self.i;
        self.i += 1;
        while self.i < self.b.len() && self.b[self.i] != b'"' {
            self.i += if self.b[self.i] == b'\\' { 2 } else { 1 };
        }
        if self.i >= self.b.len() {
            return Err(self.fail("unterminated string"));
        }
        self.i += 1;
        Ok((start, self.i))
    }

    fn value(&mut self) -> Result<(), SpecPartsError> {
        match self.b.get(self.i) {
            Some(b'"') => {
                self.string()?;
            }
            Some(b'{' | b'[') => {
                let mut depth = 0usize;
                loop {
                    match self.b.get(self.i) {
                        None => return Err(self.fail("unterminated value")),
                        Some(b'"') => {
                            self.string()?;
                            continue;
                        }
                        Some(b'/') if matches!(self.b.get(self.i + 1), Some(b'/' | b'*')) => {
                            self.skip()?;
                            continue;
                        }
                        Some(b'{' | b'[') => depth += 1,
                        Some(b'}' | b']') => depth = depth.saturating_sub(1),
                        Some(_) => {}
                    }
                    self.i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            Some(_) => {
                let start = self.i;
                while self.i < self.b.len()
                    && !self.b[self.i].is_ascii_whitespace()
                    && !matches!(self.b[self.i], b',' | b'}' | b']')
                    && !(self.b[self.i] == b'/'
                        && matches!(self.b.get(self.i + 1), Some(b'/' | b'*')))
                {
                    self.i += 1;
                }
                if self.i == start {
                    return Err(self.fail("a value was expected"));
                }
            }
            None => return Err(self.fail("a value was expected")),
        }
        Ok(())
    }
}

/// The top-level members of a JSONC object text, with the offsets of each value and, for an array,
/// of each item.
fn scan_top_level(text: &str, what: &str) -> Result<Vec<Member>, SpecPartsError> {
    let bom = text.starts_with('\u{feff}');
    let mut s = Scanner {
        src: text,
        b: text.as_bytes(),
        i: if bom { 3 } else { 0 },
        what,
    };
    s.skip()?;
    if s.b.get(s.i) != Some(&b'{') {
        return Err(s.fail("the top level is not an object"));
    }
    s.i += 1;
    let mut members = Vec::new();
    loop {
        s.skip()?;
        match s.b.get(s.i) {
            Some(b'}') => break,
            Some(b'"') => {}
            _ => return Err(s.fail("a member name was expected")),
        }
        let (ks, ke) = s.string()?;
        let key: String = serde_json::from_str(&text[ks..ke])
            .map_err(|e| s.fail(&format!("a member name is not valid: {e}")))?;
        s.skip()?;
        if s.b.get(s.i) != Some(&b':') {
            return Err(s.fail("a colon was expected"));
        }
        s.i += 1;
        s.skip()?;
        let start = s.i;
        let mut items = None;
        if s.b.get(s.i) == Some(&b'[') {
            let mut list = Vec::new();
            s.i += 1;
            loop {
                s.skip()?;
                if s.b.get(s.i) == Some(&b']') {
                    s.i += 1;
                    break;
                }
                let item = s.i;
                s.value()?;
                list.push((item, s.i));
                s.skip()?;
                if s.b.get(s.i) == Some(&b',') {
                    s.i += 1;
                }
            }
            items = Some(list);
        } else {
            s.value()?;
        }
        members.push(Member {
            key,
            start,
            end: s.i,
            items,
        });
        s.skip()?;
        match s.b.get(s.i) {
            Some(b',') => s.i += 1,
            Some(b'}') => {}
            _ => return Err(s.fail("a comma or the closing brace was expected")),
        }
    }
    Ok(members)
}

/// Whether a text is a manifest: its top level has the `specParts` key.
#[must_use]
pub fn is_manifest(text: &str) -> bool {
    text.contains(MANIFEST_KEY)
        && scan_top_level(text, "the file").is_ok_and(|m| m.iter().any(|m| m.key == MANIFEST_KEY))
}

/// A JSONC fragment with its comments and trailing commas replaced by spaces.
fn plain_json(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                if i + 1 < b.len() {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                }
                i += 2;
            }
            b',' => {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if matches!(b.get(j), Some(b']' | b'}')) {
                    out[i] = b' ';
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The part file names a manifest lists, in order.
///
/// # Errors
/// A text that is not a manifest of the right shape.
pub fn manifest_parts(text: &str, what: &str) -> Result<Vec<String>, SpecPartsError> {
    let members = scan_top_level(text, what)?;
    let others: Vec<String> = members
        .iter()
        .filter(|m| m.key != MANIFEST_KEY && m.key != PLACEMENT_KEY)
        .map(|m| format!("\"{}\"", m.key))
        .collect();
    if !others.is_empty() {
        return Err(err(
            "manifest-sections",
            format!(
                "{what} is a manifest and lists parts, so it holds no section of its own; found {}",
                others.join(", ")
            ),
        ));
    }
    let own: Vec<&Member> = members.iter().filter(|m| m.key == MANIFEST_KEY).collect();
    if own.len() != 1 {
        return Err(err(
            "manifest-shape",
            format!("{what}: \"{MANIFEST_KEY}\" appears {} times", own.len()),
        ));
    }
    let list: Vec<String> = serde_json::from_str(&plain_json(&text[own[0].start..own[0].end]))
        .map_err(|_| {
            err(
                "manifest-shape",
                format!("{what}: \"{MANIFEST_KEY}\" must be a list of part file names"),
            )
        })?;
    if list.is_empty() || list.iter().any(String::is_empty) {
        return Err(err(
            "manifest-shape",
            format!("{what}: \"{MANIFEST_KEY}\" must be a non-empty list of part file names"),
        ));
    }
    for (i, p) in list.iter().enumerate() {
        if list[..i].contains(p) {
            return Err(err(
                "manifest-shape",
                format!("{what} lists the part {p} twice"),
            ));
        }
    }
    Ok(list)
}

struct Section {
    array: bool,
    items: Vec<String>,
    files: Vec<usize>,
}

/// Assembles parts given as `(name shown in errors, text)` in manifest order.
///
/// # Errors
/// A part that does not parse, a nested manifest, a section two parts both hold (unless both hold it as
/// an array) and an id that two parts give one array section.
pub fn assemble(parts: &[(String, String)]) -> Result<String, SpecPartsError> {
    let mut order: Vec<String> = Vec::new();
    let mut sections: HashMap<String, Section> = HashMap::new();
    for (index, (name, text)) in parts.iter().enumerate() {
        let members = scan_top_level(text, name)?;
        let mut seen: Vec<&str> = Vec::new();
        for m in &members {
            if m.key == MANIFEST_KEY {
                return Err(err(
                    "nested-manifest",
                    format!("{name} is a manifest; a part holds sections, not a list of parts"),
                ));
            }
            if seen.contains(&m.key.as_str()) {
                return Err(err(
                    "duplicate-section",
                    format!("{name} names the section \"{}\" twice", m.key),
                ));
            }
            seen.push(&m.key);
            let array = m.items.is_some();
            if let Some(s) = sections.get(&m.key) {
                if !array || !s.array {
                    return Err(err(
                        "duplicate-section",
                        format!(
                            "the section \"{}\" is in both {} and {name}; only an array section may be in more than one part",
                            m.key, parts[s.files[0]].0
                        ),
                    ));
                }
            } else {
                order.push(m.key.clone());
                sections.insert(
                    m.key.clone(),
                    Section {
                        array,
                        items: Vec::new(),
                        files: Vec::new(),
                    },
                );
            }
            let s = sections.get_mut(&m.key).expect("inserted above");
            s.files.push(index);
            match &m.items {
                None => s.items.push(text[m.start..m.end].to_string()),
                Some(items) => {
                    for &(a, b) in items {
                        s.items.push(text[a..b].to_string());
                    }
                }
            }
        }
    }
    // An id that two parts give one section. Only sections in several parts are checked.
    for key in &order {
        let s = &sections[key];
        if !s.array || s.files.len() < 2 {
            continue;
        }
        let mut owner: HashMap<String, usize> = HashMap::new();
        for &index in &s.files {
            let (name, text) = &parts[index];
            let members = scan_top_level(text, name)?;
            let Some(m) = members.iter().find(|m| &m.key == key) else {
                continue;
            };
            for &(a, b) in m.items.as_deref().unwrap_or(&[]) {
                let id = serde_json::from_str::<serde_json::Value>(&plain_json(&text[a..b]))
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_str().map(str::to_string)));
                let Some(id) = id else { continue };
                if let Some(&first) = owner.get(&id) {
                    if first != index {
                        return Err(err(
                            "duplicate-id",
                            format!(
                                "the id \"{id}\" of section \"{key}\" is in both {} and {name}",
                                parts[first].0
                            ),
                        ));
                    }
                }
                owner.insert(id, index);
            }
        }
    }
    let mut out = Vec::new();
    for key in &order {
        let s = &sections[key];
        let quoted = serde_json::to_string(key).expect("a string serialises");
        if s.array {
            if s.items.is_empty() {
                out.push(format!("  {quoted}: []"));
            } else {
                let items: Vec<String> = s.items.iter().map(|t| format!("    {t}")).collect();
                out.push(format!("  {quoted}: [\n{}\n  ]", items.join(",\n")));
            }
        } else {
            out.push(format!("  {quoted}: {}", s.items[0]));
        }
    }
    Ok(format!("{{\n{}\n}}\n", out.join(",\n")))
}

/// Resolves a specification text read from `file`: a manifest is assembled from the parts beside it,
/// anything else is returned as it is.
///
/// # Errors
/// A manifest that is malformed, names a missing part, or whose parts conflict.
pub fn resolve(file: &Path, text: &str) -> Result<Resolved, SpecPartsError> {
    if !is_manifest(text) {
        return Ok(Resolved {
            text: text.to_string(),
            files: vec![file.to_path_buf()],
            manifest: false,
        });
    }
    let shown = file.display().to_string();
    let names = manifest_parts(text, &shown)?;
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    let mut files = vec![file.to_path_buf()];
    let mut parts = Vec::new();
    for name in &names {
        let path = dir.join(name);
        let body = std::fs::read_to_string(&path).map_err(|e| {
            if path.exists() {
                err("read", format!("cannot read {}: {e}", path.display()))
            } else {
                err(
                    "missing-part",
                    format!(
                        "{shown} lists the part {name}, which does not exist ({})",
                        path.display()
                    ),
                )
            }
        })?;
        files.push(path.clone());
        parts.push((path.display().to_string(), body));
    }
    Ok(Resolved {
        text: assemble(&parts)?,
        files,
        manifest: true,
    })
}

/// Reads the specification at `path`, resolving a manifest.
///
/// # Errors
/// A file that cannot be read, and what [`resolve`] refuses.
pub fn read_spec(path: &Path) -> Result<Resolved, SpecPartsError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| err("read", format!("cannot read {}: {e}", path.display())))?;
    resolve(path, &text)
}

/// The text of the specification at `path`: [`read_spec`] without the file list.
///
/// # Errors
/// As [`read_spec`].
pub fn read_spec_text(path: &Path) -> Result<String, SpecPartsError> {
    read_spec(path).map(|r| r.text)
}
