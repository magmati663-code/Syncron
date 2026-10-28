//! QA (tester 8): integracja z rclone i lokalizacje zdalne, testowana atrapą rclone
//! (`tests/fixtures/fake_rclone.py`, wskazywana przez SYNCRON_RCLONE). Wymaga python3.
//! Zmienne środowiskowe są globalne dla procesu, więc każdy test trzyma `Fx` (Mutex).
//! Testy oznaczone `#[ignore = "BUG: ..."]` odtwarzają błędy, które istnieją w kodzie.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use syncron_core::engine::{run_job, trash_list, trash_restore, Report};
use syncron_core::location::{run_rclone, Location};
use syncron_core::model::Job;

static LOCK: Mutex<()> = Mutex::new(());
const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_rclone.py");

struct Fx {
    _g: MutexGuard<'static, ()>,
    _t: tempfile::TempDir,
    /// "chmura" (FAKE_RCLONE_ROOT)
    cloud: PathBuf,
    /// lokalna strona
    l: PathBuf,
    state: PathBuf,
    ctrl: PathBuf,
    log: PathBuf,
}

fn fx() -> Fx {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = tempfile::tempdir().unwrap();
    let p = t.path().to_path_buf();
    let f = Fx {
        _g: g,
        cloud: p.join("cloud"),
        l: p.join("L"),
        state: p.join("state"),
        ctrl: p.join("ctrl.json"),
        log: p.join("calls.log"),
        _t: t,
    };
    fs::create_dir_all(&f.cloud).unwrap();
    fs::create_dir_all(&f.l).unwrap();
    std::env::set_var("SYNCRON_RCLONE", FAKE);
    std::env::set_var("FAKE_RCLONE_ROOT", &f.cloud);
    std::env::set_var("FAKE_RCLONE_CTRL", &f.ctrl);
    std::env::set_var("FAKE_RCLONE_LOG", &f.log);
    std::env::remove_var("FAKE_RCLONE_DELAY");
    f.ctrl_set(json!({}));
    f
}

impl Fx {
    fn ctrl_set(&self, v: Value) {
        // zerujemy liczniki reguł
        for e in fs::read_dir(self.ctrl.parent().unwrap()).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("ctrl.json.cnt") {
                let _ = fs::remove_file(e.path());
            }
        }
        fs::write(&self.ctrl, v.to_string()).unwrap();
    }
    fn rules(&self, rules: Value) {
        self.ctrl_set(json!({ "rules": rules }));
    }
    fn clear(&self) {
        self.ctrl_set(json!({}));
    }
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                v["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_string())
                    .collect()
            })
            .collect()
    }
    fn job(&self, right: &str, extra: Value) -> Job {
        job_between(self.l.to_str().unwrap(), right, extra)
    }
    fn run(&self, j: &Job) -> Report {
        run_job(j, &self.state, false, false).unwrap()
    }
}

fn job_between(left: &str, right: &str, extra: Value) -> Job {
    let mut v = json!({
        "id": "q8", "name": "q8", "left": left, "right": right,
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn write(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn rules_of(j: &Job) -> syncron_core::rules::RuleSet {
    j.rules()
}

fn is_rclone(s: &str) -> bool {
    matches!(Location::parse(s), Location::Rclone(_))
}

fn rss_kb() -> u64 {
    let s = fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Synchronizuje 6 plików lokalnych do chmury (stan bazowy).
fn seed_synced(f: &Fx, j: &Job) {
    for i in 0..6 {
        write(&f.l.join(format!("f{i}.txt")), &format!("tresc {i}"));
    }
    let rep = f.run(j);
    assert_eq!(rep.copied, 6, "{rep:?}");
    assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
}

// ======================= RUNDA 1: podstawy =======================

#[test]
fn r1_parse_table() {
    let remote = [
        "gdrive:",
        "gdrive:Dokumenty",
        "bitrix:/a b",
        "bitrix:a b/c d",
        "Mój Dysk:folder",
        "Dysk Zażółć:x",
        "my.remote:a:b:c",
        "my-remote_1:",
        "  gdrive:x  ",
        "ab:",
        "ab:\\foo", // 2 znaki: konto, nie dysk
        "fake:C:\\Dane",
    ];
    for s in remote {
        assert!(is_rclone(s), "powinno być zdalne: {s:?}");
    }
    let local = [
        "C:\\Dane",
        "C:/Dane",
        "c:",
        "C:",
        "D:\\",
        "C:foo", // litera dysku z względną ścieżką: nie konto (konto ma >= 2 znaki)
        "\\\\nas\\udział",
        "//server/share",
        "//server/share/a:b",
        "/Volumes/Dysk",
        "/mnt/a:b/c",
        "relative/dir",
        "",
        ":x",
        "x",
        "a/b:c", // '/' w nazwie => lokalna
        "C:\\Dane\\a:b",
        "dysk\\folder:x",
    ];
    for s in local {
        assert!(!is_rclone(s), "powinno być lokalne: {s:?}");
    }
    // trim: spacje na brzegach znikają z ścieżki zdalnej
    match Location::parse("  gdrive:x  ") {
        Location::Rclone(s) => assert_eq!(s, "gdrive:x"),
        _ => unreachable!(),
    }
}

#[test]
#[ignore = "BUG: nazwa konta rclone z polskimi znakami w NFD (macOS) nie jest rozpoznawana jako konto zdalne (znak łączący nie jest alfanumeryczny)"]
fn r1_parse_nfd_remote_name() {
    use unicode_normalization::UnicodeNormalization;
    let nfd: String = "Dysk Zażółć:folder".nfd().collect();
    assert!(
        is_rclone(&nfd),
        "NFD: {nfd:?} potraktowane jak folder lokalny"
    );
}

#[test]
fn r1_lsjson_variants() {
    let f = fx();
    use unicode_normalization::UnicodeNormalization;
    let nfd: String = "żółć.txt".nfd().collect();
    let body = json!([
        {"Path":"a.txt","Size":3,"ModTime":"2026-10-27T10:00:00.123456789Z","IsDir":false},
        {"Path":"tz.txt","Size":1,"ModTime":"2026-10-27T12:00:00+02:00","IsDir":false},
        {"Path":"nomod.txt","Size":1,"IsDir":false},
        {"Path":"dir","Size":-1,"IsDir":true},
        {"Path":"dir/in side.txt","Size":0,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":"gdoc","Size":-1,"IsDir":false},
        {"Path":"żółć.txt","Size":5,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":nfd,"Size":5,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":"Dup.TXT","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":"dup.txt","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":".syncron-trash/x/y.txt","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false},
        {"Path":"~$tmp.docx","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}
    ])
    .to_string();
    f.rules(json!([{ "op": "lsjson", "stdout": body }]));
    let j = f.job("fake:", json!({}));
    let l = Location::parse("fake:").list(&rules_of(&j)).unwrap();
    let ts = |s: &str| chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp();
    let get = |k: &str| {
        l.files
            .get(k)
            .unwrap_or_else(|| panic!("brak {k}: {:?}", l.files.keys()))
    };
    assert_eq!(get("a.txt").meta.mtime, ts("2026-10-27T10:00:00Z"));
    assert_eq!(get("tz.txt").meta.mtime, ts("2026-10-27T10:00:00Z"));
    assert_eq!(get("dir/in side.txt").meta.size, 0);
    assert!(!l.files.contains_key("dir"));
    assert!(!l.files.contains_key("gdoc"));
    assert!(!l
        .files
        .keys()
        .any(|k| k.contains("syncron-trash") || k.contains("tmp.docx")));
    // NFC/NFD i wielkość liter to kolizje, nie duplikaty w mapie
    assert_eq!(l.collisions.len(), 2, "{:?}", l.collisions);
}

#[test]
fn r1_directory_not_found_is_empty_other_error_is_err() {
    let f = fx();
    let j = f.job("fake:nie/ma", json!({}));
    let loc = Location::parse("fake:nie/ma");
    assert!(loc.list(&rules_of(&j)).unwrap().files.is_empty());
    f.rules(json!([{ "op": "lsjson", "stderr": "Failed to create file system: couldn't connect: dial tcp: no such host", "code": 1 }]));
    let e = loc.list(&rules_of(&j)).unwrap_err();
    assert!(e.contains("no such host"), "{e}");
    // wielkie litery też
    f.rules(json!([{ "op": "lsjson", "stderr": "Error: Directory Not Found", "code": 3 }]));
    assert!(loc.list(&rules_of(&j)).unwrap().files.is_empty());
}

#[test]
fn r1_stat_single_file() {
    let f = fx();
    write(&f.cloud.join("d/a b.txt"), "12345");
    set_mtime(&f.cloud.join("d/a b.txt"), 1_700_000_000);
    let loc = Location::parse("fake:d");
    let m = loc.stat("a b.txt").unwrap();
    assert_eq!((m.size, m.mtime), (5, 1_700_000_000));
    assert!(loc.stat("nie ma.txt").is_none());
}

#[test]
fn r1_end_to_end_roundtrip() {
    let f = fx();
    let j = f.job("fake:Mój folder/ąę", json!({}));
    write(&f.l.join("Zażółć/gęślą jaźń.txt"), "z lewej");
    write(&f.cloud.join("Mój folder/ąę/Łódź.txt"), "z prawej");
    let rep = f.run(&j);
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(
        fs::read_to_string(f.cloud.join("Mój folder/ąę/Zażółć/gęślą jaźń.txt")).unwrap(),
        "z lewej"
    );
    assert_eq!(
        fs::read_to_string(f.l.join("Łódź.txt")).unwrap(),
        "z prawej"
    );
    assert!(f.run(&j).planned.is_empty());

    // zmiana -> stara wersja w koszu po stronie zdalnej
    write(&f.l.join("Zażółć/gęślą jaźń.txt"), "nowa wersja!");
    set_mtime(&f.l.join("Zażółć/gęślą jaźń.txt"), 2_000_000_000);
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    let tr = trash_list(&j).unwrap();
    assert!(
        tr.iter()
            .any(|i| i.side == "right" && i.path == "Zażółć/gęślą jaźń.txt"),
        "{tr:?}"
    );

    // przeniesienie -> moveto
    fs::create_dir_all(f.l.join("Nowy")).unwrap();
    fs::rename(f.l.join("Łódź.txt"), f.l.join("Nowy/Łódź 2.txt")).unwrap();
    // cel rclone nie ma hasha: przeniesienia nie zgadujemy (T1-01), robimy kopiowanie + usunięcie
    let mut j2 = j.clone();
    j2.guard_pct = 100;
    let rep = f.run(&j2);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (0, 1, 1), "{rep:?}");
    assert!(f.cloud.join("Mój folder/ąę/Nowy/Łódź 2.txt").exists());
    assert!(!f.cloud.join("Mój folder/ąę/Łódź.txt").exists());

    // usunięcie -> kosz po prawej
    for i in 0..6 {
        write(&f.l.join(format!("p{i}.txt")), "x");
    }
    f.run(&j);
    fs::remove_file(f.l.join("p0.txt")).unwrap();
    let rep = f.run(&j);
    assert_eq!(rep.deleted, 1, "{rep:?}");
    assert!(!f.cloud.join("Mój folder/ąę/p0.txt").exists());
    let tr = trash_list(&j).unwrap();
    assert!(tr.iter().any(|i| i.path == "p0.txt"), "{tr:?}");
    // przywrócenie z kosza
    let it = tr.iter().find(|i| i.path == "p0.txt").unwrap();
    let back = trash_restore(&j, "right", &it.stamp, &it.path).unwrap();
    assert_eq!(back, "p0.txt");
    assert!(f.cloud.join("Mój folder/ąę/p0.txt").exists());
}

#[test]
fn r1_run_rclone_direct() {
    let f = fx();
    assert!(run_rclone(&["version"]).unwrap().contains("fake"));
    f.rules(json!([{ "op": "version", "stderr": "  boom \n", "code": 7 }]));
    assert_eq!(run_rclone(&["version"]).unwrap_err(), "boom");
    std::env::set_var("SYNCRON_RCLONE", "/nie/ma/takiego/rclone");
    let e = run_rclone(&["version"]).unwrap_err();
    assert!(e.contains("nie można uruchomić rclone"), "{e}");
    std::env::set_var("SYNCRON_RCLONE", FAKE);
}

// ======================= RUNDA 2: brzegi i złośliwe =======================

#[test]
fn r2_garbage_listing_aborts_without_changes() {
    for body in [
        "",
        "<html>502</html>",
        "[{\"Path\":\"a\",",
        "{\"a\":1}",
        "null",
        "[NaN]",
        "[{\"Path\":\"a\",\"Size\":NaN}]",
        "[{\"Path\":\"a\",\"Size\":1e999}]",
    ] {
        let f = fx();
        let j = f.job("fake:", json!({}));
        seed_synced(&f, &j);
        f.rules(json!([{ "op": "lsjson", "stdout": body }]));
        let e = run_job(&j, &f.state, false, false);
        assert!(
            e.is_err(),
            "śmieciowy JSON {body:?} powinien przerwać przebieg, a dał {e:?}"
        );
        assert_eq!(fs::read_dir(&f.cloud).unwrap().count(), 6);
        assert_eq!(fs::read_dir(&f.l).unwrap().count(), 6);
    }
}

#[test]
fn r2_lsjson_odd_values_do_not_panic() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    let body = r#"[1,"x",null,[],{"Path":5,"Size":3},{"Path":"ok.txt","Size":7,"ModTime":12345},
        {"Path":"f.txt","Size":2.5,"ModTime":"nie data"},{"Path":"big.txt","Size":9223372036854775807,"ModTime":"2026-10-27T10:00:00Z"},
        {"Path":"ModBad.txt","Size":1,"ModTime":"0001-01-01T00:00:00Z"}]"#;
    f.rules(json!([{ "op": "lsjson", "stdout": body }]));
    let l = Location::parse("fake:").list(&rules_of(&j)).unwrap();
    assert!(l.files.contains_key("ok.txt"));
    assert_eq!(l.files["big.txt"].meta.size, i64::MAX as u64);
    assert_eq!(l.files["modbad.txt"].meta.mtime, -62135596800);
}

#[test]
#[ignore = "BUG: wpis lsjson bez pola Path trafia na listę z pustą ścieżką (kopiowanie 'korzenia' jako pliku)"]
fn r2_missing_path_is_not_listed_as_root() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.rules(json!([{ "op": "lsjson", "stdout": r#"[{"Size":3,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"# }]));
    let l = Location::parse("fake:").list(&rules_of(&j)).unwrap();
    assert!(l.files.is_empty(), "wpis bez Path: {:?}", l.files);
}

#[test]
#[ignore = "BUG: ścieżka ze zdalnego listingu z '..' jest doklejana do korzenia lokalnego (path traversal: zapis poza folderem synchronizacji)"]
fn r2_remote_path_traversal_cannot_escape_local_root() {
    let f = fx();
    let base = f._t.path().to_path_buf();
    let left = base.join("a/L");
    fs::create_dir_all(&left).unwrap();
    // "fake:../evil.txt" = <cloud>/../evil.txt = base/evil.txt
    fs::write(base.join("evil.txt"), "zlosliwe").unwrap();
    f.rules(json!([{ "op": "lsjson", "match": "-R", "stdout":
        r#"[{"Path":"../evil.txt","Size":8,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"# }]));
    let j = job_between(
        left.to_str().unwrap(),
        "fake:",
        json!({"mode":"RightToLeft"}),
    );
    let rep = run_job(&j, &f.state, false, false).unwrap();
    // docelowo L/../evil.txt = base/a/evil.txt, czyli poza folderem synchronizacji
    assert!(
        !base.join("a/evil.txt").exists(),
        "zapis poza korzeniem lokalnym: {rep:?}"
    );
}

fn count_ops(f: &Fx, op: &str) -> usize {
    f.calls().iter().filter(|c| c[0] == op).count()
}

#[test]
fn r2_copy_failure_kinds_are_safe_and_retried() {
    // (stderr, kod, oczekiwane: busy?)
    let kinds = [
        (
            "The process cannot access the file because it is being used by another process.",
            1,
            true,
        ),
        ("file is locked", 1, true),
        ("sharing violation", 1, true),
        (
            "Failed to copyto: googleapi: Error 403: rateLimitExceeded",
            1,
            false,
        ),
        ("couldn't connect", 9, false),
    ];
    for (stderr, code, busy) in kinds {
        let f = fx();
        let j = f.job("fake:", json!({}));
        write(&f.l.join("nowy plik.txt"), "abc");
        f.rules(json!([{ "op": "copyto", "stderr": stderr, "code": code }]));
        let rep = f.run(&j);
        assert_eq!(rep.copied, 0, "{rep:?}");
        assert!(!f.cloud.join("nowy plik.txt").exists());
        assert!(rep.needs_retry());
        assert_eq!(
            (rep.busy.len(), rep.errors.len()),
            if busy { (1, 0) } else { (0, 1) },
            "{stderr}: {rep:?}"
        );
        assert!(f.l.join("nowy plik.txt").exists());
        // ponowienie po usunięciu usterki
        f.clear();
        let rep = f.run(&j);
        assert_eq!(rep.copied, 1, "{rep:?}");
        assert!(f.cloud.join("nowy plik.txt").exists());
    }
}

#[test]
fn r2_backup_failure_prevents_overwrite() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    write(&f.l.join("f0.txt"), "NOWA wersja");
    set_mtime(&f.l.join("f0.txt"), 2_000_000_000);
    // zawodzi tylko kopia do kosza
    f.rules(json!([{ "op": "copyto", "match": ".syncron-trash", "stderr": "quota exceeded", "code": 1 }]));
    let rep = f.run(&j);
    assert_eq!(rep.copied, 0, "{rep:?}");
    assert_eq!(rep.errors.len(), 1);
    assert_eq!(
        fs::read_to_string(f.cloud.join("f0.txt")).unwrap(),
        "tresc 0",
        "stara wersja nie może zniknąć bez kopii w koszu"
    );
    f.clear();
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(
        fs::read_to_string(f.cloud.join("f0.txt")).unwrap(),
        "NOWA wersja"
    );
}

#[test]
fn r2_copy_failure_after_backup_keeps_target() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    write(&f.l.join("f1.txt"), "NOWA");
    set_mtime(&f.l.join("f1.txt"), 2_000_000_000);
    // kopia do kosza przechodzi (pierwsze wywołanie copyto), właściwa kopia zawodzi
    f.rules(json!([{ "op": "copyto", "skip": 1, "times": 1, "stderr": "i/o timeout", "code": 1 }]));
    let rep = f.run(&j);
    assert_eq!(rep.errors.len(), 1, "{rep:?}");
    assert_eq!(
        fs::read_to_string(f.cloud.join("f1.txt")).unwrap(),
        "tresc 1"
    );
    assert_eq!(fs::read_to_string(f.l.join("f1.txt")).unwrap(), "NOWA");
}

#[test]
fn r2_delete_failure_does_not_resurrect_or_lose() {
    for (name, extra, op) in [
        ("kosz", json!({}), "moveto"),
        ("bez kosza", json!({"trash_days": 0}), "deletefile"),
    ] {
        let f = fx();
        let j = f.job("fake:", extra);
        seed_synced(&f, &j);
        fs::remove_file(f.l.join("f0.txt")).unwrap();
        f.rules(json!([{ "op": op, "stderr": "used by another process", "code": 1 }]));
        let rep = f.run(&j);
        assert_eq!(rep.deleted, 0, "{name}: {rep:?}");
        assert_eq!(rep.busy.len(), 1, "{name}: {rep:?}");
        assert!(f.cloud.join("f0.txt").exists(), "{name}");
        assert!(
            !f.l.join("f0.txt").exists(),
            "{name}: plik nie może wrócić na lewo"
        );
        f.clear();
        let rep = f.run(&j);
        assert_eq!(rep.deleted, 1, "{name}: {rep:?}");
        assert!(!f.cloud.join("f0.txt").exists());
        assert!(!f.l.join("f0.txt").exists());
    }
}

#[test]
fn r2_rename_failure_converges() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    fs::rename(f.l.join("f2.txt"), f.l.join("f2 nowa nazwa.txt")).unwrap();
    f.rules(json!([{ "op": "moveto", "stderr": "sharing violation", "code": 1 }]));
    let rep = f.run(&j);
    assert!(
        f.cloud.join("f2.txt").exists(),
        "stary plik nie może zniknąć: {rep:?}"
    );
    assert!(rep.needs_retry(), "{rep:?}");
    f.clear();
    f.run(&j);
    assert!(!f.cloud.join("f2.txt").exists());
    assert_eq!(
        fs::read_to_string(f.cloud.join("f2 nowa nazwa.txt")).unwrap(),
        "tresc 2"
    );
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn r2_directory_not_found_on_previously_filled_side_blocks() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    // wariant 1: katalog znika
    fs::remove_dir_all(&f.cloud).unwrap();
    let rep = f.run(&j);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!((rep.copied, rep.deleted), (0, 0));
    assert_eq!(fs::read_dir(&f.l).unwrap().count(), 6);
    // wariant 2: rclone twierdzi "directory not found" (kod 3), choć katalog jest
    fs::create_dir_all(&f.cloud).unwrap();
    f.rules(json!([{ "op": "lsjson", "stderr": "error listing: directory not found", "code": 3 }]));
    let rep = f.run(&j);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(fs::read_dir(&f.l).unwrap().count(), 6);
    // lewa strona pusta (lokalnie) a prawa pełna
    f.clear();
    write(&f.cloud.join("f0.txt"), "x");
    set_mtime(&f.cloud.join("f0.txt"), 1_700_000_000);
    for i in 1..6 {
        write(&f.cloud.join(format!("f{i}.txt")), "tresc");
    }
    let j2 = f.job("fake:", json!({}));
    for e in fs::read_dir(&f.l).unwrap().flatten() {
        fs::remove_file(e.path()).unwrap();
    }
    let rep = f.run(&j2);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(fs::read_dir(&f.cloud).unwrap().count(), 6);
}

#[test]
fn r2_first_run_with_missing_remote_dir_creates_it() {
    let f = fx();
    let j = f.job("fake:nowy/katalog z spacją", json!({}));
    write(&f.l.join("a.txt"), "a");
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert!(f.cloud.join("nowy/katalog z spacją/a.txt").exists());
}

#[test]
fn r2_nasty_names_are_passed_as_single_args() {
    let f = fx();
    let j = f.job("fake:bitrix dir/a b", json!({}));
    let names = [
        "a b.txt",
        "it's \"q\".txt",
        "$HOME `id` $(x).txt",
        "-rf.txt",
        "--delete-excluded.txt",
        "[x]?*.txt",
        "dwukropek: tak.txt",
        " spacja z przodu.txt",
        "nowa\nlinia.txt",
        "a;b|c&d>e.txt",
        "Zażółć/--flag/jaźń.txt",
    ];
    for n in names {
        write(&f.l.join(n), &format!("l:{n}"));
    }
    write(&f.cloud.join("bitrix dir/a b/-z-prawej.txt"), "r");
    let rep = f.run(&j);
    assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
    for n in names {
        assert_eq!(
            fs::read_to_string(f.cloud.join("bitrix dir/a b").join(n)).unwrap(),
            format!("l:{n}")
        );
    }
    assert_eq!(fs::read_to_string(f.l.join("-z-prawej.txt")).unwrap(), "r");
    // żaden argument nie jest samodzielną opcją pochodzącą z nazwy pliku
    for c in f.calls() {
        for a in &c[1..] {
            if a.starts_with('-') {
                assert!(
                    [
                        "-R",
                        "--files-only",
                        "--dirs-only",
                        "--no-mimetype",
                        "--bwlimit",
                        "--min-age",
                        "--rmdirs"
                    ]
                    .contains(&a.as_str()),
                    "nieoczekiwana opcja z nazwy pliku: {a:?} w {c:?}"
                );
            }
        }
    }
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn r2_bwlimit_args() {
    let f = fx();
    let j = f.job(
        "fake:",
        json!({"threads": 4, "bandwidth": {"kbps": 800, "off_kbps": 0, "hours": [8, 18]}}),
    );
    write(&f.l.join("a.txt"), "a");
    f.run(&j);
    let c = f.calls();
    let cp: Vec<_> = c.iter().filter(|c| c[0] == "copyto").collect();
    assert_eq!(cp.len(), 1);
    let i = cp[0].iter().position(|a| a == "--bwlimit").unwrap();
    assert_eq!(cp[0][i + 1], "08:00,200k 18:00,off");
    drop(f);
    let f = fx();
    let j = f.job("fake:", json!({"threads": 2, "bandwidth": {"kbps": 500}}));
    write(&f.l.join("a.txt"), "a");
    f.run(&j);
    let c = f.calls();
    let cp = c.iter().find(|c| c[0] == "copyto").unwrap();
    assert_eq!(
        cp.iter().skip_while(|a| *a != "--bwlimit").nth(1).unwrap(),
        "250k"
    );
    drop(f);
    let f = fx();
    let j = f.job("fake:", json!({}));
    write(&f.l.join("a.txt"), "a");
    f.run(&j);
    assert!(!f
        .calls()
        .iter()
        .any(|c| c.contains(&"--bwlimit".to_string())));
}

#[test]
fn r2_syncignore_on_remote_via_cat() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    write(&f.cloud.join(".syncignore"), "*.log\n");
    write(&f.l.join("a.log"), "x");
    write(&f.l.join("a.txt"), "x");
    let rep = f.run(&j);
    assert!(
        f.cloud.join("a.txt").exists() && !f.cloud.join("a.log").exists(),
        "{rep:?}"
    );
    assert!(count_ops(&f, "cat") >= 1);
    // cat zawodzi: .syncignore jest ignorowany (brak = None) — a plik się synchronizuje
    let loc = Location::parse("fake:");
    f.rules(json!([{ "op": "cat", "stderr": "i/o error", "code": 1 }]));
    assert!(loc.read_text(".syncignore").is_none());
}

#[test]
fn r2_trash_api_on_remote() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    write(&f.l.join("f0.txt"), "v2");
    set_mtime(&f.l.join("f0.txt"), 2_000_000_000);
    f.run(&j);
    let tr = trash_list(&j).unwrap();
    let it = tr
        .iter()
        .find(|i| i.side == "right" && i.path == "f0.txt")
        .unwrap();
    // gdy w miejscu docelowym jest plik, przywracamy pod inną nazwą
    let back = trash_restore(&j, "right", &it.stamp, "f0.txt").unwrap();
    assert!(back.contains("przywrócono"), "{back}");
    assert_eq!(fs::read_to_string(f.cloud.join("f0.txt")).unwrap(), "v2");
    assert_eq!(fs::read_to_string(f.cloud.join(&back)).unwrap(), "tresc 0");
    let loc = Location::parse("fake:");
    loc.delete_trash(Some(&it.stamp), Some("f0.txt")).unwrap();
    assert!(trash_list(&j).unwrap().iter().all(|i| i.path != "f0.txt"));
    loc.delete_trash(None, None).unwrap();
    assert!(!f.cloud.join(".syncron-trash").exists());
    // usunięcie pustego kosza: "directory not found" nie jest błędem (T8-12)
    loc.delete_trash(None, None).unwrap();
    assert!(loc.list_trash().unwrap().is_empty());
    // list_dirs
    write(&f.cloud.join("Podfolder ą/x.txt"), "x");
    write(&f.cloud.join(".syncron-trash/s/y.txt"), "x");
    let d = loc.list_dirs("").unwrap();
    assert_eq!(d, vec!["Podfolder ą".to_string()]);
    assert!(loc.list_dirs("nie/ma").is_err());
    f.rules(json!([{ "op": "lsjson", "stdout": "garbage" }]));
    assert!(loc.list_dirs("").is_err());
    assert!(loc.list_trash().is_err());
}

#[test]
fn r2_trash_dir_error_is_distinguished_from_missing() {
    let f = fx();
    f.rules(json!([{ "op": "lsjson", "stderr": "couldn't connect", "code": 1 }]));
    let e = Location::parse("fake:").list_trash().unwrap_err();
    assert!(e.contains("couldn't connect"));
}

#[test]
fn r2_delete_empty_remote_trash_is_ok() {
    let _f = fx();
    Location::parse("fake:")
        .delete_trash(None, None)
        .expect("pusty kosz: brak błędu");
}

#[test]
fn r2_remote_trash_survives_purge_for_old_files() {
    let f = fx();
    let j = f.job("fake:", json!({"trash_days": 30}));
    write(&f.l.join("stary.txt"), "v1");
    write(&f.cloud.join("stary.txt"), "v1");
    set_mtime(&f.l.join("stary.txt"), 1_000_000_000);
    set_mtime(&f.cloud.join("stary.txt"), 1_000_000_000);
    f.run(&j);
    write(&f.l.join("stary.txt"), "v22"); // nowa wersja, ale mtime nadal sprzed lat
    set_mtime(&f.l.join("stary.txt"), 1_100_000_000);
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    let tr = trash_list(&j).unwrap();
    assert!(
        tr.iter().any(|i| i.path == "stary.txt"),
        "poprzednia wersja zniknęła z kosza od razu: {tr:?}"
    );
}

#[test]
#[ignore = "BUG: plik o nazwie zaczynającej się od '.syncron-trash' (np. .syncron-trashcan.txt) jest pomijany w listingu rclone (starts_with bez granicy ścieżki)"]
fn r2_trash_prefix_filename_is_listed() {
    let f = fx();
    write(&f.cloud.join(".syncron-trashcan.txt"), "x");
    let j = f.job("fake:", json!({}));
    let l = Location::parse("fake:").list(&rules_of(&j)).unwrap();
    assert!(
        l.files.contains_key(".syncron-trashcan.txt"),
        "{:?}",
        l.files.keys()
    );
}

#[test]
#[ignore = "BUG: stderr zawierający słowo 'locked' w nazwie pliku (np. 'unlocked.txt') przy zwykłym błędzie jest klasyfikowany jako Busy ('plik zajęty')"]
fn r2_busy_classification_not_fooled_by_filename() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    write(&f.l.join("unlocked.txt"), "abc");
    f.rules(json!([{ "op": "copyto", "stderr": "Failed to copyto: googleapi: Error 403: storageQuotaExceeded (unlocked.txt)", "code": 1 }]));
    let rep = f.run(&j);
    assert!(
        rep.busy.is_empty(),
        "błąd limitu miejsca pokazany jako 'plik zajęty': {rep:?}"
    );
    assert_eq!(rep.errors.len(), 1);
}

#[test]
#[ignore = "BUG: rclone kończy się kodem != 0 z pustym stderr -> pusty komunikat błędu ('lewa strona: ' / 'plik: ')"]
fn r2_empty_stderr_gives_informative_message() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.rules(json!([{ "op": "lsjson", "code": 1 }]));
    let e = run_job(&j, &f.state, false, false).unwrap_err().to_string();
    assert!(
        e.trim_end_matches(&[':', ' '][..]).len() > "prawa strona".len(),
        "komunikat bez treści: {e:?}"
    );
}

#[test]
fn r2_size_minus_one_for_known_file_must_not_delete_other_side() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    seed_synced(&f, &j);
    // zdalny listing przez chwilę zwraca f3 bez rozmiaru
    let mut v: Vec<Value> = Vec::new();
    for i in 0..6 {
        let st = fs::metadata(f.cloud.join(format!("f{i}.txt"))).unwrap();
        let mt = filetime::FileTime::from_last_modification_time(&st).unix_seconds();
        let mt = chrono::DateTime::from_timestamp(mt, 0)
            .unwrap()
            .to_rfc3339();
        v.push(json!({"Path": format!("f{i}.txt"), "Size": if i == 3 { -1 } else { st.len() as i64 }, "ModTime": mt, "IsDir": false}));
    }
    f.rules(json!([{ "op": "lsjson", "match": "-R", "stdout": Value::Array(v).to_string() }]));
    let rep = f.run(&j);
    assert!(
        f.l.join("f3.txt").exists(),
        "lewy plik f3 usunięty/zakoszony: {rep:?}"
    );
}

#[test]
#[ignore = "BUG: brak ModTime w lsjson -> mtime = 'teraz', więc przy każdym przebiegu plik wygląda na zmieniony i jest kopiowany z powrotem"]
fn r2_missing_modtime_is_stable_between_runs() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.ctrl_set(json!({"omit_modtime": true}));
    write(&f.l.join("a.txt"), "a");
    f.run(&j);
    std::thread::sleep(Duration::from_millis(3200));
    let rep = f.run(&j);
    assert!(
        rep.planned.is_empty(),
        "kolejny przebieg bez zmian ma działania: {:?}",
        rep.planned
    );
}

#[test]
#[ignore = "BUG: run_rclone nie ma limitu czasu — zawieszony rclone blokuje run_job w nieskończoność"]
fn r2_hanging_rclone_does_not_block_forever() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.rules(json!([{ "op": "lsjson", "sleep": 12, "times": 1 }]));
    let (tx, rx) = std::sync::mpsc::channel();
    let state = f.state.clone();
    std::thread::spawn(move || {
        let _ = tx.send(run_job(&j, &state, false, false).is_ok());
    });
    // oczekiwanie rozsądne: kilka sekund (prawdziwy limit powinien być znacznie krótszy niż 12 s dla testu)
    assert!(
        rx.recv_timeout(Duration::from_secs(5)).is_ok(),
        "run_job nadal czeka na zawieszony rclone po 5 s (brak timeoutu)"
    );
}

// ======================= RUNDA 3: obciążenie i powtarzalność =======================

#[test]
fn r3_listing_50k_performance() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.rules(json!([{ "op": "lsjson", "synth": 50000 }]));
    let before = rss_kb();
    let t = Instant::now();
    let l = Location::parse("fake:").list(&rules_of(&j)).unwrap();
    let el = t.elapsed();
    assert_eq!(l.files.len(), 50_000);
    eprintln!("50k listing: {el:?}, VmHWM {} -> {} kB", before, rss_kb());
    assert!(el < Duration::from_secs(20), "{el:?}");
    assert!(rss_kb() < 600_000, "pamięć: {} kB", rss_kb());
}

#[test]
fn r3_dry_run_50k_remote_vs_empty_local() {
    let f = fx();
    let j = f.job("fake:", json!({"threads": 4}));
    f.rules(json!([{ "op": "lsjson", "match": "-R", "synth": 50000 }]));
    let t = Instant::now();
    let rep = run_job(&j, &f.state, true, false).unwrap();
    let el = t.elapsed();
    eprintln!("dry-run 50k: {el:?}");
    assert_eq!(rep.planned.len(), 50_000);
    assert!(el < Duration::from_secs(30), "{el:?}");
}

#[test]
fn r3_detect_moves_scales_with_many_candidates() {
    // lokalnie: 20k "starych" wpisów w bazie usuniętych po lewej + 20k nowych plików o tym samym rozmiarze i czasie
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (t.path().join("L"), t.path().join("R"), t.path().join("s"));
    fs::create_dir_all(l.join("new")).unwrap();
    fs::create_dir_all(r.join("old")).unwrap();
    let n = 20_000;
    let db = syncron_core::engine::open_db(&s).unwrap();
    db.execute_batch("BEGIN").unwrap();
    for i in 0..n {
        let rel = format!("old/f{i}.txt");
        let p = r.join(&rel);
        fs::write(&p, "0123456789").unwrap();
        set_mtime(&p, 1_700_000_000);
        db.execute(
            "INSERT INTO base(job,key,lsize,lmtime,rsize,rmtime) VALUES('q8',?1,10,1700000000,10,1700000000)",
            [&rel],
        )
        .unwrap();
        let p = l.join(format!("new/f{i}.txt"));
        fs::write(&p, "0123456789").unwrap();
        set_mtime(&p, 1_700_000_000);
    }
    db.execute_batch("COMMIT").unwrap();
    drop(db);
    let j = job_between(
        l.to_str().unwrap(),
        r.to_str().unwrap(),
        json!({"guard_pct": 100}),
    );
    let t0 = Instant::now();
    let rep = run_job(&j, &s, true, false).unwrap();
    let el = t0.elapsed();
    eprintln!(
        "planowanie przeniesień {n}+{n}: {el:?}, akcji {}",
        rep.planned.len()
    );
    assert!(el < Duration::from_secs(2), "planowanie O(n^2): {el:?}");
}

#[test]
fn r3_parallel_transfers_are_concurrent_and_complete() {
    let f = fx();
    std::env::set_var("FAKE_RCLONE_DELAY", "0.25");
    let j = f.job("fake:", json!({"threads": 8}));
    for i in 0..24 {
        write(&f.l.join(format!("p{i}.txt")), &format!("{i}"));
    }
    let t = Instant::now();
    let rep = f.run(&j);
    let el = t.elapsed();
    std::env::remove_var("FAKE_RCLONE_DELAY");
    assert_eq!(rep.copied, 24, "{rep:?}");
    // sekwencyjnie: >= 24 * 2 wywołania * 0.25 s = 12 s
    assert!(el < Duration::from_secs(9), "brak równoległości: {el:?}");
    for i in 0..24 {
        assert_eq!(
            fs::read_to_string(f.cloud.join(format!("p{i}.txt"))).unwrap(),
            format!("{i}")
        );
    }
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn r3_two_jobs_in_parallel_threads() {
    let f = fx();
    let (l1, l2) = (f.l.join("one"), f.l.join("two"));
    for (l, tag) in [(&l1, "one"), (&l2, "two")] {
        for i in 0..8 {
            write(&l.join(format!("{tag}{i}.txt")), tag);
        }
    }
    let mk = |id: &str, l: &Path, r: &str| {
        job_between(l.to_str().unwrap(), r, json!({"id": id, "threads": 3}))
    };
    let (j1, j2) = (mk("j1", &l1, "fake:j1"), mk("j2", &l2, "fake:j2"));
    let st = f.state.clone();
    drop(syncron_core::engine::open_db(&st).unwrap());
    std::thread::scope(|s| {
        let a = s.spawn(|| run_job(&j1, &st, false, false).unwrap());
        let b = s.spawn(|| run_job(&j2, &st, false, false).unwrap());
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert_eq!((a.copied, b.copied), (8, 8), "{a:?} {b:?}");
    });
    assert_eq!(fs::read_dir(f.cloud.join("j1")).unwrap().count(), 8);
    assert_eq!(fs::read_dir(f.cloud.join("j2")).unwrap().count(), 8);
}

#[test]
fn r3_same_job_twice_concurrently_does_not_corrupt() {
    let f = fx();
    let j = f.job("fake:", json!({"threads": 3}));
    // baza istnieje wcześniej (wyścig przy jej tworzeniu to osobny test r3_two_jobs_first_run_...)
    drop(syncron_core::engine::open_db(&f.state).unwrap());
    for i in 0..10 {
        write(&f.l.join(format!("s{i}.txt")), "same");
    }
    std::thread::scope(|s| {
        let a = s.spawn(|| run_job(&j, &f.state, false, false));
        let b = s.spawn(|| run_job(&j, &f.state, false, false));
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert!(a.is_ok() && b.is_ok(), "{a:?} {b:?}");
    });
    for i in 0..10 {
        assert_eq!(
            fs::read_to_string(f.cloud.join(format!("s{i}.txt"))).unwrap(),
            "same"
        );
    }
    let rep = f.run(&j);
    assert!(rep.errors.is_empty() && rep.conflicts.is_empty(), "{rep:?}");
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn r3_slow_listing_is_tolerated() {
    let f = fx();
    let j = f.job("fake:", json!({}));
    f.rules(json!([{ "op": "lsjson", "sleep": 1.5 }]));
    write(&f.l.join("a.txt"), "a");
    let t = Instant::now();
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert!(t.elapsed() >= Duration::from_millis(1500));
}

#[test]
fn r3_repeatability_flaky_remote_converges() {
    // co trzecie wywołanie rclone zawodzi (zmiennie busy/zwykły błąd); po kilku przebiegach wszystko zsynchronizowane
    let f = fx();
    let j = f.job("fake:", json!({"threads": 1}));
    for i in 0..12 {
        write(&f.l.join(format!("r{i}.txt")), &format!("{i}"));
    }
    f.rules(json!([
        {"op": "copyto", "skip": 1, "times": 1, "stderr": "used by another process", "code": 1},
        {"op": "copyto", "skip": 5, "times": 1, "stderr": "i/o timeout", "code": 1},
        {"op": "lsjson", "match": "r1.txt", "skip": 0, "times": 1, "stderr": "oops", "code": 1}
    ]));
    let mut ok = false;
    for _ in 0..6 {
        let rep = run_job(&j, &f.state, false, false);
        if let Ok(r) = rep {
            if !r.needs_retry() && r.planned.is_empty() {
                ok = true;
                break;
            }
        }
    }
    assert!(ok);
    for i in 0..12 {
        assert_eq!(
            fs::read_to_string(f.cloud.join(format!("r{i}.txt"))).unwrap(),
            format!("{i}")
        );
    }
}

#[test]
fn r3_two_jobs_first_run_share_fresh_state_db() {
    // dwa zadania startują jednocześnie na świeżym katalogu stanu (tworzenie bazy + WAL)
    let mut failures = Vec::new();
    for round in 0..40 {
        let t = tempfile::tempdir().unwrap();
        let st = t.path().join("state");
        let mut js = Vec::new();
        for k in 0..2 {
            let (l, r) = (
                t.path().join(format!("L{k}")),
                t.path().join(format!("R{k}")),
            );
            fs::create_dir_all(&l).unwrap();
            fs::create_dir_all(&r).unwrap();
            write(&l.join("a.txt"), "a");
            js.push(job_between(
                l.to_str().unwrap(),
                r.to_str().unwrap(),
                json!({"id": format!("j{k}")}),
            ));
        }
        let bar = std::sync::Barrier::new(2);
        std::thread::scope(|s| {
            let hs: Vec<_> = js
                .iter()
                .map(|j| {
                    s.spawn(|| {
                        bar.wait();
                        run_job(j, &st, false, false).map(|r| r.copied)
                    })
                })
                .collect();
            for h in hs {
                if let Err(e) = h.join().unwrap() {
                    failures.push(format!("runda {round}: {e}"));
                }
            }
        });
    }
    assert!(
        failures.is_empty(),
        "{} z 80 przebiegów padło: {:?}",
        failures.len(),
        &failures[..failures.len().min(3)]
    );
}

#[test]
fn r3_run_twice_identical_results() {
    for _ in 0..3 {
        let f = fx();
        let j = f.job("fake:Mój Dysk/ąę", json!({}));
        write(&f.l.join("A/b c.txt"), "1");
        write(&f.cloud.join("Mój Dysk/ąę/Ł.txt"), "2");
        let r1 = f.run(&j);
        assert_eq!((r1.copied, r1.deleted, r1.moved), (2, 0, 0), "{r1:?}");
        assert!(f.run(&j).planned.is_empty());
    }
}
