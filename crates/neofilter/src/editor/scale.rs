//! Pure display math: frequency/dB <-> pixel mapping, tick generation, node
//! hit-testing, drag conversion and band-slot helpers. No egui painting here,
//! so everything is unit-testable.

use crate::params::{FREQ_MAX, FREQ_MIN, GAIN_MAX_DB, Q_MAX, Q_MIN};
use neofilter_dsp::{BandParams, Shape};

/// Lowest displayed frequency.
pub const DISPLAY_F_MIN: f32 = 10.0;
/// Highest displayed frequency.
pub const DISPLAY_F_MAX: f32 = 30000.0;

/// Selectable dB ranges (symmetric, +-value).
pub const DB_RANGES: [f32; 3] = [6.0, 12.0, 30.0];

/// Mapping between plot-area pixels and (frequency, dB).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub x0: f32,
    pub y0: f32,
    pub width: f32,
    pub height: f32,
    /// dB at the top edge (bottom edge is the negative).
    pub db_range: f32,
}

impl View {
    pub fn freq_to_x(&self, f: f32) -> f32 {
        let t = (f / DISPLAY_F_MIN).ln() / (DISPLAY_F_MAX / DISPLAY_F_MIN).ln();
        self.x0 + t * self.width
    }

    pub fn x_to_freq(&self, x: f32) -> f32 {
        let t = (x - self.x0) / self.width;
        DISPLAY_F_MIN * (DISPLAY_F_MAX / DISPLAY_F_MIN).powf(t)
    }

    pub fn db_to_y(&self, db: f32) -> f32 {
        self.y0 + self.height * 0.5 * (1.0 - db / self.db_range)
    }

    pub fn y_to_db(&self, y: f32) -> f32 {
        (1.0 - (y - self.y0) / (self.height * 0.5)) * self.db_range
    }
}

/// Labelled frequency grid lines.
pub const FREQ_LABELS: [(f32, &str); 11] = [
    (10.0, "10"),
    (20.0, "20"),
    (50.0, "50"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1000.0, "1k"),
    (2000.0, "2k"),
    (5000.0, "5k"),
    (10000.0, "10k"),
    (20000.0, "20k"),
];

/// Unlabelled minor frequency grid lines (3,4,6,7,8,9 x each decade).
pub fn minor_freq_ticks() -> Vec<f32> {
    let mut v = Vec::new();
    let mut decade = 10.0f32;
    while decade < DISPLAY_F_MAX {
        for m in [3.0, 4.0, 6.0, 7.0, 8.0, 9.0] {
            let f = decade * m;
            if f <= DISPLAY_F_MAX {
                v.push(f);
            }
        }
        decade *= 10.0;
    }
    v
}

/// Step between dB grid lines for a given range.
pub fn db_tick_step(range: f32) -> f32 {
    if range <= 12.0 {
        3.0
    } else {
        10.0
    }
}

/// dB grid values from -range to +range inclusive (always contains 0).
pub fn db_ticks(range: f32) -> Vec<f32> {
    let step = db_tick_step(range);
    let n = (range / step).floor() as i32;
    (-n..=n).map(|k| k as f32 * step).collect()
}

/// Band colour (RGB), distinct for neighbouring indices.
pub fn band_rgb(index: usize) -> [u8; 3] {
    let hue = (index as f32 * 0.381_966).fract();
    let (s, v) = if index.is_multiple_of(2) {
        (0.65, 0.95)
    } else {
        (0.5, 1.0)
    };
    hsv_to_rgb(hue, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|u| ((u + m) * 255.0).round() as u8)
}

/// Whether the band's gain parameter affects its response.
pub fn shape_has_gain(shape: Shape) -> bool {
    !matches!(
        shape,
        Shape::LowCut | Shape::HighCut | Shape::Notch | Shape::BandPass
    )
}

/// Vertical node position in dB: gain (scaled by gain scale) or the 0 dB line.
pub fn node_db(shape: Shape, gain_db: f32, gain_scale: f32) -> f32 {
    if shape_has_gain(shape) {
        gain_db * gain_scale
    } else {
        0.0
    }
}

/// Inverse of the scaled node height: the gain parameter for a displayed dB value.
/// With a (near) zero gain scale the display carries no gain information, so `None`.
pub fn gain_from_node_db(db: f32, gain_scale: f32) -> Option<f32> {
    (gain_scale > 0.02).then(|| (db / gain_scale).clamp(-GAIN_MAX_DB, GAIN_MAX_DB))
}

/// A band node in pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub band: usize,
    pub x: f32,
    pub y: f32,
}

/// Build nodes for all used bands.
pub fn nodes(view: &View, bands: &[BandParams], gain_scale: f32) -> Vec<Node> {
    bands
        .iter()
        .enumerate()
        .filter(|(_, b)| b.used)
        .map(|(i, b)| Node {
            band: i,
            x: view.freq_to_x(b.freq_hz as f32),
            y: view.db_to_y(node_db(b.shape, b.gain_db as f32, gain_scale)),
        })
        .collect()
}

/// Nearest node within `radius` of `(px, py)`; ties go to the higher band
/// number (drawn on top).
pub fn hit_test(nodes: &[Node], px: f32, py: f32, radius: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for n in nodes {
        let d2 = (n.x - px).powi(2) + (n.y - py).powi(2);
        if d2 <= radius * radius && best.is_none_or(|(bd, _)| d2 <= bd) {
            best = Some((d2, n.band));
        }
    }
    best.map(|(_, b)| b)
}

/// First band slot not in use.
pub fn first_unused(bands: &[BandParams]) -> Option<usize> {
    bands.iter().position(|b| !b.used)
}

/// Shape for a band created by double-clicking at horizontal position `x_frac`
/// (0..1): far left a low cut, far right a high cut, otherwise a bell.
pub fn shape_for_position(x_frac: f32) -> Shape {
    if x_frac < 0.06 {
        Shape::LowCut
    } else if x_frac > 0.94 {
        Shape::HighCut
    } else {
        Shape::Bell
    }
}

/// Clamp a frequency to the parameter range.
pub fn clamp_freq(f: f32) -> f32 {
    f.clamp(FREQ_MIN, FREQ_MAX)
}

/// Clamp a Q to the parameter range.
pub fn clamp_q(q: f32) -> f32 {
    q.clamp(Q_MIN, Q_MAX)
}

/// Result of a drag conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragResult {
    pub freq: f32,
    /// New gain parameter, or `None` if the shape has no gain / scale is ~0.
    pub gain: Option<f32>,
}

/// Convert a node drag into new parameter values. `dx`/`dy` are the accumulated
/// pointer movement in pixels since the drag started (already scaled for fine
/// mode); `lock_x` / `lock_y` freeze one axis.
#[allow(clippy::too_many_arguments)]
pub fn drag_to(
    view: &View,
    shape: Shape,
    start_freq: f32,
    start_gain: f32,
    gain_scale: f32,
    dx: f32,
    dy: f32,
    lock_x: bool,
    lock_y: bool,
) -> DragResult {
    let freq = if lock_x {
        start_freq
    } else {
        clamp_freq(view.x_to_freq(view.freq_to_x(start_freq) + dx))
    };
    let gain = if shape_has_gain(shape) && !lock_y {
        let y = view.db_to_y(node_db(shape, start_gain, gain_scale)) + dy;
        gain_from_node_db(view.y_to_db(y), gain_scale)
    } else {
        None
    };
    DragResult { freq, gain }
}

/// New Q after `scroll` points of mouse wheel (positive = scroll up = narrower).
pub fn q_after_scroll(q: f32, scroll: f32) -> f32 {
    clamp_q(q * (scroll * 0.003).exp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use neofilter_dsp::types::NUM_BANDS;

    fn view() -> View {
        View {
            x0: 50.0,
            y0: 20.0,
            width: 800.0,
            height: 400.0,
            db_range: 12.0,
        }
    }

    #[test]
    fn freq_x_round_trip_and_edges() {
        let v = view();
        assert!((v.freq_to_x(10.0) - 50.0).abs() < 1e-3);
        assert!((v.freq_to_x(30000.0) - 850.0).abs() < 1e-2);
        for f in [10.0, 33.3, 100.0, 1000.0, 9729.0, 30000.0] {
            let r = v.x_to_freq(v.freq_to_x(f));
            assert!((r / f - 1.0).abs() < 1e-4, "{f} -> {r}");
        }
        // Equal octaves are equally wide.
        let a = v.freq_to_x(200.0) - v.freq_to_x(100.0);
        let b = v.freq_to_x(2000.0) - v.freq_to_x(1000.0);
        assert!((a - b).abs() < 1e-3);
    }

    #[test]
    fn db_y_round_trip() {
        let v = view();
        assert!((v.db_to_y(12.0) - 20.0).abs() < 1e-4);
        assert!((v.db_to_y(0.0) - 220.0).abs() < 1e-4);
        assert!((v.db_to_y(-12.0) - 420.0).abs() < 1e-4);
        for d in [-12.0, -3.3, 0.0, 7.7, 12.0] {
            assert!((v.y_to_db(v.db_to_y(d)) - d).abs() < 1e-4);
        }
    }

    #[test]
    fn ticks() {
        assert_eq!(
            db_ticks(30.0),
            vec![-30.0, -20.0, -10.0, 0.0, 10.0, 20.0, 30.0]
        );
        assert_eq!(
            db_ticks(12.0),
            vec![-12.0, -9.0, -6.0, -3.0, 0.0, 3.0, 6.0, 9.0, 12.0]
        );
        assert_eq!(db_ticks(6.0), vec![-6.0, -3.0, 0.0, 3.0, 6.0]);
        let m = minor_freq_ticks();
        assert!(m.contains(&30.0) && m.contains(&9000.0) && m.contains(&30000.0));
        assert!(m.iter().all(|f| (10.0..=30000.0).contains(f)));
        assert_eq!(FREQ_LABELS.len(), 11);
    }

    #[test]
    fn colors_distinct() {
        let c: Vec<_> = (0..NUM_BANDS).map(band_rgb).collect();
        let mut min = f32::MAX;
        for i in 0..NUM_BANDS {
            for j in i + 1..NUM_BANDS {
                assert_ne!(c[i], c[j]);
                let d: f32 = (0..3)
                    .map(|k| (f32::from(c[i][k]) - f32::from(c[j][k])).powi(2))
                    .sum::<f32>()
                    .sqrt();
                min = min.min(d);
            }
        }
        println!("min pairwise RGB distance over {NUM_BANDS} bands: {min}");
        assert!(min > 25.0, "palette too similar: {min}");
    }

    #[test]
    fn node_numbers_are_readable_on_every_colour() {
        // black text on the node disc: WCAG contrast ratio against black
        let lin = |u: u8| {
            let c = f32::from(u) / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let mut worst = f32::MAX;
        for i in 0..NUM_BANDS {
            let [r, g, b] = band_rgb(i);
            let l = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
            worst = worst.min((l + 0.05) / 0.05);
        }
        println!("worst black-on-node contrast: {worst}");
        // 4.5 is the WCAG body-text bar; the darkest hue (band 29) gives 4.27,
        // still well above the 3.0 bar for UI components
        assert!(worst >= 4.0, "contrast {worst}");
    }

    fn bands() -> [BandParams; NUM_BANDS] {
        let mut b = [BandParams::default(); NUM_BANDS];
        b[0].used = true;
        b[0].freq_hz = 1000.0;
        b[0].gain_db = 6.0;
        b[3].used = true;
        b[3].freq_hz = 1100.0;
        b[3].gain_db = 6.0;
        b[5].used = true;
        b[5].shape = Shape::LowCut;
        b[5].freq_hz = 40.0;
        b[5].gain_db = 20.0;
        b
    }

    #[test]
    fn node_positions_and_gainless_shapes() {
        let v = view();
        let b = bands();
        let n = nodes(&v, &b, 1.0);
        assert_eq!(n.len(), 3);
        assert!((n[0].y - v.db_to_y(6.0)).abs() < 1e-4);
        // Low cut sits on the 0 dB line whatever its stored gain.
        assert!((n[2].y - v.db_to_y(0.0)).abs() < 1e-4);
        // Gain scale moves nodes.
        let n2 = nodes(&v, &b, 0.5);
        assert!((n2[0].y - v.db_to_y(3.0)).abs() < 1e-4);
    }

    #[test]
    fn hit_nearest_wins_within_radius() {
        let v = view();
        let n = nodes(&v, &bands(), 1.0);
        let (x1, y1) = (n[0].x, n[0].y);
        assert_eq!(hit_test(&n, x1, y1, 10.0), Some(0));
        // Node 3 is ~ 14 px to the right; a point nearer to it picks it.
        assert_eq!(hit_test(&n, n[1].x - 1.0, n[1].y, 10.0), Some(3));
        assert_eq!(hit_test(&n, x1 + 200.0, y1 + 100.0, 10.0), None);
        assert_eq!(hit_test(&[], 0.0, 0.0, 10.0), None);
    }

    #[test]
    fn unused_slot_and_position_shape() {
        let mut b = bands();
        assert_eq!(first_unused(&b), Some(1));
        b.iter_mut().for_each(|x| x.used = true);
        assert_eq!(first_unused(&b), None);
        assert_eq!(shape_for_position(0.01), Shape::LowCut);
        assert_eq!(shape_for_position(0.5), Shape::Bell);
        assert_eq!(shape_for_position(0.99), Shape::HighCut);
    }

    #[test]
    fn drag_clamps_and_respects_shape() {
        let v = view();
        // Drag far right/up: clamps to param limits.
        let r = drag_to(
            &v,
            Shape::Bell,
            1000.0,
            0.0,
            1.0,
            10_000.0,
            -10_000.0,
            false,
            false,
        );
        assert_eq!(r.freq, FREQ_MAX);
        assert_eq!(r.gain, Some(GAIN_MAX_DB));
        let r = drag_to(
            &v,
            Shape::Bell,
            1000.0,
            0.0,
            1.0,
            -10_000.0,
            10_000.0,
            false,
            false,
        );
        assert_eq!(r.freq, FREQ_MIN);
        assert_eq!(r.gain, Some(-GAIN_MAX_DB));
        // Zero movement keeps values.
        let r = drag_to(&v, Shape::Bell, 1000.0, 4.0, 1.0, 0.0, 0.0, false, false);
        assert!((r.freq - 1000.0).abs() < 0.01);
        assert!((r.gain.unwrap() - 4.0).abs() < 1e-3);
        // Gain-less shape: no gain.
        let r = drag_to(
            &v,
            Shape::HighCut,
            1000.0,
            0.0,
            1.0,
            5.0,
            50.0,
            false,
            false,
        );
        assert_eq!(r.gain, None);
        // Axis lock.
        let r = drag_to(&v, Shape::Bell, 1000.0, 2.0, 1.0, 80.0, 20.0, true, false);
        assert_eq!(r.freq, 1000.0);
        let r = drag_to(&v, Shape::Bell, 1000.0, 2.0, 1.0, 80.0, 20.0, false, true);
        assert_eq!(r.gain, None);
        // 100 % scale: a drag of one dB-pixel-worth moves one dB.
        let px_per_db = v.height / 24.0;
        let r = drag_to(
            &v,
            Shape::Bell,
            1000.0,
            0.0,
            1.0,
            0.0,
            -px_per_db * 3.0,
            false,
            false,
        );
        assert!((r.gain.unwrap() - 3.0).abs() < 1e-3);
        // Scaled display: 50 % scale doubles the parameter change.
        let r = drag_to(
            &v,
            Shape::Bell,
            1000.0,
            0.0,
            0.5,
            0.0,
            -px_per_db * 3.0,
            false,
            false,
        );
        assert!((r.gain.unwrap() - 6.0).abs() < 1e-3);
        // Zero scale: no gain info.
        let r = drag_to(&v, Shape::Bell, 1000.0, 0.0, 0.0, 0.0, -5.0, false, false);
        assert_eq!(r.gain, None);
    }

    #[test]
    fn q_scroll_clamps() {
        assert!(q_after_scroll(1.0, 50.0) > 1.0);
        assert!(q_after_scroll(1.0, -50.0) < 1.0);
        assert_eq!(q_after_scroll(39.0, 5000.0), Q_MAX);
        assert_eq!(q_after_scroll(0.03, -5000.0), Q_MIN);
    }
}
