//! The band, generated here and streamed into the graph.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use cw_core::band::{AudioFloor, BandSource, FilterDesign};
use cw_core::{STATIONS_MIN, TrainingSettings};
use gloo_timers::callback::Interval;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBufferSourceNode, AudioContext, AudioContextState, AudioScheduledSourceNode, GainNode,
};

use super::param::Curve;
use super::{page_is_hidden, reported_latency};
use crate::audio::band_queue::{Reservation, callers_at, first_to_remake};
use crate::audio::shape::{BAND_FADE_SEC, fade_curve};

/// How much of the band each streamed buffer holds.
const STREAM_CHUNK_SEC: f64 = 0.1;
/// How far ahead of the audio clock the band is kept scheduled. Enough to
/// ride over a busy main thread; short enough that a buffer is not wasted
/// every time a slider moves.
const STREAM_LEAD_SEC: f64 = 0.5;
/// A hidden tab's timers fire about once a second at best, so the band is
/// scheduled further ahead while the page is out of sight.
const STREAM_LEAD_HIDDEN_SEC: f64 = 2.5;
/// How often the stream is topped up.
const STREAM_TICK_MS: u32 = 100;
/// Points in a band's fade curve: plenty for a quarter sine over a few tens
/// of milliseconds.
const FADE_POINTS: usize = 32;
/// How far ahead of the clock a queued band buffer has to start to be taken
/// back and made again: past whatever the browser is already rendering.
const REMAKE_MARGIN_SEC: f64 = 0.03;

/// The band, streamed into the graph.
///
/// Web Audio has nowhere to run a generator sample by sample short of an
/// AudioWorklet, and a looped buffer is a pattern: the same crash coming
/// round every few seconds is the one thing real static never does. So the
/// band is generated here, by the same [`BandSource`] the native player runs,
/// a tenth of a second at a time, and each buffer is scheduled on the audio
/// clock to start exactly where the last one ends. Nothing repeats, and the
/// browser plays the very model the desktop does.
///
/// The receiver's audio-stage floor ([`AudioFloor`]) rides in the same
/// buffers as a second channel, so the two stay in step and fade together,
/// and is split off on arrival: the band goes into the receiver, the floor
/// around it and the compressor to the limiter, which is where the native
/// player adds it.
pub struct BandStream {
    state: Rc<RefCell<StreamState>>,
    _ticker: Interval,
}

struct StreamState {
    ctx: AudioContext,
    /// The band's fade, on its way into the receiver.
    output: GainNode,
    /// The floor's fade, on its way to the limiter.
    floor_output: GainNode,
    /// Takes each buffer apart: channel 0 to `output`, 1 to `floor_output`.
    splitter: web_sys::ChannelSplitterNode,
    source: BandSource,
    floor: AudioFloor,
    chunk: Vec<f32>,
    floor_chunk: Vec<f32>,
    /// Context time the next buffer starts at.
    next_start: f64,
    /// Buffers scheduled and not yet over.
    queued: VecDeque<Queued>,
    /// A pile-up calling across the send: the band's own traffic makes room
    /// for its callers until it stops.
    pileup: Option<Reservation>,
    /// Whether the band has stations of its own to make room with. Without
    /// them a pile-up changes nothing in it, and nothing is ever remade.
    makes_room: bool,
}

/// One buffer of the band, scheduled.
struct Queued {
    node: AudioBufferSourceNode,
    starts: f64,
    ends: f64,
    /// How many callers it made room for.
    callers: usize,
    /// The generators as they stood before it was made, so it can be made
    /// again for a pile-up that started or stopped after it was queued.
    before: Option<(BandSource, AudioFloor)>,
}

impl BandStream {
    /// Start the band into `into`, the receiver's input, and its audio-stage
    /// floor into `floor_into`, past the compressor.
    ///
    /// `pileup` is the pile-up already calling when the band is started
    /// under a send in flight. It is made room for from the very first
    /// buffer: a band made for nobody would have to be taken back and made
    /// again for the callers in the same breath, and those of its buffers
    /// too close to the clock to take back would play on as if nobody were
    /// calling.
    pub fn start(
        ctx: &AudioContext,
        into: &GainNode,
        floor_into: &GainNode,
        settings: &TrainingSettings,
        pileup: Option<Reservation>,
    ) -> Result<Self, String> {
        let now = ctx.current_time();
        // The rising half of an equal-power crossfade: the outgoing band
        // falls along the other, and the hiss holds its level through the
        // change where straight lines would sag 3 dB. The band and its floor
        // rise along the one curve.
        let points = fade_curve(FADE_POINTS, true, 1.0);
        let rising = Curve::new(&points);
        let fading_in = |into: &GainNode| -> Result<GainNode, String> {
            let output = ctx.create_gain().map_err(|e| format!("band gain: {e:?}"))?;
            rising
                .play_on(&output.gain(), now, BAND_FADE_SEC)
                .map_err(|e| format!("band fade: {e:?}"))?;
            output
                .connect_with_audio_node(into)
                .map_err(|e| format!("band connect: {e:?}"))?;
            Ok(output)
        };
        let output = fading_in(into)?;
        let floor_output = fading_in(floor_into)?;
        let splitter = ctx
            .create_channel_splitter_with_number_of_outputs(2)
            .map_err(|e| format!("band splitter: {e:?}"))?;
        splitter
            .connect_with_audio_node_and_output(&output, 0)
            .map_err(|e| format!("band split: {e:?}"))?;
        splitter
            .connect_with_audio_node_and_output(&floor_output, 1)
            .map_err(|e| format!("floor split: {e:?}"))?;
        let sample_rate = ctx.sample_rate();
        let chunk = ((f64::from(sample_rate) * STREAM_CHUNK_SEC).round() as usize).max(128);
        // A fresh band every time, drawn the way the native player draws
        // its own: with stations on it, a fixed seed would replay the same
        // callsigns at the same moments on every visit.
        let seed = crate::time::seed_rng();
        let state = Rc::new(RefCell::new(StreamState {
            ctx: ctx.clone(),
            output,
            floor_output,
            splitter,
            source: BandSource::new(
                sample_rate as u32,
                &settings.band,
                &FilterDesign::from_settings(settings),
                seed,
            ),
            floor: AudioFloor::for_band(sample_rate as u32, &settings.band, seed),
            chunk: vec![0.0; chunk],
            floor_chunk: vec![0.0; chunk],
            next_start: now,
            queued: VecDeque::new(),
            pileup,
            makes_room: settings.band.stations_max > STATIONS_MIN,
        }));
        state.borrow_mut().top_up()?;
        let ticking = Rc::clone(&state);
        let ticker = Interval::new(STREAM_TICK_MS, move || {
            // A tick that finds the state busy has nothing to add: whatever
            // holds it is topping up already.
            if let Ok(mut stream) = ticking.try_borrow_mut() {
                let _ = stream.top_up();
            }
        });
        Ok(Self {
            state,
            _ticker: ticker,
        })
    }

    /// The pile-up the band is making room for, if any.
    pub fn pileup(&self) -> Option<Reservation> {
        self.state
            .try_borrow()
            .ok()
            .and_then(|stream| stream.pileup)
    }

    /// Make room for a pile-up from now on, or for none, as a stopped send
    /// does.
    pub fn reserve(&self, pileup: Option<Reservation>) {
        if let Ok(mut stream) = self.state.try_borrow_mut() {
            let _ = stream.reserve(pileup);
        }
    }

    /// Fade out and stop. Dropping `self` here stops the ticker; the output
    /// gains, the band's and the floor's, come back with the time their fade
    /// ends.
    pub fn fade_out(self) -> ([GainNode; 2], f64) {
        let stream = self.state.borrow();
        let now = stream.ctx.current_time();
        let done = now + BAND_FADE_SEC;
        for output in [&stream.output, &stream.floor_output] {
            let gain = output.gain();
            let from = f64::from(gain.value());
            let _ = gain.cancel_scheduled_values(now);
            let falling = fade_curve(FADE_POINTS, false, from);
            let _ = Curve::new(&falling).play_on(&gain, now, BAND_FADE_SEC);
        }
        for queued in &stream.queued {
            let _ = queued
                .node
                .unchecked_ref::<AudioScheduledSourceNode>()
                .stop_with_when(done);
        }
        ([stream.output.clone(), stream.floor_output.clone()], done)
    }
}

impl StreamState {
    /// Schedule buffers until the band runs far enough ahead of the clock,
    /// and let go of the ones that have finished.
    fn top_up(&mut self) -> Result<(), String> {
        if self.ctx.state() == AudioContextState::Closed {
            return Ok(());
        }
        let now = self.ctx.current_time();
        // Fallen behind — a long task, a throttled tab. Pick up just ahead of
        // the clock rather than scheduling into the past, which would play
        // everything late at once.
        if self.next_start < now + 0.005 {
            self.next_start = now + 0.01;
        }
        let lead = if page_is_hidden() {
            STREAM_LEAD_HIDDEN_SEC
        } else {
            STREAM_LEAD_SEC
        };
        let sample_rate = self.ctx.sample_rate();
        while self.next_start < now + lead {
            // Each buffer is generated for the moment it will play, so the
            // pile-up's callers are made room for exactly while they call.
            let callers = callers_at(self.pileup, self.next_start);
            let before = self
                .makes_room
                .then(|| (self.source.clone(), self.floor.clone()));
            self.source.reserve_stations(callers);
            self.source.fill(&mut self.chunk);
            self.floor.fill(&mut self.floor_chunk);
            let buffer = self
                .ctx
                .create_buffer(2, self.chunk.len() as u32, sample_rate)
                .map_err(|e| format!("band buffer: {e:?}"))?;
            buffer
                .copy_to_channel(&self.chunk, 0)
                .map_err(|e| format!("band copy: {e:?}"))?;
            buffer
                .copy_to_channel(&self.floor_chunk, 1)
                .map_err(|e| format!("floor copy: {e:?}"))?;
            let node = self
                .ctx
                .create_buffer_source()
                .map_err(|e| format!("band source: {e:?}"))?;
            node.set_buffer(Some(&buffer));
            node.connect_with_audio_node(&self.splitter)
                .map_err(|e| format!("band source connect: {e:?}"))?;
            node.start_with_when(self.next_start)
                .map_err(|e| format!("band start: {e:?}"))?;
            let ends = self.next_start + self.chunk.len() as f64 / f64::from(sample_rate);
            self.queued.push_back(Queued {
                node,
                starts: self.next_start,
                ends,
                callers,
                before,
            });
            self.next_start = ends;
        }
        while let Some(done) = self.queued.front().filter(|queued| queued.ends < now) {
            let _ = done.node.disconnect();
            self.queued.pop_front();
        }
        Ok(())
    }

    /// Make room for `pileup` from now on. The buffers already queued were
    /// made for whoever was calling when they were: half a second of them,
    /// more with the page hidden. Those made for the wrong number of callers
    /// that can still be taken back are, and the band is made again from the
    /// first of them, from where its generators stood — so the pile-up is
    /// made room for within a few tens of milliseconds of the send, and let
    /// go of as soon after a stop.
    fn reserve(&mut self, pileup: Option<Reservation>) -> Result<(), String> {
        self.pileup = pileup;
        if !self.makes_room {
            return Ok(());
        }
        let not_before = self.ctx.current_time()
            + reported_latency(&self.ctx, "baseLatency")
            + REMAKE_MARGIN_SEC;
        let Some(first) = first_to_remake(
            self.queued
                .iter()
                .map(|queued| (queued.starts, queued.callers)),
            pileup,
            not_before,
        ) else {
            return Ok(());
        };
        let Some((source, floor)) = self.queued[first].before.clone() else {
            return Ok(());
        };
        self.next_start = self.queued[first].starts;
        for taken in self.queued.drain(first..) {
            let _ = taken
                .node
                .unchecked_ref::<AudioScheduledSourceNode>()
                .stop();
            let _ = taken.node.disconnect();
        }
        self.source = source;
        self.floor = floor;
        self.top_up()
    }
}
