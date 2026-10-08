//! The M10.2 Phase 4 audio pass (PLAN-M10.2 §4): synthesized sounds behind
//! the engine's [`AudioSink`] seam, so events and UI moments feel real.
//!
//! Everything here is presentation-only (FD-9): the sink is fed after the
//! step from the wiring layer and feeds nothing back. Three pure pieces —
//!
//! - [`CueBank`]: the nine cue PCM buffers, synthesized deterministically
//!   at startup (PLAN §4.2's "no asset licensing" — mono f32 at one fixed
//!   [`SAMPLE_RATE`], every buffer under a second, all nine pairwise
//!   distinct by construction);
//! - [`RateLimiter`]: the per-cue cooldown (PLAN §4.4's "so it does not
//!   become noise") — the first cue of a kind voices, repeats inside the
//!   window drop, and the window reopens after, independently per cue;
//! - [`Sound`]: the sink itself. The constructor probes the device once
//!   (ADR-0002): a device means the rodio arm — the stream owner lives in
//!   the struct for the App's whole lifetime (dropping it silences
//!   everything), cues hand the pre-built buffer to the mixer, an
//!   infallible channel send that never blocks the render loop. ANY probe
//!   failure (no device, Xvfb, CI) or a device lost mid-session takes the
//!   other arm: the engine's [`NullAudioSink`], silently — no panic, no
//!   stderr spam, one honest startup line, and the counters keep saying
//!   what the audio did (the machine cannot hear; the evidence lines never
//!   claim more than counting).
//!
//! The machine cannot hear: Xvfb and CI have no audio device, so every
//! machine-verified run here exercises the null arm. Sounds are distinct
//! *by construction* (each cue's recipe differs in frequency, envelope,
//! and duration, pinned by test) — whether they *sound good* is the
//! owner's re-test on real hardware, never a machine claim.

use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pandemonium_engine::audio::{cue_for, AudioCue, AudioSink, NullAudioSink};
use pandemonium_sim_api::Event;
use rodio::source::Source;
use rodio::{DeviceSinkBuilder, MixerDeviceSink};

/// The one fixed synthesis rate (picked once, logged at startup): every
/// cue buffer is mono `f32` PCM at this rate, and the mixer resamples to
/// whatever the device actually wants — there is exactly one rate in this
/// pass, never a per-cue or per-device choice.
pub const SAMPLE_RATE: u32 = 44_100;

/// The per-cue cooldown (PLAN §4.4's ~50-100 ms window): a big fight's
/// many-attacks-per-tick collapse into at most one voice per cue per
/// window, so combat stays a heartbeat instead of noise.
pub const CUE_COOLDOWN: Duration = Duration::from_millis(80);

/// Mono output — one channel everywhere in this pass.
fn mono() -> rodio::ChannelCount {
    NonZero::new(1).expect("one channel is nonzero")
}

/// The synthesis rate as rodio's nonzero type.
fn rate() -> rodio::SampleRate {
    NonZero::new(SAMPLE_RATE).expect("44100 is nonzero")
}

// ── the cue bank: nine synthesized buffers ────────────────────────────

/// The nine pre-built cue buffers (PLAN §4.2's one distinct sound per
/// moment): mono [`SAMPLE_RATE`] PCM, generated once at startup by pure
/// functions — deterministic (two runs synthesize identical banks), short
/// (every buffer is well under a second), and pairwise distinct.
pub struct CueBank {
    buffers: [Vec<f32>; AudioCue::ALL.len()],
}

impl CueBank {
    /// Synthesizes the whole bank (a pure function of nothing).
    pub fn generate() -> Self {
        Self {
            buffers: AudioCue::ALL.map(generate_cue),
        }
    }

    /// The cue's PCM buffer (mono, [`SAMPLE_RATE`]).
    pub fn buffer(&self, cue: AudioCue) -> &[f32] {
        &self.buffers[cue.index()]
    }
}

/// One cue's recipe → its PCM buffer, soft-limited into the rail (peaks
/// past 0.95 pull back to it — a chord's summed samples can exceed 1.0,
/// and valid PCM is part of the bank's contract).
fn generate_cue(cue: AudioCue) -> Vec<f32> {
    soft_limit(match cue {
        AudioCue::AttackLanded => attack_landed(),
        AudioCue::UnitLost => unit_lost(),
        AudioCue::UnitReady => unit_ready(),
        AudioCue::StructureDone => structure_done(),
        AudioCue::Delivery => delivery(),
        AudioCue::MatchEnded => match_ended(),
        AudioCue::CommandAck => command_ack(),
        AudioCue::SelectionClick => selection_click(),
        AudioCue::UiClick => ui_click(),
    })
}

/// Scales a finished cue into the `[-1, 1]` rail: a peak at or above 0.95
/// is pulled back to 0.95 (every buffer valid PCM by construction,
/// shapes preserved).
fn soft_limit(mut buffer: Vec<f32>) -> Vec<f32> {
    let peak = buffer
        .iter()
        .fold(0.0f32, |acc, sample| acc.max(sample.abs()));
    if peak > 0.95 {
        let scale = 0.95 / peak;
        for sample in &mut buffer {
            *sample *= scale;
        }
    }
    buffer
}

/// A phase-accumulated sine sweeping `f0 -> f1` over `seconds`, shaped by a
/// linear attack and an exponential decay — the pass's one tone shape
/// (cues differ by frequency, sweep direction, envelope, and duration).
fn tone(f0: f32, f1: f32, seconds: f32, attack_s: f32, decay_per_s: f32, gain: f32) -> Vec<f32> {
    let frames = (seconds * SAMPLE_RATE as f32).round() as usize;
    let mut out = Vec::with_capacity(frames);
    let mut phase = 0.0f32;
    for frame in 0..frames {
        let t = frame as f32 / SAMPLE_RATE as f32;
        let sweep = f0 + (f1 - f0) * (t / seconds);
        phase += std::f32::consts::TAU * sweep / SAMPLE_RATE as f32;
        let attack = (t / attack_s).min(1.0);
        let decay = (-decay_per_s * t).exp();
        out.push(gain * attack * decay * phase.sin());
    }
    out
}

/// A decaying deterministic noise burst (a 32-bit xorshift — no RNG crate,
/// no OS entropy; the bank is a pure function of its constants, so two
/// runs synthesize identical bytes).
fn noise_burst(seconds: f32, decay_per_s: f32, gain: f32, seed: u32) -> Vec<f32> {
    let frames = (seconds * SAMPLE_RATE as f32).round() as usize;
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(frames);
    for frame in 0..frames {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let white = state as f32 / u32::MAX as f32 * 2.0 - 1.0;
        let t = frame as f32 / SAMPLE_RATE as f32;
        let decay = (-decay_per_s * t).exp();
        out.push(gain * white * decay);
    }
    out
}

/// Mixes two buffers into one (summed sample-by-sample, zero-padded to the
/// longer — a chord, not a sequence).
fn mix(a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    let len = a.len().max(b.len());
    let mut out = vec![0.0f32; len];
    for (sample, a_sample) in out.iter_mut().zip(a) {
        *sample += a_sample;
    }
    for (sample, b_sample) in out.iter_mut().zip(b) {
        *sample += b_sample;
    }
    out
}

/// Concatenates two buffers into one (a sequence, not a chord).
fn seq(a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    let mut out = a;
    out.extend_from_slice(&b);
    out
}

/// **AttackLanded** — a low thud: a fast 90->55 Hz body under a 30 ms noise
/// click. The combat heartbeat: short, low, percussive.
fn attack_landed() -> Vec<f32> {
    mix(
        tone(90.0, 55.0, 0.10, 0.002, 22.0, 0.9),
        noise_burst(0.03, 40.0, 0.35, 0x0A11_0BAD),
    )
}

/// **UnitLost** — a descending two-step (520 Hz, then 340->300 Hz): a loss
/// reads as "down", and two steps keep it distinct from any sweep.
fn unit_lost() -> Vec<f32> {
    seq(
        tone(520.0, 520.0, 0.09, 0.004, 9.0, 0.55),
        tone(340.0, 300.0, 0.13, 0.004, 7.0, 0.5),
    )
}

/// **UnitReady** — a rising 440->900 Hz chirp capped by a short 900 Hz
/// blip: production done reads as "up and ready".
fn unit_ready() -> Vec<f32> {
    seq(
        tone(440.0, 900.0, 0.13, 0.004, 6.0, 0.5),
        tone(900.0, 900.0, 0.05, 0.002, 12.0, 0.45),
    )
}

/// **StructureDone** — a solid low chord (115 Hz + 230 Hz, the octave),
/// slow-ish decay: construction finished reads as heavy and finished.
fn structure_done() -> Vec<f32> {
    mix(
        tone(115.0, 100.0, 0.26, 0.006, 6.0, 0.7),
        tone(230.0, 210.0, 0.26, 0.006, 5.0, 0.35),
    )
}

/// **Delivery** — a bright ding: 1318 Hz with its 1976 Hz overtone dying
/// faster — the economy's gentle bell.
fn delivery() -> Vec<f32> {
    mix(
        tone(1318.0, 1318.0, 0.19, 0.001, 8.0, 0.45),
        tone(1976.0, 1976.0, 0.12, 0.001, 10.0, 0.2),
    )
}

/// **MatchEnded** — a three-tone ascending fanfare (C5, E5, G5->C6): the
/// longest cue, still under 0.6 s, and unmistakably "the end".
fn match_ended() -> Vec<f32> {
    seq(
        seq(
            tone(523.0, 523.0, 0.16, 0.006, 4.0, 0.55),
            tone(659.0, 659.0, 0.16, 0.006, 4.0, 0.55),
        ),
        tone(784.0, 1046.0, 0.22, 0.006, 3.0, 0.6),
    )
}

/// **CommandAck** — a sharp high tick (1050 Hz, 35 ms): "order submitted".
fn command_ack() -> Vec<f32> {
    tone(1050.0, 1050.0, 0.035, 0.001, 45.0, 0.5)
}

/// **SelectionClick** — a soft low tick (500->480 Hz, 28 ms): quieter than
/// the ack, so selecting many units never competes with ordering them.
fn selection_click() -> Vec<f32> {
    tone(500.0, 480.0, 0.028, 0.001, 50.0, 0.45)
}

/// **UiClick** — an up-blip (760->840 Hz, 45 ms): the menus' own sound,
/// distinct from both the selection tick and the command ack.
fn ui_click() -> Vec<f32> {
    tone(760.0, 840.0, 0.045, 0.002, 30.0, 0.45)
}

// ── the rate limiter ──────────────────────────────────────────────────

/// The per-cue rate limiter (PLAN §4.4): the first cue of a kind passes,
/// repeats inside the cooldown drop, the window reopens after — each cue
/// independent of the others. Pure: the caller supplies the clock (`now`
/// is an input), so the tests pin the window without sleeping.
pub struct RateLimiter {
    cooldown: Duration,
    last: [Option<Instant>; AudioCue::ALL.len()],
}

impl RateLimiter {
    /// A limiter with this cooldown, all windows closed-open (nothing has
    /// played yet).
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            last: [None; AudioCue::ALL.len()],
        }
    }

    /// Whether this cue may voice at `now` — and remembers it if it may.
    pub fn allow(&mut self, cue: AudioCue, now: Instant) -> bool {
        match self.last[cue.index()] {
            Some(then) if now.duration_since(then) < self.cooldown => false,
            _ => {
                self.last[cue.index()] = Some(now);
                true
            }
        }
    }

    /// Forgets every window (the match-boundary reset — a fresh match's
    /// first cue always voices).
    pub fn reset(&mut self) {
        self.last = [None; AudioCue::ALL.len()];
    }
}

// ── the sink ──────────────────────────────────────────────────────────

/// The device arm's payload (boxed inside [`Arm`] — it is the heavy
/// variant, and the sink stays one pointer wide either way).
struct DeviceArm {
    /// The stream owner (0.22's `OutputStream` successor), kept for the
    /// App's whole lifetime — dropping it ends playback (ADR-0002's
    /// lifetime rule).
    sink: MixerDeviceSink,
    /// Set by the stream's error callback the moment the device dies
    /// mid-session — polled per cue, one degrade, never a panic.
    degraded: Arc<AtomicBool>,
    /// The nine pre-built buffers.
    bank: CueBank,
}

/// The output arm: a probed-live device, or the null fallback.
enum Arm {
    /// The rodio arm: the device stream owner plus its pre-built cue bank.
    Device(Box<DeviceArm>),
    /// The fallback arm: no device found at startup (CI, Xvfb, headless)
    /// or one lost mid-session. The engine's counter sink counts what
    /// would have played — that counting IS the fallback's behavior
    /// ("fall back to NullAudioSink", PLAN §4.3).
    Null(NullAudioSink),
}

/// The client's audio sink: the rate limiter, the volume, and one output
/// arm, behind the engine's [`AudioSink`] seam. Constructed once per run
/// (`App::new` / the headless smoke); the arm survives every match
/// boundary (the match reset clears counters and windows, never the
/// device).
pub struct Sound {
    arm: Arm,
    limiter: RateLimiter,
    /// Master volume, 0.0..=1.0 — applied live to every voice (A-115:
    /// changes apply immediately; Done persists, Esc leaves the run's
    /// value unsaved).
    volume: f32,
    /// Mute — beats the volume (PLAN §4.4: a muted non-zero volume is
    /// silent; unmuting restores the volume untouched). The cue
    /// counters keep counting: muting silences output, not evidence.
    muted: bool,
    /// The evidence counters, reset per match like the other match-scoped
    /// counters (the `--frames` summary reports the live segment).
    events_fed: usize,
    cues_fed: usize,
    cues_voiced: usize,
    cues_dropped: usize,
    /// How many client-side cues reached [`AudioSink::on_cue`] (the
    /// wiring-layer evidence — pre-limiter: the wiring ran).
    client_cues: usize,
}

impl Sound {
    /// The real constructor (PLAN §4.1/§4.3): probe the device ONCE at
    /// startup. A live device means the rodio arm and one honest startup
    /// line; ANY failure — no device, Xvfb, CI, a probe error — means the
    /// null arm, silently: no panic, no stderr spam, one honest line.
    /// Never blocks the render loop (the probe runs before it exists).
    /// The volume and mute come from the loaded settings (Phase 4 made
    /// them live rows).
    pub fn new(volume: f32, muted: bool) -> Self {
        match probe_device() {
            Some((sink, degraded)) => {
                println!(
                    "pandemonium client — audio: active (rodio), {} Hz, {} synthesized cues",
                    SAMPLE_RATE,
                    AudioCue::ALL.len()
                );
                Self {
                    arm: Arm::Device(Box::new(DeviceArm {
                        sink,
                        degraded,
                        bank: CueBank::generate(),
                    })),
                    limiter: RateLimiter::new(CUE_COOLDOWN),
                    volume: volume.clamp(0.0, 1.0),
                    muted,
                    events_fed: 0,
                    cues_fed: 0,
                    cues_voiced: 0,
                    cues_dropped: 0,
                    client_cues: 0,
                }
            }
            None => Self::without_device(volume, muted),
        }
    }

    /// The constructor's other arm, built directly — the no-device
    /// fallback (tests pin this path; the machine pass exercises it under
    /// Xvfb and CI, where the probe finds nothing).
    pub fn without_device(volume: f32, muted: bool) -> Self {
        println!(
            "pandemonium client — audio: no device — null fallback (cues counted, never voiced)"
        );
        Self {
            arm: Arm::Null(NullAudioSink::new()),
            limiter: RateLimiter::new(CUE_COOLDOWN),
            volume: volume.clamp(0.0, 1.0),
            muted,
            events_fed: 0,
            cues_fed: 0,
            cues_voiced: 0,
            cues_dropped: 0,
            client_cues: 0,
        }
    }

    /// The live master volume (A-115): every later voice uses it. Saved by
    /// Done, left unsaved-but-live by Esc — the settings screen's own law.
    /// Muting never touches the stored volume (mute beats it, it does not
    /// zero it — unmuting restores the loudness the player set).
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// The live mute toggle (PLAN §4.4): beats the volume — the effective
    /// gain is zero while muted, whatever the volume says. Counters keep
    /// counting (the evidence is the wiring, not the loudness).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// The effective output gain: zero when muted (mute beats volume),
    /// else the master volume.
    pub fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }

    /// Whether the device arm is live (the evidence line's
    /// active-vs-fallback fact — a claim about the arm, never about
    /// audible sound).
    pub fn is_active(&self) -> bool {
        matches!(self.arm, Arm::Device(_))
    }

    /// The match-boundary reset: counters and rate-limit windows zero; the
    /// arm, the bank, and the volume persist (the device outlives matches
    /// — a fresh match is not a fresh probe).
    pub fn reset(&mut self) {
        self.limiter.reset();
        self.events_fed = 0;
        self.cues_fed = 0;
        self.cues_voiced = 0;
        self.cues_dropped = 0;
        self.client_cues = 0;
    }

    /// How many events the sink was fed (the smoke's old "events fed"
    /// evidence, unchanged in meaning).
    pub fn events_fed(&self) -> usize {
        self.events_fed
    }

    /// How many client-side cues reached the direct path (pre-limiter —
    /// the wiring-layer evidence).
    pub fn client_cues(&self) -> usize {
        self.client_cues
    }

    /// The audio evidence line (the `--frames` summary and the headless
    /// smoke print it): the arm's fact (active or fallback — the honest
    /// frame for every number that follows) and the cues' three fates —
    /// fed, voiced (passed the limiter into the arm), dropped (the
    /// limiter ate them).
    pub fn evidence_line(&self) -> String {
        let arm = if self.is_active() {
            format!("active (rodio, {} Hz)", SAMPLE_RATE)
        } else {
            "null fallback (no device)".to_string()
        };
        let mute = if self.muted { " [muted]" } else { "" };
        let client_side = self.client_cues();
        format!(
            "{arm}{mute}, cues fed/voiced/dropped {}/{}/{} ({client_side} client-side)",
            self.cues_fed, self.cues_voiced, self.cues_dropped
        )
    }

    /// One cue attempt: count it fed, rate-limit it, hand survivors to the
    /// output arm. Cheap by construction — the drop path is one array
    /// lookup, and the voice path is bounded by the cooldown (a big fight
    /// costs at most one voice per cue per window).
    fn voice(&mut self, cue: AudioCue) {
        self.cues_fed += 1;
        if !self.limiter.allow(cue, Instant::now()) {
            self.cues_dropped += 1;
            return;
        }
        self.cues_voiced += 1;
        self.output(cue);
    }

    /// Hands a surviving cue to the arm. Device arm: the pre-built buffer
    /// wrapped and appended to the mixer — an infallible channel send to
    /// rodio's mixing thread (never blocks, never panics). A device lost
    /// mid-session (the stream's error callback fired) degrades to the
    /// null arm: one line, once, and counting continues.
    fn output(&mut self, cue: AudioCue) {
        let device_degraded = match &self.arm {
            Arm::Device(device) => device.degraded.load(Ordering::Relaxed),
            Arm::Null(_) => false,
        };
        if device_degraded {
            println!(
                "pandemonium client — audio: device lost mid-session, degrading to null (counting \
                 continues)"
            );
            self.arm = Arm::Null(NullAudioSink::new());
        }
        let gain = self.gain();
        match &mut self.arm {
            Arm::Null(null) => null.on_cue(cue),
            Arm::Device(device) => {
                let buffer = device.bank.buffer(cue).to_vec();
                device.sink.mixer().add(
                    rodio::buffer::SamplesBuffer::new(mono(), rate(), buffer)
                        .amplify_normalized(gain),
                );
            }
        }
    }
}

/// Whether an audio device is even plausible before any library wakes
/// up (the cheap pre-probe). Linux: `/dev/snd` is the kernel sound stack's
/// device directory — its absence means no card, no daemon backend,
/// nothing for ALSA to open, and libasound's config parser would chatter
/// a stderr burst on its way to that same conclusion (this pass's rule is
/// a silent fallback, PLAN §4.3 — the pre-check keeps the no-device path
/// quiet). The corner: a socket-forwarded Pulse server without a local
/// `/dev/snd` reads as no-device here — logged as A-124, accepted (the
/// CI/headless class dwarfs that corner). Other platforms probe directly.
fn device_plausible() -> bool {
    if cfg!(target_os = "linux") {
        std::path::Path::new("/dev/snd").exists()
    } else {
        true
    }
}

/// Probes the default output device once (ADR-0002): builds rodio's stream
/// owner with a silent error callback (the default one prints to stderr —
/// this pass's rule is one honest line, no spam) that flags the shared
/// `degraded` bit the sink polls per cue. `None` is any failure at all.
fn probe_device() -> Option<(MixerDeviceSink, Arc<AtomicBool>)> {
    if !device_plausible() {
        return None; // the silent short-circuit (see `device_plausible`)
    }
    let degraded = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&degraded);
    let callback = move |_: rodio::cpal::StreamError| {
        flag.store(true, Ordering::Relaxed);
    };
    let builder = DeviceSinkBuilder::from_default_device()
        .ok()?
        .with_error_callback(callback);
    let mut sink = builder.open_sink_or_fallback().ok()?;
    // The drop notice would spam stderr at exit; the sink's own evidence
    // lines say everything this pass claims.
    sink.log_on_drop(false);
    Some((sink, degraded))
}

impl AudioSink for Sound {
    fn on_events(&mut self, events: &[Event]) {
        self.events_fed += events.len();
        for cue in events.iter().filter_map(cue_for) {
            self.voice(cue);
        }
    }

    fn on_cue(&mut self, cue: AudioCue) {
        self.client_cues += 1;
        self.voice(cue);
    }
}

#[cfg(test)]
impl Sound {
    /// The null arm's client-cue counter (PLAN §4.2's wiring oracle — the
    /// engine's `NullAudioSink` counting what reached the sink's direct
    /// path, post-limiter). 0 while the device arm is live (its cues go
    /// to the device, not the counter). Test-only: production evidence
    /// uses [`Sound::evidence_line`], which never claims the oracle.
    pub fn null_client_cues(&self) -> usize {
        match &self.arm {
            Arm::Null(null) => null.client_cues,
            Arm::Device(_) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cue_generates_a_short_non_empty_in_range_buffer() {
        let bank = CueBank::generate();
        for cue in AudioCue::ALL {
            let buffer = bank.buffer(cue);
            assert!(!buffer.is_empty(), "{} synthesizes something", cue.name());
            // "small mono buffers (< 1 s)" — PLAN §4.2's own size law.
            assert!(
                buffer.len() < SAMPLE_RATE as usize,
                "{} stays under one second ({} frames)",
                cue.name(),
                buffer.len()
            );
            // PCM in range: nothing clips the [-1, 1] rail.
            assert!(
                buffer.iter().all(|s| (-1.0..=1.0).contains(s)),
                "{} stays in range",
                cue.name()
            );
        }
    }

    #[test]
    fn the_nine_cue_buffers_are_pairwise_distinct() {
        // "One distinct sound per cue" (PLAN §4.2) — pinned at the buffer
        // level: every pair of cues synthesizes different PCM. (Distinct
        // *by construction* — the recipes differ in frequency, envelope,
        // and duration; this pin is the machine's half, "sounds good" is
        // the owner's.)
        let bank = CueBank::generate();
        for (i, a) in AudioCue::ALL.iter().enumerate() {
            for b in AudioCue::ALL.iter().skip(i + 1) {
                assert_ne!(
                    bank.buffer(*a),
                    bank.buffer(*b),
                    "{} and {} are distinct cues",
                    a.name(),
                    b.name()
                );
            }
        }
    }

    #[test]
    fn the_bank_is_deterministic() {
        // Pure synthesis: two runs generate identical banks (no entropy
        // anywhere — the noise burst's xorshift is seeded by a constant).
        assert_eq!(CueBank::generate().buffers, CueBank::generate().buffers);
    }

    #[test]
    fn the_rate_limiter_first_passes_repeats_drop_and_reopens() {
        let mut limiter = RateLimiter::new(Duration::from_millis(80));
        let t0 = Instant::now();
        assert!(
            limiter.allow(AudioCue::AttackLanded, t0),
            "the first passes"
        );
        assert!(
            !limiter.allow(AudioCue::AttackLanded, t0 + Duration::from_millis(10)),
            "a repeat inside the window drops"
        );
        assert!(
            !limiter.allow(AudioCue::AttackLanded, t0 + Duration::from_millis(79)),
            "still inside the window"
        );
        assert!(
            limiter.allow(AudioCue::AttackLanded, t0 + Duration::from_millis(80)),
            "the window reopens at the cooldown"
        );
        assert!(
            limiter.allow(AudioCue::AttackLanded, t0 + Duration::from_secs(10)),
            "and long after"
        );
    }

    #[test]
    fn rate_limiting_is_independent_per_cue() {
        // A big fight's AttackLanded storm does not silence UnitLost:
        // every cue's window is its own.
        let mut limiter = RateLimiter::new(Duration::from_millis(80));
        let t0 = Instant::now();
        assert!(limiter.allow(AudioCue::AttackLanded, t0));
        assert!(!limiter.allow(AudioCue::AttackLanded, t0 + Duration::from_millis(5)));
        assert!(limiter.allow(AudioCue::UnitLost, t0 + Duration::from_millis(5)));
        assert!(limiter.allow(AudioCue::Delivery, t0 + Duration::from_millis(6)));
        assert!(!limiter.allow(AudioCue::UnitLost, t0 + Duration::from_millis(10)));
        assert!(!limiter.allow(AudioCue::Delivery, t0 + Duration::from_millis(11)));
    }

    #[test]
    fn the_limiter_reset_reopens_every_window() {
        let mut limiter = RateLimiter::new(Duration::from_millis(80));
        let t0 = Instant::now();
        assert!(limiter.allow(AudioCue::UiClick, t0));
        assert!(!limiter.allow(AudioCue::UiClick, t0));
        limiter.reset();
        assert!(
            limiter.allow(AudioCue::UiClick, t0),
            "a fresh match's first cue always voices"
        );
    }

    #[test]
    fn the_no_device_constructor_takes_the_null_arm_without_panicking() {
        // The fallback path, pinned directly: the constructor's other arm
        // counts events and cues, is not "active", and never panics. The
        // machine's own probe result is not a test input — this arm is
        // built explicitly (the CI/Xvfb path's result).
        let mut sink = Sound::without_device(1.0, false);
        assert!(!sink.is_active(), "no device means the null arm");
        assert_eq!(sink.events_fed(), 0);
        sink.on_events(&[Event::Died {
            entity: pandemonium_sim_api::EntityId(1),
        }]);
        assert_eq!(sink.events_fed(), 1);
        assert_eq!(sink.cues_fed, 1, "the death mapped to a cue");
        assert_eq!(sink.cues_voiced, 1, "the first cue passes the limiter");
        // The real constructor must not panic either, whatever this
        // machine's probe finds (its arm is the machine's fact, printed
        // honestly at startup, never asserted here).
        let _ = Sound::new(1.0, false);
    }

    #[test]
    fn client_side_cues_reach_the_null_sink_counter() {
        // The wiring oracle: on the null arm, the engine's NullAudioSink
        // counts what reached the direct path (post-limiter — the limiter
        // sits between the cue source and the sink by design).
        let mut sink = Sound::without_device(1.0, false);
        assert_eq!(sink.null_client_cues(), 0);
        sink.on_cue(AudioCue::CommandAck);
        assert_eq!(sink.null_client_cues(), 1);
        assert_eq!(sink.client_cues(), 1, "the wiring-layer counter too");
        // An immediate repeat is fed and counted by the wiring, but the
        // limiter drops it before the sink: the oracle stays at 1.
        sink.on_cue(AudioCue::CommandAck);
        assert_eq!(sink.client_cues(), 2);
        assert_eq!(sink.cues_dropped, 1);
        assert_eq!(
            sink.null_client_cues(),
            1,
            "the drop happened before the sink"
        );
    }

    #[test]
    fn event_cues_and_client_cues_share_one_rate_limiter() {
        // PLAN §4.4: the limiter sits between the cue source and the sink,
        // whichever source — an event storm and a UI click of the same
        // cue kind share that cue's window.
        let mut sink = Sound::without_device(1.0, false);
        sink.on_cue(AudioCue::AttackLanded);
        assert_eq!(sink.cues_voiced, 1);
        sink.on_events(&[Event::AttackHit {
            attacker: pandemonium_sim_api::EntityId(1),
            target: pandemonium_sim_api::EntityId(2),
            damage: 8,
        }]);
        assert_eq!(sink.cues_fed, 2);
        assert_eq!(sink.cues_voiced, 1, "the event's cue hit the same window");
        assert_eq!(sink.cues_dropped, 1);
    }

    #[test]
    fn the_match_reset_clears_counters_and_windows_but_not_the_volume() {
        let mut sink = Sound::without_device(0.25, false);
        sink.on_cue(AudioCue::UiClick);
        sink.set_volume(0.5);
        sink.reset();
        assert_eq!(sink.events_fed(), 0);
        assert_eq!(
            (sink.cues_fed, sink.cues_voiced, sink.cues_dropped),
            (0, 0, 0)
        );
        assert_eq!(sink.client_cues(), 0);
        assert_eq!(
            sink.gain(),
            0.5,
            "volume survives the reset (the device does)"
        );
        // The window reopened: the first cue after the reset voices.
        sink.on_cue(AudioCue::UiClick);
        assert_eq!(sink.cues_voiced, 1);
    }

    #[test]
    fn the_volume_is_clamped_into_its_range_and_applied_live() {
        let mut sink = Sound::without_device(2.0, false);
        assert_eq!(sink.gain(), 1.0, "construction clamps");
        sink.set_volume(-0.5);
        assert_eq!(sink.gain(), 0.0, "and so does every live change");
        sink.set_volume(0.3);
        assert_eq!(sink.gain(), 0.3);
    }

    #[test]
    fn mute_beats_volume_and_never_zeros_it() {
        // PLAN §4.4's precedence: a muted non-zero volume is silent, and
        // unmuting restores the loudness the player set (the mute toggle
        // never writes the volume field).
        let mut sink = Sound::without_device(0.7, true);
        assert_eq!(sink.gain(), 0.0, "muted construction is silent");
        sink.set_volume(0.4);
        assert_eq!(sink.gain(), 0.0, "volume changes while muted stay silent");
        sink.set_muted(false);
        assert_eq!(sink.gain(), 0.4, "unmuting restores the set volume");
        sink.set_muted(true);
        assert_eq!(sink.gain(), 0.0);
        // Muting silences output, not evidence: the counters still count.
        sink.on_cue(AudioCue::UiClick);
        assert_eq!((sink.cues_fed, sink.cues_voiced), (1, 1));
    }

    #[test]
    fn the_evidence_line_names_the_arm_the_mute_and_the_three_fates() {
        let mut sink = Sound::without_device(1.0, false);
        sink.on_cue(AudioCue::UiClick);
        sink.on_cue(AudioCue::UiClick);
        let line = sink.evidence_line();
        assert!(line.contains("null fallback"), "the arm is named: {line}");
        assert!(
            line.contains("fed/voiced/dropped 2/1/1"),
            "the three fates are named: {line}"
        );
        assert!(
            line.contains("2 client-side"),
            "the wiring count is pre-limiter: {line}"
        );
        assert!(
            !line.contains("muted"),
            "an unmuted line does not claim mute"
        );
        sink.set_muted(true);
        assert!(
            sink.evidence_line().contains("[muted]"),
            "the mute is named"
        );
    }
}
