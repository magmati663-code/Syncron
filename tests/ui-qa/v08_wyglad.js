// v0.8, zespół F: przegląd wizualny — asercje programowe.
// Sprawdza w motywie jasnym, ciemnym i wysokim kontraście: brak poziomych przepełnień (3 rozmiary okna x skale),
// kontrast tekstu >= 4.5:1 (duży tekst >= 3:1) dla głównych elementów każdej zakładki i okien, rozmiary celów dotykowych (>= 24 px),
// brak tekstów spoza i18n (EN bez polskich znaków diakrytycznych), brak słów „SyncPolak”/„rclone” w widocznym UI.
// Uruchom: node tests/ui-qa/v08_wyglad.js     (FAIL = potwierdzony błąd)
const fs = require("fs");
const path = require("path");
const { launch, reporter, mkJob, mkStatus, UI } = require("./lib");
const R = reporter("v08_wyglad");

function seed(arg) {
  const S = window.__stub, now = Math.floor(Date.now() / 1000), L = arg.st.last;
  S.jobs = arg.jobs;
  const st = (o) => Object.assign({}, arg.st, o);
  S.status = {
    "job-1": st({}),
    "job-2": st({ state: "running", progress: { done: 3, total: 10, current: "Dokumenty/raport.docx" } }),
    "job-3": st({ state: "error", error: "Brak dostępu do folderu" }),
    "job-4": st({ state: "blocked", last: Object.assign({}, L, { blocked: "plan usuwa ponad 25% plików" }) }),
    "job-5": st({ state: "conditions", note: "komputer działa na baterii (zadanie wymaga zasilania sieciowego)" }),
    "job-6": st({ state: "waiting", last: Object.assign({}, L, { busy: ["x/zajety.docx"], conflicts: ["k/konflikt.txt"], errors: ["err.txt: odmowa dostępu"], conflict_details: [{ path: "k/konflikt.txt", left: { size: 10, mtime: now - 100 }, right: { size: 20, mtime: now - 50 } }] }) }),
    "job-7": st({ state: "disabled" }),
  };
  S.history = Array.from({ length: 6 }, (_, i) => ({ ts: now - i * 70, job: "job-1", kind: ["kopiowanie →", "usunięcie", "konflikt", "błąd", "zajęty", "przeniesienie"][i], path: "a/plik" + i + ".txt", detail: "12 KB" }));
  S.trash = [{ side: "left", stamp: "2025-01-02_030405", path: "doc/a.txt", size: 100 }, { side: "right", stamp: "2025-01-03_030405", path: "b.bin", size: 5 * 1048576 }];
  S.remotes = [{ name: "serwer", kind: "ftp", needs_login: false, params: { host: "ftp.example.com" } }, { name: "biuro", kind: "sftp", needs_login: true, params: { host: "ssh.example.pl" } },
    { name: "nc", kind: "webdav", needs_login: false, params: { url: "https://nc.example.com/dav/" } }, { name: "kopie", kind: "s3", needs_login: false, params: { bucket: "kopie" } }, { name: "tls", kind: "ftps", needs_login: false, params: { host: "tls.example.com" } }];
  S.tree = { "": [{ name: "Dokumenty", is_dir: true }] };
  S.settings = Object.assign({ notify: true, language: "pl", unavailable_minutes: 30, ui_scale: arg.scale, font_scale: 1, density: "normal", sidebar: "auto", reduce_motion: false, high_contrast: arg.hc, theme: "auto" }, {});
  if (arg.extra) { if (arg.extra.delay) Object.assign(S.delay, arg.extra.delay); if (arg.extra.rcOffer) S.rcOffer = arg.extra.rcOffer; }
}
const mkJobs = () => {
  const J = [1, 2, 3, 4, 5, 6, 7].map((i) => mkJob(i));
  J[0].name = "Bardzo długa nazwa zadania synchronizacji dokumentów firmowych"; J[0].left = "C:\\Users\\Ania\\Dokumenty\\Projekty\\2025\\Bardzo długa ścieżka do katalogu z danymi\\Podfolder";
  J[2].mode = "LeftToRight"; J[3].mode = "RightToLeft"; J[6].enabled = false; J[4].left = "serwer:Dokumenty"; J[5].right = "\\\\nas\\udział";
  return J;
};
const open = (o) => launch(Object.assign({ setup: seed }, o));
const args = (scale, hc, extra) => ({ jobs: mkJobs(), st: mkStatus(), scale, hc: !!hc, extra });

// Kontrast: liczony w stronie. Zwraca listę naruszeń dla widocznych elementów pasujących do selektorów.
const CONTRAST_FN = (sels) => {
  const parse = (c) => {
    let m = /^rgba?\(([^)]+)\)$/.exec(c);
    if (m) { const p = m[1].split(/[ ,/]+/).filter(Boolean).map(parseFloat); return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1]; }
    m = /^color\(srgb ([^)]+)\)$/.exec(c);
    if (m) { const p = m[1].split(/[ /]+/).filter(Boolean).map(parseFloat); return [p[0] * 255, p[1] * 255, p[2] * 255, p.length > 3 ? p[3] : 1]; }
    return null;
  };
  const over = (f, b) => [0, 1, 2].map((i) => f[i] * f[3] + b[i] * (1 - f[3])).concat([1]);
  const lum = (c) => { const [r, g, b] = c.slice(0, 3).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4); }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  const bgOf = (el) => {
    const stack = [];
    for (let e = el; e; e = e.parentElement) {
      const cs = getComputedStyle(e);
      if (cs.backgroundImage && cs.backgroundImage !== "none") {
        // gradient: najgorszy przypadek spośród przystanków
        const cols = (cs.backgroundImage.match(/rgba?\([^)]+\)|color\(srgb [^)]+\)/g) || []).map(parse).filter(Boolean);
        if (cols.length) return { stops: cols, under: stack };
      }
      const c = parse(cs.backgroundColor);
      if (c && c[3] > 0) { stack.push(c); if (c[3] >= 1) break; }
    }
    return { stops: null, under: stack };
  };
  const flat = (under) => { let b = [255, 255, 255, 1]; for (let i = under.length - 1; i >= 0; i--) b = over(under[i], b); return b; };
  const out = [];
  for (const sel of sels) {
    for (const el of document.querySelectorAll(sel)) {
      const r = el.getBoundingClientRect(); if (!r.width || !r.height) continue;
      const cs = getComputedStyle(el); if (cs.visibility === "hidden" || cs.display === "none") continue;
      let op = 1; for (let e = el; e; e = e.parentElement) op *= parseFloat(getComputedStyle(e).opacity);
      if (op < 0.99) continue; // elementy wyszarzone (disabled) pomijamy
      if (el.disabled) continue;
      if (![...el.childNodes].some((n) => n.nodeType === 3 && n.nodeValue.trim()) && !["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName)) continue;
      const fg = parse(cs.color); if (!fg) continue;
      const { stops, under } = bgOf(el);
      const base = flat(under);
      const bgs = stops ? stops.map((s) => over(s, base)) : [base];
      const fs = parseFloat(cs.fontSize), bold = parseInt(cs.fontWeight, 10) >= 700, large = fs >= 24 || (fs >= 18.66 && bold);
      let worst = 99;
      for (const b of bgs) worst = Math.min(worst, ratio(over(fg, b), b));
      const need = large ? 3 : 4.5;
      if (worst < need) out.push(`${sel} «${(el.textContent || el.value || "").trim().slice(0, 28)}» ${worst.toFixed(2)}<${need}`);
    }
  }
  return out;
};
const CONTRAST_SELS = {
  jobs: ["h1", ".sub", ".card .name", ".pill", ".tag", ".meta", ".msg", ".loc .t span", ".loc .t b", ".tile .k", ".tile .v", "button", ".cur", ".empty h2", ".tpl b", ".tpl span", ".skel"],
  activity: ["h3", "th", "td", ".kind", "button", "input", "select", ".muted"],
  trash: ["th", "td", "button", ".muted", "input", "select"],
  accounts: ["h1", ".sub", ".remote b", ".kchip", ".kind", ".remote .rh", ".prov b", ".prov-h", "button", ".muted", "label"],
  settings: ["b", "label", ".opt b", ".opt span", ".sw span", ".sw small", ".muted", "button", "output"],
};
const NAV_SELS = ["nav button", ".badge", ".ver", "#pauseAll"];

const TABS = ["jobs", "activity", "trash", "accounts", "settings"];
const THEMES = [["light", false], ["dark", false], ["light", true], ["dark", true]];
const SIZES = [[1366, 768, 1], [1040, 700, 1], [860, 560, 1.5], [860, 560, 1], [1366, 768, 1.5]];

(async () => {
  // ---------- 1. przepełnienia poziome (każda zakładka, okna dialogowe) ----------
  for (const [w, h, sc] of SIZES) for (const theme of ["light", "dark"]) {
    const tag = `[${w}x${h} @${Math.round(sc * 100)}% ${theme}]`;
    const { browser, page, errors } = await open({ theme, viewport: { width: w, height: h }, setupArg: args(sc) });
    await page.waitForTimeout(250);
    const bad = [];
    const ov = () => page.evaluate(() => { const m = document.querySelector("main"); return [m.scrollWidth - m.clientWidth, document.documentElement.scrollWidth - innerWidth]; });
    for (const t of TABS) { await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(60); const [a, b] = await ov(); if (a > 1 || b > 2) bad.push(`${t}:${a}/${b}`); }
    await page.click("nav button[data-tab=accounts]"); await page.click("#providers .prov >> nth=1"); await page.waitForTimeout(80);
    { const [a, b] = await ov(); if (a > 1 || b > 2) bad.push(`accounts-form:${a}/${b}`); }
    // okna dialogowe nie wychodzą poza okno
    const dlgs = [];
    await page.click("nav button[data-tab=jobs]");
    await page.locator(".card button[data-act=edit]").first().click(); await page.waitForTimeout(80);
    for (let i = 0; i < 5; i++) { dlgs.push(await page.evaluate(() => { const r = document.querySelector("#editor").getBoundingClientRect(); return [r.left, r.right, r.bottom, innerWidth, innerHeight, document.querySelector("#edNext").offsetParent ? 1 : 0]; })); if (i < 4) await page.click("#edNext"); }
    await page.keyboard.press("Escape");
    dlgs.forEach((d, i) => { if (d[0] < -1 || d[1] > d[3] + 1 || d[2] > d[4] + 1) bad.push(`kreator krok ${i + 1} poza oknem ${d.map((x) => Math.round(x))}`); });
    await page.locator(".card button[data-act=dry]").first().click(); await page.waitForTimeout(200);
    { const [a] = await page.evaluate(() => { const b = document.querySelector("#repBody"); return [b.scrollWidth - b.clientWidth]; }); if (a > 1) bad.push("symulacja:" + a); }
    R.check(`${tag} brak przepełnień poziomych (zakładki, formularz konta, kreator, symulacja)`, bad.length === 0, bad.join("; "));
    R.check(`${tag} brak błędów JS`, errors.length === 0, errors.join("|"));
    await browser.close();
  }

  // ---------- 2. kontrast (jasny, ciemny, wysoki kontrast) ----------
  for (const [theme, hc] of THEMES) {
    const tag = `[kontrast ${theme}${hc ? " + wysoki kontrast" : ""}]`;
    const { browser, page } = await open({ theme, viewport: { width: 1366, height: 800 }, setupArg: args(1, hc) });
    await page.waitForTimeout(300);
    for (const t of TABS) {
      await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(120);
      if (t === "accounts") { await page.click("#providers .prov >> nth=0"); await page.click("#rAdd"); await page.waitForTimeout(60); }
      const viol = await page.evaluate(CONTRAST_FN, [...CONTRAST_SELS[t], ...NAV_SELS].map((s) => (t === "jobs" ? s : s)));
      R.check(`${tag} zakładka ${t}: tekst >= 4.5:1`, viol.length === 0, [...new Set(viol)].slice(0, 6).join(" | "));
    }
    // okna dialogowe
    await page.click("nav button[data-tab=jobs]");
    await page.locator(".card button[data-act=dry]").first().click(); await page.waitForTimeout(200);
    let v = await page.evaluate(CONTRAST_FN, ["#report h2", "#report button", "#report th", "#report td", "#report .muted", "#report .kind", "#report p"]);
    R.check(`${tag} okno symulacji: tekst >= 4.5:1`, v.length === 0, [...new Set(v)].slice(0, 6).join(" | "));
    await page.click("#repClose");
    await page.locator(".card button[data-act=del]").first().click(); await page.waitForTimeout(120);
    v = await page.evaluate(CONTRAST_FN, ["#confirm h2", "#confirm button", "#confirm .db"]);
    R.check(`${tag} okno potwierdzenia (przycisk Usuń: biały na czerwonym)`, v.length === 0, v.join(" | "));
    await page.click("#cfNo");
    await page.locator(".card button[data-act=edit]").first().click(); await page.waitForTimeout(100);
    const wv = [];
    for (let i = 0; i < 5; i++) { wv.push(...(await page.evaluate(CONTRAST_FN, ["#editor h2", "#editor label", "#editor .opt b", "#editor .opt span", "#editor .sw span", "#editor .sw small", "#editor .muted", "#editor button", "#editor .stepname", "#editor .msg", "#editor .summary", "#editor input", "#editor select", "#editor textarea"]))); if (i < 4) await page.click("#edNext"); }
    R.check(`${tag} kreator (5 kroków): tekst >= 4.5:1`, wv.length === 0, [...new Set(wv)].slice(0, 6).join(" | "));
    await browser.close();
  }

  // ---------- 3. cele dotykowe / kliknięcia (>= 24 px, WCAG 2.2 SC 2.5.8) ----------
  {
    const { browser, page } = await open({ viewport: { width: 1366, height: 800 }, setupArg: args(1) });
    await page.waitForTimeout(250);
    const small = [];
    for (const t of TABS) {
      await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(100);
      if (t === "accounts") { await page.click("#providers .prov >> nth=1"); await page.waitForTimeout(60); }
      const r = await page.evaluate(() => {
        const out = [], z = parseFloat(getComputedStyle(document.documentElement).zoom) || 1;
        for (const el of document.querySelectorAll("button, select, input:not([type=range]):not([type=radio]):not([type=checkbox]), textarea, label.opt, label.sw")) {
          const b = el.getBoundingClientRect(); if (!b.width || !b.height || el.closest("[hidden]")) continue;
          if (b.width * z < 24 || b.height * z < 24) out.push((el.id || el.className || el.tagName) + " " + Math.round(b.width * z) + "x" + Math.round(b.height * z));
        }
        return out;
      });
      small.push(...r.map((x) => t + ":" + x));
    }
    R.check("cele dotykowe/kliknięcia >= 24x24 px (wszystkie zakładki)", small.length === 0, small.slice(0, 6).join(" | "));
    await browser.close();
  }

  // ---------- 4. brak tekstów spoza i18n (EN) i zakazane słowa ----------
  const FORBID = /SyncPolak|rclone(?!\.conf)|chmur|cloud/i;
  const DIAC = /[ąćęłńóśźżĄĆĘŁŃÓŚŹŻ]/;
  for (const lang of ["pl", "en"]) {
    const { browser, page } = await open({ lang, viewport: { width: 1366, height: 800 }, setupArg: args(1, false, { rcOffer: "/home/u/.config/syncron/rclone.conf" }) });
    await page.waitForTimeout(400);
    const texts = [];
    const grab = async (label) => { texts.push([label, await page.evaluate(() => {
      const out = [], w = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
      for (let n = w.nextNode(); n; n = w.nextNode()) {
        const el = n.parentElement; if (!el || !n.nodeValue.trim() || ["SCRIPT", "STYLE"].includes(el.tagName)) continue;
        if (el.closest("[data-nt], .card .name, .loc .t span, #hist td:nth-child(2), #hist td:nth-child(4), #remotes .grow b, .cur, option:not([value=''])")) continue;
        const r = el.getBoundingClientRect(); if (!r.width || !r.height) continue;
        out.push(n.nodeValue.trim());
      }
      for (const el of document.querySelectorAll("[placeholder],[title],[aria-label]")) { const r = el.getBoundingClientRect(); if (!r.width) continue; if (el.closest("[data-nt]")) continue; for (const a of ["placeholder", "title", "aria-label"]) { const v = el.getAttribute(a); if (v) out.push(v); } }
      return out;
    })]); };
    // okno z propozycją importu otwiera się przy starcie (stub rcOffer) — zrzut tekstu, potem „Nie teraz”
    R.check(`[${lang}] propozycja importu kont przy starcie (okno potwierdzenia)`, (await page.locator("#confirm[open]").count()) === 1);
    await grab("import-offer");
    await page.click("#cfNo"); await page.waitForTimeout(100);
    R.check(`[${lang}] „Nie teraz” wywołuje rclone_import_dismiss`, (await page.evaluate(() => window.__stub.calls.some((c) => c.cmd === "rclone_import_dismiss"))));
    for (const t of TABS) { await page.click(`nav button[data-tab=${t}]`); await page.waitForTimeout(150); if (t === "accounts") { await page.click("#providers .prov >> nth=1"); } await grab(t); }
    await page.click("nav button[data-tab=accounts]"); await page.click("#rcImport"); await page.waitForTimeout(150);
    await page.evaluate(() => { window.__stub.dialogOpen = null; });
    await page.click("nav button[data-tab=jobs]");
    await page.locator(".card button[data-act=dry]").first().click(); await page.waitForTimeout(200); await grab("symulacja"); await page.click("#repClose");
    await page.locator(".card button[data-act=del]").first().click(); await page.waitForTimeout(100); await grab("potwierdzenie"); await page.click("#cfNo");
    await page.click("#ffsImport"); await page.waitForTimeout(150); await grab("ffs"); await page.keyboard.press("Escape");
    await page.locator(".card button[data-act=edit]").first().click(); await page.waitForTimeout(100);
    for (let i = 0; i < 5; i++) { await grab("krok" + (i + 1)); if (i < 4) await page.click("#edNext"); }
    for (const [label, list] of texts) {
      const bad = list.filter((x) => FORBID.test(x));
      R.check(`[${lang}] ${label}: brak „SyncPolak”/„rclone”/„chmura” w widocznym UI`, bad.length === 0, bad.slice(0, 3).join(" | "));
      if (lang === "en") { const pl = list.filter((x) => DIAC.test(x) && !/Bardzo długa nazwa/.test(x)); R.check(`[en] ${label}: brak nieprzetłumaczonych tekstów`, pl.length === 0, pl.slice(0, 3).join(" | ")); }
    }
    await browser.close();
  }
  // statycznie: nazwa aplikacji w plikach UI
  const src = ["index.html", "app.js", "style.css", "scale.js"].map((f) => [f, fs.readFileSync(path.join(UI, f), "utf8")]);
  R.check("kod UI nie zawiera „SyncPolak”", src.every(([, t]) => !/SyncPolak/i.test(t)), src.filter(([, t]) => /SyncPolak/i.test(t)).map(([f]) => f).join());
  R.check("index.html i app.js nie zawierają słów „rclone” (poza importem rclone.conf) ani „chmur”", src.slice(0, 2).every(([, t]) => !/rclone(?!\.conf|_import|Import|_conf)/i.test(t.replace(/RcloneImport/g, "")) && !/chmur/i.test(t)), "");
  const html = src[0][1];
  R.check("tytuł strony i marka to „Syncron”", /<title>Syncron<\/title>/.test(html) && /<span>Syncron<\/span>/.test(html));

  // ---------- 5. stany: szkielet ładowania, pusty stan, błąd z podpowiedzią, kolor i ikona rodzaju konta ----------
  {
    const { browser, page } = await open({ viewport: { width: 1100, height: 760 }, setupArg: args(1, false, { delay: { list_jobs: 600 } }) });
    await page.waitForTimeout(150);
    R.check("szkielet ładowania listy zadań przed odpowiedzią backendu", (await page.locator("#jobs .skel .sk").count()) > 0 && (await page.getAttribute("#jobs .skel", "aria-busy")) === "true");
    await page.waitForTimeout(1200);
    R.check("szkielet znika po załadowaniu kart", (await page.locator("#jobs .skel").count()) === 0 && (await page.locator(".card").count()) === 7);
    const hint = await page.locator(".card.s-error .msg .todo").innerText();
    R.check("błąd zadania ma podpowiedź „Co zrobić”", /Co zrobić/.test(hint), hint);
    await page.click("nav button[data-tab=accounts]"); await page.waitForTimeout(250);
    const kinds = await page.$$eval("#remotes .remote", (e) => e.map((x) => [x.querySelector(".kchip").textContent, getComputedStyle(x.querySelector(".kic")).backgroundColor, !!x.querySelector(".kic svg")]));
    R.check("każdy rodzaj konta (FTP/SFTP/WebDAV/S3/FTPS) ma ikonę", kinds.length === 5 && kinds.every((k) => k[2]), JSON.stringify(kinds));
    R.check("każdy rodzaj konta ma własny kolor", new Set(kinds.map((k) => k[1])).size >= 4, JSON.stringify(kinds.map((k) => k[1])));
    R.check("konto z needs_login: ostrzeżenie „Wpisz hasło ponownie” i przycisk Edytuj", (await page.locator("#remotes .remote .kind.warn").count()) === 1 && (await page.locator("#remotes .remote [data-e]").count()) === 5);
    // edycja konta: formularz wypełniony, hasło puste, zapis wywołuje update_remote
    await page.evaluate(() => { window.__stub.kinds = null; });
    await page.click('#remotes [data-e="biuro"]'); await page.waitForTimeout(150);
    R.check("edycja konta: nazwa tylko do odczytu, wartości z params, hasło puste", (await page.getAttribute("#rName", "readonly")) !== null && (await page.inputValue("#rf_host")) === "ssh.example.pl" && (await page.inputValue("#rf_pass")) === "");
    await page.click("#rAdd"); await page.waitForTimeout(150);
    const upd = await page.evaluate(() => window.__stub.updated);
    R.check("edycja konta: update_remote z pustym hasłem (zostaje stare)", upd && upd.name === "biuro" && upd.params.host === "ssh.example.pl" && upd.params.pass === "", JSON.stringify(upd));
    // import z rclone.conf: wybór pliku, raport
    await page.evaluate(() => { window.__stub.dialogOpen = "/tmp/x/rclone.conf"; });
    await page.click("#rcImport"); await page.waitForTimeout(200);
    const rep = await page.locator("#repBody").innerText();
    R.check("import z rclone.conf: raport (utworzone, wymagające hasła, pominięte z powodem)", /Utworzone konta: 2/.test(rep) && /Pominięte: 1/.test(rep) && /nie jest już obsługiwany/.test(rep) && /Edytuj/.test(rep), rep.replace(/\s+/g, " ").slice(0, 160));
    await page.click("#repClose");
    await browser.close();
  }
  {
    const { browser, page } = await open({ viewport: { width: 1100, height: 760 }, setup: (a) => { window.__stub.settings.ui_scale = 1; } });
    await page.waitForTimeout(300);
    const t = await page.locator("#tab-jobs").innerText();
    R.check("pusty stan: wezwanie do działania (nowe zadanie, szablony, dodaj konto), bez kafelków podsumowania", /Nowe zadanie/.test(t) && /Dodaj konto zdalne/.test(t) && (await page.locator(".tpl").count()) === 3 && !(await page.locator("#tiles").isVisible()));
    await browser.close();
  }
  R.done();
})();
