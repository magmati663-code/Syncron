// Syncron 0.8, zespół E: raporty (podsumowanie + eksport), powiadomienia i webhook w kreatorze zadania.
// Ładowany po app.js: korzysta z jego globalnych symboli (invoke, $, $$, esc, toast, jobs, errText, F, dialogApi)
// i nakłada się na funkcje kreatora (openEditor, collect, summarize) oraz loadHistory bez zmian w app.js.
(() => {
  "use strict";

  // ---- style (karty podsumowania) ----
  const css = document.createElement("style");
  css.textContent = ".rep-cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(130px,1fr));gap:10px;margin:0 0 10px}" +
    ".rcard{background:var(--panel);border:1px solid var(--line);border-radius:var(--radius);padding:10px 14px;box-shadow:var(--shadow);min-width:0}" +
    ".rcard>span{display:block;color:var(--muted);font-size:calc(12px*var(--fs))}" +
    ".rcard>b{display:block;font-size:calc(24px*var(--fs));letter-spacing:-.4px;line-height:1.2}" +
    ".rcard>small{display:block;color:var(--muted);font-size:calc(12px*var(--fs))}" +
    ".rcard.ok>b{color:var(--ok)}.rcard.bad>b{color:var(--bad)}.rcard.warn>b{color:var(--warn)}" +
    "#whMsg.ok{color:var(--ok)}#whMsg.bad{color:var(--bad)}";
  document.head.appendChild(css);

  const DAY = 86400;
  const nowSec = () => Math.floor(Date.now() / 1000);
  const fmtBytes = (b) => {
    const U = ["B", "KiB", "MiB", "GiB", "TiB"]; let v = b, i = 0;
    while (v >= 1024 && i < U.length - 1) { v /= 1024; i++; }
    return i === 0 ? `${b} B` : `${v.toFixed(1)} ${U[i]}`;
  };

  // ---------- podsumowanie w „Aktywności” ----------
  let sumSeq = 0;
  function summaryFilter() {
    const days = +$("#repRange").value, job = $("#histJob").value || null;
    return { job, kind: null, since: days > 0 ? nowSec() - days * DAY : null, until: null };
  }
  function renderSummary(s) {
    const card = (label, val, cls, sub) => `<div class="rcard ${cls || ""}"><span>${label}</span><b>${esc(val)}</b>${sub ? `<small>${sub}</small>` : ""}</div>`;
    $("#repCards").innerHTML =
      card("Przebiegi", s.runs, "") +
      card("Skopiowane", s.copied, "ok", s.bytes_known_files > 0 ? esc(fmtBytes(s.bytes_copied)) : "") +
      card("Usunięte", s.deleted, "") +
      card("Konflikty", s.conflicts, s.conflicts > 0 ? "warn" : "") +
      card("Błędy", s.errors, s.errors > 0 ? "bad" : "");
    const top = $("#repTop");
    top.hidden = !(s.top_errors && s.top_errors.length);
    if (!top.hidden) {
      top.innerHTML = "Najczęstsze błędy: " + s.top_errors.slice(0, 3).map((e) => `<span data-nt>${esc(e.message)}</span> (${e.count}×)`).join(" · ");
    }
  }
  async function refreshSummary() {
    if (!$("#tab-activity").classList.contains("active")) return;
    const my = ++sumSeq;
    try {
      const s = await invoke("report_summary", { filter: summaryFilter() });
      if (my !== sumSeq) return;
      renderSummary(s);
    } catch (e) {
      if (my !== sumSeq) return;
      $("#repCards").innerHTML = `<span class="err" data-nt>${esc(errText(e, "Nie udało się wczytać podsumowania."))}</span>`;
      $("#repTop").hidden = true;
    }
  }
  // loadHistory jest wołane po nazwie (zmiana zakładki, odświeżanie), więc nakładka działa; #histJob ma
  // przypisany oryginał, dlatego dodatkowo nasłuchujemy jego zmiany.
  const origLoadHistory = window.loadHistory;
  window.loadHistory = async function () {
    const r = await origLoadHistory.apply(this, arguments);
    refreshSummary();
    return r;
  };
  $("#histJob").addEventListener("change", refreshSummary);
  $("#repRange").addEventListener("change", refreshSummary);

  // ---------- eksport raportu ----------
  const EXT = { csv: ["CSV", "csv"], json: ["JSON", "json"], html: ["HTML", "html"] };
  function openExport() {
    $("#expErr").textContent = "";
    $("#expJob").innerHTML = '<option value="">Wszystkie zadania</option>' + jobs.map((j) => `<option value="${esc(j.id)}" data-nt>${esc(j.name)}</option>`).join("");
    $("#expJob").value = $("#histJob").value || "";
    $("#expRange").value = $("#repRange").value; // zakres z podsumowania jako punkt wyjścia
    $("#expDates").hidden = $("#expRange").value !== "c";
    $("#exportDlg").showModal();
  }
  $("#repExport").onclick = openExport;
  $("#expCancel").onclick = () => $("#exportDlg").close();
  $("#expRange").onchange = () => { $("#expDates").hidden = $("#expRange").value !== "c"; };
  function exportFilter() {
    const r = $("#expRange").value, f = { job: $("#expJob").value || null, kind: $("#expKind").value || null, since: null, until: null };
    if (r === "c") {
      const a = $("#expFrom").value, b = $("#expTo").value;
      if (!a && !b) return { error: "Podaj datę początkową lub końcową albo wybierz gotowy zakres." };
      if (a) f.since = Math.floor(new Date(a + "T00:00:00").getTime() / 1000);
      if (b) f.until = Math.floor(new Date(b + "T23:59:59").getTime() / 1000);
      if ((a && Number.isNaN(f.since)) || (b && Number.isNaN(f.until))) return { error: "Niepoprawna data." };
      if (f.since != null && f.until != null && f.since > f.until) return { error: "Data początkowa nie może być późniejsza niż końcowa." };
    } else if (+r > 0) f.since = nowSec() - +r * DAY;
    return { filter: f };
  }
  $("#expGo").onclick = async () => {
    const fmt = $("#expFormat").value, { filter, error } = exportFilter();
    if (error) { $("#expErr").textContent = error; return; }
    $("#expErr").textContent = "";
    const btn = $("#expGo"); btn.disabled = true;
    try {
      const [name, ext] = EXT[fmt];
      const path = await dialogApi.save({ defaultPath: `syncron-raport.${ext}`, filters: [{ name, extensions: [ext] }] });
      if (!path) return;
      const n = await invoke("export_report", { format: fmt, path, filter });
      $("#exportDlg").close();
      toast(`Zapisano raport: ${n} wpisów`, "ok");
    } catch (e) { $("#expErr").textContent = errText(e, "Nie udało się zapisać raportu."); }
    finally { btn.disabled = false; }
  };

  // ---------- kreator: powiadomienia i webhook ----------
  const NOTIFY_TXT = { Never: "wyłączone", Errors: "tylko błędy i konflikty", Always: "po każdym przebiegu" };
  const WH_TXT = { Never: "wyłączony", Errors: "tylko przy błędach i konfliktach", Always: "po każdym przebiegu" };
  const syncWh = () => {
    const f = F(), on = f.webhook_on.value !== "Never" || f.webhook_url.value.trim() !== "";
    $("#whFields").hidden = !on;
  };
  function fillNotify(j) {
    const f = F();
    f.notify_on.value = j.notify_on || "Errors";
    f.webhook_on.value = j.webhook_on || "Never";
    f.webhook_url.value = j.webhook_url || "";
    f.webhook_format.value = j.webhook_format || "Json";
    f.webhook_allow_private.checked = !!j.webhook_allow_private;
    $("#whMsg").textContent = ""; $("#whMsg").className = "muted";
    syncWh();
  }
  const origOpenEditor = window.openEditor;
  window.openEditor = function (job, tpl) {
    const r = origOpenEditor.apply(this, arguments);
    fillNotify(job || tpl || {});
    return r;
  };
  // http(s)://host[:port][/ścieżka], bez loginu i hasła w adresie
  const URL_OK = /^https?:\/\/[^\s/?#@]+([/?#]\S*)?$/i;
  const origCollect = window.collect;
  window.collect = function () {
    const res = origCollect.apply(this, arguments), f = F();
    const url = f.webhook_url.value.trim(), on = f.webhook_on.value;
    if (url && !URL_OK.test(url)) res.errors.push({ step: 4, field: "webhook_url", msg: "Adres webhooka musi zaczynać się od http:// lub https://, zawierać nazwę hosta i nie może mieć loginu ani hasła." });
    else if (on !== "Never" && !url) res.errors.push({ step: 4, field: "webhook_url", msg: "Podaj adres webhooka albo wybierz „Nigdy” w polu „Wysyłaj webhook”." });
    Object.assign(res.job, {
      notify_on: f.notify_on.value, webhook_on: on, webhook_url: url || null,
      webhook_format: f.webhook_format.value, webhook_allow_private: f.webhook_allow_private.checked,
    });
    return res;
  };
  const origSummarize = window.summarize;
  window.summarize = function (j) {
    const html = origSummarize.apply(this, arguments), li = [];
    if (j.notify_on && j.notify_on !== "Errors") li.push(`Powiadomienia systemowe: ${NOTIFY_TXT[j.notify_on] || esc(j.notify_on)}.`);
    if (j.webhook_url && j.webhook_on && j.webhook_on !== "Never") li.push(`Webhook (${esc(j.webhook_format || "Json")}): ${WH_TXT[j.webhook_on] || esc(j.webhook_on)}.`);
    return li.length ? html.replace(/<\/ul>$/, li.map((x) => `<li>${x}</li>`).join("") + "</ul>") : html;
  };
  ["webhook_on", "webhook_url"].forEach((n) => { F()[n].addEventListener("input", syncWh); F()[n].addEventListener("change", syncWh); });

  $("#whTest").onclick = async () => {
    const f = F(), url = f.webhook_url.value.trim(), msg = $("#whMsg"), btn = $("#whTest");
    const set = (t, c) => { msg.textContent = t; msg.className = c; };
    if (!url) return set("Najpierw podaj adres webhooka.", "bad");
    if (!URL_OK.test(url)) return set("Adres musi zaczynać się od http:// lub https:// i nie może mieć loginu ani hasła.", "bad");
    btn.disabled = true; set("Wysyłam…", "muted");
    try {
      await invoke("test_webhook", { url, format: f.webhook_format.value, allowPrivate: f.webhook_allow_private.checked });
      set("Wysłano wiadomość testową.", "ok");
    } catch (e) { set(errText(e, "Nie udało się wysłać wiadomości testowej."), "bad"); }
    finally { btn.disabled = false; }
  };
})();
