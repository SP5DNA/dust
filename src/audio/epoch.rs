//! Cancellation by epoch, as every player keeps it.

use std::cell::Cell;
use std::rc::Rc;

/// A player's count of who holds the audio. Each send is armed with the next
/// number; a stop, or a newer send, moves the count on, and every send armed
/// before then reads as cancelled — so an old waiter can never claim a new
/// send's ending.
#[derive(Clone, Default)]
pub struct Epoch(Rc<Cell<u64>>);

/// One send's claim on the audio.
pub struct Ticket {
    epoch: Epoch,
    mine: u64,
}

impl Epoch {
    /// Claim the audio for a new send.
    pub fn arm(&self) -> Ticket {
        self.bump();
        Ticket {
            epoch: self.clone(),
            mine: self.0.get(),
        }
    }

    /// Take the audio back: whatever was armed before is cancelled.
    pub fn bump(&self) {
        self.0.set(self.0.get().wrapping_add(1));
    }
}

impl Ticket {
    /// True once something else has claimed the audio.
    pub fn is_cancelled(&self) -> bool {
        self.epoch.0.get() != self.mine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_current_until_something_else_claims_the_audio() {
        let epoch = Epoch::default();
        let first = epoch.arm();
        assert!(!first.is_cancelled());

        let second = epoch.arm();
        assert!(first.is_cancelled(), "the first send should be retired");
        assert!(!second.is_cancelled());

        epoch.bump();
        assert!(second.is_cancelled());
        // A stop is not undone by the send after it.
        let third = epoch.arm();
        assert!(second.is_cancelled() && !third.is_cancelled());
    }
}
