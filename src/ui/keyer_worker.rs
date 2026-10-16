//! Desktop paddle keyer on its own OS thread.
//!
//! The UI event loop shares a thread with WebKit. Element timing and sidetone
//! gating run here instead: `thread::sleep` for dit clocks, atomics for the
//! tone, and a channel back for decoded letters. The UI only posts presses
//! and drains letters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cw_core::{KeyerMode, Paddle, PaddleKeyer, LETTER_GAP_DITS};

use crate::audio::LiveToneGate;

/// Letter-gap / squeeze poll. Element lengths use `Instant` deadlines; this is
/// only how often we wake when waiting for the next decision.
const POLL_MS: u64 = 1;

#[derive(Debug)]
enum Cmd {
    Press(Paddle),
    Release(Paddle),
    Configure {
        mode: KeyerMode,
        dit_ms: u64,
        hz: f64,
        gain: f64,
    },
    Reset,
    Shutdown,
}

/// Handle the UI keeps. Cheap to clone the sender side.
pub struct KeyerWorker {
    tx: Sender<Cmd>,
    letters: Receiver<char>,
    #[allow(dead_code)]
    join: Option<JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}

impl KeyerWorker {
    pub fn start(tone: Arc<LiveToneGate>) -> Self {
        let (tx, rx) = mpsc::channel();
        let (letter_tx, letters) = mpsc::channel();
        let alive = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&alive);
        let join = thread::Builder::new()
            .name("dust-keyer".into())
            .spawn(move || run(rx, letter_tx, tone, flag))
            .ok();
        Self {
            tx,
            letters,
            join,
            alive,
        }
    }

    pub fn configure(&self, mode: KeyerMode, dit_ms: u64, hz: f64, gain: f64) {
        let _ = self.tx.send(Cmd::Configure {
            mode,
            dit_ms: dit_ms.max(1),
            hz,
            gain,
        });
    }

    pub fn press(&self, paddle: Paddle) {
        let _ = self.tx.send(Cmd::Press(paddle));
    }

    pub fn release(&self, paddle: Paddle) {
        let _ = self.tx.send(Cmd::Release(paddle));
    }

    pub fn reset(&self) {
        let _ = self.tx.send(Cmd::Reset);
    }

    pub fn drain_letters(&self) -> Vec<char> {
        let mut out = Vec::new();
        while let Ok(ch) = self.letters.try_recv() {
            out.push(ch);
        }
        out
    }
}

impl Drop for KeyerWorker {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(
    rx: Receiver<Cmd>,
    letter_tx: Sender<char>,
    tone: Arc<LiveToneGate>,
    alive: Arc<AtomicBool>,
) {
    let mut keyer = PaddleKeyer::default();
    let mut dit_ms = 60u64;
    let mut hz = 600.0;
    let mut gain = 0.2;
    let mut until: Option<Instant> = None;
    let mut phase = Phase::Idle;

    while alive.load(Ordering::Relaxed) {
        let wait = until
            .map(|t| t.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(POLL_MS));
        match rx.recv_timeout(wait.max(Duration::from_millis(1))) {
            Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                tone.set_on(false);
                return;
            }
            Ok(Cmd::Configure {
                mode,
                dit_ms: next_dit,
                hz: next_hz,
                gain: next_gain,
            }) => {
                if keyer.mode() != mode {
                    keyer.set_mode(mode);
                    phase = Phase::Idle;
                    until = None;
                    tone.set_on(false);
                }
                dit_ms = next_dit.max(1);
                hz = next_hz;
                gain = next_gain;
            }
            Ok(Cmd::Reset) => {
                keyer.reset();
                phase = Phase::Idle;
                until = None;
                tone.set_on(false);
            }
            Ok(Cmd::Press(paddle)) => {
                if keyer.mode().is_straight() {
                    let already = keyer.any_held();
                    keyer.press_at(paddle, now_ms());
                    if !already {
                        tone.set(true, hz, gain);
                    }
                    continue;
                }
                keyer.press(paddle);
                if matches!(phase, Phase::Idle) {
                    if let Some(first) = keyer.next_element() {
                        keyer.decoder_mut().push(first);
                        tone.set(true, hz, gain);
                        phase = Phase::Element;
                        until = Some(Instant::now() + Duration::from_millis(dit_ms * first.dits()));
                    }
                }
            }
            Ok(Cmd::Release(paddle)) => {
                if keyer.mode().is_straight() {
                    keyer.release(paddle);
                    if !keyer.any_held() {
                        tone.set_on(false);
                        if let Some(element) = keyer.take_straight(now_ms(), dit_ms) {
                            keyer.decoder_mut().push(element);
                            phase = Phase::LetterGap {
                                left: dit_ms.saturating_mul(LETTER_GAP_DITS).max(1),
                            };
                            until = Some(Instant::now() + Duration::from_millis(POLL_MS));
                        }
                    }
                    continue;
                }
                keyer.release(paddle);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        let Some(deadline) = until else {
            continue;
        };
        if Instant::now() < deadline {
            continue;
        }

        match phase {
            Phase::Idle => until = None,
            Phase::Element => {
                tone.set_on(false);
                phase = Phase::IntraGap;
                until = Some(Instant::now() + Duration::from_millis(dit_ms));
            }
            Phase::IntraGap => {
                if let Some(next) = keyer.next_element() {
                    keyer.decoder_mut().push(next);
                    tone.set(true, hz, gain);
                    phase = Phase::Element;
                    until = Some(Instant::now() + Duration::from_millis(dit_ms * next.dits()));
                } else if keyer.any_held() {
                    // Squeeze still down but no element yet — poll shortly.
                    until = Some(Instant::now() + Duration::from_millis(POLL_MS));
                } else {
                    phase = Phase::LetterGap {
                        left: dit_ms.saturating_mul(LETTER_GAP_DITS).max(1),
                    };
                    until = Some(Instant::now() + Duration::from_millis(POLL_MS));
                }
            }
            Phase::LetterGap { left } => {
                if keyer.any_held() {
                    if let Some(next) = keyer.next_element() {
                        keyer.decoder_mut().push(next);
                        tone.set(true, hz, gain);
                        phase = Phase::Element;
                        until = Some(Instant::now() + Duration::from_millis(dit_ms * next.dits()));
                    } else {
                        until = Some(Instant::now() + Duration::from_millis(POLL_MS));
                    }
                    continue;
                }
                let step = left.clamp(1, POLL_MS);
                let left = left.saturating_sub(step);
                if left == 0 {
                    phase = Phase::Idle;
                    until = None;
                    if let Some(ch) = keyer.decoder_mut().take_letter() {
                        let _ = letter_tx.send(ch);
                    }
                } else {
                    phase = Phase::LetterGap { left };
                    until = Some(Instant::now() + Duration::from_millis(step));
                }
            }
        }
    }
    tone.set_on(false);
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Element,
    IntraGap,
    LetterGap { left: u64 },
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
