//! Error-weighted + coverage-balanced character sampling for group generation.

use std::collections::BTreeMap;

use crate::alignment::CopyTally;
use crate::callsign::generate_callsign;
use crate::machine::GroupSource;
use crate::pool::compute_char_pool;
use crate::rng::{FastrandRng, Rng, weighted_random_pick};
use crate::session::SessionResult;
use crate::settings::{CharSetMode, LevelAxis, TrainingSettings};
use crate::timing::resolve_group_repeats;

pub const CHAR_SAMPLING_PRIOR_ALPHA: f64 = 1.0;
pub const CHAR_SAMPLING_PRIOR_BETA: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBetaBelief {
    pub alpha: f64,
    pub beta: f64,
}

impl Default for CharBetaBelief {
    fn default() -> Self {
        Self {
            alpha: CHAR_SAMPLING_PRIOR_ALPHA,
            beta: CHAR_SAMPLING_PRIOR_BETA,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharSamplingState {
    pub beliefs: BTreeMap<char, CharBetaBelief>,
    pub session_sample_counts: BTreeMap<char, u32>,
}

impl CharSamplingState {
    /// What the history says about each character, from the sessions that
    /// trained what `settings` trains: every character copied right moves
    /// its belief one way, every miss the other.
    pub fn from_history(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        let mut state = Self::default();
        for session in history.iter().filter(|s| s.usable_for_sampling(settings)) {
            for (character, stats) in session.letter_accuracy().iter() {
                let belief = state.beliefs.entry(*character).or_default();
                belief.alpha += f64::from(stats.correct);
                belief.beta += f64::from(stats.total.saturating_sub(stats.correct));
            }
        }
        state
    }

    /// Learn from one answer: each character sent was copied or it was not.
    pub fn learn(&mut self, copied: &CopyTally) {
        for (character, stats) in &copied.sent {
            let belief = self.beliefs.entry(*character).or_default();
            belief.alpha += f64::from(stats.correct);
            belief.beta += f64::from(stats.total.saturating_sub(stats.correct));
        }
    }

    fn count_sent(&mut self, group: &str) {
        for character in group.chars() {
            *self.session_sample_counts.entry(character).or_insert(0) += 1;
        }
    }
}

/// The trainer's own group source: groups drawn for the curriculum, leaning
/// on the characters the history says are hard and on those this session
/// has not sent yet, and learning from every answer as it comes.
#[derive(Clone, Debug)]
pub struct Sampler {
    state: CharSamplingState,
    rng: FastrandRng,
}

impl Sampler {
    /// A sampler for a session of `settings`, seeded with what the history
    /// knows about the alphabet it trains. `seed` should differ from session
    /// to session.
    pub fn new(history: &[SessionResult], settings: &TrainingSettings, seed: u64) -> Self {
        Self {
            state: CharSamplingState::from_history(history, settings),
            rng: FastrandRng(seed),
        }
    }
}

impl GroupSource for Sampler {
    fn next_group(&mut self, settings: &TrainingSettings) -> (String, u32) {
        let group = generate_training_group(settings, &mut self.state, &mut self.rng);
        (group, resolve_group_repeats(settings, &mut self.rng))
    }

    fn learn(&mut self, copied: &CopyTally) {
        self.state.learn(copied);
    }
}

#[derive(Clone, Debug)]
pub struct CharSamplingConfig {
    pub error_weight_strength: f64,
    pub coverage_strength: f64,
    pub thompson_sampling: bool,
    pub mixed_letters_percent: u32,
    pub char_set_mode: CharSetMode,
}

pub fn config_from_settings(settings: &TrainingSettings) -> CharSamplingConfig {
    CharSamplingConfig {
        error_weight_strength: settings.auto_level.error_weight_strength,
        coverage_strength: settings.auto_level.char_sampling_coverage_strength,
        thompson_sampling: settings.auto_level.char_sampling_thompson,
        mixed_letters_percent: settings.curriculum.mixed_letters_percent,
        char_set_mode: settings.curriculum.char_set_mode,
    }
}

pub fn belief_for(state: &CharSamplingState, character: char) -> CharBetaBelief {
    state.beliefs.get(&character).copied().unwrap_or_default()
}

pub fn beta_posterior_mean_error(belief: CharBetaBelief) -> f64 {
    let denom = belief.alpha + belief.beta;
    if denom <= 0.0 {
        0.5
    } else {
        belief.beta / denom
    }
}

fn coverage_factors(
    pool: &[char],
    session_sample_counts: &BTreeMap<char, u32>,
    coverage_strength: f64,
) -> BTreeMap<char, f64> {
    if coverage_strength <= 0.0 {
        return pool.iter().map(|c| (*c, 1.0)).collect();
    }
    let total_samples: u32 = pool
        .iter()
        .map(|c| session_sample_counts.get(c).copied().unwrap_or(0))
        .sum();
    let expected = if pool.is_empty() {
        0.0
    } else {
        f64::from(total_samples) / pool.len() as f64
    };
    let mut factors = BTreeMap::new();
    for character in pool {
        let seen = f64::from(session_sample_counts.get(character).copied().unwrap_or(0));
        let deficit = if expected > 0.0 {
            (expected - seen).max(0.0) / expected
        } else {
            1.0
        };
        factors.insert(*character, 1.0 + coverage_strength * deficit);
    }
    factors
}

fn difficulty_factor(p_error: f64, error_weight_strength: f64) -> f64 {
    if error_weight_strength <= 0.0 {
        1.0
    } else {
        1.0 + p_error * error_weight_strength
    }
}

/// A standard normal deviate, by Box–Muller. Straight-line: no draw is
/// retried, so no generator — however badly behaved — can keep it spinning.
fn random_normal(rng: &mut impl Rng) -> f64 {
    // 1 − u maps [0, 1) onto (0, 1], where the logarithm is finite.
    let u = (1.0 - rng.f64()).clamp(f64::EPSILON, 1.0);
    let v = rng.f64();
    (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
}

/// Marsaglia–Tsang rejection sampling. Each rejection loop is bounded: real
/// draws accept within a handful of tries, and only a generator stuck on one
/// value could exhaust them, in which case the mean is as good an answer as
/// any.
fn sample_gamma(shape: f64, rng: &mut impl Rng) -> f64 {
    const TRIES: usize = 64;
    // Not `shape <= 0.0`: NaN fails every comparison and would fall through
    // into a loop that can never accept.
    if !(shape > 0.0 && shape.is_finite()) {
        return 0.0;
    }
    if shape < 1.0 {
        let boosted = sample_gamma(shape + 1.0, rng);
        return boosted * rng.f64().powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    for _ in 0..TRIES {
        let Some((x, v)) = (0..TRIES).find_map(|_| {
            let x = random_normal(rng);
            let v = 1.0 + c * x;
            (v > 0.0).then_some((x, v))
        }) else {
            break;
        };
        let v = v * v * v;
        let u = rng.f64();
        if u < 1.0 - 0.0331 * x * x * x * x || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
    shape
}

pub fn sample_beta(alpha: f64, beta: f64, rng: &mut impl Rng) -> f64 {
    let x = sample_gamma(alpha, rng);
    let y = sample_gamma(beta, rng);
    let denom = x + y;
    if denom.is_finite() && denom > 0.0 {
        x / denom
    } else {
        0.5
    }
}

pub fn compute_raw_sampling_weights(
    pool: &[char],
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> BTreeMap<char, f64> {
    let coverage = coverage_factors(pool, &state.session_sample_counts, config.coverage_strength);
    let mut weights = BTreeMap::new();
    for character in pool {
        let belief = belief_for(state, *character);
        let p_error = if config.thompson_sampling {
            // Beta(α, β) is P(correct); difficulty weighting needs P(error).
            (1.0 - sample_beta(belief.alpha, belief.beta, rng)).clamp(0.0, 1.0)
        } else {
            beta_posterior_mean_error(belief)
        };
        let coverage_factor = coverage.get(character).copied().unwrap_or(1.0);
        weights.insert(
            *character,
            difficulty_factor(p_error, config.error_weight_strength) * coverage_factor,
        );
    }
    weights
}

pub fn normalize_weights(pool: &[char], weights: &BTreeMap<char, f64>) -> BTreeMap<char, f64> {
    let total: f64 = pool
        .iter()
        .map(|c| weights.get(c).copied().unwrap_or(0.0))
        .sum();
    let mut out = BTreeMap::new();
    if total <= 0.0 {
        let uniform = if pool.is_empty() {
            0.0
        } else {
            1.0 / pool.len() as f64
        };
        for c in pool {
            out.insert(*c, uniform);
        }
        return out;
    }
    for c in pool {
        out.insert(*c, weights.get(c).copied().unwrap_or(0.0) / total);
    }
    out
}

fn pick_from_pool(
    pool: &[char],
    weights: &BTreeMap<char, f64>,
    rng: &mut impl Rng,
) -> Option<char> {
    let weight_list: Vec<f64> = pool
        .iter()
        .map(|c| weights.get(c).copied().unwrap_or(1.0))
        .collect();
    weighted_random_pick(pool, &weight_list, rng)
}

pub fn sample_training_group(
    pool: &[char],
    group_size: usize,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let raw_weights = compute_raw_sampling_weights(pool, state, config, rng);
    // Mixed draws each character from the letters or the digits by the
    // share asked for — when the pool holds both to draw from.
    let (letters, digits): (Vec<char>, Vec<char>) = pool.iter().partition(|c| !c.is_ascii_digit());
    let split =
        (config.char_set_mode == CharSetMode::Mixed && !letters.is_empty() && !digits.is_empty())
            .then(|| f64::from(config.mixed_letters_percent.min(100)) / 100.0);

    let mut group = String::with_capacity(group_size);
    for _ in 0..group_size {
        let from = match split {
            Some(letters_share) if rng.f64() < letters_share => &letters,
            Some(_) => &digits,
            None => pool,
        };
        group.extend(pick_from_pool(from, &raw_weights, rng));
    }
    state.count_sent(&group);
    group
}

/// How many realistic callsigns are weighed against each other before one is
/// sent. Enough for the weak characters to show up, few enough that the mix
/// still sounds like a band rather than a drill.
const CALLSIGN_CANDIDATES: usize = 6;

/// A callsign, chosen the way a group is: several realistic candidates, and the
/// one whose characters the sampler wants most wins.
///
/// The structure is never bent to fit the sampler — every candidate is already
/// a well-formed call — so this can only decide *which* real callsign to send.
/// It scores them with [`compute_raw_sampling_weights`], the same weighting the
/// group sampler uses, so the error-weight and coverage settings mean exactly
/// what they mean everywhere else. With both turned off every weight is equal,
/// the first candidate wins, and the draw is plainly random.
pub(crate) fn generate_callsign_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    config: &CharSamplingConfig,
    rng: &mut impl Rng,
) -> String {
    let tier = settings.level(LevelAxis::Callsign);
    let pool = compute_char_pool(settings);
    let weights = compute_raw_sampling_weights(&pool, state, config, rng);
    let appetite = |call: &str| {
        let mut total = 0.0;
        let mut count = 0.0;
        for ch in call.chars() {
            total += weights.get(&ch).copied().unwrap_or(1.0);
            count += 1.0;
        }
        if count > 0.0 { total / count } else { 0.0 }
    };

    let mut best = generate_callsign(tier, rng);
    let mut best_appetite = appetite(&best);
    for _ in 1..CALLSIGN_CANDIDATES {
        let candidate = generate_callsign(tier, rng);
        let candidate_appetite = appetite(&candidate);
        if candidate_appetite > best_appetite {
            best = candidate;
            best_appetite = candidate_appetite;
        }
    }

    state.count_sent(&best);
    best
}

/// The next group for `settings`, drawn by the sampler and counted as sent.
pub fn generate_training_group(
    settings: &TrainingSettings,
    state: &mut CharSamplingState,
    rng: &mut impl Rng,
) -> String {
    // A callsign is a group like any other once it exists — it is only built
    // differently. This is the single place the two modes part company.
    let config = config_from_settings(settings);
    if settings.curriculum.char_set_mode == CharSetMode::Callsign {
        return generate_callsign_group(settings, state, &config, rng);
    }
    let pool = compute_char_pool(settings);
    // At least one character: a group of none is a silent send nobody can
    // answer, and only settings that were never clamped could ask for it.
    let shortest = settings.curriculum.min_group_size.max(1) as usize;
    let longest = (settings.curriculum.max_group_size as usize).max(shortest);
    let group_size = rng.usize_in(shortest, longest);
    sample_training_group(&pool, group_size, state, &config, rng)
}

#[cfg(test)]
mod tests {
    /// A generator stuck on one value: the worst a broken stub can do.
    struct Stuck(f64);

    impl crate::rng::Rng for Stuck {
        fn f64(&mut self) -> f64 {
            self.0
        }
    }

    /// Nothing about the shape or the generator can hang the sampler. A NaN
    /// shape used to fall through the guard into a loop that never accepts,
    /// and a generator stuck on zero spun the normal draw forever.
    #[test]
    fn the_samplers_always_come_back() {
        for shape in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e-9, 0.5, 3.0, 1e6] {
            for stuck in [0.0, 0.5, 0.999_999, 1.0] {
                let g = sample_gamma(shape, &mut Stuck(stuck));
                assert!(g.is_finite() && g >= 0.0, "gamma({shape}) on {stuck}: {g}");
                let b = sample_beta(shape, 2.0, &mut Stuck(stuck));
                assert!((0.0..=1.0).contains(&b), "beta({shape}, 2) on {stuck}: {b}");
            }
        }
        let n = random_normal(&mut Stuck(0.0));
        assert!(n.is_finite());
    }

    /// And the real thing is still the real thing: Beta(α, β) has mean
    /// α/(α+β).
    #[test]
    fn beta_draws_have_the_right_mean() {
        let mut rng = crate::rng::FastrandRng(42);
        for (a, b) in [(2.0, 5.0), (0.5, 0.5), (9.0, 1.0)] {
            let n = 20_000;
            let mean = (0..n).map(|_| sample_beta(a, b, &mut rng)).sum::<f64>() / f64::from(n);
            let expected = a / (a + b);
            assert!(
                (mean - expected).abs() < 0.01,
                "Beta({a}, {b}): mean {mean:.4}, wanted {expected:.4}"
            );
        }
    }

    use super::*;

    #[test]
    fn group_length_respects_fixed_size() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 3;
        settings.curriculum.max_group_size = 3;
        let mut state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        let group = generate_training_group(&settings, &mut state, &mut rng);
        assert_eq!(group.len(), 3);
        for ch in group.chars() {
            assert!(ch == 'K' || ch == 'M');
        }
    }

    #[test]
    fn thompson_weights_are_positive() {
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 1.0,
            thompson_sampling: true,
            mixed_letters_percent: 70,
            char_set_mode: CharSetMode::Koch,
        };
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert!(weights.values().all(|w| *w > 0.0));
    }

    #[test]
    fn generated_group_is_never_empty_for_koch() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        settings.curriculum.min_group_size = 1;
        settings.curriculum.max_group_size = 1;
        let mut state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        let group = generate_training_group(&settings, &mut state, &mut rng);
        assert!(!group.is_empty());
    }

    #[test]
    fn repeated_letters_update_belief_once_per_occurrence() {
        let mut next = CharSamplingState::default();
        next.learn(&CopyTally::of("KK", "KK"));
        let k = next.beliefs.get(&'K').copied().unwrap_or_default();
        assert!((k.alpha - (CHAR_SAMPLING_PRIOR_ALPHA + 2.0)).abs() < 1e-9);
        assert!((k.beta - CHAR_SAMPLING_PRIOR_BETA).abs() < 1e-9);
    }

    #[test]
    fn mismatch_increments_beta() {
        let mut next = CharSamplingState::default();
        next.learn(&CopyTally::of("KM", "KX"));
        let m = next.beliefs.get(&'M').copied().unwrap_or_default();
        assert!((m.beta - (CHAR_SAMPLING_PRIOR_BETA + 1.0)).abs() < 1e-9);
    }

    #[test]
    fn thompson_weights_hard_letters_above_easy() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.beliefs.insert(
            'K',
            CharBetaBelief {
                alpha: 40.0,
                beta: 1.0,
            },
        );
        state.beliefs.insert(
            'M',
            CharBetaBelief {
                alpha: 1.0,
                beta: 40.0,
            },
        );
        let config = CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 0.0,
            thompson_sampling: true,
            mixed_letters_percent: 70,
            char_set_mode: CharSetMode::Koch,
        };
        let mut rng = FastrandRng::default();
        let mut k_sum = 0.0;
        let mut m_sum = 0.0;
        for _ in 0..400 {
            let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
            k_sum += weights.get(&'K').copied().unwrap_or(0.0);
            m_sum += weights.get(&'M').copied().unwrap_or(0.0);
        }
        assert!(m_sum > k_sum);
    }

    #[test]
    fn mixed_100_percent_never_samples_digits() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.level = 5;
        settings.curriculum.digits_level = 9;
        settings.curriculum.min_group_size = 4;
        settings.curriculum.max_group_size = 4;
        let mut state = CharSamplingState::default();
        let mut rng = FastrandRng::default();
        for _ in 0..40 {
            let group = generate_training_group(&settings, &mut state, &mut rng);
            assert!(!group.is_empty());
            assert!(group.chars().all(|c| !c.is_ascii_digit()), "{group}");
        }
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;
    use crate::rng::FastrandRng;

    /// Returns a fixed sequence of "random" numbers, so every branch that
    /// depends on a draw can be aimed at deliberately.
    struct ScriptedRng {
        values: Vec<f64>,
        next: usize,
    }

    impl ScriptedRng {
        fn new(values: &[f64]) -> Self {
            Self {
                values: values.to_vec(),
                next: 0,
            }
        }
    }

    impl Rng for ScriptedRng {
        fn f64(&mut self) -> f64 {
            let value = self.values[self.next % self.values.len()];
            self.next += 1;
            value
        }
    }

    fn config(mode: CharSetMode, pct: u32) -> CharSamplingConfig {
        CharSamplingConfig {
            error_weight_strength: 3.0,
            coverage_strength: 1.0,
            thompson_sampling: false,
            mixed_letters_percent: pct,
            char_set_mode: mode,
        }
    }

    #[test]
    fn a_belief_with_no_mass_is_a_coin_flip() {
        assert_eq!(
            beta_posterior_mean_error(CharBetaBelief {
                alpha: 0.0,
                beta: 0.0
            }),
            0.5
        );
    }

    #[test]
    fn an_untouched_character_starts_on_the_flat_prior() {
        let state = CharSamplingState::default();
        let belief = belief_for(&state, 'K');
        assert_eq!(belief.alpha, CHAR_SAMPLING_PRIOR_ALPHA);
        assert_eq!(belief.beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(beta_posterior_mean_error(belief), 0.5);
    }

    #[test]
    fn coverage_lifts_the_characters_that_have_been_seen_least() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.session_sample_counts.insert('K', 10);
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert!(weights[&'M'] > weights[&'K']);

        // With coverage off both characters weigh the same.
        let mut flat = config.clone();
        flat.coverage_strength = 0.0;
        let weights = compute_raw_sampling_weights(&pool, &state, &flat, &mut rng);
        assert_eq!(weights[&'M'], weights[&'K']);
    }

    #[test]
    fn a_fresh_session_gives_every_character_the_same_coverage_lift() {
        // Nothing sampled yet, so there is no deficit to compute a ratio from.
        let pool = ['K', 'M'];
        let state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert_eq!(weights[&'K'], weights[&'M']);
        assert!(weights[&'K'] > 0.0);
    }

    #[test]
    fn error_weighting_can_be_switched_off() {
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        state.beliefs.insert(
            'M',
            CharBetaBelief {
                alpha: 1.0,
                beta: 20.0,
            },
        );
        let mut config = config(CharSetMode::Koch, 70);
        config.error_weight_strength = 0.0;
        config.coverage_strength = 0.0;
        let mut rng = FastrandRng::default();
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        assert_eq!(weights[&'K'], weights[&'M']);
    }

    #[test]
    fn thompson_draws_stay_inside_the_unit_interval() {
        let mut rng = FastrandRng(12345);
        for _ in 0..200 {
            let x = sample_beta(0.4, 2.5, &mut rng);
            assert!((0.0..=1.0).contains(&x), "beta draw out of range: {x}");
        }
        // A degenerate pair cannot divide by zero.
        assert_eq!(sample_beta(0.0, 0.0, &mut rng), 0.5);
        assert_eq!(sample_beta(-1.0, -1.0, &mut rng), 0.5);
    }

    #[test]
    fn weights_normalise_to_one_or_fall_back_to_uniform() {
        let pool = ['K', 'M', 'U'];
        let mut weights = BTreeMap::new();
        weights.insert('K', 1.0);
        weights.insert('M', 3.0);
        let normalized = normalize_weights(&pool, &weights);
        assert!((normalized.values().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(normalized[&'U'], 0.0);

        // All-zero weights spread evenly rather than dividing by zero.
        let zeroed = normalize_weights(&pool, &BTreeMap::new());
        assert!(zeroed.values().all(|v| (*v - 1.0 / 3.0).abs() < 1e-9));
        assert!(normalize_weights(&[], &BTreeMap::new()).is_empty());
    }

    #[test]
    fn sampling_an_empty_pool_returns_an_empty_group() {
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        let group = sample_training_group(&[], 3, &mut state, &config, &mut rng);
        assert!(group.is_empty());
        assert_eq!(state, CharSamplingState::default());
        let group = sample_training_group(&['K'], 0, &mut state, &config, &mut rng);
        assert!(group.is_empty());
    }

    #[test]
    fn a_mixed_group_follows_the_letter_share() {
        let pool = ['K', 'M', '0', '1'];
        let mut next = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        // Draws below 0.5 pick letters, above pick digits. Each character costs
        // two draws: one for the subset, one for the weighted pick.
        let mut rng = ScriptedRng::new(&[0.1, 0.0, 0.9, 0.0]);
        let group = sample_training_group(&pool, 2, &mut next, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        let mut chars = group.chars();
        assert!(!chars.next().unwrap().is_ascii_digit());
        assert!(chars.next().unwrap().is_ascii_digit());
        assert_eq!(next.session_sample_counts.values().sum::<u32>(), 2);
    }

    /// Settings nobody clamped can ask for groups of no characters at all;
    /// a group still has one.
    #[test]
    fn a_group_is_never_empty() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.min_group_size = 0;
        settings.curriculum.max_group_size = 0;
        let mut rng = FastrandRng(5);
        let group = generate_training_group(&settings, &mut CharSamplingState::default(), &mut rng);
        assert_eq!(group.chars().count(), 1);
    }

    #[test]
    fn a_mixed_group_with_only_letters_unlocked_still_fills() {
        // No digits in the pool, so the digit half falls back to the whole pool.
        let pool = ['K', 'M'];
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Mixed, 50);
        let mut rng = ScriptedRng::new(&[0.9, 0.0]);
        let group = sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        assert_eq!(group.chars().count(), 2);
        assert!(group.chars().all(|c| c == 'K' || c == 'M'));
    }

    #[test]
    fn sample_counts_accumulate_across_groups() {
        let pool = ['K'];
        let mut state = CharSamplingState::default();
        let config = config(CharSetMode::Koch, 70);
        let mut rng = FastrandRng::default();
        sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        sample_training_group(&pool, 2, &mut state, &config, &mut rng);
        assert_eq!(state.session_sample_counts.get(&'K'), Some(&4));
    }

    #[test]
    fn an_answer_moves_only_the_characters_that_were_sent() {
        let mut next = CharSamplingState::default();
        // "KM" copied as "KX": K right, M wrong, X never sent.
        next.learn(&CopyTally::of("KM", "KX"));
        assert_eq!(next.beliefs[&'K'].alpha, CHAR_SAMPLING_PRIOR_ALPHA + 1.0);
        assert_eq!(next.beliefs[&'K'].beta, CHAR_SAMPLING_PRIOR_BETA);
        assert_eq!(next.beliefs[&'M'].beta, CHAR_SAMPLING_PRIOR_BETA + 1.0);
        assert!(!next.beliefs.contains_key(&'X'));
    }

    #[test]
    fn history_from_earlier_sessions_seeds_the_beliefs() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let session = |copies: &[(&str, &str)]| {
            let mut result = SessionResult::empty_for_test();
            for (sent, received) in copies {
                result.tally.add(sent, received);
            }
            result
        };
        let first = session(&[("KKKKK", "KKKXX")]);
        let second = session(&[("K", "K")]);
        let mut digits = session(&[("K", "X")]);
        digits.char_set_mode = CharSetMode::Digits;
        let state = CharSamplingState::from_history(&[first, second, digits], &settings);
        let belief = state.beliefs[&'K'];
        assert_eq!(belief.alpha, CHAR_SAMPLING_PRIOR_ALPHA + 4.0);
        assert_eq!(belief.beta, CHAR_SAMPLING_PRIOR_BETA + 2.0);
        assert!(state.session_sample_counts.is_empty());
    }

    /// The sampler a session draws from learns as the session goes: a
    /// character missed is drawn more after it.
    #[test]
    fn the_sampler_learns_from_each_answer() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 1;
        let mut sampler = Sampler::new(&[], &settings, 3);
        let (group, repeats) = sampler.next_group(&settings);
        assert!(!group.is_empty());
        assert_eq!(repeats, 1);
        sampler.learn(&CopyTally::of("MMMMM", "XXXXX"));
        let missed = sampler.state.beliefs[&'M'];
        assert_eq!(missed.beta, CHAR_SAMPLING_PRIOR_BETA + 5.0);
    }

    #[test]
    fn group_size_stays_inside_the_configured_range() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.level = 4;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 5;
        let mut state = CharSamplingState::default();
        let mut rng = FastrandRng(99);
        for _ in 0..50 {
            let group = generate_training_group(&settings, &mut state, &mut rng);
            let len = group.chars().count();
            assert!(
                (2..=5).contains(&len),
                "group {group:?} has {len} characters"
            );
        }
    }
}
