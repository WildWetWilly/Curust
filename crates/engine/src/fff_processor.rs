//! `FffProcessor.h/.cpp`: owner of the polygon generator and the G-code writer.

use crate::fff_gcode_writer::FffGcodeWriter;
use crate::fff_polygon_generator::FffPolygonGenerator;
use crate::settings::types::Duration;
use std::cell::UnsafeCell;
use std::sync::OnceLock;

pub struct FffProcessor {
    pub gcode_writer: FffGcodeWriter,
    pub polygon_generator: FffPolygonGenerator,
}

struct Instance(UnsafeCell<FffProcessor>);
// SAFETY: the processor is only used from the main thread.
unsafe impl Sync for Instance {}
unsafe impl Send for Instance {}

impl FffProcessor {
    /// `FffProcessor::getInstance()`.
    #[allow(clippy::mut_from_ref)]
    pub fn get_instance() -> &'static mut FffProcessor {
        static INSTANCE: OnceLock<Instance> = OnceLock::new();
        let i = INSTANCE.get_or_init(|| Instance(UnsafeCell::new(FffProcessor { gcode_writer: FffGcodeWriter::default(), polygon_generator: FffPolygonGenerator })));
        unsafe { &mut *i.0.get() }
    }

    pub fn set_target_file(&mut self, filename: &str) -> bool {
        self.gcode_writer.set_target_file(filename)
    }

    pub fn get_total_filament_used(&self, extruder_nr: usize) -> f64 {
        self.gcode_writer.get_total_filament_used(extruder_nr)
    }

    pub fn get_total_print_time_per_feature(&self) -> Vec<Duration> {
        self.gcode_writer.get_total_print_time_per_feature()
    }

    pub fn finalize(&mut self) {
        self.gcode_writer.finalize();
    }
}
