#!/usr/bin/env python3
"""Generate oracle job files.

make_jobs.py machines OUT [start] [count]   - every visible machine with its default setup
make_jobs.py profiles OUT                   - variant/material/quality/intent/user-setting variations
"""
import json, os, sys, glob
import os as _os, tempfile as _tempfile
# Paths (see tools/README.md): CURUST_WORK is a scratch folder for builds and results, CURA_ROOT an
# extracted UltiMaker Cura 5.12 AppImage.
WORK = _os.environ.get("CURUST_WORK", _os.path.join(_tempfile.gettempdir(), "curust-work"))
CURA_ROOT = _os.environ.get("CURA_ROOT", WORK + "/cura")
REPO = _os.path.abspath(_os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "..", ".."))

RES = CURA_ROOT + "/share/cura/resources"
MODELS = REPO + "/tests/models"
CUBE = MODELS + "/cube20.stl"
SPHERE = MODELS + "/sphere.stl"

def machines():
    out = []
    for p in sorted(glob.glob(RES + "/definitions/*.def.json")):
        d = json.load(open(p))
        mid = os.path.basename(p)[:-len(".def.json")]
        if mid in ("fdmprinter", "fdmextruder"):
            continue
        if d.get("metadata", {}).get("visible", True) is False:
            continue
        out.append(mid)
    return out

def machine_jobs(start, count):
    jobs = []
    for mid in machines()[start:start + count]:
        jobs.append({"name": "m_" + mid, "machine": mid, "models": [{"file": CUBE}]})
    return jobs

def profile_jobs():
    J = []
    def job(name, machine, **kw):
        j = {"name": name, "machine": machine, "models": kw.pop("models", [{"file": CUBE}])}
        j.update(kw)
        J.append(j)
    # Ultimaker S5: nozzles, materials, qualities, intents.
    for v in ["AA 0.4", "AA 0.8", "BB 0.4", "AA 0.25", "CC 0.6"]:
        job("s5_var_" + v.replace(" ", ""), "ultimaker_s5", extruders={"0": {"variant": v}})
    for mat in ["generic_abs", "generic_petg", "generic_tpu", "ultimaker_pla_blue", "generic_nylon", "generic_pc", "ultimaker_tough_pla_black", "generic_cpe"]:
        job("s5_mat_" + mat, "ultimaker_s5", extruders={"0": {"material": mat}})
    for q in ["draft", "fast", "normal", "high", "superdraft", "verydraft"]:
        job("s5_q_" + q, "ultimaker_s5", quality_type=q)
    for it in ["engineering", "visual", "quick", "annealing", "solid"]:
        job("s5_i_" + it, "ultimaker_s5", intent=it)
    job("s5_dual_pva", "ultimaker_s5", extruders={"1": {"variant": "BB 0.4", "material": "ultimaker_pva_natural"}},
        global_={}, models=[{"file": CUBE}, {"file": SPHERE, "extruder": 1}])
    job("s5_dual_support", "ultimaker_s5", extruders={"1": {"variant": "BB 0.4", "material": "ultimaker_pva_natural", "enabled": True}},
        global_={"support_enable": True, "support_extruder_nr": 1})
    job("s5_e1_disabled", "ultimaker_s5", extruders={"1": {"enabled": False}})
    job("um3_e2_only", "ultimaker3", extruders={"0": {"enabled": False}}, models=[{"file": CUBE, "extruder": 1}])
    job("um3_mixed", "ultimaker3", extruders={"0": {"material": "generic_abs"}, "1": {"variant": "BB 0.4", "material": "generic_pva"}},
        quality_type="normal", models=[{"file": CUBE}, {"file": SPHERE, "extruder": 1}])
    # Method / Factor
    for m in ["ultimaker_method", "ultimaker_methodx", "ultimaker_factor4", "ultimaker_s7", "ultimaker_s3", "ultimaker2_plus_connect"]:
        job(m + "_default", m)
    job("methodx_abs", "ultimaker_methodx", extruders={"0": {"material": "ultimaker_abs_black"}})
    job("factor4_q", "ultimaker_factor4", quality_type="draft")
    # Ender 3 / Prusa with user settings.
    E = "creality_ender3"
    job("e3_brim", E, global_={"adhesion_type": "brim"})
    job("e3_raft", E, global_={"adhesion_type": "raft"})
    job("e3_none", E, global_={"adhesion_type": "none"})
    job("e3_support_tree", E, global_={"support_enable": True, "support_structure": "tree"})
    job("e3_support_brim", E, global_={"adhesion_type": "none", "support_enable": True, "support_brim_enable": True})
    job("e3_layer", E, global_={"layer_height": 0.12, "infill_sparse_density": 35, "wall_line_count": 4})
    job("e3_formula", E, global_={"infill_line_distance": "=line_width * 3", "speed_print": "=40 + 10"})
    job("e3_bad_formula", E, global_={"speed_print": "=undefined_thing + 1", "speed_travel": "=1/0"})
    job("e3_strvalue", E, global_={"infill_pattern": "gyroid", "top_bottom_pattern": "concentric"})
    job("e3_one_at_a_time", E, global_={"print_sequence": "one_at_a_time"}, models=[{"file": CUBE}, {"file": SPHERE}])
    job("e3_q_draft", E, quality_type="draft")
    job("e3_q_super", E, quality_type="super")
    job("e3_q_low", E, quality_type="low")
    job("e3_mat_petg", E, extruders={"0": {"material": "generic_petg_175"}})
    job("e3_mat_tpu", E, extruders={"0": {"material": "generic_tpu_175"}})
    job("e3_var_06", E, extruders={"0": {"variant": "0.6mm Nozzle"}})
    job("e3_ext_user", E, extruders={"0": {"user": {"material_print_temperature": 215, "retraction_amount": 6.5, "speed_wall": "=speed_print / 3"}}})
    job("e3_gcode_if", E, global_={"machine_start_gcode": "G28 ;home\n{if material_bed_temperature > 55}M190 S{material_bed_temperature}\n{elif material_bed_temperature > 0}M140 S0\n{else}; cold\n{endif}\nM104 S{material_print_temperature, 0}\n;{print_time}\n{jobname} {time} {date} {day}\n{material_name} {material_type} {quality_name} {initial_extruder_nr}\n{iffy}\n",
                                       "machine_end_gcode": "{if layer_height < 0.15}fine{else}coarse{endif} {print_temperature}\n{if a}{if b}{endif}{endif}\n"})
    job("e3_gcode_nested", E, global_={"machine_end_gcode": "{if 1}\n{if 2}x{endif}\n{endif}"})
    job("e3_gcode_prepend", E, global_={"machine_start_gcode": "M190 S{material_bed_temperature_layer_0}\nM109 S{material_print_temperature_layer_0, 0}\n"})
    job("e3_gcode_prepend2", E, global_={"machine_start_gcode": ";M190 S{material_bed_temperature}\nM109 S{ print_temperature }\n; {material_print_temperature}"})
    job("e3_gcode_weird", E, global_={"machine_start_gcode": "{ layer_height , 0 }\n{layer_height,initial_extruder_nr}\n{{x}}\n{ }\n{,}\n{speed_print*2}\n{round(speed_print/7, 2)}\n{\"a\" if True else \"b\"}\n{[x for x in [1,2]]}\n{machine_name}\n"})
    job("e3_perobject", E, models=[{"file": CUBE, "settings": {"infill_sparse_density": 80, "wall_line_count": 5}},
                                   {"file": SPHERE, "settings": {"infill_mesh": True}},
                                   {"file": CUBE, "settings": {"support_mesh": True, "support_enable": True}},
                                   {"file": SPHERE, "settings": {"anti_overhang_mesh": True}},
                                   {"file": CUBE, "settings": {"top_layers": "=bottom_layers + 2", "infill_pattern": "cubic"}}])
    P = "prusa_i3_mk3"
    if os.path.exists(RES + "/definitions/" + P + ".def.json"):
        job("mk3_default", P)
    for m in ["anycubic_i3_mega", "creality_cr10s", "creality_ender3pro", "creality_ender5", "elegoo_neptune_4", "anker_m5", "bambulab_x1", "voron2_350", "flashforge_dreamer_nx", "makerbot_replicator"]:
        if os.path.exists(RES + "/definitions/" + m + ".def.json"):
            job(m + "_brim_support", m, global_={"adhesion_type": "brim", "support_enable": True})
    # multi-extruder generic machines
    for m in ["bcn3dsigma", "bcn3dsigmax", "deltacomb_dc20dual", "tizyx_evy_dual", "imade3d_jellybox_2", "custom", "ultimaker_original_dual", "felixtec4dual", "strateo3d", "zyyx_pro"]:
        if os.path.exists(RES + "/definitions/" + m + ".def.json"):
            job(m + "_two", m, models=[{"file": CUBE}, {"file": SPHERE, "extruder": 1}], global_={"adhesion_type": "skirt"})
    # dual user settings with limit_to_extruder
    job("s5_limit", "ultimaker_s5", extruders={"1": {"variant": "AA 0.4", "material": "generic_pla", "user": {"wall_line_width_0": 0.3}}},
        global_={"wall_0_extruder_nr": 1, "infill_extruder_nr": 1, "support_enable": True, "support_infill_extruder_nr": 1, "adhesion_type": "raft", "raft_base_extruder_nr": 1},
        models=[{"file": CUBE, "settings": {"wall_line_count": 3}}])
    job("s5_skirt_extr", "ultimaker_s5", global_={"adhesion_type": "skirt", "skirt_brim_extruder_nr": 1})
    for j in J:
        if "global_" in j:
            j["global"] = j.pop("global_")
    return J

if __name__ == "__main__":
    kind, out = sys.argv[1], sys.argv[2]
    if kind == "machines":
        start = int(sys.argv[3]) if len(sys.argv) > 3 else 0
        count = int(sys.argv[4]) if len(sys.argv) > 4 else 100000
        jobs = machine_jobs(start, count)
    else:
        jobs = profile_jobs()
    json.dump(jobs, open(out, "w"), indent=0)
    print(len(jobs), "jobs")
