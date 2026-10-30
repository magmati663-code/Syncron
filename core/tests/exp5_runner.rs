//! Testy eksperta 5: runner (forget, monitor, okno [a,a]).
use std::fs;
use std::thread;
use std::time::{Duration, Instant};
use syncron_core::model::Job;
use syncron_core::runner::Runner;

fn mk(id: &str, l: &std::path::Path, r: &std::path::Path, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": id, "name": id, "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn wait(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        if f() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    f()
}

#[cfg(target_os = "linux")]
fn monitors() -> usize {
    fs::read_dir("/proc/self/task")
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    fs::read_to_string(e.path().join("comm"))
                        .map(|c| c.trim() == "system-monitor")
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn forget_removes_status_entry() {
    let t = tempfile::tempdir().unwrap();
    let (l, r) = (t.path().join("L"), t.path().join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    let runner = Runner::new(t.path().join("s"));
    runner.start(mk("e5-forget", &l, &r, serde_json::json!({})));
    assert!(runner.status().contains_key("e5-forget"));
    runner.forget("e5-forget");
    assert!(!runner.status().contains_key("e5-forget"));
    assert!(!runner.run_now("e5-forget", false));
}

#[cfg(target_os = "linux")]
#[test]
fn monitor_is_idempotent_and_stoppable() {
    let runner = Runner::new(tempfile::tempdir().unwrap().path().join("s"));
    runner.start_monitor();
    runner.start_monitor();
    assert!(wait(2000, || monitors() == 1));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(monitors(), 1, "drugi start_monitor nie może dodać wątku");
    runner.stop_monitor();
    assert!(
        wait(3000, || monitors() == 0),
        "monitor powinien się zatrzymać"
    );
    runner.start_monitor();
    assert!(wait(2000, || monitors() == 1), "ponowny start po stop");
    runner.stop_monitor();
}

#[test]
fn degenerate_window_means_always() {
    let t = tempfile::tempdir().unwrap();
    let (l, r) = (t.path().join("L"), t.path().join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(t.path().join("s"));
    runner.start(mk(
        "e5-win",
        &l,
        &r,
        serde_json::json!({"window": [5, 5], "schedule": {"type": "Interval", "secs": 10}}),
    ));
    let ok = wait(6000, || r.join("a.txt").exists());
    runner.stop_all();
    assert!(ok, "okno [a,a] ma oznaczać „zawsze”");
}
