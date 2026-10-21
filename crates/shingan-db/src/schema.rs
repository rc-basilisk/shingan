use rusqlite::Connection;

/// Current schema version, stored in SQLite's `user_version` pragma.
///
/// - 1: initial schema (v0.1.x)
/// - 2: signature formats changed (MinHash for text, pHash for images, v2 tags);
///   cached v1 signatures are discarded.
pub const SCHEMA_VERSION: i64 = 2;

/// Create all tables if they don't exist, enable WAL mode and run migrations.
pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    create_tables(conn)?;
    migrate(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 2 {
        conn.execute("DELETE FROM signature_cache", [])?;
    }
    if version < SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    }
    Ok(())
}

fn create_tables(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS scan_sessions (
            id INTEGER PRIMARY KEY,
            name TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at TEXT,
            status TEXT NOT NULL DEFAULT 'pending',
            file_types TEXT,
            similarity_threshold REAL DEFAULT 0.95
        );

        CREATE TABLE IF NOT EXISTS duplicate_groups (
            id INTEGER PRIMARY KEY,
            session_id INTEGER NOT NULL REFERENCES scan_sessions(id) ON DELETE CASCADE,
            file_type TEXT NOT NULL,
            similarity_score REAL NOT NULL,
            hash_value TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_duplicate_groups_session_id
            ON duplicate_groups(session_id);

        CREATE TABLE IF NOT EXISTS file_entries (
            id INTEGER PRIMARY KEY,
            group_id INTEGER NOT NULL REFERENCES duplicate_groups(id) ON DELETE CASCADE,
            file_path TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_time TEXT,
            thumbnail_path TEXT,
            file_metadata TEXT,
            marked_for_deletion INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_file_entries_group_id
            ON file_entries(group_id);
        CREATE INDEX IF NOT EXISTS idx_file_entries_file_path ON file_entries(file_path);
        CREATE INDEX IF NOT EXISTS idx_scan_sessions_created_at ON scan_sessions(created_at);

        CREATE TABLE IF NOT EXISTS signature_cache (
            file_path TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            category TEXT NOT NULL,
            signature TEXT NOT NULL,
            PRIMARY KEY (file_path, category)
        );

        CREATE TABLE IF NOT EXISTS sorting_sessions (
            id INTEGER PRIMARY KEY,
            name TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at TEXT,
            source_paths TEXT,
            destination_path TEXT,
            use_ml_categorization INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'pending'
        );

        CREATE TABLE IF NOT EXISTS classification_cache (
            file_path TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_at INTEGER NOT NULL,
            sub_category TEXT NOT NULL,
            confidence REAL NOT NULL,
            tier INTEGER NOT NULL,
            PRIMARY KEY (file_path)
        );
        ",
    )?;

    Ok(())
}
