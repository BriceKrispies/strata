//! The manifest rule (WG-5386): array sections concatenate in manifest order, a section that is not
//! an array is in one part only, an id may not repeat across parts, and every refusal names the files.

use std::path::{Path, PathBuf};

use strata_spec_parts::{read_spec, MANIFEST_KEY};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
        .join("spec.jsonc")
}

#[test]
fn parts_assemble_in_manifest_order() {
    let resolved = read_spec(&fixture("ok")).unwrap();
    assert!(resolved.manifest);
    assert_eq!(resolved.files.len(), 3);
    let expected = "{\n  \"system\": { \"id\": \"s\" },\n  \"claims\": [\n    {\"id\":\"m-1\",\"kind\":\"invariant\"},\n    {\"id\":\"r-1\",\"kind\":\"requirement\"},\n    {\"id\":\"r-2\",\"kind\":\"requirement\"}\n  ],\n  \"goals\": []\n}\n";
    assert_eq!(resolved.text, expected);
}

#[test]
fn a_missing_part_is_refused() {
    let e = read_spec(&fixture("missing-part")).unwrap_err();
    assert_eq!(e.code, "missing-part");
    assert!(e.message.contains("gone.jsonc"), "{e}");
}

#[test]
fn an_object_section_in_two_parts_is_refused_naming_both() {
    let e = read_spec(&fixture("object-twice")).unwrap_err();
    assert_eq!(e.code, "duplicate-section");
    assert!(
        e.message.contains("a.jsonc") && e.message.contains("b.jsonc"),
        "{e}"
    );
}

#[test]
fn a_duplicate_id_across_parts_is_refused_naming_both() {
    let e = read_spec(&fixture("duplicate-id")).unwrap_err();
    assert_eq!(e.code, "duplicate-id");
    assert!(
        e.message.contains("c-1") && e.message.contains("a.jsonc") && e.message.contains("b.jsonc"),
        "{e}"
    );
}

#[test]
fn a_file_that_is_no_manifest_is_returned_as_it_is() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ok/a.jsonc");
    let resolved = read_spec(&path).unwrap();
    assert!(!resolved.manifest);
    assert_eq!(resolved.text, std::fs::read_to_string(&path).unwrap());
    assert_eq!(MANIFEST_KEY, "specParts");
}
