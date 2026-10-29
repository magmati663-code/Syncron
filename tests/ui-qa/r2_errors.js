// Runda 2: błąd backendu (rejected promise) w każdej komendzie - czy UI nie zawisa, czy komunikaty są sensowne.
// Uruchom: node tests/ui-qa/r2_errors.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("r2_errors");

const base = (extra) => (a) => {
  const S = window.__stub; S.jobs = a.jobs; S.status = {}; a.jobs.forEach((j) => (S.status[j.id] = a.st));
  S.history = [{ ts: 100, job: "job-1", kind: "kopiowanie →", path: "a.txt", detail: "" }];
  S.trash = [{ side: "left", stamp: "2025-01-02_030405", path: "a.txt", size: 1 }];
  S.remotes = [{ name: "webdav", kind: "webdav" }];
  S.settings = { notify: true, language: "pl", unavailable_minutes: 30 };
  (a.pre || []).forEach(([cmd, v]) => { S.fail[cmd] = v; });
  Object.assign(S.delay, a.delay || {});
};
async function open(pre, jobs, extra = {}, st) {
  return launch(Object.assign({ setup: base(), setupArg: { jobs: jobs || [mkJob(1), mkJob(2, { enabled: false })], st: st || mkStatus(), pre, delay: extra.delay } }, {}));
}
const toastText = (page) => page.$$eval("#toasts .toast", (e) => e.map((x) => x.textContent));

(async () => {
  // list_jobs odrzucone przy starcie
  {
    const { browser, page, errors } = await open([["list_jobs", "Nie można odczytać jobs.json: uszkodzony plik"]]);
    await page.waitForTimeout(500);
    const vis = (await page.locator("#jobs").innerText()).trim();
    const toasts = await toastText(page);
    R.check("list_jobs odrzucone na starcie: użytkownik widzi komunikat (toast/ekran), a nie pustą listę bez wyjaśnienia", toasts.length > 0 || /Nie można|Błąd/i.test(vis), `lista="${vis.slice(0, 60)}", toasty=${toasts.length}, błędy JS=${errors.length} (${errors[0] || ""})`);
    await browser.close();
  }
  // overview odrzucone: UI pokazuje nieaktualny stan bez sygnału
  {
    const { browser, page } = await open([["overview", "backend nie odpowiada"]]);
    await page.waitForTimeout(2500);
    const toasts = await toastText(page);
    R.check("overview odrzucone cyklicznie: jest jakikolwiek sygnał, że dane są nieaktualne", toasts.length > 0 || (await page.locator("#tiles").innerText()).includes("?"), "UI po cichu pokazuje stan 'Gotowe'/0 (catch { return })");
    await browser.close();
  }
  // run_now: zwraca false (zadanie wyłączone / brak wątku) a UI mówi 'Uruchomiono'
  {
    const { browser, page } = await open();
    await page.locator('.card').nth(1).locator('[data-act=run]').click(); await page.waitForTimeout(200);
    const t = (await toastText(page)).join("|");
    const dis = await page.locator('.card').nth(1).locator('[data-act=run]').isDisabled();
    R.check("'Uruchom' dla wyłączonego zadania: brak fałszywego komunikatu 'Uruchomiono' (run_now zwróciło false) albo przycisk nieaktywny", dis || !/Uruchomiono/.test(t), `przycisk aktywny=${!dis}, toast="${t}", run_now zwróciło false`);
    await browser.close();
  }
  // run_now rejected
  {
    const { browser, page } = await open([["run_now", "kanał zadania zamknięty"]]);
    await page.locator('.card').first().locator('[data-act=run]').click(); await page.waitForTimeout(200);
    const t = (await toastText(page)).join("|");
    R.check("run_now odrzucone -> toast z błędem, przycisk nadal działa", /kanał/.test(t) && !(await page.locator('.card').first().locator('[data-act=run]').isDisabled()), t);
    await browser.close();
  }
  // dry_run rejected: 'Liczę…' nie zostaje
  {
    const { browser, page } = await open([["dry_run", "brak zadania"]]);
    await page.locator('.card').first().locator('[data-act=dry]').click(); await page.waitForTimeout(300);
    const txt = await page.locator('.card').first().locator('[data-act=dry]').innerText();
    R.check("dry_run odrzucone -> przycisk wraca z 'Liczę…' do 'Symulacja' i jest aktywny", /Symulacja/.test(txt) && !(await page.locator('.card').first().locator('[data-act=dry]').isDisabled()), txt);
    R.check("dry_run odrzucone -> toast błędu", (await toastText(page)).some((x) => /brak zadania/.test(x)));
    await browser.close();
  }
  // dry_run wolny (zawieszony 'Liczę…' do czasu odpowiedzi) + kliknięcie dwukrotne
  {
    const { browser, page } = await open([], [mkJob(1)], { delay: { dry_run: 600 } });
    const b = page.locator('.card [data-act=dry]');
    await b.dblclick(); await page.waitForTimeout(900);
    const n = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "dry_run").length;
    R.check("podwójne kliknięcie 'Symulacja' = jedno wywołanie dry_run", n === 1, "wywołań: " + n);
    await browser.close();
  }
  // save_job rejected z edytora -> komunikat, edytor otwarty, przycisk działa
  {
    const { browser, page } = await open([["save_job", "Dysk jobs.json tylko do odczytu"]]);
    await page.locator('.card [data-act=edit]').first().click();
    for (let i = 0; i < 4; i++) await page.click("#edNext");
    await page.click("#edSave"); await page.waitForTimeout(250);
    R.check("save_job odrzucone -> komunikat w edytorze, dialog otwarty", /tylko do odczytu/.test(await page.textContent("#edErr")) && await page.locator("#editor[open]").count() === 1);
    R.check("save_job odrzucone -> 'Zapisz' nadal aktywny", !(await page.locator("#edSave").isDisabled()));
    await browser.close();
  }
  // save_job OK, ale list_jobs po zapisie odrzucone -> edytor zamknięty, komunikat niewidoczny
  {
    const { browser, page } = await open();
    await page.locator('.card [data-act=edit]').first().click();
    for (let i = 0; i < 4; i++) await page.click("#edNext");
    await page.evaluate(() => { window.__stub.fail.list_jobs = "I/O error"; });
    await page.click("#edSave"); await page.waitForTimeout(300);
    const toasts = (await toastText(page)).join("|");
    R.check("zapis udany, odświeżenie listy nie powiodło się -> użytkownik widzi powód (toast), a nie cichy niepowodzenie w ukrytym #edErr", toasts.length > 0 || await page.locator("#editor[open]").count() === 1, `edytor zamknięty, toasty="${toasts}", #edErr="${await page.textContent("#edErr")}"`);
    await browser.close();
  }
  // toggle / delete rejected
  {
    const { browser, page } = await open([["save_job", "błąd zapisu"], ["delete_job", "błąd usuwania"]]);
    await page.locator('.card').first().locator('[data-act=toggle]').click(); await page.waitForTimeout(200);
    R.check("toggle: save_job odrzucone -> toast", (await toastText(page)).some((x) => /błąd zapisu/.test(x)));
    await page.locator('.card').first().locator('[data-act=del]').click(); await page.click("#cfYes"); await page.waitForTimeout(250);
    R.check("delete_job odrzucone -> toast, zadanie nadal na liście", (await toastText(page)).some((x) => /błąd usuwania/.test(x)) && await page.locator(".card").count() === 2);
    await browser.close();
  }
  // history odrzucone
  {
    const { browser, page } = await open([["history", "baza zablokowana (database is locked)"]]);
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    const t = await page.locator("#hist tbody").innerText();
    const toasts = (await toastText(page)).join("|");
    R.check("history odrzucone -> błąd widoczny (toast lub w tabeli), a nie 'Brak wpisów.'", !/Brak wpisów/.test(t) || /locked/.test(toasts), `tabela="${t.trim()}", toasty="${toasts}"`);
    await browser.close();
  }
  // list_remotes odrzucone
  {
    const { browser, page } = await open([["list_remotes", "rclone: nie znaleziono pliku wykonywalnego"]]);
    await page.click("nav button[data-tab=accounts]"); await page.waitForTimeout(300);
    const t = await page.locator("#remotes").innerText();
    const toasts = (await toastText(page)).join("|");
    R.check("list_remotes odrzucone (np. brak rclone) -> błąd widoczny, a nie 'Brak połączonych kont'", !/Brak połączonych kont/.test(t) || /rclone/.test(toasts), `lista="${t.trim()}", toasty="${toasts}"`);
    await browser.close();
  }
  // add_remote odrzucone
  {
    const { browser, page } = await open([["add_remote", "Nazwa konta: min. 2 znaki"]]);
    await page.click("nav button[data-tab=accounts]"); await page.click('[data-p=sftp]');
    await page.fill("#rName", "a"); await page.fill("#rFields [data-f=host]", "serwer.example.com"); await page.click("#rAdd"); await page.waitForTimeout(250);
    R.check("add_remote odrzucone -> komunikat 'Błąd: ...' i przycisk aktywny", /Błąd: Nazwa/.test(await page.textContent("#rMsg")) && !(await page.locator("#rAdd").isDisabled()));
    await browser.close();
  }
  // add_remote: ponowne użycie istniejącej nazwy (domyślnie 'gdrive') nie jest blokowane
  {
    const { browser, page } = await open();
    await page.click("nav button[data-tab=accounts]"); await page.click('[data-p=webdav]'); await page.waitForTimeout(100); await page.fill("#rFields [data-f=url]", "https://cloud.example.com/dav/");
    const name = await page.inputValue("#rName");
    await page.click("#rAdd"); await page.waitForTimeout(250);
    const c = (await page.evaluate(() => window.__stub.calls)).filter((x) => x.cmd === "add_remote");
    R.check(`dodanie konta pod już istniejącą nazwą ("${name}") jest zablokowane/ostrzeżone (inaczej rclone config create nadpisze konto)`, c.length === 0 || (await page.locator("#confirm[open]").count()) === 1, `add_remote wywołane z name="${c[0] && c[0].args.name}" mimo istniejącego konta`);
    await browser.close();
  }
  // add_remote: hasło z białymi znakami przycinane
  {
    const { browser, page } = await open();
    await page.click("nav button[data-tab=accounts]"); await page.click('[data-p=webdav]');
    await page.fill("#rName", "nowe"); await page.fill('[data-f=pass]', "  tajne haslo  "); await page.fill('[data-f=url]', "https://x/"); await page.click("#rAdd"); await page.waitForTimeout(250);
    const c = (await page.evaluate(() => window.__stub.calls)).filter((x) => x.cmd === "add_remote")[0];
    R.check("hasło konta wysyłane bez zmian (bez przycinania spacji)", c && c.args.params.pass === "  tajne haslo  ", `wysłano "${c && c.args.params.pass}"`);
    await browser.close();
  }
  // test_remote / delete_remote
  {
    const { browser, page } = await open([["test_remote", "timeout"], ["delete_remote", "nie można usunąć"]]);
    await page.click("nav button[data-tab=accounts]"); await page.waitForTimeout(200);
    await page.click("[data-t=webdav]"); await page.waitForTimeout(250);
    R.check("test_remote odrzucone -> komunikat 'timeout' przy koncie (lub toast), przycisk 'Testuj' aktywny", (/timeout/.test(await page.textContent("[data-rt=webdav]")) || (await toastText(page)).some((x) => /timeout/.test(x))) && !(await page.locator("[data-t=webdav]").isDisabled()));
    await page.click("[data-d=webdav]"); await page.click("#cfYes"); await page.waitForTimeout(250);
    R.check("delete_remote odrzucone -> toast", (await toastText(page)).some((x) => /nie można usunąć/.test(x)));
    await browser.close();
  }
  // trash
  {
    const { browser, page } = await open([["trash_list", "nie można odczytać kosza"]]);
    await page.click("nav button[data-tab=trash]"); await page.waitForTimeout(300);
    R.check("trash_list odrzucone -> błąd w tabeli", /odczytać kosza/.test(await page.locator("#trashTbl tbody").innerText()));
    await browser.close();
  }
  {
    const { browser, page } = await open([["trash_restore", "plik docelowy zablokowany"], ["trash_delete", "odmowa dostępu"]]);
    await page.click("nav button[data-tab=trash]"); await page.waitForTimeout(300);
    await page.click("[data-tr=restore]"); await page.waitForTimeout(200);
    R.check("trash_restore odrzucone -> toast", (await toastText(page)).some((x) => /zablokowany/.test(x)));
    await page.click("[data-tr=del]"); await page.click("#cfYes"); await page.waitForTimeout(200);
    R.check("trash_delete odrzucone -> toast", (await toastText(page)).some((x) => /odmowa/.test(x)));
    await browser.close();
  }
  // set_paused odrzucone -> nieobsłużony wyjątek
  {
    const { browser, page, errors } = await open([["set_paused", "błąd"]]);
    await page.click("#pauseAll"); await page.waitForTimeout(300);
    R.check("set_paused odrzucone -> obsłużone (toast), bez nieprzechwyconego wyjątku", errors.length === 0 && (await toastText(page)).length > 0, `błędy JS: ${errors.join("|")}`);
    await browser.close();
  }
  // resolve_conflict odrzucone
  {
    const st = mkStatus(); st.last.conflicts = ["k.txt"];
    const { browser, page, errors } = await open([["resolve_conflict", "plik zajęty"]], [mkJob(1)], {}, st);
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    await page.click("[data-res=left]"); await page.waitForTimeout(300);
    R.check("resolve_conflict odrzucone -> toast i przyciski ponownie aktywne", (await toastText(page)).some((x) => /plik zajęty/.test(x)) && !(await page.locator("[data-res=left]").first().isDisabled()));
    await browser.close();
  }
  // get_settings / set_settings
  {
    const { browser, page } = await open([["get_settings", "settings.json uszkodzony"]]);
    await page.click("nav button[data-tab=settings]"); await page.waitForTimeout(200);
    R.check("get_settings odrzucone -> informacja, że ustawienia są domyślne/nieaktualne", (await toastText(page)).length > 0, "UI cicho pokazuje wartości domyślne jako rzeczywiste");
    await browser.close();
  }
  {
    const { browser, page } = await open();
    await page.click("nav button[data-tab=settings]");
    await page.fill("#unavail", "-5"); await page.dispatchEvent("#unavail", "change"); await page.waitForTimeout(200);
    const t = (await toastText(page)).join("|");
    R.check("ujemna wartość 'niedostępne dłużej niż (minuty)' -> zrozumiały komunikat (nie surowy 'invalid args')", t.length > 0 && !/invalid args/.test(t), `toast="${t}"`);
    await page.fill("#unavail", ""); await page.dispatchEvent("#unavail", "change"); await page.waitForTimeout(200);
    const saved = await page.evaluate(() => window.__stub.settings.unavailable_minutes);
    R.check("puste pole 'minuty' nie wyłącza po cichu powiadomień (zapis 0 = wyłączone)", saved !== 0, `zapisano unavailable_minutes=${saved}`);
    await page.fill("#unavail", "1.5"); await page.dispatchEvent("#unavail", "change"); await page.waitForTimeout(200);
    R.check("ułamek w 'minuty' -> zrozumiały komunikat albo zaokrąglenie", !(await toastText(page)).some((x) => /invalid args/.test(x)), (await toastText(page)).join("|"));
    await browser.close();
  }
  // export / import / diag / logs
  {
    const { browser, page, errors } = await open([["export_config", "brak miejsca na dysku"], ["import_config", "to nie jest plik konfiguracji Syncron: expected value at line 1 column 1"], ["get_diagnostics", "diag błąd"], ["open_logs_dir", "xdg-open: not found"]]);
    await page.click("nav button[data-tab=settings]");
    await page.evaluate(() => { window.__stub.dialogSave = "C:\\x.json"; window.__stub.dialogOpen = "C:\\y.json"; });
    await page.click("#cfgExport"); await page.waitForTimeout(150);
    await page.click("#cfgImport"); await page.waitForTimeout(150);
    await page.click("#diagCopy"); await page.waitForTimeout(150);
    await page.click("#logsOpen"); await page.waitForTimeout(150);
    const t = await toastText(page);
    R.check("export/import/diagnostyka/logi odrzucone -> 4 toasty błędów", t.length === 4, JSON.stringify(t));
    R.check("brak wyjątków JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }
  // check_path / list_dirs
  {
    const { browser, page } = await open([["check_path", "x"], ["list_dirs", "Brak dostępu do folderu"]]);
    await page.click("#newJob"); await page.fill("[name=left]", "C:\\A"); await page.dispatchEvent("[name=left]", "change"); await page.fill("[name=right]", "D:\\B");
    await page.click("#edNext"); await page.click("#edNext"); await page.click("#treeLoad"); await page.waitForTimeout(250);
    R.check("list_dirs odrzucone -> błąd w drzewie", /Brak dostępu/.test(await page.locator("#tree").innerText()));
    await browser.close();
  }
  // brakująca komenda (np. niezgodna wersja) -> zachowanie
  {
    const { browser, page, errors } = await open([["vss_status", "Command vss_status not found"], ["rclone_version", "rclone: nie znaleziono"]]);
    await page.click("nav button[data-tab=settings]"); await page.waitForTimeout(200);
    R.check("rclone_version odrzucone -> czytelny tekst w ustawieniach", /niedostępny/.test(await page.textContent("#rcv")));
    R.check("vss_status odrzucone -> brak błędów JS", errors.length === 0, errors.join("|"));
    await browser.close();
  }
  // brak aktualizacji: updater odrzuca
  {
    const { browser, page } = await open();
    await page.click("nav button[data-tab=settings]");
    await page.evaluate(() => { window.__TAURI__.updater.check = async () => { throw new Error("404"); }; });
    await page.click("#checkUpd"); await page.waitForTimeout(200);
    R.check("błąd updatera -> komunikat 'Nie udało się sprawdzić aktualizacji'", /Nie udało się/.test(await page.textContent("#updMsg")));
    await browser.close();
  }
  R.done();
})();
