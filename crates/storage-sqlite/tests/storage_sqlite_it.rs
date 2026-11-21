//! The routine tests of strata-storage-sqlite, gathered into one test binary: each module below is one of
//! the package's test files, unchanged.

#[path = "sqlite.rs"]
mod sqlite;

#[path = "bounded_reads.rs"]
mod bounded_reads;
