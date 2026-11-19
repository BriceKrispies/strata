//! Writing the generated crate to a directory and comparing a directory with it.
//!
//! specc owns only the files that start with its header ([`HEADER_MARKERS`]): writing replaces
//! those it generates and removes those it no longer generates, and leaves every other file
//! alone. Comparison sees the directory the same way. Line endings are compared without `\r`,
//! so a checkout with CRLF line endings is not reported as different.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::emit::{GeneratedFile, HEADER_MARKERS};

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
