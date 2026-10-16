//! Budgeted latency / throughput checks. These are not soak correctness —
//! they fail the build when a hot path gets slow enough to stutter keying.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cw_core::{
    plan_morse_playback, FastrandRng, KeyerMode, KeyingLatency, Paddle, TrainingSettings,
};

use super::live::LiveToneGate;
use super::native::MorsePlayer;
use super::render::render_plan;
use super::MorseBackend;
use crate::ui::keyer_worker::KeyerWorker;

fn percentile_us(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn measure_us(iters: usize, warm: usize, mut body: impl FnMut()) -> Vec<u64> {
    for _ in 0..warm {
        body();
    }
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        body();
        samples.push(start.elapsed().as_micros() as u64);
    }
    samples.sort_unstable();
    samples
}

#[test]
fn live_tone_gate_set_stays_under_fifty_microseconds() {
    let gate = LiveToneGate::new();
    let samples = measure_us(10_000, 1_000, || {
        gate.set(true, 600.0, 0.2);
        gate.set_on(false);
    });
    let p99 = percentile_us(&samples, 0.99);
    assert!(
        p99 < 50,
        "LiveToneGate::set p99={p99}µs (budget 50µs); median={}µs",
        percentile_us(&samples, 0.5)
    );
}

#[test]
fn keyer_worker_press_reaches_the_gate_within_two_milliseconds() {
    let gate = LiveToneGate::new();
    let worker = KeyerWorker::start(Arc::clone(&gate));
    worker.configure(KeyerMode::Straight, 60, 600.0, 0.2);

    // Warm the thread and channel.
    for _ in 0..20 {
        worker.press(Paddle::Dit);
        let deadline = Instant::now() + Duration::from_millis(20);
        while !gate.on() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        worker.release(Paddle::Dit);
        let deadline = Instant::now() + Duration::from_millis(20);
        while gate.on() && Instant::now() < deadline {
            std::thread::yield_now();
        }
    }

    let mut samples = Vec::with_capacity(200);
    for _ in 0..200 {
        while gate.on() {
            worker.release(Paddle::Dit);
            std::thread::yield_now();
        }
        let start = Instant::now();
        worker.press(Paddle::Dit);
        let deadline = start + Duration::from_millis(20);
        while !gate.on() {
            if Instant::now() >= deadline {
                break;
            }
            std::thread::yield_now();
        }
        samples.push(start.elapsed().as_micros() as u64);
        worker.release(Paddle::Dit);
    }
    samples.sort_unstable();
    let p99 = percentile_us(&samples, 0.99);
    assert!(
        p99 < 2_000,
        "keyer press→gate p99={p99}µs (budget 2ms); median={}µs",
        percentile_us(&samples, 0.5)
    );
}

#[test]
fn mixer_opens_within_the_low_latency_ceiling() {
    let Ok(mut player) = MorsePlayer::new() else {
        eprintln!("no audio device; skipping mixer open budget");
        return;
    };
    // Fresh player is tuned for playout (Balanced), not paddle Keying.
    if let Some(ms) = player.output_latency_ms() {
        assert!(
            ms <= 24.0,
            "playout buffer is {ms:.2} ms — above the Balanced ceiling"
        );
    }
    player.apply_keying_latency(KeyingLatency::Low);
    let Some(ms) = player.output_latency_ms() else {
        eprintln!("mixer opened without Fixed period; skipping ms ceiling");
        return;
    };
    // Low candidates top out at 1024 frames. At 44.1 kHz that is ~23.2 ms.
    assert!(
        ms <= 24.0,
        "Low keying buffer is {ms:.2} ms — above the 1024-frame ceiling"
    );
    player.prepare_playout();
    if let Some(playout) = player.output_latency_ms() {
        assert!(
            playout <= 24.0,
            "returning to playout left a {playout:.2} ms buffer"
        );
    }
}

#[test]
fn rendering_a_short_group_stays_under_five_milliseconds() {
    let mut settings = TrainingSettings::default();
    settings.playback.char_wpm_min = 40.0;
    settings.playback.char_wpm_max = 40.0;
    settings.playback.effective_wpm_min = 40.0;
    settings.playback.effective_wpm_max = 40.0;
    let plan = plan_morse_playback("CQ", &settings, &mut FastrandRng(1));
    // Warm.
    let _ = render_plan(&plan, 48_000);
    let start = Instant::now();
    let samples = render_plan(&plan, 48_000);
    let elapsed = start.elapsed();
    assert!(!samples.is_empty());
    assert!(
        elapsed < Duration::from_millis(5),
        "render_plan took {elapsed:?} (budget 5ms) for {} samples",
        samples.len()
    );
}

/// Manual ear check: press a paddle and listen. Not a CI gate.
#[test]
#[ignore = "manual loopback: run with --ignored when verifying speakers"]
fn manual_loopback_sidetone_is_audible() {
    let Ok(mut player) = MorsePlayer::new() else {
        panic!("no audio device");
    };
    player.apply_keying_latency(KeyingLatency::Low);
    player.set_live_tone(true, 600.0, 0.25);
    std::thread::sleep(Duration::from_millis(400));
    player.set_live_tone(false, 600.0, 0.25);
    std::thread::sleep(Duration::from_millis(100));
}
