//! CLI entry point for shingan (心眼), a multi-modal duplicate file detector.
//!
//! Provides the `scan`, `list`, `export`, `sort`, and `delete` subcommands.
//! Run `shingan --help` for usage details.

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

        /// Merge results into an existing scan session instead of creating a new one
        #[arg(long)]
        merge_session: Option<i64>,

        /// Give this scan session a name
        #[arg(long)]
        name: Option<String>,
    },

    /// List scan sessions
    List {
        /// Show details for a specific session
        session_id: Option<i64>,
    },

    /// Export scan results to CSV or JSON
    Export {
        /// Session ID to export
        session_id: i64,

        /// Output file path
        #[arg(short, long, default_value = "results.csv")]
        output: PathBuf,

        /// Output format (csv or json)
        #[arg(short, long, default_value = "csv")]
        format: String,
    },

    /// Sort files into category directories
    Sort {
        /// Source directories to sort
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// Destination directory
        #[arg(short, long)]
        dest: PathBuf,

        /// Use local ML to sub-categorize images
        #[arg(long)]
        classify: bool,

        /// Show what would be moved without actually moving
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete duplicate files from a scan session
    Delete {
        /// Session ID to delete duplicates from
        session_id: i64,

        /// Strategy: keep newest, oldest, or largest file in each group
        #[arg(long, default_value = "newest", value_parser = parse_keep_strategy)]
        keep: shingan_db::models::KeepStrategy,

        /// Show what would be deleted without actually deleting
        #[arg(long)]
        dry_run: bool,
    },
}

fn parse_keep_strategy(s: &str) -> Result<shingan_db::models::KeepStrategy, String> {
    match s.to_lowercase().as_str() {
        "newest" => Ok(shingan_db::models::KeepStrategy::Newest),
        "oldest" => Ok(shingan_db::models::KeepStrategy::Oldest),
        "largest" => Ok(shingan_db::models::KeepStrategy::Largest),
        _ => Err(format!("Invalid strategy '{}'. Use: newest, oldest, largest", s)),
    }
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
            merge_session,
            name,
        } => cmd_scan(&db, paths, types, threshold, !no_recursive, merge_session, name)?,
        Commands::List { session_id } => cmd_list(&db, session_id)?,
        Commands::Export { session_id, output, format } => cmd_export(&db, session_id, &output, &format)?,
        Commands::Sort {
            paths,
            dest,
            classify,
            dry_run,
        } => cmd_sort(paths, dest, classify, dry_run)?,
        Commands::Delete {
            session_id,
            keep,
            dry_run,
        } => cmd_delete(&db, session_id, keep, dry_run)?,
    }

    Ok(())
}

fn cmd_scan(
    db: &Database,
    paths: Vec<PathBuf>,
    types: Vec<String>,
    threshold: f64,
    recursive: bool,
    merge_session: Option<i64>,
    name: Option<String>,
) -> anyhow::Result<()> {
    let categories: Vec<FileCategory> = types
        .iter()
        .filter_map(|t| FileCategory::from_label(t))
        .collect();

    if categories.is_empty() {
        anyhow::bail!("No valid file types specified");
    }

    let session_id = if let Some(existing_id) = merge_session {
        // Verify the session exists
        db.get_scan_session(existing_id)
            .context("Merge target session not found")?;
        eprintln!("Merging into existing session {}", existing_id);
        existing_id
    } else {
        let file_types_json =
            serde_json::to_string(&types).context("Failed to serialize file types")?;
        let session_name = name.unwrap_or_else(|| format!("CLI scan {} paths", paths.len()));
        db.create_scan_session(&session_name, &file_types_json, threshold)
            .context("Failed to create scan session")?
    };

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

            // Similarity histogram
            if !groups.is_empty() {
                let mut above_95 = 0u32;
                let mut range_80_95 = 0u32;
                let mut range_60_80 = 0u32;
                let mut below_60 = 0u32;
                for g in &groups {
                    let pct = g.similarity_score * 100.0;
                    if pct >= 95.0 {
                        above_95 += 1;
                    } else if pct >= 80.0 {
                        range_80_95 += 1;
                    } else if pct >= 60.0 {
                        range_60_80 += 1;
                    } else {
                        below_60 += 1;
                    }
                }
                println!("Similarity distribution ({} groups):", groups.len());
                println!("  95-100%: {} groups", above_95);
                println!("  80-95%:  {} groups", range_80_95);
                println!("  60-80%:  {} groups", range_60_80);
                if below_60 > 0 {
                    println!("  <60%:    {} groups", below_60);
                }
                println!();
            }

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

fn cmd_export(db: &Database, session_id: i64, output: &PathBuf, format: &str) -> anyhow::Result<()> {
    let groups = db
        .get_duplicate_groups(session_id)
        .context("Failed to get groups")?;

    match format {
        "json" => {
            let mut json_groups = Vec::new();
            for group in &groups {
                let files = db
                    .get_file_entries(group.id)
                    .context("Failed to get files")?;
                let json_files: Vec<serde_json::Value> = files
                    .iter()
                    .map(|f| {
                        serde_json::json!({
                            "file_path": f.file_path,
                            "file_size_bytes": f.file_size,
                            "modified_time": f.modified_time,
                        })
                    })
                    .collect();
                json_groups.push(serde_json::json!({
                    "group_id": group.id,
                    "file_type": group.file_type,
                    "similarity": group.similarity_score,
                    "files": json_files,
                }));
            }
            let json_output = serde_json::json!({
                "session_id": session_id,
                "groups": json_groups,
            });
            let content = serde_json::to_string_pretty(&json_output)
                .context("Failed to serialize JSON")?;
            std::fs::write(output, &content).context("Failed to write output file")?;
        }
        _ => {
            let mut csv = String::from(
                "group_id,file_type,similarity,file_path,file_size_bytes,modified_time\n",
            );
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
        }
    }

    println!("Exported {} groups to {} ({})", groups.len(), output.display(), format);
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

fn cmd_sort(
    paths: Vec<PathBuf>,
    dest: PathBuf,
    classify: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let sorter = shingan_utils::auto_sorter::AutoSorter::new(paths, dest)
        .with_ml(classify)
        .with_dry_run(dry_run);

    if dry_run {
        eprintln!("Dry run mode — no files will be moved.");
    }

    let stats = sorter.sort_files(
        Some(&|current, total, filepath| {
            let pct = if total > 0 {
                (current as f64 / total as f64 * 100.0) as u32
            } else {
                0
            };
            eprint!("\r[{:3}%] {}/{}: {}", pct, current, total, filepath);
        }),
        Some(&|msg| {
            eprintln!("{}", msg);
        }),
    );

    eprintln!();
    if dry_run {
        println!(
            "Dry run: {} would be moved, {} skipped (of {} total)",
            stats.moved, stats.skipped, stats.total
        );
    } else {
        println!(
            "Sort complete: {} moved, {} failed, {} skipped (of {} total)",
            stats.moved, stats.failed, stats.skipped, stats.total
        );
    }

    Ok(())
}

fn cmd_delete(
    db: &Database,
    session_id: i64,
    keep: shingan_db::models::KeepStrategy,
    dry_run: bool,
) -> anyhow::Result<()> {
    // Verify session exists
    let _session = db.get_scan_session(session_id).context("Session not found")?;
    let strategy_name = match keep {
        shingan_db::models::KeepStrategy::Newest => "newest",
        shingan_db::models::KeepStrategy::Oldest => "oldest",
        shingan_db::models::KeepStrategy::Largest => "largest",
    };

    if dry_run {
        eprintln!(
            "Dry run: would delete duplicates from session {} (keep {})",
            session_id, strategy_name
        );
    } else {
        eprintln!(
            "Deleting duplicates from session {} (keep {})",
            session_id, strategy_name
        );
    }

    let (deleted, failed, actions) = db
        .delete_duplicates(session_id, keep, dry_run)
        .context("Failed to process deletions")?;

    for action in &actions {
        println!("  {}", action);
    }

    if dry_run {
        println!(
            "\nDry run complete: {} files would be deleted",
            actions.len()
        );
    } else {
        println!(
            "\nDeletion complete: {} deleted, {} failed",
            deleted, failed
        );
    }

    Ok(())
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
