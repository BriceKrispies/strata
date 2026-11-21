//! Installing the 14 product files into one tenant, through the platform's own install path.
//!
//! Each product file is turned into a tenant bundle by specc (`strata_spec_compiler::bundle`, the
//! code behind `specc bundle`: the layer checks first, then the bundle rules, then the check of
//! the finished bundle against `tenant-bundle`). The tenant is created by the whole-tenant install
//! (`strata_substrate::bundle::install`, which the backend's seed runs), and each product is
//! planned and installed by `plan_install` and `run_planned`, which the backend's install-bundle
//! route runs: one `change-schema` intent per product, committed through `execute` with the
//! ownership record of the product in the same commit. The segments of a product go in through
//! the segment store after its schema change, as the route does.
//!
//! What this does not go through is the HTTP layer of the route: no session, no step-up and no
//! audit entry, since there is no signed-in person. The decisions are the kernel's, the same ones.

use crate::CorpusError;
use serde_json::json;
use std::path::{Path, PathBuf};
use strata_kernel::segments::{derive_segment_id, SegmentKey};
use strata_kernel::DomainErrorExt;
use strata_kernel::{Context, Intent, Output, Principal, PrincipalId, TenantId, Timestamp};
use strata_spec_compiler::layers::{prepare_platform, ProductFile};
use strata_storage_api::segments as kept;
use strata_storage_api::{execute, Store};
use strata_substrate::bundle::{
    install, plan_install, run_planned, Bundle, InstalledRecord, Planned, ProductBundle,
    ProductInstall, ProductTarget,
};

/// The platform release this loader installs against, which a bundle's platform range is checked
/// against. It is the value of `strata_backend::products::PLATFORM_RELEASE` (the specification
/// version the backend is built from); the loader does not depend on the backend crate for it.
pub const PLATFORM_RELEASE: &str = "0.2-draft";

/// The root of the checkout this crate is in.
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Where the specification and the corpus are.
#[derive(Clone, Debug)]
pub struct CorpusPaths {
    pub spec: PathBuf,
    pub dir: PathBuf,
}

impl Default for CorpusPaths {
    fn default() -> Self {
        let root = workspace_root();
        CorpusPaths {
            spec: root.join("specifications/spec.jsonc"),
            dir: root.join("tests/corpus/enterprise"),
        }
    }
}

/// One product file turned into a tenant bundle.
#[derive(Clone, Debug)]
pub struct CompiledProduct {
    pub id: String,
    /// The file name, as in the corpus directory.
    pub file: String,
    /// The bundle, as specc writes it (`<id>.bundle.json`).
    pub bundle: String,
}

/// Compile every `*.jsonc` product file of the corpus directory to a tenant bundle, in file-name
/// order. The specification is read and parsed once for all of them. Any diagnostic specc gives
/// refuses the whole corpus: nothing is skipped.
pub fn compile_corpus(paths: &CorpusPaths) -> Result<Vec<CompiledProduct>, CorpusError> {
    let spec_text = strata_spec_parts::read_spec_text(&paths.spec)
        .map_err(|e| CorpusError::new(format!("{}: {e}", paths.spec.display())))?;
    let spec_path = paths.spec.to_string_lossy().replace('\\', "/");
    let spec_file = std::path::absolute(&paths.spec).ok();
    let prepared = prepare_platform(&spec_path, &spec_text);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&paths.dir)
        .map_err(|e| CorpusError::new(format!("{}: {e}", paths.dir.display())))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonc"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(CorpusError::new(format!(
            "{} holds no product file",
            paths.dir.display()
        )));
    }

    let mut out = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CorpusError::new(format!("{}: {e}", path.display())))?;
        let product = ProductFile {
            path: path.to_string_lossy().replace('\\', "/"),
            text,
            file: std::path::absolute(&path).ok(),
        };
        let built = strata_spec_compiler::bundle::generate_prepared(
            &spec_path,
            &prepared,
            spec_file.as_deref(),
            &product,
        )
        .map_err(|diagnostics| {
            let lines: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
            CorpusError::new(format!(
                "specc refused {name} with {} diagnostic(s):\n{}",
                lines.len(),
                lines.join("\n")
            ))
        })?;
        let bundle = built
            .files
            .iter()
            .find(|file| file.path.ends_with(".bundle.json"))
            .ok_or_else(|| CorpusError::new(format!("specc wrote no bundle for {name}")))?
            .contents
            .clone();
        out.push(CompiledProduct {
            id: built.product,
            file: name,
            bundle,
        });
    }
    Ok(out)
}

/// How the tenant is created.
#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub tenant: String,
    pub name: String,
    pub owner: String,
    /// When the tenant is created, as a UTC timestamp. Each product installs one millisecond
    /// after the one before.
    pub created_at: String,
    /// Remove a tenant of this id that exists before creating it.
    pub replace: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            tenant: "enterprise".to_string(),
            name: "Enterprise".to_string(),
            owner: "user:owner".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            replace: false,
        }
    }
}

/// What one product installed.
#[derive(Clone, Debug)]
pub struct ProductReport {
    pub product: String,
    pub version: String,
    /// The entities the ownership record names.
    pub entities: usize,
    pub segments: usize,
    pub schema_version: u64,
}

/// The tenant after the corpus is installed.
#[derive(Clone, Debug)]
pub struct LoadedCorpus {
    pub tenant: TenantId,
    pub owner: PrincipalId,
    pub products: Vec<ProductReport>,
    pub schema_version: u64,
    /// The time of the last install commit, in milliseconds. A population starts after it.
    pub clock: i64,
    /// Commits the install made.
    pub commits: u64,
}

impl LoadedCorpus {
    pub fn entities(&self) -> usize {
        self.products.iter().map(|p| p.entities).sum()
    }
}

fn context(tenant: &TenantId, principal: Principal, at: i64, correlation: String) -> Context {
    Context {
        tenant: tenant.clone(),
        principal,
        session: None,
        now: Timestamp(at),
        correlation,
        verified_address: None,
        idempotency_key: None,
        language: None,
        hop: None,
    }
}

/// Run one intent through the store's `execute`: the whole of production command handling.
pub(crate) fn run(store: &dyn Store, ctx: &Context, intent: &Intent) -> Result<Output, String> {
    execute(store, intent, ctx)
        .map(|done| done.transition.output)
        .map_err(|e| e.to_string())
}

/// Install the compiled products into a new tenant on `store`. A step that fails removes what
/// was installed, so the tenant is whole or absent.
pub fn load_corpus(
    store: &dyn Store,
    options: &LoadOptions,
    products: &[CompiledProduct],
) -> Result<LoadedCorpus, CorpusError> {
    let tenant = TenantId::new(options.tenant.clone())
        .map_err(|e| CorpusError::new(format!("tenant id {:?}: {e}", options.tenant)))?;
    let owner = PrincipalId::new(options.owner.clone())
        .map_err(|e| CorpusError::new(format!("owner {:?}: {e}", options.owner)))?;
    if options.replace {
        store
            .reset_tenant(&tenant)
            .map_err(CorpusError::from_display)?;
    }
    let mut begun = false;
    let result = install_all(store, options, &tenant, &owner, products, &mut begun);
    if result.is_err() && begun {
        // Only a tenant this call created is removed.
        let _ = store.reset_tenant(&tenant);
    }
    result
}

fn install_all(
    store: &dyn Store,
    options: &LoadOptions,
    tenant: &TenantId,
    owner: &PrincipalId,
    products: &[CompiledProduct],
    begun: &mut bool,
) -> Result<LoadedCorpus, CorpusError> {
    // The tenant itself, by the whole-tenant install the backend's seed uses: it creates the
    // tenant and nothing else, since this bundle declares nothing.
    let bundle = Bundle::from_json(
        &json!({
            "id": options.tenant,
            "name": options.name,
            "owner": options.owner,
            "created_at": options.created_at,
        })
        .to_string(),
    )
    .map_err(CorpusError::from_display)?;
    let mut commits = 0u64;
    let mut clock =
        strata_substrate::plain::parse_time(&options.created_at).map_err(CorpusError::new)?;
    install::<String>(&bundle, &mut |ctx, intent| {
        let creates = matches!(intent, Intent::CreateTenant { .. });
        let output = run(store, &ctx, &intent)?;
        commits += 1;
        if creates {
            *begun = true;
            store
                .begin_provisioning(&ctx.tenant)
                .map_err(|e| e.to_string())?;
        }
        Ok(output)
    })
    .map_err(|e| CorpusError::new(format!("creating the tenant: {e}")))?;

    let mut reports = Vec::new();
    let mut schema_version = 0;
    for product in products {
        clock += 1;
        let (report, made) =
            install_product_into(store, tenant, owner, clock, product).map_err(|problems| {
                CorpusError::new(format!(
                    "installing {}: {} problem(s):\n{}",
                    product.file,
                    problems.len(),
                    problems.join("\n")
                ))
            })?;
        commits += made;
        schema_version = report.schema_version;
        reports.push(report);
    }
    store
        .finish_provisioning(tenant)
        .map_err(CorpusError::from_display)?;
    Ok(LoadedCorpus {
        tenant: tenant.clone(),
        owner: owner.clone(),
        products: reports,
        schema_version,
        clock,
        commits,
    })
}

/// The failing checks a kernel rejection carries: every problem of the schema it found, not only
/// the first, which is the rejection's message.
fn rejection_problems(error: &strata_storage_api::ExecuteError) -> Vec<String> {
    match error {
        strata_storage_api::ExecuteError::Domain(e) => {
            let mut out: Vec<String> = e
                .evidence
                .iter()
                .filter(|check| !check.passed)
                .map(|check| format!("{}: {}", check.rule, check.detail))
                .collect();
            if out.is_empty() {
                out.push(e.describe());
            }
            out
        }
        other => vec![other.to_string()],
    }
}

/// Install one compiled product into an existing tenant: plan it, run its one `change-schema`
/// intent, define its segments. A refusal returns every problem the kernel reports (the failing
/// checks of the rejection), not the first. On success: what it installed, and the commits made.
pub fn install_product_into(
    store: &dyn Store,
    tenant: &TenantId,
    owner: &PrincipalId,
    clock: i64,
    product: &CompiledProduct,
) -> Result<(ProductReport, u64), Vec<String>> {
    let one = |message: String| vec![message];
    let state = store.load(tenant, &[]).map_err(|e| one(e.to_string()))?;
    let held = state
        .tenant
        .ok_or_else(|| one("the tenant does not exist".to_string()))?;
    let schema = held.schema.clone();
    let target = ProductTarget {
        tenant,
        principal: Principal::User(owner.clone()),
        now: clock,
        release: PLATFORM_RELEASE,
        schema: &schema,
        installed: None,
        expected_version: None,
        idempotency_key: None,
    };
    let parsed = ProductBundle::from_json(&product.bundle).map_err(|e| one(e.to_string()))?;
    let planned = plan_install(&parsed, &target).map_err(|e| one(e.to_string()))?;
    let change = match &planned {
        Planned::Change(change) => Some(change.clone()),
        Planned::Unchanged { .. } => None,
    };
    let mut commits = 0u64;
    let mut refused: Vec<String> = Vec::new();
    let applied = run_planned(planned, &target, &mut |ctx, intent| {
        commits += 1;
        match execute(store, &intent, &ctx) {
            Ok(done) => Ok(done.transition.output),
            Err(error) => {
                refused = rejection_problems(&error);
                Err(error.to_string())
            }
        }
    });
    let (version, record) = match applied {
        Ok(ProductInstall::Applied {
            schema_version,
            record,
        }) => (schema_version, record),
        Ok(ProductInstall::Unchanged { schema_version }) => (schema_version, None),
        Err(error) => {
            return Err(if refused.is_empty() {
                one(error.to_string())
            } else {
                refused
            })
        }
    };

    // The segments of the product, through the segment store after the schema change.
    let mut segments = 0;
    if let Some(change) = &change {
        let ctx = context(
            tenant,
            Principal::User(owner.clone()),
            clock,
            format!("corpus:segments:{}", product.id),
        );
        let definitions = change
            .segments
            .definitions(tenant)
            .map_err(|e| one(e.to_string()))?;
        let mut problems = Vec::new();
        for definition in definitions {
            match kept::define(store, &ctx, &definition) {
                Ok(_) => {
                    commits += 1;
                    segments += 1;
                }
                Err(e) => problems.extend(
                    rejection_problems(&e)
                        .into_iter()
                        .map(|p| format!("segment {}: {p}", definition.name)),
                ),
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
    }
    let owns = record.as_ref().map_or(0, |r| r.owns.len());
    Ok((
        ProductReport {
            product: product.id.clone(),
            version: parsed.version.clone(),
            entities: owns,
            segments,
            schema_version: version,
        },
        commits,
    ))
}

/// What `check` found for one product.
#[derive(Clone, Debug)]
pub struct CheckReport {
    pub product: String,
    pub file: String,
    /// `alone` (a fresh tenant with this product only) or `together` (the products in one
    /// tenant, in file order, each installed after those that passed).
    pub pass: &'static str,
    /// Every problem the kernel reports; empty when the product installs.
    pub problems: Vec<String>,
}

fn fresh_tenant(
    store: &dyn Store,
    options: &LoadOptions,
) -> Result<(TenantId, PrincipalId, i64), CorpusError> {
    let tenant = TenantId::new(options.tenant.clone()).map_err(CorpusError::from_display)?;
    let owner = PrincipalId::new(options.owner.clone()).map_err(CorpusError::from_display)?;
    let bundle = Bundle::from_json(
        &json!({
            "id": options.tenant, "name": options.name,
            "owner": options.owner, "created_at": options.created_at,
        })
        .to_string(),
    )
    .map_err(CorpusError::from_display)?;
    let clock =
        strata_substrate::plain::parse_time(&options.created_at).map_err(CorpusError::new)?;
    install::<String>(&bundle, &mut |ctx, intent| run(store, &ctx, &intent))
        .map_err(|e| CorpusError::new(format!("creating the tenant: {e}")))?;
    Ok((tenant, owner, clock))
}

/// Install every product, each on a fresh in-memory tenant of its own, and report every problem
/// the kernel finds in each instead of stopping at the first. The products declare only their
/// own entities and may not name each other, so none needs another installed first. Then install
/// them all into one tenant, in file order, to find what only collides across products (a route
/// two screens answer, an id both declare); a product that fails there is skipped and reported,
/// and the rest carry on.
pub fn check_corpus(
    products: &[CompiledProduct],
    options: &LoadOptions,
    report: &mut dyn FnMut(&CheckReport),
) -> Result<Vec<CheckReport>, CorpusError> {
    let mut out = Vec::new();
    for product in products {
        let store = strata_storage_api::memory::MemoryStore::new();
        let (tenant, owner, clock) = fresh_tenant(&store, options)?;
        let problems = match install_product_into(&store, &tenant, &owner, clock + 1, product) {
            Ok(_) => Vec::new(),
            Err(problems) => problems,
        };
        let done = CheckReport {
            product: product.id.clone(),
            file: product.file.clone(),
            pass: "alone",
            problems,
        };
        report(&done);
        out.push(done);
    }
    let store = strata_storage_api::memory::MemoryStore::new();
    let (tenant, owner, mut clock) = fresh_tenant(&store, options)?;
    for product in products {
        clock += 1;
        let problems = match install_product_into(&store, &tenant, &owner, clock, product) {
            Ok(_) => Vec::new(),
            Err(problems) => problems,
        };
        let done = CheckReport {
            product: product.id.clone(),
            file: product.file.clone(),
            pass: "together",
            problems,
        };
        report(&done);
        out.push(done);
    }
    Ok(out)
}

/// The entities the tenant's schema (and its segment store) holds against what the ownership
/// records say the products own.
#[derive(Clone, Debug, Default)]
pub struct Verification {
    /// Entities the ownership records name.
    pub owned: usize,
    /// The owned entities by kind.
    pub by_kind: std::collections::BTreeMap<String, usize>,
    /// Owned entities the kernel cannot look up in the schema (limits of a pool or an account).
    pub unchecked: usize,
    /// Owned entities the tenant does not hold, as `kind:id`.
    pub missing: Vec<String>,
}

/// Check that the tenant holds every entity every installed product owns.
pub fn verify_loaded(
    store: &dyn Store,
    tenant: &TenantId,
    products: &[CompiledProduct],
) -> Result<Verification, CorpusError> {
    let state = store.load(tenant, &[]).map_err(CorpusError::from_display)?;
    let held = state
        .tenant
        .ok_or_else(|| CorpusError::new("the tenant does not exist"))?;
    let schema = &held.schema;
    let catalogue = store
        .segment_catalog(tenant)
        .map_err(CorpusError::from_display)?;
    let mut out = Verification::default();
    for product in products {
        let record = InstalledRecord::of(schema, &product.id)
            .ok_or_else(|| CorpusError::new(format!("{} is not installed", product.id)))?;
        for owned in &record.owns {
            out.owned += 1;
            *out.by_kind.entry(owned.kind.clone()).or_default() += 1;
            let present = match owned.entity_ref() {
                Ok(entity) => match strata_kernel::products::exists(schema, &entity) {
                    Some(found) => found,
                    // A segment: the schema does not hold it, the segment store does.
                    None if owned.kind == "segment" => {
                        let id = derive_segment_id(tenant, &SegmentKey(owned.id.clone()));
                        catalogue.iter().any(|segment| segment.id == id)
                    }
                    // The limit of one account or pool lives in the ledger's own storage,
                    // which the schema cannot see (`products::exists` answers none): not
                    // checkable here, so it is counted, not missed.
                    None => {
                        out.unchecked += 1;
                        true
                    }
                },
                Err(_) => false,
            };
            if !present {
                out.missing.push(format!("{}:{}", owned.kind, owned.id));
            }
        }
    }
    Ok(out)
}

/// The hash of the tenant's whole state (schema, members, records, counters), as the kernel's
/// canonical encoding gives it. It is the same value on every store for the same commits.
pub fn tenant_hash(store: &dyn Store, tenant: &TenantId) -> Result<String, CorpusError> {
    let state = store
        .load_complete(tenant)
        .map_err(CorpusError::from_display)?;
    Ok(strata_kernel::canon::hash_of(&state.tenant))
}

/// The directory a relative path of the CLI is resolved from.
pub fn resolve(path: &str) -> PathBuf {
    let given = Path::new(path);
    if given.is_absolute() || given.exists() {
        given.to_path_buf()
    } else {
        workspace_root().join(given)
    }
}
