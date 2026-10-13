#!/usr/bin/env python3
"""GUI test: post-processing dialog, Marketplace dialog (installed tab + package from file),
Monitor stage with a network printer (fake OctoPrint) and a USB printer (fake Marlin).

usage: printing.py URL MODEL.stl PACKAGE.curapackage OUTDIR OCTOPRINT_URL
The server must have been started with CURUST_EXTRA_SERIAL pointing at the fake Marlin's PTY.
"""
import os, sys, time
from playwright.sync_api import sync_playwright

URL, MODEL, PACKAGE, OUT, OCTO = sys.argv[1:6]
os.makedirs(OUT, exist_ok=True)


def shot(page, name):
    page.screenshot(path=os.path.join(OUT, name + ".png"))
    print("screenshot", name)


def state(page):
    return page.evaluate("() => fetch('/api/state').then(r => r.json())")


def get(page, path):
    return page.evaluate("(p) => fetch(p).then(r => r.json())", path)


with sync_playwright() as p:
    b = p.chromium.launch(args=["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])
    page = b.new_page(viewport={"width": 1500, "height": 920})
    errors = []
    page.on("console", lambda m: errors.append(m.text) if m.type == "error" and "Marketplace" not in m.text and "Failed to load resource" not in m.text else None)
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.goto(URL)
    page.wait_for_selector(".modal .def-list")
    page.fill(".add-printer input[type=text]", "Ender-3")
    page.click(".def-item >> text=Creality Ender-3 / Ender-3 v2")
    page.click(".modal-foot >> text=Add")
    page.wait_for_function("() => document.querySelector('#printer-name').textContent.includes('Ender')")
    page.set_input_files("#file-input", MODEL)
    page.wait_for_function("() => fetch('/api/state').then(r => r.json()).then(s => s.nodes.length == 1)")
    time.sleep(1)

    # --- Post processing: add Pause at height, change its height, close.
    page.click("#menu-btn")
    page.click("#menu-panel >> text=Post processing: modify G-code…")
    page.wait_for_selector(".pp-left")
    page.click(".pp-left >> text=Add a script")
    page.wait_for_selector(".pp-add-menu")
    shot(page, "p1_pp_add_menu")
    page.click(".pp-add-menu >> text=Pause at height")
    page.wait_for_selector(".pp-settings .pp-row")
    row = page.locator(".pp-row", has_text="Pause Layer").first  # Cura 5.12 pauses at a layer number by default
    row.locator("input").fill("12")
    row.locator("input").press("Tab")
    time.sleep(0.8)
    shot(page, "p2_pp_pause_at_height")
    page.click(".pp-left >> text=Add a script")
    page.click(".pp-add-menu >> text=Search and Replace")
    time.sleep(0.8)
    row = page.locator(".pp-row", has_text="Search").first
    row.locator("input").fill("M117")
    row.locator("input").press("Tab")
    time.sleep(0.5)
    shot(page, "p3_pp_two_scripts")
    page.click(".modal-foot >> text=Close")
    time.sleep(1)
    st = state(page)
    print("post_processing:", st.get("post_processing"))
    assert len(st["post_processing"]) == 2

    # --- Slice and check the saved G-code was post-processed.
    page.click("#action-panel >> text=Slice")
    page.wait_for_selector("#action-panel >> text=Save to Disk", timeout=180000)
    g = page.evaluate("() => fetch('/api/gcode').then(r => r.text())")
    print("postprocessed:", ";POSTPROCESSED" in g, "pause block:", ";PAUSE_AT_HEIGHT" in g.upper() or "M0" in g, "lines:", g.count("\n"))
    assert ";POSTPROCESSED" in g and ";  [PauseAtHeight]" in g and ";  [SearchAndReplace]" in g

    # --- Marketplace: install a package from a file, see it in the Installed tab.
    r = page.evaluate("async (b64) => { const bin = Uint8Array.from(atob(b64), c => c.charCodeAt(0)); const r = await fetch('/api/market/install_file', {method: 'POST', body: bin}); return r.json(); }",
                      __import__("base64").b64encode(open(PACKAGE, "rb").read()).decode())
    print("install:", r)
    page.click("#menu-btn")
    page.click("#menu-panel >> text=Marketplace…")
    page.wait_for_selector(".mk-list")
    time.sleep(2)
    shot(page, "p4_marketplace_offline")
    page.click(".mk .tabs >> text=Installed")
    time.sleep(1)
    shot(page, "p5_marketplace_installed")
    page.click(".modal-foot >> text=Close")

    # --- Monitor: network printer (fake OctoPrint).
    page.click("[data-stage=monitor]")
    page.wait_for_selector("#monitor-panel .mon-head")
    page.click("#monitor-panel .tabs >> text=Network")
    page.fill("#monitor-panel input[data-key=url]", OCTO)
    page.fill("#monitor-panel input[data-key=key]", "secretkey")
    page.click("#monitor-panel >> text=Test connection")
    time.sleep(1.5)
    page.click("#monitor-panel button:text-is('Save')")
    time.sleep(4)
    shot(page, "p6_monitor_network")
    mon = get(page, "/api/monitor")
    print("net:", {k: v for k, v in (mon.get("net") or {}).items() if k != "config"})

    # --- Monitor: USB printer (fake Marlin).
    page.click("#monitor-panel .tabs >> text=USB")
    page.click("#monitor-panel button[title='Refresh ports']")
    time.sleep(1)
    page.click("#monitor-panel button:text-is('Connect')")
    page.wait_for_function("() => fetch('/api/monitor').then(r => r.json()).then(d => d.usb && d.usb.status == 'connected')", timeout=60000)
    time.sleep(3)
    shot(page, "p7_monitor_usb_connected")
    # Print over USB from the Prepare stage's action panel.
    page.click("[data-stage=prepare]")
    time.sleep(1)
    page.click("#action-panel >> text=Print over USB")
    time.sleep(4)
    shot(page, "p8_monitor_usb_printing")
    mon = get(page, "/api/monitor")
    u = mon.get("usb") or {}
    print("usb:", u.get("status"), "printing:", u.get("printing"), "progress:", u.get("progress"), u.get("firmware"))
    page.wait_for_function("() => fetch('/api/monitor').then(r => r.json()).then(d => d.usb && !d.usb.printing && d.usb.progress >= 1)", timeout=300000)
    print("usb print finished")
    print("console errors:", errors)
    b.close()
    if errors:
        sys.exit(1)
