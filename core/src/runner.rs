//! Uruchamianie zadań: watcher zdarzeń z debouncingiem, polling, interwały, harmonogram dzienny, okna czasowe, ponowienia.
use crate::engine::{run_job_with, Report};
use crate::location::{Location, TRASH_DIR};
use crate::model::{Job, Schedule};
use crate::names;
use chrono::{Local, Timelike};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

enum Msg {
    RunNow { force: bool },
    FsEvent,
    Stop,
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub progress: Option<Progress>,
    /// idle | running | waiting (pliki zajęte, ponowienie) | error | blocked | paused | disabled
    pub state: String,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last: Option<Report>,
    pub error: Option<String>,
}

struct Handle {
    tx: Sender<Msg>,
}

#[derive(Default)]
struct Inner {
    handles: Mutex<HashMap<String, Handle>>,
    status: Mutex<HashMap<String, Status>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    paused: AtomicBool,
}

impl Inner {
    /// Blokada zadania: nigdy dwa przebiegi tego samego zadania naraz (np. po edycji w trakcie synchronizacji).
    fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

#[derive(Clone)]
pub struct Runner {
    state_dir: PathBuf,
    inner: Arc<Inner>,
}

impl Runner {
    pub fn new(state_dir: PathBuf) -> Self {
        Runner {
            state_dir,
            inner: Arc::new(Inner::default()),
        }
    }

    pub fn jobs_path(&self) -> PathBuf {
        self.state_dir.join("jobs.json")
    }

    pub fn load_jobs(&self) -> Vec<Job> {
        std::fs::read_to_string(self.jobs_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_jobs(&self, jobs: &[Job]) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        let tmp = self.state_dir.join("jobs.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(jobs)?)?;
        std::fs::rename(tmp, self.jobs_path())?;
        Ok(())
    }

    pub fn start_all(&self) {
        for j in self.load_jobs() {
            self.start(j);
        }
    }

    /// (Re)startuje wątek zadania.
    pub fn start(&self, job: Job) {
        self.stop(&job.id);
        if !job.enabled {
            self.set(&job.id, |s| s.state = "disabled".into());
            return;
        }
        let (tx, rx) = channel();
        let (inner, dir, tx2, id) = (
            self.inner.clone(),
            self.state_dir.clone(),
            tx.clone(),
            job.id.clone(),
        );
        self.set(&id, |s| s.state = "idle".into());
        let _ = thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || job_loop(job, rx, tx2, inner, dir))
            .ok();
        self.inner.handles.lock().unwrap().insert(id, Handle { tx });
    }

    pub fn stop(&self, id: &str) {
        // wątek kończy się po bieżącym przebiegu; nie blokujemy interfejsu czekaniem na niego
        if let Some(h) = self.inner.handles.lock().unwrap().remove(id) {
            let _ = h.tx.send(Msg::Stop);
        }
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = self.inner.handles.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    /// Uruchomienie ręczne; `force` pomija zabezpieczenia (potwierdzone przez użytkownika).
    pub fn run_now(&self, id: &str, force: bool) -> bool {
        match self.inner.handles.lock().unwrap().get(id) {
            Some(h) => h.tx.send(Msg::RunNow { force }).is_ok(),
            None => false,
        }
    }

    pub fn set_paused(&self, p: bool) {
        self.inner.paused.store(p, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> HashMap<String, Status> {
        self.inner.status.lock().unwrap().clone()
    }

    /// Wykonuje `f` z blokadą zadania (np. ręczne rozstrzygnięcie konfliktu).
    pub fn with_job_lock<T>(&self, id: &str, f: impl FnOnce() -> T) -> T {
        let l = self.inner.lock_for(id);
        let _g = l.lock().unwrap();
        f()
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    fn set(&self, id: &str, f: impl FnOnce(&mut Status)) {
        set_status(&self.inner, id, f);
    }
}

fn set_status(inner: &Inner, id: &str, f: impl FnOnce(&mut Status)) {
    let mut m = inner.status.lock().unwrap();
    f(m.entry(id.to_string()).or_default());
}

fn in_window(w: Option<[u32; 2]>) -> bool {
    match w {
        None => true,
        Some([a, b]) => {
            let h = Local::now().hour();
            if a <= b {
                h >= a && h < b
            } else {
                h >= a || h < b
            }
        }
    }
}

fn secs_until_daily(hour: u32, minute: u32) -> u64 {
    let now = Local::now();
    let mut t = now
        .date_naive()
        .and_hms_opt(hour % 24, minute % 60, 0)
        .unwrap();
    if t.and_local_timezone(Local)
        .earliest()
        .is_none_or(|x| x <= now)
    {
        t += chrono::Duration::days(1);
    }
    t.and_local_timezone(Local)
        .earliest()
        .map_or(3600, |x| (x - now).num_seconds().max(1) as u64)
}

fn period(s: &Schedule) -> Option<Duration> {
    match s {
        Schedule::Manual => None,
        Schedule::Realtime { poll_secs } => Some(Duration::from_secs((*poll_secs).max(15))),
        Schedule::Interval { secs } => Some(Duration::from_secs((*secs).max(10))),
        Schedule::Daily { hour, minute } => {
            Some(Duration::from_secs(secs_until_daily(*hour, *minute)))
        }
    }
}

fn relevant(p: &Path) -> bool {
    let s = p.to_string_lossy();
    if s.contains(TRASH_DIR) || s.contains(".syncpolak") {
        return false;
    }
    !p.file_name()
        .map(|n| names::is_temp(&n.to_string_lossy()))
        .unwrap_or(false)
}

fn make_watcher(job: &Job, tx: Sender<Msg>) -> Option<notify::RecommendedWatcher> {
    if !matches!(job.schedule, Schedule::Realtime { .. }) {
        return None;
    }
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !matches!(ev.kind, EventKind::Access(_)) && ev.paths.iter().any(|p| relevant(p)) {
                let _ = tx.send(Msg::FsEvent);
            }
        }
    })
    .ok()?;
    let mut any = false;
    for side in [&job.left, &job.right] {
        if let Location::Local(p) = Location::parse(side) {
            any |= w.watch(&p, RecursiveMode::Recursive).is_ok();
        }
    }
    any.then_some(w)
}

fn ts_in(d: Duration) -> i64 {
    Local::now().timestamp() + d.as_secs() as i64
}

fn job_loop(job: Job, rx: Receiver<Msg>, tx: Sender<Msg>, inner: Arc<Inner>, dir: PathBuf) {
    let _watcher = make_watcher(&job, tx);
    let mut backoff = 0u32;
    // skan startowy (nadrobienie zmian z czasu, gdy aplikacja była wyłączona)
    let mut next: Option<Instant> = (job.schedule != Schedule::Manual).then(Instant::now);

    // wykonuje przebieg i wyznacza następny termin
    let mut run = |force: bool, manual: bool, next: &mut Option<Instant>| {
        if !manual {
            if inner.paused.load(Ordering::SeqCst) {
                set_status(&inner, &job.id, |s| s.state = "paused".into());
                *next = Some(Instant::now() + Duration::from_secs(10));
                return;
            }
            if !in_window(job.window) {
                *next = Some(Instant::now() + Duration::from_secs(300));
                return;
            }
        }
        let lock = inner.lock_for(&job.id);
        let _guard = lock.lock().unwrap();
        set_status(&inner, &job.id, |s| {
            s.state = "running".into();
            s.progress = None;
        });
        let report = |done: usize, total: usize, cur: &str| {
            set_status(&inner, &job.id, |s| {
                s.progress = Some(Progress {
                    done,
                    total,
                    current: cur.to_string(),
                })
            });
        };
        let res = run_job_with(&job, &dir, false, force, Some(&report));
        let mut retry = None;
        set_status(&inner, &job.id, |s| {
            s.last_run = Some(Local::now().timestamp());
            s.progress = None;
            match &res {
                Ok(rep) => {
                    s.error = None;
                    s.state = if rep.blocked.is_some() {
                        "blocked"
                    } else if rep.needs_retry() {
                        "waiting"
                    } else {
                        "idle"
                    }
                    .into();
                    if rep.needs_retry() {
                        retry = Some(Duration::from_secs((30u64 << backoff.min(5)).min(600)));
                    }
                    s.last = Some(rep.clone());
                }
                Err(e) => {
                    s.state = "error".into();
                    s.error = Some(e.to_string());
                    retry = Some(Duration::from_secs((30u64 << backoff.min(5)).min(600)));
                }
            }
        });
        backoff = if retry.is_some() { backoff + 1 } else { 0 };
        let regular = period(&job.schedule);
        let due = match (regular, retry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        *next = due.map(|d| Instant::now() + d);
        set_status(&inner, &job.id, |s| s.next_run = due.map(ts_in));
    };

    loop {
        let wait = next
            .map(|t| t.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        match rx.recv_timeout(wait) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Msg::RunNow { force }) => run(force, true, &mut next),
            Ok(Msg::FsEvent) => {
                // debouncing: czekaj, aż zdarzenia ucichną (maks. 10 s)
                let start = Instant::now();
                loop {
                    match rx.recv_timeout(Duration::from_millis(1500)) {
                        Ok(Msg::FsEvent) if start.elapsed() < Duration::from_secs(10) => continue,
                        Ok(Msg::FsEvent) | Err(RecvTimeoutError::Timeout) => break,
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(Msg::RunNow { .. }) => break,
                    }
                }
                run(false, false, &mut next);
            }
            Err(RecvTimeoutError::Timeout) => {
                if next.is_some() {
                    run(false, false, &mut next)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn daily_is_in_future() {
        let s = secs_until_daily(3, 0);
        assert!((1..=86400 + 3600).contains(&s));
    }

    #[test]
    fn window_wraps_midnight() {
        assert!(in_window(None));
        let h = Local::now().hour();
        assert!(in_window(Some([h, (h + 1) % 24])));
        assert!(!in_window(Some([(h + 1) % 24, (h + 2) % 24])));
    }

    #[test]
    fn realtime_picks_up_new_file() {
        let t = tempfile::tempdir().unwrap();
        let (l, r, s) = (
            t.path().join("L"),
            t.path().join("R"),
            t.path().join("state"),
        );
        fs::create_dir_all(&l).unwrap();
        fs::create_dir_all(&r).unwrap();
        let job: Job = serde_json::from_value(serde_json::json!({
            "id":"rt","name":"rt","left":l.to_str().unwrap(),"right":r.to_str().unwrap(),
            "mode":"TwoWay","conflict":"Newer","schedule":{"type":"Realtime","poll_secs":600}
        }))
        .unwrap();
        let runner = Runner::new(s);
        runner.start(job);
        thread::sleep(Duration::from_millis(1500)); // skan startowy
        fs::write(l.join("nowy żółw.txt"), "hej").unwrap();
        let mut ok = false;
        for _ in 0..40 {
            thread::sleep(Duration::from_millis(250));
            if r.join("nowy żółw.txt").exists() {
                ok = true;
                break;
            }
        }
        runner.stop_all();
        assert!(ok, "watcher powinien złapać nowy plik");
    }
}
