//! Ekspert 2: kosz synchronizacji i wersje plików (unikalne znaczniki, walidacja, retencja).
use std::fs;
use std::path::Path;
use syncron_core::location::Location;

fn loc(p: &Path) -> Location {
    Location::parse(p.to_str().unwrap())
}

fn put(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn backup_same_stamp_gets_unique_folder() {
    let t = tempfile::tempdir().unwrap();
    let l = loc(t.path());
    put(t.path(), "a.txt", "v1");
    l.backup("a.txt", "2026-01-01_000000").unwrap();
    put(t.path(), "a.txt", "v2");
    l.backup("a.txt", "2026-01-01_000000").unwrap();
    put(t.path(), "a.txt", "v3");
    l.backup("a.txt", "2026-01-01_000000").unwrap();
    let items = l.list_trash().unwrap();
    let stamps: Vec<&str> = items.iter().map(|i| i.0.as_str()).collect();
    assert_eq!(
        stamps,
        vec![
            "2026-01-01_000000",
            "2026-01-01_000000-1",
            "2026-01-01_000000-2"
        ]
    );
    assert_eq!(
        fs::read_to_string(t.path().join(".syncron-trash/2026-01-01_000000-1/a.txt")).unwrap(),
        "v2"
    );
}

#[test]
fn cap_versions_orders_numeric_suffix() {
    let t = tempfile::tempdir().unwrap();
    for s in ["", "-1", "-2", "-10"] {
        put(
            t.path(),
            &format!(".syncron-trash/2026-01-01_000000{s}/a.txt"),
            s,
        );
    }
    loc(t.path()).purge_trash(0, 2);
    let st: Vec<String> = loc(t.path())
        .list_trash()
        .unwrap()
        .into_iter()
        .map(|i| i.0)
        .collect();
    assert_eq!(st, vec!["2026-01-01_000000-10", "2026-01-01_000000-2"]);
}

#[test]
fn invalid_stamps_and_paths_rejected() {
    let t = tempfile::tempdir().unwrap();
    let l = loc(t.path());
    put(t.path(), "zywy.txt", "z");
    for bad in ["..", ".", "a/b", "../x", ""] {
        assert!(l.delete_trash(Some(bad), None).is_err(), "{bad}");
        assert!(l.restore_trash(bad, "zywy.txt").is_err(), "{bad}");
        assert!(l.backup("zywy.txt", bad).is_err(), "{bad}");
    }
    assert!(l.delete_trash(Some("s"), Some("../zywy.txt")).is_err());
    let e = l
        .restore_trash("2026-01-01_000000", "brak.txt")
        .unwrap_err()
        .to_string();
    assert!(e.contains("Nie znaleziono wersji w koszu"), "{e}");
    assert!(t.path().join("zywy.txt").exists());
}

#[test]
fn purge_by_stamp_name_skips_foreign_and_huge_days() {
    let t = tempfile::tempdir().unwrap();
    put(t.path(), ".syncron-trash/2000-01-01_000000/a.txt", "x");
    put(t.path(), ".syncron-trash/2000-01-01_000000-3/b.txt", "x");
    put(t.path(), ".syncron-trash/2000-obce/c.txt", "x");
    put(t.path(), ".syncron-trash/żółć-żółć/d.txt", "x");
    let l = loc(t.path());
    l.purge_trash(u32::MAX, 0);
    assert_eq!(l.list_trash().unwrap().len(), 4);
    l.purge_trash(10, 0);
    let left: Vec<String> = l.list_trash().unwrap().into_iter().map(|i| i.0).collect();
    assert_eq!(left, vec!["2000-obce", "żółć-żółć"]);
}
