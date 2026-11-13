//! Parameter writes through `ParamSetter` gestures (host automation/undo aware).

use crate::params::{BandPlace, BandShape, BandSlope, NeoFilterParams};
use neofilter_dsp::BandParams;
use nih_plug::prelude::*;

/// One complete begin/set/end gesture.
pub fn put<P: Param>(setter: &ParamSetter, param: &P, value: P::Plain) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, value);
    setter.end_set_parameter(param);
}

/// Write every parameter of band `index` (0-based) from `p`.
pub fn write_band(setter: &ParamSetter, params: &NeoFilterParams, index: usize, p: &BandParams) {
    let set = &params.bands[index];
    put(setter, &set.used, p.used);
    put(setter, &set.on, p.enabled);
    put(setter, &set.shape, BandShape::from(p.shape));
    put(setter, &set.freq, p.freq_hz as f32);
    put(setter, &set.gain, p.gain_db as f32);
    put(setter, &set.q, p.q as f32);
    put(setter, &set.slope, BandSlope::from(p.slope));
    put(setter, &set.place, BandPlace::from(p.placement));
}

/// Remove a band: mark unused and reset it to defaults.
pub fn remove_band(setter: &ParamSetter, params: &NeoFilterParams, index: usize) {
    write_band(setter, params, index, &BandParams::default());
}
