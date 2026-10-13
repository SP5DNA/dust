//! The browser's band runs ahead of the audio clock in queued buffers, each
//! made for the moment it will play. This is the arithmetic of keeping that
//! queue honest when a pile-up starts or stops calling across a send: which
//! buffers were made for the wrong number of callers, and can still be made
//! again before they are heard.

/// A pile-up calling across a send: how many stations, and until when on the
/// audio clock. The band's own traffic makes room for them until then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reservation {
    callers: usize,
    until: f64,
}

impl Reservation {
    /// `callers` stations calling until `until`; none at all is no
    /// reservation.
    pub fn new(callers: usize, until: f64) -> Option<Self> {
        (callers > 0).then_some(Self { callers, until })
    }
}

/// How many callers a buffer starting at `start` makes room for.
pub fn callers_at(reservation: Option<Reservation>, start: f64) -> usize {
    match reservation {
        Some(Reservation { callers, until }) if start < until => callers,
        _ => 0,
    }
}

/// Of the buffers queued, each given as when it starts and how many callers
/// it was made for, the first that has to be made again under
/// `reservation`: the first made for a different number of callers that
/// starts no sooner than `not_before`, the earliest a buffer can still be
/// taken back before it plays. Every buffer after it goes too — each carries
/// on from where the generators left the one before — and is made again in
/// turn.
pub fn first_to_remake(
    queued: impl IntoIterator<Item = (f64, usize)>,
    reservation: Option<Reservation>,
    not_before: f64,
) -> Option<usize> {
    queued.into_iter().position(|(start, made_for)| {
        start >= not_before && made_for != callers_at(reservation, start)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_holds_until_its_pile_up_stops_calling() {
        let pileup = Reservation::new(3, 10.0);
        assert_eq!(callers_at(pileup, 9.9), 3);
        assert_eq!(callers_at(pileup, 10.0), 0);
        assert_eq!(callers_at(None, 0.0), 0);
        assert_eq!(Reservation::new(0, 10.0), None);
    }

    /// A send starting with a pile-up remakes the band queued after the
    /// earliest moment it can, rather than leaving half a second of it made
    /// with nobody calling; and only what is wrong is remade.
    #[test]
    fn only_buffers_made_for_the_wrong_callers_and_not_yet_due_are_remade() {
        // Buffers a tenth of a second long, from 0.0 on, made with nobody
        // calling.
        let queued: Vec<(f64, usize)> = (0..5).map(|k| (f64::from(k) / 10.0, 0)).collect();
        let pileup = Reservation::new(2, 3.0);
        // The first two are too close to the clock to take back.
        assert_eq!(first_to_remake(queued.clone(), pileup, 0.15), Some(2));
        // Nothing to do when they were made for the reservation already.
        let made: Vec<(f64, usize)> = queued.iter().map(|(t, _)| (*t, 2)).collect();
        assert_eq!(first_to_remake(made.clone(), pileup, 0.0), None);
        // A stop clears the reservation, and what was made for the callers
        // is remade without them.
        assert_eq!(first_to_remake(made, None, 0.25), Some(3));
        // A pile-up that has stopped calling by the time a buffer plays is
        // not made room for in it.
        let short = Reservation::new(2, 0.25);
        assert_eq!(first_to_remake(queued, short, 0.0), Some(0));
    }

    /// A queue made under a pile-up from its first buffer, each buffer made
    /// for the callers `callers_at` gives its start, has nothing to remake
    /// for that pile-up, the buffers too close to the clock to take back
    /// included; a stop still remakes whatever was made for the callers and
    /// is not yet due. That is the queue a band started under a send in
    /// flight is meant to hold. This checks the arithmetic on such a queue,
    /// not the web stream that builds one.
    #[test]
    fn a_queue_made_under_a_pile_up_has_nothing_to_remake_for_it() {
        let made_under = |pileup: Option<Reservation>| -> Vec<(f64, usize)> {
            (0..5)
                .map(|k| {
                    let start = f64::from(k) / 10.0;
                    (start, callers_at(pileup, start))
                })
                .collect()
        };
        let pileup = Reservation::new(2, 0.35);
        let queued = made_under(pileup);
        let made_for: Vec<usize> = queued.iter().map(|(_, callers)| *callers).collect();
        assert_eq!(made_for, [2, 2, 2, 2, 0]);
        assert_eq!(first_to_remake(queued.clone(), pileup, 0.0), None);
        assert_eq!(first_to_remake(queued.clone(), pileup, 0.15), None);
        assert_eq!(first_to_remake(queued, None, 0.15), Some(2));
        // One that has stopped calling by the time the band starts is made
        // room for nowhere, as if there were none.
        assert_eq!(made_under(Reservation::new(2, 0.0)), made_under(None));
    }
}
