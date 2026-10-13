#!/usr/bin/env python3
"""GUI test: changing a setting keeps the settings panel's scroll position, and the action
panel shows a single Slice button (no "Slice automatically" box; that's in Preferences).

usage: scroll.py URL MODEL.stl OUTDIR
"""
import os, sys, time
from playwright.sync_api import sync_playwright

URL, MODEL, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(OUT, exist_ok=True)


def shot(page, name):
    page.screenshot(path=os.path.join(OUT, name + ".png"))
    print("screenshot", name)


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
    page.wait_for_selector("#empty-hint", state="hidden")
    time.sleep(1)
    panel = page.inner_text("#action-panel")
    print("action panel:", panel.replace("\n", " | "))
    assert "Slice automatically" not in panel and "Slice" in panel
    shot(page, "a1_action_panel")

    page.click("#profile-btn")
    page.wait_for_selector("#settings-panel .custom-cta")
    page.click("#settings-panel .custom-cta")
    page.wait_for_selector("#settings-panel .cat")
    page.click("#sp-info .link-btn >> nth=0")  # expand all categories
    time.sleep(0.8)
    # Support on, then scroll the support section into view.
    row = page.locator("#settings-panel .srow", has_text="Generate Support").first
    row.scroll_into_view_if_needed()
    row.locator("input[type=checkbox]").check()
    time.sleep(1.5)
    sel_row = page.locator("#settings-panel .srow", has_text="Support Structure").first
    sel_row.scroll_into_view_if_needed()
    before = page.evaluate("() => document.querySelector('#settings-panel .sp-body').scrollTop")
    print("scrollTop before:", before)
    assert before > 500, before
    sel_row.locator("select").select_option("tree")
    time.sleep(2)
    after = page.evaluate("() => document.querySelector('#settings-panel .sp-body').scrollTop")
    st = page.evaluate("() => fetch('/api/state').then(r => r.json())")
    print("scrollTop after:", after)
    shot(page, "a2_after_tree")
    assert abs(after - before) < 5, (before, after)
    # A text field too.
    r2 = page.locator("#settings-panel .srow", has_text="Support Overhang Angle").first
    r2.scroll_into_view_if_needed()
    before = page.evaluate("() => document.querySelector('#settings-panel .sp-body').scrollTop")
    inp = r2.locator("input[type=text]")
    inp.fill("50")
    inp.press("Enter")
    time.sleep(2)
    after = page.evaluate("() => document.querySelector('#settings-panel .sp-body').scrollTop")
    print("text field scrollTop:", before, after)
    assert abs(after - before) < 5, (before, after)
    print("console errors:", errors)
    b.close()
    if errors:
        sys.exit(1)
