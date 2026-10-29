//! Silnik na natywnej warstwie zdalnej (backend `mem`): scenariusze przeniesione z dawnych testów rclone
//! (`rclone.rs`, `qa_rclone.rs`, `exp7_rclone.rs`, fragmenty `qa_bezpieczenstwo.rs`/`qa_nazwy.rs`) oraz nowe:
//! round-trip, kosz, przeniesienia, błąd listingu = przerwanie bez usuwania, ścieżki ze zdalnego listingu,
//! nazwy NFD/polskie/długie, wiele wątków, wstrzykiwanie błędów (kolizje, zajętość, brak miejsca, logowanie).
mod common;
use common::*;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use syncron_core::engine::{run_job, trash_list, trash_restore, Report};
use syncron_core::location::{Location, Opts};
use syncron_core::model::Job;
use syncron_core::remote::mem::Faults;
use syncron_core::rules::RuleSet;

struct Fx {
    _t: tempfile::TempDir,
    l: PathBuf,
    state: PathBuf,
    cloud: Mem,
}

fn fx(tag: &str) -> Fx {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().to_path_buf();
    let f = Fx {
        l: p.join("L"),
        state: p.join("state"),
        cloud: Mem::new(tag),
        _t: t,
    };
    fs::create_dir_all(&f.l).unwrap();
    f
}

impl Fx {
    fn job(&self, root: &str, extra: Value) -> Job {
        job_between(self.l.to_str().unwrap(), &self.cloud.spec(root), extra)
    }
    fn run(&self, j: &Job) -> Report {
        run_job(j, &self.state, false, false).unwrap()
    }
    /// Synchronizuje 6 plików lokalnych do chmury (stan bazowy). Zwraca zadanie.
    fn seed(&self, j: &Job) {
        for i in 0..6 {
            write(&self.l.join(format!("f{i}.txt")), &format!("tresc {i}"));
        }
        let rep = self.run(j);
        assert_eq!(rep.copied, 6, "{rep:?}");
        assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
    }
}

fn is_remote(s: &str) -> bool {
    matches!(Location::parse(s), Location::Remote { .. })
}

// ======================= podstawy =======================

#[test]
fn parse_table() {
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
        assert!(is_remote(s), "powinno być zdalne: {s:?}");
    }
    let local = [
        "C:\\Dane",
        "C:/Dane",
        "c:",
        "C:",
        "D:\\",
        "C:foo",
        "\\\\nas\\udział",
        "//server/share",
        "//server/share/a:b",
        "/Volumes/Dysk",
        "/mnt/a:b/c",
        "relative/dir",
        "",
        ":x",
        "x",
        "a/b:c",
        "C:\\Dane\\a:b",
        "dysk\\folder:x",
        // składnia łączenia rclone nie jest rozpoznawana: to folder lokalny (celowo)
        "gdrive,drive_id=x:folder",
    ];
    for s in local {
        assert!(!is_remote(s), "powinno być lokalne: {s:?}");
    }
    match Location::parse("  gdrive:x  ") {
        Location::Remote { account, root } => {
            assert_eq!((account.as_str(), root.as_str()), ("gdrive", "x"))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn parse_nfd_account_name_is_normalized_to_nfc() {
    use unicode_normalization::UnicodeNormalization;
    let nfd: String = "Dysk Zażółć:folder".nfd().collect();
    match Location::parse(&nfd) {
        Location::Remote { account, root } => {
            assert_eq!(account, "Dysk Zażółć");
            assert_eq!(root, "folder");
            assert!(account.nfc().eq(account.chars()));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(Location::parse("ż:\\x"), Location::Local(_)));
}

#[test]
fn old_job_locations_keep_working_without_changes() {
    // zapis „konto:ścieżka” z jobs.json sprzed wersji z kontami natywnymi to ten sam format
    let f = fx("stary");
    let j = f.job("Mój folder/ąę", json!({}));
    assert!(j.right.contains(':'));
    write(&f.l.join("a.txt"), "a");
    assert_eq!(f.run(&j).copied, 1);
    assert!(f.cloud.has("Mój folder/ąę/a.txt"));
}

#[test]
fn end_to_end_roundtrip_trash_move_delete_restore() {
    let f = fx("rt");
    let j = f.job("Mój folder/ąę", json!({}));
    write(&f.l.join("Zażółć/gęślą jaźń.txt"), "z lewej");
    f.cloud
        .put("Mój folder/ąę/Łódź.txt", "z prawej", 1_700_000_000);
    let rep = f.run(&j);
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(
        f.cloud.get("Mój folder/ąę/Zażółć/gęślą jaźń.txt").unwrap(),
        "z lewej"
    );
    assert_eq!(
        fs::read_to_string(f.l.join("Łódź.txt")).unwrap(),
        "z prawej"
    );
    // mtime zdalnego pliku zachowany po pobraniu
    let m = fs::metadata(f.l.join("Łódź.txt")).unwrap();
    let secs = filetime::FileTime::from_last_modification_time(&m).unix_seconds();
    assert_eq!(secs, 1_700_000_000);
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

    // przeniesienie: backend bez skrótów -> nie zgadujemy, kopia + usunięcie
    fs::create_dir_all(f.l.join("Nowy")).unwrap();
    fs::rename(f.l.join("Łódź.txt"), f.l.join("Nowy/Łódź 2.txt")).unwrap();
    let mut j2 = j.clone();
    j2.guard_pct = 100;
    let rep = f.run(&j2);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (0, 1, 1), "{rep:?}");
    assert!(f.cloud.has("Mój folder/ąę/Nowy/Łódź 2.txt"));
    assert!(!f.cloud.has("Mój folder/ąę/Łódź.txt"));

    // usunięcie -> kosz po prawej, przywrócenie
    for i in 0..6 {
        write(&f.l.join(format!("p{i}.txt")), "x");
    }
    f.run(&j);
    fs::remove_file(f.l.join("p0.txt")).unwrap();
    let rep = f.run(&j);
    assert_eq!(rep.deleted, 1, "{rep:?}");
    assert!(!f.cloud.has("Mój folder/ąę/p0.txt"));
    let tr = trash_list(&j).unwrap();
    let it = tr.iter().find(|i| i.path == "p0.txt").expect("kosz");
    let back = trash_restore(&j, "right", &it.stamp, &it.path).unwrap();
    assert_eq!(back, "p0.txt");
    assert!(f.cloud.has("Mój folder/ąę/p0.txt"));
}

#[test]
fn first_run_with_missing_remote_dir_creates_it() {
    let f = fx("nowy");
    let j = f.job("nowy/katalog z spacją", json!({}));
    write(&f.l.join("a.txt"), "a");
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert!(f.cloud.has("nowy/katalog z spacją/a.txt"));
}

#[test]
fn polish_nfd_and_case_collisions_in_remote_listing() {
    use unicode_normalization::UnicodeNormalization;
    let f = fx("kol");
    let nfd: String = "żółć.txt".nfd().collect();
    f.cloud.put("żółć.txt", "1", 1_700_000_000);
    f.cloud.put(&nfd, "2", 1_700_000_000);
    f.cloud.put("Dup.TXT", "1", 1_700_000_000);
    f.cloud.put("dup.txt", "1", 1_700_000_000);
    f.cloud.put("dir/in side.txt", "", 1_700_000_000);
    f.cloud.put(".syncron-trash/x/y.txt", "1", 1_700_000_000);
    f.cloud.put("~$tmp.docx", "1", 1_700_000_000);
    f.cloud.put(".syncron-trashcan.txt", "1", 1_700_000_000);
    let l = Location::parse(&f.cloud.spec(""))
        .list(&RuleSet::default())
        .unwrap();
    assert_eq!(l.collisions.len(), 2, "{:?}", l.collisions);
    assert!(l.files.contains_key("dir/in side.txt"));
    assert_eq!(l.files["dir/in side.txt"].meta.size, 0);
    assert!(
        l.files.contains_key(".syncron-trashcan.txt"),
        "{:?}",
        l.files.keys()
    );
    assert!(!l
        .files
        .keys()
        .any(|k| k.contains("syncron-trash/") || k.contains("tmp.docx")));
}

#[test]
fn long_names_and_deep_paths() {
    let f = fx("dlug");
    let j = f.job("d", json!({}));
    // 255 B w komponencie (ograniczenie większości systemów plików) i głęboka ścieżka
    let name = format!("{}.txt", "ą".repeat(120));
    assert!(name.len() <= 255);
    let deep = (0..30)
        .map(|i| format!("kat{i}"))
        .collect::<Vec<_>>()
        .join("/");
    write(&f.l.join(&name), "dlugi");
    write(&f.l.join(&deep).join("g.txt"), "gleboko");
    let rep = f.run(&j);
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(f.cloud.get(&format!("d/{name}")).unwrap(), "dlugi");
    assert_eq!(f.cloud.get(&format!("d/{deep}/g.txt")).unwrap(), "gleboko");
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn nasty_names_travel_as_single_names() {
    let f = fx("dziw");
    let j = f.job("bitrix dir/a b", json!({}));
    let names = [
        "a b.txt",
        "it's \"q\".txt",
        "$HOME `id` $(x).txt",
        "-rf.txt",
        "--delete-excluded.txt",
        "[x]?*.txt",
        "dwukropek: tak.txt",
        " spacja z przodu.txt",
        "a;b|c&d>e.txt",
        "Zażółć/--flag/jaźń.txt",
    ];
    for n in names {
        write(&f.l.join(n), &format!("l:{n}"));
    }
    f.cloud
        .put("bitrix dir/a b/-z-prawej.txt", "r", 1_700_000_000);
    let rep = f.run(&j);
    assert!(rep.errors.is_empty() && rep.busy.is_empty(), "{rep:?}");
    for n in names {
        assert_eq!(
            f.cloud.get(&format!("bitrix dir/a b/{n}")).unwrap(),
            format!("l:{n}")
        );
    }
    assert_eq!(fs::read_to_string(f.l.join("-z-prawej.txt")).unwrap(), "r");
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn control_chars_in_names_are_rejected_not_sent() {
    let f = fx("ster");
    let j = f.job("", json!({}));
    write(&f.l.join("nowa\nlinia.txt"), "x");
    write(&f.l.join("ok.txt"), "ok");
    let rep = f.run(&j);
    assert!(f.cloud.has("ok.txt"), "{rep:?}");
    assert!(
        !f.cloud.keys().iter().any(|k| k.contains('\n')),
        "nazwa ze znakiem sterującym nie może trafić na serwer"
    );
    assert_eq!(rep.errors.len(), 1, "{rep:?}");
}

#[test]
fn missing_mtime_is_stable_between_runs() {
    let f = fx("mt0");
    let j = f.job("", json!({}));
    write(&f.l.join("a.txt"), "a");
    f.run(&j);
    // serwer nie podaje czasu modyfikacji: stała wartość 0 (stabilna między przebiegami)
    let key = "a.txt";
    f.cloud.put(key, "a", 0);
    std::thread::sleep(Duration::from_millis(1200));
    let rep = f.run(&j);
    let rep2 = f.run(&j);
    assert!(rep2.planned.is_empty(), "{rep:?} / {rep2:?}");
}

// ======================= błędy listingu i bezpieczeństwo =======================

#[test]
fn listing_error_aborts_without_changes() {
    let f = fx("lerr");
    let j = f.job("", json!({}));
    f.seed(&j);
    for fault in [
        Faults {
            fail_list: true,
            ..Default::default()
        },
        Faults {
            auth_fail: true,
            ..Default::default()
        },
    ] {
        f.cloud.faults(fault);
        let e = run_job(&j, &f.state, false, false);
        assert!(e.is_err(), "błąd listingu musi przerwać przebieg: {e:?}");
        assert_eq!(f.cloud.live_keys().len(), 6);
        assert_eq!(fs::read_dir(&f.l).unwrap().count(), 6);
    }
    f.cloud.clear_faults();
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn listing_error_on_left_remote_side_also_aborts() {
    // zdalna strona po LEWEJ: ta sama zasada
    let t = tempfile::tempdir().unwrap();
    let (r, s) = (t.path().join("R"), t.path().join("s"));
    fs::create_dir_all(&r).unwrap();
    let cloud = Mem::new("lewa");
    for i in 0..6 {
        cloud.put(&format!("f{i}.txt"), "x", 1_700_000_000);
    }
    let j = job_between(&cloud.spec(""), r.to_str().unwrap(), json!({}));
    assert_eq!(run_job(&j, &s, false, false).unwrap().copied, 6);
    cloud.faults(Faults {
        fail_list: true,
        ..Default::default()
    });
    assert!(run_job(&j, &s, false, false).is_err());
    assert_eq!(fs::read_dir(&r).unwrap().count(), 6);
}

#[test]
fn auth_error_message_tells_to_log_in_again() {
    let f = fx("auth");
    let j = f.job("", json!({}));
    f.cloud.faults(Faults {
        auth_fail: true,
        ..Default::default()
    });
    let e = run_job(&j, &f.state, false, false).unwrap_err().to_string();
    assert!(e.to_lowercase().contains("zaloguj"), "{e}");
    assert!(e.contains(&f.cloud.name), "{e}");
}

#[test]
fn directory_gone_on_previously_filled_side_blocks() {
    let f = fx("znika");
    let j = f.job("", json!({}));
    f.seed(&j);
    // folder zdalny znika (pusty listing): zabezpieczenia blokują, nic nie jest usuwane po lewej
    f.cloud.wipe();
    let rep = f.run(&j);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!((rep.copied, rep.deleted), (0, 0));
    assert_eq!(fs::read_dir(&f.l).unwrap().count(), 6);
    // lewa strona opróżniona, prawa pełna: też blokada
    for i in 0..6 {
        f.cloud.put(&format!("f{i}.txt"), "tresc", 1_700_000_000);
    }
    for e in fs::read_dir(&f.l).unwrap().flatten() {
        fs::remove_file(e.path()).unwrap();
    }
    let rep = f.run(&j);
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert_eq!(f.cloud.live_keys().len(), 6);
}

#[test]
fn remote_listing_with_unsafe_paths_never_escapes_local_root() {
    let f = fx("trav");
    let base = f._t.path().to_path_buf();
    let left = base.join("a/L");
    fs::create_dir_all(&left).unwrap();
    fs::write(base.join("evil.txt"), "zlosliwe").unwrap();
    for (k, _) in [
        ("../evil.txt", 0),
        ("a/../../escape2.txt", 0),
        ("/abs_zdalny/x.txt", 0),
        ("dobry/./x.txt", 0),
        ("a//b.txt", 0),
    ] {
        f.cloud.put(k, "z-serwera", 1_700_000_000);
    }
    f.cloud.put("ok.txt", "z-serwera", 1_700_000_000);
    let loc = Location::parse(&f.cloud.spec(""));
    let l = loc.list(&RuleSet::default()).unwrap();
    assert_eq!(l.files.keys().collect::<Vec<_>>(), ["ok.txt"]);
    assert_eq!(l.skipped.len(), 5, "{:?}", l.skipped);
    let j = job_between(
        left.to_str().unwrap(),
        &f.cloud.spec(""),
        json!({"mode": "RightToLeft"}),
    );
    let rep = run_job(&j, &f.state, false, false).unwrap();
    assert!(left.join("ok.txt").exists(), "{rep:?}");
    assert!(!base.join("a/evil.txt").exists());
    assert!(!base.join("escape2.txt").exists() && !base.join("a/escape2.txt").exists());
    assert!(!std::path::Path::new("/abs_zdalny/x.txt").exists());
    assert!(!left.join("abs_zdalny").exists());
}

#[test]
fn trash_stamp_and_rel_from_ui_cannot_escape() {
    let f = fx("kosz1");
    let loc = Location::parse(&f.cloud.spec("r"));
    f.cloud.put("r/.syncron-trash/s/a.txt", "x", 1_700_000_000);
    f.cloud.put("r/secret.txt", "tajne", 1_700_000_000);
    assert!(loc.restore_trash("..", "secret.txt").is_err());
    assert!(loc.restore_trash("s", "../secret.txt").is_err());
    assert!(loc.delete_trash(Some("../x"), None).is_err());
    assert!(loc.delete_trash(None, Some("a.txt")).is_err());
    assert!(f.cloud.has("r/secret.txt"));
}

// ======================= błędy operacji =======================

#[test]
fn copy_failure_kinds_are_safe_and_retried() {
    // (błąd, czy zajętość)
    let kinds: Vec<(&str, Faults, bool)> = vec![
        (
            "zajęty",
            Faults {
                busy_paths: vec!["nowy plik".into()],
                ..Default::default()
            },
            true,
        ),
        (
            "brak miejsca",
            Faults {
                fail_write_paths: vec!["nowy plik".into()],
                ..Default::default()
            },
            false,
        ),
        (
            "sieć",
            Faults {
                fail_upload_on: vec![0, 1, 2, 3],
                ..Default::default()
            },
            false,
        ),
    ];
    for (name, fault, busy) in kinds {
        let f = fx("bld");
        let j = f.job("", json!({}));
        write(&f.l.join("nowy plik.txt"), "abc");
        f.cloud.faults(fault);
        let rep = f.run(&j);
        assert_eq!(rep.copied, 0, "{name}: {rep:?}");
        assert!(!f.cloud.has("nowy plik.txt"), "{name}");
        assert!(rep.needs_retry());
        assert_eq!(
            (rep.busy.len(), rep.errors.len()),
            if busy { (1, 0) } else { (0, 1) },
            "{name}: {rep:?}"
        );
        assert!(f.l.join("nowy plik.txt").exists());
        f.cloud.clear_faults();
        let rep = f.run(&j);
        assert_eq!(rep.copied, 1, "{name}: {rep:?}");
        assert!(f.cloud.has("nowy plik.txt"));
    }
}

#[test]
fn busy_classification_comes_from_error_kind_not_file_name() {
    let f = fx("zajn");
    let j = f.job("", json!({}));
    write(&f.l.join("unlocked.txt"), "abc");
    f.cloud.faults(Faults {
        fail_write_paths: vec!["unlocked".into()],
        ..Default::default()
    });
    let rep = f.run(&j);
    assert!(
        rep.busy.is_empty(),
        "limit miejsca pokazany jako zajętość: {rep:?}"
    );
    assert_eq!(rep.errors.len(), 1);
}

#[test]
fn backup_failure_prevents_overwrite() {
    let f = fx("bkp");
    let j = f.job("", json!({}));
    f.seed(&j);
    write(&f.l.join("f0.txt"), "NOWA wersja");
    set_mtime(&f.l.join("f0.txt"), 2_000_000_000);
    f.cloud.faults(Faults {
        fail_write_paths: vec![".syncron-trash".into()],
        ..Default::default()
    });
    let rep = f.run(&j);
    assert_eq!(rep.copied, 0, "{rep:?}");
    assert_eq!(rep.errors.len(), 1);
    assert_eq!(
        f.cloud.get("f0.txt").unwrap(),
        "tresc 0",
        "stara wersja nie może zniknąć bez kopii w koszu"
    );
    f.cloud.clear_faults();
    let rep = f.run(&j);
    assert_eq!(rep.copied, 1, "{rep:?}");
    assert_eq!(f.cloud.get("f0.txt").unwrap(), "NOWA wersja");
}

#[test]
fn copy_failure_after_backup_keeps_target() {
    let f = fx("pobkp");
    let j = f.job("", json!({}));
    f.seed(&j);
    write(&f.l.join("f1.txt"), "NOWA");
    set_mtime(&f.l.join("f1.txt"), 2_000_000_000);
    // kopia do kosza przechodzi (wysłanie nr 0), właściwe wysłanie (nr 1..) zawodzi
    f.cloud.faults(Faults {
        fail_upload_on: vec![1, 2, 3, 4],
        ..Default::default()
    });
    let rep = f.run(&j);
    assert_eq!(rep.errors.len(), 1, "{rep:?}");
    assert_eq!(f.cloud.get("f1.txt").unwrap(), "tresc 1");
    assert_eq!(fs::read_to_string(f.l.join("f1.txt")).unwrap(), "NOWA");
}

#[test]
fn delete_failure_busy_does_not_resurrect_or_lose() {
    for (name, extra) in [("kosz", json!({})), ("bez kosza", json!({"trash_days": 0}))] {
        let f = fx("del");
        let j = f.job("", extra);
        f.seed(&j);
        fs::remove_file(f.l.join("f0.txt")).unwrap();
        f.cloud.faults(Faults {
            busy_paths: vec!["f0.txt".into()],
            ..Default::default()
        });
        let rep = f.run(&j);
        assert_eq!(rep.deleted, 0, "{name}: {rep:?}");
        assert_eq!(rep.busy.len(), 1, "{name}: {rep:?}");
        assert!(f.cloud.has("f0.txt"), "{name}");
        assert!(
            !f.l.join("f0.txt").exists(),
            "{name}: plik nie może wrócić na lewo"
        );
        f.cloud.clear_faults();
        let rep = f.run(&j);
        assert_eq!(rep.deleted, 1, "{name}: {rep:?}");
        assert!(!f.cloud.has("f0.txt"));
        assert!(!f.l.join("f0.txt").exists());
    }
}

#[test]
fn server_side_rename_failure_falls_back_to_copy_and_delete() {
    let f = fx("rnf");
    let loc = Location::parse(&f.cloud.spec("r"));
    f.cloud.put("r/a/x.txt", "zawartosc", 1_700_000_000);
    f.cloud.faults(Faults {
        fail_rename: true,
        ..Default::default()
    });
    loc.remove("a/x.txt", "2026-01-02_030405").unwrap();
    assert!(!f.cloud.has("r/a/x.txt"));
    assert_eq!(
        f.cloud
            .get("r/.syncron-trash/2026-01-02_030405/a/x.txt")
            .unwrap(),
        "zawartosc"
    );
    // ta sama zasada przy zwykłej zmianie nazwy w silniku: Location::rename zgłasza błąd, nic nie ginie
    assert!(loc.rename("a/y.txt", "a/z.txt").is_err());
}

#[test]
fn trash_slots_never_overwrite_existing_versions() {
    let f = fx("slot");
    let loc = Location::parse(&f.cloud.spec(""));
    let stamp = "2026-01-02_030405";
    f.cloud.put("a.txt", "v1", 1_700_000_000);
    loc.backup("a.txt", stamp).unwrap();
    f.cloud.put("a.txt", "v2", 1_700_000_001);
    loc.backup("a.txt", stamp).unwrap();
    f.cloud.put("a.txt", "v3", 1_700_000_002);
    loc.remove("a.txt", stamp).unwrap();
    assert_eq!(
        f.cloud
            .get(".syncron-trash/2026-01-02_030405/a.txt")
            .unwrap(),
        "v1"
    );
    assert_eq!(
        f.cloud
            .get(".syncron-trash/2026-01-02_030405-1/a.txt")
            .unwrap(),
        "v2"
    );
    assert_eq!(
        f.cloud
            .get(".syncron-trash/2026-01-02_030405-2/a.txt")
            .unwrap(),
        "v3"
    );
    assert!(!f.cloud.has("a.txt"));
}

#[test]
fn trash_check_error_is_not_treated_as_free_slot() {
    let f = fx("slot2");
    let loc = Location::parse(&f.cloud.spec(""));
    f.cloud.put("a.txt", "v1", 1_700_000_000);
    f.cloud.faults(Faults {
        fail_stat: true,
        ..Default::default()
    });
    assert!(loc.backup("a.txt", "2026-01-02_030405").is_err());
    assert!(loc.remove("a.txt", "2026-01-02_030405").is_err());
    f.cloud.clear_faults();
    assert_eq!(f.cloud.get("a.txt").unwrap(), "v1");
}

#[test]
fn unknown_existence_never_deletes_the_other_side() {
    // plik znika z listingu, a stat albo go widzi (niespójny listing), albo zawodzi (błąd sieci)
    for (name, fault) in [
        (
            "listing pomija plik",
            Faults {
                hide_from_list: vec!["f3.txt".into()],
                ..Default::default()
            },
        ),
        (
            "stat zawodzi",
            Faults {
                fail_stat: true,
                hide_from_list: vec!["f3.txt".into()],
                ..Default::default()
            },
        ),
    ] {
        let f = fx("niesp");
        let j = f.job("", json!({}));
        f.seed(&j);
        f.cloud.faults(fault);
        let res = run_job(&j, &f.state, false, false);
        assert!(
            f.l.join("f3.txt").exists() && f.cloud.has("f3.txt"),
            "{name}: f3 usunięty: {res:?}"
        );
    }
}

#[test]
fn upload_size_mismatch_is_an_error_and_leaves_no_bad_copy() {
    let f = fx("rozm");
    let j = f.job("", json!({}));
    write(&f.l.join("a.txt"), "dluzsza tresc");
    f.cloud.faults(Faults {
        truncate_upload: true,
        ..Default::default()
    });
    let rep = f.run(&j);
    assert_eq!(rep.copied, 0, "{rep:?}");
    assert_eq!(rep.errors.len(), 1, "{rep:?}");
    assert!(
        !f.cloud.has("a.txt"),
        "uszkodzona kopia nie może zostać na serwerze"
    );
    assert_eq!(
        fs::read_to_string(f.l.join("a.txt")).unwrap(),
        "dluzsza tresc"
    );
    f.cloud.clear_faults();
    assert_eq!(f.run(&j).copied, 1);
}

#[test]
fn verify_option_compares_server_hash() {
    let f = fx("weryf");
    f.cloud.hashes(true);
    let j = f.job("", json!({"verify": true}));
    write(&f.l.join("a.txt"), "tresc do weryfikacji");
    f.cloud.put("b.txt", "z serwera", 1_700_000_000);
    let rep = f.run(&j);
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn flaky_remote_converges() {
    let f = fx("flaky");
    let j = f.job("", json!({"threads": 1}));
    for i in 0..12 {
        write(&f.l.join(format!("r{i}.txt")), &format!("{i}"));
    }
    f.cloud.faults(Faults {
        busy_upload_on: vec![1, 2, 3],
        fail_upload_on: vec![5, 9],
        ..Default::default()
    });
    let mut ok = false;
    for _ in 0..6 {
        if let Ok(r) = run_job(&j, &f.state, false, false) {
            if !r.needs_retry() && r.planned.is_empty() {
                ok = true;
                break;
            }
        }
    }
    assert!(ok);
    for i in 0..12 {
        assert_eq!(f.cloud.get(&format!("r{i}.txt")).unwrap(), format!("{i}"));
    }
}

// ======================= kosz =======================

#[test]
fn trash_api_on_remote() {
    let f = fx("kapi");
    let j = f.job("", json!({}));
    f.seed(&j);
    write(&f.l.join("f0.txt"), "v2");
    set_mtime(&f.l.join("f0.txt"), 2_000_000_000);
    f.run(&j);
    let tr = trash_list(&j).unwrap();
    let it = tr
        .iter()
        .find(|i| i.side == "right" && i.path == "f0.txt")
        .unwrap();
    let back = trash_restore(&j, "right", &it.stamp, "f0.txt").unwrap();
    assert!(back.contains("przywrócono"), "{back}");
    assert_eq!(f.cloud.get("f0.txt").unwrap(), "v2");
    assert_eq!(f.cloud.get(&back).unwrap(), "tresc 0");
    let loc = Location::parse(&f.cloud.spec(""));
    loc.delete_trash(Some(&it.stamp), Some("f0.txt")).unwrap();
    assert!(trash_list(&j).unwrap().iter().all(|i| i.path != "f0.txt"));
    loc.delete_trash(None, None).unwrap();
    assert!(!f.cloud.keys().iter().any(|k| k.contains(".syncron-trash")));
    // usunięcie pustego kosza nie jest błędem
    loc.delete_trash(None, None).unwrap();
    assert!(loc.list_trash().unwrap().is_empty());
    // list_dirs
    f.cloud.put("Podfolder ą/x.txt", "x", 1_700_000_000);
    f.cloud.put(".syncron-trash/s/y.txt", "x", 1_700_000_000);
    assert_eq!(loc.list_dirs("").unwrap(), vec!["Podfolder ą".to_string()]);
    assert!(loc.list_dirs("../x").is_err());
    f.cloud.faults(Faults {
        fail_list_dir: true,
        fail_list: true,
        ..Default::default()
    });
    assert!(loc.list_dirs("").is_err());
    assert!(loc.list_trash().is_err());
}

#[test]
fn trash_listing_error_is_distinguished_from_missing() {
    let f = fx("kerr");
    let loc = Location::parse(&f.cloud.spec(""));
    assert!(loc.list_trash().unwrap().is_empty());
    f.cloud.faults(Faults {
        fail_list: true,
        ..Default::default()
    });
    assert!(loc.list_trash().is_err());
}

#[test]
fn purge_and_cap_versions_on_remote_trash() {
    let f = fx("purge");
    let loc = Location::parse(&f.cloud.spec(""));
    let t = ".syncron-trash";
    f.cloud
        .put(&format!("{t}/2000-01-01_000000/stary.txt"), "1", 1);
    f.cloud.put(&format!("{t}/obcy-folder/zostaje.txt"), "1", 1);
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    f.cloud.put(&format!("{t}/{today}_010101/nowy.txt"), "1", 1);
    loc.purge_trash(30, 0);
    assert!(!f.cloud.has(&format!("{t}/2000-01-01_000000/stary.txt")));
    assert!(f.cloud.has(&format!("{t}/obcy-folder/zostaje.txt")));
    assert!(f.cloud.has(&format!("{t}/{today}_010101/nowy.txt")));
    // limit wersji: zostają 2 najnowsze
    for (i, s) in [
        "2026-01-01_000001",
        "2026-01-02_000001",
        "2026-01-03_000001",
        "2026-01-03_000001-1",
    ]
    .iter()
    .enumerate()
    {
        f.cloud.put(&format!("{t}/{s}/a.txt"), &format!("v{i}"), 1);
    }
    loc.purge_trash(0, 2);
    let left: Vec<String> = loc
        .list_trash()
        .unwrap()
        .into_iter()
        .filter(|(_, p, _)| p == "a.txt")
        .map(|(s, _, _)| s)
        .collect();
    assert_eq!(left, ["2026-01-03_000001", "2026-01-03_000001-1"]);
}

#[test]
fn remote_trash_survives_purge_for_old_files() {
    let f = fx("pstary");
    let j = f.job("", json!({"trash_days": 30}));
    write(&f.l.join("stary.txt"), "v1");
    f.cloud.put("stary.txt", "v1", 1_000_000_000);
    set_mtime(&f.l.join("stary.txt"), 1_000_000_000);
    f.run(&j);
    write(&f.l.join("stary.txt"), "v22");
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
fn syncignore_is_read_from_remote() {
    let f = fx("ign");
    let j = f.job("", json!({}));
    f.cloud.put(".syncignore", "*.log\n", 1_700_000_000);
    write(&f.l.join("a.log"), "x");
    write(&f.l.join("a.txt"), "x");
    let rep = f.run(&j);
    assert!(f.cloud.has("a.txt") && !f.cloud.has("a.log"), "{rep:?}");
    // odczyt zawodzi: brak reguł (None), nie panika
    f.cloud.faults(Faults {
        fail_download: true,
        ..Default::default()
    });
    assert!(Location::parse(&f.cloud.spec(""))
        .read_text(".syncignore")
        .is_none());
}

// ======================= przeniesienia i zdalne↔zdalne =======================

#[test]
fn remote_to_remote_sync_via_temp_file_keeps_mtime() {
    let a = Mem::new("zrdA");
    let b = Mem::new("zrdB");
    let t = tempfile::tempdir().unwrap();
    let s = t.path().join("s");
    a.put("x/Zażółć.txt", "z A", 1_650_000_000);
    b.put("y/Łódź.txt", "z B", 1_660_000_000);
    let j = job_between(&a.spec("x"), &b.spec("y"), json!({}));
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert_eq!(b.get("y/Zażółć.txt").unwrap(), "z A");
    assert_eq!(a.get("x/Łódź.txt").unwrap(), "z B");
    assert!(run_job(&j, &s, false, false).unwrap().planned.is_empty());
}

#[test]
fn moves_between_remotes_only_when_both_ends_report_the_same_hash() {
    for (hashes, expect_moved) in [(true, 1usize), (false, 0usize)] {
        let a = Mem::new("mvA");
        let b = Mem::new("mvB");
        a.hashes(hashes);
        b.hashes(hashes);
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("s");
        for i in 0..6 {
            a.put(
                &format!("f{i}.txt"),
                &format!("tresc {i}"),
                1_700_000_000 + i,
            );
        }
        let mut j = job_between(&a.spec(""), &b.spec(""), json!({}));
        j.guard_pct = 100;
        assert_eq!(run_job(&j, &s, false, false).unwrap().copied, 6);
        // przeniesienie po stronie A
        let data = a.get("f2.txt").unwrap();
        MemBackendDelete::del(&a, "f2.txt");
        a.put("nowy/f2 przeniesiony.txt", &data, 1_700_000_002);
        let rep = run_job(&j, &s, false, false).unwrap();
        assert_eq!(rep.moved, expect_moved, "hashes={hashes}: {rep:?}");
        assert_eq!(rep.copied, 1 - expect_moved, "hashes={hashes}: {rep:?}");
        assert_eq!(b.get("nowy/f2 przeniesiony.txt").unwrap(), data);
        assert!(!b.has("f2.txt"), "stary plik musi zniknąć z B");
    }
}

/// Usuwa klucz z magazynu przez backend (bez kosza).
struct MemBackendDelete;
impl MemBackendDelete {
    fn del(m: &Mem, key: &str) {
        let b = syncron_core::remote::mem::MemBackend::shared(&m.id, "");
        syncron_core::remote::Backend::delete(&b, key).unwrap();
    }
}

#[test]
fn move_between_remotes_falls_back_when_server_cannot_rename() {
    let a = Mem::new("mrA");
    let b = Mem::new("mrB");
    a.hashes(true);
    b.hashes(true);
    let t = tempfile::tempdir().unwrap();
    let s = t.path().join("s");
    for i in 0..6 {
        a.put(
            &format!("f{i}.txt"),
            &format!("tresc {i}"),
            1_700_000_000 + i,
        );
    }
    let mut j = job_between(&a.spec(""), &b.spec(""), json!({}));
    j.guard_pct = 100;
    run_job(&j, &s, false, false).unwrap();
    b.faults(Faults {
        fail_rename: true,
        ..Default::default()
    });
    let data = a.get("f2.txt").unwrap();
    MemBackendDelete::del(&a, "f2.txt");
    a.put("nowy/f2.txt", &data, 1_700_000_002);
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.errors.is_empty(), "{rep:?}");
    assert_eq!(b.get("nowy/f2.txt").unwrap(), data);
    assert!(!b.has("f2.txt"));
}

// ======================= opcje i współbieżność =======================

#[test]
fn opts_split_bandwidth_and_threads() {
    let bw = syncron_core::model::Bandwidth {
        kbps: 800,
        off_kbps: 0,
        hours: Some([8, 18]),
    };
    let o = Opts::new(false, false, &bw, 4);
    assert_eq!(o.bw.kbps, 200);
    let f = fx("opts");
    f.cloud.put("a.txt", "dane", 1_700_000_000);
    let to = Location::Local(f.l.clone());
    let from = Location::parse(&f.cloud.spec(""));
    to.copy_from(&from, "a.txt", "b.txt", &o).unwrap();
    assert_eq!(fs::read_to_string(f.l.join("b.txt")).unwrap(), "dane");
    // i z powrotem, z limitem prędkości
    from.copy_from(&to, "b.txt", "c/d.txt", &o).unwrap();
    assert_eq!(f.cloud.get("c/d.txt").unwrap(), "dane");
}

#[test]
fn parallel_transfers_are_concurrent_and_complete() {
    let f = fx("rown");
    f.cloud.faults(Faults {
        latency_ms: 150,
        ..Default::default()
    });
    let j = f.job("", json!({"threads": 8}));
    for i in 0..24 {
        write(&f.l.join(format!("p{i}.txt")), &format!("{i}"));
    }
    let t = Instant::now();
    let rep = f.run(&j);
    let el = t.elapsed();
    assert_eq!(rep.copied, 24, "{rep:?}");
    // sekwencyjnie: 24 * 0,15 s = 3,6 s
    assert!(
        el < Duration::from_millis(3000),
        "brak równoległości: {el:?}"
    );
    for i in 0..24 {
        assert_eq!(f.cloud.get(&format!("p{i}.txt")).unwrap(), format!("{i}"));
    }
    f.cloud.clear_faults();
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn many_threads_many_files() {
    let f = fx("wiele");
    let j = f.job("", json!({"threads": 16}));
    for i in 0..300 {
        write(&f.l.join(format!("d{}/p{i}.txt", i % 7)), &format!("{i}"));
    }
    let rep = f.run(&j);
    assert_eq!(rep.copied, 300, "{rep:?}");
    assert!(rep.errors.is_empty() && rep.busy.is_empty());
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn two_jobs_in_parallel_threads() {
    let f = fx("dwa");
    let (l1, l2) = (f.l.join("one"), f.l.join("two"));
    for (l, tag) in [(&l1, "one"), (&l2, "two")] {
        for i in 0..8 {
            write(&l.join(format!("{tag}{i}.txt")), tag);
        }
    }
    let mk = |id: &str, l: &std::path::Path, r: &str| {
        job_between(
            l.to_str().unwrap(),
            &f.cloud.spec(r),
            json!({"id": id, "threads": 3}),
        )
    };
    let (j1, j2) = (mk("j1", &l1, "j1"), mk("j2", &l2, "j2"));
    drop(syncron_core::engine::open_db(&f.state).unwrap());
    std::thread::scope(|s| {
        let a = s.spawn(|| run_job(&j1, &f.state, false, false).unwrap());
        let b = s.spawn(|| run_job(&j2, &f.state, false, false).unwrap());
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert_eq!((a.copied, b.copied), (8, 8), "{a:?} {b:?}");
    });
    assert_eq!(f.cloud.live_keys().len(), 16);
}

#[test]
fn same_job_twice_concurrently_does_not_corrupt() {
    let f = fx("dwukrot");
    let j = f.job("", json!({"threads": 3}));
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
        assert_eq!(f.cloud.get(&format!("s{i}.txt")).unwrap(), "same");
    }
    let rep = f.run(&j);
    assert!(rep.errors.is_empty() && rep.conflicts.is_empty(), "{rep:?}");
    assert!(f.run(&j).planned.is_empty());
}

#[test]
fn run_twice_identical_results() {
    for _ in 0..3 {
        let f = fx("powt");
        let j = f.job("Mój Dysk/ąę", json!({}));
        write(&f.l.join("A/b c.txt"), "1");
        f.cloud.put("Mój Dysk/ąę/Ł.txt", "2", 1_700_000_000);
        let r1 = f.run(&j);
        assert_eq!((r1.copied, r1.deleted, r1.moved), (2, 0, 0), "{r1:?}");
        assert!(f.run(&j).planned.is_empty());
    }
}

// ======================= wydajność =======================

#[test]
fn listing_50k_and_dry_run() {
    let f = fx("wielki");
    for i in 0..50_000 {
        f.cloud.put(
            &format!("d{}/plik {i}.txt", i % 100),
            "x",
            1_700_000_000 + i,
        );
    }
    let loc = Location::parse(&f.cloud.spec(""));
    let t = Instant::now();
    let l = loc.list(&RuleSet::default()).unwrap();
    assert_eq!(l.files.len(), 50_000);
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    let j = f.job("", json!({"threads": 4}));
    let t = Instant::now();
    let rep = run_job(&j, &f.state, true, false).unwrap();
    assert_eq!(rep.planned.len(), 50_000);
    assert!(t.elapsed() < Duration::from_secs(30), "{:?}", t.elapsed());
}
