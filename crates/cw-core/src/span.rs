//! How long a group you can copy whole: a Bayesian estimate of your copy span,
//! and group lengths drawn near its edge.
//!
//! The model is a psychometric curve over group length, the way adaptive
//! testing models a threshold (QUEST, the Ψ method). A group of `L`
//! characters is copied whole with probability
//!
//! ```text
//! P(whole | L) = (1 − λ) · σ((S − L) / w)
//! ```
//!
//! `S` is the span, the length copied whole about half the time; `w` is how
//! sharply that falls away past it; `λ` is a small lapse rate, so one slip on
//! a short group is not read as a short span. The posterior over `(S, w)` is
//! held exactly, as log weights on a fixed grid: there is no approximation to
//! drift and nothing to diverge.
//!
//! Each length is then drawn by Thompson sampling: one `(S, w)` is drawn from
//! the posterior, and the lengths whose chance of being copied whole comes
//! closest to the target are favoured. While the posterior is wide that
//! explores; as it narrows it settles.

use crate::alignment::CopyTally;
use crate::rng::Rng;
use crate::session::SessionResult;
use crate::settings::{CharSetMode, TrainingSettings};

/// The chance of fumbling a group however short it is.
const LAPSE: f64 = 0.03;
/// The spans the grid holds: 0.5 to 20.5 characters, a tenth apart.
const SPAN_FROM: f64 = 0.5;
const SPAN_STEP: f64 = 0.1;
const SPAN_POINTS: usize = 201;
/// How sharply copying falls away past the span, in characters.
const SPREADS: [f64; 5] = [0.4, 0.7, 1.0, 1.5, 2.5];
const CELLS: usize = SPAN_POINTS * SPREADS.len();
/// How far the first guess at a span may be off, in characters.
const PRIOR_SD: f64 = 3.0;
/// What a session's evidence is worth once another has followed it: half,
/// ten sessions on, so the span follows you as you improve.
const SESSION_FADE: f64 = 0.933_032_991_536_807_4;
/// The share of lengths drawn evenly from the whole range, so every length
/// in it still comes round now and then.
const VARIETY: f64 = 0.1;
/// How far, in characters, a length can be from the aim and still be
/// favoured.
const AIM_WIDTH: f64 = 1.0;
/// How far past either end of the range the aim may go, in characters: far
/// enough that the end itself takes nearly all of the leaning.
const AIM_REACH: f64 = 3.0;

fn span_of(cell: usize) -> f64 {
    SPAN_FROM + (cell / SPREADS.len()) as f64 * SPAN_STEP
}

fn spread_of(cell: usize) -> f64 {
    SPREADS[cell % SPREADS.len()]
}

/// The chance of copying a group of `length` whole, for one span and spread.
fn p_whole(length: f64, span: f64, spread: f64) -> f64 {
    (1.0 - LAPSE) / (1.0 + ((length - span) / spread).exp())
}

/// One index, in proportion to `weights`. A table with no weight in it, or
/// one that cannot be read, is drawn from evenly.
fn pick_index(weights: &[f64], rng: &mut impl Rng) -> usize {
    let total: f64 = weights.iter().sum();
    if !(total.is_finite() && total > 0.0) {
        return rng.usize_in(0, weights.len().saturating_sub(1));
    }
    let mut left = rng.f64() * total;
    for (index, weight) in weights.iter().enumerate() {
        if left < *weight {
            return index;
        }
        left -= weight;
    }
    weights.len().saturating_sub(1)
}

/// The estimate of the span: its posterior mean and standard deviation, in
/// characters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanEstimate {
    pub mean: f64,
    pub sd: f64,
}

/// The span after one session of the history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanPoint {
    pub timestamp: u64,
    pub estimate: SpanEstimate,
}

/// One group length in the range, as the next group sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LengthForecast {
    pub length: usize,
    /// The chance this is the next group's length.
    pub drawn: f64,
    /// The chance a group this long is copied whole.
    pub whole: f64,
}

/// What the history and this session say about your copy span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanBelief {
    /// Log prior, one per (span, spread) cell.
    prior: Vec<f64>,
    /// Log likelihood of everything observed, faded by age.
    evidence: Vec<f64>,
}

impl SpanBelief {
    /// A belief with nothing learned yet, its first guess at the span being
    /// `centre` characters.
    pub fn new(centre: f64) -> Self {
        let prior = (0..CELLS)
            .map(|cell| -0.5 * ((span_of(cell) - centre) / PRIOR_SD).powi(2))
            .collect();
        Self {
            prior,
            evidence: vec![0.0; CELLS],
        }
    }

    /// The belief the history leaves for `settings`, its first guess the
    /// middle of the group size range.
    pub fn from_history(history: &[SessionResult], settings: &TrainingSettings) -> Self {
        Self::replay(history, settings, |_, _| {})
    }

    /// The span after each session of the history that counts towards it,
    /// oldest first.
    pub fn history(history: &[SessionResult], settings: &TrainingSettings) -> Vec<SpanPoint> {
        let mut points = Vec::new();
        Self::replay(history, settings, |session, belief| {
            points.push(SpanPoint {
                timestamp: session.timestamp,
                estimate: belief.estimate(),
            });
        });
        points
    }

    /// Learn from the history in the order it was practised, telling `after`
    /// where the belief stands after each session. Only sessions of the
    /// character set `settings` trains count: a callsign is as long as it
    /// is, and digits are not held the way letters are.
    fn replay(
        history: &[SessionResult],
        settings: &TrainingSettings,
        mut after: impl FnMut(&SessionResult, &Self),
    ) -> Self {
        let (shortest, longest) = crate::sampling::group_size_bounds(settings);
        let mut belief = Self::new((shortest + longest) as f64 / 2.0);
        let mode = settings.curriculum.char_set_mode;
        if mode == CharSetMode::Callsign {
            return belief;
        }
        let mut sessions: Vec<&SessionResult> = history
            .iter()
            .filter(|session| session.char_set_mode == mode)
            .collect();
        sessions.sort_by_key(|session| session.timestamp);
        for session in sessions {
            belief.fade();
            // Counted per length first: one update for each length a
            // session sent, rather than one for each group.
            let mut tally: Vec<(usize, u32, u32)> = Vec::new();
            for group in &session.groups {
                let length = group.sent.chars().count();
                match tally.iter_mut().find(|(seen, _, _)| *seen == length) {
                    Some((_, whole, missed)) => {
                        *whole += u32::from(group.correct);
                        *missed += u32::from(!group.correct);
                    }
                    None => {
                        tally.push((length, u32::from(group.correct), u32::from(!group.correct)))
                    }
                }
            }
            for (length, whole, missed) in tally {
                belief.observe(length, whole, missed);
            }
            after(session, &belief);
        }
        belief
    }

    /// One more session has passed: everything learned so far counts for a
    /// little less.
    fn fade(&mut self) {
        for evidence in &mut self.evidence {
            *evidence *= SESSION_FADE;
        }
    }

    /// Groups of `length` characters: `whole` of them copied whole, `missed`
    /// not.
    fn observe(&mut self, length: usize, whole: u32, missed: u32) {
        if length == 0 || whole + missed == 0 {
            return;
        }
        let length = length as f64;
        for (cell, evidence) in self.evidence.iter_mut().enumerate() {
            let p = p_whole(length, span_of(cell), spread_of(cell)).clamp(1e-12, 1.0 - 1e-12);
            *evidence += f64::from(whole) * p.ln() + f64::from(missed) * (1.0 - p).ln();
        }
    }

    /// Learn from one group of `length` characters.
    pub fn learn(&mut self, length: usize, whole: bool) {
        self.observe(length, u32::from(whole), u32::from(!whole));
    }

    /// Learn from what was copied of one group: how long it was, and whether
    /// every character came back with nothing added.
    pub fn learn_copy(&mut self, copied: &CopyTally) {
        let sent: u32 = copied.sent.values().map(|stats| stats.total).sum();
        let right: u32 = copied.sent.values().map(|stats| stats.correct).sum();
        self.learn(sent as usize, right == sent && copied.inserted.is_empty());
    }

    /// The posterior, one probability per cell.
    fn posterior(&self) -> Vec<f64> {
        let log: Vec<f64> = self
            .prior
            .iter()
            .zip(&self.evidence)
            .map(|(prior, evidence)| prior + evidence)
            .collect();
        let top = log.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut weights: Vec<f64> = log.iter().map(|value| (value - top).exp()).collect();
        let total: f64 = weights.iter().sum();
        if total.is_finite() && total > 0.0 {
            for weight in &mut weights {
                *weight /= total;
            }
        } else {
            weights = vec![1.0 / CELLS as f64; CELLS];
        }
        weights
    }

    pub fn estimate(&self) -> SpanEstimate {
        let posterior = self.posterior();
        let mean: f64 = posterior
            .iter()
            .enumerate()
            .map(|(cell, p)| p * span_of(cell))
            .sum();
        let variance: f64 = posterior
            .iter()
            .enumerate()
            .map(|(cell, p)| p * (span_of(cell) - mean).powi(2))
            .sum();
        SpanEstimate {
            mean,
            sd: variance.max(0.0).sqrt(),
        }
    }

    /// The next group's length, between `shortest` and `longest` characters,
    /// aiming for `target` of groups copied whole.
    pub fn draw_length(
        &self,
        shortest: usize,
        longest: usize,
        target: f64,
        rng: &mut impl Rng,
    ) -> usize {
        let (shortest, longest) = (shortest.min(longest), shortest.max(longest));
        if shortest == longest {
            return shortest;
        }
        let cell = pick_index(&self.posterior(), rng);
        let weights = length_weights(shortest, longest, target, cell);
        shortest + pick_index(&weights, rng)
    }

    /// Each length between `shortest` and `longest`: how likely it is to be
    /// drawn next, and how likely it is to be copied whole, over everything
    /// the posterior allows.
    pub fn forecast(&self, shortest: usize, longest: usize, target: f64) -> Vec<LengthForecast> {
        let (shortest, longest) = (shortest.min(longest), shortest.max(longest));
        let mut rows: Vec<LengthForecast> = (shortest..=longest)
            .map(|length| LengthForecast {
                length,
                drawn: 0.0,
                whole: 0.0,
            })
            .collect();
        for (cell, p) in self.posterior().into_iter().enumerate() {
            if p < 1e-12 {
                continue;
            }
            let weights = length_weights(shortest, longest, target, cell);
            for (row, weight) in rows.iter_mut().zip(weights) {
                row.drawn += p * weight;
                row.whole += p * p_whole(row.length as f64, span_of(cell), spread_of(cell));
            }
        }
        rows
    }
}

/// How strongly each length from `shortest` to `longest` is favoured for one
/// cell of the posterior.
///
/// The lengths lean towards an aim, counted in characters, with a share
/// spread evenly over them all. The aim is set so that, over the lengths
/// drawn, groups are copied whole `target` of the time. Aiming instead at
/// the one length that is copied whole that often falls short of it: the
/// curve bends down past the target faster than it climbs before it, so a
/// spread of lengths around that length is copied whole less often than the
/// length itself. A range that is all too easy leans on its longest groups,
/// and one that is all too hard on its shortest.
fn length_weights(shortest: usize, longest: usize, target: f64, cell: usize) -> Vec<f64> {
    let (span, spread) = (span_of(cell), spread_of(cell));
    let whole: Vec<f64> = (shortest..=longest)
        .map(|length| p_whole(length as f64, span, spread))
        .collect();
    let expected = |aim: f64| -> f64 {
        weights_around(shortest, longest, aim)
            .iter()
            .zip(&whole)
            .map(|(weight, p)| weight * p)
            .sum()
    };
    let (mut low, mut high) = (shortest as f64 - AIM_REACH, longest as f64 + AIM_REACH);
    let aim = if expected(high) >= target {
        high
    } else if expected(low) <= target {
        low
    } else {
        // Aiming longer only ever copies less whole, so halve towards it.
        for _ in 0..30 {
            let middle = (low + high) / 2.0;
            if expected(middle) > target {
                low = middle;
            } else {
                high = middle;
            }
        }
        (low + high) / 2.0
    };
    weights_around(shortest, longest, aim)
}

/// The lengths from `shortest` to `longest`, leaning towards `aim`, with a
/// share spread evenly over them all.
fn weights_around(shortest: usize, longest: usize, aim: f64) -> Vec<f64> {
    let nearness: Vec<f64> = (shortest..=longest)
        .map(|length| {
            let off = length as f64 - aim;
            (-off * off / (2.0 * AIM_WIDTH * AIM_WIDTH)).exp()
        })
        .collect();
    let total: f64 = nearness.iter().sum();
    let even = VARIETY / nearness.len() as f64;
    // Far past either end every weight can underflow to nothing: then the
    // end nearest the aim takes all of the leaning.
    let nearest_end = if aim > longest as f64 {
        nearness.len() - 1
    } else {
        0
    };
    nearness
        .into_iter()
        .enumerate()
        .map(|(index, weight)| {
            let leaning = if total > 0.0 {
                weight / total
            } else {
                f64::from(u8::from(index == nearest_end))
            };
            even + (1.0 - VARIETY) * leaning
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;
    use crate::session::GroupResult;

    /// A learner whose span is `span`: copies a group whole with the model's
    /// own chance, so the estimate has a true value to find.
    fn copies(length: usize, span: f64, spread: f64, rng: &mut impl Rng) -> bool {
        rng.f64() < p_whole(length as f64, span, spread)
    }

    fn session(timestamp: u64, groups: &[(&str, bool)]) -> SessionResult {
        SessionResult {
            timestamp,
            groups: groups
                .iter()
                .map(|(sent, correct)| GroupResult {
                    sent: (*sent).to_string(),
                    received: if *correct {
                        (*sent).to_string()
                    } else {
                        String::new()
                    },
                    correct: *correct,
                })
                .collect(),
            ..SessionResult::empty_for_test()
        }
    }

    fn koch() -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 10;
        settings
    }

    #[test]
    fn with_nothing_learned_the_guess_is_the_middle_of_the_range() {
        // A little above the centre: the grid stops at half a character,
        // which cuts more off the bottom of the guess than the top.
        let estimate = SpanBelief::new(6.0).estimate();
        assert!((estimate.mean - 6.0).abs() < 0.3, "{estimate:?}");
        assert!(
            estimate.sd > 2.0,
            "a first guess is a wide one: {estimate:?}"
        );
    }

    #[test]
    fn long_groups_copied_whole_raise_the_span_and_misses_lower_it() {
        let mut up = SpanBelief::new(5.0);
        let mut down = SpanBelief::new(5.0);
        for _ in 0..10 {
            up.learn(8, true);
            down.learn(3, false);
        }
        assert!(up.estimate().mean > 8.0, "{:?}", up.estimate());
        assert!(down.estimate().mean < 3.0, "{:?}", down.estimate());
    }

    #[test]
    fn one_slip_on_a_short_group_is_not_a_short_span() {
        let mut belief = SpanBelief::new(5.0);
        for _ in 0..8 {
            belief.learn(6, true);
        }
        let before = belief.estimate().mean;
        belief.learn(2, false);
        assert!(before - belief.estimate().mean < 0.5);
    }

    #[test]
    fn a_copy_is_whole_only_when_nothing_was_missed_or_added() {
        let mut whole = SpanBelief::new(5.0);
        let mut padded = SpanBelief::new(5.0);
        for _ in 0..6 {
            whole.learn_copy(&CopyTally::of("ABCDEFGH", "ABCDEFGH"));
            padded.learn_copy(&CopyTally::of("ABCDEFGH", "ABCDEFGHX"));
        }
        assert!(whole.estimate().mean > padded.estimate().mean + 2.0);
        assert_eq!(whole, {
            let mut by_hand = SpanBelief::new(5.0);
            for _ in 0..6 {
                by_hand.learn(8, true);
            }
            by_hand
        });
    }

    /// A simulated learner of span 6 is found within half a character, and
    /// the groups it is sent come back whole about as often as asked.
    #[test]
    fn a_simulated_learner_is_found_and_copies_near_the_target() {
        let mut rng = SplitMix64::new(11);
        let mut belief = SpanBelief::new(6.0);
        let mut whole = 0;
        for group in 0..300 {
            let length = belief.draw_length(2, 10, 0.7, &mut rng);
            let copied = copies(length, 6.0, 0.8, &mut rng);
            belief.learn(length, copied);
            if group >= 100 {
                whole += u32::from(copied);
            }
        }
        let estimate = belief.estimate();
        assert!((estimate.mean - 6.0).abs() < 0.5, "{estimate:?}");
        let rate = f64::from(whole) / 200.0;
        assert!((0.6..=0.8).contains(&rate), "copied whole {rate}");
    }

    /// For a span that is known, the lengths drawn are copied whole as often
    /// as each target asks, and a range that is all too easy or too hard
    /// leans on the end nearest the learner.
    #[test]
    fn the_lengths_drawn_are_copied_whole_as_often_as_asked() {
        let cell = |span: f64, spread: f64| {
            (0..CELLS)
                .find(|&cell| {
                    (span_of(cell) - span).abs() < 1e-9 && (spread_of(cell) - spread).abs() < 1e-9
                })
                .expect("a cell on the grid")
        };
        let known = cell(6.0, 1.0);
        for target in [0.85, 0.7, 0.5] {
            let weights = length_weights(2, 10, target, known);
            let whole: f64 = (2..=10)
                .zip(&weights)
                .map(|(length, weight)| weight * p_whole(f64::from(length), 6.0, 1.0))
                .sum();
            assert!((whole - target).abs() < 1e-6, "{target}: {whole}");
        }
        let too_easy = length_weights(2, 5, 0.7, cell(15.0, 1.0));
        let too_hard = length_weights(6, 9, 0.7, cell(2.0, 1.0));
        assert!(too_easy[3] > 0.8, "{too_easy:?}");
        assert!(too_hard[0] > 0.8, "{too_hard:?}");
    }

    #[test]
    fn lengths_stay_inside_the_range_whatever_the_generator_does() {
        struct Stuck(f64);
        impl Rng for Stuck {
            fn f64(&mut self) -> f64 {
                self.0
            }
        }
        let mut belief = SpanBelief::new(5.0);
        belief.learn(4, true);
        for stuck in [0.0, 0.5, 0.999_999, 1.0] {
            for (shortest, longest) in [(2, 9), (9, 2), (1, 20), (4, 4)] {
                let length = belief.draw_length(shortest, longest, 0.7, &mut Stuck(stuck));
                let (lo, hi) = (shortest.min(longest), shortest.max(longest));
                assert!((lo..=hi).contains(&length), "{length} outside {lo}..={hi}");
            }
        }
    }

    #[test]
    fn the_forecast_adds_up_and_longer_groups_are_harder() {
        let mut belief = SpanBelief::new(5.0);
        for _ in 0..5 {
            belief.learn(5, true);
            belief.learn(7, false);
        }
        let rows = belief.forecast(2, 9, 0.7);
        assert_eq!(rows.len(), 8);
        let drawn: f64 = rows.iter().map(|row| row.drawn).sum();
        assert!((drawn - 1.0).abs() < 1e-9, "{drawn}");
        assert!(rows.windows(2).all(|pair| pair[0].whole > pair[1].whole));
        // The lengths that land nearest the target are the likeliest.
        let likeliest = rows
            .iter()
            .max_by(|a, b| a.drawn.total_cmp(&b.drawn))
            .expect("a row");
        assert!((0.5..0.9).contains(&likeliest.whole), "{likeliest:?}");
    }

    #[test]
    fn the_history_is_learned_oldest_first_and_older_sessions_count_for_less() {
        let run = |from: u64, groups: &[(&str, bool)]| -> Vec<SessionResult> {
            (from..from + 15).map(|at| session(at, groups)).collect()
        };
        let long_ok = [("ABCDEFGHI", true); 10];
        let short_miss = [("ABC", false); 10];
        // Stored newest first: what was practised last must count most.
        let improving = [run(100, &long_ok), run(0, &short_miss)].concat();
        let slipping = [run(100, &short_miss), run(0, &long_ok)].concat();
        let up = SpanBelief::from_history(&improving, &koch()).estimate();
        let down = SpanBelief::from_history(&slipping, &koch()).estimate();
        assert!(up.mean > down.mean + 1.0, "{up:?} vs {down:?}");
    }

    #[test]
    fn only_sessions_of_the_set_being_trained_count() {
        let mut other = session(1, &[("ABCDEFGHIJ", true); 10]);
        other.char_set_mode = CharSetMode::Mixed;
        let belief = SpanBelief::from_history(&[other.clone()], &koch());
        assert_eq!(belief, SpanBelief::new(6.0));
        // A callsign is as long as it is, so callsigns teach nothing.
        let mut calls = koch();
        calls.curriculum.char_set_mode = CharSetMode::Callsign;
        other.char_set_mode = CharSetMode::Callsign;
        assert!(SpanBelief::history(&[other], &calls).is_empty());
    }

    #[test]
    fn the_history_gives_one_point_per_session() {
        let history = [
            session(1, &[("ABCD", true); 5]),
            session(2, &[("ABCDEF", true); 5]),
            session(3, &[("ABCDEFGH", true); 5]),
        ];
        let points = SpanBelief::history(&history, &koch());
        assert_eq!(
            points.iter().map(|p| p.timestamp).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(
            points
                .windows(2)
                .all(|pair| pair[1].estimate.mean > pair[0].estimate.mean)
        );
        assert_eq!(
            points.last().map(|p| p.estimate),
            Some(SpanBelief::from_history(&history, &koch()).estimate())
        );
    }
}
