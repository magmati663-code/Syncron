// Runda 3: czy przyciski stopki kreatora (Wstecz/Dalej/Zapisz) są widoczne w oknie o rozmiarach aplikacji (domyślne 1040x700, minimalne 860x560) dla każdego kroku.
// Uruchom: node tests/ui-qa/r3_dialog_fit.js     (FAIL = potwierdzony błąd)
const { launch, reporter } = require("./lib");
const path = require("path");
const os = require("os");
const R = reporter("r3_dialog_fit");
(async () => {
  for (const [w, h] of [[1040, 700], [860, 560], [860, 480], [1000, 620], [1280, 800], [1920, 1080]]) {
    const { browser, page } = await launch({ viewport: { width: w, height: h }, setup: () => { window.__stub.jobs = []; } });
    await page.click("#newJob"); await page.fill("[name=left]", "C:\\A"); await page.fill("[name=right]", "D:\\B");
    const bad = [];
    for (let s = 0; s < 5; s++) {
      const r = await page.evaluate(() => {
        const d = document.getElementById("editor").getBoundingClientRect();
        const btn = [...document.querySelectorAll(".df button")].filter((b) => b.offsetParent !== null).map((b) => ({ id: b.id, top: Math.round(b.getBoundingClientRect().top), bottom: Math.round(b.getBoundingClientRect().bottom) }));
        const el = document.elementFromPoint(btn.length ? (document.querySelector("#" + btn[btn.length - 1].id).getBoundingClientRect().left + 5) : 0, btn.length ? btn[btn.length - 1].top + 5 : 0);
        return { dlgTop: Math.round(d.top), dlgBottom: Math.round(d.bottom), vh: innerHeight, btn, hit: el && (el.id || el.tagName), formH: Math.round(document.getElementById("jobForm").getBoundingClientRect().height) };
      });
      const lastBtn = r.btn[r.btn.length - 1];
      const visible = lastBtn && lastBtn.bottom <= r.dlgBottom && lastBtn.bottom <= r.vh && (r.hit === lastBtn.id || r.hit === "SPAN");
      if (!visible) bad.push({ krok: s + 1, ...r });
      if (s < 4) await page.click("#edNext");
    }
    R.check(`[${w}x${h}] stopka kreatora (Dalej/Zapisz) widoczna i klikalna na wszystkich 5 krokach`, bad.length === 0, JSON.stringify(bad.map((b) => ({ krok: b.krok, dialog: [b.dlgTop, b.dlgBottom], przyciski: b.btn, trafiony: b.hit, wysokoscFormularza: b.formH, vh: b.vh }))));
    if (h === 700) await page.screenshot({ path: path.join(os.tmpdir(), "syncron-dialog-fit-step5.png") });
    await browser.close();
  }
  R.done();
})();
