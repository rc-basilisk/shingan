use crate::models::*;
use crate::schema;
use rusqlite::{params, Connection};
use std::collections::HashMap;
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

        if !db_path.to_str().is_some_and(|s| s.starts_with(":memory:")) {
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

    /// Get all file entries for a session (across all groups), with group info.
    pub fn get_session_files(
        &self,
        session_id: i64,
    ) -> Result<Vec<(DuplicateGroupRow, Vec<FileEntry>)>, DbError> {
        let groups = self.get_duplicate_groups(session_id)?;
        let mut result = Vec::new();
        for group in groups {
            let entries = self.get_file_entries(group.id)?;
            result.push((group, entries));
        }
        Ok(result)
    }

    /// Delete files from disk based on a keep strategy, returning (deleted, failed) counts.
    pub fn delete_duplicates(
        &self,
        session_id: i64,
        keep: KeepStrategy,
        dry_run: bool,
    ) -> Result<(u32, u32, Vec<String>), DbError> {
        let group_data = self.get_session_files(session_id)?;
        let mut deleted = 0u32;
        let mut failed = 0u32;
        let mut actions = Vec::new();

        for (group, entries) in &group_data {
            if entries.len() < 2 {
                continue;
            }

            let keep_idx = match keep {
                KeepStrategy::Newest => entries
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, e)| {
                        std::fs::metadata(&e.file_path)
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0),
                KeepStrategy::Oldest => entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, e)| {
                        std::fs::metadata(&e.file_path)
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                    })
                    .map(|(i, _)| i)
                    .unwrap_or(0),
                KeepStrategy::Largest => entries
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, e)| e.file_size)
                    .map(|(i, _)| i)
                    .unwrap_or(0),
            };

            for (i, entry) in entries.iter().enumerate() {
                if i == keep_idx {
                    continue;
                }
                if dry_run {
                    actions.push(format!(
                        "Would delete: {} ({:.2} MB) [group {} - {:.1}% similar]",
                        entry.file_path,
                        entry.file_size as f64 / 1_048_576.0,
                        group.id,
                        group.similarity_score * 100.0,
                    ));
                } else {
                    match std::fs::remove_file(&entry.file_path) {
                        Ok(_) => {
                            deleted += 1;
                            actions.push(format!("Deleted: {}", entry.file_path));
                        }
                        Err(e) => {
                            failed += 1;
                            actions.push(format!("Failed: {} ({})", entry.file_path, e));
                        }
                    }
                }
            }
        }

        Ok((deleted, failed, actions))
    }

    // --- Signature Cache ---

    /// Look up cached signatures for files that haven't changed (same size + mtime).
    /// Returns a map from file_path to signature for cache hits.
    pub fn get_cached_signatures(
        &self,
        files: &[(&str, i64, i64, &str)], // (path, size, modified_secs, category)
    ) -> Result<HashMap<String, String>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT signature FROM signature_cache
                 WHERE file_path = ?1 AND file_size = ?2 AND modified_at = ?3 AND category = ?4",
            )?;
            let mut result = HashMap::new();
            for &(path, size, mtime, category) in files {
                if let Ok(sig) = stmt.query_row(params![path, size, mtime, category], |row| {
                    row.get::<_, String>(0)
                }) {
                    result.insert(path.to_string(), sig);
                }
            }
            Ok(result)
        })
    }

    /// Store computed signatures in the cache (upsert).
    pub fn cache_signatures_batch(
        &self,
        entries: &[(&str, i64, i64, &str, &str)], // (path, size, modified_secs, category, signature)
    ) -> Result<(), DbError> {
        self.with_conn(|conn| {
            conn.execute_batch("BEGIN TRANSACTION")?;
            let result = (|| -> Result<(), DbError> {
                for &(path, size, mtime, category, signature) in entries {
                    conn.execute(
                        "INSERT OR REPLACE INTO signature_cache (file_path, file_size, modified_at, category, signature)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![path, size, mtime, category, signature],
                    )?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    conn.execute_batch("ROLLBACK").ok();
                    Err(e)
                }
            }
        })
    }

    // --- Classification Cache ---

    /// Look up cached classifications for unchanged files.
    pub fn get_cached_classifications(
        &self,
        files: &[(&str, i64, i64)], // (path, size, modified_secs)
    ) -> Result<HashMap<String, (String, f64, i64)>, DbError> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT sub_category, confidence, tier FROM classification_cache
                 WHERE file_path = ?1 AND file_size = ?2 AND modified_at = ?3",
            )?;
            let mut result = HashMap::new();
            for &(path, size, mtime) in files {
                if let Ok((sub_cat, conf, tier)) =
                    stmt.query_row(params![path, size, mtime], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, f64>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })
                {
                    result.insert(path.to_string(), (sub_cat, conf, tier));
                }
            }
            Ok(result)
        })
    }

    /// Upsert classification results.
    pub fn cache_classifications_batch(
        &self,
        entries: &[(&str, i64, i64, &str, f64, i64)], // (path, size, mtime, sub_category, confidence, tier)
    ) -> Result<(), DbError> {
        self.with_conn(|conn| {
            conn.execute_batch("BEGIN TRANSACTION")?;
            let result = (|| -> Result<(), DbError> {
                for &(path, size, mtime, sub_cat, conf, tier) in entries {
                    conn.execute(
                        "INSERT OR REPLACE INTO classification_cache (file_path, file_size, modified_at, sub_category, confidence, tier)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![path, size, mtime, sub_cat, conf, tier],
                    )?;
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    conn.execute_batch("ROLLBACK").ok();
                    Err(e)
                }
            }
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

    #[test]
    fn snapshot_session_list_format() {
        let db = test_db();
        db.create_scan_session("Photo cleanup", "[\"image\"]", 0.95)
            .unwrap();
        db.create_scan_session("Code review", "[\"code\"]", 0.90)
            .unwrap();

        let sessions = db.list_scan_sessions().unwrap();
        let output: Vec<String> = sessions
            .iter()
            .map(|s| format!("{:<6} {:<12} {}", s.id, s.status, s.name))
            .collect();
        insta::assert_snapshot!(output.join("\n"), @r"
        2      pending      Code review
        1      pending      Photo cleanup
        ");
    }

    #[test]
    fn snapshot_group_detail_format() {
        let db = test_db();
        let sid = db.create_scan_session("test", "image", 0.9).unwrap();
        let entries = [
            NewFileEntryBatch {
                file_path: "/photos/beach.jpg",
                file_size: 2_500_000,
                modified_time: None,
                thumbnail_path: None,
                file_metadata: None,
            },
            NewFileEntryBatch {
                file_path: "/photos/beach_copy.jpg",
                file_size: 2_500_100,
                modified_time: None,
                thumbnail_path: None,
                file_metadata: None,
            },
        ];
        let groups: [(&str, f64, Option<&str>, &[NewFileEntryBatch]); 1] =
            [("image", 0.97, None, &entries)];
        db.insert_duplicate_groups_batch(sid, &groups).unwrap();

        let db_groups = db.get_duplicate_groups(sid).unwrap();
        let mut output = Vec::new();
        for group in &db_groups {
            output.push(format!(
                "Group {} [{}] - {:.1}% similar",
                group.id,
                group.file_type,
                group.similarity_score * 100.0
            ));
            let files = db.get_file_entries(group.id).unwrap();
            for f in &files {
                output.push(format!(
                    "  {} ({:.2} MB)",
                    f.file_path,
                    f.file_size as f64 / 1_048_576.0
                ));
            }
        }
        insta::assert_snapshot!(output.join("\n"), @r"
        Group 1 [image] - 97.0% similar
          /photos/beach.jpg (2.38 MB)
          /photos/beach_copy.jpg (2.38 MB)
        ");
    }

    #[test]
    fn test_signature_cache() {
        let db = test_db();

        // Cache some signatures
        let entries = [
            ("/tmp/a.png", 1024_i64, 1000_i64, "image", "sig_a"),
            ("/tmp/b.png", 2048, 2000, "image", "sig_b"),
        ];
        db.cache_signatures_batch(&entries).unwrap();

        // Query with matching metadata -> cache hit
        let queries = [
            ("/tmp/a.png", 1024_i64, 1000_i64, "image"),
            ("/tmp/b.png", 2048, 2000, "image"),
            ("/tmp/c.png", 512, 500, "image"), // not cached
        ];
        let cached = db.get_cached_signatures(&queries).unwrap();
        assert_eq!(cached.len(), 2);
        assert_eq!(cached["/tmp/a.png"], "sig_a");
        assert_eq!(cached["/tmp/b.png"], "sig_b");

        // Query with changed size -> cache miss
        let queries_changed = [("/tmp/a.png", 9999_i64, 1000_i64, "image")];
        let cached2 = db.get_cached_signatures(&queries_changed).unwrap();
        assert!(cached2.is_empty());

        // Query with changed mtime -> cache miss
        let queries_mtime = [("/tmp/a.png", 1024_i64, 9999_i64, "image")];
        let cached3 = db.get_cached_signatures(&queries_mtime).unwrap();
        assert!(cached3.is_empty());

        // Upsert: update signature for existing file
        let updated = [("/tmp/a.png", 1024_i64, 3000_i64, "image", "sig_a_v2")];
        db.cache_signatures_batch(&updated).unwrap();
        let queries_new = [("/tmp/a.png", 1024_i64, 3000_i64, "image")];
        let cached4 = db.get_cached_signatures(&queries_new).unwrap();
        assert_eq!(cached4["/tmp/a.png"], "sig_a_v2");
    }
}
