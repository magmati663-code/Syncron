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
const ACTION = (a) => (typeof a === "string" ? { CopyToRight: "kopiuj →", CopyToLeft: "← kopiuj", DeleteLeft: "usuń po lewej", DeleteRight: "usuń po prawej" }[a] || a
  : a.RenameRight ? "przenieś →" : a.RenameLeft ? "← przenieś" : a.Conflict.keep_both ? "konflikt (zachowaj obie)" : "konflikt (do decyzji)");
const actCls = (a) => (typeof a === "string" ? (a.startsWith("Delete") ? "warn" : "ok") : a.Conflict ? "bad" : "ok");

function toast(msg, kind = "") {
  const t = document.createElement("div");
  t.className = "toast " + kind; t.textContent = msg;
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "bad" ? 7000 : 3500);
}
// ask() jest kolejkowane: drugie wywołanie czeka, aż użytkownik zamknie bieżące okno potwierdzenia
// (inaczej nadpisywało treść i handlery otwartego okna, a odpowiedź trafiała do złego pytania).
let askQueue = Promise.resolve();
function ask(title, text, ok = "OK", danger = false) {
  const run = () => new Promise((res) => {
    $("#cfTitle").textContent = title; $("#cfText").textContent = text;
    const y = $("#cfYes"); y.textContent = ok; y.className = danger ? "danger" : "primary";
    const d = $("#confirm"); let fin = false;
    const done = (v) => { if (fin) return; fin = true; d.onclose = null; if (d.open) d.close(); res(v); };
    y.onclick = () => done(true); $("#cfNo").onclick = () => done(false); d.onclose = () => done(false);
    d.showModal();
  });
  const p = askQueue.then(run);
  askQueue = p.catch(() => {});
  return p;
}
// Komunikat z backendu: surowe błędy serde/Tauri (niepoprawne argumenty) zastępujemy zrozumiałym tekstem.
const RAW_ERR = /invalid (args|value|type|length)|expected (a )?(u|i)(8|16|32|64|size)|unknown (variant|field)|missing field|data did not match|deserializ/i;
function errText(e, fallback = "Operacja nie powiodła się — sprawdź wprowadzone wartości.") {
  const m = String(e?.message ?? e ?? "").trim();
  return !m || RAW_ERR.test(m) ? fallback : m;
}
// jedna wiadomość na zmianę stanu (np. cykliczne odpytywanie nie zasypuje toastów)
const warned = new Set();
function warnOnce(key, msg) { if (warned.has(key)) return; warned.add(key); toast(msg, "bad"); }
const clearWarn = (key) => warned.delete(key);
// ustawia innerHTML tylko wtedy, gdy treść się zmieniła (zachowuje fokus i trwające kliknięcia)
function setHtml(el, html) { if (el._h === html) return false; el._h = html; el.innerHTML = html; return true; }
const store = { get: (k) => { try { return localStorage.getItem(k); } catch { return null; } }, set: (k, v) => { try { localStorage.setItem(k, v); } catch {} } };

let jobs = [], remotes = [], overview = { paused: false, status: {} }, lastHist = [], lastKey = "", dryBusy = new Set();
let jobsErr = "", overviewErr = false;

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
  $$("nav button").forEach((x) => { const on = x.dataset.tab === name; x.classList.toggle("active", on); if (on) x.setAttribute("aria-current", "page"); else x.removeAttribute("aria-current"); });
  $$(".tab").forEach((t) => t.classList.toggle("active", t.id === "tab-" + name));
  if (name === "activity") loadHistory();
  if (name === "accounts") loadRemotes();
  if (name === "trash") loadTrash();
}
$$("nav button").forEach((b) => b.addEventListener("click", () => gotoTab(b.dataset.tab)));

// ---------- zadania ----------
let jobsSeq = 0, statusSeq = 0;
async function refreshJobs() {
  const my = ++jobsSeq;
  try {
    const l = await invoke("list_jobs");
    if (my !== jobsSeq) return;
    jobs = l; jobsErr = ""; clearWarn("list_jobs");
  } catch (e) {
    if (my !== jobsSeq) return;
    jobsErr = errText(e, "Nie udało się odczytać listy zadań."); warnOnce("list_jobs", "Nie udało się wczytać zadań: " + jobsErr);
  }
  renderJobs(); fillHistJobs();
}
async function refreshStatus() {
  const my = ++statusSeq;
  try {
    const o = await invoke("overview");
    if (my !== statusSeq) return;
    overview = o; overviewErr = false; clearWarn("overview");
  } catch (e) {
    if (my !== statusSeq) return;
    overviewErr = true; warnOnce("overview", "Brak odpowiedzi silnika synchronizacji — widoczny stan może być nieaktualny: " + errText(e, "błąd połączenia"));
  }
  $("#pauseTxt").textContent = overview.paused ? "Wznów wszystko" : "Wstrzymaj wszystko";
  $("#pauseAll").firstElementChild.innerHTML = icon(overview.paused ? "play" : "pause");
  const att = attention();
  const b = $("#attBadge"); b.hidden = !att; b.textContent = att;
  b.setAttribute("aria-label", att + " pozycji wymaga uwagi");
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
  const health = overviewErr ? ["Brak odpowiedzi silnika", "warn"] : overview.paused ? ["Wstrzymane", "warn"] : running ? [`Synchronizuję (${running})`, ""] : att ? ["Są sprawy do przejrzenia", "warn"] : ["Wszystko w porządku", "ok"];
  setHtml($("#tiles"), `
    <div class="tile"><div class="k">Aktywne zadania</div><div class="v">${active}<span class="muted" style="font-size:14px;font-weight:600"> / ${jobs.length}</span></div></div>
    <div class="tile"><div class="k">Ostatnia synchronizacja</div><div class="v small">${esc(rel(last))}</div></div>
    <div class="tile ${att ? "warn" : ""}"><div class="k">Wymaga uwagi</div><div class="v">${att}</div></div>
    <div class="tile ${health[1]}"><div class="k">Stan</div><div class="v small">${esc(health[0])}</div></div>`);
}

const TEMPLATES = [
  { id: "two", title: "Synchronizacja dwustronna", text: "Zmiany po obu stronach trafiają na drugą — np. komputer ⇄ NAS.", job: { name: "Synchronizacja dwustronna", mode: "TwoWay", conflict: "KeepBoth" } },
  { id: "bak", title: "Kopia zapasowa", text: "Lewa → prawa, nic nie jest usuwane po prawej stronie.", job: { name: "Kopia zapasowa", mode: "LeftToRight", no_delete_right: true, conflict: "LeftWins" } },
  { id: "mir", title: "Lustro (dokładna kopia)", text: "Prawa strona jest identyczna z lewą; usunięte pliki trafiają do kosza.", job: { name: "Lustro", mode: "LeftToRight", conflict: "LeftWins" } },
];

// Karta zadania: szkielet (sig) porównywany tekstowo; sam postęp (pasek, bieżący plik) jest podmieniany w miejscu,
// więc cykliczne odświeżanie nie przebudowuje DOM (nie gubi fokusu ani trwającego kliknięcia).
function cardParts(j) {
  const st = overview.status[j.id] || {};
  const state = !j.enabled ? "disabled" : overview.paused ? "paused" : st.state || "idle";
  const rep = st.last, pr = st.progress;
  let stats = st.last_run ? `Ostatnio ${rel(st.last_run)}` : "Jeszcze nie uruchomione";
  if (rep) stats += ` · skopiowano ${rep.copied}, usunięto ${rep.deleted}` + (rep.moved ? `, przeniesiono ${rep.moved}` : "");
  if (st.next_run && state !== "disabled") stats += ` · następne ${new Date(st.next_run * 1000).toLocaleTimeString(LOCALE, { hour: "2-digit", minute: "2-digit" })}`;
  const tags = [schedText(j.schedule), j.no_delete_left && "bez usuwania po lewej", j.no_delete_right && "bez usuwania po prawej", j.only_add && "tylko dodawanie", (j.trash_days > 0 ? `kosz ${j.trash_days} dni` : "bez kosza"), j.verify && "weryfikacja sum", j.conditions?.ac_only && "tylko zasilanie sieciowe", (j.conditions?.wifi || []).length && "Wi-Fi: " + j.conditions.wifi.join(", "), (j.bandwidth?.kbps || j.bandwidth?.off_kbps) && `limit ${j.bandwidth.kbps || "∞"} KB/s`, vssInfo.supported && j.use_vss !== false && "VSS", (j.exclude || []).length && `wyklucza ${j.exclude.length}`].filter(Boolean);
  let extra = "", pct = null, cur = "";
  if (state === "running") {
    pct = pr && pr.total ? Math.round((pr.done / pr.total) * 100) : null;
    cur = pr ? `${pr.done}/${pr.total} · ${pr.current}` : "";
    extra = `<div class="bar ${pct === null ? "ind" : ""}"><i></i></div><div class="cur"${pr ? " data-nt" : ""}>${pr ? "" : "Skanowanie…"}</div>`;
  }
  if (st.error) extra += `<div class="msg" data-nt>${esc(st.error)}</div>`;
  else if (rep?.blocked) extra += `<div class="msg warn"><b>Zatrzymano ze względów bezpieczeństwa:</b> <span data-nt>${esc(rep.blocked)}</span>. Sprawdź „Symulację”; „Wymuś” pominie blokadę.</div>`;
  else if (st.state === "conditions" && st.note) extra += `<div class="msg warn">Czeka na warunki: ${esc(st.note)}</div>`;
  else if (rep?.busy.length) extra += `<div class="msg warn">${rep.busy.length} plik(ów) jest zajętych przez inne programy — ponowię automatycznie.${rep.vss_note ? `<br><b>VSS:</b> <span data-nt>${esc(rep.vss_note)}</span>` : ""}</div>`;
  else if (rep?.conflicts.length) extra += `<div class="msg warn">${rep.conflicts.length} konflikt(ów) czeka na decyzję w zakładce „Aktywność”.</div>`;
  const loc = (side, p) => `<div class="loc"><div class="ic">${icon(locKind(p))}</div><div class="t"><b>${side}</b><span title="${esc(p)}" data-nt>${esc(p)}</span></div></div>`;
  const html = `<div class="card s-${state}" data-id="${esc(j.id)}">
      <div class="top"><span class="name" data-nt>${esc(j.name)}</span><span class="pill ${state}">${STATE[state]}</span></div>
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
  return { id: j.id, html, pct, cur };
}
function patchProgress(el, p) {
  const i = el.querySelector(".bar i"); if (i) i.style.width = (p.pct ?? 35) + "%";
  const c = el.querySelector(".cur"); if (c && p.cur && c.textContent !== p.cur) c.textContent = p.cur;
}
function renderJobs() {
  renderTiles();
  const box = $("#jobs");
  if (!jobs.length) {
    box._cards = null;
    if (jobsErr) {
      setHtml(box, `<div class="box empty"><h2>Nie udało się wczytać zadań</h2><p class="err" data-nt>${esc(jobsErr)}</p><p><button class="primary" id="jobsRetry">Spróbuj ponownie</button></p></div>`);
      return;
    }
    setHtml(box, `<div class="box empty"><svg class="big" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">${P.sync}</svg>
      <h2>Zsynchronizuj pierwsze foldery</h2><p class="sub">Wybierz gotowy szablon albo utwórz zadanie od zera.</p>
      <div class="tpls">${TEMPLATES.map((t) => `<button class="tpl" data-tpl="${t.id}"><b>${esc(t.title)}</b><span>${esc(t.text)}</span></button>`).join("")}</div>
      <p><button class="primary" id="emptyNew"><span>${icon("plus")}</span>Nowe zadanie</button></p></div>`);
    return;
  }
  if (!box._cards) { box.innerHTML = ""; box._h = null; box._cards = new Map(); }
  const cards = box._cards, seen = new Set(), tpl = document.createElement("template");
  jobs.forEach((j, idx) => {
    const p = cardParts(j); seen.add(j.id);
    const sig = p.html, cur = cards.get(j.id);
    let el = cur && cur.el;
    if (!el || cur.sig !== sig) {
      tpl.innerHTML = sig; const n = tpl.content.firstElementChild;
      let refocus = null;
      if (el && el.contains(document.activeElement)) refocus = document.activeElement.dataset.act || null;
      if (el) el.replaceWith(n);
      el = n; cards.set(j.id, { el, sig });
      if (refocus) { const b = el.querySelector(`[data-act=${refocus}]`); if (b && !b.disabled) b.focus(); }
    }
    patchProgress(el, p);
    if (box.children[idx] !== el) box.insertBefore(el, box.children[idx] || null);
  });
  for (const [id, c] of cards) if (!seen.has(id)) { c.el.remove(); cards.delete(id); }
}

$("#jobs").addEventListener("click", async (e) => {
  const tpl = e.target.closest("[data-tpl]");
  if (tpl) return openEditor(null, TEMPLATES.find((t) => t.id === tpl.dataset.tpl).job);
  if (e.target.closest("#emptyNew")) return openEditor(null);
  if (e.target.closest("#jobsRetry")) { await refreshJobs(); return refreshStatus(); }
  const b = e.target.closest("button[data-act]"); if (!b) return;
  const id = b.closest(".card").dataset.id, job = jobs.find((j) => j.id === id); if (!job) return;
  try {
    switch (b.dataset.act) {
      case "run":
        if ((await invoke("run_now", { id, force: false })) === false) toast("Nie uruchomiono: zadanie jest wyłączone lub właśnie się wykonuje.", "bad");
        else toast("Uruchomiono: " + job.name);
        break;
      case "force":
        if (await ask("Wymusić synchronizację?", "Zabezpieczenie zatrzymało to zadanie, bo plan wygląda podejrzanie (np. masowe usuwanie albo pusta strona). Pliki usuwane/nadpisywane trafią do kosza synchronizacji, jeśli jest włączony.", "Wymuś", true)
          && (await invoke("run_now", { id, force: true })) === false) toast("Nie uruchomiono: zadanie jest wyłączone lub właśnie się wykonuje.", "bad");
        break;
      case "dry": dryBusy.add(id); renderJobs(); try { showReport(job.name, await invoke("dry_run", { id })); } finally { dryBusy.delete(id); renderJobs(); } break;
      case "edit": openEditor(job); break;
      case "toggle": await invoke("save_job", { job: { ...job, enabled: !job.enabled } }); await refreshJobs(); break;
      case "del": if (await ask("Usunąć zadanie?", `Zadanie „${job.name}” zostanie usunięte. Pliki na dyskach nie zostaną ruszone.`, "Usuń", true)) { await invoke("delete_job", { id }); await refreshJobs(); toast("Zadanie usunięte"); } break;
    }
  } catch (err) { toast(errText(err), "bad"); }
  refreshStatus();
});

function showReport(title, r) {
  $("#repTitle").textContent = "Symulacja: " + title;
  const treeHtml = diffTree(r.planned);
  const rows = r.planned.map(([k, a]) => `<tr><td><span class="kind ${actCls(a)}">${ACTION(a)}</span></td><td data-nt>${esc(k)}${a.RenameRight || a.RenameLeft ? ` <span class="muted">(z: ${esc((a.RenameRight || a.RenameLeft).from)})</span>` : ""}</td></tr>`).join("");
  $("#repBody").innerHTML = `<p>Przeskanowano: <b>${r.scanned_left}</b> plików po lewej i <b>${r.scanned_right}</b> po prawej. Planowanych działań: <b>${r.planned.length}</b>. <span class="muted">Nic nie zostało zmienione.</span></p>` +
    (r.blocked ? `<div class="msg warn"><b>Zabezpieczenie zatrzymałoby synchronizację:</b> <span data-nt>${esc(r.blocked)}</span></div>` : "") +
    (r.errors.length ? `<div class="msg" data-nt>${r.errors.map(esc).join("<br>")}</div>` : "") +
    ((r.warnings || []).length ? `<div class="msg warn" data-nt>${r.warnings.map(esc).join("<br>")}</div>` : "") +
    (rows ? `<div class="scroll" style="margin-top:12px">${treeHtml}</div>` : '<p class="muted">Obie strony są zgodne — nic do zrobienia.</p>');
  $("#report").showModal();
}
$("#repClose").onclick = () => $("#report").close();

// ---------- kreator zadania ----------
const F = () => $("#jobForm").elements;
const STEP_NAMES = ["Krok 1 z 5 · Lokalizacje", "Krok 2 z 5 · Kierunek i usuwanie", "Krok 3 z 5 · Filtry i podfoldery", "Krok 4 z 5 · Harmonogram i warunki", "Krok 5 z 5 · Kosz, bezpieczeństwo i podsumowanie"];
let step = 0;
let vssInfo = { supported: false, admin: false, elevated_autostart: false };
const DEFAULT_JOB = { bandwidth: { kbps: 0, off_kbps: 0, hours: null }, use_vss: true, verify: false, max_versions: 0, conditions: { ac_only: false, wifi: [] }, triggers: { on_drive: false, on_wake: false, on_network: false }, id: "", name: "", left: "", right: "", mode: "TwoWay", conflict: "Newer", include: [], exclude: [], schedule: { type: "Realtime", poll_secs: 300 }, enabled: true, guard_pct: 25, trash_days: 30, threads: 4 };

const newId = () => (crypto.randomUUID ? crypto.randomUUID() : String(Date.now()) + Math.random().toString(16).slice(2));
let edOrig = null, edInit = {}, edId = "", saving = false;
const INIT_FIELDS = ["sched_val", "win_from", "win_to", "min_mb", "max_mb", "guard_pct", "threads", "trash_days", "max_versions", "bw_kbps", "bw_off", "bw_from", "bw_to"];

function openEditor(job, tpl) {
  const f = F(); $("#edErr").textContent = "";
  $("#edTitle").textContent = job ? "Edytuj zadanie" : "Nowe zadanie";
  const j = job || { ...DEFAULT_JOB, ...(tpl || {}) };
  edOrig = job || null; edId = job ? job.id : newId(); saving = false;
  $("#edSave").disabled = false;
  f.name.value = j.name; f.left.value = j.left; f.right.value = j.right; f.mode.value = j.mode; f.conflict.value = j.conflict;
  f.no_delete_left.checked = !!j.no_delete_left; f.no_delete_right.checked = !!j.no_delete_right; f.only_add.checked = !!j.only_add;
  f.include.value = (j.include || []).join("\n"); f.exclude.value = (j.exclude || []).join("\n");
  f.min_mb.value = j.min_size != null ? j.min_size / 1048576 : ""; f.max_mb.value = j.max_size != null ? j.max_size / 1048576 : "";
  f.sched.value = j.schedule.type;
  f.sched_val.value = j.schedule.type === "Interval" ? Math.round(j.schedule.secs / 60) : j.schedule.type === "Daily" ? `${String(j.schedule.hour).padStart(2, "0")}:${String(j.schedule.minute).padStart(2, "0")}` : j.schedule.type === "Realtime" ? Math.round(j.schedule.poll_secs / 60) : "";
  f.win_from.value = j.window ? j.window[0] : ""; f.win_to.value = j.window ? j.window[1] : "";
  f.bw_kbps.value = j.bandwidth?.kbps || 0; f.bw_off.value = j.bandwidth?.off_kbps || 0; f.bw_from.value = j.bandwidth?.hours ? j.bandwidth.hours[0] : ""; f.bw_to.value = j.bandwidth?.hours ? j.bandwidth.hours[1] : "";
  f.use_trash.checked = j.trash_days > 0; f.trash_days.value = j.trash_days > 0 ? j.trash_days : 30; f.max_versions.value = j.max_versions || 0; f.verify.checked = !!j.verify;
  f.ac_only.checked = !!j.conditions?.ac_only; f.wifi.value = (j.conditions?.wifi || []).join("\n");
  f.on_drive.checked = !!j.triggers?.on_drive; f.on_wake.checked = !!j.triggers?.on_wake; f.on_network.checked = !!j.triggers?.on_network; trashToggle(); $("#tree").innerHTML = "";
  f.guard_pct.value = j.guard_pct; f.threads.value = j.threads; f.enabled.checked = j.enabled; f.use_vss.checked = j.use_vss !== false; $("#vssRow").hidden = !vssInfo.supported;
  edInit = { sched: f.sched.value }; INIT_FIELDS.forEach((n) => (edInit[n] = f[n].value));
  $$("[data-hint]").forEach((h) => (h.textContent = ""));
  schedHint(); nestHint(); showStep(0); $("#editor").showModal(); f.name.focus();
}
function showStep(n) {
  step = n;
  $$(".step").forEach((s) => s.classList.toggle("on", +s.dataset.step === n));
  $$("#steps span").forEach((s, i) => s.classList.toggle("on", i <= n));
  $("#stepName").textContent = STEP_NAMES[n];
  const prev = document.activeElement;
  $("#edBack").hidden = n === 0; $("#edNext").hidden = n === 4; $("#edSave").hidden = n !== 4;
  if (n === 4) $("#summary").innerHTML = summarize(readJob());
  // przycisk, który właśnie zniknął, nie może zostawić fokusu na BODY
  if (prev && $("#editor").open && (prev === document.body || prev.hidden || !prev.offsetParent)) {
    (n === 4 ? $("#edSave") : n === 0 ? F().name : $("#edNext")).focus();
  }
}
function schedHint() {
  const f = F(), t = f.sched.value, l = $("#schedVal");
  l.style.display = t === "Manual" ? "none" : "";
  $("#schedLbl").textContent = t === "Interval" ? "Co ile minut" : t === "Daily" ? "O której godzinie (GG:MM)" : "Dodatkowe sprawdzanie co ile minut (dyski sieciowe i chmura)";
  f.sched_val.placeholder = t === "Daily" ? "02:30" : "5";
}
$$("input[name=sched]").forEach((r) => r.addEventListener("change", schedHint));

// ---- lokalizacje: normalizacja i wykrywanie zagnieżdżenia (jedna ścieżka wewnątrz drugiej)
function normLoc(p) {
  let x = p.trim().replace(/\\/g, "/");
  const unc = x.startsWith("//");
  x = x.replace(/\/{2,}/g, "/"); if (unc) x = "/" + x;
  if (x.length > 1) x = x.replace(/\/+$/, "") || "/";
  if (/^[A-Za-z]:/.test(x) || unc) x = x.toLowerCase(); // ścieżki Windows nie rozróżniają wielkości liter
  return x;
}
const isInside = (a, b) => a !== b && a.startsWith(b) && (b.endsWith("/") || b.endsWith(":") || a[b.length] === "/");
function nestHint() {
  const l = normLoc(F().left.value), r = normLoc(F().right.value);
  $("#nestWarn").hidden = !(l && r && l !== r && (isInside(l, r) || isInside(r, l)));
}
["left", "right"].forEach((n) => F()[n].addEventListener("input", nestHint));

// ---- odczyt i walidacja formularza (wszystkie kroki naraz; natywna walidacja HTML5 jest wyłączona: novalidate)
// Zwraca { job, errors:[{step, field, msg}] }. Wartość, której użytkownik nie ruszył, jest przenoszona
// z zadania źródłowego bez zmian (np. poll_secs=20 nie zamienia się w 300, max_size=0 zostaje 0).
function collect() {
  const f = F(), errors = [], orig = edOrig;
  const err = (step, field, msg) => errors.push({ step, field, msg });
  const same = (...names) => !!orig && names.every((n) => f[n].value === edInit[n]);
  const lines = (v) => v.split("\n").map((x) => x.trim()).filter(Boolean);
  const raw = (n) => f[n].value.trim();
  // liczba całkowita z zakresu; puste => def (albo null)
  const int = (step, n, label, min, max, def) => {
    const v = raw(n); if (v === "") return def;
    if (!/^\d+$/.test(v) || +v < min || +v > max) { err(step, n, `${label}: podaj liczbę całkowitą od ${min} do ${max}.`); return def; }
    return +v;
  };
  const left = raw("left"), right = raw("right");
  if (!left || !right) err(0, !left ? "left" : "right", "Podaj obie lokalizacje.");
  else if (left === right || normLoc(left) === normLoc(right)) err(0, "right", "Lewa i prawa lokalizacja nie mogą być takie same.");

  // harmonogram
  const t = f.sched.value, v = raw("sched_val");
  let schedule;
  if (orig && orig.schedule.type === t && same("sched_val") && edInit.sched === t) schedule = orig.schedule;
  else if (t === "Manual") schedule = { type: "Manual" };
  else if (t === "Daily") {
    const m = /^(\d{1,2}):(\d{2})$/.exec(v);
    if (!m || +m[1] > 23 || +m[2] > 59) { err(3, "sched_val", "Podaj godzinę w formacie GG:MM (godzina 0–23, minuty 00–59), np. 02:30."); schedule = { type: "Daily", hour: 2, minute: 0 }; }
    else schedule = { type: "Daily", hour: +m[1], minute: +m[2] };
  } else {
    const mins = int(3, "sched_val", "Odstęp w minutach", 1, 525600, t === "Interval" ? 15 : 5);
    schedule = t === "Interval" ? { type: "Interval", secs: mins * 60 } : { type: "Realtime", poll_secs: mins * 60 };
  }
  // okno godzin
  let win;
  if (same("win_from", "win_to")) win = orig.window ?? null;
  else {
    const a = raw("win_from"), b = raw("win_to");
    if ((a === "") !== (b === "")) { err(3, a === "" ? "win_from" : "win_to", "Okno czasowe: podaj obie godziny (od i do) albo zostaw oba pola puste."); win = null; }
    else if (a === "") win = null;
    else { const x = int(3, "win_from", "Okno czasowe, od godziny", 0, 23, null), y = int(3, "win_to", "Okno czasowe, do godziny", 0, 23, null); win = x === null || y === null ? null : [x, y]; }
  }
  // rozmiary (MB -> bajty); puste lub 0 = bez limitu
  const size = (n, label) => {
    if (same(n)) return orig[n === "min_mb" ? "min_size" : "max_size"] ?? null;
    const x = raw(n); if (x === "") return null;
    const num = Number(x);
    if (!Number.isFinite(num) || num < 0 || num > 1e9) { err(2, n, `${label}: podaj liczbę MB od 0 do 1 000 000 000 (puste lub 0 = bez limitu).`); return null; }
    const bytes = Math.round(num * 1048576);
    return bytes === 0 ? null : bytes;
  };
  const minSize = size("min_mb", "Min. rozmiar pliku"), maxSize = size("max_mb", "Maks. rozmiar pliku");
  if (minSize !== null && maxSize !== null && minSize > maxSize) err(2, "min_mb", "Min. rozmiar pliku nie może być większy niż maksymalny.");
  // krok 5
  const useTrash = f.use_trash.checked;
  const keep = (n, orv, fn) => (same(n) ? orv : fn());
  const guard = keep("guard_pct", orig?.guard_pct, () => int(4, "guard_pct", "Blokada masowego usuwania", 1, 100, 25));
  const threads = keep("threads", orig?.threads, () => int(4, "threads", "Równoległe transfery", 1, 16, 4));
  const trashDays = useTrash ? keep("trash_days", orig?.trash_days, () => int(4, "trash_days", "Dni w koszu", 1, 36500, 30)) : 0;
  const maxVer = keep("max_versions", orig?.max_versions, () => int(4, "max_versions", "Maks. wersji pliku", 0, 100000, 0));
  let bw;
  if (same("bw_kbps", "bw_off", "bw_from", "bw_to")) bw = { kbps: orig.bandwidth?.kbps || 0, off_kbps: orig.bandwidth?.off_kbps || 0, hours: orig.bandwidth?.hours ?? null };
  else {
    const kb = int(4, "bw_kbps", "Limit prędkości", 0, 4294967295, 0), off = int(4, "bw_off", "Limit poza godzinami", 0, 4294967295, 0);
    const a = raw("bw_from"), b = raw("bw_to"); let hours = null;
    if ((a === "") !== (b === "")) err(4, a === "" ? "bw_from" : "bw_to", "Godziny limitu: podaj obie (od i do) albo zostaw oba pola puste.");
    else if (a !== "") { const x = int(4, "bw_from", "Limit od godziny", 0, 23, null), y = int(4, "bw_to", "Limit do godziny", 0, 23, null); if (x !== null && y !== null) hours = [x, y]; }
    bw = { kbps: kb, off_kbps: off, hours };
  }
  const job = {
    ...(orig || {}),
    id: edId, name: f.name.value.trim() || `${left} ⇄ ${right}`, left, right, mode: f.mode.value, conflict: f.conflict.value,
    no_delete_left: f.no_delete_left.checked, no_delete_right: f.no_delete_right.checked, only_add: f.only_add.checked,
    include: lines(f.include.value), exclude: lines(f.exclude.value), min_size: minSize, max_size: maxSize,
    schedule, window: win, enabled: f.enabled.checked,
    guard_pct: guard ?? 25, trash_days: trashDays ?? 0, max_versions: maxVer ?? 0, verify: f.verify.checked, bandwidth: bw,
    conditions: { ...(orig?.conditions || {}), ac_only: f.ac_only.checked, wifi: lines(f.wifi.value) }, triggers: { on_drive: f.on_drive.checked, on_wake: f.on_wake.checked, on_network: f.on_network.checked }, threads: threads ?? 4, use_vss: f.use_vss.checked,
  };
  return { job, errors };
}
const readJob = () => collect().job;
function summarize(j) {
  const L = `<b data-nt>${esc(j.left || "lewa")}</b>`, R = `<b data-nt>${esc(j.right || "prawa")}</b>`, li = [];
  li.push(j.mode === "TwoWay" ? `Zmiany z ${L} i ${R} są przenoszone na drugą stronę.` : j.mode === "LeftToRight" ? `Pliki z ${L} są kopiowane do ${R}.` : `Pliki z ${R} są kopiowane do ${L}.`);
  if (j.only_add) li.push("Nic nie jest nadpisywane ani usuwane — tylko nowe pliki są dodawane.");
  else {
    if (j.no_delete_left) li.push("Po lewej stronie nic nie jest usuwane.");
    if (j.no_delete_right) li.push("Po prawej stronie nic nie jest usuwane.");
    if (!j.no_delete_left && !j.no_delete_right) li.push(j.trash_days > 0 ? `Usunięte i nadpisane pliki trafiają do kosza na ${j.trash_days} dni${j.max_versions ? ` (maks. ${j.max_versions} wersji pliku)` : ""}.` : "Kosz wyłączony: usunięte i nadpisane pliki są tracone bez możliwości przywrócenia.");
  }
  if (j.bandwidth?.kbps || j.bandwidth?.off_kbps) li.push(j.bandwidth.hours ? `Limit prędkości: ${j.bandwidth.kbps || "bez limitu"} KB/s w godzinach ${j.bandwidth.hours[0]}:00–${j.bandwidth.hours[1]}:00, poza nimi ${j.bandwidth.off_kbps || "bez limitu"} KB/s.` : `Limit prędkości: ${j.bandwidth.kbps} KB/s.`);
  if (j.verify) li.push("Po kopiowaniu sprawdzana jest suma kontrolna pliku.");
  if (j.conditions?.ac_only) li.push("Zadanie startuje tylko przy zasilaniu sieciowym.");
  if ((j.conditions?.wifi || []).length) li.push(`Zadanie startuje tylko w sieciach Wi-Fi: <span data-nt>${esc(j.conditions.wifi.join(", "))}</span>.`);
  const tr = [j.triggers?.on_drive && "po podłączeniu dysku", j.triggers?.on_wake && "po wybudzeniu z uśpienia", j.triggers?.on_network && "po zmianie sieci"].filter(Boolean);
  if (tr.length) li.push(`Dodatkowo uruchamia się: ${tr.join(", ")}.`);
  li.push(`Uruchamianie: ${schedText(j.schedule)}${j.window ? `, tylko w godzinach ${j.window[0]}:00–${j.window[1]}:00` : ""}.`);
  return `<b>Jak to będzie działać</b><ul>${li.map((x) => `<li>${x}</li>`).join("")}</ul>`;
}
// błędy tylko dla danego kroku (przycisk „Dalej”)
function validateStep(n) { return collect().errors.find((e) => e.step === n)?.msg || ""; }
// pierwszy błąd w całym formularzu: przeskok do właściwego kroku, komunikat i fokus na polu
function failFirst(errors) {
  const e = errors[0]; if (!e) return false;
  showStep(e.step); $("#edErr").textContent = e.msg;
  const el = F()[e.field]; if (el && el.focus) el.focus();
  return true;
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
  if (saving) return;
  if (step < 4) return $("#edNext").click();
  const { job, errors } = collect();
  if (failFirst(errors)) return;
  saving = true; $("#edSave").disabled = true; $("#edErr").textContent = "";
  try { await invoke("save_job", { job }); }
  catch (er) { saving = false; $("#edSave").disabled = false; $("#edErr").textContent = errText(er, "Nie udało się zapisać zadania: dane w formularzu są niepoprawne. Sprawdź wartości liczbowe w krokach 3–5."); return; }
  saving = false; $("#editor").close(); toast("Zadanie zapisane", "ok");
  await refreshJobs(); refreshStatus();
});

// ---------- aktywność ----------
function fillHistJobs() {
  const cur = $("#histJob").value;
  $("#histJob").innerHTML = '<option value="">Wszystkie zadania</option>' + jobs.map((j) => `<option value="${esc(j.id)}" data-nt>${esc(j.name)}</option>`).join("");
  $("#histJob").value = cur;
}
const kindClass = (k) => (/błąd/.test(k) ? "bad" : /zajęty|konflikt/.test(k) ? "warn" : /kopiowanie|rozstrzygn/.test(k) ? "ok" : "");
const HIST_LIMIT = 500;
let histSeq = 0, histErr = "";
async function loadHistory() {
  const my = ++histSeq, job = $("#histJob").value || null;
  try {
    const h = await invoke("history", { job, limit: HIST_LIMIT });
    if (my !== histSeq) return; // nowsze żądanie (inny filtr) wyprzedziło tę odpowiedź
    lastHist = h; histErr = ""; clearWarn("history");
  } catch (e) {
    if (my !== histSeq) return;
    lastHist = []; histErr = errText(e, "Nie udało się odczytać historii."); warnOnce("history", "Nie udało się wczytać historii: " + histErr);
  }
  renderHistory(); renderPending();
}
// komunikaty backendu (znany zestaw) tłumaczone dokładnym dopasowaniem; reszta danych bez zmian
const trX = (t) => (window.i18nTr ? window.i18nTr(t) : t);
const histName = (id) => jobs.find((j) => j.id === id)?.name || id;
// wiersze widoczne po filtrze (używane też przez eksport CSV)
function histRows() {
  const q = $("#histQ").value.trim().toLowerCase();
  return lastHist.filter((h) => !q || `${h.kind} ${h.path} ${h.detail} ${histName(h.job)}`.toLowerCase().includes(q) || (window.i18nTr && window.i18nTr(h.kind).toLowerCase().includes(q)));
}
function renderHistory() {
  const rows = histRows();
  $("#hist tbody").innerHTML = histErr ? `<tr><td colspan="5" class="err" data-nt>${esc("Nie udało się wczytać historii: " + histErr)}</td></tr>`
    : rows.map((h) => `<tr><td>${fmtTime(h.ts)}</td><td data-nt>${esc(histName(h.job))}</td><td><span class="kind ${kindClass(h.kind)}">${esc(h.kind)}</span></td><td data-nt>${esc(h.path)}</td><td class="muted" data-nt>${esc(trX(h.detail))}</td></tr>`).join("") || '<tr><td colspan="5" class="muted">Brak wpisów.</td></tr>';
  const n = $("#histNote"), full = !histErr && lastHist.length >= HIST_LIMIT;
  n.hidden = !full;
  n.textContent = full ? `Pokazano najnowsze ${HIST_LIMIT} wpisów (limit widoku) — starsze są pominięte. Eksport CSV obejmuje tylko wpisy widoczne po filtrze.` : "";
}
function renderPending() {
  const out = [];
  for (const j of jobs) {
    const st = overview.status[j.id], r = st?.last; if (!r) continue;
    const who = (p) => `<b data-nt>${esc(j.name)}</b><br><span data-nt>${esc(p)}</span>`;
    r.conflicts.forEach((p) => out.push(`<tr><td><span class="kind warn">konflikt</span></td><td>${who(p)}</td><td class="muted">${conflictInfo(r, p)}</td><td style="white-space:nowrap"><button class="sm" data-res="left" data-job="${esc(j.id)}" data-path="${esc(p)}">Zachowaj lewą</button> <button class="sm" data-res="right" data-job="${esc(j.id)}" data-path="${esc(p)}">Zachowaj prawą</button></td></tr>`));
    r.busy.forEach((p) => out.push(`<tr><td><span class="kind warn">zajęty</span></td><td>${who(p)}</td><td class="muted" colspan="2">otwarty w innym programie — ponowię automatycznie${r.vss_note ? ` · VSS: <span data-nt>${esc(r.vss_note)}</span>` : ""}${st.next_run ? " (" + new Date(st.next_run * 1000).toLocaleTimeString(LOCALE) + ")" : ""}</td></tr>`));
    r.errors.forEach((p) => out.push(`<tr><td><span class="kind bad">błąd</span></td><td colspan="3">${who(trX(p))}</td></tr>`));
    if (st.error) out.push(`<tr><td><span class="kind bad">błąd</span></td><td colspan="3">${who(trX(st.error))}</td></tr>`);
  }
  // przebudowa tylko przy zmianie treści: zachowuje fokus i trwające kliknięcia
  setHtml($("#pending"), out.length ? `<table><tbody>${out.join("")}</tbody></table>` : '<span class="muted">✓ Nic nie czeka na Twoją uwagę.</span>');
}
$("#pending").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-res]"); if (!b) return;
  b.disabled = true;
  try { await invoke("resolve_conflict", { id: b.dataset.job, path: b.dataset.path, leftWins: b.dataset.res === "left" }); toast("Konflikt rozstrzygnięty", "ok"); }
  catch (er) { toast(errText(er), "bad"); b.disabled = false; }
  loadHistory(); refreshStatus();
});
$("#histJob").onchange = loadHistory;
$("#histQ").oninput = renderHistory;
// komórka CSV: cudzysłowy podwajane; początek =,+,-,@ (także po białych znakach) neutralizowany apostrofem (wstrzyknięcie formuł)
const csvCell = (s) => { let t = String(s ?? ""); if (/^\s*[=+\-@]/.test(t) || /^[\t\r]/.test(t)) t = "'" + t; return `"${t.replace(/"/g, '""')}"`; };
$("#exportCsv").onclick = () => {
  const csv = "﻿" + ["czas;zadanie;operacja;plik;szczegóły", ...histRows().map((h) => [fmtTime(h.ts), histName(h.job), h.kind, h.path, h.detail].map(csvCell).join(";"))].join("\r\n");
  const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" })); a.download = "syncron-historia.csv"; a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 10000);
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
  if ($("#rAdd").disabled) return;
  const params = { ...(prov.extra || {}) };
  // hasła wysyłamy bez zmian (spacje mogą być częścią hasła); pozostałe pola przycinamy
  $$("#rFields input").forEach((i) => (params[i.dataset.f] = i.type === "password" ? i.value : i.value.trim()));
  const name = $("#rName").value.trim();
  if (remotes.some((r) => r.name === name) && !(await ask("Nadpisać istniejące konto?", `Konto „${name}” już istnieje. Dodanie konta pod tą samą nazwą nadpisze jego konfigurację. Aby tego uniknąć, zmień nazwę.`, "Nadpisz", true))) return;
  $("#rMsg").textContent = "Łączenie… (dokończ logowanie w przeglądarce, jeśli się otworzyła)"; $("#rAdd").disabled = true;
  try { await invoke("add_remote", { name, kind: prov.kind, params }); $("#rMsg").textContent = ""; $("#rForm").hidden = true; toast("Konto dodane", "ok"); await loadRemotes(); }
  catch (e) { $("#rMsg").textContent = "Błąd: " + e; }
  $("#rAdd").disabled = false;
};
let remotesSeq = 0;
async function loadRemotes() {
  const my = ++remotesSeq; let err = "";
  try { const r = await invoke("list_remotes"); if (my !== remotesSeq) return; remotes = r; clearWarn("list_remotes"); }
  catch (e) { if (my !== remotesSeq) return; err = errText(e, "Nie udało się odczytać listy kont."); warnOnce("list_remotes", "Nie udało się wczytać kont: " + err); }
  fillRemoteSelects();
  $("#remotes").innerHTML = err ? `<span class="err" data-nt>${esc("Nie udało się wczytać kont: " + err)}</span>`
    : remotes.length ? remotes.map((r) => `<div class="remote"><span class="loc" style="padding:0;border:0;background:none"><span class="ic">${icon("cloud")}</span></span><div class="grow"><b data-nt>${esc(r.name)}:</b> <span class="muted" data-nt>${esc(r.kind)}</span></div><button class="sm" data-t="${esc(r.name)}">Testuj</button><button class="sm danger" data-d="${esc(r.name)}">Usuń</button></div>`).join("") : '<span class="muted">Brak połączonych kont. Wybierz usługę poniżej.</span>';
}
$("#remotes").addEventListener("click", async (e) => {
  const t = e.target.closest("[data-t]")?.dataset.t, d = e.target.closest("[data-d]")?.dataset.d;
  try {
    if (t) { e.target.disabled = true; toast(await invoke("test_remote", { name: t }), "ok"); e.target.disabled = false; }
    if (d && (await ask("Usunąć konto?", `Konto „${d}” zostanie usunięte z aplikacji (dane w chmurze zostają).`, "Usuń", true))) { await invoke("delete_remote", { name: d }); await loadRemotes(); }
  } catch (err) { toast("Błąd: " + err, "bad"); loadRemotes(); }
});

// ---------- ustawienia i start ----------
$("#pauseAll").onclick = async () => {
  try { await invoke("set_paused", { paused: !overview.paused }); } catch (e) { toast("Nie udało się zmienić stanu wstrzymania: " + errText(e), "bad"); }
  refreshStatus();
};
(async () => {
  try { $("#autostart").checked = await autostart.isEnabled(); } catch {}
  $("#autostart").onchange = async (e) => { try { e.target.checked ? await autostart.enable() : await autostart.disable(); } catch (err) { toast(String(err), "bad"); } };
  try { $("#rcv").textContent = await invoke("rclone_version"); } catch { $("#rcv").textContent = "niedostępny (foldery lokalne i dyski sieciowe działają; chmury wymagają rclone)"; }
})();
async function loadVss() {
  try { vssInfo = await invoke("vss_status"); } catch {}
  renderJobs();
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
  const cls = actCls;
  const render = (node, name, open) => {
    const items = node.items.map(([f, a]) => `<div style="padding:2px 0 2px 18px"><span class="kind ${cls(a)}">${ACTION(a)}</span> <span data-nt>${esc(f)}</span></div>`).join("");
    const kids = Object.entries(node.kids).sort(([a], [b]) => a.localeCompare(b, LOCALE)).map(([n, c]) => render(c, n, false)).join("");
    return name === null ? items + kids : `<details ${open ? "open" : ""} style="margin-left:6px"><summary style="cursor:pointer;padding:3px 0"><b data-nt>${esc(name)}</b> <span class="muted">(${node.n})</span></summary>${items}${kids}</details>`;
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
      return `<div class="treerow" data-path="${esc(path)}"><label class="sw" style="padding:4px 8px;margin:2px 0"><input type="checkbox" ${inc.includes(path + "/*") ? "checked" : ""}><span data-nt>${esc(d)}</span></label><button type="button" class="sm ghost" data-exp="1">▸</button><div class="kids" style="margin-left:22px"></div></div>`;
    }).join("") : '<span class="muted">Brak podfolderów.</span>';
  } catch (e) { box.innerHTML = `<span class="err" data-nt>${esc(String(e))}</span>`; }
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
let trashItems = [], trashSeq = 0;
const stampDate = (st) => { const m = /^(\d{4})-(\d{2})-(\d{2})_(\d{2})(\d{2})(\d{2})/.exec(st); return m ? new Date(+m[1], m[2] - 1, +m[3], +m[4], +m[5], +m[6]).toLocaleString(LOCALE) : st; };
const fmtSize = (n) => (n < 1024 ? n + " B" : n < 1048576 ? (n / 1024).toFixed(1) + " KB" : n < 1073741824 ? (n / 1048576).toFixed(1) + " MB" : (n / 1073741824).toFixed(2) + " GB");
function fillTrashJobs() {
  const cur = $("#trashJob").value;
  $("#trashJob").innerHTML = jobs.map((j) => `<option value="${esc(j.id)}" data-nt>${esc(j.name)}</option>`).join("");
  if (cur && jobs.some((j) => j.id === cur)) $("#trashJob").value = cur;
}
async function loadTrash() {
  fillTrashJobs();
  const my = ++trashSeq, id = $("#trashJob").value, job = jobs.find((j) => j.id === id);
  trashItems = []; trashShown = "";
  if (!job) { $("#trashTbl tbody").innerHTML = '<tr><td colspan="5" class="muted">Brak zadań.</td></tr>'; return; }
  $("#trashNote").textContent = job.trash_days > 0 ? `Kosz tego zadania jest włączony (${job.trash_days} dni${job.max_versions ? `, maks. ${job.max_versions} wersji pliku` : ""}).` : "Kosz tego zadania jest wyłączony — nowe pliki nie są odkładane. Włącz go w edycji zadania (krok 5).";
  $("#trashTbl tbody").innerHTML = '<tr><td colspan="5" class="muted">Wczytywanie…</td></tr>';
  let items;
  try { items = await invoke("trash_list", { id }); } catch (e) { if (my !== trashSeq) return; $("#trashTbl tbody").innerHTML = `<tr><td colspan="5" class="err" data-nt>${esc(String(e))}</td></tr>`; return; }
  if (my !== trashSeq) return; // odpowiedź dla zadania, które użytkownik już zmienił
  trashItems = items; trashShown = id;
  renderTrash();
}
let trashShown = "";
function renderTrash() {
  const q = $("#trashQ").value.trim().toLowerCase();
  const byFile = {};
  for (const it of trashItems) if (!q || it.path.toLowerCase().includes(q)) (byFile[it.side + "|" + it.path] ||= []).push(it);
  const rows = Object.values(byFile).sort((a, b) => a[0].path.localeCompare(b[0].path, LOCALE)).flatMap((vs) =>
    vs.sort((a, b) => b.stamp.localeCompare(a.stamp)).map((it, i) => `<tr><td>${i === 0 ? `<b data-nt>${esc(it.path)}</b>${vs.length > 1 ? ` <span class="muted">(${vs.length} wersji)</span>` : ""}` : ""}</td><td>${it.side === "left" ? "lewa" : "prawa"}</td><td>${esc(stampDate(it.stamp))}</td><td>${fmtSize(it.size)}</td>
      <td style="white-space:nowrap"><button class="sm" data-tr="restore" data-job="${esc(trashShown)}" data-side="${it.side}" data-stamp="${esc(it.stamp)}" data-path="${esc(it.path)}">Przywróć</button> <button class="sm danger" data-tr="del" data-job="${esc(trashShown)}" data-side="${it.side}" data-stamp="${esc(it.stamp)}" data-path="${esc(it.path)}">Usuń</button></td></tr>`));
  $("#trashTbl tbody").innerHTML = rows.join("") || '<tr><td colspan="5" class="muted">Kosz jest pusty.</td></tr>';
}
$("#trashJob").onchange = loadTrash; $("#trashQ").oninput = renderTrash;
$("#trashTbl").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-tr]"); if (!b) return;
  const d = b.dataset, id = d.job; if (!id) return; // id zadania, do którego należy wyświetlony wiersz (nie bieżący wybór listy)
  try {
    if (b.dataset.tr === "restore") { const to = await invoke("trash_restore", { id, side: d.side, stamp: d.stamp, path: d.path }); toast("Przywrócono jako: " + to, "ok"); invoke("run_now", { id, force: false }).catch(() => {}); }
    else if (await ask("Usunąć trwale?", `Wersja pliku „${d.path}” zostanie usunięta bez możliwości przywrócenia.`, "Usuń", true)) { await invoke("trash_delete", { id, side: d.side, stamp: d.stamp, path: d.path }); toast("Usunięto"); }
  } catch (err) { toast(errText(err), "bad"); }
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
  let st = { notify: true, language: LANG, unavailable_minutes: 30 }, ok = true;
  try { st = await invoke("get_settings"); if (window.SyncronScale) SyncronScale.init(st); }
  catch (e) {
    ok = false; toast("Nie udało się wczytać ustawień — widoczne wartości są domyślne i nie zostaną zapisane: " + errText(e, "błąd odczytu"), "bad");
    $$("#notify,#unavail").forEach((i) => (i.disabled = true));
  }
  // świeży start (pusta pamięć przeglądarki): język zapisany w ustawieniach (używany też przez powiadomienia) wyznacza język interfejsu
  if (ok && (st.language === "pl" || st.language === "en") && st.language !== LANG && store.get("lang") === null) {
    store.set("lang", st.language);
    if (store.get("lang") === st.language) { location.reload(); return; }
  }
  $("#notify").checked = st.notify;
  $(`input[name=lang][value=${LANG}]`).checked = true;
  let lastUnavail = st.unavailable_minutes ?? 30;
  $("#unavail").value = lastUnavail;
  const save = async () => {
    if (!ok) return false;
    try { const ns = Object.assign({}, window.SyncronScale ? SyncronScale.raw() : {}, { notify: $("#notify").checked, language: $("input[name=lang]:checked").value, unavailable_minutes: lastUnavail }); await invoke("set_settings", { settings: ns }); if (window.SyncronScale) SyncronScale.adopt(ns); return true; }
    catch (e) { toast(errText(e), "bad"); return false; }
  };
  $("#notify").onchange = save;
  $("#unavail").onchange = async () => {
    const v = $("#unavail").value.trim();
    if (v === "") { $("#unavail").value = lastUnavail; return; } // puste = bez zmian
    if (!/^\d+$/.test(v) || +v > 4294967295) { toast("Podaj liczbę całkowitą minut (0 lub więcej); 0 wyłącza powiadomienie.", "bad"); $("#unavail").value = lastUnavail; return; }
    const prev = lastUnavail; lastUnavail = +v;
    if (!(await save())) { lastUnavail = prev; $("#unavail").value = prev; }
  };
  $$("input[name=lang]").forEach((r) => (r.onchange = () => { store.set("lang", r.value); save().then(() => location.reload()); }));
})();
async function checkUpdates(silent) {
  const msg = $("#updMsg");
  // ciche sprawdzanie nie pojawia się nad otwartym oknem (potwierdzenie, kreator, raport) — czeka i ponawia
  if (silent && $$("dialog[open]").length) { setTimeout(() => checkUpdates(true), 30000); return; }
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
  try { const p = await dialogApi.save({ defaultPath: "syncron-konfiguracja.json", filters: [{ name: "JSON", extensions: ["json"] }] }); if (p) toast(`Wyeksportowano zadań: ${await invoke("export_config", { path: p })}`, "ok"); } catch (e) { toast(errText(e), "bad"); }
};
$("#cfgImport").onclick = async () => {
  try { const p = await dialogApi.open({ multiple: false, filters: [{ name: "JSON", extensions: ["json"] }] }); if (p) { toast(`Zaimportowano zadań: ${await invoke("import_config", { path: p })}`, "ok"); await refreshJobs(); } } catch (e) { toast(errText(e), "bad"); }
};

// ---------- dziennik i diagnostyka ----------
$("#logsOpen").onclick = () => invoke("open_logs_dir").catch((e) => toast(String(e), "bad"));
$("#diagCopy").onclick = async () => {
  try {
    const text = await invoke("get_diagnostics");
    try { await navigator.clipboard.writeText(text); toast("Diagnostyka skopiowana do schowka", "ok"); }
    catch { $("#repTitle").textContent = "Diagnostyka"; $("#repBody").innerHTML = `<p class="muted">Zaznacz całość (Ctrl/Cmd+A) i skopiuj.</p><textarea readonly rows="18" style="font-family:monospace;font-size:12px">${esc(text)}</textarea>`; $("#report").showModal(); }
  } catch (e) { toast(String(e), "bad"); }
};

// podgląd konfliktu: rozmiar i czas obu wersji, z zaznaczeniem nowszej/większej
function conflictInfo(rep, path) {
  const d = (rep.conflict_details || []).find((x) => x.path === path);
  if (!d || !d.left || !d.right) return "zmieniono po obu stronach";
  const side = (name, m, o) => `<b>${name}</b> ${fmtSize(m.size)}${m.size > o.size ? " (większa)" : ""}, ${fmtTime(m.mtime)}${m.mtime > o.mtime ? " (nowsza)" : ""}`;
  return `${side("Lewa:", d.left, d.right)}<br>${side("Prawa:", d.right, d.left)}`;
}
