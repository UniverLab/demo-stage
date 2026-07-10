//! Humanized typing (SPEC §3.2): turn a flat base speed into per-character
//! delays with a bounded random "salt", so playback looks like a fast human
//! rather than a robotic paste.

use super::rng::Rng;

/// Cap on the expected total duration of a single `type` step. Watching a long
/// string come in at the short-text pace gets monotonous, so past this budget
/// the per-key delay compresses with the string's length instead.
pub const MAX_TYPE_MS: u64 = 5000;

/// Per-character delays (ms) for typing `text`, jittered by ±`salt_ms` around
/// `base_ms`. Short strings type at `base_ms`; once `len × base_ms` would blow
/// past [`MAX_TYPE_MS`], the pace speeds up so the whole step still lands in
/// ~[`MAX_TYPE_MS`] (salt shrinks proportionally, keeping the jitter a fixed
/// fraction of the pace). Deterministic for a given `rng` state; never returns 0.
pub fn humanize_delays(text: &str, base_ms: u64, salt_ms: u64, rng: &mut Rng) -> Vec<u64> {
    let n = text.chars().count() as u64;
    let (base_ms, salt_ms) = if n > 0 && n * base_ms > MAX_TYPE_MS {
        let base = (MAX_TYPE_MS / n).max(1);
        (base, salt_ms * base / base_ms.max(1))
    } else {
        (base_ms, salt_ms)
    };
    text.chars()
        .map(|_| {
            if salt_ms == 0 {
                base_ms.max(1)
            } else {
                let span = salt_ms * 2 + 1;
                let delta = (rng.next_u64() % span) as i64 - salt_ms as i64;
                (base_ms as i64 + delta).max(1) as u64
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_delay_per_character() {
        let mut rng = Rng::new(7);
        assert_eq!(humanize_delays("hello", 80, 15, &mut rng).len(), 5);
    }

    #[test]
    fn stays_within_bounds() {
        let mut rng = Rng::new(7);
        for d in humanize_delays("a long-ish command", 80, 15, &mut rng) {
            assert!((65..=95).contains(&d), "delay {d} out of ±15 of 80");
        }
    }

    #[test]
    fn zero_salt_is_constant() {
        let mut rng = Rng::new(7);
        assert!(humanize_delays("abc", 80, 0, &mut rng)
            .iter()
            .all(|&d| d == 80));
    }

    #[test]
    fn deterministic_for_seed() {
        let a = humanize_delays("command", 80, 15, &mut Rng::new(99));
        let b = humanize_delays("command", 80, 15, &mut Rng::new(99));
        assert_eq!(a, b);
    }

    #[test]
    fn long_text_compresses_to_the_budget() {
        let mut rng = Rng::new(7);
        let text = "x".repeat(400); // 400 × 80ms = 32s uncapped
        let total: u64 = humanize_delays(&text, 80, 15, &mut rng).iter().sum();
        // Expected ~MAX_TYPE_MS; jitter is ±(scaled salt) per key, so allow slack.
        assert!(
            total <= MAX_TYPE_MS + MAX_TYPE_MS / 4,
            "total {total}ms blows the {MAX_TYPE_MS}ms budget"
        );
        assert!(
            total >= MAX_TYPE_MS / 2,
            "total {total}ms suspiciously fast"
        );
    }

    #[test]
    fn short_text_keeps_the_base_pace() {
        let mut rng = Rng::new(7);
        // 62 × 80 = 4960ms — just under the budget, so the pace is untouched.
        let text = "y".repeat(62);
        for d in humanize_delays(&text, 80, 15, &mut rng) {
            assert!((65..=95).contains(&d), "delay {d} out of ±15 of 80");
        }
    }

    #[test]
    fn longer_text_types_faster_per_key() {
        let short = humanize_delays(&"a".repeat(100), 80, 0, &mut Rng::new(1));
        let long = humanize_delays(&"a".repeat(300), 80, 0, &mut Rng::new(1));
        assert_eq!(short[0], 50); // 5000/100
        assert_eq!(long[0], 16); // 5000/300
    }
}
