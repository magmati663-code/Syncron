//! Edycja zadania w trakcie trwającego (długiego) przebiegu nie może blokować nowego uruchomienia ani skanowania.
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use syncron_core::model::Job;
use syncron_core::runner::Runner;

fn job(id: &str, l: &Path, r: &Path, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": id, "name": id, "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2,
        "guard_pct": 100
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(v).unwrap()
}

fn wait_until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn edit_during_long_run_does_not_block_new_run() {
    let t = tempfile::tempdir().unwrap();
    let (l, r, r2, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("R2"),
        t.path().join("state"),
    );
    for d in [&l, &r, &r2] {
        fs::create_dir_all(d).unwrap();
    }
    // duży plik + niski limit prędkości = bardzo długi przebieg
    fs::write(l.join("duzy.bin"), vec![7u8; 600 * 1024]).unwrap();
    let runner = Runner::new(s.clone());
    let slow = serde_json::json!({"bandwidth": {"kbps": 8, "off_kbps": 0, "hours": null}});
    runner.start(job("j", &l, &r, slow.clone()));
    assert!(runner.run_now("j", false));
    assert!(
        wait_until(5000, || runner
            .status()
            .get("j")
            .map(|x| x.state == "running")
            .unwrap_or(false)),
        "pierwszy przebieg powinien ruszyć"
    );
    // użytkownik edytuje zadanie (inny folder docelowy, bez limitu prędkości) i uruchamia je ponownie
    fs::remove_file(l.join("duzy.bin")).ok();
    fs::write(l.join("maly.txt"), "x").unwrap();
    let edited = job("j", &l, &r2, serde_json::json!({}));
    runner.start(edited.clone());
    assert!(
        runner.run_now("j", false),
        "run_now po edycji musi przyjąć polecenie"
    );
    assert!(
        wait_until(8000, || r2.join("maly.txt").exists()),
        "po edycji zadanie musi się uruchomić w kilka sekund, a nie czekać na koniec starego przebiegu"
    );
    runner.stop_all();
}

#[test]
fn dry_run_not_blocked_by_running_job() {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    for d in [&l, &r] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(l.join("duzy.bin"), vec![7u8; 600 * 1024]).unwrap();
    let runner = Runner::new(s.clone());
    let slow = serde_json::json!({"bandwidth": {"kbps": 8, "off_kbps": 0, "hours": null}});
    let j = job("j", &l, &r, slow);
    runner.start(j.clone());
    assert!(runner.run_now("j", false));
    assert!(wait_until(5000, || runner
        .status()
        .get("j")
        .map(|x| x.state == "running")
        .unwrap_or(false)));
    let started = Instant::now();
    let rep = syncron_core::engine::run_job(&j, &s, true, false).expect("symulacja nie może paść");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "symulacja czekała {:?} na trwający przebieg",
        started.elapsed()
    );
    assert!(rep.dry_run);
    runner.stop_all();
}
