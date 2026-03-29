use crate::models::*;
use crate::schema;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Lock poisoned")]
    LockPoisoned,
    #[error("Not found: {0}")]
    NotFound(String),
}

/// Thread-safe database wrapper around a SQLite connection.
pub struct Database {
    conn: Mutex<Connection>,
    pub path: PathBuf,
}

impl Database {
    /// Open (or create) the database at the default or specified path.
    pub fn open(path: Option<&Path>) -> Result<Self, DbError> {
        let db_path = match path {
            Some(p) => p.to_path_buf(),
            None => default_db_path(),
        };

        if !db_path
            .to_str()
            .is_some_and(|s| s.starts_with(":memory:"))
        {
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            let conn = Connection::open(&db_path)?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            schema::initialize(&conn)?;
            Ok(Self {
                conn: Mutex::new(conn),
                path: db_path,
            })
        } else {
            let conn = Connection::open_in_memory()?;
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
            schema::initialize(&conn)?;
            Ok(Self {
                conn: Mutex::new(conn),
                path: db_path,
            })
        }
    }

    fn with_conn<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError>,
    {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        f(&conn)
    }

    // --- Scan Sessions ---

    pub fn create_scan_session(
        &self,
        name: &str,
        file_types: &str,
        threshold: f64,
    ) -> Result<i64, DbError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO scan_sessions (name, file_types, similarity_threshold, status)
                 VALUES (?1, ?2, ?3, 'pending')",
                params![name, file_types, threshold],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_scan_session(&self, session_id: i64) -> Result<ScanSession, DbError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, created_at, completed_at, status, file_types, similarity_threshold
                 FROM scan_sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok(ScanSession {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        created_at: row.get(2)?,
                        completed_at: row.get(3)?,
                        status: row.get(4)?,
                        file_types: row.get(5)?,
                        similarity_threshold: row.get(6)?,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DbError::NotFound(format!("session {}", session_id))
                }
                other => DbError::Sqlite(other),
            })
        })
    }

    pub fn update_session_status(&self, session_id: i64, status: &str) -> Result<(), DbError> {
        self.with_conn(|conn| {
            if status == "completed" {
                conn.execute(
                    "UPDATE scan_sessions SET status = ?1, completed_at = datetime('now') WHERE id = ?2",
                    params![status, session_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE scan_sessions SET status = ?1 WHERE id = ?2",
                    params![status, session_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn list_scan_sessions(&self) -> Result<Vec<ScanSession>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, created_at, completed_at, status, file_types, similarity_threshold
                 FROM scan_sessions ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(ScanSession {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    completed_at: row.get(3)?,
                    status: row.get(4)?,
                    file_types: row.get(5)?,
                    similarity_threshold: row.get(6)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
        })
    }

    pub fn clear_sessions(&self) -> Result<u64, DbError> {
        self.with_conn(|conn| {
            let count = conn.execute("DELETE FROM scan_sessions", [])?;
            Ok(count as u64)
        })
    }

    // --- Duplicate Groups ---

    pub fn insert_duplicate_group(&self, group: &NewDuplicateGroup) -> Result<i64, DbError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO duplicate_groups (session_id, file_type, similarity_score, hash_value)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    group.session_id,
                    group.file_type,
                    group.similarity_score,
                    group.hash_value,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn get_duplicate_groups(&self, session_id: i64) -> Result<Vec<DuplicateGroupRow>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, file_type, similarity_score, hash_value
                 FROM duplicate_groups WHERE session_id = ?1
                 ORDER BY similarity_score DESC",
            )?;
            let rows = stmt.query_map(params![session_id], |row| {
                Ok(DuplicateGroupRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    file_type: row.get(2)?,
                    similarity_score: row.get(3)?,
                    hash_value: row.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
        })
    }

    // --- File Entries ---

    pub fn insert_file_entry(&self, entry: &NewFileEntry) -> Result<i64, DbError> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO file_entries (group_id, file_path, file_size, modified_time, thumbnail_path, file_metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.group_id,
                    entry.file_path,
                    entry.file_size,
                    entry.modified_time,
                    entry.thumbnail_path,
                    entry.file_metadata,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn insert_duplicate_groups_batch(
        &self,
        session_id: i64,
        groups: &[(&str, f64, Option<&str>, &[NewFileEntryBatch])],
    ) -> Result<Vec<i64>, DbError> {
        self.with_conn(|conn| {
            conn.execute_batch("BEGIN TRANSACTION")?;
            let mut ids = Vec::with_capacity(groups.len());
            let result = (|| -> Result<(), DbError> {
                for (file_type, similarity_score, hash_value, entries) in groups {
                    conn.execute(
                        "INSERT INTO duplicate_groups (session_id, file_type, similarity_score, hash_value)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![session_id, file_type, similarity_score, hash_value],
                    )?;
                    let group_id = conn.last_insert_rowid();
                    ids.push(group_id);
                    for entry in *entries {
                        conn.execute(
                            "INSERT INTO file_entries (group_id, file_path, file_size, modified_time, thumbnail_path, file_metadata)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![
                                group_id,
                                entry.file_path,
                                entry.file_size,
                                entry.modified_time,
                                entry.thumbnail_path,
                                entry.file_metadata,
                            ],
                        )?;
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(ids)
                }
                Err(e) => {
                    conn.execute_batch("ROLLBACK").ok();
                    Err(e)
                }
            }
        })
    }

    pub fn get_file_entries(&self, group_id: i64) -> Result<Vec<FileEntry>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, group_id, file_path, file_size, modified_time, thumbnail_path, file_metadata, marked_for_deletion
                 FROM file_entries WHERE group_id = ?1",
            )?;
            let rows = stmt.query_map(params![group_id], |row| {
                Ok(FileEntry {
                    id: row.get(0)?,
                    group_id: row.get(1)?,
                    file_path: row.get(2)?,
                    file_size: row.get(3)?,
                    modified_time: row.get(4)?,
                    thumbnail_path: row.get(5)?,
                    file_metadata: row.get(6)?,
                    marked_for_deletion: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
        })
    }

    pub fn delete_file_entry(&self, entry_id: i64) -> Result<(), DbError> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM file_entries WHERE id = ?1", params![entry_id])?;
            Ok(())
        })
    }

    // --- Maintenance ---

    pub fn vacuum(&self) -> Result<(), DbError> {
        self.with_conn(|conn| {
            conn.execute_batch("VACUUM;")?;
            Ok(())
        })
    }
}

fn default_db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("shingan")
        .join("shingan.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open(Some(Path::new(":memory:"))).unwrap()
    }

    #[test]
    fn test_create_and_get_session() {
        let db = test_db();
        let id = db.create_scan_session("test", "image", 0.95).unwrap();
        let session = db.get_scan_session(id).unwrap();
        assert_eq!(session.name, "test");
        assert_eq!(session.file_types, "image");
        assert!((session.similarity_threshold - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn test_insert_and_get_groups() {
        let db = test_db();
        let sid = db.create_scan_session("s", "image", 0.9).unwrap();
        let g = NewDuplicateGroup {
            session_id: sid,
            file_type: "image",
            similarity_score: 0.98,
            hash_value: None,
        };
        let gid = db.insert_duplicate_group(&g).unwrap();
        let groups = db.get_duplicate_groups(sid).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, gid);
        assert!((groups[0].similarity_score - 0.98).abs() < f64::EPSILON);
    }

    #[test]
    fn test_insert_and_get_file_entries() {
        let db = test_db();
        let sid = db.create_scan_session("s", "image", 0.9).unwrap();
        let g = NewDuplicateGroup {
            session_id: sid,
            file_type: "image",
            similarity_score: 0.95,
            hash_value: None,
        };
        let gid = db.insert_duplicate_group(&g).unwrap();
        let entry = NewFileEntry {
            group_id: gid,
            file_path: "/tmp/a.png",
            file_size: 1024,
            modified_time: None,
            thumbnail_path: None,
            file_metadata: None,
        };
        let eid = db.insert_file_entry(&entry).unwrap();
        let entries = db.get_file_entries(gid).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, eid);
        assert_eq!(entries[0].file_path, "/tmp/a.png");
    }

    #[test]
    fn test_delete_file_entry() {
        let db = test_db();
        let sid = db.create_scan_session("s", "image", 0.9).unwrap();
        let g = NewDuplicateGroup {
            session_id: sid,
            file_type: "image",
            similarity_score: 0.95,
            hash_value: None,
        };
        let gid = db.insert_duplicate_group(&g).unwrap();
        let entry = NewFileEntry {
            group_id: gid,
            file_path: "/tmp/b.png",
            file_size: 2048,
            modified_time: None,
            thumbnail_path: None,
            file_metadata: None,
        };
        let eid = db.insert_file_entry(&entry).unwrap();
        assert_eq!(db.get_file_entries(gid).unwrap().len(), 1);
        db.delete_file_entry(eid).unwrap();
        assert!(db.get_file_entries(gid).unwrap().is_empty());
    }

    #[test]
    fn test_batch_insert() {
        let db = test_db();
        let sid = db.create_scan_session("s", "image", 0.9).unwrap();
        let entries1 = [
            NewFileEntryBatch {
                file_path: "/tmp/x.png",
                file_size: 100,
                modified_time: None,
                thumbnail_path: None,
                file_metadata: None,
            },
            NewFileEntryBatch {
                file_path: "/tmp/y.png",
                file_size: 200,
                modified_time: None,
                thumbnail_path: None,
                file_metadata: None,
            },
        ];
        let entries2 = [NewFileEntryBatch {
            file_path: "/tmp/z.png",
            file_size: 300,
            modified_time: None,
            thumbnail_path: None,
            file_metadata: None,
        }];
        let groups: [(&str, f64, Option<&str>, &[NewFileEntryBatch]); 2] = [
            ("image", 0.95, None, &entries1),
            ("image", 0.90, None, &entries2),
        ];
        let ids = db.insert_duplicate_groups_batch(sid, &groups).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(db.get_file_entries(ids[0]).unwrap().len(), 2);
        assert_eq!(db.get_file_entries(ids[1]).unwrap().len(), 1);
    }

    #[test]
    fn test_vacuum() {
        let db = test_db();
        db.vacuum().unwrap();
    }

    #[test]
    fn test_cascade_delete() {
        let db = test_db();
        let sid = db.create_scan_session("s", "image", 0.9).unwrap();
        let g = NewDuplicateGroup {
            session_id: sid,
            file_type: "image",
            similarity_score: 0.95,
            hash_value: None,
        };
        let gid = db.insert_duplicate_group(&g).unwrap();
        let entry = NewFileEntry {
            group_id: gid,
            file_path: "/tmp/c.png",
            file_size: 512,
            modified_time: None,
            thumbnail_path: None,
            file_metadata: None,
        };
        db.insert_file_entry(&entry).unwrap();
        assert_eq!(db.get_file_entries(gid).unwrap().len(), 1);

        db.with_conn(|conn| {
            conn.execute("DELETE FROM scan_sessions WHERE id = ?1", params![sid])?;
            Ok(())
        })
        .unwrap();

        let groups = db.get_duplicate_groups(sid).unwrap();
        assert!(groups.is_empty());
        assert!(db.get_file_entries(gid).unwrap().is_empty());
    }
}
