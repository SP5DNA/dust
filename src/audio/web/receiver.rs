//! The receiver's filter in nodes, retuned by crossfading to a new bank.

use cw_core::TrainingSettings;
use cw_core::band::{FilterDesign, MAX_RECEIVER_SECTIONS, Section};
use web_sys::{AudioContext, GainNode, IirFilterNode};

use super::chain::Chain;
use super::param::{Retiring, Unplug, ramp_to};
use crate::audio::shape::BAND_FADE_SEC;

/// The receiver's filter, as it stands: the bank playing, and the banks a
/// retune replaced, fading out.
pub struct Receiver {
    bank: Option<ReceiverBank>,
    retired: Retiring<ReceiverBank>,
}

/// One `IIRFilterNode` per resonator of the design, side by side between the
/// mix and a gain that sums them — the native player's
/// [`cw_core::band::ReceiverFilter`], node for resonator, from the same
/// coefficients.
///
/// `IIRFilterNode`s rather than `BiquadFilterNode`s, because only they take
/// the numerator the design asks for: a band-pass biquad's is fixed.
///
/// Its coefficients are fixed once it is made, so a retune builds a new bank
/// and crossfades to it rather than moving the old one.
struct ReceiverBank {
    /// What it was built from.
    design: FilterDesign,
    /// The mix it is fed from.
    input: GainNode,
    sections: Vec<IirFilterNode>,
    /// Sums the sections. Ramped in and out across a retune.
    output: GainNode,
}

impl Receiver {
    pub fn new() -> Self {
        Self {
            bank: None,
            retired: Retiring::new(),
        }
    }

    /// Tune the receiver to the current pitch, width and shape.
    ///
    /// Runs on every band change, and costs a comparison when nothing has
    /// moved. When something has, a new bank is built and the old one fades
    /// out over [`BAND_FADE_SEC`] while the new one fades in, then is taken
    /// out of the graph. In straight lines: both banks are filtering the same
    /// sound, so their outputs add as amplitudes, not powers.
    pub fn tune(
        &mut self,
        ctx: &AudioContext,
        chain: &Chain,
        settings: &TrainingSettings,
    ) -> Result<(), String> {
        let now = ctx.current_time();
        self.retired.collect(now);
        let design = FilterDesign::from_settings(settings);
        if self.bank.as_ref().is_some_and(|bank| bank.design == design) {
            return Ok(());
        }
        let bank = ReceiverBank::build(ctx, &chain.mix, &chain.receiver_out, &design)?;
        let gain = bank.output.gain();
        match self.bank.replace(bank) {
            Some(old) => {
                let done = now + BAND_FADE_SEC;
                let _ = gain.set_value_at_time(0.0, now);
                let _ = gain.linear_ramp_to_value_at_time(1.0, done);
                ramp_to(&old.output.gain(), 0.0, now, done);
                self.retired.retire(old, done);
            }
            // Nothing playing through it yet: straight in.
            None => {
                let _ = gain.set_value_at_time(1.0, now);
            }
        }
        Ok(())
    }
}

impl ReceiverBank {
    /// Build a bank for `design` at the context's rate, fed from `input` and
    /// summed into `into`, its output gain starting silent.
    fn build(
        ctx: &AudioContext,
        input: &GainNode,
        into: &GainNode,
        design: &FilterDesign,
    ) -> Result<Self, String> {
        let output = ctx
            .create_gain()
            .map_err(|e| format!("receiver output: {e:?}"))?;
        output
            .gain()
            .set_value_at_time(0.0, ctx.current_time())
            .map_err(|e| format!("receiver output set: {e:?}"))?;
        let mut bank = Self {
            design: design.clone(),
            input: input.clone(),
            sections: Vec::with_capacity(MAX_RECEIVER_SECTIONS),
            output,
        };
        let sample_rate = f64::from(ctx.sample_rate());
        // Only the design's own sections — eight, or nine with the peak
        // filter — each one node.
        for section in design.sections(sample_rate) {
            if let Err(e) = bank.add_section(ctx, section) {
                bank.unplug();
                return Err(e);
            }
        }
        if let Err(e) = bank.output.connect_with_audio_node(into) {
            bank.unplug();
            return Err(format!("receiver output connect: {e:?}"));
        }
        Ok(bank)
    }

    fn add_section(&mut self, ctx: &AudioContext, section: Section) -> Result<(), String> {
        let [b0, b1] = section.feedforward;
        let [a0, a1, a2] = section.feedback;
        let feedforward = js_sys::Array::of2(&b0.into(), &b1.into());
        let feedback = js_sys::Array::of3(&a0.into(), &a1.into(), &a2.into());
        let node = ctx
            .create_iir_filter(&feedforward, &feedback)
            .map_err(|e| format!("receiver section: {e:?}"))?;
        // Kept before connecting, so a failure part-way still unplugs it.
        self.sections.push(node.clone());
        self.input
            .connect_with_audio_node(&node)
            .map_err(|e| format!("receiver section in: {e:?}"))?;
        node.connect_with_audio_node(&self.output)
            .map_err(|e| format!("receiver section out: {e:?}"))?;
        Ok(())
    }
}

impl Unplug for ReceiverBank {
    /// Off the mix it was fed from, and away from the sum. A node left
    /// connected to the mix would go on filtering silence into nothing for
    /// the life of the page.
    fn unplug(&self) {
        for node in &self.sections {
            let _ = self.input.disconnect_with_audio_node(node);
            let _ = node.disconnect();
        }
        let _ = self.output.disconnect();
    }
}
