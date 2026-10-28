//! QA tester 2: nazwy plikow, Unicode, glob/.syncignore/reguly, kolizje, conflict_name, listowanie.
//! Testy z `#[ignore = "BUG: ..."]` odtwarzaja znalezione bledy (uruchom: `cargo test -p syncron-core --test qa_nazwy -- --ignored`).
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use syncron_core::engine::run_job;
use syncron_core::location::{local_full, Location};
use syncron_core::model::Job;
use syncron_core::names::{conflict_name, invalid_on_windows, is_temp, key, same_name};
use syncron_core::plan::{plan, Action, Conflict, Meta, Mode, Options, Tree};
use syncron_core::rules::{glob, parse_ignore, FileInfo, Rule, RuleSet};

// ---------- pomocnicze ----------

fn nfd(s: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfd(s).collect()
}
fn nfc(s: &str) -> String {
    unicode_normalization::UnicodeNormalization::nfc(s).collect()
}
fn fi(p: &str, s: u64) -> FileInfo {
    FileInfo {
        path: p.into(),
        size: s,
    }
}
fn m(size: u64, t: i64) -> Meta {
    Meta { size, mtime: t }
}
fn opts(mode: Mode) -> Options {
    Options {
        mode,
        no_delete_left: false,
        no_delete_right: false,
        only_add: false,
        conflict: Conflict::Newer,
    }
}
fn job(l: &Path, r: &Path, mode: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": "t", "name": "t", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    }))
    .unwrap()
}
fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    (t, l, r, s)
}
fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}
fn names_in(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
/// Uruchamia funkcje w watku z limitem czasu; None = przekroczono limit.
fn with_timeout<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

const PL: &str = "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ";

// =====================================================================
// RUNDA 1 - podstawy
// =====================================================================

#[test]
fn r1_nfc_nfd_wszystkie_polskie_znaki() {
    for c in PL.chars() {
        let s = format!("{c}.txt");
        assert!(same_name(&s, &nfd(&s)), "{c}");
        assert_eq!(key(&s), key(&nfd(&s)), "{c}");
    }
}

#[test]
fn r1_lodz_wielkosc_liter() {
    assert_eq!(key("ŁÓDŹ"), key("łódź"));
    assert_eq!(key("ŻÓŁĆ"), key(&nfd("żółć")));
    assert!(!same_name("ŁÓDŹ", "łódź")); // same_name jest wrazliwe na wielkosc liter
    assert_ne!(key("l"), key("ł"));
    assert_ne!(key("a.txt"), key("ą.txt"));
}

#[test]
fn r1_windows_zastrzezone_i_znaki() {
    for n in [
        "CON",
        "con",
        "Prn.txt",
        "aux.tar.gz",
        "NUL",
        "COM1",
        "com9.log",
        "LPT9",
        "lpt1.txt",
    ] {
        assert!(invalid_on_windows(n), "{n}");
    }
    for n in [
        "COM0", "COM10", "LPT0", "console", "nul2", "xcon", "a.con", ".nul", "ąę",
    ] {
        assert!(!invalid_on_windows(n), "{n}");
    }
    for n in [
        "a ", "a.", "a:b", "a|b", "a\"b", "a<b", "a>b", "a*b", "a\\b", "a/b", "a\tb", "a\u{1}b",
        "", ".", "..",
    ] {
        assert!(invalid_on_windows(n), "{n:?}");
    }
}

#[test]
fn r1_temp() {
    for n in [
        "~$a.docx",
        ".~lock.a.odt#",
        "x.TMP",
        "x.CrDownload",
        "y.part",
        "Thumbs.db",
        "DESKTOP.INI",
        ".DS_Store",
        "a.syncron.tmp",
    ] {
        assert!(is_temp(n), "{n}");
    }
    for n in [
        "tmp",
        "part",
        "apart",
        "a.tmp.txt",
        "thumbs.dbx",
        "~a",
        "łódź.txt",
    ] {
        assert!(!is_temp(n), "{n}");
    }
}

#[test]
fn r1_conflict_name_podstawy() {
    assert_eq!(
        conflict_name("a/b/żółć.docx", "S"),
        "a/b/żółć (konflikt S).docx"
    );
    assert_eq!(conflict_name("README", "S"), "README (konflikt S)");
    assert_eq!(conflict_name(".bashrc", "S"), ".bashrc (konflikt S)");
    assert_eq!(
        conflict_name("a.d/plik", "S"),
        "a.d/plik (konflikt S)",
        "kropka w katalogu nie jest rozszerzeniem"
    );
    assert_eq!(conflict_name("x.tar.gz", "S"), "x.tar (konflikt S).gz");
    assert_eq!(conflict_name("", "S"), " (konflikt S)");
    // nazwa konfliktowa nie moze byc plikiem tymczasowym
    assert!(!is_temp(&conflict_name("raport.docx", "2026-10-28")));
}

#[test]
fn r1_glob_podstawy_polskie() {
    assert!(glob("*.TXT", "zażółć.txt"));
    assert!(glob("ŁÓDŹ*", "łódź_2024.pdf"));
    assert!(glob("????.txt", "łódź.txt"));
    assert!(!glob("???.txt", "łódź.txt"));
    assert!(glob("", ""));
    assert!(!glob("", "a"));
    assert!(glob("*", ""));
    assert!(!glob("?", ""));
    assert!(glob("?.txt", "😀.txt")); // znak poza BMP = 1 znak
}

#[test]
fn r1_rules_nazwa_vs_sciezka() {
    let rs = RuleSet {
        exclude: vec![Rule::Glob("*.log".into())],
        ..Default::default()
    };
    assert!(!rs.allows(&fi("a/b/c.LOG", 1)));
    assert!(rs.allows(&fi("a/b/c.logx", 1)));
    let rs = RuleSet {
        include: vec![Rule::Glob("dokumenty/*".into())],
        ..Default::default()
    };
    assert!(rs.allows(&fi("dokumenty/ą.txt", 1)));
    assert!(rs.allows(&fi("dokumenty/pod/ą.txt", 1)));
    assert!(!rs.allows(&fi("inne/ą.txt", 1)));
}

#[test]
fn r1_listowanie_nfd_nfc_i_wielkosc_liter_kolizja_zgloszona() {
    let (_t, l, _r, _s) = setup();
    fs::write(l.join("ŁÓDŹ.txt"), "a").unwrap();
    fs::write(l.join("łódź.txt"), "b").unwrap();
    fs::write(l.join(nfd("łódź.txt").replace('ł', "Ł")), "c").unwrap();
    let lst = Location::parse(l.to_str().unwrap())
        .list(&RuleSet::default())
        .unwrap();
    assert_eq!(lst.files.len(), 1);
    assert_eq!(lst.collisions.len(), 2, "{:?}", lst.collisions);
}

#[test]
fn r1_run_job_nazwy_polskie_i_emoji_dwustronnie() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("Zażółć 😀")).unwrap();
    fs::write(l.join("Zażółć 😀/gęślą jaźń 𝔘.txt"), "a").unwrap();
    fs::write(r.join("  spacje na początku.txt"), "b").unwrap();
    let j = job(&l, &r, "TwoWay");
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{rep:?}");
    assert!(r.join("Zażółć 😀/gęślą jaźń 𝔘.txt").exists());
    assert!(l.join("  spacje na początku.txt").exists());
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn r1_plan_klucze_znormalizowane_nie_dubluja() {
    let mut l = Tree::new();
    let mut r = Tree::new();
    l.insert(key("Żółć.txt"), m(1, 10));
    r.insert(key(&nfd("żółć.TXT")), m(1, 10));
    let e = Tree::new();
    assert!(plan(&l, &r, &e, &e, opts(Mode::TwoWay)).is_empty());
}

// =====================================================================
// RUNDA 2 - brzegi i zlosliwe
// =====================================================================

#[test]
fn r2_puste_i_kropki() {
    assert_eq!(key(""), "");
    assert!(same_name("", ""));
    assert!(!is_temp(""));
    assert!(!is_temp("."));
    let rs = RuleSet::default();
    assert!(rs.allows(&fi("", 0)));
    assert!(rs.allows(&fi("a/", 0)));
    assert!(rs.allows(&fi("...", 0)));
}

#[test]
fn r2_dlugie_nazwy_bez_paniki() {
    let n255 = "a".repeat(255);
    let n255pl = "ł".repeat(255); // 510 B
    let n_huge = "ą".repeat(100_000);
    for n in [&n255, &n255pl, &n_huge] {
        assert_eq!(key(n), n.to_lowercase());
        assert!(same_name(n, &nfd(n)));
        let _ = invalid_on_windows(n);
        let _ = is_temp(n);
        assert!(glob("*", n));
        assert!(glob("*ą*", &nfc(n)) || !n.contains('ą'));
        let _ = conflict_name(n, "2026-10-28");
    }
    let rs = RuleSet {
        exclude: vec![Rule::Glob("*ł".into())],
        ..Default::default()
    };
    assert!(!rs.allows(&fi(&format!("{}/{}", n255, n255pl), 1)));
}

#[test]
fn r2_sterujace_bidi_zerowa_szerokosc() {
    for n in [
        "a\0b",
        "a\nb",
        "a\u{7f}",
        "\u{202e}txt.exe",
        "a\u{200b}b",
        "😀.txt",
        "a\u{0301}",
        "\x1b[31mred",
    ] {
        let _ = key(n);
        let _ = is_temp(n);
        let _ = glob("*", n);
        let _ = invalid_on_windows(n);
        let _ = conflict_name(n, "S");
    }
    assert!(invalid_on_windows("a\0b"));
    assert!(invalid_on_windows("a\nb"));
    assert!(invalid_on_windows("\x1b[31mred"));
    // RLO/ZWSP nie sa usuwane z kluczy: "evil\u{202e}txt.exe" to INNA nazwa niz "evil.exe"
    assert_ne!(key("evil\u{202e}txt.exe"), key("evil.exe"));
}

#[test]
fn r2_ukosniki_w_nazwach_i_masce() {
    assert!(invalid_on_windows("..\\x"));
    assert!(invalid_on_windows("../x"));
    let rs = RuleSet {
        exclude: vec![Rule::Glob("..".into())],
        ..Default::default()
    };
    assert!(!rs.allows(&fi("a/..", 0)));
}

#[test]
fn r2_eszet_turecka_i_klucze() {
    assert_eq!(key("STRASSE"), "strasse");
    assert_eq!(key("STRAßE"), "straße"); // ß != ss (akceptowalne)
    assert_eq!(key("İ"), "i\u{307}");
    assert_eq!(key("İ"), key("i\u{307}"));
}

#[test]
fn r2_parse_ignore_dziwne_linie() {
    let r = parse_ignore("\n   \n# x\n\t# y\n*.bak  \n  sciezka z spacja.txt\nfoo/\n");
    let globs: Vec<String> = r
        .iter()
        .map(|x| match x {
            Rule::Glob(g) => g.clone(),
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        globs,
        vec!["*.bak", "sciezka z spacja.txt", "foo/*", "*/foo/*"]
    );
    // CRLF
    let rs = RuleSet {
        exclude: parse_ignore("*.bak\r\nnode_modules/\r\n"),
        ..Default::default()
    };
    assert!(!rs.allows(&fi("a.bak", 1)));
    assert!(!rs.allows(&fi("x/node_modules/a", 1)));
    // brak paniki dla samych znakow specjalnych
    let _ = parse_ignore("/\n//\n*/\n!\n#\n\\\n");
}

#[test]
fn r2_parse_ignore_folder_nie_lapie_podobnych_nazw() {
    let rs = RuleSet {
        exclude: parse_ignore("build/\n"),
        ..Default::default()
    };
    assert!(!rs.allows(&fi("build/a.o", 1)));
    assert!(!rs.allows(&fi("p/build/a.o", 1)));
    assert!(rs.allows(&fi("rebuild/a.o", 1)));
    assert!(rs.allows(&fi("build.txt", 1)));
}

#[test]
fn r2_parse_ignore_bom() {
    let rs = RuleSet {
        exclude: parse_ignore("\u{feff}*.bak\nsekret.txt\n"),
        ..Default::default()
    };
    assert!(
        !rs.allows(&fi("a.bak", 1)),
        "pierwsza maska pominieta przez BOM"
    );
}

#[test]
fn r2_parse_ignore_zakotwiczona_sciezka() {
    let rs = RuleSet {
        exclude: parse_ignore("/prywatne/\n"),
        ..Default::default()
    };
    assert!(!rs.allows(&fi("prywatne/haslo.txt", 1)));
}

#[test]
fn r2_glob_maska_nfc_nazwa_nfd() {
    assert!(glob("zażółć*", &nfd("zażółć gęślą.txt")));
    let rs = RuleSet {
        exclude: vec![Rule::Glob("*żółć*".into())],
        ..Default::default()
    };
    assert!(
        !rs.allows(&fi(&nfd("dane/żółć.txt"), 1)),
        "wykluczenie NFC nie dziala dla NFD"
    );
}

#[test]
fn r2_glob_maska_nfd_nazwa_nfc() {
    assert!(glob(&nfd("zażółć*"), "zażółć gęślą.txt"));
}

#[test]
fn r2_glob_pytajnik_a_nfd() {
    assert!(glob("?.txt", &nfd("ą.txt")));
}

#[test]
fn r2_glob_pytajnik_a_wielkie_i_z_kropka() {
    assert!(glob("?stanbul", "İstanbul"));
}

#[test]
fn r2_glob_redos() {
    let pat = format!("{}b", "*a".repeat(12));
    let text = "a".repeat(60);
    let t = Instant::now();
    let r = with_timeout(5, move || glob(&pat, &text));
    assert_eq!(
        r,
        Some(false),
        "glob nie skonczyl w 5 s (czas {:?})",
        t.elapsed()
    );
}

#[test]
fn r2_rules_redos_przez_allows() {
    let rs = RuleSet {
        exclude: parse_ignore(&format!("{}x.log\n", "*_".repeat(10))),
        ..Default::default()
    };
    let path = format!("{}.txt", "_".repeat(80));
    let r = with_timeout(5, move || rs.allows(&fi(&path, 1)));
    assert_eq!(r, Some(true), "allows nie skonczyl w 5 s");
}

#[test]
fn r2_glob_umiarkowane_wzorce_szybkie() {
    let t = Instant::now();
    assert!(glob("*a*b*c*", "xxaxxbxxcxx"));
    assert!(!glob("*a*b*c*d", &"abc".repeat(30)));
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn r2_kolizje_nfc_nfd_w_folderze_nie_gubia_pliku() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("żółć.txt"), "NFC").unwrap();
    fs::write(l.join(nfd("żółć.txt")), "NFD").unwrap();
    fs::write(l.join("inny.txt"), "x").unwrap();
    let j = job(&l, &r, "TwoWay");
    let rep = run_job(&j, &s, false, false).unwrap();
    // kolizja zgloszona, a reszta zsynchronizowana
    assert!(
        rep.errors.iter().any(|e| e.contains("kolizja")),
        "{:?}",
        rep.errors
    );
    assert!(r.join("inny.txt").exists());
    // dokladnie jedna z wersji trafila na prawo; zadna nie zostala usunieta po lewej
    assert_eq!(
        names_in(&r)
            .iter()
            .filter(|n| key(n) == key("żółć.txt"))
            .count(),
        1
    );
    assert!(l.join("żółć.txt").exists() && l.join(nfd("żółć.txt")).exists());
    // kolejny przebieg nie usuwa niczego po zadnej stronie
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.deleted, 0, "{rep:?}");
    assert!(l.join("żółć.txt").exists() && l.join(nfd("żółć.txt")).exists());
}

#[test]
fn r2_kolizja_wielkosci_liter_wystepuje_po_przebiegu_nie_niszczy_danych() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("Raport.txt"), "lewy").unwrap();
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    // po prawej pojawia sie drugi plik o tej samej nazwie z inna wielkoscia liter
    fs::write(r.join("RAPORT.txt"), "prawy2").unwrap();
    let rep = run_job(&j, &s, false, false).unwrap();
    assert!(rep.errors.iter().any(|e| e.contains("kolizja")), "{rep:?}");
    assert_eq!(fs::read_to_string(r.join("RAPORT.txt")).unwrap(), "prawy2");
    assert_eq!(fs::read_to_string(r.join("Raport.txt")).unwrap(), "lewy");
}

#[test]
fn r2_kopiowanie_nadpisuje_istniejaca_nazwe_po_drugiej_stronie() {
    // lewy "Łódź.TXT" (NFC), prawy "łódź.txt" (NFD): zmiana po lewej trafia do istniejacego pliku, bez duplikatu
    let (_t, l, r, s) = setup();
    fs::write(l.join("Łódź.TXT"), "v1").unwrap();
    fs::write(r.join(nfd("łódź.txt")), "v1").unwrap();
    set_mtime(&l.join("Łódź.TXT"), 1_700_000_000);
    set_mtime(&r.join(nfd("łódź.txt")), 1_700_000_000);
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    fs::write(l.join("Łódź.TXT"), "v2-dluzsza").unwrap();
    set_mtime(&l.join("Łódź.TXT"), 1_800_000_000);
    run_job(&j, &s, false, false).unwrap();
    assert_eq!(
        names_in(&r).iter().filter(|n| !n.starts_with('.')).count(),
        1,
        "{:?}",
        names_in(&r)
    );
    assert_eq!(
        fs::read_to_string(r.join(nfd("łódź.txt"))).unwrap(),
        "v2-dluzsza"
    );
}

#[test]
fn r2_local_full_z_polskimi_znakami_i_pustymi_czesciami() {
    let root = Path::new("/tmp/Źródło ąę");
    assert_eq!(local_full(root, "a//b/"), root.join("a").join("b"));
    assert_eq!(local_full(root, ""), root.to_path_buf());
    assert_eq!(local_full(root, "/abs"), root.join("abs")); // wiodacy '/' nie ucieka z korzenia
}

#[test]
#[ignore = "BUG: local_full nie odrzuca '..' - sciezka wzgledna moze wyjsc poza korzen (restore_trash/delete_trash/copy z zewnetrznych danych)"]
fn r2_local_full_nie_wychodzi_poza_korzen() {
    let root = Path::new("/tmp/korzen");
    let p = local_full(root, "../../etc/passwd");
    assert!(
        !p.components().any(|c| c == std::path::Component::ParentDir),
        "{p:?}"
    );
}

#[test]
fn r2_delete_trash_bez_stempla_z_plikiem() {
    let (_t, l, _r, _s) = setup();
    let loc = Location::parse(l.to_str().unwrap());
    fs::create_dir_all(l.join(".syncron-trash/2026-01-01_000000")).unwrap();
    fs::write(l.join(".syncron-trash/2026-01-01_000000/a.txt"), "x").unwrap();
    let _ = loc.delete_trash(None, Some("a.txt"));
    assert!(l.join(".syncron-trash/2026-01-01_000000/a.txt").exists());
}

#[test]
fn r2_purge_trash_nie_panikuje_na_obcej_nazwie() {
    let (_t, l, _r, _s) = setup();
    let loc = Location::parse(l.to_str().unwrap());
    fs::create_dir_all(l.join(".syncron-trash/abcdefghiżółw")).unwrap();
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loc.purge_trash(30, 0)));
    assert!(res.is_ok(), "purge_trash spanikowal");
}

#[test]
#[ignore = "BUG: katalog o nazwie pasujacej do is_temp (np. 'notatki.tmp', 'dane.part') jest pomijany razem z zawartoscia - pliki nie sa synchronizowane, bez ostrzezenia"]
fn r2_katalog_z_koncowka_tmp_jest_synchronizowany() {
    let (_t, l, r, s) = setup();
    fs::create_dir_all(l.join("projekt.tmp")).unwrap();
    fs::write(l.join("projekt.tmp/wazny.txt"), "dane").unwrap();
    run_job(&job(&l, &r, "TwoWay"), &s, false, false).unwrap();
    assert!(r.join("projekt.tmp/wazny.txt").exists());
}

#[test]
#[ignore = "BUG: nazwa bliska 255 B: plik tymczasowy '<nazwa>.syncron.tmp' przekracza limit nazwy - kopia nie udaje sie (ENAMETOOLONG), choc sama nazwa jest poprawna"]
fn r2_nazwa_250_znakow_kopiuje_sie() {
    let (_t, l, r, s) = setup();
    let name = format!("{}.txt", "a".repeat(246)); // 250 B
    fs::write(l.join(&name), "x").unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(r.join(&name).exists(), "{:?}", rep.errors);
}

#[test]
fn r2_nazwa_255_bajtow_blad_nie_panika() {
    // bez wzgledu na wynik - brak paniki, plik zrodlowy nietkniety, blad zgloszony po polsku/z opisem
    let (_t, l, r, s) = setup();
    let name = format!("{}.txt", "ł".repeat(125)); // 254 B
    fs::write(l.join(&name), "x").unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(l.join(&name).exists());
    assert!(r.join(&name).exists() || !rep.errors.is_empty());
}

#[test]
fn r2_conflict_name_miesci_sie_w_255() {
    let n = format!("{}.txt", "a".repeat(240));
    assert!(conflict_name(&n, "2026-10-28").len() <= 255);
}

#[test]
fn r2_keep_both_dwa_konflikty_tego_samego_dnia() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("a.txt"), "0").unwrap();
    let mut j = job(&l, &r, "TwoWay");
    j.conflict = Conflict::KeepBoth;
    j.trash_days = 0;
    run_job(&j, &s, false, false).unwrap();
    let mut t = 1_900_000_000;
    let mut seen = vec![];
    for round in 0..2 {
        t += 1000;
        fs::write(l.join("a.txt"), format!("lewy{round}")).unwrap();
        set_mtime(&l.join("a.txt"), t);
        let prawy = format!("PRAWY-unikalny-{round}");
        fs::write(r.join("a.txt"), &prawy).unwrap();
        set_mtime(&r.join("a.txt"), t + 100);
        run_job(&j, &s, false, false).unwrap();
        seen.push(prawy);
    }
    // obie "prawe" wersje powinny przetrwac gdzies w lewym folderze
    let all: String = names_in(&l)
        .iter()
        .filter(|n| n.contains("konflikt"))
        .map(|n| fs::read_to_string(l.join(n)).unwrap())
        .collect::<Vec<_>>()
        .join("|");
    for p in seen {
        assert!(all.contains(&p), "utracona wersja {p}; zostalo: {all}");
    }
}

#[test]
fn r2_syncignore_nie_utf8_nie_jest_ignorowany() {
    let (_t, l, r, s) = setup();
    // CP1250: 'ł' = 0xB3
    fs::write(l.join(".syncignore"), b"sekret_\xb3.txt\n*.bak\n").unwrap();
    fs::write(l.join("sekret.bak"), "tajne").unwrap();
    run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(
        !r.join("sekret.bak").exists(),
        "*.bak wyslane mimo .syncignore"
    );
}

#[cfg(unix)]
#[test]
#[ignore = "BUG: nazwa pliku nie-UTF-8 (Linux) jest zamieniana na U+FFFD; plik jest wykazany, ale nie da sie go skopiowac ('No such file'), mylacy komunikat"]
fn r2_nazwa_nie_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let (_t, l, r, s) = setup();
    let p = l.join(std::ffi::OsStr::from_bytes(b"cp1250_\xb3\xf3d\xbf.txt"));
    fs::write(&p, "x").unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert_eq!(rep.copied, 1, "{:?}", rep.errors);
}

#[test]
#[ignore = "DECYZJA: zmiana tylko wielkosci liter nazwy (a.txt -> A.txt) nie propaguje sie na druga strone - WYNIKA Z PROJEKTU: klucz nazw jest niewrazliwy na wielkosc liter (names::key); zmiana wymagalaby sledzenia nazw w stanie (inny obszar)"]
fn r2_zmiana_wielkosci_liter_nazwy_propaguje_sie() {
    let (_t, l, r, s) = setup();
    fs::write(l.join("raport.txt"), "x").unwrap();
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    fs::rename(l.join("raport.txt"), l.join("Raport.txt")).unwrap();
    run_job(&j, &s, false, false).unwrap();
    assert!(
        names_in(&r).contains(&"Raport.txt".to_string()),
        "{:?}",
        names_in(&r)
    );
}

#[test]
#[ignore = "BUG: invalid_on_windows nie jest nigdzie uzywane w silniku - plik 'a:b.txt' z Linuksa/macOS jest wysylany na Windows i blad jest dopiero z systemu (brak ostrzezenia przed kopiowaniem)"]
fn r2_invalid_on_windows_nieuzywane() {
    let src = include_str!("../src/engine.rs").to_string()
        + include_str!("../src/location.rs")
        + include_str!("../src/runner.rs");
    assert!(src.contains("invalid_on_windows"));
}

#[test]
fn r2_zastrzezone_rozszerzone() {
    for n in ["COM¹", "LPT².txt", "COM³", "CONIN$", "CONOUT$"] {
        assert!(invalid_on_windows(n), "{n}");
    }
}

#[test]
fn r2_zastrzezone_ze_spacja_przed_kropka() {
    assert!(invalid_on_windows("con .txt"));
}

#[test]
fn r2_dlugosc_nazwy_windows() {
    assert!(invalid_on_windows(&"a".repeat(256)));
    assert!(invalid_on_windows(&"😀".repeat(200)));
}

#[test]
fn r2_location_parse_polskie_i_dziwne() {
    assert!(matches!(
        Location::parse("Dysk Źródłowy:Dane"),
        Location::Rclone(_)
    ));
    assert!(matches!(Location::parse("/home/ą:ę/x"), Location::Local(_)));
    assert!(matches!(Location::parse("  /tmp/x  "), Location::Local(_)));
    assert!(matches!(Location::parse("C:foo"), Location::Local(_)));
    assert!(matches!(Location::parse(""), Location::Local(_)));
}

// =====================================================================
// RUNDA 3 - wlasnosci, obciazenie, regresja
// =====================================================================

fn sample_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        "a",
        "A",
        "ŁÓDŹ",
        "łódź",
        "Zażółć",
        "ZAŻÓŁĆ",
        "ß",
        "SS",
        "ẞ",
        "İ",
        "i̇",
        "I",
        "ı",
        "Σ",
        "ς",
        "σ",
        "ΑΣ",
        "ασ",
        "ας",
        "K",
        "\u{212a}",
        "Å",
        "\u{212b}",
        "Å",
        "ǅ",
        "ǆ",
        "Ǆ",
        "ﬁ",
        "FI",
        "ΐ",
        "Ω",
        "\u{2126}",
        "ω",
        "e\u{301}",
        "é",
        "É",
        "😀",
        "a\u{200b}",
        "ᾈ",
        "ᾀ",
        "ǰ",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for c in PL.chars() {
        v.push(c.to_string());
    }
    v
}

#[test]
fn r3_key_niezmienniczy_wzgledem_nfc_nfd() {
    for s in sample_names() {
        assert_eq!(key(&s), key(&nfd(&s)), "{s:?}");
        assert_eq!(key(&s), key(&nfc(&s)), "{s:?}");
    }
}

#[test]
fn r3_key_greckie_przyklady_diagnostyka() {
    for s in ["γ\u{324}ỗJ\u{30c}", "ƄϙΆ\u{345}ό", "ћῌ\u{342}"] {
        let k = key(s);
        eprintln!(
            "{s:?}: key={k:?} idem={} nfd_eq={}",
            key(&k) == k,
            key(&nfd(s)) == k
        );
    }
}

#[test]
fn r3_key_symetryczne_i_zgodne_z_same_name() {
    let v = sample_names();
    for a in &v {
        for b in &v {
            assert_eq!(key(a) == key(b), key(b) == key(a));
            if same_name(a, b) {
                assert_eq!(key(a), key(b), "{a:?} {b:?}");
            }
        }
    }
}

#[test]
fn r3_key_idempotentne_na_probce() {
    let mut bad = vec![];
    for s in sample_names() {
        let k = key(&s);
        if key(&k) != k {
            bad.push(s);
        }
    }
    assert!(bad.is_empty(), "key nie idempotentne: {bad:?}");
}

#[test]
fn r3_key_idempotentne_i_nfd_niezmiennicze_losowe() {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let mut bad = vec![];
    for _ in 0..30_000 {
        let n = (next() % 6) as usize;
        let s: String = (0..n)
            .filter_map(|_| {
                let r = next();
                let cp = match r % 5 {
                    0 => 0x41 + (r >> 8) % 0x2c0,
                    1 => 0x300 + (r >> 8) % 0x100,
                    2 => 0x370 + (r >> 8) % 0x500,
                    3 => 0x1e00 + (r >> 8) % 0x200,
                    _ => 0xfb00 + (r >> 8) % 0x60,
                };
                char::from_u32(cp as u32)
            })
            .collect();
        let k = key(&s);
        if key(&k) != k || key(&nfd(&s)) != k {
            bad.push(s);
        }
    }
    assert!(
        bad.is_empty(),
        "niespojne klucze: {:?}",
        &bad[..bad.len().min(5)]
    );
}

#[test]
fn r3_key_sigma_koncowa() {
    assert_eq!(key("ΑΣ"), key("ασ"));
}

#[test]
fn r3_plan_po_przebiegu_idempotentny_dla_kluczy() {
    let mut l = Tree::new();
    for n in ["ą.txt", "Ł/ó.txt", "😀.txt"] {
        l.insert(key(n), m(5, 50));
    }
    let e = Tree::new();
    let a = plan(&l, &Tree::new(), &e, &e, opts(Mode::TwoWay));
    assert_eq!(a.len(), 3);
    assert!(a.values().all(|x| *x == Action::CopyToRight));
    let r = l.clone();
    assert!(plan(&l, &r, &l, &r, opts(Mode::TwoWay)).is_empty());
}

#[test]
fn r3_duze_drzewo_10k_listowanie_i_plan() {
    let (_t, l, r, s) = setup();
    for d in 0..100 {
        let dir = l.join(format!("Katalog żółć {d}"));
        fs::create_dir_all(&dir).unwrap();
        for i in 0..100 {
            fs::write(dir.join(format!("zażółć_gęślą_{i}.txt")), "x").unwrap();
        }
    }
    let j = job(&l, &r, "TwoWay");
    let t = Instant::now();
    let lst = Location::parse(l.to_str().unwrap())
        .list(&j.rules())
        .unwrap();
    let t_list = t.elapsed();
    assert_eq!(lst.files.len(), 10_000);
    assert!(lst.collisions.is_empty());
    let t = Instant::now();
    let rep = run_job(&j, &s, false, false).unwrap();
    let t_run = t.elapsed();
    assert_eq!(
        rep.copied,
        10_000,
        "{:?}",
        &rep.errors[..rep.errors.len().min(3)]
    );
    let t = Instant::now();
    let rep = run_job(&j, &s, false, false).unwrap();
    let t_idle = t.elapsed();
    assert!(rep.planned.is_empty());
    eprintln!("10k: listowanie {t_list:?}, pierwszy przebieg {t_run:?}, pusty przebieg {t_idle:?}");
    assert!(t_list < Duration::from_secs(5), "{t_list:?}");
    assert!(t_idle < Duration::from_secs(20), "{t_idle:?}");
}

#[test]
fn r3_reguly_na_10k_sciezek_szybko() {
    let rs = RuleSet {
        exclude: parse_ignore("*.log\n*~\nnode_modules/\n*.bak\n"),
        include: vec![Rule::Glob("*.txt".into())],
        ..Default::default()
    };
    let t = Instant::now();
    let n = (0..10_000)
        .filter(|i| {
            rs.allows(&fi(
                &format!("katalog{}/zażółć_gęślą_{}.txt", i % 100, i),
                1,
            ))
        })
        .count();
    assert_eq!(n, 10_000);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

#[test]
fn r3_glob_dlugi_tekst_bez_wykladnika() {
    let text = "ąb".repeat(5_000);
    let t = Instant::now();
    assert!(glob("*b", &text));
    assert!(glob("ą*b", &text));
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn r3_wielokrotne_przebiegi_nazwy_unicode_stabilne() {
    // mieszane NFC/NFD i wielkosc liter po obu stronach, 3 przebiegi - brak duplikatow, brak usuniec, stabilny plan
    let (_t, l, r, s) = setup();
    for (i, c) in PL.chars().enumerate() {
        let n = format!("{c}{i}.txt");
        fs::write(l.join(&n), format!("l{i}")).unwrap();
        fs::write(r.join(nfd(&n.to_lowercase())), format!("l{i}")).unwrap();
        set_mtime(&l.join(&n), 1_700_000_000);
        set_mtime(&r.join(nfd(&n.to_lowercase())), 1_700_000_000);
    }
    let j = job(&l, &r, "TwoWay");
    for pass in 0..3 {
        let rep = run_job(&j, &s, false, false).unwrap();
        assert_eq!(rep.deleted, 0, "przebieg {pass}: {rep:?}");
        assert!(rep.errors.is_empty(), "przebieg {pass}: {:?}", rep.errors);
        if pass > 0 {
            assert!(rep.planned.is_empty(), "przebieg {pass}: {rep:?}");
        }
    }
    assert_eq!(names_in(&l).len(), PL.chars().count());
    assert_eq!(names_in(&r).len(), PL.chars().count());
}

#[test]
fn r3_kosz_zachowuje_nazwy_unicode() {
    let (_t, l, r, s) = setup();
    for i in 0..4 {
        fs::write(l.join(format!("zażółć {i}.txt")), "a").unwrap();
    }
    fs::write(l.join("Ł ó 😀.txt"), "ostatni").unwrap();
    for i in 0..5 {
        fs::write(l.join(format!("pad{i}.txt")), "p").unwrap();
    }
    let j = job(&l, &r, "TwoWay");
    run_job(&j, &s, false, false).unwrap();
    fs::remove_file(l.join("Ł ó 😀.txt")).unwrap();
    let rep = run_job(&j, &s, false, false).unwrap();
    assert_eq!(rep.deleted, 1, "{rep:?}");
    let items = Location::parse(r.to_str().unwrap()).list_trash().unwrap();
    assert!(items.iter().any(|(_, p, _)| p == "Ł ó 😀.txt"), "{items:?}");
}
