// Skalowanie i wygląd interfejsu (v0.8). Samodzielny moduł: stosuje skalę, gęstość, pasek boczny, ruch i kontrast,
// obsługuje skróty Ctrl+ +/-/0 i Ctrl+kółko oraz sekcję „Wygląd i skalowanie” w Ustawieniach.
// Zapis: settings.json przez set_settings (debounce); do czasu odczytu ustawień używana jest kopia w localStorage (bez mignięcia).
(() => {
  "use strict";
  const DEF = { ui_scale: 1, font_scale: 1, density: "normal", sidebar: "auto", reduce_motion: false, high_contrast: false, theme: "auto" };
  const RANGE = { ui_scale: [0.75, 2], font_scale: [0.85, 1.5] };
  const root = document.documentElement;
  const q = (s) => document.querySelector(s), qa = (s) => [...document.querySelectorAll(s)];
  const store = { get: (k) => { try { return localStorage.getItem(k); } catch { return null; } }, set: (k, v) => { try { localStorage.setItem(k, v); } catch {} } };
  const num = (v, key) => { const [lo, hi] = RANGE[key]; v = Number(v); if (!Number.isFinite(v)) return DEF[key]; return Math.min(hi, Math.max(lo, Math.round(v * 20) / 20)); };
  const norm = (o) => ({
    ui_scale: num(o && o.ui_scale, "ui_scale"), font_scale: num(o && o.font_scale, "font_scale"),
    density: ["compact", "normal", "comfortable"].includes(o && o.density) ? o.density : DEF.density,
    sidebar: ["auto", "expanded", "collapsed"].includes(o && o.sidebar) ? o.sidebar : DEF.sidebar,
    reduce_motion: !!(o && o.reduce_motion), high_contrast: !!(o && o.high_contrast),
    theme: ["auto", "light", "dark"].includes(o && o.theme) ? o.theme : DEF.theme,
  });
  let cached = null; try { cached = JSON.parse(store.get("appearance") || "null"); } catch {}
  let st = norm(cached), raw = null, saveTimer = 0, hudTimer = 0;
  const pct = (v) => Math.round(v * 100) + "%";

  function layout() {
    const s = st.ui_scale, ew = window.innerWidth / s, eh = window.innerHeight / s;
    root.style.setProperty("--vw", ew + "px"); root.style.setProperty("--vh", eh + "px");
    root.classList.toggle("sb-collapsed", st.sidebar === "collapsed" || (st.sidebar === "auto" && window.innerWidth < 900));
    root.classList.toggle("bp-sm", ew < 760);
    root.classList.toggle("h-short", eh < 560);
    labelNav();
  }
  // etykiety dla przycisków ikonowych (zwinięty pasek boczny): aria-label i dymek z nazwą
  function labelNav() {
    const collapsed = root.classList.contains("sb-collapsed");
    for (const b of qa("aside button, button.icon")) {
      const l = b.querySelector(".l"), t = ((l ? l.textContent : b.textContent) || "").trim();
      if (!t) continue;
      if (!b.hasAttribute("aria-label") || b.dataset.autoLabel) { b.setAttribute("aria-label", t); b.dataset.autoLabel = "1"; }
      if (!l) continue;
      if (collapsed) { b.title = t; b.dataset.autoTitle = "1"; }
      else if (b.dataset.autoTitle) { b.removeAttribute("title"); delete b.dataset.autoTitle; }
    }
  }
  function syncUi() {
    const set = (id, fn) => { const e = q(id); if (e) fn(e); };
    set("#apScale", (e) => (e.value = Math.round(st.ui_scale * 100))); set("#apScaleV", (e) => (e.textContent = pct(st.ui_scale)));
    set("#apFont", (e) => (e.value = Math.round(st.font_scale * 100))); set("#apFontV", (e) => (e.textContent = pct(st.font_scale)));
    set("#apMotion", (e) => (e.checked = st.reduce_motion)); set("#apContrast", (e) => (e.checked = st.high_contrast));
    for (const r of qa("input[name=density]")) r.checked = r.value === st.density;
    for (const r of qa("input[name=sidebar]")) r.checked = r.value === st.sidebar;
  }
  function apply() {
    root.style.setProperty("--ui-scale", String(st.ui_scale));
    root.style.setProperty("--fs", String(st.font_scale));
    root.dataset.density = st.density; root.dataset.sidebar = st.sidebar;
    if (st.reduce_motion) root.dataset.motion = "reduce"; else delete root.dataset.motion;
    if (st.high_contrast) root.dataset.contrast = "high"; else delete root.dataset.contrast;
    layout(); syncUi();
    store.set("appearance", JSON.stringify(st));
  }
  function hud() {
    const h = q("#zoomHud"); if (!h) return;
    h.textContent = pct(st.ui_scale); h.classList.add("on");
    clearTimeout(hudTimer); hudTimer = setTimeout(() => h.classList.remove("on"), 1100);
  }
  function scheduleSave() {
    if (!raw) return;           // ustawienia nie zostały wczytane z backendu: nie nadpisujemy ich
    clearTimeout(saveTimer); saveTimer = setTimeout(flush, 400);
  }
  async function flush() {
    clearTimeout(saveTimer); saveTimer = 0;
    if (!raw) return true;
    const next = Object.assign({}, raw, st);
    try { await window.__TAURI__.core.invoke("set_settings", { settings: next }); raw = next; return true; }
    catch (e) { if (typeof window.toast === "function") window.toast(typeof window.errText === "function" ? window.errText(e) : String(e), "bad"); return false; }
  }
  function set(patch, opts) {
    st = norm(Object.assign({}, st, patch)); apply();
    if (opts && opts.hud) hud();
    scheduleSave();
  }
  function zoomBy(d) { set({ ui_scale: Math.round((st.ui_scale + d) * 20) / 20 }, { hud: true }); }

  // skróty klawiszowe i kółko myszy
  window.addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const k = e.key;
    if (k === "+" || k === "=" || e.code === "NumpadAdd") { e.preventDefault(); zoomBy(0.1); }
    else if (k === "-" || k === "_" || e.code === "NumpadSubtract") { e.preventDefault(); zoomBy(-0.1); }
    else if (k === "0" || e.code === "Numpad0") { e.preventDefault(); set({ ui_scale: 1 }, { hud: true }); }
  }, true);
  window.addEventListener("wheel", (e) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    if (e.deltaY) zoomBy(e.deltaY < 0 ? 0.05 : -0.05);
  }, { passive: false, capture: true });
  window.addEventListener("resize", layout);
  window.addEventListener("pagehide", () => { if (saveTimer) flush(); });

  // dymek z pełną nazwą/ścieżką dla obciętych elementów (elipsa)
  document.addEventListener("mouseover", (e) => {
    const el = e.target.closest && e.target.closest(".card .name,.loc .t span,.cur,.tag,.remote .grow b,.remote .grow span");
    if (el && !el.title && el.scrollWidth > el.clientWidth + 1) el.title = el.textContent.trim();
  });

  function bindUi() {
    const on = (id, ev, fn) => { const e = q(id); if (e) e.addEventListener(ev, fn); };
    on("#apScale", "input", (e) => set({ ui_scale: e.target.value / 100 }));
    on("#apScaleDn", "click", () => zoomBy(-0.05)); on("#apScaleUp", "click", () => zoomBy(0.05));
    on("#apFont", "input", (e) => set({ font_scale: e.target.value / 100 }));
    on("#apMotion", "change", (e) => set({ reduce_motion: e.target.checked }));
    on("#apContrast", "change", (e) => set({ high_contrast: e.target.checked }));
    on("#apReset", "click", () => { set({ ui_scale: 1, font_scale: 1, density: "normal", sidebar: "auto", reduce_motion: false, high_contrast: false }); flush(); });
    for (const r of qa("input[name=density]")) r.addEventListener("change", () => set({ density: r.value }));
    for (const r of qa("input[name=sidebar]")) r.addEventListener("change", () => set({ sidebar: r.value }));
    for (const r of qa("input[name=theme]")) r.addEventListener("change", () => set({ theme: r.value }));
  }

  // wywoływane przez app.js po udanym get_settings (settings = obiekt Settings z backendu)
  function init(settings) {
    raw = Object.assign({}, settings);
    const t = store.get("theme"), serverTheme = norm(settings).theme;
    st = norm(settings);
    // świeża pamięć przeglądarki: motyw z ustawień
    if (t === null && serverTheme !== "auto") {
      store.set("theme", serverTheme);
      if (serverTheme === "light" || serverTheme === "dark") root.dataset.theme = serverTheme;
      const r = q(`input[name=theme][value=${serverTheme}]`); if (r) r.checked = true;
    } else if (t === "light" || t === "dark" || t === "auto") st.theme = t;
    apply();
  }
  window.SyncronScale = {
    init, set, flush, state: () => Object.assign({}, st),
    raw: () => (raw ? Object.assign({}, raw) : {}),
    adopt: (s) => { raw = Object.assign({}, s); },
    zoomBy, DEF,
  };
  bindUi(); apply();
  document.addEventListener("DOMContentLoaded", () => setTimeout(labelNav, 0));
})();
