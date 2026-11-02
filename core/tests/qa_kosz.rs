//! QA tester 4: kosz synchronizacji i wersje plików.
//! Testy oznaczone `#[ignore = "BUG: ..."]` odtwarzają błędy znalezione w kodzie.
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{run_job, trash_delete, trash_list, trash_restore};
use syncron_core::location::Location;
use syncron_core::model::Job;

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

fn loc(p: &Path) -> Location {
    Location::parse(p.to_str().unwrap())
}

fn put(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().unwrap().is_dir() {
                n += count_files(&e.path());
            } else {
                n += 1;
            }
        }
    }
    n
}

fn stamps_of(r: &Path, rel: &str) -> Vec<String> {
    loc(r)
        .list_trash()
        .unwrap()
        .into_iter()
        .filter(|t| t.1 == rel)
        .map(|t| t.0)
        .collect()
}

// ------------------------------------------------------------ RUNDA 1

#[test]
fn r1_overwrite_stamp_format_and_content() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "stara");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "nowa");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let items = loc(&r).list_trash().unwrap();
    assert_eq!(items.len(), 1);
    let st = &items[0].0;
    // YYYY-MM-DD_HHMMSS
    assert_eq!(st.len(), 17, "{st}");
    assert!(st
        .chars()
        .enumerate()
        .all(|(i, c)| matches!(i, 4 | 7) && c == '-'
            || i == 10 && c == '_'
            || !matches!(i, 4 | 7 | 10) && c.is_ascii_digit()));
    assert_eq!(
        fs::read_to_string(r.join(".syncron-trash").join(st).join("a.txt")).unwrap(),
        "stara"
    );
}

#[test]
fn r1_delete_propagates_to_trash_and_restore_roundtrip() {
    let (_t, l, r, s) = setup();
    put(&l, "d/e.txt", "tresc");
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    fs::remove_file(l.join("d/e.txt")).unwrap();
    // guard: 1 z 1 pliku = 100% - wymuś
    run_job(&j, &s, false, true).unwrap();
    assert!(!r.join("d/e.txt").exists());
    let items = trash_list(&j).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].side, "right");
    let to = trash_restore(&j, "right", &items[0].stamp, &items[0].path).unwrap();
    assert_eq!(to, "d/e.txt", "brak kolizji: oryginalna ścieżka");
    assert_eq!(fs::read_to_string(r.join("d/e.txt")).unwrap(), "tresc");
    // przywrócony plik wraca na lewą stronę przy następnym przebiegu
    run_job(&j, &s, false, true).unwrap();
    assert_eq!(fs::read_to_string(l.join("d/e.txt")).unwrap(), "tresc");
}

#[test]
fn r1_trash_disabled_delete_is_permanent() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "1");
    put(&l, "b.txt", "1");
    let mut j = job(&l, &r, "TwoWay");
    j.trash_days = 0;
    run_job(&j, &s, false, false).unwrap();
    fs::remove_file(l.join("b.txt")).unwrap();
    run_job(&j, &s, false, true).unwrap();
    assert!(!r.join("b.txt").exists());
    assert!(!r.join(".syncron-trash").exists());
    assert!(trash_list(&j).unwrap().is_empty());
    // purge z trash_days=0 i max_versions nie panikuje
    loc(&r).purge_trash(0, 3);
}

#[test]
fn r1_trash_is_not_synced_to_other_side() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "v1");
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "v2");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    assert!(r.join(".syncron-trash").exists());
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
    assert!(
        !l.join(".syncron-trash").exists(),
        "kosz nie może trafić na lewą stronę"
    );
}

#[test]
fn r1_legacy_trash_ignored_by_sync() {
    let (_t, l, r, s) = setup();
    put(&l, ".syncpolak-trash/2020-01-01_000000/x.txt", "stary");
    put(&l, "a.txt", "1");
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    assert!(!r.join(".syncpolak-trash").exists());
    assert!(r.join("a.txt").exists());
}

#[test]
fn r1_restore_with_conflict_uses_new_name_and_keeps_existing() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "v1");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "v2");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let it = &trash_list(&j).unwrap()[0];
    let to = trash_restore(&j, "right", &it.stamp, &it.path).unwrap();
    assert_ne!(to, "a.txt");
    assert_eq!(fs::read_to_string(r.join("a.txt")).unwrap(), "v2");
    assert_eq!(fs::read_to_string(r.join(&to)).unwrap(), "v1");
}

#[test]
fn r1_versions_cap_via_run_job_keeps_newest() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "0");
    let mut j = job(&l, &r, "LeftToRight");
    j.max_versions = 2;
    run_job(&j, &s, false, false).unwrap();
    // sztuczna historia w koszu
    for d in 1..=5 {
        put(
            &r,
            &format!(".syncron-trash/2099-01-0{d}_000000/a.txt"),
            &format!("v{d}"),
        );
    }
    put(&l, "a.txt", "nowe");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let st = stamps_of(&r, "a.txt");
    assert_eq!(st.len(), 2, "{st:?}");
    assert_eq!(st[1], "2099-01-05_000000");
}

// ------------------------------------------------------------ RUNDA 2

#[test]
fn r2_two_runs_same_second_keep_both_versions() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "v1");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "v2");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "v3");
    set_mtime(&l.join("a.txt"), 2_000_000_100);
    run_job(&j, &s, false, false).unwrap();
    assert_eq!(
        count_files(&r.join(".syncron-trash")),
        2,
        "powinny być wersje v1 i v2"
    );
}

#[test]
fn r2_delete_recreate_delete_same_second() {
    let l = tempfile::tempdir().unwrap();
    let lc = loc(l.path());
    put(l.path(), "a.txt", "pierwsza");
    lc.remove("a.txt", "2026-01-01_000000").unwrap();
    put(l.path(), "a.txt", "druga");
    lc.remove("a.txt", "2026-01-01_000000").unwrap();
    assert_eq!(count_files(&l.path().join(".syncron-trash")), 2);
}

#[test]
fn r2_restore_traversal_via_stamp_does_not_read_outside_trash() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("deep").join("R");
    fs::create_dir_all(root.join(".syncron-trash")).unwrap();
    put(t.path(), "secret.txt", "sekret");
    let r = loc(&root).restore_trash("../../..", "secret.txt");
    assert!(
        r.is_err() && !root.join("secret.txt").exists(),
        "stamp z `..` odczytał plik spoza kosza: {r:?}"
    );
}

#[test]
fn r2_restore_traversal_via_path_does_not_write_outside_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("deep").join("R");
    fs::create_dir_all(root.join(".syncron-trash/s")).unwrap();
    put(&root, "src.txt", "zywy plik");
    // trash/s/../../src.txt = R/src.txt (żywy plik, nie z kosza); cel = R/../../src.txt (poza rootem)
    let r = loc(&root).restore_trash("s", "../../src.txt");
    assert!(
        r.is_err() && !t.path().join("src.txt").exists(),
        "path z `..` zapisał plik poza folderem synchronizacji: {r:?}"
    );
}

#[test]
fn r2_delete_trash_stamp_dotdot_must_not_wipe_root() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("R");
    fs::create_dir_all(root.join(".syncron-trash")).unwrap();
    put(&root, "wazne/dane.txt", "dane");
    let _ = loc(&root).delete_trash(Some(".."), None);
    assert!(
        root.join("wazne/dane.txt").exists(),
        "delete_trash(\"..\", None) skasował cały folder synchronizacji"
    );
}

#[test]
fn r2_delete_trash_path_dotdot_must_not_delete_live_file() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("R");
    fs::create_dir_all(root.join(".syncron-trash/s")).unwrap();
    put(&root, "zywy.txt", "dane");
    let _ = loc(&root).delete_trash(Some("s"), Some("../../zywy.txt"));
    assert!(
        root.join("zywy.txt").exists(),
        "delete_trash z `..` w path skasował żywy plik"
    );
}

#[test]
fn r2_absolute_and_empty_values_are_harmless() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("R");
    put(&root, ".syncron-trash/s/abs/x.txt", "x");
    put(&root, "zywy.txt", "z");
    let lc = loc(&root);
    // absolutna ścieżka jest traktowana jako względna (bez ucieczki)
    let _ = lc.restore_trash("s", "/abs/x.txt");
    assert!(root.join("abs/x.txt").exists());
    // puste wartości: bez paniki, żywy plik zostaje
    let _ = lc.restore_trash("", "");
    let _ = lc.restore_trash("s", "");
    let _ = lc.delete_trash(Some("s"), Some(""));
    let _ = lc.delete_trash(Some(""), Some(""));
    assert!(root.join("zywy.txt").exists());
}

#[test]
fn r2_delete_trash_none_some_must_not_wipe_everything() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("R");
    put(&root, ".syncron-trash/s1/a.txt", "a");
    put(&root, ".syncron-trash/s2/b.txt", "b");
    let r = loc(&root).delete_trash(None, Some("a.txt"));
    assert!(
        r.is_err() || count_files(&root.join(".syncron-trash")) >= 1,
        "cały kosz skasowany przy braku stamp"
    );
}

#[test]
fn r2_delete_trash_nonexistent_is_ok() {
    let t = tempfile::tempdir().unwrap();
    let lc = loc(t.path());
    lc.delete_trash(None, None).unwrap();
    lc.delete_trash(Some("nie-ma"), None).unwrap();
    lc.delete_trash(Some("nie-ma"), Some("x.txt")).unwrap();
}

#[test]
fn r2_restore_missing_source_returns_error() {
    let t = tempfile::tempdir().unwrap();
    let r = loc(t.path()).restore_trash("2026-01-01_000000", "a.txt");
    assert!(r.is_err());
    assert!(!t.path().join("a.txt").exists());
}

#[test]
fn r2_restore_missing_source_leaves_no_empty_dirs() {
    let t = tempfile::tempdir().unwrap();
    let r = loc(t.path()).restore_trash("2026-01-01_000000", "nowy/podkat/a.txt");
    assert!(r.is_err());
    assert!(!t.path().join("nowy").exists(), "zostały puste foldery");
}

#[test]
fn r2_max_versions_one_keeps_only_newest() {
    let t = tempfile::tempdir().unwrap();
    for d in 1..=3 {
        put(
            t.path(),
            &format!(".syncron-trash/2026-01-0{d}_000000/a.txt"),
            "x",
        );
    }
    put(t.path(), ".syncron-trash/2026-01-01_000000/b.txt", "x");
    loc(t.path()).purge_trash(0, 1);
    assert_eq!(stamps_of(t.path(), "a.txt"), vec!["2026-01-03_000000"]);
    assert_eq!(stamps_of(t.path(), "b.txt"), vec!["2026-01-01_000000"]);
}

#[test]
fn r2_retention_days_boundaries() {
    let t = tempfile::tempdir().unwrap();
    let now = chrono::Local::now();
    let fmt = |d: i64| {
        (now - chrono::Duration::days(d))
            .format("%Y-%m-%d_%H%M%S")
            .to_string()
    };
    for d in [0, 1, 5] {
        put(t.path(), &format!(".syncron-trash/{}/a.txt", fmt(d)), "x");
    }
    put(
        t.path(),
        &format!(
            ".syncron-trash/{}/b.txt",
            (now + chrono::Duration::days(3)).format("%Y-%m-%d_%H%M%S")
        ),
        "x",
    );
    loc(t.path()).purge_trash(1, 0);
    let left: Vec<String> = loc(t.path())
        .list_trash()
        .unwrap()
        .into_iter()
        .map(|x| x.0)
        .collect();
    assert_eq!(left.len(), 3, "{left:?}"); // dziś, wczoraj, przyszłość; 5 dni usunięte
    assert!(!left.contains(&fmt(5)));
    assert!(left.contains(&fmt(0)) && left.contains(&fmt(1)));
}

#[test]
fn r2_retention_extreme_days_no_panic() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".syncron-trash/2000-01-01_000000/a.txt", "x");
    loc(t.path()).purge_trash(u32::MAX, 0);
    assert_eq!(loc(t.path()).list_trash().unwrap().len(), 1);
}

#[test]
fn r2_purge_non_ascii_folder_name_no_panic() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".syncron-trash/ażółćżółć/a.txt", "x");
    loc(t.path()).purge_trash(1, 0);
}

#[test]
fn r2_purge_only_removes_valid_stamp_folders() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".syncron-trash/2000-moje-dokumenty/a.txt", "x");
    put(t.path(), ".syncron-trash/1-notatki-uzytkownika/a.txt", "x");
    loc(t.path()).purge_trash(1, 0);
    assert!(t
        .path()
        .join(".syncron-trash/2000-moje-dokumenty/a.txt")
        .exists());
    assert!(t
        .path()
        .join(".syncron-trash/1-notatki-uzytkownika/a.txt")
        .exists());
}

#[test]
fn r2_stray_file_in_trash_root_does_not_break_listing() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".syncron-trash/2026-01-01_000000/a.txt", "x");
    put(t.path(), ".syncron-trash/notatka.txt", "obcy");
    let r = loc(t.path()).list_trash();
    assert!(r.is_ok() && r.unwrap().len() == 1);
}

#[test]
fn r2_polish_names_deep_paths_roundtrip() {
    let (_t, l, r, s) = setup();
    let rel = "Zażółć gęślą/jaźń/ąęłńóśźż/bardzo/głęboka/ścieżka/plik żółć.txt";
    put(&l, rel, "v1");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, rel, "v2");
    set_mtime(&l.join(rel), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let it = trash_list(&j).unwrap();
    assert_eq!(it.len(), 1);
    assert_eq!(it[0].path, rel);
    fs::remove_file(r.join(rel)).unwrap();
    let to = trash_restore(&j, "right", &it[0].stamp, &it[0].path).unwrap();
    assert_eq!(to, rel);
    assert_eq!(fs::read_to_string(r.join(rel)).unwrap(), "v1");
}

#[test]
fn r2_unknown_side_is_rejected() {
    let (_t, l, r, _s) = setup();
    put(&r, ".syncron-trash/s/a.txt", "x");
    let j = job(&l, &r, "TwoWay");
    assert!(trash_delete(&j, "xyz", Some("s"), Some("a.txt")).is_err());
    assert!(r.join(".syncron-trash/s/a.txt").exists());
}

// ------------------------------------------------------------ RUNDA 3

#[test]
fn r3_cap_versions_many_files_under_load() {
    let t = tempfile::tempdir().unwrap();
    for f in 0..40 {
        for d in 1..=9 {
            put(
                t.path(),
                &format!(".syncron-trash/2026-02-0{d}_000000/dir{}/f{f}.txt", f % 4),
                &format!("{d}"),
            );
        }
    }
    loc(t.path()).purge_trash(0, 3);
    let items = loc(t.path()).list_trash().unwrap();
    assert_eq!(items.len(), 40 * 3);
    assert_eq!(count_files(&t.path().join(".syncron-trash")), items.len());
    for (st, rel, _) in &items {
        assert!(
            ("2026-02-07_000000"..="2026-02-09_000000").contains(&st.as_str()),
            "{st} {rel}"
        );
    }
}

#[test]
fn r3_cap_versions_cleans_empty_stamp_dirs() {
    let t = tempfile::tempdir().unwrap();
    for d in 1..=5 {
        put(
            t.path(),
            &format!(".syncron-trash/2026-02-0{d}_000000/sub/f.txt"),
            "x",
        );
    }
    loc(t.path()).purge_trash(0, 1);
    let dirs = fs::read_dir(t.path().join(".syncron-trash"))
        .unwrap()
        .count();
    assert_eq!(dirs, 1);
}

#[test]
fn r3_mass_delete_guard_blocks_without_touching_trash_then_force_trashes() {
    let (_t, l, r, s) = setup();
    for i in 0..4 {
        put(&l, &format!("f{i}.txt"), "x");
    }
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    for i in 0..3 {
        fs::remove_file(l.join(format!("f{i}.txt"))).unwrap();
    }
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.blocked.is_some());
    assert!(!r.join(".syncron-trash").exists());
    assert!(r.join("f0.txt").exists());
    run_job(&j, &s, false, true).unwrap();
    assert_eq!(trash_list(&j).unwrap().len(), 3);
    assert!(!r.join("f0.txt").exists());
}

#[test]
fn r3_dry_run_creates_no_trash() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "1");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "2");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, true, false).unwrap();
    assert!(!r.join(".syncron-trash").exists());
}

#[test]
fn r3_two_sides_trash_independent_and_delete_by_side() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "l1");
    put(&l, "b.txt", "x");
    put(&l, "c.txt", "x");
    put(&l, "d.txt", "x");
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    // zmiana po prawej -> backup po lewej
    put(&r, "a.txt", "r2");
    set_mtime(&r.join("a.txt"), 2_000_000_000);
    // usunięcie po lewej -> kosz po prawej
    fs::remove_file(l.join("b.txt")).unwrap();
    run_job(&j, &s, false, false).unwrap();
    let items = trash_list(&j).unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(items.iter().any(|i| i.side == "left" && i.path == "a.txt"));
    assert!(items.iter().any(|i| i.side == "right" && i.path == "b.txt"));
    let st = items
        .iter()
        .find(|i| i.side == "left")
        .unwrap()
        .stamp
        .clone();
    trash_delete(&j, "left", Some(&st), Some("a.txt")).unwrap();
    let items = trash_list(&j).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].side, "right");
}

#[test]
fn r3_list_consistent_with_disk_after_many_ops() {
    let (_t, l, r, s) = setup();
    let j = job(&l, &r, "LeftToRight");
    for i in 0..15 {
        put(&l, &format!("sub{}/f{i}.txt", i % 3), "v0");
    }
    run_job(&j, &s, false, false).unwrap();
    for round in 1..=3 {
        for i in 0..15 {
            let p = l.join(format!("sub{}/f{i}.txt", i % 3));
            fs::write(&p, format!("v{round}")).unwrap();
            set_mtime(&p, 2_000_000_000 + round * 100);
        }
        run_job(&j, &s, false, false).unwrap();
    }
    let items = trash_list(&j).unwrap();
    assert_eq!(items.len(), count_files(&r.join(".syncron-trash")));
    for it in &items {
        assert!(r
            .join(".syncron-trash")
            .join(&it.stamp)
            .join(&it.path)
            .is_file());
    }
    trash_delete(&j, "right", None, None).unwrap();
    assert!(trash_list(&j).unwrap().is_empty());
    // po opróżnieniu kosza następny przebieg nic nie planuje
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn r3_restore_twice_same_item_is_stable() {
    let (_t, l, r, s) = setup();
    put(&l, "a.txt", "v1");
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    put(&l, "a.txt", "v2");
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let it = &trash_list(&j).unwrap()[0];
    let a = trash_restore(&j, "right", &it.stamp, &it.path).unwrap();
    let b = trash_restore(&j, "right", &it.stamp, &it.path).unwrap();
    assert_eq!(fs::read_to_string(r.join(&a)).unwrap(), "v1");
    assert_eq!(fs::read_to_string(r.join(&b)).unwrap(), "v1");
}
