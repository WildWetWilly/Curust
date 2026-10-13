//! `settings/PathConfigStorage.h/.cpp` and `settings/MeshPathConfigs.h/.cpp`: all configurations for
//! all features types of printed lines in a meshgroup.

use crate::application::current_slice;
use crate::path_planning::{GCodePathConfig, SpeedDerivatives};
use crate::print_feature::PrintFeatureType;
use crate::settings::enums::EPlatformAdhesion;
use crate::settings::settings::MAX_INFILL_COMBINE;
use crate::settings::types::{LayerIndex, Ratio};
use crate::slice_data_storage::{SliceDataStorage, SliceMeshStorage};
use crate::utils::coord::Coord;

/// `MeshPathConfigs`
#[derive(Clone, Debug)]
pub struct MeshPathConfigs {
    pub inset0_config: GCodePathConfig,
    pub inset_x_config: GCodePathConfig,
    pub inset0_roofing_config: GCodePathConfig,
    pub inset_x_roofing_config: GCodePathConfig,
    pub inset0_flooring_config: GCodePathConfig,
    pub inset_x_flooring_config: GCodePathConfig,
    pub bridge_inset0_config: GCodePathConfig,
    pub bridge_inset_x_config: GCodePathConfig,
    pub skin_config: GCodePathConfig,
    /// used for first bridge layer
    pub bridge_skin_config: GCodePathConfig,
    /// used for second bridge layer
    pub bridge_skin_config2: GCodePathConfig,
    /// used for third bridge layer
    pub bridge_skin_config3: GCodePathConfig,
    pub roofing_config: GCodePathConfig,
    pub flooring_config: GCodePathConfig,
    pub infill_config: Vec<GCodePathConfig>,
    pub ironing_config: GCodePathConfig,
    pub skin_support_config: GCodePathConfig,
}

#[inline]
fn lw(width: Coord, factor: Ratio) -> Coord {
    (width as f64 * factor) as Coord
}

impl MeshPathConfigs {
    pub fn new(mesh: &SliceMeshStorage, layer_thickness: Coord, layer_nr: LayerIndex, line_width_factor_per_extruder: &[Ratio]) -> Self {
        let s = mesh.settings();
        let f0 = line_width_factor_per_extruder[s.get_extruder_train("wall_0_extruder_nr").extruder_nr];
        let fx = line_width_factor_per_extruder[s.get_extruder_train("wall_x_extruder_nr").extruder_nr];
        let ftb = line_width_factor_per_extruder[s.get_extruder_train("top_bottom_extruder_nr").extruder_nr];
        let finf = line_width_factor_per_extruder[s.get_extruder_train("infill_extruder_nr").extruder_nr];
        let first = |key: &str| if layer_nr == 0 { s.get_ratio(key) } else { 1.0 };
        let sd = |speed: &str, acc: &str, jerk: &str| SpeedDerivatives::new(s.get_velocity(speed), s.get_acceleration(acc), s.get_velocity(jerk));

        let inset0_config = GCodePathConfig::new(
            PrintFeatureType::OuterWall,
            lw(s.get_coord("wall_line_width_0"), f0),
            layer_thickness,
            s.get_ratio("wall_0_material_flow") * first("wall_0_material_flow_layer_0"),
            sd("speed_wall_0", "acceleration_wall_0", "jerk_wall_0"),
        );
        let inset_x_config = GCodePathConfig::new(
            PrintFeatureType::InnerWall,
            lw(s.get_coord("wall_line_width_x"), fx),
            layer_thickness,
            s.get_ratio("wall_x_material_flow") * first("wall_x_material_flow_layer_0"),
            sd("speed_wall_x", "acceleration_wall_x", "jerk_wall_x"),
        );
        let inset0_roofing_config = GCodePathConfig::new(
            PrintFeatureType::OuterWall,
            lw(s.get_coord("wall_line_width_0"), f0),
            layer_thickness,
            s.get_ratio("wall_0_material_flow_roofing") * first("wall_0_material_flow_layer_0"),
            sd("speed_wall_0_roofing", "acceleration_wall_0_roofing", "jerk_wall_0_roofing"),
        );
        let inset_x_roofing_config = GCodePathConfig::new(
            PrintFeatureType::InnerWall,
            lw(s.get_coord("wall_line_width_x"), fx),
            layer_thickness,
            s.get_ratio("wall_x_material_flow_roofing") * first("wall_x_material_flow_layer_0"),
            sd("speed_wall_x_roofing", "acceleration_wall_x_roofing", "jerk_wall_x_roofing"),
        );
        let inset0_flooring_config = GCodePathConfig::new(
            PrintFeatureType::OuterWall,
            lw(s.get_coord("wall_line_width_0"), f0),
            layer_thickness,
            s.get_ratio("wall_0_material_flow_flooring") * first("wall_0_material_flow_layer_0"),
            sd("speed_wall_0_flooring", "acceleration_wall_0_flooring", "jerk_wall_0_flooring"),
        );
        let inset_x_flooring_config = GCodePathConfig::new(
            PrintFeatureType::InnerWall,
            lw(s.get_coord("wall_line_width_x"), fx),
            layer_thickness,
            s.get_ratio("wall_x_material_flow_flooring") * first("wall_x_material_flow_layer_0"),
            sd("speed_wall_x_flooring", "acceleration_wall_x_flooring", "jerk_wall_x_flooring"),
        );
        let bridge_inset0_config = GCodePathConfig::new_full(
            PrintFeatureType::OuterWall,
            lw(s.get_coord("wall_line_width_0"), f0),
            layer_thickness,
            s.get_ratio("bridge_wall_material_flow"),
            sd("bridge_wall_speed", "acceleration_wall_0", "jerk_wall_0"),
            true,
            s.get_ratio("bridge_fan_speed") * 100.0,
        );
        let bridge_inset_x_config = GCodePathConfig::new_full(
            PrintFeatureType::InnerWall,
            lw(s.get_coord("wall_line_width_x"), fx),
            layer_thickness,
            s.get_ratio("bridge_wall_material_flow"),
            sd("bridge_wall_speed", "acceleration_wall_x", "jerk_wall_x"),
            true,
            s.get_ratio("bridge_fan_speed") * 100.0,
        );
        let skin_config = GCodePathConfig::new(
            PrintFeatureType::Skin,
            lw(s.get_coord("skin_line_width"), ftb),
            layer_thickness,
            s.get_ratio("skin_material_flow") * first("skin_material_flow_layer_0"),
            sd("speed_topbottom", "acceleration_topbottom", "jerk_topbottom"),
        );
        let bridge_skin_config = GCodePathConfig::new_full(
            PrintFeatureType::Skin,
            lw(s.get_coord("skin_line_width"), ftb),
            layer_thickness,
            s.get_ratio("bridge_skin_material_flow"),
            sd("bridge_skin_speed", "acceleration_topbottom", "jerk_topbottom"),
            true,
            s.get_ratio("bridge_fan_speed") * 100.0,
        );
        let bridge_skin_config2 = GCodePathConfig::new_full(
            PrintFeatureType::Skin,
            lw(s.get_coord("skin_line_width"), ftb),
            layer_thickness,
            s.get_ratio("bridge_skin_material_flow_2"),
            sd("bridge_skin_speed_2", "acceleration_topbottom", "jerk_topbottom"),
            true,
            s.get_ratio("bridge_fan_speed_2") * 100.0,
        );
        let bridge_skin_config3 = GCodePathConfig::new_full(
            PrintFeatureType::Skin,
            lw(s.get_coord("skin_line_width"), ftb),
            layer_thickness,
            s.get_ratio("bridge_skin_material_flow_3"),
            sd("bridge_skin_speed_3", "acceleration_topbottom", "jerk_topbottom"),
            true,
            s.get_ratio("bridge_fan_speed_3") * 100.0,
        );
        let roofing_config = GCodePathConfig::new(
            PrintFeatureType::Skin,
            s.get_coord("roofing_line_width"),
            layer_thickness,
            s.get_ratio("roofing_material_flow") * first("material_flow_layer_0"),
            sd("speed_roofing", "acceleration_roofing", "jerk_roofing"),
        );
        let flooring_config = GCodePathConfig::new(
            PrintFeatureType::Skin,
            s.get_coord("flooring_line_width"),
            layer_thickness,
            s.get_ratio("flooring_material_flow") * first("material_flow_layer_0"),
            sd("speed_flooring", "acceleration_flooring", "jerk_flooring"),
        );
        let ironing_config = GCodePathConfig::new(
            PrintFeatureType::Skin,
            s.get_coord("ironing_line_spacing"),
            layer_thickness,
            s.get_ratio("ironing_flow"),
            sd("speed_ironing", "acceleration_ironing", "jerk_ironing"),
        );
        let skin_support_config = GCodePathConfig::new(
            PrintFeatureType::Infill,
            s.get_coord("infill_line_width"),
            layer_thickness,
            s.get_ratio("skin_support_material_flow"),
            sd("skin_support_speed", "acceleration_infill", "jerk_infill"),
        );

        let mut infill_config = Vec::with_capacity(MAX_INFILL_COMBINE);
        for combine_idx in 1..=MAX_INFILL_COMBINE {
            infill_config.push(GCodePathConfig::new(
                PrintFeatureType::Infill,
                lw(s.get_coord("infill_line_width"), finf),
                layer_thickness,
                s.get_ratio("infill_material_flow") * first("material_flow_layer_0") * combine_idx as f64,
                sd("speed_infill", "acceleration_infill", "jerk_infill"),
            ));
        }

        MeshPathConfigs {
            inset0_config,
            inset_x_config,
            inset0_roofing_config,
            inset_x_roofing_config,
            inset0_flooring_config,
            inset_x_flooring_config,
            bridge_inset0_config,
            bridge_inset_x_config,
            skin_config,
            bridge_skin_config,
            bridge_skin_config2,
            bridge_skin_config3,
            roofing_config,
            flooring_config,
            infill_config,
            ironing_config,
            skin_support_config,
        }
    }

    pub fn smooth_all_speeds(&mut self, first_layer_config: &SpeedDerivatives, layer_nr: LayerIndex, max_speed_layer: LayerIndex) {
        self.inset0_config.speed_derivatives.smooth_speed(first_layer_config, layer_nr, max_speed_layer);
        self.inset_x_config.speed_derivatives.smooth_speed(first_layer_config, layer_nr, max_speed_layer);
        self.skin_config.speed_derivatives.smooth_speed(first_layer_config, layer_nr, max_speed_layer);
        self.ironing_config.speed_derivatives.smooth_speed(first_layer_config, layer_nr, max_speed_layer);
        for idx in 0..MAX_INFILL_COMBINE {
            // Infill speed (per combine part per mesh).
            self.infill_config[idx].speed_derivatives.smooth_speed(first_layer_config, layer_nr, max_speed_layer);
        }
    }
}

/// `PathConfigStorage`
#[derive(Clone, Debug)]
pub struct PathConfigStorage {
    support_infill_extruder_nr: usize,
    support_roof_extruder_nr: usize,
    support_bottom_extruder_nr: usize,
    line_width_factor_per_extruder: Vec<Ratio>,

    pub raft_base_config: GCodePathConfig,
    pub raft_interface_config: GCodePathConfig,
    pub raft_surface_config: GCodePathConfig,

    /// The config used for travel moves (only speed is set!)
    pub travel_config_per_extruder: Vec<GCodePathConfig>,
    /// Configuration for skirt and brim per extruder.
    pub skirt_brim_config_per_extruder: Vec<GCodePathConfig>,
    /// Configuration for the prime tower per extruder.
    pub prime_tower_config_per_extruder: Vec<GCodePathConfig>,

    /// The config used to print the normal support, rather than the support interface
    pub support_infill_config: Vec<GCodePathConfig>,
    /// The config used to print the normal support on fractional layer-height parts.
    pub support_fractional_infill_config: Vec<GCodePathConfig>,
    /// The config used to print the dense roofs of support.
    pub support_roof_config: GCodePathConfig,
    /// The config used to print the dense roofs of support on fractional layer-height parts.
    pub support_fractional_roof_config: GCodePathConfig,
    /// The config to use to print the dense bottoms of support
    pub support_bottom_config: GCodePathConfig,

    /// For each mesh the config for all its feature types
    pub mesh_configs: Vec<MeshPathConfigs>,
}

impl PathConfigStorage {
    fn get_line_width_factor_per_extruder(layer_nr: LayerIndex) -> Vec<Ratio> {
        let mut ret = Vec::new();
        for train in &current_slice().scene.extruders {
            if layer_nr <= 0 {
                let factor = train.settings.get_ratio("initial_layer_line_width_factor");
                ret.push(factor);
            } else {
                ret.push(1.0);
            }
        }
        ret
    }

    /// Note that the layer_nr might be below zero for raft (filler) layers
    pub fn new(storage: &SliceDataStorage, layer_nr: LayerIndex, layer_thickness: Coord) -> Self {
        let scene = &current_slice().scene;
        let mesh_group_settings = &scene.current_mesh_group().settings;
        let support_infill_extruder_nr = mesh_group_settings.get_extruder_train("support_infill_extruder_nr").extruder_nr;
        let support_roof_extruder_nr = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr;
        let support_bottom_extruder_nr = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr;
        let raft_base_train = &mesh_group_settings.get_extruder_train("raft_base_extruder_nr").settings;
        let raft_interface_train = &mesh_group_settings.get_extruder_train("raft_interface_extruder_nr").settings;
        let raft_surface_train = &mesh_group_settings.get_extruder_train("raft_surface_extruder_nr").settings;
        let support_infill_train = &scene.extruders[support_infill_extruder_nr].settings;
        let support_roof_train = &scene.extruders[support_roof_extruder_nr].settings;
        let support_bottom_train = &scene.extruders[support_bottom_extruder_nr].settings;
        let line_width_factor_per_extruder = Self::get_line_width_factor_per_extruder(layer_nr);

        let raft_base_config = GCodePathConfig::new(
            PrintFeatureType::SupportInterface,
            raft_base_train.get_coord("raft_base_line_width"),
            raft_base_train.get_coord("raft_base_thickness"),
            raft_base_train.get_ratio("raft_base_flow"),
            SpeedDerivatives::new(
                raft_base_train.get_velocity("raft_base_speed"),
                raft_base_train.get_acceleration("raft_base_acceleration"),
                raft_base_train.get_velocity("raft_base_jerk"),
            ),
        );
        let raft_interface_config = GCodePathConfig::new(
            PrintFeatureType::Support,
            raft_interface_train.get_coord("raft_interface_line_width"),
            raft_interface_train.get_coord("raft_interface_thickness"),
            raft_base_train.get_ratio("raft_interface_flow"),
            SpeedDerivatives::new(
                raft_interface_train.get_velocity("raft_interface_speed"),
                raft_interface_train.get_acceleration("raft_interface_acceleration"),
                raft_interface_train.get_velocity("raft_interface_jerk"),
            ),
        );
        let raft_surface_config = GCodePathConfig::new(
            PrintFeatureType::SupportInterface,
            raft_surface_train.get_coord("raft_surface_line_width"),
            raft_surface_train.get_coord("raft_surface_thickness"),
            raft_base_train.get_ratio("raft_surface_flow"),
            SpeedDerivatives::new(
                raft_surface_train.get_velocity("raft_surface_speed"),
                raft_surface_train.get_acceleration("raft_surface_acceleration"),
                raft_surface_train.get_velocity("raft_surface_jerk"),
            ),
        );
        let support_roof_config = GCodePathConfig::new(
            PrintFeatureType::SupportInterface,
            lw(support_roof_train.get_coord("support_roof_line_width"), line_width_factor_per_extruder[support_roof_extruder_nr]),
            layer_thickness,
            support_roof_train.get_ratio("support_roof_material_flow") * if layer_nr == 0 { support_roof_train.get_ratio("material_flow_layer_0") } else { 1.0 },
            SpeedDerivatives::new(
                support_roof_train.get_velocity("speed_support_roof"),
                support_roof_train.get_acceleration("acceleration_support_roof"),
                support_roof_train.get_velocity("jerk_support_roof"),
            ),
        );
        let support_bottom_config = GCodePathConfig::new(
            PrintFeatureType::SupportInterface,
            lw(support_bottom_train.get_coord("support_bottom_line_width"), line_width_factor_per_extruder[support_bottom_extruder_nr]),
            layer_thickness,
            support_roof_train.get_ratio("support_bottom_material_flow") * if layer_nr == 0 { support_roof_train.get_ratio("material_flow_layer_0") } else { 1.0 },
            SpeedDerivatives::new(
                support_bottom_train.get_velocity("speed_support_bottom"),
                support_bottom_train.get_acceleration("acceleration_support_bottom"),
                support_bottom_train.get_velocity("jerk_support_bottom"),
            ),
        );

        let extruder_count = scene.extruders.len();
        let mut travel_config_per_extruder = Vec::with_capacity(extruder_count);
        let mut skirt_brim_config_per_extruder = Vec::with_capacity(extruder_count);
        let mut prime_tower_config_per_extruder = Vec::with_capacity(extruder_count);
        let is_raft = mesh_group_settings.get_enum::<EPlatformAdhesion>("adhesion_type") == EPlatformAdhesion::RAFT;
        for extruder_nr in 0..extruder_count {
            let train = &scene.extruders[extruder_nr].settings;
            travel_config_per_extruder.push(GCodePathConfig::new(
                PrintFeatureType::MoveUnretracted,
                0,
                0,
                0.0,
                SpeedDerivatives::new(train.get_velocity("speed_travel"), train.get_acceleration("acceleration_travel"), train.get_velocity("jerk_travel")),
            ));
            skirt_brim_config_per_extruder.push(GCodePathConfig::new(
                PrintFeatureType::SkirtBrim,
                lw(train.get_coord("skirt_brim_line_width"), if is_raft { 1.0 } else { line_width_factor_per_extruder[extruder_nr] }), // cause it's also used for the draft/ooze shield
                layer_thickness,
                train.get_ratio("skirt_brim_material_flow") * if layer_nr == 0 { train.get_ratio("material_flow_layer_0") } else { 1.0 },
                SpeedDerivatives::new(train.get_velocity("skirt_brim_speed"), train.get_acceleration("acceleration_skirt_brim"), train.get_velocity("jerk_skirt_brim")),
            ));
            prime_tower_config_per_extruder.push(GCodePathConfig::new(
                PrintFeatureType::PrimeTower,
                lw(train.get_coord("prime_tower_line_width"), if is_raft { 1.0 } else { line_width_factor_per_extruder[extruder_nr] }),
                layer_thickness,
                train.get_ratio("prime_tower_flow") * if layer_nr == 0 { train.get_ratio("material_flow_layer_0") } else { 1.0 },
                SpeedDerivatives::new(train.get_velocity("speed_prime_tower"), train.get_acceleration("acceleration_prime_tower"), train.get_velocity("jerk_prime_tower")),
            ));
        }

        let mut mesh_configs = Vec::with_capacity(storage.meshes.len());
        for mesh_storage in &storage.meshes {
            mesh_configs.push(MeshPathConfigs::new(mesh_storage, layer_thickness, layer_nr, &line_width_factor_per_extruder));
        }

        let mut support_infill_config = Vec::with_capacity(MAX_INFILL_COMBINE);
        let support_infill_line_width_factor: f64 = if is_raft { 1.0 } else { line_width_factor_per_extruder[support_infill_extruder_nr] };
        for combine_idx in 0..MAX_INFILL_COMBINE {
            support_infill_config.push(GCodePathConfig::new(
                PrintFeatureType::Support,
                (support_infill_train.get_coord("support_line_width") as f64 * support_infill_line_width_factor) as Coord,
                layer_thickness,
                support_infill_train.get_ratio("support_material_flow")
                    * (if layer_nr == 0 { support_infill_train.get_ratio("material_flow_layer_0") } else { 1.0 })
                    * (combine_idx + 1) as f64,
                SpeedDerivatives::new(
                    support_infill_train.get_velocity("speed_support_infill"),
                    support_infill_train.get_acceleration("acceleration_support_infill"),
                    support_infill_train.get_velocity("jerk_support_infill"),
                ),
            ));
        }

        let mut ret = PathConfigStorage {
            support_infill_extruder_nr,
            support_roof_extruder_nr,
            support_bottom_extruder_nr,
            line_width_factor_per_extruder,
            raft_base_config,
            raft_interface_config,
            raft_surface_config,
            travel_config_per_extruder,
            skirt_brim_config_per_extruder,
            prime_tower_config_per_extruder,
            support_infill_config,
            support_fractional_infill_config: Vec::new(),
            support_roof_config,
            support_fractional_roof_config: GCodePathConfig::default(),
            support_bottom_config,
            mesh_configs,
        };

        let initial_speedup_layer_count = mesh_group_settings.get_usize("speed_slowdown_layers");
        if layer_nr >= 0 && (layer_nr as usize) < initial_speedup_layer_count {
            ret.handle_initial_layer_speedup(storage, layer_nr, initial_speedup_layer_count);
        }

        let layer_height = mesh_group_settings.get_coord("layer_height");
        let support_top_distance = mesh_group_settings.get_coord("support_top_distance");
        let leftover_support_distance = support_top_distance % layer_height;

        ret.support_fractional_infill_config = ret.support_infill_config.clone(); // copy
        for config in ret.support_fractional_infill_config.iter_mut() {
            config.z_offset = -leftover_support_distance;
        }

        ret.support_fractional_roof_config = ret.support_roof_config; // copy
        ret.support_fractional_roof_config.z_offset = -leftover_support_distance;
        ret
    }

    fn handle_initial_layer_speedup(&mut self, storage: &SliceDataStorage, layer_nr: LayerIndex, initial_speedup_layer_count: usize) {
        let scene = &current_slice().scene;
        let speedup = initial_speedup_layer_count as LayerIndex;
        let mut global_first_layer_config_per_extruder: Vec<SpeedDerivatives> = Vec::with_capacity(scene.extruders.len());
        for extruder in &scene.extruders {
            global_first_layer_config_per_extruder.push(SpeedDerivatives::new(
                extruder.settings.get_velocity("speed_print_layer_0"),
                extruder.settings.get_acceleration("acceleration_print_layer_0"),
                extruder.settings.get_velocity("jerk_print_layer_0"),
            ));
        }

        {
            // support
            if layer_nr < speedup {
                let mesh_group_settings = &scene.current_mesh_group().settings;
                let extruder_nr_support_infill = mesh_group_settings
                    .get_extruder_train(if layer_nr <= 0 { "support_extruder_nr_layer_0" } else { "support_infill_extruder_nr" })
                    .extruder_nr;
                for idx in 0..MAX_INFILL_COMBINE {
                    self.support_infill_config[idx].speed_derivatives.smooth_speed(
                        &global_first_layer_config_per_extruder[extruder_nr_support_infill],
                        layer_nr.max(0),
                        speedup,
                    );
                }
                let extruder_nr_support_roof = mesh_group_settings.get_extruder_train("support_roof_extruder_nr").extruder_nr;
                self.support_roof_config.speed_derivatives.smooth_speed(&global_first_layer_config_per_extruder[extruder_nr_support_roof], layer_nr.max(0), speedup);
                let extruder_nr_support_bottom = mesh_group_settings.get_extruder_train("support_bottom_extruder_nr").extruder_nr;
                self.support_bottom_config.speed_derivatives.smooth_speed(&global_first_layer_config_per_extruder[extruder_nr_support_bottom], layer_nr.max(0), speedup);
            }
        }

        {
            // extruder configs: travel, skirt/brim (= shield)
            for extruder_nr in 0..scene.extruders.len() {
                let train = &scene.extruders[extruder_nr].settings;
                let initial_layer_travel_speed_config = SpeedDerivatives::new(
                    train.get_velocity("speed_travel_layer_0"),
                    train.get_acceleration("acceleration_travel_layer_0"),
                    train.get_velocity("jerk_travel_layer_0"),
                );
                let travel = &mut self.travel_config_per_extruder[extruder_nr];

                travel.speed_derivatives.smooth_speed(&initial_layer_travel_speed_config, layer_nr.max(0), speedup);

                // don't smooth speed for the skirt/brim!
                // NOTE: not smoothing skirt/brim means the speeds are also not smoothed for the draft/ooze shield

                let prime_tower = &mut self.prime_tower_config_per_extruder[extruder_nr];
                prime_tower.speed_derivatives.smooth_speed(&global_first_layer_config_per_extruder[extruder_nr], layer_nr.max(0), speedup);
            }
        }

        {
            // meshes
            for (mesh_idx, mesh) in storage.meshes.iter().enumerate() {
                let s = mesh.settings();
                let initial_layer_speed_config =
                    SpeedDerivatives::new(s.get_velocity("speed_print_layer_0"), s.get_acceleration("acceleration_print_layer_0"), s.get_velocity("jerk_print_layer_0"));

                self.mesh_configs[mesh_idx].smooth_all_speeds(&initial_layer_speed_config, layer_nr, speedup);
                self.mesh_configs[mesh_idx].roofing_config.speed_derivatives.smooth_speed(&initial_layer_speed_config, layer_nr, speedup);
            }
        }
    }

    pub fn support_infill_extruder_nr(&self) -> usize {
        self.support_infill_extruder_nr
    }
    pub fn support_roof_extruder_nr(&self) -> usize {
        self.support_roof_extruder_nr
    }
    pub fn support_bottom_extruder_nr(&self) -> usize {
        self.support_bottom_extruder_nr
    }
    pub fn line_width_factor_per_extruder(&self) -> &[Ratio] {
        &self.line_width_factor_per_extruder
    }
}
