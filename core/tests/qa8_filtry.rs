//! QA8 / T2: filtry i opcje listingu (ukryte, wiek, rozmiar, dowiązania, uprawnienia, wersjonowanie, nazwy).
//! Kluczowe pytanie: czy plik wypadający ze skanu jednej strony może zostać fałszywie nadpisany/usunięty po drugiej.
#![cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::run_job;
use syncron_core::model::Job;

struct Env {
    _t: tempfile::TempDir,
    l: PathBuf,
    r: PathBuf,
    s: PathBuf,
    ext: PathBuf,
}

fn setup() -> Env {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().to_path_buf();
    let e = Env {
        l: p.join("L"),
        r: p.join("R"),
        s: p.join("state"),
        ext: p.join("EXT"),
        _t: t,
    };
    for d in [&e.l, &e.r, &e.ext] {
        fs::create_dir_all(d).unwrap();
    }
    e
}

fn job(e: &Env, mode: &str, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "qa8", "name": "qa8", "left": e.l.to_str().unwrap(), "right": e.r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2,
        "trash_days": 30
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn mt(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn w(p: &Path, c: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn rd(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok()
}

// ---------- wiek: plik wypada ze skanu jednej strony ----------

#[test]

fn wiek_swiezy_lewy_nie_jest_nadpisany_starym_prawym() {
    let e = setup();
    w(&e.r.join("a.txt"), "STARA wersja");
    mt(&e.r.join("a.txt"), now() - 30 * 86400);
    w(&e.l.join("a.txt"), "NOWA wersja uzytkownika!!");
    // lewy: zmodyfikowany teraz (młodszy niż 7 dni) -> wykluczony przez min_age_days
    let j = job(&e, "TwoWay", serde_json::json!({"min_age_days": 7}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(
        rd(&e.l.join("a.txt")).as_deref(),
        Some("NOWA wersja uzytkownika!!"),
        "dane użytkownika zniszczone: {rep:?}"
    );
}

#[test]

fn rozmiar_urosly_lewy_nie_jest_nadpisany_starym_prawym() {
    let e = setup();
    w(&e.r.join("a.bin"), "mala");
    w(&e.l.join("a.bin"), &"X".repeat(5000));
    let j = job(&e, "TwoWay", serde_json::json!({"max_size": 100}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(
        rd(&e.l.join("a.bin")).map(|s| s.len()),
        Some(5000),
        "{rep:?}"
    );
}

#[test]

fn wiek_maks_stary_lewy_nie_jest_nadpisany() {
    let e = setup();
    w(&e.l.join("a.txt"), "lewa archiwalna");
    mt(&e.l.join("a.txt"), now() - 400 * 86400);
    w(&e.r.join("a.txt"), "prawa swieza");
    let j = job(&e, "TwoWay", serde_json::json!({"max_age_days": 30}));
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.l.join("a.txt")).as_deref(), Some("lewa archiwalna"));
}

#[test]
fn wiek_swiezy_lewy_nie_jest_usuniety_po_prawej_po_synchronizacji() {
    // z bazą: usunięcie propagowane, ale `survives` chroni plik wykluczony wiekiem
    let e = setup();
    w(&e.l.join("a.txt"), "v1");
    mt(&e.l.join("a.txt"), now() - 30 * 86400);
    let j = job(&e, "TwoWay", serde_json::json!({"min_age_days": 7}));
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join("a.txt")).as_deref(), Some("v1"));
    // użytkownik zmienia lewy plik na świeży
    w(&e.l.join("a.txt"), "v2 zmieniony dzisiaj");
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(
        rd(&e.l.join("a.txt")).as_deref(),
        Some("v2 zmieniony dzisiaj")
    );
    assert!(e.r.join("a.txt").exists(), "prawa kopia usunięta");
}

#[test]
fn wiek_tryb_lustra_nie_kasuje_prawego_gdy_lewy_za_swiezy() {
    let e = setup();
    w(&e.r.join("a.txt"), "stare");
    w(&e.l.join("a.txt"), "swieze");
    let j = job(&e, "LeftToRight", serde_json::json!({"min_age_days": 7}));
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join("a.txt")).as_deref(), Some("stare"));
    assert_eq!(rd(&e.l.join("a.txt")).as_deref(), Some("swieze"));
}

#[test]
fn wiek_granica_min_max_dokladne() {
    let e = setup();
    let n = now();
    w(&e.l.join("m6.txt"), "1");
    mt(&e.l.join("m6.txt"), n - 6 * 86400);
    w(&e.l.join("m8.txt"), "2");
    mt(&e.l.join("m8.txt"), n - 8 * 86400);
    w(&e.l.join("m40.txt"), "3");
    mt(&e.l.join("m40.txt"), n - 40 * 86400);
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"min_age_days": 7, "max_age_days": 30}),
    );
    run_job(&j, &e.s, false, true).unwrap();
    assert!(!e.r.join("m6.txt").exists());
    assert!(e.r.join("m8.txt").exists());
    assert!(!e.r.join("m40.txt").exists());
}

#[test]
fn wiek_plik_z_przyszlosci_jest_swiezy() {
    let e = setup();
    w(&e.l.join("f.txt"), "x");
    mt(&e.l.join("f.txt"), now() + 10 * 86400);
    let j = job(&e, "LeftToRight", serde_json::json!({"min_age_days": 1}));
    run_job(&j, &e.s, false, true).unwrap();
    assert!(!e.r.join("f.txt").exists());
}

// ---------- ukryte ----------

#[test]
fn ukryte_pomijane_pliki_i_foldery_ale_syncignore_dziala() {
    let e = setup();
    w(&e.l.join(".sekret"), "s");
    w(&e.l.join(".git/config"), "c");
    w(&e.l.join("dir/.ukryty.txt"), "u");
    w(&e.l.join("dir/jawny.txt"), "j");
    w(&e.l.join(".syncignore"), "*.log\n");
    w(&e.l.join("a.log"), "l");
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"ignore_hidden": true}),
    );
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.r.join("dir/jawny.txt").exists());
    assert!(!e.r.join(".sekret").exists());
    assert!(!e.r.join(".git").exists());
    assert!(!e.r.join("dir/.ukryty.txt").exists());
    assert!(!e.r.join(".syncignore").exists());
    assert!(!e.r.join("a.log").exists(), ".syncignore nie zadziałał");
}

#[test]
fn ukryte_dwukierunkowo_nie_kasuje_i_nie_kopiuje() {
    let e = setup();
    w(&e.l.join(".x"), "left");
    w(&e.r.join(".x"), "right");
    w(&e.l.join("a.txt"), "a");
    let j = job(&e, "TwoWay", serde_json::json!({"ignore_hidden": true}));
    run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.l.join(".x")).as_deref(), Some("left"));
    assert_eq!(rd(&e.r.join(".x")).as_deref(), Some("right"));
}

#[test]
fn ukryte_wlaczenie_po_fakcie_nie_usuwa_istniejacych() {
    // plik był już zsynchronizowany; użytkownik włącza ignore_hidden: ukryte pliki NIE mogą zostać skasowane
    let e = setup();
    w(&e.l.join(".cfg"), "ustawienia");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.r.join(".cfg").exists());
    let j2 = job(&e, "TwoWay", serde_json::json!({"ignore_hidden": true}));
    run_job(&j2, &e.s, false, true).unwrap();
    assert!(e.l.join(".cfg").exists());
    assert!(e.r.join(".cfg").exists());
}

#[test]
fn ukryte_jednostronne_przeksztalcenie_nazwy_nie_gubi_danych() {
    // lewy "a.txt" zmieniony na ".a.txt" (ukryty) przy ignore_hidden: prawy a.txt usunięty jest akceptowalne tylko z koszem
    let e = setup();
    w(&e.l.join("a.txt"), "tresc");
    let j = job(&e, "TwoWay", serde_json::json!({"ignore_hidden": true}));
    run_job(&j, &e.s, false, true).unwrap();
    fs::rename(e.l.join("a.txt"), e.l.join(".a.txt")).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    // treść musi istnieć gdzieś (lewy ukryty, prawy lub kosz)
    let kosz = e.r.join(".syncron-trash").exists();
    assert!(e.r.join("a.txt").exists() || kosz);
    assert_eq!(rd(&e.l.join(".a.txt")).as_deref(), Some("tresc"));
}

// ---------- dowiązania ----------

#[test]
fn symlink_skip_pomija_i_nie_kopiuje() {
    let e = setup();
    w(&e.ext.join("f.txt"), "zewn");
    std::os::unix::fs::symlink(e.ext.join("f.txt"), e.l.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&e.ext, e.l.join("dirlink")).unwrap();
    w(&e.l.join("real.txt"), "r");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.r.join("real.txt").exists());
    assert!(!e.r.join("link.txt").exists());
    assert!(!e.r.join("dirlink").exists());
}

#[test]

fn symlink_skip_nie_jest_niszczony_przez_plik_z_prawej() {
    let e = setup();
    w(&e.ext.join("f.txt"), "zewn");
    std::os::unix::fs::symlink(e.ext.join("f.txt"), e.l.join("a.txt")).unwrap();
    w(&e.r.join("a.txt"), "zwykly plik z prawej");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    let md = fs::symlink_metadata(e.l.join("a.txt")).unwrap();
    assert!(md.file_type().is_symlink(), "dowiązanie zastąpione plikiem");
}

#[test]
fn symlink_follow_plik_i_folder_kopiowane_jako_zawartosc() {
    let e = setup();
    w(&e.ext.join("f.txt"), "zewn");
    w(&e.ext.join("sub/g.txt"), "glebokie");
    std::os::unix::fs::symlink(e.ext.join("f.txt"), e.l.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&e.ext, e.l.join("dirlink")).unwrap();
    std::os::unix::fs::symlink(e.ext.join("brak"), e.l.join("zerwany")).unwrap();
    let j = job(&e, "LeftToRight", serde_json::json!({"symlinks": "Follow"}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join("link.txt")).as_deref(), Some("zewn"));
    assert_eq!(
        rd(&e.r.join("dirlink/sub/g.txt")).as_deref(),
        Some("glebokie")
    );
    assert!(!e.r.join("zerwany").exists());
    // po prawej to zwykłe pliki
    assert!(!fs::symlink_metadata(e.r.join("link.txt"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}

#[test]
fn symlink_follow_petle_koncza_sie() {
    let e = setup();
    w(&e.l.join("a/b/f.txt"), "1");
    std::os::unix::fs::symlink("..", e.l.join("a/b/up")).unwrap(); // -> a
    std::os::unix::fs::symlink("../..", e.l.join("a/b/root")).unwrap(); // -> korzeń
    std::os::unix::fs::symlink(".", e.l.join("self")).unwrap();
    std::os::unix::fs::symlink("p2", e.l.join("p1")).unwrap();
    std::os::unix::fs::symlink("p1", e.l.join("p2")).unwrap(); // pętla dowiązań
    let j = job(&e, "LeftToRight", serde_json::json!({"symlinks": "Follow"}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join("a/b/f.txt")).as_deref(), Some("1"));
    assert!(
        rep.scanned_left < 20,
        "skan rozrósł się: {}",
        rep.scanned_left
    );
}

#[test]
fn symlink_follow_zapis_nie_wychodzi_poza_korzen() {
    let e = setup();
    w(&e.ext.join("f.txt"), "zewn");
    std::os::unix::fs::symlink(&e.ext, e.l.join("dirlink")).unwrap();
    let j = job(&e, "TwoWay", serde_json::json!({"symlinks": "Follow"}));
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join("dirlink/f.txt")).as_deref(), Some("zewn"));
    // nowy plik po prawej w dirlink/ -> próba zapisu przez dowiązanie po lewej: musi być odmowa
    w(&e.r.join("dirlink/nowy.txt"), "wtargniecie");
    // usunięcie po prawej też nie może skasować pliku zewnętrznego
    fs::remove_file(e.r.join("dirlink/f.txt")).unwrap();
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert!(
        !e.ext.join("nowy.txt").exists(),
        "zapis poza korzeń! {rep:?}"
    );
    assert_eq!(
        rd(&e.ext.join("f.txt")).as_deref(),
        Some("zewn"),
        "usunięcie poza korzeń! {rep:?}"
    );
}

#[test]
fn symlink_follow_wersja_w_koszu_nie_dotyka_celu_dowiazania() {
    // zmiana pliku po prawej, lewy jest dowiązaniem do pliku zewnętrznego: cel zewnętrzny nie może być nadpisany
    let e = setup();
    w(&e.ext.join("f.txt"), "oryginal");
    mt(&e.ext.join("f.txt"), now() - 1000);
    std::os::unix::fs::symlink(e.ext.join("f.txt"), e.l.join("a.txt")).unwrap();
    let j = job(&e, "TwoWay", serde_json::json!({"symlinks": "Follow"}));
    run_job(&j, &e.s, false, true).unwrap();
    w(&e.r.join("a.txt"), "zmienione po prawej, dluzsze");
    mt(&e.r.join("a.txt"), now());
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.ext.join("f.txt")).as_deref(), Some("oryginal"));
}

// ---------- uprawnienia ----------

#[test]
fn uprawnienia_setuid_setgid_sticky_nie_przenoszone() {
    use std::os::unix::fs::PermissionsExt;
    let e = setup();
    for (n, m) in [
        ("suid", 0o4755),
        ("sgid", 0o2755),
        ("stick", 0o1755),
        ("rwx", 0o755),
        ("priv", 0o600),
    ] {
        w(&e.l.join(n), "x");
        fs::set_permissions(e.l.join(n), fs::Permissions::from_mode(m)).unwrap();
    }
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"preserve_permissions": true}),
    );
    run_job(&j, &e.s, false, true).unwrap();
    let mode = |n: &str| fs::metadata(e.r.join(n)).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode("suid"), 0o755);
    assert_eq!(mode("sgid"), 0o755);
    assert_eq!(mode("stick"), 0o755);
    assert_eq!(mode("rwx"), 0o755);
    assert_eq!(mode("priv"), 0o600);
}

#[test]
fn uprawnienia_bez_opcji_tez_bez_specjalnych_bitow() {
    use std::os::unix::fs::PermissionsExt;
    let e = setup();
    w(&e.l.join("suid"), "x");
    fs::set_permissions(e.l.join("suid"), fs::Permissions::from_mode(0o4755)).unwrap();
    let j = job(&e, "LeftToRight", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    let m = fs::metadata(e.r.join("suid")).unwrap().permissions().mode() & 0o7000;
    assert_eq!(m, 0);
}

#[test]
fn uprawnienia_kierunek_prawy_do_lewego_i_nadpisanie_readonly() {
    use std::os::unix::fs::PermissionsExt;
    let e = setup();
    w(&e.l.join("a"), "stary");
    mt(&e.l.join("a"), now() - 5000);
    fs::set_permissions(e.l.join("a"), fs::Permissions::from_mode(0o444)).unwrap();
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"preserve_permissions": true}),
    );
    run_job(&j, &e.s, false, true).unwrap();
    w(&e.r.join("a"), "nowy dluzszy");
    fs::set_permissions(e.r.join("a"), fs::Permissions::from_mode(0o751)).unwrap();
    mt(&e.r.join("a"), now());
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rd(&e.l.join("a")).as_deref(), Some("nowy dluzszy"));
    assert_eq!(
        fs::metadata(e.l.join("a")).unwrap().permissions().mode() & 0o7777,
        0o751
    );
}

// ---------- wersjonowanie ----------

fn trash_files(root: &Path) -> Vec<String> {
    let mut out = vec![];
    fn go(d: &Path, rel: String, out: &mut Vec<String>) {
        if let Ok(rd) = fs::read_dir(d) {
            for e in rd.flatten() {
                let n = format!("{rel}/{}", e.file_name().to_string_lossy());
                if e.path().is_dir() {
                    go(&e.path(), n, out);
                } else {
                    out.push(n);
                }
            }
        }
    }
    go(&root.join(".syncron-trash"), String::new(), &mut out);
    out.sort();
    out
}

fn bump(e: &Env, content: &str, t: i64) {
    w(&e.l.join("d/a.txt"), content);
    mt(&e.l.join("d/a.txt"), t);
}

#[test]
fn wersjonowanie_replace_trzyma_jedna_kopie_folders_wiele() {
    for (ver, expect) in [("Replace", 1usize), ("Folders", 3usize)] {
        let e = setup();
        let j = job(&e, "LeftToRight", serde_json::json!({"versioning": ver}));
        let base = now() - 100_000;
        bump(&e, "v1", base);
        run_job(&j, &e.s, false, true).unwrap();
        for (i, c) in [
            "v2 dluzsze",
            "v3 jeszcze dluzsze",
            "v4 najdluzsze z wszystkich",
        ]
        .iter()
        .enumerate()
        {
            // stamp ma rozdzielczość sekundy: kolizja jest rozwiązywana przyrostkiem -N
            bump(&e, c, base + 1000 * (i as i64 + 1));
            run_job(&j, &e.s, false, true).unwrap();
        }
        let t = trash_files(&e.r);
        assert_eq!(t.len(), expect, "{ver}: {t:?}");
        assert_eq!(
            rd(&e.r.join("d/a.txt")).as_deref(),
            Some("v4 najdluzsze z wszystkich")
        );
        if ver == "Replace" {
            // zostaje NAJNOWSZA poprzednia wersja (v3)
            let p =
                e.r.join(".syncron-trash")
                    .join(t[0].trim_start_matches('/'));
            assert_eq!(rd(&p).as_deref(), Some("v3 jeszcze dluzsze"), "{t:?}");
        }
    }
}

#[test]
fn wersjonowanie_replace_nie_rusza_innych_plikow_o_podobnej_nazwie() {
    let e = setup();
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"versioning": "Replace"}),
    );
    let base = now() - 100_000;
    for n in ["d/a.txt", "d/a.txt.bak2", "e/d/a.txt", "a.txt"] {
        w(&e.l.join(n), "v1");
        mt(&e.l.join(n), base);
    }
    run_job(&j, &e.s, false, true).unwrap();
    for (i, n) in ["d/a.txt", "d/a.txt.bak2", "e/d/a.txt", "a.txt"]
        .iter()
        .enumerate()
    {
        w(&e.l.join(n), &format!("v2 {i} dluzsze"));
        mt(&e.l.join(n), base + 1000);
    }
    run_job(&j, &e.s, false, true).unwrap();
    for n in ["d/a.txt", "d/a.txt.bak2", "e/d/a.txt", "a.txt"] {
        w(&e.l.join(n), &format!("v3 {n} jeszcze dluzsze"));
        mt(&e.l.join(n), base + 2000);
    }
    run_job(&j, &e.s, false, true).unwrap();
    let t = trash_files(&e.r);
    assert_eq!(t.len(), 4, "{t:?}");
}

#[test]
fn wersjonowanie_replace_usuniecie_dwa_razy_jedna_kopia() {
    let e = setup();
    let j = job(&e, "TwoWay", serde_json::json!({"versioning": "Replace"}));
    for round in 0..3 {
        w(
            &e.l.join("x.txt"),
            &format!("runda {round} {}", "z".repeat(round)),
        );
        run_job(&j, &e.s, false, true).unwrap();
        fs::remove_file(e.l.join("x.txt")).unwrap();
        run_job(&j, &e.s, false, true).unwrap();
        assert!(!e.r.join("x.txt").exists());
    }
    assert_eq!(trash_files(&e.r).len(), 1, "{:?}", trash_files(&e.r));
}

// ---------- maski + nowe filtry ----------

#[test]
fn maski_include_exclude_syncignore_razem_z_filtrami_bez_falszywych_usuniec() {
    let e = setup();
    for n in [
        "a.docx",
        "b.log",
        "c.txt",
        ".h.docx",
        "node_modules/x.docx",
        "old.docx",
    ] {
        w(&e.l.join(n), "tresc");
    }
    mt(&e.l.join("old.docx"), now() - 400 * 86400);
    w(&e.l.join(".syncignore"), "node_modules/\n");
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"include": ["*.docx"], "exclude": ["*.log"], "ignore_hidden": true, "max_age_days": 30}),
    );
    run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.r.join("a.docx").exists());
    for n in [
        "b.log",
        "c.txt",
        ".h.docx",
        "node_modules/x.docx",
        "old.docx",
    ] {
        assert!(!e.r.join(n).exists(), "{n}");
        assert!(e.l.join(n).exists(), "{n} usunięty po lewej");
    }
}

#[test]
fn syncignore_tylko_po_jednej_stronie_nie_kasuje_drugiej() {
    // .syncignore po prawej wyklucza *.dat: plik istnieje po obu stronach (zsynchronizowany wcześniej)
    let e = setup();
    w(&e.l.join("a.dat"), "dane");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.r.join("a.dat").exists());
    w(&e.r.join(".syncignore"), "*.dat\n");
    run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.l.join("a.dat").exists());
    assert!(e.r.join("a.dat").exists());
}

#[test]
fn syncignore_rozni_sie_miedzy_stronami_zmiana_pliku_nie_nadpisuje_wykluczonej_strony() {
    // prawa strona ignoruje *.dat; lewy plik a.dat zmieniony, prawy ma starą wersję, brak bazy
    let e = setup();
    w(&e.r.join(".syncignore"), "*.dat\n");
    w(&e.r.join("a.dat"), "prawa");
    w(&e.l.join("a.dat"), "lewa");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    // maski są wspólne (z obu stron), więc plik wypada po obu stronach i nic się nie dzieje
    assert_eq!(rd(&e.l.join("a.dat")).as_deref(), Some("lewa"));
    assert_eq!(rd(&e.r.join("a.dat")).as_deref(), Some("prawa"));
}

// ---------- nazwy ----------

#[test]
fn nazwy_nfc_nfd_ta_sama_nazwa_bez_duplikatow() {
    use unicode_normalization::UnicodeNormalization;
    let e = setup();
    let nfc = "zażółć gęślą jaźń.txt";
    let nfd: String = nfc.nfd().collect();
    w(&e.l.join(nfc), "tresc");
    let mut p = e.r.clone();
    p.push(&nfd);
    w(&p, "tresc");
    mt(&e.l.join(nfc), 1_700_000_000);
    mt(&p, 1_700_000_000);
    let j = job(&e, "TwoWay", serde_json::json!({}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    let nl = fs::read_dir(&e.l).unwrap().count();
    let nr = fs::read_dir(&e.r).unwrap().count();
    assert_eq!((nl, nr), (1, 1), "{rep:?}");
}

#[test]
fn nazwy_nfd_usuniecie_propaguje_wlasciwa_nazwe() {
    use unicode_normalization::UnicodeNormalization;
    let e = setup();
    let nfc = "łódź.txt";
    let nfd: String = nfc.nfd().collect();
    w(&e.l.join(nfc), "x");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    run_job(&j, &e.s, false, true).unwrap();
    // prawa strona: ta sama nazwa zapisana w NFD (symulacja macOS) i usunięta po lewej
    fs::rename(e.r.join(nfc), e.r.join(&nfd)).unwrap();
    fs::remove_file(e.l.join(nfc)).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(
        fs::read_dir(&e.r)
            .unwrap()
            .filter(|x| x.as_ref().unwrap().file_name().to_string_lossy() != ".syncron-trash")
            .count(),
        0
    );
}

#[test]
fn nazwy_dlugie_255_bajtow_i_polskie() {
    let e = setup();
    let long = format!("{}.txt", "ż".repeat(125)); // 250 B + 4
    assert!(long.len() <= 255);
    w(&e.l.join(&long), "dlugie");
    let j = job(&e, "TwoWay", serde_json::json!({"verify": true}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert_eq!(rd(&e.r.join(&long)).as_deref(), Some("dlugie"), "{rep:?}");
    // konflikt na takiej nazwie: nazwa kopii konfliktowej mieści się w 255 B
    let j2 = job(&e, "TwoWay", serde_json::json!({"conflict": "KeepBoth"}));
    w(&e.l.join(&long), "lewa wersja");
    mt(&e.l.join(&long), now() - 50);
    w(&e.r.join(&long), "prawa wersja inna");
    mt(&e.r.join(&long), now());
    let rep = run_job(&j2, &e.s, false, true).unwrap();
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
}

#[test]
fn nazwy_zastrzezone_windows_na_linuksie_kopiuja_sie() {
    let e = setup();
    for n in [
        "CON.txt",
        "aux",
        "nul.log",
        "trailing.",
        "sp ace ",
        "a:b",
        "q?.txt",
    ] {
        w(&e.l.join(n), "x");
    }
    let j = job(&e, "LeftToRight", serde_json::json!({}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    for n in [
        "CON.txt",
        "aux",
        "nul.log",
        "trailing.",
        "sp ace ",
        "a:b",
        "q?.txt",
    ] {
        assert!(e.r.join(n).exists(), "{n}");
    }
}

#[test]
fn nazwy_kolizja_wielkosci_liter_bez_utraty_danych() {
    let e = setup();
    w(&e.l.join("Raport.txt"), "A");
    w(&e.l.join("raport.txt"), "B-inna-dluzsza");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    // oba pliki lewej muszą nadal istnieć z oryginalną treścią
    assert_eq!(rd(&e.l.join("Raport.txt")).as_deref(), Some("A"));
    assert_eq!(
        rd(&e.l.join("raport.txt")).as_deref(),
        Some("B-inna-dluzsza")
    );
    assert!(!rep.errors.is_empty(), "kolizja powinna być zgłoszona");
}

#[test]
fn nazwy_nieutf8_pomijane_i_zgloszone_bez_kasowania() {
    use std::os::unix::ffi::OsStrExt;
    let e = setup();
    let bad = std::ffi::OsStr::from_bytes(b"bad\xff\xfe.txt");
    fs::write(e.l.join(bad), "x").unwrap();
    w(&e.l.join("ok.txt"), "ok");
    let j = job(&e, "TwoWay", serde_json::json!({}));
    let rep = run_job(&j, &e.s, false, true).unwrap();
    run_job(&j, &e.s, false, true).unwrap();
    assert!(e.l.join(bad).exists());
    assert!(e.r.join("ok.txt").exists());
    assert!(!rep.errors.is_empty());
}
