//! The 24-band EQ engine: the API the plugin codes against.
//!
//! Design notes
//! * Bands run in series. Every band owns two [`Voice`]s: the current one and
//!   the one that is fading out while a topology change (shape, slope,
//!   placement, used/enabled) is crossfaded over 10 ms.
//! * Smoothable parameters (frequency, gain, Q) glide with a 20 ms time
//!   constant; coefficients are redesigned every 32 samples while a band is
//!   moving. All state changes happen on a global 32-sample grid, so the output
//!   does not depend on how the host partitions its blocks.
//! * Redesigns are rationed: at most `TICK_SECTION_BUDGET` biquad sections are
//!   redesigned per 32-sample update (round robin, a postponed band catches up
//!   with a proportionally larger smoothing step), so loading a preset that moves
//!   all 24 bands cannot cause a CPU spike.
//! * The first `set_band` after `new` / `reset` / `set_sample_rate` snaps the
//!   band to its parameters without smoothing or fades.
//! * Nothing here allocates; all storage is fixed size.

use crate::analog::{effective_gain_db, shape_uses_gain};
use crate::band::BandFilter;
use crate::conventions::{bell_prototype_order, cut_shelf_order};
use crate::smoothing::{fade_len, update_coeff, BandSmoother, UPDATE_INTERVAL};
use crate::types::{
    BandParams, Placement, Shape, Slope, MAX_FREQ_HZ, MAX_Q, MIN_FREQ_HZ, MIN_Q, NUM_BANDS,
};

/// Redesign budget per update interval, in biquad sections (a section costs
/// roughly 2-5 microseconds to design).
const TICK_SECTION_BUDGET: usize = 16;

/// Everything that cannot be smoothed and is crossfaded instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Topology {
    active: bool,
    shape: Shape,
    slope: Slope,
    placement: Placement,
}

impl Topology {
    fn of(p: &BandParams) -> Self {
        if p.used && p.enabled {
            Self {
                active: true,
                shape: p.shape,
                slope: p.slope,
                placement: p.placement,
            }
        } else {
            Self::INACTIVE
        }
    }
    const INACTIVE: Self = Self {
        active: false,
        shape: Shape::Bell,
        slope: Slope::Db12,
        placement: Placement::Stereo,
    };
}

/// One realisation of a band: filter plus the topology it was built for.
#[derive(Debug, Clone, Copy)]
struct Voice {
    topo: Topology,
    filter: BandFilter,
}

impl Voice {
    fn new() -> Self {
        Self {
            topo: Topology::INACTIVE,
            filter: BandFilter::new(),
        }
    }

    fn processes(&self) -> bool {
        self.topo.active && !self.filter.is_identity()
    }

    fn process_stereo(&mut self, l: &mut [f64], r: &mut [f64]) {
        if !self.processes() {
            return;
        }
        match self.topo.placement {
            Placement::Stereo => {
                self.filter.process(0, l);
                self.filter.process(1, r);
            }
            Placement::Left => self.filter.process(0, l),
            Placement::Right => self.filter.process(1, r),
            Placement::Mid | Placement::Side => {
                let n = l.len().min(r.len()).min(UPDATE_INTERVAL);
                let mid = self.topo.placement == Placement::Mid;
                let mut x = [0.0; UPDATE_INTERVAL];
                let mut y = [0.0; UPDATE_INTERVAL];
                for i in 0..n {
                    x[i] = if mid {
                        0.5 * (l[i] + r[i])
                    } else {
                        0.5 * (l[i] - r[i])
                    };
                    y[i] = x[i];
                }
                self.filter.process(0, &mut y[..n]);
                // Apply only the change, so a flat filter reconstructs exactly.
                for i in 0..n {
                    let d = y[i] - x[i];
                    l[i] += d;
                    r[i] += if mid { d } else { -d };
                }
            }
        }
    }

    fn process_mono(&mut self, x: &mut [f64]) {
        if self.processes() {
            self.filter.process(0, x);
        }
    }
}

/// State of one band slot.
#[derive(Debug, Clone, Copy)]
struct Band {
    target: BandParams,
    smooth: BandSmoother,
    cur: Voice,
    old: Voice,
    fading: bool,
    fade_pos: usize,
    /// Smoothing in progress or target changed: needs attention at the next tick.
    dirty: bool,
    /// The next `set_band` must snap.
    needs_snap: bool,
    /// Update intervals this band has been postponed (rationing).
    postponed: u32,
}

impl Band {
    fn new() -> Self {
        let p = BandParams::default();
        Self {
            target: p,
            smooth: BandSmoother::new(p.freq_hz, 0.0, p.q),
            cur: Voice::new(),
            old: Voice::new(),
            fading: false,
            fade_pos: 0,
            dirty: false,
            needs_snap: true,
            postponed: 0,
        }
    }

    /// Rough number of sections a redesign of the target would produce.
    fn design_cost(&self) -> usize {
        let t = &self.target;
        match t.shape {
            Shape::FlatTilt => 8,
            Shape::Bell | Shape::Notch | Shape::BandPass => bell_prototype_order(t.slope),
            _ => cut_shelf_order(t.slope).div_ceil(2),
        }
    }

    fn idle(&self) -> bool {
        !self.fading && !self.cur.topo.active
    }

    /// Effective (frequency, gain dB, Q) targets including the gain scale.
    fn eff_targets(&self, gain_scale: f64) -> (f64, f64, f64) {
        let t = &self.target;
        (
            t.freq_hz,
            effective_gain_db(t.shape, t.gain_db, gain_scale),
            t.q,
        )
    }

    fn redesign_cur(&mut self, sample_rate: f64) {
        if !self.cur.topo.active {
            return;
        }
        let (f, g, q) = self.smooth.values();
        let p = BandParams {
            used: true,
            enabled: true,
            shape: self.cur.topo.shape,
            freq_hz: f,
            gain_db: g,
            q,
            slope: self.cur.topo.slope,
            placement: self.cur.topo.placement,
        };
        self.cur.filter.redesign(&p, 1.0, sample_rate);
    }

    /// Jump to the target without smoothing or fades.
    fn snap(&mut self, gain_scale: f64, sample_rate: f64) {
        let (f, g, q) = self.eff_targets(gain_scale);
        self.smooth.set_targets(f, g, q);
        self.smooth.snap();
        self.cur = Voice::new();
        self.cur.topo = Topology::of(&self.target);
        self.old = Voice::new();
        self.fading = false;
        self.fade_pos = 0;
        self.dirty = false;
        self.redesign_cur(sample_rate);
    }

    /// Per-update work: start fades for topology changes, step smoothing.
    fn tick(&mut self, gain_scale: f64, sample_rate: f64, coeff: f64) {
        if !self.dirty {
            return;
        }
        let want = Topology::of(&self.target);
        let (f, g, q) = self.eff_targets(gain_scale);
        if want != self.cur.topo && !self.fading {
            // Topology change: freeze the old voice, build the new one at the
            // final parameters and crossfade.
            self.old = self.cur;
            self.fading = true;
            self.fade_pos = 0;
            self.cur = Voice::new();
            self.cur.topo = want;
            self.smooth.set_targets(f, g, q);
            self.smooth.snap();
            self.redesign_cur(sample_rate);
            self.dirty = false;
            return;
        }
        self.smooth.set_targets(f, g, q);
        if self.smooth.step(coeff) {
            self.redesign_cur(sample_rate);
        }
        // Stay dirty while smoothing or while a topology change is pending
        // behind a running fade.
        self.dirty = !self.smooth.is_settled() || want != self.cur.topo;
    }

    fn process_stereo(&mut self, l: &mut [f64], r: &mut [f64], fade_len: usize) {
        if self.fading {
            let n = l.len();
            let mut ol = [0.0; UPDATE_INTERVAL];
            let mut or = [0.0; UPDATE_INTERVAL];
            ol[..n].copy_from_slice(l);
            or[..n].copy_from_slice(r);
            self.old.process_stereo(&mut ol[..n], &mut or[..n]);
            self.cur.process_stereo(l, r);
            let inv = 1.0 / fade_len as f64;
            for i in 0..n {
                let t = ((self.fade_pos + i) as f64 * inv).min(1.0);
                l[i] = ol[i] + t * (l[i] - ol[i]);
                r[i] = or[i] + t * (r[i] - or[i]);
            }
            self.advance_fade(n, fade_len);
        } else {
            self.cur.process_stereo(l, r);
        }
    }

    fn process_mono(&mut self, x: &mut [f64], fade_len: usize) {
        if self.fading {
            let n = x.len();
            let mut o = [0.0; UPDATE_INTERVAL];
            o[..n].copy_from_slice(x);
            self.old.process_mono(&mut o[..n]);
            self.cur.process_mono(x);
            let inv = 1.0 / fade_len as f64;
            for i in 0..n {
                let t = ((self.fade_pos + i) as f64 * inv).min(1.0);
                x[i] = o[i] + t * (x[i] - o[i]);
            }
            self.advance_fade(n, fade_len);
        } else {
            self.cur.process_mono(x);
        }
    }

    fn advance_fade(&mut self, n: usize, fade_len: usize) {
        self.fade_pos += n;
        if self.fade_pos >= fade_len {
            self.fading = false;
            self.fade_pos = 0;
            self.old = Voice::new();
        }
    }
}

/// 24-band equaliser.
#[derive(Debug, Clone)]
pub struct EqEngine {
    sample_rate: f64,
    gain_scale: f64,
    coeff: f64,
    fade_len: usize,
    until_tick: usize,
    rr: usize,
    bands: [Band; NUM_BANDS],
}

fn sanitize(p: &BandParams) -> BandParams {
    let mut p = *p;
    p.freq_hz = if p.freq_hz.is_finite() {
        p.freq_hz.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ)
    } else {
        1000.0
    };
    p.q = if p.q.is_finite() {
        p.q.clamp(MIN_Q, MAX_Q)
    } else {
        1.0
    };
    p.gain_db = if p.gain_db.is_finite() {
        p.gain_db
    } else {
        0.0
    };
    p
}

impl EqEngine {
    pub fn new(sample_rate: f64) -> Self {
        let sr = valid_rate(sample_rate);
        Self {
            sample_rate: sr,
            gain_scale: 1.0,
            coeff: update_coeff(sr),
            fade_len: fade_len(sr),
            until_tick: 0,
            rr: 0,
            bands: [Band::new(); NUM_BANDS],
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn gain_scale(&self) -> f64 {
        self.gain_scale
    }

    /// Change the sample rate. Clears all state; the next `set_band` per
    /// band snaps (bands keep their last parameters meanwhile).
    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        let sr = valid_rate(sample_rate);
        self.sample_rate = sr;
        self.coeff = update_coeff(sr);
        self.fade_len = fade_len(sr);
        self.reset();
    }

    /// Clear all filter state and cancel smoothing and fades. Bands keep their
    /// parameters and are redesigned at once; the next `set_band` snaps.
    pub fn reset(&mut self) {
        self.until_tick = 0;
        for b in &mut self.bands {
            b.snap(self.gain_scale, self.sample_rate);
            b.needs_snap = true;
        }
    }

    /// Set target params for one band (smoothed internally). Cheap when
    /// unchanged. Realtime-safe. Out-of-range indices are ignored.
    pub fn set_band(&mut self, index: usize, params: &BandParams) {
        let (gs, sr) = (self.gain_scale, self.sample_rate);
        let Some(b) = self.bands.get_mut(index) else {
            return;
        };
        let p = sanitize(params);
        if b.needs_snap {
            b.target = p;
            b.snap(gs, sr);
            b.needs_snap = false;
        } else if p != b.target {
            b.target = p;
            b.dirty = true;
        }
    }

    /// 0.0..=2.0, scales dB gain of bell/shelf/tilt bands.
    pub fn set_gain_scale(&mut self, scale: f64) {
        let s = if scale.is_finite() {
            scale.clamp(0.0, 2.0)
        } else {
            1.0
        };
        if s != self.gain_scale {
            self.gain_scale = s;
            for b in &mut self.bands {
                if shape_uses_gain(b.target.shape) {
                    b.dirty = true;
                }
            }
        }
    }

    /// Run the 32-sample housekeeping.
    fn tick(&mut self) {
        let (gs, sr, c) = (self.gain_scale, self.sample_rate, self.coeff);
        let mut budget = TICK_SECTION_BUDGET;
        let start = self.rr;
        self.rr = (self.rr + 1) % NUM_BANDS;
        for k in 0..NUM_BANDS {
            let b = &mut self.bands[(start + k) % NUM_BANDS];
            if !b.dirty {
                continue;
            }
            let cost = b.design_cost();
            if cost > budget && budget < TICK_SECTION_BUDGET {
                b.postponed += 1;
                continue;
            }
            budget = budget.saturating_sub(cost);
            // a band that had to wait catches up: 1 - (1 - c)^(1 + waited)
            let coeff = 1.0 - (1.0 - c).powi(1 + b.postponed as i32);
            b.postponed = 0;
            b.tick(gs, sr, coeff);
        }
    }

    /// Process a stereo block in place. Realtime-safe.
    pub fn process_stereo(&mut self, left: &mut [f64], right: &mut [f64]) {
        let total = left.len().min(right.len());
        let mut pos = 0;
        while pos < total {
            if self.until_tick == 0 {
                self.tick();
                self.until_tick = UPDATE_INTERVAL;
            }
            let n = (total - pos).min(self.until_tick);
            let (l, r) = (&mut left[pos..pos + n], &mut right[pos..pos + n]);
            for b in &mut self.bands {
                if !b.idle() {
                    b.process_stereo(l, r, self.fade_len);
                }
            }
            self.until_tick -= n;
            pos += n;
        }
    }

    /// Process a mono block in place (placement is ignored). Realtime-safe.
    pub fn process_mono(&mut self, samples: &mut [f64]) {
        let total = samples.len();
        let mut pos = 0;
        while pos < total {
            if self.until_tick == 0 {
                self.tick();
                self.until_tick = UPDATE_INTERVAL;
            }
            let n = (total - pos).min(self.until_tick);
            let x = &mut samples[pos..pos + n];
            for b in &mut self.bands {
                if !b.idle() {
                    b.process_mono(x, self.fade_len);
                }
            }
            self.until_tick -= n;
            pos += n;
        }
    }
}

fn valid_rate(sr: f64) -> f64 {
    if sr.is_finite() && sr >= 8000.0 {
        sr.min(768_000.0)
    } else {
        44100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_unused() {
        let mut e = EqEngine::new(48000.0);
        e.set_band(0, &BandParams::default());
        let mut l = [0.5, -0.25];
        let mut r = [0.1, 0.2];
        e.process_stereo(&mut l, &mut r);
        assert_eq!(l, [0.5, -0.25]);
        assert_eq!(r, [0.1, 0.2]);
    }

    #[test]
    fn empty_blocks_are_fine() {
        let mut e = EqEngine::new(48000.0);
        e.process_stereo(&mut [], &mut []);
        e.process_mono(&mut []);
    }
}
