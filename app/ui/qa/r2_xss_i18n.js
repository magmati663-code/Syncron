// Runda 2: XSS we wszystkich ścieżkach renderowania danych z backendu + wierność danych (round-trip atrybutów) + wpływ tłumaczenia EN na dane użytkownika i wartości formularzy.
// Uruchom: node app/ui/qa/r2_xss_i18n.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("r2_xss_i18n");
const EVIL = '"><img src=x onerror="window.__xss=(window.__xss||0)+1"><b id=pwn>x</b>\'&amp;';
const NASTY_PATH = `a"b'c&d<e>.txt`;

const seed = (a) => {
  const S = window.__stub, E = a.EVIL;
  window.__xss = 0;
  const j = a.jobs[0]; j.name = E; j.left = "C:\\" + E; j.right = "gdrive:" + E; j.include = [E]; j.exclude = [E]; j.conditions.wifi = [E];
  S.jobs = a.jobs;
  const st = a.st; st.error = E; st.note = E; st.progress = { done: 1, total: 2, current: E }; st.state = "running";
  st.last.blocked = E; st.last.busy = [E]; st.last.errors = [E]; st.last.conflicts = [a.NASTY, E]; st.last.vss_note = E;
  st.last.conflict_details = [{ path: a.NASTY, left: { size: 1, mtime: 1 }, right: { size: 2, mtime: 2 } }];
  S.status = { [j.id]: st };
  S.history = [{ ts: 1, job: j.id, kind: E, path: E, detail: E }];
  S.trash = [{ side: "left", stamp: E, path: a.NASTY, size: 5 }];
  S.remotes = [{ name: E, kind: E }];
  S.report = { started: 1, dry_run: true, scanned_left: 1, scanned_right: 1, copied: 0, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [E], planned: [[E + "/" + E, "CopyToRight"], [E, { RenameRight: { from: E } }]], blocked: E, vss_note: null };
  S.dirs[""] = [E];
};

(async () => {
  for (const lang of ["pl", "en"]) {
    const { browser, page, errors } = await launch({ lang, setup: seed, setupArg: { jobs: [mkJob(1)], st: mkStatus(), EVIL, NASTY: NASTY_PATH } });
    await page.waitForTimeout(2300);
    const T = lang.toUpperCase();
    const chk = async (what) => {
      const x = await page.evaluate(() => ({ xss: window.__xss, pwn: document.querySelectorAll("#pwn").length, img: document.querySelectorAll("img").length }));
      R.check(`[${T}] XSS: ${what}`, x.xss === 0 && x.pwn === 0 && x.img === 0, JSON.stringify(x));
    };
    await chk("karta zadania (nazwa, ścieżki, błąd, postęp, blokada, busy, VSS)");
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(2300);
    await chk("Aktywność: oczekujące + historia");
    // wierność ścieżki w atrybutach data-path: klik 'Zachowaj lewą' dla ścieżki ze znakami specjalnymi
    await page.locator(`[data-res=left]`).first().click(); await page.waitForTimeout(200);
    const rc = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "resolve_conflict")[0];
    R.check(`[${T}] resolve_conflict dostaje dokładną ścieżkę ze znakami "'&<>`, rc && rc.args.path === NASTY_PATH, JSON.stringify(rc && rc.args));
    await page.click("nav button[data-tab=trash]"); await page.waitForTimeout(300);
    await chk("Kosz");
    await page.click("[data-tr=restore]"); await page.waitForTimeout(200);
    const tr = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "trash_restore")[0];
    R.check(`[${T}] trash_restore dostaje dokładną ścieżkę i stempel`, tr && tr.args.path === NASTY_PATH && tr.args.stamp === EVIL, JSON.stringify(tr && tr.args));
    await page.click("nav button[data-tab=accounts]"); await page.waitForTimeout(300);
    await chk("Konta chmurowe");
    await page.click("nav button[data-tab=jobs]");
    await page.locator("[data-act=dry]").first().click(); await page.waitForTimeout(300);
    await chk("raport symulacji + drzewo różnic");
    await page.click("#repClose");
    await page.locator("[data-act=edit]").first().click(); await page.waitForTimeout(100);
    for (let i = 0; i < 2; i++) await page.click("#edNext");
    await page.click("#treeLoad"); await page.waitForTimeout(300);
    await chk("drzewo podfolderów w kreatorze");
    await page.click("#edNext"); await page.click("#edNext");
    await chk("podsumowanie kreatora");
    R.check(`[${T}] edycja: wartości pól po wejściu do kreatora nie zostały zmienione przez tłumacza`, (await page.inputValue("[name=name]")) === EVIL, "");
    R.check(`[${T}] brak błędów JS`, errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // --- tłumaczenie EN zniekształca dane użytkownika (nazwy zadań, ścieżki, pliki)
  {
    const names = ["Lustro", "Zadanie", "Kopia zapasowa", "Plik", "i"];
    const jobs = names.map((n, i) => mkJob(i + 1, { name: n }));
    const { browser, page } = await launch({
      lang: "en",
      setup: (a) => { const S = window.__stub; S.jobs = a.jobs; S.status = {}; S.history = [{ ts: 1, job: "job-1", kind: "usunięcie", path: "Plik", detail: "" }, { ts: 2, job: "job-1", kind: "kopiowanie →", path: "ręcznie.txt", detail: "" }, { ts: 3, job: "job-1", kind: "kopiowanie →", path: "usunięto skopiowano.docx", detail: "" }]; },
      setupArg: { jobs },
    });
    await page.waitForTimeout(500);
    const shown = await page.$$eval(".card .name", (e) => e.map((x) => x.textContent));
    R.check("EN: nazwy zadań użytkownika wyświetlane bez zmian", JSON.stringify(shown) === JSON.stringify(names), `oczekiwano ${JSON.stringify(names)}, widać ${JSON.stringify(shown)}`);
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    const paths = await page.$$eval("#hist tbody tr td:nth-child(4)", (e) => e.map((x) => x.textContent));
    R.check("EN: nazwy plików w historii wyświetlane bez zmian", JSON.stringify(paths) === JSON.stringify(["Plik", "ręcznie.txt", "usunięto skopiowano.docx"]), `widać ${JSON.stringify(paths)}`);
    await browser.close();
  }
  // --- tłumaczenie nie rusza wartości formularzy i tekstu w textarea; zmiana języka zachowuje dane
  {
    const { browser, page, errors } = await launch({ lang: "en", setup: (a) => { window.__stub.jobs = []; } });
    await page.click("#newJob");
    await page.fill("[name=name]", "Zadania"); await page.fill("[name=left]", "Lustro"); await page.fill("[name=right]", "Kosz i wersje");
    await page.click("#edNext"); await page.click("#edNext");
    await page.fill("[name=include]", "Zadania\nUsuń\nOK"); await page.waitForTimeout(100);
    R.check("EN: wartość <input>/<textarea> wpisana przez użytkownika nie jest tłumaczona", (await page.inputValue("[name=name]")) === "Zadania" && (await page.inputValue("[name=include]")) === "Zadania\nUsuń\nOK");
    R.check("EN: brak błędów JS w kreatorze", errors.length === 0, errors.join("|"));
    await browser.close();
  }
  // --- pętla MutationObserver: stabilność (liczba mutacji po wyciszeniu)
  {
    const { browser, page } = await launch({ lang: "en", setup: (a) => { window.__stub.jobs = a.jobs; window.__stub.status = {}; }, setupArg: { jobs: [1, 2, 3].map((i) => mkJob(i)) } });
    await page.waitForTimeout(500);
    const n = await page.evaluate(() => new Promise((res) => { let c = 0; const o = new MutationObserver((m) => (c += m.length)); o.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true }); setTimeout(() => { o.disconnect(); res(c); }, 1500); }));
    R.check("EN: w stanie bezczynności liczba mutacji DOM jest mała i ograniczona (brak pętli tłumaczenia/przebudowy; kilka mutacji/tick od #pauseTxt i ikony to znany narzut)", n < 30, "mutacji w 1,5 s: " + n);
    await browser.close();
  }
  // --- logika zależna od tekstu: porównania w angielskim
  {
    const { browser, page } = await launch({ lang: "en", setup: (a) => { const S = window.__stub; S.jobs = a.jobs; S.status = { "job-1": a.st }; S.history = [{ ts: 1, job: "job-1", kind: "błąd", path: "x", detail: "" }, { ts: 2, job: "job-1", kind: "zajęty", path: "y", detail: "" }, { ts: 3, job: "job-1", kind: "kopiowanie →", path: "z", detail: "" }]; }, setupArg: { jobs: [mkJob(1)], st: mkStatus() } });
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    const cls = await page.$$eval("#hist .kind", (e) => e.map((x) => x.className));
    R.check("EN: kolory etykiet historii (błąd=bad, zajęty=warn, kopiowanie=ok) zależą od danych, nie od tłumaczenia", cls[0].includes("bad") && cls[1].includes("warn") && cls[2].includes("ok"), JSON.stringify(cls));
    await browser.close();
  }
  R.done();
})();
