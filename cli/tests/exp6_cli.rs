//! Ekspert 6: dodatkowe testy CLI (parsowanie argumentów, błędy usług).
use std::fs;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_syncron"))
}
fn err_s(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn data_dir_with_command_name_as_value_is_error() {
    let o = bin().args(["--data-dir", "list"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        err_s(&o).contains("--data-dir wymaga folderu"),
        "{}",
        err_s(&o)
    );
}

#[test]
fn data_dir_twice_is_error() {
    let o = bin()
        .args(["--data-dir", "a", "--data-dir", "b", "list"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("więcej niż raz"));
}

#[test]
fn unknown_flags_rejected_for_all_commands() {
    let t = tempfile::tempdir().unwrap();
    for a in [
        vec!["list", "--x"],
        vec!["export", "--x"],
        vec!["import", "--x"],
        vec!["run", "a", "--dry_run"],
        vec!["service", "uninstall", "--x"],
    ] {
        let o = bin()
            .args(["--data-dir", t.path().to_str().unwrap()])
            .args(&a)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(1), "{a:?}");
    }
}

#[test]
fn import_rejects_empty_id_and_does_not_write() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("in.json");
    fs::write(
        &f,
        r#"[{"id":"","name":"x","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]"#,
    )
    .unwrap();
    let d = t.path().join("d");
    let o = bin()
        .args([
            "--data-dir",
            d.to_str().unwrap(),
            "import",
            f.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(!d.join("jobs.json").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn service_uninstall_when_not_installed_reports_error() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let fake = t.path().join("bin");
    fs::create_dir_all(&fake).unwrap();
    fs::write(
        fake.join("systemctl"),
        "#!/bin/sh\necho nie ma >&2\nexit 5\n",
    )
    .unwrap();
    fs::set_permissions(fake.join("systemctl"), fs::Permissions::from_mode(0o755)).unwrap();
    let o = bin()
        .args(["service", "uninstall"])
        .env("HOME", t.path())
        .env("PATH", &fake)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&o.stdout).contains("Usunięto"));
}
