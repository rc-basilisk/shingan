//! MinHash signatures for fuzzy text comparison.
//!
//! Documents and source files are reduced to a set of overlapping token
//! "shingles"; the MinHash of that set is a fixed-size sketch whose agreement
//! rate estimates the Jaccard similarity of the two sets. This lets the scanner
//! compare texts from cached signatures alone, without re-reading files, and
//! enables LSH banding so only plausible pairs are compared.
//!
//! The similarity reported is the Sorensen-Dice coefficient of the shingle sets
//! (`2J / (1 + J)`), which is a little more forgiving than raw Jaccard and
//! matches the coefficient shingan has always advertised.

use super::{mix64, stable_hash64, PreparedSignatures};

/// Number of hash functions in a signature.
pub const NUM_HASHES: usize = 128;
/// LSH banding: `BANDS * ROWS == NUM_HASHES`. 32 bands of 4 rows gives a
/// ~50% chance of becoming a candidate at Jaccard 0.42 and >99.9% at 0.8.
const BANDS: usize = 32;
const ROWS: usize = NUM_HASHES / BANDS;
/// Signature format tag, bumped whenever the hashing scheme changes.
const PREFIX: &str = "mh1:";

/// Build a MinHash signature string from a token stream using `k`-token shingles.
/// Returns `None` when there are no tokens at all.
pub fn signature_from_tokens<'a, I>(tokens: I, k: usize) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let token_hashes: Vec<u64> = tokens
        .into_iter()
        .map(|t| stable_hash64(t.as_bytes()))
        .collect();
    if token_hashes.is_empty() {
        return None;
    }
    let k = k.clamp(1, token_hashes.len());

    let mut mins = [u64::MAX; NUM_HASHES];
    for window in token_hashes.windows(k) {
        let mut shingle = 0u64;
        for &h in window {
            shingle = mix64(shingle ^ h);
        }
        for (i, min) in mins.iter_mut().enumerate() {
            let v = mix64(shingle ^ SEEDS[i]);
            if v < *min {
                *min = v;
            }
        }
    }

    let mut out = String::with_capacity(PREFIX.len() + NUM_HASHES * 16);
    out.push_str(PREFIX);
    for m in mins {
        out.push_str(&format!("{m:016x}"));
    }
    Some(out)
}

/// Parse a signature string produced by [`signature_from_tokens`].
pub fn parse(sig: &str) -> Option<[u64; NUM_HASHES]> {
    let hex = sig.strip_prefix(PREFIX)?;
    if hex.len() != NUM_HASHES * 16 {
        return None;
    }
    let mut out = [0u64; NUM_HASHES];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u64::from_str_radix(&hex[i * 16..(i + 1) * 16], 16).ok()?;
    }
    Some(out)
}

/// Dice-equivalent similarity of two parsed signatures.
pub fn similarity(a: &[u64; NUM_HASHES], b: &[u64; NUM_HASHES]) -> f64 {
    let matches = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    let j = matches as f64 / NUM_HASHES as f64;
    2.0 * j / (1.0 + j)
}

/// Compare two signature strings; unparsable signatures score 0.
pub fn compare(sig1: &str, sig2: &str) -> f64 {
    match (parse(sig1), parse(sig2)) {
        (Some(a), Some(b)) => similarity(&a, &b),
        _ => 0.0,
    }
}

/// LSH band keys for a signature string.
pub fn band_keys(sig: &str) -> Vec<u64> {
    let Some(mins) = parse(sig) else {
        return vec![stable_hash64(sig.as_bytes())];
    };
    (0..BANDS)
        .map(|band| {
            let mut h = mix64(band as u64);
            for v in &mins[band * ROWS..(band + 1) * ROWS] {
                h = mix64(h ^ v);
            }
            h
        })
        .collect()
}

/// Pre-parsed MinHash signatures for fast pairwise comparison.
pub struct PreparedMinHash {
    parsed: Vec<Option<[u64; NUM_HASHES]>>,
}

impl PreparedMinHash {
    pub fn new(sigs: &[&str]) -> Self {
        Self {
            parsed: sigs.iter().map(|s| parse(s)).collect(),
        }
    }
}

impl PreparedSignatures for PreparedMinHash {
    fn similarity(&self, a: usize, b: usize) -> f64 {
        match (&self.parsed[a], &self.parsed[b]) {
            (Some(x), Some(y)) => similarity(x, y),
            _ => 0.0,
        }
    }
}

/// Fixed per-hash seeds (generated from SplitMix64 so they are reproducible).
const SEEDS: [u64; NUM_HASHES] = {
    let mut seeds = [0u64; NUM_HASHES];
    let mut state: u64 = 0x5348_494e_4741_4e00; // "SHINGAN\0"
    let mut i = 0;
    while i < NUM_HASHES {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        seeds[i] = z ^ (z >> 31);
        i += 1;
    }
    seeds
};

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(text: &str) -> String {
        signature_from_tokens(text.split_whitespace(), 3).unwrap()
    }

    #[test]
    fn identical_texts_score_one() {
        let s = sig("the quick brown fox jumps over the lazy dog");
        assert!((compare(&s, &s) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn near_duplicates_score_high_and_share_a_band() {
        let base: String = (0..400).map(|i| format!("word{} ", i % 97 + i)).collect();
        let edited = base.replacen("word5 ", "changed ", 1);
        let (a, b) = (sig(&base), sig(&edited));
        assert!(compare(&a, &b) > 0.9, "got {}", compare(&a, &b));
        let ka = band_keys(&a);
        let kb = band_keys(&b);
        assert!(ka.iter().any(|k| kb.contains(k)));
    }

    #[test]
    fn unrelated_texts_score_low() {
        let a = sig(&(0..300).map(|i| format!("alpha{i} ")).collect::<String>());
        let b = sig(&(0..300).map(|i| format!("beta{i} ")).collect::<String>());
        assert!(compare(&a, &b) < 0.1);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse("deadbeef").is_none());
        assert!(parse("mh1:zz").is_none());
        assert_eq!(compare("x", "y"), 0.0);
    }

    #[test]
    fn short_inputs_still_produce_signatures() {
        assert!(signature_from_tokens(["one"], 3).is_some());
        assert!(signature_from_tokens(std::iter::empty(), 3).is_none());
    }
}
