//! The enterprise tenant corpus: the 14 product files of `tests/corpus/enterprise` installed into
//! one tenant on any [`Store`], and the tenant filled with seeded records.
//!
//! ```ignore
//! let store = strata_storage_api::memory::MemoryStore::new();
//! let built = strata_corpus::load_and_populate(&store, Scale::Small, 7)?;
//! let tenant = built.catalogue.tenant;          // "enterprise"
//! let articles = built.catalogue.ids("content__article");
//! let hot_products = built.catalogue.hot("commerce__product");
//! ```
//!
//! - [`load_corpus`] installs the products (the platform's install path, see [`load`]).
//! - [`populate`] creates the records (see [`populate`](mod@populate)).
//! - [`load_and_populate`] does both with the default tenant and returns a [`Built`]: the tenant
//!   id, the [`Catalogue`] of what was created (the ids of each object type, its hot set, its
//!   large records, the hot people) and the hash of the populated tenant.
//! - [`tenant_hash`] is the hash a determinism check compares.

pub mod catalogue;
pub mod load;
pub mod populate;
pub mod rng;
pub mod scale;
pub mod text;

pub use catalogue::{Catalogue, HotPerson, RefEdge, TypeEntry};
pub use load::{
    check_corpus, compile_corpus, install_product_into, load_corpus, tenant_hash, verify_loaded,
    workspace_root, CheckReport, CompiledProduct, CorpusPaths, LoadOptions, LoadedCorpus,
    ProductReport, Verification,
};
pub use populate::{planned_counts, populate, populate_with, PopulateOptions, Progress};
pub use scale::Scale;

use std::fmt;
use strata_storage_api::Store;

/// Why the corpus could not be loaded or populated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorpusError(pub String);

impl CorpusError {
    pub fn new(message: impl Into<String>) -> Self {
        CorpusError(message.into())
    }

    pub fn from_display(error: impl fmt::Display) -> Self {
        CorpusError(error.to_string())
    }
}

impl fmt::Display for CorpusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CorpusError {}

/// A loaded and populated tenant.
#[derive(Clone, Debug)]
pub struct Built {
    pub loaded: LoadedCorpus,
    pub catalogue: Catalogue,
    /// The hash of the populated tenant's state.
    pub hash: String,
    pub verification: Verification,
}

impl Built {
    pub fn tenant(&self) -> &str {
        self.loaded.tenant.as_str()
    }
}

/// Load the corpus and populate the tenant, in one call, with the default tenant (`enterprise`,
/// owned by `user:owner`) and the corpus of this checkout.
pub fn load_and_populate(store: &dyn Store, scale: Scale, seed: u64) -> Result<Built, CorpusError> {
    let products = compile_corpus(&CorpusPaths::default())?;
    build(
        store,
        &products,
        &LoadOptions::default(),
        &PopulateOptions::new(scale, seed),
    )
}

/// Load already compiled products and populate the tenant. Compile once with [`compile_corpus`]
/// to fill several stores.
pub fn build(
    store: &dyn Store,
    products: &[CompiledProduct],
    load: &LoadOptions,
    options: &PopulateOptions,
) -> Result<Built, CorpusError> {
    build_with(store, products, load, options, &mut |_| {})
}

/// [`build`] with progress reports.
pub fn build_with(
    store: &dyn Store,
    products: &[CompiledProduct],
    load: &LoadOptions,
    options: &PopulateOptions,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Built, CorpusError> {
    let loaded = load_corpus(store, load, products)?;
    let verification = verify_loaded(store, &loaded.tenant, products)?;
    if !verification.missing.is_empty() {
        return Err(CorpusError::new(format!(
            "the tenant lacks {} entities its products own, for example {}",
            verification.missing.len(),
            verification.missing[..verification.missing.len().min(5)].join(", ")
        )));
    }
    let catalogue = populate_with(store, &loaded, options, progress)?;
    let hash = tenant_hash(store, &loaded.tenant)?;
    Ok(Built {
        loaded,
        catalogue,
        hash,
        verification,
    })
}
