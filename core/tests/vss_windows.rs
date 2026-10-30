//! Test VSS na prawdziwym Windows: plik zablokowany na wyłączność przez inny program ma zostać skopiowany z migawki.
//! Wymaga uprawnień administratora (w GitHub Actions runner je ma); bez nich test jest pomijany.
#![cfg(windows)]
use std::fs::{self, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use syncron_core::engine::run_job;
use syncron_core::model::Job;

fn job(l: &std::path::Path, r: &std::path::Path, vss: bool) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": "vss", "name": "vss", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 1, "use_vss": vss
    }))
    .unwrap()
}

#[test]
fn locked_file_is_copied_from_vss_snapshot() {
    if !syncron_core::vss::is_admin() {
        eprintln!("pominięto: test VSS wymaga uprawnień administratora");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    let locked = l.join("zablokowany żółć.txt");
    fs::write(&locked, "zawartość sprzed blokady").unwrap();
    fs::write(l.join("zwykły.txt"), "ok").unwrap();

    // blokada wyłączna (share_mode 0): inny program nie może otworzyć pliku do odczytu
    let _lock = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&locked)
        .unwrap();

    // bez VSS plik jest „zajęty”, reszta synchronizacji idzie dalej
    let rep = run_job(&job(&l, &r, false), &s, false, false).unwrap();
    assert!(
        rep.busy.iter().any(|p| p.contains("zablokowany")),
        "bez VSS plik powinien być zajęty: {rep:?}"
    );
    assert!(r.join("zwykły.txt").exists());

    // z VSS plik jest kopiowany z migawki
    let rep = run_job(&job(&l, &r, true), &s, false, false).unwrap();
    eprintln!("raport VSS: {rep:?}");
    assert!(
        rep.busy.is_empty() && rep.errors.is_empty(),
        "VSS nie zadziałał: {rep:?}"
    );
    assert_eq!(
        fs::read_to_string(r.join("zablokowany żółć.txt")).unwrap(),
        "zawartość sprzed blokady"
    );
}
