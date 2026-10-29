//! `syncron`: synchronizacja z linii poleceń i w tle (tryb usługi) — bez interfejsu graficznego.
//! Używa tego samego folderu danych i tych samych zadań co aplikacja okienkowa.
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};
use syncron_core::engine;
use syncron_core::model::{Job, Schedule};
use syncron_core::names;
use syncron_core::runner::Runner;

const HELP: &str = "Syncron (linia poleceń)

UŻYCIE:
  syncron [--data-dir FOLDER] <polecenie>

POLECENIA:
  list                          pokaż zadania
  run <nazwa|id> [--dry-run] [--force]
                                uruchom zadanie raz (--dry-run: tylko symulacja, --force: pomiń zabezpieczenia)
  report [--job NAZWA] [--kind RODZAJ] [--since 7d|RRRR-MM-DD] [--until 1d|RRRR-MM-DD] [--format csv|json|html] [--out PLIK]
                                raport z historii (bez --out: na standardowe wyjście; rodzaje: copy, delete, move, conflict, error, warning)
  daemon                        działaj w tle i uruchamiaj zadania według harmonogramów (Ctrl+C: koniec)
  export <plik.json>            zapisz zadania do pliku
  import <plik.json>            wczytaj zadania z pliku (polecenia przed/po z pliku są usuwane)
  import-ffs <plik>             zaimportuj ustawienia FreeFileSync (.ffs_gui/.ffs_batch) jako zadania wyłączone
  service install [--boot]      zainstaluj usługę w tle (domyślnie przy logowaniu; --boot: przy starcie systemu, Windows, wymaga administratora)
  service uninstall             usuń usługę
  version                       pokaż wersję

Folder danych: --data-dir, zmienna SYNCRON_DATA albo domyślny folder aplikacji.
Kody wyjścia: 0 = ok, 1 = błąd, 2 = zakończono z konfliktami/błędami plików/zablokowaniem.
";

/// Wartość zmiennej środowiskowej, pomijając puste (puste = nieustawione).
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn default_data_dir() -> PathBuf {
    if let Some(d) = env_nonempty("SYNCRON_DATA").or_else(|| env_nonempty("SYNCPOLAK_DATA")) {
        return d.into();
    }
    let id = "pl.syncron.app";
    #[cfg(windows)]
    {
        PathBuf::from(env_nonempty("APPDATA").unwrap_or_else(|| ".".into())).join(id)
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(env_nonempty("HOME").unwrap_or_else(|| ".".into()))
            .join("Library/Application Support")
            .join(id)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        match env_nonempty("XDG_DATA_HOME") {
            Some(x) => PathBuf::from(x).join(id),
            None => PathBuf::from(env_nonempty("HOME").unwrap_or_else(|| ".".into()))
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

/// Skrót nowych opcji zadania (tylko te różne od domyślnych), np. `własny, treść, ukryte pomijane`.
pub fn options_text(j: &Job) -> String {
    use syncron_core::model::{Compare, Symlinks, Versioning};
    use syncron_core::plan::Mode;
    let mut v: Vec<String> = Vec::new();
    if j.mode == Mode::Custom {
        v.push("tryb własny".into());
    }
    match j.compare {
        Compare::TimeAndSize => {}
        Compare::SizeOnly => v.push("porównanie: rozmiar".into()),
        Compare::Content => v.push("porównanie: zawartość".into()),
    }
    if j.time_tolerance_secs != 2 {
        v.push(format!("tolerancja {} s", j.time_tolerance_secs));
    }
    if j.time_shift_hours > 0 {
        v.push(format!("przesunięcie czasu ≤{} h", j.time_shift_hours));
    }
    if j.symlinks == Symlinks::Follow {
        v.push("dowiązania: podążaj".into());
    }
    if j.ignore_hidden {
        v.push("bez ukrytych".into());
    }
    if let Some(d) = j.min_age_days {
        v.push(format!("wiek ≥{d} d"));
    }
    if let Some(d) = j.max_age_days {
        v.push(format!("wiek ≤{d} d"));
    }
    if !j.detect_moves {
        v.push("bez wykrywania przeniesień".into());
    }
    if j.preserve_permissions {
        v.push("uprawnienia".into());
    }
    if j.versioning == Versioning::Replace {
        v.push("wersje: zastępuj".into());
    }
    if j.run_before
        .as_deref()
        .is_some_and(|c| !c.trim().is_empty())
    {
        v.push("polecenie przed".into());
    }
    if j.run_after.as_deref().is_some_and(|c| !c.trim().is_empty()) {
        v.push("polecenie po".into());
    }
    v.join(", ")
}

fn sched_text(s: &Schedule) -> String {
    match s {
        Schedule::Manual => "ręcznie".into(),
        Schedule::Realtime { .. } => "na bieżąco".into(),
        Schedule::Interval { secs } if *secs < 60 => format!("co {secs} s"),
        Schedule::Interval { secs } => format!("co {} min", secs / 60),
        Schedule::Daily { hour, minute } => format!("codziennie {hour:02}:{minute:02}"),
    }
}

/// Tekst bezpieczny do wypisania w terminalu: znaki sterujące (w tym ESC) zastępowane znakiem „?”.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn find_job<'a>(jobs: &'a [Job], q: &str) -> Result<&'a Job, String> {
    if q.trim().is_empty() {
        return Err("nazwa lub identyfikator zadania nie może być pusty".into());
    }
    if let Some(j) = jobs
        .iter()
        .find(|j| j.id == q || names::same_name(&j.name, q))
    {
        return Ok(j);
    }
    let qk = names::key(q);
    let c: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.id.starts_with(q) || names::key(&j.name).contains(&qk))
        .collect();
    let q = clean(q);
    match c.len() {
        1 => Ok(c[0]),
        0 => Err(format!(
            "nie znaleziono zadania „{q}” (zobacz: syncron list)"
        )),
        _ => Err(format!(
            "„{q}” pasuje do wielu zadań: {}",
            c.iter()
                .map(|j| format!("„{}”", clean(&j.name)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c => o.push(c),
        }
    }
    o
}

pub fn launchd_plist(exe: &str, dir: &str) -> String {
    let (exe, dir) = (xml_escape(exe), xml_escape(dir));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n  <key>Label</key><string>pl.syncron.daemon</string>\n  <key>ProgramArguments</key><array><string>{exe}</string><string>--data-dir</string><string>{dir}</string><string>daemon</string></array>\n  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><true/>\n</dict></plist>\n"
    )
}

/// Argument w cudzysłowie dla ExecStart: `\`, `"`, `%`, `$` i znaki sterujące są escapowane,
/// więc ścieżka nie może wstrzyknąć dyrektyw ani zmiennych systemd.
fn systemd_quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '%' => o.push_str("%%"),
            '$' => o.push_str("$$"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn systemd_unit(exe: &str, dir: &str) -> String {
    format!(
        "[Unit]\nDescription=Syncron\n\n[Service]\nExecStart={} --data-dir {} daemon\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        systemd_quote(exe),
        systemd_quote(dir)
    )
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

/// Czy plik wykonywalny leży w katalogu, którego zwykły użytkownik nie może zmieniać (Program Files, Windows)?
/// Dla konta SYSTEM (`--boot`) exe w zapisywalnym katalogu to droga do podniesienia uprawnień.
fn exe_in_protected_dir(exe: &Path) -> bool {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "SystemRoot",
    ]
    .iter()
    .filter_map(|v| env_nonempty(v))
    .any(|p| exe.starts_with(p))
}

fn absolute(p: &Path) -> Result<PathBuf, String> {
    std::path::absolute(p).map_err(|e| {
        format!(
            "nie można ustalić ścieżki bezwzględnej {}: {e}",
            p.display()
        )
    })
}

fn service(args: &[String], dir: &Path) -> Result<(), String> {
    let sub = args.first().map(String::as_str);
    let install = sub == Some("install");
    if !install && sub != Some("uninstall") {
        return Err("użycie: service install [--boot] | service uninstall".into());
    }
    let allowed: &[&str] = if install { &["--boot"] } else { &[] };
    if let Some(bad) = args[1..].iter().find(|a| !allowed.contains(&a.as_str())) {
        return Err(format!(
            "nieznany argument „{}” (użycie: service install [--boot] | service uninstall)",
            clean(bad)
        ));
    }
    let boot = args.iter().any(|a| a == "--boot");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = absolute(dir)?;
    let (exe_s, dir_s) = (
        exe.to_string_lossy().into_owned(),
        dir.to_string_lossy().into_owned(),
    );
    if install {
        for (what, v) in [("pliku wykonywalnego", &exe_s), ("folderu danych", &dir_s)] {
            if v.chars().any(|c| c.is_control()) {
                return Err(format!(
                    "ścieżka {what} zawiera znaki sterujące — nie można zainstalować usługi"
                ));
            }
        }
    }
    if cfg!(windows) {
        if install {
            if boot && !exe_in_protected_dir(&exe) {
                eprintln!("UWAGA: --boot uruchamia {exe_s} jako SYSTEM, a ten plik leży poza Program Files/Windows — zwykły użytkownik może go podmienić i przejąć konto SYSTEM. Zainstaluj Syncron w Program Files.");
            }
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
        let home = env_nonempty("HOME").ok_or("brak zmiennej HOME")?;
        let plist = PathBuf::from(&home).join("Library/LaunchAgents/pl.syncron.daemon.plist");
        if install {
            std::fs::create_dir_all(plist.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&plist, launchd_plist(&exe_s, &dir_s)).map_err(|e| e.to_string())?;
            run_cmd("launchctl", &["load", "-w", &plist.to_string_lossy()])?;
            println!("Zainstalowano agenta launchd: {}", plist.display());
        } else {
            let unloaded = run_cmd("launchctl", &["unload", "-w", &plist.to_string_lossy()]);
            remove_service_file(&plist, unloaded, "agenta launchd")?;
            println!("Usunięto agenta launchd.");
        }
    } else {
        let home = env_nonempty("HOME").ok_or("brak zmiennej HOME")?;
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
            let disabled = run_cmd("systemctl", &["--user", "disable", "--now", "syncron"]);
            remove_service_file(&unit, disabled, "usługi systemd")?;
            println!("Usunięto usługę systemd.");
        }
    }
    Ok(())
}

/// Usuwa plik usługi; błąd polecenia wyłączającego lub usunięcia pliku jest zgłaszany (nie „Usunięto”).
fn remove_service_file(file: &Path, stopped: Result<(), String>, what: &str) -> Result<(), String> {
    let existed = file.exists();
    let removed = match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("nie można usunąć {}: {e}", file.display())),
    };
    removed?;
    match stopped {
        Ok(()) => Ok(()),
        Err(e) if !existed => Err(format!("{what} nie jest zainstalowane(a) ({e})")),
        Err(e) => Err(format!(
            "plik {what} usunięto, ale polecenie wyłączające zawiodło: {e}"
        )),
    }
}

/// Odrzuca nieznane opcje (każdy argument zaczynający się od `--` spoza listy) — literówka nie może
/// uruchomić prawdziwej synchronizacji.
fn check_opts(rest: &[String], allowed: &[&str]) -> Result<(), String> {
    match rest
        .iter()
        .find(|a| a.starts_with("--") && !allowed.contains(&a.as_str()))
    {
        Some(bad) => Err(format!("nieznana opcja „{}” (syncron help)", clean(bad))),
        None => Ok(()),
    }
}

const COMMANDS: [&str; 10] = [
    "list",
    "report",
    "run",
    "daemon",
    "export",
    "import",
    "import-ffs",
    "service",
    "version",
    "help",
];

/// Czyta plik JSON (UTF-8, z opcjonalnym BOM) do tekstu.
fn read_json_file(f: &str) -> Result<String, String> {
    let bytes = std::fs::read(f).map_err(|e| format!("nie można odczytać pliku {f}: {e}"))?;
    let text =
        String::from_utf8(bytes).map_err(|e| format!("plik {f} nie jest poprawnym UTF-8: {e}"))?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_string())
}

fn real_main() -> Result<ExitCode, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = default_data_dir();
    if args.iter().filter(|a| *a == "--data-dir").count() > 1 {
        return Err("opcja --data-dir podana więcej niż raz".into());
    }
    if let Some(i) = args.iter().position(|a| a == "--data-dir") {
        if i + 1 >= args.len() {
            return Err("--data-dir wymaga wartości".into());
        }
        let v = args.remove(i + 1);
        args.remove(i);
        if v.trim().is_empty() || v.starts_with("--") || COMMANDS.contains(&v.as_str()) {
            return Err(format!(
                "--data-dir wymaga folderu, a otrzymano „{}” (czy zabrakło wartości? Folder o takiej nazwie podaj jako ./{})",
                clean(&v),
                clean(&v)
            ));
        }
        dir = PathBuf::from(v);
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
            check_opts(rest, &[])?;
            let jobs = runner.load_jobs_checked()?;
            if jobs.is_empty() {
                println!("Brak zadań (folder danych: {}).", dir.display());
            }
            for j in jobs {
                let opts = options_text(&j);
                println!(
                    "{}  {}  {} {:?} {}  [{}] {}{}",
                    clean(&j.id.chars().take(8).collect::<String>()),
                    clean(&j.name),
                    clean(&j.left),
                    j.mode,
                    clean(&j.right),
                    sched_text(&j.schedule),
                    if j.enabled { "" } else { "(wyłączone)" },
                    if opts.is_empty() {
                        String::new()
                    } else {
                        format!("  <{}>", clean(&opts))
                    }
                );
            }
        }
        "run" => {
            check_opts(rest, &["--dry-run", "--force"])?;
            let q = rest
                .iter()
                .find(|a| !a.starts_with("--"))
                .ok_or("podaj nazwę lub identyfikator zadania")?;
            let (dry, force) = (
                rest.iter().any(|a| a == "--dry-run"),
                rest.iter().any(|a| a == "--force"),
            );
            let jobs = runner.load_jobs_checked()?;
            let job = find_job(&jobs, q)?;
            let progress = |d: usize, t: usize, cur: &str| eprint!("\r[{d}/{t}] {cur:<70}");
            let rep = engine::run_job_with(job, &dir, dry, force, Some(&progress))
                .map_err(|e| e.to_string())?;
            eprintln!();
            println!(
                "Zadanie „{}”: przeskanowano {} (lewa) i {} (prawa).",
                clean(&job.name),
                rep.scanned_left,
                rep.scanned_right
            );
            if dry {
                println!("SYMULACJA: planowanych działań: {}", rep.planned.len());
                for (k, a) in &rep.planned {
                    println!("  {a:?}  {}", clean(k));
                }
            } else {
                println!("skopiowano {}, usunięto {}, przeniesiono {}, konflikty {}, zajęte {}, błędy {}", rep.copied, rep.deleted, rep.moved, rep.conflicts.len(), rep.busy.len(), rep.errors.len());
            }
            if let Some(b) = &rep.blocked {
                println!(
                    "ZATRZYMANO przez zabezpieczenie: {} (użyj --force, aby pominąć)",
                    clean(b)
                );
            }
            for e in rep.errors.iter().chain(rep.busy.iter()) {
                println!("  ! {}", clean(e));
            }
            for w in &rep.warnings {
                println!("  * {}", clean(w));
            }
            if let Some(n) = &rep.vss_note {
                println!("  VSS: {}", clean(n));
            }
            if rep.blocked.is_some()
                || !rep.errors.is_empty()
                || !rep.conflicts.is_empty()
                || !rep.busy.is_empty()
            {
                return Ok(ExitCode::from(2));
            }
        }
        "report" => {
            const OPTS: [&str; 6] = ["--job", "--kind", "--since", "--until", "--format", "--out"];
            let mut vals: std::collections::HashMap<&str, String> = Default::default();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                let key = OPTS
                    .iter()
                    .find(|o| **o == a.as_str())
                    .ok_or_else(|| format!("nieznana opcja „{}” (syncron help)", clean(a)))?;
                let v = it
                    .next()
                    .ok_or_else(|| format!("opcja {key} wymaga wartości"))?;
                vals.insert(key, v.clone());
            }
            let now = chrono::Local::now().timestamp();
            let jobs = runner.load_jobs_checked().unwrap_or_default();
            let mut filter = syncron_core::report::Filter::default();
            if let Some(q) = vals.get("--job") {
                // zadanie mogło zostać usunięte, a jego historia nadal istnieć: akceptujemy też surowy identyfikator
                filter.job = Some(match find_job(&jobs, q) {
                    Ok(j) => j.id.clone(),
                    Err(e) if jobs.iter().any(|j| j.id == *q) || e.contains("wielu") => {
                        return Err(e)
                    }
                    Err(_) => q.clone(),
                });
            }
            filter.kind = vals.get("--kind").cloned();
            if let Some(v) = vals.get("--since") {
                filter.since = Some(syncron_core::report::parse_since(v, now)?);
            }
            if let Some(v) = vals.get("--until") {
                let t = syncron_core::report::parse_since(v, now)?;
                // data bez godziny oznacza koniec tego dnia
                filter.until = Some(if v.contains('-') { t + 86_399 } else { t });
            }
            let format = syncron_core::report::Format::parse(
                vals.get("--format").map(String::as_str).unwrap_or("csv"),
            )?;
            let names: std::collections::HashMap<String, String> = jobs
                .iter()
                .map(|j| (j.id.clone(), j.name.clone()))
                .collect();
            match vals.get("--out") {
                Some(out) => {
                    let n =
                        syncron_core::report::export(&dir, format, &filter, &names, Path::new(out))
                            .map_err(|e| e.to_string())?;
                    println!("Zapisano raport: {n} wpisów → {}", clean(out));
                }
                None => {
                    let rows =
                        syncron_core::report::query(&dir, &filter, syncron_core::report::MAX_ROWS)
                            .map_err(|e| e.to_string())?;
                    print!(
                        "{}",
                        syncron_core::report::render(format, &rows, &filter, &names)
                    );
                }
            }
        }
        "daemon" => {
            check_opts(rest, &[])?;
            let _ = log::set_logger(&StdoutLogger);
            log::set_max_level(log::LevelFilter::Info);
            log::info!(
                "Syncron {} (daemon), folder danych: {}",
                env!("CARGO_PKG_VERSION"),
                dir.display()
            );
            let path = runner.jobs_path();
            // czas modyfikacji zapisujemy PRZED odczytem: zmiana w trakcie odczytu wywoła kolejne przeładowanie
            let mut seen = mtime(&path);
            match runner.load_jobs_checked() {
                Ok(jobs) => jobs.into_iter().for_each(|j| runner.start(j)),
                Err(e) => log::error!("{e}"),
            }
            runner.start_monitor();
            loop {
                std::thread::sleep(Duration::from_secs(15));
                let now = mtime(&path);
                if now != seen {
                    seen = now;
                    match runner.load_jobs_checked() {
                        Ok(jobs) => {
                            // zadania zmieniono (np. w aplikacji okienkowej): przeładuj
                            log::info!("zmieniono konfigurację zadań — przeładowuję");
                            runner.stop_all();
                            jobs.into_iter().for_each(|j| runner.start(j));
                        }
                        Err(e) => log::error!(
                            "{e} Pozostawiam uruchomione zadania bez zmian do czasu naprawy pliku."
                        ),
                    }
                }
            }
        }
        "export" => {
            check_opts(rest, &[])?;
            let f = rest.first().ok_or("podaj plik docelowy")?;
            let jobs = runner.load_jobs_checked()?;
            std::fs::write(
                f,
                serde_json::to_string_pretty(&jobs).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("nie można zapisać pliku {f}: {e}"))?;
            println!("Wyeksportowano zadań: {}", jobs.len());
        }
        "import" => {
            check_opts(rest, &[])?;
            let f = rest.first().ok_or("podaj plik źródłowy")?;
            let mut incoming: Vec<Job> = serde_json::from_str(&read_json_file(f)?)
                .map_err(|e| format!("to nie jest plik konfiguracji Syncron: {e}"))?;
            // polecenia przed/po to wykonanie dowolnego kodu: z importu nigdy ich nie przejmujemy
            let had_hooks = incoming.iter().any(|j| {
                j.run_before.is_some() || j.run_after.is_some() || j.webhook_url.is_some()
            });
            for j in &mut incoming {
                j.run_before = None;
                j.run_after = None;
                j.webhook_url = None;
            }
            if had_hooks {
                eprintln!("Uwaga: usunięto polecenia przed/po i adresy webhooków z importowanych zadań (ustaw je ręcznie w aplikacji).");
            }
            for (n, j) in incoming.iter().enumerate() {
                for (what, v) in [
                    ("identyfikator", &j.id),
                    ("lewa lokalizacja", &j.left),
                    ("prawa lokalizacja", &j.right),
                ] {
                    if v.trim().is_empty() {
                        return Err(format!("zadanie nr {} w pliku {f}: pusty {what}", n + 1));
                    }
                }
            }
            for (n, j) in incoming.iter().enumerate() {
                j.validate()
                    .map_err(|e| format!("zadanie nr {} w pliku {f}: {e}", n + 1))?;
            }
            let mut ids: Vec<&str> = incoming.iter().map(|j| j.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            let imported = ids.len();
            runner.update_jobs(|mut jobs| {
                for j in &incoming {
                    match jobs.iter().position(|x| x.id == j.id) {
                        Some(i) => jobs[i] = j.clone(),
                        None => jobs.push(j.clone()),
                    }
                }
                Ok(jobs)
            })?;
            println!("Zaimportowano zadań: {imported}");
        }
        "import-ffs" => {
            check_opts(rest, &[])?;
            let f = rest.first().ok_or("podaj plik .ffs_gui lub .ffs_batch")?;
            let incoming = syncron_core::ffs::read_ffs_file(Path::new(f))?;
            let count = incoming.len();
            runner.update_jobs(|mut jobs| {
                jobs.extend(incoming.iter().cloned());
                Ok(jobs)
            })?;
            println!("Zaimportowano zadań z FreeFileSync: {count} (wyłączone — sprawdź i włącz w aplikacji)");
            for j in &incoming {
                println!("  {}  {}", clean(&j.id), clean(&j.name));
            }
        }
        "service" => service(rest, &dir)?,
        other => {
            return Err(format!(
                "nieznane polecenie „{}” (syncron help)",
                clean(other)
            ))
        }
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
