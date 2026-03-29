use crate::detector::Detector;
use std::collections::HashMap;
use std::path::PathBuf;

/// A group of duplicate files with their similarity score.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub files: Vec<PathBuf>,
    pub similarity: f64,
}

/// Result of the grouping phase.
pub struct GroupingResult {
    pub exact_groups: Vec<DuplicateGroup>,
    pub fuzzy_groups: Vec<DuplicateGroup>,
}

/// Find exact duplicate groups (identical signatures).
pub fn find_exact_groups(
    file_signatures: &HashMap<PathBuf, String>,
) -> Vec<DuplicateGroup> {
    // Group files by identical signature
    let mut sig_to_files: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    for (path, sig) in file_signatures {
        sig_to_files
            .entry(sig.as_str())
            .or_default()
            .push(path.clone());
    }

    sig_to_files
        .into_values()
        .filter(|files| files.len() >= 2)
        .map(|files| DuplicateGroup {
            files,
            similarity: 1.0,
        })
        .collect()
}

/// Find fuzzy duplicate groups using prefix-based locality-sensitive hashing.
///
/// Uses strict group membership: a file pair only joins an existing group if
/// BOTH files match ALL existing group members above the threshold.
pub fn find_fuzzy_groups(
    file_signatures: &HashMap<PathBuf, String>,
    detector: &dyn Detector,
    threshold: f64,
    prefix_len: usize,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Vec<DuplicateGroup> {
    // Build prefix groups for LSH
    let mut prefix_groups: HashMap<String, Vec<(PathBuf, String)>> = HashMap::new();
    for (path, sig) in file_signatures {
        let prefix = if sig.len() >= prefix_len {
            &sig[..prefix_len]
        } else {
            sig.as_str()
        };
        prefix_groups
            .entry(prefix.to_string())
            .or_default()
            .push((path.clone(), sig.clone()));
    }

    // Calculate total comparisons for progress reporting
    let total_comparisons: usize = prefix_groups
        .values()
        .map(|group| group.len() * (group.len().saturating_sub(1)) / 2)
        .sum();

    let mut groups: Vec<DuplicateGroup> = Vec::new();
    let mut processed_pairs: std::collections::HashSet<(PathBuf, PathBuf)> = Default::default();
    let mut comparisons_done: usize = 0;

    for prefix_group in prefix_groups.values() {
        if prefix_group.len() < 2 {
            continue;
        }

        for i in 0..prefix_group.len() {
            for j in (i + 1)..prefix_group.len() {
                let (path1, sig1) = &prefix_group[i];
                let (path2, sig2) = &prefix_group[j];

                // Deduplicate pairs
                let pair = if path1 < path2 {
                    (path1.clone(), path2.clone())
                } else {
                    (path2.clone(), path1.clone())
                };
                if processed_pairs.contains(&pair) {
                    comparisons_done += 1;
                    continue;
                }
                processed_pairs.insert(pair);

                let similarity = detector.compare_signatures(sig1, sig2);
                comparisons_done += 1;

                if let Some(cb) = progress {
                    if comparisons_done % 100 == 0 {
                        cb(comparisons_done, total_comparisons);
                    }
                }

                if similarity < threshold {
                    continue;
                }

                // Strict group membership: find a group where BOTH files match
                // all existing members
                let mut found_group = false;
                for group in groups.iter_mut() {
                    let both_match = group.files.iter().all(|existing| {
                        let existing_sig = file_signatures.get(existing).unwrap();
                        let sim1 = detector.compare_signatures(sig1, existing_sig);
                        let sim2 = detector.compare_signatures(sig2, existing_sig);
                        sim1 >= threshold && sim2 >= threshold
                    });

                    if both_match {
                        if !group.files.contains(path1) {
                            group.files.push(path1.clone());
                        }
                        if !group.files.contains(path2) {
                            group.files.push(path2.clone());
                        }
                        // Update similarity to minimum
                        group.similarity = group.similarity.min(similarity);
                        found_group = true;
                        break;
                    }
                }

                if !found_group {
                    groups.push(DuplicateGroup {
                        files: vec![path1.clone(), path2.clone()],
                        similarity,
                    });
                }
            }
        }
    }

    groups
}
