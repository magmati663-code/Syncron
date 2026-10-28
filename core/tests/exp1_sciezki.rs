//! Ekspert 1: ścieżki, symlinki i bezpieczeństwo zapisu (core/src/location.rs).
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::run_job;
use syncron_core::location::{local_full, Location, OpError, Opts};
use syncron_core::model::Job;

fn job(l: &Path, r: &Path, mode: &str) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": "e1", "name": "t", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 1
    }))
    .unwrap()
}

fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
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

fn opts() -> Opts {
    Opts::new(false, false, &Default::default(), 1)
}

#[test]
fn local_full_przycina_do_korzenia() {
    let root = Path::new("/tmp/k");
    for rel in ["../x", "a/../../x", "./a/./b", "/abs/x"] {
        let p = local_full(root, rel);
        assert!(p.starts_with(root), "{rel} -> {p:?}");
        assert!(!p.components().any(|c| c == std::path::Component::ParentDir));
    }
    assert_eq!(local_full(root, "a/../../x"), root.join("a").join("x"));
}

#[test]
fn list_dirs_odrzuca_dotdot_i_bezwzgledne() {
    let (_t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join("a/b")).unwrap();
    let loc = Location::parse(l.to_str().unwrap());
    assert_eq!(loc.list_dirs("").unwrap(), vec!["a".to_string()]);
    assert_eq!(loc.list_dirs("a").unwrap(), vec!["b".to_string()]);
    assert!(loc.list_dirs("..").is_err());
    assert!(loc.list_dirs("a/../..").is_err());
    assert!(loc.list_dirs("/etc").is_err());
}

#[test]
fn copy_i_rename_odrzucaja_dotdot_z_komunikatem() {
    let (t, l, r, _s) = dirs();
    fs::write(r.join("src.txt"), "x").unwrap();
    fs::write(l.join("a.txt"), "x").unwrap();
    let ll = Location::parse(l.to_str().unwrap());
    let rr = Location::parse(r.to_str().unwrap());
    let e = ll
        .copy_from(&rr, "src.txt", "../kopia.txt", &opts())
        .unwrap_err();
    assert!(matches!(e, OpError::Failed(m) if m.contains("niebezpieczna")));
    assert!(ll.rename("a.txt", "../p.txt").is_err());
    assert!(ll.rename("../a.txt", "b.txt").is_err());
    assert!(ll.remove("../R/src.txt", "").is_err());
    assert!(!t.path().join("kopia.txt").exists() && !t.path().join("p.txt").exists());
    assert!(r.join("src.txt").exists());
}

#[cfg(unix)]
#[test]
fn remove_nie_podaza_za_symlinkiem() {
    let (t, l, _r, _s) = dirs();
    let out = t.path().join("OUT");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("ofiara.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&out, l.join("sub")).unwrap();
    let ll = Location::parse(l.to_str().unwrap());
    let e = ll.remove("sub/ofiara.txt", "").unwrap_err();
    assert!(e.to_string().contains("dowiązaniem symbolicznym"), "{e}");
    assert!(out.join("ofiara.txt").exists());
}

#[cfg(unix)]
#[test]
fn zapis_przez_symlink_zglaszany_po_polsku() {
    let (t, l, r, s) = dirs();
    let out = t.path().join("OUT");
    fs::create_dir_all(&out).unwrap();
    fs::create_dir_all(l.join("sub")).unwrap();
    fs::write(l.join("sub/x.txt"), "dane").unwrap();
    std::os::unix::fs::symlink(&out, r.join("sub")).unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(!out.join("x.txt").exists());
    assert!(
        rep.errors.iter().any(|e| e.contains("dowiązaniem")),
        "{:?}",
        rep.errors
    );
}

#[cfg(unix)]
#[test]
fn tmp_symlink_nie_jest_podazany() {
    // symlink o nazwie pliku tymczasowego (ta sama funkcja skrótu) nie może przekierować zapisu poza korzeń
    let (t, l, r, s) = dirs();
    let victim = t.path().join("ofiara.txt");
    fs::write(&victim, "stare").unwrap();
    fs::write(l.join("a.txt"), "nowe").unwrap();
    let h = blake3::hash(b"a.txt").to_hex();
    for n in 0..8 {
        let tmp = r.join(format!(
            ".{}-{:x}-{:x}.syncron.tmp",
            &h[..12],
            std::process::id(),
            n
        ));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
    }
    run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert_eq!(fs::read(r.join("a.txt")).unwrap(), b"nowe");
    assert_eq!(fs::read(&victim).unwrap(), b"stare");
}

#[test]
fn dluga_nazwa_i_tmp_stalej_dlugosci() {
    let (_t, l, r, s) = dirs();
    for n in [250usize, 255] {
        let name = format!("{}.txt", "b".repeat(n - 4));
        fs::write(l.join(&name), "x").unwrap();
    }
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert_eq!(rep.copied, 2, "{:?}", rep.errors);
}

#[test]
fn plik_a_pusty_katalog_konwerguje() {
    let (_t, l, r, s) = dirs();
    fs::create_dir_all(r.join("d")).unwrap();
    fs::write(l.join("d"), "plik").unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(fs::read(r.join("d")).unwrap(), b"plik");
}

#[test]
fn remove_sprzata_puste_katalogi_ale_nie_korzen() {
    let (_t, l, _r, _s) = dirs();
    fs::create_dir_all(l.join("a/b")).unwrap();
    fs::write(l.join("a/b/x.txt"), "x").unwrap();
    fs::write(l.join("a/zostaje.txt"), "x").unwrap();
    let ll = Location::parse(l.to_str().unwrap());
    ll.remove("a/b/x.txt", "").unwrap();
    assert!(!l.join("a/b").exists());
    assert!(l.join("a/zostaje.txt").exists());
    ll.remove("a/zostaje.txt", "").unwrap();
    assert!(!l.join("a").exists());
    assert!(l.exists());
}

#[test]
fn kosz_pomijany_tylko_w_korzeniu_i_tylko_jako_katalog() {
    let (_t, l, r, s) = dirs();
    fs::create_dir_all(l.join(".syncron-trash/x")).unwrap();
    fs::write(l.join(".syncron-trash/x/k.txt"), "k").unwrap();
    fs::write(l.join(".syncron-trashcan.txt"), "c").unwrap();
    fs::create_dir_all(l.join("sub/.syncron-trash")).unwrap();
    fs::write(l.join("sub/.syncron-trash/d.txt"), "d").unwrap();
    run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(r.join(".syncron-trashcan.txt").exists());
    assert!(r.join("sub/.syncron-trash/d.txt").exists());
    assert!(!r.join(".syncron-trash/x/k.txt").exists());
}

#[test]
fn katalog_tmp_synchronizowany_ale_plik_tmp_nie() {
    let (_t, l, r, s) = dirs();
    fs::create_dir_all(l.join("projekt.tmp")).unwrap();
    fs::write(l.join("projekt.tmp/w.txt"), "w").unwrap();
    fs::write(l.join("x.tmp"), "t").unwrap();
    run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert!(r.join("projekt.tmp/w.txt").exists());
    assert!(!r.join("x.tmp").exists());
}

#[cfg(unix)]
#[test]
fn nazwa_nie_utf8_pominieta_i_zgloszona() {
    use std::os::unix::ffi::OsStrExt;
    let (_t, l, r, s) = dirs();
    fs::write(
        l.join(std::ffi::OsStr::from_bytes(b"cp1250_\xb3\xf3d\xbf.txt")),
        "x",
    )
    .unwrap();
    fs::write(l.join("ok.txt"), "x").unwrap();
    let rep = run_job(&job(&l, &r, "LeftToRight"), &s, false, false).unwrap();
    assert_eq!(rep.copied, 1);
    assert!(
        rep.errors.iter().any(|e| e.contains("spoza UTF-8")),
        "{:?}",
        rep.errors
    );
}

#[test]
fn brak_uprawnien_to_blad_a_nie_zajetosc() {
    let e: OpError = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
    assert!(
        matches!(&e, OpError::Failed(m) if m.starts_with("brak uprawnień")),
        "{e:?}"
    );
    let e: OpError = std::io::Error::from(std::io::ErrorKind::Interrupted).into();
    assert!(matches!(e, OpError::Busy(_)));
}
