//! Polecenia uruchamiane przed i po synchronizacji (`run_before` / `run_after`).
//!
//! Polecenie podaje użytkownik ręcznie (nigdy nie pochodzi z importu). Uruchamiamy je przez `sh -c` (Unix) albo
//! `cmd /C` (Windows) — bez powłoki logowania, ze standardowym wejściem zamkniętym, z limitem czasu (po jego
//! przekroczeniu zabijamy cały proces wraz z potomnymi) i zmiennymi środowiskowymi `SYNCRON_*`.
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Wynik uruchomienia polecenia.
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// Kod wyjścia (None: zabity po przekroczeniu limitu albo przez sygnał).
    pub code: Option<i32>,
    pub timed_out: bool,
    /// Błąd uruchomienia (np. brak powłoki).
    pub spawn_error: Option<String>,
    /// Ostatnie bajty wyjścia (stdout + stderr), przycięte i oczyszczone ze znaków sterujących.
    pub output: String,
}

impl HookResult {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out && self.spawn_error.is_none()
    }

    /// Opis po polsku do komunikatów (`what` = „przed synchronizacją” / „po synchronizacji”).
    pub fn describe(&self, what: &str) -> String {
        let tail = if self.output.is_empty() {
            String::new()
        } else {
            format!(" Wyjście: {}", self.output)
        };
        if let Some(e) = &self.spawn_error {
            format!("Polecenie {what} nie uruchomiło się: {e}")
        } else if self.timed_out {
            format!("Polecenie {what} przekroczyło limit czasu i zostało przerwane.{tail}")
        } else {
            match self.code {
                Some(0) => format!("Polecenie {what} zakończone powodzeniem.{tail}"),
                Some(c) => format!("Polecenie {what} zakończyło się błędem (kod {c}).{tail}"),
                None => format!("Polecenie {what} zostało przerwane sygnałem.{tail}"),
            }
        }
    }
}

const MAX_OUT: usize = 2000;

fn sanitize(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(MAX_OUT);
    String::from_utf8_lossy(&bytes[start..])
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else if c.is_control() {
                '?'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // proces jest liderem własnej grupy (process_group(0)): zabijamy całą grupę
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Uruchamia polecenie z limitem czasu `timeout_secs` i dodatkowymi zmiennymi środowiskowymi.
pub fn run_hook(command: &str, timeout_secs: u32, env: &[(&str, String)]) -> HookResult {
    let mut res = HookResult::default();
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        use std::os::unix::process::CommandExt;
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c.process_group(0);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            res.spawn_error = Some(e.to_string());
            return res;
        }
    };
    // wyjście czytane w osobnych wątkach (pełny potok nie może zawiesić polecenia)
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let pipes: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    for mut p in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut keep: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = p.read(&mut buf) {
                if n == 0 {
                    break;
                }
                keep.extend_from_slice(&buf[..n]);
                if keep.len() > 4 * MAX_OUT {
                    let cut = keep.len() - MAX_OUT;
                    keep.drain(..cut);
                }
            }
            let _ = tx.send(keep);
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1) as u64);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                res.code = st.code();
                break;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    res.timed_out = true;
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => {
                res.spawn_error = Some(e.to_string());
                kill_tree(&mut child);
                break;
            }
        }
    }
    // zbieramy wyjście z krótkim limitem (potomek mógł odziedziczyć potok i nadal go trzymać)
    let mut all: Vec<u8> = Vec::new();
    let grace = Instant::now() + Duration::from_millis(500);
    while let Some(left) = grace.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(v) => all.extend(v),
            Err(_) => break,
        }
    }
    res.output = sanitize(&all);
    res
}
