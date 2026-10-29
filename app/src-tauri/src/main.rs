#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::collections::HashMap;
use syncron_core::engine::{self, HistoryRow, Report, TrashItem};
use syncron_core::location::{
    rclone_config_path, rclone_obscure, remote_exists, run_rclone, set_remote_keys,
    valid_param_key, REMOTE_KINDS,
};
use syncron_core::model::Job;
use syncron_core::runner::{Runner, Status};
use syncron_core::vss;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

type Res<T> = Result<T, String>;

/// Prosty dziennik plikowy z rotacją (2 MB, 3 kopie): `logs/syncron.log` w folderze danych aplikacji.
struct FileLogger {
    path: std::path::PathBuf,
    lock: std::sync::Mutex<()>,
}

impl FileLogger {
    fn rotate(&self) {
        if std::fs::metadata(&self.path)
            .map(|m| m.len() > 2 * 1024 * 1024)
            .unwrap_or(false)
        {
            for i in (1..3).rev() {
                let _ = std::fs::rename(
                    self.path.with_extension(format!("log.{i}")),
                    self.path.with_extension(format!("log.{}", i + 1)),
                );
            }
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        use std::io::Write;
        let _g = self.lock.lock().unwrap();
        self.rotate();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(
                f,
                "[{}] {:<5} {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                r.level(),
                // nowe linie w komunikacie (np. nazwa pliku) nie mogą fałszować kolejnych wpisów
                r.args()
                    .to_string()
                    .replace('\r', "\\r")
                    .replace('\n', "\\n")
            );
        }
    }
    fn flush(&self) {}
}

fn init_logging(dir: &std::path::Path) {
    let logs = dir.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let logger = FileLogger {
        path: logs.join("syncron.log"),
        lock: std::sync::Mutex::new(()),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|info| log::error!("PANIC: {info}")));
    log::info!(
        "Syncron {} uruchomiony ({} {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

/// Ikona zasobnika z kolorową kropką stanu (zielona: ok, niebieska: trwa, żółta: uwaga, czerwona: błąd, szara: wstrzymane).
fn badge_icon(base: &tauri::image::Image<'_>, color: [u8; 3]) -> tauri::image::Image<'static> {
    let (w, h) = (base.width() as i32, base.height() as i32);
    let mut px = base.rgba().to_vec();
    let (cx, cy, r) = (w as f32 * 0.74, h as f32 * 0.74, w as f32 * 0.24);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let i = ((y * w + x) * 4) as usize;
            if d <= r * 1.28 && d > r {
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]); // biała obwódka
            } else if d <= r {
                px[i..i + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        }
    }
    tauri::image::Image::new_owned(px, w as u32, h as u32)
}

/// Ogólny stan do ikony i podpowiedzi: (kolor, klucz).
fn overall_state(r: &Runner) -> (&'static str, [u8; 3]) {
    if r.is_paused() {
        return ("paused", [120, 130, 150]);
    }
    let st = r.status();
    let any = |f: &dyn Fn(&Status) -> bool| st.values().any(f);
    if any(&|s| s.state == "error" || s.state == "blocked") {
        ("error", [220, 38, 38])
    } else if any(&|s| {
        s.state == "waiting"
            || s.state == "conditions"
            || s.last
                .as_ref()
                .map(|l| !l.conflicts.is_empty())
                .unwrap_or(false)
    }) {
        ("attention", [217, 119, 6])
    } else if any(&|s| s.state == "running") {
        ("running", [37, 99, 235])
    } else {
        ("ok", [22, 163, 74])
    }
}

fn kind_of(loc: &str) -> String {
    match syncron_core::location::Location::parse(loc) {
        syncron_core::location::Location::Rclone(s) => {
            format!("chmura ({})", s.split(':').next().unwrap_or(""))
        }
        syncron_core::location::Location::Local(_)
            if loc.starts_with("\\\\") || loc.starts_with("//") =>
        {
            "dysk sieciowy".into()
        }
        _ => "folder lokalny".into(),
    }
}

/// Zastępuje ścieżki (`C:\\...`, `\\\\serwer\\...`, `/home/...`) tekstem `<ścieżka>`; ścieżka kończy się na cudzysłowie,
/// nawiasie zamykającym, sekwencji `: ` albo końcu wiersza.
fn redact_paths(text: &str) -> String {
    let ch: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < ch.len() {
        let prev_ok =
            i == 0 || !(ch[i - 1].is_alphanumeric() || ch[i - 1] == ':' || ch[i - 1] == '/');
        let at = |k: usize| ch.get(i + k).copied().unwrap_or('\0');
        let win_drive = ch[i].is_ascii_alphabetic()
            && at(1) == ':'
            && (at(2) == '\\' || at(2) == '/')
            && (i == 0 || !ch[i - 1].is_alphanumeric());
        let unc = ch[i] == '\\' && at(1) == '\\';
        let unix = ch[i] == '/' && prev_ok && (at(1).is_alphanumeric() || "._~-".contains(at(1)));
        if win_drive || unc || unix {
            let mut j = i;
            while j < ch.len() {
                let c = ch[j];
                if "”\"'»)]\n\r".contains(c) || (c == ':' && ch.get(j + 1) == Some(&' ')) {
                    break;
                }
                j += 1;
            }
            out.push_str("<ścieżka>");
            i = j;
        } else {
            out.push(ch[i]);
            i += 1;
        }
    }
    out
}

/// Tekst diagnostyczny do wklejenia w zgłoszeniu: wersja, system, zadania i końcówka dziennika.
/// Ścieżki w błędach i dzienniku są zastąpione `<ścieżka>`; nazwy zadań i kont rclone pozostają.
#[tauri::command]
async fn get_diagnostics(r: State<'_, Runner>) -> Res<String> {
    let runner = r.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut o = String::new();
        o += &format!("Syncron {}\nSystem: {} {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
        o += &format!("rclone: {}\n", run_rclone(&["version"]).map(|s| s.lines().next().unwrap_or("").to_string()).unwrap_or_else(|e| format!("niedostępny ({})", redact_paths(&e))));
        o += &format!("VSS: obsługa={}, administrator={}\n", vss::supported(), vss::is_admin());
        let st = runner.status();
        let jobs = runner.load_jobs();
        o += &format!("\nZadania ({}), wstrzymane globalnie: {}\n", jobs.len(), runner.is_paused());
        for j in &jobs {
            let s = st.get(&j.id);
            o += &format!(
                "- „{}”: {} ⇄ {}, tryb {:?}, harmonogram {:?}, kosz {} dni, stan: {}{}\n",
                j.name, kind_of(&j.left), kind_of(&j.right), j.mode, j.schedule, j.trash_days,
                s.map(|s| s.state.clone()).unwrap_or_default(),
                s.and_then(|s| s.error.clone()).map(|e| format!(", błąd: {}", redact_paths(&e))).unwrap_or_default()
            );
            if let Some(l) = s.and_then(|s| s.last.as_ref()) {
                o += &format!("    ostatni przebieg: skopiowano {}, usunięto {}, przeniesiono {}, konflikty {}, zajęte {}, błędy {}{}\n",
                    l.copied, l.deleted, l.moved, l.conflicts.len(), l.busy.len(), l.errors.len(),
                    l.vss_note.as_ref().map(|n| format!(", VSS: {}", redact_paths(n))).unwrap_or_default());
            }
        }
        let log = std::fs::read_to_string(runner.state_dir().join("logs").join("syncron.log")).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(200).collect();
        o += "\nOstatnie wpisy dziennika:\n";
        o += &redact_paths(&tail.into_iter().rev().collect::<Vec<_>>().join("\n"));
        Ok(o)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_logs_dir(r: State<Runner>) -> Res<()> {
    let dir = r.state_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Overview {
    paused: bool,
    status: HashMap<String, Status>,
}

#[tauri::command]
fn list_jobs(r: State<Runner>) -> Vec<Job> {
    r.load_jobs()
}

#[tauri::command]
fn save_job(r: State<Runner>, job: Job) -> Res<()> {
    // wyłączenie starego, nieprawidłowego zadania musi być możliwe; pełna walidacja dotyczy włączonych
    if job.enabled {
        job.validate()?;
    }
    r.update_jobs(|mut jobs| {
        match jobs.iter().position(|j| j.id == job.id) {
            Some(i) => {
                if jobs[i].left != job.left || jobs[i].right != job.right {
                    engine::reset_state(r.state_dir(), &job.id).map_err(|e| e.to_string())?;
                }
                jobs[i] = job.clone();
            }
            None => jobs.push(job.clone()),
        }
        Ok(jobs)
    })?;
    r.start(job);
    Ok(())
}

#[tauri::command]
fn delete_job(r: State<Runner>, id: String) -> Res<()> {
    find_job(&r, &id)?;
    r.stop(&id);
    r.update_jobs(|jobs| Ok(jobs.into_iter().filter(|j| j.id != id).collect()))?;
    engine::reset_state(r.state_dir(), &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn run_now(r: State<Runner>, id: String, force: bool) -> bool {
    r.run_now(&id, force)
}

/// Symulacja: pokazuje plan działań bez zmian na dyskach.
#[tauri::command]
async fn dry_run(r: State<'_, Runner>, id: String) -> Res<Report> {
    let job = find_job(&r, &id)?;
    let dir = r.state_dir().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        engine::run_job(&job, &dir, true, false).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Ręczne rozstrzygnięcie konfliktu: `left_wins` wskazuje zwycięską stronę.
#[tauri::command]
async fn resolve_conflict(
    r: State<'_, Runner>,
    id: String,
    path: String,
    left_wins: bool,
) -> Res<()> {
    let job = find_job(&r, &id)?;
    let runner = r.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runner.with_job_lock(&job.id, || {
            engine::resolve_conflict(&job, runner.state_dir(), &path, left_wins)
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    r.run_now(&id, false);
    Ok(())
}

/// Podpowiedź przy wpisywaniu ścieżki: czy folder lokalny istnieje (dyski sieciowe mogą być chwilowo niedostępne).
#[tauri::command]
async fn check_path(path: String) -> Option<String> {
    // is_dir() na niedostępnym UNC potrafi blokować dziesiątki sekund: sprawdzamy w osobnym wątku z limitem czasu
    tauri::async_runtime::spawn_blocking(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let msg = match syncron_core::location::Location::parse(&path) {
                syncron_core::location::Location::Local(p)
                    if !p.as_os_str().is_empty() && !p.is_dir() =>
                {
                    Some("Ten folder nie jest teraz dostępny. Jeśli to dysk sieciowy, upewnij się, że jest podłączony.".to_string())
                }
                _ => None,
            };
            let _ = tx.send(msg);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(4)) {
            Ok(m) => m,
            Err(_) => Some("Nie można teraz sprawdzić tego folderu (brak odpowiedzi w ciągu 4 s). Jeśli to dysk sieciowy, sprawdź połączenie.".to_string()),
        }
    })
    .await
    .unwrap_or(None)
}

#[tauri::command]
fn overview(r: State<Runner>) -> Overview {
    Overview {
        paused: r.is_paused(),
        status: r.status(),
    }
}

#[tauri::command]
fn set_paused(r: State<Runner>, paused: bool) {
    r.set_paused(paused);
}

#[tauri::command]
fn history(r: State<Runner>, job: Option<String>, limit: usize) -> Res<Vec<HistoryRow>> {
    engine::history(r.state_dir(), job.as_deref(), limit.min(2000)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Remote {
    name: String,
    kind: String,
}

#[tauri::command]
async fn list_remotes() -> Res<Vec<Remote>> {
    tauri::async_runtime::spawn_blocking(|| {
        let out = run_rclone(&["listremotes", "--long"])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let (n, k) = l.split_once(':')?;
                Some(Remote {
                    name: n.trim().to_string(),
                    kind: k.trim().to_string(),
                })
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn check_remote_name(name: &str) -> Res<()> {
    if syncron_core::location::valid_remote_name(name) {
        Ok(())
    } else {
        Err("Nazwa konta: min. 2 znaki, zaczyna się od litery lub cyfry; tylko litery ASCII, cyfry, _ i -."
            .to_string())
    }
}

/// Tworzy konto rclone. Pytania konfiguratora (np. wybór dysku OneDrive) są zatwierdzane wartościami domyślnymi;
/// logowanie Google/Microsoft otwiera przeglądarkę. Sekrety (hasła, tokeny) nie trafiają do argumentów procesu:
/// są zapisywane bezpośrednio w pliku konfiguracji rclone (hasła zaciemnione przez `rclone obscure -`).
#[tauri::command]
async fn add_remote(name: String, kind: String, params: HashMap<String, String>) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || {
        check_remote_name(&name)?;
        if !kind.chars().all(|c| c.is_ascii_alphanumeric())
            || !REMOTE_KINDS.contains(&kind.as_str())
        {
            return Err(format!("Nieobsługiwany rodzaj konta: „{kind}”."));
        }
        if let Some(k) = params.keys().find(|k| !valid_param_key(k)) {
            return Err(format!("Niedozwolony parametr konta: „{k}”."));
        }
        if params.values().any(|v| v.contains(['\n', '\r'])) {
            return Err("Wartości pól konta nie mogą zawierać znaków nowej linii.".to_string());
        }
        if remote_exists(&name)? {
            return Err(format!(
                "Konto „{name}” już istnieje. Wybierz inną nazwę albo najpierw usuń to konto."
            ));
        }
        let result = create_remote(&name, &kind, &params);
        if result.is_err() {
            // nie zostawiaj częściowo utworzonego konta
            let _ = run_rclone(&["config", "delete", "--", &name]);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

fn is_secret_key(k: &str) -> bool {
    let k = k.to_lowercase();
    ["pass", "secret", "token", "key"]
        .iter()
        .any(|w| k.contains(w))
}

fn create_remote(name: &str, kind: &str, params: &HashMap<String, String>) -> Res<()> {
    let (secret, plain): (Vec<_>, Vec<_>) = params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .partition(|(k, _)| is_secret_key(k));
    let kv: Vec<String> = plain.iter().map(|(k, v)| k.to_string() + "=" + v).collect();
    let mut out;
    if secret.is_empty() {
        let mut args: Vec<&str> = vec!["config", "create", "--non-interactive", "--obscure", "--"];
        args.extend([name, kind]);
        args.extend(kv.iter().map(|s| s.as_str()));
        out = run_rclone(&args)?;
    } else {
        let mut keys = vec![("type".to_string(), kind.to_string())];
        for (k, v) in &secret {
            let v = if k.to_lowercase().contains("pass") {
                rclone_obscure(v)?
            } else {
                v.to_string()
            };
            keys.push((k.to_string(), v));
        }
        set_remote_keys(&rclone_config_path()?, name, &keys)?;
        let mut args: Vec<&str> = vec!["config", "update", "--non-interactive", "--obscure", "--"];
        args.push(name);
        args.extend(kv.iter().map(|s| s.as_str()));
        out = run_rclone(&args)?;
    }
    for _ in 0..20 {
        let v: serde_json::Value = serde_json::from_str(out.trim())
            .map_err(|_| "Konfigurator rclone zwrócił nieoczekiwaną odpowiedź.".to_string())?;
        if let Some(e) = v["Error"].as_str().filter(|e| !e.is_empty()) {
            return Err(e.to_string());
        }
        let state = v["State"].as_str().unwrap_or("");
        if state.is_empty() {
            return Ok(());
        }
        let opt = &v["Option"];
        if opt["IsPassword"].as_bool().unwrap_or(false) {
            return Err(
                "Konfigurator rclone wymaga podania hasła; uruchom `rclone config` ręcznie."
                    .to_string(),
            );
        }
        let mut answer = opt["Value"].as_str().unwrap_or("").to_string();
        if answer.is_empty() {
            answer = opt["DefaultStr"].as_str().unwrap_or("").to_string();
        }
        if answer.is_empty() {
            answer = opt["Examples"][0]["Value"]
                .as_str()
                .unwrap_or("")
                .to_string();
        }
        out = run_rclone(&[
            "config",
            "update",
            "--continue",
            "--state",
            state,
            "--result",
            &answer,
            "--non-interactive",
            "--",
            name,
        ])?;
    }
    Err("Konfigurator rclone nie zakończył się; uruchom `rclone config` ręcznie.".to_string())
}

#[tauri::command]
async fn delete_remote(name: String) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || {
        check_remote_name(&name)?;
        run_rclone(&["config", "delete", "--", &name]).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn remote_kinds() -> Vec<syncron_core::remote::spec::KindSpec> {
    syncron_core::remote::spec::remote_kinds()
}

#[derive(Serialize)]
struct RemoteDirEntry {
    name: String,
    is_dir: bool,
}

#[derive(Serialize)]
struct RemoteAbout {
    total: Option<u64>,
    used: Option<u64>,
    free: Option<u64>,
}

/// TODO(integracja): podłączyć natywny backend zdalny (przeglądanie jednego poziomu folderów).
#[tauri::command]
async fn browse_remote(name: String, path: String) -> Res<Vec<RemoteDirEntry>> {
    let _ = (name, path);
    Err("Przeglądanie folderów zdalnych: niezaimplementowane.".to_string())
}

/// TODO(integracja): podłączyć natywny backend zdalny (informacja o miejscu).
#[tauri::command]
async fn remote_about(name: String) -> Res<RemoteAbout> {
    let _ = name;
    Err("Informacja o miejscu na koncie: niezaimplementowane.".to_string())
}

#[tauri::command]
async fn test_remote(name: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || {
        check_remote_name(&name)?;
        run_rclone(&[
            "lsd",
            "--max-depth",
            "1",
            "--contimeout",
            "15s",
            "--",
            &format!("{name}:"),
        ])
        .map(|o| {
            format!(
                "Połączono. Folderów na najwyższym poziomie: {}",
                o.lines().count()
            )
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct VssStatus {
    supported: bool,
    admin: bool,
    elevated_autostart: bool,
}

#[tauri::command]
async fn vss_status() -> VssStatus {
    tauri::async_runtime::spawn_blocking(|| VssStatus {
        supported: vss::supported(),
        admin: vss::is_admin(),
        elevated_autostart: vss::elevated_autostart_enabled(),
    })
    .await
    .unwrap_or(VssStatus {
        supported: false,
        admin: false,
        elevated_autostart: false,
    })
}

/// Uruchamia aplikację ponownie jako administrator (monit UAC) i zamyka bieżącą instancję.
#[tauri::command]
async fn restart_elevated(app: tauri::AppHandle) -> Res<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || vss::restart_elevated(&exe))
        .await
        .map_err(|e| e.to_string())??;
    app.state::<Runner>().stop_all();
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn set_elevated_autostart(enable: bool) -> Res<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || vss::set_elevated_autostart(&exe, enable))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Settings {
    #[serde(default = "yes")]
    notify: bool,
    #[serde(default = "pl")]
    language: String,
    /// Po ilu minutach niedostępności zadania wysłać powiadomienie (0 = wyłączone).
    #[serde(default = "d30")]
    unavailable_minutes: u32,
}
fn d30() -> u32 {
    30
}
fn yes() -> bool {
    true
}
fn pl() -> String {
    "pl".into()
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            notify: true,
            language: pl(),
            unavailable_minutes: 30,
        }
    }
}

fn settings_path(r: &Runner) -> std::path::PathBuf {
    r.state_dir().join("settings.json")
}
fn load_settings(r: &Runner) -> Settings {
    std::fs::read_to_string(settings_path(r))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn get_settings(r: State<Runner>) -> Settings {
    load_settings(&r)
}

#[tauri::command]
fn set_settings(r: State<Runner>, mut settings: Settings) -> Res<()> {
    if settings.language != "pl" && settings.language != "en" {
        return Err("Nieobsługiwany język (dostępne: pl, en).".into());
    }
    settings.unavailable_minutes = settings.unavailable_minutes.min(10_080);
    std::fs::write(
        settings_path(&r),
        serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn find_job(r: &Runner, id: &str) -> Res<Job> {
    r.load_jobs()
        .into_iter()
        .find(|j| j.id == id)
        .ok_or_else(|| {
            "Nie znaleziono zadania o podanym identyfikatorze (mogło zostać usunięte).".to_string()
        })
}

#[tauri::command]
async fn trash_list(r: State<'_, Runner>, id: String) -> Res<Vec<TrashItem>> {
    let job = find_job(&r, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine::trash_list(&job).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn trash_restore(
    r: State<'_, Runner>,
    id: String,
    side: String,
    stamp: String,
    path: String,
) -> Res<String> {
    let job = find_job(&r, &id)?;
    let runner = r.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runner
            .with_job_lock(&job.id, || {
                engine::trash_restore(&job, &side, &stamp, &path)
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn trash_delete(
    r: State<'_, Runner>,
    id: String,
    side: String,
    stamp: Option<String>,
    path: Option<String>,
) -> Res<()> {
    let job = find_job(&r, &id)?;
    tauri::async_runtime::spawn_blocking(move || {
        engine::trash_delete(&job, &side, stamp.as_deref(), path.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Podfoldery folderu `rel` w lokalizacji `location` (wybór podfolderów w kreatorze).
#[tauri::command]
async fn list_dirs(location: String, rel: String) -> Res<Vec<String>> {
    tauri::async_runtime::spawn_blocking(move || {
        engine::list_dirs(&location, &rel).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Kopia konfiguracji zadań do pliku JSON (bez haseł i tokenów kont chmurowych).
#[tauri::command]
fn export_config(r: State<Runner>, path: String) -> Res<usize> {
    let jobs = r.load_jobs();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(jobs.len())
}

/// Import zadań z pliku: zadania o tym samym identyfikatorze są zastępowane, reszta dopisywana.
/// Zadania są walidowane i zapisywane jako wyłączone (`enabled = false`) — użytkownik włącza je po sprawdzeniu.
/// Zwraca komunikat do pokazania w interfejsie (np. „3 (wyłączone — włącz je po sprawdzeniu)”).
#[tauri::command]
fn import_config(r: State<Runner>, path: String) -> Res<String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("Nie można odczytać pliku importu: {e}"))?;
    let incoming = syncron_core::model::parse_import(&text)?;
    r.update_jobs(|mut jobs| {
        for j in &incoming {
            match jobs.iter().position(|x| x.id == j.id) {
                Some(i) => {
                    if jobs[i].left != j.left || jobs[i].right != j.right {
                        engine::reset_state(r.state_dir(), &j.id).map_err(|e| e.to_string())?;
                    }
                    jobs[i] = j.clone();
                }
                None => jobs.push(j.clone()),
            }
        }
        Ok(jobs)
    })?;
    for mut j in incoming.iter().cloned() {
        j.enabled = false;
        r.start(j); // wyłączone zadanie tylko zatrzymuje poprzednią wersję
    }
    Ok(format!(
        "{} (wyłączone — włącz je po sprawdzeniu)",
        incoming.len()
    ))
}

/// Elementy menu zasobnika (tłumaczone zgodnie z `settings.language`).
struct TrayMenu {
    show: MenuItem<tauri::Wry>,
    pause: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

impl TrayMenu {
    fn apply(&self, en: bool) {
        let (a, b, c) = if en {
            ("Open Syncron", "Pause / resume syncing", "Quit")
        } else {
            (
                "Otwórz Syncron",
                "Wstrzymaj / wznów synchronizację",
                "Zakończ",
            )
        };
        let _ = self.show.set_text(a);
        let _ = self.pause.set_text(b);
        let _ = self.quit.set_text(c);
    }
}

/// Wątek powiadomień systemowych: nowy błąd, zatrzymanie przez zabezpieczenie, nowy konflikt.
fn spawn_notifier(app: tauri::AppHandle, tray: tauri::tray::TrayIcon, menu: TrayMenu) {
    use tauri_plugin_notification::NotificationExt;
    std::thread::spawn(move || {
        let mut seen: HashMap<String, (String, usize)> = HashMap::new();
        // od kiedy zadanie jest w stanie błędu (czas, czy wysłano powiadomienie o długiej niedostępności)
        let mut down: HashMap<String, (std::time::Instant, bool)> = HashMap::new();
        let mut last_state = String::new();
        let mut last_lang = String::new();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(4));
            let r = app.state::<Runner>();
            let st = load_settings(&r);
            let en = st.language == "en";
            if st.language != last_lang {
                last_lang = st.language.clone();
                menu.apply(en);
                last_state.clear(); // podpowiedź ikony też w nowym języku
            }
            let jobs = r.load_jobs();
            // ikona i podpowiedź w zasobniku zależą od ogólnego stanu
            let (state, color) = overall_state(&r);
            if state != last_state {
                last_state = state.to_string();
                if let Some(base) = app.default_window_icon() {
                    let _ = tray.set_icon(Some(badge_icon(base, color)));
                }
                let tip = match (state, en) {
                    ("paused", false) => "Syncron — wstrzymane",
                    ("error", false) => "Syncron — błąd lub zatrzymanie",
                    ("attention", false) => "Syncron — coś wymaga uwagi",
                    ("running", false) => "Syncron — synchronizuję",
                    (_, false) => "Syncron — wszystko w porządku",
                    ("paused", true) => "Syncron — paused",
                    ("error", true) => "Syncron — error or stopped",
                    ("attention", true) => "Syncron — needs attention",
                    ("running", true) => "Syncron — syncing",
                    _ => "Syncron — all good",
                };
                let _ = tray.set_tooltip(Some(tip));
            }
            for (id, s) in r.status() {
                let name = jobs
                    .iter()
                    .find(|j| j.id == id)
                    .map(|j| j.name.clone())
                    .unwrap_or(id.clone());
                let conflicts = s.last.as_ref().map(|l| l.conflicts.len()).unwrap_or(0);
                let say = |body: String| {
                    let _ = app
                        .notification()
                        .builder()
                        .title(format!("Syncron — {name}"))
                        .body(body)
                        .show();
                };
                if s.state == "error" {
                    let e = down
                        .entry(id.clone())
                        .or_insert((std::time::Instant::now(), false));
                    let limit =
                        std::time::Duration::from_secs(u64::from(st.unavailable_minutes) * 60);
                    if st.notify && st.unavailable_minutes > 0 && !e.1 && e.0.elapsed() >= limit {
                        e.1 = true;
                        log::warn!(
                            "zadanie „{name}” jest niedostępne od {} min",
                            st.unavailable_minutes
                        );
                        say(if en {
                            format!(
                                "Unavailable for {} min: {}",
                                st.unavailable_minutes,
                                s.error.clone().unwrap_or_default()
                            )
                        } else {
                            format!(
                                "Niedostępne od {} min: {}",
                                st.unavailable_minutes,
                                s.error.clone().unwrap_or_default()
                            )
                        });
                    }
                } else if let Some((_, notified)) = down.remove(&id) {
                    if notified && st.notify {
                        log::info!("zadanie „{name}” znów działa");
                        say(if en {
                            "Back to normal.".to_string()
                        } else {
                            "Znów działa.".to_string()
                        });
                    }
                }
                let prev = seen.insert(id.clone(), (s.state.clone(), conflicts));
                let Some((pstate, pconf)) = prev else {
                    continue;
                };
                if !st.notify {
                    continue;
                }
                let msg = if s.state != pstate && s.state == "error" {
                    Some(if en {
                        format!("Error: {}", s.error.clone().unwrap_or_default())
                    } else {
                        format!("Błąd: {}", s.error.clone().unwrap_or_default())
                    })
                } else if s.state != pstate && s.state == "blocked" {
                    Some(if en {
                        "Stopped by a safety check — review the simulation.".to_string()
                    } else {
                        "Zatrzymano przez zabezpieczenie — sprawdź symulację.".to_string()
                    })
                } else if conflicts > pconf {
                    Some(if en {
                        format!("{conflicts} file(s) in conflict need a decision.")
                    } else {
                        format!("Konflikty do rozstrzygnięcia: {conflicts}.")
                    })
                } else {
                    None
                };
                if let Some(body) = msg {
                    let _ = app
                        .notification()
                        .builder()
                        .title(format!("Syncron — {name}"))
                        .body(body)
                        .show();
                }
            }
        }
    });
}

#[tauri::command]
fn rclone_version() -> Res<String> {
    run_rclone(&["version"]).map(|s| s.lines().next().unwrap_or("").to_string())
}

fn locate_rclone(app: &tauri::AppHandle) {
    let exe = if cfg!(windows) {
        "rclone.exe"
    } else {
        "rclone"
    };
    if let Ok(dir) = app.path().resource_dir() {
        for p in [dir.join("bin").join(exe), dir.join(exe)] {
            if p.is_file() {
                std::env::set_var("SYNCRON_RCLONE", &p);
                break;
            }
        }
    }
}

/// Dopasowuje rozmiar okna do dostępnego obszaru ekranu (bez paska zadań): na niskich ekranach okno
/// o stałej wysokości wystawało poza ekran i ucinało stopkę z przyciskami kreatora.
fn fit_window(w: &tauri::WebviewWindow) {
    let Ok(Some(mon)) = w.current_monitor().or_else(|_| w.primary_monitor()) else {
        return;
    };
    let scale = mon.scale_factor().max(0.5);
    let area = mon.work_area();
    // zapas na ramkę i pasek tytułu okna
    let max_w = (area.size.width as f64 / scale - 24.0).max(480.0);
    let max_h = (area.size.height as f64 / scale - 48.0).max(400.0);
    let (want_w, want_h) = (1040.0_f64.min(max_w), 700.0_f64.min(max_h));
    let _ = w.set_min_size(Some(tauri::LogicalSize::new(
        860.0_f64.min(max_w),
        480.0_f64.min(max_h),
    )));
    let _ = w.set_size(tauri::LogicalSize::new(want_w, want_h));
    let _ = w.center();
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            syncron_core::legacy::migrate_data_dir(&dir); // dane z czasów SyncPolak
            init_logging(&dir);
            // własna konfiguracja rclone w folderze danych aplikacji
            std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));
            locate_rclone(app.handle());
            vss::set_data_dir(&dir); // rejestr migawek i kopie tymczasowe w prywatnym katalogu danych
            vss::cleanup_stale(); // migawki pozostawione po awarii
            let runner = Runner::new(dir);
            runner.start_all();
            runner.start_monitor(); // dysk podłączony / wybudzenie / zmiana sieci
            app.manage(runner);

            let show = MenuItem::with_id(app, "show", "Otwórz Syncron", true, None::<&str>)?;
            let pause = MenuItem::with_id(
                app,
                "pause",
                "Wstrzymaj / wznów synchronizację",
                true,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Zakończ", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &pause, &quit])?;
            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Syncron")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "show" => show_main(app),
                    "pause" => {
                        let r = app.state::<Runner>();
                        r.set_paused(!r.is_paused());
                    }
                    "quit" => {
                        app.state::<Runner>().stop_all();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let tauri::tray::TrayIconEvent::DoubleClick { .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            let tm = TrayMenu { show, pause, quit };
            tm.apply(load_settings(&app.state::<Runner>()).language == "en");
            spawn_notifier(app.handle().clone(), tray, tm);

            if let Some(w) = app.get_webview_window("main") {
                fit_window(&w);
                if std::env::args().any(|a| a == "--hidden") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|w, ev| {
            // zamknięcie okna chowa aplikację do zasobnika; synchronizacja trwa w tle
            if let WindowEvent::CloseRequested { api, .. } = ev {
                let _ = w.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_jobs,
            save_job,
            delete_job,
            run_now,
            dry_run,
            resolve_conflict,
            check_path,
            overview,
            set_paused,
            history,
            list_remotes,
            add_remote,
            delete_remote,
            test_remote,
            remote_kinds,
            browse_remote,
            remote_about,
            rclone_version,
            vss_status,
            get_settings,
            set_settings,
            trash_list,
            trash_restore,
            trash_delete,
            list_dirs,
            export_config,
            get_diagnostics,
            open_logs_dir,
            import_config,
            restart_elevated,
            set_elevated_autostart
        ])
        .run(tauri::generate_context!())
        .expect("błąd uruchamiania Syncron");
}

#[cfg(test)]
mod tests {
    use super::redact_paths;

    #[test]
    fn redaguje_sciezki() {
        assert_eq!(
            redact_paths("błąd: nie można zapisać „C:\\Users\\Jan Kowalski\\a.txt”: odmowa"),
            "błąd: nie można zapisać „<ścieżka>”: odmowa"
        );
        assert_eq!(
            redact_paths("open /home/jan/x.txt: denied"),
            "open <ścieżka>: denied"
        );
        assert_eq!(redact_paths("\\\\serwer\\udział\\f"), "<ścieżka>");
        assert_eq!(redact_paths("https://x.pl/a i 1/2"), "https://x.pl/a i 1/2");
        assert_eq!(redact_paths("zwykły tekst"), "zwykły tekst");
    }
}
