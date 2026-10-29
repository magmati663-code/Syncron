#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::collections::HashMap;
use syncron_core::accounts::{self, AboutInfo, DirEntryInfo, ImportReport};
use syncron_core::engine::{self, HistoryRow, Report, TrashItem};
use syncron_core::model::Job;
use syncron_core::remote::spec::KindSpec;
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
        syncron_core::location::Location::Remote { account, .. } => {
            format!("chmura ({account})")
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
/// Ścieżki w błędach i dzienniku są zastąpione `<ścieżka>`; nazwy zadań i kont zdalnych pozostają.
#[tauri::command]
async fn get_diagnostics(r: State<'_, Runner>) -> Res<String> {
    let runner = r.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut o = String::new();
        o += &format!("Syncron {}\nSystem: {} {}\n", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
        o += &format!(
            "Konta zdalne: {}\n",
            accounts::global()
                .and_then(|st| st.list().ok())
                .map(|v| v.len().to_string())
                .unwrap_or_else(|| "niedostępne".into())
        );
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
    /// Konto wymaga zalogowania (zaimportowane z rclone albo brakuje zapisanego hasła/tokenu).
    needs_login: bool,
    /// Tylko wartości jawne (hasła i tokeny nigdy nie opuszczają magazynu sekretów).
    params: std::collections::BTreeMap<String, String>,
}

fn store() -> Res<std::sync::Arc<accounts::Store>> {
    accounts::global().ok_or_else(|| "Magazyn kont nie jest gotowy.".to_string())
}

/// Wykonuje blokującą operację na kontach poza wątkiem interfejsu (sieć, klucze systemu).
async fn blocking<T: Send + 'static>(
    f: impl FnOnce(std::sync::Arc<accounts::Store>) -> Res<T> + Send + 'static,
) -> Res<T> {
    let st = store()?;
    tauri::async_runtime::spawn_blocking(move || f(st))
        .await
        .map_err(|e| e.to_string())?
}

/// Opis formularzy kont (rodzaje i pola) do dynamicznego budowania interfejsu.
#[tauri::command]
fn remote_kinds() -> Vec<KindSpec> {
    syncron_core::remote::spec::remote_kinds()
}

#[tauri::command]
async fn list_remotes() -> Res<Vec<Remote>> {
    blocking(|st| {
        Ok(st
            .list_view()?
            .into_iter()
            .map(|a| Remote {
                name: a.name,
                kind: a.kind,
                needs_login: a.needs_login,
                params: a.params,
            })
            .collect())
    })
    .await
}

/// Tworzy konto natywne. Jawne pola trafiają do `accounts.json`, hasła i tokeny do magazynu sekretów
/// (klucze systemu albo zaszyfrowany plik) — nigdy do argumentów procesów ani dziennika.
#[tauri::command]
async fn add_remote(name: String, kind: String, params: HashMap<String, String>) -> Res<()> {
    blocking(move |st| {
        st.add(&name, &kind, &params.into_iter().collect())
            .map(|_| ())
    })
    .await
}

/// Zmienia konto: jawne pola są zastępowane, sekrety tylko gdy podano nową (niepustą) wartość.
#[tauri::command]
async fn update_remote(name: String, params: HashMap<String, String>) -> Res<()> {
    blocking(move |st| st.update(&name, &params.into_iter().collect()).map(|_| ())).await
}

#[tauri::command]
async fn delete_remote(name: String) -> Res<()> {
    blocking(move |st| st.delete(&name)).await
}

/// Test połączenia: zwraca opis z czasem odpowiedzi.
#[tauri::command]
async fn test_remote(name: String) -> Res<String> {
    blocking(move |st| {
        let t = st.test(&name)?;
        Ok(format!("Połączono ({} ms). {}", t.ms, t.info))
    })
    .await
}

/// Zawartość folderu na koncie (jeden poziom) do przeglądarki w kreatorze.
#[tauri::command]
async fn browse_remote(name: String, path: String) -> Res<Vec<DirEntryInfo>> {
    blocking(move |st| st.browse(&name, &path)).await
}

/// Miejsce na koncie (jeśli serwer je podaje).
#[tauri::command]
async fn remote_about(name: String) -> Res<AboutInfo> {
    blocking(move |st| st.about(&name)).await
}

/// Stary `rclone.conf` w folderze danych, który można zaimportować (aplikacja proponuje import).
#[tauri::command]
fn rclone_import_offer() -> Option<String> {
    let st = accounts::global()?;
    accounts::pending_rclone_import(st.dir()).map(|p| p.to_string_lossy().into_owned())
}

/// Użytkownik odrzucił propozycję importu.
#[tauri::command]
fn rclone_import_dismiss() {
    if let Some(st) = accounts::global() {
        accounts::dismiss_rclone_import(st.dir());
    }
}

/// Import kont z `rclone.conf` (domyślnie z folderu danych). `remove_source` usuwa plik po imporcie bez błędów.
#[tauri::command]
async fn import_rclone_conf(
    path: Option<String>,
    remove_source: Option<bool>,
) -> Res<ImportReport> {
    blocking(move |st| {
        let p = match path {
            Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => st.dir().join("rclone.conf"),
        };
        let mut rep = st.import_rclone_conf(&p)?;
        if remove_source.unwrap_or(false) && rep.skipped.is_empty() {
            rep.source_removed = st.remove_rclone_source(&p).is_ok();
        }
        Ok(rep)
    })
    .await
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

/// Zapamiętane położenie i rozmiar okna (piksele fizyczne; rozmiar = obszar roboczy okna).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
struct WinState {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    #[serde(default)]
    maximized: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct Settings {
    #[serde(default = "yes")]
    notify: bool,
    #[serde(default = "pl")]
    language: String,
    /// Po ilu minutach niedostępności zadania wysłać powiadomienie (0 = wyłączone).
    #[serde(default = "d30")]
    unavailable_minutes: u32,
    /// Skala interfejsu 0.75..=2.0 (krok 0.05).
    #[serde(default = "one")]
    ui_scale: f64,
    /// Skala tekstu 0.85..=1.5 (krok 0.05).
    #[serde(default = "one")]
    font_scale: f64,
    /// `compact` | `normal` | `comfortable`.
    #[serde(default = "normal")]
    density: String,
    /// `auto` | `expanded` | `collapsed`.
    #[serde(default = "auto")]
    sidebar: String,
    #[serde(default)]
    reduce_motion: bool,
    #[serde(default)]
    high_contrast: bool,
    /// `auto` | `light` | `dark`.
    #[serde(default = "auto")]
    theme: String,
    /// Zarządzane wyłącznie przez backend (zapis przy zmianie okna); `set_settings` ignoruje wartość z interfejsu.
    #[serde(default)]
    window: Option<WinState>,
}
fn d30() -> u32 {
    30
}
fn one() -> f64 {
    1.0
}
fn normal() -> String {
    "normal".into()
}
fn auto() -> String {
    "auto".into()
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
            ui_scale: 1.0,
            font_scale: 1.0,
            density: normal(),
            sidebar: auto(),
            reduce_motion: false,
            high_contrast: false,
            theme: auto(),
            window: None,
        }
    }
}

/// Wartość z zakresu `lo..=hi` zaokrąglona do kroku 0,05; poza zakresem lub nie-liczba = błąd.
fn check_scale(v: f64, lo: f64, hi: f64, what: &str) -> Res<f64> {
    if !v.is_finite() || v < lo - 1e-9 || v > hi + 1e-9 {
        return Err(format!(
            "{what}: dozwolony zakres to {:.0}%–{:.0}%.",
            lo * 100.0,
            hi * 100.0
        ));
    }
    Ok(((v * 20.0).round() / 20.0).clamp(lo, hi))
}

impl Settings {
    /// Walidacja z komunikatem po polsku (polecenie `set_settings`).
    fn validate(&mut self) -> Res<()> {
        if self.language != "pl" && self.language != "en" {
            return Err("Nieobsługiwany język (dostępne: pl, en).".into());
        }
        if !["compact", "normal", "comfortable"].contains(&self.density.as_str()) {
            return Err("Nieobsługiwana gęstość (dostępne: compact, normal, comfortable).".into());
        }
        if !["auto", "expanded", "collapsed"].contains(&self.sidebar.as_str()) {
            return Err(
                "Nieobsługiwany tryb paska bocznego (dostępne: auto, expanded, collapsed).".into(),
            );
        }
        if !["auto", "light", "dark"].contains(&self.theme.as_str()) {
            return Err("Nieobsługiwany motyw (dostępne: auto, light, dark).".into());
        }
        self.ui_scale = check_scale(self.ui_scale, 0.75, 2.0, "Skala interfejsu")?;
        self.font_scale = check_scale(self.font_scale, 0.85, 1.5, "Skala tekstu")?;
        self.unavailable_minutes = self.unavailable_minutes.min(10_080);
        Ok(())
    }

    /// Łagodna naprawa wartości z pliku (uszkodzony/ręcznie edytowany settings.json nie może wyłączyć interfejsu).
    fn sanitize(&mut self) {
        let d = Settings::default();
        if self.language != "pl" && self.language != "en" {
            self.language = d.language;
        }
        self.ui_scale = if self.ui_scale.is_finite() {
            ((self.ui_scale.clamp(0.75, 2.0)) * 20.0).round() / 20.0
        } else {
            1.0
        };
        self.font_scale = if self.font_scale.is_finite() {
            ((self.font_scale.clamp(0.85, 1.5)) * 20.0).round() / 20.0
        } else {
            1.0
        };
        if !["compact", "normal", "comfortable"].contains(&self.density.as_str()) {
            self.density = d.density;
        }
        if !["auto", "expanded", "collapsed"].contains(&self.sidebar.as_str()) {
            self.sidebar = d.sidebar;
        }
        if !["auto", "light", "dark"].contains(&self.theme.as_str()) {
            self.theme = d.theme;
        }
        if let Some(w) = &self.window {
            if w.w < 200
                || w.h < 150
                || w.w > 30_000
                || w.h > 30_000
                || w.x.abs() > 100_000
                || w.y.abs() > 100_000
            {
                self.window = None;
            }
        }
    }
}

fn settings_path(r: &Runner) -> std::path::PathBuf {
    r.state_dir().join("settings.json")
}
/// Serializuje odczyt-modyfikację-zapis settings.json (polecenie `set_settings` i zapis okna z wątku tła).
static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn load_settings(r: &Runner) -> Settings {
    let mut s: Settings = std::fs::read_to_string(settings_path(r))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    s.sanitize();
    s
}
fn write_settings(r: &Runner, s: &Settings) -> Res<()> {
    std::fs::write(
        settings_path(r),
        serde_json::to_string_pretty(s).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(r: State<Runner>) -> Settings {
    load_settings(&r)
}

#[tauri::command]
fn set_settings(r: State<Runner>, settings: Settings) -> Res<()> {
    let mut settings = settings;
    settings.validate()?;
    let _g = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    settings.window = load_settings(&r).window; // położenie okna zapisuje wyłącznie backend
    write_settings(&r, &settings)
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

/// Przebieg „z wykluczeniami”: wykonuje plan zadania bez akcji o kluczach z `skip` (klucze jak w `Report.planned`).
#[tauri::command]
async fn run_selected(r: State<'_, Runner>, id: String, skip: Vec<String>) -> Res<Report> {
    let job = find_job(&r, &id)?;
    let runner = r.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        runner
            .run_selected(&job, skip.into_iter().collect())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import ustawień z FreeFileSync (`.ffs_gui` / `.ffs_batch`). Zwraca zadania (wyłączone, bez poleceń) do zapisania
/// przez interfejs poleceniem `save_job` po przejrzeniu; sam nic nie zapisuje.
#[tauri::command]
fn import_freefilesync(path: String) -> Res<Vec<Job>> {
    syncron_core::ffs::read_ffs_file(std::path::Path::new(&path))
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

/// Obszar roboczy monitora (bez paska zadań) w pikselach fizycznych oraz jego skala.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    scale: f64,
}

/// Minimalny rozmiar okna (piksele logiczne).
const MIN_W: f64 = 860.0;
const MIN_H: f64 = 480.0;

/// Przycina zapamiętane okno do dostępnych monitorów (pierwszy na liście = główny): wybiera monitor o największym
/// przecięciu (inaczej główny), ogranicza rozmiar do obszaru roboczego i wsuwa okno w obszar (także ujemne współrzędne).
fn restore_rect(saved: &WinState, areas: &[Area]) -> Option<(i32, i32, u32, u32)> {
    let first = *areas.first()?;
    let overlap = |a: &Area| -> i64 {
        let x1 = saved.x.max(a.x) as i64;
        let y1 = saved.y.max(a.y) as i64;
        let x2 = (saved.x as i64 + saved.w as i64).min(a.x as i64 + a.w as i64);
        let y2 = (saved.y as i64 + saved.h as i64).min(a.y as i64 + a.h as i64);
        (x2 - x1).max(0) * (y2 - y1).max(0)
    };
    let mut best = first;
    let mut best_ov = overlap(&first);
    for a in &areas[1..] {
        let o = overlap(a);
        if o > best_ov {
            best = *a;
            best_ov = o;
        }
    }
    let min_w = ((MIN_W * best.scale).round() as u32).min(best.w);
    let min_h = ((MIN_H * best.scale).round() as u32).min(best.h);
    let w = saved.w.clamp(min_w, best.w);
    let h = saved.h.clamp(min_h, best.h);
    let x = saved.x.clamp(best.x, best.x + (best.w - w) as i32);
    let y = saved.y.clamp(best.y, best.y + (best.h - h) as i32);
    Some((x, y, w, h))
}

fn monitor_areas(w: &tauri::WebviewWindow) -> Vec<Area> {
    let to_area = |m: &tauri::Monitor| {
        let a = m.work_area();
        Area {
            x: a.position.x,
            y: a.position.y,
            w: a.size.width,
            h: a.size.height,
            scale: m.scale_factor().max(0.5),
        }
    };
    let mut out = Vec::new();
    if let Ok(Some(p)) = w.primary_monitor() {
        out.push(to_area(&p));
    }
    if let Ok(list) = w.available_monitors() {
        for m in &list {
            let a = to_area(m);
            if !out.iter().any(|o| o.x == a.x && o.y == a.y && o.w == a.w) {
                out.push(a);
            }
        }
    }
    out
}

/// Dopasowuje rozmiar okna do dostępnego obszaru ekranu (bez paska zadań): na niskich ekranach okno
/// o stałej wysokości wystawało poza ekran i ucinało stopkę z przyciskami kreatora. Jeśli zapamiętano położenie
/// z poprzedniego uruchomienia, jest przywracane (przycięte do aktualnych monitorów).
fn fit_window(w: &tauri::WebviewWindow, saved: Option<&WinState>) {
    let Ok(Some(mon)) = w.current_monitor().or_else(|_| w.primary_monitor()) else {
        return;
    };
    let scale = mon.scale_factor().max(0.5);
    let area = mon.work_area();
    // zapas na ramkę i pasek tytułu okna
    let max_w = (area.size.width as f64 / scale - 24.0).max(480.0);
    let max_h = (area.size.height as f64 / scale - 48.0).max(400.0);
    let _ = w.set_min_size(Some(tauri::LogicalSize::new(
        MIN_W.min(max_w),
        MIN_H.min(max_h),
    )));
    if let Some(st) = saved {
        if let Some((x, y, rw, rh)) = restore_rect(st, &monitor_areas(w)) {
            let _ = w.set_size(tauri::PhysicalSize::new(rw, rh));
            let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
            if st.maximized {
                let _ = w.maximize();
            }
            return;
        }
    }
    let (want_w, want_h) = (1040.0_f64.min(max_w), 700.0_f64.min(max_h));
    let _ = w.set_size(tauri::LogicalSize::new(want_w, want_h));
    let _ = w.center();
}

/// Zapisuje położenie, rozmiar i stan zmaksymalizowania okna w settings.json.
fn persist_window(w: &tauri::WebviewWindow) {
    if w.is_minimized().unwrap_or(false) || !w.is_visible().unwrap_or(true) {
        return;
    }
    let Some(r) = w.app_handle().try_state::<Runner>() else {
        return;
    };
    let maximized = w.is_maximized().unwrap_or(false);
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) else {
        return;
    };
    let _g = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut st = load_settings(&r);
    let prev = st.window.clone();
    st.window = if maximized {
        // przy zmaksymalizowanym oknie zachowujemy ostatni „normalny” rozmiar do przywrócenia
        Some(match &prev {
            Some(p) => WinState {
                maximized: true,
                ..p.clone()
            },
            None => WinState {
                x: pos.x,
                y: pos.y,
                w: size.width,
                h: size.height,
                maximized: true,
            },
        })
    } else {
        Some(WinState {
            x: pos.x,
            y: pos.y,
            w: size.width,
            h: size.height,
            maximized: false,
        })
    };
    if st.window != prev {
        let _ = write_settings(&r, &st);
    }
}

static WIN_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static WIN_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Zapis położenia okna z opóźnieniem (debounce): przeciąganie generuje setki zdarzeń.
fn schedule_window_save(w: &tauri::Window) {
    use std::sync::atomic::Ordering::SeqCst;
    WIN_GEN.fetch_add(1, SeqCst);
    if WIN_PENDING.swap(true, SeqCst) {
        return;
    }
    let app = w.app_handle().clone();
    let label = w.label().to_string();
    std::thread::spawn(move || {
        loop {
            let g = WIN_GEN.load(SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(700));
            if WIN_GEN.load(SeqCst) == g {
                break;
            }
        }
        WIN_PENDING.store(false, SeqCst);
        if let Some(w) = app.get_webview_window(&label) {
            persist_window(&w);
        }
    });
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
            // konta zdalnych magazynów (accounts.json + sekrety) w folderze danych aplikacji
            accounts::set_data_dir(&dir);
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
                        if let Some(w) = app.get_webview_window("main") {
                            persist_window(&w);
                        }
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
                let saved = load_settings(&app.state::<Runner>()).window;
                fit_window(&w, saved.as_ref());
                if std::env::args().any(|a| a == "--hidden") {
                    let _ = w.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|w, ev| {
            // zamknięcie okna chowa aplikację do zasobnika; synchronizacja trwa w tle
            match ev {
                WindowEvent::CloseRequested { api, .. } => {
                    if let Some(ww) = w.app_handle().get_webview_window(w.label()) {
                        persist_window(&ww);
                    }
                    let _ = w.hide();
                    api.prevent_close();
                }
                WindowEvent::Resized(_) | WindowEvent::Moved(_) => schedule_window_save(w),
                _ => {}
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
            update_remote,
            test_remote,
            browse_remote,
            remote_about,
            remote_kinds,
            import_rclone_conf,
            rclone_import_offer,
            rclone_import_dismiss,
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
            run_selected,
            import_freefilesync,
            restart_elevated,
            set_elevated_autostart
        ])
        .run(tauri::generate_context!())
        .expect("błąd uruchamiania Syncron");
}

#[cfg(test)]
mod tests {
    use super::{redact_paths, restore_rect, Area, Settings, WinState};

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

    #[test]
    fn stare_ustawienia_dostaja_domyslne_nowe_pola() {
        let s: Settings =
            serde_json::from_str(r#"{"notify":false,"language":"en","unavailable_minutes":5}"#)
                .unwrap();
        assert!(!s.notify);
        assert_eq!(s.language, "en");
        assert_eq!(s.ui_scale, 1.0);
        assert_eq!(s.font_scale, 1.0);
        assert_eq!(s.density, "normal");
        assert_eq!(s.sidebar, "auto");
        assert_eq!(s.theme, "auto");
        assert!(!s.reduce_motion && !s.high_contrast);
        assert!(s.window.is_none());
        let e: Settings = serde_json::from_str("{}").unwrap();
        assert!(e.notify && e.language == "pl");
    }

    #[test]
    fn ustawienia_wygladu_roundtrip() {
        let s = Settings {
            ui_scale: 1.25,
            font_scale: 1.1,
            density: "comfortable".into(),
            sidebar: "collapsed".into(),
            reduce_motion: true,
            high_contrast: true,
            theme: "dark".into(),
            window: Some(WinState {
                x: -10,
                y: 20,
                w: 1000,
                h: 700,
                maximized: true,
            }),
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let b: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(b.ui_scale, 1.25);
        assert_eq!(b.density, "comfortable");
        assert_eq!(b.window, s.window);
    }

    #[test]
    fn walidacja_zakresow() {
        let ok = |f: &dyn Fn(&mut Settings)| {
            let mut s = Settings::default();
            f(&mut s);
            s.validate().map(|_| s)
        };
        assert!(ok(&|s| s.ui_scale = 0.74).is_err());
        assert!(ok(&|s| s.ui_scale = 2.01).is_err());
        assert!(ok(&|s| s.ui_scale = f64::NAN).is_err());
        assert!(ok(&|s| s.font_scale = 0.8).is_err());
        assert!(ok(&|s| s.font_scale = 1.6).is_err());
        assert!(ok(&|s| s.density = "huge".into()).is_err());
        assert!(ok(&|s| s.sidebar = "left".into()).is_err());
        assert!(ok(&|s| s.theme = "pink".into()).is_err());
        assert!(ok(&|s| s.language = "de".into()).is_err());
        assert_eq!(ok(&|s| s.ui_scale = 0.75).unwrap().ui_scale, 0.75);
        assert_eq!(ok(&|s| s.ui_scale = 2.0).unwrap().ui_scale, 2.0);
        assert_eq!(ok(&|s| s.ui_scale = 1.1234).unwrap().ui_scale, 1.1);
        assert_eq!(ok(&|s| s.font_scale = 1.5).unwrap().font_scale, 1.5);
    }

    #[test]
    fn sanityzacja_pliku_naprawia_wartosci() {
        let mut s: Settings = serde_json::from_str(
            r#"{"ui_scale":9,"font_scale":0.1,"density":"x","sidebar":"y","theme":"z","language":"fr","window":{"x":0,"y":0,"w":10,"h":10}}"#,
        )
        .unwrap();
        s.sanitize();
        assert_eq!((s.ui_scale, s.font_scale), (2.0, 0.85));
        assert_eq!(
            (
                s.density.as_str(),
                s.sidebar.as_str(),
                s.theme.as_str(),
                s.language.as_str()
            ),
            ("normal", "auto", "auto", "pl")
        );
        assert!(s.window.is_none());
    }

    fn area(x: i32, y: i32, w: u32, h: u32) -> Area {
        Area {
            x,
            y,
            w,
            h,
            scale: 1.0,
        }
    }

    #[test]
    fn przywracanie_okna_miesci_sie_i_przycina() {
        let main = area(0, 0, 1920, 1040);
        // mieści się: bez zmian
        let st = WinState {
            x: 100,
            y: 50,
            w: 1000,
            h: 700,
            maximized: false,
        };
        assert_eq!(restore_rect(&st, &[main]), Some((100, 50, 1000, 700)));
        // większe niż ekran: przycięte do obszaru roboczego
        let big = WinState {
            x: -50,
            y: -50,
            w: 5000,
            h: 4000,
            maximized: false,
        };
        assert_eq!(restore_rect(&big, &[main]), Some((0, 0, 1920, 1040)));
        // poza ekranem (odłączony monitor): wsunięte na główny
        let off = WinState {
            x: 4000,
            y: 3000,
            w: 1000,
            h: 700,
            maximized: false,
        };
        assert_eq!(restore_rect(&off, &[main]), Some((920, 340, 1000, 700)));
        // monitor po lewej (ujemne współrzędne) pozostaje wybrany, gdy istnieje
        let left = area(-1280, 0, 1280, 984);
        let on_left = WinState {
            x: -1200,
            y: 100,
            w: 900,
            h: 600,
            maximized: false,
        };
        assert_eq!(
            restore_rect(&on_left, &[main, left]),
            Some((-1200, 100, 900, 600))
        );
        // monitor po lewej odłączony: okno wraca na główny
        assert_eq!(restore_rect(&on_left, &[main]), Some((0, 100, 900, 600)));
        // za małe: podniesione do minimum (860x480)
        let tiny = WinState {
            x: 10,
            y: 10,
            w: 300,
            h: 200,
            maximized: false,
        };
        assert_eq!(restore_rect(&tiny, &[main]), Some((10, 10, 860, 480)));
        assert_eq!(restore_rect(&tiny, &[]), None);
    }
}
