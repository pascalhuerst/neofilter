//! Parameter ids and plugin metadata are frozen.
use neoriaa::params::{ModeParam, NeoRiaaParams};
use neoriaa::NeoRiaa;
use nih_plug::prelude::*;

#[test]
fn parameter_ids_defaults_and_ranges() {
    let p = NeoRiaaParams::default();
    let map = p.param_map();
    let ids: Vec<&str> = map.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, ["mode", "iec", "ultrasonic", "trim", "bypass"]);
    assert_eq!(p.mode.value(), ModeParam::PreEmphasis);
    assert!(!p.iec.value() && !p.ultrasonic.value() && !p.bypass.value());
    assert_eq!(p.trim.value(), 0.0);
    assert_eq!(p.trim.unmodulated_plain_value(), 0.0);
    assert_eq!(p.trim.preview_plain(0.0), -36.0);
    assert_eq!(p.trim.preview_plain(1.0), 12.0);
    assert_eq!(p.mode.preview_plain(0.0), ModeParam::PreEmphasis);
    assert_eq!(p.mode.preview_plain(1.0), ModeParam::DeEmphasis);
    assert_eq!(p.options(), neo_measure::riaa::Options::default());
}

#[test]
fn plugin_metadata() {
    assert_eq!(NeoRiaa::NAME, "NeoRIAA");
    assert_eq!(NeoRiaa::VENDOR, "NeoFilter");
    assert_eq!(<NeoRiaa as ClapPlugin>::CLAP_ID, "org.neofilter.neoriaa");
    assert_eq!(NeoRiaa::AUDIO_IO_LAYOUTS.len(), 2);
    let l = NeoRiaa::AUDIO_IO_LAYOUTS;
    assert_eq!(l[0].main_input_channels.unwrap().get(), 2);
    assert_eq!(l[0].main_output_channels.unwrap().get(), 2);
    assert_eq!(l[1].main_input_channels.unwrap().get(), 1);
    assert_eq!(l[1].main_output_channels.unwrap().get(), 1);
}
