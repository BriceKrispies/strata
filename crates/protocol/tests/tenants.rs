//! A tenant bundle reads only files inside its own directory (and, for a template, the shared
//! template directory beside the tenants).

use std::fs;
use std::path::PathBuf;
use strata_protocol::tenants::load_tenant;

/// A fresh tenants root with a tenant directory `alpha`, a shared `_templates` directory, a secret
/// file outside both, and the given `tenant.json`.
fn root(name: &str, tenant_json: &str) -> (PathBuf, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tenants-{name}"));
    let _ = fs::remove_dir_all(&root);
    let alpha = root.join("alpha");
    fs::create_dir_all(alpha.join("posts")).unwrap();
    fs::create_dir_all(root.join("_templates")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(root.join("_templates").join("t.json"), "{}").unwrap();
    fs::write(root.join("other").join("secret.json"), "{}").unwrap();
    fs::write(root.join("secret.txt"), "secret").unwrap();
    fs::write(alpha.join("inside.md"), "inside").unwrap();
    fs::write(alpha.join("tenant.json"), tenant_json).unwrap();
    (root, alpha)
}

fn error_of(name: &str, tenant_json: &str) -> String {
    let (_root, alpha) = root(name, tenant_json);
    load_tenant(&alpha)
        .expect_err("the bundle was loaded")
        .to_string()
}

#[test]
fn a_template_outside_the_tenant_and_the_shared_directory_is_refused() {
    for path in [
        "../other/secret.json",
        "../../secret.txt",
        "../alpha/../other/secret.json",
    ] {
        let text = error_of("template-escape", &format!(r#"{{"template": "{path}"}}"#));
        assert!(
            text.contains("outside the tenant directory"),
            "{path}: {text}"
        );
    }
}

#[test]
fn an_absolute_template_path_is_refused() {
    let (root, alpha) = root("template-absolute", "{}");
    let secret = root.join("other").join("secret.json");
    let json = serde_json::json!({ "template": secret.to_string_lossy() }).to_string();
    fs::write(alpha.join("tenant.json"), json).unwrap();
    let text = load_tenant(&alpha).unwrap_err();
    assert!(text.contains("outside the tenant directory"), "{text}");
}

#[test]
fn a_shared_template_and_a_file_inside_the_tenant_are_not_refused_as_outside() {
    let (_root, alpha) = root(
        "inside",
        r#"{"template": "../_templates/t.json",
            "records": [{"fields": {"body": {"$file": "inside.md"}}}]}"#,
    );
    if let Err(text) = load_tenant(&alpha) {
        assert!(
            !text.contains("outside the tenant directory"),
            "a path that is allowed was refused: {text}"
        );
    }
}

#[test]
fn a_file_field_that_leaves_the_tenant_directory_is_refused() {
    for path in [
        "../secret.txt",
        "../_templates/t.json",
        "posts/../../secret.txt",
    ] {
        let text = error_of(
            "file-escape",
            &format!(r#"{{"records": [{{"fields": {{"body": {{"$file": "{path}"}}}}}}]}}"#),
        );
        assert!(
            text.contains("outside the tenant directory"),
            "{path}: {text}"
        );
    }
}
