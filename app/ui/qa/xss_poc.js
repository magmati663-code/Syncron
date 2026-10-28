// QA tester 9 — PoC XSS dla UI (NIE ładowany przez aplikację).
// Uruchomienie z korzenia repo: NODE_PATH=$(npm root -g) node app/ui/qa/xss_poc.js
// Stub window.__TAURI__ zwraca złośliwe dane (nazwy plików, zadań, kont, błędy) zawierające HTML;
// sprawdzamy, czy jakikolwiek kod z payloadu się wykonał lub czy powstały obce elementy.
// Kod wyjścia 0 = brak wykonania payloadu (UI bezpieczne), 1 = XSS.
let pw;
try { pw = require("playwright"); } catch { pw = require(require("child_process").execSync("npm root -g").toString().trim() + "/playwright"); }
const { chromium } = pw;
const path = require("path");

const init = `
(() => {
  const XSS = (t) => '"><img src=x onerror="window.__pwned=(window.__pwned||[]).concat(\\'' + t + '\\')"><svg onload="window.__pwned=(window.__pwned||[]).concat(\\'' + t + '-svg\\')">\\'';
  const job = { id: XSS("jid"), name: XSS("jname"), left: XSS("left"), right: "gdrive:" + XSS("right"), mode: "TwoWay", conflict: "Newer",
    schedule: { type: "Manual" }, enabled: true, trash_days: 30, max_versions: 0, include: [], exclude: [], threads: 4, use_vss: true,
    bandwidth: { kbps: 0, off_kbps: 0, hours: null }, conditions: { ac_only: false, wifi: [XSS("wifi")] }, triggers: {}, no_delete_left: false, no_delete_right: false, only_add: false, guard_pct: 25, verify: false };
  const rep = { copied: 1, deleted: 0, moved: 0, scanned_left: 1, scanned_right: 1, conflicts: [XSS("conflict")], busy: [XSS("busy")], errors: [XSS("err")],
    planned: [[XSS("plan"), "CopyToRight"], [XSS("plan2"), { RenameRight: { from: XSS("from") } }]], conflict_details: [], vss_note: XSS("vss"), blocked: XSS("blocked") };
  const cmds = {
    list_jobs: () => [job],
    overview: () => ({ paused: false, status: { [job.id]: { state: "error", error: XSS("staterr"), last: rep, progress: { done: 1, total: 2, current: XSS("cur") }, note: XSS("note") } } }),
    history: () => [{ ts: 1, job: job.id, kind: XSS("kind"), path: XSS("hpath"), detail: XSS("detail") }],
    list_remotes: () => [{ name: XSS("rname"), kind: XSS("rkind") }],
    trash_list: () => [{ side: "left", stamp: XSS("stamp"), path: XSS("tpath"), size: 1 }],
    list_dirs: () => [XSS("dir")],
    dry_run: () => rep,
    vss_status: () => ({ supported: true, admin: false, elevated_autostart: false }),
    get_settings: () => ({ notify: true, language: "pl", unavailable_minutes: 30 }),
    rclone_version: () => XSS("ver"),
    get_diagnostics: () => XSS("diag"),
    check_path: () => XSS("hint"),
  };
  window.__TAURI__ = {
    core: { invoke: async (c) => { if (c in cmds) return cmds[c](); return null; } },
    dialog: { open: async () => null, save: async () => null },
    autostart: { isEnabled: async () => false, enable: async () => {}, disable: async () => {} },
    updater: { check: async () => null }, process: { relaunch: async () => {} },
  };
})();`;

(async () => {
  const b = await chromium.launch();
  const pg = await b.newPage();
  const errs = [];
  pg.on("pageerror", (e) => errs.push(e.message));
  pg.on("dialog", (d) => { errs.push("DIALOG " + d.message()); d.dismiss(); });
  await pg.addInitScript(init);
  await pg.goto("file://" + path.resolve(__dirname, "../index.html"));
  await pg.waitForTimeout(800);
  for (const tab of ["jobs", "activity", "trash", "accounts", "settings"]) {
    await pg.click(`[data-tab=${tab}]`).catch(() => {});
    await pg.waitForTimeout(500);
  }
  await pg.click('[data-act="dry"]').catch(() => {}); // symulacja (raport z payloadami)
  await pg.waitForTimeout(500);
  await pg.click('[data-act="edit"]').catch(() => {}); // edycja: pola formularza z danych zadania
  await pg.waitForTimeout(500);
  const res = await pg.evaluate(() => ({
    pwned: window.__pwned || [],
    injected: document.querySelectorAll("img[src='x'], svg[onload]").length,
    // kontrola harnessu: payload ma być widoczny jako TEKST (escapowany) w wielu miejscach
    renderedAsText: (document.body.innerText.match(/onerror=/g) || []).length + (document.body.textContent.match(/onerror=/g) || []).length,
  }));
  if (!res.renderedAsText) { console.log("HARNESS: payload nie został wyrenderowany"); process.exit(3); }
  console.log(JSON.stringify(res), errs.length ? "BŁĘDY: " + errs.join("; ") : "");
  await b.close();
  process.exit(res.pwned.length || res.injected ? 1 : 0);
})();
