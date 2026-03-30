use crate::detector::Detector;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub files: Vec<PathBuf>,
    pub similarity: f64,
}

pub fn find_exact_groups(file_signatures: &HashMap<PathBuf, String>) -> Vec<DuplicateGroup> {
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

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
            self.parent[x]
        } else {
            x
        }
    }

    fn union(&mut self, a: usize, b: usize) -> bool {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return false;
        }
        if self.rank[ra] < self.rank[rb] {
            self.parent[ra] = rb;
        } else if self.rank[ra] > self.rank[rb] {
            self.parent[rb] = ra;
        } else {
            self.parent[rb] = ra;
            self.rank[ra] += 1;
        }
        true
    }
}

pub fn find_fuzzy_groups(
    file_signatures: &HashMap<PathBuf, String>,
    detector: &dyn Detector,
    threshold: f64,
    prefix_len: usize,
    progress: Option<&dyn Fn(usize, usize)>,
) -> Vec<DuplicateGroup> {
    let indexed: Vec<&PathBuf> = file_signatures.keys().collect();

    let mut prefix_buckets: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, path) in indexed.iter().enumerate() {
        let sig = &file_signatures[*path];
        let prefix = if sig.len() >= prefix_len {
            &sig[..prefix_len]
        } else {
            sig.as_str()
        };
        prefix_buckets.entry(prefix).or_default().push(i);
    }

    let total_comparisons: usize = prefix_buckets
        .values()
        .map(|b| b.len() * (b.len().saturating_sub(1)) / 2)
        .sum();

    let mut seen_pairs: HashSet<(usize, usize)> = HashSet::new();
    let mut similar_pairs: Vec<(usize, usize, f64)> = Vec::new();
    let mut comparisons_done: usize = 0;

    for bucket in prefix_buckets.values() {
        if bucket.len() < 2 {
            continue;
        }
        for i in 0..bucket.len() {
            for j in (i + 1)..bucket.len() {
                let (a, b) = if bucket[i] < bucket[j] {
                    (bucket[i], bucket[j])
                } else {
                    (bucket[j], bucket[i])
                };
                if !seen_pairs.insert((a, b)) {
                    comparisons_done += 1;
                    continue;
                }

                let sig_a = &file_signatures[indexed[a]];
                let sig_b = &file_signatures[indexed[b]];
                let similarity = detector.compare_signatures(sig_a, sig_b);
                comparisons_done += 1;

                if let Some(cb) = progress {
                    if comparisons_done.is_multiple_of(100) {
                        cb(comparisons_done, total_comparisons);
                    }
                }

                if similarity >= threshold {
                    similar_pairs.push((a, b, similarity));
                }
            }
        }
    }

    similar_pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    let n = indexed.len();
    let mut uf = UnionFind::new(n);
    // Maintain root -> members map incrementally to avoid O(n) scans per pair
    let mut root_members: HashMap<usize, Vec<usize>> = (0..n).map(|i| (i, vec![i])).collect();

    for (a, b, _sim) in &similar_pairs {
        let ra = uf.find(*a);
        let rb = uf.find(*b);
        if ra == rb {
            continue;
        }

        let set_a = &root_members[&ra];
        let set_b = &root_members[&rb];

        let mut all_similar = true;
        'outer: for &member_a in set_a {
            let sig_ma = &file_signatures[indexed[member_a]];
            for &member_b in set_b {
                let sig_mb = &file_signatures[indexed[member_b]];
                if detector.compare_signatures(sig_ma, sig_mb) < threshold {
                    all_similar = false;
                    break 'outer;
                }
            }
        }

        if all_similar {
            // Merge the smaller set into the larger one in the members map
            let (keep_root, merge_root) = if root_members[&ra].len() >= root_members[&rb].len() {
                (ra, rb)
            } else {
                (rb, ra)
            };
            uf.union(*a, *b);
            let new_root = uf.find(*a);
            let merged = root_members.remove(&merge_root).unwrap();
            let keep = root_members.remove(&keep_root).unwrap();
            let mut combined = keep;
            combined.extend(merged);
            root_members.insert(new_root, combined);
        }
    }

    root_members
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|members| {
            let files: Vec<PathBuf> = members.iter().map(|&i| indexed[i].clone()).collect();
            let mut min_sim = f64::MAX;
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let sig_a = &file_signatures[indexed[members[i]]];
                    let sig_b = &file_signatures[indexed[members[j]]];
                    let sim = detector.compare_signatures(sig_a, sig_b);
                    min_sim = min_sim.min(sim);
                }
            }
            DuplicateGroup {
                files,
                similarity: min_sim,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::Detector;
    use crate::error::Result;
    use crate::file_info::FileCategory;
    use std::collections::HashMap;
    use std::path::Path;
    use proptest::prelude::*;

    struct MockDetector {
        similarities: HashMap<(String, String), f64>,
    }

    impl MockDetector {
        fn new() -> Self {
            Self {
                similarities: HashMap::new(),
            }
        }

        fn set_similar(&mut self, sig_a: &str, sig_b: &str, sim: f64) {
            self.similarities
                .insert((sig_a.to_string(), sig_b.to_string()), sim);
            self.similarities
                .insert((sig_b.to_string(), sig_a.to_string()), sim);
        }
    }

    impl Detector for MockDetector {
        fn compute_signature(&self, _path: &Path) -> Result<Option<String>> {
            Ok(None)
        }

        fn compare_signatures(&self, sig1: &str, sig2: &str) -> f64 {
            if sig1 == sig2 {
                return 1.0;
            }
            self.similarities
                .get(&(sig1.to_string(), sig2.to_string()))
                .copied()
                .unwrap_or(0.0)
        }

        fn compare_files(&self, _file1: &Path, _file2: &Path) -> Result<f64> {
            Ok(0.0)
        }

        fn category(&self) -> FileCategory {
            FileCategory::Image
        }
        fn threshold(&self) -> f64 {
            0.9
        }
    }

    fn make_sigs(pairs: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        pairs
            .iter()
            .map(|(path, sig)| (PathBuf::from(path), sig.to_string()))
            .collect()
    }

    #[test]
    fn test_empty_input() {
        let sigs = HashMap::new();
        let detector = MockDetector::new();
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_single_file() {
        let sigs = make_sigs(&[("a.png", "sig_a")]);
        let detector = MockDetector::new();
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_two_identical_files() {
        let sigs = make_sigs(&[("a.png", "sig_x"), ("b.png", "sig_x")]);
        let detector = MockDetector::new();
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 2);
        assert!((groups[0].similarity - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_multiple_distinct_groups() {
        let sigs = make_sigs(&[
            ("a.png", "sig_a"),
            ("b.png", "sig_b"),
            ("c.png", "sig_c"),
            ("d.png", "sig_d"),
        ]);
        let mut detector = MockDetector::new();
        detector.set_similar("sig_a", "sig_b", 0.95);
        detector.set_similar("sig_c", "sig_d", 0.95);
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
        assert_eq!(groups.len(), 2);
        for g in &groups {
            assert_eq!(g.files.len(), 2);
        }
    }

    #[test]
    fn test_strict_membership() {
        let sigs = make_sigs(&[("a.png", "sig_a"), ("b.png", "sig_b"), ("c.png", "sig_c")]);
        let mut detector = MockDetector::new();
        detector.set_similar("sig_a", "sig_b", 0.95);
        detector.set_similar("sig_b", "sig_c", 0.95);
        detector.set_similar("sig_a", "sig_c", 0.5);
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 3, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].files.len(), 2);
        let paths: Vec<String> = groups[0]
            .files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(paths.len(), 2);
        let has_ab = paths.contains(&"a.png".to_string()) && paths.contains(&"b.png".to_string());
        let has_bc = paths.contains(&"b.png".to_string()) && paths.contains(&"c.png".to_string());
        assert!(has_ab || has_bc);
    }

    #[test]
    fn test_below_threshold() {
        let sigs = make_sigs(&[("a.png", "sig_a"), ("b.png", "sig_b")]);
        let mut detector = MockDetector::new();
        detector.set_similar("sig_a", "sig_b", 0.5);
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_exact_groups() {
        let sigs = make_sigs(&[
            ("a.png", "hash1"),
            ("b.png", "hash1"),
            ("c.png", "hash2"),
            ("d.png", "hash2"),
            ("e.png", "hash3"),
        ]);
        let groups = find_exact_groups(&sigs);
        assert_eq!(groups.len(), 2);
        for g in &groups {
            assert_eq!(g.files.len(), 2);
            assert!((g.similarity - 1.0).abs() < f64::EPSILON);
        }
    }

    // -- Property-based tests for union-find grouping invariants --

    proptest! {
        /// Every file must appear in at most one group (no orphan duplication).
        #[test]
        fn prop_no_file_in_multiple_groups(n in 2..20usize) {
            let pairs: Vec<(String, String)> = (0..n)
                .map(|i| (format!("{}.png", i), format!("sig_{:04}", i)))
                .collect();
            let sigs: HashMap<PathBuf, String> = pairs
                .iter()
                .map(|(p, s)| (PathBuf::from(p), s.clone()))
                .collect();
            let groups = find_exact_groups(&sigs);
            let mut seen = HashSet::new();
            for group in &groups {
                for file in &group.files {
                    prop_assert!(
                        seen.insert(file.clone()),
                        "File {:?} appeared in multiple groups",
                        file
                    );
                }
            }
        }

        /// Every group must have >= 2 members.
        #[test]
        fn prop_groups_have_at_least_two(
            n in 2..15usize,
            num_sigs in 1..5usize,
        ) {
            // Create n files with num_sigs distinct signatures (so some will share)
            let pairs: Vec<(String, String)> = (0..n)
                .map(|i| (format!("{}.png", i), format!("sig_{:04}", i % num_sigs)))
                .collect();
            let sigs: HashMap<PathBuf, String> = pairs
                .iter()
                .map(|(p, s)| (PathBuf::from(p), s.clone()))
                .collect();
            let groups = find_exact_groups(&sigs);
            for group in &groups {
                prop_assert!(
                    group.files.len() >= 2,
                    "Group has {} files, expected >= 2",
                    group.files.len()
                );
            }
        }

        /// Similarity scores are within [0.0, 1.0].
        #[test]
        fn prop_similarity_bounded(
            n in 2..10usize,
            num_sigs in 1..4usize,
        ) {
            let pairs: Vec<(String, String)> = (0..n)
                .map(|i| (format!("{}.png", i), format!("sig_{:04}", i % num_sigs)))
                .collect();
            let sigs: HashMap<PathBuf, String> = pairs
                .iter()
                .map(|(p, s)| (PathBuf::from(p), s.clone()))
                .collect();
            let detector = MockDetector::new();
            let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
            for group in &groups {
                prop_assert!(
                    group.similarity >= 0.0 && group.similarity <= 1.0,
                    "Similarity {} out of bounds",
                    group.similarity
                );
            }
        }

        /// Union-find transitivity: if A and B are in the same group, and B and C are in the
        /// same group, then A and C must be in the same group.
        #[test]
        fn prop_transitive_groups(n in 3..12usize) {
            // All files share the same signature => one big group
            let pairs: Vec<(String, String)> = (0..n)
                .map(|i| (format!("{}.png", i), "same_sig".to_string()))
                .collect();
            let sigs: HashMap<PathBuf, String> = pairs
                .iter()
                .map(|(p, s)| (PathBuf::from(p), s.clone()))
                .collect();
            let detector = MockDetector::new();
            let groups = find_fuzzy_groups(&sigs, &detector, 0.9, 2, None);
            // All files should be in a single group
            prop_assert_eq!(groups.len(), 1);
            prop_assert_eq!(groups[0].files.len(), n);
        }
    }
}
