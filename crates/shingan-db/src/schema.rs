use rusqlite::Connection;

/// Create all tables if they don't exist. Enables WAL mode.
pub fn initialize(conn: &Connection) -> rusqlite::Result<()> {
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

        CREATE TABLE IF NOT EXISTS scanned_paths (
            id INTEGER PRIMARY KEY,
            session_id INTEGER NOT NULL REFERENCES scan_sessions(id) ON DELETE CASCADE,
            path TEXT NOT NULL,
            include_subdirs INTEGER NOT NULL DEFAULT 1,
            processed INTEGER NOT NULL DEFAULT 0
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
        ",
    )?;

    Ok(())
}
