//! Plugin metadata and parameter table.
use neometer::params::NeoMeterParams;
use neometer::NeoMeter;
use nih_plug::prelude::*;

#[test]
fn metadata() {
    assert_eq!(NeoMeter::NAME, "NeoMeter");
    assert_eq!(NeoMeter::VENDOR, "NeoFilter");
    assert_eq!(<NeoMeter as ClapPlugin>::CLAP_ID, "org.neofilter.neometer");
    let f = <NeoMeter as ClapPlugin>::CLAP_FEATURES;
    for want in [
        ClapFeature::AudioEffect,
        ClapFeature::Analyzer,
        ClapFeature::Stereo,
        ClapFeature::Mono,
    ] {
        assert!(f.contains(&want));
    }
    let v = <NeoMeter as Vst3Plugin>::VST3_SUBCATEGORIES;
    assert!(v.contains(&Vst3SubCategory::Fx) && v.contains(&Vst3SubCategory::Analyzer));
    // unique among this repo's plugins
    assert_ne!(
        <NeoMeter as Vst3Plugin>::VST3_CLASS_ID,
        *b"NeoFilterEQ00001"
    );
    assert_ne!(
        <NeoMeter as Vst3Plugin>::VST3_CLASS_ID,
        *b"NeoRIAA000000001"
    );
    assert_eq!(NeoMeter::AUDIO_IO_LAYOUTS.len(), 2);
    assert_eq!(
        NeoMeter::AUDIO_IO_LAYOUTS[0]
            .main_input_channels
            .unwrap()
            .get(),
        2
    );
    assert_eq!(
        NeoMeter::AUDIO_IO_LAYOUTS[1]
            .main_output_channels
            .unwrap()
            .get(),
        1
    );
}

#[test]
fn parameters_are_frozen_and_few() {
    let p = NeoMeterParams::default();
    let map = p.param_map();
    let ids: Vec<&str> = map.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, ["reference_level", "lf_crossover"]);
    assert_eq!(p.reference_level.value(), -18.0);
    assert_eq!(p.lf_crossover.value(), 300.0);
    // persisted, not automatable: GUI choices live under these keys
    let keys: Vec<String> = p.serialize_fields().keys().cloned().collect();
    assert!(keys.contains(&"meter-settings".to_string()));
    assert!(keys.contains(&"editor-state".to_string()));
}

#[test]
fn default_editor_size() {
    let s = neometer::params::default_editor_state();
    assert_eq!(s.size(), (980, 620));
}
