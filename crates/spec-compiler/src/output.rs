//! Writing the generated crate to a directory and comparing a directory with it.
//!
//! specc owns only the files that start with its header ([`HEADER_MARKERS`]): writing replaces
//! those it generates and removes those it no longer generates, and leaves every other file
//! alone. Comparison sees the directory the same way. Line endings are compared without `\r`,
//! so a checkout with CRLF line endings is not reported as different.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::diagnostic::Diagnostic;
use crate::emit::{GeneratedFile, HEADER_MARKERS};

/// Code of two generated files that one path names, or of a path a Windows or case-insensitive
/// file system cannot keep apart or cannot hold.
pub const PATH_COLLISION: &str = "specc.generated-path-collision";

/// Windows device names, which no file may be named, with or without an extension.
const DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "conin$", "conout$", "com1", "com2", "com3", "com4", "com5",
    "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8",
    "lpt9",
];

/// True when `name` (one path component) is a Windows device name, as `con` and `con.spec.js`.
#[must_use]
pub fn is_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let stem = stem.to_lowercase();
    DEVICE_NAMES.contains(&stem.as_str())
}

/// Every reason the paths of `files` cannot all be written side by side: two files with one
/// path, two paths that differ only in letter case (one file system keeps them apart, another
/// does not), and a component that is a Windows device name or ends in a dot or space.
#[must_use]
pub fn path_collisions(files: &[GeneratedFile]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut seen: std::collections::BTreeMap<String, &str> = std::collections::BTreeMap::new();
    for file in files {
        let folded = file.path.to_lowercase();
        match seen.get(&folded) {
            Some(first) if *first == file.path => out.push(Diagnostic::new(
                PATH_COLLISION,
                &file.path,
                format!("Two generated files have the path \"{}\"; one would overwrite the other. Rename the entity that names one of them.", file.path),
            )),
            Some(first) => out.push(Diagnostic::new(
                PATH_COLLISION,
                &file.path,
                format!("The generated files \"{first}\" and \"{}\" differ only in letter case, so on a case-insensitive file system one would overwrite the other. Rename one entity.", file.path),
            )),
            None => {
                seen.insert(folded, &file.path);
            }
        }
        for component in file.path.split('/') {
            if is_device_name(component) {
                out.push(Diagnostic::new(
                    PATH_COLLISION,
                    &file.path,
                    format!("The generated file \"{}\" would be named \"{component}\", a device name on Windows. Rename the entity that names it.", file.path),
                ));
            } else if component.ends_with('.') || component.ends_with(' ') {
                out.push(Diagnostic::new(
                    PATH_COLLISION,
                    &file.path,
                    format!("The generated file \"{}\" has a name ending in a dot or a space, which Windows drops.", file.path),
                ));
            }
        }
    }
    out
}

/// Directories never searched for generated files.
const SKIPPED_DIRS: &[&str] = &["target", ".git", "node_modules"];

/// How a directory differs from the generated crate. Paths are relative, with forward slashes,
/// sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Differences {
    /// Generated files whose contents on disk differ.
    pub differing: Vec<String>,
    /// Generated files that are not on disk.
    pub missing: Vec<String>,
    /// Files on disk with specc's header that specc would not generate.
    pub extra: Vec<String>,
}

impl Differences {
    /// True when the directory is exactly what generation would leave.
    pub fn is_empty(&self) -> bool {
        self.differing.is_empty() && self.missing.is_empty() && self.extra.is_empty()
    }
}

/// Writes the files into `dir`, creating directories as needed and leaving files whose
/// contents are already right untouched, then removes stale generated files. Returns the
/// removed paths.
pub fn write(dir: &Path, files: &[GeneratedFile]) -> io::Result<Vec<String>> {
    for file in files {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let current = fs::read_to_string(&path).ok();
        if current.as_deref() != Some(file.contents.as_str()) {
            fs::write(&path, &file.contents)?;
        }
    }
    let mut removed = Vec::new();
    for stale in stale_files(dir, files)? {
        fs::remove_file(dir.join(&stale))?;
        removed.push(stale);
    }
    Ok(removed)
}

/// Compares `dir` with the files generation would write.
pub fn compare(dir: &Path, files: &[GeneratedFile]) -> io::Result<Differences> {
    let mut out = Differences::default();
    for file in files {
        match fs::read_to_string(dir.join(&file.path)) {
            Ok(on_disk) => {
                if on_disk.replace('\r', "") != file.contents.replace('\r', "") {
                    out.differing.push(file.path.clone());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => out.missing.push(file.path.clone()),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                out.differing.push(file.path.clone());
            }
            Err(e) => return Err(e),
        }
    }
    out.extra = stale_files(dir, files)?;
    out.differing.sort();
    out.missing.sort();
    Ok(out)
}

/// Files under `dir` that carry specc's header but are not among `files`.
fn stale_files(dir: &Path, files: &[GeneratedFile]) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    if dir.is_dir() {
        collect_generated(dir, "", &mut found)?;
    }
    found.retain(|path| !files.iter().any(|f| f.path == *path));
    found.sort();
    Ok(found)
}

fn collect_generated(dir: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                collect_generated(&entry.path(), &relative, out)?;
            }
        } else if kind.is_file() && has_header(&entry.path())? {
            out.push(relative);
        }
    }
    Ok(())
}

/// True when the file starts with one of specc's header markers.
pub fn has_header(path: &Path) -> io::Result<bool> {
    let mut start = Vec::new();
    fs::File::open(path)?.take(64).read_to_end(&mut start)?;
    Ok(HEADER_MARKERS
        .iter()
        .any(|marker| start.starts_with(marker.as_bytes())))
}
