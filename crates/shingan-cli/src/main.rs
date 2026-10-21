//! CLI entry point for shingan (心眼), a multi-modal duplicate file detector.
//!
//! Run `shingan --help` for usage details.

use anyhow::{bail, Context};
use clap::{Parser, Subcommand, ValueEnum};
use shingan_core::detector::archive::ArchiveDetector;
use shingan_core::detector::Detector;
use shingan_core::file_info::FileCategory;
use shingan_core::scanner::duplicate::{DuplicateScanner, ScanControl, ScanProgress};
use shingan_db::models::*;
use shingan_db::Database;
use shingan_utils::cleanup::{self, KeepStrategy, RemoveMode};
use shingan_utils::settings::{self, AppSettings};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "shingan",
    version,
    about = "Find duplicate and near-duplicate files, and sort messy folders.",
    after_help = "Examples:\n  \
        shingan scan ~/Pictures                 find duplicate images and documents\n  \
        shingan scan ~/Downloads -t all -T 0.9  every file type, 90% similarity\n  \
        shingan list 3                          show the groups found by scan #3\n  \
        shingan delete 3 --keep oldest          move extra copies to the trash\n  \
        shingan sort ~/Downloads -d ~/Sorted    organize files into folders\n  \
        shingan undo-sort ~/Sorted              put them back\n  \
        shingan doctor                          check optional components"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan folders for duplicate and near-duplicate files
    Scan {
        /// Folders to scan
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// File types to compare: image, document, video, archive, code, or all
        /// (comma-separated or repeated)
        #[arg(short, long, value_delimiter = ',', default_values_t = vec!["image".to_string(), "document".to_string()])]
        types: Vec<String>,

        /// How similar files must be to count as duplicates (0.5 - 1.0)
        #[arg(short = 'T', long, default_value_t = 0.95)]
        threshold: f64,

        /// Only scan the given folders, not their subfolders
        #[arg(long)]
        no_recursive: bool,

        /// Ignore files smaller than this many KiB (default: from settings, 1)
        #[arg(long)]
        min_size_kb: Option<u64>,

        /// Worker threads (default: from settings, 0 = all cores)
        #[arg(long)]
        threads: Option<usize>,

        /// Add results to an existing scan session instead of creating a new one
        #[arg(long)]
        merge_session: Option<i64>,

        /// Give this scan session a name
        #[arg(long)]
        name: Option<String>,
    },

    /// List scan sessions, or show the duplicate groups of one session
    List {
        /// Session to show in detail
        session_id: Option<i64>,
    },

    /// Export a session's results to CSV or JSON
    Export {
        /// Session ID to export
        session_id: i64,

        /// Output file (format is taken from the extension unless --format is given)
        #[arg(short, long, default_value = "results.csv")]
        output: PathBuf,

        /// Output format
        #[arg(short, long, value_enum)]
        format: Option<ExportFormat>,
    },

    /// Remove duplicate copies found by a scan, keeping one file per group
    Delete {
        /// Session ID to clean up
        session_id: i64,

        /// Which copy to keep: newest, oldest, largest, shortest-path
        #[arg(long, default_value = "oldest", value_parser = parse_keep_strategy)]
        keep: KeepStrategy,

        /// Show what would be removed without touching anything
        #[arg(long)]
        dry_run: bool,

        /// Delete permanently instead of moving to the trash
        #[arg(long)]
        permanent: bool,

        /// Don't ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },

    /// Sort files into folders by type (images/, documents/, videos/, ...)
    Sort {
        /// Source folders to sort
        #[arg(required = true)]
        paths: Vec<PathBuf>,

        /// Destination folder
        #[arg(short, long)]
        dest: PathBuf,

        /// Also sort images into sub-folders (photo_landscape, screenshot_desktop, ...)
        #[arg(long)]
        classify: bool,

        /// Ask the cloud provider configured in settings about uncertain images
        /// (implies --classify)
        #[arg(long)]
        cloud: bool,

        /// Leave files of unknown types where they are instead of moving them to others/
        #[arg(long)]
        skip_unknown: bool,

        /// Show what would be moved without moving anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Undo a sort, moving files back where they came from
    UndoSort {
        /// The sort destination folder (uses its latest journal) or a journal file
        target: PathBuf,
    },

    /// Manage the optional local image-classification model
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },

    /// Check which optional components are available
    Doctor,
}

#[derive(Subcommand)]
enum ModelAction {
    /// Show whether the model is installed
    Status,
    /// Download the CLIP model (~340 MB)
    Download,
    /// Delete the downloaded model
    Remove,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportFormat {
    Csv,
    Json,
}

fn parse_keep_strategy(s: &str) -> Result<KeepStrategy, String> {
    KeepStrategy::parse(s).ok_or_else(|| {
        format!("invalid strategy '{s}'. Use: newest, oldest, largest, shortest-path")
    })
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let settings = settings::load_settings();

    match cli.command {
        Commands::Scan {
            paths,
            types,
            threshold,
            no_recursive,
            min_size_kb,
            threads,
            merge_session,
            name,
        } => {
            let db = open_db()?;
            cmd_scan(
                &db,
                &settings,
                ScanArgs {
                    paths,
                    types,
                    threshold,
                    recursive: !no_recursive,
                    min_size_kb: min_size_kb.unwrap_or(settings.min_file_size_kb),
                    threads: threads.unwrap_or(settings.thread_count as usize),
                    merge_session,
                    name,
                },
            )
        }
        Commands::List { session_id } => cmd_list(&open_db()?, session_id),
        Commands::Export {
            session_id,
            output,
            format,
        } => cmd_export(&open_db()?, session_id, &output, format),
        Commands::Delete {
            session_id,
            keep,
            dry_run,
            permanent,
            yes,
        } => {
            let mode = if permanent || settings.delete_permanently {
                RemoveMode::Permanent
            } else {
                RemoveMode::Trash
            };
            cmd_delete(&open_db()?, session_id, keep, dry_run, mode, yes)
        }
        Commands::Sort {
            paths,
            dest,
            classify,
            cloud,
            skip_unknown,
            dry_run,
        } => cmd_sort(
            &settings,
            paths,
            dest,
            classify || cloud,
            cloud,
            skip_unknown,
            dry_run,
        ),
        Commands::UndoSort { target } => cmd_undo_sort(&target),
        Commands::Model { action } => cmd_model(&settings, action),
        Commands::Doctor => cmd_doctor(&settings),
    }
}

fn open_db() -> anyhow::Result<Database> {
    Database::open(None).context("Failed to open the shingan database")
}

struct ScanArgs {
    paths: Vec<PathBuf>,
    types: Vec<String>,
    threshold: f64,
    recursive: bool,
    min_size_kb: u64,
    threads: usize,
    merge_session: Option<i64>,
    name: Option<String>,
}

fn parse_categories(types: &[String]) -> anyhow::Result<Vec<FileCategory>> {
    let mut categories = Vec::new();
    for t in types {
        let t = t.trim().to_lowercase();
        if t == "all" {
            return Ok(FileCategory::all().to_vec());
        }
        let label = match t.as_str() {
            "image" | "images" | "img" | "photo" | "photos" | "pictures" => "image",
            "document" | "documents" | "doc" | "docs" | "text" => "document",
            "video" | "videos" | "vid" | "movies" => "video",
            "archive" | "archives" | "zip" => "archive",
            "code" | "source" | "src" => "code",
            other => other,
        };
        match FileCategory::from_label(label) {
            Some(c) if !categories.contains(&c) => categories.push(c),
            Some(_) => {}
            None => {
                bail!("unknown file type '{t}'. Use: image, document, video, archive, code, all")
            }
        }
    }
    if categories.is_empty() {
        bail!("no file types specified");
    }
    Ok(categories)
}

fn build_detectors(
    categories: &[FileCategory],
    threshold: f64,
    settings: &AppSettings,
) -> HashMap<FileCategory, Box<dyn Detector>> {
    use shingan_core::detector::{code, document, image, video};
    let mut detectors: HashMap<FileCategory, Box<dyn Detector>> = HashMap::new();
    for cat in categories {
        let detector: Box<dyn Detector> = match cat {
            FileCategory::Archive => Box::new(ArchiveDetector::new(threshold)),
            FileCategory::Image => Box::new(image::ImageDetector::new(threshold, 12)),
            FileCategory::Code => Box::new(code::CodeDetector::new(threshold)),
            FileCategory::Document => Box::new(document::DocumentDetector::new(threshold)),
            FileCategory::Video => Box::new(video::VideoDetector::with_sampling(
                threshold,
                settings.video_skip_secs,
                settings.video_duration_secs,
            )),
        };
        detectors.insert(*cat, detector);
    }
    detectors
}

fn cmd_scan(db: &Database, settings: &AppSettings, args: ScanArgs) -> anyhow::Result<()> {
    let categories = parse_categories(&args.types)?;
    if !(0.5..=1.0).contains(&args.threshold) {
        bail!(
            "threshold must be between 0.5 and 1.0 (got {})",
            args.threshold
        );
    }
    for p in &args.paths {
        if !p.is_dir() {
            bail!("{} is not a folder", p.display());
        }
    }

    let session_id = if let Some(existing_id) = args.merge_session {
        db.get_scan_session(existing_id)
            .context("Merge target session not found")?;
        eprintln!("Merging into existing session {}", existing_id);
        existing_id
    } else {
        let labels: Vec<&str> = categories.iter().map(|c| c.label()).collect();
        let file_types_json = serde_json::to_string(&labels)?;
        let session_name = args.name.clone().unwrap_or_else(|| {
            let names: Vec<String> = args
                .paths
                .iter()
                .map(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.display().to_string())
                })
                .collect();
            names.join(", ")
        });
        db.create_scan_session(&session_name, &file_types_json, args.threshold)
            .context("Failed to create scan session")?
    };
    db.update_session_status(session_id, "running").ok();

    let db_cache = Arc::new(Database::open(Some(&db.path)).context("Failed to open database")?);
    let (progress_tx, progress_rx) = crossbeam_channel::unbounded();
    let control = Arc::new(ScanControl::new());
    let scanner = DuplicateScanner::new(
        &categories,
        build_detectors(&categories, args.threshold, settings),
        args.threshold,
        control,
        progress_tx,
    )
    .with_signature_cache(Box::new(db_cache))
    .with_size_limits(args.min_size_kb * 1024, None)
    .with_threads(args.threads);

    let scan_paths: Vec<(PathBuf, bool)> = args
        .paths
        .iter()
        .map(|p| (p.clone(), args.recursive))
        .collect();
    let handle = std::thread::spawn(move || scanner.scan_paths(&scan_paths));

    use std::io::IsTerminal;
    let tty = std::io::stderr().is_terminal();
    let mut in_progress_line = false;
    for msg in &progress_rx {
        match msg {
            ScanProgress::Status(s) => {
                if in_progress_line {
                    eprintln!();
                    in_progress_line = false;
                }
                eprintln!("{}", s);
            }
            ScanProgress::Progress { .. } if !tty => {}
            ScanProgress::Progress {
                message,
                elapsed_secs,
                eta_secs,
                ..
            } => {
                let eta = eta_secs
                    .map(|e| format!(", ~{} left", format_duration(e)))
                    .unwrap_or_default();
                eprint!(
                    "\r\x1b[2K{} ({} elapsed{})",
                    message,
                    format_duration(elapsed_secs),
                    eta
                );
                let _ = std::io::stderr().flush();
                in_progress_line = true;
            }
            ScanProgress::PhaseCompleted { category, groups } => {
                if in_progress_line {
                    eprintln!();
                    in_progress_line = false;
                }
                eprintln!(
                    "  Found {} {} duplicate groups",
                    groups.len(),
                    category.label()
                );
                persist_groups(db, session_id, category, &groups)?;
            }
            ScanProgress::Completed => {}
            ScanProgress::Error(e) => eprintln!("\nError: {}", e),
        }
    }
    if in_progress_line {
        eprintln!();
    }

    let (results, new_sigs) = handle
        .join()
        .map_err(|_| anyhow::anyhow!("Scanner thread panicked"))?;
    db.update_session_status(session_id, "completed").ok();

    if !new_sigs.is_empty() {
        if let Err(e) = db.cache_new_signatures(&new_sigs) {
            eprintln!("Warning: could not cache signatures: {e}");
        }
    }

    let total_groups: usize = results.values().map(|g| g.len()).sum();
    let total_files: usize = results.values().flatten().map(|g| g.files.len()).sum();
    let reclaimable: u64 = results
        .values()
        .flatten()
        .map(|g| {
            let mut sizes: Vec<u64> = g
                .files
                .iter()
                .filter_map(|f| std::fs::metadata(f).ok().map(|m| m.len()))
                .collect();
            sizes.sort_unstable();
            sizes.iter().rev().skip(1).sum::<u64>()
        })
        .sum();
    if total_groups == 0 {
        println!("\nNo duplicates found (session {}).", session_id);
    } else {
        println!(
            "\nSession {}: {} groups, {} files, up to {} reclaimable.",
            session_id,
            total_groups,
            total_files,
            format_size(reclaimable)
        );
        println!("Next: `shingan list {0}` to review, `shingan delete {0} --dry-run` to preview cleanup.", session_id);
    }
    Ok(())
}

fn persist_groups(
    db: &Database,
    session_id: i64,
    category: FileCategory,
    groups: &[shingan_core::scanner::grouping::DuplicateGroup],
) -> anyhow::Result<()> {
    struct OwnedEntry {
        file_path: String,
        file_size: i64,
        modified_time: Option<String>,
    }
    let owned: Vec<(f64, Vec<OwnedEntry>)> = groups
        .iter()
        .map(|group| {
            let entries = group
                .files
                .iter()
                .map(|file_path| {
                    let meta = std::fs::metadata(file_path).ok();
                    OwnedEntry {
                        file_path: file_path.to_string_lossy().into_owned(),
                        file_size: meta.as_ref().map(|m| m.len() as i64).unwrap_or(0),
                        modified_time: meta
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs().to_string()),
                    }
                })
                .collect();
            (group.similarity, entries)
        })
        .collect();

    let batches: Vec<(f64, Vec<NewFileEntryBatch>)> = owned
        .iter()
        .map(|(sim, entries)| {
            let e = entries
                .iter()
                .map(|e| NewFileEntryBatch {
                    file_path: &e.file_path,
                    file_size: e.file_size,
                    modified_time: e.modified_time.as_deref(),
                    thumbnail_path: None,
                    file_metadata: None,
                })
                .collect();
            (*sim, e)
        })
        .collect();
    let slices: Vec<(&str, f64, Option<&str>, &[NewFileEntryBatch])> = batches
        .iter()
        .map(|(sim, entries)| (category.label(), *sim, None, entries.as_slice()))
        .collect();

    db.insert_duplicate_groups_batch(session_id, &slices)
        .context("Failed to save duplicate groups")?;
    Ok(())
}

fn cmd_list(db: &Database, session_id: Option<i64>) -> anyhow::Result<()> {
    match session_id {
        None => {
            let sessions = db.list_scan_sessions().context("Failed to list sessions")?;
            if sessions.is_empty() {
                println!("No scan sessions yet. Run `shingan scan <folder>` to create one.");
                return Ok(());
            }
            println!(
                "{:<6} {:<10} {:<20} {:>7} {:>7}  Name",
                "ID", "Status", "Created (UTC)", "Groups", "Files"
            );
            println!("{}", "-".repeat(78));
            for s in sessions {
                let (groups, files) = db.session_counts(s.id).unwrap_or((0, 0));
                println!(
                    "{:<6} {:<10} {:<20} {:>7} {:>7}  {}",
                    s.id, s.status, s.created_at, groups, files, s.name
                );
            }
        }
        Some(id) => {
            let session = db.get_scan_session(id).context("Session not found")?;
            println!(
                "Session {}: {} ({})",
                session.id, session.name, session.status
            );
            println!("Created: {} UTC", session.created_at);
            println!("Threshold: {:.0}%", session.similarity_threshold * 100.0);
            println!();

            let groups = db
                .get_duplicate_groups(id)
                .context("Failed to get groups")?;
            if groups.is_empty() {
                println!("No duplicate groups in this session.");
                return Ok(());
            }

            let mut buckets = [0u32; 4];
            for g in &groups {
                let pct = g.similarity_score * 100.0;
                let i = if pct >= 99.95 {
                    0
                } else if pct >= 95.0 {
                    1
                } else if pct >= 80.0 {
                    2
                } else {
                    3
                };
                buckets[i] += 1;
            }
            println!("Similarity distribution ({} groups):", groups.len());
            println!("  identical: {} groups", buckets[0]);
            println!("  95-100%:   {} groups", buckets[1]);
            println!("  80-95%:    {} groups", buckets[2]);
            if buckets[3] > 0 {
                println!("  <80%:      {} groups", buckets[3]);
            }
            println!();

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
                    let missing = if Path::new(&f.file_path).exists() {
                        ""
                    } else {
                        "  (missing)"
                    };
                    println!(
                        "    {} ({}){}",
                        f.file_path,
                        format_size(f.file_size.max(0) as u64),
                        missing
                    );
                }
            }
        }
    }

    Ok(())
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn cmd_export(
    db: &Database,
    session_id: i64,
    output: &Path,
    format: Option<ExportFormat>,
) -> anyhow::Result<()> {
    db.get_scan_session(session_id)
        .context("Session not found")?;
    let groups = db
        .get_duplicate_groups(session_id)
        .context("Failed to get groups")?;
    let format = format.unwrap_or_else(|| match output.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("json") => ExportFormat::Json,
        _ => ExportFormat::Csv,
    });

    let content = match format {
        ExportFormat::Json => {
            let mut json_groups = Vec::new();
            for group in &groups {
                let files = db.get_file_entries(group.id)?;
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
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": session_id,
                "groups": json_groups,
            }))?
        }
        ExportFormat::Csv => {
            let mut csv = String::from(
                "group_id,file_type,similarity,file_path,file_size_bytes,modified_time\n",
            );
            for group in &groups {
                for f in &db.get_file_entries(group.id)? {
                    csv.push_str(&format!(
                        "{},{},{:.4},{},{},{}\n",
                        group.id,
                        group.file_type,
                        group.similarity_score,
                        csv_field(&f.file_path),
                        f.file_size,
                        f.modified_time.as_deref().unwrap_or(""),
                    ));
                }
            }
            csv
        }
    };
    std::fs::write(output, content).context("Failed to write output file")?;

    println!(
        "Exported {} groups to {} ({})",
        groups.len(),
        output.display(),
        match format {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    );
    Ok(())
}

fn confirm(prompt: &str) -> bool {
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

fn cmd_delete(
    db: &Database,
    session_id: i64,
    keep: KeepStrategy,
    dry_run: bool,
    mode: RemoveMode,
    yes: bool,
) -> anyhow::Result<()> {
    db.get_scan_session(session_id)
        .context("Session not found")?;
    let groups = db.get_session_files(session_id)?;

    let mut to_remove: Vec<PathBuf> = Vec::new();
    let mut bytes = 0u64;
    for (group, entries) in &groups {
        let files: Vec<PathBuf> = entries
            .iter()
            .map(|e| PathBuf::from(&e.file_path))
            .collect();
        let Some(plan) = cleanup::plan_group(&files, keep) else {
            continue;
        };
        println!(
            "Group {} ({:.1}% similar) - keeping {}",
            group.id,
            group.similarity_score * 100.0,
            plan.keep.display()
        );
        for p in &plan.remove {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            bytes += size;
            println!("    remove {} ({})", p.display(), format_size(size));
        }
        to_remove.extend(plan.remove);
    }

    if to_remove.is_empty() {
        println!("Nothing to remove (fewer than two copies of each group still exist).");
        return Ok(());
    }

    let what = format!("{} files ({})", to_remove.len(), format_size(bytes));
    let action = match mode {
        RemoveMode::Trash => format!("move {what} to the trash"),
        RemoveMode::Permanent => format!("PERMANENTLY DELETE {what}"),
    };
    if dry_run {
        println!("\nDry run: would {action}. Nothing was changed.");
        return Ok(());
    }
    let mut prompt = action.clone();
    if let Some(first) = prompt.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    if !yes && !confirm(&format!("\n{prompt}?")) {
        println!("Cancelled.");
        return Ok(());
    }

    let report = cleanup::remove_files(&to_remove, mode);
    let removed: Vec<String> = report
        .removed
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let removed_refs: Vec<&str> = removed.iter().map(String::as_str).collect();
    db.remove_file_entries(session_id, &removed_refs).ok();

    for (p, e) in &report.failed {
        eprintln!("  failed: {} ({e})", p.display());
    }
    println!(
        "\nDone: {} files {}, {} freed, {} failed.",
        report.removed.len(),
        match mode {
            RemoveMode::Trash => "moved to the trash",
            RemoveMode::Permanent => "deleted",
        },
        format_size(report.bytes_freed),
        report.failed.len()
    );
    if !report.failed.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_sort(
    settings: &AppSettings,
    paths: Vec<PathBuf>,
    dest: PathBuf,
    classify: bool,
    cloud: bool,
    skip_unknown: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    for p in &paths {
        if !p.is_dir() {
            bail!("{} is not a folder", p.display());
        }
    }
    let cloud_categorizer = if cloud {
        let s = AppSettings {
            cloud_enabled: true,
            ..settings.clone()
        };
        let c = s
            .cloud_categorizer()
            .map_err(|e| anyhow::anyhow!("cloud classification unavailable: {e}"))?;
        eprintln!(
            "Using {} ({}) for uncertain images",
            settings.cloud_provider,
            s.effective_cloud_model()
        );
        c
    } else {
        None
    };

    let sorter = shingan_utils::auto_sorter::AutoSorter::new(paths, dest)
        .with_ml(classify)
        .with_pipeline_config(settings.pipeline_config())
        .with_cloud(cloud_categorizer)
        .with_include_unknown(!skip_unknown)
        .with_dry_run(dry_run);

    if dry_run {
        eprintln!("Dry run — no files will be moved.");
    }

    use std::io::IsTerminal;
    let tty = std::io::stderr().is_terminal();
    let stats = sorter.sort_files(
        Some(&|current, total, filepath| {
            if !tty {
                return;
            }
            let name = Path::new(filepath)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            eprint!("\r\x1b[2K[{}/{}] {}", current, total, name);
            let _ = std::io::stderr().flush();
        }),
        Some(&|msg| {
            if tty {
                eprint!("\r\x1b[2K");
            }
            eprintln!("{}", msg);
        }),
    );

    for e in &stats.errors {
        eprintln!("  {e}");
    }
    if let Some(j) = &stats.journal {
        println!(
            "Undo with: shingan undo-sort \"{}\"",
            j.parent().unwrap_or(j).display()
        );
    }
    if stats.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_undo_sort(target: &Path) -> anyhow::Result<()> {
    let journal = if target.is_dir() {
        shingan_utils::auto_sorter::latest_journal(target)
            .with_context(|| format!("no sort journal found in {}", target.display()))?
    } else {
        target.to_path_buf()
    };
    let stats = shingan_utils::auto_sorter::undo_sort(&journal).map_err(anyhow::Error::msg)?;
    for e in &stats.errors {
        eprintln!("  {e}");
    }
    println!(
        "Restored {} files ({} could not be restored).",
        stats.restored, stats.failed
    );
    Ok(())
}

fn cmd_model(settings: &AppSettings, action: ModelAction) -> anyhow::Result<()> {
    use shingan_ml::model_registry as reg;
    let dir = reg::resolve_models_dir(settings.ml_model_path.as_deref().unwrap_or(""));
    let model = &reg::DEFAULT_MODELS[0];
    match action {
        ModelAction::Status => {
            let installed = reg::model_installed(model, &dir);
            println!(
                "{}: {} ({})",
                model.name,
                if installed {
                    "installed"
                } else {
                    "not installed"
                },
                dir.display()
            );
        }
        ModelAction::Download => {
            let need_runtime = shingan_ml::onnx::ensure_runtime(&dir).is_err();
            println!(
                "Downloading {} ({}){} to {}",
                model.name,
                model.size_label,
                if need_runtime {
                    " and ONNX Runtime (~8 MB)"
                } else {
                    ""
                },
                dir.display()
            );
            let cancel = std::sync::atomic::AtomicBool::new(false);
            use std::io::IsTerminal;
            let tty = std::io::stderr().is_terminal();
            let last_pct = std::cell::Cell::new(u64::MAX);
            reg::download_model_blocking(
                model,
                &dir,
                need_runtime,
                &|name, _i, _n, done, total| {
                    // Without a terminal, print one line per 10%.
                    if !tty {
                        let pct = if total > 0 { done * 10 / total } else { 0 };
                        if pct != last_pct.get() {
                            last_pct.set(pct);
                            eprintln!("{name}: {}%", pct * 10);
                        }
                        return;
                    }
                    if total > 0 {
                        eprint!(
                            "\r\x1b[2K{name}: {} / {} ({:.0}%)",
                            format_size(done),
                            format_size(total),
                            done as f64 / total as f64 * 100.0
                        );
                    } else {
                        eprint!("\r\x1b[2K{name}: {}", format_size(done));
                    }
                    let _ = std::io::stderr().flush();
                },
                &cancel,
            )
            .map_err(anyhow::Error::msg)?;
            eprintln!();
            match shingan_ml::onnx::ClipOnnxClassifier::try_load(Some(&dir)) {
                Ok(Some(_)) => {
                    println!("Done. Use `shingan sort --classify` to sort images into sub-folders.")
                }
                Ok(None) => println!("Done, but the model file was not found afterwards."),
                Err(e) => println!("Downloaded, but the model can't run yet: {e}"),
            }
        }
        ModelAction::Remove => {
            reg::remove_model(model, &dir).map_err(anyhow::Error::msg)?;
            reg::remove_runtime(&dir);
            println!("Removed {}.", model.name);
        }
    }
    Ok(())
}

fn cmd_doctor(settings: &AppSettings) -> anyhow::Result<()> {
    use shingan_ml::model_registry as reg;
    let ok = |b: bool| if b { "ok     " } else { "missing" };
    let on_path = |cmd: &str| {
        std::process::Command::new(cmd)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    };

    println!("shingan {}", env!("CARGO_PKG_VERSION"));
    println!("  database:  {}", shingan_db::default_db_path().display());
    println!("  settings:  {}", settings::settings_path().display());
    println!();
    let dir = reg::resolve_models_dir(settings.ml_model_path.as_deref().unwrap_or(""));
    let model = &reg::DEFAULT_MODELS[0];
    let installed = reg::model_installed(model, &dir);
    println!(
        "  [{}] CLIP model       {} (optional; `shingan model download`)",
        ok(installed),
        dir.display()
    );
    match shingan_ml::onnx::ensure_runtime(&dir) {
        Ok(loc) => println!("  [ok     ] ONNX Runtime     {loc}"),
        Err(e) => println!("  [missing] ONNX Runtime     {e}"),
    }
    if installed {
        match shingan_ml::onnx::ClipOnnxClassifier::try_load(Some(&dir)) {
            Ok(Some(_)) => println!("  [ok     ] CLIP loads       image sub-categories available"),
            Ok(None) => {}
            Err(e) => println!("  [error  ] CLIP loads       {e}"),
        }
    }
    println!(
        "  [{}] ffmpeg           video thumbnails in the GUI preview",
        ok(on_path("ffmpeg"))
    );
    println!(
        "  [{}] pdftoppm         PDF page previews in the GUI (poppler-utils)",
        ok(on_path("pdftoppm"))
    );
    if settings.cloud_enabled {
        match settings.cloud_categorizer() {
            Ok(_) => println!(
                "  [ok     ] cloud            {} / {}",
                settings.cloud_provider,
                settings.effective_cloud_model()
            ),
            Err(e) => println!("  [error  ] cloud            {e}"),
        }
    } else {
        println!("  [off    ] cloud            disabled in settings");
    }
    Ok(())
}

fn format_duration(secs: f64) -> String {
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

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_accept_commas_plurals_and_all() {
        let c = parse_categories(&["images".into(), "document".into()]).unwrap();
        assert_eq!(c, vec![FileCategory::Image, FileCategory::Document]);
        assert_eq!(parse_categories(&["all".into()]).unwrap().len(), 5);
        assert!(parse_categories(&["spreadsheets".into()]).is_err());
        assert_eq!(
            parse_categories(&["docs".into()]).unwrap(),
            vec![FileCategory::Document]
        );
    }

    #[test]
    fn cli_parses_comma_separated_types() {
        let cli = Cli::try_parse_from(["shingan", "scan", ".", "-t", "image,video"]).unwrap();
        match cli.command {
            Commands::Scan { types, .. } => assert_eq!(types, vec!["image", "video"]),
            _ => panic!(),
        }
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0 MB");
    }
}
