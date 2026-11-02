//! Ekspert 6: magazyn zadań (jobs.json) — rozróżnienie błędu od braku pliku, zachowanie nieznanych zadań.
use std::fs;
use std::path::Path;
use syncron_core::legacy::migrate_data_dir;
use syncron_core::runner::Runner;

fn good(id: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "name": id, "left": "/a", "right": "/b",
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}})
}

fn corrupt_copies(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains("jobs.json.corrupt-")
        })
        .count()
}

#[test]
fn missing_file_is_ok_empty_but_corrupt_is_error_with_path_and_backup() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    assert!(r.load_jobs_checked().unwrap().is_empty());
    fs::write(r.jobs_path(), "[{\"id\":").unwrap();
    let e = r.load_jobs_checked().unwrap_err();
    assert!(e.contains("jobs.json") && e.contains("uszkodzony"), "{e}");
    assert_eq!(corrupt_copies(t.path()), 1);
    // powtórny odczyt nie mnoży kopii
    let _ = r.load_jobs_checked();
    assert_eq!(corrupt_copies(t.path()), 1);
}

#[test]
fn unknown_job_is_skipped_on_load_but_kept_on_save() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    let mut bad = good("z-przyszlosci");
    bad["mode"] = serde_json::json!("TrybZPrzyszlosci");
    fs::write(
        r.jobs_path(),
        serde_json::json!([good("a"), bad]).to_string(),
    )
    .unwrap();
    let mut jobs = r.load_jobs_checked().unwrap();
    assert_eq!(jobs.len(), 1);
    jobs.push(serde_json::from_value(good("b")).unwrap());
    r.save_jobs(&jobs).unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(r.jobs_path()).unwrap()).unwrap();
    let ids: Vec<&str> = raw
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "b", "z-przyszlosci"]);
}

#[test]
fn update_jobs_refuses_on_corrupt_file() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    fs::write(r.jobs_path(), "{{{").unwrap();
    assert!(r.update_jobs(Ok).is_err());
    assert_eq!(fs::read_to_string(r.jobs_path()).unwrap(), "{{{");
    assert!(!t.path().join("jobs.json.lock").exists());
}

#[test]
fn bom_is_stripped() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend(serde_json::json!([good("a")]).to_string().into_bytes());
    fs::write(r.jobs_path(), b).unwrap();
    assert_eq!(r.load_jobs_checked().unwrap().len(), 1);
}

#[test]
fn legacy_does_not_mix_state_db_files() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = (
        t.path().join("pl.syncpolak.app"),
        t.path().join("pl.syncron.app"),
    );
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(old.join("jobs.json"), "[]").unwrap();
    fs::write(old.join("state.db"), "stara").unwrap();
    fs::write(old.join("state.db-wal"), "stary-wal").unwrap();
    fs::write(new.join("state.db"), "nowa").unwrap();
    assert!(migrate_data_dir(&new));
    assert_eq!(fs::read_to_string(new.join("state.db")).unwrap(), "nowa");
    assert!(!new.join("state.db-wal").exists());
    assert!(!new.join("state.db.migracja-tmp").exists());
}
