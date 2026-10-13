//! `LayerPlanBuffer.h/.cpp` and `Preheat.h/.cpp`: a buffer of layer plans in which travel moves
//! between layers are connected and nozzle temperature commands are inserted, before the plans
//! are written to G-code.

use crate::application::{communication, current_slice};
use crate::extruder_plan::ExtruderPlan;
use crate::gcode_export::GCodeExport;
use crate::layer_plan::LayerPlan;
use crate::path_planning::NozzleTempInsert;
use crate::settings::enums::InsetDirection;
use crate::settings::settings::{Settings, MAX_EXTRUDERS};
use crate::settings::types::{Duration, LayerIndex, Temperature};
use std::collections::VecDeque;

/// `Preheat::WarmUpResult`
#[derive(Clone, Copy, Debug, Default)]
pub struct WarmUpResult {
    pub total_time_window: Duration,
    pub heating_time: Duration,
    pub lowest_temperature: Temperature,
}

/// `Preheat::CoolDownResult`
#[derive(Clone, Copy, Debug, Default)]
pub struct CoolDownResult {
    pub total_time_window: Duration,
    pub cooling_time: Duration,
    pub highest_temperature: Temperature,
}

fn extruder_settings(extruder: usize) -> &'static Settings {
    &current_slice().scene.extruders[extruder].settings
}

/// `Preheat`: nozzle temperature computations.
#[derive(Default)]
pub struct Preheat;

impl Preheat {
    pub fn get_time_to_go_from_temp_to_temp(&self, extruder: usize, temp_before: Temperature, temp_after: Temperature, during_printing: bool) -> Duration {
        let extruder_settings = extruder_settings(extruder);
        let time: Duration;
        if temp_after > temp_before {
            let mut heat_up_speed = extruder_settings.get_temperature("machine_nozzle_heat_up_speed");
            if during_printing {
                heat_up_speed -= extruder_settings.get_temperature("material_extrusion_cool_down_speed");
            }
            time = Duration::new((temp_after - temp_before) / heat_up_speed);
        } else {
            let mut cool_down_speed = extruder_settings.get_temperature("machine_nozzle_cool_down_speed");
            if during_printing {
                cool_down_speed += extruder_settings.get_temperature("material_extrusion_cool_down_speed");
            }
            time = Duration::new((temp_before - temp_after) / cool_down_speed);
        }
        if Duration::new(0.0) < time {
            time
        } else {
            Duration::new(0.0)
        }
    }

    pub fn get_temp(&self, extruder: usize, is_initial_layer: bool) -> Temperature {
        let extruder_settings = extruder_settings(extruder);
        if is_initial_layer && extruder_settings.get_temperature("material_print_temperature_layer_0") != 0.0 {
            return extruder_settings.get_temperature("material_print_temperature_layer_0");
        }
        extruder_settings.get_temperature("material_print_temperature")
    }

    pub fn get_warm_up_point_after_cool_down(&self, time_window: f64, extruder: usize, temp_start: f64, temp_mid: f64, temp_end: f64, during_printing: bool) -> WarmUpResult {
        let mut result = WarmUpResult::default();
        let extruder_settings = extruder_settings(extruder);
        let mut cool_down_speed = extruder_settings.get_temperature("machine_nozzle_cool_down_speed");
        if during_printing {
            cool_down_speed += extruder_settings.get_temperature("material_extrusion_cool_down_speed");
        }
        let time_to_cooldown_1_degree = Duration::new(1.0 / cool_down_speed).value();
        let mut heat_up_speed = extruder_settings.get_temperature("machine_nozzle_heat_up_speed");
        if during_printing {
            heat_up_speed -= extruder_settings.get_temperature("material_extrusion_cool_down_speed");
        }
        let time_to_heatup_1_degree = Duration::new(1.0 / heat_up_speed).value();
        result.total_time_window = Duration::new(time_window);

        let outer_temp;
        let limited_time_window;
        if temp_start < temp_end {
            // extra time needed during heating
            let extra_heatup_time = (temp_end - temp_start) * time_to_heatup_1_degree;
            result.heating_time = Duration::new(extra_heatup_time);
            limited_time_window = time_window - extra_heatup_time;
            outer_temp = temp_start;
        } else {
            let extra_cooldown_time = (temp_start - temp_end) * time_to_cooldown_1_degree;
            result.heating_time = Duration::new(0.0);
            limited_time_window = time_window - extra_cooldown_time;
            outer_temp = temp_end;
        }
        if limited_time_window < 0.0 {
            result.heating_time = Duration::new(0.0);
            result.lowest_temperature = if temp_end < temp_start { temp_end } else { temp_start };
            return result;
        }

        let time_ratio_cooldown_heatup = time_to_cooldown_1_degree / time_to_heatup_1_degree;
        let time_to_heat_from_standby_to_print_temp = self.get_time_to_go_from_temp_to_temp(extruder, temp_mid, outer_temp, during_printing).value();
        let time_needed_to_reach_standby_temp = time_to_heat_from_standby_to_print_temp * (1.0 + time_ratio_cooldown_heatup);
        if time_needed_to_reach_standby_temp < limited_time_window {
            result.heating_time += Duration::new(time_to_heat_from_standby_to_print_temp);
            result.lowest_temperature = temp_mid;
        } else {
            result.heating_time += Duration::new(limited_time_window * time_to_heatup_1_degree / (time_to_cooldown_1_degree + time_to_heatup_1_degree));
            let v = temp_end - result.heating_time.value() / time_to_heatup_1_degree;
            result.lowest_temperature = if temp_mid < v { v } else { temp_mid };
        }

        if result.heating_time.value() > time_window || result.heating_time.value() < 0.0 {
            log_warn!("getWarmUpPointAfterCoolDown returns result outside of the time window!");
        }
        result
    }

    pub fn get_cool_down_point_after_warm_up(&self, time_window: f64, extruder: usize, temp_start: f64, temp_mid: f64, temp_end: f64, during_printing: bool) -> CoolDownResult {
        let mut result = CoolDownResult::default();
        let extruder_settings = extruder_settings(extruder);
        let mut cool_down_speed = extruder_settings.get_temperature("machine_nozzle_cool_down_speed");
        if during_printing {
            cool_down_speed += extruder_settings.get_temperature("material_extrusion_cool_down_speed");
        }
        let time_to_cooldown_1_degree = Duration::new(1.0 / cool_down_speed).value();
        let mut heat_up_speed = extruder_settings.get_temperature("machine_nozzle_heat_up_speed");
        if during_printing {
            heat_up_speed -= extruder_settings.get_temperature("material_extrusion_cool_down_speed");
        }
        let time_to_heatup_1_degree = Duration::new(1.0 / heat_up_speed).value();

        result.total_time_window = Duration::new(time_window);

        let outer_temp;
        let limited_time_window;
        if temp_start < temp_end {
            // extra time needed during heating
            let extra_heatup_time = (temp_end - temp_start) * time_to_heatup_1_degree;
            result.cooling_time = Duration::new(0.0);
            limited_time_window = time_window - extra_heatup_time;
            outer_temp = temp_end;
        } else {
            let extra_cooldown_time = (temp_start - temp_end) * time_to_cooldown_1_degree;
            result.cooling_time = Duration::new(extra_cooldown_time);
            limited_time_window = time_window - extra_cooldown_time;
            outer_temp = temp_start;
        }
        if limited_time_window < 0.0 {
            result.cooling_time = Duration::new(0.0);
            result.highest_temperature = if temp_start < temp_end { temp_end } else { temp_start };
            return result;
        }
        let time_ratio_cooldown_heatup = time_to_cooldown_1_degree / time_to_heatup_1_degree;
        let cool_down_time = self.get_time_to_go_from_temp_to_temp(extruder, temp_mid, outer_temp, during_printing).value();
        let time_needed_to_reach_temp1 = cool_down_time * (1.0 + time_ratio_cooldown_heatup);
        if time_needed_to_reach_temp1 < limited_time_window {
            result.cooling_time += Duration::new(cool_down_time);
            result.highest_temperature = temp_mid;
        } else {
            result.cooling_time += Duration::new(limited_time_window * time_to_heatup_1_degree / (time_to_cooldown_1_degree + time_to_heatup_1_degree));
            let v = temp_end + result.cooling_time.value() / time_to_cooldown_1_degree;
            result.highest_temperature = if v < temp_mid { v } else { temp_mid };
        }

        if result.cooling_time.value() > time_window || result.cooling_time.value() < 0.0 {
            log_warn!("getCoolDownPointAfterWarmUp returns result outside of the time window!");
        }
        result
    }
}

/// should be as low as possible while still allowing enough time in the buffer to heat up from standby temp to printing temp
const BUFFER_SIZE: usize = 5;
/// Time to start heating earlier than computed to avoid accummulative discrepancy between actual heating times and computed ones.
const EXTRA_PREHEAT_TIME: f64 = 1.0;

/// A reference to an extruder plan in the buffer: (index of the layer plan in the buffer, index of the extruder plan).
type PlanRef = (usize, usize);

pub struct LayerPlanBuffer<'a> {
    preheat_config: Preheat,
    /// For each extruder whether it has already been planned once in this meshgroup.
    extruder_used_in_meshgroup: Vec<bool>,
    buffer: VecDeque<Box<LayerPlan<'a>>>,
}

impl<'a> Default for LayerPlanBuffer<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> LayerPlanBuffer<'a> {
    pub fn new() -> Self {
        LayerPlanBuffer { preheat_config: Preheat, extruder_used_in_meshgroup: vec![false; MAX_EXTRUDERS], buffer: VecDeque::new() }
    }

    /// The C++ buffer is a member of FffGcodeWriter and keeps `extruder_used_in_meshgroup` across mesh groups;
    /// here the buffer is local to `write_gcode`, so this state is handed in and out explicitly.
    pub fn with_extruder_used_in_meshgroup(extruder_used_in_meshgroup: Vec<bool>) -> Self {
        let mut buffer = Self::new();
        if extruder_used_in_meshgroup.len() == MAX_EXTRUDERS {
            buffer.extruder_used_in_meshgroup = extruder_used_in_meshgroup;
        }
        buffer
    }

    pub fn into_extruder_used_in_meshgroup(self) -> Vec<bool> {
        self.extruder_used_in_meshgroup
    }

    /// Place a new layer in the buffer and write the oldest layer to G-code if the buffer is full.
    pub fn handle(&mut self, layer_plan: Box<LayerPlan<'a>>, gcode: &mut GCodeExport) {
        self.buffer.push_back(layer_plan);

        if let Some(mut to_be_written) = self.process_buffer(gcode) {
            to_be_written.write_gcode(gcode);
        }
    }

    fn process_buffer(&mut self, gcode: &mut GCodeExport) -> Option<Box<LayerPlan<'a>>> {
        if self.buffer.is_empty() {
            return None;
        }
        self.process_fan_speed_layer_time();
        if self.buffer.len() >= 2 {
            let n = self.buffer.len();
            self.add_connecting_travel_move(n - 2, n - 1);
        }
        if !self.buffer.is_empty() {
            self.insert_temp_commands(gcode); // insert preheat commands of the just completed layer plan (not the newly emplaced one)
        }
        if self.buffer.len() > BUFFER_SIZE {
            communication().flush_gcode();
            return self.buffer.pop_front();
        }
        None
    }

    /// Write all remaining layer plans (of the last meshgroup) to g-code.
    pub fn flush(&mut self, gcode: &mut GCodeExport) {
        communication().flush_gcode(); // If there was still g-code in a layer, flush that as a separate layer.
        if !self.buffer.is_empty() {
            self.insert_temp_commands(gcode); // insert preheat commands of the very last layer
        }
        while let Some(mut front) = self.buffer.pop_front() {
            front.write_gcode(gcode);
            communication().flush_gcode();
        }
    }

    /// Get the layer plan of the given layer number (if it is still in the buffer).
    pub fn get_completed_layer_plan(&self, layer_nr: LayerIndex) -> Option<&LayerPlan<'a>> {
        self.buffer.iter().find(|p| p.get_layer_nr() == layer_nr).map(|b| &**b)
    }

    fn add_connecting_travel_move(&mut self, prev_idx: usize, newest_idx: usize) {
        let (new_layer_destination_state, newest_z, first_printed_mesh, newest_layer_nr, newest_first_acc_jerk) = {
            let newest_layer = &self.buffer[newest_idx];
            (
                newest_layer.get_first_travel_destination_state(),
                newest_layer.z,
                newest_layer.find_first_printed_mesh(),
                newest_layer.get_layer_nr(),
                newest_layer.first_extrusion_acc_jerk,
            )
        };

        let (first_location_new_layer, is_inside) = match new_layer_destination_state {
            Some(s) => s,
            None => {
                log_warn!("Layer {} is empty (or it has empty extruder plans). Temperature control and cross layer travel moves might suffer!", newest_layer_nr);
                return;
            }
        };

        let storage = self.buffer[prev_idx].storage();
        let prev_layer = &mut self.buffer[prev_idx];
        // if the last planned position in the previous layer isn't the same as the first location of the new layer, travel to the new location
        if prev_layer.last_planned_position.is_none() || prev_layer.last_planned_position.unwrap().to_point2ll() != first_location_new_layer {
            let mesh_group_settings = &current_slice().scene.current_mesh_group().settings;
            let extruder_settings = extruder_settings(prev_layer.extruder_plans.last().unwrap().extruder_nr);
            prev_layer.set_is_inside(is_inside);

            let travel_retract_before_outer_wall = mesh_group_settings.get_bool("travel_retract_before_outer_wall");
            let retract_at_layer_change = extruder_settings.get_bool("retract_at_layer_change");
            let mut next_mesh_retract_before_outer_wall = false;
            if !retract_at_layer_change && travel_retract_before_outer_wall {
                if let Some(mesh_idx) = first_printed_mesh {
                    // Check whether we are moving toving towards an outer wall and it should be retracted
                    let mesh_settings = storage.meshes[mesh_idx].settings();
                    let inset_direction: InsetDirection = mesh_settings.get_enum("inset_direction");
                    let wall_line_count = mesh_settings.get_usize("wall_line_count");

                    next_mesh_retract_before_outer_wall = inset_direction == InsetDirection::OUTSIDE_IN || wall_line_count == 1;
                }
            }
            let force_retract = retract_at_layer_change || next_mesh_retract_before_outer_wall;
            prev_layer.final_travel_z = newest_z;
            let path = prev_layer.add_travel(first_location_new_layer, force_retract, 0);
            let path = prev_layer.path_mut(path);
            if force_retract && !path.retract {
                // addTravel() won't use retraction if the travel distance is less than retraction minimum travel setting
                // so to avoid blobs when moving to the new layer height, which can occur if the z-axis speed is very slow,
                // we force the path to use retraction
                path.retract = true;
            }
        }

        // If not using travel-specific jerk and acceleration, the layer plan needs to know the jerk/acc of the first extrusion move of the next layer.
        prev_layer.next_layer_acc_jerk = newest_first_acc_jerk;
    }

    fn process_fan_speed_layer_time(&mut self) {
        let n = self.buffer.len();
        // Assume the print head is homed at the start of a mesh group.
        let mut starting_position = crate::geometry::point2ll::pt(0, 0);
        if n >= 2 {
            starting_position = self.buffer[n - 2].get_last_planned_position_or_starting_position();
        }
        self.buffer[n - 1].process_fan_speed_and_minimal_layer_time(starting_position);
    }

    fn plan_mut(&mut self, r: PlanRef) -> &mut ExtruderPlan {
        &mut self.buffer[r.0].extruder_plans[r.1]
    }

    fn plan(&self, r: PlanRef) -> &ExtruderPlan {
        &self.buffer[r.0].extruder_plans[r.1]
    }

    fn insert_preheat_command(extruder_plan_before: &mut ExtruderPlan, time_before_extruder_plan_end: f64, extruder_nr: usize, temp: Temperature) {
        let mut acc_time = Duration::new(0.0);
        let mut path_idx: u32 = (extruder_plan_before.paths.len() as u32).wrapping_sub(1);
        while path_idx as i32 != -1 {
            let path = &extruder_plan_before.paths[path_idx as usize];
            acc_time += Duration::new(path.estimates.get_total_time());
            if acc_time.value() >= time_before_extruder_plan_end {
                let wait = false;
                extruder_plan_before.insert_command(NozzleTempInsert {
                    path_idx: path_idx as usize,
                    extruder: extruder_nr,
                    temperature: temp,
                    wait,
                    time_after_path_start: acc_time.value() - time_before_extruder_plan_end,
                });
                return;
            }
            path_idx = path_idx.wrapping_sub(1);
        }
        let wait = false;
        let path_idx = 0;
        extruder_plan_before.insert_command(NozzleTempInsert { path_idx, extruder: extruder_nr, temperature: temp, wait, time_after_path_start: 0.0 }); // insert at start of extruder plan if time_after_extruder_plan_start > extruder_plan.time
    }

    fn compute_standby_temp_plan(&self, extruder_plans: &[PlanRef], extruder_plan_idx: usize) -> WarmUpResult {
        let extruder_plan = self.plan(extruder_plans[extruder_plan_idx]);
        let extruder = extruder_plan.extruder_nr;
        let extruder_settings = extruder_settings(extruder);
        let initial_print_temp = extruder_plan.required_start_temperature;

        let mut in_between_time = Duration::new(0.0); // the duration during which the extruder isn't used
        let mut extruder_plan_before_idx = extruder_plan_idx.wrapping_sub(1);
        while extruder_plan_before_idx as i32 >= 0 {
            // find a previous extruder plan where the same extruder is used to see what time this extruder wasn't used
            let extruder_plan_before = self.plan(extruder_plans[extruder_plan_before_idx]);
            if extruder_plan_before.extruder_nr == extruder {
                let mut temp_before = extruder_settings.get_temperature("material_final_print_temperature");
                if temp_before == 0.0 {
                    temp_before = extruder_plan_before.extrusion_temperature.unwrap_or(initial_print_temp);
                }
                let during_printing = false;
                let mut warm_up = self.preheat_config.get_warm_up_point_after_cool_down(
                    in_between_time.value(),
                    extruder,
                    temp_before,
                    extruder_settings.get_temperature("material_standby_temperature"),
                    initial_print_temp,
                    during_printing,
                );
                let candidate = warm_up.heating_time + Duration::new(EXTRA_PREHEAT_TIME);
                warm_up.heating_time = if candidate < in_between_time { candidate } else { in_between_time };
                return warm_up;
            }
            in_between_time += Duration::new(extruder_plan_before.estimates.get_total_time());
            extruder_plan_before_idx = extruder_plan_before_idx.wrapping_sub(1);
        }
        // The last extruder plan with the same extruder falls outside of the buffer
        // assume the nozzle has cooled down to strandby temperature already.
        let mut warm_up = WarmUpResult { total_time_window: in_between_time, ..Default::default() };
        warm_up.lowest_temperature = extruder_settings.get_temperature("material_standby_temperature");
        let during_printing = false;
        warm_up.heating_time = self.preheat_config.get_time_to_go_from_temp_to_temp(extruder, warm_up.lowest_temperature, initial_print_temp, during_printing);
        if warm_up.heating_time > in_between_time {
            warm_up.heating_time = in_between_time;
            warm_up.lowest_temperature = initial_print_temp - in_between_time.value() * extruder_settings.get_temperature("machine_nozzle_heat_up_speed");
        }
        warm_up.heating_time = warm_up.heating_time + Duration::new(EXTRA_PREHEAT_TIME);
        warm_up
    }

    fn insert_preheat_command_single_extrusion(&mut self, prev_extruder_plan: PlanRef, extruder_nr: usize, required_temp: Temperature) {
        if !extruder_settings(extruder_nr).get_bool("machine_nozzle_temp_enabled") {
            return;
        }
        // time_before_extruder_plan_end is halved, so that at the layer change the temperature will be half way betewen the two requested temperatures
        let during_printing = true;
        let plan = self.plan(prev_extruder_plan);
        let prev_extrusion_temp = plan.extrusion_temperature.unwrap_or(plan.required_start_temperature);
        let mut time_before_extruder_plan_end = 0.5 * self.preheat_config.get_time_to_go_from_temp_to_temp(extruder_nr, prev_extrusion_temp, required_temp, during_printing).value();
        let total = plan.estimates.get_total_time();
        time_before_extruder_plan_end = if time_before_extruder_plan_end < total { time_before_extruder_plan_end } else { total };

        Self::insert_preheat_command(self.plan_mut(prev_extruder_plan), time_before_extruder_plan_end, extruder_nr, required_temp);
    }

    fn handle_standby_temp(&mut self, extruder_plans: &[PlanRef], extruder_plan_idx: usize, standby_temp: f64) {
        let extruder = self.plan(extruder_plans[extruder_plan_idx]).extruder_nr;
        let mut extruder_plan_before_idx = extruder_plan_idx.wrapping_sub(2);
        while extruder_plan_before_idx as i32 >= 0 {
            if self.plan(extruder_plans[extruder_plan_before_idx]).extruder_nr == extruder {
                self.plan_mut(extruder_plans[extruder_plan_before_idx + 1]).prev_extruder_standby_temp = Some(standby_temp);
                return;
            }
            extruder_plan_before_idx = extruder_plan_before_idx.wrapping_sub(1);
        }
        log_warn!("Couldn't find previous extruder plan so as to set the standby temperature. Inserting temp command in earliest available layer.");
        let wait = false;
        self.plan_mut(extruder_plans[0]).insert_command(NozzleTempInsert { path_idx: 0, extruder, temperature: standby_temp, wait, time_after_path_start: 0.0 });
    }

    fn insert_preheat_command_multi_extrusion(&mut self, extruder_plans: &[PlanRef], extruder_plan_idx: usize) {
        let extruder = self.plan(extruder_plans[extruder_plan_idx]).extruder_nr;
        let extruder_settings = extruder_settings(extruder);
        if !extruder_settings.get_bool("machine_nozzle_temp_enabled") {
            return;
        }
        let initial_print_temp = self.plan(extruder_plans[extruder_plan_idx]).required_start_temperature;

        let heating_time_and_from_temp = self.compute_standby_temp_plan(extruder_plans, extruder_plan_idx);

        if heating_time_and_from_temp.total_time_window < extruder_settings.get_duration("machine_min_cool_heat_time_window") {
            self.handle_standby_temp(extruder_plans, extruder_plan_idx, initial_print_temp);
            return; // don't insert preheat command and just stay on printing temperature
        } else if heating_time_and_from_temp.heating_time < heating_time_and_from_temp.total_time_window {
            // only insert command to cool down to standby temperature if there is some time to cool before heating up again
            self.handle_standby_temp(extruder_plans, extruder_plan_idx, heating_time_and_from_temp.lowest_temperature);
        }

        // handle preheat command
        let mut time_before_extruder_plan_to_insert = heating_time_and_from_temp.heating_time.value();
        let mut extruder_plan_before_idx: u32 = (extruder_plan_idx as u32).wrapping_sub(1);
        while extruder_plan_before_idx as i32 >= 0 {
            let r = extruder_plans[extruder_plan_before_idx as usize];
            let time_here = self.plan(r).estimates.get_total_time();
            if time_here >= time_before_extruder_plan_to_insert {
                Self::insert_preheat_command(self.plan_mut(r), time_before_extruder_plan_to_insert, extruder, initial_print_temp);
                return;
            }
            time_before_extruder_plan_to_insert -= time_here;
            extruder_plan_before_idx = extruder_plan_before_idx.wrapping_sub(1);
        }

        // time_before_extruder_plan_to_insert falls before all plans in the buffer
        let wait = false;
        let path_idx = 0;
        self.plan_mut(extruder_plans[0]).insert_command(NozzleTempInsert { path_idx, extruder, temperature: initial_print_temp, wait, time_after_path_start: 0.0 }); // insert preheat command at verfy beginning of buffer
    }

    fn insert_temp_commands_for(&mut self, extruder_plans: &[PlanRef], extruder_plan_idx: usize) {
        let extruder_plan_ref = extruder_plans[extruder_plan_idx];
        let extruder = self.plan(extruder_plan_ref).extruder_nr;

        let prev_extruder_plan = extruder_plans[extruder_plan_idx - 1];

        let prev_extruder = self.plan(prev_extruder_plan).extruder_nr;

        if prev_extruder != extruder {
            // set previous extruder to standby temperature
            let previous_extruder_settings = extruder_settings(prev_extruder);
            self.plan_mut(extruder_plan_ref).prev_extruder_standby_temp = Some(previous_extruder_settings.get_temperature("material_standby_temperature"));
        }

        if prev_extruder == extruder {
            let required = self.plan(extruder_plan_ref).required_start_temperature;
            self.insert_preheat_command_single_extrusion(prev_extruder_plan, extruder, required);
            let last_id = self.plan(prev_extruder_plan).last_insert_id();
            self.plan_mut(prev_extruder_plan).extrusion_temperature_command = last_id;
        } else if extruder_settings(extruder).get_bool("machine_extruders_share_heater") {
            // extruders share a heater so command the previous extruder to change to the temperature required for this extruder
            let required = self.plan(extruder_plan_ref).required_start_temperature;
            self.insert_preheat_command_single_extrusion(prev_extruder_plan, prev_extruder, required);
        } else {
            self.insert_preheat_command_multi_extrusion(extruder_plans, extruder_plan_idx);
            self.insert_final_print_temp_command(extruder_plans, extruder_plan_idx - 1);
            self.insert_print_temp_command(extruder_plan_ref);
        }
    }

    fn insert_print_temp_command(&mut self, extruder_plan_ref: PlanRef) {
        let extruder_plan = self.plan_mut(extruder_plan_ref);
        let print_temp = match extruder_plan.extrusion_temperature {
            Some(t) => t,
            None => {
                log_warn!("Empty extruder plan detected! Discarding extrusion temperature command.");
                return;
            }
        };

        let extruder = extruder_plan.extruder_nr;
        let extruder_settings = extruder_settings(extruder);
        if !extruder_settings.get_bool("machine_nozzle_temp_enabled") {
            return;
        }

        let mut heated_pre_travel_time = 0.0;
        if extruder_settings.get_temperature("material_initial_print_temperature") != 0.0 {
            // handle heating from initial_print_temperature to printing_tempreature
            let mut path_idx = 0usize;
            while path_idx < extruder_plan.paths.len() {
                let path = &extruder_plan.paths[path_idx];
                heated_pre_travel_time += path.estimates.get_total_time();
                if !path.is_travel_path() {
                    break;
                }
                path_idx += 1;
            }
            let wait = false;
            extruder_plan.insert_command(NozzleTempInsert { path_idx, extruder, temperature: print_temp, wait, time_after_path_start: 0.0 });
        }
        extruder_plan.heated_pre_travel_time = heated_pre_travel_time;
    }

    fn insert_final_print_temp_command(&mut self, extruder_plans: &[PlanRef], last_extruder_plan_idx: usize) {
        let last_extruder_plan_ref = extruder_plans[last_extruder_plan_idx];
        let extruder = self.plan(last_extruder_plan_ref).extruder_nr;
        let extruder_settings = extruder_settings(extruder);
        if !extruder_settings.get_bool("machine_nozzle_temp_enabled") {
            return;
        }

        let final_print_temp = extruder_settings.get_temperature("material_final_print_temperature");
        if final_print_temp == 0.0 {
            return;
        }

        let mut heated_post_travel_time = 0.0; // The time after the last extrude move toward the end of the extruder plan during which the nozzle is stable at the final print temperature
        {
            // compute heated_post_travel_time
            let last_extruder_plan = self.plan(last_extruder_plan_ref);
            let mut path_idx: u32 = (last_extruder_plan.paths.len() as u32).wrapping_sub(1);
            while path_idx as i32 >= 0 {
                let path = &last_extruder_plan.paths[path_idx as usize];
                if !path.is_travel_path() {
                    break;
                }
                heated_post_travel_time += path.estimates.get_total_time();
                path_idx = path_idx.wrapping_sub(1);
            }
        }

        let mut time_window = 0.0; // The time window within which the nozzle needs to heat from the initial print temp to the printing temperature and then back to the final print temp
        let mut weighted_average_extrusion_temp = 0.0; // The average of the normal extrusion temperatures of the extruder plans, weighted by time
        let mut initial_print_temp: Option<f64> = None; // The initial print temp of the first extruder plan with this extruder
        {
            // compute time window and print temp statistics
            let mut heated_pre_travel_time = -1.0; // The time before the first extrude move from the start of the extruder plan during which the nozzle is stable at the initial print temperature
            let mut prev_extruder_plan_idx: u32 = last_extruder_plan_idx as u32;
            while prev_extruder_plan_idx as i32 >= 0 {
                let prev_extruder_plan = self.plan(extruder_plans[prev_extruder_plan_idx as usize]);
                if prev_extruder_plan.extruder_nr != extruder {
                    break;
                }
                let prev_extruder_plan_time = prev_extruder_plan.estimates.get_total_time();
                time_window += prev_extruder_plan_time;
                heated_pre_travel_time = prev_extruder_plan.heated_pre_travel_time;

                if prev_extruder_plan.estimates.get_total_unretracted_time() > 0.0 {
                    // handle temp statistics
                    weighted_average_extrusion_temp += prev_extruder_plan.extrusion_temperature.unwrap_or(prev_extruder_plan.required_start_temperature) * prev_extruder_plan_time;
                    initial_print_temp = Some(prev_extruder_plan.required_start_temperature);
                }
                prev_extruder_plan_idx = prev_extruder_plan_idx.wrapping_sub(1);
            }
            if time_window <= 0.0 {
                // There was a move in this plan but it was length 0.
                log_warn!("Unnecessary extruder switch detected! SliceDataStorage::getExtrudersUsed should probably be updated.");
                return;
            }
            weighted_average_extrusion_temp /= time_window;
            time_window -= heated_pre_travel_time + heated_post_travel_time;
        }

        let initial_print_temp = match initial_print_temp {
            Some(t) => t,
            None => {
                // none of the extruder plans had unretracted moves
                log_warn!("Unnecessary extruder switch detected! Discarding final print temperature commands.");
                return;
            }
        };

        let during_printing = true;
        let warm_cool_result =
            self.preheat_config.get_cool_down_point_after_warm_up(time_window, extruder, initial_print_temp, weighted_average_extrusion_temp, final_print_temp, during_printing);
        let mut cool_down_time = warm_cool_result.cooling_time.value();

        // find extruder plan in which to insert cooling command
        let mut precool_extruder_plan = last_extruder_plan_ref;
        {
            let mut precool_extruder_plan_idx: u32 = last_extruder_plan_idx as u32;
            while precool_extruder_plan_idx as i32 >= 0 {
                precool_extruder_plan = extruder_plans[precool_extruder_plan_idx as usize];
                if let Some(cmd) = self.plan(precool_extruder_plan).extrusion_temperature_command {
                    // the precool command ends up before the command to go to the print temperature of the next extruder plan, so remove that print temp command
                    self.plan_mut(precool_extruder_plan).erase_insert(cmd);
                }
                let time_here = self.plan(precool_extruder_plan).estimates.get_total_time();
                if cool_down_time < time_here {
                    break;
                }
                cool_down_time -= time_here;
                precool_extruder_plan_idx = precool_extruder_plan_idx.wrapping_sub(1);
            }
        }

        // at this point cool_down_time is what time is left if cool down time of extruder plans after precool_extruder_plan (up until last_extruder_plan) are already taken into account

        {
            // insert temp command in precool_extruder_plan
            let plan = self.plan_mut(precool_extruder_plan);
            let mut extrusion_time_seen = 0.0;
            let mut path_idx: u32 = (plan.paths.len() as u32).wrapping_sub(1);
            while path_idx as i32 >= 0 {
                let path = &plan.paths[path_idx as usize];
                extrusion_time_seen += path.estimates.get_total_time();
                if extrusion_time_seen >= cool_down_time {
                    break;
                }
                path_idx = path_idx.wrapping_sub(1);
            }
            let wait = false;
            let time_after_path_start = extrusion_time_seen - cool_down_time;
            plan.insert_command(NozzleTempInsert { path_idx: path_idx as usize, extruder, temperature: final_print_temp, wait, time_after_path_start });
        }
    }

    fn insert_temp_commands(&mut self, gcode: &mut GCodeExport) {
        {
            let back = self.buffer.back().unwrap();
            if back.extruder_plans.is_empty() || (back.extruder_plans.len() == 1 && back.extruder_plans[0].paths.is_empty()) {
                // disregard empty layer
                self.buffer.pop_back();
                return;
            }
        }

        let mut extruder_plans: Vec<PlanRef> = Vec::with_capacity(self.buffer.len() * 2); // sorted in print order
        for (layer_idx, layer_plan) in self.buffer.iter().enumerate() {
            for plan_idx in 0..layer_plan.extruder_plans.len() {
                extruder_plans.push((layer_idx, plan_idx));
            }
        }

        // insert commands for all extruder plans on this layer
        let scene = &current_slice().scene;
        let back_idx = self.buffer.len() - 1;
        let n_plans = self.buffer[back_idx].extruder_plans.len();
        for extruder_plan_idx in 0..n_plans {
            let overall_extruder_plan_idx = extruder_plans.len() - n_plans + extruder_plan_idx;
            let r = (back_idx, extruder_plan_idx);
            let extruder = self.plan(r).extruder_nr;
            let extruder_settings = extruder_settings(extruder);

            let (is_initial_layer, temperature_factor) = {
                let p = self.plan(r);
                (p.is_initial_layer, p.temperature_factor)
            };
            let mut print_temp = self.preheat_config.get_temp(extruder, is_initial_layer);
            let mut initial_print_temp = extruder_settings.get_temperature("material_initial_print_temperature");

            if temperature_factor > 0.0 {
                // force lower printing temperatures due to minimum layer time
                print_temp = print_temp * (1.0 - temperature_factor) + temperature_factor * extruder_settings.get_temperature("cool_min_temperature");
                initial_print_temp = if print_temp < initial_print_temp { print_temp } else { initial_print_temp };
            }

            let prev_same_and_heated = overall_extruder_plan_idx > 0 && {
                let prev = self.plan(extruder_plans[overall_extruder_plan_idx - 1]);
                prev.extruder_nr == extruder && prev.estimates.get_total_unretracted_time() > 0.0
            };
            if initial_print_temp == 0.0 // user doesn't want to use initial print temp feature
                || extruder_settings.get_bool("machine_extruders_share_heater") // ignore initial print temps when extruders share a heater
                || (!self.extruder_used_in_meshgroup[extruder] && extruder_settings.get_bool("prime_blob_enable")) // prime blob uses print temp rather than initial print temp
                || prev_same_and_heated
            {
                self.plan_mut(r).required_start_temperature = print_temp;
                self.extruder_used_in_meshgroup[extruder] = true;
            } else {
                let p = self.plan_mut(r);
                p.required_start_temperature = initial_print_temp;
                p.extrusion_temperature = Some(print_temp);
            }

            if self.buffer.len() == 1 && extruder_plan_idx == 0 {
                // the very first extruder plan of the current meshgroup
                for extruder_idx in 0..scene.extruders.len() {
                    // set temperature of the first nozzle, turn other nozzles down
                    let other_extruder_settings = &scene.extruders[extruder_idx].settings;
                    if scene.current_mesh_group == 0 {
                        // First mesh group.
                        // override values from GCodeExport::setInitialTemps
                        // the first used extruder should be set to the required temp in the start gcode
                        // see  FffGcodeWriter::processStartingCode
                        if extruder_idx == extruder {
                            let p = self.plan(r);
                            gcode.set_initial_temp(extruder_idx, p.extrusion_temperature.unwrap_or(p.required_start_temperature));
                        } else {
                            gcode.set_initial_temp(extruder_idx, other_extruder_settings.get_temperature("material_standby_temperature"));
                        }
                    } else if extruder_idx != extruder {
                        // TODO: do we need to do this?
                        self.plan_mut(r).prev_extruder_standby_temp = Some(other_extruder_settings.get_temperature("material_standby_temperature"));
                    }
                }
                continue;
            }

            self.insert_temp_commands_for(&extruder_plans, overall_extruder_plan_idx);
        }
    }
}
