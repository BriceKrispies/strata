//! The routine tests of strata-substrate, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "product_bundle.rs"]
mod product_bundle;

#[path = "staged_upgrade.rs"]
mod staged_upgrade;

#[path = "publication_template.rs"]
mod publication_template;

#[path = "ui_definition.rs"]
mod ui_definition;

#[path = "hot_reload_fixtures.rs"]
mod hot_reload_fixtures;

#[path = "thread_descendants.rs"]
mod thread_descendants;

#[path = "visibility.rs"]
mod visibility;
