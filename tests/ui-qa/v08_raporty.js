// v0.8, zespół E: podsumowanie i eksport raportu w „Aktywności” oraz powiadomienia/webhook w kreatorze zadania.
// Uruchom: node tests/ui-qa/v08_raporty.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("v08_raporty");

const PL_DIA = /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/;
const setup = (a) => {
  const S = window.__stub; S.jobs = a.jobs; S.status = {}; a.jobs.forEach((j) => (S.status[j.id] = a.st));
  S.history = a.history || [];
  if (a.summary) S.summary = a.summary;
  if (a.dialogSave !== undefined) S.dialogSave = a.dialogSave;
  if (a.fail) Object.assign(S.fail, a.fail);
};
const fresh = (o = {}) => launch(Object.assign({ setup, setupArg: { jobs: o.jobs || [mkJob(1), mkJob(2, { name: "<b>Dwa</b>" })], st: mkStatus(), history: o.history, summary: o.summary, dialogSave: o.dialogSave, fail: o.fail } }, o.launch || {}));
const gotoActivity = async (page) => { await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(400); };
const lastCall = async (calls, cmd) => (await calls()).filter((c) => c.cmd === cmd).pop();
const toasts = (page) => page.$$eval("#toasts .toast", (e) => e.map((x) => x.textContent));
const next = async (page, n) => { for (let i = 0; i < n; i++) await page.click("#edNext"); };

(async () => {
  // ---- 1. podsumowanie
  {
    const { browser, page, errors, calls } = await fresh();
    await gotoActivity(page);
    const cards = await page.$$eval("#repCards .rcard", (e) => e.map((x) => x.innerText.replace(/\s+/g, " ").trim()));
    R.check("5 kart podsumowania", cards.length === 5, JSON.stringify(cards));
    R.check("karty: przebiegi, skopiowane (z rozmiarem), błędy", cards[0].includes("3") && cards[1].includes("4") && cards[1].includes("3.0 MiB") && cards[4].includes("2"), JSON.stringify(cards));
    R.check("karta błędów podświetlona", await page.locator("#repCards .rcard.bad").count() === 1 && await page.locator("#repCards .rcard.warn").count() === 1);
    R.check("najczęstsze błędy pokazane", (await page.textContent("#repTop")).includes("brak dostępu") && (await page.textContent("#repTop")).includes("2×"));
    let c = await lastCall(calls, "report_summary");
    const now = Date.now() / 1000;
    R.check("domyślny zakres 30 dni, bez zadania", c && c.args.filter.job === null && Math.abs(c.args.filter.since - (now - 30 * 86400)) < 120, JSON.stringify(c?.args));
    await page.selectOption("#repRange", "0"); await page.waitForTimeout(250);
    c = await lastCall(calls, "report_summary");
    R.check("„Cała historia” = brak dolnej granicy", c.args.filter.since === null);
    await page.selectOption("#histJob", "job-2"); await page.waitForTimeout(300);
    c = await lastCall(calls, "report_summary");
    R.check("zmiana zadania przelicza podsumowanie dla tego zadania", c.args.filter.job === "job-2", JSON.stringify(c.args));
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 2. podsumowanie: błąd backendu i XSS w komunikatach
  {
    const evil = { total: 1, runs: 1, copied: 0, deleted: 0, moved: 0, conflicts: 0, resolved: 0, errors: 1, warnings: 0, busy: 0, bytes_copied: 0, bytes_known_files: 0, first_ts: 1, last_ts: 1, top_errors: [{ message: "<img src=x onerror=window.__xss=1>", count: 1 }], by_job: [] };
    const { browser, page, errors } = await fresh({ summary: evil });
    await gotoActivity(page);
    R.check("XSS: komunikat błędu jest tekstem", await page.locator("#repTop img").count() === 0 && (await page.textContent("#repTop")).includes("<img") && !(await page.evaluate(() => window.__xss)));
    R.check("bez rozmiaru gdy nieznany", !(await page.textContent("#repCards")).includes("MiB"));
    await browser.close();
    const f = await fresh({ fail: { report_summary: "Błąd odczytu historii." } });
    await gotoActivity(f.page);
    R.check("błąd podsumowania pokazany w miejscu kart", (await f.page.textContent("#repCards")).includes("Błąd odczytu historii.") && await f.page.locator("#repTop").isHidden());
    R.check("błąd podsumowania nie psuje historii (tabela jest)", await f.page.locator("#hist").count() === 1);
    await f.browser.close();
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
  }

  // ---- 3. eksport raportu
  {
    const { browser, page, errors, calls } = await fresh({ dialogSave: "/tmp/raport.html" });
    await gotoActivity(page);
    await page.click("#repExport");
    R.check("okno eksportu otwarte, zadania w liście (z escapowaniem)", await page.locator("#exportDlg[open]").count() === 1 && await page.locator("#expJob option").count() === 3 && await page.locator("#expJob b").count() === 0);
    R.check("domyślnie 30 dni, daty własne ukryte", (await page.inputValue("#expRange")) === "30" && await page.locator("#expDates").isHidden());
    await page.selectOption("#expRange", "c");
    R.check("własny zakres pokazuje daty", await page.locator("#expDates").isVisible());
    await page.click("#expGo");
    R.check("własny zakres bez dat: komunikat, brak zapisu", (await page.textContent("#expErr")).includes("Podaj datę") && !(await calls()).some((x) => x.cmd === "export_report"));
    await page.fill("#expFrom", "2026-03-10"); await page.fill("#expTo", "2026-03-01");
    await page.click("#expGo");
    R.check("odwrócony zakres odrzucony", (await page.textContent("#expErr")).includes("nie może być późniejsza"));
    await page.fill("#expTo", "2026-03-20");
    await page.selectOption("#expFormat", "html"); await page.selectOption("#expJob", "job-1"); await page.selectOption("#expKind", "błąd");
    await page.click("#expGo"); await page.waitForTimeout(300);
    const e = await lastCall(calls, "export_report");
    const from = new Date("2026-03-10T00:00:00").getTime() / 1000, to = new Date("2026-03-20T23:59:59").getTime() / 1000;
    R.check("export_report: format, ścieżka z okna zapisu, filtr", e && e.args.format === "html" && e.args.path === "/tmp/raport.html" && e.args.filter.job === "job-1" && e.args.filter.kind === "błąd" && e.args.filter.since === from && e.args.filter.until === to, JSON.stringify(e?.args));
    R.check("okno zamknięte i toast z liczbą wpisów", await page.locator("#exportDlg[open]").count() === 0 && (await toasts(page)).some((t) => t.includes("Zapisano raport: 12")), JSON.stringify(await toasts(page)));
    // anulowanie okna zapisu = brak eksportu
    await page.evaluate(() => { window.__stub.dialogSave = null; window.__stub.calls.length = 0; });
    await page.click("#repExport"); await page.selectOption("#expRange", "7"); await page.click("#expGo"); await page.waitForTimeout(200);
    R.check("anulowany wybór pliku: brak export_report, okno nadal otwarte", !(await calls()).some((x) => x.cmd === "export_report") && await page.locator("#exportDlg[open]").count() === 1);
    // błąd backendu
    await page.evaluate(() => { window.__stub.dialogSave = "/tmp/x.csv"; window.__stub.fail.export_report = "Nie można zapisać raportu: brak miejsca."; });
    await page.click("#expGo"); await page.waitForTimeout(200);
    R.check("błąd eksportu widoczny w oknie, przycisk znów aktywny", (await page.textContent("#expErr")).includes("brak miejsca") && await page.locator("#expGo").isEnabled());
    const e2 = await lastCall(calls, "export_report");
    R.check("zakres 7 dni → since ≈ teraz-7d, until null", e2 && Math.abs(e2.args.filter.since - (Date.now() / 1000 - 7 * 86400)) < 120 && e2.args.filter.until === null, JSON.stringify(e2?.args));
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 4. kreator: powiadomienia i webhook
  {
    const { browser, page, errors, calls } = await fresh();
    await page.click("#newJob");
    await page.fill("[name=name]", "N"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    await next(page, 4);
    R.check("krok 5 ma sekcję powiadomień", await page.locator("#notifyBox").isVisible());
    R.check("domyślnie: powiadomienia „Errors”, webhook „Never”, pola webhooka ukryte", (await page.inputValue("[name=notify_on]")) === "Errors" && (await page.inputValue("[name=webhook_on]")) === "Never" && await page.locator("#whFields").isHidden());
    await page.selectOption("[name=webhook_on]", "Errors");
    R.check("wybór webhooka odsłania pola", await page.locator("#whFields").isVisible());
    R.check("ostrzeżenie o prywatności URL", (await page.textContent("#whFields .msg.warn")).includes("Prywatność"));
    // brak adresu przy włączonym webhooku
    await page.click("#edSave"); await page.waitForTimeout(200);
    R.check("webhook włączony bez adresu: błąd i brak zapisu", (await page.textContent("#edErr")).includes("Podaj adres webhooka") && !(await calls()).some((c) => c.cmd === "save_job"));
    for (const bad of ["ftp://x/y", "hooks.example.com/abc", "https://user:pw@host/x", "https:///x"]) {
      await page.fill("[name=webhook_url]", bad); await page.click("#edSave"); await page.waitForTimeout(120);
      R.check(`zły adres „${bad}” odrzucony`, (await page.textContent("#edErr")).includes("musi zaczynać się") && !(await calls()).some((c) => c.cmd === "save_job"));
    }
    // przycisk testowy
    await page.fill("[name=webhook_url]", "");
    await page.click("#whTest");
    R.check("test bez adresu: komunikat, brak wywołania", (await page.textContent("#whMsg")).includes("Najpierw podaj") && !(await calls()).some((c) => c.cmd === "test_webhook"));
    await page.fill("[name=webhook_url]", "http://192.168.1.5:8080/moj-temat"); await page.selectOption("[name=webhook_format]", "Ntfy"); await page.check("[name=webhook_allow_private]");
    await page.click("#whTest"); await page.waitForTimeout(250);
    const t = await lastCall(calls, "test_webhook");
    R.check("test_webhook: url, format, allowPrivate", t && t.args.url === "http://192.168.1.5:8080/moj-temat" && t.args.format === "Ntfy" && t.args.allowPrivate === true, JSON.stringify(t?.args));
    R.check("komunikat powodzenia testu", (await page.textContent("#whMsg")).includes("Wysłano") && await page.locator("#whMsg.ok").count() === 1);
    await page.evaluate(() => { window.__stub.fail.test_webhook = "Webhook http://1…(zamaskowano) nie powiódł się: serwer odpowiedział kodem 500"; });
    await page.click("#whTest"); await page.waitForTimeout(250);
    R.check("błąd testu pokazany (bez pełnego adresu)", (await page.textContent("#whMsg")).includes("kodem 500") && !(await page.textContent("#whMsg")).includes("moj-temat") && await page.locator("#whMsg.bad").count() === 1);
    await page.selectOption("[name=notify_on]", "Always"); await page.selectOption("[name=webhook_on]", "Always");
    R.check("podsumowanie kreatora wspomina o powiadomieniach i webhooku", (await page.textContent("#summary")).includes("Powiadomienia systemowe: po każdym przebiegu") && (await page.textContent("#summary")).includes("Webhook (Ntfy): po każdym przebiegu"), await page.textContent("#summary"));
    await page.click("#edSave"); await page.waitForTimeout(400);
    const s = await lastCall(calls, "save_job");
    R.check("save_job zawiera pola powiadomień", s && s.args.job.notify_on === "Always" && s.args.job.webhook_on === "Always" && s.args.job.webhook_url === "http://192.168.1.5:8080/moj-temat" && s.args.job.webhook_format === "Ntfy" && s.args.job.webhook_allow_private === true, (await page.textContent("#edErr")));
    // edycja: wartości wczytane z zadania (czekamy na odświeżenie listy zadań po zapisie)
    await page.waitForSelector(".card [data-act=edit]"); await page.waitForTimeout(700);
    await page.locator(".card", { hasText: "N" }).last().locator("[data-act=edit]").click(); await next(page, 4);
    R.check("edycja wczytuje zapisane wartości", (await page.inputValue("[name=notify_on]")) === "Always" && (await page.inputValue("[name=webhook_url]")) === "http://192.168.1.5:8080/moj-temat" && (await page.inputValue("[name=webhook_format]")) === "Ntfy" && await page.isChecked("[name=webhook_allow_private]") && await page.locator("#whFields").isVisible(), JSON.stringify(await page.evaluate(() => { const f = document.querySelector("#jobForm").elements; return [f.notify_on.value, f.webhook_url.value, f.webhook_format.value, f.webhook_allow_private.checked, document.querySelector("#whFields").hidden, document.querySelector("#editor").open]; })));
    await page.click("#edCancel");
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 5. stare zadanie bez nowych pól + wyłączenie webhooka czyści adres
  {
    const old = mkJob(7, { name: "Stare" });
    const { browser, page, errors, calls } = await fresh({ jobs: [old] });
    await page.click(".card [data-act=edit]"); await next(page, 4);
    R.check("stare zadanie: domyślne wartości", (await page.inputValue("[name=notify_on]")) === "Errors" && (await page.inputValue("[name=webhook_on]")) === "Never" && (await page.inputValue("[name=webhook_url]")) === "");
    await page.click("#edSave"); await page.waitForTimeout(300);
    const s = await lastCall(calls, "save_job");
    R.check("zapis starego zadania: webhook_url null, notify_on Errors", s && s.args.job.webhook_url === null && s.args.job.notify_on === "Errors" && s.args.job.webhook_on === "Never", JSON.stringify(s?.args?.job));
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---- 6. angielski: nowy interfejs bez polskich tekstów
  {
    const { browser, page, errors } = await fresh({ launch: { lang: "en" } });
    await gotoActivity(page);
    const act = await page.evaluate(() => [document.querySelector("#repBar").innerText, document.querySelector("#repCards").innerText].join("\n"));
    R.check("EN: pasek raportu i karty po angielsku", !PL_DIA.test(act) && /Export report/.test(act) && /Runs/.test(act) && /Copied/.test(act) && /Errors/.test(act), act.replace(/\n/g, " | "));
    await page.click("#repExport");
    const dlg = await page.evaluate(() => [...document.querySelectorAll("#exportDlg .dh, #exportDlg label, #exportDlg option, #exportDlg p, #exportDlg button")].map((e) => e.textContent).join("\n"));
    const names = ["Dwa"];
    R.check("EN: okno eksportu po angielsku", !PL_DIA.test(dlg.replace(/Zadanie \d|<b>Dwa<\/b>/g, "")) && /Save report/.test(dlg), dlg.slice(0, 200).replace(/\n/g, " | "));
    await page.click("#expCancel");
    await page.click("nav button[data-tab=jobs]"); await page.click("#newJob");
    await page.fill("[name=name]", "N"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    await next(page, 4);
    await page.selectOption("[name=webhook_on]", "Always");
    const box = await page.evaluate(() => [...document.querySelectorAll("#notifyBox b, #notifyBox label, #notifyBox option, #notifyBox p, #notifyBox button, #notifyBox small")].map((e) => e.textContent).join("\n") + document.querySelector("[name=webhook_url]").placeholder);
    R.check("EN: sekcja powiadomień w kreatorze po angielsku", !PL_DIA.test(box) && /Privacy/.test(box) && /Send test/.test(box) && /After every run/.test(box), box.slice(0, 200).replace(/\n/g, " | "));
    void names;
    R.check("brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }
  R.done();
})().catch((e) => { console.error("BŁĄD SKRYPTU", e); process.exit(2); });
