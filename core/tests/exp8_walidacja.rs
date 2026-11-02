//! Ekspert 8 — `Job::validate` (T10-02, T10-11d).
use syncron_core::model::{Job, Schedule};

fn job(l: &str, r: &str) -> Job {
    let mut j: Job = serde_json::from_str(
        r#"{"id":"1","name":"t","left":"a","right":"b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}"#,
    )
    .unwrap();
    j.left = l.into();
    j.right = r.into();
    j
}

fn err(l: &str, r: &str) -> String {
    job(l, r).validate().unwrap_err()
}

#[test]
fn poprawne_zadania_przechodza() {
    assert!(job("/x/a", "/x/b").validate().is_ok());
    assert!(job("/x/a", "/x/ab").validate().is_ok()); // wspólny przedrostek tekstu, nie komponentów
    assert!(job("/x/a", "gd:Kopia").validate().is_ok());
    assert!(job("gd:A", "dropbox:A").validate().is_ok());
    assert!(job("gd:A", "gd:B").validate().is_ok());
    assert!(job("C:\\a", "D:\\a").validate().is_ok());
}

#[test]
fn puste_pola() {
    let mut j = job("/a", "/b");
    j.id = " ".into();
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.name = "".into();
    assert!(j.validate().is_err());
    assert!(err("", "/b").contains("lokalizacje"));
    assert!(err("/a", "  ").contains("lokalizacje"));
}

#[test]
fn takie_same_po_normalizacji() {
    assert!(err("/x/a", "/x/a/").contains("takie same"));
    assert!(err("/x/a", "/x/./b/../a").contains("takie same"));
    assert!(err("gd:A/B", "gd:A/B/").contains("takie same"));
}

#[test]
fn zagniezdzenie_w_obie_strony() {
    assert!(err("/x/a", "/x/a/kopia").contains("wewnątrz"));
    assert!(err("/x/a/kopia", "/x/a").contains("wewnątrz"));
    assert!(err("/x/a", "/x/a/b/../c").contains("wewnątrz"));
    assert!(err("gd:A", "gd:A/kopia").contains("wewnątrz"));
    assert!(err("gd:", "gd:cokolwiek").contains("wewnątrz"));
}

#[test]
fn windows_separatory_i_litery() {
    assert!(err("C:\\x\\a", "C:\\x\\a\\kopia").contains("wewnątrz"));
    assert!(err("C:\\x\\a\\", "C:/x/a").contains("takie same"));
    assert!(err("\\\\srv\\share\\a", "\\\\srv\\share\\a\\b").contains("wewnątrz"));
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn wielkosc_liter_na_windows_i_macos() {
    assert!(err("/X/A", "/x/a").contains("takie same"));
}

#[test]
fn zakresy() {
    let mut j = job("/a", "/b");
    j.schedule = Schedule::Daily {
        hour: 25,
        minute: 99,
    };
    assert!(j.validate().is_err());
    j.schedule = Schedule::Daily {
        hour: 23,
        minute: 59,
    };
    assert!(j.validate().is_ok());
    j.schedule = Schedule::Interval { secs: 0 };
    assert!(j.validate().is_err());
    j.schedule = Schedule::Realtime { poll_secs: 0 };
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.window = Some([24, 3]);
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.guard_pct = 0;
    assert!(j.validate().is_err());
    j.guard_pct = 101;
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.threads = 0;
    assert!(j.validate().is_err());
    j.threads = 17;
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.bandwidth.kbps = u32::MAX;
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.bandwidth.hours = Some([8, 30]);
    assert!(j.validate().is_err());
    let mut j = job("/a", "/b");
    j.trash_days = u32::MAX;
    assert!(j.validate().is_err());
}

// ---------- parse_import (T9-12, T10-18d) ----------

use syncron_core::model::parse_import;

const J: &str = r#"{"id":"1","name":"t","left":"/a","right":"/b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"},"enabled":true}"#;

#[test]
fn import_wylacza_zadania() {
    let v = parse_import(&format!("[{J}]")).unwrap();
    assert_eq!(v.len(), 1);
    assert!(!v[0].enabled);
}

#[test]
fn import_toleruje_bom() {
    assert!(parse_import(&format!("\u{feff}[{J}]")).is_ok());
}

#[test]
fn import_czytelny_blad_zamiast_serde() {
    let e = parse_import("{nie json").unwrap_err();
    assert!(
        e.starts_with("To nie jest plik konfiguracji Syncron"),
        "{e}"
    );
    assert!(!e.contains("expected"), "{e}");
    assert!(parse_import("[]").is_err());
}

#[test]
fn import_odrzuca_zle_zadania() {
    let bad = J.replace("\"/b\"", "\"/a/kopia\"");
    let e = parse_import(&format!("[{bad}]")).unwrap_err();
    assert!(e.contains("Zadanie nr 1") && e.contains("wewnątrz"), "{e}");
    let bad = J.replace(
        "{\"type\":\"Manual\"}",
        "{\"type\":\"Daily\",\"hour\":25,\"minute\":99}",
    );
    assert!(parse_import(&format!("[{bad}]")).is_err());
}

#[test]
fn import_scala_duplikaty_id() {
    let v = parse_import(&format!("[{J},{}]", J.replace("\"t\"", "\"drugie\""))).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "drugie");
}
