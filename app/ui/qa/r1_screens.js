// Runda 1: przejście wszystkich ekranów w PL i EN na atrapie backendu; błędy konsoli; w EN wykrycie nieprzetłumaczonych tekstów.
// Uruchom: node app/ui/qa/r1_screens.js
const { launch, mkJob, mkStatus, reporter } = require("./lib");
const R = reporter("r1_screens");

function seed(arg) {
  const S = window.__stub, now = Math.floor(Date.now() / 1000);
  const J = arg.jobs;
  S.jobs = J;
  const st = (o) => Object.assign({}, arg.st, o);
  S.status = {
    "job-1": st({}),
    "job-2": st({ state: "running", progress: { done: 3, total: 10, current: "a/b.txt" } }),
    "job-3": st({ state: "error", error: "Brak dostępu do folderu" }),
    "job-4": st({ state: "blocked", last: Object.assign({}, arg.st.last, { blocked: "plan usuwa ponad 25% plików" }) }),
    "job-5": st({ state: "conditions", note: "Czeka na warunki: komputer działa na baterii (zadanie wymaga zasilania sieciowego)" }),
    "job-6": st({ state: "waiting", last: Object.assign({}, arg.st.last, { busy: ["x/zajety.docx"], conflicts: ["k/konflikt.txt"], errors: ["err.txt: odmowa dostępu"], conflict_details: [{ path: "k/konflikt.txt", left: { size: 10, mtime: now - 100 }, right: { size: 2000, mtime: now - 5 } }] }) }),
  };
  S.history = [
    { ts: now - 10, job: "job-1", kind: "kopiowanie →", path: "a/Zażółć gęślą.txt", detail: "12 KB" },
    { ts: now - 20, job: "job-1", kind: "usunięcie", path: "b.txt", detail: "" },
    { ts: now - 30, job: "job-6", kind: "konflikt", path: "k/konflikt.txt", detail: "wymaga decyzji" },
    { ts: now - 40, job: "job-3", kind: "błąd", path: "e.txt", detail: "odmowa dostępu" },
    { ts: now - 50, job: "job-6", kind: "zajęty", path: "x/zajety.docx", detail: "plik zajęty" },
    { ts: now - 60, job: "job-1", kind: "przeniesienie", path: "n.txt", detail: "z: m.txt" },
  ];
  S.trash = [
    { side: "left", stamp: "2025-01-02_030405", path: "doc/a.txt", size: 100 },
    { side: "left", stamp: "2025-01-03_030405", path: "doc/a.txt", size: 2048 },
    { side: "right", stamp: "2025-01-03_030405", path: "b.bin", size: 5 * 1048576 },
  ];
  S.remotes = [{ name: "gdrive", kind: "drive" }, { name: "nc", kind: "webdav" }];
}

async function walk(lang) {
  const J = [1, 2, 3, 4, 5, 6, 7].map((i) => mkJob(i));
  J[0].schedule = { type: "Daily", hour: 2, minute: 5 }; J[0].no_delete_left = true; J[0].only_add = true; J[0].verify = true; J[0].exclude = ["*.log"];
  J[0].conditions = { ac_only: true, wifi: ["Dom", "Biuro"] }; J[0].bandwidth = { kbps: 500, off_kbps: 0, hours: [8, 18] };
  J[1].schedule = { type: "Interval", secs: 900 }; J[2].schedule = { type: "Manual" }; J[2].mode = "LeftToRight"; J[3].mode = "RightToLeft"; J[3].trash_days = 0;
  J[6].enabled = false; J[4].left = "gdrive:Dokumenty"; J[5].right = "\\\\serwer\\udział";
  const { browser, page, errors, calls } = await launch({ lang, setup: seed, setupArg: { jobs: J, st: mkStatus() } });
  const tag = lang.toUpperCase();
  await page.waitForTimeout(2300); // polling
  const cards = await page.locator(".card").count();
  R.check(`[${tag}] 7 kart zadań`, cards === 7, "kart: " + cards);
  const pills = await page.$$eval(".card .pill", (e) => e.map((x) => x.textContent.trim()));
  console.log(tag, "pills:", pills.join(" | "));
  R.check(`[${tag}] brak "undefined" w pigułkach stanu`, !pills.some((p) => /undefined/.test(p)));
  const texts = {};
  const grab = async (name) => { texts[name] = await page.evaluate(() => document.querySelector(".tab.active").innerText); };
  await grab("jobs");
  for (const t of ["activity", "trash", "accounts", "settings"]) {
    await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(250); await grab(t);
  }
  // symulacja
  await page.click("nav button[data-tab=jobs]");
  await page.locator('.card button[data-act=dry]').first().click(); await page.waitForTimeout(200);
  texts.report = await page.locator("#report").innerText();
  R.check(`[${tag}] okno symulacji otwarte`, await page.locator("#report[open]").count() === 1);
  await page.click("#repClose");
  // kreator: wszystkie 5 kroków
  await page.locator('.card button[data-act=edit]').first().click(); await page.waitForTimeout(100);
  for (let i = 0; i < 5; i++) { texts["wiz" + i] = await page.locator("#editor").innerText(); if (i < 4) await page.click("#edNext"); }
  await page.keyboard.press("Escape");
  // potwierdzenie
  await page.locator('.card button[data-act=del]').first().click(); await page.waitForTimeout(100);
  texts.confirm = await page.locator("#confirm").innerText(); await page.click("#cfNo");
  // diagnostyka (fallback bez schowka)
  await page.click("nav button[data-tab=settings]"); await page.click("#diagCopy"); await page.waitForTimeout(200);
  texts.diag = await page.evaluate(() => document.querySelector("#report").innerText + "|" + (document.querySelector("#repBody textarea")?.value || ""));
  if (await page.locator("#report[open]").count()) await page.click("#repClose");
  R.check(`[${tag}] brak błędów konsoli/JS`, errors.length === 0, errors.join(" || "));
  await browser.close();
  return texts;
}

(async () => {
  const pl = await walk("pl");
  const en = await walk("en");
  // w EN: linie zawierające polskie znaki diakrytyczne lub typowe polskie słowa = nieprzetłumaczone
  const PL = /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]|\b(kopiowanie|usunięcie|konflikt|zajęty|błąd|przeniesienie|plik|folder niedostępny)\b/;
  const untranslated = new Set();
  for (const [k, v] of Object.entries(en)) for (const line of v.split("\n")) if (PL.test(line)) untranslated.add(`${k}: ${line.trim()}`);
  console.log("\nNieprzetłumaczone linie w EN (do oceny; dane użytkownika/backendu są celowo zostawione):");
  [...untranslated].forEach((l) => console.log("  - " + l));
  R.check("[EN] brak nieprzetłumaczonych tekstów interfejsu (poza danymi)", untranslated.size === 0, untranslated.size + " linii");
  R.done();
})();
