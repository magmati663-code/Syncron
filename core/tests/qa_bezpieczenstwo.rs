//! QA tester 9 — bezpieczeństwo. Testy/PoC potwierdzające luki (oznaczone `#[ignore = "BUG: ..."]`;
//! uruchom `cargo test -p syncron-core --test qa_bezpieczenstwo -- --ignored`, aby zobaczyć, że padają)
//! oraz testy regresyjne zabezpieczeń, które działają.
//!
//! Testy `src_*` analizują kod źródłowy tam, gdzie funkcja jest zależna od Windows / leży w crate'ach bin
//! (main.rs aplikacji, CLI) i nie da się jej wywołać z testu na Linuksie.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use syncron_core::engine::{self, run_job};
use syncron_core::location::Location;
use syncron_core::model::Job;

fn job_id(id: &str, l: &Path, r: &str, mode: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": "t", "left": l.to_str().unwrap(), "right": r,
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 1
    }))
    .unwrap()
}

fn job(l: &Path, r: &Path, mode: &str) -> Job {
    job_id("t", l, r.to_str().unwrap(), mode)
}

fn repo_file(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    (t, l, r, s)
}

// ======================================================================
// 1. Komendy kosza (trash_restore / trash_delete / list_dirs): `..` w stamp/path
//    Wywoływalne z IPC Tauri (UI); wymagają wywołania z podmienionym argumentem (XSS / zmodyfikowany front).
// ======================================================================

#[test]
#[ignore = "BUG T9-01: restore_trash z path='../x' zapisuje plik poza korzeniem zadania"]
fn trash_restore_path_dotdot_nie_wychodzi_poza_korzen() {
    let (t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join(".syncron-trash/s")).unwrap();
    // plik "z kosza" osiągalny przez .syncron-trash/s/../outside.txt
    fs::write(l.join(".syncron-trash/outside.txt"), "x").unwrap();
    let j = job(&l, &t.path().join("R"), "TwoWay");
    let _ = engine::trash_restore(&j, "left", "s", "../outside.txt");
    assert!(
        !t.path().join("outside.txt").exists(),
        "plik zapisany poza korzeniem zadania L"
    );
}

#[test]
#[ignore = "BUG T9-01: restore_trash ze stamp='../..' kopiuje dowolny plik z dysku do folderu synchronizowanego"]
fn trash_restore_stamp_dotdot_nie_czyta_spoza_kosza() {
    let (t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join(".syncron-trash")).unwrap();
    fs::write(t.path().join("secret.txt"), "TAJNE").unwrap(); // poza folderem zadania
    let j = job(&l, &t.path().join("R"), "TwoWay");
    let _ = engine::trash_restore(&j, "left", "../..", "secret.txt");
    assert!(
        !l.join("secret.txt").exists(),
        "cudzy plik trafił do synchronizowanego folderu (i dalej do chmury)"
    );
}

#[test]
#[ignore = "BUG T9-01: delete_trash ze stamp='../../x' usuwa katalog poza koszem/korzeniem (remove_dir_all)"]
fn trash_delete_stamp_dotdot_nie_usuwa_poza_koszem() {
    let (t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join(".syncron-trash")).unwrap();
    let victim = t.path().join("wazny_folder");
    fs::create_dir_all(&victim).unwrap();
    fs::write(victim.join("dane.txt"), "x").unwrap();
    let j = job(&l, &t.path().join("R"), "TwoWay");
    let _ = engine::trash_delete(&j, "left", Some("../../wazny_folder"), None);
    assert!(
        victim.join("dane.txt").exists(),
        "folder poza koszem usunięty"
    );
}

#[test]
#[ignore = "BUG T9-01: delete_trash (stamp, path='../../plik') usuwa żywy plik użytkownika w folderze zadania"]
fn trash_delete_path_dotdot_nie_usuwa_zywych_plikow() {
    let (t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join(".syncron-trash/s")).unwrap();
    fs::write(l.join("zywy.txt"), "x").unwrap();
    let j = job(&l, &t.path().join("R"), "TwoWay");
    let _ = engine::trash_delete(&j, "left", Some("s"), Some("../../zywy.txt"));
    assert!(
        l.join("zywy.txt").exists(),
        "żywy plik usunięty przez trash_delete"
    );
}

#[test]
#[ignore = "BUG T9-02: list_dirs z rel='..' wylistowuje foldery spoza korzenia (ujawnienie struktury dysku)"]
fn list_dirs_dotdot_nie_wychodzi_poza_korzen() {
    let (t, l, _r, _s) = dirs();
    fs::create_dir_all(t.path().join("sekretny_projekt")).unwrap();
    let res = engine::list_dirs(l.to_str().unwrap(), "..");
    let names = res.unwrap_or_default();
    assert!(
        !names.iter().any(|n| n == "sekretny_projekt"),
        "listing wyszedł poza korzeń: {names:?}"
    );
}

#[test]
fn resolve_conflict_odrzuca_sciezke_spoza_listingu() {
    let (t, l, r, s) = dirs();
    fs::write(t.path().join("outside.txt"), "x").unwrap();
    let j = job(&l, &r, "TwoWay");
    let e = engine::resolve_conflict(&j, &s, "../outside.txt", true);
    assert!(e.is_err(), "ścieżka musi istnieć w listingu obu stron");
}

#[test]
fn identyfikator_zadania_jest_parametryzowany_w_sql() {
    let (_t, l, r, s) = dirs();
    fs::write(l.join("a.txt"), "a").unwrap();
    let j = job_id("x'; DROP TABLE base; --", &l, r.to_str().unwrap(), "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    run_job(&j, &s, false, false).unwrap();
    let c = engine::open_db(&s).unwrap();
    let n: i64 = c
        .query_row("SELECT count(*) FROM base", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "tabela base nietknięta, wpis zapisany");
    assert!(r.join("a.txt").exists());
}

// ======================================================================
// 2. Dowiązania symboliczne
// ======================================================================

#[cfg(unix)]
#[test]
fn listing_pomija_dowiazania_do_plikow_i_folderow() {
    use std::os::unix::fs::symlink;
    let (t, l, _r, _s) = dirs();
    let out = t.path().join("OUT");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("sekret.txt"), "x").unwrap();
    symlink(&out, l.join("link_dir")).unwrap();
    symlink(out.join("sekret.txt"), l.join("link_plik.txt")).unwrap();
    fs::write(l.join("zwykly.txt"), "ok").unwrap();
    let li = Location::parse(l.to_str().unwrap())
        .list(&Default::default())
        .unwrap();
    let keys: Vec<_> = li.files.keys().cloned().collect();
    assert_eq!(keys, vec!["zwykly.txt".to_string()], "{keys:?}");
}

#[cfg(unix)]
#[test]
#[ignore = "BUG T9-03: zapis przez dowiązanie symboliczne w katalogu docelowym wychodzi poza korzeń (R/sub -> OUT)"]
fn zapis_nie_podaza_za_symlinkiem_w_folderze_docelowym() {
    use std::os::unix::fs::symlink;
    let (t, l, r, s) = dirs();
    let out = t.path().join("OUT");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(l.join("sub")).unwrap();
    fs::write(l.join("sub/x.txt"), "dane").unwrap();
    symlink(&out, r.join("sub")).unwrap(); // lista po prawej pomija symlink, więc x.txt wygląda na "nowy"
    let j = job(&l, &r, "LeftToRight");
    let _ = run_job(&j, &s, false, false);
    assert!(
        !out.join("x.txt").exists(),
        "plik zapisany poza korzeniem R przez symlink"
    );
}

// ======================================================================
// 3. Ścieżki względne ze zdalnego listingu (fałszywy rclone)
// ======================================================================

static FAKE: Mutex<()> = Mutex::new(());

/// Fałszywy `rclone`: `lsjson` zwraca JSON z pliku, `copyto` zapisuje plik w miejscu docelowym i loguje argumenty.
#[cfg(unix)]
fn with_fake_rclone(json: &str, f: impl FnOnce(&Path, &Path, &Path)) {
    use std::os::unix::fs::PermissionsExt;
    let _g = FAKE.lock().unwrap_or_else(|e| e.into_inner());
    let (t, l, _r, s) = dirs();
    let bin = t.path().join("fake-rclone.sh");
    fs::write(
        &bin,
        "#!/bin/sh\necho \"$@\" >> \"$FAKE_LOG\"\ncase \"$1\" in\n lsjson) cat \"$FAKE_JSON\";;\n copyto) mkdir -p \"$(dirname \"$3\")\"; echo z-serwera > \"$3\";;\n cat) exit 1;;\n *) ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let jf = t.path().join("list.json");
    fs::write(&jf, json).unwrap();
    std::env::set_var("SYNCRON_RCLONE", &bin);
    std::env::set_var("FAKE_JSON", &jf);
    std::env::set_var("FAKE_LOG", t.path().join("rclone.log"));
    f(t.path(), &l, &s);
    std::env::remove_var("SYNCRON_RCLONE");
}

#[cfg(unix)]
fn pobierz_ze_zdalnego(t: &Path, l: &Path, s: &Path) {
    let j = job_id("rc", l, "fake:", "RightToLeft");
    let rep = run_job(&j, s, false, false).unwrap();
    let _ = (t, rep);
}

#[cfg(unix)]
#[test]
fn kontrola_harnessu_zwykla_sciezka_trafia_do_korzenia() {
    let json = r#"[{"Path":"ok.txt","Size":9,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#;
    with_fake_rclone(json, |t, l, s| {
        pobierz_ze_zdalnego(t, l, s);
        assert!(l.join("ok.txt").exists(), "harness działa");
    });
}

#[cfg(unix)]
#[test]
fn sciezka_bezwzgledna_ze_zdalnego_trafia_do_korzenia() {
    let json =
        r#"[{"Path":"/abs_zdalny/x.txt","Size":9,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#;
    with_fake_rclone(json, |t, l, s| {
        pobierz_ze_zdalnego(t, l, s);
        assert!(!Path::new("/abs_zdalny/x.txt").exists());
        assert!(l.join("abs_zdalny/x.txt").exists(), "{:?}", t);
    });
}

#[cfg(unix)]
#[test]
#[ignore = "BUG T9-04: listing zdalny z Path='../escape.txt' powoduje zapis poza korzeniem (brak walidacji komponentów '..')"]
fn zdalny_listing_z_dotdot_nie_zapisuje_poza_korzeniem() {
    let json = r#"[{"Path":"../escape.txt","Size":9,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
                   {"Path":"a/../../escape2.txt","Size":9,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#;
    with_fake_rclone(json, |t, l, s| {
        pobierz_ze_zdalnego(t, l, s);
        assert!(
            !t.join("escape.txt").exists(),
            "zapis poza L (../escape.txt)"
        );
        assert!(
            !t.join("escape2.txt").exists(),
            "zapis poza L (a/../../escape2.txt)"
        );
    });
}

#[cfg(unix)]
#[test]
#[ignore = "BUG T9-04: lokalny cel kopiowania/rename z '..' w ścieżce względnej (Location::copy_from/rename) wychodzi poza korzeń"]
fn copy_from_i_rename_nie_wychodza_poza_korzen() {
    let (t, l, r, _s) = dirs();
    fs::write(r.join("src.txt"), "x").unwrap();
    let ll = Location::parse(l.to_str().unwrap());
    let rr = Location::parse(r.to_str().unwrap());
    let o = syncron_core::location::Opts::new(false, false, &Default::default(), 1);
    let _ = ll.copy_from(&rr, "src.txt", "../kopia.txt", &o);
    fs::write(l.join("a.txt"), "x").unwrap();
    let _ = ll.rename("a.txt", "../przeniesiony.txt");
    assert!(!t.path().join("kopia.txt").exists());
    assert!(!t.path().join("przeniesiony.txt").exists());
}

/// Windows: nazwa pliku z listingu (np. Google Drive / Linux / macOS) może zawierać `\` lub literę dysku;
/// `PathBuf::push` traktuje takie komponenty jak ścieżkę (absolutną lub z `..`).
#[cfg(windows)]
#[test]
#[ignore = "BUG T9-05: local_full na Windows: komponent z '\\' lub 'C:\\' ucieka z korzenia"]
fn local_full_windows_nie_wychodzi_poza_korzen() {
    use syncron_core::location::local_full;
    let root = Path::new(r"C:\Sync");
    for rel in [
        r"..\..\Windows\evil.txt",
        r"D:\evil.txt",
        r"C:\Windows\evil.txt",
    ] {
        let p = local_full(root, rel);
        let s = p.to_string_lossy().replace(r"\\?\", "");
        assert!(
            s.starts_with(r"C:\Sync\") && !s.contains(".."),
            "{rel} -> {s}"
        );
    }
}

#[test]
#[ignore = "BUG T9-05: names::invalid_on_windows nie jest używana — nazwy z '\\', ':', '..' z innych systemów/chmur trafiają na Windows bez walidacji"]
fn src_invalid_on_windows_jest_uzywana_w_synchronizacji() {
    let used = ["location.rs", "engine.rs", "plan.rs", "runner.rs"]
        .iter()
        .any(|f| repo_file(&format!("core/src/{f}")).contains("invalid_on_windows"));
    assert!(
        used,
        "żaden moduł synchronizacji nie waliduje nazw względem Windows"
    );
}

// ======================================================================
// 4. VSS / PowerShell (kod cfg(windows) — analiza źródła)
// ======================================================================

#[test]
#[ignore = "BUG T9-06: delete_shadow wstawia id z %TEMP%\\syncron-vss-active.txt do skryptu PowerShell bez walidacji (wstrzyknięcie przy cleanup_stale jako administrator)"]
fn src_vss_id_z_pliku_rejestru_jest_walidowane() {
    let s = repo_file("core/src/vss.rs");
    assert!(
        s.contains("$_.ID -eq '{id}'"),
        "zmieniono skrypt — zaktualizuj test"
    );
    // oczekiwana poprawka: walidacja GUID (np. is_ascii_hexdigit / '-' / '{' '}') przed użyciem w skrypcie
    assert!(
        s.contains("is_ascii_hexdigit"),
        "id jest interpolowane do PowerShell bez sprawdzenia formatu; payload: x' }}; Start-Process calc; {{ '"
    );
}

/// Model: jak PowerShell parsuje literał w apostrofach (`''` -> `'`).
fn ps_single_unquote(lit: &str) -> String {
    lit.replace("''", "'")
}

#[test]
#[ignore = "BUG T9-07: restart_elevated — apostrof w ścieżce exe jest cytowany tylko raz, a wartość trafia do zagnieżdżonego skryptu (psuje uruchomienie lub pozwala na wstrzyknięcie)"]
fn src_restart_elevated_podwojne_cytowanie_apostrofu() {
    let s = repo_file("core/src/vss.rs");
    let tpl = "Start-Process -FilePath ''{e}'' -Verb RunAs";
    assert!(s.contains(tpl), "zmieniono skrypt — zaktualizuj test");
    // replika: e = exe z apostrofem zdublowanym raz (jak w kodzie)
    let exe = "C:\\Users\\O'Brien\\Syncron\\syncron.exe";
    let e = exe.replace('\'', "''");
    let outer_literal = format!("Start-Sleep -Seconds 2; {}", tpl.replace("{e}", &e));
    let inner = ps_single_unquote(&outer_literal); // to, co zobaczy wewnętrzny PowerShell
                                                   // poprawnie: wewnętrzny skrypt zawiera FilePath 'C:\Users\O''Brien\...' (apostrof zdublowany po raz drugi)
    assert!(
        inner.contains("O''Brien"),
        "wewnętrzny skrypt po rozpakowaniu: {inner}  (apostrof kończy literał -> błąd składni / injection)"
    );
}

#[test]
#[ignore = "BUG T9-08: plik rejestru migawek w %TEMP% o stałej, przewidywalnej nazwie i bez kontroli właściciela/uprawnień (m.in. C:\\Windows\\Temp dla konta SYSTEM)"]
fn src_vss_rejestr_nie_lezy_w_przewidywalnym_temp() {
    let s = repo_file("core/src/vss.rs");
    assert!(
        !s.contains("temp_dir().join(\"syncron-vss-active.txt\")"),
        "rejestr migawek powinien leżeć w katalogu danych aplikacji (prywatnym), nie w %TEMP%"
    );
}

#[test]
#[ignore = "BUG T9-09: kopie z migawki VSS trafiają do %TEMP%\\syncron-vss\\<pid>-<n> (przewidywalne nazwy; pozostają po awarii)"]
fn src_vss_temp_nie_jest_przewidywalny() {
    let s = repo_file("core/src/location.rs");
    assert!(
        !s.contains("temp_dir().join(\"syncron-vss\")"),
        "użyj tempfile z losową nazwą / katalog w danych aplikacji"
    );
}

// ======================================================================
// 5. Komendy Tauri: sekrety, walidacja, import (analiza main.rs)
// ======================================================================

#[test]
#[ignore = "BUG T9-10: add_remote przekazuje hasła/tokeny jako argumenty procesu rclone (widoczne dla innych procesów/użytkowników)"]
fn src_add_remote_nie_przekazuje_sekretow_w_argumentach() {
    let s = repo_file("app/src-tauri/src/main.rs");
    assert!(
        s.contains("format!(\"{k}={v}\")"),
        "zmieniono kod — zaktualizuj test"
    );
    // poprawka: wartości wrażliwe przez zmienną środowiskową (RCLONE_CONFIG_<NAME>_PASS) albo stdin/`rclone obscure -`
    assert!(
        !s.contains("format!(\"{k}={v}\")"),
        "wartości pól (pass, client_secret, ...) lądują w argv"
    );
}

#[test]
#[ignore = "BUG T9-11: add_remote/test_remote/delete_remote nie walidują `kind` ani kluczy `params` (klucz '--...' staje się flagą rclone)"]
fn src_add_remote_waliduje_kind_i_klucze() {
    let s = repo_file("app/src-tauri/src/main.rs");
    let start = s.find("async fn add_remote").unwrap();
    let body = &s[start..start + 1200];
    assert!(
        body.contains("kind.chars()")
            || body.contains("is_ascii_alphanumeric")
                && body.matches("is_ascii_alphanumeric").count() >= 2,
        "brak walidacji `kind` i kluczy `params`"
    );
}

#[test]
#[ignore = "BUG T9-12: import_config nie waliduje zadań jak save_job i od razu je uruchamia (enabled z pliku)"]
fn src_import_config_waliduje_i_nie_uruchamia_od_razu() {
    let s = repo_file("app/src-tauri/src/main.rs");
    let start = s.find("fn import_config").unwrap();
    let body = &s[start..start + 900];
    assert!(
        body.contains("enabled = false") || body.contains("is_empty()"),
        "import przyjmuje dowolne `left`/`right` (np. \\\\serwer\\udział) i startuje zadanie bez potwierdzenia"
    );
}

#[test]
fn zadanie_z_pliku_importu_niczego_nie_sanityzuje_ale_ui_to_escapuje() {
    // dokumentuje: nazwy/ścieżki z importu trafiają do UI surowe — bezpieczeństwo zależy od esc() w app.js
    let raw = r#"[{"id":"<img src=x onerror=alert(1)>","name":"<script>alert(1)</script>","left":"a","right":"b",
        "mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]"#;
    let v: Vec<Job> = serde_json::from_str(raw).unwrap();
    assert_eq!(v[0].name, "<script>alert(1)</script>");
}

// ======================================================================
// 6. tauri.conf.json / capabilities
// ======================================================================

fn tauri_conf() -> serde_json::Value {
    serde_json::from_str(&repo_file("app/src-tauri/tauri.conf.json")).unwrap()
}

#[test]
#[ignore = "BUG T9-13: tauri.conf.json: app.security.csp = null (brak Content-Security-Policy; ewentualny XSS = pełny dostęp do poleceń IPC)"]
fn tauri_csp_jest_ustawione() {
    let c = tauri_conf();
    assert!(
        c["app"]["security"]["csp"].is_string(),
        "csp = {}",
        c["app"]["security"]["csp"]
    );
}

#[test]
fn updater_uzywa_https_i_klucza_publicznego() {
    let c = tauri_conf();
    let u = &c["plugins"]["updater"];
    assert!(u["pubkey"].as_str().unwrap().len() > 40);
    for e in u["endpoints"].as_array().unwrap() {
        assert!(e.as_str().unwrap().starts_with("https://"), "{e}");
    }
}

#[test]
fn capabilities_nie_zawieraja_fs_shell_http() {
    let caps = repo_file("app/src-tauri/capabilities/default.json");
    for bad in [
        "fs:",
        "shell:",
        "http:",
        "opener:",
        "core:webview:allow-create",
        "allow-eval",
    ] {
        assert!(!caps.contains(bad), "zbędne uprawnienie: {bad}");
    }
    let v: serde_json::Value = serde_json::from_str(&caps).unwrap();
    assert_eq!(v["windows"], serde_json::json!(["main"]));
}

#[test]
fn ui_nie_laduje_zasobow_zewnetrznych() {
    let html = repo_file("app/ui/index.html");
    assert!(!html.contains("http://") && !html.contains("https://"));
    for l in html.lines().filter(|l| l.contains("<script")) {
        assert!(!l.contains("http"), "{l}");
    }
}

// ======================================================================
// 7. CLI: pliki usługi (analiza źródła)
// ======================================================================

#[test]
#[ignore = "BUG T9-14: launchd_plist/systemd_unit wstawiają ścieżki bez escapowania XML (&,<) i specyfikatorów systemd (% $)"]
fn src_cli_pliki_uslug_escapuja_sciezki() {
    let s = repo_file("cli/src/main.rs");
    assert!(
        s.contains("&amp;") && (s.contains("%%") || s.contains("replace('%'")),
        "ścieżka z '&' psuje plist; '%' w ExecStart jest rozwijane przez systemd"
    );
}

// ======================================================================
// 8. Location::parse — brak interpretacji connection-stringów / flag
// ======================================================================

#[test]
fn parse_nie_traktuje_connection_stringow_ani_flag_jako_zdalnych() {
    for s in [
        ":local:/etc",
        "--config=/x",
        ":sftp,host=h,pass=p:/",
        "a,b:c",
    ] {
        assert!(
            matches!(Location::parse(s), Location::Local(_)),
            "{s} powinno być lokalne"
        );
    }
}
