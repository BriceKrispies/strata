//! Pure interpreters over kernel data.
//!
//! The kernel decides; this crate reads and writes the forms people and programs use around
//! those decisions. Nothing here performs I/O.
//!
//! - [`syntax`]: the text form of kernel expressions.
//! - [`bundle`]: a whole tenant as one document, installed as ordinary kernel intents.
//! - [`plain`]: plain JSON to and from kernel values, directed by field types.
//! - [`project`]: view definitions interpreted into reads and view models.
//! - [`markdown`]: the constrained rich-text format, parsed to structured blocks.
//! - [`theme`]: the token schema tenant themes select from.
//! - [`subscribe`]: what a subscriber is told about an event.

pub mod bundle;
pub mod markdown;
pub mod plain;
pub mod project;
pub mod subscribe;
pub mod syntax;
pub mod theme;
