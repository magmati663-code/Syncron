//! QA8 T5: konta, sekrety, migracja rclone.
use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Barrier};
use syncron_core::accounts::{
    self, rclone_obscure_with_iv, rclone_reveal, Account, EncryptedFile, SecretStore, Store,
};

fn kv(p: &[(&str, &str)]) -> BTreeMap<String, String> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn files(dir: &std::path::Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn rclone_reveal_matches_independent_vectors() {
    // wektory wygenerowane niezależnie (python cryptography, AES-256-CTR, klucz rclone)
    assert_eq!(
        rclone_reveal("AAECAwQFBgcICQoLDA0OD3H-FFZwvTLu").unwrap(),
        "password"
    );
    assert_eq!(
        rclone_reveal("_____________________5ZyEh_fgU4bdsBFeDdiEFkoHyxCP55te-S1NNtsHRk").unwrap(),
        "zażółć gęślą jaźń 😀"
    );
    assert_eq!(
        rclone_reveal(
            "AAAAAAAAAAAAAAAAAAAA_jj3kSxhHU0HUMXGm0yCkRn9Izx_L9bDBW-5XFcuWeEXmJF3UCx52cs"
        )
        .unwrap(),
        "x".repeat(40)
    );
    assert_eq!(
        rclone_obscure_with_iv("password", std::array::from_fn(|i| i as u8)),
        "AAECAwQFBgcICQoLDA0OD3H-FFZwvTLu"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn concurrent_distinct_stores_lose_nothing() {
    let t = tempfile::tempdir().unwrap();
    let dir = t.path().to_path_buf();
    let hs: Vec<_> = (0..12)
        .map(|i| {
            let d = dir.clone();
            std::thread::spawn(move || {
                let st = Store::file_only(&d);
                st.add(
                    &format!("konto{i:02}"),
                    "webdav",
                    &kv(&[("url", "https://x.pl"), ("pass", &format!("pw{i}"))]),
                )
                .unwrap();
                st.set_secret(&format!("konto{i:02}"), "extra", &format!("e{i}"))
                    .unwrap();
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let st = Store::file_only(&dir);
    assert_eq!(st.list().unwrap().len(), 12);
    let ef = EncryptedFile::new(&dir);
    for i in 0..12 {
        let n = format!("konto{i:02}");
        assert_eq!(
            ef.get(&n, "pass").unwrap().as_deref(),
            Some(format!("pw{i}").as_str())
        );
        assert_eq!(
            ef.get(&n, "extra").unwrap().as_deref(),
            Some(format!("e{i}").as_str())
        );
    }
}

#[cfg(target_os = "linux")]
#[test]

fn same_name_race_keeps_winner_secrets() {
    for it in 0..40 {
        let t = tempfile::tempdir().unwrap();
        let b = Arc::new(Barrier::new(2));
        let hs: Vec<_> = (0..2)
            .map(|i| {
                let (d, b) = (t.path().to_path_buf(), b.clone());
                std::thread::spawn(move || {
                    let st = Store::file_only(&d);
                    let acc = Account {
                        name: "Wspolne".into(),
                        kind: "webdav".into(),
                        params: kv(&[("url", "https://x.pl")]),
                        secret_keys: vec![],
                        secret_store: String::new(),
                        needs_login: false,
                    };
                    b.wait();
                    st.add_split(acc, &kv(&[("pass", &format!("pw{i}"))]))
                        .is_ok()
                })
            })
            .collect();
        let r: Vec<bool> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(r.iter().filter(|x| **x).count(), 1);
        let win = r.iter().position(|x| *x).unwrap();
        let got = EncryptedFile::new(t.path()).get("Wspolne", "pass").unwrap();
        assert_eq!(
            got.as_deref(),
            Some(format!("pw{win}").as_str()),
            "iteracja {it}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]

fn slightly_malformed_key_does_not_wipe_secrets() {
    let t = tempfile::tempdir().unwrap();
    let ef = EncryptedFile::new(t.path());
    ef.set("a1", "pass", "tajne").unwrap();
    let kp = t.path().join("secrets.key");
    let mut k = fs::read(&kp).unwrap();
    k.push(b'\n');
    fs::write(&kp, &k).unwrap();
    ef.set("b2", "pass", "inne").unwrap();
    let kept_key = files(t.path())
        .iter()
        .any(|f| f.starts_with("secrets.key.corrupt"));
    assert!(kept_key || ef.get("a1", "pass").unwrap().is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn tampering_never_panics_or_returns_wrong_value() {
    let t = tempfile::tempdir().unwrap();
    let ef = EncryptedFile::new(t.path());
    ef.set("a1", "pass", "tajne").unwrap();
    let p = t.path().join("secrets.enc");
    let orig = fs::read(&p).unwrap();
    for i in 0..orig.len() {
        let mut m = orig.clone();
        m[i] ^= 1;
        fs::write(&p, &m).unwrap();
        assert_eq!(ef.get("a1", "pass").unwrap(), None, "bajt {i}");
    }
    for n in 0..orig.len() {
        fs::write(&p, &orig[..n]).unwrap();
        assert_eq!(ef.get("a1", "pass").unwrap(), None, "obcięcie {n}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn nonce_is_fresh_on_every_write_and_modes_are_0600() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let ef = EncryptedFile::new(t.path());
    let mut seen = std::collections::HashSet::new();
    for i in 0..200 {
        ef.set("a1", "pass", "to samo").unwrap();
        let b = fs::read(t.path().join("secrets.enc")).unwrap();
        assert!(
            seen.insert(b[8..32].to_vec()),
            "powtórzony nonce w zapisie {i}"
        );
    }
    let st = Store::file_only(t.path());
    st.add(
        "konto",
        "webdav",
        &kv(&[("url", "https://x.pl"), ("pass", "p")]),
    )
    .unwrap();
    for f in ["accounts.json", "secrets.enc", "secrets.key"] {
        let m = fs::metadata(t.path().join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "{f}");
    }
}

#[cfg(target_os = "linux")]
#[test]

fn corrupt_accounts_backup_not_spammed() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("accounts.json"), "{ nie json").unwrap();
    let st = Store::file_only(t.path());
    for _ in 0..25 {
        assert!(st.list().is_err());
    }
    let n = files(t.path())
        .iter()
        .filter(|f| f.contains(".corrupt-"))
        .count();
    assert!(n <= 1, "kopii: {n}");
}

#[cfg(target_os = "linux")]
#[test]

fn accounts_json_with_bom_is_read() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("accounts.json"),
        "\u{feff}{\"version\":1,\"accounts\":[]}",
    )
    .unwrap();
    assert!(Store::file_only(t.path()).list().is_ok());
}

#[test]
#[ignore = "BUG T5-5: Store::add przyjmuje rodzaj 'mem' (dane znikają) z poziomu add_remote"]
fn mem_kind_rejected_for_users() {
    let t = tempfile::tempdir().unwrap();
    assert!(Store::file_only(t.path())
        .add("pamiec", "mem", &kv(&[]))
        .is_err());
}

#[cfg(target_os = "linux")]
#[test]

fn url_userinfo_password_not_in_accounts_json() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    let _ = st.add(
        "dav",
        "webdav",
        &kv(&[("url", "https://jan:CanaryPW77@host.pl/dav")]),
    );
    let j = fs::read_to_string(t.path().join("accounts.json")).unwrap_or_default();
    assert!(!j.contains("CanaryPW77"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "BUG T5-7: import rclone omija check_new (limity długości/NUL) obowiązujące przy add"]
fn import_enforces_same_limits_as_add() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    let c = t.path().join("r.conf");
    fs::write(
        &c,
        format!(
            "[big]\ntype = webdav\nurl = https://x.pl/{}\n",
            "a".repeat(100_000)
        ),
    )
    .unwrap();
    let rep = st.import_rclone_conf(&c).unwrap();
    assert!(rep.imported.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn import_formats_and_no_overwrite() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    st.add(
        "istnieje",
        "webdav",
        &kv(&[("url", "https://stare.pl"), ("pass", "stare")]),
    )
    .unwrap();
    let ob = rclone_obscure_with_iv("nowe", [3; 16]);
    let conf = format!("[Istnieje]\ntype = webdav\nurl = https://nowe.pl\npass = {ob}\n[zażółć]\ntype = ftp\nhost = h\npass = {ob}\n");
    // UTF-16 LE z BOM (PowerShell >) i CRLF
    let text = conf.replace('\n', "\r\n");
    let mut b = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        b.extend(u.to_le_bytes());
    }
    let c = t.path().join("rclone.conf");
    fs::write(&c, b).unwrap();
    let rep = st.import_rclone_conf(&c).unwrap();
    assert_eq!(rep.imported, ["zażółć"]);
    assert_eq!(rep.skipped.len(), 1);
    assert_eq!(
        st.get("istnieje").unwrap().params["url"],
        "https://stare.pl"
    );
    assert_eq!(
        EncryptedFile::new(t.path())
            .get("istnieje", "pass")
            .unwrap()
            .as_deref(),
        Some("stare")
    );
    assert_eq!(
        EncryptedFile::new(t.path())
            .get("zażółć", "pass")
            .unwrap()
            .as_deref(),
        Some("nowe")
    );
    assert!(t.path().join("rclone-import.done").exists());
    assert!(accounts::pending_rclone_import(t.path()).is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn names_nfd_case_control_and_update_semantics() {
    let t = tempfile::tempdir().unwrap();
    let st = Store::file_only(t.path());
    let nfd = "Zazo\u{0301}łc";
    st.add(
        nfd,
        "webdav",
        &kv(&[("url", "https://x.pl"), ("pass", "p1")]),
    )
    .unwrap();
    assert!(st
        .add("ZAZÓŁC", "webdav", &kv(&[("url", "https://x.pl")]))
        .is_err());
    for bad in [
        "a\u{0}b",
        "a\nb",
        "ab\u{202e}",
        "a\u{200b}b",
        "x",
        &"z".repeat(65),
    ] {
        assert!(st.add(bad, "webdav", &kv(&[])).is_err(), "{bad:?}");
    }
    assert!(st.add(&"ż".repeat(64), "webdav", &kv(&[])).is_ok());
    // pusty pass = zostaw; inne wielkości liter
    st.update("ZAZÓŁC", &kv(&[("url", "https://y.pl"), ("pass", "")]))
        .unwrap();
    let ef = EncryptedFile::new(t.path());
    assert_eq!(ef.get("Zazółc", "pass").unwrap().as_deref(), Some("p1"));
    st.delete("zazółc").unwrap();
    assert_eq!(ef.get("Zazółc", "pass").unwrap(), None);
    st.add("Zazółc", "webdav", &kv(&[("url", "https://x.pl")]))
        .unwrap();
    assert!(st.params_for_test_free());
}

trait Free {
    fn params_for_test_free(&self) -> bool;
}
impl Free for Store {
    fn params_for_test_free(&self) -> bool {
        self.get("zazółc").unwrap().secret_keys.is_empty()
    }
}

#[cfg(target_os = "linux")]
#[test]
fn no_canary_leaks_on_disk_or_errors() {
    let t = tempfile::tempdir().unwrap();
    let st = Arc::new(Store::file_only(t.path()));
    let c = "CANARY-s3cr3t-Zq9";
    let mut errs = String::new();
    for (n, k, f) in [
        (
            "w",
            "webdav",
            kv(&[("url", "http://127.0.0.1:1/d"), ("user", "u"), ("pass", c)]),
        ),
        (
            "f",
            "ftp",
            kv(&[("host", "127.0.0.1"), ("port", "1"), ("pass", c)]),
        ),
        (
            "s",
            "sftp",
            kv(&[
                ("host", "127.0.0.1"),
                ("port", "1"),
                ("user", "u"),
                ("pass", c),
                ("key_file_pass", c),
            ]),
        ),
        (
            "s3",
            "s3",
            kv(&[
                ("provider", "MinIO"),
                ("endpoint", "http://127.0.0.1:1"),
                ("access_key_id", c),
                ("secret_access_key", c),
            ]),
        ),
    ] {
        let name = format!("acc-{n}");
        st.add(&name, k, &f).unwrap();
        errs += &format!("{:?}{:?}", st.test(&name), st.browse(&name, ""));
        errs += &format!("{:?}", st.list_view().unwrap());
        errs += &format!("{:?}", st.get(&name).unwrap());
        st.update(&name, &f).unwrap();
    }
    assert!(!errs.contains(c), "{errs}");
    for f in files(t.path()) {
        let b = fs::read(t.path().join(&f)).unwrap_or_default();
        assert!(!b.windows(c.len()).any(|w| w == c.as_bytes()), "{f}");
    }
    for n in ["acc-w", "acc-f", "acc-s", "acc-s3"] {
        st.delete(n).unwrap();
    }
    assert!(EncryptedFile::new(t.path())
        .get("acc-w", "pass")
        .unwrap()
        .is_none());
}
