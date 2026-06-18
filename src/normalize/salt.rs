//! Humanized typing (SPEC §3.2): turn a flat base speed into per-character
//! delays with a bounded random "salt", so playback looks like a fast human
//! rather than a robotic paste.

use super::rng::Rng;

/// Per-character delays (ms) for typing `text`, jittered by ±`salt_ms` around
/// `base_ms`. Deterministic for a given `rng` state; never returns 0.
pub fn humanize_delays(text: &str, base_ms: u64, salt_ms: u64, rng: &mut Rng) -> Vec<u64> {
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
}
