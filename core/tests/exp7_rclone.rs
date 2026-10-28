//! Ekspert 7: integracja z rclone (limit czasu, stdin, sekrety w środowisku, listing lsjson, nazwy kont).
//! Zmienne środowiskowe są globalne dla procesu, więc testy trzymają wspólny Mutex.
#![cfg(unix)]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use syncron_core::location::{
    run_rclone, run_rclone_with, set_remote_keys, valid_param_key, valid_remote_name, Location,
};
use syncron_core::rules::RuleSet;

static LOCK: Mutex<()> = Mutex::new(());

struct Env {
    _g: MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("SYNCRON_RCLONE_TIMEOUT");
        Env {
            _g: g,
            dir: tempfile::tempdir().unwrap(),
        }
    }
    /// Skrypt powłoki jako „rclone”.
    fn script(&self, body: &str) -> PathBuf {
        let p = self.dir.path().join("rclone.sh");
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("SYNCRON_RCLONE", &p);
        p
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        std::env::remove_var("SYNCRON_RCLONE_TIMEOUT");
        std::env::remove_var("SYNCRON_RCLONE");
        std::env::remove_var("EXP7_SECRET");
    }
}

fn list(json: &str) -> Result<Vec<(String, u64, i64)>, String> {
    let env = Env::new();
    let f = env.dir.path().join("out.json");
    fs::write(&f, json).unwrap();
    env.script(&format!("cat '{}'", f.display()));
    let l = Location::parse("fake:").list(&RuleSet::default())?;
    Ok(l.files
        .values()
        .map(|e| (e.real.clone(), e.meta.size, e.meta.mtime))
        .collect())
}

#[test]
fn timeout_kills_hanging_rclone() {
    let env = Env::new();
    env.script("sleep 30");
    std::env::set_var("SYNCRON_RCLONE_TIMEOUT", "1");
    let t = Instant::now();
    let e = run_rclone(&["lsjson", "fake:"]).unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(e.contains("limit czasu"), "{e}");
}

#[test]
fn timeout_applies_to_slow_pipe_holder() {
    // proces potomny trzyma rury po zakończeniu rodzica: nie blokuje po sukcesie dłużej niż limit odczytu
    let env = Env::new();
    env.script("echo ok");
    assert_eq!(run_rclone(&["version"]).unwrap().trim(), "ok");
}

#[test]
fn stdin_is_null() {
    let env = Env::new();
    // `cat` czyta stdin: przy zamkniętym/pustym stdin kończy się od razu
    env.script("cat; echo koniec");
    std::env::set_var("SYNCRON_RCLONE_TIMEOUT", "5");
    let t = Instant::now();
    assert_eq!(run_rclone(&["version"]).unwrap().trim(), "koniec");
    assert!(t.elapsed() < Duration::from_secs(4));
}

#[test]
fn secrets_go_through_environment_and_stdin() {
    let env = Env::new();
    env.script(r#"if [ "$1" = obscure ]; then cat; else echo "$EXP7_SECRET"; fi"#);
    let out = run_rclone_with(
        &["version"],
        &[("EXP7_SECRET".to_string(), "tajne".to_string())],
        None,
    )
    .unwrap();
    assert_eq!(out.trim(), "tajne");
    assert_eq!(
        syncron_core::location::rclone_obscure("hasło")
            .unwrap()
            .as_str(),
        "hasło"
    );
}

#[test]
fn network_ops_get_timeouts_in_environment_not_in_argv() {
    let env = Env::new();
    env.script(r#"echo "$RCLONE_CONTIMEOUT $RCLONE_TIMEOUT $#""#);
    assert_eq!(run_rclone(&["lsjson", "x:"]).unwrap().trim(), "30s 120s 2");
}

#[test]
fn empty_stderr_gives_exit_code_message() {
    let env = Env::new();
    env.script("exit 7");
    let e = run_rclone(&["lsjson", "x:"]).unwrap_err();
    assert!(e.contains("kodem 7"), "{e}");
    env.script("echo cos poszlo nie tak; exit 3");
    let e = run_rclone(&["lsjson", "x:"]).unwrap_err();
    assert!(e.contains("kodem 3") && e.contains("cos poszlo"), "{e}");
}

#[test]
fn missing_rclone_has_readable_message() {
    let _env = Env::new();
    std::env::set_var("SYNCRON_RCLONE", "/nie/ma/rclone");
    let e = run_rclone(&["version"]).unwrap_err();
    assert!(e.contains("nie znaleziono programu"), "{e}");
}

#[test]
fn lsjson_size_out_of_i64_is_listing_error() {
    let e = list(
        r#"[{"Path":"a.txt","Size":18446744073709551615,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#,
    )
    .unwrap_err();
    assert!(e.contains("a.txt") && e.contains("zakresem"), "{e}");
}

#[test]
fn lsjson_unknown_size_policy() {
    // prawdziwy dokument Google (typ MIME) jest pomijany
    let v = list(
        r#"[{"Path":"doc","Size":-1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false,"MimeType":"application/vnd.google-apps.document"},
            {"Path":"ok.txt","Size":2,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#,
    )
    .unwrap();
    assert_eq!(v.len(), 1);
    // zwykły plik bez rozmiaru to błąd listingu (inaczej wyglądałby na usunięty)
    let e = list(
        r#"[{"Path":"x.txt","Size":-1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false,"MimeType":"text/plain"}]"#,
    )
    .unwrap_err();
    assert!(e.contains("x.txt"), "{e}");
}

#[test]
fn lsjson_missing_modtime_is_constant() {
    let j = r#"[{"Path":"a.txt","Size":1,"IsDir":false},{"Path":"b.txt","Size":1,"ModTime":"zły","IsDir":false}]"#;
    let a = list(j).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let b = list(j).unwrap();
    assert_eq!(a, b);
    assert!(a.iter().all(|(_, _, m)| *m == 0));
}

#[test]
fn parse_nfd_and_limits() {
    use unicode_normalization::UnicodeNormalization;
    let nfd: String = "Dysk Zażółć:folder".nfd().collect();
    match Location::parse(&nfd) {
        Location::Rclone(s) => assert_eq!(s, "Dysk Zażółć:folder"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(Location::parse("C:\\x"), Location::Local(_)));
    assert!(matches!(Location::parse("ż:\\x"), Location::Local(_)));
    assert!(matches!(Location::parse("ab:x"), Location::Rclone(_)));
    // składnia łączenia rclone (`remote,opcja=x:ścieżka`) nie jest rozpoznawana: to folder lokalny (celowo)
    assert!(matches!(
        Location::parse("gdrive,drive_id=x:folder"),
        Location::Local(_)
    ));
}

#[test]
fn account_validation() {
    for ok in ["gdrive", "a1", "my-remote_2"] {
        assert!(valid_remote_name(ok), "{ok}");
    }
    for bad in ["", "a", "-x", "_x", "a b", "zażółć", "a:b", "--flag", "a/b"] {
        assert!(!valid_remote_name(bad), "{bad}");
    }
    assert!(valid_param_key("client_secret"));
    for bad in ["", "--password-command", "a b", "a=b", "-x"] {
        assert!(!valid_param_key(bad), "{bad}");
    }
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

#[test]
fn set_remote_keys_edits_only_its_section() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("rclone.conf");
    fs::write(
        &c,
        "[a]\ntype = drive\n\n[dav]\ntype = webdav\npass = stare\nurl = u\n\n[z]\ntype = sftp\n",
    )
    .unwrap();
    set_remote_keys(&c, "dav", &[("pass".into(), "nowe".into())]).unwrap();
    assert_eq!(
        read(&c),
        "[a]\ntype = drive\n\n[dav]\ntype = webdav\nurl = u\npass = nowe\n\n[z]\ntype = sftp\n"
    );
    set_remote_keys(
        &c,
        "nowy",
        &[("type".into(), "sftp".into()), ("pass".into(), "p".into())],
    )
    .unwrap();
    assert!(read(&c).ends_with("[z]\ntype = sftp\n\n[nowy]\ntype = sftp\npass = p\n"));
    assert_eq!(
        fs::metadata(&c).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // wartość z nową linią i zły klucz są odrzucane
    assert!(set_remote_keys(&c, "dav", &[("pass".into(), "a\nb = c".into())]).is_err());
    assert!(set_remote_keys(&c, "dav", &[("--x".into(), "a".into())]).is_err());
}

#[test]
fn set_remote_keys_new_file_is_private() {
    let d = tempfile::tempdir().unwrap();
    let c = d.path().join("sub/rclone.conf");
    set_remote_keys(&c, "dav", &[("type".into(), "webdav".into())]).unwrap();
    assert_eq!(
        fs::metadata(&c).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
