// Runda 2: save_job/kreator nie odrzucają zagnieżdżonych lokalizacji (prawa wewnątrz lewej) - silnik kopiuje folder sam do siebie w nieskończoność.
// Wymaga zbudowanego CLI:  CARGO_TARGET_DIR=/tmp/qa_target cargo build -p syncron-cli
// Uruchom: SYNCRON_BIN=/tmp/qa_target/debug/syncron node app/ui/qa/r2_nested_locations.js      (FAIL = potwierdzony błąd)
const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");
const { reporter } = require("./lib");
const R = reporter("r2_nested_locations");
const BIN = process.env.SYNCRON_BIN || "/tmp/qa_target/debug/syncron";
if (!fs.existsSync(BIN)) { console.log("Brak CLI: " + BIN + " (zbuduj: CARGO_TARGET_DIR=/tmp/qa_target cargo build -p syncron-cli)"); process.exit(2); }
const root = fs.mkdtempSync(path.join(os.tmpdir(), "syncron-nest-"));
const a = path.join(root, "a"), data = path.join(root, "data");
fs.mkdirSync(path.join(a, "kopia"), { recursive: true }); fs.mkdirSync(data);
fs.writeFileSync(path.join(a, "f1.txt"), "hi");
const job = [{ id: "n1", name: "nest", left: a, right: path.join(a, "kopia"), mode: "LeftToRight", conflict: "Newer", schedule: { type: "Manual" } }];
fs.writeFileSync(path.join(root, "job.json"), JSON.stringify(job));
const cli = (...args) => { try { return execFileSync(BIN, ["--data-dir", data, ...args], { encoding: "utf8", timeout: 60000 }); } catch (e) { return (e.stdout || "") + (e.stderr || ""); } };
// import_config (jak save_job) nie waliduje lokalizacji - to ta sama ścieżka co zapis z kreatora
cli("import", path.join(root, "job.json"));
for (let i = 0; i < 3; i++) cli("run", "n1");
const depth = (p) => { let d = 0; for (let c = p; fs.existsSync(path.join(c, "kopia")); c = path.join(c, "kopia")) d++; return d; };
const d = depth(a);
R.check("lewa=X, prawa=X/kopia: kolejne uruchomienia nie tworzą coraz głębszych kopii (kopia/kopia/kopia/...)", d <= 1, `po 3 uruchomieniach głębokość zagnieżdżenia kopia/ = ${d}`);
fs.rmSync(root, { recursive: true, force: true });
R.done();
