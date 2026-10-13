//! `settings/FlowTempGraph.h`: flow (mm3/s) to temperature mapping.

use super::types::Temperature;

#[derive(Clone, Debug, Default)]
pub struct FlowTempGraph {
    /// (flow, temperature) pairs.
    pub data: Vec<(f64, Temperature)>,
}

impl FlowTempGraph {
    pub fn get_temp(&self, flow: f64, material_print_temperature: Temperature, flow_dependent_temperature: bool) -> f64 {
        if !flow_dependent_temperature || self.data.is_empty() {
            return material_print_temperature;
        }
        if self.data.len() == 1 {
            return self.data[0].1;
        }
        if flow < self.data[0].0 {
            crate::log_warn!("Warning! Flow too low!");
            return self.data[0].1;
        }
        let mut last_datum = self.data[0];
        for datum_idx in 1..self.data.len() {
            let datum = self.data[datum_idx];
            if datum.0 >= flow {
                return last_datum.1 + (datum.1 - last_datum.1) * (flow - last_datum.0) / (datum.0 - last_datum.0);
            }
            last_datum = datum;
        }
        crate::log_warn!("Warning! Flow too high!");
        self.data.last().unwrap().1
    }
}
