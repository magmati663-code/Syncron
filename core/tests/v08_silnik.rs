//! Syncron 0.8: nowe pola zadania i zachowania silnika (kontrakt A).
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{run_job, run_job_opts, RunOpts};
use syncron_core::model::{parse_import, Job};
use syncron_core::plan::{
    plan, plan_ext, Action, Cmp, Conflict, CustomAction, CustomRules, Meta, Mode, Options, PlanExt,
    Tree,
};

struct Env {
    _t: tempfile::TempDir,
    l: PathBuf,
    r: PathBuf,
    s: PathBuf,
}

fn setup() -> Env {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    Env { _t: t, l, r, s }
}

fn job(e: &Env, mode: &str, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "v08", "name": "v08", "left": e.l.to_str().unwrap(), "right": e.r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn write(p: &Path, c: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn m(size: u64, t: i64) -> Meta {
    Meta { size, mtime: t }
}

fn tree(v: &[(&str, Meta)]) -> Tree {
    v.iter().map(|(k, x)| (k.to_string(), *x)).collect()
}

fn opts(mode: Mode) -> Options {
    Options {
        mode,
        no_delete_left: false,
        no_delete_right: false,
        only_add: false,
        conflict: Conflict::Newer,
    }
}

// ---------- model ----------

#[test]
fn stary_jobs_json_bez_nowych_pol() {
    let old = r#"[{"id":"1","name":"Stare zadanie","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]"#;
    let jobs: Vec<Job> = serde_json::from_str(old).unwrap();
    let j = &jobs[0];
    assert_eq!(format!("{:?}", j.compare), "TimeAndSize");
    assert_eq!(j.time_tolerance_secs, 2);
    assert_eq!(j.time_shift_hours, 0);
    assert!(!j.ignore_hidden && !j.preserve_permissions);
    assert!(j.detect_moves);
    assert_eq!(j.min_age_days, None);
    assert_eq!(j.max_age_days, None);
    assert!(j.custom.is_none() && j.run_before.is_none() && j.run_after.is_none());
    assert_eq!(j.run_timeout_secs, 300);
    j.validate().unwrap();
    // zapis i ponowny odczyt zachowują pola
    let back: Job = serde_json::from_str(&serde_json::to_string(j).unwrap()).unwrap();
    assert_eq!(back.run_timeout_secs, 300);
    assert!(back.detect_moves);
}

#[test]
fn kontrakt_json_nowe_pola_i_tryb_wlasny() {
    let j: Job = serde_json::from_value(serde_json::json!({
        "id": "1", "name": "Własny", "left": "/a", "right": "/b", "mode": "Custom",
        "conflict": "Newer", "schedule": {"type": "Manual"},
        "compare": "Content", "time_tolerance_secs": 5, "time_shift_hours": 1,
        "symlinks": "Follow", "ignore_hidden": true, "min_age_days": 1, "max_age_days": 30,
        "detect_moves": false, "preserve_permissions": true, "versioning": "Replace",
        "custom": {"left_only": "CopyToRight", "right_only": "DeleteRight", "left_newer": "CopyToRight",
                   "right_newer": "CopyToLeft", "different": "Skip"},
        "run_before": "echo a", "run_after": "echo b", "run_timeout_secs": 10
    }))
    .unwrap();
    j.validate().unwrap();
    assert_eq!(j.mode, Mode::Custom);
    let c = j.custom.unwrap();
    assert_eq!(c.right_only, CustomAction::DeleteRight);
    assert_eq!(c.different, CustomAction::Skip);
}

#[test]
fn walidacja_zakresow() {
    let base = |extra: serde_json::Value| {
        let e = setup();
        job(&e, "TwoWay", extra)
    };
    assert!(base(serde_json::json!({})).validate().is_ok());
    for (extra, frag) in [
        (
            serde_json::json!({"time_tolerance_secs": 86401}),
            "Tolerancja",
        ),
        (serde_json::json!({"time_shift_hours": 24}), "Przesunięcie"),
        (serde_json::json!({"min_age_days": 40000}), "wiek"),
        (serde_json::json!({"max_age_days": 40000}), "wiek"),
        (
            serde_json::json!({"min_age_days": 10, "max_age_days": 5}),
            "większy",
        ),
        (serde_json::json!({"run_timeout_secs": 0}), "Limit czasu"),
        (
            serde_json::json!({"run_timeout_secs": 86401}),
            "Limit czasu",
        ),
        (
            serde_json::json!({"run_before": "a\u{0}b"}),
            "przed synchronizacją",
        ),
    ] {
        let err = base(extra.clone()).validate().unwrap_err();
        assert!(err.contains(frag), "{extra}: {err}");
    }
    // granice dozwolone
    assert!(base(serde_json::json!({"time_tolerance_secs": 86400, "time_shift_hours": 23, "run_timeout_secs": 86400, "min_age_days": 5, "max_age_days": 5})).validate().is_ok());
}

#[test]
fn walidacja_trybu_wlasnego() {
    let e = setup();
    let err = job(&e, "Custom", serde_json::json!({}))
        .validate()
        .unwrap_err();
    assert!(err.contains("Tryb własny"), "{err}");
    let ok = job(
        &e,
        "Custom",
        serde_json::json!({"custom": {"left_only": "CopyToRight", "right_only": "CopyToLeft"}}),
    );
    ok.validate().unwrap();
    for custom in [
        serde_json::json!({"left_only": "CopyToLeft"}),
        serde_json::json!({"left_only": "DeleteRight"}),
        serde_json::json!({"right_only": "CopyToRight"}),
        serde_json::json!({"right_only": "DeleteLeft"}),
    ] {
        let j = job(&e, "Custom", serde_json::json!({ "custom": custom }));
        assert!(j.validate().is_err(), "{custom}");
    }
}

#[test]
fn import_czysci_polecenia_i_wylacza() {
    let e = setup();
    let mut j = job(
        &e,
        "TwoWay",
        serde_json::json!({"run_before": "echo x", "run_after": "rm -rf /tmp/nic", "run_timeout_secs": 7}),
    );
    j.enabled = true;
    let text = format!("\u{feff}{}", serde_json::to_string(&vec![j]).unwrap());
    let out = parse_import(&text).unwrap();
    assert!(out[0].run_before.is_none() && out[0].run_after.is_none());
    assert!(!out[0].enabled);
    assert_eq!(out[0].run_timeout_secs, 7);
}

// ---------- planer ----------

#[test]
fn tolerancja_czasu_z_zadania() {
    let l = tree(&[("a", m(1, 100))]);
    let r = tree(&[("a", m(1, 108))]);
    // domyślnie 2 s: różne (nowszy lewy/prawy, brak bazy → kopiowanie)
    assert!(!plan(&l, &r, &Tree::new(), &Tree::new(), opts(Mode::TwoWay)).is_empty());
    let ext = |tol| PlanExt {
        cmp: Cmp {
            tol_secs: tol,
            ..Cmp::default()
        },
        custom: CustomRules::default(),
    };
    let none = Tree::new();
    assert!(plan_ext(&l, &r, &none, &none, opts(Mode::TwoWay), ext(8)).is_empty());
    assert!(!plan_ext(&l, &r, &none, &none, opts(Mode::TwoWay), ext(7)).is_empty());
    // tolerancja 0: jedna sekunda to już różnica
    let r1 = tree(&[("a", m(1, 101))]);
    assert!(!plan_ext(&l, &r1, &none, &none, opts(Mode::TwoWay), ext(0)).is_empty());
}

#[test]
fn przesuniecie_czasu_godzinowe() {
    let none = Tree::new();
    let l = tree(&[("a", m(5, 1_000_000))]);
    let ext = |shift| PlanExt {
        cmp: Cmp {
            tol_secs: 2,
            shift_hours: shift,
            size_only: false,
        },
        custom: CustomRules::default(),
    };
    let p = |dt: i64, shift: u32| {
        let r = tree(&[("a", m(5, 1_000_000 + dt))]);
        plan_ext(&l, &r, &none, &none, opts(Mode::TwoWay), ext(shift)).is_empty()
    };
    assert!(p(3600, 1)); // dokładnie 1 h
    assert!(p(-3600, 1)); // w drugą stronę
    assert!(p(3601, 1)); // 1 h + 1 s: w tolerancji
    assert!(!p(3603, 1)); // poza tolerancją
    assert!(!p(7200, 1)); // 2 h przy limicie 1
    assert!(p(7200, 2));
    assert!(!p(3600, 0)); // wyłączone
    assert!(!p(1800, 23)); // pół godziny to nie pełna godzina
    assert!(p(23 * 3600, 23));
    assert!(!p(24 * 3600, 23));
    // różny rozmiar zawsze różny
    let r = tree(&[("a", m(6, 1_000_000 + 3600))]);
    assert!(!plan_ext(&l, &r, &none, &none, opts(Mode::TwoWay), ext(1)).is_empty());
}

#[test]
fn tryb_wlasny_pieciu_kategorii_w_planerze() {
    use CustomAction::*;
    let rules = CustomRules {
        left_only: CopyToRight,
        right_only: DeleteRight,
        left_newer: CopyToRight,
        right_newer: CopyToLeft,
        different: DeleteLeft,
    };
    let ext = PlanExt {
        cmp: Cmp::default(),
        custom: rules,
    };
    let l = tree(&[
        ("tylko_lewy", m(1, 10)),
        ("lnowszy", m(2, 100)),
        ("pnowszy", m(2, 10)),
        ("rozne", m(1, 50)),
        ("takie_same", m(3, 5)),
    ]);
    let r = tree(&[
        ("tylko_prawy", m(1, 10)),
        ("lnowszy", m(3, 50)),
        ("pnowszy", m(3, 100)),
        ("rozne", m(9, 50)),
        ("takie_same", m(3, 5)),
    ]);
    let n = Tree::new();
    let a = plan_ext(&l, &r, &n, &n, opts(Mode::Custom), ext);
    assert_eq!(a["tylko_lewy"], Action::CopyToRight);
    assert_eq!(a["tylko_prawy"], Action::DeleteRight);
    assert_eq!(a["lnowszy"], Action::CopyToRight);
    assert_eq!(a["pnowszy"], Action::CopyToLeft);
    assert_eq!(a["rozne"], Action::DeleteLeft);
    assert!(!a.contains_key("takie_same"));
    // bez rozszerzeń (domyślne reguły = pomiń) plan trybu własnego jest pusty
    assert!(plan(&l, &r, &n, &n, opts(Mode::Custom)).is_empty());
    // no_delete_*: usunięcia odrzucane
    let mut o = opts(Mode::Custom);
    o.no_delete_right = true;
    o.no_delete_left = true;
    let a = plan_ext(&l, &r, &n, &n, o, ext);
    assert!(!a.contains_key("tylko_prawy") && !a.contains_key("rozne"));
    // only_add: tylko kopiowanie na brakujące miejsce
    let mut o = opts(Mode::Custom);
    o.only_add = true;
    let a = plan_ext(&l, &r, &n, &n, o, ext);
    // tylko plik brakujący po drugiej stronie może zostać skopiowany
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(a.contains_key("tylko_lewy"));
}

#[test]
fn tryb_wlasny_akcje_bez_zrodla_sa_pomijane() {
    use CustomAction::*;
    // nonsens (kopiuj w lewo plik tylko po lewej) nie powoduje paniki ani działania
    let ext = PlanExt {
        cmp: Cmp::default(),
        custom: CustomRules {
            left_only: CopyToLeft,
            right_only: CopyToRight,
            ..CustomRules::default()
        },
    };
    let l = tree(&[("a", m(1, 1))]);
    let r = tree(&[("b", m(1, 1))]);
    let n = Tree::new();
    assert!(plan_ext(&l, &r, &n, &n, opts(Mode::Custom), ext).is_empty());
}

// ---------- silnik: tryb własny ----------

#[test]
fn silnik_tryb_wlasny_z_koszem() {
    let e = setup();
    write(&e.l.join("Zażółć/gęślą.txt"), "lewy");
    write(&e.r.join("Łódź/tylko prawy.txt"), "prawy");
    write(&e.l.join("nowszy_lewy.txt"), "L-nowy");
    write(&e.r.join("nowszy_lewy.txt"), "R-stary");
    set_mtime(&e.l.join("nowszy_lewy.txt"), 2_000_000_000);
    set_mtime(&e.r.join("nowszy_lewy.txt"), 1_900_000_000);
    write(&e.l.join("nowszy_prawy.txt"), "L-stary");
    write(&e.r.join("nowszy_prawy.txt"), "R-nowy");
    set_mtime(&e.l.join("nowszy_prawy.txt"), 1_900_000_000);
    set_mtime(&e.r.join("nowszy_prawy.txt"), 2_000_000_000);
    write(&e.l.join("rozne.txt"), "aaa");
    write(&e.r.join("rozne.txt"), "bbbbbb");
    set_mtime(&e.l.join("rozne.txt"), 1_950_000_000);
    set_mtime(&e.r.join("rozne.txt"), 1_950_000_000);
    let j = job(
        &e,
        "Custom",
        serde_json::json!({"custom": {"left_only": "CopyToRight", "right_only": "DeleteRight",
            "left_newer": "CopyToRight", "right_newer": "CopyToLeft", "different": "Skip"}, "guard_pct": 100}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(
        fs::read_to_string(e.r.join("Zażółć/gęślą.txt")).unwrap(),
        "lewy"
    );
    assert!(!e.r.join("Łódź/tylko prawy.txt").exists());
    assert_eq!(
        fs::read_to_string(e.r.join("nowszy_lewy.txt")).unwrap(),
        "L-nowy"
    );
    assert_eq!(
        fs::read_to_string(e.l.join("nowszy_prawy.txt")).unwrap(),
        "R-nowy"
    );
    // „different” = pomiń
    assert_eq!(fs::read_to_string(e.l.join("rozne.txt")).unwrap(), "aaa");
    assert_eq!(fs::read_to_string(e.r.join("rozne.txt")).unwrap(), "bbbbbb");
    // usunięty plik trafił do kosza prawej strony
    let trash: Vec<_> = walk(&e.r.join(".syncron-trash"));
    assert!(
        trash.iter().any(|p| p.ends_with("Łódź/tylko prawy.txt")),
        "{trash:?}"
    );
    // stara wersja nadpisanego pliku też w koszu
    assert!(
        trash.iter().any(|p| p.ends_with("nowszy_lewy.txt")),
        "{trash:?}"
    );
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut st = vec![dir.to_path_buf()];
    while let Some(d) = st.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                st.push(p);
            } else {
                out.push(p.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn silnik_tryb_wlasny_straznik_i_no_delete() {
    let e = setup();
    for i in 0..10 {
        write(&e.r.join(format!("prawy{i}.txt")), "x");
    }
    write(&e.l.join("jeden.txt"), "x");
    let rules = serde_json::json!({"left_only": "CopyToRight", "right_only": "DeleteRight"});
    // blokada masowego usuwania (10 z 11 plików)
    let j = job(
        &e,
        "Custom",
        serde_json::json!({"custom": rules, "guard_pct": 25}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert!(e.r.join("prawy0.txt").exists() && !e.r.join("jeden.txt").exists());
    // no_delete_right: nic nie usuwamy, nowy plik kopiujemy
    let j = job(
        &e,
        "Custom",
        serde_json::json!({"custom": rules, "no_delete_right": true, "guard_pct": 100}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.deleted, 0, "{rep:?}");
    assert!(e.r.join("prawy0.txt").exists() && e.r.join("jeden.txt").exists());
    // wymuszenie omija blokadę
    let j = job(
        &e,
        "Custom",
        serde_json::json!({"custom": rules, "guard_pct": 25}),
    );
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rep.deleted, 10, "{rep:?}");
}

#[test]
fn silnik_tryb_wlasny_only_add() {
    let e = setup();
    write(&e.l.join("nowy.txt"), "n");
    write(&e.r.join("prawy.txt"), "p");
    let j = job(
        &e,
        "Custom",
        serde_json::json!({"only_add": true, "guard_pct": 100,
            "custom": {"left_only": "CopyToRight", "right_only": "DeleteRight"}}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 1);
    assert!(e.r.join("prawy.txt").exists());
    assert!(e.r.join("nowy.txt").exists());
}

// ---------- porównywanie ----------

#[test]
fn compare_size_only() {
    let e = setup();
    write(&e.l.join("a.txt"), "abc");
    write(&e.r.join("a.txt"), "xyz");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_800_000_000);
    write(&e.l.join("b.txt"), "dłuższy");
    write(&e.r.join("b.txt"), "b");
    set_mtime(&e.l.join("b.txt"), 1_900_000_000);
    set_mtime(&e.r.join("b.txt"), 1_800_000_000);
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"compare": "SizeOnly"}),
    );
    let rep = run_job(&j, &e.s, true, false).unwrap();
    let keys: Vec<_> = rep.planned.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["b.txt"], "{rep:?}");
    // domyślne porównanie widzi obie różnice
    let d = job(&e, "LeftToRight", serde_json::json!({}));
    assert_eq!(run_job(&d, &e.s, true, false).unwrap().planned.len(), 2);
}

#[test]
fn compare_content_lokalne() {
    let e = setup();
    // taka sama treść, różne czasy: bez działań
    write(&e.l.join("Zażółć.txt"), "ta sama treść");
    write(&e.r.join("Zażółć.txt"), "ta sama treść");
    set_mtime(&e.l.join("Zażółć.txt"), 1_900_000_000);
    set_mtime(&e.r.join("Zażółć.txt"), 1_800_000_000);
    // ten sam rozmiar, inna treść, lewy nowszy: kopiowanie
    write(&e.l.join("inna.txt"), "AAAA");
    write(&e.r.join("inna.txt"), "BBBB");
    set_mtime(&e.l.join("inna.txt"), 1_900_000_000);
    set_mtime(&e.r.join("inna.txt"), 1_800_000_000);
    let j = job(&e, "LeftToRight", serde_json::json!({"compare": "Content"}));
    let rep = run_job(&j, &e.s, true, false).unwrap();
    let keys: Vec<_> = rep.planned.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["inna.txt"], "{rep:?}");
    // domyślnie oba pliki są „różne” (czas)
    let d = job(&e, "LeftToRight", serde_json::json!({}));
    assert_eq!(run_job(&d, &e.s, true, false).unwrap().planned.len(), 2);
    // pełny przebieg i idempotencja
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(fs::read_to_string(e.r.join("inna.txt")).unwrap(), "AAAA");
    let again = run_job(&j, &e.s, false, false).unwrap();
    assert!(again.planned.is_empty(), "{again:?}");
}

#[test]
fn compare_content_dwukierunkowo_bez_pingpongu() {
    let e = setup();
    write(&e.l.join("a.txt"), "xx");
    write(&e.r.join("a.txt"), "xx");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_700_000_000);
    let j = job(&e, "TwoWay", serde_json::json!({"compare": "Content"}));
    for _ in 0..3 {
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert!(
            rep.planned.is_empty() && rep.conflicts.is_empty(),
            "{rep:?}"
        );
    }
    // zmiana treści po prawej (ten sam rozmiar) jest wykryta i propagowana
    write(&e.r.join("a.txt"), "yy");
    set_mtime(&e.r.join("a.txt"), 1_950_000_000);
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(fs::read_to_string(e.l.join("a.txt")).unwrap(), "yy");
}

#[test]
fn tolerancja_i_przesuniecie_w_silniku() {
    let e = setup();
    write(&e.l.join("a.txt"), "abc");
    write(&e.r.join("a.txt"), "abc");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_900_000_000 + 3600);
    let plain = job(&e, "LeftToRight", serde_json::json!({}));
    assert_eq!(run_job(&plain, &e.s, true, false).unwrap().planned.len(), 1);
    let shifted = job(
        &e,
        "LeftToRight",
        serde_json::json!({"time_shift_hours": 1}),
    );
    assert!(run_job(&shifted, &e.s, true, false)
        .unwrap()
        .planned
        .is_empty());
    set_mtime(&e.r.join("a.txt"), 1_900_000_000 + 30);
    let tol = job(
        &e,
        "LeftToRight",
        serde_json::json!({"time_tolerance_secs": 30}),
    );
    assert!(run_job(&tol, &e.s, true, false).unwrap().planned.is_empty());
}

// ---------- wykrywanie przeniesień ----------

#[test]
fn detect_moves_wlaczone_i_wylaczone() {
    for detect in [true, false] {
        let e = setup();
        write(
            &e.l.join("stara nazwa.bin"),
            "zawartość pliku do przeniesienia",
        );
        set_mtime(&e.l.join("stara nazwa.bin"), 1_900_000_000);
        let j = job(
            &e,
            "TwoWay",
            serde_json::json!({"detect_moves": detect, "guard_pct": 100}),
        );
        run_job(&j, &e.s, false, false).unwrap();
        fs::rename(e.l.join("stara nazwa.bin"), e.l.join("nowa nazwa.bin")).unwrap();
        let rep = run_job(&j, &e.s, true, false).unwrap();
        let has_rename = rep
            .planned
            .iter()
            .any(|(_, a)| matches!(a, Action::RenameRight { .. }));
        assert_eq!(has_rename, detect, "{rep:?}");
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert_eq!(rep.moved, usize::from(detect), "{rep:?}");
        assert!(e.r.join("nowa nazwa.bin").exists());
        assert!(!e.r.join("stara nazwa.bin").exists());
    }
}

// ---------- filtry ----------

#[test]
fn ignore_hidden() {
    let e = setup();
    write(&e.l.join(".ukryty"), "x");
    write(&e.l.join(".folder/w środku.txt"), "x");
    write(&e.l.join("jawny/.cichy"), "x");
    write(&e.l.join("jawny/widoczny.txt"), "x");
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"ignore_hidden": true}),
    );
    run_job(&j, &e.s, false, false).unwrap();
    assert!(e.r.join("jawny/widoczny.txt").exists());
    assert!(!e.r.join(".ukryty").exists());
    assert!(!e.r.join(".folder").exists());
    assert!(!e.r.join("jawny/.cichy").exists());
    // domyślnie ukryte też się synchronizują
    let e2 = setup();
    write(&e2.l.join(".ukryty"), "x");
    let j = job(&e2, "LeftToRight", serde_json::json!({}));
    run_job(&j, &e2.s, false, false).unwrap();
    assert!(e2.r.join(".ukryty").exists());
}

#[test]
fn wiek_plikow_min_max() {
    let e = setup();
    let t = now();
    write(&e.l.join("stary.txt"), "s");
    set_mtime(&e.l.join("stary.txt"), t - 100 * 86400);
    write(&e.l.join("sredni.txt"), "s");
    set_mtime(&e.l.join("sredni.txt"), t - 10 * 86400);
    write(&e.l.join("nowy.txt"), "n");
    let keys = |extra: serde_json::Value| {
        let j = job(&e, "LeftToRight", extra);
        let rep = run_job(&j, &e.s, true, false).unwrap();
        let mut v: Vec<String> = rep.planned.iter().map(|(k, _)| k.clone()).collect();
        v.sort();
        v
    };
    assert_eq!(
        keys(serde_json::json!({})),
        ["nowy.txt", "sredni.txt", "stary.txt"]
    );
    // pomijaj młodsze niż 30 dni
    assert_eq!(keys(serde_json::json!({"min_age_days": 30})), ["stary.txt"]);
    // pomijaj starsze niż 30 dni
    assert_eq!(
        keys(serde_json::json!({"max_age_days": 30})),
        ["nowy.txt", "sredni.txt"]
    );
    // okno: 5..=30 dni
    assert_eq!(
        keys(serde_json::json!({"min_age_days": 5, "max_age_days": 30})),
        ["sredni.txt"]
    );
    // min_age_days = 0 nic nie wycina
    assert_eq!(keys(serde_json::json!({"min_age_days": 0})).len(), 3);
}

#[cfg(target_os = "linux")]
#[test]
fn symlinks_skip_i_follow_tylko_odczyt() {
    use std::os::unix::fs::symlink;
    let e = setup();
    let outside = e._t.path().join("poza");
    write(&outside.join("sekret.txt"), "dane spoza");
    write(&e.l.join("zwykly.txt"), "z");
    symlink(outside.join("sekret.txt"), e.l.join("link-plik")).unwrap();
    symlink(&outside, e.l.join("link-folder")).unwrap();
    symlink(e.l.join("nie-istnieje"), e.l.join("zepsuty")).unwrap();
    // pętla: dowiązanie do własnego rodzica
    symlink(&e.l, e.l.join("petla")).unwrap();

    let skip = job(&e, "LeftToRight", serde_json::json!({}));
    run_job(&skip, &e.s, false, false).unwrap();
    assert!(e.r.join("zwykly.txt").exists());
    assert!(!e.r.join("link-plik").exists() && !e.r.join("link-folder").exists());

    let e2 = setup();
    write(&e2.l.join("zwykly.txt"), "z");
    symlink(outside.join("sekret.txt"), e2.l.join("link-plik")).unwrap();
    symlink(&outside, e2.l.join("link-folder")).unwrap();
    symlink(e2.l.join("nie-istnieje"), e2.l.join("zepsuty")).unwrap();
    symlink(&e2.l, e2.l.join("petla")).unwrap();
    let follow = job(
        &e2,
        "LeftToRight",
        serde_json::json!({"symlinks": "Follow"}),
    );
    let rep = run_job(&follow, &e2.s, false, false).unwrap();
    assert!(rep.errors.is_empty(), "{rep:?}");
    // w celu powstają zwykłe pliki z treścią, nie dowiązania
    assert_eq!(
        fs::read_to_string(e2.r.join("link-plik")).unwrap(),
        "dane spoza"
    );
    assert_eq!(
        fs::read_to_string(e2.r.join("link-folder/sekret.txt")).unwrap(),
        "dane spoza"
    );
    assert!(!fs::symlink_metadata(e2.r.join("link-plik"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!e2.r.join("zepsuty").exists());
    // pętla nie powoduje nieskończonego skanowania
    assert!(!e2.r.join("petla/petla").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn symlinks_follow_nie_pozwala_pisac_przez_dowiazanie() {
    use std::os::unix::fs::symlink;
    let e = setup();
    let outside = e._t.path().join("poza");
    write(&outside.join("istniejacy.txt"), "stary");
    // prawa strona ma dowiązanie do folderu spoza korzenia
    symlink(&outside, e.r.join("lnk")).unwrap();
    write(&e.l.join("lnk/nowy.txt"), "nie wolno tu zapisać");
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"symlinks": "Follow", "guard_pct": 100}),
    );
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert!(
        !outside.join("nowy.txt").exists(),
        "zapis przez dowiązanie wyprowadził poza korzeń: {rep:?}"
    );
    assert!(
        !rep.errors.is_empty(),
        "oczekiwano błędu odmowy zapisu: {rep:?}"
    );
    // plik spoza korzenia nie został usunięty ani zmieniony
    assert_eq!(
        fs::read_to_string(outside.join("istniejacy.txt")).unwrap(),
        "stary"
    );
}

// ---------- uprawnienia i wersje ----------

#[cfg(unix)]
#[test]
fn preserve_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let e = setup();
    write(&e.l.join("skrypt.sh"), "#!/bin/sh\n");
    fs::set_permissions(e.l.join("skrypt.sh"), fs::Permissions::from_mode(0o750)).unwrap();
    write(&e.l.join("prywatny.txt"), "p");
    fs::set_permissions(e.l.join("prywatny.txt"), fs::Permissions::from_mode(0o600)).unwrap();
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"preserve_permissions": true}),
    );
    run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(mode(&e.r.join("skrypt.sh")), 0o750);
    assert_eq!(mode(&e.r.join("prywatny.txt")), 0o600);
    // domyślnie uprawnienia nie są kopiowane
    let e2 = setup();
    write(&e2.l.join("skrypt.sh"), "#!/bin/sh\n");
    fs::set_permissions(e2.l.join("skrypt.sh"), fs::Permissions::from_mode(0o750)).unwrap();
    let j = job(&e2, "LeftToRight", serde_json::json!({}));
    run_job(&j, &e2.s, false, false).unwrap();
    assert_ne!(mode(&e2.r.join("skrypt.sh")), 0o750);
}

#[cfg(unix)]
#[test]
fn preserve_permissions_nie_przenosi_setuid() {
    use std::os::unix::fs::PermissionsExt;
    let e = setup();
    write(&e.l.join("suid"), "x");
    fs::set_permissions(e.l.join("suid"), fs::Permissions::from_mode(0o4755)).unwrap();
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"preserve_permissions": true}),
    );
    run_job(&j, &e.s, false, false).unwrap();
    let m = fs::metadata(e.r.join("suid")).unwrap().permissions().mode();
    assert_eq!(m & 0o7000, 0, "setuid nie może być kopiowany: {m:o}");
    assert_eq!(m & 0o777, 0o755);
}

fn versions_of(side: &Path, name: &str) -> usize {
    walk(&side.join(".syncron-trash"))
        .iter()
        .filter(|p| p.ends_with(&format!("/{name}")))
        .count()
}

#[test]
fn versioning_folders_vs_replace() {
    for (style, expected) in [("Folders", 3usize), ("Replace", 1usize)] {
        let e = setup();
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({"versioning": style, "trash_days": 30}),
        );
        write(&e.l.join("dokument żółty.txt"), "v1");
        set_mtime(&e.l.join("dokument żółty.txt"), 1_900_000_000);
        run_job(&j, &e.s, false, false).unwrap();
        for (i, v) in ["v2", "v3", "v4"].iter().enumerate() {
            write(&e.l.join("dokument żółty.txt"), v);
            set_mtime(
                &e.l.join("dokument żółty.txt"),
                1_900_000_100 + i as i64 * 100,
            );
            let rep = run_job(&j, &e.s, false, false).unwrap();
            assert!(rep.errors.is_empty(), "{rep:?}");
        }
        assert_eq!(
            versions_of(&e.r, "dokument żółty.txt"),
            expected,
            "{style}: {:?}",
            walk(&e.r.join(".syncron-trash"))
        );
        assert_eq!(
            fs::read_to_string(e.r.join("dokument żółty.txt")).unwrap(),
            "v4"
        );
        if style == "Replace" {
            // w koszu jest wersja bezpośrednio poprzednia (v3)
            let f = walk(&e.r.join(".syncron-trash"));
            assert_eq!(fs::read_to_string(&f[0]).unwrap(), "v3");
        }
    }
}

#[test]
fn versioning_replace_usuniecie_pliku() {
    let e = setup();
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"versioning": "Replace", "trash_days": 30, "guard_pct": 100}),
    );
    write(&e.l.join("a.txt"), "v1");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    run_job(&j, &e.s, false, false).unwrap();
    write(&e.l.join("a.txt"), "v2");
    set_mtime(&e.l.join("a.txt"), 1_900_000_500);
    run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(versions_of(&e.r, "a.txt"), 1);
    fs::remove_file(e.l.join("a.txt")).unwrap();
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rep.deleted, 1, "{rep:?}");
    // po usunięciu: jedna (najnowsza) kopia, bez wersji v1
    assert_eq!(versions_of(&e.r, "a.txt"), 1);
    let f = walk(&e.r.join(".syncron-trash"));
    assert_eq!(fs::read_to_string(&f[0]).unwrap(), "v2");
}

// ---------- run_job_opts / skip ----------

#[test]
fn planned_zawiera_klucze_i_skip_pomija_akcje() {
    let e = setup();
    write(&e.l.join("Zażółć/gęślą jaźń.txt"), "1");
    write(&e.l.join("b.txt"), "2");
    write(&e.l.join("c.txt"), "3");
    let j = job(&e, "LeftToRight", serde_json::json!({}));
    let dry = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            dry: true,
            ..Default::default()
        },
    )
    .unwrap();
    let keys: Vec<String> = dry.planned.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().any(|k| k.contains("gęślą")), "{keys:?}");
    // pomijamy dwie akcje, w tym ścieżkę z polskimi znakami (klucz z planned)
    let polish = keys.iter().find(|k| k.contains("gęślą")).unwrap().clone();
    let skip: HashSet<String> = [
        polish.clone(),
        "b.txt".to_string(),
        "nie-ma-takiego".to_string(),
    ]
    .into_iter()
    .collect();
    let rep = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            skip,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(rep.skipped, 2);
    assert!(e.r.join("c.txt").exists());
    assert!(!e.r.join("b.txt").exists());
    assert!(!e.r.join("Zażółć").exists());
    // pominięte akcje nadal są w planie następnego zwykłego przebiegu
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert!(e.r.join("b.txt").exists() && e.r.join("Zażółć/gęślą jaźń.txt").exists());
}

#[test]
fn skip_usuniecia_zostaje_w_planie_i_omija_straznika() {
    let e = setup();
    for i in 0..5 {
        write(&e.l.join(format!("p{i}.txt")), "x");
    }
    let j = job(&e, "TwoWay", serde_json::json!({"guard_pct": 25}));
    run_job(&j, &e.s, false, false).unwrap();
    for i in 0..5 {
        fs::remove_file(e.l.join(format!("p{i}.txt"))).unwrap();
    }
    write(&e.l.join("zostaje.txt"), "z");
    // zwykły przebieg: usuwa 5 z 6 plików → blokada
    let blocked = run_job(&j, &e.s, false, false).unwrap();
    assert!(blocked.blocked.is_some(), "{blocked:?}");
    // użytkownik odznaczył 4 usunięcia: zostaje 1 (z ~17%) → bez blokady
    let skip: HashSet<String> = (0..4).map(|i| format!("p{i}.txt")).collect();
    let rep = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            skip,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rep.blocked.is_none(), "{rep:?}");
    assert_eq!(rep.deleted, 1);
    for i in 0..4 {
        assert!(e.r.join(format!("p{i}.txt")).exists());
    }
    assert!(!e.r.join("p4.txt").exists());
    // pominięte usunięcia nie zamieniły się w „przywracanie” - nadal planowane jako usunięcia
    let dry = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            dry: true,
            force: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(dry.planned.len(), 4, "{dry:?}");
    assert!(dry.planned.iter().all(|(_, a)| *a == Action::DeleteRight));
}

#[test]
fn run_opts_dry_z_skip_nie_zmienia_dysku() {
    let e = setup();
    write(&e.l.join("a.txt"), "1");
    let j = job(&e, "LeftToRight", serde_json::json!({}));
    let rep = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            dry: true,
            skip: ["a.txt".to_string()].into_iter().collect(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(rep.dry_run);
    assert_eq!(rep.planned.len(), 1);
    assert!(!e.r.join("a.txt").exists());
}

#[test]
fn puste_foldery_i_stary_stan() {
    let e = setup();
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"compare": "Content", "ignore_hidden": true, "min_age_days": 1}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty() && rep.errors.is_empty(), "{rep:?}");
    let c = job(
        &e,
        "Custom",
        serde_json::json!({"custom": {"left_only": "CopyToRight"}}),
    );
    let rep = run_job(&c, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty() && rep.blocked.is_none(), "{rep:?}");
}

// ---------- polecenia przed/po ----------

#[cfg(unix)]
mod hooks {
    use super::*;
    use syncron_core::hooks::run_hook;

    #[test]
    fn modul_hooks_kod_wyjscia_wyjscie_i_srodowisko() {
        let r = run_hook("echo witaj; echo błąd 1>&2; exit 3", 10, &[]);
        assert_eq!(r.code, Some(3));
        assert!(!r.ok() && !r.timed_out);
        assert!(
            r.output.contains("witaj") && r.output.contains("błąd"),
            "{r:?}"
        );
        assert!(r.describe("przed synchronizacją").contains("kod 3"));
        let r = run_hook("echo $SYNCRON_JOB", 10, &[("SYNCRON_JOB", "Zażółć".into())]);
        assert!(r.ok());
        assert_eq!(r.output, "Zażółć");
        // nieistniejące polecenie → kod 127 z powłoki
        assert_eq!(
            run_hook("polecenie-ktorego-nie-ma-xyz", 10, &[]).code,
            Some(127)
        );
    }

    #[test]
    fn modul_hooks_limit_czasu_zabija_potomkow() {
        let t = std::time::Instant::now();
        // potomek (sleep) trzyma potok: limit i tak ma zadziałać szybko
        let r = run_hook("sleep 30 & sleep 30", 1, &[]);
        assert!(r.timed_out && !r.ok(), "{r:?}");
        assert!(
            t.elapsed().as_secs() < 8,
            "polecenie nie zostało przerwane: {:?}",
            t.elapsed()
        );
        assert!(r.describe("po synchronizacji").contains("limit czasu"));
    }

    #[test]
    fn run_before_ok_i_run_after_ze_zmiennymi() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        write(&e.l.join("b.txt"), "2");
        let flag_before = e._t.path().join("przed.txt");
        let flag_after = e._t.path().join("po.txt");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({
                "name": "Zadanie żółte",
                "run_before": format!("echo przed > '{}'", flag_before.display()),
                "run_after": format!(
                    "echo \"$SYNCRON_JOB|$SYNCRON_COPIED|$SYNCRON_DELETED|$SYNCRON_ERRORS|$SYNCRON_LEFT|$SYNCRON_RIGHT\" > '{}'",
                    flag_after.display()
                ),
            }),
        );
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert!(rep.blocked.is_none() && rep.errors.is_empty(), "{rep:?}");
        assert!(flag_before.exists());
        let after = fs::read_to_string(&flag_after).unwrap();
        let expect = format!("Zadanie żółte|2|0|0|{}|{}\n", e.l.display(), e.r.display());
        assert_eq!(after, expect);
        assert!(rep.hook_after.is_some());
    }

    #[test]
    fn run_before_blad_anuluje_przebieg() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        let flag_after = e._t.path().join("po.txt");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({
                "run_before": "echo nie działa 1>&2; exit 3",
                "run_after": format!("touch '{}'", flag_after.display()),
            }),
        );
        let rep = run_job(&j, &e.s, false, false).unwrap();
        let b = rep.blocked.as_deref().expect("oczekiwano anulowania");
        assert!(
            b.contains("anulowany") && b.contains("kod 3") && b.contains("nie działa"),
            "{b}"
        );
        assert_eq!(rep.copied, 0);
        assert!(!e.r.join("a.txt").exists());
        // po anulowaniu run_after się nie uruchamia
        assert!(!flag_after.exists());
        assert!(!rep.needs_retry());
    }

    #[test]
    fn run_before_limit_czasu_anuluje() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({"run_before": "sleep 30", "run_timeout_secs": 1}),
        );
        let t = std::time::Instant::now();
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert!(t.elapsed().as_secs() < 10);
        assert!(
            rep.blocked.as_deref().unwrap().contains("limit czasu"),
            "{rep:?}"
        );
        assert!(!e.r.join("a.txt").exists());
    }

    #[test]
    fn run_before_nie_uruchamia_sie_w_symulacji() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        let flag = e._t.path().join("flaga.txt");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({
                "run_before": format!("touch '{}'", flag.display()),
                "run_after": format!("touch '{}'", flag.display()),
            }),
        );
        let rep = run_job(&j, &e.s, true, false).unwrap();
        assert_eq!(rep.planned.len(), 1);
        assert!(!flag.exists(), "symulacja nie może uruchamiać poleceń");
    }

    #[test]
    fn run_after_blad_to_ostrzezenie_nie_blad() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({"run_after": "exit 9"}),
        );
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert_eq!(rep.copied, 1);
        assert!(rep.errors.is_empty() && !rep.needs_retry(), "{rep:?}");
        assert!(rep.warnings.iter().any(|w| w.contains("kod 9")), "{rep:?}");
        assert!(e.r.join("a.txt").exists());
        // wynik zapisany w historii
        let h = syncron_core::engine::history(&e.s, Some("v08"), 50).unwrap();
        assert!(h
            .iter()
            .any(|r| r.kind == "polecenie po" && r.detail.contains("kod 9")));
    }

    #[test]
    fn run_after_uruchamia_sie_gdy_zablokowano_straznikiem() {
        let e = setup();
        for i in 0..5 {
            write(&e.r.join(format!("p{i}.txt")), "x");
        }
        write(&e.l.join("n.txt"), "x");
        let flag = e._t.path().join("po.txt");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({"guard_pct": 25, "run_after": format!("touch '{}'", flag.display())}),
        );
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert!(rep.blocked.is_some());
        assert!(flag.exists());
    }

    #[test]
    fn puste_polecenie_jest_ignorowane() {
        let e = setup();
        write(&e.l.join("a.txt"), "1");
        let j = job(
            &e,
            "LeftToRight",
            serde_json::json!({"run_before": "   ", "run_after": ""}),
        );
        let rep = run_job(&j, &e.s, false, false).unwrap();
        assert_eq!(rep.copied, 1);
        assert!(rep.hook_before.is_none() && rep.hook_after.is_none());
    }
}
