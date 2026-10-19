//! Letter diagnostics, bigrams, confusion, and sampling snapshots for the stats UI.

use std::collections::BTreeMap;

use crate::alignment::align_group;
use crate::belief::EstimatePoint;
use crate::morse::LCWO_SEQUENCE;
use crate::pool::compute_char_pool;
use crate::sampling::{
    CharSamplingConfig, CharSamplingState, adaptive_bounds, belief_for, beta_posterior_mean_error,
    compute_raw_sampling_weights, config_from_settings, generate_callsign_group,
    generate_word_group, group_size_bounds, normalize_weights,
};
use crate::session::SessionResult;
use crate::settings::{CharSetMode, TrainingSettings};
use crate::span::{LengthForecast, SpanBelief};
use crate::speed::SpeedBelief;

pub const MASTERED_MIN_ATTEMPTS: u32 = 5;
pub const MASTERED_MIN_ACCURACY: f64 = 0.9;
pub const BUILDING_MIN_ACCURACY: f64 = 0.7;
pub const SLOW_AVG_MS: f64 = 800.0;
pub const GROUP_START_BIGRAM_TOKEN: char = '▸';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MasteryStatus {
    Mastered,
    Building,
    Weak,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterDiagnostic {
    pub letter: char,
    pub accuracy_pct: f64,
    pub avg_ms: f64,
    pub total: u32,
    pub correct: u32,
    pub status: MasteryStatus,
    pub is_slow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccuracyPoint {
    pub date: String,
    pub accuracy_pct: f64,
    pub timestamp: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigramCell {
    pub row: char,
    pub col: char,
    pub rate: f64,
    pub total: u32,
    pub wrong: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigramHeatmap {
    pub letters: Vec<char>,
    pub cells: Vec<BigramCell>,
    pub max_rate: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfusionEntry {
    pub sent: char,
    pub typed: Option<char>,
    pub count: u32,
    pub percentage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SamplingRow {
    pub character: char,
    pub p_error: f64,
    pub sampling_prob: f64,
    pub alpha: f64,
    pub beta: f64,
    pub is_letter: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionHistoryRow {
    pub date: String,
    pub accuracy_pct: f64,
    pub score: f64,
    pub groups: u32,
    pub correct_groups: u32,
    pub total_chars: u32,
    pub timestamp: u64,
    /// Where the session sits in the history the rows were made from, so a
    /// row names one session even when two finished in the same millisecond.
    pub index: usize,
}

/// Sort rank for a character: its place in the LCWO sequence, or last for a
/// character that sequence never teaches (`+` is a Morse character but is not
/// part of LCWO, and any custom alphabet can hold others).
fn lcwo_index(ch: char) -> usize {
    LCWO_SEQUENCE
        .iter()
        .position(|c| *c == ch)
        .unwrap_or(usize::MAX)
}

pub fn classify_mastery(accuracy01: f64, attempts: u32) -> MasteryStatus {
    if attempts < MASTERED_MIN_ATTEMPTS {
        MasteryStatus::Weak
    } else if accuracy01 >= MASTERED_MIN_ACCURACY {
        MasteryStatus::Mastered
    } else if accuracy01 >= BUILDING_MIN_ACCURACY {
        MasteryStatus::Building
    } else {
        MasteryStatus::Weak
    }
}

pub fn accuracy_chart(sessions: &[SessionResult]) -> Vec<AccuracyPoint> {
    let mut rows: Vec<&SessionResult> = sessions.iter().collect();
    rows.sort_by_key(|s| s.timestamp);
    rows.into_iter()
        .map(|s| AccuracyPoint {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            timestamp: s.timestamp,
        })
        .collect()
}

pub fn character_diagnostics(sessions: &[SessionResult]) -> Vec<CharacterDiagnostic> {
    let mut totals: BTreeMap<char, (u32, u32)> = BTreeMap::new();
    let mut timings: BTreeMap<char, Vec<f64>> = BTreeMap::new();
    for session in sessions {
        for (letter, stats) in session.letter_accuracy().iter() {
            let entry = totals.entry(*letter).or_insert((0, 0));
            entry.0 += stats.correct;
            entry.1 += stats.total;
        }
        for (group, timing) in session.groups.iter().zip(session.group_timings.iter()) {
            if timing.time_to_complete_ms <= 0.0 {
                continue;
            }
            let sent: Vec<char> = group.sent.to_ascii_uppercase().chars().collect();
            if sent.is_empty() {
                continue;
            }
            let per = timing.time_to_complete_ms / sent.len() as f64;
            for ch in sent {
                timings.entry(ch).or_default().push(per);
            }
        }
    }
    let mut out: Vec<CharacterDiagnostic> = totals
        .into_iter()
        .map(|(letter, (correct, total))| {
            let accuracy01 = if total == 0 {
                0.0
            } else {
                f64::from(correct) / f64::from(total)
            };
            let samples = timings.get(&letter);
            let avg_ms = samples
                .map(|s| s.iter().sum::<f64>() / s.len() as f64)
                .unwrap_or(0.0);
            let status = classify_mastery(accuracy01, total);
            let is_slow = matches!(status, MasteryStatus::Mastered | MasteryStatus::Building)
                && avg_ms >= SLOW_AVG_MS;
            CharacterDiagnostic {
                letter,
                accuracy_pct: accuracy01 * 100.0,
                avg_ms,
                total,
                correct,
                status,
                is_slow,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        let rank = |s: MasteryStatus| match s {
            MasteryStatus::Weak => 0,
            MasteryStatus::Building => 1,
            MasteryStatus::Mastered => 2,
        };
        rank(a.status).cmp(&rank(b.status)).then(
            a.accuracy_pct
                .partial_cmp(&b.accuracy_pct)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    out
}

/// What the mistakes screen shows: which characters are taken for which,
/// and which pairs of characters are hard together.
#[derive(Clone, Debug, PartialEq)]
pub struct MistakeStats {
    /// The most common confusions, most common first.
    pub confusions: Vec<ConfusionEntry>,
    pub bigrams: BigramHeatmap,
}

/// Both mistake views from one alignment of every group, rather than one
/// each: the history can be hundreds of sessions long.
pub fn mistake_stats(sessions: &[SessionResult], confusion_limit: usize) -> MistakeStats {
    let mut confused: BTreeMap<(char, Option<char>), u32> = BTreeMap::new();
    let mut sent_totals: BTreeMap<char, u32> = BTreeMap::new();
    let mut letters = Vec::new();
    let mut pairs: BTreeMap<(char, char), (u32, u32)> = BTreeMap::new();
    for group in sessions.iter().flat_map(|session| &session.groups) {
        let mut previous = GROUP_START_BIGRAM_TOKEN;
        for pair in align_group(&group.sent, &group.received) {
            let Some(sent) = pair.sent_char else {
                continue;
            };
            *sent_totals.entry(sent).or_insert(0) += 1;
            if !pair.matched {
                *confused.entry((sent, pair.received_char)).or_insert(0) += 1;
            }
            for ch in [previous, sent] {
                if !letters.contains(&ch) {
                    letters.push(ch);
                }
            }
            let entry = pairs.entry((previous, sent)).or_insert((0, 0));
            entry.1 += 1;
            if !pair.matched {
                entry.0 += 1;
            }
            previous = sent;
        }
    }
    MistakeStats {
        confusions: confusion_rows(confused, &sent_totals, confusion_limit),
        bigrams: bigram_grid(letters, &pairs),
    }
}

fn confusion_rows(
    confused: BTreeMap<(char, Option<char>), u32>,
    sent_totals: &BTreeMap<char, u32>,
    limit: usize,
) -> Vec<ConfusionEntry> {
    let mut rows: Vec<ConfusionEntry> = confused
        .into_iter()
        .map(|((sent, typed), count)| {
            let total = sent_totals.get(&sent).copied().unwrap_or(1);
            ConfusionEntry {
                sent,
                typed,
                count,
                percentage: 100.0 * f64::from(count) / f64::from(total),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.count.cmp(&a.count).then(
            b.percentage
                .partial_cmp(&a.percentage)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rows.truncate(limit);
    rows
}

fn bigram_grid(
    mut letters: Vec<char>,
    pairs: &BTreeMap<(char, char), (u32, u32)>,
) -> BigramHeatmap {
    // The start-of-group token sorts first, so every real character is shifted
    // one rank up. `lcwo_index` saturates for characters outside the sequence,
    // which would otherwise overflow here and panic in a debug build.
    letters.sort_by_key(|c| {
        if *c == GROUP_START_BIGRAM_TOKEN {
            0
        } else {
            lcwo_index(*c).saturating_add(1)
        }
    });
    let mut max_rate: f64 = 0.0;
    let mut cells = Vec::with_capacity(letters.len() * letters.len());
    for &row in &letters {
        for &col in &letters {
            let (wrong, total) = pairs.get(&(row, col)).copied().unwrap_or((0, 0));
            let rate = if total == 0 {
                0.0
            } else {
                f64::from(wrong) / f64::from(total)
            };
            max_rate = max_rate.max(rate);
            cells.push(BigramCell {
                row,
                col,
                rate,
                total,
                wrong,
            });
        }
    }
    BigramHeatmap {
        letters,
        cells,
        max_rate,
    }
}

/// How often each character actually turns up, measured by running the real
/// callsign generator rather than modelling it. Nothing else can answer this:
/// the share of `Q` depends on the prefix tables and the tier, not on a weight.
fn whole_group_character_shares(
    settings: &TrainingSettings,
    state: &CharSamplingState,
    config: &CharSamplingConfig,
    pool: &[char],
    rng: &mut impl crate::rng::Rng,
) -> BTreeMap<char, f64> {
    const DRAWS: usize = 1_000;
    let mut counts: BTreeMap<char, f64> = pool.iter().map(|c| (*c, 0.0)).collect();
    let mut total = 0.0;
    let mut sampling = state.clone();
    for _ in 0..DRAWS {
        let call = if settings.curriculum.char_set_mode == CharSetMode::Words {
            generate_word_group(settings, &mut sampling, config, rng)
        } else {
            generate_callsign_group(settings, &mut sampling, config, rng)
        };
        for ch in call.chars() {
            if let Some(count) = counts.get_mut(&ch) {
                *count += 1.0;
                total += 1.0;
            }
        }
    }
    if total <= 0.0 {
        return counts;
    }
    for count in counts.values_mut() {
        *count /= total;
    }
    counts
}

pub fn sampling_rows(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<SamplingRow> {
    let state = CharSamplingState::from_history(sessions, settings);
    let pool = compute_char_pool(settings);
    let mut config = config_from_settings(settings);
    config.thompson_sampling = false;
    let mut rng = crate::rng::SplitMix64::default();
    let probs = if !settings.curriculum.char_set_mode.draws_characters() {
        // Callsigns and words are not drawn character by character, so a
        // per-character weight would not describe them. Ask the real
        // generator instead.
        whole_group_character_shares(settings, &state, &config, &pool, &mut rng)
    } else {
        let weights = compute_raw_sampling_weights(&pool, &state, &config, &mut rng);
        normalize_weights(&pool, &weights)
    };
    pool.into_iter()
        .map(|character| {
            let belief = belief_for(&state, character);
            SamplingRow {
                character,
                p_error: beta_posterior_mean_error(belief),
                sampling_prob: probs.get(&character).copied().unwrap_or(0.0),
                alpha: belief.alpha,
                beta: belief.beta,
                is_letter: !character.is_ascii_digit(),
            }
        })
        .collect()
}

pub fn session_history(sessions: &[SessionResult]) -> Vec<SessionHistoryRow> {
    let mut rows: Vec<SessionHistoryRow> = sessions
        .iter()
        .enumerate()
        .map(|(index, s)| SessionHistoryRow {
            date: s.date.clone(),
            accuracy_pct: s.accuracy * 100.0,
            score: s.score,
            groups: s.groups.len() as u32,
            correct_groups: s.groups.iter().filter(|g| g.correct).count() as u32,
            total_chars: s.total_chars,
            timestamp: s.timestamp,
            index,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
    rows
}

/// Your copy span after each session of the character set `settings`
/// trains, oldest first. Empty for callsigns, which are as long as they are.
pub fn span_history(settings: &TrainingSettings, sessions: &[SessionResult]) -> Vec<EstimatePoint> {
    SpanBelief::history(sessions, settings)
}

/// Your copy speed after each session of the character set `settings`
/// trains, oldest first: the speed at which you copy the share of characters
/// their "Aim for" asks, in WPM.
pub fn speed_history(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<EstimatePoint> {
    SpeedBelief::history(
        sessions,
        settings,
        settings.curriculum.group_size_target.char_rate(),
    )
}

/// Each group length the next session can send: how likely it is to come
/// next, and how often a group that long is copied whole. With adaptive group
/// size off, every length is as likely as the next. Empty for callsigns and
/// words, which are as long as they are.
pub fn group_length_forecast(
    settings: &TrainingSettings,
    sessions: &[SessionResult],
) -> Vec<LengthForecast> {
    if !settings.curriculum.char_set_mode.draws_characters() {
        return Vec::new();
    }
    let (shortest, longest) = group_size_bounds(settings);
    let target =
        adaptive_bounds(settings).map(|_| settings.curriculum.group_size_target.success_rate());
    SpanBelief::from_history(sessions, settings).forecast(shortest, longest, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alignment::{CopyTally, LetterAccuracy};
    use crate::session::{GroupResult, SessionTiming};
    use crate::settings::CharSetMode;

    fn session() -> SessionResult {
        SessionResult {
            timestamp: 1,
            groups: vec![GroupResult {
                sent: "KM".into(),
                received: "K?".into(),
                correct: false,
            }],
            group_timings: vec![SessionTiming {
                time_to_complete_ms: 400.0,
                char_wpm: None,
                effective_wpm: None,
                sends: 1,
            }],
            accuracy: 0.5,
            tally: CopyTally::of("KM", "K?"),
            avg_response_ms: 400.0,
            total_chars: 2,
            score: 1.0,
            ..SessionResult::empty_for_test()
        }
    }

    #[test]
    fn diagnostics_and_confusion() {
        let sessions = [session()];
        let diag = character_diagnostics(&sessions);
        assert_eq!(diag.len(), 2);
        assert!(!mistake_stats(&sessions, 8).confusions.is_empty());
    }

    fn plain(date: &str, timestamp: u64, groups: &[(&str, &str)]) -> SessionResult {
        let mut out = session();
        out.date = date.to_string();
        out.timestamp = timestamp;
        out.tally = CopyTally::default();
        out.groups = groups
            .iter()
            .map(|(sent, received)| GroupResult {
                sent: (*sent).to_string(),
                received: (*received).to_string(),
                correct: sent == received,
            })
            .collect();
        out.group_timings = groups
            .iter()
            .map(|(sent, _)| SessionTiming {
                time_to_complete_ms: 1000.0 * sent.chars().count() as f64,
                char_wpm: Some(20.0),
                effective_wpm: None,
                sends: 1,
            })
            .collect();
        out
    }

    #[test]
    fn letter_accuracy_falls_back_to_the_stored_groups() {
        // Sessions written before letter_accuracy existed still show per-letter rows.
        let legacy = plain("2026-09-01", 1, &[("KM", "KX")]);
        let diag = character_diagnostics(&[legacy]);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!((m.correct, m.total), (0, 1));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!((k.correct, k.total), (1, 1));
    }

    #[test]
    fn mastery_needs_both_attempts_and_accuracy() {
        assert_eq!(classify_mastery(1.0, 4), MasteryStatus::Weak);
        assert_eq!(
            classify_mastery(MASTERED_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Mastered
        );
        assert_eq!(
            classify_mastery(BUILDING_MIN_ACCURACY, MASTERED_MIN_ATTEMPTS),
            MasteryStatus::Building
        );
        assert_eq!(classify_mastery(0.1, 50), MasteryStatus::Weak);
    }

    #[test]
    fn diagnostics_rank_weak_letters_first_and_flag_slow_ones() {
        // Ten sends of each letter: K always right and slow, M always wrong.
        let groups: Vec<(&str, &str)> = vec![("KM", "KX"); 10];
        let session = plain("2026-09-01", 1, &groups);
        let diag = character_diagnostics(&[session]);
        assert_eq!(diag.first().map(|d| d.letter), Some('M'));
        let k = diag.iter().find(|d| d.letter == 'K').expect("K row");
        assert_eq!(k.status, MasteryStatus::Mastered);
        // 2000 ms over two characters is 1000 ms each, past the slow mark.
        assert!(k.is_slow);
        assert!(k.avg_ms >= SLOW_AVG_MS);
        let m = diag.iter().find(|d| d.letter == 'M').expect("M row");
        assert_eq!(m.status, MasteryStatus::Weak);
        // A weak letter is never called slow on top of it.
        assert!(!m.is_slow);
    }

    #[test]
    fn zero_length_timings_are_not_averaged_in() {
        let mut session = plain("2026-09-01", 1, &[("KM", "KM")]);
        session.group_timings[0].time_to_complete_ms = 0.0;
        let diag = character_diagnostics(&[session]);
        assert!(diag.iter().all(|d| d.avg_ms == 0.0));
    }

    #[test]
    fn the_accuracy_chart_is_ordered_by_time() {
        let newest = plain("2026-09-03", 30, &[("KM", "KM")]);
        let oldest = plain("2026-09-01", 10, &[("KM", "KX")]);
        let chart = accuracy_chart(&[newest, oldest]);
        assert_eq!(
            chart.iter().map(|p| p.timestamp).collect::<Vec<_>>(),
            vec![10, 30]
        );
        assert_eq!(chart[0].date, "2026-09-01");
        assert_eq!(chart[0].accuracy_pct, 50.0);
        assert!(accuracy_chart(&[]).is_empty());
    }

    #[test]
    fn bigrams_rank_the_group_start_first_and_unknown_characters_last() {
        // `+` is a Morse character the LCWO sequence never teaches; it must not
        // overflow the sort key.
        let session = plain("2026-09-01", 1, &[("K+", "KM")]);
        let heat = mistake_stats(&[session], 8).bigrams;
        assert_eq!(heat.letters.first(), Some(&GROUP_START_BIGRAM_TOKEN));
        assert_eq!(heat.letters.last(), Some(&'+'));
        assert_eq!(heat.cells.len(), heat.letters.len() * heat.letters.len());
        // K→+ was copied as K→M: one attempt, one error.
        let cell = heat
            .cells
            .iter()
            .find(|c| c.row == 'K' && c.col == '+')
            .expect("K+ cell");
        assert_eq!((cell.total, cell.wrong), (1, 1));
        assert_eq!(cell.rate, 1.0);
        assert_eq!(heat.max_rate, 1.0);
        // An empty history draws no grid at all.
        assert!(mistake_stats(&[], 8).bigrams.letters.is_empty());
    }

    #[test]
    fn confusion_is_ordered_and_capped() {
        let session = plain(
            "2026-09-01",
            1,
            &[("KMU", "XMU"), ("KMU", "XMU"), ("KMU", "KMR")],
        );
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows[0].sent, 'K');
        assert_eq!(rows[0].typed, Some('X'));
        assert_eq!(rows[0].count, 2);
        // Two of the three K sends were copied as X.
        assert!((rows[0].percentage - 200.0 / 3.0).abs() < 1e-9);
        assert!(mistake_stats(&[], 8).confusions.is_empty());
        // The limit is honoured.
        let long = plain("2026-09-02", 2, &[("KMURE", "XXXXX")]);
        assert_eq!(mistake_stats(&[long], 2).confusions.len(), 2);
    }

    #[test]
    fn a_dropped_character_is_reported_as_no_answer() {
        let session = plain("2026-09-01", 1, &[("KM", "K")]);
        let rows = mistake_stats(&[session], 8).confusions;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sent, 'M');
        assert_eq!(rows[0].typed, None);
    }

    #[test]
    fn characters_that_were_never_sent_are_left_out_of_every_table() {
        // An extra character was typed in the middle of the group.
        let session = plain("2026-09-01", 1, &[("KM", "KXM")]);
        let diagnostics = character_diagnostics(std::slice::from_ref(&session));
        assert!(diagnostics.iter().all(|d| d.letter != 'X'));
        let mistakes = mistake_stats(&[session], 8);
        assert!(!mistakes.bigrams.letters.contains(&'X'));
        assert!(mistakes.confusions.iter().all(|c| c.sent != 'X'));
    }

    #[test]
    fn history_is_newest_first() {
        let a = plain("2026-09-01", 10, &[("KM", "KM"), ("KM", "KX")]);
        let b = plain("2026-09-02", 20, &[("KM", "KM")]);
        let rows = session_history(&[a, b]);
        assert_eq!(rows[0].date, "2026-09-02");
        assert_eq!(rows[1].groups, 2);
        assert_eq!(rows[1].correct_groups, 1);
        assert_eq!(rows[1].total_chars, 2);
        assert_eq!(rows[1].accuracy_pct, 50.0);
        assert!(session_history(&[]).is_empty());
        // Each row still names the session it came from.
        assert_eq!((rows[0].index, rows[1].index), (1, 0));
    }

    #[test]
    fn sampling_rows_follow_the_pool_it_falls_back_to() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.mixed_letters_percent = 100;
        settings.curriculum.custom_sequence = vec!['0'];
        // A letters-only mixed pool over a digits-only sequence sends digits.
        let rows = sampling_rows(&settings, &[]);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| !row.is_letter));
    }

    #[test]
    fn sampling_rows_mark_digits_and_sum_to_one() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Mixed;
        settings.curriculum.level = 1;
        settings.curriculum.digits_level = 1;
        let rows = sampling_rows(&settings, &[]);
        assert!(rows.iter().any(|r| !r.is_letter));
        assert!(rows.iter().any(|r| r.is_letter));
        let total: f64 = rows.iter().map(|r| r.sampling_prob).sum();
        assert!((total - 1.0).abs() < 1e-9);
        // With no history every character starts on the same flat prior.
        assert!(rows.iter().all(|r| (r.p_error - 0.5).abs() < 1e-9));
    }

    #[test]
    fn callsign_sampling_shows_what_callsigns_actually_contain() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        let rows = sampling_rows(&settings, &[]);

        let pool = compute_char_pool(&settings);
        assert_eq!(rows.len(), pool.len());
        let total: f64 = rows.iter().map(|row| row.sampling_prob).sum();
        assert!(
            (total - 1.0).abs() < 1e-6,
            "the shares should account for every character sent, got {total}"
        );
        assert!(
            rows.iter().all(|row| row.sampling_prob > 0.0),
            "every character a callsign can contain should turn up"
        );
        // A share is measured, not modelled: the separator is a digit on every
        // single call, so digits cannot be the rare ones here.
        let digits: f64 = rows
            .iter()
            .filter(|row| row.character.is_ascii_digit())
            .map(|row| row.sampling_prob)
            .sum();
        assert!(digits > 0.1, "digits carry every separator, got {digits}");

        // A tier that sends no portables must never show a slash.
        let mut tier_one = settings;
        tier_one.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MIN;
        assert!(
            !sampling_rows(&tier_one, &[])
                .iter()
                .any(|row| row.character == '/'),
            "tier 1 has no portable calls"
        );
    }

    #[test]
    fn callsign_sampling_shows_mean_weights_even_with_thompson_sampling_on() {
        // The table describes the average draw, so random Thompson draws must
        // not leak into it, whatever the setting says.
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        settings.curriculum.callsign_level = crate::callsign::CALLSIGN_TIER_MAX;
        settings.auto_level.char_sampling_thompson = false;
        let shares = |rows: Vec<SamplingRow>| -> Vec<(char, f64)> {
            rows.into_iter()
                .map(|row| (row.character, row.sampling_prob))
                .collect()
        };
        let without = shares(sampling_rows(&settings, &[]));
        settings.auto_level.char_sampling_thompson = true;
        assert_eq!(shares(sampling_rows(&settings, &[])), without);
    }

    #[test]
    fn sampling_rows_ignore_other_char_set_modes() {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        let mut koch = session();
        koch.char_set_mode = CharSetMode::Koch;
        koch.tally = CopyTally::default();
        koch.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 10,
                total: 10,
            },
        );
        let mut digits = koch.clone();
        digits.char_set_mode = CharSetMode::Digits;
        digits.tally.sent.insert(
            'K',
            LetterAccuracy {
                correct: 0,
                total: 10,
            },
        );
        let k_belief = |rows: &[SamplingRow]| {
            rows.iter()
                .find(|row| row.character == 'K')
                .map(|row| (row.alpha, row.beta))
                .expect("K in pool")
        };
        let only_koch = k_belief(&sampling_rows(&settings, &[koch.clone()]));
        let mixed_history = k_belief(&sampling_rows(&settings, &[koch, digits]));
        assert_eq!(only_koch, mixed_history);
    }

    fn length_settings(adaptive: bool) -> TrainingSettings {
        let mut settings = TrainingSettings::default();
        settings.curriculum.char_set_mode = CharSetMode::Koch;
        settings.curriculum.min_group_size = 2;
        settings.curriculum.max_group_size = 7;
        settings.curriculum.adaptive_group_size = adaptive;
        settings
    }

    #[test]
    fn with_adaptive_length_off_every_length_is_as_likely() {
        let rows = group_length_forecast(&length_settings(false), &[session()]);
        assert_eq!(
            rows.iter().map(|row| row.length).collect::<Vec<_>>(),
            [2, 3, 4, 5, 6, 7]
        );
        assert!(rows.iter().all(|row| (row.drawn - 1.0 / 6.0).abs() < 1e-12));
        // What is copied whole is still the model's to say.
        assert!(rows.windows(2).all(|pair| pair[0].whole > pair[1].whole));
    }

    #[test]
    fn with_adaptive_length_on_the_lengths_lean_and_still_add_up() {
        let rows = group_length_forecast(&length_settings(true), &[session()]);
        let total: f64 = rows.iter().map(|row| row.drawn).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert!(rows.iter().any(|row| (row.drawn - 1.0 / 6.0).abs() > 0.01));
    }

    #[test]
    fn callsigns_have_no_group_length_to_show() {
        let mut settings = length_settings(true);
        settings.curriculum.char_set_mode = CharSetMode::Callsign;
        assert!(group_length_forecast(&settings, &[session()]).is_empty());
        assert!(span_history(&settings, &[session()]).is_empty());
    }

    #[test]
    fn the_span_history_has_a_point_for_each_session_of_the_set() {
        let mut later = session();
        later.timestamp = 2;
        let points = span_history(&length_settings(false), &[session(), later]);
        assert_eq!(points.len(), 2);
    }
}
