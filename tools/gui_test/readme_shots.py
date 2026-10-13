#!/usr/bin/env python3
"""Screenshots for the README (docs/screenshots).

usage: readme_shots.py URL OUTDIR MODEL [MODEL...]
"""
import os, sys, time
from playwright.sync_api import sync_playwright

URL, OUT, MODELS = sys.argv[1], sys.argv[2], sys.argv[3:]
os.makedirs(OUT, exist_ok=True)


def shot(page, name):
    page.mouse.move(5, 500)
    time.sleep(0.6)
    page.screenshot(path=os.path.join(OUT, name + ".png"))
    print("screenshot", name)


with sync_playwright() as p:
    b = p.chromium.launch(args=["--use-gl=angle", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"])
    page = b.new_page(viewport={"width": 1600, "height": 960})
    page.goto(URL)
    page.wait_for_selector(".modal .def-list")
    page.fill(".add-printer input[type=text]", "Ender-3")
    time.sleep(0.5)
    shot(page, "add-printer")
    page.click(".def-item >> text=Creality Ender-3 / Ender-3 v2")
    page.click(".modal-foot >> text=Add")
    page.wait_for_function("() => document.querySelector('#printer-name').textContent.includes('Ender')")
    page.set_input_files("#file-input", MODELS)
    page.wait_for_selector("#empty-hint", state="hidden")
    time.sleep(2)
    page.evaluate("""async () => {
        const st = await fetch('/api/state').then(r => r.json());
        for (const n of st.nodes) await fetch('/api/model/transform', {method: 'POST', body: JSON.stringify({id: n.id, scale: [1.6, 1.6, 1.6]})});
    }""")
    page.click("#menu-btn")
    page.click("#menu-panel >> text=Arrange all models")
    time.sleep(1.5)
    page.mouse.click(700, 300)  # deselect
    page.mouse.move(640, 600)
    for _ in range(3):
        page.mouse.wheel(0, -120)
        time.sleep(0.15)
    time.sleep(1)
    # Custom settings with tree support.
    page.click("#profile-btn")
    page.wait_for_selector("#settings-panel .custom-cta")
    page.click("#settings-panel .custom-cta")
    page.wait_for_selector("#settings-panel .cat")
    page.click("#settings-panel .cat-head >> text=Support")
    time.sleep(0.5)
    row = page.locator("#settings-panel .srow", has_text="Generate Support").first
    row.scroll_into_view_if_needed()
    row.locator("input[type=checkbox]").check()
    time.sleep(1.5)
    sel = page.locator("#settings-panel .srow", has_text="Support Structure").first
    sel.scroll_into_view_if_needed()
    sel.locator("select").select_option("tree")
    time.sleep(1.5)
    page.evaluate("() => { const b = document.querySelector('#settings-panel .sp-body'); const r = [...b.querySelectorAll('.cat-head')].find(e => e.textContent.includes('Support')); b.scrollTop = r.offsetTop - 8; }")
    time.sleep(0.8)
    shot(page, "prepare-settings")
    page.click("#settings-panel >> text=Close")
    time.sleep(0.5)
    # Slice, then the layer view.
    page.click("#action-panel >> text=Slice")
    page.wait_for_selector("#action-panel >> text=Save to Disk", timeout=600000)
    time.sleep(1)
    shot(page, "sliced")
    page.click("[data-stage=preview]")
    time.sleep(6)
    n = int(page.input_value("#ls-max-input"))
    page.fill("#ls-max-input", str(int(n * 0.45)))
    page.press("#ls-max-input", "Enter")
    time.sleep(3)
    shot(page, "preview")
    # Post-processing dialog.
    page.click("[data-stage=prepare]")
    time.sleep(1)
    page.click("#menu-btn")
    page.click("#menu-panel >> text=Post processing: modify G-code…")
    page.wait_for_selector(".pp-left")
    page.click(".pp-left >> text=Add a script")
    page.click(".pp-add-menu >> text=Pause at height")
    time.sleep(1)
    shot(page, "post-processing")
    page.click(".modal-foot >> text=Close")
    time.sleep(1)
    # Monitor.
    page.click("[data-stage=monitor]")
    page.wait_for_selector("#monitor-panel .mon-head")
    time.sleep(1)
    shot(page, "monitor")
    b.close()
