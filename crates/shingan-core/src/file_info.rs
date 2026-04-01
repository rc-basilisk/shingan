use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FileCategory {
    Image,
    Document,
    Video,
    Archive,
    Code,
}

impl FileCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Document => "document",
            Self::Video => "video",
            Self::Archive => "archive",
            Self::Code => "code",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "image" => Some(Self::Image),
            "document" => Some(Self::Document),
            "video" => Some(Self::Video),
            "archive" => Some(Self::Archive),
            "code" => Some(Self::Code),
            _ => None,
        }
    }

    pub fn all() -> &'static [FileCategory] {
        &[
            Self::Image,
            Self::Document,
            Self::Video,
            Self::Archive,
            Self::Code,
        ]
    }
}

impl std::fmt::Display for FileCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub name: String,
    pub extension: String,
    pub category: FileCategory,
    /// Sniffed or guessed MIME type (when enriched).
    pub mime_type: Option<String>,
    /// Image width/height when decodable (when enriched).
    pub dimensions: Option<(u32, u32)>,
    /// True when EXIF suggests camera/scan capture (when enriched).
    pub has_exif: bool,
    /// Last classification sub-category label (e.g. image sub-folder name).
    pub sub_category: Option<String>,
    pub classification_confidence: Option<f32>,
    /// Classification tier (0=heuristics, 1=structure, 2=local ONNX, 3=cloud).
    pub classification_tier: Option<u8>,
}

impl FileInfo {
    pub fn from_path(path: &Path, category: FileCategory) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        Ok(Self {
            path: path.to_path_buf(),
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            name,
            extension,
            category,
            mime_type: None,
            dimensions: None,
            has_exif: false,
            sub_category: None,
            classification_confidence: None,
            classification_tier: None,
        })
    }

    /// Fill MIME, dimensions, and EXIF flag for image files (no-op for other categories).
    pub fn enrich_metadata(&mut self) {
        crate::enrichment::enrich_image_file_info(
            &self.path,
            &self.extension,
            self.category,
            &mut self.mime_type,
            &mut self.dimensions,
            &mut self.has_exif,
        );
    }
}

/// Maps file extensions to their category.
pub struct ExtensionMap {
    map: HashMap<&'static str, FileCategory>,
}

impl ExtensionMap {
    pub fn new() -> Self {
        let mut map = HashMap::new();

        let image_exts = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "svg"];
        let document_exts = [
            "txt", "doc", "docx", "odt", "pdf", "rtf", "srt", "vtt", "sub",
        ];
        let video_exts = ["mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "m4v"];
        let archive_exts = ["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst"];
        let code_exts = [
            "py", "js", "ts", "exs", "html", "css", "jsx", "tsx", "vue", "rs", "go", "cpp", "c",
            "h",
        ];

        for ext in image_exts {
            map.insert(ext, FileCategory::Image);
        }
        for ext in document_exts {
            map.insert(ext, FileCategory::Document);
        }
        for ext in video_exts {
            map.insert(ext, FileCategory::Video);
        }
        for ext in archive_exts {
            map.insert(ext, FileCategory::Archive);
        }
        for ext in code_exts {
            map.insert(ext, FileCategory::Code);
        }

        Self { map }
    }

    pub fn get(&self, extension: &str) -> Option<FileCategory> {
        self.map.get(extension.to_lowercase().as_str()).copied()
    }

    pub fn extensions_for(&self, category: FileCategory) -> Vec<&'static str> {
        self.map
            .iter()
            .filter(|(_, &cat)| cat == category)
            .map(|(&ext, _)| ext)
            .collect()
    }
}

impl Default for ExtensionMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Directories to skip during scanning.
pub const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".git",
    ".svn",
    "venv",
    "env",
    ".venv",
    "dist",
    "build",
    ".cache",
    ".pytest_cache",
    ".mypy_cache",
];
