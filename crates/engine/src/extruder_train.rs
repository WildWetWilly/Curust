//! `ExtruderTrain.h`.

use crate::settings::settings::Settings;

#[derive(Debug, Clone)]
pub struct ExtruderTrain {
    pub settings: Settings,
    pub extruder_nr: usize,
}

impl ExtruderTrain {
    pub fn new(extruder_nr: usize, parent_settings: *const Settings) -> Self {
        let mut settings = Settings::new();
        settings.set_parent(parent_settings);
        ExtruderTrain { settings, extruder_nr }
    }
}
