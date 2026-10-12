//! The two things every part of the graph does with its gains: move one
//! smoothly from wherever it is, and keep a fading node connected until its
//! fade has played.

use web_sys::{AudioParam, GainNode};

/// Move `param` from wherever it is now to `to` in a straight line ending at
/// `until`. What was scheduled is cancelled and the value held where it
/// stands first, so the ramp starts from what is playing rather than
/// jumping to wherever the cancelled automation began.
pub fn ramp_to(param: &AudioParam, to: f32, now: f64, until: f64) {
    let _ = param.cancel_scheduled_values(now);
    let _ = param.set_value_at_time(param.value(), now);
    let _ = param.linear_ramp_to_value_at_time(to, until);
}

/// Something in the graph that has to be taken out of it once it has gone
/// quiet.
pub trait Unplug {
    fn unplug(&self);
}

impl Unplug for GainNode {
    fn unplug(&self) {
        let _ = self.disconnect();
    }
}

/// Things fading out, each with the context time it is silent by.
///
/// They stay connected until then: taking a node out of the graph while its
/// fade is still playing is a click. Unplugged once the fade has run — a node
/// left connected goes on processing silence for the life of the page.
pub struct Retiring<T: Unplug>(Vec<(T, f64)>);

impl<T: Unplug> Retiring<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn retire(&mut self, item: T, silent_at: f64) {
        self.0.push((item, silent_at));
    }

    /// Unplug whatever has gone quiet by `now`.
    pub fn collect(&mut self, now: f64) {
        self.0.retain(|(item, silent_at)| {
            let quiet = *silent_at <= now;
            if quiet {
                item.unplug();
            }
            !quiet
        });
    }
}
