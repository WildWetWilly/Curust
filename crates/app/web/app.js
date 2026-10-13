// Curust user interface.
"use strict";

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => Array.from(el.querySelectorAll(s));
const h = (tag, attrs = {}, ...children) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style" && typeof v === "object") Object.assign(e.style, v);
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "html") e.innerHTML = v;
    else if (v === true) e.setAttribute(k, "");
    else e.setAttribute(k, v);
  }
  for (const c of children.flat()) if (c !== null && c !== undefined && c !== false) e.append(c.nodeType ? c : document.createTextNode(String(c)));
  return e;
};
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const fmt = (v, d = 2) => (Math.round(v * 10 ** d) / 10 ** d).toString();

// ---------------------------------------------------------------- API
const api = {
  async get(path) {
    const r = await fetch(path);
    const ct = r.headers.get("content-type") || "";
    const body = ct.includes("json") ? await r.json() : await r.arrayBuffer();
    if (!r.ok) throw new Error(body.error || r.statusText);
    return body;
  },
  async post(path, data = {}) {
    const r = await fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(data) });
    const body = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(body.error || r.statusText);
    return body;
  },
};

function toast(msg, err = false, ms = 4200) {
  const t = h("div", { class: "toast" + (err ? " err" : "") }, msg);
  $("#toasts").append(t);
  setTimeout(() => t.remove(), ms);
}
async function act(fn) {
  try { return await fn(); } catch (e) { toast(e.message || String(e), true); }
}

// ---------------------------------------------------------------- state
const ui = {
  state: null,
  stage: "prepare",
  tool: null,
  selected: new Set(),
  settingsMode: localStorage.getItem("curust.settingsMode") || "recommended",
  settingsOpen: false,
  panelWidth: Math.max(360, parseInt(localStorage.getItem("curust.panelWidth") || "470")),
  openCats: new Set(JSON.parse(localStorage.getItem("curust.openCats") || '["resolution","shell","infill"]')),
  search: "",
  spScroll: {},
  meshVersions: new Map(),
  layersFor: -1,
  colorScheme: "type",
  pollTimer: null,
};
let viewer;
try {
  viewer = new Viewer($("#view"));
} catch (e) {
  document.body.innerHTML = `<div style="padding:40px;font-family:sans-serif"><h2>Curust needs WebGL 2</h2><p>${esc(e.message)}</p></div>`;
  throw e;
}

function extruderColors() {
  const m = ui.state?.machine;
  return (m?.extruders || []).map((e) => hexToRgb(e.material.color));
}

async function refresh() {
  const st = await api.get("/api/state");
  const prevGen = ui.state?.generation;
  const prevMachine = ui.state?.machine ? JSON.stringify([ui.state.machine.width, ui.state.machine.depth, ui.state.machine.height, ui.state.machine.shape, ui.state.machine.disallowed_areas, ui.state.machine.border, ui.state.machine.center_is_zero]) : null;
  ui.state = st;
  for (const t of st.toasts || []) toast(t);
  if (st.resource_error) { showResourceError(st.resource_error); return; }
  const m = st.machine;
  const machineKey = m ? JSON.stringify([m.width, m.depth, m.height, m.shape, m.disallowed_areas, m.border, m.center_is_zero]) : null;
  if (machineKey !== prevMachine) {
    viewer.setMachine(m);
    if (!prevMachine) viewer.setView("3d");
  }
  // Meshes: fetch new ones, drop removed ones.
  const ids = new Set(st.nodes.map((n) => n.id));
  for (const id of Array.from(viewer.meshes.keys())) if (!ids.has(id)) viewer.dropMesh(id);
  for (const id of Array.from(ui.selected)) if (!ids.has(id)) ui.selected.delete(id);
  await Promise.all(st.nodes.filter((n) => !viewer.meshes.has(n.id)).map(async (n) => {
    const buf = await api.get(`/api/mesh?id=${n.id}`);
    viewer.setMesh(n.id, new Float32Array(buf));
  }));
  viewer.selected = ui.selected;
  viewer.setNodes(st.nodes, extruderColors());
  renderTopbar();
  renderObjectList();
  renderAction();
  renderToolbar();
  renderToolPanel();
  $("#empty-hint").hidden = !(m && st.nodes.length === 0 && ui.stage === "prepare");
  if (!m && !document.querySelector(".modal-back")) openAddPrinter(true);
  if (ui.settingsOpen && prevGen !== st.generation) renderSettings();
  if (!$("#printer-panel").hidden) renderPrinterPanel();
  if (!$("#material-panel").hidden && ui.materialPos !== undefined) renderMaterialPanel(ui.materialPos);
  schedulePoll();
  if (ui.stage === "preview") syncPreview();
}

function schedulePoll() {
  clearTimeout(ui.pollTimer);
  const s = ui.state?.slice?.state;
  ui.pollTimer = setTimeout(async () => {
    if (s === "slicing") {
      try {
        const sl = await api.get("/api/slice");
        if (sl.state !== "slicing") { await refresh(); return; }
        ui.state.slice = sl; renderAction();
      } catch (e) { /* server gone */ }
      schedulePoll();
    } else {
      // Keep-alive (the server quits some time after the window closes).
      try { await api.get("/api/slice"); } catch (e) {}
      schedulePoll();
    }
  }, s === "slicing" ? 250 : 2000);
}

function showResourceError(msg) {
  $("#main").innerHTML = "";
  $("#main").append(h("div", { style: { padding: "60px", maxWidth: "720px", margin: "0 auto" } },
    h("h2", {}, "Printer profiles not found"),
    h("p", {}, msg),
    h("p", {}, "Curust uses the printer, material and quality profiles of UltiMaker Cura 5.x. Install Cura, put ", h("code", {}, "curust-resources.zip"), " next to curust.exe, or start Curust with ", h("code", {}, "--resources <folder>"), ".")));
}

// ---------------------------------------------------------------- top bar
function renderTopbar() {
  const st = ui.state, m = st.machine;
  $("#printer-name").textContent = m ? m.name : "Add a printer";
  const cards = $("#extruder-cards");
  cards.innerHTML = "";
  if (m) {
    for (const e of m.extruders) {
      const c = h("button", { class: "ext-card" + (e.enabled ? "" : " disabled") + (m.extruders.length > 1 && e.position === m.active_extruder ? " active" : ""), title: "Material and nozzle" },
        h("span", { class: "ext-num", style: { background: e.material.color } }, String(e.position + 1)),
        h("span", { class: "ext-text" },
          h("span", { class: "t1" }, e.enabled ? (e.material.name || "No material") : "Disabled"),
          h("span", { class: "t2" }, [m.has_variants ? e.variant : `${fmt(e.nozzle_size, 2)} mm`, e.material.brand].filter(Boolean).join(" · "))));
      c.onclick = (ev) => { ev.stopPropagation(); toggleMaterialPanel(e.position); };
      cards.append(c);
    }
    const q = m.quality, r = m.recommended;
    const intent = m.intent && m.intent !== "default" ? ` · ${cap(m.intent)}` : "";
    $("#profile-summary").textContent = q.supported
      ? `${q.profile_name || q.name}${q.custom ? "*" : ""} - ${fmt(q.layer_height, 2)}mm${intent} | ${fmt(r.infill, 0)}% | ${r.support ? "Support" : "No support"} | ${r.adhesion ? "Adhesion" : "No adhesion"}`
      : "Not supported";
  } else {
    $("#profile-summary").textContent = "Print settings";
  }
}
const cap = (s) => s ? s[0].toUpperCase() + s.slice(1) : s;

function closePopovers(except) {
  for (const id of ["#printer-panel", "#material-panel", "#menu-panel"]) if (id !== except) $(id).hidden = true;
}
document.addEventListener("mousedown", (e) => {
  for (const id of ["#printer-panel", "#material-panel", "#menu-panel"]) {
    const p = $(id);
    if (!p.hidden && !p.contains(e.target) && !e.target.closest(".ext-card,#printer-btn,#menu-btn") && !e.target.closest(".modal-back")) p.hidden = true;
  }
});

$("#printer-btn").onclick = (e) => { e.stopPropagation(); const p = $("#printer-panel"); closePopovers("#printer-panel"); p.hidden = !p.hidden; if (!p.hidden) renderPrinterPanel(); };
$("#menu-btn").onclick = (e) => { e.stopPropagation(); const p = $("#menu-panel"); closePopovers("#menu-panel"); p.hidden = !p.hidden; if (!p.hidden) renderMenu(); };
$("#profile-btn").onclick = () => { ui.settingsOpen = !ui.settingsOpen; $("#settings-panel").hidden = !ui.settingsOpen; if (ui.settingsOpen) renderSettings(); };

function renderPrinterPanel() {
  const st = ui.state, p = $("#printer-panel");
  p.innerHTML = "";
  p.append(h("div", { class: "section-label" }, "Printers"));
  const list = h("div", { class: "pp-list" });
  st.machines.forEach((m, i) => {
    list.append(h("div", { class: "pp-item" + (i === st.active ? " active" : ""), onclick: () => act(async () => { await api.post("/api/machine/select", { index: i }); p.hidden = true; await refresh(); viewer.setView("3d"); }) },
      h("span", { class: "icon i-printer" }), h("div", { class: "grow" }, h("div", {}, m.name), h("div", { class: "sub" }, m.definition_name))));
  });
  if (!st.machines.length) list.append(h("div", { class: "muted" }, "No printers yet."));
  p.append(list);
  p.append(h("div", { class: "btn-row" },
    h("button", { class: "btn", onclick: () => { p.hidden = true; openAddPrinter(false); } }, "Add printer"),
    h("button", { class: "btn", disabled: !st.machine, onclick: () => { p.hidden = true; openMachineSettings(); } }, h("span", { class: "icon i-cog" }), "Machine settings"),
    h("button", { class: "btn", onclick: () => { p.hidden = true; openManagePrinters(); } }, "Manage")));
}

// ---------------------------------------------------------------- material popover
function toggleMaterialPanel(pos) {
  const p = $("#material-panel");
  closePopovers("#material-panel");
  if (!p.hidden && ui.materialPos === pos) { p.hidden = true; return; }
  ui.materialPos = pos;
  p.hidden = false;
  act(async () => { await api.post("/api/extruder/active", { position: pos }); ui.state.machine.active_extruder = pos; renderTopbar(); if (ui.settingsOpen) renderSettings(); });
  renderMaterialPanel(pos);
}

async function renderMaterialPanel(pos) {
  const m = ui.state.machine, p = $("#material-panel");
  if (!m) return;
  const e = m.extruders[pos];
  if (!e) { p.hidden = true; return; }
  const mats = await api.get(`/api/materials?position=${pos}`).catch(() => []);
  p.innerHTML = "";
  p.append(h("div", { class: "row", style: { justifyContent: "space-between" } },
    h("b", {}, m.extruders.length > 1 ? `${e.name}` : "Material & nozzle"),
    m.extruders.length > 1 ? h("label", { class: "row", style: { gap: "6px" } }, h("input", { type: "checkbox", checked: e.enabled, onchange: (ev) => act(async () => { await api.post("/api/extruder/enabled", { position: pos, enabled: ev.target.checked }); await refresh(); }) }), "Enabled") : null));
  if (m.has_variants && m.variants.length) {
    const sel = h("select", { class: "grow", onchange: (ev) => act(async () => { await api.post("/api/extruder/variant", { position: pos, name: ev.target.value }); await refresh(); }) },
      m.variants.map((v) => h("option", { value: v, selected: v === e.variant }, v)));
    p.append(h("div", { class: "row", style: { marginTop: "10px" } }, h("label", {}, m.variants_name), sel));
  }
  p.append(h("div", { class: "section-label" }, `Material (${fmt(e.diameter, 2)} mm)`));
  if (!m.has_materials) { p.append(h("div", { class: "muted" }, "This printer does not use material profiles.")); return; }
  const search = h("div", { class: "searchbox" }, h("span", { class: "icon i-search" }), h("input", { type: "text", placeholder: "Search materials", value: ui.matSearch || "" }));
  const list = h("div", { class: "mat-list" });
  const draw = () => {
    const q = (ui.matSearch || "").toLowerCase();
    list.innerHTML = "";
    const brands = new Map();
    for (const mt of mats) {
      if (q && !(`${mt.name} ${mt.brand} ${mt.type}`.toLowerCase().includes(q))) continue;
      if (!brands.has(mt.brand)) brands.set(mt.brand, []);
      brands.get(mt.brand).push(mt);
    }
    const order = Array.from(brands.keys()).sort((a, b) => (a === "Generic" ? -1 : b === "Generic" ? 1 : a.localeCompare(b)));
    for (const b of order) {
      const items = brands.get(b).sort((x, y) => x.type.localeCompare(y.type) || x.name.localeCompare(y.name));
      const open = q || b === e.material.brand || b === "Generic";
      const grp = h("div", { class: "mat-brand" + (open ? " open" : "") },
        h("div", { class: "mb-head", onclick: () => grp.classList.toggle("open") }, b, h("span", { class: "muted" }, String(items.length))),
        h("div", { class: "mb-items" }, items.map((mt) => h("div", { class: "mat-item" + (mt.base === e.material.base ? " active" : ""), onclick: () => act(async () => { await api.post("/api/extruder/material", { position: pos, base: mt.base }); await refresh(); }) },
          h("span", { class: "sw", style: { background: mt.color } }), mt.name, h("span", { class: "ty" }, mt.type)))));
      list.append(grp);
    }
  };
  search.querySelector("input").oninput = (ev) => { ui.matSearch = ev.target.value; draw(); };
  p.append(search, h("div", { style: { height: "8px" } }), list,
    h("div", { class: "row", style: { marginTop: "8px" } }, h("button", { class: "btn small", onclick: () => { p.hidden = true; openManageMaterials(pos); } }, "Manage materials…")));
  draw();
}

// ---------------------------------------------------------------- material manager
/** Preferences > Materials: browse, create, duplicate, edit, import and export materials. */
async function openManageMaterials(pos) {
  if (!ui.state.machine) { toast("Add a printer first"); return; }
  if (pos === undefined) pos = ui.state.machine.active_extruder || 0;
  let mats = [], selected = ui.state.machine.extruders[pos]?.material?.base, tab = "info", search = "";
  const listEl = h("div", { class: "mm-list" });
  const detail = h("div", { class: "mm-detail" });
  const searchEl = h("input", { type: "text", placeholder: "Search materials", oninput: (e) => { search = e.target.value.toLowerCase(); drawList(); } });
  const body = h("div", { class: "mm" }, h("div", { class: "mm-left" }, h("div", { class: "searchbox" }, h("span", { class: "icon i-search" }), searchEl), listEl), detail);
  const load = async () => { mats = await api.get(`/api/material_manager?position=${pos}`); if (!mats.find((x) => x.base === selected) && mats.length) selected = mats[0].base; drawList(); drawDetail(); };
  const after = async (fn) => act(async () => { const r = await fn(); await refresh(); if (r && r.base) selected = r.base; await load(); });
  const drawList = () => {
    listEl.innerHTML = "";
    const brands = new Map();
    for (const mt of mats) {
      if (search && !(`${mt.name} ${mt.brand} ${mt.type} ${mt.color_name}`.toLowerCase().includes(search))) continue;
      const b = mt.user ? "Custom materials" : mt.brand;
      if (!brands.has(b)) brands.set(b, []);
      brands.get(b).push(mt);
    }
    const order = Array.from(brands.keys()).sort((a, b) => (a === "Custom materials" ? -1 : b === "Custom materials" ? 1 : a === "Generic" ? -1 : b === "Generic" ? 1 : a.localeCompare(b)));
    for (const b of order) {
      const items = brands.get(b).sort((x, y) => x.type.localeCompare(y.type) || x.name.localeCompare(y.name));
      const open = search || items.some((x) => x.base === selected) || b === "Custom materials";
      const grp = h("div", { class: "mat-brand" + (open ? " open" : "") },
        h("div", { class: "mb-head", onclick: () => grp.classList.toggle("open") }, b, h("span", { class: "muted" }, String(items.length))),
        h("div", { class: "mb-items" }, items.map((mt) => h("div", { class: "mat-item" + (mt.base === selected ? " active" : ""), onclick: () => { selected = mt.base; drawList(); drawDetail(); } },
          h("span", { class: "sw", style: { background: mt.color } }), mt.name, mt.active ? h("span", { class: "badge" }, "in use") : null, h("span", { class: "ty" }, mt.type)))));
      listEl.append(grp);
    }
  };
  const field = (mt, label, key, value, opts = {}) => {
    const inp = opts.multiline ? h("textarea", { rows: 3, disabled: !mt.user }, value || "") : h("input", { type: opts.type || "text", value: value ?? "", disabled: !mt.user, step: opts.step || "any" });
    inp.addEventListener("change", () => after(() => api.post("/api/material/update", { base: mt.base, field: key, value: String(inp.value) })));
    return h("div", { class: "mm-row" }, h("label", {}, label), inp, opts.unit ? h("span", { class: "muted" }, opts.unit) : null);
  };
  const drawDetail = () => {
    // Keep the scroll position when the same material and tab are redrawn after a change.
    const oldPane = detail.querySelector(".mm-pane");
    const keepScroll = oldPane && detail.dataset.view === `${selected}|${tab}` ? oldPane.scrollTop : 0;
    detail.dataset.view = `${selected}|${tab}`;
    detail.innerHTML = "";
    const mt = mats.find((x) => x.base === selected);
    if (!mt) { detail.append(h("div", { class: "muted" }, "No material selected.")); return; }
    const head = h("div", { class: "mm-head" }, h("span", { class: "sw big", style: { background: mt.color } }), h("div", { class: "grow" }, h("b", {}, mt.name), h("div", { class: "muted" }, `${mt.brand} · ${mt.type}${mt.user ? " · custom material" : ""}`)));
    const tabs = h("div", { class: "tabs" }, [["info", "Information"], ["settings", "Print settings"]].map(([k, l]) => h("button", { class: "tab" + (tab === k ? " active" : ""), onclick: () => { tab = k; drawDetail(); } }, l)));
    const pane = h("div", { class: "mm-pane" });
    if (!mt.user) pane.append(h("div", { class: "note" }, "Materials that come with Cura are read-only. Use Duplicate to make an editable copy."));
    if (tab === "info") {
      const color = h("input", { type: "color", value: /^#[0-9a-f]{6}$/i.test(mt.color) ? mt.color : "#888888", disabled: !mt.user });
      color.addEventListener("change", () => after(() => api.post("/api/material/update", { base: mt.base, field: "color_code", value: color.value })));
      pane.append(field(mt, "Display name", "name", mt.name), field(mt, "Brand", "brand", mt.brand), field(mt, "Material type", "material", mt.type),
        h("div", { class: "mm-row" }, h("label", {}, "Color"), color, h("input", { type: "text", value: mt.color_name, disabled: !mt.user, onchange: (e) => after(() => api.post("/api/material/update", { base: mt.base, field: "color_name", value: e.target.value })) })),
        h("div", { class: "section-label" }, "Properties"),
        field(mt, "Density", "density", mt.density, { type: "number", unit: "g/cm³", step: "0.01" }),
        field(mt, "Diameter", "diameter", mt.diameter, { type: "number", unit: "mm", step: "0.01" }));
      const cost = h("input", { type: "number", step: "0.01", value: mt.spool_cost ?? "" });
      const weight = h("input", { type: "number", step: "1", value: mt.spool_weight ?? (parseFloat(mt.weight) || 1000) });
      const saveCost = () => act(() => api.post("/api/prefs", { material_guid: mt.guid, spool_cost: parseFloat(cost.value) || 0, spool_weight: parseFloat(weight.value) || 0 }));
      cost.onchange = saveCost; weight.onchange = saveCost;
      const d = parseFloat(mt.diameter) || 1.75, dens = parseFloat(mt.density) || 1.24, w = parseFloat(weight.value) || 0;
      const len = w / (dens * Math.PI * (d / 2) ** 2);
      pane.append(h("div", { class: "mm-row" }, h("label", {}, "Filament cost"), cost, h("span", { class: "muted" }, ui.state.prefs.currency)),
        h("div", { class: "mm-row" }, h("label", {}, "Filament weight"), weight, h("span", { class: "muted" }, "g")),
        h("div", { class: "mm-row" }, h("label", {}, "Filament length"), h("span", {}, `~${fmt(len, 0)} m`)),
        h("div", { class: "section-label" }, "Description"), field(mt, "", "description", mt.description, { multiline: true }),
        h("div", { class: "section-label" }, "Adhesion information"), field(mt, "", "adhesion_info", mt.adhesion_info, { multiline: true }),
        h("div", { class: "muted small" }, `GUID ${mt.guid}`));
    } else {
      pane.append(h("div", { class: "muted", style: { marginBottom: "8px" } }, "Values for the active printer and nozzle. Changing one stores it in this material's profile."));
      for (const st of mt.settings) {
        const inp = h("input", { type: "number", step: st.unit === "mm" ? "0.01" : "1", value: st.value ?? "", disabled: !mt.user, title: st.description });
        inp.addEventListener("change", () => after(() => api.post("/api/material/update", { base: mt.base, field: st.key, value: String(inp.value) })));
        pane.append(h("div", { class: "mm-row", title: st.description }, h("label", {}, st.label), inp, h("span", { class: "muted" }, st.unit)));
      }
    }
    const actions = h("div", { class: "row mm-actions" },
      h("button", { class: "btn primary", disabled: mt.active, onclick: () => act(async () => { await api.post("/api/extruder/material", { position: pos, base: mt.base }); await refresh(); await load(); }) }, mt.active ? "Active" : "Activate"),
      h("button", { class: "btn", onclick: () => after(() => api.post("/api/material/create")) }, "Create new"),
      h("button", { class: "btn", onclick: () => after(() => api.post("/api/material/duplicate", { base: mt.base })) }, "Duplicate"),
      h("button", { class: "btn danger", disabled: !mt.user || mt.in_use, title: !mt.user ? "Only custom materials can be removed" : mt.in_use ? "The material is in use" : "", onclick: async () => { if (await confirmDialog("Remove material", `Remove the material "${mt.name}"? This cannot be undone.`, "Remove", "Cancel")) after(() => api.post("/api/material/remove", { base: mt.base })); } }, "Remove"),
      h("span", { class: "grow" }),
      h("button", { class: "btn", title: "Import .xml.fdm_material files (from Cura, a manufacturer or another Curust)", onclick: async () => {
        const files = await pickFiles(".fdm_material,.xml", true);
        for (const f of files) {
          await after(async () => { const r = await fetch(`/api/material/import?name=${encodeURIComponent(f.name)}`, { method: "POST", body: await f.arrayBuffer() }); if (!r.ok) throw new Error(await r.text()); const j = await r.json(); toast(j.message); return j; });
        }
      } }, "Import…"),
      h("button", { class: "btn", onclick: () => downloadFrom(`/api/material/export?base=${encodeURIComponent(mt.base)}`, `${mt.base.replace(/ /g, "+")}.xml.fdm_material`, "Material profile", "application/x-ultimaker-material-profile", ".fdm_material") }, "Export…"));
    detail.append(head, tabs, pane, actions);
    pane.scrollTop = keepScroll;
  };
  modal("Materials", body, [["Close", async () => { await refresh(); }]], "wide tall");
  await load();
}

// ---------------------------------------------------------------- post processing (Cura's PostProcessingPlugin)
async function openPostProcessing() {
  if (!ui.state.machine) { toast("Add a printer first"); return; }
  let data = await api.get("/api/postproc");
  let sel = data.scripts.length ? 0 : -1;
  const left = h("div", { class: "pp-left" });
  const right = h("div", { class: "pp-right" });
  const op = (body) => act(async () => { data = await api.post("/api/postproc/op", body); draw(); });
  const addMenu = (anchor) => {
    document.querySelectorAll(".ctx-menu").forEach((e) => e.remove());
    const menu = h("div", { class: "ctx-menu panel pp-add-menu" },
      data.available.map((a) => h("button", { class: "menu-item", onclick: () => { menu.remove(); sel = data.scripts.length; op({ op: "add", key: a.key }); } },
        h("span", { class: "grow" }, a.label), a.origin !== "Cura (bundled)" ? h("span", { class: "muted", title: a.origin }, "user") : null)));
    document.body.append(menu);
    const r = anchor.getBoundingClientRect();
    menu.style.left = r.left + "px";
    menu.style.top = Math.min(window.innerHeight - menu.offsetHeight - 8, r.bottom + 4) + "px";
    setTimeout(() => document.addEventListener("mousedown", function off(e) { if (!menu.contains(e.target)) { menu.remove(); document.removeEventListener("mousedown", off); } }), 0);
  };
  const draw = () => {
    if (sel >= data.scripts.length) sel = data.scripts.length - 1;
    left.innerHTML = "";
    const list = h("div", { class: "pp-list" });
    data.scripts.forEach((sc, i) => {
      list.append(h("div", { class: "pp-item" + (i === sel ? " active" : "") + (sc.known ? "" : " unknown"), onclick: () => { sel = i; draw(); } },
        h("span", { class: "grow" }, sc.label),
        h("button", { class: "ib", title: "Move up", disabled: i === 0, onclick: (e) => { e.stopPropagation(); sel = i - 1; op({ op: "move", index: i, to: i - 1 }); } }, "▲"),
        h("button", { class: "ib", title: "Move down", disabled: i === data.scripts.length - 1, onclick: (e) => { e.stopPropagation(); sel = i + 1; op({ op: "move", index: i, to: i + 1 }); } }, "▼"),
        h("button", { class: "ib", title: "Remove", onclick: (e) => { e.stopPropagation(); op({ op: "remove", index: i }); } }, "✕")));
    });
    if (!data.scripts.length) list.append(h("div", { class: "muted", style: { padding: "10px" } }, "No scripts. Scripts run on the G-code when it is saved, in this order."));
    const add = h("button", { class: "btn block", onclick: () => addMenu(add) }, "Add a script");
    left.append(h("div", { class: "section-label" }, "Post Processing Scripts"), list, add,
      h("div", { class: "muted small" }, `${data.available.length} scripts available: Cura's own plus any .py scripts in `,
        h("a", { href: "#", onclick: (e) => { e.preventDefault(); act(async () => { const r = await api.post("/api/postproc/open_folder"); toast(`Scripts folder: ${r.path}`); }); } }, "Curust's scripts folder"),
        data.folders.length > 1 ? ` or Cura's (${data.folders.slice(1).join(", ")})` : "", "."));
    // Keep the settings' scroll position when the same script is redrawn after a change.
    const oldPane = right.querySelector(".pp-settings");
    const keepScroll = oldPane && right.dataset.sel === String(sel) ? oldPane.scrollTop : 0;
    right.dataset.sel = String(sel);
    right.innerHTML = "";
    const sc = data.scripts[sel];
    if (!sc) { right.append(h("div", { class: "muted pp-empty" }, "Add a script to change the G-code: pause at a height, change temperatures or speeds at a layer, insert G-code, search and replace, time-lapse, thumbnails, and more.")); return; }
    right.append(h("div", { class: "pp-title" }, sc.label));
    if (!sc.known) { right.append(h("div", { class: "note" }, `The script ${sc.key} is not installed. It will be dropped when the dialog closes.`)); return; }
    if (sc.settings && sc.settings.error) { right.append(h("div", { class: "msg err" }, sc.settings.error)); return; }
    const def = (data.definitions[sc.key] || {}).settings || {};
    const pane = h("div", { class: "pp-settings" });
    for (const st of sc.settings) {
      if (!st.enabled) continue;
      const d = def[st.key] || {};
      const set = (v) => op({ op: "set", index: sel, setting: st.key, value: String(v) });
      let input;
      if (d.type === "bool") {
        input = h("input", { type: "checkbox", checked: st.value === true || st.string === "True", onchange: (e) => set(e.target.checked ? "True" : "False") });
      } else if (d.type === "enum") {
        input = h("select", { onchange: (e) => set(e.target.value) }, Object.entries(d.options || {}).map(([k, l]) => h("option", { value: k, selected: k === st.string }, l)));
      } else if ((d.comments || "").includes("multiline")) {
        input = h("textarea", { rows: 4, onchange: (e) => set(e.target.value) }, st.string);
      } else {
        input = h("input", { type: "text", value: st.string, onchange: (e) => set(e.target.value) });
        const num = parseFloat(st.string);
        const bad = (k, cmp) => st[k] !== undefined && st[k] !== null && !isNaN(num) && cmp(num, Number(st[k]));
        if (bad("minimum_value", (a, b) => a < b) || bad("maximum_value", (a, b) => a > b)) input.classList.add("err");
        else if (bad("minimum_value_warning", (a, b) => a < b) || bad("maximum_value_warning", (a, b) => a > b)) input.classList.add("warn");
      }
      pane.append(h("div", { class: "pp-row", title: d.description || "" }, h("label", {}, d.label || st.key), h("div", { class: "pp-in" }, input, d.unit ? h("span", { class: "muted" }, d.unit) : null)));
    }
    right.append(pane);
    pane.scrollTop = keepScroll;
  };
  modal("Post Processing Plugin", h("div", { class: "pp" }, left, right), [["Close", null, true]], "wide tall", async () => {
    try { await api.post("/api/postproc/close"); } catch (e) { toast(e.message, true); }
    await refresh();
  });
  draw();
}

// ---------------------------------------------------------------- printing (USB / network) and the Monitor stage
const mon = { timer: null, data: {}, ports: [], tab: localStorage.getItem("curust.monTab") || "usb", net: null, consoleOpen: false, draft: {} };
function printButtons() {
  const d = mon.data || {};
  const row = h("div", { class: "btn-row print-row" });
  if (d.usb && d.usb.status === "connected" && !d.usb.printing)
    row.append(h("button", { class: "btn primary", onclick: () => act(async () => { await api.post("/api/usb/print"); toast("Printing over USB"); setStage("monitor"); }) }, h("span", { class: "icon i-usb" }), "Print over USB"));
  if (d.net && !(d.net.job && d.net.job.busy))
    row.append(h("button", { class: "btn primary", onclick: () => act(async () => { await api.post("/api/net/print", { start: true }); toast(`Sending to ${d.net.label}…`); setStage("monitor"); }) }, h("span", { class: "icon i-wifi" }), `Print via ${d.net.label.split(" (")[0]}`));
  return row.childNodes.length ? row : null;
}
// The print buttons in the action panel depend on the connections; re-render it when they change.
function printSignature(d) {
  d = d || {};
  return JSON.stringify([!!(d.usb && d.usb.status === "connected" && !d.usb.printing), !!(d.net && !(d.net.job && d.net.job.busy)), d.net ? d.net.label : ""]);
}
async function pollMonitor() {
  const before = printSignature(mon.data);
  try { mon.data = await api.get("/api/monitor"); } catch (e) { mon.data = {}; }
  if (ui.stage === "monitor") renderMonitor();
  if (printSignature(mon.data) !== before) renderAction();
}
function startMonitor() {
  stopMonitor();
  api.get("/api/usb/ports").then((p) => { mon.ports = p; renderMonitor(); }).catch(() => {});
  pollMonitor();
  mon.timer = setInterval(pollMonitor, 1500);
}
function stopMonitor() { if (mon.timer) clearInterval(mon.timer); mon.timer = null; }
setInterval(() => { if (ui.stage !== "monitor") api.get("/api/monitor").then((d) => { const had = printSignature(mon.data); mon.data = d; if (had !== printSignature(d)) renderAction(); }).catch(() => {}); }, 5000);
function tempRow(label, t, set) {
  const inp = h("input", { type: "number", min: 0, max: 400, step: 1, placeholder: "°C", value: t.target ? Math.round(t.target) : "" });
  return h("div", { class: "mon-temp" }, h("span", { class: "grow" }, label), h("b", {}, `${fmt(t.current, 1)} °C`), h("span", { class: "muted" }, ` / ${fmt(t.target, 0)} °C`), inp,
    h("button", { class: "btn small", onclick: () => set(parseFloat(inp.value) || 0) }, "Set"), h("button", { class: "btn small", title: "Off", onclick: () => set(0) }, "Off"));
}
function renderMonitor() {
  const p = $("#monitor-panel");
  const d = mon.data || {};
  // Don't rebuild while the user types in (or picks from) one of the fields; what has been typed is
  // kept in mon.draft, so rebuilding later (e.g. after clicking a button) doesn't lose it.
  const ae = document.activeElement;
  if (ae && p.contains(ae) && ["INPUT", "TEXTAREA", "SELECT"].includes(ae.tagName)) return;
  const dr = mon.draft;
  const keep = (key) => ({ oninput: (e) => { dr[key] = e.target.value; }, onchange: (e) => { dr[key] = e.target.value; if (e.target.tagName === "SELECT") e.target.blur(); } });
  p.innerHTML = "";
  const tabs = h("div", { class: "tabs" }, [["usb", "USB"], ["net", "Network"]].map(([k, l]) => h("button", { class: "tab" + (mon.tab === k ? " active" : ""), onclick: () => { mon.tab = k; localStorage.setItem("curust.monTab", k); renderMonitor(); } }, l)));
  const conn = h("div", { class: "mon-conn" });
  const usb = d.usb, net = d.net;
  const send = (code) => act(async () => { if (mon.tab === "usb") await api.post("/api/usb/gcode", { gcode: code }); else await api.post("/api/net/gcode", { gcode: code }); });
  if (mon.tab === "usb") {
    const curPort = dr.port ?? (usb && usb.port);
    const curBaud = dr.baud !== undefined ? parseInt(dr.baud) : (usb ? usb.baud : 0);
    const portSel = h("select", { "data-key": "port", ...keep("port") }, mon.ports.length ? mon.ports.map((x) => h("option", { value: x.name, selected: curPort === x.name }, `${x.name} — ${x.description}`)) : h("option", { value: "" }, "No serial ports found"));
    const baudSel = h("select", { "data-key": "baud", ...keep("baud") }, h("option", { value: 0 }, "Auto-detect"), [115200, 250000, 500000, 230400, 76800, 57600, 38400, 19200, 9600].map((b) => h("option", { value: b, selected: curBaud === b }, String(b))));
    conn.append(h("div", { class: "row" }, portSel, baudSel,
      h("button", { class: "btn", title: "Refresh ports", onclick: () => api.get("/api/usb/ports").then((x) => { mon.ports = x; renderMonitor(); }) }, "↻"),
      usb && usb.status !== "closed" && usb.status !== "error"
        ? h("button", { class: "btn", onclick: () => act(async () => { await api.post("/api/usb/disconnect"); await pollMonitor(); renderAction(); }) }, "Disconnect")
        : h("button", { class: "btn primary", disabled: !mon.ports.length, onclick: () => act(async () => { await api.post("/api/usb/connect", { port: portSel.value, baud: parseInt(baudSel.value) }); await pollMonitor(); }) }, "Connect")));
    if (usb) conn.append(h("div", { class: "muted mon-status" + (usb.status === "error" ? " err" : "") }, `${usb.status === "connected" ? "Connected" : usb.status === "connecting" ? "Connecting…" : usb.status === "error" ? "Error" : "Disconnected"}${usb.message ? " · " + usb.message : ""}${usb.firmware ? " · " + usb.firmware : ""}`));
    else conn.append(h("div", { class: "muted mon-status" }, "Connect the printer with a USB cable. Curust talks to it like Cura does (Marlin, Prusa, RepRap and other G-code firmware)."));
  } else {
    const saved = (net && net.config) || mon.net || { kind: "octoprint", url: "", key: "", user: "" };
    const cfg = { kind: dr.kind ?? saved.kind, url: dr.url ?? saved.url, key: dr.key ?? saved.key, user: dr.user ?? (saved.user || "") };
    const kind = h("select", { "data-key": "kind", ...keep("kind") }, [["octoprint", "OctoPrint"], ["moonraker", "Klipper (Moonraker: Mainsail, Fluidd)"], ["prusalink", "PrusaLink"], ["duet", "Duet (RepRapFirmware)"]].map(([k, l]) => h("option", { value: k, selected: cfg.kind === k }, l)));
    const url = h("input", { type: "text", "data-key": "url", placeholder: "http://192.168.1.50 or octopi.local", value: cfg.url, ...keep("url") });
    const key = h("input", { type: "password", "data-key": "key", placeholder: "API key / password", value: cfg.key, ...keep("key") });
    const user = h("input", { type: "text", "data-key": "user", placeholder: "user (PrusaLink digest only)", value: cfg.user, ...keep("user") });
    const clearDraft = () => { for (const k of ["kind", "url", "key", "user"]) delete dr[k]; };
    const read = () => ({ kind: kind.value, url: url.value, key: key.value, user: user.value });
    conn.append(h("div", { class: "row" }, kind), h("div", { class: "row" }, url), h("div", { class: "row" }, key, user),
      h("div", { class: "row" },
        h("button", { class: "btn", onclick: () => act(async () => { const r = await api.post("/api/net/test", read()); toast(`Connected: ${r.message}`); }) }, "Test connection"),
        h("button", { class: "btn primary", onclick: () => act(async () => { const c = read(); mon.net = c; await api.post("/api/net/config", c); clearDraft(); toast("Saved for this printer"); await pollMonitor(); renderAction(); }) }, "Save"),
        net ? h("button", { class: "btn", onclick: () => act(async () => { await api.post("/api/net/config", { kind: "" }); mon.net = null; clearDraft(); await pollMonitor(); renderAction(); }) }, "Remove") : null));
    if (net && net.error) conn.append(h("div", { class: "msg err" }, net.error));
    if (net && net.job) conn.append(h("div", { class: "msg" + (net.job.error ? " err" : "") }, net.job.message));
  }
  p.append(h("div", { class: "mon-head" }, h("b", {}, "Printer connection"), tabs), conn);
  const live = mon.tab === "usb" ? (usb && usb.status === "connected" ? usb : null) : (net && net.status ? net.status : null);
  if (live) {
    const temps = h("div", { class: "mon-temps" });
    (live.hotends || []).forEach((t, i) => temps.append(tempRow(live.hotends.length > 1 ? `Extruder ${i + 1}` : "Nozzle", t, (v) => send(`M104 S${v} T${i}`))));
    temps.append(tempRow("Build plate", live.bed || { current: 0, target: 0 }, (v) => send(`M140 S${v}`)));
    const printing = !!live.printing, paused = !!live.paused;
    const name = mon.tab === "usb" ? live.job_name : live.file;
    const prog = Math.max(0, Math.min(1, live.progress || 0));
    const elapsed = live.elapsed || 0, remaining = mon.tab === "usb" ? Math.max(0, (live.estimated || 0) - elapsed) : (live.remaining || 0);
    const ctl = (action) => act(async () => { await api.post(mon.tab === "usb" ? "/api/usb/control" : "/api/net/control", { action }); await pollMonitor(); });
    const job = h("div", { class: "mon-job" },
      h("div", { class: "row" }, h("b", { class: "grow" }, printing ? (name || "Printing") : (mon.tab === "net" && live.state ? live.state : "Idle")), printing ? h("span", {}, `${Math.round(prog * 100)}%`) : null),
      h("div", { class: "progress" }, h("div", { style: { width: `${prog * 100}%` } })),
      printing ? h("div", { class: "muted" }, `${humanTime(elapsed)} elapsed · ${humanTime(remaining)} left`) : null,
      printing ? h("div", { class: "btn-row" },
        paused ? h("button", { class: "btn", onclick: () => ctl("resume") }, "Resume") : h("button", { class: "btn", onclick: () => ctl("pause") }, "Pause"),
        h("button", { class: "btn danger", onclick: async () => { if (await confirmDialog("Abort print", "Abort the print? It can't be resumed.", "Abort", "Keep printing")) ctl("cancel"); } }, "Abort")) : null);
    const jog = h("div", { class: "mon-jog" });
    let step = parseFloat(localStorage.getItem("curust.jogStep") || "10");
    const mv = (axis, dist) => send(`G91\nG0 ${axis}${dist} F${axis === "Z" ? 600 : 3000}\nG90`);
    jog.append(h("div", { class: "row" }, h("span", { class: "muted grow" }, "Move head"),
      ...[0.1, 1, 10, 100].map((v) => h("button", { class: "btn small" + (step === v ? " active" : ""), onclick: () => { localStorage.setItem("curust.jogStep", v); renderMonitor(); } }, String(v)))),
      h("div", { class: "jog-grid" },
        h("button", { class: "btn", onclick: () => mv("Y", step) }, "Y+"), h("button", { class: "btn", onclick: () => mv("Z", step) }, "Z+"),
        h("button", { class: "btn", onclick: () => mv("X", -step) }, "X−"), h("button", { class: "btn", onclick: () => send("G28 X Y") }, "⌂ XY"), h("button", { class: "btn", onclick: () => mv("X", step) }, "X+"),
        h("button", { class: "btn", onclick: () => mv("Y", -step) }, "Y−"), h("button", { class: "btn", onclick: () => mv("Z", -step) }, "Z−"),
        h("button", { class: "btn", onclick: () => send("G28") }, "⌂ All")));
    const cmd = h("input", { type: "text", "data-key": "cmd", placeholder: "Send G-code (e.g. M503)", value: dr.cmd || "", oninput: (e) => { dr.cmd = e.target.value; }, onkeydown: (e) => { if (e.key === "Enter" && e.target.value.trim()) { send(e.target.value); e.target.value = ""; dr.cmd = ""; } } });
    const consoleBox = mon.tab === "usb" ? h("pre", { class: "mon-console" }, (live.console || []).slice(-60).join("\n")) : null;
    p.append(h("div", { class: "mon-grid" }, h("div", {}, h("div", { class: "section-label" }, "Temperatures"), temps, h("div", { class: "section-label" }, "Print job"), job),
      h("div", {}, printing ? null : jog, h("div", { class: "section-label" }, "Console"), cmd, consoleBox)));
    if (consoleBox) consoleBox.scrollTop = consoleBox.scrollHeight;
  }
}

// ---------------------------------------------------------------- Marketplace
async function openMarketplace() {
  let tab = "plugin", search = "", items = [], next = "", loading = false, error = "", installed = { curust: [], cura: [], cura_dirs: [] };
  const list = h("div", { class: "mk-list" });
  const status = h("div", { class: "mk-status muted" });
  const searchEl = h("input", { type: "text", placeholder: "Search the Marketplace", onkeydown: (e) => { if (e.key === "Enter") { search = e.target.value; reload(); } } });
  const tabs = h("div", { class: "tabs" });
  const loadInstalled = async () => { installed = await api.get("/api/market/installed").catch(() => installed); };
  const drawTabs = () => {
    tabs.innerHTML = "";
    for (const [k, l] of [["plugin", "Plugins"], ["material", "Materials"], ["installed", "Installed"]]) tabs.append(h("button", { class: "tab" + (tab === k ? " active" : ""), onclick: () => { tab = k; reload(); } }, l));
  };
  const card = (p, installedHere) => {
    const icon = p.icon_url ? h("img", { src: p.icon_url, alt: "", loading: "lazy", onerror: (e) => e.target.replaceWith(h("div", { class: "mk-icon ph" }, (p.display_name || "?")[0])) }) : h("div", { class: "mk-icon ph" }, (p.display_name || "?")[0]);
    const btn = installedHere
      ? h("button", { class: "btn", onclick: () => act(async () => { await api.post("/api/market/uninstall", { package_id: p.package_id }); toast(`Removed ${p.display_name}`); await refresh(); await loadInstalled(); reload(); }) }, "Uninstall")
      : h("button", { class: "btn primary", onclick: (e) => act(async () => {
          e.target.disabled = true; e.target.textContent = "Installing…";
          const r = await api.post("/api/market/install", { package_id: p.package_id, download_url: p.download_url || "" });
          toast(r.message, !r.usable, 8000); await refresh(); await loadInstalled(); reload();
        }) }, "Install");
    const author = (p.author && p.author.display_name) || p.author_name || "";
    const tags = [];
    if (p.cura_installed) tags.push(h("span", { class: "badge" }, "installed in Cura"));
    if (p.package_type === "plugin" && !installedHere) tags.push(h("span", { class: "badge grey", title: "Plugins are Python code for Cura's interface. Curust uses the materials, printer profiles and post-processing scripts inside them." }, "plugin"));
    if (installedHere && p.curust_summary) tags.push(h("span", { class: "muted small-inline" }, p.curust_summary));
    return h("div", { class: "mk-card" }, icon,
      h("div", { class: "grow mk-text" },
        h("div", { class: "row", style: { gap: "8px", alignItems: "baseline" } }, h("b", {}, p.display_name || p.package_id), p.package_version ? h("span", { class: "muted" }, "v" + p.package_version) : null, tags),
        h("div", { class: "muted" }, [author, p.download_count ? `${p.download_count.toLocaleString()} downloads` : null].filter(Boolean).join(" · ")),
        h("div", { class: "mk-desc" }, (p.description || "").replace(/<[^>]+>/g, "")),
        p.marketplace_url ? h("a", { href: p.marketplace_url, target: "_blank", rel: "noopener" }, "Open on the Marketplace website") : null),
      btn);
  };
  const draw = () => {
    list.innerHTML = "";
    if (tab === "installed") {
      if (!installed.curust.length && !installed.cura.length) list.append(h("div", { class: "muted mk-empty" }, "No packages installed yet."));
      if (installed.curust.length) list.append(h("div", { class: "section-label" }, "Installed in Curust"));
      for (const p of installed.curust) list.append(card(p, true));
      if (installed.cura.length) list.append(h("div", { class: "section-label" }, "Installed in UltiMaker Cura (used automatically)"));
      for (const p of installed.cura) list.append(card({ ...p, cura_installed: true }, false));
      status.textContent = installed.cura_dirs.length ? `Materials and profiles from ${installed.cura_dirs.join(", ")} are loaded too.` : "";
      return;
    }
    for (const p of items) list.append(card(p, p.curust_installed));
    if (loading) list.append(h("div", { class: "muted mk-empty" }, "Loading…"));
    else if (error) list.append(h("div", { class: "msg err" }, error));
    else if (!items.length) list.append(h("div", { class: "muted mk-empty" }, "Nothing found."));
    if (next && !loading) list.append(h("button", { class: "btn block", onclick: () => more() }, "Load more"));
    status.textContent = "";
  };
  const fetchPage = async (url) => {
    loading = true; error = ""; draw();
    try {
      const q = url ? `next=${encodeURIComponent(url)}` : `type=${tab}&q=${encodeURIComponent(search)}`;
      const r = await api.get(`/api/market/search?${q}`);
      items = items.concat(r.data); next = r.next;
    } catch (e) { error = e.message; }
    loading = false; draw();
  };
  const reload = async () => { drawTabs(); items = []; next = ""; if (tab === "installed") { await loadInstalled(); draw(); } else await fetchPage(""); };
  const more = () => fetchPage(next);
  const installFile = async () => {
    const files = await pickFiles(".curapackage", true);
    for (const f of files) await act(async () => {
      const r = await fetch("/api/market/install_file", { method: "POST", body: await f.arrayBuffer() });
      const res = await r.json();
      if (!r.ok) throw new Error(res.error || r.statusText);
      toast(res.message, !res.usable, 8000);
    });
    if (files.length) { await refresh(); await loadInstalled(); reload(); }
  };
  const body = h("div", { class: "mk" },
    h("div", { class: "row mk-top" }, tabs, h("div", { class: "grow" }), h("div", { class: "searchbox" }, h("span", { class: "icon i-search" }), searchEl),
      h("button", { class: "btn", title: "Install a .curapackage file", onclick: () => installFile() }, "Install from file…")),
    list, status);
  modal("Marketplace", body, [["Close", null, true]], "wide tall");
  await loadInstalled();
  reload();
}

// ---------------------------------------------------------------- menu
function renderMenu() {
  const p = $("#menu-panel"); p.innerHTML = "";
  const item = (label, fn, extra) => h("button", { class: "menu-item", onclick: () => { p.hidden = true; fn(); } }, label, extra ? h("span", { class: "muted", style: { marginLeft: "auto" } }, extra) : null);
  p.append(
    item("Open models…", () => $("#file-input").click(), "Ctrl+O"),
    item("Export scene as 3MF…", () => export3mf()),
    item("Save G-code…", () => saveGcode()),
    h("div", { class: "menu-sep" }),
    item("Arrange all models", () => act(async () => { await api.post("/api/models/arrange"); await refresh(); }), "Ctrl+R"),
    item("Select all models", () => { ui.selected = new Set(ui.state.nodes.map((n) => n.id)); refresh(); }, "Ctrl+A"),
    item("Clear build plate", () => act(async () => { await api.post("/api/models/clear"); ui.selected.clear(); await refresh(); })),
    h("div", { class: "menu-sep" }),
    item("Add printer…", () => openAddPrinter(false)),
    item("Manage printers…", () => openManagePrinters()),
    item("Machine settings…", () => ui.state.machine ? openMachineSettings() : toast("Add a printer first")),
    item("Manage materials…", () => openManageMaterials()),
    item("Manage profiles…", () => ui.state.machine ? openManageProfiles() : toast("Add a printer first")),
    h("div", { class: "menu-sep" }),
    item("Post processing: modify G-code…", () => openPostProcessing(), (ui.state.post_processing || []).length ? `${ui.state.post_processing.length} active` : null),
    item("Marketplace…", () => openMarketplace()),
    item("Preferences…", () => openPreferences()),
    h("div", { class: "menu-sep" }),
    item("About Curust", () => openAbout()),
    item("Quit", () => { api.post("/api/quit").catch(() => {}); setTimeout(() => window.close(), 150); }),
  );
}

// ---------------------------------------------------------------- object list
$("#object-list .ol-head").onclick = () => $("#object-list").classList.toggle("collapsed");
function renderObjectList() {
  const st = ui.state;
  const body = $("#object-list .ol-body");
  $("#ol-title").textContent = st.nodes.length ? `Objects (${st.nodes.length})` : "Object list";
  body.innerHTML = "";
  const cols = extruderColors();
  const ordered = [];
  const visit = (parent) => { for (const n of st.nodes) if ((n.parent ?? null) === parent) { ordered.push(n); visit(n.id); } };
  visit(null);
  for (const n of st.nodes) if (!ordered.includes(n)) ordered.push(n);
  for (const n of ordered) {
    const c = cols[n.extruder] || [200, 200, 200];
    body.append(h("div", { class: "ol-item" + (ui.selected.has(n.id) ? " sel" : "") + (n.parent != null ? " child" : ""), onclick: (e) => selectNode(n.id, e.ctrlKey || e.shiftKey) },
      h("span", { class: "dot", style: { background: `rgb(${c.join(",")})` } }),
      h("span", { class: "nm", title: n.name }, n.name),
      n.mesh_type !== "normal" ? h("span", { class: "muted" }, n.mesh_type.replace("_mesh", "").replace("_", " ")) : null,
      n.outside ? h("span", { class: "out", title: "Outside the build volume" }, "outside") : null));
  }
}

function selectNode(id, additive) {
  if (!additive) ui.selected.clear();
  if (id !== null) { if (additive && ui.selected.has(id)) ui.selected.delete(id); else ui.selected.add(id); }
  viewer.selected = ui.selected; viewer.dirty = true;
  renderObjectList(); renderToolbar(); renderToolPanel();
}

// ---------------------------------------------------------------- toolbar and tools
$$("#toolbar button").forEach((b) => b.onclick = () => {
  const t = b.dataset.tool;
  if (t === "open") return $("#file-input").click();
  if (t === "delete") return deleteSelected();
  ui.tool = ui.tool === t ? null : t;
  renderToolbar(); renderToolPanel();
});
function renderToolbar() {
  const has = ui.selected.size > 0;
  $$("#toolbar button").forEach((b) => {
    const t = b.dataset.tool;
    if (t !== "open") b.disabled = !has || ui.stage !== "prepare";
    b.classList.toggle("active", t === ui.tool && has);
  });
}
async function deleteSelected() {
  if (!ui.selected.size) return;
  await act(async () => { await api.post("/api/model/delete", { ids: Array.from(ui.selected) }); ui.selected.clear(); await refresh(); });
}
function selNodes() { return ui.state.nodes.filter((n) => ui.selected.has(n.id)); }

function numberField(value, unit, onCommit, step = "any") {
  const inp = h("input", { type: "number", step, value: value });
  inp.onchange = () => { const v = parseFloat(inp.value); if (Number.isFinite(v)) onCommit(v); };
  inp.onkeydown = (e) => { if (e.key === "Enter") inp.blur(); };
  return h("div", { class: "unit-input grow" }, inp, unit ? h("span", { class: "u" }, unit) : null);
}

function renderToolPanel() {
  const p = $("#tool-panel");
  const nodes = selNodes();
  if (!ui.tool || !nodes.length || ui.stage !== "prepare") { p.hidden = true; return; }
  p.hidden = false;
  p.innerHTML = "";
  const n = nodes[0];
  const m = ui.state.machine;
  const tx = (pos) => api.post("/api/model/transform", { id: n.id, ...pos });
  const many = (body) => act(async () => { for (const x of nodes) await api.post("/api/model/transform", { id: x.id, ...body }); await refresh(); });
  const size = [n.max[0] - n.min[0], n.max[1] - n.min[1], n.max[2] - n.min[2]];
  if (ui.tool === "move") {
    const ox = m && !m.center_is_zero ? 0 : 0;
    p.append(h("h3", {}, "Move"),
      h("div", { class: "row" }, h("label", {}, "X"), numberField(fmt(n.position[0] + ox, 2), "mm", (v) => act(async () => { await tx({ position: [v - ox, n.position[1], n.position[2]] }); await refresh(); }))),
      h("div", { class: "row" }, h("label", {}, "Y"), numberField(fmt(-n.position[2], 2), "mm", (v) => act(async () => { await tx({ position: [n.position[0], n.position[1], -v] }); await refresh(); }))),
      h("div", { class: "row" }, h("label", {}, "Z"), numberField(fmt(n.min[1], 2), "mm", (v) => act(async () => { await tx({ position: [n.position[0], n.position[1] + (v - n.min[1]), n.position[2]] }); await refresh(); }))),
      h("div", { class: "row", style: { marginTop: "12px" } }, h("button", { class: "btn small", onclick: () => many({ drop: true }) }, "Drop to build plate"),
        h("button", { class: "btn small", onclick: () => act(async () => { await api.post("/api/models/arrange"); await refresh(); }) }, "Arrange all")),
      h("div", { class: "muted", style: { marginTop: "10px", fontSize: "11.5px" } }, "Drag a selected model to move it on the build plate."));
  } else if (ui.tool === "scale") {
    const pct = n.scale.map((s) => Math.abs(s) * 100);
    const uniform = ui.uniformScale !== false;
    const setScale = (axis, factor) => {
      const ns = n.scale.slice();
      if (uniform) for (let k = 0; k < 3; k++) ns[k] = Math.sign(ns[k] || 1) * Math.abs(ns[k]) * factor;
      else ns[axis] = Math.sign(ns[axis] || 1) * Math.abs(ns[axis]) * factor;
      act(async () => { await tx({ scale: ns }); await refresh(); });
    };
    const axes = [["X", 0, 0], ["Y", 2, 1], ["Z", 1, 2]];
    p.append(h("h3", {}, "Scale"));
    for (const [lbl, k] of axes) {
      p.append(h("div", { class: "row" }, h("label", { style: { width: "24px" } }, lbl),
        numberField(fmt(size[k], 2), "mm", (v) => size[k] > 0 && setScale(k, v / size[k])),
        numberField(fmt(pct[k], 1), "%", (v) => v > 0 && setScale(k, v / pct[k]))));
    }
    p.append(h("label", { class: "row", style: { marginTop: "10px", gap: "6px" } }, h("input", { type: "checkbox", checked: uniform, onchange: (e) => { ui.uniformScale = e.target.checked; renderToolPanel(); } }), "Uniform scaling"),
      h("div", { class: "row" }, h("button", { class: "btn small", onclick: () => act(async () => { await tx({ scale: [Math.sign(n.scale[0]) || 1, Math.sign(n.scale[1]) || 1, Math.sign(n.scale[2]) || 1] }); await refresh(); }) }, "Reset scale"),
        h("button", { class: "btn small", onclick: () => { if (!m) return; const f = Math.min((m.width - 2) / size[0], (m.height - 1) / size[1], (m.depth - 2) / size[2]); setScaleUniform(n, f); } }, "Scale to max")));
  } else if (ui.tool === "rotate") {
    const rot = (axis, deg) => many({ rotate_axis: axis, rotate_degrees: deg });
    p.append(h("h3", {}, "Rotate"));
    for (const [lbl, axis] of [["X", 0], ["Y", 2], ["Z", 1]]) {
      p.append(h("div", { class: "row" }, h("label", { style: { width: "24px" } }, lbl),
        h("button", { class: "btn small", onclick: () => rot(axis, axis === 2 ? 90 : -90) }, "−90°"),
        h("button", { class: "btn small", onclick: () => rot(axis, axis === 2 ? -90 : 90) }, "+90°"),
        (() => { const i = h("input", { type: "number", value: 15, style: { width: "56px" } }); return h("span", { class: "row", style: { gap: "4px" } }, i, h("button", { class: "btn small", onclick: () => rot(axis, axis === 2 ? -parseFloat(i.value) : parseFloat(i.value)) }, "Apply")); })()));
    }
    p.append(h("div", { class: "row", style: { marginTop: "12px" } },
      h("button", { class: "btn small" + (ui.layFlat ? " primary" : ""), onclick: () => { ui.layFlat = !ui.layFlat; renderToolPanel(); } }, ui.layFlat ? "Click a face…" : "Select face to align to plate"),
      h("button", { class: "btn small", onclick: () => many({ reset_rotation: true }) }, "Reset")));
  } else if (ui.tool === "mirror") {
    p.append(h("h3", {}, "Mirror"),
      h("div", { class: "row" }, h("button", { class: "btn", onclick: () => many({ mirror: 0 }) }, "X (left/right)"), h("button", { class: "btn", onclick: () => many({ mirror: 2 }) }, "Y (front/back)")),
      h("div", { class: "row" }, h("button", { class: "btn", onclick: () => many({ mirror: 1 }) }, "Z (top/bottom)")));
  } else if (ui.tool === "permodel") {
    renderPerModel(p, n);
  } else if (ui.tool === "blocker") {
    const count = ui.state.nodes.filter((x) => x.parent != null).length;
    p.append(h("h3", {}, "Support blocker"),
      h("div", { class: "muted", style: { lineHeight: 1.5, maxWidth: "240px" } }, "Click on a model to place a 10 mm block where no support should be generated. Click a block to remove it. Blocks move with their model and can be selected and moved like models (Ctrl+click switches to Move)."),
      h("div", { class: "row", style: { marginTop: "10px" } }, h("span", { class: "muted" }, `${count} blocker${count === 1 ? "" : "s"} in the scene`),
        count ? h("button", { class: "btn small", onclick: () => act(async () => { await api.post("/api/model/delete", { ids: ui.state.nodes.filter((x) => x.parent != null).map((x) => x.id) }); await refresh(); }) }, "Remove all") : null));
  }
}
function setScaleUniform(n, f) {
  act(async () => { await api.post("/api/model/transform", { id: n.id, scale: n.scale.map((s) => s * f) }); await refresh(); });
}

async function renderPerModel(p, n) {
  const m = ui.state.machine;
  p.append(h("h3", {}, "Per-model settings"), h("div", { class: "muted", style: { marginBottom: "8px" } }, n.name));
  const types = [["normal", "Normal model"], ["support_mesh", "Print as support"], ["infill_mesh", "Modify settings for overlaps (infill mesh)"], ["cutting_mesh", "Cutting mesh"], ["anti_overhang_mesh", "Don't support overlaps"]];
  p.append(h("div", { class: "row" }, h("label", { style: { width: "70px" } }, "Mesh type"), h("select", { class: "grow", onchange: (e) => act(async () => { await api.post("/api/model/mesh_type", { id: n.id, type: e.target.value }); await refresh(); }) },
    types.map(([v, l]) => h("option", { value: v, selected: v === n.mesh_type }, l)))));
  if (m && m.extruders.length > 1) {
    p.append(h("div", { class: "row" }, h("label", { style: { width: "70px" } }, "Extruder"), h("div", { class: "seg" }, m.extruders.map((e) =>
      h("button", { class: e.position === n.extruder ? "active" : "", onclick: () => act(async () => { for (const x of selNodes()) await api.post("/api/model/extruder", { id: x.id, extruder: e.position }); await refresh(); }) }, String(e.position + 1))))));
  }
  const box = h("div", { style: { marginTop: "10px", maxHeight: "40vh", overflow: "auto", marginLeft: "-14px", marginRight: "-14px" } });
  p.append(box);
  const data = await api.get(`/api/settings?target=o:${n.id}`).catch(() => ({ settings: [] }));
  for (const s of data.settings) {
    if (["infill_mesh", "cutting_mesh", "support_mesh", "anti_overhang_mesh"].includes(s.key)) continue;
    box.append(settingRow(s, { target: `o:${n.id}`, objectId: n.id, removable: true }));
  }
  p.append(h("button", { class: "btn small", style: { marginTop: "8px" }, onclick: () => openPickSettings(n) }, "Select settings…"));
}

async function openPickSettings(n) {
  const groups = await api.get("/api/per_mesh_settings");
  const chosen = new Set(n.settings.map(([k]) => k));
  const list = h("div", { style: { maxHeight: "55vh", overflow: "auto" } });
  const search = h("input", { type: "text", placeholder: "Filter…", style: { width: "100%", marginBottom: "10px" } });
  const draw = () => {
    const q = search.value.toLowerCase();
    list.innerHTML = "";
    for (const g of groups) {
      const items = g.settings.filter(([k, l]) => !q || k.includes(q) || l.toLowerCase().includes(q));
      if (!items.length) continue;
      list.append(h("div", { class: "section-label" }, g.label));
      for (const [k, l] of items) list.append(h("label", { class: "row", style: { padding: "2px 0" } }, h("input", { type: "checkbox", checked: chosen.has(k), onchange: (e) => e.target.checked ? chosen.add(k) : chosen.delete(k) }), l, h("span", { class: "muted" }, k)));
    }
  };
  search.oninput = draw; draw();
  modal("Select settings to customize for this model", h("div", {}, search, list), [
    ["Cancel", null],
    ["Apply", async () => {
      const before = new Set(n.settings.map(([k]) => k));
      for (const k of chosen) if (!before.has(k)) await api.post("/api/model/setting", { id: n.id, key: k, value: "" });
      for (const k of before) if (!chosen.has(k) && !["infill_mesh", "cutting_mesh", "support_mesh", "anti_overhang_mesh"].includes(k)) await api.post("/api/model/setting", { id: n.id, key: k, reset: true });
      await refresh();
    }, true],
  ]);
}

// ---------------------------------------------------------------- settings panel
let settingsRenderToken = 0;
async function renderSettings() {
  const token = ++settingsRenderToken;
  const p = $("#settings-panel");
  const m = ui.state.machine;
  if (!m) { p.hidden = true; return; }
  // The scroll position is remembered per mode rather than read from the panel: changing a
  // setting re-renders the panel twice (refresh() and the setting row), and the second render
  // would otherwise read the still-empty panel of the first and jump back to the top.
  const mode = ui.settingsMode;
  const scroll = ui.spScroll[mode] || 0;
  p.innerHTML = "";
  p.style.width = ui.panelWidth + "px";
  // Drag the left edge to resize the panel.
  const grip = h("div", { class: "sp-grip", title: "Drag to resize" });
  grip.onmousedown = (e) => {
    e.preventDefault();
    const x0 = e.clientX, w0 = p.offsetWidth;
    const move = (ev) => { ui.panelWidth = Math.max(360, Math.min(window.innerWidth - 120, w0 + (x0 - ev.clientX))); p.style.width = ui.panelWidth + "px"; };
    const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); localStorage.setItem("curust.panelWidth", String(ui.panelWidth)); };
    window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
  };
  p.append(grip);
  const setMode = (mode) => { ui.settingsMode = mode; localStorage.setItem("curust.settingsMode", mode); renderSettings(); };
  const head = h("div", { class: "sp-head" },
    h("div", { class: "title" }, h("b", {}, "Print settings"),
      h("div", { class: "seg" },
        h("button", { class: ui.settingsMode === "recommended" ? "active" : "", onclick: () => setMode("recommended") }, "Recommended"),
        h("button", { class: ui.settingsMode === "custom" ? "active" : "", onclick: () => setMode("custom") }, "Custom"))));
  p.append(head);
  const body = h("div", { class: "sp-body" });
  // Profile selector: built-in qualities and custom profiles (both modes).
  const qsel = h("select", { class: "profile-select", onchange: (e) => act(async () => {
    const v = e.target.value;
    if (v.startsWith("custom:")) await changeProfile(parseInt(v.slice(7)));
    else await changeQuality(v);
  }) });
  const builtIn = h("optgroup", { label: "Default profiles" },
    m.qualities.length ? m.qualities.map((q) => h("option", { value: q.type, selected: m.quality.profile === null && q.type === m.quality.type, disabled: !q.available }, `${q.name}${q.layer_height ? " - " + fmt(q.layer_height, 3) + "mm" : ""}${q.available ? "" : " (not available)"}`)) : h("option", {}, "Not supported"));
  qsel.append(builtIn);
  if (m.custom_profiles.length) {
    qsel.append(h("optgroup", { label: "Custom profiles" }, m.custom_profiles.map((cp) => h("option", { value: "custom:" + cp.index, selected: cp.index === m.quality.profile, disabled: !cp.available }, `${cp.name}${cp.available ? "" : " (not available)"}`))));
  }
  const profMenu = h("button", { class: "btn icon-btn", title: "Profile actions" }, h("span", { class: "icon ci-dots" }));
  profMenu.onclick = (e) => { e.stopPropagation(); openProfileMenu(profMenu); };
  const isel = h("select", { onchange: (e) => act(async () => { await api.post("/api/intent", { category: e.target.value }); await refresh(); }) },
    m.intents.map((i) => h("option", { value: i, selected: i === m.intent }, i === "default" ? "Default" : cap(i))));
  head.append(h("div", { class: "profile-row" }, h("span", { class: "muted" }, "Profile"), qsel, profMenu));
  if (m.extruders.length > 1) {
    head.append(h("div", { class: "row", style: { margin: "2px 0 8px" } }, h("span", { class: "muted" }, "Extruder"),
      h("div", { class: "seg" }, m.extruders.map((e) => h("button", { class: e.position === m.active_extruder ? "active" : "", onclick: () => act(async () => { await api.post("/api/extruder/active", { position: e.position }); await refresh(); renderSettings(); }) },
        h("span", { class: "ext-num", style: { background: e.material.color, width: "16px", height: "16px", fontSize: "10px", display: "inline-grid", marginRight: "5px", verticalAlign: "-3px" } }, String(e.position + 1)), e.material.type || e.material.name)))));
  }
  p.append(body);
  if (ui.settingsMode === "recommended") {
    renderRecommended(body, m, isel);
  } else {
    if (m.intents.length > 1) head.append(h("div", { class: "profile-row" }, h("span", { class: "muted" }, "Intent"), isel));
    const sb = h("div", { class: "searchbox" }, h("span", { class: "icon i-search" }), h("input", { type: "text", placeholder: "Search all settings", value: ui.search }));
    const presets = h("select", { title: "Setting visibility", onchange: (e) => act(async () => { await api.post("/api/prefs", { visibility_preset: e.target.value }); await refresh(); renderSettings(); }) },
      [["basic", "Basic"], ["advanced", "Advanced"], ["expert", "Expert"], ["all", "All settings"], ["custom", "Custom selection"]].map(([v, l]) => h("option", { value: v, selected: v === ui.state.prefs.visibility_preset }, l)));
    const gear = h("button", { class: "btn icon-btn", title: "Configure setting visibility…", onclick: () => openVisibilityDialog() }, h("span", { class: "icon i-cog" }));
    head.append(h("div", { class: "row" }, h("div", { class: "grow" }, sb), presets, gear));
    head.append(h("div", { class: "sp-info", id: "sp-info" }));
    let tmr;
    sb.querySelector("input").oninput = (e) => { ui.search = e.target.value; clearTimeout(tmr); tmr = setTimeout(() => renderCustom(body), 220); };
    await renderCustom(body);
    if (token !== settingsRenderToken) return; // a newer render replaced this one
  }
  const q = m.quality;
  const foot = h("div", { class: "sp-foot" },
    h("span", { class: "muted" }, q.custom ? "Settings changed" : (q.profile_name ? "Custom profile" : "")),
    h("div", { class: "row" },
      q.custom ? h("button", { class: "btn small", title: "Discard the changed settings", onclick: () => act(async () => { await api.post("/api/clear_user"); await refresh(); renderSettings(); }) }, h("span", { class: "icon i-reset", style: { width: "13px", height: "13px" } }), "Discard") : null,
      q.custom && q.profile_name ? h("button", { class: "btn small", title: `Save the changes into "${q.profile_name}"`, onclick: () => act(async () => { await api.post("/api/profile/update"); await refresh(); renderSettings(); toast(`Updated profile ${q.profile_name}`); }) }, "Update profile") : null,
      q.custom ? h("button", { class: "btn small primary", title: "Save the current settings as a custom profile", onclick: () => createProfileDialog() }, "Save as profile…") : null,
      h("button", { class: "btn small", onclick: () => { ui.settingsOpen = false; p.hidden = true; } }, "Close")));
  p.append(foot);
  body.scrollTop = scroll;
  body.addEventListener("scroll", () => { if (body.isConnected) ui.spScroll[mode] = body.scrollTop; });
}

async function changeQuality(type) {
  const m = ui.state.machine;
  if (m.quality.custom) {
    const keep = await confirmDialog("Keep your changes?", "You changed some settings. Keep them on the new profile, or discard them?", "Keep", "Discard");
    if (keep === null) { renderSettings(); return; }
    await api.post("/api/quality", { type, discard: !keep });
  } else {
    await api.post("/api/quality", { type });
  }
  await refresh(); renderSettings();
}

async function changeProfile(index) {
  const m = ui.state.machine;
  let discard = false;
  if (m.quality.custom) {
    const keep = await confirmDialog("Keep your changes?", "You changed some settings. Keep them on top of the profile, or discard them?", "Keep", "Discard");
    if (keep === null) { renderSettings(); return; }
    discard = !keep;
  }
  await api.post("/api/profile/activate", { index, discard });
  await refresh(); renderSettings();
}

function promptDialog(title, label, value, okLabel) {
  return new Promise((resolve) => {
    let done = false;
    const inp = h("input", { type: "text", value: value || "", style: { width: "100%" } });
    const md = modal(title, h("div", {}, h("div", { class: "muted", style: { marginBottom: "6px" } }, label), inp),
      [["Cancel", () => { done = true; resolve(null); }], [okLabel || "OK", () => { if (!inp.value.trim()) { toast("Enter a name", true); return false; } done = true; resolve(inp.value.trim()); }, true]], "small");
    inp.onkeydown = (e) => { if (e.key === "Enter") md.el.querySelector(".modal-foot .primary").click(); };
    setTimeout(() => { inp.focus(); inp.select(); }, 30);
    const obs = new MutationObserver(() => { if (!document.body.contains(md.el)) { obs.disconnect(); if (!done) resolve(null); } });
    obs.observe($("#modal-root"), { childList: true });
  });
}

async function createProfileDialog() {
  const m = ui.state.machine;
  const name = await promptDialog("Create profile", "Save the current settings and changes as a new custom profile, based on " + m.quality.name + ".", m.quality.profile_name ? m.quality.profile_name + " copy" : "My profile", "Create");
  if (!name) return;
  await act(async () => { await api.post("/api/profile/create", { name }); await refresh(); renderSettings(); toast(`Created profile ${name}`); });
}

function openProfileMenu(anchor) {
  const m = ui.state.machine, q = m.quality;
  document.querySelectorAll(".ctx-menu").forEach((e) => e.remove());
  const item = (label, icon, fn, disabled) => h("button", { class: "menu-item", disabled: !!disabled, onclick: () => { menu.remove(); fn(); } }, icon ? h("span", { class: "icon " + icon }) : null, label);
  const menu = h("div", { class: "ctx-menu panel" },
    item("Create profile from current settings…", "ci-save", () => createProfileDialog()),
    item(q.profile_name ? `Update "${q.profile_name}" with current changes` : "Update profile with current changes", "ci-save", () => act(async () => { await api.post("/api/profile/update"); await refresh(); renderSettings(); }), !(q.custom && q.profile_name)),
    item("Discard current changes", "i-reset", () => act(async () => { await api.post("/api/clear_user"); await refresh(); renderSettings(); }), !q.custom),
    h("div", { class: "menu-sep" }),
    item("Manage profiles…", "ci-dots", () => openManageProfiles()));
  document.body.append(menu);
  const r = anchor.getBoundingClientRect();
  menu.style.top = r.bottom + 4 + "px";
  menu.style.left = Math.max(8, r.right - menu.offsetWidth) + "px";
  setTimeout(() => document.addEventListener("mousedown", function off(e) { if (!menu.contains(e.target)) { menu.remove(); document.removeEventListener("mousedown", off); } }), 0);
}

function openManageProfiles() {
  const body = h("div", { class: "manage-profiles" });
  const md = modal("Manage profiles", body, [["Close", null]], "wide");
  const draw = () => {
    const m = ui.state.machine, q = m.quality;
    body.innerHTML = "";
    body.append(h("div", { class: "muted", style: { marginBottom: "10px" } }, `Profiles for ${m.definition_name}. Custom profiles store your changed settings on top of a default profile and can be used by every printer of this type.`));
    const list = h("div", { class: "prof-list" });
    list.append(h("div", { class: "prof-group" }, "Default profiles"));
    for (const dq of m.qualities) {
      const active = q.profile === null && dq.type === q.type;
      list.append(h("div", { class: "prof-item" + (active ? " active" : "") + (dq.available ? "" : " unavailable") },
        h("div", { class: "grow" }, h("b", {}, dq.name), h("span", { class: "muted" }, dq.layer_height ? ` · ${fmt(dq.layer_height, 3)} mm` : "")),
        active ? h("span", { class: "badge" }, "Active") : h("button", { class: "btn small", disabled: !dq.available, onclick: () => act(async () => { await changeQuality(dq.type); draw(); }) }, "Activate")));
    }
    list.append(h("div", { class: "prof-group" }, "Custom profiles"));
    if (!m.custom_profiles.length) list.append(h("div", { class: "muted", style: { padding: "8px 10px" } }, "No custom profiles yet. Change some settings and use “Create profile”."));
    for (const cp of m.custom_profiles) {
      const active = cp.index === q.profile;
      const base = (m.qualities.find((x) => x.type === cp.quality_type) || {}).name || cp.quality_type;
      list.append(h("div", { class: "prof-item" + (active ? " active" : "") + (cp.available ? "" : " unavailable") },
        h("div", { class: "grow" }, h("b", {}, cp.name), h("span", { class: "muted" }, ` · based on ${base}${cp.intent !== "default" ? " · " + cap(cp.intent) : ""} · ${cp.settings} setting${cp.settings === 1 ? "" : "s"}`)),
        active ? h("span", { class: "badge" }, "Active") : h("button", { class: "btn small", disabled: !cp.available, onclick: () => act(async () => { await changeProfile(cp.index); draw(); }) }, "Activate"),
        h("button", { class: "btn small", onclick: async () => { const n = await promptDialog("Rename profile", "New name", cp.name, "Rename"); if (n) await act(async () => { await api.post("/api/profile/rename", { index: cp.index, name: n }); await refresh(); draw(); }); } }, "Rename"),
        h("button", { class: "btn small", onclick: async () => { const n = await promptDialog("Duplicate profile", "Name of the copy", cp.name + " copy", "Duplicate"); if (n) await act(async () => { await api.post("/api/profile/duplicate", { index: cp.index, name: n }); await refresh(); draw(); }); } }, "Duplicate"),
        h("button", { class: "btn small", title: "Save this profile as a .curaprofile file (Cura can import it too)", onclick: () => downloadFrom(`/api/profile/export?index=${cp.index}`, `${cp.name}.curaprofile`, "Cura profile", "application/x-curaprofile", ".curaprofile") }, "Export"),
        h("button", { class: "btn small danger", onclick: async () => { if (await confirmDialog("Remove profile", `Remove the profile "${cp.name}"?`, "Remove", "Cancel")) await act(async () => { await api.post("/api/profile/delete", { index: cp.index }); await refresh(); draw(); }); } }, "Remove")));
    }
    body.append(list);
    body.append(h("div", { class: "row", style: { marginTop: "12px" } },
      h("button", { class: "btn", disabled: !q.custom, title: q.custom ? "" : "Change some settings first", onclick: async () => { await createProfileDialog(); draw(); } }, "Create from current settings…"),
      h("button", { class: "btn", title: "Import a profile from a .curaprofile file (exported by Cura or Curust) or from the settings stored at the end of a G-code file", onclick: async () => { await importProfileFiles(); draw(); } }, "Import…")));
  };
  draw();
  return md;
}

function renderRecommended(body, m, isel) {
  const r = m.recommended;
  const rec = h("div", { class: "rec" });
  const grid = h("div", { class: "qual-grid" }, m.qualities.map((q) => h("button", { class: "qual" + (m.quality.profile === null && q.type === m.quality.type ? " active" : ""), disabled: !q.available, title: q.name, onclick: () => act(() => changeQuality(q.type)) },
    h("b", {}, q.layer_height ? fmt(q.layer_height, 2) : "–"), h("span", {}, q.name.replace(/ quality/i, "")))));
  rec.append(h("div", {}, h("div", { class: "section-label" }, "Profiles"), grid));
  if (m.quality.profile_name) rec.append(h("div", { class: "muted" }, `Custom profile “${m.quality.profile_name}” is active.`));
  if (m.intents.length > 1) rec.append(h("div", { class: "rec-row" }, h("div", { class: "rl" }, "Intent"), isel));
  const val = h("span", { class: "val" }, `${fmt(r.infill, 0)}%`);
  const slider = h("input", { type: "range", min: 0, max: 100, step: 10, value: r.infill, oninput: (e) => { val.textContent = `${e.target.value}%`; }, onchange: (e) => act(async () => { await api.post("/api/recommended", { infill: parseFloat(e.target.value) }); await refresh(); renderSettings(); }) });
  rec.append(h("div", { class: "rec-row" }, h("div", { class: "rl" }, "Infill density"), slider, val));
  rec.append(h("label", { class: "rec-row" }, h("div", { class: "rl" }, "Gradual infill"), h("input", { type: "checkbox", checked: r.gradual_infill, onchange: (e) => act(async () => { await api.post("/api/recommended", { gradual_infill: e.target.checked }); await refresh(); renderSettings(); }) })));
  const sup = h("div", { class: "rec-row" }, h("div", { class: "rl" }, "Support"), h("input", { type: "checkbox", checked: r.support, onchange: (e) => act(async () => { await api.post("/api/recommended", { support: e.target.checked }); await refresh(); renderSettings(); }) }));
  if (r.support && m.extruders.length > 1) sup.append(h("select", { onchange: (e) => act(async () => { await api.post("/api/recommended", { support_extruder: parseInt(e.target.value) }); await refresh(); }) }, m.extruders.map((e) => h("option", { value: e.position, selected: e.position === r.support_extruder }, `Extruder ${e.position + 1}`))));
  rec.append(sup);
  rec.append(h("div", { class: "rec-row" }, h("div", { class: "rl" }, "Adhesion"), h("input", { type: "checkbox", checked: r.adhesion, onchange: (e) => act(async () => { await api.post("/api/recommended", { adhesion: e.target.checked }); await refresh(); renderSettings(); }) }), h("span", { class: "muted" }, cap(r.adhesion_type))));
  rec.append(h("button", { class: "btn block custom-cta", onclick: () => { ui.settingsMode = "custom"; localStorage.setItem("curust.settingsMode", "custom"); renderSettings(); } },
    h("span", { class: "icon i-sliders" }), h("span", { class: "grow", style: { textAlign: "left" } }, h("b", {}, "Custom settings"), h("div", { class: "muted" }, "All 600+ print settings of Cura: walls, infill, speed, travel, cooling, support, adhesion, …")), h("span", { class: "chev-r" })));
  body.append(rec);
}

let customRenderToken = 0;
async function renderCustom(body) {
  const token = ++customRenderToken;
  const preset = ui.state.prefs.visibility_preset;
  const data = await api.get(`/api/settings?preset=${encodeURIComponent(preset)}&search=${encodeURIComponent(ui.search)}`).catch((e) => { toast(e.message, true); return { categories: [] }; });
  if (token !== customRenderToken) return;
  body.innerHTML = "";
  const info = $("#sp-info");
  if (info) {
    info.innerHTML = "";
    if (ui.search) {
      const n = data.categories.reduce((a, c) => a + c.settings.length, 0);
      info.append(h("span", {}, `${n} setting${n === 1 ? "" : "s"} match “${ui.search}”`), h("span", { class: "muted" }, " · hidden ones are marked "), h("span", { class: "icon ci-eyeoff mini" }));
    } else {
      info.append(h("span", {}, `Showing ${data.visible} of ${data.total} settings`));
      if (preset !== "all") info.append(h("a", { href: "#", onclick: (e) => { e.preventDefault(); act(async () => { await api.post("/api/prefs", { visibility_preset: "all" }); await refresh(); renderSettings(); }); } }, "Show all"));
      info.append(h("span", { class: "grow" }));
      info.append(h("button", { class: "link-btn", title: "Expand all categories", onclick: () => { data.categories.forEach((c) => ui.openCats.add(c.key)); saveCats(); renderCustom(body); } }, h("span", { class: "icon ci-expand" })),
        h("button", { class: "link-btn", title: "Collapse all categories", onclick: () => { ui.openCats.clear(); saveCats(); renderCustom(body); } }, h("span", { class: "icon ci-collapse" })));
    }
  }
  for (const c of data.categories) {
    const open = ui.search ? true : ui.openCats.has(c.key);
    const cat = h("div", { class: "cat" + (open ? " open" : "") });
    const head = h("div", { class: "cat-head", title: c.description || "", onclick: () => {
      cat.classList.toggle("open");
      if (cat.classList.contains("open")) ui.openCats.add(c.key); else ui.openCats.delete(c.key);
      saveCats();
    } }, h("span", { class: `icon cat-icon ci-${c.key}` }), h("span", { class: "cat-label" }, c.label),
      c.modified ? h("span", { class: "mod-badge", title: `${c.modified} changed` }, String(c.modified)) : null,
      h("span", { class: "cat-count" }, ui.search ? `${c.settings.length}` : `${c.settings.length}/${c.total}`), h("span", { class: "chev" }));
    const cb = h("div", { class: "cat-body" });
    for (const s of c.settings) cb.append(settingRow(s, {}));
    if (!c.settings.length) cb.append(h("div", { class: "muted", style: { padding: "6px 14px" } }, "No visible settings in this category are enabled."));
    cat.append(head, cb);
    body.append(cat);
  }
  if (!data.categories.length) body.append(h("div", { class: "muted", style: { padding: "20px" } }, ui.search ? "No settings match your search." : "No settings visible."));
}
function saveCats() { localStorage.setItem("curust.openCats", JSON.stringify(Array.from(ui.openCats))); }

async function setVisibility(key, visible) {
  await act(async () => { await api.post("/api/visibility", { key, visible }); await refresh(); renderSettings(); });
}

async function copyToAllExtruders(s) {
  const m = ui.state.machine;
  await act(async () => {
    for (const e of m.extruders) await api.post("/api/setting", { key: s.key, value: s.formula ? "=" + s.formula : s.value_str === "True" || s.value_str === "False" ? s.value_str : String(s.type === "float" ? trimFloat(s.value) : s.value_str), target: "e:" + e.position });
    await refresh(); renderSettings(); toast(`Copied ${s.label} to all extruders`);
  });
}

function settingMenu(ev, s, opts) {
  ev.preventDefault();
  document.querySelectorAll(".ctx-menu").forEach((e) => e.remove());
  const m = ui.state.machine;
  const item = (label, icon, fn, disabled) => h("button", { class: "menu-item", disabled: !!disabled, onclick: () => { menu.remove(); fn(); } }, icon ? h("span", { class: "icon " + icon }) : null, label);
  const items = [];
  if (opts.objectId === undefined) {
    if (s.per_extruder && m.extruders.length > 1) items.push(item("Copy value to all extruders", "ci-copy", () => copyToAllExtruders(s)));
    if (s.user) items.push(item("Reset to profile value", "i-reset", () => opts.reset()));
    items.push(s.visible === false ? item("Keep this setting visible", "ci-eye", () => setVisibility(s.key, true)) : item("Hide this setting", "ci-eyeoff", () => setVisibility(s.key, false)));
    items.push(h("div", { class: "menu-sep" }), item("Configure setting visibility…", "i-cog", () => openVisibilityDialog()));
  } else if (s.user) {
    items.push(item("Remove override", "i-close", () => opts.reset()));
  }
  if (!items.length) return;
  const menu = h("div", { class: "ctx-menu panel" }, items);
  document.body.append(menu);
  menu.style.left = Math.min(window.innerWidth - menu.offsetWidth - 8, ev.clientX) + "px";
  menu.style.top = Math.min(window.innerHeight - menu.offsetHeight - 8, ev.clientY) + "px";
  setTimeout(() => document.addEventListener("mousedown", function off(e) { if (!menu.contains(e.target)) { menu.remove(); document.removeEventListener("mousedown", off); } }), 0);
}

async function openVisibilityDialog() {
  let preset = ui.state.prefs.visibility_preset;
  let data = await api.get(`/api/visibility_tree?preset=${encodeURIComponent(preset)}`);
  const checked = new Set();
  const load = () => { checked.clear(); for (const c of data.categories) for (const s of c.settings) if (s.visible) checked.add(s.key); };
  load();
  const search = h("input", { type: "text", placeholder: "Filter settings…", style: { width: "100%" } });
  const presetSel = h("select", {}, [["basic", "Basic"], ["advanced", "Advanced"], ["expert", "Expert"], ["all", "All settings"], ["custom", "Custom selection"]].map(([v, l]) => h("option", { value: v, selected: v === preset }, l)));
  const count = h("span", { class: "muted" });
  const list = h("div", { class: "vis-list" });
  const draw = () => {
    const q = search.value.toLowerCase();
    list.innerHTML = "";
    for (const c of data.categories) {
      const items = c.settings.filter((s) => !q || s.label.toLowerCase().includes(q) || s.key.includes(q));
      if (!items.length) continue;
      const all = items.every((s) => checked.has(s.key));
      const cbox = h("input", { type: "checkbox", checked: all, onchange: (e) => { for (const s of items) { if (e.target.checked) checked.add(s.key); else checked.delete(s.key); } presetSel.value = "custom"; draw(); } });
      list.append(h("label", { class: "vis-cat" }, cbox, h("span", { class: `icon cat-icon ci-${c.key}` }), c.label, h("span", { class: "muted", style: { marginLeft: "auto" } }, `${items.filter((s) => checked.has(s.key)).length}/${items.length}`)));
      for (const s of items) {
        list.append(h("label", { class: "vis-item", style: { paddingLeft: `${26 + (q ? 0 : s.depth) * 16}px` }, title: s.key },
          h("input", { type: "checkbox", checked: checked.has(s.key), onchange: (e) => { if (e.target.checked) checked.add(s.key); else checked.delete(s.key); presetSel.value = "custom"; count.textContent = `${checked.size} visible`; } }), s.label));
      }
    }
    count.textContent = `${checked.size} visible`;
  };
  presetSel.onchange = async () => {
    if (presetSel.value === "custom") return;
    data = await api.get(`/api/visibility_tree?preset=${encodeURIComponent(presetSel.value)}`);
    load(); draw();
  };
  search.oninput = draw;
  draw();
  const body = h("div", { class: "vis-dialog" },
    h("div", { class: "row", style: { marginBottom: "8px" } }, h("div", { class: "grow" }, search), presetSel),
    h("div", { class: "row", style: { marginBottom: "6px" } }, count, h("span", { class: "grow" }),
      h("button", { class: "btn small", onclick: () => { for (const c of data.categories) for (const s of c.settings) checked.add(s.key); presetSel.value = "custom"; draw(); } }, "Check all"),
      h("button", { class: "btn small", onclick: () => { checked.clear(); presetSel.value = "custom"; draw(); } }, "Uncheck all")),
    list);
  modal("Setting visibility", body, [["Cancel", null], ["Apply", async () => {
    if (presetSel.value !== "custom") await api.post("/api/prefs", { visibility_preset: presetSel.value });
    else await api.post("/api/prefs", { visibility_preset: "custom", custom_visible: Array.from(checked) });
    await refresh(); renderSettings();
  }, true]], "wide");
}

let tipEl = null;
function showTip(e, s) {
  hideTip();
  tipEl = h("div", { class: "tip" }, h("b", {}, s.label), s.description, h("div", { class: "muted", style: { marginTop: "4px", color: "#9aa3ad" } }, `${s.key}${s.formula ? "  =" + s.formula : ""}`), h("div", { class: "muted", style: { marginTop: "2px", color: "#9aa3ad" } }, "Right-click for more options"));
  document.body.append(tipEl);
  const r = e.target.getBoundingClientRect();
  tipEl.style.left = Math.max(8, r.left - tipEl.offsetWidth - 12) + "px";
  tipEl.style.top = Math.min(window.innerHeight - tipEl.offsetHeight - 8, r.top) + "px";
}
function hideTip() { if (tipEl) { tipEl.remove(); tipEl = null; } }

function settingRow(s, opts) {
  const target = opts.target;
  const send = (value) => act(async () => {
    if (opts.objectId !== undefined) await api.post("/api/model/setting", { id: opts.objectId, key: s.key, value });
    else await api.post("/api/setting", { key: s.key, value, target: target || undefined });
    await refresh();
    if (opts.objectId !== undefined) renderToolPanel(); else renderSettings();
  });
  const reset = () => act(async () => {
    if (opts.objectId !== undefined) await api.post("/api/model/setting", { id: opts.objectId, key: s.key, reset: true });
    else await api.post("/api/setting", { key: s.key, reset: true, target: target || undefined });
    await refresh();
    if (opts.objectId !== undefined) renderToolPanel(); else renderSettings();
  });
  opts = Object.assign({}, opts, { reset });
  const row = h("div", { class: `srow state-${s.state}` + (s.user ? " user" : "") + (s.enabled ? "" : " disabled") + (s.visible === false ? " hidden-setting" : "") });
  row.style.paddingLeft = `${14 + s.depth * 14}px`;
  row.addEventListener("contextmenu", (e) => { hideTip(); settingMenu(e, s, opts); });
  const lbl = h("span", { class: "lbl", onmouseenter: (e) => showTip(e, s), onmouseleave: hideTip }, s.label);
  const eye = s.visible === false && opts.objectId === undefined ? h("button", { class: "rst eye", title: "Hidden by the visibility preset — click to keep it visible", onclick: () => setVisibility(s.key, true) }, h("span", { class: "icon ci-eyeoff" })) : null;
  const ctl = h("div", { class: "ctl" });
  const t = s.type;
  if (t === "bool") {
    ctl.append(h("input", { type: "checkbox", checked: !!s.value, onchange: (e) => send(e.target.checked ? "True" : "False") }));
  } else if (t === "enum" || t === "extruder" || t === "optional_extruder") {
    let options = s.options;
    if (t !== "enum") {
      const m = ui.state.machine;
      options = (t === "optional_extruder" ? [["-1", "Not overridden"]] : []).concat(m.extruders.map((e) => [String(e.position), `Extruder ${e.position + 1}`]));
    }
    const cur = String(s.value);
    ctl.append(h("select", { onchange: (e) => send(e.target.value) }, options.map(([k, l]) => h("option", { value: k, selected: k === cur }, l))));
  } else if (t === "str" && /gcode|code$/.test(s.key)) {
    row.append(lbl, h("button", { class: "rst", title: "Reset", onclick: reset }, h("span", { class: "icon i-reset" })));
    const wrap = h("div", { class: "gcode-edit", style: { paddingLeft: `${14 + s.depth * 14}px` } });
    const ta = h("textarea", { spellcheck: "false" }, String(s.value ?? ""));
    ta.onchange = () => send(ta.value);
    wrap.append(ta);
    const frag = document.createDocumentFragment(); frag.append(row, wrap);
    return frag;
  } else {
    const shown = s.value_str;
    const inp = h("input", { type: "text", value: (t === "float" ? trimFloat(s.value) : shown) });
    inp.title = s.formula ? "=" + s.formula : "";
    inp.onchange = () => send(inp.value.trim());
    inp.onkeydown = (e) => { if (e.key === "Enter") inp.blur(); if (e.key === "Escape") { inp.value = shown; inp.blur(); } };
    ctl.append(h("div", { class: "unit-input" }, inp, s.unit ? h("span", { class: "u" }, s.unit) : null));
  }
  const rst = h("button", { class: "rst", title: opts.removable ? "Remove override" : "Reset to profile value", onclick: reset }, h("span", { class: "icon " + (opts.removable ? "i-close" : "i-reset") }));
  if (opts.removable) row.classList.add("user");
  row.append(...[lbl, eye, s.per_extruder && opts.objectId === undefined ? h("span", { class: "ext-badge", title: "Set for the selected extruder" }, "E" + (parseInt((s.stack || "e:0").split(":")[1]) + 1)) : null, ctl, rst].filter(Boolean));
  return row;
}
function trimFloat(v) {
  if (typeof v !== "number") return String(v ?? "");
  const r = Math.round(v * 1e6) / 1e6;
  return String(r);
}

// ---------------------------------------------------------------- action panel
function renderAction() {
  const st = ui.state, p = $("#action-panel");
  if (!st || !st.machine) { p.hidden = true; return; }
  p.hidden = false;
  const sl = st.slice;
  p.innerHTML = "";
  const jn = h("input", { type: "text", value: st.job_name, title: "Job name", onchange: (e) => act(async () => { await api.post("/api/job_name", { name: e.target.value }); await refresh(); }) });
  p.append(h("div", { class: "job-name" }, jn));
  requestAnimationFrame(() => document.documentElement.style.setProperty("--action-h", `${p.offsetHeight}px`));
  const nodes = st.nodes.filter((n) => !n.outside);
  if (sl.state === "slicing") {
    p.append(h("div", { class: "row", style: { justifyContent: "space-between" } }, h("b", {}, "Slicing…"), h("span", { class: "muted" }, `${Math.round((sl.progress || 0) * 100)}%`)),
      h("div", { class: "progress" }, h("div", { style: { width: `${Math.max(2, (sl.progress || 0) * 100)}%` } })),
      h("button", { class: "btn block", onclick: () => act(async () => { await api.post("/api/slice/cancel"); await refresh(); }) }, "Cancel"));
  } else if (sl.state === "done") {
    const res = h("div", { class: "result" });
    res.append(h("div", { class: "r" }, h("span", { class: "icon i-clock" }), h("b", {}, humanTime(sl.print_time))));
    const lens = sl.material_lengths.map((l, i) => ({ l, w: sl.material_weights[i], c: sl.material_costs[i], n: sl.material_names[i] })).filter((x) => x.l > 0 || x.w > 0);
    res.append(h("div", { class: "r" }, h("span", { class: "icon i-spool" }),
      h("span", {}, lens.length ? lens.map((x) => `${fmt(x.w, 0)} g · ${fmt(x.l, 2)} m`).join(" + ") : "–",
        lens.some((x) => x.c > 0) ? h("span", { class: "muted" }, `  ${sl.currency}${fmt(lens.reduce((a, x) => a + x.c, 0), 2)}`) : null)));
    res.append(h("div", { class: "muted" }, `${sl.layer_count} layers · ${(sl.gcode_size / 1048576).toFixed(1)} MB G-code`));
    p.append(res,
      h("div", { class: "btn-row" },
        h("button", { class: "btn", onclick: () => setStage("preview") }, "Preview"),
        ppButton(),
        h("button", { class: "btn primary", onclick: () => saveGcode() }, "Save to Disk")));
    const pb = printButtons();
    if (pb) p.append(pb);
  } else {
    if (sl.state === "unable") p.append(h("div", { class: "msg" }, h("span", { class: "icon i-warn", style: { width: "14px", height: "14px", marginRight: "6px", verticalAlign: "-2px" } }), sl.message));
    if (sl.state === "error") p.append(h("div", { class: "msg err" }, sl.message));
    if (!st.machine.quality.supported) p.append(h("div", { class: "msg" }, "This material/nozzle combination has no profile. Choose another material."));
    p.append(h("button", { class: "btn primary big block", disabled: nodes.length === 0, onclick: () => slice() }, "Slice"));
    if (!st.nodes.length) p.append(h("div", { class: "muted", style: { marginTop: "8px", textAlign: "center" } }, "Load a model to slice it."));
    else if (!nodes.length) p.append(h("div", { class: "muted", style: { marginTop: "8px", textAlign: "center" } }, "All models are outside the printable area."));
  }
}
function humanTime(sec) {
  const d = Math.floor(sec / 86400), hh = Math.floor((sec % 86400) / 3600), mm = Math.floor((sec % 3600) / 60);
  return (d ? `${d} day${d > 1 ? "s" : ""} ` : "") + (d || hh ? `${hh} hour${hh !== 1 ? "s" : ""} ` : "") + `${mm} minute${mm !== 1 ? "s" : ""}`;
}
async function slice() { await act(async () => { await api.post("/api/slice"); await refresh(); }); }

// Saves a server resource with the system's Save dialog (File System Access API in Edge /
// Chrome). Returns false when that API is not available, so the caller can fall back.
async function saveWithSystemDialog(url, suggestedName, description, mime, ext) {
  if (!window.showSaveFilePicker) return false;
  let handle;
  try {
    handle = await window.showSaveFilePicker({ suggestedName, id: "curust-" + ext.slice(1), types: [{ description, accept: { [mime]: [ext] } }] });
  } catch (e) {
    if (e && e.name === "AbortError") return true; // cancelled by the user
    return false;
  }
  try {
    const r = await fetch(url);
    if (!r.ok) throw new Error(await r.text());
    const w = await handle.createWritable();
    await w.write(await r.blob());
    await w.close();
    toast(`Saved ${handle.name}`);
  } catch (e) {
    toast(`Unable to save ${handle.name}: ${e.message || e}`, true);
  }
  return true;
}

/** Let the user pick files (an <input type=file> click). */
function pickFiles(accept, multiple = false) {
  return new Promise((resolve) => {
    const inp = h("input", { type: "file", accept, multiple, style: { display: "none" } });
    inp.addEventListener("change", () => { resolve([...inp.files]); inp.remove(); });
    inp.addEventListener("cancel", () => { resolve([]); inp.remove(); });
    document.body.append(inp);
    inp.click();
  });
}

/** Save a server file: the system Save dialog when available, else a browser download. */
async function downloadFrom(url, suggestedName, description, mime, ext) {
  if (await saveWithSystemDialog(url, suggestedName, description, mime, ext)) return;
  const a = h("a", { href: url, download: suggestedName, style: { display: "none" } });
  document.body.append(a);
  a.click();
  a.remove();
}

/** Preferences > Profiles > Import (`.curaprofile`, or the profile stored in a G-code file). */
async function importProfileFiles() {
  const files = await pickFiles(".curaprofile,.gcode", true);
  for (const f of files) {
    const r = await fetch(`/api/profile/import?name=${encodeURIComponent(f.name)}`, { method: "POST", body: await f.arrayBuffer() });
    const res = r.ok ? await r.json() : { status: "error", message: await r.text() };
    toast(res.message, res.status === "error");
  }
  if (files.length) await refresh();
}

async function export3mf() {
  if (!ui.state.nodes.length) { toast("The build plate is empty"); return; }
  if (await saveWithSystemDialog("/api/export_3mf", `${ui.state.job_name}.3mf`, "3MF project", "model/3mf", ".3mf")) return;
  location.href = "/api/export_3mf";
}

// Cura shows the post-processing scripts next to the save button.
function ppButton() {
  const n = (ui.state.post_processing || []).length;
  if (!n) return null;
  return h("button", { class: "btn pp-chip", title: `Post-processing scripts run when the G-code is saved:\n${ui.state.post_processing.join("\n")}`, onclick: () => openPostProcessing() }, h("span", { class: "icon i-script" }), String(n));
}
async function saveGcode() {
  if (ui.state.slice.state !== "done") { toast("Slice first"); return; }
  if (await saveWithSystemDialog("/api/gcode", `${ui.state.job_name}.gcode`, "G-code", "text/x-gcode", ".gcode")) { setTimeout(refresh, 300); return; }
  const def = await api.get("/api/default_dir").catch(() => ({ dir: "" }));
  const dir = h("input", { type: "text", value: localStorage.getItem("curust.saveDir") || def.dir, style: { width: "100%" } });
  const name = h("input", { type: "text", value: `${ui.state.job_name}.gcode`, style: { width: "100%" } });
  modal("Save G-code", h("div", {},
    h("div", { class: "row" }, h("label", {}, "Folder"), h("div", { class: "grow" }, dir)),
    h("div", { class: "row" }, h("label", {}, "File name"), h("div", { class: "grow" }, name)),
    h("div", { class: "muted", style: { marginTop: "10px" } }, "Or ", h("a", { href: "/api/gcode", download: `${ui.state.job_name}.gcode` }, "download it"), " with the browser.")), [
    ["Cancel", null],
    ["Save", async () => {
      localStorage.setItem("curust.saveDir", dir.value);
      const r = await api.post("/api/save_file", { dir: dir.value, name: name.value });
      toast(`Saved ${r.path}`);
      setTimeout(refresh, 300);
    }, true],
  ]);
}

// ---------------------------------------------------------------- stages and preview
$$(".stages button").forEach((b) => b.onclick = () => setStage(b.dataset.stage));
function setStage(s) {
  ui.stage = s;
  $$(".stages button").forEach((b) => b.classList.toggle("active", b.dataset.stage === s));
  viewer.mode = s;
  viewer.dirty = true;
  $("#preview-panel").hidden = s !== "preview";
  $("#monitor-panel").hidden = s !== "monitor";
  if (s === "monitor") startMonitor(); else stopMonitor();
  $("#layer-slider").hidden = s !== "preview";
  $("#toolbar").hidden = s !== "prepare";
  $("#tool-panel").hidden = s !== "prepare" || !ui.tool;
  $("#empty-hint").hidden = !(s === "prepare" && ui.state?.machine && ui.state.nodes.length === 0);
  if (s === "preview") {
    if (ui.state.slice.state !== "done" && ui.state.slice.state !== "slicing") slice();
    syncPreview();
  }
  renderToolbar();
}

async function syncPreview() {
  const sl = ui.state.slice;
  const p = $("#preview-panel");
  if (sl.state !== "done") {
    viewer.clearLayers(); ui.layersFor = -1;
    p.innerHTML = "";
    p.append(h("b", {}, "Preview"), h("div", { class: "muted", style: { marginTop: "6px" } }, sl.state === "slicing" ? "Slicing…" : "Slice to see the layer preview."));
    $("#layer-slider").hidden = true;
    return;
  }
  $("#layer-slider").hidden = false;
  if (ui.layersFor !== ui.state.generation) {
    ui.layersFor = ui.state.generation;
    const buf = await api.get("/api/layers");
    viewer.colorScheme = ui.colorScheme;
    viewer.setLayers(buf, ui.state.machine, extruderColors());
    setupLayerSlider();
  }
  renderPreviewPanel();
}

function renderPreviewPanel() {
  const p = $("#preview-panel");
  p.innerHTML = "";
  const schemes = [["type", "Line type"], ["material", "Material color"], ["speed", "Speed"], ["width", "Line width"]];
  p.append(h("div", { class: "row" }, h("b", { class: "grow" }, "Color scheme"),
    h("select", { onchange: async (e) => { ui.colorScheme = e.target.value; ui.layersFor = -1; await syncPreview(); } }, schemes.map(([v, l]) => h("option", { value: v, selected: v === ui.colorScheme }, l)))));
  const legend = h("div", { class: "legend" });
  if (ui.colorScheme === "type") {
    for (const t of [1, 2, 3, 6, 4, 10, 5, 11]) legend.append(h("div", { class: "li" }, h("span", { class: "sw", style: { background: `rgb(${LINE_TYPE_COLORS[t].join(",")})` } }), LINE_TYPE_NAMES[t]));
  } else if (ui.colorScheme === "material") {
    ui.state.machine.extruders.forEach((e) => legend.append(h("div", { class: "li" }, h("span", { class: "sw", style: { background: e.material.color } }), `${e.position + 1}: ${e.material.name}`)));
  } else {
    const s = viewer.layerStats || {};
    const lo = ui.colorScheme === "speed" ? `${fmt(s.minFeed, 0)} mm/s` : `${fmt(s.minW, 2)} mm`;
    const hi = ui.colorScheme === "speed" ? `${fmt(s.maxFeed, 0)} mm/s` : `${fmt(s.maxW, 2)} mm`;
    legend.append(h("div", { class: "gradient", style: { background: `linear-gradient(90deg, ${[0, .25, .5, .75, 1].map((k) => `rgb(${heat(k).join(",")})`).join(",")})` } }),
      h("div", { class: "row", style: { justifyContent: "space-between" } }, h("span", { class: "muted" }, lo), h("span", { class: "muted" }, hi)));
  }
  p.append(legend);
  p.append(h("label", { class: "row", style: { marginTop: "10px", gap: "6px" } }, h("input", { type: "checkbox", checked: viewer.showTravel, onchange: (e) => { viewer.showTravel = e.target.checked; viewer.dirty = true; } }), "Show travel moves"));
  const L = viewer.layers;
  if (L && L.length) {
    const cur = L[viewer.layerMax];
    p.append(h("div", { class: "muted", style: { marginTop: "8px" } }, `Layer ${viewer.layerMax + 1} of ${L.length} · Z ${fmt(cur.z, 2)} mm`));
  }
}

function setupLayerSlider() {
  const L = viewer.layers || [];
  const r = $("#ls-range"), mx = $("#ls-max-input"), mn = $("#ls-min-input");
  r.min = 1; r.max = Math.max(1, L.length); r.value = L.length;
  mx.max = L.length; mx.value = L.length; mn.max = L.length; mn.value = 1;
  viewer.layerMax = L.length - 1; viewer.layerMin = 0;
  const upd = () => { viewer.layerMax = Math.max(0, Math.min(L.length - 1, parseInt(r.value) - 1)); mx.value = viewer.layerMax + 1; viewer.dirty = true; renderPreviewPanel(); };
  r.oninput = upd;
  mx.onchange = () => { r.value = Math.max(1, Math.min(L.length, parseInt(mx.value) || 1)); upd(); };
  mn.onchange = () => { viewer.layerMin = Math.max(0, Math.min(viewer.layerMax, (parseInt(mn.value) || 1) - 1)); mn.value = viewer.layerMin + 1; viewer.dirty = true; };
}

// ---------------------------------------------------------------- viewport interaction
$$("#view-buttons button").forEach((b) => b.onclick = () => viewer.setView(b.dataset.view));
const canvas = $("#view");
let drag = null;
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("mousedown", (e) => {
  canvas.focus();
  const mode = e.button === 2 ? "orbit" : e.button === 1 || (e.button === 0 && e.shiftKey) ? "pan" : "left";
  drag = { mode, x: e.clientX, y: e.clientY, moved: false };
  if (mode === "left" && ui.stage === "prepare") {
    const id = viewer.pick(e.clientX, e.clientY);
    if (ui.tool === "blocker" && !e.ctrlKey && id !== null) {
      // SupportEraser: a click on a blocker removes it; a click on a model adds one there.
      drag = null;
      const n = ui.state.nodes.find((x) => x.id === id);
      if (n && n.parent != null) { act(async () => { await api.post("/api/model/support_blocker", { id }); await refresh(); }); return; }
      if (n && n.mesh_type !== "normal") { toast("Support blockers can only be added to normal models"); return; }
      const pt = viewer.pickPoint(id, e.clientX, e.clientY);
      if (pt) act(async () => { await api.post("/api/model/support_blocker", { id, position: pt }); await refresh(); });
      return;
    }
    if (ui.tool === "blocker" && e.ctrlKey) { ui.tool = "move"; renderToolbar(); renderToolPanel(); }
    if (ui.layFlat && id !== null) {
      const nrm = viewer.pickFace(id, e.clientX, e.clientY);
      ui.layFlat = false;
      drag = null;
      if (nrm) layFlat(id, nrm);
      return;
    }
    if (id !== null) {
      if (!ui.selected.has(id)) selectNode(id, e.ctrlKey);
      const n = ui.state.nodes.find((x) => x.id === id);
      const hit = viewer.rayPlane(e.clientX, e.clientY, n ? Math.max(0, n.min[1]) : 0);
      drag.node = n; drag.start = hit; drag.startPos = n.position.slice();
    } else {
      drag.mode = "orbit-or-deselect";
    }
  }
});
window.addEventListener("mousemove", (e) => {
  if (!drag) return;
  const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
  if (Math.abs(dx) + Math.abs(dy) > 2) drag.moved = true;
  if (drag.mode === "orbit" || drag.mode === "orbit-or-deselect") { viewer.orbit(dx, dy); drag.x = e.clientX; drag.y = e.clientY; }
  else if (drag.mode === "pan") { viewer.pan(dx, dy); drag.x = e.clientX; drag.y = e.clientY; }
  else if (drag.mode === "left" && drag.node && drag.start && drag.moved) {
    const hit = viewer.rayPlane(e.clientX, e.clientY, drag.start[1]);
    if (!hit) return;
    const n = drag.node;
    const nx = drag.startPos[0] + hit[0] - drag.start[0], nz = drag.startPos[2] + hit[2] - drag.start[2];
    drag.newPos = [nx, drag.startPos[1], nz];
    const w = n.world.slice(); w[3] = nx; w[11] = nz;
    viewer.dragOverride = { id: n.id, world: w }; viewer.dirty = true;
  }
});
window.addEventListener("mouseup", (e) => {
  if (!drag) return;
  const d = drag; drag = null;
  if (d.mode === "orbit-or-deselect" && !d.moved && !e.ctrlKey) selectNode(null, false);
  if (d.mode === "left" && d.node && d.newPos) {
    act(async () => {
      const others = selNodes().filter((x) => x.id !== d.node.id);
      const ddx = d.newPos[0] - d.startPos[0], ddz = d.newPos[2] - d.startPos[2];
      await api.post("/api/model/transform", { id: d.node.id, position: d.newPos });
      for (const o of others) await api.post("/api/model/transform", { id: o.id, position: [o.position[0] + ddx, o.position[1], o.position[2] + ddz] });
      viewer.dragOverride = null;
      await refresh();
    });
  }
});
canvas.addEventListener("wheel", (e) => { e.preventDefault(); viewer.zoom(e.deltaY, e.clientX, e.clientY); }, { passive: false });

function layFlat(id, nrm) {
  // Rotation taking the face normal to -Y (Rodrigues).
  const a = nrm, b = [0, -1, 0];
  const v = [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]];
  const c = a[0]*b[0]+a[1]*b[1]+a[2]*b[2];
  let R;
  if (c < -0.999999) R = [1, 0, 0, 0, -1, 0, 0, 0, -1];
  else {
    const k = 1 / (1 + c);
    R = [v[0]*v[0]*k + c, v[0]*v[1]*k - v[2], v[0]*v[2]*k + v[1],
         v[1]*v[0]*k + v[2], v[1]*v[1]*k + c, v[1]*v[2]*k - v[0],
         v[2]*v[0]*k - v[1], v[2]*v[1]*k + v[0], v[2]*v[2]*k + c];
  }
  act(async () => { await api.post("/api/model/transform", { id, orientation: R, drop: true }); await refresh(); renderToolPanel(); });
}

document.addEventListener("keydown", (e) => {
  if (e.target.matches("input, textarea, select")) return;
  const k = e.key.toLowerCase();
  if ((e.key === "Delete" || e.key === "Backspace") && ui.stage === "prepare") { e.preventDefault(); deleteSelected(); }
  else if (e.ctrlKey && k === "a") { e.preventDefault(); ui.selected = new Set(ui.state.nodes.map((n) => n.id)); selectNode(null, true); }
  else if (e.ctrlKey && k === "o") { e.preventDefault(); $("#file-input").click(); }
  else if (e.ctrlKey && k === "r") { e.preventDefault(); act(async () => { await api.post("/api/models/arrange"); await refresh(); }); }
  else if (e.ctrlKey && k === "d" && ui.selected.size) { e.preventDefault(); act(async () => { for (const id of ui.selected) await api.post("/api/model/duplicate", { id, count: 1 }); await refresh(); }); }
  else if (!e.ctrlKey && ui.selected.size && ["t", "s", "r", "m", "e"].includes(k)) { ui.tool = { t: "move", s: "scale", r: "rotate", m: "mirror", e: "blocker" }[k]; renderToolbar(); renderToolPanel(); }
  else if (e.key === "Escape") { ui.tool = null; ui.layFlat = false; closePopovers(); renderToolbar(); renderToolPanel(); }
  else if (ui.stage === "preview" && (e.key === "ArrowUp" || e.key === "ArrowDown") && viewer.layers) {
    e.preventDefault();
    const r = $("#ls-range"); r.value = Math.max(1, Math.min(viewer.layers.length, parseInt(r.value) + (e.key === "ArrowUp" ? 1 : -1))); r.oninput();
  }
});

// ---------------------------------------------------------------- file loading
const IMAGE_EXT = /\.(png|jpe?g|bmp|gif)$/i;

/** Cura's image import dialog (ImageReader): relief / lithophane options. */
function imageOptionsDialog(file) {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(file);
    const im = new Image();
    im.onload = async () => {
      // The pixel size as Qt reads it (browsers apply EXIF rotation, Cura does not).
      const info = await fetch("/api/image_info", { method: "POST", body: await file.arrayBuffer() }).then((r) => r.json()).catch(() => null);
      if (info && info.width) { Object.defineProperty(im, "naturalWidth", { value: info.width }); Object.defineProperty(im, "naturalHeight", { value: info.height }); }
      let saved = {};
      try { saved = JSON.parse(localStorage.getItem("curust.imageOptions") || "{}"); } catch (e) { /* ignore */ }
      const o = Object.assign({ peak: 2.5, base: 0.4, smoothing: 1, lighter_is_higher: false, transparency: false, transmittance: 50 }, saved);
      const largest = Math.max(im.naturalWidth, im.naturalHeight) || 1;
      const aspect = im.naturalWidth / (im.naturalHeight || 1);
      let width = im.naturalWidth / largest * 120, depth = im.naturalHeight / largest * 120;
      const num = (v, step = "0.1") => h("input", { type: "number", step, value: Math.round(v * 1000) / 1000 });
      const peak = num(o.peak), base = num(o.base), wIn = num(width), dIn = num(depth), trans = num(o.transmittance, "1");
      wIn.oninput = () => { width = parseFloat(wIn.value) || 0; depth = width / aspect; dIn.value = Math.round(depth * 1000) / 1000; };
      dIn.oninput = () => { depth = parseFloat(dIn.value) || 0; width = depth * aspect; wIn.value = Math.round(width * 1000) / 1000; };
      const invert = h("select", {}, h("option", { value: "0", selected: !o.lighter_is_higher }, "Darker is higher"), h("option", { value: "1", selected: o.lighter_is_higher }, "Lighter is higher"));
      const model = h("select", {}, h("option", { value: "0", selected: !o.transparency }, "Linear"), h("option", { value: "1", selected: o.transparency }, "Translucency"));
      const transRow = h("div", { class: "mm-row", title: "The percentage of light penetrating a print with a thickness of 1 millimeter. Lowering this value increases the contrast in dark regions and decreases the contrast in light regions of the image." }, h("label", {}, "1 mm transmittance"), trans, h("span", { class: "muted" }, "%"));
      const showTrans = () => { transRow.style.display = model.value === "1" ? "" : "none"; };
      model.onchange = showTrans; showTrans();
      const smoothVal = h("span", { class: "val" }, String(o.smoothing));
      const smooth = h("input", { type: "range", min: 0, max: 100, step: 1, value: o.smoothing, oninput: (e) => { smoothVal.textContent = e.target.value; } });
      const row = (label, el, unit, tip) => h("div", { class: "mm-row", title: tip || "" }, h("label", {}, label), el, unit ? h("span", { class: "muted" }, unit) : null);
      const body = h("div", { class: "image-dialog" },
        h("div", { class: "row", style: { gap: "14px", alignItems: "flex-start" } },
          h("img", { src: url, style: { width: "120px", maxHeight: "120px", objectFit: "contain", borderRadius: "6px", border: "1px solid var(--line)" } }),
          h("div", { class: "grow" },
            row("Height", peak, "mm", "The maximum distance of each pixel from \"Base.\""),
            row("Base", base, "mm", "The base height from the build plate in millimeters."),
            row("Width", wIn, "mm", "The width in millimeters on the build plate"),
            row("Depth", dIn, "mm", "The depth in millimeters on the build plate"))),
        row("", invert, null, "For lithophanes dark pixels should correspond to thicker locations in order to block more light coming through. For height maps lighter pixels signify higher terrain, so lighter pixels should correspond to thicker locations in the generated 3D model."),
        row("Color model", model, null, "For lithophanes a simple logarithmic model for translucency is available. For height maps the pixel values correspond to heights linearly."),
        transRow,
        row("Smoothing", smooth, null, "The amount of smoothing to apply to the image."), h("div", { class: "muted", style: { textAlign: "right" } }, smoothVal));
      let done = false;
      const finish = (v) => { if (!done) { done = true; URL.revokeObjectURL(url); resolve(v); } };
      const md = modal(`Convert image: ${file.name}`, body, [["Cancel", () => finish(null)], ["OK", () => {
        const r = { peak: parseFloat(peak.value) || 0, base: parseFloat(base.value) || 0, size: Math.max(width, depth), smoothing: parseInt(smooth.value) || 0,
          lighter_is_higher: invert.value === "1", transparency: model.value === "1", transmittance: parseFloat(trans.value) || 50 };
        try { localStorage.setItem("curust.imageOptions", JSON.stringify({ peak: r.peak, base: r.base, smoothing: r.smoothing, lighter_is_higher: r.lighter_is_higher, transparency: r.transparency, transmittance: r.transmittance })); } catch (e) { /* ignore */ }
        finish(r);
      }, true]], "small sticky");
      const obs = new MutationObserver(() => { if (!document.body.contains(md.el)) { obs.disconnect(); finish(null); } });
      obs.observe($("#modal-root"), { childList: true });
    };
    im.onerror = () => { URL.revokeObjectURL(url); toast(`${file.name}: the image is corrupt or in an unsupported format`, true); resolve(null); };
    im.src = url;
  });
}

async function loadFiles(files) {
  if (!ui.state?.machine) { toast("Add a printer first"); return; }
  for (const f of files) {
    let query = "";
    if (IMAGE_EXT.test(f.name)) {
      const o = await imageOptionsDialog(f);
      if (!o) continue;
      query = `&size=${o.size}&peak=${o.peak}&base=${o.base}&smoothing=${o.smoothing}&lighter_is_higher=${o.lighter_is_higher ? 1 : 0}&transparency=${o.transparency ? 1 : 0}&transmittance=${o.transmittance}`;
    }
    const t = toastPersistent(`Loading ${f.name}…`);
    try {
      const data = await f.arrayBuffer();
      const r = await fetch(`/api/models/upload?name=${encodeURIComponent(f.name)}${query}`, { method: "POST", body: data });
      const body = await r.json();
      if (!r.ok) throw new Error(body.error || r.statusText);
      ui.selected = new Set(body.ids);
    } catch (e) { toast(`${f.name}: ${e.message}`, true); }
    t.remove();
  }
  if (ui.stage !== "prepare") setStage("prepare");
  await refresh();
}
function toastPersistent(msg) { const t = h("div", { class: "toast" }, msg); $("#toasts").append(t); return t; }
$("#hint-card").onclick = () => $("#file-input").click();
$("#hint-card").onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); $("#file-input").click(); } };
$("#file-input").onchange = (e) => { loadFiles(Array.from(e.target.files)); e.target.value = ""; };
let dragDepth = 0;
window.addEventListener("dragenter", (e) => { if (e.dataTransfer?.types?.includes("Files")) { dragDepth++; $("#drop-overlay").hidden = false; } });
window.addEventListener("dragleave", () => { dragDepth = Math.max(0, dragDepth - 1); if (!dragDepth) $("#drop-overlay").hidden = true; });
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => { e.preventDefault(); dragDepth = 0; $("#drop-overlay").hidden = true; loadFiles(Array.from(e.dataTransfer.files)); });

// ---------------------------------------------------------------- dialogs
function modal(title, content, buttons, cls = "", onClose = null) {
  const back = h("div", { class: "modal-back" });
  let closed = false;
  const close = () => { if (closed) return; closed = true; back.remove(); if (onClose) onClose(); };
  const foot = h("div", { class: "modal-foot" });
  for (const [label, fn, primary] of buttons || [["Close", null]]) {
    foot.append(h("button", { class: "btn" + (primary ? " primary" : ""), onclick: async () => {
      if (fn) { try { const keep = await fn(); if (keep === false) return; } catch (e) { toast(e.message, true); return; } }
      close();
    } }, label));
  }
  const m = h("div", { class: "modal " + cls }, h("div", { class: "modal-head" }, h("b", {}, title), h("button", { onclick: close }, h("span", { class: "icon i-close" }))), h("div", { class: "modal-body" }, content), foot);
  back.append(m);
  back.addEventListener("mousedown", (e) => { if (e.target === back && !cls.includes("sticky")) close(); });
  $("#modal-root").append(back);
  return { close, el: m };
}
function confirmDialog(title, text, yes, no) {
  return new Promise((resolve) => {
    let done = false;
    const md = modal(title, h("p", {}, text), [[no, () => { done = true; resolve(false); }], [yes, () => { done = true; resolve(true); }, true]], "small");
    const obs = new MutationObserver(() => { if (!document.body.contains(md.el)) { obs.disconnect(); if (!done) resolve(null); } });
    obs.observe($("#modal-root"), { childList: true });
  });
}

async function openAddPrinter(first) {
  const defs = await api.get("/api/definitions");
  let chosen = null;
  const name = h("input", { type: "text", placeholder: "Printer name", style: { width: "100%" } });
  const info = h("div", { class: "muted", style: { marginTop: "10px", lineHeight: 1.5 } }, "Select a printer from the list.");
  const list = h("div", { class: "def-list" });
  const search = h("input", { type: "text", placeholder: "Search printers…", style: { width: "100%", marginBottom: "8px" } });
  const draw = () => {
    const q = search.value.toLowerCase();
    list.innerHTML = "";
    const groups = new Map();
    for (const [id, nm, mf] of defs) {
      if (q && !(`${nm} ${mf} ${id}`.toLowerCase().includes(q))) continue;
      if (!groups.has(mf)) groups.set(mf, []);
      groups.get(mf).push([id, nm]);
    }
    for (const [mf, items] of groups) {
      const g = h("div", { class: "def-group" + (q || mf === "UltiMaker" || mf === "Creality3D" ? " open" : "") },
        h("div", { class: "dg-head", onclick: () => g.classList.toggle("open") }, mf, h("span", { class: "muted" }, String(items.length))),
        h("div", { class: "dg-items" }, items.map(([id, nm]) => h("div", { class: "def-item" + (chosen === id ? " sel" : ""), onclick: (e) => {
          chosen = id; $$(".def-item.sel", list).forEach((x) => x.classList.remove("sel")); e.target.classList.add("sel");
          name.value = nm; info.textContent = `${nm} (${id})`;
        } }, nm))));
      list.append(g);
    }
  };
  search.oninput = draw; draw();
  modal(first ? "Welcome to Curust — add your printer" : "Add a printer", h("div", { class: "add-printer" },
    h("div", {}, search, list),
    h("div", {}, h("div", { class: "section-label" }, "Printer name"), name, info,
      h("div", { class: "muted", style: { marginTop: "20px", lineHeight: 1.5 } }, "Curust uses Cura's printer definitions and profiles, so every printer Cura supports can be added here."))), [
    ["Cancel", null],
    ["Add", async () => {
      if (!chosen) { toast("Select a printer"); return false; }
      await api.post("/api/machine/add", { definition: chosen, name: name.value });
      await refresh();
      viewer.setView("3d");
    }, true],
  ], first ? "sticky" : "");
}

function openManagePrinters() {
  const st = ui.state;
  const body = h("div", {});
  const draw = () => {
    body.innerHTML = "";
    const tbl = h("table", { class: "plain" }, h("tr", {}, h("th", {}, "Name"), h("th", {}, "Printer"), h("th", {}, "")));
    ui.state.machines.forEach((m, i) => {
      const nm = h("input", { type: "text", value: m.name, onchange: (e) => act(async () => { await api.post("/api/machine/rename", { index: i, name: e.target.value }); await refresh(); draw(); }) });
      tbl.append(h("tr", {}, h("td", {}, nm), h("td", { class: "muted" }, m.definition_name), h("td", { style: { textAlign: "right", whiteSpace: "nowrap" } },
        h("button", { class: "btn small", disabled: i === ui.state.active, onclick: () => act(async () => { await api.post("/api/machine/select", { index: i }); await refresh(); draw(); }) }, i === ui.state.active ? "Active" : "Activate"), " ",
        h("button", { class: "btn small danger", onclick: () => act(async () => { if (!(await confirmDialog("Remove printer", `Remove ${m.name}?`, "Remove", "Cancel"))) return; await api.post("/api/machine/remove", { index: i }); await refresh(); draw(); }) }, "Remove"))));
    });
    body.append(tbl);
  };
  draw();
  void st;
  modal("Manage printers", body, [["Add printer…", () => openAddPrinter(false)], ["Close", null, true]]);
}

async function openMachineSettings() {
  const data = await api.get("/api/machine_settings");
  const tabs = h("div", { class: "tabs" });
  const content = h("div", {});
  const set = (target, key, value) => act(async () => { await api.post("/api/machine/setting", { target, key, value }); await refresh(); });
  const field = (s, target) => {
    let ctl;
    if (s.type === "bool") ctl = h("input", { type: "checkbox", checked: !!s.value, onchange: (e) => set(target, s.key, e.target.checked ? "True" : "False") });
    else if (s.type === "enum") ctl = h("select", { class: "grow", onchange: (e) => set(target, s.key, e.target.value) }, s.options.map(([k, l]) => h("option", { value: k, selected: k === String(s.value) }, l)));
    else if (s.type === "str") ctl = h("textarea", { spellcheck: "false", onchange: (e) => set(target, s.key, e.target.value) }, String(s.value ?? ""));
    else ctl = numberField(s.value_str, s.unit, (v) => set(target, s.key, String(v)));
    return { s, ctl };
  };
  const show = (idx) => {
    $$("button", tabs).forEach((b, i) => b.classList.toggle("active", i === idx));
    content.innerHTML = "";
    if (idx === 0) {
      const grid = h("div", { class: "ms-grid" });
      const codes = h("div", { class: "ms-code" });
      for (const s of data.printer) {
        const f = field(s, "global");
        if (s.type === "str") codes.append(h("div", {}, h("div", { class: "section-label" }, s.label), f.ctl));
        else grid.append(h("div", { class: "row" }, h("label", {}, s.label), f.ctl));
      }
      const hd = data.head;
      const head = h("div", { class: "ms-grid", style: { marginTop: "14px" } });
      const hv = { xmin: hd.xmin, ymin: hd.ymin, xmax: hd.xmax, ymax: hd.ymax };
      for (const [k, l] of [["xmin", "Printhead X min"], ["ymin", "Printhead Y min"], ["xmax", "Printhead X max"], ["ymax", "Printhead Y max"]]) {
        head.append(h("div", { class: "row" }, h("label", {}, l), numberField(fmt(hv[k], 2), "mm", (v) => { hv[k] = v; set("global", "machine_head_with_fans_polygon_minmax", `${hv.xmin},${hv.ymin},${hv.xmax},${hv.ymax}`); })));
      }
      content.append(h("div", { class: "section-label" }, "Printer"), grid, h("div", { class: "section-label", style: { marginTop: "14px" } }, "Printhead"), head, codes);
    } else {
      const e = data.extruders[idx - 1];
      const grid = h("div", { class: "ms-grid" });
      const codes = h("div", { class: "ms-code" });
      for (const s of e.settings) {
        const f = field(s, `e:${e.position}`);
        if (s.type === "str") codes.append(h("div", {}, h("div", { class: "section-label" }, s.label), f.ctl));
        else grid.append(h("div", { class: "row" }, h("label", {}, s.label), f.ctl));
      }
      content.append(grid, codes);
    }
  };
  tabs.append(h("button", { onclick: () => show(0) }, "Printer"));
  data.extruders.forEach((e, i) => tabs.append(h("button", { onclick: () => show(i + 1) }, data.extruders.length > 1 ? `Extruder ${e.position + 1}` : "Extruder")));
  show(0);
  modal(`Machine settings — ${ui.state.machine.name}`, h("div", {}, tabs, content), [["Close", async () => { await refresh(); }, true]]);
}

async function openPreferences() {
  const st = ui.state;
  const prefs = st.prefs;
  const cur = h("input", { type: "text", value: prefs.currency, style: { width: "60px" }, onchange: (e) => api.post("/api/prefs", { currency: e.target.value }) });
  const body = h("div", {},
    h("div", { class: "section-label" }, "General"),
    h("label", { class: "row" }, h("input", { type: "checkbox", checked: prefs.jobname_prefix, onchange: (e) => api.post("/api/prefs", { jobname_prefix: e.target.checked }) }), "Add the printer abbreviation to the job name"),
    h("label", { class: "row" }, h("input", { type: "checkbox", checked: prefs.auto_drop, onchange: (e) => api.post("/api/prefs", { auto_drop: e.target.checked }) }), "Automatically drop models to the build plate"),
    h("label", { class: "row" }, h("input", { type: "checkbox", checked: prefs.auto_slice, onchange: (e) => api.post("/api/prefs", { auto_slice: e.target.checked }) }), "Slice automatically"),
    h("label", { class: "row" }, h("input", { type: "checkbox", checked: viewer.overhang, onchange: (e) => { viewer.overhang = e.target.checked; viewer.dirty = true; } }), "Show overhangs in red"),
    h("div", { class: "row" }, h("label", {}, "Currency"), cur),
    h("div", { class: "section-label", style: { marginTop: "18px" } }, "Material costs"),
    h("div", { class: "muted", style: { marginBottom: "8px" } }, "Spool price and weight are used to estimate the cost of a print."));
  const tbl = h("table", { class: "plain" }, h("tr", {}, h("th", {}, "Extruder material"), h("th", {}, "Spool cost"), h("th", {}, "Spool weight (g)")));
  for (const e of st.machine?.extruders || []) {
    const mats = await api.get(`/api/materials?position=${e.position}`).catch(() => []);
    const mt = mats.find((x) => x.base === e.material.base);
    if (!mt) continue;
    const cost = h("input", { type: "number", value: "", style: { width: "90px" } });
    const weight = h("input", { type: "number", value: "1000", style: { width: "90px" } });
    const save = () => api.post("/api/prefs", { material_guid: mt.guid, spool_cost: parseFloat(cost.value) || 0, spool_weight: parseFloat(weight.value) || 0 });
    cost.onchange = save; weight.onchange = save;
    tbl.append(h("tr", {}, h("td", {}, `${e.position + 1}: ${mt.name}`), h("td", {}, cost), h("td", {}, weight)));
  }
  body.append(tbl);
  modal("Preferences", body, [["Close", async () => { await refresh(); }, true]], "small");
}

function openAbout() {
  modal("About Curust", h("div", { style: { lineHeight: 1.6 } },
    h("p", {}, h("b", {}, `Curust ${ui.state.version}`), " — a 3D printing slicer written in Rust."),
    h("p", {}, "Curust is a port of UltiMaker Cura 5.12 and CuraEngine 5.12: it uses Cura's printer, material and quality profiles and produces the same G-code."),
    h("p", {}, "Source code, new versions and bug reports: ",
      h("a", { href: "#", onclick: (e) => { e.preventDefault(); api.post("/api/open_homepage", {}); } }, "github.com/WildWetWilly/Curust"), " · ",
      h("a", { href: "#", onclick: (e) => { e.preventDefault(); api.post("/api/open_homepage", { page: "issues" }); } }, "Report a problem")),
    h("p", { class: "muted" }, "Licensed under the GNU AGPL v3 or later. Cura and CuraEngine are © UltiMaker (LGPLv3 / AGPLv3). Curust is not affiliated with UltiMaker."),
    h("p", { class: "muted" }, `Profiles: ${ui.state.resources || ""}`)), [["Close", null, true]], "small");
}

// ---------------------------------------------------------------- start
refresh().catch((e) => toast(e.message, true));
setStage("prepare");
