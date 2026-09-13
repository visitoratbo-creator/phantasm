//! THE SONIC DREAD ENGINE — a hand-written DSP graph hosted by rodio.
//!
//! Signal flow (all per-sample, all on rodio's audio thread):
//!
//!   [ ambient bed ]  --\
//!   [ rumble x N  ]    +--> sum --> master fade --> tanh drive --> clamp 0.95 --> out
//!   [ rush voice  ]    /     (positional rumbles are stereo-panned;
//!   [ screech     ] --/      the rush voice is strictly MONO into an
//!                             AGC-normalised overdrive channel)
//!
//! Control flow: the game thread pushes `AudioCommand`s into a shared
//! VecDeque mailbox; the audio callback drains it with `try_lock` so the
//! realtime thread never blocks on the game thread. Steady state is
//! allocation-free — the only heap events on the audio thread are the
//! one-shot voice spawns at their triggers.
//!
//! The output is brickwalled at 0.95 FS: maximum perceived terror,
//! not speaker homicide.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::canvas::TinyRng;

pub const SAMPLE_RATE: f32 = 48_000.0;
const SR64: f64 = 48_000.0;
const TAU: f64 = std::f64::consts::TAU;
const TAU_F: f32 = std::f32::consts::TAU;
const FRAME_DT: f32 = 1.0 / SAMPLE_RATE;
const CEILING: f32 = 0.95; // absolute output ceiling, post-limiter

fn hard_clip(x: f32, limit: f32) -> f32 {
    x.clamp(-limit, limit)
}

fn soft_clip(x: f32) -> f32 {
    x.tanh()
}

// ----------------------------------------------------------------------
// Command surface
// ----------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum AudioCommand {
    /// Fade the ambient bed in. The game has begun.
    Begin,
    /// One positional corridor rumble; pan in [-1, 1] (equal-power).
    CorridorRumble { pan: f32 },
    /// The Rush approach: mono overdrive channel for `duration_ms`.
    TriggerRush { duration_ms: u64 },
    /// Instantly mute every ambient source. Used by the snap macro.
    KillAmbient,
    /// Full-scale decorrelated dual-channel static for `duration_ms`.
    StaticScreech { duration_ms: u64, gain: f32 },
    /// Ramp the master to silence over ~2 s. The engine is ending.
    FadeOut,
}

#[derive(Clone)]
pub struct AudioHandle {
    mailbox: Arc<Mutex<VecDeque<AudioCommand>>>,
}

impl AudioHandle {
    /// Non-blocking from the caller's perspective (game thread); the audio
    /// thread pops with try_lock. Nobody ever waits on anybody.
    pub fn send(&self, cmd: AudioCommand) {
        self.mailbox.lock().unwrap().push_back(cmd);
    }
}

/// THE GLITCHED WINDOW SNAP — the spec'd meta-scare macro.
/// Instantly kills every ambient track, then forces a maximum-volume,
/// dual-channel static screech for `dur_ms`. The limiter pins it at the
/// ceiling for the full duration. There is no fade-in. That is the point.
#[macro_export]
macro_rules! glitched_window_snap {
    ($audio:expr, $dur_ms:expr) => {{
        $audio.send($crate::audio::AudioCommand::KillAmbient);
        $audio
            .send($crate::audio::AudioCommand::StaticScreech { duration_ms: $dur_ms, gain: 0.95 });
    }};
}

// ----------------------------------------------------------------------
// DSP primitives
// ----------------------------------------------------------------------

/// One-pole lowpass. The room tone and rumble body both live in here.
struct OnePoleLP {
    g: f32,
    z: f32,
}

impl OnePoleLP {
    fn new(cutoff_hz: f32) -> Self {
        let g = 1.0 - (-TAU_F * cutoff_hz / SAMPLE_RATE).exp();
        OnePoleLP { g: g.clamp(0.0, 1.0), z: 0.0 }
    }
    fn run(&mut self, x: f32) -> f32 {
        self.z += self.g * (x - self.z);
        self.z
    }
}

/// Dynamic real-time bitcrusher: quantise to `bits` levels AND sample-hold
/// every `hold_every` input samples (decimation). Over the Rush approach the
/// bit depth collapses 12 -> 3 while the hold interval stretches 1x -> 26x —
/// the entity's voice de-resolving as it gets closer.
struct Bitcrusher {
    bits: f32,
    hold_every: u32,
    counter: u32,
    held: f32,
}

impl Bitcrusher {
    fn new() -> Self {
        Bitcrusher { bits: 12.0, hold_every: 1, counter: 0, held: 0.0 }
    }
    fn process(&mut self, x: f32) -> f32 {
        if self.counter == 0 {
            let levels = 2f32.powf(self.bits - 1.0).max(1.0);
            self.held = (x * levels).round() / levels;
        }
        self.counter += 1;
        if self.counter >= self.hold_every.max(1) {
            self.counter = 0;
        }
        self.held
    }
}

/// Feedback clipping: a fractional-read comb with >unity feedback and a hard
/// clipper inside the loop. Above unity the loop runaway-breeds harmonics
/// until the clipper catches them — the resonant howl of the Rush entity.
/// The delay time is modulated live: shortening it drives the pitch up.
struct FeedbackHowl {
    buf: Vec<f32>,
    write: usize,
    delay_samps: f32,
    feedback: f32,
}

impl FeedbackHowl {
    fn new() -> Self {
        FeedbackHowl { buf: vec![0.0; 2048], write: 0, delay_samps: 380.0, feedback: 0.9 }
    }
    fn set_delay(&mut self, d: f32) {
        self.delay_samps = d.clamp(4.0, (self.buf.len() - 2) as f32);
    }
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        let rpos = (self.write as f32 - self.delay_samps).rem_euclid(n as f32);
        let i0 = rpos.floor() as usize % n;
        let i1 = (i0 + 1) % n;
        let frac = rpos - rpos.floor();
        let y = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;
        // the loop: gain above unity, bounded only by the clipper
        let v = hard_clip(x + y * self.feedback, 0.97);
        self.buf[self.write] = v;
        self.write = (self.write + 1) % n;
        v
    }
}

// ----------------------------------------------------------------------
// Voices
// ----------------------------------------------------------------------

/// A positional corridor rumble: heavily lowpassed noise body plus a 42 Hz
/// sub thump on the onset, equal-power panned across the stereo field.
struct RumbleVoice {
    t: f32,
    dur: f32,
    pan: f32,
    lp1: OnePoleLP,
    lp2: OnePoleLP,
}

impl RumbleVoice {
    fn new(pan: f32) -> Self {
        RumbleVoice {
            t: 0.0,
            dur: 2.6,
            pan: pan.clamp(-1.0, 1.0),
            lp1: OnePoleLP::new(95.0),
            lp2: OnePoleLP::new(120.0),
        }
    }
    fn render(&mut self, rng: &mut TinyRng, dt: f32) -> (f32, f32) {
        self.t += dt;
        let p = self.t / self.dur;
        let env = if p < 0.12 {
            p / 0.12
        } else {
            (1.0 - (p - 0.12) / 0.88).max(0.0).powf(1.7)
        };
        let body = self.lp2.run(self.lp1.run(rng.next_bipolar())) * env;
        let thump = if self.t < 0.5 {
            ((self.t as f64 * 42.0 * TAU).sin() as f32) * (1.0 - self.t / 0.5).powi(2) * 0.9
        } else {
            0.0
        };
        let mono = (body * 1.4 + thump) * 0.55;
        let theta = (self.pan + 1.0) * std::f32::consts::FRAC_PI_2 / 2.0;
        (mono * theta.cos(), mono * theta.sin())
    }
    fn alive(&self) -> bool {
        self.t < self.dur
    }
}

/// THE RUSH VOICE — the entity's mono overdrive channel.
///
/// Layers, driven by the approach parameter k = t/duration:
///   (1) sub foundation, 28 -> 75 Hz, rising as k^2
///   (2) detuned saw pair through the bitcrusher (12 -> 3 bits, 1x -> 26x)
///   (3) the feedback howl: comb delay shrinking 380 -> ~58 samples
///       (pitch climbing), feedback 0.92 -> 1.47 (runaway into the clipper)
///   (4) terminal hiss, arriving only in the last third
///   (5) the normaliser: a slow AGC rides the gain so the peak pins at 0.9
///       regardless of what the layers are doing — constant, hot, LOUD.
struct RushVoice {
    t: f32,
    dur: f32,
    sub_ph: f64,
    saw_ph: f64,
    saw2_ph: f64,
    crush: Bitcrusher,
    howl: FeedbackHowl,
    rng: TinyRng,
    peak_env: f32,
    agc: f32,
}

impl RushVoice {
    fn new(duration_s: f32) -> Self {
        RushVoice {
            t: 0.0,
            dur: duration_s.max(1.0),
            sub_ph: 0.0,
            saw_ph: 0.0,
            saw2_ph: 0.0,
            crush: Bitcrusher::new(),
            howl: FeedbackHowl::new(),
            rng: TinyRng::new(0xD00_5EED),
            peak_env: 0.02,
            agc: 1.0,
        }
    }

    fn render(&mut self, dt: f32) -> f32 {
        self.t += dt;
        let k = (self.t / self.dur).min(1.0);
        let k2 = k * k;
        let tail = if self.t > self.dur {
            (1.0 - (self.t - self.dur) / 0.35).max(0.0)
        } else {
            1.0
        };
        if tail <= 0.0 {
            return 0.0;
        }

        // (1) rising sub
        let sub_f = 28.0 + 47.0 * k2;
        self.sub_ph += sub_f as f64 * TAU / SR64;
        let sub = self.sub_ph.sin() as f32 * (0.20 + 0.55 * k2);

        // (2) overdriven growl into the bitcrusher
        let growl_f = 52.0 + 98.0 * k2;
        self.saw_ph += growl_f as f64 * TAU / SR64;
        self.saw2_ph += growl_f as f64 * 1.011 * TAU / SR64;
        let saw = (self.saw_ph.fract() as f32 * 2.0 - 1.0
            + self.saw2_ph.fract() as f32 * 2.0
            - 1.0)
            * 0.5;
        self.crush.bits = 12.0 - 9.0 * k;
        self.crush.hold_every = 1 + (k * 25.0) as u32;
        let crushed = self.crush.process(saw) * (0.45 + 0.85 * k);

        // (3) the runaway feedback howl
        let vib = (self.t * 13.0).sin();
        self.howl.set_delay(380.0 - 322.0 * k2 + vib * 14.0);
        self.howl.feedback = 0.92 + 0.55 * k;
        let howl = self.howl.process(crushed * 0.6) * (0.5 + 0.6 * k);

        // (4) terminal hiss
        let hiss = self.rng.next_bipolar() * (k2 * 0.30);

        let mix = sub + crushed * 0.75 + howl + hiss;

        // (5) volume-normalised mono overdrive
        let a = mix.abs();
        self.peak_env = a.max(self.peak_env * 0.99990);
        let desired = (0.9 / self.peak_env.max(0.02)).min(8.0);
        self.agc += (desired - self.agc) * 0.00025;
        soft_clip(mix * self.agc) * tail
    }

    fn alive(&self) -> bool {
        self.t < self.dur + 0.4
    }
}

/// THE SCREECH — decorrelated dual-channel white static at full gain,
/// chopped by an irregular ~31 Hz gate. Left and right are *independent*
/// noise streams: the brain cannot localise it, which is precisely the
/// discomfort we are buying. Hard cut at the end — no fade, no mercy.
struct ScreechVoice {
    t: f32,
    dur: f32,
    gain: f32,
    rng: TinyRng,
    gate_ph: f64,
}

impl ScreechVoice {
    fn new(duration_s: f32, gain: f32) -> Self {
        ScreechVoice {
            t: 0.0,
            dur: duration_s,
            gain,
            rng: TinyRng::new(0x57A_CC0),
            gate_ph: 0.0,
        }
    }
    fn render(&mut self, dt: f32) -> (f32, f32) {
        self.t += dt;
        self.gate_ph += dt as f64 * TAU * 31.0;
        let wob = (self.gate_ph + (self.t as f64 * 13.1).sin() * 1.7).sin();
        let gate = if wob > -0.3 { 1.0 } else { 0.0 };
        let l = hard_clip(self.rng.next_bipolar() * 3.4, 1.0);
        let r = hard_clip(self.rng.next_bipolar() * 3.4, 1.0);
        (l * self.gain * gate, r * self.gain * gate)
    }
    fn alive(&self) -> bool {
        self.t < self.dur
    }
}

// ----------------------------------------------------------------------
// The mixer — rodio Source, the heart of the engine
// ----------------------------------------------------------------------

pub struct DreadMixer {
    mailbox: Arc<Mutex<VecDeque<AudioCommand>>>,
    rng: TinyRng,
    // ambient bed
    ambient_on: bool,
    ambient_env: f32,
    ambient_target: f32,
    ambient_rate: f32,
    dr1: f64,
    dr2: f64,
    dr3: f64,
    lfo_ph: f64,
    room_l: OnePoleLP,
    room_r: OnePoleLP,
    // voices
    rumbles: Vec<RumbleVoice>,
    rush: Option<RushVoice>,
    screech: Option<ScreechVoice>,
    // master
    master: f32,
    fading_out: bool,
    // stereo interleaving
    sample: u64,
    frame: [f32; 2],
}

impl DreadMixer {
    fn new(mailbox: Arc<Mutex<VecDeque<AudioCommand>>>) -> Self {
        DreadMixer {
            mailbox,
            rng: TinyRng::new(0xBA_D1E_A),
            ambient_on: false,
            ambient_env: 0.0,
            ambient_target: 0.0,
            ambient_rate: 0.35,
            dr1: 0.0,
            dr2: 0.0,
            dr3: 0.0,
            lfo_ph: 0.0,
            room_l: OnePoleLP::new(320.0),
            room_r: OnePoleLP::new(290.0),
            rumbles: Vec::with_capacity(4),
            rush: None,
            screech: None,
            master: 1.0,
            fading_out: false,
            sample: 0,
            frame: [0.0, 0.0],
        }
    }

    /// Drains the command mailbox. try_lock only — the realtime thread must
    /// never wait on the game thread holding the lock.
    fn drain_mailbox(&mut self) {
        let Ok(mut q) = self.mailbox.try_lock() else { return };
        let mut budget = 16;
        while budget > 0 {
            match q.pop_front() {
                Some(cmd) => {
                    self.apply(cmd);
                    budget -= 1;
                }
                None => break,
            }
        }
    }

    fn apply(&mut self, cmd: AudioCommand) {
        match cmd {
            AudioCommand::Begin => {
                self.ambient_on = true;
                self.ambient_target = 1.0;
                self.ambient_rate = 0.35; // slow fade-in over ~3 s
            }
            AudioCommand::CorridorRumble { pan } => {
                if self.rumbles.len() < 4 {
                    self.rumbles.push(RumbleVoice::new(pan));
                }
            }
            AudioCommand::TriggerRush { duration_ms } => {
                // The transition the spec demands: the 3D corridor stage
                // ducks while the mono overdrive channel takes over.
                self.rush = Some(RushVoice::new(duration_ms as f32 / 1000.0));
                self.ambient_target = 0.35;
                self.ambient_rate = 2.0;
            }
            AudioCommand::KillAmbient => {
                // Instantly. No fade. The snap macro's first move.
                self.ambient_target = 0.0;
                self.ambient_rate = 40.0;
                self.rumbles.clear();
            }
            AudioCommand::StaticScreech { duration_ms, gain } => {
                self.screech = Some(ScreechVoice::new(duration_ms as f32 / 1000.0, gain));
            }
            AudioCommand::FadeOut => {
                self.fading_out = true;
            }
        }
    }

    /// The ambient bed: detuned drone oscillators breathing on a 0.11 Hz LFO,
    /// over decorrelated filtered room tone per channel.
    fn render_ambient(&mut self, dt: f32) -> (f32, f32) {
        if !self.ambient_on {
            return (0.0, 0.0);
        }
        let d = self.ambient_target - self.ambient_env;
        self.ambient_env += d.clamp(-self.ambient_rate * dt, self.ambient_rate * dt);
        if self.ambient_env <= 0.0001 && self.ambient_target <= 0.0 {
            return (0.0, 0.0);
        }
        let e = self.ambient_env;

        let drone = (self.dr1.sin() as f32) * 0.4
            + (self.dr2.sin() as f32) * 0.35
            + (self.dr3.sin() as f32) * 0.5;
        let lfo = self.lfo_ph.sin() as f32;
        let drone = drone * (0.6 + 0.4 * lfo);

        self.dr1 += 52.0 * TAU / SR64;
        self.dr2 += 52.33 * TAU / SR64;
        self.dr3 += 26.0 * TAU / SR64;
        self.lfo_ph += 0.11 * TAU / SR64;

        let rl = self.room_l.run(self.rng.next_bipolar()) * 0.55;
        let rr = self.room_r.run(self.rng.next_bipolar()) * 0.55;
        let bed = 0.16 * e;
        ((drone * 0.5 + rl) * bed, (drone * 0.5 + rr) * bed)
    }

    /// Renders one stereo frame: sum, master fade, tanh overdrive, brickwall.
    fn render_frame(&mut self) {
        self.drain_mailbox();
        let dt = FRAME_DT;

        let (mut l, mut r) = self.render_ambient(dt);

        // positional corridor rumbles (the 3D stage)
        self.rumbles.retain(|v| v.alive());
        for v in self.rumbles.iter_mut() {
            let (rl, rr) = v.render(&mut self.rng, dt);
            l += rl;
            r += rr;
        }

        // the rush voice: MONO, summed equally into both channels
        let mut rush_m = 0.0;
        let mut rush_done = false;
        if let Some(rush) = self.rush.as_mut() {
            rush_m = rush.render(dt);
            rush_done = !rush.alive();
        }
        if rush_done {
            self.rush = None;
        }
        l += rush_m;
        r += rush_m;

        // the screech: independent L/R
        let mut scr = (0.0, 0.0);
        let mut scr_done = false;
        if let Some(s) = self.screech.as_mut() {
            scr = s.render(dt);
            scr_done = !s.alive();
        }
        if scr_done {
            self.screech = None;
        }
        l += scr.0;
        r += scr.1;

        // master: fade-out ramp -> tanh drive -> hard brickwall
        if self.fading_out {
            self.master = (self.master - dt * 0.5).max(0.0);
        }
        l *= self.master;
        r *= self.master;
        let l = soft_clip(l * 2.4) * 0.98;
        let r = soft_clip(r * 2.4) * 0.98;
        self.frame = [hard_clip(l, CEILING), hard_clip(r, CEILING)];
    }
}

impl Iterator for DreadMixer {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let out = if self.sample & 1 == 0 {
            self.render_frame();
            self.frame[0]
        } else {
            self.frame[1]
        };
        self.sample = self.sample.wrapping_add(1);
        Some(out)
    }
}

impl rodio::Source for DreadMixer {
    fn current_frame_len(&self) -> Option<usize> {
        None // infinite — the engine's lifecycle is managed above the mixer
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE as u32
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// ----------------------------------------------------------------------
// Engine bootstrap
// ----------------------------------------------------------------------

/// Opens the output device and returns the three things main.rs must keep
/// alive: the cheap clone-able command handle, the rodio OutputStream (owning
/// the device), and the Sink (the kill-switch's first target).
pub fn init_audio() -> (AudioHandle, rodio::OutputStream, rodio::Sink) {
    let mailbox = Arc::new(Mutex::new(VecDeque::with_capacity(16)));
    let handle = AudioHandle { mailbox: mailbox.clone() };

    let (stream, stream_handle) = rodio::OutputStream::try_default()
        .expect("the Sonic Dread Engine could not open an audio device");
    let sink = rodio::Sink::try_new(&stream_handle)
        .expect("failed to attach the output sink");

    let mixer = DreadMixer::new(mailbox);
    sink.append(mixer);

    (handle, stream, sink)
}
