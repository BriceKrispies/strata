//! Pure interpreters over kernel data.
//!
//! The kernel decides; this crate reads and writes the forms people and programs use around
//! those decisions. Nothing here performs I/O. The text form of kernel expressions is the
//! generated rule language's: `strata_kernel::expr::parse` reads it and `Display` of an
//! expression prints it.
//!
//! - [`bundle`]: a whole tenant as one document, installed as ordinary kernel intents.
//! - [`product`]: the bundle of a product, read, installed, upgraded and removed as one schema change.
//! - [`plain`]: plain JSON to and from kernel values, directed by field types.
//! - [`project`]: view definitions interpreted into reads and view models.
//! - [`markdown`]: the constrained rich-text format, parsed to structured blocks.
//! - [`proposal`]: proposed schema changes written as text.
//! - [`theme`]: the token schema tenant themes select from.
//! - [`subscribe`]: what a subscriber is told about an event.
//! - [`ui`]: the interface definition of a tenant, in parts named by their hashes.

pub mod bundle;
pub mod markdown;
pub mod plain;
pub mod product;
pub mod project;
pub mod proposal;
pub mod staged;
pub mod stages;
pub mod subscribe;
pub mod theme;
pub mod ui;
pub mod view_needs;
