//! Records the files `tests/recorded_data.rs` reads, under `tests/recorded/`.
//!
//! The rule, repeated from that test: the files were recorded with the hand-written kernel types
//! (the commit before the kernel adopted the generated ones) and must not be re-recorded from the
//! generated types, which would compare the generated types with themselves. Run this only to
//! add a fixture or after an intended change to the stored form, and review the diff.
//! `cargo run -p strata-storage-sqlite --example record_data`

#[path = "../tests/recorded_support/mod.rs"]
mod recorded_support;

use recorded_support::{dir, files, notes, publication};

fn main() {
    std::fs::create_dir_all(dir()).expect("make the recording directory");
    for (prefix, rec) in [("notes", notes()), ("publication", publication())] {
        for (name, text) in files(prefix, &rec) {
            std::fs::write(dir().join(&name), text).expect("write a recorded file");
            println!("wrote {name}");
        }
    }
}
