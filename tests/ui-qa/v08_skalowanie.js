// v0.8 (zespół C): skalowanie interfejsu 75%..200% w oknach 860x480 .. 1920x1080.
// Sprawdza: stopka kreatora (5 kroków) widoczna i klikalna, brak poziomego przewijania, zwijanie paska bocznego,
// skróty Ctrl+/-/0 i Ctrl+kółko, zapis ustawień (stub set_settings), "Przywróć domyślne", tryby gęstości/kontrastu.
// Uruchom: node tests/ui-qa/v08_skalowanie.js     (FAIL = potwierdzony błąd)
const { launch, reporter, mkJob, mkStatus } = require("./lib");
const R = reporter("v08_skalowanie");

const SCALES = [0.75, 1, 1.5, 2];
const SIZES = [[860, 480], [1024, 600], [1366, 768], [1920, 1080]];
const TABS = ["jobs", "activity", "trash", "accounts", "settings"];

function startOpts(w, h, scale, extra) {
  return {
    viewport: { width: w, height: h },
    setup: (a) => {
      window.__stub.jobs = [Object.assign({}, a.job, { name: "Bardzo długa nazwa zadania ".repeat(6), left: "C:\\" + "Bardzo\\Długa\\Ścieżka\\".repeat(8), right: "D:\\Prawa" })];
      window.__stub.status = { [a.job.id]: a.status };
      window.__stub.settings = Object.assign({ notify: true, language: "pl", unavailable_minutes: 30, ui_scale: a.scale, font_scale: 1, density: "normal", sidebar: "auto", reduce_motion: false, high_contrast: false, theme: "auto" }, a.extra || {});
      window.__stub.history = Array.from({ length: 12 }, (_, i) => ({ ts: 1700000000 + i, job: a.job.id, kind: "copy", path: "C:\\bardzo\\dluga\\sciezka\\".repeat(6) + "plik" + i + ".txt", detail: "x".repeat(80) }));
    },
    setupArg: { scale, extra, job: mkJob(1), status: mkStatus() },
  };
}

(async () => {
  for (const scale of SCALES) for (const [w, h] of SIZES) {
    const tag = `[${Math.round(scale * 100)}% ${w}x${h}]`;
    const { browser, page, errors } = await launch(startOpts(w, h, scale));
    const applied = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--ui-scale").trim());
    R.check(`${tag} skala z ustawień zastosowana`, Math.abs(parseFloat(applied) - scale) < 1e-9, "--ui-scale=" + applied);

    // pasek boczny
    const sb = await page.evaluate(() => ({ cls: document.documentElement.classList.contains("sb-collapsed"), iw: innerWidth, aw: document.querySelector("aside").getBoundingClientRect().width, title: document.querySelector("nav button[data-tab=settings]").title, aria: document.querySelector("nav button[data-tab=settings]").getAttribute("aria-label") }));
    R.check(`${tag} pasek boczny ${w < 900 ? "zwinięty (ikony + tooltip)" : "rozwinięty"}`, sb.cls === (w < 900) && (!sb.cls || (sb.title && sb.aria)), JSON.stringify(sb));

    // brak przewijania poziomego na każdej zakładce
    const bad = [];
    for (const t of TABS) {
      await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(60);
      const r = await page.evaluate(() => { const m = document.querySelector("main"); return { mx: m.scrollWidth - m.clientWidth, dx: document.documentElement.scrollWidth - innerWidth, bx: document.body.scrollWidth * (+getComputedStyle(document.documentElement).getPropertyValue("--ui-scale")) - innerWidth }; });
      if (r.mx > 1 || r.dx > 1 || r.bx > 1) bad.push({ t, ...r });
    }
    R.check(`${tag} brak poziomego przewijania stron`, bad.length === 0, JSON.stringify(bad));

    // kreator
    await page.click("nav button[data-tab=jobs]");
    await page.click("#newJob"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    const badSteps = [];
    for (let s = 0; s < 5; s++) {
      const r = await page.evaluate(() => {
        const ed = document.getElementById("editor").getBoundingClientRect();
        const btns = [...document.querySelectorAll(".df button")].filter((b) => b.offsetParent !== null);
        const last = btns[btns.length - 1], lr = last.getBoundingClientRect();
        const hit = document.elementFromPoint(lr.left + lr.width / 2, lr.top + lr.height / 2);
        const all = btns.every((b) => { const x = b.getBoundingClientRect(); return x.top >= -0.5 && x.bottom <= innerHeight + 0.5 && x.left >= -0.5 && x.right <= innerWidth + 0.5; });
        return { id: last.id, hit: hit && (hit.closest("button") || {}).id, inView: all, edTop: Math.round(ed.top), edBottom: Math.round(ed.bottom), ih: innerHeight };
      });
      if (!(r.inView && r.hit === r.id && r.edTop >= -0.5 && r.edBottom <= r.ih + 0.5)) badSteps.push({ krok: s + 1, ...r });
      if (s < 4) await page.click("#edNext");
    }
    R.check(`${tag} stopka kreatora widoczna i klikalna (5 kroków)`, badSteps.length === 0, JSON.stringify(badSteps));
    await page.keyboard.press("Escape");
    R.check(`${tag} brak błędów JS`, errors.length === 0, errors.join(" | "));
    await browser.close();
  }

  // skróty i zapis ustawień
  {
    const { browser, page, errors, calls } = await launch(startOpts(1280, 800, 1));
    const getScale = () => page.evaluate(() => +getComputedStyle(document.documentElement).getPropertyValue("--ui-scale"));
    await page.keyboard.press("Control+=");
    R.check("Ctrl+= powiększa o 10%", Math.abs((await getScale()) - 1.1) < 1e-9, String(await getScale()));
    const hud = await page.evaluate(() => ({ t: document.getElementById("zoomHud").textContent, on: document.getElementById("zoomHud").classList.contains("on") }));
    R.check("wskaźnik skali pokazuje 110%", hud.t === "110%" && hud.on, JSON.stringify(hud));
    await page.keyboard.press("Control+-"); await page.keyboard.press("Control+-");
    R.check("Ctrl+- ×2 pomniejsza do 90%", Math.abs((await getScale()) - 0.9) < 1e-9, String(await getScale()));
    await page.mouse.move(400, 300);
    await page.keyboard.down("Control"); await page.mouse.wheel(0, -100); await page.keyboard.up("Control");
    R.check("Ctrl+kółko w górę powiększa o 5%", Math.abs((await getScale()) - 0.95) < 1e-9, String(await getScale()));
    for (let i = 0; i < 30; i++) await page.keyboard.press("Control+=");
    R.check("skala ograniczona do 200%", (await getScale()) === 2, String(await getScale()));
    for (let i = 0; i < 40; i++) await page.keyboard.press("Control+-");
    R.check("skala ograniczona do 75%", (await getScale()) === 0.75, String(await getScale()));
    await page.keyboard.press("Control+0");
    R.check("Ctrl+0 przywraca 100%", (await getScale()) === 1);
    await page.keyboard.press("Control+=");
    const before = (await calls()).filter((c) => c.cmd === "set_settings").length;
    await page.waitForTimeout(900);
    const sets = (await calls()).filter((c) => c.cmd === "set_settings");
    R.check("zmiany skali zapisane z debounce (jedno wywołanie, bez wywołania w trakcie serii)", sets.length >= 1 && before === 0 && sets.length <= 2, `przed=${before} po=${sets.length}`);
    const last = sets[sets.length - 1].args.settings;
    R.check("zapisane ui_scale=1.1 i zachowane pozostałe pola (notify, language, unavailable_minutes)", last.ui_scale === 1.1 && last.notify === true && last.language === "pl" && last.unavailable_minutes === 30, JSON.stringify(last));

    // sekcja w Ustawieniach
    await page.click("nav button[data-tab=settings]");
    R.check("suwak skali odzwierciedla 110%", (await page.inputValue("#apScale")) === "110");
    await page.fill("#apScale", "150"); await page.dispatchEvent("#apScale", "input");
    await page.fill("#apFont", "120"); await page.dispatchEvent("#apFont", "input");
    await page.check("#apContrast"); await page.check("#apMotion");
    await page.check("input[name=density][value=comfortable]", { force: true });
    await page.check("input[name=sidebar][value=collapsed]", { force: true });
    const live = await page.evaluate(() => { const r = document.documentElement; return { s: r.style.getPropertyValue("--ui-scale"), f: r.style.getPropertyValue("--fs"), c: r.dataset.contrast, m: r.dataset.motion, d: r.dataset.density, sb: r.classList.contains("sb-collapsed"), tap: getComputedStyle(document.querySelector("#newJob")).minHeight }; });
    R.check("podgląd na żywo: wszystkie ustawienia zastosowane natychmiast", live.s === "1.5" && live.f === "1.2" && live.c === "high" && live.m === "reduce" && live.d === "comfortable" && live.sb, JSON.stringify(live));
    await page.waitForTimeout(700);
    const s2 = (await calls()).filter((c) => c.cmd === "set_settings").pop().args.settings;
    R.check("ustawienia wyglądu zapisane w settings", s2.ui_scale === 1.5 && s2.font_scale === 1.2 && s2.density === "comfortable" && s2.sidebar === "collapsed" && s2.reduce_motion === true && s2.high_contrast === true, JSON.stringify(s2));
    const motion = await page.evaluate(() => getComputedStyle(document.querySelector(".tab.active")).animationDuration);
    R.check("ograniczenie animacji wyłącza animacje", parseFloat(motion) < 0.01, motion);
    await page.click("#apReset");
    await page.waitForTimeout(100);
    const s3 = (await calls()).filter((c) => c.cmd === "set_settings").pop().args.settings;
    R.check("Przywróć domyślne zapisuje wartości domyślne", s3.ui_scale === 1 && s3.font_scale === 1 && s3.density === "normal" && s3.sidebar === "auto" && !s3.reduce_motion && !s3.high_contrast && s3.notify === true, JSON.stringify(s3));
    R.check("brak błędów JS", errors.length === 0, errors.join(" | "));
    await browser.close();
  }

  // skala odtwarzana po ponownym uruchomieniu (z ustawień backendu) oraz niepoprawne wartości z pliku
  {
    const { browser, page } = await launch(startOpts(1280, 800, 1, { ui_scale: 9, font_scale: "x", density: "huge", sidebar: 3 }));
    const v = await page.evaluate(() => ({ s: getComputedStyle(document.documentElement).getPropertyValue("--ui-scale").trim(), d: document.documentElement.dataset.density }));
    R.check("niepoprawne wartości ustawień są ograniczane (200% / domyślne)", v.s === "2" && v.d === "normal", JSON.stringify(v));
    await browser.close();
  }

  // dotykowe cele >= 36 px (w px ekranu) w trybie wygodnym
  for (const scale of [0.75, 1, 2]) {
    const { browser, page } = await launch(startOpts(1280, 800, scale, { density: "comfortable" }));
    const r = await page.evaluate(() => [...document.querySelectorAll("#newJob,.actions button,nav button")].filter((b) => b.offsetParent).map((b) => b.getBoundingClientRect().height));
    R.check(`[${Math.round(scale * 100)}%] tryb wygodny: cele dotykowe >= 36 px`, r.length > 0 && r.every((x) => x >= 35.5), JSON.stringify(r.map(Math.round)));
    await browser.close();
  }

  // długie nazwy: elipsa i pełny tytuł po najechaniu
  {
    const { browser, page } = await launch(startOpts(860, 480, 1));
    await page.hover(".card .name");
    const r = await page.evaluate(() => { const n = document.querySelector(".card .name"); return { t: n.title.length, clipped: n.scrollWidth > n.clientWidth, ov: getComputedStyle(n).textOverflow }; });
    R.check("długa nazwa zadania: elipsa + pełny tytuł", r.clipped && r.ov === "ellipsis" && r.t > 40, JSON.stringify(r));
    await browser.close();
  }
  const res = R.done();
  process.exit(res.some((r) => !r.ok) ? 1 : 0);
})();
