// Runda 1: statyczna zgodność kontraktu JS (invoke) <-> Rust (#[tauri::command], generate_handler!, nazwy/liczba argumentów).
// Uruchom: node app/ui/qa/r1_contract.js
const fs = require("fs");
const path = require("path");
const { reporter } = require("./lib");
const R = reporter("r1_contract");
const js = fs.readFileSync(path.join(__dirname, "../app.js"), "utf8");
const rs = fs.readFileSync(path.join(__dirname, "../../src-tauri/src/main.rs"), "utf8");
const cap = fs.readFileSync(path.join(__dirname, "../../src-tauri/capabilities/default.json"), "utf8");

// komendy Rust
const cmds = {};
for (const m of rs.matchAll(/#\[tauri::command\][\s\S]*?\n(?:pub )?(?:async )?fn (\w+)\s*(?:<[^>]*>)?\(([\s\S]*?)\)\s*(?:->|\{)/g)) {
  const args = m[2].split(/,\s*(?![^<]*>)/).map((s) => s.trim()).filter(Boolean)
    .filter((s) => !/State<|AppHandle|^_/.test(s)).map((s) => s.split(":")[0].trim());
  cmds[m[1]] = args;
}
const handler = (/generate_handler!\[([\s\S]*?)\]/.exec(rs)[1]).split(",").map((s) => s.trim()).filter(Boolean);
const camel = (s) => s.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

R.check("Każda #[tauri::command] jest w generate_handler!", Object.keys(cmds).every((c) => handler.includes(c)), Object.keys(cmds).filter((c) => !handler.includes(c)).join(","));
R.check("Każdy wpis generate_handler! ma definicję", handler.every((c) => cmds[c]), handler.filter((c) => !cmds[c]).join(","));

// wywołania invoke w JS
const used = new Set();
for (const m of js.matchAll(/invoke\("(\w+)"(?:,\s*(\{[^}]*\}))?\)/g)) {
  const [, name, objSrc] = m; used.add(name);
  R.check(`invoke("${name}") istnieje w Rust`, !!cmds[name]);
  if (!cmds[name]) continue;
  const keys = objSrc ? [...objSrc.matchAll(/(?:^|[{,]\s*)(\w+)\s*(?::|(?=[,}]))/g)].map((x) => x[1]) : [];
  const need = cmds[name].map(camel);
  const missing = need.filter((k) => !keys.includes(k)), extra = keys.filter((k) => !need.includes(k));
  R.check(`invoke("${name}") argumenty zgodne (${need.join(",") || "-"})`, !missing.length && !extra.length, `brak: ${missing} nadmiarowe: ${extra}`);
}
const unusedUi = Object.keys(cmds).filter((c) => !used.has(c));
console.log("Komendy Rust nieużywane w UI:", unusedUi.join(", ") || "(brak)");

// uprawnienia pluginów używanych w JS
const perms = { "dialog.open": "dialog:allow-open", "dialog.save": "dialog:allow-save", "autostart.enable": "autostart:allow-enable", "autostart.disable": "autostart:allow-disable", "autostart.isEnabled": "autostart:allow-is-enabled", "updater.check": "updater:default", "process.relaunch": "process:allow-restart" };
for (const [call, p] of Object.entries(perms)) if (js.includes(call.replace(".", "Api.").replace("dialogApi", "dialogApi")) || js.includes(call.split(".")[0]))
  R.check(`uprawnienie ${p} dla ${call}`, cap.includes(p) || (p.startsWith("dialog") && cap.includes("dialog:default")) || (p.startsWith("updater") && cap.includes("updater:default")));
R.check("Plugin 'process' zarejestrowany i w uprawnieniach (relaunch po aktualizacji)", rs.includes("tauri_plugin_process::init") && cap.includes("process:allow-restart"));
R.done();
