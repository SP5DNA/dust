//! One send's tones, scheduled on the context clock.

use cw_core::PlannedTransmission;
use cw_core::band::Qsb;
use web_sys::{AudioContext, GainNode, OscillatorType};

use super::param::ramp_to;
use crate::audio::shape::SEND_RELEASE_SEC;

/// A scheduled send: every station's tones through one gain, which the
/// fading rides and a stop ramps down.
pub struct ScheduledSend {
    gain: GainNode,
}

impl ScheduledSend {
    /// Schedule every station's tones from `start` on the context clock,
    /// into `into`.
    pub fn schedule(
        ctx: &AudioContext,
        into: &GainNode,
        planned: &PlannedTransmission,
        fading: Option<Qsb>,
        start: f64,
    ) -> Result<Self, String> {
        let gain = ctx.create_gain().map_err(|e| format!("gain: {e:?}"))?;
        gain.connect_with_audio_node(into)
            .map_err(|e| format!("connect: {e:?}"))?;
        // The fading, on the send's own gain, read off the context's clock
        // as the native player reads it off its own: a send drops into a
        // fade already in progress rather than starting one. The curve is
        // the shared fade, sampled — nothing else the graph can build stays
        // inside its range.
        if let Some(fading) = fading {
            let length = planned.wanted.duration_sec.max(0.01);
            let mut curve = fading.curve(start, length);
            gain.gain()
                .set_value_curve_at_time(&mut curve, start, length)
                .map_err(|e| format!("fading: {e:?}"))?;
        }
        // The wanted station and everyone calling over it go into the same
        // gain: interference is addition, and each station's events already
        // carry its own pitch and level.
        let every_station = std::iter::once(&planned.wanted).chain(planned.others.iter());
        for event in every_station.flat_map(|plan| plan.events.iter()) {
            let osc = ctx.create_oscillator().map_err(|e| format!("osc: {e:?}"))?;
            let element = ctx.create_gain().map_err(|e| format!("sym gain: {e:?}"))?;
            osc.set_type(OscillatorType::Sine);
            osc.frequency().set_value(event.frequency_hz as f32);
            osc.connect_with_audio_node(&element)
                .map_err(|e| format!("osc connect: {e:?}"))?;
            element
                .connect_with_audio_node(&gain)
                .map_err(|e| format!("gain connect: {e:?}"))?;

            let t0 = start + event.start_sec;
            let param = element.gain();
            let _ = param.set_value_at_time(0.0, t0);
            if event.envelope.len() >= 2 {
                let mut curve = event.envelope.to_vec();
                if param
                    .set_value_curve_at_time(&mut curve, t0, event.duration_sec)
                    .is_err()
                {
                    let rise = event.duration_sec.min(0.02);
                    let _ = param.linear_ramp_to_value_at_time(event.target_gain as f32, t0 + rise);
                    let _ = param.set_value_at_time(
                        event.target_gain as f32,
                        t0 + event.duration_sec - rise,
                    );
                    let _ = param.linear_ramp_to_value_at_time(0.0, t0 + event.duration_sec);
                }
            }
            osc.start_with_when(t0)
                .map_err(|e| format!("start: {e:?}"))?;
            osc.stop_with_when(t0 + event.duration_sec)
                .map_err(|e| format!("stop: {e:?}"))?;
        }
        Ok(Self { gain })
    }

    /// Ramp the send down instead of cutting it, and hand back its gain with
    /// the context time it is silent by.
    ///
    /// Disconnecting on the spot is what made the old stop click: the fade
    /// was scheduled and then the node that would have played it was taken
    /// out of the graph in the same breath. The gain stays connected until
    /// the ramp has run.
    pub fn release(self, ctx: &AudioContext) -> (GainNode, f64) {
        let now = ctx.current_time();
        let done = now + SEND_RELEASE_SEC;
        ramp_to(&self.gain.gain(), 0.0, now, done);
        (self.gain, done)
    }
}
