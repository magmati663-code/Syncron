// v0.8, zespół D: kreator zadania (tryb Własny, porównywanie, filtry, zaawansowane), podgląd z odznaczaniem, import FreeFileSync.
// Uruchom: node tests/ui-qa/v08_kreator.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("v08_kreator");

const PL_DIA = /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/;
const setupJobs = (a) => { const S = window.__stub; S.jobs = a.jobs; S.status = {}; a.jobs.forEach((j) => (S.status[j.id] = a.st)); if (a.report) S.report = a.report; if (a.dialogOpen !== undefined) S.dialogOpen = a.dialogOpen; if (a.ffs !== undefined) S.ffs = a.ffs; };
const fresh = (o = {}) => launch(Object.assign({ setup: setupJobs, setupArg: { jobs: o.jobs || [], st: mkStatus(), report: o.report, dialogOpen: o.dialogOpen, ffs: o.ffs } }, o.launch || {}));
const saves = (calls) => calls.filter((c) => c.cmd === "save_job");
const next = (page, n = 1) => (async () => { for (let i = 0; i < n; i++) await page.click("#edNext"); })();
async function basics(page) { await page.fill("[name=name]", "T"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B"); }
const mode = (page, v) => page.click(`label.opt:has(input[name=mode][value=${v}])`);
const visibleText = (page) => page.evaluate(() => {
  const out = [];
  const root = document.querySelector(".step.on");
  out.push(document.querySelector("#stepName").innerText, root.innerText);
  root.querySelectorAll("option").forEach((o) => out.push(o.textContent));
  root.querySelectorAll("[placeholder]").forEach((o) => out.push(o.getAttribute("placeholder")));
  return out.join("\n");
});

(async () => {
  // ---- 1. pełne przejście kreatora z nowymi opcjami
  {
    const { browser, page, errors, calls } = await fresh();
    await page.click("#newJob");
    await basics(page);
    await next(page);
    R.check("krok 2: 4 tryby, w tym Własny", await page.locator("input[name=mode]").count() === 4);
    R.check("siatka trybu własnego ukryta poza trybem Własny", await page.locator("#customBox").isHidden());
    await mode(page, "Custom");
    R.check("tryb Własny pokazuje siatkę 5 reguł", await page.locator("#customBox").isVisible() && await page.locator("#customRows select").count() === 5);
    R.check("tryb Własny ukrywa wybór konfliktu", await page.locator("#conflictRow").isHidden());
    R.check("ostrzeżenie, gdy wszystkie reguły to Pomiń", await page.locator("#customAllSkip").isVisible());
    await page.selectOption("[name=cu_left_only]", "CopyToRight");
    await page.selectOption("[name=cu_right_only]", "DeleteRight");
    await page.selectOption("[name=cu_left_newer]", "CopyToRight");
    await page.selectOption("[name=cu_right_newer]", "CopyToLeft");
    R.check("ostrzeżenie znika po wybraniu reguły", await page.locator("#customAllSkip").isHidden());
    R.check("domyślne porównywanie: czas+rozmiar, tolerancja 2, przesunięcie 0",
      (await page.inputValue("[name=compare]")) === "TimeAndSize" && (await page.inputValue("[name=time_tolerance_secs]")) === "2" && (await page.inputValue("[name=time_shift_hours]")) === "0");
    await page.selectOption("[name=compare]", "Content");
    R.check("opis porównywania zawartości wspomina o wolniejszym działaniu", /wolne/.test(await page.textContent("#cmpHint")));
    await page.fill("[name=time_tolerance_secs]", "5"); await page.fill("[name=time_shift_hours]", "1");
    await next(page);
    // filtry
    await page.fill("[name=min_age_days]", "3"); await page.fill("[name=max_age_days]", "400");
    await page.check("[name=ignore_hidden]");
    R.check("dowiązania: Skip bez ostrzeżenia", await page.locator("#symWarn").isHidden());
    await page.selectOption("[name=symlinks]", "Follow");
    R.check("dowiązania: Follow pokazuje ostrzeżenie", await page.locator("#symWarn").isVisible());
    await page.fill("[name=include]", "*.docx"); await page.fill("[name=min_mb]", "1");
    await next(page, 2);
    // zaawansowane
    R.check("krok 5: ostrzeżenie o poleceniach widoczne", /Twoimi uprawnieniami/.test(await page.textContent(".step.on")));
    await page.uncheck("[name=detect_moves]"); await page.check("[name=preserve_permissions]");
    await page.selectOption("[name=versioning]", "Replace");
    await page.fill("[name=run_before]", "  /bin/echo przed  "); await page.fill("[name=run_timeout_secs]", "60");
    const sum = await page.textContent("#summary");
    await page.click("#edSave"); await page.waitForTimeout(300);
    const sv = saves(await calls());
    const j = sv[0]?.args.job;
    R.check("zapis wysłał jeden save_job", sv.length === 1, JSON.stringify(sv.map((x) => x.args)).slice(0, 200));
    const exp = { mode: "Custom", custom: { left_only: "CopyToRight", right_only: "DeleteRight", left_newer: "CopyToRight", right_newer: "CopyToLeft", different: "Skip" }, compare: "Content", time_tolerance_secs: 5, time_shift_hours: 1,
      symlinks: "Follow", ignore_hidden: true, min_age_days: 3, max_age_days: 400, detect_moves: false, preserve_permissions: true, versioning: "Replace", run_before: "/bin/echo przed", run_after: null, run_timeout_secs: 60 };
    for (const [k, v] of Object.entries(exp)) R.check("JSON zgodny z kontraktem: " + k, JSON.stringify(j?.[k]) === JSON.stringify(v), JSON.stringify(j?.[k]));
    R.check("pozostałe pola (include, min_size) zachowane", JSON.stringify(j?.include) === '["*.docx"]' && j?.min_size === 1048576);
    R.check("brak błędów stubu serde (zadanie na liście)", (await page.locator(".card").count()) === 1);
    R.check("podsumowanie opisuje porównywanie zawartości", /Porównywanie: zawartość plików \(wolniejsze, dokładniejsze\)/.test(sum), sum.slice(0, 300));
    const wants = [/Własny — tylko po lewej: kopiuj w prawo\./, /młodsze niż 3 dni/, /starsze niż 400 dni/, /ukryte/, /Dowiązania/, /przeniesień jest wyłączone/, /Uprawnienia/, /Przed synchronizacją zostanie uruchomione polecenie/, /Tolerancja czasu: 5 s/, /przesunięcie czasu: do 1 godz/, /Limit czasu poleceń: 60 s/, /jedna najnowsza kopia/];
    const miss = wants.filter((re) => !re.test(sum)).map(String);
    R.check("podsumowanie opisuje reguły własne, wiek, ukryte, dowiązania, ruchy, uprawnienia, polecenia, wersjonowanie", miss.length === 0, "brak: " + miss.join(", ") + " | " + sum);
    R.check("podsumowanie nie ujawnia treści polecenia", !/echo/.test(sum));
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 2. walidacje zakresów
  {
    const { browser, page } = await fresh();
    const cases = [
      [1, "time_tolerance_secs", "86401", /Tolerancja czasu/], [1, "time_tolerance_secs", "abc", /Tolerancja czasu/], [1, "time_tolerance_secs", "1.5", /Tolerancja czasu/], [1, "time_tolerance_secs", "-1", /Tolerancja czasu/],
      [1, "time_shift_hours", "24", /Przesunięcie czasu/], [1, "time_shift_hours", "2.5", /Przesunięcie czasu/],
      [2, "min_age_days", "-1", /młodsze/], [2, "max_age_days", "x", /starsze/], [2, "min_age_days", "40000", /młodsze/],
      [4, "run_timeout_secs", "0", /Limit czasu poleceń/], [4, "run_timeout_secs", "86401", /Limit czasu poleceń/], [4, "run_timeout_secs", "1e3", /Limit czasu poleceń/],
    ];
    for (const [stepIdx, name, val, re] of cases) {
      await page.click("#newJob"); await basics(page);
      // wypełnij pole na właściwym kroku
      await next(page, stepIdx === 1 ? 1 : stepIdx === 2 ? 2 : 4);
      await page.evaluate(() => {}); // fokus
      // wartość wpisujemy przez fill (pole type=number nie przyjmuje tekstu, więc ustawiamy przez DOM)
      await page.evaluate(([n, v]) => { const el = document.querySelector(`[name=${n}]`); el.type = "text"; el.value = v; }, [name, val]);
      if (stepIdx === 4) { await page.click("#edSave"); } else { await page.click("#edNext"); }
      const msg = await page.textContent("#edErr");
      const onStep = await page.locator(`.step.on[data-step="${stepIdx}"]`).count();
      R.check(`walidacja ${name}=${val} -> błąd na kroku ${stepIdx + 1}`, re.test(msg) && onStep === 1, `${msg} | krok=${await page.locator(".step.on").getAttribute("data-step")}`);
      await page.keyboard.press("Escape");
    }
    // min wieku > max wieku
    await page.click("#newJob"); await basics(page); await next(page, 2);
    await page.fill("[name=min_age_days]", "50"); await page.fill("[name=max_age_days]", "10"); await page.click("#edNext");
    R.check("min wieku > max wieku -> błąd", /młodsze niż/.test(await page.textContent("#edErr")));
    await page.fill("[name=max_age_days]", "50"); await page.click("#edNext");
    R.check("min wieku = max wieku jest dozwolone", (await page.textContent("#edErr")) === "");
    await page.keyboard.press("Escape");
    // wartości brzegowe poprawne
    await page.click("#newJob"); await basics(page); await next(page);
    await page.fill("[name=time_tolerance_secs]", "86400"); await page.fill("[name=time_shift_hours]", "23"); await page.click("#edNext");
    R.check("wartości brzegowe 86400 i 23 przechodzą", (await page.textContent("#edErr")) === "" && await page.locator('.step.on[data-step="2"]').count() === 1);
    await browser.close();
  }

  // ---- 3. edycja starego zadania (bez nowych pól): domyślne, nieznane pola zostają, nic się nie zmienia poza dodanymi domyślnymi
  {
    const old = mkJob(1, { future_field: { x: 1 }, schedule: { type: "Realtime", poll_secs: 20 }, max_size: 0, guard_pct: 7 });
    const { browser, page, calls } = await fresh({ jobs: [old] });
    await page.waitForTimeout(300);
    await page.click(".card [data-act=edit]");
    R.check("stare zadanie: nowe pola dostają domyślne w formularzu", (await page.inputValue("[name=compare]")) === "TimeAndSize" && (await page.inputValue("[name=time_tolerance_secs]")) === "2"
      && (await page.inputValue("[name=run_timeout_secs]")) === "300" && (await page.isChecked("[name=detect_moves]")) && !(await page.isChecked("[name=ignore_hidden]")));
    await next(page, 4); await page.click("#edSave"); await page.waitForTimeout(300);
    const j = saves(await calls())[0]?.args.job;
    R.check("stare zadanie: nieznane pole przeniesione", JSON.stringify(j?.future_field) === '{"x":1}');
    R.check("stare zadanie: nietknięte wartości zachowane (poll_secs=20, max_size=0, guard=7)", j?.schedule.poll_secs === 20 && j?.max_size === 0 && j?.guard_pct === 7);
    const def = { mode: "TwoWay", compare: "TimeAndSize", time_tolerance_secs: 2, time_shift_hours: 0, symlinks: "Skip", ignore_hidden: false, min_age_days: null, max_age_days: null, detect_moves: true, preserve_permissions: false, versioning: "Folders", custom: null, run_before: null, run_after: null, run_timeout_secs: 300 };
    for (const [k, v] of Object.entries(def)) R.check("stare zadanie dostaje domyślne: " + k, JSON.stringify(j?.[k]) === JSON.stringify(v), JSON.stringify(j?.[k]));
    await browser.close();
  }

  // ---- 3b. edycja zadania z nowymi polami: wartości nieruszone zostają, jedna zmiana nie rusza reszty
  {
    const cust = { left_only: "DeleteLeft", right_only: "Skip", left_newer: "CopyToRight", right_newer: "CopyToLeft", different: "CopyToRight" };
    const job = mkJob(2, { mode: "Custom", custom: cust, compare: "SizeOnly", time_tolerance_secs: 0, time_shift_hours: 3, symlinks: "Follow", ignore_hidden: true, min_age_days: 0, max_age_days: 90,
      detect_moves: false, preserve_permissions: true, versioning: "Replace", run_before: "a b", run_after: "c d", run_timeout_secs: 99 });
    const { browser, page, calls } = await fresh({ jobs: [job] });
    await page.waitForTimeout(300);
    await page.click(".card [data-act=edit]");
    R.check("edycja: pola odtworzone z zadania", (await page.inputValue("[name=cu_left_only]")) === "DeleteLeft" && (await page.inputValue("[name=compare]")) === "SizeOnly" && (await page.inputValue("[name=min_age_days]")) === "0"
      && (await page.inputValue("[name=run_before]")) === "a b" && (await page.inputValue("[name=versioning]")) === "Replace");
    await page.fill("[name=name]", "Zmieniona"); await next(page, 4); await page.click("#edSave"); await page.waitForTimeout(300);
    const j = saves(await calls())[0]?.args.job;
    const { name, ...rest } = j || {};
    const same = Object.entries(job).every(([k, v]) => k === "name" || JSON.stringify(j?.[k]) === JSON.stringify(v));
    R.check("edycja: tylko nazwa zmieniona, reszta identyczna (w tym min_age_days=0, custom)", name === "Zmieniona" && same, JSON.stringify(rest).slice(0, 200));
    // wyczyszczenie polecenia => null
    await page.click(".card [data-act=edit]"); await next(page, 4); await page.fill("[name=run_before]", "   "); await page.click("#edSave"); await page.waitForTimeout(300);
    const j2 = saves(await calls()).pop().args.job;
    R.check("puste polecenie = null, drugie bez zmian", j2.run_before === null && j2.run_after === "c d");
    // zmiana trybu Custom -> TwoWay nie gubi reguł (custom zostaje w zadaniu)
    await page.click(".card [data-act=edit]"); await next(page); await mode(page, "TwoWay"); await next(page, 3); await page.click("#edSave"); await page.waitForTimeout(300);
    const j3 = saves(await calls()).pop().args.job;
    R.check("zmiana trybu na TwoWay: mode zmieniony, reguły własne zachowane", j3.mode === "TwoWay" && JSON.stringify(j3.custom) === JSON.stringify(cust));
    await browser.close();
  }

  // ---- 4. podgląd: tabela akcji, wirtualizacja, filtr, run_selected
  {
    const N = 20000;
    const planned = [];
    for (let i = 0; i < N; i++) planned.push([`dir${i % 50}/plik${i}.txt`, i % 7 === 0 ? "DeleteLeft" : "CopyToRight", i * 1000]);
    planned.push(["<img src=x onerror=window.__xss=1>.txt", "CopyToLeft", 5]);
    planned.push(["stary.txt", { RenameRight: { from: "<b>z</b>.txt" } }, 10]);
    const report = { started: 1, dry_run: true, scanned_left: N, scanned_right: 2, copied: 0, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [], planned, blocked: null, vss_note: null };
    const { browser, page, calls, errors } = await fresh({ jobs: [mkJob(1)], report });
    await page.addInitScript(() => { window.__xss = 0; });
    await page.waitForTimeout(300);
    const t0 = Date.now();
    await page.click(".card [data-act=dry]"); await page.waitForSelector("#pvBody tr");
    const dt = Date.now() - t0;
    const rows = await page.locator("#pvBody tr").count();
    R.check("podgląd 20 000 pozycji: renderowana tylko porcja wierszy", rows > 5 && rows < 120, `wierszy w DOM: ${rows}`);
    R.check("podgląd 20 000 pozycji otwiera się szybko", dt < 3000, dt + " ms");
    R.check("kolumny Akcja/Ścieżka/Rozmiar", (await page.$$eval(".pvt th", (e) => e.map((x) => x.textContent.trim()))).join("|") === "|Akcja|Ścieżka|Rozmiar");
    R.check("licznik: wszystko zaznaczone domyślnie", /Zaznaczono 20002 z 20002/.test(await page.textContent("#pvCount")), await page.textContent("#pvCount"));
    R.check("wszystkie checkboxy domyślnie zaznaczone", (await page.locator("#pvBody input:not(:checked)").count()) === 0);
    R.check("rozmiar sformatowany", /1000 B/.test(await page.textContent("#pvBody")));
    // odznacz dwa pierwsze
    const firstKeys = await page.$$eval("#pvBody .pvp", (e) => e.slice(0, 2).map((x) => x.textContent));
    await page.locator("#pvBody input").nth(0).uncheck(); await page.locator("#pvBody input").nth(1).uncheck();
    R.check("licznik po odznaczeniu 2", /Zaznaczono 20000 z 20002/.test(await page.textContent("#pvCount")));
    // przewiń na koniec; odznacz ostatni (XSS)
    await page.evaluate(() => { const s = document.querySelector("#pvScroll"); s.scrollTop = s.scrollHeight; });
    await page.waitForTimeout(200);
    const lastTxt = await page.$$eval("#pvBody .pvp", (e) => e.map((x) => x.textContent));
    R.check("po przewinięciu widać koniec listy (wirtualizacja)", lastTxt.some((x) => x.includes("stary.txt")), lastTxt.slice(-2).join("|"));
    R.check("XSS w ścieżce wyświetlony jako tekst", lastTxt.some((x) => x.includes("<img src=x")) && (await page.locator("#pvBody img").count()) === 0 && (await page.evaluate(() => window.__xss || 0)) === 0);
    R.check("zmiana nazwy pokazuje 'z:' jako tekst", lastTxt.some((x) => x.includes("(z: <b>z</b>.txt)")) && (await page.locator("#pvBody b").count()) === 0);
    const xssRow = page.locator("#pvBody tr", { hasText: "<img src=x" });
    await xssRow.locator("input").uncheck();
    // filtr
    await page.fill("#pvQ", "plik19999"); await page.waitForTimeout(300);
    R.check("filtr zawęża listę", (await page.locator("#pvBody tr").count()) === 1 && /widocznych: 1/.test(await page.textContent("#pvCount")), await page.textContent("#pvCount"));
    await page.click("#pvNone"); await page.waitForTimeout(100);
    R.check("Odznacz wszystko działa na widocznych (filtr)", /Zaznaczono 19998 z 20002/.test(await page.textContent("#pvCount")), await page.textContent("#pvCount"));
    await page.click("#pvAll");
    R.check("Zaznacz wszystko na widocznych przywraca pozycję", /Zaznaczono 19999 z 20002/.test(await page.textContent("#pvCount")), await page.textContent("#pvCount"));
    await page.fill("#pvQ", "kopiuj"); await page.waitForTimeout(300);
    R.check("filtr obejmuje też nazwę akcji", !/widocznych: 0/.test(await page.textContent("#pvCount")));
    await page.fill("#pvQ", "nie-ma-takiego"); await page.waitForTimeout(300);
    R.check("brak dopasowań -> komunikat", await page.locator("#pvEmpty").isVisible());
    await page.fill("#pvQ", "");  await page.waitForTimeout(300);
    await page.click("#repRun"); await page.waitForTimeout(300);
    const rs = (await calls()).filter((c) => c.cmd === "run_selected");
    R.check("run_selected wywołane raz z id zadania", rs.length === 1 && rs[0].args.id === "job-1");
    const skip = rs[0]?.args.skip || [];
    const expectSkip = [...firstKeys, "<img src=x onerror=window.__xss=1>.txt"].sort();
    R.check("skip = dokładnie odznaczone ścieżki", JSON.stringify(skip.slice().sort()) === JSON.stringify(expectSkip), JSON.stringify(skip));
    R.check("okno podglądu zamknięte po wykonaniu", !(await page.locator("#report").evaluate((d) => d.open)));
    R.check("brak błędów JS w podglądzie", errors.length === 0, errors.join("|"));
    // odznaczone wszystkie => przycisk nieaktywny
    await page.click(".card [data-act=dry]"); await page.waitForSelector("#pvBody tr");
    await page.click("#pvNone");
    R.check("nic nie zaznaczone => 'Wykonaj zaznaczone' nieaktywne", await page.locator("#repRun").isDisabled());
    await page.click("#pvAll");
    R.check("zaznaczone wszystko => skip pusty", await (async () => { await page.click("#repRun"); await page.waitForTimeout(300); const r = (await calls()).filter((c) => c.cmd === "run_selected").pop(); return r.args.skip.length === 0; })());
    await browser.close();
  }

  // ---- 4b. pusty podgląd i podgląd bez przycisku wykonania
  {
    const rep = { started: 1, dry_run: true, scanned_left: 1, scanned_right: 1, copied: 0, deleted: 0, moved: 0, conflicts: [], conflict_details: [], busy: [], errors: [], planned: [], blocked: null, vss_note: null };
    const { browser, page } = await fresh({ jobs: [mkJob(1)], report: rep });
    await page.waitForTimeout(300);
    await page.click(".card [data-act=dry]"); await page.waitForTimeout(300);
    R.check("pusty podgląd: komunikat zgodności, brak przycisku wykonania", /nic do zrobienia/.test(await page.textContent("#repBody")) && await page.locator("#repRun").isHidden());
    await browser.close();
  }

  // ---- 5. import z FreeFileSync
  {
    const { browser, page, calls } = await fresh({ dialogOpen: "/home/u/sync.ffs_gui" });
    await page.click("#ffsImport"); await page.waitForSelector("#ffsList input");
    const ic = (await calls()).filter((c) => c.cmd === "import_freefilesync");
    R.check("import_freefilesync wywołany ze ścieżką pliku", ic.length === 1 && ic[0].args.path === "/home/u/sync.ffs_gui");
    R.check("lista zadań do przejrzenia (2 pozycje, domyślnie zaznaczone)", (await page.locator("#ffsList input:checked").count()) === 2 && /Import A/.test(await page.textContent("#ffsList")));
    await page.locator("#ffsList input").nth(1).uncheck();
    await page.click("#ffsSave"); await page.waitForTimeout(400);
    const sv = saves(await calls());
    R.check("zapisano tylko zaznaczone, jako wyłączone i bez poleceń", sv.length === 1 && sv[0].args.job.name === "Import A" && sv[0].args.job.enabled === false && sv[0].args.job.run_before === null && sv[0].args.job.run_after === null, JSON.stringify(sv.map((x) => [x.args.job.name, x.args.job.enabled, x.args.job.run_before])));
    R.check("okno importu zamknięte, zadanie widoczne na liście", !(await page.locator("#ffsDlg").evaluate((d) => d.open)) && (await page.locator(".card .name", { hasText: "Import A" }).count()) === 1);
    await browser.close();
  }
  {
    const { browser, page, calls } = await fresh({ dialogOpen: "/x/zly.ffs_gui" });
    await page.addInitScript(() => {});
    await page.evaluate(() => { window.__stub.fail.import_freefilesync = "Plik nie jest poprawnym XML-em FreeFileSync (linia 3)."; });
    await page.click("#ffsImport"); await page.waitForTimeout(300);
    R.check("błąd importu pokazany po polsku w toaście", /Plik nie jest poprawnym XML/.test(await page.textContent("#toasts")));
    await page.evaluate(() => { window.__stub.fail.import_freefilesync = "invalid args `path` for command `import_freefilesync`"; });
    await page.click("#ffsImport"); await page.waitForTimeout(300);
    R.check("surowy błąd serde zastąpiony komunikatem po polsku", /Nie udało się zaimportować pliku FreeFileSync/.test(await page.textContent("#toasts")));
    await page.evaluate(() => { delete window.__stub.fail.import_freefilesync; window.__stub.ffs = []; });
    await page.click("#ffsImport"); await page.waitForTimeout(300);
    R.check("pusty wynik importu -> komunikat", /nie znaleziono żadnych par folderów/.test(await page.textContent("#toasts")));
    await page.evaluate(() => { window.__stub.dialogOpen = null; });
    const before = (await calls()).filter((c) => c.cmd === "import_freefilesync").length;
    await page.click("#ffsImport"); await page.waitForTimeout(200);
    R.check("anulowanie dialogu pliku nic nie wywołuje", (await calls()).filter((c) => c.cmd === "import_freefilesync").length === before);
    // XSS w nazwach importowanych zadań + polecenia z importu czyszczone
    await page.evaluate(() => { window.__xss = 0; window.__stub.dialogOpen = "/x/a.ffs_batch"; window.__stub.ffs = [Object.assign({}, window.__stub.jobs[0] || {}, { id: "", name: "<img src=x onerror=window.__xss=1>", left: "<b>L</b>", right: "R2", run_before: "evil" })]; });
    await page.click("#ffsImport"); await page.waitForSelector("#ffsList input");
    R.check("XSS w nazwie importowanej pozycji nie wykonuje się", (await page.locator("#ffsList img, #ffsList b b").count()) === 0 && (await page.evaluate(() => window.__xss)) === 0);
    await browser.close();
  }

  // ---- 6. angielski: stały interfejs bez polskich znaków
  {
    const { browser, page, errors } = await fresh({ launch: { lang: "en" }, report: undefined, jobs: [mkJob(1, { name: "Job", left: "C:\\L", right: "D:\\R" })] });
    await page.waitForTimeout(300);
    await page.click("#newJob"); await basics(page);
    const leaks = [];
    const scan = async (label) => { const t = await visibleText(page); const bad = t.split("\n").filter((l) => PL_DIA.test(l)); if (bad.length) leaks.push(label + ": " + bad.join(" || ")); };
    await scan("step1"); await next(page); await scan("step2");
    await mode(page, "Custom"); await scan("step2-custom");
    await page.selectOption("[name=compare]", "Content"); await scan("step2-content");
    await page.selectOption("[name=cu_left_only]", "CopyToRight");
    await next(page); await page.selectOption("[name=symlinks]", "Follow"); await page.check("[name=ignore_hidden]"); await page.fill("[name=min_age_days]", "2"); await page.fill("[name=max_age_days]", "9"); await scan("step3");
    await next(page, 2); await page.fill("[name=run_before]", "x"); await page.fill("[name=run_after]", "y"); await page.selectOption("[name=versioning]", "Replace"); await page.uncheck("[name=detect_moves]"); await page.check("[name=preserve_permissions]"); await scan("step5");
    await page.waitForTimeout(100);
    const sum = await page.innerText("#summary");
    const sumBad = sum.split("\n").filter((l) => PL_DIA.test(l));
    R.check("EN: kreator (kroki 1-5) bez polskich znaków w stałym UI", leaks.length === 0, leaks.join(" ## ").slice(0, 600));
    R.check("EN: podsumowanie bez polskich znaków", sumBad.length === 0, sumBad.join(" || "));
    R.check("EN: podsumowanie opisuje porównywanie", /Comparison: file content \(slower, more accurate\)/.test(sum), sum.slice(0, 200));
    R.check("EN: reguła własna", /Custom — left only: copy to the right\./.test(sum));
    // błędy walidacji po angielsku
    await page.evaluate(() => { const el = document.querySelector("[name=run_timeout_secs]"); el.type = "text"; el.value = "0"; });
    await page.click("#edSave");
    const em = await page.textContent("#edErr");
    R.check("EN: komunikat walidacji po angielsku", /Command time limit: enter a whole number from 1 to 86400/.test(em), em);
    await page.keyboard.press("Escape");
    // podgląd
    await page.click(".card [data-act=dry]"); await page.waitForSelector("#pvBody tr");
    const rt = await page.innerText("#report");
    R.check("EN: podgląd bez polskich znaków", !PL_DIA.test(rt.replace(/a\/b\.txt|c\.txt|d\.txt|e\.txt|old\.txt/g, "")), rt.split("\n").filter((l) => PL_DIA.test(l)).join("||"));
    R.check("EN: licznik i przyciski podglądu", /Selected 4 of 4/.test(rt) && /Run selected/.test(rt) && /Select all/.test(rt) && /Deselect all/.test(rt), rt.slice(0, 300));
    await page.keyboard.press("Escape");
    // import
    await page.evaluate(() => { window.__stub.dialogOpen = "/x/a.ffs_gui"; });
    await page.click("#ffsImport"); await page.waitForSelector("#ffsList input");
    const ft = await page.innerText("#ffsDlg");
    R.check("EN: okno importu bez polskich znaków", !PL_DIA.test(ft), ft);
    R.check("EN: brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  R.done();
})().catch((e) => { console.error("BŁĄD SKRYPTU", e); process.exit(2); });
