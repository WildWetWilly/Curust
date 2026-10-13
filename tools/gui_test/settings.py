#!/usr/bin/env python3
"""GUI test: custom settings panel, setting visibility, custom profiles.

usage: settings.py URL MODEL.stl OUTDIR
"""
import os, sys, time
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
    page.wait_for_selector(".modal .def-list")
    page.fill(".add-printer input[type=text]", "Ender-3")
    page.click(".def-item >> text=Creality Ender-3 / Ender-3 v2")
    page.click(".modal-foot >> text=Add")
    page.wait_for_function("() => document.querySelector('#printer-name').textContent.includes('Ender')")
    page.set_input_files("#file-input", MODEL)
    page.wait_for_function("() => document.querySelectorAll('.ol-item').length == 1")
    page.click("#profile-btn")
    page.wait_for_selector("#settings-panel .custom-cta")
    shot(page, "s1_recommended")
    page.click("#settings-panel .custom-cta")
    page.wait_for_selector("#settings-panel .cat")
    info = page.inner_text("#sp-info")
    print("info:", info)
    assert "Showing" in info
    shot(page, "s2_custom_advanced")
    # Expand all
    page.click("#sp-info .link-btn >> nth=0")
    time.sleep(0.8)
    shot(page, "s3_expanded")
    # Change a setting: wall line count
    row = page.locator("#settings-panel .srow", has_text="Wall Line Count").first
    inp = row.locator("input[type=text]")
    inp.fill("4"); inp.press("Enter")
    time.sleep(1)
    st = state(page)
    print("custom after change:", st["machine"]["quality"]["custom"])
    # Right-click a setting → context menu
    page.locator("#settings-panel .srow", has_text="Infill Density").first.click(button="right")
    page.wait_for_selector(".ctx-menu")
    shot(page, "s4_context_menu")
    page.click(".ctx-menu >> text=Hide this setting")
    time.sleep(1)
    assert st["prefs"]["visibility_preset"] in ("advanced", "custom")
    print("preset now:", state(page)["prefs"]["visibility_preset"])
    # Search finds the hidden setting, with an eye button
    page.fill("#settings-panel .searchbox input", "infill density")
    time.sleep(1)
    shot(page, "s5_search_hidden")
    page.click("#settings-panel .srow .rst.eye >> nth=0")
    time.sleep(1)
    page.fill("#settings-panel .searchbox input", "")
    time.sleep(1)
    # Visibility dialog
    page.click("#settings-panel button[title='Configure setting visibility…']")
    page.wait_for_selector(".vis-dialog .vis-item")
    shot(page, "s6_visibility_dialog")
    page.select_option(".vis-dialog select", "expert")
    time.sleep(0.8)
    page.click(".modal-foot >> text=Apply")
    time.sleep(1)
    print("preset after dialog:", state(page)["prefs"]["visibility_preset"], page.inner_text("#sp-info"))
    # Save as profile
    page.click("#settings-panel .sp-foot >> text=Save as profile…")
    page.wait_for_selector(".modal input[type=text]")
    page.fill(".modal input[type=text]", "Strong walls")
    page.click(".modal-foot >> text=Create")
    time.sleep(1.2)
    st = state(page)
    q = st["machine"]["quality"]
    print("profile:", q["profile_name"], "custom:", q["custom"], "profiles:", [c["name"] for c in st["machine"]["custom_profiles"]])
    assert q["profile_name"] == "Strong walls" and not q["custom"]
    shot(page, "s7_profile_created")
    # Switch to the Draft quality, then back to the profile
    page.select_option("#settings-panel .profile-select", "low")
    time.sleep(1.2)
    st = state(page); print("after low:", st["machine"]["quality"]["type"], st["machine"]["quality"]["profile_name"])
    idx = st["machine"]["custom_profiles"][0]["index"]
    page.select_option("#settings-panel .profile-select", f"custom:{idx}")
    time.sleep(1.2)
    st = state(page); print("reactivated:", st["machine"]["quality"]["profile_name"])
    # Slice and check the profile's setting in the G-code footer
    page.click("#settings-panel .sp-foot >> text=Close")
    page.click("#action-panel >> text=Slice")
    page.wait_for_selector("#action-panel >> text=Save to Disk", timeout=120000)
    g = page.evaluate("() => fetch('/api/gcode').then(r => r.text())")
    footer = "".join(l[len(";SETTING_3 "):] for l in g.splitlines() if l.startswith(";SETTING_3 "))
    print("footer has profile name:", "Strong walls" in footer, "wall_line_count" in footer)
    # Manage profiles: duplicate, rename, delete
    page.click("#profile-btn")
    page.wait_for_selector("#settings-panel button[title='Profile actions']")
    page.click("#settings-panel button[title='Profile actions']")
    page.click(".ctx-menu >> text=Manage profiles…")
    page.wait_for_selector(".manage-profiles .prof-item")
    page.click(".manage-profiles .prof-item:has-text('Strong walls') >> text=Duplicate")
    page.wait_for_selector(".modal.small input[type=text]")
    page.click(".modal.small .modal-foot >> text=Duplicate")
    time.sleep(1)
    page.click(".manage-profiles .prof-item:has-text('Strong walls copy') >> text=Rename")
    page.fill(".modal.small input[type=text]", "Fine walls")
    page.click(".modal.small .modal-foot >> text=Rename")
    time.sleep(1)
    shot(page, "s8_manage_profiles")
    page.click(".manage-profiles .prof-item:has-text('Fine walls') >> text=Remove")
    page.click(".modal.small .modal-foot >> text=Remove")
    time.sleep(1)
    st = state(page)
    print("profiles at end:", [c["name"] for c in st["machine"]["custom_profiles"]])
    print("console errors:", errors)
    b.close()
    if errors:
        sys.exit(1)
