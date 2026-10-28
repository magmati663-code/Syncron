//! QA tester 6: runner, harmonogram, watcher, warunki.
//!
//! Testy `#[ignore = "BUG: ..."]` odtwarzają błędy znalezione w kodzie produkcyjnym
//! (uruchom je jawnie: `cargo test -p syncron-core --test qa_runner -- --ignored`).
//!
//! Testy liczące wątki całego procesu biorą `exclusive()`, pozostałe `shared()`,
//! dzięki czemu wyniki nie zależą od równoległych testów.
use std::collections::BTreeSet;
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
#[allow(unused_imports)]
use std::sync::{Arc, Barrier, Mutex, Once, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};
use syncron_core::conditions;
use syncron_core::model::{Conditions, Job};
use syncron_core::runner::{mounted_volumes, Runner, Status};

static GLOBAL: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    GLOBAL.read().unwrap_or_else(|e| e.into_inner())
}
#[allow(dead_code)]
fn exclusive() -> RwLockWriteGuard<'static, ()> {
    GLOBAL.write().unwrap_or_else(|e| e.into_inner())
}

// ---------- logger przechwytujący komunikaty runnera ----------
struct Cap(Mutex<Vec<String>>);
static CAP: Cap = Cap(Mutex::new(Vec::new()));
impl log::Log for Cap {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if let Ok(mut v) = self.0.lock() {
            v.push(r.args().to_string());
        }
    }
    fn flush(&self) {}
}
fn init_log() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = log::set_logger(&CAP);
        log::set_max_level(log::LevelFilter::Info);
    });
}
/// Ile przebiegów z pracą (skopiowano/usunięto...) zalogowało zadanie o danej nazwie.
fn productive_runs(name: &str) -> usize {
    let pat = format!("„{name}”: skopiowano");
    CAP.0
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.contains(&pat))
        .count()
}

// ---------- pomocnicze ----------
struct Fx {
    _t: tempfile::TempDir,
    l: PathBuf,
    r: PathBuf,
    s: PathBuf,
}

fn fx() -> Fx {
    init_log();
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    Fx { _t: t, l, r, s }
}

fn job_with(
    id: &str,
    l: &Path,
    r: &Path,
    schedule: serde_json::Value,
    extra: serde_json::Value,
) -> Job {
    let mut v = serde_json::json!({
        "id": id, "name": id, "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "TwoWay", "conflict": "Newer", "schedule": schedule, "threads": 2
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    serde_json::from_value(v).unwrap()
}

fn job(id: &str, l: &Path, r: &Path, schedule: serde_json::Value) -> Job {
    job_with(id, l, r, schedule, serde_json::json!({}))
}

fn realtime() -> serde_json::Value {
    serde_json::json!({"type":"Realtime","poll_secs":600})
}
fn manual() -> serde_json::Value {
    serde_json::json!({"type":"Manual"})
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

fn st(r: &Runner, id: &str) -> Status {
    r.status().get(id).cloned().unwrap_or_default()
}

/// Czeka na zakończenie pierwszego przebiegu (skan startowy).
fn wait_first_run(r: &Runner, id: &str) -> bool {
    wait_until(8000, || st(r, id).last_run.is_some())
}

#[cfg(target_os = "linux")]
fn threads_named(prefix: &str) -> usize {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            if let Ok(c) = fs::read_to_string(e.path().join("comm")) {
                if c.trim().starts_with(prefix) {
                    n += 1;
                }
            }
        }
    }
    n
}
#[cfg(target_os = "linux")]
fn all_threads() -> usize {
    fs::read_dir("/proc/self/task")
        .map(|r| r.count())
        .unwrap_or(0)
}

fn hour() -> u32 {
    use chrono::Timelike;
    chrono::Local::now().hour()
}

/// Próbnik stanu: zlicza wejścia w "running" (dolna granica liczby przebiegów) i zbiera widziane stany.
struct Sampler {
    stop: Arc<AtomicBool>,
    h: thread::JoinHandle<(usize, BTreeSet<String>, bool)>,
}
fn sample(runner: &Runner, id: &str) -> Sampler {
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, r2, id) = (stop.clone(), runner.clone(), id.to_string());
    let h = thread::spawn(move || {
        let (mut edges, mut was, mut states) = (0, false, BTreeSet::new());
        let mut bad_progress = false;
        while !s2.load(Ordering::SeqCst) {
            let s = st(&r2, &id);
            let running = s.state == "running";
            if running && !was {
                edges += 1;
            }
            // postęp bez stanu running to niespójność
            if s.progress.is_some() && s.state == "idle" {
                bad_progress = true;
            }
            was = running;
            states.insert(s.state);
            thread::sleep(Duration::from_micros(500));
        }
        (edges, states, bad_progress)
    });
    Sampler { stop, h }
}
impl Sampler {
    fn finish(self) -> (usize, BTreeSet<String>, bool) {
        self.stop.store(true, Ordering::SeqCst);
        self.h.join().unwrap()
    }
}

// =====================================================================
// RUNDA 1: podstawy
// =====================================================================

#[test]
fn r1_realtime_both_directions_and_modify() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-bidir", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-bidir"));
    fs::write(f.r.join("od prawej ąę.txt"), "p1").unwrap();
    assert!(wait_until(8000, || f.l.join("od prawej ąę.txt").exists()));
    fs::write(f.l.join("od lewej.txt"), "l1").unwrap();
    assert!(wait_until(8000, || f.r.join("od lewej.txt").exists()));
    // modyfikacja (dłuższa treść, żeby zmienił się rozmiar)
    fs::write(f.l.join("od lewej.txt"), "l1 zmienione").unwrap();
    assert!(wait_until(8000, || fs::read_to_string(
        f.r.join("od lewej.txt")
    )
    .map(|s| s == "l1 zmienione")
    .unwrap_or(false)));
    runner.stop_all();
}

#[test]
fn r1_manual_does_not_run_by_itself_and_run_now_works() {
    let _g = shared();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-man", &f.l, &f.r, manual()));
    thread::sleep(Duration::from_millis(1500));
    assert!(
        !f.r.join("a.txt").exists(),
        "Manual nie powinien sam startować"
    );
    let s = st(&runner, "q-man");
    assert_eq!(s.state, "idle");
    assert!(s.last_run.is_none() && s.next_run.is_none());
    assert!(runner.run_now("q-man", false));
    assert!(wait_until(8000, || f.r.join("a.txt").exists()));
    assert!(wait_until(3000, || st(&runner, "q-man").last_run.is_some()));
    let s = st(&runner, "q-man");
    assert_eq!(s.state, "idle");
    assert!(
        s.next_run.is_none(),
        "Manual po udanym przebiegu: brak next_run"
    );
    assert_eq!(s.last.as_ref().map(|r| r.copied), Some(1));
    runner.stop_all();
}

#[test]
fn r1_run_now_unknown_or_stopped_returns_false() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    assert!(!runner.run_now("nie-ma", false));
    runner.start(job("q-rn", &f.l, &f.r, manual()));
    assert!(runner.run_now("q-rn", false));
    runner.stop("q-rn");
    assert!(!runner.run_now("q-rn", true));
}

#[test]
fn r1_disabled_job_is_not_started() {
    let _g = shared();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    let j = job_with(
        "q-off",
        &f.l,
        &f.r,
        realtime(),
        serde_json::json!({"enabled": false}),
    );
    runner.start(j);
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(st(&runner, "q-off").state, "disabled");
    assert!(!f.r.join("a.txt").exists());
    assert!(!runner.run_now("q-off", false));
}

#[test]
fn r1_interval_and_poll_zero_are_clamped() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let f2 = fx();
    runner.start(job(
        "q-iv0",
        &f.l,
        &f.r,
        serde_json::json!({"type":"Interval","secs":0}),
    ));
    runner.start(job(
        "q-poll0",
        &f2.l,
        &f2.r,
        serde_json::json!({"type":"Realtime","poll_secs":0}),
    ));
    assert!(wait_first_run(&runner, "q-iv0"));
    assert!(wait_first_run(&runner, "q-poll0"));
    let now = chrono::Local::now().timestamp();
    let iv = st(&runner, "q-iv0").next_run.expect("next_run");
    let po = st(&runner, "q-poll0").next_run.expect("next_run");
    assert!(
        (now + 8..=now + 12).contains(&iv),
        "Interval 0 => min 10 s, jest {}",
        iv - now
    );
    assert!(
        (now + 13..=now + 17).contains(&po),
        "poll 0 => min 15 s, jest {}",
        po - now
    );
    runner.stop_all();
}

#[test]
fn r1_interval_job_runs_periodically() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job(
        "q-iv",
        &f.l,
        &f.r,
        serde_json::json!({"type":"Interval","secs":10}),
    ));
    assert!(wait_first_run(&runner, "q-iv"));
    fs::write(f.l.join("pozniej.txt"), "x").unwrap(); // Interval nie ma watchera
    thread::sleep(Duration::from_millis(1500));
    assert!(
        !f.r.join("pozniej.txt").exists(),
        "Interval nie reaguje natychmiast"
    );
    assert!(wait_until(14000, || f.r.join("pozniej.txt").exists()));
    runner.stop_all();
}

#[test]
fn r1_status_serializes_and_has_next_run() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-ser", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-ser"));
    let s = st(&runner, "q-ser");
    assert!(s.next_run.unwrap() > s.last_run.unwrap());
    serde_json::to_string(&runner.status()).unwrap();
    runner.stop_all();
}

#[test]
fn r1_no_endless_loop_from_own_copies() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-loop", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-loop"));
    for i in 0..5 {
        fs::write(f.l.join(format!("p{i}.txt")), "dane").unwrap();
    }
    assert!(wait_until(8000, || (0..5)
        .all(|i| f.r.join(format!("p{i}.txt")).exists())));
    thread::sleep(Duration::from_millis(4500)); // wygaszenie przebiegów wywołanych własnymi zapisami
    let sm = sample(&runner, "q-loop");
    thread::sleep(Duration::from_millis(5000));
    let (edges, _, _) = sm.finish();
    assert_eq!(
        edges, 0,
        "po synchronizacji nie powinno być kolejnych przebiegów"
    );
    runner.stop_all();
}

#[test]
fn r1_parse_netsh_ssid_cases() {
    use conditions::parse_netsh_ssid as p;
    assert_eq!(
        p("    SSID                   : Dom\r\n    BSSID : aa:bb:cc\r\n"),
        Some("Dom".into())
    );
    assert_eq!(p("    BSSID                  : aa:bb\n"), None);
    assert_eq!(
        p("    SSID                   : Dom:5G\n"),
        Some("Dom:5G".into())
    );
    assert_eq!(p("    SSID                   : \n"), None);
    assert_eq!(
        p("    SSID                   :   z  spacjami  \n"),
        Some("z  spacjami".into())
    );
    assert_eq!(p("\tSSID\t: Tab\n"), Some("Tab".into()));
    assert_eq!(p(""), None);
    // pierwszy interfejs bez SSID, drugi z SSID
    assert_eq!(
        p("Name : Wi-Fi 1\n State : disconnected\n\nName : Wi-Fi 2\n SSID : Biuro\n"),
        Some("Biuro".into())
    );
    assert_eq!(p("SSID: Brak spacji"), Some("Brak spacji".into()));
}

#[test]
fn r1_conditions_defaults_pass() {
    let _g = shared();
    assert!(conditions::check(&Conditions::default()).is_ok());
}

#[test]
fn r1_mounted_volumes_does_not_panic() {
    let v = mounted_volumes();
    for x in &v {
        assert!(!x.is_empty());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn r1_stop_all_ends_threads_quickly() {
    let _g = exclusive();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-thr", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-thr"));
    assert_eq!(threads_named("job-q-thr"), 1);
    let t = Instant::now();
    runner.stop_all();
    assert!(
        t.elapsed() < Duration::from_millis(300),
        "stop nie może blokować"
    );
    assert!(
        wait_until(2000, || threads_named("job-q-thr") == 0),
        "wątek zadania powinien się zakończyć"
    );
}

// ---- warunki (fałszywe iwgetid w PATH; tylko Linux) ----
#[cfg(unix)]
#[test]
fn r1_conditions_block_background_run_but_not_manual() {
    use std::os::unix::fs::PermissionsExt;
    let _g = exclusive();
    let f = fx();
    let bin = f._t.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("iwgetid");
    fs::write(&script, "#!/bin/sh\necho Obca\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&old));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    let j = job_with(
        "q-cond",
        &f.l,
        &f.r,
        realtime(),
        serde_json::json!({"conditions": {"wifi": ["Dom"]}}),
    );
    runner.start(j);
    let blocked = wait_until(4000, || st(&runner, "q-cond").state == "conditions");
    let note = st(&runner, "q-cond").note.unwrap_or_default();
    let copied_bg = f.r.join("a.txt").exists();
    // ręczne uruchomienie omija warunki
    let ran_manual =
        runner.run_now("q-cond", false) && wait_until(8000, || f.r.join("a.txt").exists());
    runner.stop_all();
    // lista Wi-Fi z samych białych znaków = brak ograniczenia (probe zbuforowany z fałszywym iwgetid)
    let c = Conditions {
        ac_only: false,
        wifi: vec!["  ".into(), "".into()],
    };
    let blank_ok = conditions::check(&c).is_ok();
    std::env::set_var("PATH", old);
    assert!(blank_ok);
    assert!(blocked, "stan powinien być 'conditions'");
    assert!(note.contains("Obca"), "komunikat: {note}");
    assert!(!copied_bg);
    assert!(ran_manual);
}

// =====================================================================
// RUNDA 2: brzegi i złośliwe
// =====================================================================

#[cfg(target_os = "linux")]
#[test]
fn r2_start_same_job_many_times_sequential_keeps_single_thread() {
    let _g = exclusive();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let j = job("q-many", &f.l, &f.r, realtime());
    for _ in 0..30 {
        runner.start(j.clone());
    }
    assert!(
        wait_until(5000, || threads_named("job-q-many") == 1),
        "powinien zostać dokładnie 1 wątek, jest {}",
        threads_named("job-q-many")
    );
    runner.stop_all();
    assert!(wait_until(3000, || threads_named("job-q-many") == 0));
}

#[cfg(target_os = "linux")]
#[test]
fn r2_concurrent_start_same_id_leaks_thread() {
    let _g = exclusive();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let j = job("q-race", &f.l, &f.r, realtime());
    for _ in 0..20 {
        let b = Arc::new(Barrier::new(6));
        let hs: Vec<_> = (0..6)
            .map(|_| {
                let (r, j, b) = (runner.clone(), j.clone(), b.clone());
                thread::spawn(move || {
                    b.wait();
                    r.start(j);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }
    runner.stop_all();
    assert!(
        wait_until(4000, || threads_named("job-q-race") == 0),
        "po stop_all zostało {} wątków zadania",
        threads_named("job-q-race")
    );
}

#[test]
fn r2_missing_folder_error_then_recovers_and_watcher_is_rebuilt() {
    let _g = shared();
    let f = fx();
    let missing = f._t.path().join("BRAK");
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-miss", &missing, &f.r, realtime()));
    assert!(wait_until(8000, || st(&runner, "q-miss").state == "error"));
    let s = st(&runner, "q-miss");
    let e = s.error.unwrap_or_default();
    assert!(e.contains("lewa strona"), "komunikat: {e}");
    assert!(
        s.next_run.is_some(),
        "po błędzie musi być zaplanowane ponowienie"
    );
    // folder wraca: ręczne uruchomienie + watcher musi zostać odtworzony
    fs::create_dir_all(&missing).unwrap();
    assert!(runner.run_now("q-miss", false));
    assert!(wait_until(8000, || st(&runner, "q-miss").state == "idle"));
    thread::sleep(Duration::from_millis(300));
    fs::write(missing.join("po powrocie.txt"), "x").unwrap();
    assert!(
        wait_until(8000, || f.r.join("po powrocie.txt").exists()),
        "watcher powinien działać po powrocie folderu (poll 600 s)"
    );
    runner.stop_all();
}

#[test]
fn r2_error_backoff_not_shorter_than_30s() {
    let _g = shared();
    let f = fx();
    let missing = f._t.path().join("BRAK");
    let runner = Runner::new(f.s.clone());
    runner.start(job(
        "q-bo",
        &missing,
        &f.r,
        serde_json::json!({"type":"Interval","secs":10}),
    ));
    assert!(wait_until(8000, || st(&runner, "q-bo").state == "error"));
    let s = st(&runner, "q-bo");
    let d = s.next_run.unwrap() - s.last_run.unwrap();
    runner.stop_all();
    assert!(
        d >= 29,
        "ponowienie po błędzie za {d} s (oczekiwane >= 30 s z backoffem)"
    );
}

#[test]
fn r2_manual_job_is_not_auto_retried_after_error() {
    let _g = shared();
    let f = fx();
    let missing = f._t.path().join("BRAK");
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-manerr", &missing, &f.r, manual()));
    assert!(runner.run_now("q-manerr", false));
    assert!(wait_until(8000, || st(&runner, "q-manerr").state == "error"));
    let s = st(&runner, "q-manerr");
    runner.stop_all();
    assert!(
        s.next_run.is_none(),
        "Manual nie powinien mieć zaplanowanego next_run={:?}",
        s.next_run
    );
}

#[test]
fn r2_pause_blocks_background_but_not_manual_and_resume_works() {
    let _g = shared();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.set_paused(true);
    assert!(runner.is_paused());
    runner.start(job("q-pause", &f.l, &f.r, realtime()));
    assert!(wait_until(4000, || st(&runner, "q-pause").state == "paused"));
    assert!(!f.r.join("a.txt").exists());
    // ręczne omija pauzę
    assert!(runner.run_now("q-pause", false));
    assert!(wait_until(8000, || f.r.join("a.txt").exists()));
    // wznowienie: nowy plik trafia do R najpóźniej po ~10 s (ponowne sprawdzenie pauzy)
    runner.set_paused(true);
    fs::write(f.l.join("b.txt"), "b").unwrap();
    thread::sleep(Duration::from_millis(2500));
    assert!(
        !f.r.join("b.txt").exists(),
        "pauza globalna musi wstrzymać watcher"
    );
    runner.set_paused(false);
    assert!(
        wait_until(14000, || f.r.join("b.txt").exists()),
        "po wznowieniu zmiana nie może zginąć"
    );
    runner.stop_all();
}

#[test]
fn r2_run_now_during_debounce_is_still_manual() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.set_paused(true);
    runner.start(job("q-dbm", &f.l, &f.r, realtime()));
    assert!(wait_until(4000, || st(&runner, "q-dbm").state == "paused"));
    fs::write(f.l.join("a.txt"), "a").unwrap(); // zdarzenie -> debouncing 1,5 s
    thread::sleep(Duration::from_millis(250));
    assert!(runner.run_now("q-dbm", false));
    let ok = wait_until(5000, || f.r.join("a.txt").exists());
    runner.stop_all();
    assert!(
        ok,
        "ręczne uruchomienie w trakcie debouncingu zostało zignorowane przez pauzę"
    );
}

fn prepare_blocked(f: &Fx, id: &str, schedule: serde_json::Value) -> Runner {
    for i in 0..4 {
        fs::write(f.l.join(format!("s{i}.txt")), "dane").unwrap();
    }
    let runner = Runner::new(f.s.clone());
    let is_manual = schedule == manual();
    runner.start(job(id, &f.l, &f.r, schedule));
    if is_manual {
        assert!(runner.run_now(id, false));
    }
    assert!(wait_until(8000, || (0..4)
        .all(|i| f.r.join(format!("s{i}.txt")).exists())));
    assert!(wait_first_run(&runner, id));
    runner
}

#[test]
fn r2_run_now_force_overrides_empty_side_guard() {
    let _g = shared();
    let f = fx();
    let runner = prepare_blocked(&f, "q-frc", manual());
    // symulacja odpiętego dysku: lewa strona pusta
    for i in 0..4 {
        fs::remove_file(f.l.join(format!("s{i}.txt"))).unwrap();
    }
    assert!(runner.run_now("q-frc", false));
    assert!(wait_until(8000, || st(&runner, "q-frc").state == "blocked"));
    assert!(
        f.r.join("s0.txt").exists(),
        "zabezpieczenie nie może usuwać po prawej"
    );
    assert!(runner.run_now("q-frc", true));
    assert!(
        wait_until(8000, || (0..4)
            .all(|i| !f.r.join(format!("s{i}.txt")).exists())),
        "force powinno przepuścić usunięcia"
    );
    assert!(wait_until(5000, || st(&runner, "q-frc").state == "idle"));
    runner.stop_all();
}

#[test]
fn r2_force_run_now_during_debounce_keeps_force() {
    let _g = shared();
    let f = fx();
    let runner = prepare_blocked(&f, "q-frd", realtime());
    for i in 0..4 {
        fs::remove_file(f.l.join(format!("s{i}.txt"))).unwrap();
    }
    assert!(wait_until(8000, || st(&runner, "q-frd").state == "blocked"));
    fs::write(f.r.join("nowy.txt"), "n").unwrap(); // zdarzenie => debouncing
    thread::sleep(Duration::from_millis(250));
    assert!(runner.run_now("q-frd", true));
    let ok = wait_until(5000, || st(&runner, "q-frd").state != "blocked");
    runner.stop_all();
    assert!(ok, "po RunNow(force) stan nadal 'blocked'");
}

fn window_job(id: &str, f: &Fx, w: [u32; 2]) -> Job {
    job_with(id, &f.l, &f.r, realtime(), serde_json::json!({"window": w}))
}

#[test]
fn r2_window_inside_runs_outside_waits_manual_bypasses() {
    let _g = shared();
    let h = hour();
    // okno zawierające teraz i następną godzinę (zabezpieczenie na zmianę godziny w trakcie testu)
    let f = fx();
    fs::write(f.l.join("in.txt"), "x").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(window_job("q-win-in", &f, [h, (h + 2) % 24]));
    assert!(
        wait_until(6000, || f.r.join("in.txt").exists()),
        "w oknie zadanie powinno działać"
    );
    runner.stop_all();

    let f = fx();
    fs::write(f.l.join("out.txt"), "x").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(window_job("q-win-out", &f, [(h + 2) % 24, (h + 4) % 24]));
    thread::sleep(Duration::from_millis(2000));
    assert!(
        !f.r.join("out.txt").exists(),
        "poza oknem zadanie nie startuje"
    );
    assert!(runner.run_now("q-win-out", false));
    assert!(
        wait_until(6000, || f.r.join("out.txt").exists()),
        "ręczne omija okno"
    );
    runner.stop_all();
}

#[test]
fn r2_window_wrapping_midnight_contains_current_hour() {
    let _g = shared();
    let h = hour();
    // okno [od, do) z od > do (przez północ), zawierające h i h+1
    let w = if h == 0 { [0, 23] } else { [h, h - 1] };
    let f = fx();
    fs::write(f.l.join("n.txt"), "x").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(window_job("q-win-wrap", &f, w));
    assert!(
        wait_until(6000, || f.r.join("n.txt").exists()),
        "okno {w:?} powinno obejmować godzinę {h}"
    );
    runner.stop_all();
}

#[test]
fn r2_out_of_window_status_explains_waiting() {
    let _g = shared();
    let h = hour();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(window_job("q-win-st", &f, [(h + 2) % 24, (h + 4) % 24]));
    thread::sleep(Duration::from_millis(1500));
    let s = st(&runner, "q-win-st");
    runner.stop_all();
    assert!(
        s.note.is_some() || s.next_run.is_some(),
        "stan={:?} note={:?} next={:?}",
        s.state,
        s.note,
        s.next_run
    );
}

#[test]
fn r2_paused_status_has_next_run() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.set_paused(true);
    runner.start(job("q-pnx", &f.l, &f.r, realtime()));
    assert!(wait_until(4000, || st(&runner, "q-pnx").state == "paused"));
    let s = st(&runner, "q-pnx");
    runner.stop_all();
    assert!(s.next_run.is_some());
}

#[test]
fn r2_daily_next_run_lands_exactly_on_minute() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let h = (hour() + 5) % 24;
    runner.start(job(
        "q-daily",
        &f.l,
        &f.r,
        serde_json::json!({"type":"Daily","hour":h,"minute":0}),
    ));
    assert!(wait_first_run(&runner, "q-daily"));
    let n = st(&runner, "q-daily").next_run.unwrap();
    runner.stop_all();
    assert_eq!(
        n % 60,
        0,
        "next_run wypada {} s przed pełną minutą",
        60 - n % 60
    );
}

#[test]
fn r2_daily_next_run_is_in_future_and_within_a_day() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let now = chrono::Local::now().timestamp();
    // godzina 24 i minuty > 59 nie mogą panikować ani dawać terminu z przeszłości
    for (id, h, m) in [("q-d24", 24u32, 0u32), ("q-d99", 99, 99), ("q-d0", 0, 0)] {
        let f = fx();
        runner.start(job(
            id,
            &f.l,
            &f.r,
            serde_json::json!({"type":"Daily","hour":h,"minute":m}),
        ));
        assert!(wait_first_run(&runner, id), "{id}");
        let n = st(&runner, id).next_run.unwrap();
        assert!(n > now && n <= now + 86400 + 3600 + 5, "{id}: {}", n - now);
    }
    let _ = f;
    runner.stop_all();
}

#[cfg(target_os = "linux")]
#[test]
fn r2_stop_during_run_does_not_block_and_thread_ends() {
    let _g = exclusive();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-sdr", &f.l, &f.r, manual()));
    // przytrzymaj blokadę zadania => przebieg utknie na starcie
    let (held_tx, held_rx) = channel();
    let (rel_tx, rel_rx) = channel::<()>();
    let r2 = runner.clone();
    let holder = thread::spawn(move || {
        r2.with_job_lock("q-sdr", || {
            held_tx.send(()).unwrap();
            let _ = rel_rx.recv();
        })
    });
    held_rx.recv().unwrap();
    assert!(runner.run_now("q-sdr", false));
    thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    runner.stop("q-sdr");
    runner.stop_all();
    assert!(
        t.elapsed() < Duration::from_millis(300),
        "stop nie czeka na wątek"
    );
    assert!(!runner.run_now("q-sdr", false));
    rel_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(
        wait_until(3000, || threads_named("job-q-sdr") == 0),
        "wątek powinien się zakończyć po przebiegu"
    );
    // zaczęty przebieg zostaje dokończony (nie ma połowicznej kopii)
    assert!(wait_until(3000, || f.r.join("a.txt").exists()));
}

#[test]
fn r2_restart_while_running_never_runs_twice_in_parallel() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let j = job("q-rst", &f.l, &f.r, manual());
    runner.start(j.clone());
    let (held_tx, held_rx) = channel();
    let (rel_tx, rel_rx) = channel::<()>();
    let r2 = runner.clone();
    let holder = thread::spawn(move || {
        r2.with_job_lock("q-rst", || {
            held_tx.send(()).unwrap();
            let _ = rel_rx.recv();
        })
    });
    held_rx.recv().unwrap();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    assert!(runner.run_now("q-rst", false));
    thread::sleep(Duration::from_millis(200));
    runner.start(j.clone()); // "edycja" w trakcie przebiegu
    assert!(runner.run_now("q-rst", false));
    rel_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(wait_until(8000, || f.r.join("a.txt").exists()));
    thread::sleep(Duration::from_millis(800));
    let s = st(&runner, "q-rst");
    assert!(s.state == "idle" || s.state == "running", "{:?}", s.state);
    assert!(s.error.is_none(), "{:?}", s.error);
    runner.stop_all();
}

#[test]
fn r2_disable_during_run_keeps_disabled_state() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let j = job(
        "q-dis",
        &f.l,
        &f.r,
        serde_json::json!({"type":"Interval","secs":10}),
    );
    let (held_tx, held_rx) = channel();
    let (rel_tx, rel_rx) = channel::<()>();
    let r2 = runner.clone();
    let holder = thread::spawn(move || {
        r2.with_job_lock("q-dis", || {
            held_tx.send(()).unwrap();
            let _ = rel_rx.recv();
        })
    });
    held_rx.recv().unwrap();
    runner.start(j.clone()); // skan startowy utknie na blokadzie
    thread::sleep(Duration::from_millis(300));
    let mut off = j.clone();
    off.enabled = false;
    runner.start(off);
    assert_eq!(st(&runner, "q-dis").state, "disabled");
    rel_tx.send(()).unwrap();
    holder.join().unwrap();
    thread::sleep(Duration::from_millis(1500));
    let s = st(&runner, "q-dis");
    runner.stop_all();
    assert_eq!(
        s.state, "disabled",
        "stary wątek nadpisał status: next_run={:?}",
        s.next_run
    );
}

#[test]
fn r2_poisoned_job_lock_does_not_break_later_use() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    let _ = catch_unwind(AssertUnwindSafe(|| {
        runner.with_job_lock("q-poi", || panic!("awaria w trakcie przebiegu"))
    }));
    let res = catch_unwind(AssertUnwindSafe(|| runner.with_job_lock("q-poi", || 1)));
    assert!(
        res.is_ok(),
        "kolejne użycie blokady po panice powinno działać"
    );
}

#[test]
fn r2_two_jobs_same_folders_both_converge() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-twoA", &f.l, &f.r, realtime()));
    runner.start(job("q-twoB", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-twoA"));
    assert!(wait_first_run(&runner, "q-twoB"));
    for i in 0..6 {
        fs::write(f.l.join(format!("w{i}.txt")), format!("tresc {i}")).unwrap();
    }
    assert!(wait_until(10000, || (0..6)
        .all(|i| f.r.join(format!("w{i}.txt")).exists())));
    // po wygaszeniu oba zadania w stanie idle, bez błędów (np. 'database is locked')
    assert!(
        wait_until(15000, || {
            ["q-twoA", "q-twoB"].iter().all(|id| {
                let s = st(&runner, id);
                s.state == "idle" && s.error.is_none()
            })
        }),
        "A={:?} B={:?}",
        st(&runner, "q-twoA").state,
        st(&runner, "q-twoB").state
    );
    for i in 0..6 {
        assert_eq!(
            fs::read_to_string(f.r.join(format!("w{i}.txt"))).unwrap(),
            format!("tresc {i}")
        );
    }
    runner.stop_all();
}

#[test]
fn r2_trigger_wake_runs_only_interested_jobs() {
    let _g = shared();
    let f = fx();
    let f2 = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    fs::write(f2.l.join("b.txt"), "b").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job_with(
        "q-wk",
        &f.l,
        &f.r,
        manual(),
        serde_json::json!({"triggers":{"on_wake":true}}),
    ));
    runner.start(job("q-nowk", &f2.l, &f2.r, manual()));
    runner.trigger("wake", None);
    runner.trigger("zly-rodzaj", None);
    assert!(
        wait_until(9000, || f.r.join("a.txt").exists()),
        "on_wake powinien uruchomić zadanie"
    );
    thread::sleep(Duration::from_millis(500));
    assert!(!f2.r.join("b.txt").exists());
    runner.stop_all();
}

#[test]
fn r2_trigger_drive_only_for_matching_volume() {
    let _g = shared();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job_with(
        "q-drv",
        &f.l,
        &f.r,
        manual(),
        serde_json::json!({"triggers":{"on_drive":true}}),
    ));
    runner.trigger("drive", Some("/zupelnie/inny/wolumin"));
    runner.trigger("drive", None);
    thread::sleep(Duration::from_millis(4500));
    assert!(
        !f.r.join("a.txt").exists(),
        "obcy wolumin nie może wyzwolić zadania"
    );
    runner.trigger("drive", Some(f._t.path().to_str().unwrap()));
    assert!(wait_until(9000, || f.r.join("a.txt").exists()));
    runner.stop_all();
}

#[test]
fn r2_trigger_drive_prefix_collision() {
    let _g = shared();
    let f = fx();
    let base = f._t.path().join("USB2");
    let (l, r) = (base.join("L"), base.join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job_with(
        "q-pfx",
        &l,
        &r,
        manual(),
        serde_json::json!({"triggers":{"on_drive":true}}),
    ));
    runner.trigger("drive", Some(f._t.path().join("USB").to_str().unwrap()));
    thread::sleep(Duration::from_millis(5000));
    let ran = r.join("a.txt").exists();
    runner.stop_all();
    assert!(!ran, "wolumin USB nie dotyczy zadania na USB2");
}

#[test]
fn r2_queued_trigger_does_not_run_after_stop() {
    let _g = shared();
    let f = fx();
    fs::write(f.l.join("a.txt"), "a").unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job_with(
        "q-tstop",
        &f.l,
        &f.r,
        manual(),
        serde_json::json!({"triggers":{"on_wake":true}}),
    ));
    runner.trigger("wake", None);
    thread::sleep(Duration::from_millis(100));
    runner.stop("q-tstop");
    thread::sleep(Duration::from_millis(5500));
    assert!(
        !f.r.join("a.txt").exists(),
        "zatrzymane zadanie wykonało przebieg"
    );
}

#[test]
fn r2_watcher_ignores_events_under_path_containing_syncron_substring() {
    let _g = shared();
    let f = fx();
    let base = f._t.path().join("moje.syncron-dane");
    let (l, r) = (base.join("L"), base.join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-sub", &l, &r, realtime()));
    assert!(wait_first_run(&runner, "q-sub"));
    fs::write(l.join("nowy.txt"), "x").unwrap();
    let ok = wait_until(6000, || r.join("nowy.txt").exists());
    runner.stop_all();
    assert!(
        ok,
        "zmiana w folderze zawierającym '.syncron' w ścieżce nie została wykryta (poll 600 s)"
    );
}

#[test]
fn r2_watcher_ignores_file_with_syncron_substring_in_name() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-sub2", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-sub2"));
    fs::write(f.l.join("raport.syncron-kopia.txt"), "x").unwrap();
    let ok = wait_until(6000, || f.r.join("raport.syncron-kopia.txt").exists());
    runner.stop_all();
    assert!(ok);
}

#[test]
fn r2_temp_files_do_not_trigger_but_final_file_does() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-tmp", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-tmp"));
    fs::write(f.l.join("pobierane.crdownload"), "x").unwrap();
    thread::sleep(Duration::from_millis(2500));
    assert!(!f.r.join("pobierane.crdownload").exists());
    fs::rename(f.l.join("pobierane.crdownload"), f.l.join("gotowe.txt")).unwrap();
    assert!(
        wait_until(8000, || f.r.join("gotowe.txt").exists()),
        "zmiana nazwy z tymczasowej na docelową musi być wykryta"
    );
    runner.stop_all();
}

// =====================================================================
// RUNDA 3: regresja / obciążenie
// =====================================================================

#[test]
fn r3_burst_of_changes_is_debounced_and_nothing_is_lost() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-burst", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-burst"));
    thread::sleep(Duration::from_millis(500));
    let sm = sample(&runner, "q-burst");
    for i in 0..60 {
        fs::write(f.l.join(format!("b{i:02}.txt")), format!("tresc {i}")).unwrap();
        thread::sleep(Duration::from_millis(20));
    }
    // seria modyfikacji jednego pliku
    for k in 0..20 {
        fs::write(
            f.l.join("b00.txt"),
            format!("zmiana numer {k} {}", "x".repeat(k)),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(20));
    }
    let last = format!("zmiana numer 19 {}", "x".repeat(19));
    assert!(
        wait_until(15000, || {
            (0..60).all(|i| f.r.join(format!("b{i:02}.txt")).exists())
                && fs::read_to_string(f.r.join("b00.txt"))
                    .map(|s| s == last)
                    .unwrap_or(false)
        }),
        "zmiana zginęła"
    );
    assert!(wait_until(10000, || st(&runner, "q-burst").state == "idle"));
    thread::sleep(Duration::from_millis(3500)); // ewentualne przebiegi następcze
    let (edges, states, bad_progress) = sm.finish();
    let n = productive_runs("q-burst");
    assert!(n <= 4, "zbyt wiele przebiegów z pracą: {n}");
    assert!(edges <= 8, "zbyt wiele przebiegów: {edges}");
    assert!(!bad_progress, "postęp bez stanu running");
    let ok: BTreeSet<&str> = ["idle", "running", "waiting"].into_iter().collect();
    for s in &states {
        assert!(ok.contains(s.as_str()), "nieoczekiwany stan {s}");
    }
    assert!(st(&runner, "q-burst").progress.is_none());
    // zawartość zgodna
    for i in 1..60 {
        assert_eq!(
            fs::read_to_string(f.r.join(format!("b{i:02}.txt"))).unwrap(),
            format!("tresc {i}")
        );
    }
    runner.stop_all();
}

#[test]
fn r3_continuous_writer_beyond_debounce_cap_still_syncs_and_final_state_converges() {
    let _g = shared();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-cont", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-cont"));
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, lp) = (stop.clone(), f.l.join("log.txt"));
    let writer = thread::spawn(move || {
        let mut n = 0;
        let mut content = String::new();
        while !s2.load(Ordering::SeqCst) {
            content.push_str(&format!("linia {n}\n"));
            fs::write(&lp, &content).unwrap();
            n += 1;
            thread::sleep(Duration::from_millis(100));
        }
        content
    });
    // cięcie debouncingu po 10 s: w trakcie ciągłego pisania musi pojawić się częściowa kopia
    let mid = wait_until(14000, || f.r.join("log.txt").exists());
    stop.store(true, Ordering::SeqCst);
    let content = writer.join().unwrap();
    assert!(
        mid,
        "przy ciągłym zapisie przebieg musi ruszyć najpóźniej po ~10 s"
    );
    assert!(
        wait_until(10000, || fs::read_to_string(f.r.join("log.txt"))
            .map(|s| s == content)
            .unwrap_or(false)),
        "ostatnia zmiana zginęła"
    );
    runner.stop_all();
}

#[cfg(target_os = "linux")]
#[test]
fn r3_start_stop_cycles_do_not_leak_threads() {
    let _g = exclusive();
    let base = all_threads();
    let fs_: Vec<Fx> = (0..4).map(|_| fx()).collect();
    for round in 0..40 {
        let runner = Runner::new(fs_[0].s.clone());
        for (i, f) in fs_.iter().enumerate() {
            runner.start(job(&format!("q-cy{i}"), &f.l, &f.r, realtime()));
        }
        if round % 4 == 0 {
            thread::sleep(Duration::from_millis(50));
        }
        runner.stop_all();
    }
    assert!(
        wait_until(8000, || all_threads() <= base + 1),
        "wątki: było {base}, jest {}",
        all_threads()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn r3_long_running_30s_stable_threads_and_status() {
    let _g = exclusive();
    let f = fx();
    let runner = Runner::new(f.s.clone());
    runner.start(job("q-long", &f.l, &f.r, realtime()));
    assert!(wait_first_run(&runner, "q-long"));
    thread::sleep(Duration::from_millis(500));
    let base = all_threads();
    let sm = sample(&runner, "q-long");
    let (mut max_t, mut min_t) = (base, base);
    let start = Instant::now();
    let mut i = 0usize;
    while start.elapsed() < Duration::from_secs(30) {
        fs::write(f.l.join(format!("d{}.txt", i % 25)), format!("rewizja {i}")).unwrap();
        if i.is_multiple_of(7) {
            runner.run_now("q-long", false);
        }
        i += 1;
        let t = all_threads();
        max_t = max_t.max(t);
        min_t = min_t.min(t);
        thread::sleep(Duration::from_millis(250));
    }
    assert!(wait_until(10000, || st(&runner, "q-long").state == "idle"));
    let (_, states, bad) = sm.finish();
    // przebiegi używają wątków roboczych (scope), więc dopuszczamy chwilowy skok, ale nie wzrost
    assert!(
        max_t <= base + 12,
        "liczba wątków rośnie: baza {base}, max {max_t}"
    );
    assert!(
        wait_until(5000, || all_threads() <= base + 1),
        "wątki po zakończeniu: {} (baza {base})",
        all_threads()
    );
    assert!(!bad);
    assert!(!states.contains("error"), "{states:?}");
    // zbieżność: ostatnie wersje po prawej
    for k in 0..25 {
        let last = (0..i).rev().find(|j| j % 25 == k).unwrap();
        let want = format!("rewizja {last}");
        assert!(
            wait_until(10000, || fs::read_to_string(f.r.join(format!("d{k}.txt")))
                .map(|s| s == want)
                .unwrap_or(false)),
            "d{k}.txt nie zbiegł do ostatniej wersji"
        );
    }
    runner.stop_all();
    let _ = min_t;
}

#[test]
fn r3_many_jobs_share_state_dir_without_errors() {
    let _g = shared();
    let runner_dir = tempfile::tempdir().unwrap();
    let fxs: Vec<Fx> = (0..8).map(|_| fx()).collect();
    let runner = Runner::new(runner_dir.path().join("state"));
    for (i, f) in fxs.iter().enumerate() {
        fs::write(f.l.join("a.txt"), format!("zadanie {i}")).unwrap();
        runner.start(job(&format!("q-m{i}"), &f.l, &f.r, realtime()));
    }
    for (i, f) in fxs.iter().enumerate() {
        assert!(
            wait_until(15000, || f.r.join("a.txt").exists()),
            "zadanie {i}"
        );
    }
    assert!(wait_until(15000, || (0..8).all(|i| {
        let s = st(&runner, &format!("q-m{i}"));
        s.state == "idle" && s.error.is_none()
    })));
    runner.stop_all();
}
