//! The mixer, listened to the way the stream plays it.

use super::*;
use cw_core::band::{BandMixer, FilterDesign};
use cw_core::{FastrandRng, TrainingSettings, Transmission, plan_transmission, resolve_station};

/// Low enough to keep the long listens quick; the band sounds the same at
/// any rate.
const SR: u32 = 16_000;

/// The default band with its static off: the steady hiss, whose level is
/// the thing to hear a seam in.
fn hiss() -> TrainingSettings {
    let mut settings = TrainingSettings::default();
    settings.band.qrn_enabled = false;
    settings.band.qsb_enabled = false;
    settings
}

fn band(settings: &TrainingSettings, seed: u64) -> BandVoice {
    BandVoice::new(BandMixer::new(SR, settings, seed), SR)
}

fn send(settings: &TrainingSettings, text: &str) -> SendVoice {
    let voice = resolve_station(settings, &mut FastrandRng(5));
    let plan = plan_transmission(&Transmission::alone(text, voice), settings);
    SendVoice::new(&plan, &FilterDesign::from_settings(settings), None, SR, 0.0)
}

fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    mixer.render(&mut out, 0);
    out
}

fn power(samples: &[f32]) -> f64 {
    samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// The biggest change between neighbouring samples: where a waveform is cut
/// off, or a layer drops out, it steps.
fn biggest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// A band rebuilt — every slider move in the band preview does it — comes
/// in without a hole in the hiss. Switched in cold, a new band's filter was
/// empty and its AGC's look-ahead full of silence, and the old one faded out
/// under it: some ten decibels gone for a few milliseconds on every rebuild.
/// A straight-line crossfade of two unrelated noises sags 3 dB on its own.
///
/// Averaged over many rebuilds, in 5 ms windows, against the hiss before
/// each one.
#[test]
fn a_new_band_comes_in_without_a_hole_in_the_hiss() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    render(&mut mixer, SR as usize / 2);
    let window = SR as usize / 200;
    let windows = 10;
    let swaps = 300;
    let (mut steady, mut around) = (0.0, vec![0.0; windows]);
    for k in 0..swaps {
        steady += power(&render(&mut mixer, window * windows));
        let mut swept = Swept::new();
        mixer.set_band(Some(band(&settings, 100 + k)), &mut swept);
        let after = render(&mut mixer, window * windows);
        for (w, chunk) in after.chunks(window).enumerate() {
            around[w] += power(chunk);
        }
    }
    let levels: Vec<f64> = around.iter().map(|p| db(p / steady)).collect();
    assert!(
        levels.iter().all(|level| level.abs() < 1.0),
        "the hiss through a rebuild, in dB against before it: {levels:.2?}"
    );
}

/// Bands rebuilt faster than one can fade out each fade out in full: none is
/// dropped part of the way down, which is a step in the waveform.
#[test]
fn bands_rebuilt_in_quick_succession_all_fade_out() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    mixer.set_band(Some(band(&settings, 1)), &mut swept);
    let steady = biggest_step(&render(&mut mixer, SR as usize * 2));
    let mut worst = 0.0f32;
    for k in 0..40 {
        let mut through = Vec::new();
        for j in 0..3 {
            let mut swept = Swept::new();
            mixer.set_band(Some(band(&settings, 10 * k + j)), &mut swept);
            through.extend(render(&mut mixer, SR as usize / 500));
        }
        through.extend(render(&mut mixer, SR as usize / 20));
        worst = worst.max(biggest_step(&through));
    }
    assert!(
        worst < 1.5 * steady,
        "rebuilds stepped by {worst}, against {steady} in the hiss itself"
    );
}

/// A send started while the last one is still sounding lets the last one
/// ride down over its release rather than cutting it off — whatever point
/// of its cycle it was at.
#[test]
fn a_new_send_lets_the_last_one_ride_out_its_release() {
    let settings = hiss();
    for offset in 0..6 {
        let mut mixer = Mixer::new(SR);
        let mut swept = Swept::new();
        mixer.start(send(&settings, "T"), &mut swept);
        // Into the middle of the dah, at a different point of its cycle
        // each time.
        let into = render(&mut mixer, SR as usize / 10 + 3 * offset);
        assert!(into.iter().any(|s| s.abs() > 0.1), "the dah is sounding");
        mixer.start(send(&settings, "T"), &mut swept);
        let mut across = vec![into[into.len() - 1]];
        across.extend(render(&mut mixer, SR as usize / 50));
        // A 600 Hz tone at full level moves at most a quarter of its height
        // between samples at this rate; a cut is a step of all of it.
        let step = biggest_step(&across);
        assert!(step < 0.1, "offset {offset}: a step of {step}");
    }
}

/// A send stopped part way leaves nothing of itself in the receiver: the
/// band comes back up as if it had never been ducked. A send stopped mid-way
/// used to leave its level with the AGC for good, and the band sat 4 dB
/// down until the next send overwrote it.
#[test]
fn a_stopped_send_leaves_the_band_as_it_was() {
    let settings = hiss();
    let seconds = |s: f64| (s * f64::from(SR)) as usize;
    let (mut heard, mut untouched) = (Mixer::new(SR), Mixer::new(SR));
    let mut swept = Swept::new();
    heard.set_band(Some(band(&settings, 7)), &mut swept);
    untouched.set_band(Some(band(&settings, 7)), &mut swept);
    heard.start(send(&settings, "TTTTTTTTTT"), &mut swept);
    render(&mut heard, seconds(0.5));
    render(&mut untouched, seconds(0.5));
    heard.stop();
    render(&mut heard, seconds(2.0));
    render(&mut untouched, seconds(2.0));
    let (a, b) = (
        render(&mut heard, seconds(1.0)),
        render(&mut untouched, seconds(1.0)),
    );
    let level = db(power(&a) / power(&b));
    assert!(
        level.abs() < 0.2,
        "two seconds after the stop the band was {level:.2} dB off"
    );
}

/// A send is over once its last sample has been heard, `latency` frames
/// after it was played.
#[test]
fn a_send_finishes_when_it_has_been_heard() {
    let settings = hiss();
    let mut mixer = Mixer::new(SR);
    let mut swept = Swept::new();
    let voice = send(&settings, "E");
    let finished = voice.finished_flag();
    mixer.start(voice, &mut swept);
    let latency = SR as usize / 10;
    let mut played = 0;
    while !finished.load(Ordering::Acquire) && played < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        mixer.render(&mut out, latency);
        played += 64;
    }
    let expected = send(&settings, "E");
    let mut alone = Mixer::new(SR);
    let flag = expected.finished_flag();
    alone.start(expected, &mut swept);
    let mut unheard = 0;
    while !flag.load(Ordering::Acquire) && unheard < SR as usize * 2 {
        let mut out = vec![0.0f32; 64];
        alone.render(&mut out, 0);
        unheard += 64;
    }
    assert!(
        played >= unheard + latency && played <= unheard + latency + 64,
        "finished after {played} frames, {unheard} with no latency"
    );
}

/// The output stage catches what is too loud rather than squaring it off:
/// the same curve the band is calibrated through, applied once to the sum.
#[test]
fn the_output_stage_catches_rather_than_clips() {
    assert_eq!(finish(0.5), 0.5, "below the knee it does nothing");
    assert_eq!(finish(-0.5), -0.5);
    assert!(finish(1.4) > 0.9 && finish(1.4) < 1.0);
    assert!(finish(2.0) > finish(1.4), "limiting, not clipping flat");
    assert!(finish(-3.0) < -0.9 && finish(-3.0) >= -1.0);
    assert_eq!(finish(f32::NAN), 0.0);
    assert_eq!(finish(f32::INFINITY), 0.0);
}

/// The audio thread never waits: a callback that finds the UI holding the
/// mixer goes without it for one buffer, and the paddle's sidetone, which
/// needs no lock, plays regardless. A lock poisoned elsewhere is still a
/// good one.
#[test]
fn the_stream_never_waits_for_the_mixer() {
    let settings = hiss();
    let mixer = Arc::new(Mutex::new(Mixer::new(SR)));
    let mut swept = Swept::new();
    mixer
        .lock()
        .expect("mixer")
        .start(send(&settings, "TTT"), &mut swept);
    let control = Arc::new(SidetoneControl::new());
    let failed = Arc::new(AtomicBool::new(false));
    let mut output = Output::new(
        Arc::clone(&mixer),
        Arc::clone(&control),
        Arc::clone(&failed),
        SR,
    );

    control.set(crate::audio::shape::ToneRequest::new(true, 700.0, 0.2));
    let mut out = vec![0.0f32; 512];
    {
        let _held = mixer.lock().expect("mixer");
        output.fill(&mut out, 0);
    }
    let peak = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(
        peak > 0.15 && peak <= 0.2 + 1e-6,
        "the sidetone alone, peak {peak}"
    );

    let poisoner = Arc::clone(&mixer);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the lock");
    })
    .join();
    assert!(mixer.is_poisoned());
    control.off();
    let mut out = vec![0.0f32; SR as usize / 5];
    output.fill(&mut out, 0);
    assert!(
        out.iter().skip(SR as usize / 50).any(|s| s.abs() > 0.05),
        "the send still plays through a poisoned lock"
    );
    assert!(!failed.load(Ordering::Acquire));
}
