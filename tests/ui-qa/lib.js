// Wspólny harness QA UI Syncron (nie jest częścią aplikacji).
// Użycie skryptów: node tests/ui-qa/<skrypt>.js  (playwright z NODE_PATH albo z globalnego node_modules)
const path = require("path");
const { execSync } = require("child_process");
function pw() {
  try { return require("playwright"); } catch {}
  return require(path.join(execSync("npm root -g").toString().trim(), "playwright"));
}
const UI = path.resolve(__dirname, "../../app/ui");
const URL = "file://" + path.join(UI, "index.html");

// Skrypt wstrzykiwany przed aplikacją: atrapa window.__TAURI__ z backendem w pamięci.
// Walidacja save_job odwzorowuje serde (typy liczbowe bez znaku, brak ułamków, Option itd.).
const STUB = `(() => {
  const S = window.__stub = { calls: [], fail: {}, delay: {}, jobs: [], status: {}, paused: false, history: [], trash: [], remotes: [], dirs: {},
    settings: { notify: true, language: "pl", unavailable_minutes: 30 }, report: null, upd: null, dialogOpen: null, dialogSave: null };
  const isU = (v, max) => typeof v === "number" && Number.isInteger(v) && v >= 0 && v <= max;
  const u32 = 4294967295;
  function serdeJob(j) {
    const bad = (m) => { throw 'invalid args \`job\` for command \`save_job\`: ' + m; };
    ["id","name","left","right"].forEach((k) => { if (typeof j[k] !== "string") bad("invalid type for field " + k); });
    if (!["TwoWay","LeftToRight","RightToLeft","Custom"].includes(j.mode)) bad("unknown variant " + j.mode + " (mode)");
    if (!["Newer","Larger","LeftWins","RightWins","KeepBoth","Ask"].includes(j.conflict)) bad("unknown variant (conflict)");
    const s = j.schedule || {};
    if (s.type === "Interval") { if (!isU(s.secs, 1.8e19)) bad("invalid value: schedule.secs = " + s.secs + ", expected u64"); }
    else if (s.type === "Realtime") { if (!isU(s.poll_secs, 1.8e19)) bad("invalid value: schedule.poll_secs = " + s.poll_secs + ", expected u64"); }
    else if (s.type === "Daily") { if (!isU(s.hour, u32) || !isU(s.minute, u32)) bad("invalid value: schedule.hour/minute = " + s.hour + ":" + s.minute + ", expected u32"); }
    else if (s.type !== "Manual") bad("unknown variant schedule.type");
    for (const k of ["min_size","max_size"]) if (j[k] != null && !isU(j[k], 1.8e19)) bad("invalid value: " + k + " = " + j[k] + ", expected u64");
    if (j.window != null) { if (!Array.isArray(j.window) || j.window.length !== 2 || !j.window.every((x) => isU(x, u32))) bad("invalid value: window = " + JSON.stringify(j.window) + ", expected [u32; 2]"); }
    if (!isU(j.guard_pct, u32)) bad("invalid value: guard_pct = " + j.guard_pct + ", expected u32");
    if (!isU(j.trash_days, u32)) bad("invalid value: trash_days = " + j.trash_days + ", expected u32");
    if (!isU(j.threads, 1.8e19)) bad("invalid value: threads = " + j.threads + ", expected usize");
    if (!isU(j.max_versions, u32)) bad("invalid value: max_versions = " + j.max_versions + ", expected u32");
    const b = j.bandwidth || {};
    if (!isU(b.kbps, u32) || !isU(b.off_kbps, u32)) bad("invalid value: bandwidth.kbps/off_kbps = " + b.kbps + "/" + b.off_kbps + ", expected u32");
    if (b.hours != null && !(Array.isArray(b.hours) && b.hours.length === 2 && b.hours.every((x) => isU(x, u32)))) bad("invalid value: bandwidth.hours = " + JSON.stringify(b.hours));
    for (const k of ["no_delete_left","no_delete_right","only_add","enabled","use_vss","verify"]) if (typeof j[k] !== "boolean") bad("invalid type: " + k);
    if (!Array.isArray(j.include) || !Array.isArray(j.exclude)) bad("include/exclude");
    if (j.conditions && !Array.isArray(j.conditions.wifi)) bad("conditions.wifi");
    // v08 (kontrakt A): nowe pola są opcjonalne (serde default), ale gdy są — muszą mieć właściwy typ i zakres
    const en = (k, vals) => { if (j[k] !== undefined && !vals.includes(j[k])) bad("unknown variant " + j[k] + " (" + k + ")"); };
    en("compare", ["TimeAndSize","SizeOnly","Content"]); en("symlinks", ["Skip","Follow"]); en("versioning", ["Folders","Replace"]);
    const rng = (k, max) => { if (j[k] !== undefined && !isU(j[k], max)) bad("invalid value: " + k + " = " + j[k] + ", expected 0.." + max); };
    rng("time_tolerance_secs", 86400); rng("time_shift_hours", 23); rng("run_timeout_secs", u32);
    for (const k of ["min_age_days","max_age_days"]) if (j[k] != null && !isU(j[k], u32)) bad("invalid value: " + k + " = " + j[k] + ", expected u32");
    for (const k of ["ignore_hidden","detect_moves","preserve_permissions"]) if (j[k] !== undefined && typeof j[k] !== "boolean") bad("invalid type: " + k);
    for (const k of ["run_before","run_after"]) if (j[k] != null && typeof j[k] !== "string") bad("invalid type: " + k);
    if (j.custom != null) {
      const acts = ["Skip","CopyToRight","CopyToLeft","DeleteLeft","DeleteRight"];
      for (const k of ["left_only","right_only","left_newer","right_newer","different"]) if (!acts.includes(j.custom[k])) bad("missing/unknown custom." + k);
    }
    if (j.mode === "Custom" && j.custom == null) bad("mode Custom requires custom rules");
  }
  const H = {
    list_jobs: () => JSON.parse(JSON.stringify(S.jobs)),
    overview: () => ({ paused: S.paused, status: JSON.parse(JSON.stringify(S.status)) }),
    save_job: ({ job }) => {
      serdeJob(job);
      if (!job.name.trim() || !job.left.trim() || !job.right.trim()) throw "Podaj nazwę oraz obie lokalizacje.";
      if (job.left.trim() === job.right.trim()) throw "Lewa i prawa lokalizacja nie mogą być takie same.";
      const i = S.jobs.findIndex((x) => x.id === job.id);
      if (i >= 0) S.jobs[i] = job; else S.jobs.push(job);
      S.status[job.id] = Object.assign(S.status[job.id] || {}, { state: job.enabled ? "idle" : "disabled" });
    },
    delete_job: ({ id }) => { S.jobs = S.jobs.filter((j) => j.id !== id); delete S.status[id]; },
    run_now: ({ id }) => { const j = S.jobs.find((x) => x.id === id); return !!(j && j.enabled); },
    dry_run: ({ id }) => S.report || { started: 1, dry_run: true, scanned_left: 3, scanned_right: 2, copied: 0, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [], planned: [["a/b.txt", "CopyToRight"], ["c.txt", "DeleteLeft"], ["d.txt", { Conflict: { keep_both: false } }], ["e.txt", { RenameRight: { from: "old.txt" } }]], blocked: null, vss_note: null },
    resolve_conflict: () => null,
    run_selected: ({ id, skip }) => { if (!Array.isArray(skip) || !skip.every((x) => typeof x === "string")) throw "invalid args \`skip\` for command \`run_selected\`"; const j = S.jobs.find((x) => x.id === id); return !!(j && j.enabled); },
    import_freefilesync: ({ path }) => S.ffs !== undefined ? S.ffs : [Object.assign(mkJobStub("ffs-1", "Import A"), { left: "/ffs/a", right: "/ffs/b" }), Object.assign(mkJobStub("ffs-2", "Import B"), { left: "/ffs/c", right: "/ffs/d", mode: "LeftToRight" })],
    check_path: ({ path }) => (path.startsWith("/nope") ? "Ten folder nie jest teraz dostępny. Jeśli to dysk sieciowy, upewnij się, że jest podłączony." : null),
    set_paused: ({ paused }) => { S.paused = paused; },
    history: ({ job, limit }) => S.history.filter((h) => !job || h.job === job).slice(0, limit),
    remote_kinds: () => S.kinds || [{ kind: "webdav", label: "WebDAV / Nextcloud", help: "", oauth: false, fields: [{ key: "url", label: "Adres WebDAV", kind: "text", required: true, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "user", label: "Login", kind: "text", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "pass", label: "Hasło", kind: "password", required: false, secret: true, default: "", options: [], help: "", min: null, max: null, warn: "" }] }, { kind: "sftp", label: "SFTP (SSH)", help: "", oauth: false, fields: [{ key: "host", label: "Serwer", kind: "text", required: true, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "port", label: "Port", kind: "number", required: false, secret: false, default: "22", options: [], help: "", min: null, max: null, warn: "" }, { key: "user", label: "Login", kind: "text", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "pass", label: "Hasło", kind: "password", required: false, secret: true, default: "", options: [], help: "", min: null, max: null, warn: "" }] }, { kind: "ftps", label: "FTPS (FTP z TLS)", help: "", oauth: false, fields: [{ key: "host", label: "Serwer", kind: "text", required: true, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "port", label: "Port", kind: "number", required: false, secret: false, default: "21", options: [], help: "", min: null, max: null, warn: "" }, { key: "user", label: "Login", kind: "text", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "pass", label: "Hasło", kind: "password", required: false, secret: true, default: "", options: [], help: "", min: null, max: null, warn: "" }] }, { kind: "s3", label: "Amazon S3 i zgodne", help: "", oauth: false, fields: [{ key: "bucket", label: "Bucket", kind: "text", required: true, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "access_key_id", label: "Access Key ID", kind: "text", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }, { key: "secret_access_key", label: "Secret Access Key", kind: "password", required: false, secret: true, default: "", options: [], help: "", min: null, max: null, warn: "" }] }],
    browse_remote: ({ name, path }) => (S.tree && S.tree[path || ""]) || [],
    remote_about: () => ({ total: null, used: null, free: null }),
    list_remotes: () => S.remotes,
    add_remote: ({ name, kind, params }) => { S.remotes.push({ name, kind }); },
    delete_remote: ({ name }) => { S.remotes = S.remotes.filter((r) => r.name !== name); },
    test_remote: ({ name }) => "Połączono. Folderów na najwyższym poziomie: 3",
    rclone_version: () => "rclone v1.66.0",
    vss_status: () => S.vss || ({ supported: false, admin: false, elevated_autostart: false }),
    restart_elevated: () => null, set_elevated_autostart: () => null,
    get_settings: () => S.settings,
    set_settings: ({ settings }) => {
      if (!isU(settings.unavailable_minutes, u32)) throw 'invalid args \`settings\` for command \`set_settings\`: invalid value: ' + settings.unavailable_minutes + ', expected u32';
      S.settings = settings;
    },
    trash_list: ({ id }) => S.trash.filter((t) => !t.job || t.job === id),
    trash_restore: ({ path }) => path + " (przywrócony)", trash_delete: () => null,
    list_dirs: ({ location, rel }) => (S.dirs[rel] || ["Dokumenty", "Zdjęcia", "<b>x</b>"]),
    export_config: () => S.jobs.length, import_config: () => 0,
    open_logs_dir: () => null, get_diagnostics: () => "Syncron 0.7.0\\nZadania (1)\\n- „Kopia zapasowa”: ostatni przebieg: skopiowano 1",
  };
  // ---- v08 E: raporty, powiadomienia, webhook (osobny blok) ----
  Object.assign(H, {
    report_summary: ({ filter }) => {
      S.summaryFilters = (S.summaryFilters || []).concat([filter]);
      return S.summary || { total: 6, runs: 3, copied: 4, deleted: 1, moved: 0, conflicts: 1, resolved: 0, errors: 2, warnings: 0, busy: 0, bytes_copied: 3 * 1048576, bytes_known_files: 4, first_ts: 1, last_ts: 2, top_errors: [{ message: "brak dostępu", count: 2 }], by_job: [] };
    },
    export_report: ({ format, path, filter }) => {
      if (!["csv", "json", "html"].includes(format)) throw "Nieznany format raportu „" + format + "” (dostępne: csv, json, html).";
      if (typeof path !== "string" || !path) throw "Podaj ścieżkę pliku docelowego raportu.";
      S.exports = (S.exports || []).concat([{ format, path, filter }]);
      return S.exportCount != null ? S.exportCount : 12;
    },
    test_webhook: ({ url, format, allowPrivate }) => {
      if (!(url.startsWith("http://") || url.startsWith("https://"))) throw "Adres webhooka musi zaczynać się od http:// albo https://.";
      if (!["Json", "Slack", "Discord", "Ntfy"].includes(format)) throw "Nieznany format webhooka (Json, Slack, Discord, Ntfy).";
      S.webhookTests = (S.webhookTests || []).concat([{ url, format, allowPrivate }]);
      return null;
    },
  });
  const _saveJobE = H.save_job;
  H.save_job = (a) => {
    const j = a.job, bad = (m) => { throw "invalid args job for command save_job: " + m; };
    for (const k of ["notify_on", "webhook_on"]) if (j[k] !== undefined && !["Never", "Errors", "Always"].includes(j[k])) bad("unknown variant " + j[k] + " (" + k + ")");
    if (j.webhook_format !== undefined && !["Json", "Slack", "Discord", "Ntfy"].includes(j.webhook_format)) bad("unknown variant " + j.webhook_format + " (webhook_format)");
    if (j.webhook_url != null && typeof j.webhook_url !== "string") bad("invalid type: webhook_url");
    if (j.webhook_allow_private !== undefined && typeof j.webhook_allow_private !== "boolean") bad("invalid type: webhook_allow_private");
    if (j.webhook_url && !(j.webhook_url.startsWith("http://") || j.webhook_url.startsWith("https://"))) throw "Adres webhooka musi zaczynać się od http:// albo https://.";
    return _saveJobE(a);
  };

  function mkJobStub(id, name) { return { id, name, left: "L", right: "R", mode: "TwoWay", no_delete_left: false, no_delete_right: false, only_add: false, conflict: "Newer", include: [], exclude: [], min_size: null, max_size: null, schedule: { type: "Manual" }, window: null, enabled: true, guard_pct: 25, trash_days: 30, threads: 4, use_vss: true, verify: false, max_versions: 0, conditions: { ac_only: false, wifi: [] }, bandwidth: { kbps: 0, off_kbps: 0, hours: null }, triggers: { on_drive: false, on_wake: false, on_network: false }, run_before: "rm -rf /", run_after: null }; }
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  async function invoke(cmd, args) {
    S.calls.push({ cmd, args: JSON.parse(JSON.stringify(args ?? null)), t: performance.now() });
    const dl = S.delayFn ? S.delayFn(cmd, args) : S.delay[cmd];
    if (dl) await sleep(dl);
    if (S.fail[cmd] !== undefined) throw S.fail[cmd];
    if (!H[cmd]) throw "Command " + cmd + " not found";
    return H[cmd](args || {});
  }
  window.__TAURI__ = {
    core: { invoke },
    dialog: { open: async (o) => S.dialogOpen, save: async (o) => S.dialogSave },
    autostart: { isEnabled: async () => false, enable: async () => {}, disable: async () => {} },
    updater: { check: async () => S.upd },
    process: { relaunch: async () => {} },
  };
})();`;

function mkJob(i, extra) {
  return Object.assign({ id: "job-" + i, name: "Zadanie " + i, left: "C:\\Lewa" + i, right: "D:\\Prawa" + i, mode: "TwoWay", no_delete_left: false, no_delete_right: false, only_add: false,
    conflict: "Newer", include: [], exclude: [], min_size: null, max_size: null, schedule: { type: "Realtime", poll_secs: 300 }, window: null, enabled: true, guard_pct: 25, trash_days: 30, threads: 4,
    use_vss: true, verify: false, max_versions: 0, conditions: { ac_only: false, wifi: [] }, bandwidth: { kbps: 0, off_kbps: 0, hours: null }, triggers: { on_drive: false, on_wake: false, on_network: false } }, extra || {});
}
function mkStatus(extra) {
  return Object.assign({ state: "idle", last_run: Math.floor(Date.now() / 1000) - 120, next_run: null, error: null, note: null, progress: null,
    last: { started: 1, dry_run: false, scanned_left: 1, scanned_right: 1, copied: 2, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [], planned: [], blocked: null, vss_note: null } }, extra || {});
}

// Zwraca {browser, page, errors, calls}; setup (funkcja) wykonywany w stronie przed startem aplikacji i widzi window.__stub.
async function launch({ lang, theme, viewport, setup, setupArg } = {}) {
  const { chromium } = pw();
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: viewport || { width: 1100, height: 760 } });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  page.on("console", (m) => { if (m.type() === "error") errors.push("console: " + m.text()); });
  await page.addInitScript(STUB);
  await page.addInitScript(({ lang, theme }) => { try { if (lang) localStorage.setItem("lang", lang); if (theme) localStorage.setItem("theme", theme); } catch {} }, { lang, theme });
  if (setup) await page.addInitScript(setup, setupArg);
  await page.goto(URL);
  await page.waitForTimeout(200);
  const calls = () => page.evaluate(() => window.__stub.calls.map((c) => ({ cmd: c.cmd, args: c.args })));
  return { browser, page, errors, calls };
}

function reporter(name) {
  const res = [];
  return {
    // ok=false oznacza potwierdzony błąd aplikacji
    check(id, ok, info) { res.push({ id, ok: !!ok, info }); console.log((ok ? "PASS " : "FAIL ") + id + (info ? "  -- " + info : "")); },
    done() { const f = res.filter((r) => !r.ok).length; console.log(`\n[${name}] przypadków: ${res.length}, niepowodzeń (potwierdzone błędy): ${f}`); return res; },
  };
}
module.exports = { launch, mkJob, mkStatus, reporter, UI, URL, STUB };
