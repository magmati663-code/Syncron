// Runda 2: kreator zadania - wpisy brzegowe/złośliwe, zgodność wysyłanego JSON z modelem Job (serde), walidacje, podwójne kliknięcie.
// Uruchom: node tests/ui-qa/r2_wizard.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("r2_wizard");

async function fresh(extra = {}) {
  const ctx = await launch(Object.assign({ setup: (a) => { window.__stub.jobs = a.jobs; window.__stub.status = {}; a.jobs.forEach((j) => (window.__stub.status[j.id] = a.st)); }, setupArg: { jobs: extra.jobs || [], st: mkStatus() } }, {}));
  return ctx;
}
const saves = (calls) => calls.filter((c) => c.cmd === "save_job");
async function fillBasics(page, o = {}) {
  await page.fill('[name=name]', o.name ?? "T");
  await page.fill('[name=left]', o.left ?? "C:\\A");
  await page.fill('[name=right]', o.right ?? "D:\\B");
}
// Zapisz jest widoczny dopiero w kroku 5; gdy walidacja zatrzymała kreator wcześniej, błąd jest w #edErr
async function clickSave(page) { if (await page.locator("#edSave").isVisible()) await page.click("#edSave"); }
async function toStep(page, n) { for (let i = 0; i < n; i++) await page.click("#edNext"); }

(async () => {
  // ---- 1. walidacje kroku 1
  {
    const { browser, page, errors } = await fresh();
    await page.click("#newJob");
    await page.fill('[name=left]', "   "); await page.fill('[name=right]', "x");
    await page.click("#edNext");
    R.check("puste/spacje w lewej lokalizacji -> komunikat, bez przejścia", (await page.textContent("#edErr")).length > 0 && await page.locator('.step.on[data-step="0"]').count() === 1);
    await page.fill('[name=left]', " X "); await page.fill('[name=right]', "X");
    await page.click("#edNext");
    R.check("te same lokalizacje (po trim) -> komunikat", /takie same/.test(await page.textContent("#edErr")));
    await page.keyboard.press("Escape"); await page.click("#newJob");
    await page.fill('[name=left]', "C:\\Dane"); await page.fill('[name=right]', "C:\\Dane\\");
    await page.click("#edNext");
    const sameWithSlash = await page.locator('.step.on[data-step="1"]').count() === 1;
    R.check("lokalizacje różniące się tylko końcowym separatorem (C:\\Dane vs C:\\Dane\\) są odrzucane", !sameWithSlash, "UI przepuszcza ten sam folder jako dwie strony (do weryfikacji, czy rdzeń to wykrywa)");
    await page.keyboard.press("Escape"); await page.click("#newJob");
    await page.fill('[name=left]', "C:\\Dane"); await page.fill('[name=right]', "c:\\dane");
    await page.click("#edNext");
    R.check("lokalizacje różniące się wielkością liter (Windows) są odrzucane", await page.locator('.step.on[data-step="1"]').count() === 0, "przepuszczone");
    await browser.close();
  }

  // ---- 2. XSS/HTML w nazwie, ścieżkach, maskach, Wi-Fi - brak wykonania, wartości zachowane
  {
    const { browser, page, errors } = await fresh();
    await page.addInitScript(() => { window.__xss = 0; });
    await page.click("#newJob");
    const evil = '<img src=x onerror="window.__xss=1">"\'&<script>window.__xss=2</script>';
    await page.fill('[name=name]', evil); await page.fill('[name=left]', "C:\\" + evil); await page.fill('[name=right]', "D:\\" + evil);
    await page.click("#edNext"); await page.click("#edNext");
    await page.fill('[name=include]', evil); await page.fill('[name=exclude]', evil);
    await page.click("#edNext"); await page.fill('[name=wifi]', evil);
    await page.click("#edNext");
    const sum = await page.innerHTML("#summary");
    R.check("podsumowanie (krok 5) nie wstawia HTML ze ścieżek/Wi-Fi", !/<img/i.test(sum) && !/<script/i.test(sum));
    await clickSave(page); await page.waitForTimeout(400);
    R.check("XSS nie wykonany po zapisie i renderze karty", (await page.evaluate(() => window.__xss || 0)) === 0 && await page.locator(".card img").count() === 0);
    const c = saves(await page.evaluate(() => window.__stub.calls));
    R.check("nazwa z HTML zapisana literalnie", c.length === 1 && c[0].args.job.name === evil);
    await page.click('.card [data-act=edit]');
    R.check("edycja: pola odtworzone literalnie (round-trip HTML)", (await page.inputValue('[name=name]')) === evil && (await page.inputValue('[name=include]')) === evil);
    await browser.close();
  }

  // ---- 3. Harmonogram: Interval / Realtime / Daily z wartościami brzegowymi
  const schedCases = [
    ["Interval", "1.01", "ułamkowe minuty (1.01 -> 60.6 s, nie-całkowite sekundy)"],
    ["Interval", "7.77", "ułamkowe minuty (7.77 -> 466.2 s)"],
    ["Interval", "0.01", "bardzo mały ułamek (0.6 s)"],
    ["Interval", "-5", "ujemne minuty"],
    ["Interval", "abc", "tekst nieliczbowy"],
    ["Interval", "1e400", "Infinity"],
    ["Realtime", "1.01", "ułamkowe minuty (Realtime, 60.6 s)"],
    ["Realtime", "0", "zero minut"],
    ["Daily", "25:99", "godzina poza zakresem 25:99"],
    ["Daily", "7:5", "format 7:5"],
    ["Daily", "07:05", "poprawna godzina"],
  ];
  for (const [t, v, desc] of schedCases) {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 3);
    await page.check(`input[name=sched][value=${t}]`, { force: true }).catch(async () => { await page.evaluate((t) => { document.querySelector(`input[name=sched][value=${t}]`).click(); }, t); });
    await page.fill('[name=sched_val]', v);
    await page.click("#edNext");
    const err4 = await page.textContent("#edErr");
    if (await page.locator('.step.on[data-step="4"]').count() === 0) { R.check(`sched ${t} "${v}" (${desc}): zatrzymane walidacją`, true, err4); await browser.close(); continue; }
    await clickSave(page); await page.waitForTimeout(300);
    const err = await page.textContent("#edErr");
    const c = saves(await page.evaluate(() => window.__stub.calls));
    const s = c[0]?.args.job.schedule;
    const ok = c.length === 1 && !!(await page.evaluate(() => window.__stub.jobs.length));
    R.check(`sched ${t} "${v}" (${desc}) -> zapis przechodzi i deserializuje się w Rust`, ok, `wysłano ${JSON.stringify(s)}; błąd w UI: "${err}"`);
    await browser.close();
  }

  // ---- 4. natywna walidacja HTML5 pól w ukrytych krokach: cichy brak zapisu
  const hidden = [
    ["win_from", "30", 3, "godzina okna 30 (max 23) - krok 4"],
    ["win_from", "5.5", 3, "godzina okna 5.5 - krok 4"],
    ["min_mb", "-5", 2, "ujemny min. rozmiar - krok 3"],
  ];
  for (const [name, val, step, desc] of hidden) {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, step);
    await page.fill(`[name=${name}]`, val);
    await toStep(page, 4 - step);
    await clickSave(page); await page.waitForTimeout(400);
    const c = saves(await page.evaluate(() => window.__stub.calls));
    const err = await page.textContent("#edErr");
    const open = await page.locator("#editor[open]").count();
    R.check(`${desc}: zapis zablokowany Z KOMUNIKATEM (a nie po cichu)`, c.length > 0 || err.trim().length > 0, `save_job wywołań: ${c.length}, #edErr="${err}", edytor otwarty: ${open}`);
    await browser.close();
  }

  // ---- 5. pola rozmiaru: 0 i duże wartości
  {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 2); await page.fill('[name=max_mb]', "0"); await toStep(page, 2);
    await clickSave(page); await page.waitForTimeout(300);
    const j = saves(await page.evaluate(() => window.__stub.calls))[0]?.args.job;
    R.check('"Maks. rozmiar = 0" nie tworzy zadania wykluczającego wszystkie pliki (max_size=0 => Rust wyklucza size>=1)', j && (j.max_size === null), `wysłano max_size=${j && j.max_size}`);
    await browser.close();
  }
  {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 2); await page.fill('[name=max_mb]', "1e30"); await toStep(page, 2);
    await clickSave(page); await page.waitForTimeout(300);
    const err = await page.textContent("#edErr");
    R.check('"Maks. rozmiar = 1e30 MB" daje zrozumiały komunikat / jest ograniczone (zamiast surowego błędu serde)', !/invalid|expected u64/i.test(err), `#edErr="${err}"`);
    await browser.close();
  }

  // ---- 6. częściowe okno czasowe jest po cichu ignorowane
  {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 3); await page.fill('[name=win_from]', "22"); await toStep(page, 1);
    // walidacja blokuje przejście dalej z komunikatem (Zapisz jest wtedy niewidoczny) albo zapis z komunikatem
    if (await page.locator("#edSave").isVisible()) { await clickSave(page); await page.waitForTimeout(300); }
    const j = saves(await page.evaluate(() => window.__stub.calls))[0]?.args.job;
    const msg = await page.textContent("#edErr");
    R.check("okno czasowe z samym 'od' (bez 'do') -> komunikat zamiast cichego odrzucenia", !(j && j.window === null) && (j || msg.length > 0), `wysłano window=${JSON.stringify(j && j.window)} komunikat="${msg}"`);
    await browser.close();
  }

  // ---- 7. podwójne kliknięcie Zapisz: duplikaty zadań (id generowane przy każdym readJob)
  {
    const { browser, page } = await launch({ setup: () => { window.__stub.delay.save_job = 300; } });
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 4);
    await page.dblclick("#edSave"); await page.waitForTimeout(900);
    const n = await page.evaluate(() => window.__stub.jobs.length);
    const calls = saves(await page.evaluate(() => window.__stub.calls)).length;
    R.check("podwójne kliknięcie 'Zapisz' tworzy jedno zadanie", n === 1, `save_job wywołań: ${calls}, zadań po zapisie: ${n}`);
    await browser.close();
  }
  {
    const { browser, page } = await launch({ setup: () => { window.__stub.delay.save_job = 300; } });
    await page.click("#newJob"); await fillBasics(page);
    await toStep(page, 4);
    await page.focus("#edSave"); await page.keyboard.press("Enter"); await page.keyboard.press("Enter"); await page.waitForTimeout(900);
    const n = await page.evaluate(() => window.__stub.jobs.length);
    R.check("dwa Enter na 'Zapisz' tworzą jedno zadanie", n === 1, `zadań: ${n}`);
    await browser.close();
  }

  // ---- 8. edycja istniejących zadań: stary schemat / wartości niestandardowe - round-trip bez zmian
  const rt = [
    ["Realtime poll_secs=20", { schedule: { type: "Realtime", poll_secs: 20 } }, (j) => j.schedule.poll_secs === 20],
    ["Interval secs=90", { schedule: { type: "Interval", secs: 90 } }, (j) => j.schedule.secs === 90],
    ["max_size=0", { max_size: 0 }, (j) => j.max_size === 0],
    ["min_size=1 bajt", { min_size: 1 }, (j) => j.min_size === 1],
    ["window [22,24]", { window: [22, 24] }, (j) => JSON.stringify(j.window) === "[22,24]"],
    ["trash_days=0 max_versions=5", { trash_days: 0, max_versions: 5 }, (j) => j.trash_days === 0 && j.max_versions === 5],
    ["Daily 02:05", { schedule: { type: "Daily", hour: 2, minute: 5 } }, (j) => j.schedule.hour === 2 && j.schedule.minute === 5],
  ];
  for (const [name, extra, test] of rt) {
    const { browser, page, errors } = await fresh({ jobs: [mkJob(1, extra)] });
    await page.click('.card [data-act=edit]'); await toStep(page, 4);
    await clickSave(page); await page.waitForTimeout(300);
    const c = saves(await page.evaluate(() => window.__stub.calls));
    const j = c[0]?.args.job;
    const err = await page.textContent("#edErr");
    R.check(`edycja bez zmian zachowuje wartość: ${name}`, !!j && test(j), j ? `po zapisie: ${JSON.stringify({ schedule: j.schedule, max_size: j.max_size, window: j.window, guard_pct: j.guard_pct, bandwidth: j.bandwidth, threads: j.threads })}` : `brak zapisu; #edErr="${err}"`);
    await browser.close();
  }

  // ---- 9. stary schemat: brak opcjonalnych pól w danych z backendu - edytor się otwiera
  {
    const old = mkJob(1); delete old.bandwidth; delete old.conditions; delete old.triggers; delete old.window; delete old.min_size; delete old.max_size; delete old.verify; delete old.max_versions; delete old.use_vss;
    const { browser, page, errors } = await fresh({ jobs: [old] });
    await page.click('.card [data-act=edit]'); await toStep(page, 4);
    await clickSave(page); await page.waitForTimeout(300);
    const j = saves(await page.evaluate(() => window.__stub.calls))[0]?.args.job;
    R.check("stary schemat (brak bandwidth/conditions/triggers/...) edytowalny i zapisywalny", !!j && errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 10. liczby w polach kroku 5 (natywna walidacja widoczna): ujemne wątki, guard 0, itp. -> zachowanie
  {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page); await toStep(page, 4);
    await page.fill('[name=threads]', "-3"); await page.fill('[name=guard_pct]', "250"); await page.fill('[name=bw_kbps]', "-1");
    await clickSave(page); await page.waitForTimeout(300);
    const c = saves(await page.evaluate(() => window.__stub.calls));
    R.check("wartości poza zakresem w kroku 5 nie trafiają do backendu", c.length === 0, `save_job wywołań: ${c.length}`);
    await browser.close();
  }
  // ---- 11. Enter w polu na kroku <5 nie zapisuje, tylko przechodzi dalej
  {
    const { browser, page } = await fresh();
    await page.click("#newJob"); await fillBasics(page);
    await page.focus('[name=right]'); await page.keyboard.press("Enter");
    const st = await page.locator('.step.on').getAttribute("data-step");
    R.check("Enter w kroku 1 przechodzi do kroku 2 (bez zapisu)", st === "1" && saves(await page.evaluate(() => window.__stub.calls)).length === 0);
    await browser.close();
  }
  R.done();
})();
