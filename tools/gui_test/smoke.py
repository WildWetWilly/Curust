#!/usr/bin/env python3
"""End-to-end GUI test with Playwright (headless Chromium).

usage: smoke.py URL MODEL.stl OUTDIR
Adds a printer, loads a model, moves/rotates/scales it, changes settings, slices, previews and
saves G-code, taking screenshots along the way.
"""
import json, os, sys, time
from playwright.sync_api import sync_playwright

URL, MODEL, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(OUT, exist_ok=True)

def shot(page, name):
    page.screenshot(path=os.path.join(OUT, name + ".png"))
    print("screenshot", name)

def state(page):
    return page.evaluate("() => fetch('/api/state').then(r => r.json())")

with sync_playwright() as p:
    b = p.chromium.launch(args=["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])
    page = b.new_page(viewport={"width": 1500, "height": 920})
    errors = []
    page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(URL)
    page.wait_for_selector(".modal")
    shot(page, "01_add_printer")
    page.fill(".add-printer input[type=text]", "ender-3")
    page.click(".def-item >> text=Creality Ender-3 / Ender-3 v2")
    page.click(".modal-foot >> text=Add")
    page.wait_for_function("() => document.querySelector('#printer-name').textContent.includes('Ender')")
    time.sleep(0.5)
    shot(page, "02_printer_added")
    page.set_input_files("#file-input", MODEL)
    page.wait_for_function("() => document.querySelectorAll('.ol-item').length > 0")
    time.sleep(0.5)
    shot(page, "03_model_loaded")
    st = state(page)
    assert len(st["nodes"]) == 1, st["nodes"]
    node = st["nodes"][0]
    # Move tool: type a position
    page.click("#toolbar [data-tool=move]")
    page.wait_for_selector("#tool-panel h3")
    inp = page.locator("#tool-panel input[type=number]").first
    inp.fill("20"); inp.press("Enter")
    page.wait_for_function("() => fetch('/api/state').then(r=>r.json()).then(s => Math.abs(s.nodes[0].position[0]-20) < 1e-6)")
    # Rotate +90 around Z
    page.click("#toolbar [data-tool=rotate]")
    page.click("#tool-panel >> text=+90° >> nth=2")
    time.sleep(0.4)
    shot(page, "04_rotated")
    # Scale 150 %
    page.click("#toolbar [data-tool=scale]")
    pct = page.locator("#tool-panel .unit-input input").nth(1)
    pct.fill("150"); pct.press("Enter")
    time.sleep(0.5)
    st = state(page)
    print("scale", st["nodes"][0]["scale"])
    assert abs(st["nodes"][0]["scale"][0] - 1.5) < 1e-6
    page.keyboard.press("Escape")
    # Settings panel: recommended -> custom
    page.click("#profile-btn")
    page.wait_for_selector("#settings-panel .qual")
    shot(page, "05_recommended")
    page.click("#settings-panel >> text=Custom")
    page.wait_for_selector("#settings-panel .cat")
    shot(page, "06_custom_settings")
    # Change infill density via the custom panel (search)
    page.fill("#settings-panel .searchbox input", "infill density")
    page.wait_for_selector("#settings-panel .srow")
    time.sleep(0.5)
    row = page.locator("#settings-panel .srow", has_text="Infill Density").first
    box = row.locator("input")
    box.fill("35"); box.press("Enter")
    time.sleep(0.8)
    st = state(page)
    print("summary", page.inner_text("#profile-summary"))
    assert "35%" in page.inner_text("#profile-summary")
    page.click("#settings-panel >> text=Close")
    # Slice
    page.click("#action-panel >> text=Slice")
    page.wait_for_selector("#action-panel >> text=Save to Disk", timeout=120000)
    shot(page, "07_sliced")
    st = state(page)
    print("slice", {k: st["slice"][k] for k in ("state", "print_time_iso", "layer_count", "material_lengths")})
    # Preview
    page.click("#action-panel >> text=Preview")
    page.wait_for_selector("#preview-panel .legend")
    time.sleep(1.0)
    shot(page, "08_preview")
    page.select_option("#preview-panel select", "speed")
    time.sleep(1.0)
    shot(page, "09_preview_speed")
    # Layer slider to the middle
    n = st["slice"]["layer_count"]
    page.fill("#ls-max-input", str(max(1, n // 2))); page.press("#ls-max-input", "Enter")
    time.sleep(0.6)
    shot(page, "10_preview_half")
    # Save G-code via the API endpoint the dialog uses
    page.click(".stages >> text=Prepare")
    res = page.evaluate("() => fetch('/api/gcode').then(r => r.text())")
    open(os.path.join(OUT, "out.gcode"), "w").write(res)
    print("gcode bytes", len(res), res.splitlines()[0])
    # Machine settings dialog
    page.click("#printer-btn")
    page.click("#printer-panel >> text=Machine settings")
    page.wait_for_selector(".modal .ms-grid")
    shot(page, "11_machine_settings")
    page.click(".modal-foot >> text=Close")
    # Material popover
    page.click(".ext-card")
    page.wait_for_selector("#material-panel .mat-item")
    shot(page, "12_materials")
    print("console errors:", errors)
    b.close()
    if errors:
        sys.exit(1)
