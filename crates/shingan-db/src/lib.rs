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
//!
//! ## Performance
//!
//! - **Indexed queries** -- `file_entries(file_path)` and `scan_sessions(created_at)`
//!   are indexed for fast lookups and session listing.
//! - **Batch inserts** -- [`Database::insert_duplicate_groups_batch`] persists multiple
//!   groups in a single transaction, reducing lock overhead and write latency.

pub mod models;
pub mod repository;
pub mod schema;

pub use repository::Database;
