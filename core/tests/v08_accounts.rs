//! Magazyn kont i sekretów: zapis atomowy, brak jawnego tekstu na dysku, szyfrowanie, odzyskiwanie po uszkodzeniu,
//! blokada współbieżna, migracja `rclone.conf` (hasła „obscure” odwracane), konta jako lokalizacje.
mod common;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use syncron_core::accounts::{
    self, rclone_obscure_with_iv, rclone_reveal, EncryptedFile, MemSecrets, SecretStore, Store,
};
use syncron_core::location::Location;

fn kv(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Wszystkie pliki katalogu (rekurencyjnie) jako (ścieżka, bajty).
fn all_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push((
                    e.path().display().to_string(),
                    fs::read(e.path()).unwrap_or_default(),
                ));
            }
        }
    }
    out
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

const SECRET: &str = "Tajne-Haslo-ąęść-12345!";

fn webdav_form() -> BTreeMap<String, String> {
    kv(&[
        ("url", "https://dysk.firma.pl/remote.php/dav/files/jan"),
        ("vendor", "nextcloud"),
        ("user", "jan"),
        ("pass", SECRET),
    ])
}

// ======================= model i magazyn =======================

#[test]
fn add_list_get_update_delete() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    assert!(st.list().unwrap().is_empty());
    let a = st.add("Nextcloud", "webdav", &webdav_form()).unwrap();
    assert_eq!(a.kind, "webdav");
    assert_eq!(a.secret_keys, ["pass"]);
    assert_eq!(a.params["user"], "jan");
    assert!(
        !a.params.contains_key("pass"),
        "hasło nie może być w jawnych parametrach"
    );

    let g = st.get("nextcloud").unwrap(); // wielkość liter nie ma znaczenia
    assert_eq!(g.name, "Nextcloud");
    assert_eq!(st.list().unwrap().len(), 1);

    // zmiana jawnych pól; puste hasło = zostaje stare
    let mut upd = webdav_form();
    upd.insert("user".into(), "anna".into());
    upd.insert("pass".into(), String::new());
    st.update("Nextcloud", &upd).unwrap();
    let p = st.params_for("Nextcloud").unwrap();
    assert_eq!(p.get("user"), Some("anna"));
    assert_eq!(p.secret("pass"), Some(SECRET));

    // nowe hasło
    upd.insert("pass".into(), "inne-haslo".into());
    st.update("Nextcloud", &upd).unwrap();
    assert_eq!(
        st.params_for("Nextcloud").unwrap().secret("pass"),
        Some("inne-haslo")
    );

    st.delete("Nextcloud").unwrap();
    assert!(st.list().unwrap().is_empty());
    assert!(st.get("Nextcloud").is_err());
    assert!(st.delete("Nextcloud").is_err());
}

#[test]
fn delete_clears_secrets() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    let ef = EncryptedFile::new(t.path());
    assert_eq!(ef.get("Konto", "pass").unwrap().as_deref(), Some(SECRET));
    st.delete("Konto").unwrap();
    assert_eq!(ef.get("Konto", "pass").unwrap(), None);
    // dodanie konta o tej samej nazwie nie „dziedziczy” starego hasła
    st.add(
        "Konto",
        "webdav",
        &kv(&[("url", "https://x.pl"), ("user", "u")]),
    )
    .unwrap();
    assert_eq!(ef.get("Konto", "pass").unwrap(), None);
}

#[test]
fn secrets_never_in_plain_text_on_disk() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    st.add(
        "Chmura S3",
        "s3",
        &kv(&[
            ("access_key_id", "AKIAJAWNE"),
            ("secret_access_key", "tajny-klucz-s3-9876"),
            ("region", "eu-central-1"),
        ]),
    )
    .unwrap();
    st.set_secret("Konto", "refresh_token", "1//odswiezony-token-xyz")
        .unwrap();
    for (name, bytes) in all_files(t.path()) {
        for s in [
            SECRET,
            "tajny-klucz-s3-9876",
            "1//odswiezony-token-xyz",
            "inne",
        ] {
            if s == "inne" {
                continue;
            }
            assert!(!contains(&bytes, s), "jawny sekret w pliku {name}");
        }
    }
    let json = fs::read_to_string(t.path().join("accounts.json")).unwrap();
    assert!(json.contains("eu-central-1") && !json.contains("AKIAJAWNE"));
    assert!(
        json.contains("\"pass\"") && !json.contains(SECRET),
        "w pliku tylko NAZWY sekretów"
    );
    // magazyn jest czytelny dla nowej instancji (po restarcie)
    let st2 = Arc::new(Store::file_only(t.path()));
    assert_eq!(
        st2.params_for("Konto").unwrap().secret("refresh_token"),
        Some("1//odswiezony-token-xyz")
    );
}

#[cfg(unix)]
#[test]
fn private_files_have_0600() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    for f in ["secrets.enc", "secrets.key", "accounts.json"] {
        let m = fs::metadata(t.path().join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "{f}");
    }
    // po zakończeniu nie zostają pliki tymczasowe ani blokady
    for (n, _) in all_files(t.path()) {
        assert!(!n.contains(".tmp-") && !n.ends_with(".lock"), "{n}");
    }
}

#[test]
fn encrypted_file_is_randomized_and_authenticated() {
    let t = tempfile::tempdir().unwrap();
    let ef = EncryptedFile::new(t.path());
    ef.set("a", "k", "wartość").unwrap();
    let c1 = fs::read(t.path().join("secrets.enc")).unwrap();
    ef.set("a", "k", "wartość").unwrap();
    let c2 = fs::read(t.path().join("secrets.enc")).unwrap();
    assert_ne!(c1, c2, "każdy zapis ma świeży nonce");
    assert!(c1.starts_with(b"SYNSEC1\0"));
    // modyfikacja choćby jednego bajtu szyfrogramu wykrywana (uwierzytelnienie)
    let mut bad = c2.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    fs::write(t.path().join("secrets.enc"), &bad).unwrap();
    assert!(ef.health().is_err());
    assert_eq!(ef.get("a", "k").unwrap(), None);
}

#[test]
fn corrupted_secret_file_is_recovered_with_backup() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    fs::write(
        t.path().join("secrets.enc"),
        b"to nie jest zaszyfrowany plik",
    )
    .unwrap();
    // konto pokazuje „wymaga zalogowania”, a użycie daje czytelny błąd (nie panikę)
    let v = st.list_view().unwrap();
    assert!(v[0].needs_login);
    let e = st.params_for("Konto").err().unwrap();
    assert!(e.contains("Brakuje") || e.contains("zaloguj"), "{e}");
    // ponowne wpisanie hasła naprawia magazyn; uszkodzony plik zostaje jako kopia
    st.update("Konto", &webdav_form()).unwrap();
    assert_eq!(st.params_for("Konto").unwrap().secret("pass"), Some(SECRET));
    let kopie = all_files(t.path())
        .into_iter()
        .filter(|(n, _)| n.contains("secrets.enc.corrupt-"))
        .count();
    assert_eq!(kopie, 1);
    assert!(!st.list_view().unwrap()[0].needs_login);
}

#[test]
fn lost_key_file_is_recoverable() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    fs::remove_file(t.path().join("secrets.key")).unwrap();
    assert!(st.list_view().unwrap()[0].needs_login);
    st.update("Konto", &webdav_form()).unwrap();
    assert_eq!(st.params_for("Konto").unwrap().secret("pass"), Some(SECRET));
}

#[test]
fn corrupted_accounts_file_is_never_overwritten() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    fs::write(t.path().join("accounts.json"), "{ to nie json").unwrap();
    assert!(st.list().is_err());
    assert!(st.add("Konto", "webdav", &webdav_form()).is_err());
    assert_eq!(
        fs::read_to_string(t.path().join("accounts.json")).unwrap(),
        "{ to nie json"
    );
    assert!(all_files(t.path())
        .iter()
        .any(|(n, _)| n.contains("accounts.json.corrupt-")));
    // add nie zostawia osieroconych sekretów po nieudanym zapisie kont
    assert!(
        !t.path().join("secrets.enc").exists()
            || EncryptedFile::new(t.path())
                .get("Konto", "pass")
                .unwrap()
                .is_none()
    );
}

#[test]
fn empty_or_missing_accounts_file_is_empty_list() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    assert!(st.list().unwrap().is_empty());
    fs::write(t.path().join("accounts.json"), "  \n").unwrap();
    assert!(st.list().unwrap().is_empty());
}

#[test]
fn name_and_parameter_validation() {
    for ok in [
        "gdrive",
        "a1",
        "my-remote_2",
        "Mój Dysk",
        "Dysk Zażółć",
        "firma.pl",
    ] {
        assert!(accounts::valid_name(ok), "{ok}");
    }
    for bad in [
        "",
        "a",
        "-x",
        "_x",
        " x",
        "x ",
        "a:b",
        "a/b",
        "a\\b",
        "--flag",
        &"x".repeat(65),
    ] {
        assert!(!accounts::valid_name(bad), "{bad:?}");
    }
    assert!(accounts::valid_param_key("client_secret"));
    for bad in ["", "--password-command", "a b", "a=b", "-x"] {
        assert!(!accounts::valid_param_key(bad), "{bad}");
    }
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    assert!(st.add("a:b", "webdav", &BTreeMap::new()).is_err());
    assert!(st.add("Konto", "webdav\n", &BTreeMap::new()).is_err());
    assert!(st.add("Konto", "web dav", &BTreeMap::new()).is_err());
    assert!(st.add("Konto", "webdav", &kv(&[("bad key", "x")])).is_err());
    assert!(st.add("Konto", "webdav", &kv(&[("user", "a\nb")])).is_err());
}

#[test]
fn names_are_unique_case_insensitive_and_nfc() {
    use unicode_normalization::UnicodeNormalization;
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    st.add("Dysk Zażółć", "webdav", &kv(&[("url", "https://x.pl")]))
        .unwrap();
    let nfd: String = "Dysk Zażółć".nfd().collect();
    assert!(st
        .add(&nfd, "webdav", &kv(&[("url", "https://x.pl")]))
        .is_err());
    assert!(st
        .add("DYSK ZAŻÓŁĆ", "webdav", &kv(&[("url", "https://x.pl")]))
        .is_err());
    assert_eq!(st.get(&nfd).unwrap().name, "Dysk Zażółć");
    // nazwa zapisana w NFC
    st.add(
        &"Łódź NFD".nfd().collect::<String>(),
        "webdav",
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(st
        .list()
        .unwrap()
        .iter()
        .any(|a| a.name == "Łódź NFD" && a.name.nfc().eq(a.name.chars())));
}

#[test]
fn concurrent_adds_are_serialized() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().to_path_buf();
    std::thread::scope(|s| {
        for i in 0..16 {
            let dir = dir.clone();
            s.spawn(move || {
                let st = Store::file_only(&dir);
                st.add(
                    &format!("konto{i}"),
                    "webdav",
                    &kv(&[("url", "https://x.pl"), ("pass", &format!("haslo-{i}"))]),
                )
                .unwrap();
            });
        }
    });
    let st = Arc::new(Store::file_only(&dir));
    assert_eq!(st.list().unwrap().len(), 16);
    for i in 0..16 {
        assert_eq!(
            st.params_for(&format!("konto{i}")).unwrap().secret("pass"),
            Some(format!("haslo-{i}").as_str())
        );
    }
    serde_json::from_str::<serde_json::Value>(
        &fs::read_to_string(dir.join("accounts.json")).unwrap(),
    )
    .unwrap();
}

#[test]
fn refreshed_token_is_saved_through_callback() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    st.add(
        "Dysk",
        "mem",
        &kv(&[("id", "x"), ("refresh_token", "stary")]),
    )
    .unwrap();
    let p = st.params_for("Dysk").unwrap();
    assert_eq!(p.secret("refresh_token"), Some("stary"));
    (p.on_secret_update.as_ref().unwrap())("refresh_token", "nowy");
    (p.on_secret_update.as_ref().unwrap())("access_token", "krotki");
    let again = Arc::new(Store::file_only(t.path()))
        .params_for("Dysk")
        .unwrap();
    assert_eq!(again.secret("refresh_token"), Some("nowy"));
    assert_eq!(again.secret("access_token"), Some("krotki"));
    assert!(st
        .get("Dysk")
        .unwrap()
        .secret_keys
        .contains(&"access_token".to_string()));
    for (n, b) in all_files(t.path()) {
        assert!(!contains(&b, "nowy") && !contains(&b, "krotki"), "{n}");
    }
}

#[test]
fn custom_secret_store_is_used_and_cleaned() {
    let t = tempfile::tempdir().unwrap();
    let mem = Arc::new(MemSecrets::default());
    let st = Store::with_secrets(t.path(), mem.clone());
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    assert_eq!(mem.get("Konto", "pass").unwrap().as_deref(), Some(SECRET));
    assert!(!t.path().join("secrets.enc").exists());
    st.delete("Konto").unwrap();
    assert_eq!(mem.get("Konto", "pass").unwrap(), None);
}

#[test]
fn default_store_works_with_forced_file_backend() {
    // SYNCRON_SECRETS=file: serwery i usługi bez sesji D-Bus; nie dotykamy kluczy systemu
    std::env::set_var("SYNCRON_SECRETS", "file");
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::open(t.path()));
    st.add("Konto", "webdav", &webdav_form()).unwrap();
    assert_eq!(st.get("Konto").unwrap().secret_store, "file");
    assert_eq!(st.params_for("Konto").unwrap().secret("pass"), Some(SECRET));
    st.delete("Konto").unwrap();
}

#[test]
#[ignore = "dotyka prawdziwego magazynu kluczy systemu (Credential Manager / Keychain / Secret Service)"]
fn real_system_keyring_roundtrip_or_fallback() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::open(t.path()));
    let long_key = "-----BEGIN KEY-----\n".to_string() + &"A".repeat(5000) + "\n-----END KEY-----";
    st.add(
        "Syncron test kont",
        "mem",
        &kv(&[("id", "k"), ("pass", SECRET), ("private_key", &long_key)]),
    )
    .unwrap();
    let p = st.params_for("Syncron test kont").unwrap();
    assert_eq!(p.secret("pass"), Some(SECRET));
    assert_eq!(p.secret("private_key"), Some(long_key.as_str()));
    st.delete("Syncron test kont").unwrap();
}

// ======================= konta jako lokalizacje =======================

#[test]
fn accounts_from_store_resolve_in_locations() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::with_secrets(
        t.path(),
        Arc::new(MemSecrets::default()),
    ));
    accounts::set_global_store(st.clone());
    let id = format!("loc-{}", std::process::id());
    st.add("Moja chmura", "mem", &kv(&[("id", &id), ("pass", "x")]))
        .unwrap();
    syncron_core::remote::mem::MemBackend::put_raw(&id, "Dok/a.txt", b"abc", 1_700_000_000);
    syncron_core::remote::mem::MemBackend::put_raw(&id, "Dok/Pod/b.txt", b"b", 1_700_000_000);
    let loc = Location::parse("Moja chmura:Dok");
    let l = loc.list(&Default::default()).unwrap();
    assert_eq!(l.files.len(), 2);
    assert_eq!(loc.list_dirs("").unwrap(), ["Pod"]);
    // test, przeglądanie, miejsce
    let tinfo = st.test("Moja chmura").unwrap();
    assert!(tinfo.info.len() < 200);
    let b = st.browse("Moja chmura", "Dok").unwrap();
    assert_eq!(
        b.iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect::<Vec<_>>(),
        [("Pod", true), ("a.txt", false)]
    );
    assert!(st.browse("Moja chmura", "../x").is_err());
    assert!(st.about("Moja chmura").unwrap().total.is_none());
    // usunięcie konta unieważnia lokalizację: błąd, nie panika
    st.delete("Moja chmura").unwrap();
    assert!(loc.list(&Default::default()).is_err());
    assert!(Location::parse("Nie ma takiego:x")
        .list(&Default::default())
        .is_err());
}

#[test]
fn account_flagged_needs_login_cannot_be_opened() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    st.add_split(
        accounts::Account {
            name: "gd".into(),
            kind: "mem".into(),
            params: kv(&[("id", "x")]),
            secret_keys: vec![],
            secret_store: String::new(),
            needs_login: true,
        },
        &BTreeMap::new(),
    )
    .unwrap();
    let e = st.open_backend("gd", "").err().unwrap();
    assert!(e.contains("zalogowania"), "{e}");
    assert!(st.list_view().unwrap()[0].needs_login);
}

// ======================= migracja rclone.conf =======================

#[test]
fn rclone_obscure_roundtrip_and_errors() {
    for plain in ["", "a", "hasło", SECRET, &"x".repeat(300), "😀 emoji"] {
        let ob = rclone_obscure_with_iv(plain, [7u8; 16]);
        assert!(
            !ob.contains('=') && !ob.contains('+') && !ob.contains('/'),
            "{ob}"
        );
        assert_eq!(rclone_reveal(&ob).unwrap(), plain);
    }
    // inny IV daje inny zapis, ten sam tekst
    assert_ne!(
        rclone_obscure_with_iv("x", [1; 16]),
        rclone_obscure_with_iv("x", [2; 16])
    );
    assert!(rclone_reveal("!!!").is_err());
    assert!(rclone_reveal("AAAA").is_err()); // krótsze niż IV
    assert!(rclone_reveal("").is_err());
}

fn sample_conf() -> String {
    let ob = |p: &str| rclone_obscure_with_iv(p, [9u8; 16]);
    format!(
        "# komentarz\n\
[Nextcloud]\ntype = webdav\nurl = https://dysk.firma.pl/remote.php/dav/files/jan\nvendor = nextcloud\nuser = jan\npass = {pw}\n\n\
[bitrix24]\ntype = webdav\nurl = https://firma.bitrix24.pl/docs/pub/x/\nuser = a@b.pl\npass = {pw}\n\n\
[serwer-ftp]\ntype = ftp\nhost = ftp.firma.pl\nport = 2121\nuser = ftpuser\npass = {pw}\nexplicit_tls = true\nno_check_certificate = true\n\n\
[ftp-zwykly]\ntype = ftp\nhost = ftp2.firma.pl\nuser = u\npass = {pw}\n\n\
[nas-sftp]\ntype = sftp\nhost = 10.0.0.5\nuser = admin\nkey_file = /home/jan/.ssh/id_ed25519\nkey_file_pass = {kp}\nknown_hosts_file = /home/jan/.ssh/known_hosts\n\n\
[s3-eu]\ntype = s3\nprovider = AWS\naccess_key_id = AKIAEXAMPLE\nsecret_access_key = jawny-klucz-tajny\nregion = eu-central-1\n\n\
[gdrive]\ntype = drive\nscope = drive\ntoken = {{\"access_token\":\"X\",\"refresh_token\":\"Y\"}}\n\n\
[onedrive]\ntype = onedrive\ndrive_id = ABC\ndrive_type = personal\ntoken = {{\"access_token\":\"X\"}}\n\n\
[dbx]\ntype = dropbox\ntoken = {{\"access_token\":\"X\"}}\n\n\
[smb-nas]\ntype = smb\nhost = nas\n\n\
[zaszyfrowany]\ntype = crypt\nremote = nextcloud:sejf\npassword = {pw}\n\n\
[zepsute]\ntype = webdav\nurl = https://x.pl\npass = !!!nie-obscure!!!\n\n\
[a:b]\ntype = webdav\nurl = https://x.pl\n",
        pw = ob("Haslo-z-rclone-ąę"),
        kp = ob("fraza klucza")
    )
}

#[test]
fn import_rclone_conf_creates_native_accounts_with_secrets() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    let conf = t.path().join("rclone.conf");
    fs::write(&conf, sample_conf()).unwrap();
    assert_eq!(
        accounts::pending_rclone_import(t.path()),
        Some(conf.clone())
    );

    let rep = st.import_rclone_conf(&conf).unwrap();
    assert_eq!(
        rep.imported,
        [
            "Nextcloud",
            "bitrix24",
            "serwer-ftp",
            "ftp-zwykly",
            "nas-sftp",
            "s3-eu"
        ]
    );
    assert!(rep.needs_login.is_empty());
    let skipped: Vec<&str> = rep.skipped.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        skipped,
        [
            "gdrive",
            "onedrive",
            "dbx",
            "smb-nas",
            "zaszyfrowany",
            "zepsute",
            "a:b"
        ],
        "{:?}",
        rep.skipped
    );
    for n in ["gdrive", "onedrive", "dbx", "zaszyfrowany"] {
        let r = &rep.skipped.iter().find(|s| s.name == n).unwrap().reason;
        assert!(r.contains("nieobsługiwane w tej wersji"), "{n}: {r}");
    }

    // webdav: hasło odwrócone i zapisane w magazynie sekretów
    let p = st.params_for("Nextcloud").unwrap();
    assert_eq!(
        p.get("url"),
        Some("https://dysk.firma.pl/remote.php/dav/files/jan")
    );
    assert_eq!(p.get("vendor"), Some("nextcloud"));
    assert_eq!(p.secret("pass"), Some("Haslo-z-rclone-ąę"));
    assert_eq!(
        st.params_for("bitrix24").unwrap().get("vendor"),
        Some("other")
    );
    // ftp: tryb TLS
    let f = st.get("serwer-ftp").unwrap();
    assert_eq!(
        (
            f.kind.as_str(),
            f.params["tls"].as_str(),
            f.params["port"].as_str()
        ),
        ("ftps", "explicit", "2121")
    );
    assert_eq!(f.params["no_check_certificate"], "true");
    assert_eq!(st.get("ftp-zwykly").unwrap().kind, "ftp");
    assert_eq!(st.get("ftp-zwykly").unwrap().params["tls"], "none");
    // sftp: ścieżki jawne, fraza klucza jako sekret
    let s = st.params_for("nas-sftp").unwrap();
    assert_eq!(s.get("key_file"), Some("/home/jan/.ssh/id_ed25519"));
    assert_eq!(s.secret("key_file_pass"), Some("fraza klucza"));
    // s3: klucz tajny w sekretach, identyfikator jawny
    let s3 = st.params_for("s3-eu").unwrap();
    assert_eq!(s3.secret("access_key_id"), Some("AKIAEXAMPLE"));
    assert_eq!(s3.secret("secret_access_key"), Some("jawny-klucz-tajny"));
    // nic jawnego na dysku poza źródłowym rclone.conf
    for (n, b) in all_files(t.path()) {
        if n.ends_with("rclone.conf") {
            continue;
        }
        for s in ["Haslo-z-rclone", "fraza klucza", "jawny-klucz-tajny"] {
            assert!(!contains(&b, s), "jawny sekret w {n}");
        }
    }
    // import wykonany: nie proponujemy ponownie; drugi import niczego nie dubluje
    assert_eq!(accounts::pending_rclone_import(t.path()), None);
    let rep2 = st.import_rclone_conf(&conf).unwrap();
    assert!(rep2.imported.is_empty() && rep2.needs_login.is_empty());
    assert_eq!(st.list().unwrap().len(), 6);
}

#[test]
fn import_offer_is_only_for_non_empty_conf_and_can_be_dismissed() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(accounts::pending_rclone_import(t.path()), None);
    fs::write(t.path().join("rclone.conf"), "").unwrap();
    assert_eq!(accounts::pending_rclone_import(t.path()), None);
    fs::write(t.path().join("rclone.conf"), "[a]\ntype = drive\n").unwrap();
    assert!(accounts::pending_rclone_import(t.path()).is_some());
    accounts::dismiss_rclone_import(t.path());
    assert_eq!(accounts::pending_rclone_import(t.path()), None);
}

#[test]
fn import_handles_odd_conf_files() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    // brak pliku
    assert!(st
        .import_rclone_conf(&t.path().join("nie ma.conf"))
        .is_err());
    // BOM UTF-8, końce linii CRLF, klucze bez sekcji, śmieci
    let conf = t.path().join("c.conf");
    fs::write(&conf, "\u{feff}osierocony = klucz\r\n[ok]\r\ntype = webdav\r\nurl = https://x.pl\r\n\r\n[puste]\r\n[typ-pusty]\r\ntype =\r\n").unwrap();
    let rep = st.import_rclone_conf(&conf).unwrap();
    assert_eq!(rep.imported, ["ok"]);
    assert_eq!(rep.skipped.len(), 2, "{:?}", rep.skipped);
}
