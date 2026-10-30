// Runda 2: polling co 2 s i wyścigi - utrata fokusu, nieaktualny czas względny, wyścig w koszu, ponowne wejście ask(), dostępność klawiatury, wyszukiwanie w EN.
// Uruchom: node tests/ui-qa/r2_races.js     (FAIL = potwierdzony błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("r2_races");
const toastText = (page) => page.$$eval("#toasts .toast", (e) => e.map((x) => x.textContent));
const seed = (a) => {
  const S = window.__stub; S.jobs = a.jobs; S.status = a.status; S.history = a.history || []; S.trash = a.trash || [];
  if (a.delayFnSrc) S.delayFn = new Function("cmd", "args", a.delayFnSrc);
};

(async () => {
  // 1. fokus na przycisku 'Zachowaj lewą' przeżywa odświeżenie co 2 s?
  {
    const st = mkStatus(); st.last.conflicts = ["k.txt"];
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": st } } });
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    await page.focus("[data-res=left]");
    await page.waitForTimeout(2600);
    const still = await page.evaluate(() => document.activeElement && document.activeElement.matches("[data-res=left]"));
    R.check("Aktywność: fokus na przycisku konfliktu nie ginie po cyklu odświeżania (renderPending co 2 s przebudowuje DOM)", still, "document.activeElement po 2,6 s: " + (await page.evaluate(() => document.activeElement.tagName)));
    // mousedown przed odświeżeniem, mouseup po -> klik gubiony
    const box = await page.locator("[data-res=left]").boundingBox();
    await page.mouse.move(box.x + 5, box.y + 5); await page.mouse.down();
    await page.waitForTimeout(2300);
    await page.mouse.up(); await page.waitForTimeout(200);
    const resolved = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "resolve_conflict").length;
    R.check("Aktywność: klik trwający >2 s (mousedown, odświeżenie, mouseup) nadal działa", resolved === 1, "resolve_conflict wywołań: " + resolved);
    await browser.close();
  }
  // 2. fokus na karcie zadania podczas synchronizacji (zmieniający się postęp)
  {
    const st = mkStatus({ state: "running", progress: { done: 1, total: 100, current: "a" } });
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": st } } });
    await page.focus("[data-act=edit]");
    await page.evaluate(() => { const S = window.__stub; let n = 1; setInterval(() => { S.status["job-1"].progress.done = ++n; }, 500); });
    await page.waitForTimeout(2600);
    const still = await page.evaluate(() => document.activeElement && document.activeElement.matches("[data-act=edit]"));
    R.check("Zadania: fokus klawiatury na przycisku karty przeżywa odświeżanie podczas synchronizacji", still, "karta przebudowywana (innerHTML) przy każdej zmianie postępu");
    await browser.close();
  }
  // 3. nieaktualny czas względny
  {
    const st = mkStatus({ last_run: Math.floor(Date.now() / 1000) - 10 });
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": st } } });
    await page.waitForTimeout(300);
    const before = await page.locator(".card .meta").innerText();
    await page.evaluate(() => { const o = Date.now; Date.now = () => o() + 3 * 3600 * 1000; });
    await page.waitForTimeout(2600);
    const after = await page.locator(".card .meta").innerText();
    R.check("'Ostatnio …' odświeża się z upływem czasu (bez zmiany stanu zadania)", before !== after, `przed: "${before}" po +3 h: "${after}"`);
    await browser.close();
  }
  // 4. wyścig w koszu: odpowiedź dla poprzedniego zadania nadpisuje tabelę nowego
  {
    const jobs = [mkJob(1), mkJob(2)];
    const trash = [{ job: "job-1", side: "left", stamp: "2025-01-02_030405", path: "z-zadania-1.txt", size: 1 }, { job: "job-2", side: "left", stamp: "2025-01-02_030405", path: "z-zadania-2.txt", size: 1 }];
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs, status: {}, trash, delayFnSrc: "return cmd==='trash_list' && args.id==='job-1' ? 900 : 0;" } });
    await page.click("nav button[data-tab=trash]");
    await page.selectOption("#trashJob", "job-2");
    await page.waitForTimeout(1500);
    const tbl = await page.locator("#trashTbl tbody").innerText();
    const sel = await page.inputValue("#trashJob");
    R.check("Kosz: po szybkiej zmianie zadania tabela pokazuje pliki wybranego zadania", /z-zadania-2/.test(tbl) && !/z-zadania-1/.test(tbl), `wybrane: ${sel}, tabela: "${tbl.replace(/\s+/g, " ").trim()}"`);
    await browser.close();
  }
  // 4b. to samo w historii (filtr zadania)
  {
    const jobs = [mkJob(1), mkJob(2)];
    const history = [{ ts: 1, job: "job-1", kind: "usunięcie", path: "h-job1.txt", detail: "" }, { ts: 2, job: "job-2", kind: "usunięcie", path: "h-job2.txt", detail: "" }];
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs, status: {}, history, delayFnSrc: "return cmd==='history' && args.job===null ? 900 : 0;" } });
    await page.click("nav button[data-tab=activity]");
    await page.selectOption("#histJob", "job-2");
    await page.waitForTimeout(1500);
    const tbl = await page.locator("#hist tbody").innerText();
    R.check("Historia: po szybkiej zmianie filtra zadania lista odpowiada filtrowi", !/h-job1/.test(tbl), `tabela: "${tbl.replace(/\s+/g, " ").trim()}"`);
    await browser.close();
  }
  // 5. ask() wywołane, gdy okno potwierdzenia jest już otwarte (np. cicha aktualizacja po 8 s)
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": mkStatus() } } });
    await page.click("[data-act=del]"); await page.waitForTimeout(100);
    await page.evaluate(() => { window.__stub.upd = { version: "9.9.9", downloadAndInstall: async () => { window.__installed = 1; } }; checkUpdates(true); });
    await page.waitForTimeout(200);
    const title = await page.textContent("#cfTitle");
    await page.click("#cfYes"); await page.waitForTimeout(300);
    const del = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "delete_job").length;
    R.check("Okno potwierdzenia 'Usunąć zadanie?' nie jest nadpisywane przez równoległe ask() (aktualizacja)", /Usunąć zadanie/.test(title), `tytuł po nałożeniu: "${title}"`);
    R.check("Kliknięcie 'Usuń' w tym oknie usuwa zadanie (obietnica nie zostaje osierocona)", del === 1, `delete_job wywołań: ${del}`);
    R.check("Kliknięcie 'Usuń' NIE jest odczytane jako zgoda na instalację aktualizacji", !(await page.evaluate(() => window.__installed)), "odpowiedź użytkownika trafiła do ask() aktualizacji: downloadAndInstall() + relaunch");
    await browser.close();
  }
  // 6. wyszukiwanie w historii w EN działa na tekście widocznym
  {
    const { browser, page } = await launch({ lang: "en", setup: seed, setupArg: { jobs: [mkJob(1)], status: {}, history: [{ ts: 5, job: "job-1", kind: "kopiowanie →", path: "a.txt", detail: "" }] } });
    await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(300);
    const vis = await page.locator("#hist tbody tr").first().innerText();
    await page.fill("#histQ", "copy"); await page.waitForTimeout(150);
    const rows = await page.locator("#hist tbody tr").first().innerText();
    R.check("EN: wyszukiwanie 'copy' znajduje wiersz z widocznym 'copy →'", /a\.txt/.test(rows), `widoczne: "${vis.replace(/\s+/g, " ")}", po filtrze: "${rows.replace(/\s+/g, " ")}"`);
    await browser.close();
  }
  // 7. dostępność klawiaturowa
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [], status: {} } });
    await page.click("#newJob"); await page.fill("[name=left]", "A"); await page.fill("[name=right]", "B"); await page.click("#edNext");
    let reached = false;
    await page.focus("#edBack");
    for (let i = 0; i < 12; i++) { await page.keyboard.press("Shift+Tab"); if (await page.evaluate(() => document.activeElement.name === "mode")) reached = true; }
    for (let i = 0; i < 12 && !reached; i++) { await page.keyboard.press("Tab"); if (await page.evaluate(() => document.activeElement.name === "mode")) reached = true; }
    R.check("Kreator krok 2: wybór kierunku (radio 'mode') osiągalny klawiaturą (Tab)", reached, ".opt input{display:none} -> radio nie są fokusowalne; wybór tylko myszą");
    await page.click("#edNext"); await page.click("#edNext");
    let sch = false;
    for (let i = 0; i < 14; i++) { await page.keyboard.press("Tab"); if (await page.evaluate(() => document.activeElement.name === "sched")) sch = true; }
    R.check("Kreator krok 4: wybór harmonogramu (radio 'sched') osiągalny klawiaturą", sch);
    await page.click("#edNext");
    await page.focus("#edNext").catch(() => {});
    await browser.close();
  }
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [], status: {} } });
    await page.click("#newJob"); await page.fill("[name=left]", "A"); await page.fill("[name=right]", "B");
    for (let i = 0; i < 3; i++) await page.click("#edNext");
    await page.focus("#edNext"); await page.keyboard.press("Enter"); await page.waitForTimeout(100);
    const ae = await page.evaluate(() => document.activeElement.tagName + "#" + document.activeElement.id);
    R.check("Po 'Dalej' na kroku 4 (przycisk znika) fokus trafia na sensowny element dialogu, a nie na BODY", !/^BODY/.test(ae), "activeElement: " + ae);
    await browser.close();
  }
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [], status: {} } });
    const live = await page.evaluate(() => ({ toasts: document.getElementById("toasts").getAttribute("aria-live") || document.getElementById("toasts").getAttribute("role"), err: document.getElementById("edErr").getAttribute("role") || document.getElementById("edErr").getAttribute("aria-live"), nav: document.querySelector("nav button.active").getAttribute("aria-current") }));
    R.check("Komunikaty (toasty, #edErr) mają role/aria-live dla czytników ekranu", !!live.toasts && !!live.err, JSON.stringify(live));
    await browser.close();
  }
  // 8. Esc w oknie potwierdzenia = Anuluj; Esc w edytorze zamyka
  {
    const { browser, page } = await launch({ setup: seed, setupArg: { jobs: [mkJob(1)], status: { "job-1": mkStatus() } } });
    await page.click("[data-act=del]"); await page.keyboard.press("Escape"); await page.waitForTimeout(100);
    const del = (await page.evaluate(() => window.__stub.calls)).filter((c) => c.cmd === "delete_job").length;
    R.check("Esc w oknie 'Usunąć zadanie?' anuluje (bez usunięcia)", del === 0 && await page.locator("#confirm[open]").count() === 0);
    await page.click("[data-act=del]"); await page.click("#cfNo"); await page.click("[data-act=del]");
    R.check("Okno potwierdzenia można otworzyć ponownie po anulowaniu i zamknąć", await page.locator("#confirm[open]").count() === 1);
    await browser.close();
  }
  R.done();
})();
