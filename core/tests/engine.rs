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
