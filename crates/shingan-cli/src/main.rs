//! CLI entry point for shingan (心眼), a multi-modal duplicate file detector.
//!
//! Provides the `scan`, `list`, and `export` subcommands. Run `shingan --help`
//! for usage details.

use anyhow::Context;
use clap::{Parser, Subcommand};
use shingan_core::detector::archive::ArchiveDetector;
use shingan_core::detector::Detector;
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl, ScanProgress};
use shingan_db::models::*;
use shingan_db::Database;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "shingan", about = "Shingan — file deduplicator CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan directories for duplicate files
    Scan {
        /// Directories to scan
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// File types to scan (image, document, video, archive, code)
        #[arg(short, long, default_values_t = vec!["image".to_string(), "document".to_string()])]
        types: Vec<String>,

        /// Similarity threshold (0.80 - 1.00)
        #[arg(short = 'T', long, default_value_t = 0.95)]
        threshold: f64,

        /// Don't recurse into subdirectories
        #[arg(long)]
        no_recursive: bool,
    },

    /// List scan sessions
    List {
        /// Show details for a specific session
        session_id: Option<i64>,
    },

    /// Export scan results to CSV
    Export {
        /// Session ID to export
        session_id: i64,

        /// Output file path
        #[arg(short, long, default_value = "results.csv")]
        output: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let db = Database::open(None).context("Failed to open database")?;

    match cli.command {
        Commands::Scan {
            paths,
            types,
            threshold,
            no_recursive,
        } => cmd_scan(&db, paths, types, threshold, !no_recursive)?,
        Commands::List { session_id } => cmd_list(&db, session_id)?,
        Commands::Export { session_id, output } => cmd_export(&db, session_id, &output)?,
    }

    Ok(())
}

fn cmd_scan(
    db: &Database,
    paths: Vec<PathBuf>,
    types: Vec<String>,
    threshold: f64,
    recursive: bool,
) -> anyhow::Result<()> {
    let categories: Vec<FileCategory> = types
        .iter()
        .filter_map(|t| FileCategory::from_label(t))
        .collect();

    if categories.is_empty() {
        anyhow::bail!("No valid file types specified");
    }

    let file_types_json =
        serde_json::to_string(&types).context("Failed to serialize file types")?;
    let session_id = db
        .create_scan_session(
            &format!("CLI scan {} paths", paths.len()),
            &file_types_json,
            threshold,
        )
        .context("Failed to create scan session")?;

    db.update_session_status(session_id, "running").ok();

    // Build detectors for each requested category
    let mut detectors: HashMap<FileCategory, Box<dyn Detector>> = HashMap::new();
    for cat in &categories {
        let detector: Option<Box<dyn Detector>> = match cat {
            FileCategory::Archive => Some(Box::new(ArchiveDetector::new(threshold))),
            FileCategory::Image => Some(Box::new(
                shingan_core::detector::image::ImageDetector::new(threshold, 12),
            )),
            FileCategory::Code => Some(Box::new(shingan_core::detector::code::CodeDetector::new(
                threshold,
            ))),
            FileCategory::Document => Some(Box::new(
                shingan_core::detector::document::DocumentDetector::new(threshold),
            )),
            FileCategory::Video => Some(Box::new(
                shingan_core::detector::video::VideoDetector::new(threshold),
            )),
        };
        if let Some(d) = detector {
            detectors.insert(*cat, d);
        }
    }

    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    let control = Arc::new(ScanControl::new());

    // Pre-load cached signatures from DB
    let scan_paths: Vec<(PathBuf, bool)> = paths.iter().map(|p| (p.clone(), recursive)).collect();
    let cached = load_cached_signatures_cli(db, &scan_paths, &categories);
    let cache_count = cached.len();
    if cache_count > 0 {
        eprintln!("Loaded {} cached signatures from previous scans", cache_count);
    }

    let scanner = DuplicateScanner::new(
        &categories,
        detectors,
        threshold,
        control.clone(),
        progress_tx,
    )
    .with_cached_signatures(cached);

    // Run scanner in background thread
    let handle = std::thread::spawn(move || scanner.scan_paths(&scan_paths));

    // Print progress
    for msg in &progress_rx {
        match msg {
            ScanProgress::Status(s) => eprintln!("{}", s),
            ScanProgress::Progress {
                current,
                total,
                message,
                elapsed_secs,
                eta_secs,
            } => {
                if total > 0 {
                    let pct = (current as f64 / total as f64 * 100.0) as u32;
                    let elapsed = format_duration_cli(elapsed_secs);
                    let eta = eta_secs
                        .map(|e| format!(" ETA: {}", format_duration_cli(e)))
                        .unwrap_or_default();
                    eprint!("\r[{:3}%] {} | {}{}", pct, message, elapsed, eta);
                }
            }
            ScanProgress::PhaseCompleted { category, groups } => {
                eprintln!(
                    "\n  Found {} {} duplicate groups",
                    groups.len(),
                    category.label()
                );

                struct OwnedEntry {
                    file_path: String,
                    file_size: i64,
                    modified_time: Option<String>,
                }
                struct OwnedGroup {
                    entries: Vec<OwnedEntry>,
                    similarity: f64,
                }

                let owned: Vec<OwnedGroup> = groups
                    .iter()
                    .map(|group| {
                        let entries = group
                            .files
                            .iter()
                            .map(|file_path| {
                                let meta = std::fs::metadata(file_path).ok();
                                let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                                let modified = meta.and_then(|m| m.modified().ok()).and_then(|t| {
                                    t.duration_since(std::time::UNIX_EPOCH)
                                        .ok()
                                        .map(|d| chrono_format_timestamp(d.as_secs()))
                                });
                                OwnedEntry {
                                    file_path: file_path.to_string_lossy().into_owned(),
                                    file_size: size,
                                    modified_time: modified,
                                }
                            })
                            .collect();
                        OwnedGroup {
                            entries,
                            similarity: group.similarity,
                        }
                    })
                    .collect();

                let batch_refs: Vec<(&str, f64, Option<&str>, Vec<NewFileEntryBatch>)> = owned
                    .iter()
                    .map(|group| {
                        let entries: Vec<NewFileEntryBatch> = group
                            .entries
                            .iter()
                            .map(|e| NewFileEntryBatch {
                                file_path: &e.file_path,
                                file_size: e.file_size,
                                modified_time: e.modified_time.as_deref(),
                                thumbnail_path: None,
                                file_metadata: None,
                            })
                            .collect();
                        (
                            category.label(),
                            group.similarity,
                            None as Option<&str>,
                            entries,
                        )
                    })
                    .collect();

                let batch_slices: Vec<(&str, f64, Option<&str>, &[NewFileEntryBatch])> = batch_refs
                    .iter()
                    .map(|(ft, sim, hv, entries)| (*ft, *sim, hv.as_deref(), entries.as_slice()))
                    .collect();

                db.insert_duplicate_groups_batch(session_id, &batch_slices)
                    .context("Failed to insert batch groups")?;
            }
            ScanProgress::Completed => {
                eprintln!("\nScan complete!");
            }
            ScanProgress::Error(e) => {
                eprintln!("\nError: {}", e);
            }
        }
    }

    let (results, new_sigs) = handle
        .join()
        .map_err(|_| anyhow::anyhow!("Scanner thread panicked"))?;
    db.update_session_status(session_id, "completed").ok();

    // Persist newly computed signatures for future rescans
    if !new_sigs.is_empty() {
        persist_new_signatures_cli(db, &new_sigs, &paths, recursive);
        eprintln!("Cached {} new signatures for future scans", new_sigs.len());
    }

    let total_groups: usize = results.values().map(|g| g.len()).sum();
    let total_files: usize = results
        .values()
        .flat_map(|g| g.iter())
        .map(|g| g.files.len())
        .sum();
    println!(
        "\nSession {}: {} groups, {} duplicate files",
        session_id, total_groups, total_files
    );

    Ok(())
}

fn cmd_list(db: &Database, session_id: Option<i64>) -> anyhow::Result<()> {
    match session_id {
        None => {
            let sessions = db.list_scan_sessions().context("Failed to list sessions")?;
            if sessions.is_empty() {
                println!("No scan sessions found.");
                return Ok(());
            }
            println!("{:<6} {:<12} {:<22} Name", "ID", "Status", "Created");
            println!("{}", "-".repeat(60));
            for s in sessions {
                println!(
                    "{:<6} {:<12} {:<22} {}",
                    s.id, s.status, s.created_at, s.name
                );
            }
        }
        Some(id) => {
            let session = db.get_scan_session(id).context("Session not found")?;
            println!(
                "Session {}: {} ({})",
                session.id, session.name, session.status
            );
            println!("Created: {}", session.created_at);
            println!("Threshold: {:.0}%", session.similarity_threshold * 100.0);
            println!();

            let groups = db
                .get_duplicate_groups(id)
                .context("Failed to get groups")?;
            for group in &groups {
                println!(
                    "  Group {} [{}] - {:.1}% similar",
                    group.id,
                    group.file_type,
                    group.similarity_score * 100.0
                );
                let files = db
                    .get_file_entries(group.id)
                    .context("Failed to get files")?;
                for f in &files {
                    println!(
                        "    {} ({:.2} MB)",
                        f.file_path,
                        f.file_size as f64 / 1_048_576.0
                    );
                }
            }
        }
    }

    Ok(())
}

fn cmd_export(db: &Database, session_id: i64, output: &PathBuf) -> anyhow::Result<()> {
    let groups = db
        .get_duplicate_groups(session_id)
        .context("Failed to get groups")?;

    let mut csv =
        String::from("group_id,file_type,similarity,file_path,file_size_bytes,modified_time\n");

    for group in &groups {
        let files = db
            .get_file_entries(group.id)
            .context("Failed to get files")?;
        for f in &files {
            csv.push_str(&format!(
                "{},{},{:.4},\"{}\",{},{}\n",
                group.id,
                group.file_type,
                group.similarity_score,
                f.file_path.replace('"', "\"\""),
                f.file_size,
                f.modified_time.as_deref().unwrap_or(""),
            ));
        }
    }

    std::fs::write(output, &csv).context("Failed to write output file")?;
    println!("Exported {} groups to {}", groups.len(), output.display());

    Ok(())
}

/// Simple timestamp formatter (avoids chrono dependency).
fn chrono_format_timestamp(secs: u64) -> String {
    // Basic ISO-ish format: just return the unix timestamp as a string
    // In a real app, you'd use chrono or time crate
    format!("{}", secs)
}

fn format_duration_cli(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

fn load_cached_signatures_cli(
    db: &Database,
    scan_paths: &[(PathBuf, bool)],
    categories: &[FileCategory],
) -> HashMap<String, String> {
    use shingan_core::file_info::ExtensionMap;
    use std::collections::HashSet;

    let ext_map = ExtensionMap::new();
    let cat_set: HashSet<FileCategory> = categories.iter().copied().collect();
    let mut queries: Vec<(String, i64, i64, String)> = Vec::new();

    for (path, recursive) in scan_paths {
        let walker = if *recursive {
            walkdir::WalkDir::new(path).follow_links(false).into_iter()
        } else {
            walkdir::WalkDir::new(path)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let fpath = entry.path();
            let ext = match fpath.extension().and_then(|e| e.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };
            let cat = match ext_map.get(&ext) {
                Some(c) if cat_set.contains(&c) => c,
                _ => continue,
            };
            if let Ok(meta) = std::fs::metadata(fpath) {
                let size = meta.len() as i64;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                queries.push((
                    fpath.to_string_lossy().into_owned(),
                    size,
                    mtime,
                    cat.label().to_string(),
                ));
            }
        }
    }

    if queries.is_empty() {
        return HashMap::new();
    }

    let query_refs: Vec<(&str, i64, i64, &str)> = queries
        .iter()
        .map(|(p, s, m, c)| (p.as_str(), *s, *m, c.as_str()))
        .collect();

    db.get_cached_signatures(&query_refs).unwrap_or_default()
}

fn persist_new_signatures_cli(
    db: &Database,
    new_sigs: &[(String, String)],
    paths: &[PathBuf],
    recursive: bool,
) {
    let ext_map = shingan_core::file_info::ExtensionMap::new();
    let mut file_meta: HashMap<String, (i64, i64, String)> = HashMap::new();

    for path in paths {
        let walker = if recursive {
            walkdir::WalkDir::new(path).follow_links(false).into_iter()
        } else {
            walkdir::WalkDir::new(path)
                .max_depth(1)
                .follow_links(false)
                .into_iter()
        };
        for entry in walker.filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let fpath = entry.path();
            if let Ok(meta) = std::fs::metadata(fpath) {
                let ext = fpath
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let cat = match ext_map.get(&ext) {
                    Some(c) => c.label().to_string(),
                    None => continue,
                };
                let size = meta.len() as i64;
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                file_meta.insert(fpath.to_string_lossy().into_owned(), (size, mtime, cat));
            }
        }
    }

    let entries: Vec<(&str, i64, i64, &str, &str)> = new_sigs
        .iter()
        .filter_map(|(path, sig)| {
            file_meta
                .get(path)
                .map(|(size, mtime, cat)| (path.as_str(), *size, *mtime, cat.as_str(), sig.as_str()))
        })
        .collect();

    if !entries.is_empty() {
        if let Err(e) = db.cache_signatures_batch(&entries) {
            eprintln!("Failed to cache signatures: {e}");
        }
    }
}
