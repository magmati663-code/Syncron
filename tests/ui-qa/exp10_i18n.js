// Ekspert 10: tłumaczenie PL->EN nie może zniekształcać danych użytkownika, a stały interfejs ma być po angielsku.
// Uruchom: node tests/ui-qa/exp10_i18n.js     (FAIL = błąd aplikacji)
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("exp10_i18n");

const NAMES = ["Lustro", "Zadanie", "Kopia zapasowa", "Plik", "i", "ręcznie", "usunięto skopiowano", "Zadania $& $' $1", "Wymaga uwagi"];
const FILES = ["Plik", "ręcznie.txt", "usunięto skopiowano.docx", "Zadanie", "i", "price $&.txt", "a$'b$`c.txt"];
const PL_DIA = /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/;

(async () => {
  const jobs = NAMES.map((n, i) => mkJob(i + 1, { name: n, left: "C:\\Dane\\" + n, right: "D:\\Kopia\\" + n }));
  const { browser, page, errors } = await launch({
    lang: "en",
    setup: (a) => {
      const S = window.__stub; S.jobs = a.jobs; S.status = {};
      S.status[a.jobs[0].id] = a.st;
      S.history = a.files.map((f, i) => ({ ts: 100 + i, job: a.jobs[i % a.jobs.length].id, kind: "kopiowanie →", path: f, detail: "" }));
      S.trash = a.files.map((f, i) => ({ side: i % 2 ? "left" : "right", stamp: "20260101T000000", path: f, size: 5 }));
      S.remotes = [{ name: "Zadanie", kind: "drive" }, { name: "Plik", kind: "webdav" }];
    },
    setupArg: { jobs, files: FILES, st: mkStatus() },
  });
  await page.waitForTimeout(2300);
  const txt = (sel) => page.$$eval(sel, (e) => e.map((x) => x.textContent.trim()));
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

  // --- karty: nazwy, ścieżki
  const names = await txt(".card .name");
  R.check("EN: nazwy zadań na kartach bez zmian", same(names, NAMES), JSON.stringify(names));
  const locs = await txt(".card .loc .t span");
  R.check("EN: ścieżki lokalizacji na kartach bez zmian", locs.includes("C:\\Dane\\Lustro") && locs.includes("D:\\Kopia\\Kopia zapasowa") && locs.includes("C:\\Dane\\Zadania $& $' $1"), JSON.stringify(locs.slice(0, 4)));
  const titles = await page.$$eval(".card .loc .t span", (e) => e.map((x) => x.title));
  R.check("EN: atrybut title ścieżek nie jest tłumaczony", titles.includes("D:\\Kopia\\Plik"), JSON.stringify(titles.slice(0, 4)));

  // --- aktywność: historia, oczekujące
  await page.click("nav button[data-tab=activity]"); await page.waitForTimeout(400);
  const hp = await txt("#hist tbody tr td:nth-child(4)");
  R.check("EN: pliki w historii bez zmian", same(hp.sort(), FILES.slice().sort()), JSON.stringify(hp));
  const hj = await txt("#hist tbody tr td:nth-child(2)");
  R.check("EN: nazwy zadań w historii bez zmian", hj.every((x) => NAMES.includes(x)), JSON.stringify(hj));
  const kinds = await txt("#hist tbody tr td:nth-child(3)");
  R.check("EN: rodzaj operacji w historii przetłumaczony", kinds.every((k) => k === "copy →"), JSON.stringify(kinds));

  // --- kosz
  await page.click("nav button[data-tab=trash]"); await page.waitForTimeout(500);
  const tp = await txt("#trashTbl tbody tr td:first-child");
  R.check("EN: pliki w koszu bez zmian", FILES.every((f) => tp.includes(f)), JSON.stringify(tp));
  const opts = await txt("#trashJob option");
  R.check("EN: nazwy zadań w liście kosza bez zmian", same(opts, NAMES), JSON.stringify(opts));

  // --- konta
  await page.click("nav button[data-tab=accounts]"); await page.waitForTimeout(400);
  const acc = await txt("#remotes .grow > b");
  R.check("EN: nazwy kont chmurowych bez zmian", same(acc, ["Zadanie:", "Plik:"]), JSON.stringify(acc));
  const delBtns = await txt("#remotes button.danger");
  R.check("EN: przyciski kont przetłumaczone (Delete)", delBtns.every((x) => x === "Delete"), JSON.stringify(delBtns));

  // --- stały interfejs przetłumaczony
  await page.click("nav button[data-tab=jobs]"); await page.waitForTimeout(200);
  const nav = await txt("nav button");
  R.check("EN: nawigacja przetłumaczona", nav.some((x) => x.includes("Jobs")) && nav.some((x) => x.includes("Settings")), JSON.stringify(nav));
  const acts = await txt(".card:first-child .actions button");
  R.check("EN: przyciski karty przetłumaczone", acts.includes("Run") && acts.includes("Simulate") && acts.includes("Edit") && acts.includes("Delete"), JSON.stringify(acts));

  // --- brak polskich znaków w stałym UI (poza danymi), każda zakładka
  const dia = {};
  for (const tab of ["jobs", "activity", "trash", "accounts", "settings"]) {
    await page.click(`nav button[data-tab=${tab}]`); await page.waitForTimeout(300);
    dia[tab] = await page.evaluate(() => {
      const DATA = "[data-nt], textarea, input, .card .name, .loc .t span, .treerow span, #hist td:nth-child(2), #hist td:nth-child(4), #pending td:nth-child(2), #pending td[colspan='3'], #trashTbl td:first-child, #remotes .grow > b, option:not([value='']), .cur, .tag, .msg";
      const out = [];
      const w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      for (let n = w.nextNode(); n; n = w.nextNode()) {
        const el = n.parentElement; if (!el || el.closest("script,style") || el.closest(DATA)) continue;
        if (el.offsetParent === null && !el.closest("dialog[open]")) continue;
        if (/[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/.test(n.nodeValue)) out.push(n.nodeValue.trim());
      }
      return out;
    });
    R.check(`EN: brak polskich znaków w stałym UI (zakładka ${tab})`, dia[tab].length === 0, JSON.stringify(dia[tab]));
  }
  // placeholdery i title w stałym UI
  const attrs = await page.evaluate(() => [...document.querySelectorAll("[placeholder],[title]")].filter((e) => !e.closest(".card .loc, [data-nt]")).flatMap((e) => [e.getAttribute("placeholder"), e.getAttribute("title")]).filter((v) => v && /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/.test(v)));
  R.check("EN: placeholdery/title bez polskich znaków", attrs.length === 0, JSON.stringify(attrs));

  // --- kreator: stałe teksty kroków
  await page.click("nav button[data-tab=jobs]");
  await page.locator("[data-act=edit]").first().click(); await page.waitForTimeout(150);
  const bad = [];
  for (let i = 0; i < 5; i++) {
    const t = await page.locator("#editor").innerText();
    const lines = t.split("\n").filter((l) => PL_DIA.test(l) && !/\\\\/.test(l));
    if (lines.length) bad.push(...lines.map((l) => `${i}: ${l}`));
    if (i < 4) await page.click("#edNext");
  }
  R.check("EN: kreator (5 kroków) bez polskich znaków w stałym tekście", bad.length === 0, JSON.stringify(bad.slice(0, 5)));
  const summary = await page.locator("#summary").innerText();
  R.check("EN: podsumowanie kreatora bez 'bez limitu KB/s'", !/bez limitu/.test(summary), summary.replace(/\n/g, " | ").slice(0, 300));
  await page.keyboard.press("Escape");
  R.check("EN: brak błędów JS", errors.length === 0, errors.join("|"));
  await browser.close();

  // --- $& w tłumaczonym tekście: dane z '$&' nie są rozwijane (np. toast "Uruchomiono: <nazwa>")
  {
    const nm = "Zadanie $& $' x";
    const L = await launch({ lang: "en", setup: (a) => { window.__stub.jobs = a.jobs; window.__stub.status = {}; }, setupArg: { jobs: [mkJob(1, { name: nm })] } });
    await L.page.waitForTimeout(500);
    await L.page.locator("[data-act=run]").first().click(); await L.page.waitForTimeout(300);
    const toast = await L.page.$$eval(".toast", (e) => e.map((x) => x.textContent));
    R.check("EN: toast z nazwą zadania zawierającą $& nie jest zniekształcony", toast.some((t) => t === "Started: " + nm), JSON.stringify(toast));
    await L.browser.close();
  }
  R.done();
})();
