//! Leaning a draw towards an aim. Candidates — group lengths, speeds — are
//! weighted by how near they are to an aim, with a share spread wider so the
//! rest still come round, and the aim is set so that over everything drawn
//! the chance of success is a target.
//!
//! Aiming at the one candidate that succeeds exactly that often falls short
//! of it: the chance of success bends down past the target faster than it
//! climbs before it, so a spread of candidates around that one succeeds less
//! often than the one itself (Jensen's inequality). So the aim is searched
//! for instead. Candidates are in order of difficulty, each one harder than
//! the last, so aiming further along only ever succeeds less, and halving
//! finds it. A set of candidates all too easy leans on its hardest end, and
//! one all too hard on its easiest.

/// How a draw leans: how close to the aim it keeps, and how its wider share
/// is spread.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Leaning {
    /// How far from the aim, in the candidates' own units, a candidate can
    /// be and still be favoured.
    pub width: f64,
    /// The share of the draw spread wider.
    pub variety: f64,
    /// How that share is spread.
    pub spread: Spread,
    /// How far past either end of the candidates the aim may go: far enough
    /// that the end itself takes nearly all of the leaning.
    pub reach: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Spread {
    /// Evenly over every candidate.
    Even,
    /// Around the aim, this far either way: close enough that a draw far
    /// from it stays rare.
    Around(f64),
}

fn nearness(position: f64, aim: f64, width: f64) -> f64 {
    let off = position - aim;
    (-off * off / (2.0 * width * width)).exp()
}

/// Each candidate's share of a spread `width` wide around `aim`. Should
/// every nearness underflow, the end nearest the aim takes all of it.
fn shares(positions: &[f64], aim: f64, width: f64) -> Vec<f64> {
    let near: Vec<f64> = positions.iter().map(|p| nearness(*p, aim, width)).collect();
    let total: f64 = near.iter().sum();
    let end = if positions.last().is_some_and(|last| aim > *last) {
        positions.len().saturating_sub(1)
    } else {
        0
    };
    near.iter()
        .enumerate()
        .map(|(index, weight)| {
            if total > 0.0 {
                weight / total
            } else {
                f64::from(u8::from(index == end))
            }
        })
        .collect()
}

/// Each candidate's share of the draw when it aims at `aim`.
pub(crate) fn weights(positions: &[f64], aim: f64, leaning: &Leaning) -> Vec<f64> {
    let close = shares(positions, aim, leaning.width);
    let wide = match leaning.spread {
        Spread::Even => vec![1.0 / positions.len().max(1) as f64; positions.len()],
        Spread::Around(width) => shares(positions, aim, width),
    };
    close
        .iter()
        .zip(&wide)
        .map(|(close, wide)| (1.0 - leaning.variety) * close + leaning.variety * wide)
        .collect()
}

/// The chance of success over the draw aimed at `aim`, `chances` holding
/// each candidate's.
fn expected(positions: &[f64], chances: &[f64], aim: f64, leaning: &Leaning) -> f64 {
    weights(positions, aim, leaning)
        .iter()
        .zip(chances)
        .map(|(weight, chance)| weight * chance)
        .sum()
}

/// The aim at which the draw succeeds `target` of the time.
pub(crate) fn aim_for(positions: &[f64], chances: &[f64], target: f64, leaning: &Leaning) -> f64 {
    let (Some(first), Some(last)) = (positions.first(), positions.last()) else {
        return 0.0;
    };
    let expected = |aim: f64| expected(positions, chances, aim, leaning);
    let (mut easy, mut hard) = (first - leaning.reach, last + leaning.reach);
    if expected(hard) >= target {
        return hard;
    }
    if expected(easy) <= target {
        return easy;
    }
    for _ in 0..30 {
        let middle = (easy + hard) / 2.0;
        if expected(middle) > target {
            easy = middle;
        } else {
            hard = middle;
        }
    }
    (easy + hard) / 2.0
}

/// Each candidate's share of a draw that succeeds `target` of the time.
pub(crate) fn lean(positions: &[f64], chances: &[f64], target: f64, leaning: &Leaning) -> Vec<f64> {
    weights(
        positions,
        aim_for(positions, chances, target, leaning),
        leaning,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVEN: Leaning = Leaning {
        width: 1.0,
        variety: 0.1,
        spread: Spread::Even,
        reach: 3.0,
    };

    /// Lengths 2 to 10, each harder than the last.
    fn lengths() -> (Vec<f64>, Vec<f64>) {
        let positions: Vec<f64> = (2..=10).map(f64::from).collect();
        let chances = positions
            .iter()
            .map(|length| 1.0 / (1.0 + (length - 5.0_f64).exp()))
            .collect();
        (positions, chances)
    }

    #[test]
    fn the_draw_succeeds_as_often_as_asked() {
        let (positions, chances) = lengths();
        for target in [0.5, 0.7, 0.85] {
            let drawn = lean(&positions, &chances, target, &EVEN);
            let total: f64 = drawn.iter().sum();
            let success: f64 = drawn.iter().zip(&chances).map(|(w, p)| w * p).sum();
            assert!((total - 1.0).abs() < 1e-9);
            assert!((success - target).abs() < 1e-6, "{success} for {target}");
        }
    }

    #[test]
    fn too_easy_leans_on_the_hardest_and_too_hard_on_the_easiest() {
        let (positions, _) = lengths();
        let easy = vec![0.99; positions.len()];
        let hard = vec![0.01; positions.len()];
        let to_easy = lean(&positions, &easy, 0.7, &EVEN);
        let to_hard = lean(&positions, &hard, 0.7, &EVEN);
        assert!(to_easy.last() > to_easy.first());
        assert!(to_hard.first() > to_hard.last());
        assert_eq!(aim_for(&[], &[], 0.7, &EVEN), 0.0);
    }

    #[test]
    fn a_wider_share_around_the_aim_stays_near_it() {
        let positions: Vec<f64> = (0..=100).map(|n| f64::from(n) / 100.0).collect();
        let around = Leaning {
            width: 0.05,
            variety: 0.15,
            spread: Spread::Around(0.2),
            reach: 0.15,
        };
        let drawn = weights(&positions, 0.2, &around);
        let far: f64 = drawn
            .iter()
            .zip(&positions)
            .filter(|(_, p)| **p > 0.8)
            .map(|(w, _)| w)
            .sum();
        assert!(far < 1e-3, "{far} of the draw far from the aim");
        let even = weights(
            &positions,
            0.2,
            &Leaning {
                spread: Spread::Even,
                ..around
            },
        );
        let far_even: f64 = even
            .iter()
            .zip(&positions)
            .filter(|(_, p)| **p > 0.8)
            .map(|(w, _)| w)
            .sum();
        assert!(
            far_even > 0.02,
            "evenly spread, the far end still comes round"
        );
    }

    #[test]
    fn an_aim_past_every_candidate_still_leans_on_the_nearest_end() {
        let positions = [1.0, 2.0, 3.0];
        let narrow = Leaning {
            width: 1e-3,
            variety: 0.0,
            spread: Spread::Even,
            reach: 0.0,
        };
        assert_eq!(weights(&positions, 50.0, &narrow), vec![0.0, 0.0, 1.0]);
        assert_eq!(weights(&positions, -50.0, &narrow), vec![1.0, 0.0, 0.0]);
    }
}
