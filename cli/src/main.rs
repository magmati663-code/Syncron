//! `syncron`: synchronizacja z linii poleceń i w tle (tryb usługi) — bez interfejsu graficznego.
//! Używa tego samego folderu danych i tych samych zadań co aplikacja okienkowa.
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};
use syncron_core::engine;
use syncron_core::model::{Job, Schedule};
use syncron_core::runner::Runner;

const HELP: &str = "Syncron (linia poleceń)

UŻYCIE:
  syncron [--data-dir FOLDER] <polecenie>

POLECENIA:
  list                          pokaż zadania
  run <nazwa|id> [--dry-run] [--force]
                                uruchom zadanie raz (--dry-run: tylko symulacja, --force: pomiń zabezpieczenia)
  daemon                        działaj w tle i uruchamiaj zadania według harmonogramów (Ctrl+C: koniec)
  export <plik.json>            zapisz zadania do pliku
  import <plik.json>            wczytaj zadania z pliku
  service install [--boot]      zainstaluj usługę w tle (domyślnie przy logowaniu; --boot: przy starcie systemu, Windows, wymaga administratora)
  service uninstall             usuń usługę
  version                       pokaż wersję

Folder danych: --data-dir, zmienna SYNCRON_DATA albo domyślny folder aplikacji.
Kody wyjścia: 0 = ok, 1 = błąd, 2 = zakończono z konfliktami/błędami plików/zablokowaniem.
";

fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SYNCRON_DATA").or_else(|_| std::env::var("SYNCPOLAK_DATA")) {
        return d.into();
    }
    let id = "pl.syncron.app";
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join(id)
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
            .join("Library/Application Support")
            .join(id)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        match std::env::var("XDG_DATA_HOME") {
            Ok(x) => PathBuf::from(x).join(id),
            Err(_) => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
                .join(".local/share")
                .join(id),
        }
    }
}

struct StdoutLogger;
impl log::Log for StdoutLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            println!(
                "[{}] {:<5} {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                r.level(),
                r.args()
            );
        }
    }
    fn flush(&self) {}
}

fn sched_text(s: &Schedule) -> String {
    match s {
        Schedule::Manual => "ręcznie".into(),
        Schedule::Realtime { .. } => "na bieżąco".into(),
        Schedule::Interval { secs } => format!("co {} min", secs / 60),
        Schedule::Daily { hour, minute } => format!("codziennie {hour:02}:{minute:02}"),
    }
}

fn find_job<'a>(jobs: &'a [Job], q: &str) -> Result<&'a Job, String> {
    if let Some(j) = jobs.iter().find(|j| j.id == q || j.name == q) {
        return Ok(j);
    }
    let ql = q.to_lowercase();
    let c: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.id.starts_with(q) || j.name.to_lowercase().contains(&ql))
        .collect();
    match c.len() {
        1 => Ok(c[0]),
        0 => Err(format!(
            "nie znaleziono zadania „{q}” (zobacz: syncron list)"
        )),
        _ => Err(format!(
            "„{q}” pasuje do wielu zadań: {}",
            c.iter()
                .map(|j| format!("„{}”", j.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub fn launchd_plist(exe: &str, dir: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>pl.syncron.daemon</string>\n  <key>ProgramArguments</key><array><string>{exe}</string><string>--data-dir</string><string>{dir}</string><string>daemon</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n</dict></plist>\n"
    )
}

pub fn systemd_unit(exe: &str, dir: &str) -> String {
    format!("[Unit]\nDescription=Syncron\n\n[Service]\nExecStart=\"{exe}\" --data-dir \"{dir}\" daemon\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n")
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn service(args: &[String], dir: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (exe_s, dir_s) = (
        exe.to_string_lossy().into_owned(),
        dir.to_string_lossy().into_owned(),
    );
    let install = args.first().map(String::as_str) == Some("install");
    if !install && args.first().map(String::as_str) != Some("uninstall") {
        return Err("użycie: service install [--boot] | service uninstall".into());
    }
    let boot = args.iter().any(|a| a == "--boot");
    if cfg!(windows) {
        if install {
            let tr = format!("\"{exe_s}\" --data-dir \"{dir_s}\" daemon");
            let mut a = vec!["/Create", "/TN", "Syncron Daemon", "/TR", &tr, "/F"];
            a.extend(if boot {
                vec!["/SC", "ONSTART", "/RU", "SYSTEM", "/RL", "HIGHEST"]
            } else {
                vec!["/SC", "ONLOGON"]
            });
            run_cmd("schtasks", &a)?;
            println!(
                "Zainstalowano zadanie „Syncron Daemon” ({}).",
                if boot {
                    "przy starcie systemu, konto SYSTEM"
                } else {
                    "przy logowaniu"
                }
            );
        } else {
            run_cmd("schtasks", &["/Delete", "/TN", "Syncron Daemon", "/F"])?;
            println!("Usunięto zadanie „Syncron Daemon”.");
        }
    } else if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        let plist = PathBuf::from(&home).join("Library/LaunchAgents/pl.syncron.daemon.plist");
        if install {
            std::fs::create_dir_all(plist.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&plist, launchd_plist(&exe_s, &dir_s)).map_err(|e| e.to_string())?;
            run_cmd("launchctl", &["load", "-w", &plist.to_string_lossy()])?;
            println!("Zainstalowano agenta launchd: {}", plist.display());
        } else {
            let _ = run_cmd("launchctl", &["unload", "-w", &plist.to_string_lossy()]);
            let _ = std::fs::remove_file(&plist);
            println!("Usunięto agenta launchd.");
        }
    } else {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        let unit = PathBuf::from(&home).join(".config/systemd/user/syncron.service");
        if install {
            std::fs::create_dir_all(unit.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&unit, systemd_unit(&exe_s, &dir_s)).map_err(|e| e.to_string())?;
            run_cmd("systemctl", &["--user", "daemon-reload"])?;
            run_cmd("systemctl", &["--user", "enable", "--now", "syncron"])?;
            println!(
                "Zainstalowano usługę systemd (użytkownika): {}",
                unit.display()
            );
        } else {
            let _ = run_cmd("systemctl", &["--user", "disable", "--now", "syncron"]);
            let _ = std::fs::remove_file(&unit);
            println!("Usunięto usługę systemd.");
        }
    }
    Ok(())
}

fn real_main() -> Result<ExitCode, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = default_data_dir();
    if let Some(i) = args.iter().position(|a| a == "--data-dir") {
        if i + 1 >= args.len() {
            return Err("--data-dir wymaga wartości".into());
        }
        dir = PathBuf::from(args.remove(i + 1));
        args.remove(i);
    }
    syncron_core::legacy::migrate_data_dir(&dir);
    // jak w aplikacji okienkowej: własna konfiguracja rclone w folderze danych
    std::env::set_var("RCLONE_CONFIG", dir.join("rclone.conf"));
    let runner = Runner::new(dir.clone());
    let cmd = args
        .first()
        .map(String::as_str)
        .unwrap_or("help")
        .to_string();
    let rest = &args[args.len().min(1)..];

    match cmd.as_str() {
        "help" | "--help" | "-h" => print!("{HELP}"),
        "version" | "--version" | "-V" => println!("syncron {}", env!("CARGO_PKG_VERSION")),
        "list" => {
            let jobs = runner.load_jobs();
            if jobs.is_empty() {
                println!("Brak zadań (folder danych: {}).", dir.display());
            }
            for j in jobs {
                println!(
                    "{}  {}  {} {:?} {}  [{}] {}",
                    &j.id[..j.id.len().min(8)],
                    j.name,
                    j.left,
                    j.mode,
                    j.right,
                    sched_text(&j.schedule),
                    if j.enabled { "" } else { "(wyłączone)" }
                );
            }
        }
        "run" => {
            let q = rest
                .iter()
                .find(|a| !a.starts_with("--"))
                .ok_or("podaj nazwę lub identyfikator zadania")?;
            let (dry, force) = (
                rest.iter().any(|a| a == "--dry-run"),
                rest.iter().any(|a| a == "--force"),
            );
            let jobs = runner.load_jobs();
            let job = find_job(&jobs, q)?;
            let progress = |d: usize, t: usize, cur: &str| eprint!("\r[{d}/{t}] {cur:<70}");
            let rep = engine::run_job_with(job, &dir, dry, force, Some(&progress))
                .map_err(|e| e.to_string())?;
            eprintln!();
            println!(
                "Zadanie „{}”: przeskanowano {} (lewa) i {} (prawa).",
                job.name, rep.scanned_left, rep.scanned_right
            );
            if dry {
                println!("SYMULACJA: planowanych działań: {}", rep.planned.len());
                for (k, a) in &rep.planned {
                    println!("  {a:?}  {k}");
                }
            } else {
                println!("skopiowano {}, usunięto {}, przeniesiono {}, konflikty {}, zajęte {}, błędy {}", rep.copied, rep.deleted, rep.moved, rep.conflicts.len(), rep.busy.len(), rep.errors.len());
            }
            if let Some(b) = &rep.blocked {
                println!("ZATRZYMANO przez zabezpieczenie: {b} (użyj --force, aby pominąć)");
            }
            for e in rep.errors.iter().chain(rep.busy.iter()) {
                println!("  ! {e}");
            }
            if let Some(n) = &rep.vss_note {
                println!("  VSS: {n}");
            }
            if rep.blocked.is_some()
                || !rep.errors.is_empty()
                || !rep.conflicts.is_empty()
                || !rep.busy.is_empty()
            {
                return Ok(ExitCode::from(2));
            }
        }
        "daemon" => {
            let _ = log::set_logger(&StdoutLogger);
            log::set_max_level(log::LevelFilter::Info);
            log::info!(
                "Syncron {} (daemon), folder danych: {}",
                env!("CARGO_PKG_VERSION"),
                dir.display()
            );
            runner.start_all();
            runner.start_monitor();
            let path = runner.jobs_path();
            let mut seen = mtime(&path);
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let now = mtime(&path);
                if now != seen {
                    // zadania zmieniono (np. w aplikacji okienkowej): przeładuj
                    seen = now;
                    log::info!("zmieniono konfigurację zadań — przeładowuję");
                    runner.stop_all();
                    runner.start_all();
                }
            }
        }
        "export" => {
            let f = rest.first().ok_or("podaj plik docelowy")?;
            let jobs = runner.load_jobs();
            std::fs::write(
                f,
                serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            println!("Wyeksportowano zadań: {}", jobs.len());
        }
        "import" => {
            let f = rest.first().ok_or("podaj plik źródłowy")?;
            let incoming: Vec<Job> =
                serde_json::from_str(&std::fs::read_to_string(f).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("to nie jest plik konfiguracji Syncron: {e}"))?;
            let mut jobs = runner.load_jobs();
            for j in &incoming {
                match jobs.iter().position(|x| x.id == j.id) {
                    Some(i) => jobs[i] = j.clone(),
                    None => jobs.push(j.clone()),
                }
            }
            runner.save_jobs(&jobs).map_err(|e| e.to_string())?;
            println!("Zaimportowano zadań: {}", incoming.len());
        }
        "service" => service(rest, &dir)?,
        other => return Err(format!("nieznane polecenie „{other}” (syncron help)")),
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match real_main() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("błąd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_files() {
        let p = launchd_plist("/usr/local/bin/syncron", "/Users/x/dane");
        assert!(p.contains("<string>daemon</string>") && p.contains("RunAtLoad"));
        let u = systemd_unit("/opt/syncron", "/home/x/dane");
        assert!(u.contains("ExecStart=\"/opt/syncron\" --data-dir \"/home/x/dane\" daemon"));
    }
}
