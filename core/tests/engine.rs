use std::fs;
use std::path::Path;
use syncpolak_core::engine::run_job;
use syncpolak_core::model::Job;

fn job(l: &Path, r: &Path, mode: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": "t", "name": "t", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    }))
    .unwrap()
}

fn setup() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
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

#[test]
fn polish_names_two_way() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("Zażółć")).unwrap();
    fs::write(l.join("Zażółć/gęślą jaźń.txt"), "a").unwrap();
    fs::write(r.join("Łódź.txt"), "bbb").unwrap();
    let j = job(&l, &r, "TwoWay");
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(
        fs::read_to_string(r.join("Zażółć/gęślą jaźń.txt")).unwrap(),
        "a"
    );
    assert_eq!(fs::read_to_string(l.join("Łódź.txt")).unwrap(), "bbb");
    // drugi przebieg: nic do zrobienia
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn nfd_on_one_side_no_duplicates() {
    use unicode_normalization::UnicodeNormalization;
    let (_t, l, r, s) = setup();
    let nfc = "żółć.txt";
    let nfd: String = nfc.nfd().collect();
    fs::write(l.join(nfc), "x").unwrap();
    fs::write(r.join(&nfd), "x").unwrap();
    set_mtime(&l.join(nfc), 1_700_000_000);
    set_mtime(&r.join(&nfd), 1_700_000_000);
    let rep = run_job(&job(&l, &r, "TwoWay"), &s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn modify_and_delete_propagate_with_trash() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "1").unwrap();
    fs::write(l.join("b.txt"), "1").unwrap();
    fs::write(l.join("c.txt"), "1").unwrap();
    fs::write(l.join("d.txt"), "1").unwrap();
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    fs::write(r.join("a.txt"), "zmienione").unwrap();
    set_mtime(&r.join("a.txt"), 2_000_000_000);
    fs::remove_file(l.join("b.txt")).unwrap();
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!((rep.copied, rep.deleted), (1, 1), "{rep:?}");
    assert_eq!(fs::read_to_string(l.join("a.txt")).unwrap(), "zmienione");
    assert!(!r.join("b.txt").exists());
    // usunięty plik trafił do kosza
    let trash = r.join(".syncpolak-trash");
    assert!(fs::read_dir(trash).unwrap().next().is_some());
}

#[test]
fn no_delete_right_keeps_and_restores() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "1").unwrap();
    fs::write(l.join("b.txt"), "1").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.no_delete_right = true;
    j.no_delete_left = true;
    run_job(&j, &s, false, false).unwrap();
    fs::remove_file(l.join("a.txt")).unwrap();
    run_job(&j, &s, false, false).unwrap();
    assert!(r.join("a.txt").exists());
    assert!(l.join("a.txt").exists(), "plik wraca na lewą stronę");
}

#[test]
fn one_way_and_dry_run() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "1").unwrap();
    fs::write(r.join("obcy.txt"), "2").unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    let rep = run_job(&j, &s, true, false).unwrap();
    assert!(rep.dry_run && rep.planned.len() == 2);
    assert!(!r.join("a.txt").exists(), "symulacja nic nie zmienia");
    j.no_delete_right = true;
    run_job(&j, &s, false, false).unwrap();
    assert!(r.join("a.txt").exists() && r.join("obcy.txt").exists());
}

#[test]
fn guard_blocks_empty_source_and_mass_delete() {
    let (_t, l, r, s) = setup();
    for i in 0..8 {
        fs::write(l.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    for i in 0..4 {
        fs::remove_file(l.join(format!("f{i}.txt"))).unwrap();
    }
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(
        fs::read_dir(&r)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().is_file())
            .count(),
        8
    );
    // opróżnienie źródła
    for i in 4..8 {
        fs::remove_file(l.join(format!("f{i}.txt"))).unwrap();
    }
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.blocked.is_some());
}

#[test]
fn conflict_keep_both() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "0").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.conflict = syncpolak_core::plan::Conflict::KeepBoth;
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("a.txt"), "lewy").unwrap();
    set_mtime(&l.join("a.txt"), 1_900_000_000);
    fs::write(r.join("a.txt"), "prawy!").unwrap();
    set_mtime(&r.join("a.txt"), 1_900_000_100);
    run_job(&j, &s, false, false).unwrap();
    assert_eq!(fs::read_to_string(r.join("a.txt")).unwrap(), "lewy");
    let names: Vec<String> = fs::read_dir(&l)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n.contains("konflikt")), "{names:?}");
}

#[test]
fn rules_and_temp_files_skipped() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("ok.txt"), "1").unwrap();
    fs::write(l.join("skip.log"), "1").unwrap();
    fs::write(l.join("~$otwarty.docx"), "1").unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.exclude = vec!["*.log".into()];
    run_job(&j, &s, false, false).unwrap();
    assert!(r.join("ok.txt").exists());
    assert!(!r.join("skip.log").exists() && !r.join("~$otwarty.docx").exists());
}

#[test]
fn resolve_conflict_manually() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "0").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.conflict = syncpolak_core::plan::Conflict::Ask;
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("a.txt"), "lewy").unwrap();
    set_mtime(&l.join("a.txt"), 1_900_000_000);
    fs::write(r.join("a.txt"), "prawy!").unwrap();
    set_mtime(&r.join("a.txt"), 1_900_000_100);
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.conflicts.len(), 1);
    syncpolak_core::engine::resolve_conflict(&j, &s, "a.txt", false).unwrap();
    assert_eq!(fs::read_to_string(l.join("a.txt")).unwrap(), "prawy!");
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(
        rep.planned.is_empty() && rep.conflicts.is_empty(),
        "{rep:?}"
    );
}

#[test]
fn progress_is_reported() {
    let (_t, l, r, s) = setup();
    for i in 0..5 {
        fs::write(l.join(format!("f{i}.txt")), "x").unwrap();
    }
    let n = std::sync::atomic::AtomicUsize::new(0);
    let cb = |_d: usize, t: usize, _c: &str| {
        assert_eq!(t, 5);
        n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    };
    syncpolak_core::engine::run_job_with(&job(&l, &r, "TwoWay"), &s, false, false, Some(&cb))
        .unwrap();
    assert_eq!(n.load(std::sync::atomic::Ordering::SeqCst), 5);
}

#[test]
fn overwritten_file_is_backed_up_to_trash() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "stara").unwrap();
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("a.txt"), "nowa wersja").unwrap();
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    assert_eq!(fs::read_to_string(r.join("a.txt")).unwrap(), "nowa wersja");
    let day = fs::read_dir(r.join(".syncpolak-trash"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read_to_string(day.join("a.txt")).unwrap(), "stara");
}

#[test]
fn trash_list_restore_and_delete() {
    use syncpolak_core::engine::{trash_delete, trash_list, trash_restore};
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("Zażółć")).unwrap();
    fs::write(l.join("Zażółć/a.txt"), "v1").unwrap();
    let j = job(&l, &r, "LeftToRight");
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("Zażółć/a.txt"), "wersja 2").unwrap();
    set_mtime(&l.join("Zażółć/a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    let items = trash_list(&j).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].side.as_str(), items[0].path.as_str()),
        ("right", "Zażółć/a.txt")
    );
    // przywrócenie obok istniejącego pliku: osobna nazwa
    let to = trash_restore(&j, "right", &items[0].stamp, &items[0].path).unwrap();
    assert!(to.contains("przywrócono"), "{to}");
    assert_eq!(fs::read_to_string(r.join(&to)).unwrap(), "v1");
    trash_delete(&j, "right", None, None).unwrap();
    assert!(trash_list(&j).unwrap().is_empty());
}

#[test]
fn trash_can_be_disabled() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "1").unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.trash_days = 0;
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("a.txt"), "nowe").unwrap();
    set_mtime(&l.join("a.txt"), 2_000_000_000);
    run_job(&j, &s, false, false).unwrap();
    assert!(
        !r.join(".syncpolak-trash").exists(),
        "przy wyłączonym koszu nic nie jest odkładane"
    );
}

#[test]
fn versions_are_capped() {
    use syncpolak_core::location::Location;
    let (_t, l, r, _s) = setup();
    let loc = Location::parse(r.to_str().unwrap());
    for (i, stamp) in [
        "2026-01-01_000000",
        "2026-01-02_000000",
        "2026-01-03_000000",
    ]
    .iter()
    .enumerate()
    {
        let d = r.join(".syncpolak-trash").join(stamp);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("a.txt"), format!("v{i}")).unwrap();
    }
    let _ = l;
    loc.purge_trash(0, 2);
    let left: Vec<_> = loc.list_trash().unwrap().into_iter().map(|t| t.0).collect();
    assert_eq!(left, vec!["2026-01-02_000000", "2026-01-03_000000"]);
}

#[test]
fn verify_checksum_and_list_dirs() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("Łódź")).unwrap();
    fs::create_dir_all(l.join("Archiwum")).unwrap();
    fs::write(l.join("Łódź/duży.bin"), vec![7u8; 3_000_000]).unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.verify = true;
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(fs::read(r.join("Łódź/duży.bin")).unwrap().len(), 3_000_000);
    let dirs = syncpolak_core::engine::list_dirs(l.to_str().unwrap(), "").unwrap();
    assert_eq!(dirs, vec!["Archiwum", "Łódź"]);
}

#[test]
fn unavailable_target_is_safe_and_recovers() {
    let (_t, l, r, s) = setup();
    for i in 0..6 {
        fs::write(l.join(format!("f{i}.txt")), "x").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    // 1. udział znika (zerwane połączenie / restart serwera): błąd, nic nie jest usuwane
    fs::remove_dir_all(&r).unwrap();
    assert!(run_job(&j, &s, false, false).is_err());
    assert_eq!(
        fs::read_dir(&l)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().is_file())
            .count(),
        6
    );
    // 2. udział wraca pusty (np. niezamontowany dysk): zabezpieczenie blokuje, lewa strona nietknięta
    fs::create_dir_all(&r).unwrap();
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(
        fs::read_dir(&l)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().is_file())
            .count(),
        6
    );
    // 3. po jawnym potwierdzeniu użytkownika pliki wracają na prawą stronę
    let rep = run_job(&j, &s, false, true).unwrap();
    assert!(rep.blocked.is_none());
}

#[test]
fn stale_temp_files_are_cleaned() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "x").unwrap();
    let old = r.join("duży.bin.syncpolak.tmp");
    let fresh = r.join("trwa.bin.syncpolak.tmp");
    fs::write(&old, "ucięty").unwrap();
    fs::write(&fresh, "w toku").unwrap();
    set_mtime(&old, 1_000_000_000);
    run_job(&job(&l, &r, "TwoWay"), &s, false, false).unwrap();
    assert!(!old.exists(), "stary plik tymczasowy powinien zniknąć");
    assert!(fresh.exists(), "świeży (trwające kopiowanie) zostaje");
    assert!(
        !l.join("duży.bin.syncpolak.tmp").exists(),
        "plik tymczasowy nigdy się nie synchronizuje"
    );
}

#[test]
fn moved_file_is_renamed_not_recopied() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("Stary")).unwrap();
    fs::write(l.join("Stary/zażółć.bin"), vec![1u8; 4096]).unwrap();
    fs::write(l.join("inny.txt"), "x").unwrap();
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(r.join("Stary/zażółć.bin")).unwrap().ino()
    };
    fs::create_dir_all(l.join("Nowy/Podfolder")).unwrap();
    fs::rename(
        l.join("Stary/zażółć.bin"),
        l.join("Nowy/Podfolder/przeniesiony.bin"),
    )
    .unwrap();
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert!(!r.join("Stary/zażółć.bin").exists());
    assert_eq!(
        fs::read(r.join("Nowy/Podfolder/przeniesiony.bin"))
            .unwrap()
            .len(),
        4096
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(r.join("Nowy/Podfolder/przeniesiony.bin"))
                .unwrap()
                .ino(),
            ino,
            "to samo i-node: przeniesienie, nie kopia"
        );
    }
    assert!(run_job(&j, &s, false, false).unwrap().planned.is_empty());
}

#[test]
fn ambiguous_move_falls_back_to_copy() {
    let (_t, l, r, s) = setup();
    for n in ["a.bin", "b.bin"] {
        fs::write(l.join(n), vec![9u8; 100]).unwrap();
        set_mtime(&l.join(n), 1_800_000_000);
    }
    for i in 0..12 {
        fs::write(l.join(format!("c{i}.txt")), format!("tresc {i}")).unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    // dwa identyczne z metadanych pliki znikają i dwa takie same się pojawiają: nie zgadujemy par
    fs::remove_file(l.join("a.bin")).unwrap();
    fs::remove_file(l.join("b.bin")).unwrap();
    for n in ["x.bin", "y.bin"] {
        fs::write(l.join(n), vec![9u8; 100]).unwrap();
        set_mtime(&l.join(n), 1_800_000_000);
    }
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.moved, 0, "{rep:?}");
    assert!(r.join("x.bin").exists() && r.join("y.bin").exists() && !r.join("a.bin").exists());
}

#[test]
fn bandwidth_limit_slows_local_copy() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("duży.bin"), vec![3u8; 300 * 1024]).unwrap();
    let mut j = job(&l, &r, "LeftToRight");
    j.bandwidth = syncpolak_core::model::Bandwidth {
        kbps: 200,
        off_kbps: 0,
        hours: None,
    };
    j.threads = 1;
    let t = std::time::Instant::now();
    run_job(&j, &s, false, false).unwrap();
    assert!(
        t.elapsed().as_secs_f64() >= 1.2,
        "300 KB przy 200 KB/s powinno trwać ok. 1,5 s, trwało {:?}",
        t.elapsed()
    );
    assert_eq!(fs::read(r.join("duży.bin")).unwrap().len(), 300 * 1024);
}

#[test]
fn syncignore_file_excludes_from_both_sides() {
    let (_t, l, r, s) = setup();
    fs::write(l.join(".syncignore"), "# kopie\n*.bak\nbuild/\n").unwrap();
    fs::write(l.join("raport.docx"), "x").unwrap();
    fs::write(l.join("stary.bak"), "x").unwrap();
    fs::create_dir_all(l.join("projekt/build")).unwrap();
    fs::write(l.join("projekt/build/wynik.o"), "x").unwrap();
    fs::write(l.join("projekt/kod.rs"), "x").unwrap();
    // maski z pliku po prawej stronie też obowiązują
    fs::write(r.join(".syncignore"), "tajne/\n").unwrap();
    fs::create_dir_all(r.join("tajne")).unwrap();
    fs::write(r.join("tajne/klucz.txt"), "x").unwrap();
    run_job(&job(&l, &r, "TwoWay"), &s, false, false).unwrap();
    assert!(r.join("raport.docx").exists() && r.join("projekt/kod.rs").exists());
    assert!(!r.join("stary.bak").exists() && !r.join("projekt/build/wynik.o").exists());
    assert!(!l.join("tajne/klucz.txt").exists());
}

#[test]
fn conflict_details_report_both_versions() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "0").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.conflict = syncpolak_core::plan::Conflict::Ask;
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("a.txt"), "lewy").unwrap();
    set_mtime(&l.join("a.txt"), 1_900_000_000);
    fs::write(r.join("a.txt"), "prawy!!").unwrap();
    set_mtime(&r.join("a.txt"), 1_900_000_100);
    let rep = run_job(&j, &s, false, false).unwrap();
    let d = &rep.conflict_details[0];
    assert_eq!(d.path, "a.txt");
    assert_eq!((d.left.unwrap().size, d.right.unwrap().size), (4, 7));
    assert!(d.right.unwrap().mtime > d.left.unwrap().mtime);
}
