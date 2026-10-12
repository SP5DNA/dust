//! Minimal RNG trait so sampling and audio planning stay deterministic in tests.

pub trait Rng {
    /// A uniform draw from `[0, 1)`.
    ///
    /// The helpers below stay in range even for an implementation that hands
    /// back exactly 1.0, as test stubs do: a contract is only as good as what
    /// happens when it is broken, and here that would be an index past the
    /// end of a table.
    fn f64(&mut self) -> f64;

    fn usize_in(&mut self, min_inclusive: usize, max_inclusive: usize) -> usize {
        if max_inclusive <= min_inclusive {
            return min_inclusive;
        }
        let range = max_inclusive - min_inclusive;
        // In floating point, so a range of the whole of usize cannot overflow
        // the +1.
        let offset = (self.f64() * (range as f64 + 1.0)).floor() as usize;
        min_inclusive + offset.min(range)
    }

    fn pick_in_range(&mut self, min: f64, max: f64) -> f64 {
        if (min - max).abs() < f64::EPSILON {
            return min;
        }
        let lo = min.min(max);
        let hi = min.max(max);
        lo + self.f64() * (hi - lo)
    }

    fn pick_in_range_inclusive_int(&mut self, min: f64, max: f64) -> f64 {
        if (min - max).abs() < f64::EPSILON {
            return min;
        }
        let lo = min.min(max);
        let hi = min.max(max);
        (lo + self.f64() * (hi - lo + 1.0)).floor().min(hi.floor())
    }
}

/// A small, fast, seedable generator (SplitMix64). Deterministic for a given
/// seed, which is what lets every test replay the same session and the same
/// band.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FastrandRng(pub u64);

impl Default for FastrandRng {
    fn default() -> Self {
        Self(0x4d595df4d0f33173)
    }
}

impl Rng for FastrandRng {
    fn f64(&mut self) -> f64 {
        // SplitMix64 → unit interval, no extra crate in cw-core.
        self.0 = self.0.wrapping_add(SPLITMIX_GAMMA);
        (mix(self.0) >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

impl FastrandRng {
    /// A generator of its own for `stream`, derived from this one's state
    /// without drawing from it: the same state and stream always give the
    /// same generator, and any other pair an unrelated one.
    ///
    /// Adding to the seed is not the same thing. SplitMix64 steps its state
    /// by a fixed increment, so a seed that is another plus a multiple of
    /// that increment is the other's stream a few draws on — two streams
    /// seeded that way overlap, draw for draw. Hashing the stream into the
    /// state puts each one somewhere unrelated on the cycle.
    #[must_use]
    pub fn fork(&self, stream: u64) -> Self {
        Self(mix(self.0 ^ mix(stream)))
    }
}

/// SplitMix64's state increment.
const SPLITMIX_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64's output function: a bijection on `u64` that scatters nearby
/// inputs across the whole range.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One of `pool`, each as likely as its weight; a missing weight counts as
/// one. `None` only for an empty pool.
pub fn weighted_random_pick(pool: &[char], weights: &[f64], rng: &mut impl Rng) -> Option<char> {
    let last = pool.len().checked_sub(1)?;
    let weight = |i: usize| weights.get(i).copied().unwrap_or(1.0);
    let total: f64 = (0..pool.len()).map(weight).sum();
    if pool.len() == 1 || weights.is_empty() || total <= 0.0 {
        return pool.get(rng.usize_in(0, last)).copied();
    }
    let mut r = rng.f64() * total;
    for (i, ch) in pool.iter().enumerate() {
        r -= weight(i);
        if r <= 0.0 {
            return Some(*ch);
        }
    }
    pool.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pool_has_nothing_to_pick() {
        let mut rng = FastrandRng::default();
        assert_eq!(weighted_random_pick(&[], &[], &mut rng), None);
    }

    #[test]
    fn single_element() {
        let mut rng = FastrandRng::default();
        assert_eq!(weighted_random_pick(&['A'], &[1.0], &mut rng), Some('A'));
    }

    #[test]
    fn heavy_bias() {
        let mut rng = FastrandRng::default();
        let pool = ['A', 'B'];
        let weights = [100.0, 0.01];
        let mut a = 0;
        for _ in 0..1000 {
            if weighted_random_pick(&pool, &weights, &mut rng) == Some('A') {
                a += 1;
            }
        }
        assert!(a > 900);
    }

    struct Fixed(f64);

    impl Rng for Fixed {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    #[test]
    fn a_collapsed_range_returns_its_only_value() {
        let mut rng = Fixed(0.99);
        assert_eq!(rng.pick_in_range(18.0, 18.0), 18.0);
        assert_eq!(rng.pick_in_range_inclusive_int(600.0, 600.0), 600.0);
        assert_eq!(rng.usize_in(4, 4), 4);
        assert_eq!(rng.usize_in(4, 2), 4);
    }

    #[test]
    fn a_reversed_range_is_read_in_the_right_order() {
        let mut rng = Fixed(0.0);
        assert_eq!(rng.pick_in_range(30.0, 10.0), 10.0);
        assert_eq!(rng.pick_in_range_inclusive_int(600.0, 400.0), 400.0);
    }

    #[test]
    fn integer_picks_cover_both_ends_and_never_overshoot() {
        for (draw, expected) in [(0.0, 400.0), (0.5, 500.0), (0.999_999, 600.0)] {
            let mut rng = Fixed(draw);
            assert_eq!(rng.pick_in_range_inclusive_int(400.0, 600.0), expected);
        }
        for (draw, expected) in [(0.0, 1), (0.999_999, 5)] {
            let mut rng = Fixed(draw);
            assert_eq!(rng.usize_in(1, 5), expected);
        }
    }

    #[test]
    fn weights_that_add_up_to_nothing_fall_back_to_an_even_pick() {
        let mut rng = Fixed(0.75);
        assert_eq!(
            weighted_random_pick(&['A', 'B', 'C', 'D'], &[0.0; 4], &mut rng),
            Some('D')
        );
        // Fewer weights than characters: the missing ones count as one each,
        // so the total is positive and the walk starts at the first character.
        let mut rng = Fixed(0.0);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[0.0], &mut rng),
            Some('A')
        );
        let mut rng = Fixed(0.99);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[0.0], &mut rng),
            Some('B')
        );
    }

    /// A generator that hands back exactly 1.0 breaks the `[0, 1)` contract,
    /// and test stubs do. The helpers stay in range anyway, rather than
    /// indexing one past the end of a table.
    #[test]
    fn draws_at_the_very_top_stay_in_range() {
        let mut rng = Fixed(1.0);
        assert_eq!(rng.usize_in(0, 25), 25);
        assert_eq!(rng.usize_in(3, 3), 3);
        assert_eq!(rng.pick_in_range_inclusive_int(400.0, 600.0), 600.0);
        // The whole of usize, which used to overflow working out the span.
        assert_eq!(rng.usize_in(0, usize::MAX), usize::MAX);
        let mut rng = Fixed(0.0);
        assert_eq!(rng.usize_in(0, usize::MAX), 0);
        assert_eq!(rng.usize_in(7, 9), 7);
    }

    #[test]
    fn a_draw_at_the_very_top_still_lands_on_a_character() {
        let mut rng = Fixed(1.0);
        assert_eq!(
            weighted_random_pick(&['A', 'B'], &[1.0, 1.0], &mut rng),
            Some('B')
        );
    }

    #[test]
    fn the_default_generator_is_deterministic_and_in_range() {
        let mut a = FastrandRng::default();
        let mut b = FastrandRng::default();
        for _ in 0..100 {
            let x = a.f64();
            assert_eq!(x, b.f64());
            assert!((0.0..1.0).contains(&x));
        }
        // A different seed gives a different stream.
        let mut other = FastrandRng(1);
        assert_ne!(FastrandRng::default().f64(), other.f64());
    }

    /// Streams forked from neighbouring seeds, and neighbouring streams of
    /// one seed, share no draws. Seeding each with a multiple of SplitMix64's
    /// own increment added in made one session's stations the last-but-one
    /// session's, two draws on.
    #[test]
    fn forked_streams_do_not_overlap() {
        let mut seen = std::collections::HashSet::new();
        for seed in 0..64u64 {
            for stream in 0..16u64 {
                let mut rng = FastrandRng(seed).fork(stream);
                for _ in 0..8 {
                    assert!(
                        seen.insert(rng.f64().to_bits()),
                        "seed {seed}, stream {stream} repeats a draw of another stream"
                    );
                }
            }
        }
        // And the same seed and stream are the same generator, every time.
        assert_eq!(FastrandRng(5).fork(3), FastrandRng(5).fork(3));
        assert_ne!(FastrandRng(5).fork(3), FastrandRng(5).fork(4));
    }
}
