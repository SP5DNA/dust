//! The paddle's sidetone: one oscillator, gated by its gain.

use web_sys::{AudioContext, GainNode, OscillatorNode, OscillatorType};

use super::param::ramp_to;
use crate::audio::shape::ToneRequest;

/// How quickly the gate opens and closes: quick enough to follow a fast
/// fist, slow enough not to click.
const GATE_SEC: f64 = 0.001;

/// The sidetone's oscillator and gate, made the first time a key asks for
/// them and kept running, so a squeeze is a gain change, not a node start.
#[derive(Default)]
pub struct Sidetone {
    running: Option<(OscillatorNode, GainNode)>,
}

impl Sidetone {
    /// Play `tone` into `into`, opening the gate or closing it.
    pub fn set(
        &mut self,
        ctx: &AudioContext,
        into: &GainNode,
        tone: ToneRequest,
    ) -> Result<(), String> {
        if self.running.is_none() {
            self.running = Some(start(ctx, into)?);
        }
        let Some((osc, gate)) = &self.running else {
            return Ok(());
        };
        let now = ctx.current_time();
        let _ = osc.frequency().set_value_at_time(tone.hz, now);
        let level = if tone.on { tone.gain } else { 0.0 };
        ramp_to(&gate.gain(), level, now, now + GATE_SEC);
        Ok(())
    }

    /// Close the gate at once.
    pub fn silence(&self, ctx: &AudioContext) {
        if let Some((_, gate)) = &self.running {
            let now = ctx.current_time();
            let _ = gate.gain().cancel_scheduled_values(now);
            let _ = gate.gain().set_value_at_time(0.0, now);
        }
    }
}

impl Drop for Sidetone {
    fn drop(&mut self) {
        if let Some((osc, _)) = self.running.take() {
            let _ = osc.stop();
        }
    }
}

fn start(ctx: &AudioContext, into: &GainNode) -> Result<(OscillatorNode, GainNode), String> {
    let osc = ctx
        .create_oscillator()
        .map_err(|e| format!("live osc: {e:?}"))?;
    let gate = ctx.create_gain().map_err(|e| format!("live gain: {e:?}"))?;
    osc.set_type(OscillatorType::Sine);
    let now = ctx.current_time();
    osc.frequency()
        .set_value_at_time(500.0, now)
        .map_err(|e| format!("live freq: {e:?}"))?;
    gate.gain()
        .set_value_at_time(0.0, now)
        .map_err(|e| format!("live mute: {e:?}"))?;
    osc.connect_with_audio_node(&gate)
        .map_err(|e| format!("live osc connect: {e:?}"))?;
    gate.connect_with_audio_node(into)
        .map_err(|e| format!("live out: {e:?}"))?;
    osc.start().map_err(|e| format!("live start: {e:?}"))?;
    Ok((osc, gate))
}
