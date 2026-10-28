//! QA tester 3: odporność na awarie (zniknięty folder, przerwane kopiowanie, race, symlinki, baza stanu).
//! Testy oznaczone `#[ignore = "BUG: ..."]` odtwarzają błędy znalezione w silniku.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{open_db, run_job, Report};
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

/// Zawartość wszystkich zwykłych plików (bez kosza i plików tymczasowych), klucz = ścieżka względna.
fn snap(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == ".syncron-trash" || n.ends_with(".syncron.tmp") {
                continue;
            }
            let r = if rel.is_empty() {
                n.clone()
            } else {
                format!("{rel}/{n}")
            };
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(&e.path(), &r, out);
            } else if ft.is_file() {
                out.insert(r, fs::read(e.path()).unwrap());
            }
        }
    }
    let mut m = BTreeMap::new();
    walk(root, "", &mut m);
    m
}

/// Wszystkie zawartości plików przechowywane w koszu danego folderu.
fn trash_contents(root: &Path) -> Vec<Vec<u8>> {
    let mut v = vec![];
    fn walk(d: &Path, v: &mut Vec<Vec<u8>>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(&e.path(), v);
            } else if ft.is_file() {
                v.push(fs::read(e.path()).unwrap());
            }
        }
    }
    walk(&root.join(".syncron-trash"), &mut v);
    v
}

fn find_tmp(root: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    fn walk(d: &Path, v: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(&e.path(), v);
            } else if e.file_name().to_string_lossy().ends_with(".syncron.tmp") {
                v.push(e.path());
            }
        }
    }
    walk(root, &mut v);
    v
}

fn run(j: &Job, s: &Path) -> Report {
    run_job(j, s, false, false).unwrap()
}

// ======================= RUNDA 1: podstawy =======================

#[test]
fn r1_missing_left_folder_is_error_and_right_untouched() {
    let (_t, l, r, s) = setup();
    for i in 0..5 {
        fs::write(r.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_dir_all(&l).unwrap();
    let e = run_job(&j, &s, false, false).unwrap_err().to_string();
    assert!(
        e.contains("lewa"),
        "komunikat powinien wskazywać stronę: {e}"
    );
    assert_eq!(snap(&r).len(), 5);
}

#[test]
fn r1_target_replaced_by_regular_file_is_error() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "a").unwrap();
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_dir_all(&r).unwrap();
    fs::write(&r, "to jest plik, nie folder").unwrap();
    assert!(run_job(&j, &s, false, false).is_err());
    assert_eq!(fs::read(l.join("a.txt")).unwrap(), b"a");
    assert_eq!(fs::read(&r).unwrap(), b"to jest plik, nie folder");
}

#[test]
fn r1_empty_left_blocked_symmetrically_twoway() {
    let (_t, l, r, s) = setup();
    for i in 0..6 {
        fs::write(r.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_dir_all(&l).unwrap();
    fs::create_dir_all(&l).unwrap(); // niezamontowany dysk: pusty punkt montowania
    let rep = run(&j, &s);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(snap(&r).len(), 6);
    assert!(snap(&l).is_empty());
}

#[test]
fn r1_empty_source_first_run_oneway_is_blocked() {
    // brak bazy (np. po reset_state), źródło puste, cel pełny: tryb lustrzany nie może wyczyścić celu
    let (_t, l, r, s) = setup();
    for i in 0..10 {
        fs::write(r.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "LeftToRight");
    let rep = run(&j, &s);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(snap(&r).len(), 10);
}

#[test]
fn r1_mass_delete_guard_blocks_and_force_proceeds() {
    let (_t, l, r, s) = setup();
    for i in 0..10 {
        fs::write(l.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    for i in 0..5 {
        fs::remove_file(l.join(format!("f{i}.txt"))).unwrap();
    }
    let rep = run(&j, &s);
    assert!(rep.blocked.is_some());
    assert_eq!(snap(&r).len(), 10);
    let rep = run_job(&j, &s, false, true).unwrap();
    assert_eq!(rep.deleted, 5);
    assert_eq!(snap(&r).len(), 5);
    assert_eq!(trash_contents(&r).len(), 5, "usunięte pliki w koszu");
}

#[test]
fn r1_dry_run_changes_nothing_and_db_stays_empty() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "a").unwrap();
    let j = job(&l, &r, "TwoWay");
    let rep = run_job(&j, &s, true, false).unwrap();
    assert_eq!(rep.planned.len(), 1);
    assert!(snap(&r).is_empty());
    let c = open_db(&s).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM base", [], |x| x.get(0))
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn r1_stale_tmp_cleanup_and_fresh_kept_and_not_synced() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "a").unwrap();
    let old = l.join("sub");
    fs::create_dir_all(&old).unwrap();
    let old = old.join("x.bin.syncron.tmp");
    let fresh = l.join("y.bin.syncron.tmp");
    fs::write(&old, "u").unwrap();
    fs::write(&fresh, "u").unwrap();
    set_mtime(&old, 1_600_000_000);
    run(&job(&l, &r, "TwoWay"), &s);
    assert!(!old.exists(), "stary tmp w podkatalogu sprzątany");
    assert!(fresh.exists());
    assert!(find_tmp(&r).is_empty(), "tmp nie jest synchronizowany");
}

#[test]
fn r1_no_tmp_left_after_success() {
    let (_t, l, r, s) = setup();
    for i in 0..20 {
        fs::write(l.join(format!("f{i}.txt")), vec![i as u8; 50_000]).unwrap();
    }
    run(&job(&l, &r, "TwoWay"), &s);
    assert!(find_tmp(&r).is_empty() && find_tmp(&l).is_empty());
}

// ======================= RUNDA 2: brzegi i złośliwe =======================

#[test]
fn r2_file_replaced_by_dir_converges() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a"), "plik").unwrap();
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_file(l.join("a")).unwrap();
    fs::create_dir_all(l.join("a")).unwrap();
    fs::write(l.join("a/b.txt"), "w katalogu").unwrap();
    fs::write(l.join("z1"), "1").unwrap(); // żeby guard nie zadziałał
    fs::write(l.join("z2"), "1").unwrap();
    fs::write(l.join("z3"), "1").unwrap();
    for _ in 0..3 {
        let _ = run_job(&j, &s, false, true);
    }
    assert_eq!(
        snap(&l),
        snap(&r),
        "strony powinny się zbiec w 3 przebiegach"
    );
    assert_eq!(fs::read(r.join("a/b.txt")).unwrap(), b"w katalogu");
}

#[test]
#[ignore = "BUG: katalog zamieniony na plik zostawia pusty katalog po drugiej stronie; kopiowanie pliku (rename na katalog) zawodzi w każdym przebiegu"]
fn r2_dir_replaced_by_file_converges() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("d")).unwrap();
    fs::write(l.join("d/x.txt"), "x").unwrap();
    fs::write(l.join("z1"), "1").unwrap();
    fs::write(l.join("z2"), "1").unwrap();
    fs::write(l.join("z3"), "1").unwrap();
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_dir_all(l.join("d")).unwrap();
    fs::write(l.join("d"), "teraz plik").unwrap();
    let mut last = Report::default();
    for _ in 0..4 {
        last = run_job(&j, &s, false, true).unwrap();
    }
    assert!(
        last.errors.is_empty(),
        "błędy po 4 przebiegach: {:?}",
        last.errors
    );
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_target_path_is_existing_dir_error_no_loss_no_tmp() {
    // lewa: plik "p", prawa: niepusty katalog "p" (kolizja ścieżki docelowej)
    let (_t, l, r, s) = setup();
    fs::write(l.join("p"), "plik").unwrap();
    fs::create_dir_all(r.join("p")).unwrap();
    fs::write(r.join("p/inner.txt"), "wewn").unwrap();
    let rep = run(&job(&l, &r, "TwoWay"), &s);
    assert!(!rep.errors.is_empty(), "{rep:?}");
    assert_eq!(fs::read(r.join("p/inner.txt")).unwrap(), b"wewn");
    assert_eq!(fs::read(l.join("p")).unwrap(), b"plik");
    assert!(find_tmp(&r).is_empty(), "tmp nie zostaje po błędzie");
}

#[test]
fn r2_parent_of_target_is_a_file_error_no_loss() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("sub")).unwrap();
    fs::write(l.join("sub/a.txt"), "a").unwrap();
    fs::write(r.join("sub"), "plik zamiast katalogu").unwrap();
    let rep = run(&job(&l, &r, "TwoWay"), &s);
    assert!(!rep.errors.is_empty(), "{rep:?}");
    assert_eq!(fs::read(r.join("sub")).unwrap(), b"plik zamiast katalogu");
    assert_eq!(fs::read(l.join("sub/a.txt")).unwrap(), b"a");
}

#[test]
fn r2_source_deleted_during_run_no_panic_and_converges() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("zz_big.bin"), vec![7u8; 400_000]).unwrap();
    fs::write(l.join("aa_small.txt"), "s").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.threads = 1;
    j.bandwidth.kbps = 256; // ~1,5 s na duży plik; kolejka zaczyna od końca (zz_big)
    let victim = l.join("aa_small.txt");
    let h = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        fs::remove_file(victim).unwrap();
    });
    let rep = run_job(&j, &s, false, true).unwrap();
    h.join().unwrap();
    assert_eq!(fs::read(r.join("zz_big.bin")).unwrap().len(), 400_000);
    // plik zniknął przed kopiowaniem: błąd lub pominięcie, ale nie panika; kolejny przebieg czysty
    let _ = rep;
    let rep2 = run_job(&j, &s, false, true).unwrap();
    assert!(rep2.errors.is_empty(), "{rep2:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_file_changed_once_during_copy_is_retried_and_consistent() {
    let (_t, l, r, s) = setup();
    let src = l.join("duzy.bin");
    fs::write(&src, vec![1u8; 300_000]).unwrap();
    set_mtime(&src, 1_700_000_000);
    let mut j = job(&l, &r, "LeftToRight");
    j.bandwidth.kbps = 256 * 2; // 2 wątki: 256 KB/s na transfer
    let s2 = src.clone();
    let h = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        fs::write(&s2, vec![2u8; 300_000]).unwrap();
        set_mtime(&s2, 1_700_000_100);
    });
    let rep = run(&j, &s);
    h.join().unwrap();
    assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
    let dst = fs::read(r.join("duzy.bin")).unwrap();
    assert!(
        dst == vec![2u8; 300_000] || dst == vec![1u8; 300_000],
        "plik docelowy nie może być mieszanką wersji"
    );
    // następny przebieg doprowadza do zgodności z nową wersją
    run(&j, &s);
    assert_eq!(fs::read(r.join("duzy.bin")).unwrap(), vec![2u8; 300_000]);
    assert!(find_tmp(&r).is_empty());
}

#[test]
fn r2_file_changing_all_the_time_is_busy_not_corrupt() {
    let (_t, l, r, s) = setup();
    let src = l.join("log.bin");
    fs::write(&src, vec![1u8; 200_000]).unwrap();
    set_mtime(&src, 1_700_000_000);
    fs::write(r.join("log.bin"), b"stara wersja docelowa").unwrap();
    set_mtime(&r.join("log.bin"), 1_600_000_000);
    let mut j = job(&l, &r, "LeftToRight");
    j.bandwidth.kbps = 512; // 256 KB/s na wątek -> ok. 0,8 s na próbę
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (st, s2) = (stop.clone(), src.clone());
    let h = std::thread::spawn(move || {
        let mut t = 1_700_000_001;
        while !st.load(std::sync::atomic::Ordering::SeqCst) {
            set_mtime(&s2, t);
            t += 1;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
    let rep = run(&j, &s);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    h.join().unwrap();
    assert_eq!(rep.busy.len(), 1, "{rep:?}");
    assert!(rep.needs_retry());
    assert_eq!(
        fs::read(r.join("log.bin")).unwrap(),
        b"stara wersja docelowa",
        "cel nietknięty po nieudanym kopiowaniu"
    );
    assert!(find_tmp(&r).is_empty());
}

#[cfg(unix)]
#[test]
fn r2_symlink_loop_and_dangling_are_ignored() {
    use std::os::unix::fs::symlink;
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("d")).unwrap();
    fs::write(l.join("d/a.txt"), "a").unwrap();
    symlink(&l, l.join("d/petla")).unwrap(); // pętla na korzeń
    symlink("nie_istnieje", l.join("wiszacy")).unwrap();
    symlink("d/a.txt", l.join("lnk.txt")).unwrap();
    let rep = run(&job(&l, &r, "TwoWay"), &s);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(snap(&r).keys().collect::<Vec<_>>(), vec!["d/a.txt"]);
}

#[cfg(unix)]
#[test]
fn r2_dir_replaced_by_symlink_must_not_delete_other_side() {
    use std::os::unix::fs::symlink;
    let (t, l, r, s) = setup();
    fs::create_dir_all(l.join("proj")).unwrap();
    for i in 0..4 {
        fs::write(l.join(format!("proj/f{i}.txt")), format!("tresc{i}")).unwrap();
    }
    for i in 0..6 {
        fs::write(l.join(format!("inne{i}.txt")), "i").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    // użytkownik przenosi katalog gdzie indziej i zostawia symlink
    let elsewhere = t.path().join("gdzie_indziej");
    fs::rename(r.join("proj"), &elsewhere).unwrap();
    symlink(&elsewhere, r.join("proj")).unwrap();
    let _ = run_job(&j, &s, false, true);
    let _ = run_job(&j, &s, false, true);
    assert_eq!(
        fs::read(l.join("proj/f0.txt")).unwrap(),
        b"tresc0",
        "pliki lewej strony nie mogą zniknąć z powodu symlinku po prawej"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "BUG: kopiowanie podąża za symlinkiem katalogu po stronie docelowej i zapisuje poza folderem synchronizacji"]
fn r2_copy_through_symlinked_dir_escapes_root() {
    use std::os::unix::fs::symlink;
    let (t, l, r, s) = setup();
    let outside = t.path().join("poza");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, r.join("sub")).unwrap();
    fs::create_dir_all(l.join("sub")).unwrap();
    fs::write(l.join("sub/x.txt"), "x").unwrap();
    run(&job(&l, &r, "LeftToRight"), &s);
    assert!(
        !outside.join("x.txt").exists(),
        "zapis poza korzeniem synchronizacji przez symlink"
    );
}

#[cfg(unix)]
#[test]
fn r2_file_replaced_by_symlink_is_recoverable_from_trash_or_kept() {
    // plik zamieniony na symlink po lewej: wygląda na usunięty. Dane prawej muszą pozostać (kosz).
    use std::os::unix::fs::symlink;
    let (_t, l, r, s) = setup();
    for i in 0..8 {
        fs::write(l.join(format!("f{i}.txt")), format!("c{i}")).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_file(l.join("f0.txt")).unwrap();
    symlink("f1.txt", l.join("f0.txt")).unwrap();
    run(&j, &s);
    let kept = r.join("f0.txt").exists() || trash_contents(&r).contains(&b"c0".to_vec());
    assert!(kept, "treść f0 przepadła");
}

#[test]
fn r2_deep_tree_syncs_and_converges() {
    let (_t, l, r, s) = setup();
    let mut p = l.clone();
    for i in 0..120 {
        p.push(format!("poziom_{i:03}_żółć"));
    }
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("gleboko.txt"), "głęboko").unwrap();
    let j = job(&l, &r, "TwoWay");
    let rep = run(&j, &s);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
    assert_eq!(snap(&r).len(), 1);
    assert!(run(&j, &s).planned.is_empty());
}

#[test]
#[ignore = "BUG: plik lub podkatalog o nazwie .syncron-trash jest po cichu pomijany na każdym poziomie (dane użytkownika nie są synchronizowane, bez ostrzeżenia)"]
fn r2_user_item_named_like_trash_dir_is_synced_or_reported() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("sub/.syncron-trash")).unwrap();
    fs::write(l.join("sub/.syncron-trash/dane.txt"), "ważne").unwrap();
    fs::write(l.join("normalny.txt"), "n").unwrap();
    let rep = run(&job(&l, &r, "TwoWay"), &s);
    let synced = r.join("sub/.syncron-trash/dane.txt").exists();
    assert!(synced || !rep.errors.is_empty(), "cicho pominięte: {rep:?}");
}

#[test]
fn r2_partial_failure_records_only_successes_and_heals() {
    let (_t, l, r, s) = setup();
    for n in ["a", "b", "c", "d"] {
        fs::write(l.join(format!("{n}.txt")), n).unwrap();
    }
    // blokada: po prawej "c.txt" to niepusty katalog
    fs::create_dir_all(r.join("c.txt")).unwrap();
    fs::write(r.join("c.txt/x"), "x").unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.guard_pct = 100;
    let rep = run(&j, &s);
    assert_eq!(rep.copied, 3, "{rep:?}");
    assert_eq!(rep.errors.len(), 1, "{rep:?}");
    let c = open_db(&s).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM base WHERE key='c.txt'", [], |x| {
            x.get(0)
        })
        .unwrap();
    assert_eq!(n, 0, "nieudany plik nie może trafić do bazy");
    // uzdrowienie
    fs::remove_dir_all(r.join("c.txt")).unwrap();
    let rep = run(&j, &s);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(fs::read(r.join("c.txt")).unwrap(), b"c");
}

#[test]
fn r2_concurrent_runs_same_job_do_not_corrupt() {
    let (_t, l, r, s) = setup();
    for i in 0..30 {
        fs::write(l.join(format!("f{i}.bin")), vec![i as u8; 100_000]).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    let (j1, j2, s1, s2) = (j.clone(), j.clone(), s.clone(), s.clone());
    let a = std::thread::spawn(move || run_job(&j1, &s1, false, false));
    let b = std::thread::spawn(move || run_job(&j2, &s2, false, false));
    let (ra, rb) = (a.join().unwrap(), b.join().unwrap());
    let _ = (&ra, &rb); // błędy dopuszczalne, uszkodzenie danych nie
    for i in 0..30 {
        if let Ok(d) = fs::read(r.join(format!("f{i}.bin"))) {
            assert_eq!(d, vec![i as u8; 100_000], "uszkodzony plik f{i}");
        }
    }
    // po jednym spokojnym przebiegu pełna zgodność
    let rep = run(&j, &s);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
    assert!(find_tmp(&r).is_empty());
}

#[test]
fn r2_corrupt_state_db_is_error_not_data_loss() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "a").unwrap();
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    for f in ["state.db-wal", "state.db-shm"] {
        let _ = fs::remove_file(s.join(f));
    }
    fs::write(
        s.join("state.db"),
        "to nie jest baza sqlite, tylko śmieci........",
    )
    .unwrap();
    let res = run_job(&j, &s, false, false);
    assert!(res.is_err());
    assert_eq!(snap(&l).len(), 1);
    assert_eq!(snap(&r).len(), 1);
}

#[test]
fn r2_lost_state_db_restores_deleted_file_instead_of_deleting() {
    let (_t, l, r, s) = setup();
    for i in 0..4 {
        fs::write(l.join(format!("f{i}.txt")), format!("{i}")).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    fs::remove_dir_all(&s).unwrap();
    fs::remove_file(l.join("f0.txt")).unwrap(); // bez bazy usunięcie jest nierozróżnialne od nowego pliku
    let rep = run(&j, &s);
    assert_eq!(rep.deleted, 0, "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
}

#[test]
fn r2_stale_state_db_after_crash_before_commit_is_harmless() {
    // symulacja awarii po wykonaniu akcji, ale przed zapisem bazy: przywracamy starą bazę
    let (t, l, r, s) = setup();
    for i in 0..6 {
        fs::write(l.join(format!("f{i}.txt")), format!("v1-{i}")).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    let backup = t.path().join("state_backup");
    fs::create_dir_all(&backup).unwrap();
    {
        let c = open_db(&s).unwrap();
        c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    }
    fs::copy(s.join("state.db"), backup.join("state.db")).unwrap();
    fs::write(l.join("f1.txt"), "v2-1").unwrap();
    set_mtime(&l.join("f1.txt"), 1_900_000_000);
    fs::remove_file(l.join("f2.txt")).unwrap();
    fs::write(r.join("nowy.txt"), "nowy").unwrap();
    run(&j, &s);
    for f in ["state.db-wal", "state.db-shm"] {
        let _ = fs::remove_file(s.join(f));
    }
    fs::copy(backup.join("state.db"), s.join("state.db")).unwrap();
    let rep = run(&j, &s);
    assert!(rep.errors.is_empty() && rep.conflicts.is_empty(), "{rep:?}");
    assert_eq!(snap(&l), snap(&r));
    assert_eq!(fs::read(l.join("f1.txt")).unwrap(), b"v2-1");
    assert!(!l.join("f2.txt").exists());
    assert_eq!(fs::read(l.join("nowy.txt")).unwrap(), b"nowy");
}

#[test]
fn r2_keep_both_same_day_second_conflict_does_not_lose_first_version() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "baza").unwrap();
    set_mtime(&l.join("a.txt"), 1_700_000_000);
    let mut j = job(&l, &r, "TwoWay");
    j.conflict = serde_json::from_str("\"KeepBoth\"").unwrap();
    run(&j, &s);
    // konflikt 1
    fs::write(l.join("a.txt"), "lewa-1").unwrap();
    set_mtime(&l.join("a.txt"), 1_800_000_000);
    fs::write(r.join("a.txt"), "prawa-1-UNIKAT").unwrap();
    set_mtime(&r.join("a.txt"), 1_800_000_100);
    run(&j, &s);
    // konflikt 2 tego samego dnia
    fs::write(l.join("a.txt"), "lewa-2").unwrap();
    set_mtime(&l.join("a.txt"), 1_800_001_000);
    fs::write(r.join("a.txt"), "prawa-2").unwrap();
    set_mtime(&r.join("a.txt"), 1_800_001_100);
    run(&j, &s);
    run(&j, &s);
    let all: Vec<Vec<u8>> = snap(&l)
        .into_values()
        .chain(snap(&r).into_values())
        .chain(trash_contents(&l))
        .chain(trash_contents(&r))
        .collect();
    assert!(
        all.contains(&b"prawa-1-UNIKAT".to_vec()),
        "pierwsza wersja z konfliktu przepadła"
    );
}

#[cfg(target_os = "linux")]
struct Tmpfs(PathBuf);
#[cfg(target_os = "linux")]
impl Tmpfs {
    /// Montuje mały tmpfs (wymaga roota); None = nie da się, test jest pomijany.
    fn mount(p: &Path, kb: u32) -> Option<Tmpfs> {
        let ok = std::process::Command::new("mount")
            .args(["-t", "tmpfs", "-o", &format!("size={kb}k"), "tmpfs"])
            .arg(p)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ok.then(|| Tmpfs(p.to_path_buf()))
    }
}
#[cfg(target_os = "linux")]
impl Drop for Tmpfs {
    fn drop(&mut self) {
        let _ = std::process::Command::new("umount")
            .arg("-l")
            .arg(&self.0)
            .status();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn r2_disk_full_leaves_no_tmp_and_keeps_old_destination() {
    let (_t, l, r, s) = setup();
    let Some(_m) = Tmpfs::mount(&r, 300) else {
        eprintln!("pominięto: brak możliwości zamontowania tmpfs");
        return;
    };
    fs::write(r.join("a.bin"), b"stara wersja").unwrap();
    set_mtime(&r.join("a.bin"), 1_600_000_000);
    fs::write(l.join("a.bin"), vec![9u8; 600_000]).unwrap(); // nie zmieści się
    fs::write(l.join("male.txt"), "mały").unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.trash_days = 0;
    let rep = run(&j, &s);
    assert_eq!(rep.errors.len() + rep.busy.len(), 1, "{rep:?}");
    assert!(
        rep.busy.is_empty(),
        "brak miejsca to nie 'plik zajęty': {rep:?}"
    );
    assert_eq!(fs::read(r.join("a.bin")).unwrap(), b"stara wersja");
    assert!(
        find_tmp(&r).is_empty(),
        "niedokończony tmp zostaje na pełnym dysku"
    );
    assert_eq!(fs::read(r.join("male.txt")).unwrap(), "mały".as_bytes());
}

#[cfg(target_os = "linux")]
#[test]
fn r2_disk_full_trash_backup_failure_keeps_old_destination() {
    // kosz włączony: kopia zapasowa starej wersji też zajmuje miejsce
    let (_t, l, r, s) = setup();
    let Some(_m) = Tmpfs::mount(&r, 300) else {
        return;
    };
    fs::write(r.join("a.bin"), vec![1u8; 150_000]).unwrap();
    set_mtime(&r.join("a.bin"), 1_600_000_000);
    fs::write(l.join("a.bin"), vec![2u8; 150_000]).unwrap();
    let j = job(&l, &r, "LeftToRight");
    let _ = run_job(&j, &s, false, false);
    let d = fs::read(r.join("a.bin")).unwrap();
    assert!(
        d == vec![1u8; 150_000] || d == vec![2u8; 150_000],
        "plik docelowy uszkodzony/ucięty ({} B)",
        d.len()
    );
    assert!(find_tmp(&r).is_empty());
}

// ======================= RUNDA 3: regresja / obciążenie =======================

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const POOL: [&str; 9] = [
    "a.txt",
    "b.txt",
    "d1/c.txt",
    "d1/d2/e.txt",
    "ż ó/ł.txt",
    "f.bin",
    "g.bin",
    "d1/h.txt",
    "ś/ć/ń.txt",
];

/// Losowe zaburzenia (zniknięcie folderu, śmieciowe tmp, cofnięta baza, ręczne zmiany) z ustalonym ziarnem.
/// Zwraca końcowy stan lewej strony. Sprawdza: brak fałszywych usunięć, brak błędnej zawartości, zbieżność.
fn chaos(seed: u64) -> BTreeMap<String, Vec<u8>> {
    let (t, l, r, s) = setup();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let j = job(&l, &r, "TwoWay");
    let mut clock: i64 = 1_700_000_000;
    // ostatnia operacja użytkownika na ścieżce: Some(zawartość) = zapis, None = usunięcie
    let mut last: BTreeMap<&str, Option<Vec<u8>>> = BTreeMap::new();
    let stale = t.path().join("stale_state.db");
    let mut have_stale = false;

    for step in 0..60 {
        match rng.below(10) {
            0..=3 => {
                let p = POOL[rng.below(POOL.len())];
                let side = if rng.below(2) == 0 { &l } else { &r };
                let f = side.join(p);
                fs::create_dir_all(f.parent().unwrap()).unwrap();
                let body = format!("{seed}-{step}-{p}-{}", "x".repeat(rng.below(3000)));
                fs::write(&f, body.as_bytes()).unwrap();
                clock += 10;
                set_mtime(&f, clock);
                last.insert(p, Some(body.into_bytes()));
            }
            4 | 5 => {
                let p = POOL[rng.below(POOL.len())];
                let side = if rng.below(2) == 0 { &l } else { &r };
                if fs::remove_file(side.join(p)).is_ok() {
                    last.insert(p, None);
                }
            }
            6 => {
                // folder docelowy znika (zerwane połączenie), potem wraca z tą samą zawartością
                let bak = t.path().join(format!("bak{step}"));
                fs::rename(&r, &bak).unwrap();
                assert!(run_job(&j, &s, false, true).is_err(), "krok {step}");
                fs::rename(&bak, &r).unwrap();
            }
            7 => {
                let dir = if rng.below(2) == 0 { &l } else { &r };
                let f = dir.join(format!("zombie{step}.bin.syncron.tmp"));
                fs::write(&f, "ucięty").unwrap();
                if rng.below(2) == 0 {
                    set_mtime(&f, 1_500_000_000);
                }
            }
            8 => {
                // zapamiętaj albo przywróć starą bazę (symulacja awarii przed zapisem stanu)
                if have_stale && rng.below(2) == 0 {
                    for f in ["state.db-wal", "state.db-shm"] {
                        let _ = fs::remove_file(s.join(f));
                    }
                    fs::copy(&stale, s.join("state.db")).unwrap();
                } else {
                    let c = open_db(&s).unwrap();
                    c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
                    drop(c);
                    fs::copy(s.join("state.db"), &stale).unwrap();
                    have_stale = true;
                }
            }
            _ => {
                let rep = run_job(&j, &s, false, false).unwrap();
                assert!(rep.errors.is_empty(), "krok {step}: {:?}", rep.errors);
            }
        }
    }
    // uzdrowienie: kilka przebiegów z potwierdzeniem
    let mut rep = Report::default();
    for _ in 0..5 {
        rep = run_job(&j, &s, false, true).unwrap();
        if rep.planned.is_empty() {
            break;
        }
    }
    assert!(
        rep.planned.is_empty(),
        "seed {seed}: brak zbieżności: {rep:?}"
    );
    assert!(rep.errors.is_empty(), "seed {seed}: {:?}", rep.errors);
    let (sl, sr) = (snap(&l), snap(&r));
    assert_eq!(sl, sr, "seed {seed}: strony się różnią");
    for (p, want) in &last {
        match (want, sl.get(*p)) {
            (Some(w), Some(got)) => assert_eq!(got, w, "seed {seed}: zła zawartość {p}"),
            (Some(_), None) => panic!("seed {seed}: plik {p} zniknął mimo ostatniego zapisu"),
            (None, Some(_)) | (None, None) => {} // usunięcie mogło przegrać z wcześniejszą zmianą po drugiej stronie
        }
    }
    assert!(find_tmp(&l).iter().all(|p| p.metadata().is_ok()));
    sl
}

#[test]
fn r3_chaos_converges_without_loss_and_is_repeatable() {
    for seed in 1..=15u64 {
        let a = chaos(seed);
        let b = chaos(seed);
        assert_eq!(a, b, "seed {seed}: wynik niepowtarzalny");
    }
}

#[test]
#[ignore = "BUG: kosz używa znacznika z dokładnością do sekundy; dwa nadpisania tego samego pliku w tej samej sekundzie nadpisują poprzednią wersję w koszu (utrata historii wersji)"]
fn r3_two_overwrites_in_same_second_keep_both_old_versions_in_trash() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "wersja-1").unwrap();
    set_mtime(&l.join("a.txt"), 1_700_000_000);
    let mut j = job(&l, &r, "LeftToRight");
    j.guard_pct = 100;
    run(&j, &s);
    fs::write(l.join("a.txt"), "wersja-2").unwrap();
    set_mtime(&l.join("a.txt"), 1_700_000_100);
    run(&j, &s);
    fs::write(l.join("a.txt"), "wersja-3").unwrap();
    set_mtime(&l.join("a.txt"), 1_700_000_200);
    run(&j, &s);
    let tr = trash_contents(&r);
    assert!(
        tr.contains(&b"wersja-1".to_vec()) && tr.contains(&b"wersja-2".to_vec()),
        "w koszu: {:?}",
        tr.iter()
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn r3_repeated_runs_are_idempotent_after_heal() {
    let (_t, l, r, s) = setup();
    for i in 0..25 {
        let p = l.join(format!("d{}/f{i}.txt", i % 4));
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, format!("{i}")).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run(&j, &s);
    let before = (snap(&l), snap(&r));
    for _ in 0..5 {
        let rep = run(&j, &s);
        assert!(rep.planned.is_empty(), "{rep:?}");
    }
    assert_eq!(before, (snap(&l), snap(&r)));
}

// ======================= Uprawnienia (uruchamiane jako użytkownik nobody, bo root omija uprawnienia) =======================

#[cfg(unix)]
mod perms {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn chmod(p: &Path, mode: u32) {
        fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// true = ciało testu ma się wykonać w tym procesie. Jako root test uruchamia sam siebie jako `nobody`.
    fn as_nobody(test: &str) -> bool {
        if std::env::var("QA_NOBODY").is_ok() {
            return true;
        }
        let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
        let root = status
            .lines()
            .find(|l| l.starts_with("Uid:"))
            .map(|l| l.split_whitespace().nth(2) == Some("0"))
            .unwrap_or(false);
        if !root {
            return true; // zwykły użytkownik: uprawnienia działają
        }
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new("setpriv")
            .args(["--reuid=65534", "--regid=65534", "--clear-groups"])
            .arg(exe)
            .args(["--exact", test, "--nocapture", "--include-ignored"])
            .env("QA_NOBODY", "1")
            .output();
        match out {
            Ok(o) => {
                assert!(
                    o.status.success(),
                    "podproces (nobody) nie przeszedł:\n{}\n{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => eprintln!("pominięto (brak setpriv): {e}"),
        }
        false
    }

    #[test]
    fn unreadable_subdir_aborts_run_without_false_deletions() {
        if !as_nobody("perms::unreadable_subdir_aborts_run_without_false_deletions") {
            return;
        }
        let (_t, l, r, s) = setup();
        fs::create_dir_all(l.join("tajne")).unwrap();
        for i in 0..5 {
            fs::write(l.join(format!("tajne/f{i}.txt")), "x").unwrap();
        }
        fs::write(l.join("jawny.txt"), "j").unwrap();
        let j = job(&l, &r, "TwoWay");
        run(&j, &s);
        chmod(&l.join("tajne"), 0o000);
        let res = run_job(&j, &s, false, true); // nawet z force: brak dostępu nie może oznaczać usunięcia
        chmod(&l.join("tajne"), 0o755);
        assert!(res.is_err(), "oczekiwano błędu skanu, a było {res:?}");
        assert_eq!(snap(&r).len(), 6, "prawa strona nietknięta");
        assert_eq!(snap(&l).len(), 6);
        // po przywróceniu dostępu wszystko znów działa i nic nie zostało usunięte
        let rep = run(&j, &s);
        assert_eq!(rep.deleted, 0);
        assert!(trash_contents(&r).is_empty());
    }

    #[test]
    fn unreadable_subdir_on_right_no_false_delete_on_left() {
        if !as_nobody("perms::unreadable_subdir_on_right_no_false_delete_on_left") {
            return;
        }
        let (_t, l, r, s) = setup();
        fs::create_dir_all(l.join("sub")).unwrap();
        fs::write(l.join("sub/a.txt"), "a").unwrap();
        fs::write(l.join("b.txt"), "b").unwrap();
        let j = job(&l, &r, "TwoWay");
        run(&j, &s);
        chmod(&r.join("sub"), 0o000);
        let res = run_job(&j, &s, false, true);
        chmod(&r.join("sub"), 0o755);
        assert!(res.is_err());
        assert_eq!(fs::read(l.join("sub/a.txt")).unwrap(), b"a");
    }

    #[test]
    #[ignore = "BUG: brak uprawnień do odczytu pliku jest klasyfikowany jako 'plik zajęty' (PermissionDenied = Busy): zła etykieta, 4 ponowienia i ~6 s zwłoki na plik"]
    fn unreadable_source_file_is_error_not_busy() {
        if !as_nobody("perms::unreadable_source_file_is_error_not_busy") {
            return;
        }
        let (_t, l, r, s) = setup();
        fs::write(l.join("sekret.txt"), "x").unwrap();
        fs::write(l.join("ok.txt"), "ok").unwrap();
        chmod(&l.join("sekret.txt"), 0o000);
        let started = std::time::Instant::now();
        let rep = run(&job(&l, &r, "TwoWay"), &s);
        chmod(&l.join("sekret.txt"), 0o644);
        assert_eq!(fs::read(r.join("ok.txt")).unwrap(), b"ok");
        assert!(
            rep.busy.is_empty() && rep.errors.len() == 1,
            "brak uprawnień to błąd, nie zajętość: {rep:?}"
        );
        assert!(started.elapsed().as_secs() < 3, "zbędne ponowienia");
    }

    #[test]
    fn read_only_target_dir_is_reported_and_nothing_lost() {
        if !as_nobody("perms::read_only_target_dir_is_reported_and_nothing_lost") {
            return;
        }
        let (_t, l, r, s) = setup();
        fs::write(l.join("a.txt"), "a").unwrap();
        chmod(&r, 0o555);
        let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false);
        chmod(&r, 0o755);
        let rep = rep.unwrap();
        assert!(rep.needs_retry(), "{rep:?}");
        assert_eq!(rep.copied, 0);
        assert!(find_tmp(&r).is_empty());
        assert_eq!(fs::read(l.join("a.txt")).unwrap(), b"a");
        // po naprawie uprawnień następny przebieg synchronizuje
        let rep = run(&job(&l, &r, "LeftToRight"), &s);
        assert_eq!(rep.copied, 1, "{rep:?}");
    }

    #[test]
    fn read_only_target_dir_blocks_delete_but_keeps_data() {
        if !as_nobody("perms::read_only_target_dir_blocks_delete_but_keeps_data") {
            return;
        }
        let (_t, l, r, s) = setup();
        for i in 0..4 {
            fs::write(l.join(format!("f{i}.txt")), "x").unwrap();
        }
        let j = job(&l, &r, "TwoWay");
        run(&j, &s);
        fs::remove_file(l.join("f0.txt")).unwrap();
        chmod(&r, 0o555);
        let rep = run_job(&j, &s, false, true);
        chmod(&r, 0o755);
        let rep = rep.unwrap();
        assert_eq!(rep.deleted, 0, "{rep:?}");
        assert!(r.join("f0.txt").exists(), "plik po prawej nie może zniknąć");
        // po naprawie usunięcie jest ponawiane (stan w bazie zachowany)
        let rep = run_job(&j, &s, false, true).unwrap();
        assert_eq!(rep.deleted, 1, "{rep:?}");
    }

    #[test]
    fn read_only_dest_file_is_replaced_atomically() {
        if !as_nobody("perms::read_only_dest_file_is_replaced_atomically") {
            return;
        }
        let (_t, l, r, s) = setup();
        fs::write(l.join("a.txt"), "nowa").unwrap();
        set_mtime(&l.join("a.txt"), 1_800_000_000);
        fs::write(r.join("a.txt"), "stara").unwrap();
        set_mtime(&r.join("a.txt"), 1_700_000_000);
        chmod(&r.join("a.txt"), 0o444);
        let rep = run(&job(&l, &r, "LeftToRight"), &s);
        assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
        assert_eq!(fs::read(r.join("a.txt")).unwrap(), b"nowa");
    }
}
