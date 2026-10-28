//! QA tester 7: testy rdzenia istotne dla CLI/daemona: migracja ze starego folderu, load_jobs/save_jobs.
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use syncron_core::legacy::migrate_data_dir;
use syncron_core::model::Job;
use syncron_core::runner::Runner;

fn mk_job(id: &str, name: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "left": "/a", "right": "/b",
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}
    }))
    .unwrap()
}

fn old_and_new(t: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let (old, new) = (t.join("pl.syncpolak.app"), t.join("pl.syncron.app"));
    fs::create_dir_all(&old).unwrap();
    (old, new)
}

// ── legacy ──

#[test]
fn legacy_no_old_folder_does_nothing() {
    let t = tempfile::tempdir().unwrap();
    let new = t.path().join("pl.syncron.app");
    assert!(!migrate_data_dir(&new));
    assert!(!new.exists(), "nie wolno tworzyć folderu bez migracji");
}

#[test]
fn legacy_old_without_jobs_json_does_nothing() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = old_and_new(t.path());
    fs::write(old.join("settings.json"), "{}").unwrap();
    assert!(!migrate_data_dir(&new));
    assert!(!new.exists());
}

#[test]
fn legacy_existing_new_jobs_json_is_never_touched() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = old_and_new(t.path());
    fs::write(old.join("jobs.json"), "[1]").unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("jobs.json"), "[]").unwrap();
    assert!(!migrate_data_dir(&new));
    assert_eq!(fs::read_to_string(new.join("jobs.json")).unwrap(), "[]");
}

#[test]
fn legacy_copies_all_files_and_keeps_old() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = old_and_new(t.path());
    for f in [
        "jobs.json",
        "settings.json",
        "rclone.conf",
        "state.db",
        "state.db-wal",
        "state.db-shm",
    ] {
        fs::write(old.join(f), format!("stare-{f}-ąęł")).unwrap();
    }
    assert!(migrate_data_dir(&new));
    for f in [
        "jobs.json",
        "settings.json",
        "rclone.conf",
        "state.db",
        "state.db-wal",
        "state.db-shm",
    ] {
        assert_eq!(
            fs::read_to_string(new.join(f)).unwrap(),
            format!("stare-{f}-ąęł")
        );
        assert!(old.join(f).exists());
    }
}

#[test]
fn legacy_relative_path_without_parent_does_not_panic() {
    // parent() == Some("") dla ścieżki względnej; "/" nie ma rodzica
    assert!(!migrate_data_dir(Path::new("/")));
    let _ = migrate_data_dir(Path::new("syncron-qa-nie-istnieje-12345"));
    assert!(!Path::new("syncron-qa-nie-istnieje-12345").exists());
}

#[test]
#[ignore = "BUG T7-19: migracja nadpisuje istniejące w nowym folderze settings.json/rclone.conf/state.db (nowa wersja zdążyła już zapisać ustawienia/konta), jeśli brak jobs.json"]
fn legacy_does_not_overwrite_existing_files_in_new_dir() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = old_and_new(t.path());
    fs::write(old.join("jobs.json"), "[]").unwrap();
    fs::write(old.join("settings.json"), "STARE").unwrap();
    fs::write(old.join("rclone.conf"), "[stare]").unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("settings.json"), "NOWE-USTAWIENIA").unwrap();
    fs::write(new.join("rclone.conf"), "[nowe-konto]").unwrap();
    migrate_data_dir(&new);
    assert_eq!(
        fs::read_to_string(new.join("settings.json")).unwrap(),
        "NOWE-USTAWIENIA"
    );
    assert_eq!(
        fs::read_to_string(new.join("rclone.conf")).unwrap(),
        "[nowe-konto]"
    );
}

#[test]
#[ignore = "BUG T7-20 (do weryfikacji): migracja pomija stary folder z samym rclone.conf (konta bez zadań) – konta chmur przepadają"]
fn legacy_migrates_accounts_even_without_jobs() {
    let t = tempfile::tempdir().unwrap();
    let (old, new) = old_and_new(t.path());
    fs::write(old.join("rclone.conf"), "[gdrive]").unwrap();
    migrate_data_dir(&new);
    assert!(new.join("rclone.conf").exists());
}

// ── load_jobs / save_jobs ──

#[test]
fn load_jobs_missing_corrupt_and_partial_return_empty_without_panic() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    assert!(r.load_jobs().is_empty());
    fs::write(r.jobs_path(), "").unwrap();
    assert!(r.load_jobs().is_empty());
    fs::write(r.jobs_path(), "[{\"id\":").unwrap();
    assert!(r.load_jobs().is_empty());
    fs::write(r.jobs_path(), [0xFFu8, 0xFE, 0x00]).unwrap();
    assert!(r.load_jobs().is_empty());
}

#[test]
fn save_then_load_roundtrip_with_polish_names() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().join("nie/istnieje/jeszcze"));
    r.save_jobs(&[mk_job("1", "Zażółć gęślą jaźń"), mk_job("2", "日本語 🙂")])
        .unwrap();
    let j = r.load_jobs();
    assert_eq!(j.len(), 2);
    assert_eq!(j[0].name, "Zażółć gęślą jaźń");
    assert!(!r.state_dir().join("jobs.json.tmp").exists());
}

#[test]
#[ignore = "BUG T7-02: load_jobs nie odróżnia „brak/uszkodzony plik” od „pusta lista” – kolejny save_jobs nadpisuje uszkodzony plik (utrata zadań); brak kopii .bak"]
fn save_after_failed_load_does_not_destroy_corrupt_file() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    let corrupt = "[{\"id\":\"a\",\"name\":\"Ważne\",";
    fs::write(r.jobs_path(), corrupt).unwrap();
    let mut jobs = r.load_jobs(); // uszkodzony -> pusta lista
    jobs.push(mk_job("n", "Nowe"));
    r.save_jobs(&jobs).unwrap();
    let kept = fs::read_dir(t.path()).unwrap().flatten().any(|e| {
        fs::read_to_string(e.path())
            .map(|s| s == corrupt)
            .unwrap_or(false)
    });
    assert!(kept, "oryginalna (uszkodzona) treść zniknęła bez kopii");
}

#[test]
#[ignore = "BUG T7-21: save_jobs używa stałej nazwy jobs.json.tmp – równoległe zapisy (GUI + CLI import + daemon) wzajemnie się psują / rename kończy się błędem NotFound"]
fn concurrent_save_jobs_never_fails_nor_corrupts() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().to_path_buf();
    let errors = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let bad_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // czytelnik: po pierwszym udanym zapisie plik nigdy nie może być pusty/niepoprawny
    let reader = {
        let (dir, stop, bad) = (dir.clone(), stop.clone(), bad_reads.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Ok(s) = fs::read_to_string(dir.join("jobs.json")) {
                    if serde_json::from_str::<Vec<Job>>(&s).is_err() {
                        bad.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        })
    };
    let writers: Vec<_> = (0..4)
        .map(|w| {
            let (dir, errors) = (dir.clone(), errors.clone());
            std::thread::spawn(move || {
                let r = Runner::new(dir);
                let jobs: Vec<Job> = (0..200)
                    .map(|i| mk_job(&format!("{w}-{i}"), &"nazwa ąę ".repeat(50)))
                    .collect();
                for _ in 0..150 {
                    if r.save_jobs(&jobs).is_err() {
                        errors.fetch_add(1, Ordering::SeqCst);
                    }
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    stop.store(true, Ordering::SeqCst);
    reader.join().unwrap();
    assert_eq!(
        errors.load(Ordering::SeqCst),
        0,
        "save_jobs zwrócił błąd przy równoległym zapisie"
    );
    assert_eq!(
        bad_reads.load(Ordering::SeqCst),
        0,
        "czytelnik zobaczył niepoprawny jobs.json"
    );
}

#[test]
fn sequential_save_overwrites_atomically_and_removes_tmp() {
    let t = tempfile::tempdir().unwrap();
    let r = Runner::new(t.path().to_path_buf());
    for n in 0..20 {
        let jobs: Vec<Job> = (0..n).map(|i| mk_job(&i.to_string(), "x")).collect();
        r.save_jobs(&jobs).unwrap();
        assert_eq!(r.load_jobs().len(), n);
        assert!(!t.path().join("jobs.json.tmp").exists());
    }
}
