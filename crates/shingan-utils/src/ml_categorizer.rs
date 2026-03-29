use std::path::{Path, PathBuf};

/// ML-powered image categorizer using Ollama vision models.
pub struct MLImageCategorizer {
    ollama_url: String,
    model: String,
}

const CATEGORIES: &[&str] = &[
    "screenshots",
    "photos",
    "memes",
    "artworks",
    "anime_manga",
    "schematics_infographics",
    "others",
];

impl MLImageCategorizer {
    pub fn new(ollama_url: &str, model: &str) -> Self {
        Self {
            ollama_url: ollama_url.to_string(),
            model: model.to_string(),
        }
    }

    /// Categorize a single image via Ollama API.
    pub async fn categorize_image(&self, path: &Path) -> String {
        let image_data = match std::fs::read(path) {
            Ok(data) => data,
            Err(_) => return "others".to_string(),
        };

        let base64_image = base64_encode(&image_data);

        let prompt = format!(
            "Classify this image into exactly ONE of these categories: {}. \
             Respond with only the category name, nothing else.",
            CATEGORIES.join(", ")
        );

        let body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "images": [base64_image],
            "stream": false,
        });

        let url = format!("{}/api/generate", self.ollama_url);

        let client = reqwest::Client::new();
        match client.post(&url).json(&body).send().await {
            Ok(resp) => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(response) = json["response"].as_str() {
                        let category = response.trim().to_lowercase();
                        if CATEGORIES.contains(&category.as_str()) {
                            return category;
                        }
                    }
                }
                "others".to_string()
            }
            Err(_) => "others".to_string(),
        }
    }

    /// Categorize all images in a folder.
    pub async fn categorize_folder(
        &self,
        folder: &Path,
        progress: Option<&dyn Fn(u64, u64, &str)>,
    ) -> Vec<(PathBuf, String)> {
        let mut results = Vec::new();

        let entries: Vec<PathBuf> = walkdir::WalkDir::new(folder)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect();

        let total = entries.len() as u64;

        for (i, path) in entries.iter().enumerate() {
            if let Some(cb) = progress {
                cb(i as u64 + 1, total, &path.to_string_lossy());
            }

            let category = self.categorize_image(path).await;
            results.push((path.clone(), category));
        }

        results
    }

    /// Move images into category subdirectories.
    pub fn sort_by_category(&self, base_dir: &Path, results: &[(PathBuf, String)]) {
        for (path, category) in results {
            let dest_dir = base_dir.join(category);
            std::fs::create_dir_all(&dest_dir).ok();

            if let Some(filename) = path.file_name() {
                let dest = dest_dir.join(filename);
                let _ = std::fs::rename(path, &dest);
            }
        }
    }
}

fn base64_encode(data: &[u8]) -> String {
    // Simple base64 encoding without external dependency
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(4 * (data.len() / 3 + 1));

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARSET[((n >> 18) & 0x3f) as usize] as char);
        result.push(CHARSET[((n >> 12) & 0x3f) as usize] as char);

        if chunk.len() > 1 {
            result.push(CHARSET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(CHARSET[(n & 0x3f) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}
