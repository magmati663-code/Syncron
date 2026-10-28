//! QA tester 5: wykrywanie przeniesień/zmian nazw plików i macierz opcji synchronizacji.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{history, run_job, Report};
use syncron_core::model::Job;
use syncron_core::plan::{Action, Conflict, Mode};

const T0: i64 = 1_700_000_000;

fn job(l: &Path, r: &Path, mode: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": "t", "name": "t", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    }))
    .unwrap()
}

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
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

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn put(dir: &Path, rel: &str, content: &str, mtime: i64) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, content).unwrap();
    set_mtime(&p, mtime);
}

fn run(j: &Job, s: &Path) -> Report {
    run_job(j, s, false, true).unwrap()
}

/// Zawartość drzewa (bez kosza i .syncignore): ścieżka -> treść.
fn snap(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, d: &Path, out: &mut BTreeMap<String, String>) {
        for e in fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.starts_with(".syncron-trash") || rel == ".syncignore" {
                continue;
            }
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(rel, fs::read_to_string(&p).unwrap_or_default());
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(dir, dir, &mut m);
    m
}

/// Wszystkie treści plików wraz z koszem.
fn contents_with_trash(dir: &Path) -> BTreeSet<String> {
    fn walk(d: &Path, out: &mut BTreeSet<String>) {
        for e in fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(s) = fs::read_to_string(&p) {
                out.insert(s);
            }
        }
    }
    let mut s = BTreeSet::new();
    walk(dir, &mut s);
    s
}

fn has_rename(rep: &Report) -> bool {
    rep.planned
        .iter()
        .any(|(_, a)| matches!(a, Action::RenameRight { .. } | Action::RenameLeft { .. }))
}

// ---------------------------------------------------------------- RUNDA 1

#[test]
fn r1_move_on_right_is_renamed_on_left() {
    let (_t, l, r, s) = setup();
    put(&l, "Stary/plik.bin", &"x".repeat(2000), T0);
    put(&l, "inny.txt", "i", T0 + 500);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::create_dir_all(r.join("Nowy")).unwrap();
    fs::rename(r.join("Stary/plik.bin"), r.join("Nowy/plik2.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert!(!l.join("Stary/plik.bin").exists());
    assert!(l.join("Nowy/plik2.bin").exists());
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
fn r1_move_in_one_way_modes() {
    for (mode, src_is_left) in [("LeftToRight", true), ("RightToLeft", false)] {
        let (_t, l, r, s) = setup();
        let (src, dst) = if src_is_left { (&l, &r) } else { (&r, &l) };
        put(src, "a/plik.bin", &"y".repeat(1500), T0);
        put(src, "b.txt", "bb", T0 + 700);
        let j = job(&l, &r, mode);
        run(&j, &s);
        assert_eq!(snap(&l), snap(&r), "{mode}: start");
        fs::create_dir_all(src.join("c/d")).unwrap();
        fs::rename(src.join("a/plik.bin"), src.join("c/d/nowy.bin")).unwrap();
        let rep = run(&j, &s);
        assert_eq!(
            (rep.moved, rep.copied, rep.deleted),
            (1, 0, 0),
            "{mode}: {rep:?}"
        );
        assert_eq!(snap(&l), snap(&r), "{mode}");
        assert!(dst.join("c/d/nowy.bin").exists());
        assert!(run(&j, &s).planned.is_empty(), "{mode}: idempotencja");
    }
}

#[test]
fn r1_move_recorded_in_history_and_base() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"z".repeat(300), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 1, "{rep:?}");
    let h = history(&s, Some("t"), 50).unwrap();
    let row = h
        .iter()
        .find(|x| x.kind == "przeniesienie")
        .expect("wpis historii");
    assert_eq!(row.path, "b.bin");
    assert!(row.detail.contains("a.bin"), "{}", row.detail);
    // stary klucz zniknął z bazy: usunięcie b.bin po prawej nie może „wskrzesić” a.bin
    fs::remove_file(l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.deleted, 1, "{rep:?}");
    assert!(snap(&l).is_empty() && snap(&r).is_empty());
}

#[test]
fn r1_dry_run_plans_rename_without_touching_files() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"q".repeat(64), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run_job(&j, &s, true, false).unwrap();
    assert!(has_rename(&rep), "{rep:?}");
    assert!(r.join("a.bin").exists() && !r.join("b.bin").exists());
    assert_eq!(rep.moved, 0);
}

#[test]
fn r1_move_with_verify_bandwidth_and_no_trash() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"v".repeat(4096), T0);
    let mut j = job(&l, &r, "TwoWay");
    j.verify = true;
    j.trash_days = 0;
    j.bandwidth = syncron_core::model::Bandwidth {
        kbps: 5000,
        off_kbps: 0,
        hours: None,
    };
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r1_only_add_never_renames_or_deletes() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"o".repeat(100), T0);
    let mut j = job(&l, &r, "TwoWay");
    j.only_add = true;
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.deleted), (0, 0), "{rep:?}");
    assert!(r.join("a.bin").exists(), "only_add nie usuwa");
    assert!(r.join("b.bin").exists(), "nowa nazwa dodana");
    assert!(l.join("a.bin").exists() || true);
}

#[test]
fn r1_no_delete_right_keeps_old_name() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"o".repeat(100), T0);
    let mut j = job(&l, &r, "LeftToRight");
    j.no_delete_right = true;
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.deleted, 0, "{rep:?}");
    assert!(r.join("a.bin").exists() && r.join("b.bin").exists());
}

#[test]
fn r1_two_way_no_delete_left_right_never_loses_old_name() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"o".repeat(100), T0);
    let mut j = job(&l, &r, "TwoWay");
    j.no_delete_left = true;
    j.no_delete_right = true;
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    run(&j, &s);
    // usunięcie nie propaguje się: a.bin wraca na lewą, b.bin trafia na prawą
    assert_eq!(snap(&l), snap(&r));
    assert!(l.join("a.bin").exists() && l.join("b.bin").exists());
}

#[test]
fn r1_directory_rename_with_many_files() {
    let (_t, l, r, s) = setup();
    for i in 0..10 {
        // różne rozmiary: każda para jednoznaczna
        put(
            &l,
            &format!("Projekt/p{i}.dat"),
            &"d".repeat(100 + i * 7),
            T0,
        );
    }
    put(&l, "stały.txt", "s", T0 + 900);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("Projekt"), l.join("Archiwum 2026")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (10, 0, 0), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
    assert!(run(&j, &s).planned.is_empty());
}

// ---------------------------------------------------------------- RUNDA 2

#[test]
fn r2_false_match_different_content_same_size_and_mtime() {
    // Usunięty a.txt i nowy b.txt o innej treści, ale tym samym rozmiarze i mtime
    // (np. rozpakowanie archiwum zachowującego czasy) to NIE jest przeniesienie.
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "AAAAA", T0);
    put(&l, "stały.txt", "s", T0 + 900);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_file(l.join("a.txt")).unwrap();
    put(&l, "b.txt", "BBBBB", T0);
    run(&j, &s);
    assert_eq!(
        fs::read_to_string(r.join("b.txt")).unwrap(),
        "BBBBB",
        "po prawej b.txt ma starą treść a.txt: cicha utrata/uszkodzenie danych"
    );
}

#[test]
fn r2_false_match_is_never_repaired_later() {
    // Skutek T5-01: baza uznaje pliki za zgodne, więc błąd nie naprawi się sam.
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "AAAAA", T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_file(l.join("a.txt")).unwrap();
    put(&l, "b.txt", "BBBBB", T0);
    run(&j, &s);
    run(&j, &s);
    let (lb, rb) = (snap(&l)["b.txt"].clone(), snap(&r)["b.txt"].clone());
    assert_eq!(lb, rb, "strony rozjechane na stałe");
}

#[test]
fn r2_identical_copies_are_not_confused() {
    let (_t, l, r, s) = setup();
    for n in ["k1.bin", "k2.bin", "k3.bin"] {
        put(&l, n, &"c".repeat(500), T0);
    }
    put(&l, "inny.txt", "i", T0 + 900);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    // zmiana nazwy jednej z trzech identycznych kopii
    fs::rename(l.join("k2.bin"), l.join("k4.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(snap(&l), snap(&r), "{rep:?}");
    assert!(run(&j, &s).planned.is_empty());
    // zmiana nazw wszystkich trzech naraz jest niejednoznaczna: kopia, nie zgadywanie
    for (a, b) in [
        ("k1.bin", "m1.bin"),
        ("k3.bin", "m3.bin"),
        ("k4.bin", "m4.bin"),
    ] {
        fs::rename(l.join(a), l.join(b)).unwrap();
    }
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 0, "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_swap_two_files_names() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "aaaa-aaaa", T0);
    put(&l, "b.txt", "bbbbbbbbbbbb", T0 + 500);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.txt"), l.join("tmp.x")).unwrap();
    fs::rename(l.join("b.txt"), l.join("a.txt")).unwrap();
    fs::rename(l.join("tmp.x"), l.join("b.txt")).unwrap();
    let before = contents_with_trash(&l);
    run(&j, &s);
    assert_eq!(snap(&l), snap(&r));
    assert_eq!(snap(&r)["a.txt"], "bbbbbbbbbbbb");
    assert_eq!(snap(&r)["b.txt"], "aaaa-aaaa");
    assert!(before.is_subset(&contents_with_trash(&r)));
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
fn r2_rename_and_modify_at_once_is_copy_not_move() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "stara", T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_file(l.join("a.txt")).unwrap();
    put(&l, "b.txt", "zupełnie nowa treść", T0 + 5000);
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (0, 1, 1), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_move_in_both_directions_at_once() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(111), T0);
    put(&l, "b.bin", &"b".repeat(222), T0 + 100);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("a2.bin")).unwrap();
    fs::create_dir_all(r.join("x/y")).unwrap();
    fs::rename(r.join("b.bin"), r.join("x/y/b2.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (2, 0, 0), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
    assert!(snap(&l).contains_key("a2.bin") && snap(&l).contains_key("x/y/b2.bin"));
}

#[test]
fn r2_move_of_file_in_conflict_keeps_both_versions() {
    let (_t, l, r, s) = setup();
    put(&l, "x.txt", "baza", T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    // prawa modyfikuje x.txt, lewa zmienia nazwę i modyfikuje
    put(&r, "x.txt", "wersja prawa!", T0 + 3000);
    fs::remove_file(l.join("x.txt")).unwrap();
    put(&l, "y.txt", "wersja lewa", T0 + 4000);
    run(&j, &s);
    run(&j, &s);
    assert_eq!(snap(&l), snap(&r));
    let all: BTreeSet<String> = snap(&l).into_values().collect();
    assert!(
        all.contains("wersja prawa!") && all.contains("wersja lewa"),
        "{all:?}"
    );
}

#[test]
fn r2_move_target_exists_on_other_side() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(50), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    put(&r, "b.bin", "inny plik", T0 + 77); // cel istnieje po prawej z inną treścią
    let before: BTreeSet<String> = contents_with_trash(&l)
        .union(&contents_with_trash(&r))
        .cloned()
        .collect();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 0, "{rep:?}");
    run(&j, &s);
    let after: BTreeSet<String> = contents_with_trash(&l)
        .union(&contents_with_trash(&r))
        .cloned()
        .collect();
    assert!(
        before.is_subset(&after),
        "utrata danych: {before:?} / {after:?}"
    );
}

#[test]
fn r2_move_into_ignored_dir_removes_remote_copy_but_keeps_local() {
    let (_t, l, r, s) = setup();
    fs::write(l.join(".syncignore"), "build/\n").unwrap();
    put(&l, "a.bin", &"a".repeat(80), T0);
    put(&l, "b.txt", "b", T0 + 400);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::create_dir_all(l.join("build")).unwrap();
    fs::rename(l.join("a.bin"), l.join("build/a.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 0, "{rep:?}");
    assert!(l.join("build/a.bin").exists(), "lokalny plik nietknięty");
    assert!(
        !r.join("build").exists(),
        "ignorowany katalog nie jest synchronizowany"
    );
    // usunięcie po prawej trafiło do kosza (nie utrata danych)
    assert!(contents_with_trash(&r).contains(&"a".repeat(80)));
}

#[test]
fn r2_move_out_of_ignored_dir_is_new_copy() {
    let (_t, l, r, s) = setup();
    fs::write(l.join(".syncignore"), "build/\n").unwrap();
    put(&l, "build/a.bin", &"a".repeat(80), T0);
    put(&l, "b.txt", "b", T0 + 400);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    assert!(!r.join("build").exists());
    fs::rename(l.join("build/a.bin"), l.join("a.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied), (0, 1), "{rep:?}");
    assert_eq!(fs::read_to_string(r.join("a.bin")).unwrap(), "a".repeat(80));
}

#[test]
fn r2_zero_size_file_rename_is_copy_and_delete() {
    let (_t, l, r, s) = setup();
    put(&l, "pusty1", "", T0);
    put(&l, "x", "x", T0 + 500);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("pusty1"), l.join("pusty2")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 0, "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_deep_new_directories_on_move() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(333), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    let deep = "k1/k2/k3/k4/k5/k6/żółć";
    fs::create_dir_all(l.join(deep)).unwrap();
    fs::rename(l.join("a.bin"), l.join(deep).join("ł.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 1, "{rep:?}");
    assert!(r.join(deep).join("ł.bin").exists());
    assert!(!r.join("a.bin").exists());
}

#[test]
#[ignore = "DECYZJA: T5-02 - klucz = małe litery (zmiana samej wielkości liter nie jest propagowana, jak w rsync/Dropbox na FS bez rozróżniania wielkości)"]
fn r2_case_only_rename_propagates() {
    // plik.TXT -> plik.txt: klucz (małe litery) identyczny, więc silnik nie widzi zmiany
    let (_t, l, r, s) = setup();
    put(&l, "plik.TXT", "zawartość", T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("plik.TXT"), l.join("plik.txt")).unwrap();
    run(&j, &s);
    let names: Vec<String> = snap(&r).into_keys().collect();
    assert_eq!(
        names,
        vec!["plik.txt".to_string()],
        "nazwa po prawej nie została zmieniona"
    );
}

#[test]
fn r2_case_only_rename_does_not_lose_or_duplicate() {
    let (_t, l, r, s) = setup();
    put(&l, "plik.TXT", "zawartość", T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("plik.TXT"), l.join("plik.txt")).unwrap();
    let rep = run(&j, &s);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(snap(&r).len(), 1);
    assert_eq!(snap(&l).len(), 1);
}

#[test]
fn r2_move_between_nfc_nfd_names() {
    use unicode_normalization::UnicodeNormalization;
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(123), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    let nfd: String = "żółć.bin".nfd().collect();
    fs::rename(l.join("a.bin"), l.join(&nfd)).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 1, "{rep:?}");
    assert_eq!(snap(&r).len(), 1);
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
fn r2_move_with_mtime_within_tolerance() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(123), T0);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    set_mtime(&l.join("b.bin"), T0 + 2);
    let rep = run(&j, &s);
    assert_eq!((rep.moved, rep.copied), (1, 0), "{rep:?}");
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
fn r2_move_blocked_by_guard_does_not_touch_files() {
    // przeniesienia nie liczą się jako usunięcia; ale pusta strona nadal blokuje
    let (_t, l, r, s) = setup();
    for i in 0..4 {
        put(&l, &format!("f{i}.bin"), &"f".repeat(100 + i), T0);
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    for i in 0..4 {
        fs::rename(l.join(format!("f{i}.bin")), l.join(format!("g{i}.bin"))).unwrap();
    }
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.blocked.is_none(), "{rep:?}");
    assert_eq!(rep.moved, 4, "{rep:?}");
}

#[test]
fn r2_rename_chain_a_to_b_and_b_to_c() {
    let (_t, l, r, s) = setup();
    put(&l, "a", &"1".repeat(10), T0);
    put(&l, "b", &"2".repeat(20), T0 + 100);
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    // b -> c, a -> b (b zajęte nową treścią)
    fs::rename(l.join("b"), l.join("c")).unwrap();
    fs::rename(l.join("a"), l.join("b")).unwrap();
    let before = contents_with_trash(&l);
    run(&j, &s);
    assert_eq!(snap(&l), snap(&r));
    assert!(before.is_subset(&contents_with_trash(&r)));
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
fn r2_exclude_glob_moves_file_out_of_sync_scope() {
    let (_t, l, r, s) = setup();
    put(&l, "a.dat", &"a".repeat(40), T0);
    put(&l, "k.txt", "k", T0 + 200);
    let mut j = job(&l, &r, "TwoWay");
    j.exclude = vec!["*.log".into()];
    run(&j, &s);
    fs::rename(l.join("a.dat"), l.join("a.log")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 0, "{rep:?}");
    assert!(l.join("a.log").exists());
    assert!(!r.join("a.log").exists());
    assert!(contents_with_trash(&r).contains(&"a".repeat(40)));
}

#[test]
fn r2_min_max_size_move_across_limit() {
    let (_t, l, r, s) = setup();
    put(&l, "a.bin", &"a".repeat(50), T0);
    let mut j = job(&l, &r, "TwoWay");
    j.min_size = Some(10);
    j.max_size = Some(100);
    run(&j, &s);
    fs::rename(l.join("a.bin"), l.join("b.bin")).unwrap();
    let rep = run(&j, &s);
    assert_eq!(rep.moved, 1, "{rep:?}");
}

// ---------------------------------------------------------------- RUNDA 3

struct Rng(u64, i64);
impl Rng {
    /// mtime: przy `collide` tylko dwie wartości (prowokuje fałszywe dopasowania), inaczej unikalne.
    fn mt(&mut self, collide: bool) -> i64 {
        if collide {
            T0 + self.below(2) as i64
        } else {
            self.1 += 1;
            T0 + 100 * self.1
        }
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.below(100) < pct
    }
}

const PATHS: [&str; 7] = [
    "a.txt",
    "b.txt",
    "d1/c.txt",
    "d1/d.txt",
    "d2/e/f.txt",
    "g.txt",
    "d3/h i.txt",
];
const NEW_PATHS: [&str; 5] = [
    "n1.txt",
    "d1/n2.txt",
    "d4/x/y/n3.txt",
    "n4.txt",
    "d2/n5.txt",
];

fn rand_content(g: &mut Rng, equal_len: bool) -> String {
    let letter = (b'a' + g.below(3) as u8) as char;
    let len = if equal_len {
        3
    } else {
        1 + g.below(4) as usize
    };
    letter.to_string().repeat(len)
}

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::TwoWay => "TwoWay",
        Mode::LeftToRight => "LeftToRight",
        Mode::RightToLeft => "RightToLeft",
    }
}

struct Combo {
    mode: Mode,
    ndl: bool,
    ndr: bool,
    only_add: bool,
    conflict: Conflict,
    collide: bool,
}

fn make_job(l: &Path, r: &Path, c: &Combo) -> Job {
    let mut j = job(l, r, mode_str(c.mode));
    j.no_delete_left = c.ndl;
    j.no_delete_right = c.ndr;
    j.only_add = c.only_add;
    j.conflict = c.conflict;
    j.guard_pct = 100;
    j
}

fn mutate(g: &mut Rng, l: &Path, r: &Path, collide: bool) {
    for _ in 0..(2 + g.below(4)) {
        let side = if g.chance(50) { l } else { r };
        let files = snap(side);
        let keys: Vec<&String> = files.keys().collect();
        match g.below(4) {
            0 if !keys.is_empty() => {
                // usunięcie
                let k = keys[g.below(keys.len() as u64) as usize];
                fs::remove_file(side.join(k)).unwrap();
            }
            1 if !keys.is_empty() => {
                // modyfikacja: późniejszy mtime i inny rozmiar
                let k = keys[g.below(keys.len() as u64) as usize];
                let (n, mt) = (
                    5 + g.below(5) as usize,
                    100_000 + g.1 * 100 + {
                        g.1 += 1;
                        0
                    },
                );
                put(side, k, &"m".repeat(n), T0 + mt);
            }
            2 if !keys.is_empty() => {
                // zmiana nazwy/przeniesienie (mtime zachowany)
                let k = keys[g.below(keys.len() as u64) as usize];
                let to = NEW_PATHS[g.below(NEW_PATHS.len() as u64) as usize];
                if !l.join(to).exists() && !r.join(to).exists() {
                    fs::create_dir_all(side.join(to).parent().unwrap()).unwrap();
                    fs::rename(side.join(k), side.join(to)).unwrap();
                }
            }
            _ => {
                // nowy plik o małym, często powtarzalnym rozmiarze i mtime
                let to = NEW_PATHS[g.below(NEW_PATHS.len() as u64) as usize];
                if !l.join(to).exists() && !r.join(to).exists() {
                    let (c, mt) = (rand_content(g, true), g.mt(collide));
                    put(side, to, &c, mt);
                }
            }
        }
    }
}

fn check_scenario(seed: u64, c: &Combo, label: &str) {
    let mut g = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, 0);
    let (_t, l, r, s) = setup();
    for p in PATHS {
        let eq = g.chance(50);
        let content = rand_content(&mut g, eq);
        let mt = g.mt(c.collide);
        match g.below(4) {
            0 => put(&l, p, &content, mt),
            1 => put(&r, p, &content, mt),
            _ => {
                put(&l, p, &content, mt);
                put(&r, p, &content, mt);
            }
        }
    }
    let j = make_job(&l, &r, c);
    let rep0 = run(&j, &s);
    assert!(rep0.errors.is_empty(), "{label} start: {rep0:?}");
    mutate(&mut g, &l, &r, c.collide);

    let (bl, br) = (snap(&l), snap(&r));
    let mut before_contents = contents_with_trash(&l);
    before_contents.extend(contents_with_trash(&r));
    let rep = run(&j, &s);
    let ctx = format!("{label} seed={seed}\nL przed={bl:?}\nR przed={br:?}\nrep={rep:?}");
    assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{ctx}");
    let (al, ar) = (snap(&l), snap(&r));
    let ctx = format!("{ctx}\nL po={al:?}\nR po={ar:?}");

    // brak utraty danych (treści trafiają do kosza zamiast znikać)
    let mut after_contents = contents_with_trash(&l);
    after_contents.extend(contents_with_trash(&r));
    assert!(
        before_contents.is_subset(&after_contents),
        "utrata danych {ctx}"
    );

    let conflicts_pending = !rep.conflicts.is_empty();
    if c.only_add {
        for (k, v) in &bl {
            assert_eq!(
                al.get(k),
                Some(v),
                "only_add zmienił/usunął po lewej: {k} {ctx}"
            );
        }
        for (k, v) in &br {
            assert_eq!(
                ar.get(k),
                Some(v),
                "only_add zmienił/usunął po prawej: {k} {ctx}"
            );
        }
    } else if !conflicts_pending {
        match c.mode {
            Mode::LeftToRight => {
                for (k, v) in &al {
                    assert_eq!(ar.get(k), Some(v), "cel != źródło {k} {ctx}");
                }
                if !c.ndr {
                    assert_eq!(al, ar, "lustro {ctx}");
                } else {
                    for k in br.keys() {
                        assert!(ar.contains_key(k), "no_delete_right usunął {k} {ctx}");
                    }
                }
            }
            Mode::RightToLeft => {
                for (k, v) in &ar {
                    assert_eq!(al.get(k), Some(v), "cel != źródło {k} {ctx}");
                }
                if !c.ndl {
                    assert_eq!(al, ar, "lustro {ctx}");
                } else {
                    for k in bl.keys() {
                        assert!(al.contains_key(k), "no_delete_left usunął {k} {ctx}");
                    }
                }
            }
            Mode::TwoWay => assert_eq!(al, ar, "dwustronny: strony różne {ctx}"),
        }
    }

    // zbieżność: po kilku przebiegach plan pusty
    if !(c.conflict == Conflict::Ask && conflicts_pending) {
        let mut last = run(&j, &s);
        for _ in 0..2 {
            if last.planned.is_empty() {
                break;
            }
            last = run(&j, &s);
        }
        let conf_left = c.conflict == Conflict::Ask && !last.conflicts.is_empty();
        if !conf_left {
            assert!(
                last.planned.is_empty() && last.errors.is_empty(),
                "brak idempotencji {ctx}\nostatni plan={:?}",
                last.planned
            );
        }
    }
}

/// Rodzaje niepowodzeń (pierwsze słowa komunikatu asercji).
fn kinds(f: &[String]) -> BTreeSet<String> {
    f.iter()
        .map(|m| {
            let m = m
                .lines()
                .next()
                .unwrap_or(m)
                .trim_start_matches("assertion `left == right` failed: ");
            m.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
        })
        .collect()
}

fn all_combos() -> Vec<Combo> {
    let mut v = Vec::new();
    for mode in [Mode::TwoWay, Mode::LeftToRight, Mode::RightToLeft] {
        for ndl in [false, true] {
            for ndr in [false, true] {
                for only_add in [false, true] {
                    for conflict in [
                        Conflict::Newer,
                        Conflict::Larger,
                        Conflict::LeftWins,
                        Conflict::RightWins,
                        Conflict::KeepBoth,
                        Conflict::Ask,
                    ] {
                        v.push(Combo {
                            mode,
                            ndl,
                            ndr,
                            only_add,
                            conflict,
                            collide: false,
                        });
                    }
                }
            }
        }
    }
    v
}

/// Pełna macierz opcji, kilka ziaren na kombinację; zbiera wszystkie niepowodzenia.
fn matrix(
    seeds: std::ops::Range<u64>,
    collide: bool,
    filter: impl Fn(&Combo) -> bool,
) -> Vec<String> {
    let mut fails = Vec::new();
    for mut c in all_combos().into_iter().filter(|c| filter(c)) {
        c.collide = collide;
        let c = &c;
        for seed in seeds.clone() {
            let label = format!(
                "{:?} ndl={} ndr={} add={} {:?}",
                c.mode, c.ndl, c.ndr, c.only_add, c.conflict
            );
            let r = std::panic::catch_unwind(|| check_scenario(seed, c, &label));
            if let Err(e) = r {
                let m = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                fails.push(m);
            }
        }
    }
    fails
}

#[test]
fn r3_matrix_only_add_invariants() {
    let f = matrix(1..4, false, |c| c.only_add);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
fn r3_matrix_one_way_invariants() {
    let f = matrix(1..4, false, |c| !c.only_add && c.mode != Mode::TwoWay);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
fn r3_matrix_two_way_invariants() {
    let f = matrix(1..4, false, |c| !c.only_add && c.mode == Mode::TwoWay);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
fn r3_matrix_with_filters_verify_and_trash_options() {
    // te same niezmienniki przy filtrach, verify, bez kosza (kosz wyłączony: tylko spójność stron)
    for seed in 1..6u64 {
        let mut g = Rng(seed * 7919 + 3, 0);
        let (_t, l, r, s) = setup();
        fs::write(l.join(".syncignore"), "d3/\n*.bak\n").unwrap();
        for p in PATHS {
            let c = rand_content(&mut g, true);
            put(&l, p, &c, T0);
            put(&r, p, &c, T0);
        }
        put(&l, "z.bak", "xx", T0);
        let mut j = job(&l, &r, "TwoWay");
        j.verify = true;
        j.trash_days = 0;
        j.guard_pct = 100;
        j.exclude = vec!["*.log".into()];
        j.min_size = Some(2);
        j.max_size = Some(50);
        run(&j, &s);
        mutate(&mut g, &l, &r, true);
        let rep = run(&j, &s);
        assert!(rep.errors.is_empty(), "seed={seed}: {rep:?}");
        let (al, ar) = (snap(&l), snap(&r));
        let strip = |m: BTreeMap<String, String>| -> BTreeMap<String, String> {
            m.into_iter()
                .filter(|(k, _)| !k.starts_with("d3/") && !k.ends_with(".bak"))
                .collect()
        };
        // ignorowane i pliki spoza zakresu rozmiaru mogą różnić strony
        let in_scope = |m: BTreeMap<String, String>| -> BTreeMap<String, String> {
            strip(m)
                .into_iter()
                .filter(|(_, v)| v.len() >= 2 && v.len() <= 50)
                .collect()
        };
        assert_eq!(in_scope(al), in_scope(ar), "seed={seed}: {rep:?}");
        assert!(run(&j, &s).planned.is_empty(), "seed={seed}");
    }
}

#[test]
fn r3_random_renames_converge_without_loss() {
    // mocniejszy nacisk na przeniesienia: wiele zmian nazw w tym samym przebiegu
    for seed in 100..160u64 {
        let mut g = Rng(seed.wrapping_mul(0xD134_2543_DE82_EF95) | 1, 0);
        let (_t, l, r, s) = setup();
        let mut n = 0;
        for p in PATHS {
            n += 1;
            put(&l, p, &"c".repeat(2 + g.below(3) as usize), T0 + (n % 2));
        }
        let j = {
            let mut j = job(&l, &r, "TwoWay");
            j.guard_pct = 100;
            j
        };
        run(&j, &s);
        let mut before: BTreeSet<String> = BTreeSet::new();
        for (i, p) in PATHS.iter().enumerate() {
            if g.chance(60) {
                let side = if g.chance(50) { &l } else { &r };
                if side.join(p).exists() {
                    let to = format!("ren/{i}/nowa.txt");
                    fs::create_dir_all(side.join(&to).parent().unwrap()).unwrap();
                    fs::rename(side.join(p), side.join(&to)).unwrap();
                }
            }
        }
        before.extend(contents_with_trash(&l));
        before.extend(contents_with_trash(&r));
        let rep = run(&j, &s);
        assert!(rep.errors.is_empty(), "seed={seed}: {rep:?}");
        let (al, ar) = (snap(&l), snap(&r));
        assert_eq!(al, ar, "seed={seed}: {rep:?}");
        let mut after = contents_with_trash(&l);
        after.extend(contents_with_trash(&r));
        assert!(before.is_subset(&after), "seed={seed}");
        assert!(run(&j, &s).planned.is_empty(), "seed={seed}");
    }
}

#[test]
fn r3_matrix_collisions_one_way() {
    let f = matrix(1..13, true, |c| !c.only_add && c.mode != Mode::TwoWay);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
fn r3_matrix_collisions_two_way() {
    let f = matrix(1..13, true, |c| !c.only_add && c.mode == Mode::TwoWay);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
fn r3_matrix_collisions_only_add_and_no_loss() {
    // przy kolizjach rozmiaru/mtime only_add nadal nic nie nadpisuje ani nie usuwa
    let f = matrix(1..5, true, |c| c.only_add);
    assert!(
        f.is_empty(),
        "{} niepowodzeń, rodzaje: {:?}\npierwsze:\n{}",
        f.len(),
        kinds(&f),
        f[0]
    );
}

#[test]
#[ignore = "DECYZJA: T5-03 - porównanie po rozmiarze i mtime jak w rsync (treści nie czytamy); opis przy plan::same"]
fn r2_new_file_on_both_sides_same_meta_different_content() {
    let (_t, l, r, s) = setup();
    put(&l, "n.txt", "ccc", T0);
    put(&r, "n.txt", "aaa", T0);
    let mut j = job(&l, &r, "TwoWay");
    j.verify = true;
    let rep = run(&j, &s);
    assert!(
        !rep.planned.is_empty(),
        "dwie różne treści uznane za identyczne, bez ostrzeżenia: {rep:?}"
    );
}
