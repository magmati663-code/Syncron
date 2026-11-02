// v0.8, zespół B: zakładka „Konta” (formularze z remote_kinds, test, przeglądarka folderów) i przycisk „Przeglądaj…” w kreatorze.
// Uruchom: node tests/ui-qa/v08_konta.js
const { launch, reporter } = require("./lib");
const R = reporter("v08_konta");

const KINDS = [
  { kind: "gdrive", label: "Google Drive", help: "Logowanie w przeglądarce.", oauth: true, fields: [{ key: "client_id", label: "Własny identyfikator klienta (Client ID)", kind: "text", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" }] },
  { kind: "sftp", label: "SFTP (SSH)", help: "x", oauth: false, fields: [
    { key: "host", label: "Serwer", kind: "text", required: true, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" },
    { key: "port", label: "Port", kind: "number", required: false, secret: false, default: "22", options: [], help: "", min: 1, max: 65535, warn: "" },
    { key: "pass", label: "Hasło", kind: "password", required: false, secret: true, default: "", options: [], help: "", min: null, max: null, warn: "" },
    { key: "key_file", label: "Plik klucza prywatnego", kind: "file", required: false, secret: false, default: "", options: [], help: "", min: null, max: null, warn: "" },
  ] },
  { kind: "ftps", label: "<img src=x onerror=window.__xss=1>", help: "x", oauth: false, fields: [
    { key: "tls", label: "Szyfrowanie (TLS)", kind: "select", required: false, secret: false, default: "explicit", options: [{ value: "none", label: "Brak" }, { value: "explicit", label: "Jawne" }], help: "", min: null, max: null, warn: "" },
    { key: "no_check_certificate", label: "Nie sprawdzaj certyfikatu serwera", kind: "bool", required: false, secret: false, default: "false", options: [], help: "", min: null, max: null, warn: "Ryzykowne: test" },
  ] },
];

function setup(kinds) {
  window.__KINDS = kinds;
  const S = window.__stub, orig = window.__TAURI__.core.invoke;
  S.remotes = [{ name: "gdrive", kind: "drive" }, { name: "bad", kind: "ftp" }, { name: "x<b>y", kind: "sftp" }];
  S.tree = { "": [{ name: "Dokumenty", is_dir: true }, { name: "<img src=x onerror=window.__xss=2>", is_dir: true }], "Dokumenty": [{ name: "Faktury", is_dir: true }] };
  S.testRes = { gdrive: { ok: true, ms: 123, message: "Połączono. Folderów na najwyższym poziomie: 2", items: 2, error: "", hint: "" },
    bad: { ok: false, ms: 77, message: "", items: 0, error: "login failed <i>x</i>", hint: "Logowanie nie powiodło się: nieprawidłowy login, hasło lub klucz." } };
  window.__TAURI__.core.invoke = async (cmd, args) => {
    if (cmd === "remote_kinds") { S.calls.push({ cmd, args: null }); return window.__KINDS; }
    if (cmd === "browse_remote") { S.calls.push({ cmd, args }); return S.tree[args.path] || []; }
    if (cmd === "test_remote" && S.testRes[args.name]) { S.calls.push({ cmd, args }); return S.testRes[args.name]; }
    if (cmd === "test_remote" && args.name === "x<b>y") { S.calls.push({ cmd, args }); throw "timeout"; }
    return orig(cmd, args);
  };
}

(async () => {
  const { browser, page, errors, calls } = await launch({ setup, setupArg: KINDS });
  await page.evaluate(() => { window.__stub.dialogOpen = "/home/u/.ssh/id_rsa"; });
  await page.click('[data-tab="accounts"]');
  const provs = await page.$$eval("#providers .prov", (e) => e.map((x) => x.dataset.p));
  R.check("kafelki usług z remote_kinds", provs.join() === "gdrive,sftp,ftps", provs.join());
  R.check("etykieta usługi nie wstrzykuje HTML (XSS)", !(await page.evaluate(() => window.__xss)) && (await page.locator("#providers .prov b").nth(2).innerText()).includes("<img"), "");

  await page.click('[data-p="sftp"]');
  R.check("formularz: pola z kontraktu", (await page.$$eval("#rFields [data-f]", (e) => e.map((x) => x.dataset.f))).join() === "host,port,pass,key_file");
  R.check("port ma wartość domyślną 22", (await page.inputValue("#rf_port")) === "22");
  await page.click("[data-eye=rf_pass]");
  R.check("oczko pokazuje hasło", (await page.getAttribute("#rf_pass", "type")) === "text");
  await page.click("[data-eye=rf_pass]");
  R.check("oczko ukrywa hasło", (await page.getAttribute("#rf_pass", "type")) === "password");
  await page.click("[data-file=rf_key_file]");
  R.check("przycisk pliku używa dialogApi", (await page.inputValue("#rf_key_file")) === "/home/u/.ssh/id_rsa");
  await page.fill("#rName", "serwer");
  await page.click("#rAdd");
  R.check("brak wymaganego pola: komunikat i brak add_remote", /Uzupełnij pole „Serwer”/.test(await page.innerText("#rMsg")) && !(await calls()).some((c) => c.cmd === "add_remote"));
  await page.fill("#rf_host", "sftp.example.com"); await page.fill("#rf_port", "70000");
  await page.click("#rAdd");
  R.check("port poza zakresem odrzucony", /poza zakresem/.test(await page.innerText("#rMsg")));
  await page.fill("#rf_port", "2222"); await page.fill("#rf_pass", " tajne ");
  await page.click("#rAdd"); await page.waitForTimeout(200);
  const add = (await calls()).find((c) => c.cmd === "add_remote");
  R.check("add_remote z parametrami (hasło bez przycinania)", add && add.args.kind === "sftp" && add.args.params.host === "sftp.example.com" && add.args.params.port === "2222" && add.args.params.pass === " tajne ", JSON.stringify(add));

  await page.click('[data-p="ftps"]');
  R.check("ostrzeżenie o certyfikacie ukryte domyślnie", await page.locator("#rf_no_check_certificate_w").isHidden());
  await page.check("#rf_no_check_certificate");
  R.check("ostrzeżenie widoczne po zaznaczeniu", await page.locator("#rf_no_check_certificate_w").isVisible());
  R.check("OAuth: przycisk logowania", (await page.evaluate(() => { document.querySelector('[data-p="gdrive"]').click(); return document.querySelector("#rAdd").textContent; })) === "Zaloguj przez przeglądarkę");

  // test połączenia
  await page.click('[data-t="gdrive"]'); await page.waitForTimeout(100);
  const t1 = await page.innerText('[data-rt="gdrive"]');
  R.check("test OK pokazuje czas", /Połączono/.test(t1) && /123 ms/.test(t1), t1);
  await page.click('[data-t="bad"]'); await page.waitForTimeout(100);
  const t2 = await page.innerText('[data-rt="bad"]');
  R.check("test błąd: podpowiedź + surowy błąd, bez HTML", /Błąd połączenia/.test(t2) && /Logowanie nie powiodło się/.test(t2) && /<i>x<\/i>/.test(t2) && (await page.locator('[data-rt="bad"] i').count()) === 0, t2);
  await page.locator('[data-t]').nth(2).click(); await page.waitForTimeout(100);
  R.check("test odrzucony: komunikat, brak zawieszenia", /Błąd połączenia/.test(await page.locator("[data-rt]").nth(2).innerText()));

  // przeglądarka z zakładki Konta
  await page.click('[data-b="gdrive"]'); await page.waitForTimeout(100);
  const names = await page.$$eval("#rbList [data-dir]", (e) => e.map((x) => x.textContent.trim()));
  R.check("przeglądarka listuje foldery (nazwy jako tekst)", names.length === 2 && names[1].includes("<img") && !(await page.evaluate(() => window.__xss)), names.join("|"));
  await page.click('#rbList [data-dir="Dokumenty"]'); await page.waitForTimeout(100);
  R.check("wejście do folderu: ścieżka i zapytanie", (await page.innerText("#rbPath")) === "gdrive:Dokumenty" && (await calls()).some((c) => c.cmd === "browse_remote" && c.args.path === "Dokumenty"));
  await page.click("#rbUp"); await page.waitForTimeout(100);
  R.check("W górę wraca do korzenia", (await page.innerText("#rbPath")) === "gdrive:");
  await page.click("#rbClose");

  // kreator
  await page.click('[data-tab="jobs"]'); await page.click("#newJob");
  const left = page.locator('input[name="left"]');
  R.check("przycisk Przeglądaj ukryty dla ścieżki lokalnej", await page.locator('[data-browse="left"]').isHidden());
  await left.fill("gdrive:"); await left.dispatchEvent("input");
  R.check("przycisk Przeglądaj widoczny dla konta", await page.locator('[data-browse="left"]').isVisible());
  await page.click('[data-browse="left"]'); await page.waitForTimeout(100);
  await page.click('#rbList [data-dir="Dokumenty"]'); await page.waitForTimeout(100);
  R.check("klik w folder wpisuje ścieżkę w kreatorze", (await left.inputValue()) === "gdrive:Dokumenty", await left.inputValue());
  await page.click("#rbClose");
  R.check("brak błędów JS (PL)", errors.length === 0, errors.join(" | "));
  await browser.close();

  // EN
  {
    const r = await launch({ lang: "en", setup, setupArg: KINDS });
    await r.page.click('[data-tab="accounts"]');
    await r.page.click('[data-p="sftp"]'); await r.page.waitForTimeout(200);
    const txt = await r.page.innerText("#rFields");
    R.check("EN: etykiety pól przetłumaczone", /Server/.test(txt) && /Password/.test(txt) && !/Serwer/.test(txt), txt.slice(0, 80));
    await r.page.click('[data-t="bad"]'); await r.page.waitForTimeout(100);
    R.check("EN: nazwa konta nietłumaczona", (await r.page.innerText("#remotes")).includes("x<b>y:"));
    R.check("brak błędów JS (EN)", r.errors.length === 0, r.errors.join(" | "));
    await r.browser.close();
  }
  const res = R.done();
  process.exit(res.some((x) => !x.ok) ? 1 : 0);
})().catch((e) => { console.error(e); process.exit(2); });
