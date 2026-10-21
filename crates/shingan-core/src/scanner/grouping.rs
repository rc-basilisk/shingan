use crate::detector::Detector;
use crate::scanner::duplicate::ScanControl;
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub files: Vec<PathBuf>,
    pub similarity: f64,
}

/// Progress callback: `(comparisons_done, comparisons_total)`. May be called from
/// several threads.
pub type GroupingProgress<'a> = Option<&'a (dyn Fn(usize, usize) + Sync)>;

/// Group files whose signatures are byte-identical.
pub fn find_exact_groups(file_signatures: &HashMap<PathBuf, String>) -> Vec<DuplicateGroup> {
    let mut sig_to_files: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    for (path, sig) in file_signatures {
        sig_to_files
            .entry(sig.as_str())
            .or_default()
            .push(path.clone());
    }

    let mut groups: Vec<DuplicateGroup> = sig_to_files
        .into_values()
        .filter(|files| files.len() >= 2)
        .map(|mut files| {
            files.sort();
            DuplicateGroup {
                files,
                similarity: 1.0,
            }
        })
        .collect();
    sort_groups(&mut groups);
    groups
}

/// Largest/most similar groups first, ties broken by path for stable output.
fn sort_groups(groups: &mut [DuplicateGroup]) {
    groups.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.files.first().cmp(&b.files.first()))
    });
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

    fn find(&mut self, mut x: usize) -> usize {
        // Iterative path halving: no recursion depth issues on huge inputs.
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) -> usize {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return ra;
        }
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => {
                self.parent[ra] = rb;
                rb
            }
            std::cmp::Ordering::Greater => {
                self.parent[rb] = ra;
                ra
            }
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
                ra
            }
        }
    }
}

/// Smallest key present in both sorted, de-duplicated key lists.
fn first_common_key(a: &[u64], b: &[u64]) -> Option<u64> {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Equal => return Some(a[i]),
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    None
}

/// Cluster near-duplicate files.
///
/// 1. Each signature is mapped to locality-sensitive bucket keys
///    ([`Detector::candidate_keys`]); only files sharing a bucket are compared.
/// 2. Candidate pairs are compared in parallel; pairs at or above `threshold`
///    are kept.
/// 3. Pairs are merged most-similar-first with a union-find, and a merge is only
///    accepted if *every* member of one cluster is similar to every member of
///    the other. This prevents chaining (A~B, B~C but A!~C).
pub fn find_fuzzy_groups(
    file_signatures: &HashMap<PathBuf, String>,
    detector: &dyn Detector,
    threshold: f64,
    progress: GroupingProgress<'_>,
    control: Option<&ScanControl>,
) -> Vec<DuplicateGroup> {
    // Sort for deterministic results regardless of HashMap iteration order.
    let mut entries: Vec<(&PathBuf, &str)> = file_signatures
        .iter()
        .map(|(p, s)| (p, s.as_str()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let paths: Vec<&PathBuf> = entries.iter().map(|e| e.0).collect();
    let sigs: Vec<&str> = entries.iter().map(|e| e.1).collect();
    let n = sigs.len();
    if n < 2 {
        return Vec::new();
    }

    let prepared = detector.prepare(&sigs);
    let keys: Vec<Vec<u64>> = sigs
        .par_iter()
        .map(|s| {
            let mut k = detector.candidate_keys(s, threshold);
            k.sort_unstable();
            k.dedup();
            k
        })
        .collect();

    let mut bucket_map: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, ks) in keys.iter().enumerate() {
        for &k in ks {
            bucket_map.entry(k).or_default().push(i);
        }
    }
    let mut buckets: Vec<(u64, Vec<usize>)> = bucket_map
        .into_iter()
        .filter(|(_, m)| m.len() >= 2)
        .collect();
    buckets.sort_by_key(|(k, _)| *k);

    let total: usize = buckets
        .iter()
        .map(|(_, m)| m.len() * (m.len() - 1) / 2)
        .sum();
    let done = AtomicUsize::new(0);
    let last_report = AtomicUsize::new(0);
    let report_step = (total / 500).max(1);
    let stopped = || control.is_some_and(|c| c.is_stopped());

    let mut similar_pairs: Vec<(usize, usize, f64)> = buckets
        .par_iter()
        .flat_map(|(key, members)| {
            let key = *key;
            let prepared = &prepared;
            let keys = &keys;
            let done = &done;
            let last_report = &last_report;
            (0..members.len()).into_par_iter().flat_map_iter(move |i| {
                let mut out = Vec::new();
                if stopped() {
                    return out;
                }
                let a = members[i];
                for &b in &members[i + 1..] {
                    // Only evaluate each pair in the first bucket the two share.
                    if keys[a].len() > 1 && first_common_key(&keys[a], &keys[b]) != Some(key) {
                        continue;
                    }
                    let sim = prepared.similarity(a, b);
                    if sim >= threshold {
                        out.push((a, b, sim));
                    }
                }
                let now = done.fetch_add(members.len() - i - 1, Ordering::Relaxed) + members.len()
                    - i
                    - 1;
                if let Some(cb) = progress {
                    let prev = last_report.load(Ordering::Relaxed);
                    if now >= prev + report_step
                        && last_report
                            .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
                            .is_ok()
                    {
                        cb(now, total);
                    }
                }
                out
            })
        })
        .collect();

    if stopped() {
        return Vec::new();
    }
    if let Some(cb) = progress {
        cb(total, total);
    }

    similar_pairs.sort_by(|x, y| {
        y.2.partial_cmp(&x.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then((x.0, x.1).cmp(&(y.0, y.1)))
    });

    let mut uf = UnionFind::new(n);
    let mut root_members: HashMap<usize, Vec<usize>> = HashMap::new();
    let members_of = |root_members: &HashMap<usize, Vec<usize>>, r: usize| -> Vec<usize> {
        root_members.get(&r).cloned().unwrap_or_else(|| vec![r])
    };

    for &(a, b, _) in &similar_pairs {
        let ra = uf.find(a);
        let rb = uf.find(b);
        if ra == rb {
            continue;
        }
        let set_a = members_of(&root_members, ra);
        let set_b = members_of(&root_members, rb);
        let all_similar = set_a.iter().all(|&x| {
            set_b
                .iter()
                .all(|&y| prepared.similarity(x, y) >= threshold)
        });
        if all_similar {
            root_members.remove(&ra);
            root_members.remove(&rb);
            let new_root = uf.union(a, b);
            let mut combined = set_a;
            combined.extend(set_b);
            root_members.insert(new_root, combined);
        }
    }

    let mut groups: Vec<DuplicateGroup> = root_members
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|mut members| {
            members.sort_unstable();
            let mut min_sim = 1.0f64;
            for (i, &x) in members.iter().enumerate() {
                for &y in &members[i + 1..] {
                    min_sim = min_sim.min(prepared.similarity(x, y));
                }
            }
            DuplicateGroup {
                files: members.iter().map(|&i| paths[i].clone()).collect(),
                similarity: min_sim,
            }
        })
        .collect();
    sort_groups(&mut groups);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::Detector;
    use crate::error::Result;
    use crate::file_info::FileCategory;
    use proptest::prelude::*;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

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

        fn candidate_keys(&self, _sig: &str, _threshold: f64) -> Vec<u64> {
            vec![0]
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
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_single_file() {
        let sigs = make_sigs(&[("a.png", "sig_a")]);
        let detector = MockDetector::new();
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_two_identical_files() {
        let sigs = make_sigs(&[("a.png", "sig_x"), ("b.png", "sig_x")]);
        let detector = MockDetector::new();
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
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
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
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
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
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
        let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
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
            let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
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
            let groups = find_fuzzy_groups(&sigs, &detector, 0.9, None, None);
            // All files should be in a single group
            prop_assert_eq!(groups.len(), 1);
            prop_assert_eq!(groups[0].files.len(), n);
        }
    }
}
