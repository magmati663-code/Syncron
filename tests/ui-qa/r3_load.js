// Runda 3: obciążenie i regresja - 500 zadań, 5000 wpisów kosza/historii, odświeżanie 2 s przy otwartym kreatorze, język, motyw ciemny, wąskie okno, kontrast.
// Uruchom: node tests/ui-qa/r3_load.js     (FAIL = potwierdzony błąd/wada; liczby czasów drukowane informacyjnie)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const fs = require("fs");
const os = require("os");
const path = require("path");
const R = reporter("r3_load");
const SHOTS = fs.mkdtempSync(path.join(os.tmpdir(), "syncron-qa-"));

const bigSeed = (a) => {
  const S = window.__stub, now = Math.floor(Date.now() / 1000);
  S.jobs = []; S.status = {};
  for (let i = 1; i <= a.n; i++) {
    const j = JSON.parse(JSON.stringify(a.tpl)); j.id = "job-" + i; j.name = "Zadanie " + i + " Lustro"; j.left = "C:\\Dane\\Lewa" + i; j.right = i % 3 ? "D:\\Prawa" + i : "gdrive:Folder" + i;
    S.jobs.push(j);
    const st = JSON.parse(JSON.stringify(a.st)); st.state = ["idle", "running", "error", "waiting", "conditions", "blocked"][i % 6]; if (st.state === "error") st.error = "Brak dostępu " + i;
    if (st.state === "running") st.progress = { done: i % 50, total: 50, current: "plik" + i + ".txt" };
    S.status[j.id] = st;
  }
  S.history = []; for (let i = 0; i < a.h; i++) S.history.push({ ts: now - i, job: "job-" + (1 + (i % a.n)), kind: ["kopiowanie →", "usunięcie", "błąd", "zajęty"][i % 4], path: "folder/podfolder/plik" + i + ".txt", detail: "12 KB" });
  S.trash = []; for (let i = 0; i < a.t; i++) S.trash.push({ side: i % 2 ? "left" : "right", stamp: "2025-01-" + String(1 + (i % 28)).padStart(2, "0") + "_030405", path: "d" + (i % 50) + "/plik" + (i % 1000) + ".txt", size: i * 100 });
  S.report = { started: 1, dry_run: true, scanned_left: 20000, scanned_right: 20000, copied: 0, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [], blocked: null, vss_note: null, planned: Array.from({ length: a.p }, (_, i) => ["d" + (i % 200) + "/s" + (i % 17) + "/plik" + i + ".txt", i % 5 ? "CopyToRight" : "DeleteRight"]) };
};
const timeRender = (page) => page.evaluate(async () => { lastKey = ""; const t = performance.now(); renderJobs(); await Promise.resolve(); await new Promise((r) => setTimeout(r, 0)); return performance.now() - t; });

(async () => {
  for (const lang of ["pl", "en"]) {
    const T = lang.toUpperCase();
    const t0 = Date.now();
    const { browser, page, errors } = await launch({ lang, setup: bigSeed, setupArg: { n: 500, h: 5000, t: 5000, p: 20000, tpl: mkJob(0), st: mkStatus() } });
    await page.waitForSelector(".card");
    const cards = await page.locator(".card").count();
    console.log(`[${T}] start + render 500 kart: ${Date.now() - t0} ms`);
    R.check(`[${T}] 500 kart wyrenderowanych`, cards === 500, "kart: " + cards);
    const rr = [];
    for (let i = 0; i < 3; i++) rr.push(Math.round(await timeRender(page)));
    console.log(`[${T}] przebudowa listy 500 kart (renderJobs + obserwator tłumaczenia): ${rr.join(", ")} ms`);
    R.check(`[${T}] przebudowa 500 kart < 300 ms (odświeżanie co 2 s przy zmianie postępu)`, Math.max(...rr) < 300, `czasy: ${rr.join(", ")} ms`);
    // wpływ polling-u: zmiana postępu w jednym zadaniu przebudowuje całą listę
    const longTasks = await page.evaluate(async () => {
      const S = window.__stub; let max = 0, n = 0;
      const po = new PerformanceObserver((l) => l.getEntries().forEach((e) => { n++; max = Math.max(max, e.duration); })); po.observe({ entryTypes: ["longtask"] });
      for (let k = 0; k < 3; k++) { S.status["job-1"].progress.done = 10 + k; await new Promise((r) => setTimeout(r, 2200)); }
      po.disconnect(); return { n, max: Math.round(max) };
    });
    console.log(`[${T}] długie zadania (>50 ms) w 3 cyklach odświeżania z drobną zmianą: ${JSON.stringify(longTasks)}`);
    R.check(`[${T}] drobna zmiana postępu jednego zadania nie powoduje długiego zadania >200 ms`, longTasks.max < 200, JSON.stringify(longTasks));
    // aktywność: 500 wierszy historii (limit wysyłany z UI)
    let t1 = Date.now(); await page.click("nav button[data-tab=activity]"); await page.waitForSelector("#hist tbody tr");
    const hrows = await page.locator("#hist tbody tr").count();
    console.log(`[${T}] Aktywność: ${hrows} wierszy historii w ${Date.now() - t1} ms`);
    R.check(`[${T}] historia: UI informuje, że pokazuje tylko ostatnie ${hrows} z większej liczby wpisów (lub ma paginację)`, hrows < 500 || /ostatni|najnowsz|last \d+|latest|limit/i.test(await page.locator("#tab-activity").innerText()), `wyświetlono ${hrows} wierszy z 5000 w bazie bez żadnej wzmianki; CSV obejmuje tylko te wiersze`);
    // wyszukiwanie
    t1 = Date.now(); await page.fill("#histQ", "plik4999"); await page.waitForTimeout(50); console.log(`[${T}] filtr historii: ${Date.now() - t1} ms`);
    await page.fill("#histQ", "");
    // kosz 5000 pozycji
    t1 = Date.now(); await page.click("nav button[data-tab=trash]"); await page.waitForFunction(() => document.querySelectorAll("#trashTbl tbody tr").length > 100, null, { timeout: 20000 });
    const trows = await page.locator("#trashTbl tbody tr").count();
    console.log(`[${T}] Kosz: ${trows} wierszy w ${Date.now() - t1} ms`);
    t1 = Date.now(); await page.fill("#trashQ", "plik12"); await page.waitForTimeout(50); console.log(`[${T}] filtr kosza (1 klawisz): ${Date.now() - t1} ms`); await page.fill("#trashQ", "");
    // symulacja 20000 pozycji
    await page.click("nav button[data-tab=jobs]");
    t1 = Date.now(); await page.locator('.card [data-act=dry]').first().click(); await page.waitForSelector("#report[open]", { timeout: 30000 });
    const dt = Date.now() - t1; console.log(`[${T}] Symulacja 20000 pozycji: ${dt} ms, węzłów DOM: ${await page.evaluate(() => document.querySelectorAll("#repBody *").length)}`);
    R.check(`[${T}] symulacja 20 000 pozycji otwiera się w < 3 s`, dt < 3000, dt + " ms");
    await page.click("#repClose");
    R.check(`[${T}] brak błędów JS`, errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // --- kreator otwarty podczas odświeżania co 2 s: dane i fokus
  {
    const { browser, page } = await launch({ setup: bigSeed, setupArg: { n: 40, h: 10, t: 0, p: 1, tpl: mkJob(0), st: mkStatus() } });
    await page.click("#newJob");
    await page.fill("[name=name]", "Wpisana nazwa"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    await page.click("#edNext"); await page.click("#edNext");
    await page.fill("[name=include]", "*.docx\nfaktury/*"); await page.fill("[name=min_mb]", "3");
    await page.focus("[name=exclude]"); await page.keyboard.type("*.tmp");
    await page.evaluate(() => { const S = window.__stub; setInterval(() => { for (const k in S.status) if (S.status[k].progress) S.status[k].progress.done++; S.status["job-1"].state = S.status["job-1"].state === "idle" ? "error" : "idle"; }, 700); });
    await page.waitForTimeout(6500);
    const v = await page.evaluate(() => ({ inc: F().include.value, exc: F().exclude.value, min: F().min_mb.value, ae: document.activeElement.name, step: step, open: document.getElementById("editor").open }));
    R.check("kreator: wpisane dane, krok i fokus przetrwały 3 cykle odświeżania z ciągłymi zmianami stanu", v.inc === "*.docx\nfaktury/*" && v.exc === "*.tmp" && v.min === "3" && v.ae === "exclude" && v.step === 2 && v.open, JSON.stringify(v));
    await page.keyboard.type("X"); R.check("kreator: po odświeżeniach nadal można pisać w polu", (await page.inputValue("[name=exclude]")) === "*.tmpX");
    await browser.close();
  }

  // --- język: przełączenie przeładowuje stronę, ustawienie zapisane
  {
    const { browser, page } = await launch({ setup: (a) => { window.__stub.jobs = []; } });
    await page.click("nav button[data-tab=settings]");
    await page.evaluate(() => window.addEventListener("beforeunload", () => sessionStorage.setItem("qa_calls", JSON.stringify(window.__stub.calls))));
    await page.locator("input[name=lang][value=en]").evaluate((e) => e.click()); await page.waitForTimeout(800);
    const lang = await page.evaluate(() => document.documentElement.lang);
    const sc = await page.evaluate(() => JSON.parse(sessionStorage.getItem("qa_calls") || "[]").filter((c) => c.cmd === "set_settings"));
    const st = sc.length ? sc[sc.length - 1].args.settings : null;
    const h1 = await page.locator("nav button.active .l").first().innerText().catch(() => "");
    R.check("zmiana języka na EN: strona w EN i set_settings zapisane", lang === "en" && st && st.language === "en", `lang=${lang}, settings=${JSON.stringify(st)}`);
    await browser.close();
  }
  {
    const { browser, page } = await launch({ setup: (a) => { window.__stub.settings = { notify: true, language: "en", unavailable_minutes: 30 }; window.__stub.jobs = []; } });
    const l = await page.evaluate(() => document.documentElement.lang);
    R.check("język zapisany w settings.json (używany też przez powiadomienia) zgodny z językiem UI po świeżym starcie (czysty localStorage)", l === "en", `settings.language=en, UI lang=${l}`);
    await browser.close();
  }

  // --- motyw ciemny + wąskie okna: przepełnienie poziome, kontrast
  for (const [w, h, theme] of [[860, 560, "dark"], [480, 800, "dark"], [480, 800, "light"]]) {
    const { browser, page } = await launch({ theme, viewport: { width: w, height: h }, setup: bigSeed, setupArg: { n: 6, h: 30, t: 6, p: 5, tpl: mkJob(0, { schedule: { type: "Daily", hour: 2, minute: 0 }, conditions: { ac_only: true, wifi: ["Dom"] }, bandwidth: { kbps: 500, off_kbps: 0, hours: [8, 18] }, exclude: ["*.log"], verify: true }), st: mkStatus() } });
    await page.waitForTimeout(400);
    const ov = async (name) => {
      const o = await page.evaluate(() => { const m = document.querySelector("main"); return { doc: document.documentElement.scrollWidth - innerWidth, main: m.scrollWidth - m.clientWidth }; });
      return { name, ...o };
    };
    const res = [];
    for (const tab of ["jobs", "activity", "trash", "accounts", "settings"]) { await page.click(`nav button[data-tab=${tab}]`); await page.waitForTimeout(250); res.push(await ov(tab)); }
    await page.screenshot({ path: path.join(SHOTS, `${w}-${theme}-settings.png`) });
    R.check(`[${w}px ${theme}] brak poziomego przewijania w głównych zakładkach`, res.every((r) => r.main <= 1 && r.doc <= 1), JSON.stringify(res.filter((r) => r.main > 1 || r.doc > 1)));
    await page.click("nav button[data-tab=jobs]"); await page.click("#newJob"); await page.fill("[name=left]", "A"); await page.fill("[name=right]", "B");
    const dl = await page.evaluate(() => { const d = document.querySelector("#editor").getBoundingClientRect(); return { w: Math.round(d.width), l: Math.round(d.left), r: Math.round(d.right), vw: innerWidth, vh: innerHeight, b: Math.round(d.bottom) }; });
    R.check(`[${w}px ${theme}] kreator mieści się w oknie`, dl.l >= 0 && dl.r <= dl.vw && dl.b <= dl.vh, JSON.stringify(dl));
    const btnVis = await page.evaluate(() => { const b = document.querySelector("#edNext").getBoundingClientRect(); return b.bottom <= innerHeight && b.right <= innerWidth; });
    R.check(`[${w}px ${theme}] przycisk 'Dalej' widoczny w kreatorze`, btnVis);
    if (w === 860 || theme === "dark") {
      // kontrast WCAG tekstu kluczowych elementów
      await page.keyboard.press("Escape");
      await page.click("nav button[data-tab=jobs]");
      const bad = await page.evaluate(() => {
        const lum = (c) => { const [r, g, b] = c.map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
        const parse = (s) => { const m = s.match(/rgba?\(([^)]+)\)/); if (!m) return null; const p = m[1].split(/[ ,\/]+/).map(Number); return { c: p.slice(0, 3), a: p.length > 3 ? p[3] : 1 }; };
        const bg = (el) => { let e = el; while (e) { const p = parse(getComputedStyle(e).backgroundColor); if (p && p.a > 0.9) return p.c; e = e.parentElement; } return [255, 255, 255]; };
        const out = [];
        for (const sel of [".pill", ".tag", ".meta", ".msg", ".tile .k", ".kind", ".hint", ".sub", "nav button", ".badge", ".loc .t b"]) {
          const el = document.querySelector(sel); if (!el) continue;
          const fg = parse(getComputedStyle(el).color); if (!fg) continue;
          const L1 = lum(fg.c), L2 = lum(bg(el)), cr = (Math.max(L1, L2) + 0.05) / (Math.min(L1, L2) + 0.05);
          if (cr < 4.5) out.push(sel + " " + cr.toFixed(2));
        }
        return out;
      });
      console.log(`[${w}px ${theme}] elementy o kontraście < 4.5:1: ${bad.join(", ") || "(brak)"}`);
    }
    await browser.close();
  }
  console.log("Zrzuty ekranu (poza repo):", SHOTS);
  R.done();
})();
