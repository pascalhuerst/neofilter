use crate::params::NeoMeterParams;
use crate::shared::Shared;
use nih_plug::prelude::*;
use std::sync::Arc;
pub fn create(_p: Arc<NeoMeterParams>, _s: Arc<Shared>) -> Option<Box<dyn Editor>> {
    None
}
