const { invoke } = window.__TAURI__.core;
const dialogApi = window.__TAURI__.dialog;
const autostart = window.__TAURI__.autostart;
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

// ---------- ikony ----------
const P = {
  sync: '<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/>',
  activity: '<path d="M22 12h-4l-3 9L9 3l-3 9H2"/>',
  cloud: '<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>',
  sliders: '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>',
  pause: '<rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/>',
  play: '<polygon points="6 3 20 12 6 21 6 3"/>',
  plus: '<path d="M5 12h14M12 5v14"/>',
  download: '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  network: '<rect x="2" y="2" width="20" height="8" rx="2"/><rect x="2" y="14" width="20" height="8" rx="2"/><path d="M6 6h.01M6 18h.01"/>',
  search: '<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>',
  edit: '<path d="M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/>',
  trash: '<path d="M3 6h18M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6M10 11v6M14 11v6M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>',
  both: '<path d="M17 3l4 4-4 4M3 7h18M7 21l-4-4 4-4M21 17H3"/>',
  right: '<path d="M5 12h14M12 5l7 7-7 7"/>',
  left: '<path d="M19 12H5M12 19l-7-7 7-7"/>',
  power: '<path d="M18.36 6.64a9 9 0 1 1-12.73 0M12 2v10"/>',
  shield: '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z"/>',
};
const icon = (n) => `<svg class="i" viewBox="0 0 24 24">${P[n] || ""}</svg>`;
const hydrateIcons = (root = document) => $$("[data-i]", root).forEach((e) => (e.innerHTML = icon(e.dataset.i)));
$("#logo").innerHTML = icon("sync");

// ---------- pomocnicze ----------
let LANG = "pl"; try { LANG = localStorage.getItem("lang") || "pl"; } catch {}
const LOCALE = LANG === "en" ? "en-GB" : "pl-PL";
const STATE = { conditions: "Czeka na warunki", idle: "Gotowe", running: "Synchronizuję", waiting: "Czeka na pliki", error: "Błąd", blocked: "Zatrzymane", paused: "Wstrzymane", disabled: "Wyłączone" };
const DIRICON = { TwoWay: "both", LeftToRight: "right", RightToLeft: "left" };
const fmtTime = (t) => (t ? new Date(t * 1000).toLocaleString(LOCALE) : "—");
function rel(t) {
  if (!t) return "jeszcze nie";
  const d = Math.max(0, Date.now() / 1000 - t);
  if (d < 45) return "przed chwilą";
  if (d < 3600) return `${Math.round(d / 60)} min temu`;
  if (d < 86400) return `${Math.round(d / 3600)} godz. temu`;
  return new Date(t * 1000).toLocaleDateString(LOCALE);
}
const schedText = (s) => ({ Manual: "ręcznie", Realtime: "na bieżąco", Interval: `co ${Math.round((s.secs || 0) / 60)} min`, Daily: `codziennie ${String(s.hour).padStart(2, "0")}:${String(s.minute).padStart(2, "0")}` }[s.type] || "");
function locKind(p) {
  if (/^[^\\/:]{2,}:/.test(p) && !/^[A-Za-z]:[\\/]/.test(p)) return "cloud";
  if (/^(\\\\|\/\/|\/Volumes|\/mnt|\/net)/.test(p)) return "network";
  return "folder";
}
const ACTION = (a) => (typeof a === "string" ? { CopyToRight: "kopiuj →", CopyToLeft: "← kopiuj", DeleteLeft: "usuń po lewej", DeleteRight: "usuń po prawej" }[a] || a : a.Conflict.keep_both ? "konflikt (zachowaj obie)" : "konflikt (do decyzji)");

function toast(msg, kind = "") {
  const t = document.createElement("div");
  t.className = "toast " + kind; t.textContent = msg;
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "bad" ? 7000 : 3500);
}
function ask(title, text, ok = "OK", danger = false) {
  return new Promise((res) => {
    $("#cfTitle").textContent = title; $("#cfText").textContent = text;
    const y = $("#cfYes"); y.textContent = ok; y.className = danger ? "danger" : "primary";
    const d = $("#confirm"), done = (v) => { d.close(); res(v); };
    y.onclick = () => done(true); $("#cfNo").onclick = () => done(false); d.oncancel = () => res(false);
    d.showModal();
  });
}
const store = { get: (k) => { try { return localStorage.getItem(k); } catch { return null; } }, set: (k, v) => { try { localStorage.setItem(k, v); } catch {} } };

let jobs = [], remotes = [], overview = { paused: false, status: {} }, lastHist = [], lastKey = "", dryBusy = new Set();

// ---------- motyw ----------
function applyTheme(t) {
  if (t === "light" || t === "dark") document.documentElement.dataset.theme = t; else delete document.documentElement.dataset.theme;
  store.set("theme", t);
  const r = $(`input[name=theme][value=${t}]`); if (r) r.checked = true;
}
applyTheme(store.get("theme") || "auto");
$$("input[name=theme]").forEach((r) => r.addEventListener("change", () => applyTheme(r.value)));

// ---------- nawigacja ----------
function gotoTab(name) {
  $$("nav button").forEach((x) => x.classList.toggle("active", x.dataset.tab === name));
  $$(".tab").forEach((t) => t.classList.toggle("active", t.id === "tab-" + name));
  if (name === "activity") loadHistory();
  if (name === "accounts") loadRemotes();
  if (name === "trash") loadTrash();
}
$$("nav button").forEach((b) => b.addEventListener("click", () => gotoTab(b.dataset.tab)));

// ---------- zadania ----------
async function refreshJobs() { jobs = await invoke("list_jobs"); lastKey = ""; renderJobs(); fillHistJobs(); }
async function refreshStatus() {
  try { overview = await invoke("overview"); } catch { return; }
  $("#pauseTxt").textContent = overview.paused ? "Wznów wszystko" : "Wstrzymaj wszystko";
  $("#pauseAll").firstElementChild.innerHTML = icon(overview.paused ? "play" : "pause");
  const att = attention();
  const b = $("#attBadge"); b.hidden = !att; b.textContent = att;
  renderJobs();
  if ($("#tab-activity").classList.contains("active")) renderPending();
}
function attention() {
  let n = 0;
  for (const j of jobs) { const r = overview.status[j.id]?.last; if (r) n += r.conflicts.length + r.busy.length + r.errors.length; if (overview.status[j.id]?.error) n++; }
  return n;
}

function renderTiles() {
  const active = jobs.filter((j) => j.enabled).length;
  const last = Math.max(0, ...jobs.map((j) => overview.status[j.id]?.last_run || 0));
  const att = attention(), running = jobs.filter((j) => overview.status[j.id]?.state === "running").length;
  const health = overview.paused ? ["Wstrzymane", "warn"] : running ? [`Synchronizuję (${running})`, ""] : att ? ["Są sprawy do przejrzenia", "warn"] : ["Wszystko w porządku", "ok"];
  $("#tiles").innerHTML = `
    <div class="tile"><div class="k">Aktywne zadania</div><div class="v">${active}<span class="muted" style="font-size:14px;font-weight:600"> / ${jobs.length}</span></div></div>
    <div class="tile"><div class="k">Ostatnia synchronizacja</div><div class="v small">${esc(rel(last))}</div></div>
    <div class="tile ${att ? "warn" : ""}"><div class="k">Wymaga uwagi</div><div class="v">${att}</div></div>
    <div class="tile ${health[1]}"><div class="k">Stan</div><div class="v small">${esc(health[0])}</div></div>`;
}

const TEMPLATES = [
  { id: "two", title: "Synchronizacja dwustronna", text: "Zmiany po obu stronach trafiają na drugą — np. komputer ⇄ NAS.", job: { name: "Synchronizacja dwustronna", mode: "TwoWay", conflict: "KeepBoth" } },
  { id: "bak", title: "Kopia zapasowa", text: "Lewa → prawa, nic nie jest usuwane po prawej stronie.", job: { name: "Kopia zapasowa", mode: "LeftToRight", no_delete_right: true, conflict: "LeftWins" } },
  { id: "mir", title: "Lustro (dokładna kopia)", text: "Prawa strona jest identyczna z lewą; usunięte pliki trafiają do kosza.", job: { name: "Lustro", mode: "LeftToRight", conflict: "LeftWins" } },
];

function renderJobs() {
  const key = JSON.stringify([jobs, overview, [...dryBusy]]);
  if (key === lastKey) return;
  lastKey = key;
  renderTiles();
  const box = $("#jobs");
  if (!jobs.length) {
    box.innerHTML = `<div class="box empty"><svg class="big" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">${P.sync}</svg>
      <h2>Zsynchronizuj pierwsze foldery</h2><p class="sub">Wybierz gotowy szablon albo utwórz zadanie od zera.</p>
      <div class="tpls">${TEMPLATES.map((t) => `<button class="tpl" data-tpl="${t.id}"><b>${esc(t.title)}</b><span>${esc(t.text)}</span></button>`).join("")}</div>
      <p><button class="primary" id="emptyNew"><span>${icon("plus")}</span>Nowe zadanie</button></p></div>`;
    return;
  }
  box.innerHTML = jobs.map((j) => {
    const st = overview.status[j.id] || {};
    const state = !j.enabled ? "disabled" : overview.paused ? "paused" : st.state || "idle";
    const rep = st.last, pr = st.progress;
    let stats = st.last_run ? `Ostatnio ${rel(st.last_run)}` : "Jeszcze nie uruchomione";
    if (rep) stats += ` · skopiowano ${rep.copied}, usunięto ${rep.deleted}`;
    if (st.next_run && state !== "disabled") stats += ` · następne ${new Date(st.next_run * 1000).toLocaleTimeString(LOCALE, { hour: "2-digit", minute: "2-digit" })}`;
    const tags = [schedText(j.schedule), j.no_delete_left && "bez usuwania po lewej", j.no_delete_right && "bez usuwania po prawej", j.only_add && "tylko dodawanie", (j.trash_days > 0 ? `kosz ${j.trash_days} dni` : "bez kosza"), j.verify && "weryfikacja sum", j.conditions?.ac_only && "tylko zasilanie sieciowe", (j.conditions?.wifi || []).length && "Wi-Fi: " + j.conditions.wifi.join(", "), vssInfo.supported && j.use_vss !== false && "VSS", (j.exclude || []).length && `wyklucza ${j.exclude.length}`].filter(Boolean);
    let extra = "";
    if (state === "running") {
      const pct = pr && pr.total ? Math.round((pr.done / pr.total) * 100) : null;
      extra = `<div class="bar ${pct === null ? "ind" : ""}"><i style="width:${pct ?? 35}%"></i></div><div class="cur">${pr ? `${pr.done}/${pr.total} · ${esc(pr.current)}` : "Skanowanie…"}</div>`;
    }
    if (st.error) extra += `<div class="msg">${esc(st.error)}</div>`;
    else if (rep?.blocked) extra += `<div class="msg warn"><b>Zatrzymano ze względów bezpieczeństwa:</b> ${esc(rep.blocked)}. Sprawdź „Symulację”; „Wymuś” pominie blokadę.</div>`;
    else if (st.state === "conditions" && st.note) extra += `<div class="msg warn">Czeka na warunki: ${esc(st.note)}</div>`;
    else if (rep?.busy.length) extra += `<div class="msg warn">${rep.busy.length} plik(ów) jest zajętych przez inne programy — ponowię automatycznie.${rep.vss_note ? `<br><b>VSS:</b> ${esc(rep.vss_note)}` : ""}</div>`;
    else if (rep?.conflicts.length) extra += `<div class="msg warn">${rep.conflicts.length} konflikt(ów) czeka na decyzję w zakładce „Aktywność”.</div>`;
    const loc = (side, p) => `<div class="loc"><div class="ic">${icon(locKind(p))}</div><div class="t"><b>${side}</b><span title="${esc(p)}">${esc(p)}</span></div></div>`;
    return `<div class="card s-${state}" data-id="${esc(j.id)}">
      <div class="top"><span class="name">${esc(j.name)}</span><span class="pill ${state}">${STATE[state]}</span></div>
      <div class="route">${loc("Lewa", j.left)}<div class="dir">${icon(DIRICON[j.mode])}</div>${loc("Prawa", j.right)}</div>
      <div class="tags">${tags.map((t) => `<span class="tag">${esc(t)}</span>`).join("")}</div>
      ${extra}<div class="meta">${esc(stats)}</div>
      <div class="actions">
        <button class="primary sm" data-act="run" ${state === "running" ? "disabled" : ""}>${icon("play")}Uruchom</button>
        <button class="sm" data-act="dry" ${dryBusy.has(j.id) ? "disabled" : ""}>${icon("search")}${dryBusy.has(j.id) ? "Liczę…" : "Symulacja"}</button>
        ${rep?.blocked ? `<button class="sm" data-act="force">${icon("shield")}Wymuś</button>` : ""}
        <button class="sm" data-act="edit">${icon("edit")}Edytuj</button>
        <button class="sm" data-act="toggle">${icon("power")}${j.enabled ? "Wyłącz" : "Włącz"}</button>
        <button class="sm danger" data-act="del">${icon("trash")}Usuń</button>
      </div></div>`;
  }).join("");
}

$("#jobs").addEventListener("click", async (e) => {
  const tpl = e.target.closest("[data-tpl]");
  if (tpl) return openEditor(null, TEMPLATES.find((t) => t.id === tpl.dataset.tpl).job);
  if (e.target.closest("#emptyNew")) return openEditor(null);
  const b = e.target.closest("button[data-act]"); if (!b) return;
  const id = b.closest(".card").dataset.id, job = jobs.find((j) => j.id === id);
  try {
    switch (b.dataset.act) {
      case "run": await invoke("run_now", { id, force: false }); toast("Uruchomiono: " + job.name); break;
      case "force":
        if (await ask("Wymusić synchronizację?", "Zabezpieczenie zatrzymało to zadanie, bo plan wygląda podejrzanie (np. masowe usuwanie albo pusta strona). Pliki usuwane/nadpisywane trafią do kosza synchronizacji, jeśli jest włączony.", "Wymuś", true)) await invoke("run_now", { id, force: true });
        break;
      case "dry": dryBusy.add(id); renderJobs(); try { showReport(job.name, await invoke("dry_run", { id })); } finally { dryBusy.delete(id); renderJobs(); } break;
      case "edit": openEditor(job); break;
      case "toggle": await invoke("save_job", { job: { ...job, enabled: !job.enabled } }); await refreshJobs(); break;
      case "del": if (await ask("Usunąć zadanie?", `Zadanie „${job.name}” zostanie usunięte. Pliki na dyskach nie zostaną ruszone.`, "Usuń", true)) { await invoke("delete_job", { id }); await refreshJobs(); toast("Zadanie usunięte"); } break;
    }
  } catch (err) { toast(String(err), "bad"); }
  refreshStatus();
});

function showReport(title, r) {
  $("#repTitle").textContent = "Symulacja: " + title;
  const treeHtml = diffTree(r.planned);
  const rows = r.planned.map(([k, a]) => `<tr><td><span class="kind ${typeof a === "string" && a.startsWith("Delete") ? "warn" : typeof a !== "string" ? "bad" : "ok"}">${ACTION(a)}</span></td><td>${esc(k)}</td></tr>`).join("");
  $("#repBody").innerHTML = `<p>Przeskanowano: <b>${r.scanned_left}</b> plików po lewej i <b>${r.scanned_right}</b> po prawej. Planowanych działań: <b>${r.planned.length}</b>. <span class="muted">Nic nie zostało zmienione.</span></p>` +
    (r.blocked ? `<div class="msg warn"><b>Zabezpieczenie zatrzymałoby synchronizację:</b> ${esc(r.blocked)}</div>` : "") +
    (r.errors.length ? `<div class="msg">${r.errors.map(esc).join("<br>")}</div>` : "") +
    (rows ? `<div class="scroll" style="margin-top:12px">${treeHtml}</div>` : '<p class="muted">Obie strony są zgodne — nic do zrobienia.</p>');
  $("#report").showModal();
}
$("#repClose").onclick = () => $("#report").close();

// ---------- kreator zadania ----------
const F = () => $("#jobForm").elements;
const STEP_NAMES = ["Krok 1 z 5 · Lokalizacje", "Krok 2 z 5 · Kierunek i usuwanie", "Krok 3 z 5 · Filtry i podfoldery", "Krok 4 z 5 · Harmonogram i warunki", "Krok 5 z 5 · Kosz, bezpieczeństwo i podsumowanie"];
let step = 0;
let vssInfo = { supported: false, admin: false, elevated_autostart: false };
const DEFAULT_JOB = { use_vss: true, verify: false, max_versions: 0, conditions: { ac_only: false, wifi: [] }, triggers: { on_drive: false, on_wake: false, on_network: false }, id: "", name: "", left: "", right: "", mode: "TwoWay", conflict: "Newer", include: [], exclude: [], schedule: { type: "Realtime", poll_secs: 300 }, enabled: true, guard_pct: 25, trash_days: 30, threads: 4 };

function openEditor(job, tpl) {
  const f = F(); $("#edErr").textContent = "";
  $("#edTitle").textContent = job ? "Edytuj zadanie" : "Nowe zadanie";
  const j = job || { ...DEFAULT_JOB, ...(tpl || {}) };
  f.name.value = j.name; f.left.value = j.left; f.right.value = j.right; f.mode.value = j.mode; f.conflict.value = j.conflict;
  f.no_delete_left.checked = !!j.no_delete_left; f.no_delete_right.checked = !!j.no_delete_right; f.only_add.checked = !!j.only_add;
  f.include.value = (j.include || []).join("\n"); f.exclude.value = (j.exclude || []).join("\n");
  f.min_mb.value = j.min_size ? j.min_size / 1048576 : ""; f.max_mb.value = j.max_size ? j.max_size / 1048576 : "";
  f.sched.value = j.schedule.type;
  f.sched_val.value = j.schedule.type === "Interval" ? Math.round(j.schedule.secs / 60) : j.schedule.type === "Daily" ? `${String(j.schedule.hour).padStart(2, "0")}:${String(j.schedule.minute).padStart(2, "0")}` : j.schedule.type === "Realtime" ? Math.round(j.schedule.poll_secs / 60) : "";
  f.win_from.value = j.window ? j.window[0] : ""; f.win_to.value = j.window ? j.window[1] : "";
  f.use_trash.checked = j.trash_days > 0; f.trash_days.value = j.trash_days > 0 ? j.trash_days : 30; f.max_versions.value = j.max_versions || 0; f.verify.checked = !!j.verify;
  f.ac_only.checked = !!j.conditions?.ac_only; f.wifi.value = (j.conditions?.wifi || []).join("\n");
  f.on_drive.checked = !!j.triggers?.on_drive; f.on_wake.checked = !!j.triggers?.on_wake; f.on_network.checked = !!j.triggers?.on_network; trashToggle(); $("#tree").innerHTML = "";
  f.guard_pct.value = j.guard_pct; f.threads.value = j.threads; f.enabled.checked = j.enabled; f.use_vss.checked = j.use_vss !== false; $("#vssRow").hidden = !vssInfo.supported;
  $("#jobForm").dataset.id = j.id || "";
  $$("[data-hint]").forEach((h) => (h.textContent = ""));
  schedHint(); showStep(0); $("#editor").showModal(); f.name.focus();
}
function showStep(n) {
  step = n;
  $$(".step").forEach((s) => s.classList.toggle("on", +s.dataset.step === n));
  $$("#steps span").forEach((s, i) => s.classList.toggle("on", i <= n));
  $("#stepName").textContent = STEP_NAMES[n];
  $("#edBack").hidden = n === 0; $("#edNext").hidden = n === 4; $("#edSave").hidden = n !== 4;
  if (n === 4) $("#summary").innerHTML = summarize(readJob());
}
function schedHint() {
  const f = F(), t = f.sched.value, l = $("#schedVal");
  l.style.display = t === "Manual" ? "none" : "";
  $("#schedLbl").textContent = t === "Interval" ? "Co ile minut" : t === "Daily" ? "O której godzinie (GG:MM)" : "Dodatkowe sprawdzanie co ile minut (dyski sieciowe i chmura)";
  f.sched_val.placeholder = t === "Daily" ? "02:30" : "5";
}
$$("input[name=sched]").forEach((r) => r.addEventListener("change", schedHint));

function readJob() {
  const f = F(), lines = (v) => v.split("\n").map((x) => x.trim()).filter(Boolean), num = (v) => (v === "" ? null : Number(v));
  const t = f.sched.value, v = f.sched_val.value.trim();
  let schedule;
  if (t === "Manual") schedule = { type: "Manual" };
  else if (t === "Interval") schedule = { type: "Interval", secs: Math.max(1, Number(v) || 15) * 60 };
  else if (t === "Daily") { const m = /^(\d{1,2}):(\d{2})$/.exec(v); schedule = { type: "Daily", hour: m ? +m[1] : 2, minute: m ? +m[2] : 0, bad: !m }; }
  else schedule = { type: "Realtime", poll_secs: Math.max(1, Number(v) || 5) * 60 };
  const from = num(f.win_from.value), to = num(f.win_to.value);
  const left = f.left.value.trim(), right = f.right.value.trim();
  return {
    id: $("#jobForm").dataset.id || (crypto.randomUUID ? crypto.randomUUID() : String(Date.now())),
    name: f.name.value.trim() || `${left} ⇄ ${right}`, left, right, mode: f.mode.value, conflict: f.conflict.value,
    no_delete_left: f.no_delete_left.checked, no_delete_right: f.no_delete_right.checked, only_add: f.only_add.checked,
    include: lines(f.include.value), exclude: lines(f.exclude.value),
    min_size: f.min_mb.value === "" ? null : Math.round(Number(f.min_mb.value) * 1048576),
    max_size: f.max_mb.value === "" ? null : Math.round(Number(f.max_mb.value) * 1048576),
    schedule, window: from !== null && to !== null ? [from, to] : null, enabled: f.enabled.checked,
    guard_pct: Number(f.guard_pct.value) || 25, trash_days: f.use_trash.checked ? Math.max(1, Number(f.trash_days.value) || 30) : 0, max_versions: Number(f.max_versions.value) || 0, verify: f.verify.checked,
    conditions: { ac_only: f.ac_only.checked, wifi: lines(f.wifi.value) }, triggers: { on_drive: f.on_drive.checked, on_wake: f.on_wake.checked, on_network: f.on_network.checked }, threads: Number(f.threads.value) || 4, use_vss: f.use_vss.checked,
  };
}
function summarize(j) {
  const L = `<b>${esc(j.left || "lewa")}</b>`, R = `<b>${esc(j.right || "prawa")}</b>`, li = [];
  li.push(j.mode === "TwoWay" ? `Zmiany z ${L} i ${R} są przenoszone na drugą stronę.` : j.mode === "LeftToRight" ? `Pliki z ${L} są kopiowane do ${R}.` : `Pliki z ${R} są kopiowane do ${L}.`);
  if (j.only_add) li.push("Nic nie jest nadpisywane ani usuwane — tylko nowe pliki są dodawane.");
  else {
    if (j.no_delete_left) li.push("Po lewej stronie nic nie jest usuwane.");
    if (j.no_delete_right) li.push("Po prawej stronie nic nie jest usuwane.");
    if (!j.no_delete_left && !j.no_delete_right) li.push(j.trash_days > 0 ? `Usunięte i nadpisane pliki trafiają do kosza na ${j.trash_days} dni${j.max_versions ? ` (maks. ${j.max_versions} wersji pliku)` : ""}.` : "Kosz wyłączony: usunięte i nadpisane pliki są tracone bez możliwości przywrócenia.");
  }
  if (j.verify) li.push("Po kopiowaniu sprawdzana jest suma kontrolna pliku.");
  if (j.conditions?.ac_only) li.push("Zadanie startuje tylko przy zasilaniu sieciowym.");
  if ((j.conditions?.wifi || []).length) li.push(`Zadanie startuje tylko w sieciach Wi-Fi: ${esc(j.conditions.wifi.join(", "))}.`);
  const tr = [j.triggers?.on_drive && "po podłączeniu dysku", j.triggers?.on_wake && "po wybudzeniu z uśpienia", j.triggers?.on_network && "po zmianie sieci"].filter(Boolean);
  if (tr.length) li.push(`Dodatkowo uruchamia się: ${tr.join(", ")}.`);
  li.push(`Uruchamianie: ${schedText(j.schedule)}${j.window ? `, tylko w godzinach ${j.window[0]}:00–${j.window[1]}:00` : ""}.`);
  return `<b>Jak to będzie działać</b><ul>${li.map((x) => `<li>${x}</li>`).join("")}</ul>`;
}
function validateStep(n) {
  const f = F();
  if (n === 0) {
    if (!f.left.value.trim() || !f.right.value.trim()) return "Podaj obie lokalizacje.";
    if (f.left.value.trim() === f.right.value.trim()) return "Lewa i prawa lokalizacja nie mogą być takie same.";
  }
  if (n === 3 && f.sched.value === "Daily" && !/^(\d{1,2}):(\d{2})$/.test(f.sched_val.value.trim())) return "Podaj godzinę w formacie GG:MM, np. 02:30.";
  return "";
}
$("#edNext").onclick = () => { const e = validateStep(step); $("#edErr").textContent = e; if (!e) showStep(step + 1); };
$("#edBack").onclick = () => { $("#edErr").textContent = ""; showStep(step - 1); };
$("#edCancel").onclick = () => $("#editor").close();
$("#newJob").onclick = () => openEditor(null);

$$("button[data-pick]").forEach((b) => b.addEventListener("click", async () => {
  try { const dir = await dialogApi.open({ directory: true, multiple: false, title: "Wybierz folder" }); if (dir) { F()[b.dataset.pick].value = dir; checkPath(b.dataset.pick); } } catch (e) { toast(String(e), "bad"); }
}));
$$("select[data-remote]").forEach((s) => s.addEventListener("change", () => { if (s.value) F()[s.dataset.remote].value = s.value + ":"; s.value = ""; }));
async function checkPath(side) {
  const v = F()[side].value.trim(), h = $(`[data-hint=${side}]`);
  h.textContent = v ? (await invoke("check_path", { path: v }).catch(() => null)) || "" : "";
}
["left", "right"].forEach((s) => F()[s].addEventListener("change", () => checkPath(s)));
function fillRemoteSelects() {
  $$("select[data-remote]").forEach((s) => { s.innerHTML = '<option value="">Konto chmurowe…</option>' + remotes.map((r) => `<option value="${esc(r.name)}">${esc(r.name)} (${esc(r.kind)})</option>`).join(""); });
}

$("#jobForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (step < 4) return $("#edNext").click();
  const err = validateStep(3) || validateStep(0); if (err) { $("#edErr").textContent = err; return; }
  const job = readJob(); delete job.schedule.bad;
  try { await invoke("save_job", { job }); $("#editor").close(); await refreshJobs(); refreshStatus(); toast("Zadanie zapisane", "ok"); }
  catch (er) { $("#edErr").textContent = String(er); }
});

// ---------- aktywność ----------
function fillHistJobs() {
  const cur = $("#histJob").value;
  $("#histJob").innerHTML = '<option value="">Wszystkie zadania</option>' + jobs.map((j) => `<option value="${esc(j.id)}">${esc(j.name)}</option>`).join("");
  $("#histJob").value = cur;
}
const kindClass = (k) => (/błąd/.test(k) ? "bad" : /zajęty|konflikt/.test(k) ? "warn" : /kopiowanie|rozstrzygn/.test(k) ? "ok" : "");
async function loadHistory() {
  try { lastHist = await invoke("history", { job: $("#histJob").value || null, limit: 500 }); } catch { lastHist = []; }
  renderHistory(); renderPending();
}
function renderHistory() {
  const q = $("#histQ").value.trim().toLowerCase(), name = (id) => jobs.find((j) => j.id === id)?.name || id;
  const rows = lastHist.filter((h) => !q || `${h.kind} ${h.path} ${h.detail} ${name(h.job)}`.toLowerCase().includes(q));
  $("#hist tbody").innerHTML = rows.map((h) => `<tr><td>${fmtTime(h.ts)}</td><td>${esc(name(h.job))}</td><td><span class="kind ${kindClass(h.kind)}">${esc(h.kind)}</span></td><td>${esc(h.path)}</td><td class="muted">${esc(h.detail)}</td></tr>`).join("") || '<tr><td colspan="5" class="muted">Brak wpisów.</td></tr>';
}
function renderPending() {
  const out = [];
  for (const j of jobs) {
    const st = overview.status[j.id], r = st?.last; if (!r) continue;
    r.conflicts.forEach((p) => out.push(`<tr><td><span class="kind warn">konflikt</span></td><td><b>${esc(j.name)}</b><br>${esc(p)}</td><td class="muted">zmieniono po obu stronach</td><td style="white-space:nowrap"><button class="sm" data-res="left" data-job="${esc(j.id)}" data-path="${esc(p)}">Zachowaj lewą</button> <button class="sm" data-res="right" data-job="${esc(j.id)}" data-path="${esc(p)}">Zachowaj prawą</button></td></tr>`));
    r.busy.forEach((p) => out.push(`<tr><td><span class="kind warn">zajęty</span></td><td><b>${esc(j.name)}</b><br>${esc(p)}</td><td class="muted" colspan="2">otwarty w innym programie — ponowię automatycznie${r.vss_note ? ` · VSS: ${esc(r.vss_note)}` : ""}${st.next_run ? " (" + new Date(st.next_run * 1000).toLocaleTimeString(LOCALE) + ")" : ""}</td></tr>`));
    r.errors.forEach((p) => out.push(`<tr><td><span class="kind bad">błąd</span></td><td colspan="3"><b>${esc(j.name)}</b><br>${esc(p)}</td></tr>`));
    if (st.error) out.push(`<tr><td><span class="kind bad">błąd</span></td><td colspan="3"><b>${esc(j.name)}</b><br>${esc(st.error)}</td></tr>`);
  }
  $("#pending").innerHTML = out.length ? `<table><tbody>${out.join("")}</tbody></table>` : '<span class="muted">✓ Nic nie czeka na Twoją uwagę.</span>';
}
$("#pending").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-res]"); if (!b) return;
  b.disabled = true;
  try { await invoke("resolve_conflict", { id: b.dataset.job, path: b.dataset.path, leftWins: b.dataset.res === "left" }); toast("Konflikt rozstrzygnięty", "ok"); }
  catch (er) { toast(String(er), "bad"); }
  loadHistory(); refreshStatus();
});
$("#histJob").onchange = loadHistory;
$("#histQ").oninput = renderHistory;
$("#exportCsv").onclick = () => {
  const q = (s) => `"${String(s ?? "").replace(/"/g, '""')}"`;
  const csv = "﻿" + ["czas;zadanie;operacja;plik;szczegóły", ...lastHist.map((h) => [fmtTime(h.ts), jobs.find((j) => j.id === h.job)?.name || h.job, h.kind, h.path, h.detail].map(q).join(";"))].join("\r\n");
  const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" })); a.download = "syncpolak-historia.csv"; a.click();
};

// ---------- konta ----------
const PROVIDERS = [
  { id: "drive", name: "Google Drive", info: "Logowanie w przeglądarce", color: "#4285f4", mark: "G", kind: "drive", suggest: "gdrive", fields: [] },
  { id: "onedrive", name: "OneDrive / SharePoint", info: "Logowanie w przeglądarce", color: "#0078d4", mark: "O", kind: "onedrive", suggest: "onedrive", fields: [] },
  { id: "bitrix", name: "Bitrix24 Dysk", info: "WebDAV: adres, login, hasło", color: "#2fc6f6", mark: "B", kind: "webdav", suggest: "bitrix", extra: { vendor: "other" },
    fields: [["url", "Adres WebDAV (Bitrix24 → Dysk → „Połącz dysk sieciowy”)", "https://firma.bitrix24.pl/company/personal/user/1/disk/path/"], ["user", "Login"], ["pass", "Hasło lub hasło aplikacji", "", "password"]] },
  { id: "webdav", name: "WebDAV / Nextcloud", info: "Adres, login, hasło", color: "#64748b", mark: "W", kind: "webdav", suggest: "webdav", extra: { vendor: "other" }, fields: [["url", "Adres WebDAV"], ["user", "Login"], ["pass", "Hasło", "", "password"]] },
  { id: "crypt", name: "Szyfrowanie (crypt)", info: "Szyfruje pliki przed wysłaniem", color: "#7c3aed", mark: "🔒", kind: "crypt", suggest: "szyfr", extra: { filename_encryption: "standard", directory_name_encryption: "true" },
    warn: "Zapisz hasła w bezpiecznym miejscu — bez nich zaszyfrowanych danych nie da się odzyskać. Konto-źródło musi już istnieć (np. gdrive:Szyfrowane).",
    fields: [["remote", "Konto i folder, w którym trafią zaszyfrowane pliki", "gdrive:Szyfrowane"], ["password", "Hasło szyfrowania", "", "password"], ["password2", "Dodatkowe hasło (sól, zalecane)", "", "password"]] },
  { id: "sftp", name: "SFTP", info: "Serwer, login, hasło", color: "#16a34a", mark: "S", kind: "sftp", suggest: "sftp", fields: [["host", "Serwer"], ["user", "Login"], ["pass", "Hasło", "", "password"], ["port", "Port", "22"]] },
];
let prov = null;
$("#providers").innerHTML = PROVIDERS.map((p) => `<button class="prov" data-p="${p.id}"><span class="pi" style="background:${p.color}">${p.mark}</span><b>${esc(p.name)}</b><span class="muted" style="font-weight:500;font-size:12px">${esc(p.info)}</span></button>`).join("");
$("#providers").addEventListener("click", (e) => {
  const b = e.target.closest("[data-p]"); if (!b) return;
  prov = PROVIDERS.find((p) => p.id === b.dataset.p);
  $$(".prov").forEach((x) => x.classList.toggle("sel", x === b));
  $("#rForm").hidden = false; $("#rName").value = prov.suggest; $("#rMsg").textContent = "";
  $("#rFields").innerHTML = (prov.warn ? `<div class="msg warn">${esc(prov.warn)}</div>` : "") + prov.fields.map(([k, l, ph, ty]) => `<label>${esc(l)}<input data-f="${k}" type="${ty || "text"}" placeholder="${esc(ph || "")}"></label>`).join("") ||
    '<p class="muted">Po kliknięciu „Połącz konto” otworzy się przeglądarka z logowaniem — dokończ je tam.</p>';
});
$("#rAdd").onclick = async () => {
  const params = { ...(prov.extra || {}) };
  $$("#rFields input").forEach((i) => (params[i.dataset.f] = i.value.trim()));
  $("#rMsg").textContent = "Łączenie… (dokończ logowanie w przeglądarce, jeśli się otworzyła)"; $("#rAdd").disabled = true;
  try { await invoke("add_remote", { name: $("#rName").value.trim(), kind: prov.kind, params }); $("#rMsg").textContent = ""; $("#rForm").hidden = true; toast("Konto dodane", "ok"); await loadRemotes(); }
  catch (e) { $("#rMsg").textContent = "Błąd: " + e; }
  $("#rAdd").disabled = false;
};
async function loadRemotes() {
  try { remotes = await invoke("list_remotes"); } catch { remotes = []; }
  fillRemoteSelects();
  $("#remotes").innerHTML = remotes.length ? remotes.map((r) => `<div class="remote"><span class="loc" style="padding:0;border:0;background:none"><span class="ic">${icon("cloud")}</span></span><div class="grow"><b>${esc(r.name)}:</b> <span class="muted">${esc(r.kind)}</span></div><button class="sm" data-t="${esc(r.name)}">Testuj</button><button class="sm danger" data-d="${esc(r.name)}">Usuń</button></div>`).join("") : '<span class="muted">Brak połączonych kont. Wybierz usługę poniżej.</span>';
}
$("#remotes").addEventListener("click", async (e) => {
  const t = e.target.closest("[data-t]")?.dataset.t, d = e.target.closest("[data-d]")?.dataset.d;
  try {
    if (t) { e.target.disabled = true; toast(await invoke("test_remote", { name: t }), "ok"); e.target.disabled = false; }
    if (d && (await ask("Usunąć konto?", `Konto „${d}” zostanie usunięte z aplikacji (dane w chmurze zostają).`, "Usuń", true))) { await invoke("delete_remote", { name: d }); await loadRemotes(); }
  } catch (err) { toast("Błąd: " + err, "bad"); loadRemotes(); }
});

// ---------- ustawienia i start ----------
$("#pauseAll").onclick = async () => { await invoke("set_paused", { paused: !overview.paused }); refreshStatus(); };
(async () => {
  try { $("#autostart").checked = await autostart.isEnabled(); } catch {}
  $("#autostart").onchange = async (e) => { try { e.target.checked ? await autostart.enable() : await autostart.disable(); } catch (err) { toast(String(err), "bad"); } };
  try { $("#rcv").textContent = await invoke("rclone_version"); } catch { $("#rcv").textContent = "niedostępny (foldery lokalne i dyski sieciowe działają; chmury wymagają rclone)"; }
})();
async function loadVss() {
  try { vssInfo = await invoke("vss_status"); } catch {}
  $("#vssBox").hidden = !vssInfo.supported;
  if (!vssInfo.supported) return;
  $("#vssState").textContent = vssInfo.admin ? "✓ Działasz jako administrator — pliki zablokowane przez inne programy będą kopiowane z migawki VSS." : "Brak uprawnień administratora — zablokowane pliki będą ponawiane, ale VSS nie zadziała.";
  $("#vssElevate").hidden = vssInfo.admin;
  $("#vssAutoRow").hidden = !vssInfo.admin; $("#vssAuto").checked = vssInfo.elevated_autostart;
}
$("#vssElevate").onclick = async () => {
  if (!(await ask("Uruchomić ponownie jako administrator?", "Windows poprosi o potwierdzenie (UAC). Synchronizacja zostanie na chwilę zatrzymana.", "Uruchom ponownie"))) return;
  try { await invoke("restart_elevated"); } catch (e) { toast(String(e), "bad"); }
};
$("#vssAuto").onchange = async (e) => {
  try { await invoke("set_elevated_autostart", { enable: e.target.checked }); if (e.target.checked) { try { await autostart.disable(); $("#autostart").checked = false; } catch {} } toast("Zapisano", "ok"); }
  catch (er) { e.target.checked = !e.target.checked; toast(String(er), "bad"); }
};
hydrateIcons();
loadVss();
loadRemotes(); refreshJobs().then(refreshStatus);
setInterval(refreshStatus, 2000);

// ---------- drzewo różnic (symulacja) ----------
function diffTree(planned) {
  const root = { kids: {}, items: [], n: 0 };
  for (const [k, a] of planned) {
    const parts = k.split("/"), file = parts.pop(); let node = root; node.n++;
    for (const p of parts) { node = node.kids[p] ||= { kids: {}, items: [], n: 0 }; node.n++; }
    node.items.push([file, a]);
  }
  const cls = (a) => (typeof a === "string" && a.startsWith("Delete") ? "warn" : typeof a !== "string" ? "bad" : "ok");
  const render = (node, name, open) => {
    const items = node.items.map(([f, a]) => `<div style="padding:2px 0 2px 18px"><span class="kind ${cls(a)}">${ACTION(a)}</span> ${esc(f)}</div>`).join("");
    const kids = Object.entries(node.kids).sort(([a], [b]) => a.localeCompare(b, LOCALE)).map(([n, c]) => render(c, n, false)).join("");
    return name === null ? items + kids : `<details ${open ? "open" : ""} style="margin-left:6px"><summary style="cursor:pointer;padding:3px 0"><b>${esc(name)}</b> <span class="muted">(${node.n})</span></summary>${items}${kids}</details>`;
  };
  return render(root, null, true);
}

// ---------- drzewo podfolderów ----------
const includeList = () => F().include.value.split("\n").map((x) => x.trim()).filter(Boolean);
async function loadTree(rel, box) {
  const left = F().left.value.trim();
  if (!left) { box.innerHTML = '<span class="muted">Najpierw podaj lewą lokalizację (krok 1).</span>'; return; }
  box.innerHTML = '<span class="muted">Wczytywanie…</span>';
  try {
    const dirs = await invoke("list_dirs", { location: left, rel });
    const inc = includeList();
    box.innerHTML = dirs.length ? dirs.map((d) => {
      const path = rel ? rel + "/" + d : d;
      return `<div class="treerow" data-path="${esc(path)}"><label class="sw" style="padding:4px 8px;margin:2px 0"><input type="checkbox" ${inc.includes(path + "/*") ? "checked" : ""}><span>${esc(d)}</span></label><button type="button" class="sm ghost" data-exp="1">▸</button><div class="kids" style="margin-left:22px"></div></div>`;
    }).join("") : '<span class="muted">Brak podfolderów.</span>';
  } catch (e) { box.innerHTML = `<span class="err">${esc(String(e))}</span>`; }
}
$("#treeLoad").onclick = () => loadTree("", $("#tree"));
$("#tree").addEventListener("click", (e) => {
  const b = e.target.closest("[data-exp]"); if (!b) return;
  const row = b.closest(".treerow"), kids = row.querySelector(".kids");
  if (kids.dataset.open) { kids.innerHTML = ""; delete kids.dataset.open; b.textContent = "▸"; }
  else { kids.dataset.open = "1"; b.textContent = "▾"; loadTree(row.dataset.path, kids); }
});
$("#tree").addEventListener("change", (e) => {
  const cb = e.target.closest("input[type=checkbox]"); if (!cb) return;
  const pat = cb.closest(".treerow").dataset.path + "/*", set = new Set(includeList());
  cb.checked ? set.add(pat) : set.delete(pat);
  F().include.value = [...set].join("\n");
});
function trashToggle() { const on = F().use_trash.checked; $$("#trashOpts input").forEach((i) => (i.disabled = !on)); }
F().use_trash.addEventListener("change", trashToggle);

// ---------- kosz i wersje ----------
let trashItems = [];
const stampDate = (st) => { const m = /^(\d{4})-(\d{2})-(\d{2})_(\d{2})(\d{2})(\d{2})/.exec(st); return m ? new Date(+m[1], m[2] - 1, +m[3], +m[4], +m[5], +m[6]).toLocaleString(LOCALE) : st; };
const fmtSize = (n) => (n < 1024 ? n + " B" : n < 1048576 ? (n / 1024).toFixed(1) + " KB" : n < 1073741824 ? (n / 1048576).toFixed(1) + " MB" : (n / 1073741824).toFixed(2) + " GB");
function fillTrashJobs() {
  const cur = $("#trashJob").value;
  $("#trashJob").innerHTML = jobs.map((j) => `<option value="${esc(j.id)}">${esc(j.name)}</option>`).join("");
  if (cur && jobs.some((j) => j.id === cur)) $("#trashJob").value = cur;
}
async function loadTrash() {
  fillTrashJobs();
  const id = $("#trashJob").value, job = jobs.find((j) => j.id === id);
  if (!job) { $("#trashTbl tbody").innerHTML = '<tr><td colspan="5" class="muted">Brak zadań.</td></tr>'; return; }
  $("#trashNote").textContent = job.trash_days > 0 ? `Kosz tego zadania jest włączony (${job.trash_days} dni${job.max_versions ? `, maks. ${job.max_versions} wersji pliku` : ""}).` : "Kosz tego zadania jest wyłączony — nowe pliki nie są odkładane. Włącz go w edycji zadania (krok 5).";
  $("#trashTbl tbody").innerHTML = '<tr><td colspan="5" class="muted">Wczytywanie…</td></tr>';
  try { trashItems = await invoke("trash_list", { id }); } catch (e) { trashItems = []; $("#trashTbl tbody").innerHTML = `<tr><td colspan="5" class="err">${esc(String(e))}</td></tr>`; return; }
  renderTrash();
}
function renderTrash() {
  const q = $("#trashQ").value.trim().toLowerCase();
  const byFile = {};
  for (const it of trashItems) if (!q || it.path.toLowerCase().includes(q)) (byFile[it.side + "|" + it.path] ||= []).push(it);
  const rows = Object.values(byFile).sort((a, b) => a[0].path.localeCompare(b[0].path, LOCALE)).flatMap((vs) =>
    vs.sort((a, b) => b.stamp.localeCompare(a.stamp)).map((it, i) => `<tr><td>${i === 0 ? `<b>${esc(it.path)}</b>${vs.length > 1 ? ` <span class="muted">(${vs.length} wersji)</span>` : ""}` : ""}</td><td>${it.side === "left" ? "lewa" : "prawa"}</td><td>${esc(stampDate(it.stamp))}</td><td>${fmtSize(it.size)}</td>
      <td style="white-space:nowrap"><button class="sm" data-tr="restore" data-side="${it.side}" data-stamp="${esc(it.stamp)}" data-path="${esc(it.path)}">Przywróć</button> <button class="sm danger" data-tr="del" data-side="${it.side}" data-stamp="${esc(it.stamp)}" data-path="${esc(it.path)}">Usuń</button></td></tr>`));
  $("#trashTbl tbody").innerHTML = rows.join("") || '<tr><td colspan="5" class="muted">Kosz jest pusty.</td></tr>';
}
$("#trashJob").onchange = loadTrash; $("#trashQ").oninput = renderTrash;
$("#trashTbl").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-tr]"); if (!b) return;
  const id = $("#trashJob").value, d = b.dataset;
  try {
    if (b.dataset.tr === "restore") { const to = await invoke("trash_restore", { id, side: d.side, stamp: d.stamp, path: d.path }); toast("Przywrócono jako: " + to, "ok"); invoke("run_now", { id, force: false }).catch(() => {}); }
    else if (await ask("Usunąć trwale?", `Wersja pliku „${d.path}” zostanie usunięta bez możliwości przywrócenia.`, "Usuń", true)) { await invoke("trash_delete", { id, side: d.side, stamp: d.stamp, path: d.path }); toast("Usunięto"); }
  } catch (err) { toast(String(err), "bad"); }
  loadTrash();
});
$("#trashEmpty").onclick = async () => {
  const id = $("#trashJob").value, job = jobs.find((j) => j.id === id); if (!job) return;
  if (!(await ask("Opróżnić kosz?", `Cały kosz zadania „${job.name}” (obie strony) zostanie usunięty trwale.`, "Opróżnij", true))) return;
  try { for (const side of ["left", "right"]) await invoke("trash_delete", { id, side, stamp: null, path: null }); toast("Kosz opróżniony", "ok"); } catch (err) { toast(String(err), "bad"); }
  loadTrash();
};

// ---------- ustawienia: powiadomienia, język, aktualizacje, kopia ----------
(async () => {
  let st = { notify: true, language: LANG };
  try { st = await invoke("get_settings"); } catch {}
  $("#notify").checked = st.notify;
  $(`input[name=lang][value=${LANG}]`).checked = true;
  const save = () => invoke("set_settings", { settings: { notify: $("#notify").checked, language: $("input[name=lang]:checked").value } }).catch((e) => toast(String(e), "bad"));
  $("#notify").onchange = save;
  $$("input[name=lang]").forEach((r) => (r.onchange = () => { store.set("lang", r.value); save().then(() => location.reload()); }));
})();
async function checkUpdates(silent) {
  const msg = $("#updMsg");
  try {
    if (!window.__TAURI__.updater) throw new Error("brak modułu aktualizacji");
    if (!silent) msg.textContent = "Sprawdzanie…";
    const u = await window.__TAURI__.updater.check();
    if (!u) { if (!silent) msg.textContent = "Masz najnowszą wersję."; return; }
    msg.textContent = "Dostępna wersja " + u.version;
    if (await ask("Dostępna aktualizacja " + u.version, "Pobrać i zainstalować teraz? Aplikacja uruchomi się ponownie.", "Zainstaluj")) {
      msg.textContent = "Pobieranie…"; await u.downloadAndInstall(); await window.__TAURI__.process.relaunch();
    }
  } catch (e) { if (!silent) msg.textContent = "Nie udało się sprawdzić aktualizacji (brak opublikowanej aktualizacji lub brak sieci)."; }
}
$("#checkUpd").onclick = () => checkUpdates(false);
setTimeout(() => checkUpdates(true), 8000);
$("#cfgExport").onclick = async () => {
  try { const p = await dialogApi.save({ defaultPath: "syncpolak-konfiguracja.json", filters: [{ name: "JSON", extensions: ["json"] }] }); if (p) toast(`Wyeksportowano zadań: ${await invoke("export_config", { path: p })}`, "ok"); } catch (e) { toast(String(e), "bad"); }
};
$("#cfgImport").onclick = async () => {
  try { const p = await dialogApi.open({ multiple: false, filters: [{ name: "JSON", extensions: ["json"] }] }); if (p) { toast(`Zaimportowano zadań: ${await invoke("import_config", { path: p })}`, "ok"); await refreshJobs(); } } catch (e) { toast(String(e), "bad"); }
};
