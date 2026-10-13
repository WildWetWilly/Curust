#!/usr/bin/env python3
"""GUI test: dual-extrusion printer, per-model settings, support extruder, 3MF export.

usage: dual.py URL MODELS_DIR OUTDIR
"""
import os, sys, time
from playwright.sync_api import sync_playwright

URL, MODELS, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(OUT, exist_ok=True)

def shot(page, name):
    page.screenshot(path=os.path.join(OUT, name + ".png"))
    print("screenshot", name)

def state(page):
    return page.evaluate("() => fetch('/api/state').then(r => r.json())")

def post(page, path, body):
    return page.evaluate("([p, b]) => fetch(p, {method: 'POST', headers: {'content-type': 'application/json'}, body: JSON.stringify(b)}).then(r => r.json())", [path, body])

with sync_playwright() as p:
    b = p.chromium.launch(args=["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])
    page = b.new_page(viewport={"width": 1500, "height": 920})
    errors = []
    page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(URL)
    # Add the S5 through the menu (a printer may already exist).
    time.sleep(1)
    if page.locator(".modal").count() == 0:
        page.click("#printer-btn"); page.click("#printer-panel >> text=Add printer")
    page.wait_for_selector(".modal .def-list")
    page.fill(".add-printer input[type=text]", "S5")
    page.click(".def-item >> text=UltiMaker S5")
    page.click(".modal-foot >> text=Add")
    page.wait_for_function("() => document.querySelectorAll('.ext-card').length == 2")
    # Extruder 2: BB 0.4 + PVA
    page.click(".ext-card >> nth=1")
    page.wait_for_selector("#material-panel select")
    page.select_option("#material-panel select", "BB 0.4")
    time.sleep(0.6)
    page.fill("#material-panel .searchbox input", "pva")
    page.click("#material-panel .mat-item >> text=PVA >> nth=0")
    time.sleep(0.6)
    st = state(page)
    print("extruders", [(e["variant"], e["material"]["name"]) for e in st["machine"]["extruders"]])
    shot(page, "d1_materials")
    page.mouse.click(700, 500)
    # Models
    page.set_input_files("#file-input", [os.path.join(MODELS, "overhang_t.stl"), os.path.join(MODELS, "sphere.stl")])
    page.wait_for_function("() => document.querySelectorAll('.ol-item').length == 2")
    st = state(page)
    ids = [n["id"] for n in st["nodes"]]
    # Second model on extruder 2 via the per-model tool
    page.click("#object-list .ol-head")
    page.click(".ol-item >> nth=1")
    page.click("#toolbar [data-tool=permodel]")
    page.wait_for_selector("#tool-panel .seg")
    page.click("#tool-panel .seg button >> nth=1")
    time.sleep(0.5)
    # First model: per-model wall line count
    page.click(".ol-item >> nth=0")
    page.wait_for_selector("#tool-panel >> text=Select settings")
    post(page, "/api/model/setting", {"id": ids[0], "key": "wall_line_count", "value": "4"})
    page.click(".ol-item >> nth=1"); page.click(".ol-item >> nth=0")
    time.sleep(0.6)
    shot(page, "d2_permodel")
    st = state(page)
    print("nodes", [(n["name"], n["extruder"], n["settings"]) for n in st["nodes"]])
    assert st["nodes"][1]["extruder"] == 1
    # Support on extruder 2 (recommended mode)
    page.keyboard.press("Escape")
    page.click("#profile-btn")
    page.click("#settings-panel >> text=Recommended")
    page.wait_for_selector("#settings-panel .rec")
    page.locator("#settings-panel .rec-row", has_text="Support").locator("input[type=checkbox]").check()
    time.sleep(0.8)
    page.locator("#settings-panel .rec-row", has_text="Support").locator("select").select_option("1")
    time.sleep(0.8)
    shot(page, "d3_recommended")
    page.click("#settings-panel >> text=Close")
    page.click("#action-panel >> text=Slice")
    page.wait_for_selector("#action-panel >> text=Save to Disk", timeout=300000)
    st = state(page)
    print("slice", {k: st["slice"][k] for k in ("state", "print_time_iso", "layer_count", "material_lengths", "material_names")})
    shot(page, "d4_sliced")
    page.click("#action-panel >> text=Preview")
    page.wait_for_selector("#preview-panel .legend")
    page.select_option("#preview-panel select", "material")
    time.sleep(1.5)
    shot(page, "d5_preview_material")
    g = page.evaluate("() => fetch('/api/gcode').then(r => r.text())")
    head = g.splitlines()[:25]
    print("\n".join(head))
    open(os.path.join(OUT, "dual.gcode"), "w").write(g)
    data = page.evaluate("() => fetch('/api/export_3mf').then(r => r.arrayBuffer()).then(b => b.byteLength)")
    print("3mf bytes", data)
    print("console errors:", errors)
    b.close()
    if errors:
        sys.exit(1)
