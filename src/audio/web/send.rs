//! One send's tones, scheduled on the context clock.

use std::sync::Arc;

use cw_core::band::Qsb;
use cw_core::{PlannedTransmission, ToneEvent};
use web_sys::{AudioContext, GainNode, OscillatorType};

use super::param::{Curve, ramp_to};
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
        // A send the browser will not finish building is taken out whole,
        // rather than left playing whatever tones it did get.
        if let Err(err) = schedule_into(ctx, &gain, planned, fading, start) {
            let _ = gain.disconnect();
            return Err(err);
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

/// The fading on `gain`, and every station's tones into it, from `start`.
fn schedule_into(
    ctx: &AudioContext,
    gain: &GainNode,
    planned: &PlannedTransmission,
    fading: Option<Qsb>,
    start: f64,
) -> Result<(), String> {
    // The fading, read off the context's clock as the native player reads it
    // off its own: a send drops into a fade already in progress rather than
    // starting one. The curve is the shared fade, sampled — nothing else the
    // graph can build stays inside its range.
    if let Some(fading) = fading {
        let length = planned.wanted.duration_sec.max(0.01);
        let curve = fading.curve(start, length);
        Curve::new(&curve)
            .play_on(&gain.gain(), start, length)
            .map_err(|e| format!("fading: {e:?}"))?;
    }
    // The wanted station and everyone calling over it go into the same gain:
    // interference is addition, and each station's events already carry its
    // own pitch and level.
    let every_station = std::iter::once(&planned.wanted).chain(planned.others.iter());
    let mut envelopes = Envelopes::default();
    for event in every_station.flat_map(|plan| plan.events.iter()) {
        schedule_tone(ctx, gain, event, &mut envelopes, start + event.start_sec)?;
    }
    Ok(())
}

/// The keying envelopes of one send's tones, each made into a [`Curve`] the
/// first time a tone is keyed with it.
///
/// Every dit a station sends has the same envelope, and so does every dah,
/// so the tones of a send share a few curves between them, told apart by
/// where they live. Kept for one send only: the plan is borrowed for as long
/// as they are, so none of them can go and leave its address to another
/// envelope — which is just what one kept from an earlier send could do.
#[derive(Default)]
struct Envelopes<'p>(Vec<(&'p Arc<[f32]>, Curve<'p>)>);

impl<'p> Envelopes<'p> {
    /// The curve for `envelope`, made the first time it is asked for.
    fn curve(&mut self, envelope: &'p Arc<[f32]>) -> &Curve<'p> {
        let known = self
            .0
            .iter()
            .position(|(seen, _)| Arc::ptr_eq(seen, envelope));
        let at = known.unwrap_or_else(|| {
            self.0.push((envelope, Curve::new(envelope)));
            self.0.len() - 1
        });
        &self.0[at].1
    }
}

/// One element: an oscillator through its own keying envelope, from `t0`.
fn schedule_tone<'p>(
    ctx: &AudioContext,
    into: &GainNode,
    event: &'p ToneEvent,
    envelopes: &mut Envelopes<'p>,
    t0: f64,
) -> Result<(), String> {
    let osc = ctx.create_oscillator().map_err(|e| format!("osc: {e:?}"))?;
    let element = ctx.create_gain().map_err(|e| format!("sym gain: {e:?}"))?;
    osc.set_type(OscillatorType::Sine);
    osc.frequency().set_value(event.frequency_hz as f32);
    osc.connect_with_audio_node(&element)
        .map_err(|e| format!("osc connect: {e:?}"))?;
    element
        .connect_with_audio_node(into)
        .map_err(|e| format!("gain connect: {e:?}"))?;
    let param = element.gain();
    let _ = param.set_value_at_time(0.0, t0);
    if event.envelope.len() >= 2
        && envelopes
            .curve(&event.envelope)
            .play_on(&param, t0, event.duration_sec)
            .is_err()
    {
        let rise = event.duration_sec.min(0.02);
        let _ = param.linear_ramp_to_value_at_time(event.target_gain as f32, t0 + rise);
        let _ = param.set_value_at_time(event.target_gain as f32, t0 + event.duration_sec - rise);
        let _ = param.linear_ramp_to_value_at_time(0.0, t0 + event.duration_sec);
    }
    osc.start_with_when(t0)
        .map_err(|e| format!("start: {e:?}"))?;
    osc.stop_with_when(t0 + event.duration_sec)
        .map_err(|e| format!("stop: {e:?}"))?;
    Ok(())
}
