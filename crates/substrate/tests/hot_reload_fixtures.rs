//! The hot reload fixtures (WG-5345): the product family the hot reload scenarios run against.
//! Knowledge-base v1 and service-desk v1 are installed into one tenant, service-desk is upgraded
//! through v2 to v6, and after each step the tenant interface definition (its manifest and every
//! part, by hash) is compared with the committed files under
//! `web/test/fixtures/hot-reload/service-desk-v<n>/`, or rewritten when `STRATA_BLESS=1` is set.
//!
//! The bundles are the output of `specc bundle` for the product files under
//! `crates/spec-compiler/tests/fixtures/products/hot-reload/`; see
//! `web/test/fixtures/hot-reload/README.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value as Json};
use strata_kernel::*;
use strata_substrate::bundle::{
    install_into, install_product, upgrade_product, Bundle, ProductBundle, ProductInstall,
    ProductInstallError, ProductTarget,
};
use strata_substrate::ui::definition;

fn tenant() -> TenantId {
    TenantId::new("acme").unwrap()
}

fn admin() -> Principal {
    Principal::User(PrincipalId::new("user:admin").unwrap())
}

fn world() -> World {
    let doc = json!({
        "id": "acme",
        "name": "Acme",
        "owner": "user:admin",
        "created_at": "2026-01-01T00:00:00Z"
    });
    let bundle = Bundle::from_json(&doc.to_string()).unwrap();
    install_into(&World::default(), &bundle).unwrap_or_else(|e| panic!("{e}"))
}

fn bundle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../spec-compiler/tests/fixtures/products/expected/hot-reload")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/hot-reload")
}

fn read_bundle(name: &str) -> ProductBundle {
    let path = bundle_dir().join(format!("{name}.bundle.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; compile the product files with specc bundle (see web/test/fixtures/hot-reload/README.md)",
            path.display()
        )
    });
    ProductBundle::from_json(&text).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn schema_of(world: &World) -> TenantSchema {
    (*world.tenants[&tenant()].schema).clone()
}

type Changed = Result<ProductInstall, ProductInstallError<DomainError>>;

fn change(
    world: &mut World,
    run: impl FnOnce(
        &ProductTarget<'_>,
        &mut dyn FnMut(Context, Intent) -> Result<Output, DomainError>,
    ) -> Changed,
) -> Changed {
    let tenant = tenant();
    let schema = schema_of(world);
    let target = ProductTarget {
        tenant: &tenant,
        principal: admin(),
        now: 1_800_000_000_000,
        release: "0.2",
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let mut execute = |ctx: Context, intent: Intent| -> Result<Output, DomainError> {
        let (next, outcome) = world.step(&intent, &ctx)?;
        *world = next;
        Ok(outcome.output)
    };
    run(&target, &mut execute)
}

/// Pretty JSON with a trailing newline, the form of every golden file.
fn pretty(value: &Json) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

fn blessing() -> bool {
    std::env::var("STRATA_BLESS").is_ok_and(|v| v == "1")
}

/// Writes (`STRATA_BLESS=1`) or compares byte for byte the files of one step: the manifest and
/// every part, named by its hash.
fn golden(version: u32, world: &World) {
    let schema = schema_of(world);
    let defined = definition(&schema);
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    files.insert(
        "manifest.json".to_string(),
        pretty(&serde_json::to_value(&defined.manifest).unwrap()),
    );
    for (hash, part) in &defined.parts {
        files.insert(format!("parts/{hash}.json"), pretty(part));
    }
    let dir = golden_dir().join(format!("service-desk-v{version}"));
    if blessing() {
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        for (name, text) in &files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
        }
        return;
    }
    for (name, text) in &files {
        let path = dir.join(name);
        let held = fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; run the test with STRATA_BLESS=1 to write it",
                path.display()
            )
        });
        assert_eq!(
            &held,
            text,
            "{} differs; STRATA_BLESS=1 rewrites it",
            path.display()
        );
    }
    let mut on_disk = Vec::new();
    for entry in fs::read_dir(dir.join("parts")).unwrap() {
        on_disk.push(format!(
            "parts/{}",
            entry.unwrap().file_name().to_string_lossy()
        ));
    }
    for name in on_disk {
        assert!(
            files.contains_key(&name),
            "{}/{name} is not a part of service-desk v{version}; STRATA_BLESS=1 removes it",
            dir.display()
        );
    }
}

fn accepted(step: &str, result: Changed) {
    match result {
        Ok(ProductInstall::Applied { .. }) => {}
        other => panic!("{step} was not applied: {other:?}"),
    }
}

/// Verifies: INT-3252, INT-3253
///
/// Knowledge-base v1 and service-desk v1 are installed, service-desk is upgraded through v6, each
/// step is accepted by the kernel and gives the committed manifest and parts.
#[test]
fn the_hot_reload_family_installs_and_upgrades_to_its_golden_parts() {
    let mut world = world();

    let knowledge_base = read_bundle("knowledge-base");
    accepted(
        "install knowledge-base v1",
        change(&mut world, |t, e| install_product(&knowledge_base, t, e)),
    );
    let first = read_bundle("service-desk-v1");
    accepted(
        "install service-desk v1",
        change(&mut world, |t, e| install_product(&first, t, e)),
    );
    golden(1, &world);

    for version in 2..=6u32 {
        let bundle = read_bundle(&format!("service-desk-v{version}"));
        accepted(
            &format!("upgrade service-desk to v{version}"),
            change(&mut world, |t, e| upgrade_product(&bundle, t, e)),
        );
        golden(version, &world);
    }
}
