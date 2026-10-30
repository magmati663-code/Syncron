use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_syncron"))
}

fn setup() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let t = tempfile::tempdir().unwrap();
    let (l, r, d) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("dane"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::create_dir_all(&d).unwrap();
    let job = serde_json::json!([{
        "id": "abcdef123456", "name": "Zażółć kopia", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}
    }]);
    fs::write(d.join("jobs.json"), job.to_string()).unwrap();
    (t, l, r, d)
}

#[test]
fn list_and_run_with_polish_names() {
    let (_t, l, r, d) = setup();
    fs::write(l.join("gęślą jaźń.txt"), "treść").unwrap();
    let out = bin()
        .args(["--data-dir", d.to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Zażółć kopia"));

    // symulacja nic nie zmienia
    let out = bin()
        .args([
            "--data-dir",
            d.to_str().unwrap(),
            "run",
            "zażółć",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("SYMULACJA"));
    assert!(!r.join("gęślą jaźń.txt").exists());

    // prawdziwy przebieg (nazwa po fragmencie, bez rozróżniania wielkości liter)
    let out = bin()
        .args(["--data-dir", d.to_str().unwrap(), "run", "ZAŻÓŁĆ"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("skopiowano 1"));
    assert_eq!(
        fs::read_to_string(r.join("gęślą jaźń.txt")).unwrap(),
        "treść"
    );
}

#[test]
fn unknown_job_and_command_fail() {
    let (_t, _l, _r, d) = setup();
    let o = bin()
        .args(["--data-dir", d.to_str().unwrap(), "run", "nie ma takiego"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let o = bin()
        .args(["--data-dir", d.to_str().unwrap(), "wat"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn export_import_roundtrip() {
    let (t, _l, _r, d) = setup();
    let f = t.path().join("kopia.json");
    assert!(bin()
        .args([
            "--data-dir",
            d.to_str().unwrap(),
            "export",
            f.to_str().unwrap()
        ])
        .status()
        .unwrap()
        .success());
    let d2 = t.path().join("dane2");
    assert!(bin()
        .args([
            "--data-dir",
            d2.to_str().unwrap(),
            "import",
            f.to_str().unwrap()
        ])
        .status()
        .unwrap()
        .success());
    let out = bin()
        .args(["--data-dir", d2.to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Zażółć kopia"));
}
