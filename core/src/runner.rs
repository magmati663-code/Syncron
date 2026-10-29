//! Uruchamianie zadań: watcher zdarzeń z debouncingiem, polling, interwały, harmonogram dzienny, okna czasowe, ponowienia.
use crate::conditions;
use crate::engine::{run_job_with, Report};
use crate::location::{Location, LEGACY_TRASH_DIR, TRASH_DIR};
use crate::model::{Job, Schedule};
use crate::names;
use chrono::{Local, Timelike};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

enum Msg {
    /// Zdarzenie systemowe: "drive" (z nazwą woluminu), "wake", "network".
    Trigger {
        kind: String,
        vol: Option<String>,
    },
    RunNow {
        force: bool,
    },
    FsEvent,
    /// Wznowienie po pauzie: budzi wątek, by nie czekał do końca 10-sekundowego odpytywania.
    Resume,
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
    /// Dodatkowa informacja (np. czemu zadanie czeka na warunki).
    pub note: Option<String>,
    /// idle | running | waiting (pliki zajęte, ponowienie) | error | blocked | paused | disabled
    pub state: String,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
    pub last: Option<Report>,
    pub error: Option<String>,
}

struct Handle {
    tx: Sender<Msg>,
    /// Numer generacji wątku: tylko wątek o aktualnym numerze może zapisywać status i wykonywać przebiegi.
    epoch: u64,
}

#[derive(Default)]
struct Inner {
    handles: Mutex<HashMap<String, Handle>>,
    status: Mutex<HashMap<String, Status>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    paused: AtomicBool,
    next_epoch: AtomicU64,
    /// Flaga zatrzymania aktualnego monitora systemowego (None: monitor nie działa).
    monitor: Mutex<Option<Arc<AtomicBool>>>,
}

/// Blokada odporna na zatrucie (panika w innym wątku nie może psuć kolejnych użyć).
fn lock_ok<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    /// Blokada zadania: nigdy dwa przebiegi tego samego zadania naraz (np. po edycji w trakcie synchronizacji).
    fn lock_for(&self, id: &str) -> Arc<Mutex<()>> {
        lock_ok(&self.locks)
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

/// Blokada magazynu zadań; zwalnia plik `.lock` przy zniszczeniu.
struct StoreLock {
    path: PathBuf,
    _g: std::sync::MutexGuard<'static, ()>,
}
impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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

    /// Wczytuje zadania. Brak pliku = pusta lista; błąd odczytu/parsowania = `Err` (po polsku, ze ścieżką).
    /// Zadania, których nie da się odczytać (np. nieznany `mode` z nowszej wersji), są pomijane,
    /// ale pozostają w pliku (`save_jobs` zachowuje ich surowy JSON). Uszkodzony plik jest kopiowany
    /// do `jobs.json.corrupt-<znacznik czasu>`.
    pub fn load_jobs_checked(&self) -> Result<Vec<Job>, String> {
        self.load_inner().map(|(jobs, _)| jobs)
    }

    /// Jak `load_jobs_checked`, ale błąd jest tylko logowany (zwraca pustą listę). Kolejny `save_jobs`
    /// i tak nie zniszczy uszkodzonego pliku (zostaje kopia `.corrupt-*`).
    pub fn load_jobs(&self) -> Vec<Job> {
        match self.load_jobs_checked() {
            Ok(j) => j,
            Err(e) => {
                log::error!("{e}");
                Vec::new()
            }
        }
    }

    /// (zadania, surowe wpisy, których nie udało się odczytać)
    fn load_inner(&self) -> Result<(Vec<Job>, Vec<serde_json::Value>), String> {
        let path = self.jobs_path();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], vec![])),
            Err(e) => {
                return Err(format!(
                    "Nie można odczytać pliku zadań {}: {e}",
                    path.display()
                ))
            }
        };
        let bad = |why: String| {
            let kept = self.backup_corrupt(&bytes);
            format!(
                "Plik zadań {} jest uszkodzony ({why}).{}",
                path.display(),
                match kept {
                    Some(k) => format!(" Kopię zachowano w {}.", k.display()),
                    None => String::new(),
                }
            )
        };
        let text =
            std::str::from_utf8(&bytes).map_err(|e| bad(format!("nieprawidłowy UTF-8: {e}")))?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let v: serde_json::Value =
            serde_json::from_str(text).map_err(|e| bad(format!("błąd składni JSON: {e}")))?;
        let serde_json::Value::Array(items) = v else {
            return Err(bad("oczekiwano listy zadań".into()));
        };
        let (mut jobs, mut unknown) = (Vec::new(), Vec::new());
        for it in items {
            match serde_json::from_value::<Job>(it.clone()) {
                Ok(j) => jobs.push(j),
                Err(e) => {
                    log::warn!("Pominięto nieczytelne zadanie w {}: {e}", path.display());
                    unknown.push(it);
                }
            }
        }
        Ok((jobs, unknown))
    }

    /// Zapisuje kopię uszkodzonej zawartości jobs.json (raz dla tej samej treści). Zwraca ścieżkę kopii.
    fn backup_corrupt(&self, content: &[u8]) -> Option<PathBuf> {
        if content.iter().all(|b| b.is_ascii_whitespace()) {
            return None;
        }
        let prefix = "jobs.json.corrupt-";
        if let Ok(rd) = std::fs::read_dir(&self.state_dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(prefix)
                    && std::fs::read(e.path())
                        .map(|c| c == content)
                        .unwrap_or(false)
                {
                    return Some(e.path());
                }
            }
        }
        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
        for n in 0..1000 {
            let name = if n == 0 {
                format!("{prefix}{ts}")
            } else {
                format!("{prefix}{ts}-{n}")
            };
            let p = self.state_dir.join(name);
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                use std::io::Write;
                return f.write_all(content).ok().map(|_| p);
            }
        }
        None
    }

    /// Blokada zapisu: mutex procesu + plik `jobs.json.lock` (między procesami: GUI, CLI, daemon).
    fn lock_store(&self) -> Result<StoreLock, String> {
        static PROC: Mutex<()> = Mutex::new(());
        let guard = PROC.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(&self.state_dir).map_err(|e| {
            format!(
                "Nie można utworzyć folderu danych {}: {e}",
                self.state_dir.display()
            )
        })?;
        let lock = self.state_dir.join("jobs.json.lock");
        let start = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock)
            {
                Ok(_) => {
                    return Ok(StoreLock {
                        path: lock,
                        _g: guard,
                    })
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // porzucona blokada (proces zakończył się awaryjnie)
                    let stale = std::fs::metadata(&lock)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|d| d > Duration::from_secs(30));
                    if stale {
                        let _ = std::fs::remove_file(&lock);
                    } else if start.elapsed() > Duration::from_secs(15) {
                        return Err(format!(
                            "Nie można uzyskać blokady {} (inny proces zapisuje zadania).",
                            lock.display()
                        ));
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                }
                Err(e) => {
                    return Err(format!(
                        "Nie można utworzyć blokady {}: {e}",
                        lock.display()
                    ))
                }
            }
        }
    }

    fn write_locked(&self, jobs: &[Job], unknown: &[serde_json::Value]) -> anyhow::Result<()> {
        use anyhow::Context;
        // nieczytelne zadania (np. z nowszej wersji) zostają w pliku, o ile nie zastąpiono ich id
        let mut all: Vec<serde_json::Value> = jobs
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        for u in unknown {
            let id = u.get("id").and_then(|v| v.as_str());
            if id.is_none() || !jobs.iter().any(|j| Some(j.id.as_str()) == id) {
                all.push(u.clone());
            }
        }
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let dest = self.jobs_path();
        let tmp = self.state_dir.join(format!(
            "jobs.json.tmp-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let r = std::fs::write(&tmp, serde_json::to_string_pretty(&all)?)
            .with_context(|| format!("Nie można zapisać pliku tymczasowego {}", tmp.display()))
            .and_then(|_| {
                std::fs::rename(&tmp, &dest)
                    .with_context(|| format!("Nie można zastąpić pliku zadań {}", dest.display()))
            });
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r
    }

    pub fn save_jobs(&self, jobs: &[Job]) -> anyhow::Result<()> {
        let _lock = self.lock_store().map_err(anyhow::Error::msg)?;
        // uszkodzony plik nie zostaje zniszczony bez śladu: ładowanie robi kopię .corrupt-*
        let unknown = self.load_inner().map(|(_, u)| u).unwrap_or_default();
        self.write_locked(jobs, &unknown)
    }

    /// Odczyt-zmiana-zapis pod blokadą (np. import). Przy uszkodzonym pliku zwraca błąd i niczego nie zapisuje.
    pub fn update_jobs<F>(&self, f: F) -> Result<Vec<Job>, String>
    where
        F: FnOnce(Vec<Job>) -> Result<Vec<Job>, String>,
    {
        let _lock = self.lock_store()?;
        let (jobs, unknown) = self.load_inner()?;
        let new = f(jobs)?;
        self.write_locked(&new, &unknown)
            .map_err(|e| format!("{e:#}"))?;
        Ok(new)
    }

    pub fn start_all(&self) {
        for j in self.load_jobs() {
            self.start(j);
        }
    }

    /// (Re)startuje wątek zadania. Cały start (stop starego, uruchomienie, rejestracja) odbywa się
    /// pod jedną blokadą `handles`, więc równoległe `start()` tego samego id nie gubią uchwytów.
    pub fn start(&self, job: Job) {
        let mut handles = lock_ok(&self.inner.handles);
        if let Some(old) = handles.remove(&job.id) {
            let _ = old.tx.send(Msg::Stop);
        }
        if !job.enabled {
            set_status(&self.inner, &job.id, |s| {
                s.state = "disabled".into();
                s.progress = None;
                s.next_run = None;
            });
            return;
        }
        let epoch = self.inner.next_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = channel();
        let (inner, dir, tx2, id) = (
            self.inner.clone(),
            self.state_dir.clone(),
            tx.clone(),
            job.id.clone(),
        );
        set_status(&self.inner, &id, |s| {
            s.state = "idle".into();
            s.progress = None;
        });
        let _ = thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || job_loop(job, rx, tx2, inner, dir, epoch))
            .ok();
        if let Some(prev) = handles.insert(id, Handle { tx, epoch }) {
            let _ = prev.tx.send(Msg::Stop);
        }
    }

    pub fn stop(&self, id: &str) {
        // wątek kończy się po bieżącym przebiegu; nie blokujemy interfejsu czekaniem na niego
        let mut handles = lock_ok(&self.inner.handles);
        if let Some(h) = handles.remove(id) {
            let _ = h.tx.send(Msg::Stop);
            // stary wątek nie może już zapisywać statusu: nie zostawiamy "running" na zawsze
            set_status(&self.inner, id, |s| {
                if s.state == "running" {
                    s.state = "idle".into();
                }
                s.progress = None;
            });
        }
    }

    /// Zatrzymuje zadanie i usuwa jego wpisy stanu (status, blokada) - np. po usunięciu zadania.
    pub fn forget(&self, id: &str) {
        self.stop(id);
        let handles = lock_ok(&self.inner.handles);
        if !handles.contains_key(id) {
            lock_ok(&self.inner.status).remove(id);
            let mut locks = lock_ok(&self.inner.locks);
            // blokadę trzymaną przez trwający przebieg zostawiamy (usunięcie pozwoliłoby na przebieg równoległy)
            if locks.get(id).is_some_and(|l| Arc::strong_count(l) == 1) {
                locks.remove(id);
            }
        }
    }

    pub fn stop_all(&self) {
        let ids: Vec<String> = lock_ok(&self.inner.handles).keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
    }

    /// Uruchomienie ręczne; `force` pomija zabezpieczenia (potwierdzone przez użytkownika).
    pub fn run_now(&self, id: &str, force: bool) -> bool {
        match lock_ok(&self.inner.handles).get(id) {
            Some(h) => h.tx.send(Msg::RunNow { force }).is_ok(),
            None => false,
        }
    }

    /// Jednorazowy przebieg „z wykluczeniami” (podgląd z ręcznymi zmianami): wykonuje plan zadania `id` bez akcji
    /// o kluczach z `skip` (jak w `Report.planned`). Działa pod blokadą zadania, więc nie nakłada się na przebieg
    /// harmonogramu; zabezpieczenia (masowe usuwanie) działają jak zwykle. Wynik trafia też do statusu zadania.
    pub fn run_selected(
        &self,
        job: &Job,
        skip: std::collections::HashSet<String>,
    ) -> anyhow::Result<Report> {
        let lock = self.inner.lock_for(&job.id);
        let _g = lock_ok(&lock);
        let res = crate::engine::run_job_opts(
            job,
            &self.state_dir,
            crate::engine::RunOpts {
                skip,
                ..Default::default()
            },
        );
        if let Ok(rep) = &res {
            set_status(&self.inner, &job.id, |s| {
                s.last_run = Some(Local::now().timestamp());
                s.error = None;
                s.state = if rep.blocked.is_some() {
                    "blocked"
                } else if rep.needs_retry() {
                    "waiting"
                } else {
                    "idle"
                }
                .into();
                s.last = Some(rep.clone());
            });
        }
        res
    }

    pub fn set_paused(&self, p: bool) {
        let was = self.inner.paused.swap(p, Ordering::SeqCst);
        if was && !p {
            for h in lock_ok(&self.inner.handles).values() {
                let _ = h.tx.send(Msg::Resume);
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> HashMap<String, Status> {
        lock_ok(&self.inner.status).clone()
    }

    /// Wykonuje `f` z blokadą zadania (np. ręczne rozstrzygnięcie konfliktu).
    pub fn with_job_lock<T>(&self, id: &str, f: impl FnOnce() -> T) -> T {
        let l = self.inner.lock_for(id);
        let _g = lock_ok(&l);
        f()
    }

    /// Rozsyła zdarzenie systemowe do wszystkich zadań (każde samo sprawdza, czy je interesuje).
    pub fn trigger(&self, kind: &str, vol: Option<&str>) {
        for h in lock_ok(&self.inner.handles).values() {
            let _ = h.tx.send(Msg::Trigger {
                kind: kind.into(),
                vol: vol.map(String::from),
            });
        }
    }

    /// Uruchamia wątek wykrywający: podłączenie dysku, wybudzenie z uśpienia i zmianę sieci.
    /// Wywołanie, gdy monitor już działa, nic nie robi.
    pub fn start_monitor(&self) {
        let mut slot = lock_ok(&self.inner.monitor);
        if slot.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        *slot = Some(stop.clone());
        let me = self.clone();
        let spawned = thread::Builder::new()
            .name("system-monitor".into())
            .spawn(move || {
                let mut vols = mounted_volumes();
                let mut ip = local_ip();
                let mut last = std::time::SystemTime::now();
                loop {
                    // 5 s w krótkich odcinkach, by zatrzymanie było szybkie
                    for _ in 0..20 {
                        if stop.load(Ordering::SeqCst) {
                            return;
                        }
                        thread::sleep(Duration::from_millis(250));
                    }
                    let now = std::time::SystemTime::now();
                    if now
                        .duration_since(last)
                        .map(|d| d > Duration::from_secs(45))
                        .unwrap_or(false)
                    {
                        log::info!("wykryto wybudzenie z uśpienia");
                        me.trigger("wake", None);
                    }
                    last = now;
                    let v = mounted_volumes();
                    for new in v.difference(&vols) {
                        log::info!("podłączono wolumin {new}");
                        me.trigger("drive", Some(new));
                    }
                    vols = v;
                    let i = local_ip();
                    if i != ip && i.is_some() {
                        log::info!("zmiana sieci: {ip:?} -> {i:?}");
                        me.trigger("network", None);
                    }
                    ip = i;
                }
            });
        if spawned.is_err() {
            *slot = None;
        }
    }

    /// Zatrzymuje monitor systemowy (jeśli działa); można go potem uruchomić ponownie.
    pub fn stop_monitor(&self) {
        if let Some(f) = lock_ok(&self.inner.monitor).take() {
            f.store(true, Ordering::SeqCst);
        }
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

fn set_status(inner: &Inner, id: &str, f: impl FnOnce(&mut Status)) {
    let mut m = lock_ok(&inner.status);
    f(m.entry(id.to_string()).or_default());
}

/// Czy wątek o numerze `epoch` jest nadal aktualnym wątkiem zadania.
fn is_current(inner: &Inner, id: &str, epoch: u64) -> bool {
    lock_ok(&inner.handles)
        .get(id)
        .is_some_and(|h| h.epoch == epoch)
}

/// Zapis statusu tylko przez aktualny wątek (przestarzały wątek nie nadpisuje stanu nowego/wyłączonego).
/// Kolejność blokad (handles, potem status) jest taka sama jak w `start`/`stop`.
fn set_status_ep(inner: &Inner, id: &str, epoch: u64, f: impl FnOnce(&mut Status)) {
    let handles = lock_ok(&inner.handles);
    if handles.get(id).is_some_and(|h| h.epoch == epoch) {
        set_status(inner, id, f);
    }
}

/// Okno godzin [a, b): przy a > b obejmuje północ. Okno [a, a] (puste) traktujemy jako „zawsze”
/// (a nie „nigdy”), by błędna konfiguracja nie wyłączała zadania po cichu.
fn in_window(w: Option<[u32; 2]>) -> bool {
    match w {
        None => true,
        Some([a, b]) if a == b => true,
        Some([a, b]) => {
            let h = Local::now().hour();
            if a < b {
                h >= a && h < b
            } else {
                h >= a || h < b
            }
        }
    }
}

/// Pierwszy poprawny czas lokalny nie wcześniejszy niż `t` (w luce DST przesuwa za lukę).
fn resolve_local(mut t: chrono::NaiveDateTime) -> Option<chrono::DateTime<Local>> {
    for _ in 0..=180 {
        if let Some(x) = t.and_local_timezone(Local).earliest() {
            return Some(x);
        }
        t += chrono::Duration::minutes(1);
    }
    None
}

/// Liczba sekund do najbliższej (przyszłej) godziny:minuty czasu lokalnego, liczona tak, że
/// `ts_teraz + wynik` wypada dokładnie na pełną minutę (nigdy przed terminem).
fn secs_until_daily(hour: u32, minute: u32) -> u64 {
    if hour >= 24 || minute >= 60 {
        log::warn!(
            "godzina {hour}:{minute} poza zakresem - użyto {}:{}",
            hour % 24,
            minute % 60
        );
    }
    let now = Local::now();
    let t = now
        .date_naive()
        .and_hms_opt(hour % 24, minute % 60, 0)
        .unwrap();
    let mut target = resolve_local(t);
    if target.is_none_or(|x| x <= now) {
        target = resolve_local(t + chrono::Duration::days(1));
    }
    target.map_or(3600, |x| (x.timestamp() - now.timestamp()).max(1) as u64)
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

/// Rozbija ścieżkę na znormalizowane komponenty (bez rozróżniania wielkości liter i rodzaju separatora).
fn norm_components(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(['/', '\\'])
        .filter(|c| !c.is_empty())
        .map(String::from)
        .collect()
}

/// Czy któraś z lokalizacji lokalnych zadania leży na podanym woluminie (litera dysku / punkt montowania).
/// Porównanie komponentami: „/x/USB” nie obejmuje „/x/USB2”.
fn touches_volume(job: &Job, vol: &str) -> bool {
    let v = norm_components(vol);
    if v.is_empty() {
        return false;
    }
    [&job.left, &job.right].iter().any(|s| {
        matches!(Location::parse(s), Location::Local(_)) && norm_components(s).starts_with(&v)
    })
}

/// Czy zdarzenie dla ścieżki `p` powinno wyzwolić synchronizację. Kosz i pliki tymczasowe ocenia się
/// po komponentach ścieżki względem korzenia zadania (nie po podciągach ścieżki bezwzględnej).
fn relevant(p: &Path, roots: &[PathBuf]) -> bool {
    let rel = roots
        .iter()
        .find_map(|r| p.strip_prefix(r).ok())
        .unwrap_or(p);
    let in_trash = rel.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        c == TRASH_DIR || c == LEGACY_TRASH_DIR
    });
    if in_trash {
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
    let roots: Vec<PathBuf> = [&job.left, &job.right]
        .into_iter()
        .filter_map(|s| match Location::parse(s) {
            Location::Local(p) => Some(p),
            _ => None,
        })
        .collect();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !matches!(ev.kind, EventKind::Access(_))
                && ev.paths.iter().any(|p| relevant(p, &roots))
            {
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

/// Podłączone woluminy (litery dysków / punkty montowania).
pub fn mounted_volumes() -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let mask = unsafe { GetLogicalDrives() };
        for i in 0..26u32 {
            if mask & (1 << i) != 0 {
                out.insert(format!("{}:\\", (b'A' + i as u8) as char));
            }
        }
    }
    #[cfg(not(windows))]
    for dir in ["/Volumes", "/mnt"] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            out.extend(
                rd.flatten()
                    .map(|e| e.path().to_string_lossy().into_owned()),
            );
        }
    }
    out
}

/// Lokalny adres IP używany do wyjścia w sieć (nic nie jest wysyłane).
fn local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

fn ts_in(d: Duration) -> i64 {
    Local::now().timestamp() + d.as_secs() as i64
}

fn job_loop(
    job: Job,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    inner: Arc<Inner>,
    dir: PathBuf,
    epoch: u64,
) {
    let current = || is_current(&inner, &job.id, epoch);
    let set = |f: &mut dyn FnMut(&mut Status)| set_status_ep(&inner, &job.id, epoch, f);
    let mut watcher = make_watcher(&job, tx.clone());
    // po awarii/odłączeniu folderu uchwyt watchera przestaje działać: odtwarzamy go po powrocie do zdrowia
    let mut stale = false;
    let refresh = |watcher: &mut Option<notify::RecommendedWatcher>, stale: &mut bool| {
        let state = lock_ok(&inner.status)
            .get(&job.id)
            .map(|s| s.state.clone())
            .unwrap_or_default();
        if state == "error" || state == "waiting" {
            *stale = true;
        } else if *stale {
            *watcher = make_watcher(&job, tx.clone());
            *stale = false;
        }
    };
    let manual_only = job.schedule == Schedule::Manual;
    let mut backoff = 0u32;
    // skan startowy (nadrobienie zmian z czasu, gdy aplikacja była wyłączona)
    let mut next: Option<Instant> = (!manual_only).then(Instant::now);

    // wykonuje przebieg i wyznacza następny termin
    let mut run = |force: bool, manual: bool, next: &mut Option<Instant>| {
        if !current() {
            return;
        }
        if !manual {
            if inner.paused.load(Ordering::SeqCst) {
                set(&mut |s| {
                    s.state = "paused".into();
                    s.next_run = Some(ts_in(Duration::from_secs(10)));
                });
                *next = Some(Instant::now() + Duration::from_secs(10));
                return;
            }
            if !in_window(job.window) {
                // czekamy do początku okna (nie dłużej niż godzinę naraz)
                let until = job
                    .window
                    .map(|[a, _]| secs_until_daily(a, 0))
                    .unwrap_or(300)
                    .min(3600);
                let a = job.window.map(|w| w[0]).unwrap_or(0);
                set(&mut |s| {
                    s.note = Some(format!("poza oknem godzin do {a:02}:00"));
                    s.next_run = Some(ts_in(Duration::from_secs(until)));
                });
                *next = Some(Instant::now() + Duration::from_secs(until));
                return;
            }
            if let Err(reason) = conditions::check(&job.conditions) {
                log::info!("zadanie „{}”: czeka na warunki: {reason}", job.name);
                set(&mut |s| {
                    s.state = "conditions".into();
                    s.note = Some(reason.clone());
                    s.next_run = Some(ts_in(Duration::from_secs(60)));
                });
                *next = Some(Instant::now() + Duration::from_secs(60));
                return;
            }
        }
        let lock = inner.lock_for(&job.id);
        let _guard = lock_ok(&lock);
        // zadanie mogło zostać zatrzymane/zrestartowane w czasie oczekiwania na blokadę; przebieg
        // automatyczny wtedy pomijamy, a ręczny (już zaakceptowany przez użytkownika) dokańczamy
        if !manual && !current() {
            return;
        }
        set(&mut |s| {
            s.state = "running".into();
            s.progress = None;
            s.note = None;
        });
        let report = |done: usize, total: usize, cur: &str| {
            set_status_ep(&inner, &job.id, epoch, |s| {
                s.progress = Some(Progress {
                    done,
                    total,
                    current: cur.to_string(),
                })
            });
        };
        let res = catch_unwind(AssertUnwindSafe(|| {
            run_job_with(&job, &dir, false, force, Some(&report))
        }))
        .unwrap_or_else(|p| {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(anyhow::anyhow!(
                "awaria wewnętrzna podczas przebiegu: {msg}"
            ))
        });
        match &res {
            Ok(r) => {
                let busy = r.busy.len();
                if r.copied + r.deleted + r.moved + r.conflicts.len() + busy + r.errors.len() > 0
                    || r.blocked.is_some()
                {
                    log::info!(
                        "zadanie „{}”: skopiowano {}, usunięto {}, przeniesiono {}, konflikty {}, zajęte {}, błędy {}{}",
                        job.name, r.copied, r.deleted, r.moved, r.conflicts.len(), busy, r.errors.len(),
                        r.blocked.as_ref().map(|b| format!(", ZATRZYMANO: {b}")).unwrap_or_default()
                    );
                    for e in r.errors.iter().take(20) {
                        log::warn!("zadanie „{}”: {e}", job.name);
                    }
                    if let Some(n) = &r.vss_note {
                        log::warn!("zadanie „{}”: VSS: {n}", job.name);
                    }
                }
            }
            Err(e) => log::error!("zadanie „{}”: {e}", job.name),
        }
        let backoff_d = Duration::from_secs((30u64 << backoff.min(5)).min(600));
        // ponowienia dotyczą tylko harmonogramów automatycznych: zadanie Manual nigdy nie startuje samo
        let mut retry = None;
        match &res {
            Ok(rep) => {
                if rep.needs_retry() && !manual_only {
                    retry = Some(backoff_d);
                }
            }
            Err(_) => {
                if !manual_only {
                    retry = Some(backoff_d);
                }
            }
        }
        backoff = if retry.is_some() { backoff + 1 } else { 0 };
        let regular = period(&job.schedule);
        // przy ponowieniu liczy się backoff (nie krótszy okres harmonogramu); przy braku ponowienia - okres
        let due = retry.or(regular);
        *next = due.map(|d| Instant::now() + d);
        set(&mut |s| {
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
                    s.last = Some(rep.clone());
                }
                Err(e) => {
                    s.state = "error".into();
                    s.error = Some(e.to_string());
                }
            }
            s.next_run = due.map(ts_in);
        });
    };

    // komunikat odebrany w trakcie debouncingu/oczekiwania na wyzwalacz - wykonywany z własnymi parametrami
    let mut pending: Option<Msg> = None;
    loop {
        if !current() {
            return;
        }
        let wait = next
            .map(|t| t.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600));
        let msg = match pending.take() {
            Some(m) => Ok(m),
            None => rx.recv_timeout(wait),
        };
        match msg {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Msg::RunNow { force }) => {
                run(force, true, &mut next);
                refresh(&mut watcher, &mut stale);
            }
            Ok(Msg::Resume) => {
                let paused = lock_ok(&inner.status)
                    .get(&job.id)
                    .is_some_and(|s| s.state == "paused");
                if paused && !manual_only {
                    next = Some(Instant::now());
                }
            }
            Ok(Msg::Trigger { kind, vol }) => {
                let want = match kind.as_str() {
                    "wake" => job.triggers.on_wake,
                    "network" => job.triggers.on_network,
                    "drive" => {
                        job.triggers.on_drive && vol.is_some_and(|v| touches_volume(&job, &v))
                    }
                    _ => false,
                };
                if want {
                    log::info!("zadanie „{}”: wyzwalacz „{kind}”", job.name);
                    // dysk/sieć potrzebują chwili na ustabilizowanie; czekanie przerywa Stop
                    let until = Instant::now() + Duration::from_secs(3);
                    let mut skip = false;
                    loop {
                        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                            Err(RecvTimeoutError::Timeout) => break,
                            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                            Ok(m @ Msg::RunNow { .. }) => {
                                pending = Some(m); // przebieg ręczny i tak obejmie zmiany
                                skip = true;
                                break;
                            }
                            Ok(_) => {}
                        }
                    }
                    if !current() {
                        return;
                    }
                    watcher = make_watcher(&job, tx.clone()); // dysk/sieć wróciły: nowy uchwyt
                    if !skip {
                        run(false, false, &mut next);
                    }
                    refresh(&mut watcher, &mut stale);
                }
            }
            Ok(Msg::FsEvent) => {
                // debouncing: czekaj, aż zdarzenia ucichną (maks. 10 s)
                let start = Instant::now();
                let mut skip = false;
                loop {
                    match rx.recv_timeout(Duration::from_millis(1500)) {
                        Ok(Msg::FsEvent) if start.elapsed() < Duration::from_secs(10) => continue,
                        Ok(Msg::FsEvent) | Err(RecvTimeoutError::Timeout) => break,
                        Ok(Msg::Resume) => continue,
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(m) => {
                            // RunNow/Trigger zachowują swoje parametry; ręczny przebieg obejmuje też zmiany z FS
                            skip = matches!(m, Msg::RunNow { .. });
                            pending = Some(m);
                            break;
                        }
                    }
                }
                if !skip {
                    run(false, false, &mut next);
                }
                refresh(&mut watcher, &mut stale);
            }
            Err(RecvTimeoutError::Timeout) => {
                if next.is_some() {
                    run(false, false, &mut next);
                    refresh(&mut watcher, &mut stale);
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
    fn volume_matching() {
        let j: Job = serde_json::from_value(serde_json::json!({
            "id":"v","name":"v","left":"E:\\Dane","right":"gdrive:x","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}
        }))
        .unwrap();
        assert!(touches_volume(&j, "E:\\"));
        assert!(touches_volume(&j, "e:/"));
        assert!(!touches_volume(&j, "F:\\"));
    }

    #[test]
    fn volume_matching_is_by_component() {
        let j: Job = serde_json::from_value(serde_json::json!({
            "id":"v","name":"v","left":"/x/USB2/d","right":"gdrive:x","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}
        }))
        .unwrap();
        assert!(!touches_volume(&j, "/x/USB"));
        assert!(touches_volume(&j, "/x/USB2"));
        assert!(!touches_volume(&j, ""));
    }

    #[test]
    fn relevant_checks_components_relative_to_root() {
        let root = PathBuf::from("/data/moje.syncron-dane/L");
        let roots = [root.clone()];
        assert!(relevant(&root.join("raport.syncron-kopia.txt"), &roots));
        assert!(relevant(&root.join(".syncron-dane/x.txt"), &roots));
        assert!(!relevant(&root.join(".syncron-trash/1/x.txt"), &roots));
        assert!(!relevant(&root.join(".syncpolak-trash/x"), &roots));
        assert!(!relevant(&root.join("a.syncron.tmp"), &roots));
    }

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
