//! Test integracyjny ścieżki rclone: lokalizacja zdalna typu `local` obsługiwana przez prawdziwy rclone.
//! Pomijany, gdy rclone nie jest zainstalowany (w CI jest instalowany).
use std::fs;
use std::process::Command;
use syncpolak_core::engine::{run_job, trash_list};
use syncpolak_core::model::Job;

fn rclone_ok() -> bool {
    Command::new("rclone")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn set_mtime(p: &std::path::Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

#[test]
fn local_and_rclone_backend_roundtrip() {
    if !rclone_ok() {
        eprintln!("pominięto: brak rclone");
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
    std::env::set_var("RCLONE_CONFIG", t.path().join("rclone.conf"));
    let st = Command::new("rclone")
        .args(["config", "create", "testloc", "local"])
        .status()
        .unwrap();
    assert!(
        st.success(),
        "nie udało się utworzyć konta testowego rclone"
    );

    let job: Job = serde_json::from_value(serde_json::json!({
        "id": "rc", "name": "rc", "left": l.to_str().unwrap(), "right": format!("testloc:{}", r.display()),
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    }))
    .unwrap();

    // nowe pliki w obie strony (polskie znaki)
    fs::create_dir_all(l.join("Zażółć")).unwrap();
    fs::write(l.join("Zażółć/gęślą jaźń.txt"), "z lewej").unwrap();
    fs::write(r.join("Łódź.txt"), "z prawej").unwrap();
    let rep = run_job(&job, &s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(
        fs::read_to_string(r.join("Zażółć/gęślą jaźń.txt")).unwrap(),
        "z lewej"
    );
    assert_eq!(fs::read_to_string(l.join("Łódź.txt")).unwrap(), "z prawej");
    let again = run_job(&job, &s, false, false).unwrap();
    assert!(
        again.planned.is_empty(),
        "po synchronizacji nie powinno być działań: {again:?}"
    );

    // zmiana pliku: nowa wersja w prawo, stara w koszu
    fs::write(l.join("Zażółć/gęślą jaźń.txt"), "zmieniony tekst").unwrap();
    set_mtime(&l.join("Zażółć/gęślą jaźń.txt"), 2_000_000_000);
    let rep = run_job(&job, &s, false, false).unwrap();
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(
        fs::read_to_string(r.join("Zażółć/gęślą jaźń.txt")).unwrap(),
        "zmieniony tekst"
    );
    let trash = trash_list(&job).unwrap();
    assert!(
        trash
            .iter()
            .any(|i| i.side == "right" && i.path == "Zażółć/gęślą jaźń.txt"),
        "{trash:?}"
    );

    // przeniesienie pliku po lewej: po prawej `moveto`
    fs::create_dir_all(l.join("Nowy")).unwrap();
    fs::rename(l.join("Łódź.txt"), l.join("Nowy/Łódź przeniesiona.txt")).unwrap();
    let rep = run_job(&job, &s, false, false).unwrap();
    assert_eq!(rep.moved, 1, "{rep:?}");
    assert!(r.join("Nowy/Łódź przeniesiona.txt").exists() && !r.join("Łódź.txt").exists());

    // usunięcie po lewej: po prawej plik trafia do kosza (przy kilku plikach, żeby nie zadziałała blokada)
    for i in 0..6 {
        fs::write(l.join(format!("p{i}.txt")), "x").unwrap();
    }
    run_job(&job, &s, false, false).unwrap();
    fs::remove_file(l.join("p0.txt")).unwrap();
    let rep = run_job(&job, &s, false, false).unwrap();
    assert_eq!(rep.deleted, 1, "{rep:?}");
    assert!(!r.join("p0.txt").exists());
}
