/// Row struct for scan_sessions table.
#[derive(Debug, Clone)]
pub struct ScanSession {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub file_types: String,
    pub similarity_threshold: f64,
}

/// Row struct for scanned_paths table.
#[derive(Debug, Clone)]
pub struct ScannedPath {
    pub id: i64,
    pub session_id: i64,
    pub path: String,
    pub include_subdirs: bool,
    pub processed: bool,
}

/// Row struct for duplicate_groups table.
#[derive(Debug, Clone)]
pub struct DuplicateGroupRow {
    pub id: i64,
    pub session_id: i64,
    pub file_type: String,
    pub similarity_score: f64,
    pub hash_value: Option<String>,
}

/// Row struct for file_entries table.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: i64,
    pub group_id: i64,
    pub file_path: String,
    pub file_size: i64,
    pub modified_time: Option<String>,
    pub thumbnail_path: Option<String>,
    pub file_metadata: Option<String>,
    pub marked_for_deletion: bool,
}

/// Row struct for sorting_sessions table.
#[derive(Debug, Clone)]
pub struct SortingSession {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub source_paths: String,
    pub destination_path: String,
    pub use_ml_categorization: bool,
    pub status: String,
}

/// Data for creating a new duplicate group.
pub struct NewDuplicateGroup<'a> {
    pub session_id: i64,
    pub file_type: &'a str,
    pub similarity_score: f64,
    pub hash_value: Option<&'a str>,
}

/// Data for creating a new file entry.
pub struct NewFileEntry<'a> {
    pub group_id: i64,
    pub file_path: &'a str,
    pub file_size: i64,
    pub modified_time: Option<&'a str>,
    pub thumbnail_path: Option<&'a str>,
    pub file_metadata: Option<&'a str>,
}
