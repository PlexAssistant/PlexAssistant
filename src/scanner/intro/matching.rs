//! Aligning two fingerprints to find the audio they share.

/// Chromaprint produces one 32-bit entry per ~0.1238 seconds at the
/// default settings; the exact step depends on the sample rate.
const FINGERPRINT_STEP_SECS: f64 = 0.1238;

/// Minimum intro duration for a marker (Plex honors ≥15s).
const MIN_INTRO_SECS: f64 = 15.0;

/// Bit similarity threshold: two entries match when enough of their
/// 32 bits agree. 0.75 works well for chromaprint's perceptual hashing.
const SIMILARITY_THRESHOLD: f64 = 0.75;

/// One matching segment between two fingerprints, with the offset
/// preserved so each episode gets its own intro times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// offset in entries: B's match starts this many entries after A's
    pub offset: i32,
    /// match start in A's timeline (seconds)
    pub start_a: f64,
    /// match end in A's timeline
    pub end_a: f64,
    /// match start in B's timeline
    pub start_b: f64,
    /// match end in B's timeline
    pub end_b: f64,
    /// average bit similarity across the segment
    pub score: f64,
}

impl Match {
    pub fn duration(&self) -> f64 {
        self.end_a - self.start_a
    }
}

/// Bit similarity between two chromaprint entries: the fraction of the
/// 32 bits that agree.
fn similarity(a: u32, b: u32) -> f64 {
    let matching_bits = 32 - (a ^ b).count_ones();
    matching_bits as f64 / 32.0
}

/// The longest matching run between two score slices.
fn longest_run(scores: &[f64]) -> Option<(usize, usize, f64)> {
    let mut best: Option<(usize, usize, f64)> = None;
    let mut run_start = 0;
    let mut run_sum = 0.0;
    let mut run_len = 0usize;

    let better = |best: &Option<(usize, usize, f64)>, len: usize| {
        best.is_none_or(|(bs, be, _)| len > be - bs)
    };

    for (i, &s) in scores.iter().enumerate() {
        if s >= SIMILARITY_THRESHOLD {
            if run_len == 0 {
                run_start = i;
            }
            run_sum += s;
            run_len += 1;
        } else if run_len > 0 {
            let avg = run_sum / run_len as f64;
            if better(&best, run_len) {
                best = Some((run_start, i, avg));
            }
            run_sum = 0.0;
            run_len = 0;
        }
    }
    // trailing run
    if run_len > 0 {
        let avg = run_sum / run_len as f64;
        if better(&best, run_len) {
            best = Some((run_start, scores.len(), avg));
        }
    }
    best
}

/// Finds the best-aligned matching segment between two fingerprints by
/// sliding one over the other. Each episode gets its own intro times
/// (offset by the alignment), so shows whose intros start at different
/// positions per episode are handled.
///
/// The search covers offsets within ±SEARCH_ENTRIES (≈60 seconds at
/// the default step). For 120-second fingerprints this is ~970 × 970
/// comparisons — about a million popcounts, well under a millisecond.
const SEARCH_ENTRIES: i32 = 484; // ≈60 seconds

pub fn find_common_intro(a: &[u32], b: &[u32]) -> Option<Match> {
    if a.len() < 10 || b.len() < 10 {
        return None;
    }

    let mut best: Option<Match> = None;

    // try offset 0 first (the common case: intro at the head in both)
    if let Some(m) = try_offset(a, b, 0) {
        best = Some(m);
    }
    // if offset 0 already found a long match, we're done
    if let Some(ref m) = best
        && m.duration() >= MIN_INTRO_SECS
    {
        return best;
    }

    // otherwise scan all plausible offsets (skip 0, already tried)
    for offset in -SEARCH_ENTRIES..=SEARCH_ENTRIES {
        if offset == 0 {
            continue;
        }
        if let Some(m) = try_offset(a, b, offset)
            && m.duration() > best.as_ref().map(|b| b.duration()).unwrap_or(0.0)
        {
            best = Some(m);
        }
    }

    // only report matches that meet the minimum duration
    best.filter(|m| m.duration() >= MIN_INTRO_SECS)
}

/// Scores one alignment offset and builds a Match if a long enough
/// run exists.
fn try_offset(a: &[u32], b: &[u32], offset: i32) -> Option<Match> {
    let (a_lo, a_hi, b_lo) = if offset >= 0 {
        let b_lo = offset as usize;
        if b_lo >= b.len() {
            return None;
        }
        let a_hi = (b.len() - b_lo).min(a.len());
        (0, a_hi, b_lo)
    } else {
        let a_lo = (-offset) as usize;
        if a_lo >= a.len() {
            return None;
        }
        let a_hi = a.len().min(a_lo + b.len());
        (a_lo, a_hi, 0)
    };
    let overlap = a_hi.saturating_sub(a_lo);
    if overlap < 10 {
        return None;
    }

    let scores: Vec<f64> = (0..overlap)
        .map(|i| similarity(a[a_lo + i], b[b_lo + i]))
        .collect();
    let (run_start, run_end, avg) = longest_run(&scores)?;

    let start_a = (a_lo + run_start) as f64 * FINGERPRINT_STEP_SECS;
    let end_a = (a_lo + run_end) as f64 * FINGERPRINT_STEP_SECS;
    let start_b = (b_lo + run_start) as f64 * FINGERPRINT_STEP_SECS;
    let end_b = (b_lo + run_end) as f64 * FINGERPRINT_STEP_SECS;

    Some(Match {
        offset,
        start_a,
        end_a,
        start_b,
        end_b,
        score: avg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_identical_intro_at_offset_zero() {
        let common: Vec<u32> = (0..150).map(|i| 0xDEAD_BEEF ^ i).collect();
        let tail_a: Vec<u32> = (0..50).map(|i| 0x1234_5678u32.wrapping_add(i)).collect();
        let tail_b: Vec<u32> = (0..50).map(|i| 0x9ABC_DEF0u32.wrapping_sub(i)).collect();

        let a: Vec<u32> = common.iter().chain(tail_a.iter()).cloned().collect();
        let b: Vec<u32> = common.iter().chain(tail_b.iter()).cloned().collect();

        let m = find_common_intro(&a, &b).expect("identical head must match");
        assert_eq!(m.offset, 0);
        assert!(m.start_a < 1.0, "A starts near zero: {}", m.start_a);
        assert!(m.end_a > 15.0, "at least 15 seconds in A: {}", m.end_a);
        assert!(
            (m.start_a - m.start_b).abs() < 0.1,
            "same position in both at offset 0"
        );
        assert!(m.score > 0.95, "near-perfect similarity: {}", m.score);
    }

    #[test]
    fn finds_shifted_intro() {
        // A: 50 junk + 150 common + 50 junk
        // B: 100 junk + 150 common + 50 junk (common starts 50 entries later)
        let common: Vec<u32> = (0..150).map(|i| 0xFEED_FACE ^ i).collect();
        let junk: Vec<u32> = (0..50)
            .map(|i| 0x1111_2222u32.wrapping_mul(i + 1))
            .collect();
        let junk2: Vec<u32> = (0..100)
            .map(|i| 0x3333_4444u32.wrapping_mul(i + 1))
            .collect();

        let a: Vec<u32> = junk
            .iter()
            .chain(common.iter())
            .chain(junk.iter())
            .cloned()
            .collect();
        let b: Vec<u32> = junk2
            .iter()
            .chain(common.iter())
            .chain(junk.iter())
            .cloned()
            .collect();

        let m = find_common_intro(&a, &b).expect("shifted common block must be found");
        assert!(m.offset > 0, "B's intro starts later: {}", m.offset);
        assert!(m.duration() > 15.0, "long enough: {}", m.duration());
        // A's intro starts around 50 entries ≈ 6.2 seconds
        assert!(
            (6.0..7.0).contains(&m.start_a),
            "A's intro at ~6.2s: {}",
            m.start_a
        );
        // B's intro starts around 100 entries ≈ 12.4 seconds
        assert!(
            (11.5..13.0).contains(&m.start_b),
            "B's intro at ~12.4s: {}",
            m.start_b
        );
    }

    #[test]
    fn no_match_when_different() {
        // Knuth multiplicative hash: well-distributed, no linear structure
        let a: Vec<u32> = (0..200)
            .map(|i| (i as u32).wrapping_mul(2654435761))
            .collect();
        let b: Vec<u32> = (0..200).map(|i| (i as u32).wrapping_mul(40503)).collect();
        assert!(find_common_intro(&a, &b).is_none());
    }

    #[test]
    fn too_short_is_rejected() {
        // only 50 common entries ≈ 6.19 seconds, below the 15s minimum
        let common: Vec<u32> = (0..50).map(|i| 0xCAFE_F00D ^ i).collect();
        let rest_a: Vec<u32> = (0..100).map(|i| i as u32).collect();
        let rest_b: Vec<u32> = (0..100).map(|i| (i * 13 + 7) as u32).collect();
        let a: Vec<u32> = common.iter().chain(rest_a.iter()).cloned().collect();
        let b: Vec<u32> = common.iter().chain(rest_b.iter()).cloned().collect();
        assert!(find_common_intro(&a, &b).is_none());
    }
}
