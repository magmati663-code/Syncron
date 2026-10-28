#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::collections::HashMap;
use syncpolak_core::engine::{self, HistoryRow, Report, TrashItem};
use syncpolak_core::location::run_rclone;
use syncpolak_core::model::Job;
use syncpolak_core::runner::{Runner, Status};
use syncpolak_core::vss;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

type Res<T> = Result<T, String>;

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
    if job.name.trim().is_empty() || job.left.trim().is_empty() || job.right.trim().is_empty() {
        return Err("Podaj nazwę oraz obie lokalizacje.".into());
    }
    if job.left.trim() == job.right.trim() {
        return Err("Lewa i prawa lokalizacja nie mogą być takie same.".into());
    }
    let mut jobs = r.load_jobs();
    match jobs.iter().position(|j| j.id == job.id) {
        Some(i) => {
            if jobs[i].left != job.left || jobs[i].right != job.right {
                engine::reset_state(r.state_dir(), &job.id).map_err(|e| e.to_string())?;
            }
            jobs[i] = job.clone();
        }
        None => jobs.push(job.clone()),
    }
    r.save_jobs(&jobs).map_err(|e| e.to_string())?;
    r.start(job);
    Ok(())
}

#[tauri::command]
fn delete_job(r: State<Runner>, id: String) -> Res<()> {
    r.stop(&id);
    let jobs: Vec<Job> = r.load_jobs().into_iter().filter(|j| j.id != id).collect();
    r.save_jobs(&jobs).map_err(|e| e.to_string())?;
    engine::reset_state(r.state_dir(), &id).map_err(|e| e.to_string())
}

#[tauri::command]
fn run_now(r: State<Runner>, id: String, force: bool) -> bool {
    r.run_now(&id, force)
}

/// Symulacja: pokazuje plan działań bez zmian na dyskach.
#[tauri::command]
async fn dry_run(r: State<'_, Runner>, id: String) -> Res<Report> {
    let job = r
        .load_jobs()
        .into_iter()
        .find(|j| j.id == id)
        .ok_or("brak zadania")?;
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
    let job = r
        .load_jobs()
        .into_iter()
        .find(|j| j.id == id)
        .ok_or("brak zadania")?;
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
fn check_path(path: String) -> Option<String> {
    match syncpolak_core::location::Location::parse(&path) {
        syncpolak_core::location::Location::Local(p) if !p.as_os_str().is_empty() && !p.is_dir() => {
            Some("Ten folder nie jest teraz dostępny. Jeśli to dysk sieciowy, upewnij się, że jest podłączony.".into())
        }
        _ => None,
    }
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

/// Tworzy konto rclone. Pytania konfiguratora (np. wybór dysku OneDrive) są zatwierdzane wartościami domyślnymi;
/// logowanie Google/Microsoft otwiera przeglądarkę.
#[tauri::command]
async fn add_remote(name: String, kind: String, params: HashMap<String, String>) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || {
        if name.len() < 2
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("Nazwa konta: min. 2 znaki, tylko litery ASCII, cyfry, _ i -.".to_string());
        }
        let kv: Vec<String> = params
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let mut args: Vec<&str> = vec!["config", "create", &name, &kind];
        args.extend(kv.iter().map(|s| s.as_str()));
        args.extend(["--non-interactive", "--obscure"]);
        let mut out = run_rclone(&args)?;
        for _ in 0..20 {
            let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or_default();
            if let Some(e) = v["Error"].as_str().filter(|e| !e.is_empty()) {
                return Err(e.to_string());
            }
            let state = v["State"].as_str().unwrap_or("");
            if state.is_empty() {
                return Ok(());
            }
            let opt = &v["Option"];
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
                &name,
                "--continue",
                "--state",
                state,
                "--result",
                &answer,
                "--non-interactive",
            ])?;
        }
        Err("Konfigurator rclone nie zakończył się; uruchom `rclone config` ręcznie.".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn delete_remote(name: String) -> Res<()> {
    tauri::async_runtime::spawn_blocking(move || {
        run_rclone(&["config", "delete", &name]).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn test_remote(name: String) -> Res<String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_rclone(&[
            "lsd",
            &format!("{name}:"),
            "--max-depth",
            "1",
            "--contimeout",
            "15s",
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
fn set_settings(r: State<Runner>, settings: Settings) -> Res<()> {
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
        .ok_or_else(|| "brak zadania".to_string())
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
#[tauri::command]
fn import_config(r: State<Runner>, path: String) -> Res<usize> {
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let incoming: Vec<Job> = serde_json::from_str(&text)
        .map_err(|e| format!("to nie jest plik konfiguracji SyncPolak: {e}"))?;
    let mut jobs = r.load_jobs();
    for j in &incoming {
        match jobs.iter().position(|x| x.id == j.id) {
            Some(i) => jobs[i] = j.clone(),
            None => jobs.push(j.clone()),
        }
    }
    r.save_jobs(&jobs).map_err(|e| e.to_string())?;
    for j in incoming.iter() {
        r.start(j.clone());
    }
    Ok(incoming.len())
}

/// Wątek powiadomień systemowych: nowy błąd, zatrzymanie przez zabezpieczenie, nowy konflikt.
fn spawn_notifier(app: tauri::AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    std::thread::spawn(move || {
        let mut seen: HashMap<String, (String, usize)> = HashMap::new();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(4));
            let r = app.state::<Runner>();
            let st = load_settings(&r);
            let en = st.language == "en";
            let jobs = r.load_jobs();
            for (id, s) in r.status() {
                let name = jobs
                    .iter()
                    .find(|j| j.id == id)
                    .map(|j| j.name.clone())
                    .unwrap_or(id.clone());
                let conflicts = s.last.as_ref().map(|l| l.conflicts.len()).unwrap_or(0);
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
                        .title(format!("SyncPolak — {name}"))
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
                std::env::set_var("SYNCPOLAK_RCLONE", &p);
                break;
            }
        }
    }
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
            // własna konfiguracja rclone w folderze danych aplikacji
            std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));
            locate_rclone(app.handle());
            vss::cleanup_stale(); // migawki pozostawione po awarii
            let runner = Runner::new(dir);
            runner.start_all();
            runner.start_monitor(); // dysk podłączony / wybudzenie / zmiana sieci
            app.manage(runner);
            spawn_notifier(app.handle().clone());

            let show = MenuItem::with_id(app, "show", "Otwórz SyncPolak", true, None::<&str>)?;
            let pause = MenuItem::with_id(
                app,
                "pause",
                "Wstrzymaj / wznów synchronizację",
                true,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "Zakończ", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &pause, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("SyncPolak")
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

            if std::env::args().any(|a| a == "--hidden") {
                if let Some(w) = app.get_webview_window("main") {
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
            rclone_version,
            vss_status,
            get_settings,
            set_settings,
            trash_list,
            trash_restore,
            trash_delete,
            list_dirs,
            export_config,
            import_config,
            restart_elevated,
            set_elevated_autostart
        ])
        .run(tauri::generate_context!())
        .expect("błąd uruchamiania SyncPolak");
}
