//! Safe removal of duplicate files.
//!
//! Both the CLI and the GUI go through this module so they share the same
//! safety rules:
//!
//! - Files are moved to the operating system's trash/recycle bin by default;
//!   permanent deletion has to be requested explicitly.
//! - At least one existing copy of every group is always kept, even if the
//!   caller selected every file in it.
//! - Files that have disappeared since the scan are ignored (and never chosen
//!   as the "kept" copy, which would otherwise delete the only real copy).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Which copy to keep in each duplicate group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepStrategy {
    /// Keep the most recently modified file.
    Newest,
    /// Keep the least recently modified file (usually the original).
    Oldest,
    /// Keep the largest file (often the highest quality).
    Largest,
    /// Keep the file with the shortest path (avoids "Copy of ..." and deep backups).
    ShortestPath,
}

impl KeepStrategy {
    pub const ALL: [KeepStrategy; 4] = [
        KeepStrategy::Newest,
        KeepStrategy::Oldest,
        KeepStrategy::Largest,
        KeepStrategy::ShortestPath,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Largest => "largest",
            Self::ShortestPath => "shortest-path",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "newest" => Some(Self::Newest),
            "oldest" => Some(Self::Oldest),
            "largest" => Some(Self::Largest),
            "shortest-path" | "shortest" => Some(Self::ShortestPath),
            _ => None,
        }
    }
}

impl std::fmt::Display for KeepStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Newest => "Keep newest",
            Self::Oldest => "Keep oldest",
            Self::Largest => "Keep largest",
            Self::ShortestPath => "Keep shortest path",
        })
    }
}

/// How files are removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveMode {
    /// Move to the system trash / recycle bin (recoverable).
    Trash,
    /// Delete permanently (not recoverable).
    Permanent,
}

/// The decision for one duplicate group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    pub keep: PathBuf,
    pub remove: Vec<PathBuf>,
}

struct Candidate {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

fn existing(files: &[PathBuf]) -> Vec<Candidate> {
    files
        .iter()
        .filter_map(|p| {
            let meta = std::fs::metadata(p).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some(Candidate {
                path: p.clone(),
                size: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect()
}

/// Decide which file of a group to keep. Returns `None` when fewer than two of
/// the files still exist (nothing to do). Ties are broken by path so results
/// are deterministic.
pub fn plan_group(files: &[PathBuf], keep: KeepStrategy) -> Option<GroupPlan> {
    let mut cands = existing(files);
    if cands.len() < 2 {
        return None;
    }
    cands.sort_by(|a, b| a.path.cmp(&b.path));
    let keep_idx = match keep {
        KeepStrategy::Newest => index_of_best(&cands, |a, b| a.modified > b.modified),
        KeepStrategy::Oldest => index_of_best(&cands, |a, b| a.modified < b.modified),
        KeepStrategy::Largest => index_of_best(&cands, |a, b| a.size > b.size),
        KeepStrategy::ShortestPath => index_of_best(&cands, |a, b| {
            a.path.as_os_str().len() < b.path.as_os_str().len()
        }),
    };
    let keep_path = cands[keep_idx].path.clone();
    let remove = cands
        .into_iter()
        .map(|c| c.path)
        .filter(|p| *p != keep_path)
        .collect();
    Some(GroupPlan {
        keep: keep_path,
        remove,
    })
}

/// Index of the first candidate that no other candidate strictly beats.
fn index_of_best(cands: &[Candidate], better: impl Fn(&Candidate, &Candidate) -> bool) -> usize {
    let mut best = 0;
    for i in 1..cands.len() {
        if better(&cands[i], &cands[best]) {
            best = i;
        }
    }
    best
}

/// Trim a user selection so that at least one existing file per group survives.
///
/// Returns the paths that are safe to remove and the number of groups where the
/// selection had to be reduced (every remaining copy was selected).
pub fn protect_last_copies<'a>(
    groups: impl IntoIterator<Item = &'a [PathBuf]>,
    selected: &std::collections::HashSet<PathBuf>,
) -> (Vec<PathBuf>, usize) {
    let mut safe = Vec::new();
    let mut protected = 0;
    for files in groups {
        let alive: Vec<&PathBuf> = files.iter().filter(|p| p.is_file()).collect();
        let chosen: Vec<&PathBuf> = alive
            .iter()
            .copied()
            .filter(|p| selected.contains(*p))
            .collect();
        if chosen.is_empty() {
            continue;
        }
        if chosen.len() == alive.len() {
            // Keep the first one (paths in a group are sorted).
            protected += 1;
            safe.extend(chosen.into_iter().skip(1).cloned());
        } else {
            safe.extend(chosen.into_iter().cloned());
        }
    }
    safe.sort();
    safe.dedup();
    (safe, protected)
}

/// Outcome of removing a batch of files.
#[derive(Debug, Default, Clone)]
pub struct RemoveReport {
    pub removed: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub bytes_freed: u64,
}

/// Remove one file according to `mode`.
pub fn remove_file(path: &Path, mode: RemoveMode) -> Result<(), String> {
    match mode {
        RemoveMode::Trash => trash::delete(path).map_err(|e| {
            format!(
                "could not move to trash ({e}); use permanent deletion if this system has no trash"
            )
        }),
        RemoveMode::Permanent => std::fs::remove_file(path).map_err(|e| e.to_string()),
    }
}

/// Remove many files, collecting successes and failures.
pub fn remove_files(paths: &[PathBuf], mode: RemoveMode) -> RemoveReport {
    let mut report = RemoveReport::default();
    for p in paths {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        match remove_file(p, mode) {
            Ok(()) => {
                report.bytes_freed += size;
                report.removed.push(p.clone());
            }
            Err(e) => report.failed.push((p.clone(), e)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn touch(dir: &Path, name: &str, bytes: usize) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![0u8; bytes]).unwrap();
        p
    }

    #[test]
    fn missing_files_are_never_kept() {
        let dir = tempfile::tempdir().unwrap();
        let real = touch(dir.path(), "real.jpg", 10);
        let gone = dir.path().join("gone.jpg");
        // Only one copy exists: nothing must be deleted, whatever the strategy.
        for k in KeepStrategy::ALL {
            assert!(plan_group(&[gone.clone(), real.clone()], k).is_none());
        }
    }

    #[test]
    fn largest_and_shortest_path() {
        let dir = tempfile::tempdir().unwrap();
        let small = touch(dir.path(), "a_much_longer_name.jpg", 10);
        let big = touch(dir.path(), "b.jpg", 20);
        let plan = plan_group(&[small.clone(), big.clone()], KeepStrategy::Largest).unwrap();
        assert_eq!(plan.keep, big);
        assert_eq!(plan.remove, vec![small.clone()]);
        let plan = plan_group(&[small.clone(), big.clone()], KeepStrategy::ShortestPath).unwrap();
        assert_eq!(plan.keep, big);
    }

    #[test]
    fn selecting_every_copy_keeps_one() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a", 1);
        let b = touch(dir.path(), "b", 1);
        let c = touch(dir.path(), "c", 1);
        let d = touch(dir.path(), "d", 1);
        let g1 = vec![a.clone(), b.clone()];
        let g2 = vec![c.clone(), d.clone()];
        let selected: HashSet<PathBuf> = [a.clone(), b.clone(), d.clone()].into();
        let (safe, protected) = protect_last_copies([g1.as_slice(), g2.as_slice()], &selected);
        assert_eq!(protected, 1);
        assert_eq!(safe, vec![b, d]);
    }

    #[test]
    fn permanent_removal_reports() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a", 5);
        let missing = dir.path().join("missing");
        let report = remove_files(&[a.clone(), missing.clone()], RemoveMode::Permanent);
        assert_eq!(report.removed, vec![a.clone()]);
        assert_eq!(report.bytes_freed, 5);
        assert_eq!(report.failed.len(), 1);
        assert!(!a.exists());
    }

    #[test]
    fn strategy_parse_round_trip() {
        for k in KeepStrategy::ALL {
            assert_eq!(KeepStrategy::parse(k.label()), Some(k));
        }
        assert_eq!(KeepStrategy::parse("bogus"), None);
    }
}
