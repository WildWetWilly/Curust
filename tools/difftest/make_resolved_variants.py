#!/usr/bin/env python3
"""Derive resolved-settings files that exercise CommandLine -r's handling of several extruders and
of models listed in the JSON itself (the C++ engine walks std::unordered_map<std::string, ...> here,
so the extruders and models are processed in libstdc++ hash-table order).

usage: make_resolved_variants.py   (writes next to this script)
"""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))
dual = json.load(open(os.path.join(HERE, "ender3_dual_resolved.json")))

# Three extruders: "extruder.2" is processed before "extruder.1" / "extruder.0", which (in the C++
# engine) creates all three ExtruderTrains with extruder_nr 2.
tri = json.loads(json.dumps(dual))
tri["global"]["machine_extruder_count"] = "3"
tri["global"]["extruders_enabled_count"] = "3"
tri["extruder.2"] = dict(tri["extruder.1"], extruder_nr="2", material_print_temperature="215")
json.dump(tri, open(os.path.join(HERE, "ender3_tri_resolved.json"), "w"), indent=1)

# Models in the JSON (paths relative to the repository root, where the harness runs the engines).
models = json.loads(json.dumps(dual))
models["tests/models/cube20.stl"] = {"mesh_position_x": "-20", "extruder_nr": "1"}
models["tests/models/sphere.stl"] = {"mesh_position_x": "15", "extruder_nr": "0"}
models["tests/models/overhang_t.stl"] = {"mesh_position_y": "30", "extruder_nr": "0", "infill_sparse_density": "40"}
json.dump(models, open(os.path.join(HERE, "ender3_dual_models_resolved.json"), "w"), indent=1)
