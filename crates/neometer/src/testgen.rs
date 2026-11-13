//! Test-signal generator of the Test page.
//!
//! **Safety rule:** the generator state is *not* a host parameter and is not
//! persisted. [`GenControl::new`] always creates it switched off, the plugin
//! forces it off again in `initialize()`, so a saved project or a host
//! automation lane can never make the plugin emit a signal by itself.
//!
//! Levels follow `neo_measure::generator`: tones are given as **peak** dBFS,
//! noise as **RMS** dBFS.
//!
//! Starting and stopping use a 20 ms raised-cosine envelope. In *replace*
//! mode the envelope also cross-fades with the input (`in * (1 - env) +
//! gen * env`), so switching on/off is click-free and, once the envelope is
//! back at zero, the input passes through bit-exactly again. Changing the
//! waveform while running fades out, switches, and fades in again.
//! Un-routed channels in replace mode are silent (fully faded input).

use atomic_float::AtomicF64;
use neo_measure::generator::{Generator, LogSweep, PinkNoise, Sine, WhiteNoise};
use neo_measure::util::db_to_amp;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Sine,
    Sweep,
    White,
    Pink,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Sine, Kind::Sweep, Kind::White, Kind::Pink];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sine => "Sine",
            Kind::Sweep => "Log sweep",
            Kind::White => "White noise",
            Kind::Pink => "Pink noise",
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Kind::Sweep,
            2 => Kind::White,
            3 => Kind::Pink,
            _ => Kind::Sine,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Routing {
    Left,
    Right,
    #[default]
    Both,
    /// R = -L ("vertical" modulation on a disc).
    BothInverted,
}

impl Routing {
    pub const ALL: [Routing; 4] = [
        Routing::Left,
        Routing::Right,
        Routing::Both,
        Routing::BothInverted,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Routing::Left => "L",
            Routing::Right => "R",
            Routing::Both => "L + R",
            Routing::BothInverted => "L + R inverted",
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Routing::Left,
            1 => Routing::Right,
            3 => Routing::BothInverted,
            _ => Routing::Both,
        }
    }
    fn gains(self) -> (f64, f64) {
        match self {
            Routing::Left => (1.0, 0.0),
            Routing::Right => (0.0, 1.0),
            Routing::Both => (1.0, 1.0),
            Routing::BothInverted => (1.0, -1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Output = generator (input is faded out).
    #[default]
    Replace,
    /// Output = input + generator.
    Add,
}

/// Plain copy of the generator settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenSettings {
    pub on: bool,
    pub kind: Kind,
    pub freq_hz: f64,
    pub level_db: f64,
    pub routing: Routing,
    pub mode: Mode,
    pub sweep_s: f64,
}

impl Default for GenSettings {
    fn default() -> Self {
        Self {
            on: false,
            kind: Kind::Sine,
            freq_hz: 1000.0,
            level_db: -20.0,
            routing: Routing::Both,
            mode: Mode::Replace,
            sweep_s: 10.0,
        }
    }
}

/// Lock-free generator settings shared by GUI and audio thread. Not
/// persisted, not automatable.
pub struct GenControl {
    on: AtomicBool,
    kind: AtomicU8,
    freq_hz: AtomicF64,
    level_db: AtomicF64,
    routing: AtomicU8,
    mode: AtomicU8,
    sweep_s: AtomicF64,
}

impl Default for GenControl {
    fn default() -> Self {
        Self::new()
    }
}

impl GenControl {
    /// Defaults, **switched off**.
    pub fn new() -> Self {
        let d = GenSettings::default();
        Self {
            on: AtomicBool::new(false),
            kind: AtomicU8::new(0),
            freq_hz: AtomicF64::new(d.freq_hz),
            level_db: AtomicF64::new(d.level_db),
            routing: AtomicU8::new(2),
            mode: AtomicU8::new(0),
            sweep_s: AtomicF64::new(d.sweep_s),
        }
    }

    pub fn get(&self) -> GenSettings {
        GenSettings {
            on: self.on.load(Ordering::Relaxed),
            kind: Kind::from_u8(self.kind.load(Ordering::Relaxed)),
            freq_hz: self.freq_hz.load(Ordering::Relaxed),
            level_db: self.level_db.load(Ordering::Relaxed),
            routing: Routing::from_u8(self.routing.load(Ordering::Relaxed)),
            mode: if self.mode.load(Ordering::Relaxed) == 1 {
                Mode::Add
            } else {
                Mode::Replace
            },
            sweep_s: self.sweep_s.load(Ordering::Relaxed),
        }
    }

    pub fn set(&self, s: GenSettings) {
        self.kind.store(s.kind as u8, Ordering::Relaxed);
        self.freq_hz
            .store(sane(s.freq_hz, 1.0, 96_000.0, 1000.0), Ordering::Relaxed);
        self.level_db
            .store(sane(s.level_db, -120.0, 0.0, -20.0), Ordering::Relaxed);
        self.routing.store(s.routing as u8, Ordering::Relaxed);
        self.mode
            .store(u8::from(s.mode == Mode::Add), Ordering::Relaxed);
        self.sweep_s
            .store(sane(s.sweep_s, 1.0, 120.0, 10.0), Ordering::Relaxed);
        self.on.store(s.on, Ordering::Relaxed);
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Switch off immediately (the audio thread fades out).
    pub fn force_off(&self) {
        self.on.store(false, Ordering::Relaxed);
    }
}

fn sane(v: f64, lo: f64, hi: f64, d: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        d
    }
}

const RAMP_S: f64 = 0.020;
const LEVEL_SMOOTH_S: f64 = 0.010;

/// Generator DSP (audio thread). All waveforms are constructed up front
/// (the pink-noise filter fit takes milliseconds), nothing allocates later.
pub struct GenEngine {
    fs: f64,
    sine: Sine,
    sweep: LogSweep,
    sweep_s: f64,
    white: WhiteNoise,
    pink: PinkNoise,
    kind: Kind,
    /// Envelope position 0..=1 (raised cosine applied on top).
    pos: f64,
    step: f64,
    gain: f64,
    gain_alpha: f64,
    target_on: bool,
}

impl GenEngine {
    pub fn new(fs: f64) -> Self {
        let s = GenSettings::default();
        Self {
            fs,
            sine: Sine::new(fs, s.freq_hz, 0.0),
            sweep: Self::make_sweep(fs, s.sweep_s),
            sweep_s: s.sweep_s,
            white: WhiteNoise::new(0.0, 0x5eed_0001),
            pink: PinkNoise::new(fs, 0.0, 0x5eed_0002),
            kind: Kind::Sine,
            pos: 0.0,
            step: 1.0 / (RAMP_S * fs).max(1.0),
            gain: db_to_amp(s.level_db),
            gain_alpha: 1.0 - (-1.0 / (LEVEL_SMOOTH_S * fs)).exp(),
            target_on: false,
        }
    }

    fn make_sweep(fs: f64, dur: f64) -> LogSweep {
        LogSweep::new(fs, 20.0, 20_000.0f64.min(0.45 * fs), dur, 0.0, true)
    }

    /// True while the generator is producing or fading (the output differs
    /// from the input).
    pub fn is_active(&self) -> bool {
        self.target_on || self.pos > 0.0
    }

    /// Render `g.len()` samples of generator output (envelope applied) and the
    /// envelope itself into `e`. Returns false (and leaves the buffers
    /// untouched) when the generator is idle.
    pub fn render(&mut self, s: &GenSettings, g: &mut [f64], e: &mut [f64]) -> bool {
        if s.kind != self.kind && self.pos <= 0.0 {
            self.kind = s.kind;
            self.sweep.restart();
        }
        let want_on = s.on && s.kind == self.kind;
        if want_on && !self.target_on && self.pos <= 0.0 {
            self.sweep.restart();
        }
        self.target_on = want_on;
        if !self.is_active() {
            self.gain = db_to_amp(s.level_db);
            return false;
        }
        let nyq_cap = 0.49 * self.fs;
        self.sine
            .set_frequency(self.fs, s.freq_hz.clamp(1.0, nyq_cap));
        if (s.sweep_s - self.sweep_s).abs() > 1e-9 {
            self.sweep_s = s.sweep_s;
            self.sweep = Self::make_sweep(self.fs, s.sweep_s);
        }
        let target = db_to_amp(s.level_db);
        let n = g.len().min(e.len());
        for i in 0..n {
            self.pos = if self.target_on {
                (self.pos + self.step).min(1.0)
            } else {
                (self.pos - self.step).max(0.0)
            };
            let env = 0.5 - 0.5 * (PI * self.pos).cos();
            self.gain += self.gain_alpha * (target - self.gain);
            let raw = match self.kind {
                Kind::Sine => self.sine.next_sample(),
                Kind::Sweep => self.sweep.next_sample(),
                Kind::White => self.white.next_sample(),
                Kind::Pink => self.pink.next_sample(),
            };
            g[i] = raw * self.gain * env;
            e[i] = env;
        }
        true
    }

    /// Apply the rendered generator signal to a block of one channel.
    /// `route` is the channel's routing gain (0, 1 or -1).
    #[inline]
    pub fn mix(mode: Mode, x: &mut [f32], g: &[f64], e: &[f64], route: f64) {
        for ((o, &gv), &ev) in x.iter_mut().zip(g).zip(e) {
            let v = f64::from(*o);
            *o = match mode {
                Mode::Replace => (v * (1.0 - ev) + gv * route) as f32,
                Mode::Add => (v + gv * route) as f32,
            };
        }
    }

    pub fn gains(r: Routing) -> (f64, f64) {
        r.gains()
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn control_is_off_by_default_and_force_off_works() {
        let c = GenControl::new();
        assert!(!c.is_on());
        let mut s = c.get();
        s.on = true;
        c.set(s);
        assert!(c.is_on());
        c.force_off();
        assert!(!c.get().on);
    }

    #[test]
    fn set_sanitizes() {
        let c = GenControl::new();
        let mut s = GenSettings::default();
        s.freq_hz = f64::NAN;
        s.level_db = 50.0;
        c.set(s);
        let g = c.get();
        assert_eq!(g.freq_hz, 1000.0);
        assert_eq!(g.level_db, 0.0);
    }

    #[test]
    fn idle_engine_does_nothing() {
        let mut g = GenEngine::new(48_000.0);
        let (mut a, mut b) = ([1.0; 16], [1.0; 16]);
        assert!(!g.render(&GenSettings::default(), &mut a, &mut b));
        assert_eq!(a, [1.0; 16]);
        assert!(!g.is_active());
    }

    #[test]
    fn envelope_reaches_one_and_returns_to_zero() {
        let mut g = GenEngine::new(48_000.0);
        let mut s = GenSettings::default();
        s.on = true;
        let (mut a, mut e) = (vec![0.0; 4800], vec![0.0; 4800]);
        assert!(g.render(&s, &mut a, &mut e));
        assert!((e[4799] - 1.0).abs() < 1e-9);
        assert!(e[0] < 0.01);
        s.on = false;
        g.render(&s, &mut a, &mut e);
        assert!(!g.is_active());
    }
}
