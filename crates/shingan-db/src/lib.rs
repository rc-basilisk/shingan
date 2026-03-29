//! # shingan-db
//!
//! SQLite persistence layer for shingan scan results.
//!
//! This crate stores scan sessions, duplicate groups, and individual file entries
//! in a local SQLite database. The default database location is
//! `$XDG_DATA_HOME/shingan/shingan.db` (typically `~/.local/share/shingan/shingan.db`).
//!
//! The [`Database`] handle is both `Send` and `Sync`, achieved through interior
//! mutability, so it can be shared safely across threads. The database runs in
//! WAL (Write-Ahead Logging) mode for improved concurrent read performance.

pub mod models;
pub mod repository;
pub mod schema;

pub use repository::Database;
