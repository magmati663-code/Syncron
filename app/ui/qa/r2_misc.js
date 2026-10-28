// Runda 1/2: funkcje poboczne - szablony, drzewo podfolderów, kosz w kreatorze, CSV, wstrzymanie, plakietka uwagi, VSS, import/eksport, podpowiedź ścieżki.
// Uruchom: node app/ui/qa/r2_misc.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const fs = require("fs");
const R = reporter("r2_misc");
const seed = (a) => {
  a = a || {}; const S = window.__stub; S.jobs = a.jobs || []; S.status = a.status || {}; S.history = a.history || []; if (a.vss) S.vss = a.vss;
};
const calls = (page, cmd) => page.evaluate((c) => window.__stub.calls.filter((x) => x.cmd === c), cmd);

(async () => {
  // szablony ze stanu pustego
  {
    const { browser, page, errors } = await launch({ setup: seed });
    R.check("pusty stan: 3 szablony i przycisk 'Nowe zadanie'", await page.locator(".tpl").count() === 3 && await page.locator("#emptyNew").count() === 1);
    await page.locator('[data-tpl=bak]').click();
    R.check("szablon 'Kopia zapasowa': tryb L→P, nie usuwaj po prawej, konflikt LeftWins", (await page.locator("input[name=mode]:checked").getAttribute("value")) === "LeftToRight" && await page.locator("[name=no_delete_right]").isChecked() && (await page.inputValue("[name=conflict]")) === "LeftWins" && (await page.inputValue("[name=name]")) === "Kopia zapasowa");
    await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    for (let i = 0; i < 4; i++) await page.click("#edNext");
    await page.click("#edSave"); await page.waitForTimeout(300);
    const j = (await calls(page, "save_job"))[0]?.args.job;
    R.check("zapis z szablonu: poprawne pola (schedule Realtime 300 s, trash 30, guard 25, threads 4)", j && j.schedule.type === "Realtime" && j.schedule.poll_secs === 300 && j.trash_days === 30 && j.guard_pct === 25 && j.threads === 4 && j.no_delete_right === true, JSON.stringify(j && { s: j.schedule, t: j.trash_days }));
    R.check("po zapisie: karta zadania widoczna, toast 'Zadanie zapisane'", await page.locator(".card").count() === 1 && (await page.textContent("#toasts")).includes("Zadanie zapisane"));
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }
  // kosz w kreatorze
  {
    const { browser, page } = await launch({ setup: seed });
    await page.click("#newJob"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    for (let i = 0; i < 4; i++) await page.click("#edNext");
    await page.uncheck("[name=use_trash]"); await page.click("#edSave"); await page.waitForTimeout(300);
    let j = (await calls(page, "save_job"))[0]?.args.job;
    R.check("kosz wyłączony -> trash_days=0", j && j.trash_days === 0);
    const sum = await page.evaluate(() => 0);
    await browser.close();
  }
  // drzewo podfolderów
  {
    const { browser, page } = await launch({ setup: seed });
    await page.click("#newJob"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    await page.click("#edNext"); await page.click("#edNext");
    await page.click("#treeLoad"); await page.waitForSelector(".treerow");
    await page.locator(".treerow input[type=checkbox]").first().check();
    R.check("zaznaczenie podfolderu dopisuje maskę 'Dokumenty/*'", (await page.inputValue("[name=include]")) === "Dokumenty/*");
    await page.locator(".treerow input[type=checkbox]").first().uncheck();
    R.check("odznaczenie usuwa maskę", (await page.inputValue("[name=include]")) === "");
    await page.locator("[data-exp]").first().click(); await page.waitForTimeout(150);
    R.check("rozwinięcie podfolderu ładuje poziom niżej (list_dirs z rel)", (await calls(page, "list_dirs")).some((c) => c.args.rel === "Dokumenty"));
    await page.fill("[name=include]", "Dokumenty/*\ninne/*"); await page.click("#treeLoad"); await page.waitForSelector(".treerow");
    R.check("ponowne wczytanie drzewa odzwierciedla istniejące maski (Dokumenty zaznaczony)", await page.locator(".treerow input[type=checkbox]").first().isChecked());
    await browser.close();
  }
  // CSV
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: {}, history: [
      { ts: 1700000000, job: "job-1", kind: "kopiowanie →", path: '=HYPERLINK("http://zlosliwy.example","kliknij")', detail: 'a"b;c\nd' },
      { ts: 1700000001, job: "job-1", kind: "usunięcie", path: "zwykły.txt", detail: "" }] } });
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    await page.fill("#histQ", "zwykły"); await page.waitForTimeout(100);
    const [dl] = await Promise.all([page.waitForEvent("download"), page.click("#exportCsv")]);
    const p = await dl.path(); const csv = fs.readFileSync(p, "utf8");
    R.check("CSV: BOM UTF-8, nagłówek i separator ';'", csv.startsWith("\uFEFFczas;zadanie;operacja;plik;szczegóły"));
    R.check("CSV: cudzysłowy podwojone, nowa linia w polu zachowana w cudzysłowie", csv.includes('"a""b;c\nd"'));
    R.check("CSV: nazwa pliku zaczynająca się od '=' jest zabezpieczona przed formułami arkusza (prefiks ' lub spacja)", !/(^|;)"=HYPERLINK/m.test(csv), "pole zaczyna się od =HYPERLINK(...) -> Excel/LibreOffice wykona formułę (CSV injection)");
    R.check("CSV: eksport respektuje aktywny filtr wyszukiwania (widoczne 1 wiersz)", csv.trim().split("\r\n").length <= 2, `wierszy w CSV: ${csv.trim().split("\r\n").length - 1} przy 1 widocznym po filtrze`);
    await browser.close();
  }
  // pauza i plakietka uwagi
  {
    const st = mkStatus(); st.last.conflicts = ["a", "b"]; st.last.busy = ["c"]; st.last.errors = ["d"];
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": st } } });
    await page.waitForTimeout(300);
    R.check("plakietka 'Aktywność' = konflikty + zajęte + błędy (4)", (await page.textContent("#attBadge")) === "4");
    await page.click("#pauseAll"); await page.waitForTimeout(300);
    R.check("pauza: przycisk zmienia się na 'Wznów wszystko', wywołanie set_paused(true)", (await page.textContent("#pauseTxt")).includes("Wznów") && (await calls(page, "set_paused"))[0].args.paused === true);
    await browser.close();
  }
  // VSS: znacznik na karcie po wczytaniu statusu VSS (render wyprzedza vss_status)
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": mkStatus() }, vss: { supported: true, admin: true, elevated_autostart: false } } });
    await page.waitForTimeout(2600);
    const tags = await page.$$eval(".card .tag", (e) => e.map((x) => x.textContent));
    R.check("VSS obsługiwany: znacznik 'VSS' widoczny na karcie bez dodatkowej zmiany stanu", tags.includes("VSS"), `znaczniki: ${tags.join(", ")} (renderJobs nie uwzględnia vssInfo w kluczu, a loadVss kończy się po pierwszym renderze)`);
    await browser.close();
  }
  // import / eksport: anulowanie okna dialogowego
  {
    const { browser, page } = await launch({ setup: seed });
    await page.click("nav button[data-tab=settings]");
    await page.click("#cfgExport"); await page.click("#cfgImport"); await page.waitForTimeout(150);
    R.check("anulowanie dialogów eksportu/importu nie wywołuje backendu ani toastów", (await calls(page, "export_config")).length === 0 && (await calls(page, "import_config")).length === 0 && (await page.locator(".toast").count()) === 0);
    await page.evaluate(() => { window.__stub.dialogSave = "C:\\k.json"; window.__stub.dialogOpen = "C:\\k.json"; });
    await page.click("#cfgExport"); await page.click("#cfgImport"); await page.waitForTimeout(250);
    R.check("eksport/import z wybranym plikiem: wywołania z parametrem path", (await calls(page, "export_config"))[0]?.args.path === "C:\\k.json" && (await calls(page, "import_config"))[0]?.args.path === "C:\\k.json");
    await browser.close();
  }
  // podpowiedź ścieżki i lista kont w kreatorze
  {
    const { browser, page } = await launch({ setup: seed });
    await page.click("#newJob"); await page.fill("[name=left]", "/nope/dir"); await page.dispatchEvent("[name=left]", "change"); await page.waitForTimeout(150);
    R.check("podpowiedź o niedostępnym folderze przy zmianie ścieżki", /nie jest teraz dostępny/.test(await page.textContent("[data-hint=left]")));
    await browser.close();
  }
  // wymuszenie po blokadzie
  {
    const st = mkStatus({ state: "blocked" }); st.last.blocked = "plan usuwa ponad 25% plików";
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": st } } });
    await page.waitForTimeout(300);
    await page.click("[data-act=force]"); await page.waitForTimeout(100);
    await page.click("#cfYes"); await page.waitForTimeout(200);
    R.check("'Wymuś' po potwierdzeniu wywołuje run_now(force=true)", (await calls(page, "run_now")).some((c) => c.args.force === true));
    await browser.close();
  }
  // przełączanie zadania wyłączone/włączone
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": mkStatus() } } });
    await page.click("[data-act=toggle]"); await page.waitForTimeout(400);
    R.check("'Wyłącz' zapisuje enabled=false i karta pokazuje 'Wyłączone'", (await calls(page, "save_job"))[0].args.job.enabled === false && /Wyłączone/.test(await page.textContent(".card .pill")));
    await browser.close();
  }
  R.done();
})();
