//! CuraEngine 5.12 ported to Rust.
//!
//! This crate is a faithful port of UltiMaker's CuraEngine (AGPL-3.0). Module and function
//! names follow the C++ sources (in snake_case) so that the two code bases can be compared
//! side by side.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::comparison_chain)]
#![allow(non_camel_case_types)]

#[macro_use]
pub mod logging;

pub mod application;
pub mod arachne;
pub mod beading_strategy;
pub mod bridge;
pub mod communication;
pub mod conical_overhang;
pub mod debug_dump;
pub mod extruder_plan;
pub mod extruder_use;
pub mod extruder_train;
pub mod fff_gcode_writer;
pub mod gcode_export;
pub mod gradual_flow;
pub mod fff_polygon_generator;
pub mod fff_processor;
pub mod geometry;
pub mod infill;
pub mod inset_order_optimizer;
pub mod interlocking_generator;
pub mod layer_plan;
pub mod layer_plan_buffer;
pub mod layer_part;
pub mod mesh;
pub mod mesh_group;
pub mod mesh_material_splitter;
pub mod mold;
pub mod multi_volumes;
pub mod path_adapter;
pub mod path_order_monotonic;
pub mod path_order_optimizer;
pub mod path_planning;
pub mod prime_tower;
pub mod print_feature;
pub mod progress;
pub mod raft;
pub mod retraction_config;
pub mod settings;
pub mod skin;
pub mod skirt_brim;
pub mod slice;
pub mod slice_data_storage;
pub mod slicer;
pub mod support;
pub mod support_infill_part;
pub mod texture_data_provider;
pub mod top_surface;
pub mod time_estimate;
// Explicit path: an older single-file `tree_support.rs` may linger in synced copies of the repo.
#[path = "tree_support/mod.rs"]
pub mod tree_support;
pub mod wall_tool_paths;
pub mod walls_computation;
pub mod utils;
