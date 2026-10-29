//! Syncron 0.8 (zespół E): CLI `syncron report` oraz czyszczenie `webhook_url` przy imporcie.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn bin(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_syncron"));
    c.args(["--data-dir", data.to_str().unwrap()]);
    c
}
fn out_s(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn err_s(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Folder danych z zadaniem „Zdjęcia” (id abcdef12) i historią: 2 wpisy świeże, 1 sprzed 20 dni.
fn setup() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("jobs.json"),
        r#"[{"id":"abcdef12","name":"Zdjęcia","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]"#,
    )
    .unwrap();
    let c = syncron_core::engine::open_db(t.path()).unwrap();
    let n = now();
    for (ts, job, kind, path, detail) in [
        (n - 3600, "abcdef12", "kopiowanie →", "=cmd.txt", "2048 B"),
        (n - 3600, "abcdef12", "błąd", "<b>x</b>.txt", "brak dostępu"),
        (
            n - 20 * 86_400,
            "abcdef12",
            "kopiowanie →",
            "stary.txt",
            "10 B",
        ),
        (n - 3600, "inne", "usunięcie", "obce.txt", ""),
    ] {
        let q = |x: &str| x.replace('\'', "''");
        c.execute_batch(&format!(
            "INSERT INTO history(ts,job,kind,path,detail) VALUES({ts},'{}','{}','{}','{}')",
            q(job),
            q(kind),
            q(path),
            q(detail)
        ))
        .unwrap();
    }
    t
}

#[test]
fn report_csv_na_stdout_z_filtrem_zadania_i_czasu() {
    let t = setup();
    let o = bin(t.path())
        .args(["report", "--job", "Zdjęcia", "--since", "7d"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    let s = out_s(&o);
    assert!(s.starts_with('\u{feff}'), "BOM");
    assert!(s.contains("Zdjęcia"), "nazwa zadania w kolumnie");
    assert!(s.contains("'=cmd.txt"), "ochrona przed formułą: {s}");
    assert!(!s.contains("stary.txt"), "wpis sprzed 20 dni poza zakresem");
    assert!(!s.contains("obce.txt"), "inne zadanie poza filtrem");
}

#[test]
fn report_html_do_pliku() {
    let t = setup();
    let out = t.path().join("r.html");
    let o = bin(t.path())
        .args([
            "report",
            "--job",
            "abcdef12",
            "--since",
            "7d",
            "--format",
            "html",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(out_s(&o).contains("Zapisano raport: 2"));
    let h = fs::read_to_string(&out).unwrap();
    assert!(h.contains("<!doctype html>") && h.contains("Raport Syncron"));
    assert!(h.contains("&lt;b&gt;x&lt;/b&gt;.txt") && !h.contains("<b>x</b>"));
    assert!(h.contains("brak dostępu"));
}

#[test]
fn report_json_i_rodzaj() {
    let t = setup();
    let o = bin(t.path())
        .args(["report", "--format", "json", "--kind", "error"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    let v: serde_json::Value = serde_json::from_str(&out_s(&o)).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 1);
    assert_eq!(v["summary"]["errors"], 1);
}

#[test]
fn report_bledne_argumenty() {
    let t = setup();
    for args in [
        vec!["report", "--format", "xml"],
        vec!["report", "--since", "wczoraj"],
        vec!["report", "--nieznana", "x"],
        vec!["report", "--out"],
        vec![
            "report",
            "--job",
            "nie-istnieje-ale-id",
            "--since",
            "kiedys",
        ],
    ] {
        let o = bin(t.path()).args(&args).output().unwrap();
        assert!(!o.status.success(), "{args:?}");
        assert!(!err_s(&o).is_empty());
    }
}

#[test]
fn import_czysci_webhook_url() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("in.json");
    fs::write(
        &f,
        r#"[{"id":"w1","name":"W","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"},"webhook_url":"https://hooks.example/TOKEN","webhook_on":"Always","run_after":"echo x"}]"#,
    )
    .unwrap();
    let data = t.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let o = bin(&data)
        .args(["import", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(
        err_s(&o).contains("webhook"),
        "ostrzeżenie o usunięciu: {}",
        err_s(&o)
    );
    let saved = fs::read_to_string(data.join("jobs.json")).unwrap();
    assert!(
        !saved.contains("TOKEN") && !saved.contains("echo x"),
        "{saved}"
    );
    assert!(saved.contains("Always"), "reszta ustawień zostaje");
}
