//! QA tester 7: testy wersji z linią poleceń (parsowanie, kody wyjścia, import/export, usługi, daemon).
//! Testy oznaczone `#[ignore = "BUG: ..."]` odtwarzają znane błędy (obecnie padają).
#![allow(dead_code, unused_imports)]

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

// Generatory plików usług są funkcjami binarki; dołączamy źródło binarki jako moduł, żeby je przetestować.
#[path = "../src/main.rs"]
mod cli_src;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_syncron"));
    c.env_remove("SYNCRON_DATA").env_remove("SYNCPOLAK_DATA");
    c
}

fn out_s(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err_s(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

struct Env {
    _t: tempfile::TempDir,
    root: PathBuf,
    l: PathBuf,
    r: PathBuf,
    d: PathBuf,
}

fn env() -> Env {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().to_path_buf();
    let (l, r, d) = (root.join("L"), root.join("R"), root.join("dane"));
    for p in [&l, &r, &d] {
        fs::create_dir_all(p).unwrap();
    }
    Env {
        _t: t,
        root,
        l,
        r,
        d,
    }
}

fn job(id: &str, name: &str, l: &Path, r: &Path) -> Value {
    json!({"id": id, "name": name, "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
           "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}})
}

fn write_jobs(d: &Path, v: &Value) {
    fs::write(d.join("jobs.json"), v.to_string()).unwrap();
}

fn run(d: &Path, args: &[&str]) -> Output {
    let mut a = vec!["--data-dir", d.to_str().unwrap()];
    a.extend_from_slice(args);
    bin().args(a).output().unwrap()
}

/// Zrzut drzewa: ścieżka -> (czy katalog, zawartość, mtime).
fn snap(root: &Path) -> BTreeMap<String, (bool, Vec<u8>, std::time::SystemTime)> {
    fn walk(
        base: &Path,
        p: &Path,
        m: &mut BTreeMap<String, (bool, Vec<u8>, std::time::SystemTime)>,
    ) {
        let Ok(rd) = fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let md = e.metadata().unwrap();
            let rel = e
                .path()
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if md.is_dir() {
                m.insert(rel, (true, vec![], md.modified().unwrap()));
                walk(base, &e.path(), m);
            } else {
                m.insert(
                    rel,
                    (
                        false,
                        fs::read(e.path()).unwrap_or_default(),
                        md.modified().unwrap(),
                    ),
                );
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(root, root, &mut m);
    m
}

// ───────────────────────── runda 1: podstawy ─────────────────────────

#[test]
fn help_variants_exit_zero() {
    for a in [vec![], vec!["help"], vec!["--help"], vec!["-h"]] {
        let o = bin().args(&a).output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{a:?}");
        assert!(out_s(&o).contains("UŻYCIE"), "{a:?}");
    }
}

#[test]
fn version_prints_crate_version() {
    for a in ["version", "--version", "-V"] {
        let o = bin().arg(a).output().unwrap();
        assert_eq!(o.status.code(), Some(0));
        assert_eq!(
            out_s(&o).trim(),
            format!("syncron {}", env!("CARGO_PKG_VERSION"))
        );
    }
    assert!(env!("CARGO_PKG_VERSION").starts_with("0.7"));
}

#[test]
fn data_dir_accepted_anywhere() {
    let e = env();
    write_jobs(&e.d, &json!([job("abcdef123456", "Foto", &e.l, &e.r)]));
    let ds = e.d.to_str().unwrap();
    for a in [
        vec!["--data-dir", ds, "list"],
        vec!["list", "--data-dir", ds],
        vec!["run", "Foto", "--dry-run", "--data-dir", ds],
        vec!["run", "--data-dir", ds, "Foto", "--dry-run"],
    ] {
        let o = bin().args(&a).output().unwrap();
        assert!(o.status.success(), "{a:?}: {}", err_s(&o));
    }
}

#[test]
fn data_dir_missing_value_is_error_exit_1() {
    let o = bin().args(["list", "--data-dir"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("--data-dir wymaga wartości"));
}

#[test]
fn unknown_command_exit_1() {
    let o = bin().arg("wat").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("nieznane polecenie"));
}

#[test]
fn run_without_name_exit_1() {
    let e = env();
    for a in [vec!["run"], vec!["run", "--dry-run"]] {
        let o = run(&e.d, &a);
        assert_eq!(o.status.code(), Some(1), "{a:?}");
        assert!(err_s(&o).contains("podaj nazwę"));
    }
}

#[test]
fn export_import_without_arg_exit_1() {
    let e = env();
    assert_eq!(run(&e.d, &["export"]).status.code(), Some(1));
    assert_eq!(run(&e.d, &["import"]).status.code(), Some(1));
}

#[test]
fn list_empty_dir_mentions_folder() {
    let e = env();
    let o = run(&e.d, &["list"]);
    assert!(o.status.success());
    assert!(out_s(&o).contains("Brak zadań"));
}

#[test]
fn list_shows_disabled_and_schedule() {
    let e = env();
    let mut j = job("abcdef123456", "Foto", &e.l, &e.r);
    j["enabled"] = json!(false);
    j["schedule"] = json!({"type": "Interval", "secs": 600});
    write_jobs(&e.d, &json!([j]));
    let o = out_s(&run(&e.d, &["list"]));
    assert!(o.contains("(wyłączone)") && o.contains("co 10 min"), "{o}");
}

// ───────────────────────── dopasowanie nazwy zadania ─────────────────────────

fn two_jobs(e: &Env) {
    write_jobs(
        &e.d,
        &json!([
            job("aaaa1111", "Zdjęcia rodzinne", &e.l, &e.r),
            job("aaaa2222", "Zdjęcia służbowe", &e.l, &e.r),
            job("bbbb3333", "Łódź", &e.l, &e.r)
        ]),
    );
}

#[test]
fn match_exact_name_wins_over_substring() {
    let e = env();
    write_jobs(
        &e.d,
        &json!([
            job("11111111", "Kopia", &e.l, &e.r),
            job("22222222", "Kopia zapasowa", &e.l, &e.r)
        ]),
    );
    let o = run(&e.d, &["run", "Kopia", "--dry-run"]);
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(out_s(&o).contains("Zadanie „Kopia”"));
}

#[test]
fn match_ambiguous_lists_candidates_exit_1() {
    let e = env();
    two_jobs(&e);
    let o = run(&e.d, &["run", "zdjęcia", "--dry-run"]);
    assert_eq!(o.status.code(), Some(1));
    let s = err_s(&o);
    assert!(s.contains("pasuje do wielu") && s.contains("rodzinne") && s.contains("służbowe"));
}

#[test]
fn match_by_id_prefix_and_polish_case() {
    let e = env();
    two_jobs(&e);
    assert!(run(&e.d, &["run", "bbbb", "--dry-run"]).status.success());
    // Ł/ł oraz Ó/ó, Ź/ź
    assert!(run(&e.d, &["run", "ŁÓDŹ", "--dry-run"]).status.success());
    assert!(run(&e.d, &["run", "łódź", "--dry-run"]).status.success());
    assert!(run(&e.d, &["run", "ÓDŹ", "--dry-run"]).status.success());
}

#[test]
fn match_not_found_exit_1() {
    let e = env();
    two_jobs(&e);
    let o = run(&e.d, &["run", "zzz", "--dry-run"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("nie znaleziono"));
}

#[test]
fn match_nfd_query_matches_nfc_name() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Zażółć", &e.l, &e.r)]));
    // „Zażółć” w NFD: z + kropka, o + akcent (ł i ć: ć = c + akcent; ł nie ma rozkładu)
    let nfd = "Za\u{17c}o\u{301}\u{142}c\u{301}";
    let nfd = nfd.replace('\u{17c}', "z\u{307}");
    let o = run(&e.d, &["run", &nfd, "--dry-run"]);
    assert!(o.status.success(), "{}", err_s(&o));
}

#[test]
fn match_empty_query_rejected() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Jedyne", &e.l, &e.r)]));
    fs::write(e.l.join("a.txt"), "x").unwrap();
    let o = run(&e.d, &["run", ""]);
    assert_eq!(o.status.code(), Some(1), "{}", out_s(&o));
    assert!(!e.r.join("a.txt").exists());
}

// ───────────────────────── run: opcje, dry-run, force, kody wyjścia ─────────────────────────

#[test]
fn run_unknown_flag_does_not_execute() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &e.r)]));
    fs::write(e.l.join("a.txt"), "x").unwrap();
    let o = run(&e.d, &["run", "Foto", "--dryrun"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(
        !e.r.join("a.txt").exists(),
        "literówka w --dry-run skopiowała pliki!"
    );
}

#[test]
fn dry_run_changes_nothing_in_either_side() {
    let e = env();
    let mut j = job("11111111", "Foto", &e.l, &e.r);
    j["mode"] = json!("TwoWay");
    write_jobs(&e.d, &json!([j]));
    fs::create_dir_all(e.l.join("pod/głęboko")).unwrap();
    fs::write(e.l.join("pod/głęboko/ą.txt"), "lewy").unwrap();
    fs::write(e.l.join("tylko_lewy.txt"), "1").unwrap();
    fs::write(e.r.join("tylko_prawy.txt"), "2").unwrap();
    fs::write(e.l.join("oba.txt"), "L").unwrap();
    fs::write(e.r.join("oba.txt"), "R-inna-dluzsza").unwrap();
    let (a_l, a_r) = (snap(&e.l), snap(&e.r));
    let o = run(&e.d, &["run", "Foto", "--dry-run"]);
    assert!(o.status.code().is_some(), "{}", err_s(&o));
    assert!(out_s(&o).contains("SYMULACJA"));
    assert_eq!(a_l, snap(&e.l), "dry-run zmienił lewą stronę");
    assert_eq!(a_r, snap(&e.r), "dry-run zmienił prawą stronę");
}

#[test]
fn dry_run_does_not_create_missing_right_dir() {
    let e = env();
    let missing = e.root.join("nie_ma");
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &missing)]));
    fs::write(e.l.join("a.txt"), "x").unwrap();
    let _ = run(&e.d, &["run", "Foto", "--dry-run"]);
    assert!(!missing.exists(), "dry-run utworzył folder docelowy");
}

#[test]
fn dry_run_then_real_run_still_copies() {
    // dry-run nie może zapisać migawki (inaczej prawdziwy przebieg uznałby pliki za zsynchronizowane)
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &e.r)]));
    fs::write(e.l.join("a.txt"), "x").unwrap();
    run(&e.d, &["run", "Foto", "--dry-run"]);
    let o = run(&e.d, &["run", "Foto"]);
    assert!(o.status.success(), "{}", err_s(&o));
    assert_eq!(fs::read_to_string(e.r.join("a.txt")).unwrap(), "x");
}

#[test]
fn run_missing_left_folder_is_error_exit_1() {
    let e = env();
    let missing = e.root.join("brak");
    write_jobs(&e.d, &json!([job("11111111", "Foto", &missing, &e.r)]));
    let o = run(&e.d, &["run", "Foto"]);
    assert_eq!(o.status.code(), Some(1), "{}{}", out_s(&o), err_s(&o));
    assert!(err_s(&o).contains("lewa strona"));
}

#[test]
fn run_blocked_by_guard_exit_2_and_force_proceeds() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &e.r)]));
    for i in 0..4 {
        fs::write(e.l.join(format!("f{i}.txt")), "x").unwrap();
    }
    assert!(run(&e.d, &["run", "Foto"]).status.success());
    for i in 0..4 {
        fs::remove_file(e.l.join(format!("f{i}.txt"))).unwrap();
    }
    let o = run(&e.d, &["run", "Foto"]);
    assert_eq!(o.status.code(), Some(2), "{}", out_s(&o));
    assert!(out_s(&o).contains("ZATRZYMANO") && out_s(&o).contains("--force"));
    assert!(e.r.join("f0.txt").exists(), "zabezpieczenie nie zadziałało");
    let o = run(&e.d, &["run", "Foto", "--force"]);
    assert!(o.status.success(), "{}", out_s(&o));
    assert!(!e.r.join("f0.txt").exists());
}

#[test]
fn run_conflict_exit_2() {
    let e = env();
    let mut j = job("11111111", "Foto", &e.l, &e.r);
    j["mode"] = json!("TwoWay");
    j["conflict"] = json!("Ask");
    write_jobs(&e.d, &json!([j]));
    fs::write(e.l.join("a.txt"), "baza").unwrap();
    assert!(run(&e.d, &["run", "Foto"]).status.success());
    std::thread::sleep(Duration::from_millis(1100));
    fs::write(e.l.join("a.txt"), "lewa wersja").unwrap();
    fs::write(e.r.join("a.txt"), "prawa wersja dluzsza").unwrap();
    let o = run(&e.d, &["run", "Foto"]);
    // oba pliki muszą przetrwać niezależnie od kodu wyjścia
    assert_eq!(
        fs::read_to_string(e.l.join("a.txt")).unwrap(),
        "lewa wersja"
    );
    assert_eq!(
        fs::read_to_string(e.r.join("a.txt")).unwrap(),
        "prawa wersja dluzsza"
    );
    assert_eq!(o.status.code(), Some(2), "{}{}", out_s(&o), err_s(&o));
}

#[test]
fn run_job_name_with_spaces_and_unicode_args() {
    let e = env();
    let l = e.root.join("Zdjęcia z wakacji – Łeba");
    let r = e.root.join("Kopia zapasowa ąę");
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join("żółw.txt"), "x").unwrap();
    write_jobs(&e.d, &json!([job("11111111", "Łeba – 2024", &l, &r)]));
    let o = run(&e.d, &["run", "Łeba – 2024"]);
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(r.join("żółw.txt").exists());
}

// ───────────────────────── runda 2: export / import ─────────────────────────

#[test]
fn import_invalid_json_exit_1_and_jobs_untouched() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &e.r)]));
    let before = fs::read(e.d.join("jobs.json")).unwrap();
    let f = e.root.join("zly.json");
    fs::write(&f, "{ to nie json").unwrap();
    let o = run(&e.d, &["import", f.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("to nie jest plik konfiguracji"));
    assert_eq!(before, fs::read(e.d.join("jobs.json")).unwrap());
}

#[test]
fn import_wrong_shape_and_missing_file_exit_1() {
    let e = env();
    let f = e.root.join("obj.json");
    fs::write(&f, r#"{"jobs": []}"#).unwrap();
    assert_eq!(
        run(&e.d, &["import", f.to_str().unwrap()]).status.code(),
        Some(1)
    );
    let o = run(
        &e.d,
        &["import", e.root.join("nie_ma.json").to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(
        !e.d.join("jobs.json").exists(),
        "nieudany import utworzył jobs.json"
    );
}

#[test]
fn import_empty_list_keeps_existing_jobs() {
    let e = env();
    write_jobs(&e.d, &json!([job("11111111", "Foto", &e.l, &e.r)]));
    let f = e.root.join("pusty.json");
    fs::write(&f, "[]").unwrap();
    let o = run(&e.d, &["import", f.to_str().unwrap()]);
    assert!(o.status.success());
    assert!(out_s(&o).contains("Zaimportowano zadań: 0"));
    assert!(out_s(&run(&e.d, &["list"])).contains("Foto"));
}

#[test]
fn import_duplicate_ids_stored_once_last_wins() {
    let e = env();
    let f = e.root.join("dup.json");
    fs::write(
        &f,
        json!([
            job("same", "Pierwsze", &e.l, &e.r),
            job("same", "Drugie", &e.l, &e.r)
        ])
        .to_string(),
    )
    .unwrap();
    assert!(run(&e.d, &["import", f.to_str().unwrap()]).status.success());
    let l = out_s(&run(&e.d, &["list"]));
    assert_eq!(l.lines().count(), 1, "{l}");
    assert!(l.contains("Drugie"));
}

#[test]
fn import_duplicate_ids_message_count_is_accurate() {
    let e = env();
    let f = e.root.join("dup.json");
    fs::write(
        &f,
        json!([job("same", "A", &e.l, &e.r), job("same", "B", &e.l, &e.r)]).to_string(),
    )
    .unwrap();
    let o = run(&e.d, &["import", f.to_str().unwrap()]);
    assert!(!out_s(&o).contains("zadań: 2"), "{}", out_s(&o));
}

#[test]
fn import_is_idempotent_and_merges_by_id() {
    let e = env();
    write_jobs(
        &e.d,
        &json!([job("a", "Stare A", &e.l, &e.r), job("b", "B", &e.l, &e.r)]),
    );
    let f = e.root.join("in.json");
    fs::write(
        &f,
        json!([job("a", "Nowe A", &e.l, &e.r), job("c", "C", &e.l, &e.r)]).to_string(),
    )
    .unwrap();
    for _ in 0..2 {
        assert!(run(&e.d, &["import", f.to_str().unwrap()]).status.success());
    }
    let l = out_s(&run(&e.d, &["list"]));
    assert_eq!(l.lines().count(), 3, "{l}");
    assert!(
        l.contains("Nowe A")
            && !l.contains("Stare A")
            && l.contains("  B  ")
            && l.contains("  C  ")
    );
}

#[test]
fn import_unknown_fields_ignored_and_old_schema_gets_defaults() {
    let e = env();
    // zadanie ze schematu v0.1: bez bandwidth/conditions/triggers/verify/use_vss/max_versions/threads/...
    let f = e.root.join("stare.json");
    fs::write(
        &f,
        json!([{"id": "old1", "name": "Stare", "left": e.l.to_str().unwrap(), "right": e.r.to_str().unwrap(),
                "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Interval", "secs": 300},
                "pole_z_przyszlosci": {"x": [1, 2, 3]}}])
        .to_string(),
    )
    .unwrap();
    assert!(run(&e.d, &["import", f.to_str().unwrap()]).status.success());
    let out = e.root.join("out.json");
    assert!(run(&e.d, &["export", out.to_str().unwrap()])
        .status
        .success());
    let v: Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let j = &v[0];
    assert_eq!(j["enabled"], true);
    assert_eq!(j["use_vss"], true);
    assert_eq!(j["verify"], false);
    assert_eq!(j["guard_pct"], 25);
    assert_eq!(j["trash_days"], 30);
    assert_eq!(j["threads"], 4);
    assert_eq!(j["max_versions"], 0);
    assert_eq!(j["bandwidth"]["kbps"], 0);
    assert_eq!(j["conditions"]["ac_only"], false);
    assert_eq!(j["triggers"]["on_wake"], false);
    assert_eq!(j["no_delete_left"], false);
    assert!(j.get("pole_z_przyszlosci").is_none());
}

#[test]
fn export_to_missing_dir_exit_1() {
    let e = env();
    write_jobs(&e.d, &json!([job("a", "A", &e.l, &e.r)]));
    let o = run(
        &e.d,
        &["export", e.root.join("nie/ma/pliku.json").to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn export_roundtrip_preserves_special_characters() {
    let e = env();
    let name = "Zażółć \"gęślą\" jaźń \\ / <&> 日本語 🙂";
    write_jobs(&e.d, &json!([job("a", name, &e.l, &e.r)]));
    let f = e.root.join("x.json");
    assert!(run(&e.d, &["export", f.to_str().unwrap()]).status.success());
    let d2 = e.root.join("d2");
    assert!(run(&d2, &["import", f.to_str().unwrap()]).status.success());
    let v: Value =
        serde_json::from_str(&fs::read_to_string(d2.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(v[0]["name"], name);
}

#[test]
fn import_creates_missing_data_dir_and_leaves_no_tmp() {
    let e = env();
    let d2 = e.root.join("a/b/c");
    let f = e.root.join("in.json");
    fs::write(&f, json!([job("a", "A", &e.l, &e.r)]).to_string()).unwrap();
    assert!(run(&d2, &["import", f.to_str().unwrap()]).status.success());
    assert!(d2.join("jobs.json").exists());
    assert!(!d2.join("jobs.json.tmp").exists());
}

// ───── utrata konfiguracji: uszkodzony / niezgodny jobs.json ─────

#[test]
fn import_into_corrupt_jobs_json_does_not_destroy_it() {
    let e = env();
    let corrupt = r#"[{"id":"a","name":"Ważne","left":"/x","right":"/y","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}},{"id":"b","name":"Uciętе"#;
    fs::write(e.d.join("jobs.json"), corrupt).unwrap();
    let f = e.root.join("in.json");
    fs::write(&f, json!([job("n", "Nowe", &e.l, &e.r)]).to_string()).unwrap();
    let o = run(&e.d, &["import", f.to_str().unwrap()]);
    let now = fs::read_to_string(e.d.join("jobs.json")).unwrap();
    let preserved = now == corrupt
        || fs::read_dir(&e.d).unwrap().flatten().any(|x| {
            x.path() != e.d.join("jobs.json")
                && fs::read_to_string(x.path())
                    .map(|s| s == corrupt)
                    .unwrap_or(false)
        });
    assert!(
        preserved,
        "uszkodzony jobs.json zniszczony (exit {:?})",
        o.status.code()
    );
}

#[test]
fn list_corrupt_jobs_json_reports_error() {
    let e = env();
    fs::write(e.d.join("jobs.json"), "[{\"id\":\"a\",\"name\":").unwrap();
    let o = run(&e.d, &["list"]);
    assert_ne!(o.status.code(), Some(0), "{}", out_s(&o));
}

#[test]
fn run_corrupt_jobs_json_message_mentions_file() {
    let e = env();
    fs::write(e.d.join("jobs.json"), "{{{").unwrap();
    let o = run(&e.d, &["run", "cokolwiek"]);
    assert!(
        !err_s(&o).contains("nie znaleziono zadania"),
        "{}",
        err_s(&o)
    );
}

#[test]
fn export_corrupt_jobs_json_fails() {
    let e = env();
    fs::write(e.d.join("jobs.json"), "{{{").unwrap();
    let f = e.root.join("kopia.json");
    let o = run(&e.d, &["export", f.to_str().unwrap()]);
    assert_ne!(o.status.code(), Some(0));
}

#[test]
fn jobs_json_with_bom_is_read() {
    let e = env();
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend(
        json!([job("a", "Foto", &e.l, &e.r)])
            .to_string()
            .into_bytes(),
    );
    fs::write(e.d.join("jobs.json"), b).unwrap();
    assert!(out_s(&run(&e.d, &["list"])).contains("Foto"));
}

#[test]
fn import_file_with_bom_works() {
    let e = env();
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend(
        json!([job("a", "Foto", &e.l, &e.r)])
            .to_string()
            .into_bytes(),
    );
    let f = e.root.join("bom.json");
    fs::write(&f, b).unwrap();
    let o = run(&e.d, &["import", f.to_str().unwrap()]);
    assert!(o.status.success(), "{}", err_s(&o));
}

#[test]
fn one_bad_job_does_not_hide_the_others() {
    let e = env();
    let mut bad = job("zly", "Z przyszłości", &e.l, &e.r);
    bad["mode"] = json!("TrybZPrzyszlosci");
    write_jobs(&e.d, &json!([job("a", "Dobre", &e.l, &e.r), bad]));
    assert!(out_s(&run(&e.d, &["list"])).contains("Dobre"));
}

#[test]
fn utf8_invalid_jobs_json_does_not_panic() {
    let e = env();
    fs::write(e.d.join("jobs.json"), [0xFF, 0xFE, 0x00, 0x80]).unwrap();
    let o = run(&e.d, &["list"]);
    assert!(o.status.code().is_some() && o.status.code() != Some(101));
}

// ───── panika / znaki specjalne ─────

#[test]
fn list_multibyte_id_does_not_panic() {
    let e = env();
    write_jobs(&e.d, &json!([job("abcdefgą-id", "Foto", &e.l, &e.r)]));
    let o = run(&e.d, &["list"]);
    assert_eq!(o.status.code(), Some(0), "{}", err_s(&o));
}

#[test]
fn list_escapes_control_chars_in_name() {
    let e = env();
    write_jobs(&e.d, &json!([job("a", "Foto\u{1b}[2J\u{7}", &e.l, &e.r)]));
    let o = run(&e.d, &["list"]);
    assert!(!o.stdout.contains(&0x1b));
}

#[test]
fn very_long_name_and_unusual_chars_ok() {
    let e = env();
    let name = "Ż".repeat(50_000) + "\tkoniec\nnowa linia";
    write_jobs(&e.d, &json!([job("id-dlugie", &name, &e.l, &e.r)]));
    let o = run(&e.d, &["list"]);
    assert!(o.status.success());
    let o = run(&e.d, &["run", "id-dl", "--dry-run"]);
    assert!(o.status.success(), "{}", err_s(&o));
}

#[test]
fn list_interval_below_minute_not_zero_min() {
    let e = env();
    let mut j = job("a", "Foto", &e.l, &e.r);
    j["schedule"] = json!({"type": "Interval", "secs": 30});
    write_jobs(&e.d, &json!([j]));
    assert!(!out_s(&run(&e.d, &["list"])).contains("co 0 min"));
}

#[cfg(unix)]
#[test]
fn readonly_data_dir_import_reports_error_not_panic() {
    use std::os::unix::fs::PermissionsExt;
    let e = env();
    let ro = e.root.join("ro");
    fs::create_dir_all(&ro).unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    // root omija uprawnienia – wtedy test nic nie dowodzi, ale też nie może paść
    let f = e.root.join("in.json");
    fs::write(&f, json!([job("a", "A", &e.l, &e.r)]).to_string()).unwrap();
    let o = run(&ro, &["import", f.to_str().unwrap()]);
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        o.status.code() == Some(0) || o.status.code() == Some(1),
        "{:?}",
        o.status
    );
}

// ───── folder danych: domyślny, zmienne środowiskowe, migracja ─────

#[test]
fn data_dir_resolution_order() {
    let e = env();
    let (a, b, c) = (e.root.join("a"), e.root.join("b"), e.root.join("c"));
    for (p, n) in [(&a, "Zad-A"), (&b, "Zad-B"), (&c, "Zad-C")] {
        fs::create_dir_all(p).unwrap();
        write_jobs(p, &json!([job("id", n, &e.l, &e.r)]));
    }
    // SYNCRON_DATA ma pierwszeństwo przed SYNCPOLAK_DATA
    let o = bin()
        .arg("list")
        .env("SYNCRON_DATA", &a)
        .env("SYNCPOLAK_DATA", &b)
        .output()
        .unwrap();
    assert!(out_s(&o).contains("Zad-A"));
    // sama stara zmienna nadal działa
    let o = bin()
        .arg("list")
        .env("SYNCPOLAK_DATA", &b)
        .output()
        .unwrap();
    assert!(out_s(&o).contains("Zad-B"));
    // --data-dir przebija zmienne
    let o = bin()
        .args(["--data-dir", c.to_str().unwrap(), "list"])
        .env("SYNCRON_DATA", &a)
        .output()
        .unwrap();
    assert!(out_s(&o).contains("Zad-C"));
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn default_data_dir_follows_xdg_and_home() {
    let e = env();
    let o = bin()
        .arg("list")
        .env("XDG_DATA_HOME", e.root.join("xdg"))
        .env("HOME", &e.root)
        .output()
        .unwrap();
    assert!(
        out_s(&o).contains(e.root.join("xdg/pl.syncron.app").to_str().unwrap()),
        "{}",
        out_s(&o)
    );
    let o = bin()
        .arg("list")
        .env_remove("XDG_DATA_HOME")
        .env("HOME", &e.root)
        .output()
        .unwrap();
    assert!(
        out_s(&o).contains(e.root.join(".local/share/pl.syncron.app").to_str().unwrap()),
        "{}",
        out_s(&o)
    );
}

#[test]
fn empty_env_var_is_ignored() {
    let e = env();
    let cwd = e.root.join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    let f = e.root.join("in.json");
    fs::write(&f, json!([job("a", "A", &e.l, &e.r)]).to_string()).unwrap();
    let o = bin()
        .args(["import", f.to_str().unwrap()])
        .current_dir(&cwd)
        .env("SYNCRON_DATA", "")
        .env("HOME", &e.root)
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(
        !cwd.join("jobs.json").exists(),
        "jobs.json zapisany w bieżącym katalogu"
    );
}

#[test]
fn cli_migrates_from_old_sibling_folder() {
    let e = env();
    let old = e.root.join("pl.syncpolak.app");
    let new = e.root.join("pl.syncron.app");
    fs::create_dir_all(&old).unwrap();
    write_jobs(&old, &json!([job("a", "Stare zadanie", &e.l, &e.r)]));
    fs::write(old.join("rclone.conf"), "[gdrive]\n").unwrap();
    let o = run(&new, &["list"]);
    assert!(out_s(&o).contains("Stare zadanie"), "{}", out_s(&o));
    assert_eq!(
        fs::read_to_string(new.join("rclone.conf")).unwrap(),
        "[gdrive]\n"
    );
    assert!(old.join("jobs.json").exists());
}

// ───────────────────────── pliki usług ─────────────────────────

#[cfg(unix)]
fn fake_systemctl(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin_dir = dir.join("fakebin");
    fs::create_dir_all(&bin_dir).unwrap();
    let s = bin_dir.join("systemctl");
    fs::write(
        &s,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"{}/systemctl.log\"\nexit 0\n",
            dir.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&s, fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

#[cfg(unix)]
fn install_service(e: &Env, data_dir: &str) -> (Output, String) {
    let home = e.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake = fake_systemctl(&e.root);
    let o = bin()
        .args(["--data-dir", data_dir, "service", "install"])
        .env("HOME", &home)
        .env("PATH", &fake)
        .output()
        .unwrap();
    let unit =
        fs::read_to_string(home.join(".config/systemd/user/syncron.service")).unwrap_or_default();
    (o, unit)
}

#[cfg(unix)]
#[cfg(target_os = "linux")]
#[test]
fn service_install_writes_unit_with_quoted_polish_path_with_spaces() {
    let e = env();
    let dd = e.root.join("Moje dane żółć");
    let (o, unit) = install_service(&e, dd.to_str().unwrap());
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(
        unit.contains(&format!("--data-dir \"{}\" daemon", dd.display())),
        "{unit}"
    );
    let log = fs::read_to_string(e.root.join("systemctl.log")).unwrap();
    assert!(log.contains("--user daemon-reload") && log.contains("--user enable --now syncron"));
}

#[cfg(unix)]
#[cfg(target_os = "linux")]
#[test]
fn service_uninstall_removes_unit() {
    let e = env();
    let (_o, unit) = install_service(&e, e.d.to_str().unwrap());
    assert!(!unit.is_empty());
    let home = e.root.join("home");
    let o = bin()
        .args(["service", "uninstall"])
        .env("HOME", &home)
        .env("PATH", e.root.join("fakebin"))
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(!home.join(".config/systemd/user/syncron.service").exists());
}

#[cfg(unix)]
#[cfg(target_os = "linux")]
#[test]
fn systemd_unit_newline_in_path_cannot_inject_directives() {
    let e = env();
    let evil = format!(
        "{}/x\"\nExecStartPre=/bin/touch /tmp/pwned\n#",
        e.root.display()
    );
    let (_o, unit) = install_service(&e, &evil);
    assert!(
        !unit.lines().any(|l| l.starts_with("ExecStartPre")),
        "{unit}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn systemd_unit_escapes_percent_and_dollar() {
    let u = cli_src::systemd_unit("/opt/syncron", "/home/x/100%/$HOME/dane");
    assert!(u.contains("100%%") && u.contains("$$HOME"), "{u}");
}

#[cfg(target_os = "linux")]
#[test]
fn systemd_unit_escapes_quote_and_backslash() {
    let u = cli_src::systemd_unit("/opt/syncron", "/home/x/a\"b\\c");
    assert!(u.contains("a\\\"b\\\\c"), "{u}");
}

#[cfg(unix)]
#[cfg(target_os = "linux")]
#[test]
fn service_unit_uses_absolute_data_dir() {
    let e = env();
    let home = e.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake = fake_systemctl(&e.root);
    bin()
        .args(["--data-dir", "wzgledny/folder", "service", "install"])
        .current_dir(&e.root)
        .env("HOME", &home)
        .env("PATH", &fake)
        .output()
        .unwrap();
    let unit = fs::read_to_string(home.join(".config/systemd/user/syncron.service")).unwrap();
    assert!(
        unit.contains(&format!("--data-dir \"{}", e.root.display())),
        "{unit}"
    );
}

#[test]
fn launchd_plist_escapes_xml() {
    let p = cli_src::launchd_plist("/Users/Tom & Jerry/syncron", "/Users/a<b>/dane");
    assert!(
        p.contains("Tom &amp; Jerry") && p.contains("a&lt;b&gt;"),
        "{p}"
    );
}

#[test]
fn launchd_plist_plain_paths_and_polish_chars() {
    let p = cli_src::launchd_plist(
        "/Users/Łukasz/Aplikacje/syncron",
        "/Users/Łukasz/Library/Application Support/pl.syncron.app",
    );
    assert!(
        p.starts_with("<?xml")
            && p.contains(
                "<string>/Users/Łukasz/Library/Application Support/pl.syncron.app</string>"
            )
    );
    assert!(p.contains("<string>--data-dir</string>") && p.contains("<string>daemon</string>"));
}

#[test]
fn service_bad_subcommand_exit_1() {
    for a in [vec!["service"], vec!["service", "frob"]] {
        let o = bin()
            .args(&a)
            .env("HOME", "/nonexistent-home")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(1), "{a:?}");
    }
}

#[cfg(unix)]
#[cfg(target_os = "linux")]
#[test]
fn service_install_unknown_flag_rejected() {
    let e = env();
    let home = e.root.join("home");
    fs::create_dir_all(&home).unwrap();
    let o = bin()
        .args(["service", "install", "--bot"])
        .env("HOME", &home)
        .env("PATH", fake_systemctl(&e.root))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
}

// ───────────────────────── runda 3: daemon i równoległość ─────────────────────────

struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_daemon(d: &Path) -> Daemon {
    Daemon(
        bin()
            .args(["--data-dir", d.to_str().unwrap(), "daemon"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

fn wait_for(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn interval_job(id: &str, l: &Path, r: &Path) -> Value {
    let mut j = job(id, id, l, r);
    j["schedule"] = json!({"type": "Interval", "secs": 10});
    j
}

fn atomic_write_jobs(d: &Path, v: &Value) {
    let tmp = d.join("jobs.json.tmp");
    fs::write(&tmp, v.to_string()).unwrap();
    fs::rename(tmp, d.join("jobs.json")).unwrap();
}

#[cfg(unix)]
#[test]
fn daemon_runs_interval_job_and_dies_cleanly_on_sigterm() {
    let e = env();
    write_jobs(&e.d, &json!([interval_job("odstep", &e.l, &e.r)]));
    fs::write(e.l.join("żółć.txt"), "dane").unwrap();
    let mut dm = spawn_daemon(&e.d);
    assert!(
        wait_for(Duration::from_secs(20), || e.r.join("żółć.txt").exists()),
        "daemon nie skopiował pliku"
    );
    // kolejny plik – zadanie musi się powtórzyć wg interwału (10 s)
    fs::write(e.l.join("drugi.txt"), "2").unwrap();
    assert!(
        wait_for(Duration::from_secs(25), || e.r.join("drugi.txt").exists()),
        "brak ponownego przebiegu"
    );
    assert!(
        dm.0.try_wait().unwrap().is_none(),
        "daemon zakończył się sam"
    );
    let pid = dm.0.id().to_string();
    assert!(Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap()
        .success());
    let t = Instant::now();
    let st = loop {
        if let Some(s) = dm.0.try_wait().unwrap() {
            break s;
        }
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "daemon nie reaguje na SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    use std::os::unix::process::ExitStatusExt;
    assert!(st.signal() == Some(15) || st.code().is_some(), "{st:?}");
    // nie zostaje częściowy plik tymczasowy konfiguracji
    assert!(!e.d.join("jobs.json.tmp").exists());
}

#[test]
fn daemon_reloads_jobs_json_after_atomic_replace() {
    let e = env();
    let (l2, r2) = (e.root.join("L2"), e.root.join("R2"));
    fs::create_dir_all(&l2).unwrap();
    fs::create_dir_all(&r2).unwrap();
    write_jobs(&e.d, &json!([interval_job("pierwsze", &e.l, &e.r)]));
    let _dm = spawn_daemon(&e.d);
    fs::write(e.l.join("a.txt"), "1").unwrap();
    assert!(wait_for(Duration::from_secs(20), || e
        .r
        .join("a.txt")
        .exists()));
    std::thread::sleep(Duration::from_millis(1100)); // mtime ma rozdzielczość
    atomic_write_jobs(
        &e.d,
        &json!([
            interval_job("pierwsze", &e.l, &e.r),
            interval_job("drugie", &l2, &r2)
        ]),
    );
    fs::write(l2.join("b.txt"), "2").unwrap();
    // przeładowanie co 15 s + start zadania
    assert!(
        wait_for(Duration::from_secs(40), || r2.join("b.txt").exists()),
        "daemon nie przeładował nowego zadania"
    );
}

#[test]
fn daemon_keeps_running_jobs_when_jobs_json_becomes_corrupt() {
    let e = env();
    write_jobs(&e.d, &json!([interval_job("odstep", &e.l, &e.r)]));
    let _dm = spawn_daemon(&e.d);
    fs::write(e.l.join("a.txt"), "1").unwrap();
    assert!(wait_for(Duration::from_secs(20), || e
        .r
        .join("a.txt")
        .exists()));
    std::thread::sleep(Duration::from_millis(1100));
    fs::write(e.d.join("jobs.json"), "[{\"id\": \"odst").unwrap(); // ucięty plik
    std::thread::sleep(Duration::from_secs(20)); // przeładowanie
    fs::write(e.l.join("po_uszkodzeniu.txt"), "2").unwrap();
    assert!(
        wait_for(Duration::from_secs(25), || e
            .r
            .join("po_uszkodzeniu.txt")
            .exists()),
        "daemon przestał synchronizować po uszkodzeniu konfiguracji"
    );
}

#[test]
fn parallel_runs_of_same_job_do_not_corrupt_or_panic() {
    let e = env();
    write_jobs(&e.d, &json!([job("rownolegle", "Rown", &e.l, &e.r)]));
    for i in 0..30 {
        fs::write(
            e.l.join(format!("plik{i}.txt")),
            format!("zawartość {i} ąę"),
        )
        .unwrap();
    }
    let kids: Vec<_> = (0..6)
        .map(|_| {
            bin()
                .args(["--data-dir", e.d.to_str().unwrap(), "run", "Rown"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut failures = vec![];
    for k in kids {
        let o = k.wait_with_output().unwrap();
        if !matches!(o.status.code(), Some(0) | Some(2)) {
            failures.push(format!("{:?}: {}", o.status.code(), err_s(&o)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    for i in 0..30 {
        assert_eq!(
            fs::read_to_string(e.r.join(format!("plik{i}.txt"))).unwrap(),
            format!("zawartość {i} ąę")
        );
    }
    // żadnych osieroconych plików tymczasowych w celu
    let tmp: Vec<_> = snap(&e.r)
        .into_keys()
        .filter(|k| k.contains(".tmp") || k.contains(".part"))
        .collect();
    assert!(tmp.is_empty(), "{tmp:?}");
    // kolejny przebieg nic nie robi
    let o = run(&e.d, &["run", "Rown"]);
    assert!(out_s(&o).contains("skopiowano 0"), "{}", out_s(&o));
}

#[test]
fn parallel_import_does_not_lose_jobs_or_error() {
    let e = env();
    let kids: Vec<_> = (0..6)
        .map(|i| {
            let f = e.root.join(format!("in{i}.json"));
            fs::write(
                &f,
                json!([job(&format!("id{i}"), &format!("Zad{i}"), &e.l, &e.r)]).to_string(),
            )
            .unwrap();
            bin()
                .args([
                    "--data-dir",
                    e.d.to_str().unwrap(),
                    "import",
                    f.to_str().unwrap(),
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let errs: Vec<String> = kids
        .into_iter()
        .map(|k| k.wait_with_output().unwrap())
        .filter(|o| !o.status.success())
        .map(|o| err_s(&o))
        .collect();
    // plik konfiguracji musi pozostać poprawnym JSON-em
    let v: Value =
        serde_json::from_str(&fs::read_to_string(e.d.join("jobs.json")).unwrap()).unwrap();
    assert!(v.is_array());
    assert!(errs.is_empty(), "błędy równoległego importu: {errs:?}");
}
