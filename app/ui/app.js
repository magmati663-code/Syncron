const { invoke } = window.__TAURI__.core;
const dialogApi = window.__TAURI__.dialog;
const autostart = window.__TAURI__.autostart;
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const STATE = { idle: "Gotowe", running: "Synchronizuję…", waiting: "Oczekuje (pliki zajęte)", error: "Błąd", blocked: "Zablokowane", paused: "Wstrzymane", disabled: "Wyłączone" };
const MODE = { TwoWay: "⇄ dwustronna", LeftToRight: "→ lewa do prawej", RightToLeft: "← prawa do lewej" };
const ACTION = (a) => typeof a === "string"
  ? ({ CopyToRight: "kopiuj →", CopyToLeft: "← kopiuj", DeleteLeft: "usuń po lewej", DeleteRight: "usuń po prawej" }[a] || a)
  : "konflikt";
let jobs = [], remotes = [], overview = { paused: false, status: {} }, lastHist = [];

const fmtTime = (t) => (t ? new Date(t * 1000).toLocaleString("pl-PL") : "—");
const schedText = (s) => ({ Manual: "ręcznie", Realtime: "na bieżąco", Interval: `co ${Math.round((s.secs || 0) / 60)} min`, Daily: `codziennie ${String(s.hour).padStart(2, "0")}:${String(s.minute).padStart(2, "0")}` }[s.type] || "");

function toast(msg) { alert(msg); }

// ---------- nawigacja ----------
$$("nav button").forEach((b) => b.addEventListener("click", () => {
  $$("nav button").forEach((x) => x.classList.toggle("active", x === b));
  $$(".tab").forEach((t) => t.classList.toggle("active", t.id === "tab-" + b.dataset.tab));
  if (b.dataset.tab === "activity") loadHistory();
  if (b.dataset.tab === "accounts") loadRemotes();
}));

// ---------- zadania ----------
async function refreshJobs() { jobs = await invoke("list_jobs"); renderJobs(); fillHistJobs(); }
async function refreshStatus() {
  overview = await invoke("overview");
  $("#pauseAll").textContent = overview.paused ? "▶ Wznów wszystko" : "⏸ Wstrzymaj wszystko";
  renderJobs();
  if ($("#tab-activity").classList.contains("active")) renderPending();
}

function renderJobs() {
  const box = $("#jobs");
  if (!jobs.length) { box.innerHTML = '<div class="empty">Brak zadań. Kliknij „+ Nowe zadanie”, aby zsynchronizować pierwsze foldery.</div>'; return; }
  box.innerHTML = jobs.map((j) => {
    const st = overview.status[j.id] || {};
    const state = overview.paused && j.enabled ? "paused" : st.state || (j.enabled ? "idle" : "disabled");
    const rep = st.last;
    let stats = st.last_run ? `Ostatnio: ${fmtTime(st.last_run)}` : "Jeszcze nie uruchomione";
    if (rep) stats += ` · skopiowano ${rep.copied}, usunięto ${rep.deleted}` + (rep.conflicts.length ? ` · konflikty ${rep.conflicts.length}` : "") + (rep.busy.length ? ` · zajęte ${rep.busy.length}` : "");
    if (st.next_run) stats += ` · następne: ${fmtTime(st.next_run)}`;
    const msg = st.error ? `<div class="err">${esc(st.error)}</div>` : rep?.blocked ? `<div class="err">Zatrzymano: ${esc(rep.blocked)}. Sprawdź symulację; „Wymuś” pominie blokadę.</div>` : "";
    return `<div class="card" data-id="${esc(j.id)}">
      <div class="top"><span class="name">${esc(j.name)}</span><span class="chip ${state}">${STATE[state] || state}</span></div>
      <div class="paths">${esc(j.left)} &nbsp;${MODE[j.mode] || ""}&nbsp; ${esc(j.right)}</div>
      <div class="stats">${schedText(j.schedule)}${j.no_delete_left ? " · bez usuwania po lewej" : ""}${j.no_delete_right ? " · bez usuwania po prawej" : ""}${j.only_add ? " · tylko dodawanie" : ""}</div>
      <div class="stats">${stats}</div>${msg}
      <div class="actions">
        <button data-act="run">▶ Uruchom teraz</button>
        <button data-act="dry">🔍 Symulacja</button>
        ${rep?.blocked ? '<button data-act="force">Wymuś</button>' : ""}
        <button data-act="edit">✎ Edytuj</button>
        <button data-act="toggle">${j.enabled ? "Wyłącz" : "Włącz"}</button>
        <button data-act="del" class="danger">Usuń</button>
      </div></div>`;
  }).join("");
}

$("#jobs").addEventListener("click", async (e) => {
  const b = e.target.closest("button[data-act]"); if (!b) return;
  const id = b.closest(".card").dataset.id, job = jobs.find((j) => j.id === id);
  try {
    switch (b.dataset.act) {
      case "run": await invoke("run_now", { id, force: false }); break;
      case "force":
        if (confirm("Wymusić synchronizację mimo zabezpieczenia? Pliki usuwane/nadpisywane trafią do kosza synchronizacji (jeśli włączony).")) await invoke("run_now", { id, force: true });
        break;
      case "dry": b.textContent = "Liczę…"; showReport(job.name, await invoke("dry_run", { id })); b.textContent = "🔍 Symulacja"; break;
      case "edit": openEditor(job); break;
      case "toggle": await invoke("save_job", { job: { ...job, enabled: !job.enabled } }); await refreshJobs(); break;
      case "del": if (confirm(`Usunąć zadanie „${job.name}”? Pliki nie zostaną usunięte.`)) { await invoke("delete_job", { id }); await refreshJobs(); } break;
    }
  } catch (err) { toast(String(err)); renderJobs(); }
  refreshStatus();
});

function showReport(title, r) {
  $("#repTitle").textContent = "Symulacja: " + title;
  const rows = r.planned.map(([k, a]) => `<tr><td>${ACTION(a)}${a.Conflict ? (a.Conflict.keep_both ? " (zachowaj obie)" : " (do decyzji)") : ""}</td><td>${esc(k)}</td></tr>`).join("");
  $("#repBody").innerHTML = `<p>Przeskanowano: ${r.scanned_left} plików po lewej, ${r.scanned_right} po prawej. Planowanych działań: <b>${r.planned.length}</b>.</p>` +
    (r.blocked ? `<p class="err">Zabezpieczenie zatrzymałoby synchronizację: ${esc(r.blocked)}</p>` : "") +
    (r.errors.length ? `<p class="err">${r.errors.map(esc).join("<br>")}</p>` : "") +
    (rows ? `<table><tbody>${rows}</tbody></table>` : '<p class="muted">Obie strony są zgodne.</p>');
  $("#report").showModal();
}
$("#repClose").onclick = () => $("#report").close();

// ---------- edytor ----------
const F = () => $("#jobForm").elements;
function openEditor(job) {
  const f = F(); $("#edErr").textContent = "";
  $("#edTitle").textContent = job ? "Edytuj zadanie" : "Nowe zadanie";
  const j = job || { id: "", name: "", left: "", right: "", mode: "TwoWay", conflict: "Newer", include: [], exclude: [], schedule: { type: "Realtime", poll_secs: 300 }, enabled: true, guard_pct: 25, trash_days: 30, threads: 4 };
  f.name.value = j.name; f.left.value = j.left; f.right.value = j.right; f.mode.value = j.mode; f.conflict.value = j.conflict;
  f.no_delete_left.checked = !!j.no_delete_left; f.no_delete_right.checked = !!j.no_delete_right; f.only_add.checked = !!j.only_add;
  f.include.value = (j.include || []).join("\n"); f.exclude.value = (j.exclude || []).join("\n");
  f.min_mb.value = j.min_size ? j.min_size / 1048576 : ""; f.max_mb.value = j.max_size ? j.max_size / 1048576 : "";
  f.sched.value = j.schedule.type;
  f.sched_val.value = j.schedule.type === "Interval" ? Math.round(j.schedule.secs / 60) : j.schedule.type === "Daily" ? `${String(j.schedule.hour).padStart(2, "0")}:${String(j.schedule.minute).padStart(2, "0")}` : j.schedule.type === "Realtime" ? Math.round(j.schedule.poll_secs / 60) : "";
  f.win_from.value = j.window ? j.window[0] : ""; f.win_to.value = j.window ? j.window[1] : "";
  f.trash_days.value = j.trash_days; f.guard_pct.value = j.guard_pct; f.threads.value = j.threads; f.enabled.checked = j.enabled;
  $("#jobForm").dataset.id = j.id;
  schedHint(); $("#editor").showModal();
}
function schedHint() {
  const f = F(), t = f.sched.value, l = $("#schedVal");
  l.style.display = t === "Manual" ? "none" : "";
  l.firstChild.textContent = t === "Interval" ? "Co ile minut" : t === "Daily" ? "Godzina (GG:MM)" : "Dodatkowe sprawdzanie co ile minut (dyski sieciowe/chmura)";
  f.sched_val.placeholder = t === "Daily" ? "02:30" : "5";
}
$("#jobForm").elements.sched.addEventListener("change", schedHint);
$("#newJob").onclick = () => openEditor(null);
$("#edCancel").onclick = () => $("#editor").close();

$$("button[data-pick]").forEach((b) => b.addEventListener("click", async () => {
  const dir = await dialogApi.open({ directory: true, multiple: false, title: "Wybierz folder" });
  if (dir) F()[b.dataset.pick].value = dir;
}));
$$("select[data-remote]").forEach((s) => s.addEventListener("change", () => { if (s.value) F()[s.dataset.remote].value = s.value + ":"; s.value = ""; }));
function fillRemoteSelects() {
  $$("select[data-remote]").forEach((s) => { s.innerHTML = '<option value="">Konto chmurowe…</option>' + remotes.map((r) => `<option value="${esc(r.name)}">${esc(r.name)} (${esc(r.kind)})</option>`).join(""); });
}

$("#jobForm").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = F(), lines = (v) => v.split("\n").map((x) => x.trim()).filter(Boolean);
  const num = (v) => (v === "" ? null : Number(v));
  const t = f.sched.value, v = f.sched_val.value.trim();
  let schedule;
  if (t === "Manual") schedule = { type: "Manual" };
  else if (t === "Interval") schedule = { type: "Interval", secs: Math.max(1, Number(v) || 15) * 60 };
  else if (t === "Daily") { const m = /^(\d{1,2}):(\d{2})$/.exec(v); if (!m) { $("#edErr").textContent = "Podaj godzinę w formacie GG:MM."; return; } schedule = { type: "Daily", hour: +m[1], minute: +m[2] }; }
  else schedule = { type: "Realtime", poll_secs: Math.max(1, Number(v) || 5) * 60 };
  const from = num(f.win_from.value), to = num(f.win_to.value);
  const job = {
    id: $("#jobForm").dataset.id || (crypto.randomUUID ? crypto.randomUUID() : String(Date.now())),
    name: f.name.value.trim(), left: f.left.value.trim(), right: f.right.value.trim(), mode: f.mode.value, conflict: f.conflict.value,
    no_delete_left: f.no_delete_left.checked, no_delete_right: f.no_delete_right.checked, only_add: f.only_add.checked,
    include: lines(f.include.value), exclude: lines(f.exclude.value),
    min_size: f.min_mb.value === "" ? null : Math.round(Number(f.min_mb.value) * 1048576),
    max_size: f.max_mb.value === "" ? null : Math.round(Number(f.max_mb.value) * 1048576),
    schedule, window: from !== null && to !== null ? [from, to] : null, enabled: f.enabled.checked,
    guard_pct: Number(f.guard_pct.value) || 25, trash_days: Number(f.trash_days.value) || 0, threads: Number(f.threads.value) || 4,
  };
  try { await invoke("save_job", { job }); $("#editor").close(); await refreshJobs(); refreshStatus(); }
  catch (err) { $("#edErr").textContent = String(err); }
});

// ---------- aktywność ----------
function fillHistJobs() {
  const cur = $("#histJob").value;
  $("#histJob").innerHTML = '<option value="">Wszystkie zadania</option>' + jobs.map((j) => `<option value="${esc(j.id)}">${esc(j.name)}</option>`).join("");
  $("#histJob").value = cur;
}
async function loadHistory() {
  try { lastHist = await invoke("history", { job: $("#histJob").value || null, limit: 500 }); } catch (e) { lastHist = []; }
  const name = (id) => jobs.find((j) => j.id === id)?.name || id;
  $("#hist tbody").innerHTML = lastHist.map((h) => `<tr><td>${fmtTime(h.ts)}</td><td>${esc(name(h.job))}</td><td>${esc(h.kind)}</td><td>${esc(h.path)}</td><td>${esc(h.detail)}</td></tr>`).join("") || '<tr><td colspan="5" class="muted">Brak wpisów.</td></tr>';
  renderPending();
}
function renderPending() {
  const out = [];
  for (const j of jobs) {
    const r = overview.status[j.id]?.last; if (!r) continue;
    r.busy.forEach((p) => out.push(`<tr><td>${esc(j.name)}</td><td>${esc(p)}</td><td>plik zajęty przez inny program — ponowię automatycznie</td></tr>`));
    r.conflicts.forEach((p) => out.push(`<tr><td>${esc(j.name)}</td><td>${esc(p)}</td><td>konflikt — zmiany po obu stronach</td></tr>`));
    r.errors.forEach((p) => out.push(`<tr><td>${esc(j.name)}</td><td colspan="2">${esc(p)}</td></tr>`));
  }
  $("#pending").innerHTML = out.length ? `<table><tbody>${out.join("")}</tbody></table>` : '<span class="muted">Nic nie oczekuje.</span>';
}
$("#histJob").onchange = loadHistory;
$("#exportCsv").onclick = () => {
  const q = (s) => `"${String(s ?? "").replace(/"/g, '""')}"`;
  const csv = "\ufeff" + ["czas;zadanie;operacja;plik;szczegóły", ...lastHist.map((h) => [fmtTime(h.ts), jobs.find((j) => j.id === h.job)?.name || h.job, h.kind, h.path, h.detail].map(q).join(";"))].join("\r\n");
  const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" })); a.download = "syncpolak-historia.csv"; a.click();
};

// ---------- konta ----------
const FIELDS = {
  drive: [], onedrive: [],
  bitrix: [["url", "Adres WebDAV z Bitrix24 (np. https://firma.bitrix24.pl/company/personal/user/1/disk/path/)"], ["user", "Login"], ["pass", "Hasło (lub hasło aplikacji)", "password"]],
  webdav: [["url", "Adres WebDAV"], ["user", "Login"], ["pass", "Hasło", "password"]],
  sftp: [["host", "Serwer"], ["user", "Login"], ["pass", "Hasło", "password"], ["port", "Port (22)"]],
};
function renderFields() {
  const t = $("#rType").value;
  $("#rFields").innerHTML = (FIELDS[t].map(([k, l, ty]) => `<label>${esc(l)}<input data-f="${k}" type="${ty || "text"}"></label>`).join("")) ||
    '<p class="muted">Po kliknięciu „Połącz konto” otworzy się przeglądarka z logowaniem.</p>';
}
$("#rType").onchange = renderFields;
$("#rAdd").onclick = async () => {
  const t = $("#rType").value, params = {};
  $$("#rFields input").forEach((i) => (params[i.dataset.f] = i.value.trim()));
  let kind = t;
  if (t === "bitrix") { kind = "webdav"; params.vendor = "other"; }
  if (t === "webdav") params.vendor = "other";
  $("#rMsg").textContent = "Łączenie… (dokończ logowanie w przeglądarce, jeśli się otworzyła)"; $("#rAdd").disabled = true;
  try { await invoke("add_remote", { name: $("#rName").value.trim(), kind, params }); $("#rMsg").textContent = "Konto dodane."; await loadRemotes(); }
  catch (e) { $("#rMsg").textContent = "Błąd: " + e; }
  $("#rAdd").disabled = false;
};
async function loadRemotes() {
  try { remotes = await invoke("list_remotes"); } catch (e) { remotes = []; }
  fillRemoteSelects();
  $("#remotes").innerHTML = remotes.length ? `<table><tbody>${remotes.map((r) => `<tr><td><b>${esc(r.name)}:</b></td><td>${esc(r.kind)}</td><td><button data-t="${esc(r.name)}">Testuj</button> <button class="danger" data-d="${esc(r.name)}">Usuń</button></td></tr>`).join("")}</tbody></table>` : '<span class="muted">Brak kont.</span>';
}
$("#remotes").addEventListener("click", async (e) => {
  const t = e.target.dataset.t, d = e.target.dataset.d;
  try {
    if (t) toast(await invoke("test_remote", { name: t }));
    if (d && confirm(`Usunąć konto „${d}”?`)) { await invoke("delete_remote", { name: d }); await loadRemotes(); }
  } catch (err) { toast("Błąd: " + err); }
});

// ---------- ustawienia ----------
$("#pauseAll").onclick = async () => { await invoke("set_paused", { paused: !overview.paused }); refreshStatus(); };
(async () => {
  try { $("#autostart").checked = await autostart.isEnabled(); } catch (e) {}
  $("#autostart").onchange = async (e) => { try { e.target.checked ? await autostart.enable() : await autostart.disable(); } catch (err) { toast(String(err)); } };
  try { $("#rcv").textContent = await invoke("rclone_version"); } catch (e) { $("#rcv").textContent = "rclone niedostępny (lokalne foldery i dyski sieciowe działają, chmury wymagają rclone)"; }
})();

// ---------- start ----------
renderFields(); loadRemotes(); refreshJobs().then(refreshStatus);
setInterval(refreshStatus, 2000);
