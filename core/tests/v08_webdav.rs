//! Testy natywnego klienta WebDAV na prawdziwym (minimalnym) serwerze WebDAV uruchamianym w procesie testowym
//! (127.0.0.1, losowy port, katalog tymczasowy). Serwer obsługuje PROPFIND/GET/PUT/DELETE/MKCOL/MOVE/PROPPATCH
//! i potrafi symulować awarie (zerwanie połączenia, `..` w href, przekierowania, kody 423/507, Digest).
use filetime::FileTime;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Instant;
use syncron_core::model::Bandwidth;
use syncron_core::remote::http::{url_decode, url_path};
use syncron_core::remote::{self, Backend, Params, RError, Xfer};

#[derive(Default)]
struct Cfg {
    /// oczekiwany nagłówek Authorization
    auth: Option<String>,
    digest_only: bool,
    /// 0 = `D:`, 1 = `d:`, 2 = domyślna przestrzeń nazw
    style: AtomicU8,
    evil: AtomicBool,
    no_self: AtomicBool,
    honor_xoc: AtomicBool,
    honor_proppatch: AtomicBool,
    drop_put: AtomicUsize,
    drop_get: AtomicBool,
    drop_propfind: AtomicBool,
    redirect: AtomicBool,
    put_status: AtomicU16,
    puts: AtomicUsize,
}

struct Srv {
    port: u16,
    dir: tempfile::TempDir,
    cfg: Arc<Cfg>,
    stop: Arc<AtomicBool>,
}

impl Drop for Srv {
    fn drop(&mut self) {
        self.stop.store(true, SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn start(cfg: Cfg) -> Srv {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let cfg = Arc::new(cfg);
    let stop = Arc::new(AtomicBool::new(false));
    let (root, c2, s2) = (dir.path().to_path_buf(), cfg.clone(), stop.clone());
    std::thread::spawn(move || {
        for s in l.incoming() {
            if s2.load(SeqCst) {
                break;
            }
            if let Ok(s) = s {
                let (root, cfg) = (root.clone(), c2.clone());
                std::thread::spawn(move || {
                    let _ = handle(s, &root, &cfg);
                });
            }
        }
    });
    Srv {
        port,
        dir,
        cfg,
        stop,
    }
}

fn reply(
    s: &mut TcpStream,
    code: u16,
    hdrs: &[(&str, String)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut h = format!(
        "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in hdrs {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    s.write_all(h.as_bytes())?;
    s.write_all(body)?;
    s.flush()?;
    let _ = s.shutdown(Shutdown::Write);
    Ok(())
}

fn fs_path(root: &Path, url_path_: &str, prefix: &str) -> Option<PathBuf> {
    let p = url_decode(url_path_.split('?').next().unwrap());
    let rest = p.strip_prefix(prefix)?;
    let mut out = root.to_path_buf();
    for c in rest.split('/').filter(|c| !c.is_empty()) {
        if c == ".." {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

fn rfc1123(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

fn mtime_of(md: &std::fs::Metadata) -> i64 {
    FileTime::from_last_modification_time(md).unix_seconds()
}

fn entry_xml(style: u8, href: &str, md: &std::fs::Metadata) -> String {
    let p = match style {
        0 => "D:",
        1 => "d:",
        _ => "",
    };
    let rt = if md.is_dir() {
        format!("<{p}collection/>")
    } else {
        String::new()
    };
    let file = if md.is_dir() {
        String::new()
    } else {
        format!(
            "<{p}getcontentlength>{}</{p}getcontentlength><{p}getlastmodified>{}</{p}getlastmodified><{p}getetag>\"e{}\"</{p}getetag>",
            md.len(),
            rfc1123(mtime_of(md)),
            md.len()
        )
    };
    format!(
        "<{p}response><{p}href>{href}</{p}href><{p}propstat><{p}prop><{p}resourcetype>{rt}</{p}resourcetype>{file}</{p}prop><{p}status>HTTP/1.1 200 OK</{p}status></{p}propstat></{p}response>"
    )
}

fn handle(mut s: TcpStream, root: &Path, cfg: &Cfg) -> std::io::Result<()> {
    let mut rd = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    rd.read_line(&mut line)?;
    let mut it = line.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let target = it.next().unwrap_or("").to_string();
    let mut hdr = std::collections::HashMap::new();
    loop {
        let mut l = String::new();
        rd.read_line(&mut l)?;
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            hdr.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let clen: usize = hdr
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // ---- uwierzytelnianie
    if cfg.digest_only {
        return reply(
            &mut s,
            401,
            &[("WWW-Authenticate", "Digest realm=\"x\", nonce=\"y\"".into())],
            b"",
        );
    }
    if let Some(a) = &cfg.auth {
        if hdr.get("authorization") != Some(a) {
            return reply(
                &mut s,
                401,
                &[("WWW-Authenticate", "Basic realm=\"x\"".into())],
                b"nope",
            );
        }
    }
    // ---- przekierowanie /old/... -> /dav/...
    if cfg.redirect.load(SeqCst) {
        if let Some(rest) = target.strip_prefix("/old") {
            return reply(&mut s, 307, &[("Location", format!("/dav{rest}"))], b"");
        }
    }
    let Some(path) = fs_path(root, &target, "/dav") else {
        return reply(&mut s, 403, &[], b"");
    };
    let style = cfg.style.load(SeqCst);
    match method.as_str() {
        "PROPFIND" => {
            let mut body = vec![0u8; clen];
            rd.read_exact(&mut body)?;
            if cfg.drop_propfind.load(SeqCst) {
                return Ok(());
            }
            if !path.exists() {
                return reply(&mut s, 404, &[], b"");
            }
            let depth = hdr.get("depth").map(|s| s.as_str()).unwrap_or("1");
            let href_of = |p: &Path| {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut h = format!("/dav/{}", url_path(&rel));
                if p.is_dir() && !h.ends_with('/') {
                    h.push('/');
                }
                h
            };
            let mut xml = String::new();
            if !cfg.no_self.load(SeqCst) {
                xml.push_str(&entry_xml(style, &href_of(&path), &path.metadata()?));
            }
            if depth != "0" && path.is_dir() {
                let mut kids: Vec<_> = std::fs::read_dir(&path)?.flatten().collect();
                kids.sort_by_key(|k| k.file_name());
                for k in kids {
                    let name = k.file_name().to_string_lossy().to_string();
                    if name.starts_with(".dav-tmp") {
                        continue;
                    }
                    xml.push_str(&entry_xml(style, &href_of(&k.path()), &k.metadata()?));
                }
                if cfg.evil.load(SeqCst) {
                    let md = path.metadata()?;
                    for h in [
                        "/dav/../secret.txt",
                        "/dav/%2e%2e/secret2.txt",
                        "/dav/a%2F..%2Fb.txt",
                        "/elsewhere/x.txt",
                        "/dav/",
                        "/dav/sub/../../../etc/passwd",
                    ] {
                        xml.push_str(&entry_xml(style, h, &md));
                    }
                }
            }
            let ns = match style {
                0 => "D:multistatus xmlns:D=\"DAV:\"".to_string(),
                1 => "d:multistatus xmlns:d=\"DAV:\"".to_string(),
                _ => "multistatus xmlns=\"DAV:\"".to_string(),
            };
            let close = match style {
                0 => "D:multistatus",
                1 => "d:multistatus",
                _ => "multistatus",
            };
            let out = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><{ns}>{xml}</{close}>");
            reply(
                &mut s,
                207,
                &[("Content-Type", "application/xml".into())],
                out.as_bytes(),
            )
        }
        "GET" => {
            if !path.is_file() {
                return reply(&mut s, 404, &[], b"");
            }
            let data = std::fs::read(&path)?;
            if cfg.drop_get.load(SeqCst) {
                s.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len()).as_bytes(),
                )?;
                s.write_all(&data[..data.len() / 2])?;
                s.flush()?;
                let _ = s.shutdown(Shutdown::Both);
                return Ok(());
            }
            reply(&mut s, 200, &[], &data)
        }
        "PUT" => {
            cfg.puts.fetch_add(1, SeqCst);
            let mut body = vec![0u8; clen];
            if cfg
                .drop_put
                .fetch_update(SeqCst, SeqCst, |v| v.checked_sub(1))
                .is_ok()
            {
                let _ = rd.read_exact(&mut body[..clen / 2]);
                let _ = s.shutdown(Shutdown::Both);
                return Ok(());
            }
            rd.read_exact(&mut body)?;
            let st = cfg.put_status.load(SeqCst);
            if st != 0 {
                return reply(&mut s, st, &[], b"");
            }
            if !path.parent().is_some_and(|p| p.is_dir()) {
                return reply(&mut s, 409, &[], b"");
            }
            let tmp = path.with_file_name(".dav-tmp");
            std::fs::write(&tmp, &body)?;
            std::fs::rename(&tmp, &path)?;
            let mut h = vec![];
            if cfg.honor_xoc.load(SeqCst) {
                if let Some(t) = hdr.get("x-oc-mtime").and_then(|v| v.parse::<i64>().ok()) {
                    filetime::set_file_mtime(&path, FileTime::from_unix_time(t, 0))?;
                    h.push(("X-OC-Mtime", "accepted".to_string()));
                }
            }
            reply(&mut s, 201, &h, b"")
        }
        "PROPPATCH" => {
            let mut body = vec![0u8; clen];
            rd.read_exact(&mut body)?;
            if !cfg.honor_proppatch.load(SeqCst) {
                return reply(&mut s, 403, &[], b"");
            }
            let b = String::from_utf8_lossy(&body).to_string();
            let a = b.find("<d:getlastmodified>").unwrap() + "<d:getlastmodified>".len();
            let e = b.find("</d:getlastmodified>").unwrap();
            let t = chrono::DateTime::parse_from_rfc2822(&b[a..e])
                .unwrap()
                .timestamp();
            filetime::set_file_mtime(&path, FileTime::from_unix_time(t, 0))?;
            reply(&mut s, 207, &[], b"<multistatus xmlns=\"DAV:\"/>")
        }
        "DELETE" => {
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else if path.is_file() {
                std::fs::remove_file(&path)?;
            } else {
                return reply(&mut s, 404, &[], b"");
            }
            reply(&mut s, 204, &[], b"")
        }
        "MKCOL" => {
            if path.exists() {
                return reply(&mut s, 405, &[], b"");
            }
            if !path.parent().is_some_and(|p| p.is_dir()) {
                return reply(&mut s, 409, &[], b"");
            }
            std::fs::create_dir(&path)?;
            reply(&mut s, 201, &[], b"")
        }
        "MOVE" => {
            let dest = hdr.get("destination").cloned().unwrap_or_default();
            assert_eq!(hdr.get("overwrite").map(|s| s.as_str()), Some("T"));
            let dp = dest.split_once("://").map(|x| x.1).unwrap_or(&dest);
            let dp = &dp[dp.find('/').unwrap_or(0)..];
            let Some(to) = fs_path(root, dp, "/dav") else {
                return reply(&mut s, 403, &[], b"");
            };
            if !path.exists() {
                return reply(&mut s, 404, &[], b"");
            }
            if !to.parent().is_some_and(|p| p.is_dir()) {
                return reply(&mut s, 409, &[], b"");
            }
            std::fs::rename(&path, &to)?;
            reply(&mut s, 201, &[], b"")
        }
        _ => reply(&mut s, 405, &[], b""),
    }
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in data.chunks(3) {
        let n = (ch[0] as u32) << 16
            | (*ch.get(1).unwrap_or(&0) as u32) << 8
            | *ch.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if ch.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if ch.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn basic(u: &str, p: &str) -> String {
    format!("Basic {}", b64(format!("{u}:{p}").as_bytes()))
}

fn params(srv: &Srv, vendor: &str, prefix: &str, pass: &str) -> Params {
    let mut p = Params::default();
    p.public.insert(
        "url".into(),
        format!("http://127.0.0.1:{}{prefix}", srv.port),
    );
    p.public.insert("vendor".into(), vendor.into());
    p.public.insert("user".into(), "jan".into());
    p.public.insert("retry_delay_ms".into(), "20".into());
    p.secret.insert("pass".into(), pass.into());
    p
}

fn open(srv: &Srv, vendor: &str) -> Box<dyn Backend> {
    remote::open("webdav", &params(srv, vendor, "/dav/", "tajne"), "").unwrap()
}

fn authed() -> Cfg {
    Cfg {
        auth: Some(basic("jan", "tajne")),
        ..Default::default()
    }
}

fn write(p: &Path, data: &[u8]) {
    std::fs::write(p, data).unwrap();
}

fn x(bw: &Bandwidth) -> Xfer<'_> {
    Xfer::new(bw)
}

#[test]
fn roundtrip_polish_names_spaces_and_mtime_nextcloud_header() {
    let srv = start(authed());
    srv.cfg.honor_xoc.store(true, SeqCst);
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let bw = Bandwidth::default();
    let names = [
        "Zażółć gęślą/jaźń #1 (kopia) & więcej.txt",
        "a b/c d/e f.txt",
        "płaski plik.bin",
    ];
    for (i, n) in names.iter().enumerate() {
        let src = work.path().join("src");
        write(&src, format!("treść {i} ąęć").as_bytes());
        b.upload(&src, n, 1_600_000_000 + i as i64 * 1000, &x(&bw))
            .unwrap();
        assert_eq!(
            std::fs::read(srv.dir.path().join(n)).unwrap(),
            format!("treść {i} ąęć").as_bytes()
        );
    }
    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 3, "{l:?}");
    for (i, n) in names.iter().enumerate() {
        let e = l
            .iter()
            .find(|e| e.path == *n)
            .unwrap_or_else(|| panic!("brak {n}: {l:?}"));
        assert_eq!(e.meta.mtime, 1_600_000_000 + i as i64 * 1000);
        let st = b.stat(n).unwrap().unwrap();
        assert_eq!(st.meta.size, e.meta.size);
        assert_eq!(st.meta.mtime, e.meta.mtime);
        let dst = work.path().join("dst");
        b.download(n, &dst, &x(&bw)).unwrap();
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            format!("treść {i} ąęć").as_bytes()
        );
    }
    // listing podfolderu zwraca ścieżki względem korzenia
    let sub = b.list_files("a b").unwrap();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].path, "a b/c d/e f.txt");
    // przeglądarka folderów
    let mut d: Vec<_> = b
        .list_dir("")
        .unwrap()
        .into_iter()
        .map(|i| (i.name, i.is_dir))
        .collect();
    d.sort();
    assert!(d.contains(&("a b".to_string(), true)));
    assert!(d.contains(&("płaski plik.bin".to_string(), false)));
    assert!(b.stat("a b").unwrap().is_none(), "stat folderu = None");
}

#[test]
fn mtime_via_proppatch_for_other_vendor() {
    let srv = start(authed());
    srv.cfg.honor_proppatch.store(true, SeqCst);
    let b = open(&srv, "other");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"abc");
    b.upload(&src, "x/y.txt", 1_500_000_123, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(
        b.stat("x/y.txt").unwrap().unwrap().meta.mtime,
        1_500_000_123
    );
}

#[test]
fn mtime_unsupported_server_is_tolerated() {
    let srv = start(authed());
    let b = open(&srv, "other");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"abc");
    b.upload(&src, "y.txt", 1_500_000_123, &x(&Bandwidth::default()))
        .unwrap();
    assert!(b.stat("y.txt").unwrap().is_some());
}

#[test]
fn big_file_with_speed_limit() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("big.bin");
    let mut data = vec![0u8; 20 * 1024 * 1024];
    let mut v = 12345u32;
    for c in data.iter_mut() {
        v = v.wrapping_mul(1664525).wrapping_add(1013904223);
        *c = (v >> 24) as u8;
    }
    write(&src, &data);
    let bw = Bandwidth {
        kbps: 10 * 1024,
        ..Default::default()
    };
    let sent = std::sync::atomic::AtomicU64::new(0);
    let prog = |n: u64| {
        sent.fetch_add(n, SeqCst);
    };
    let mut xf = Xfer::new(&bw);
    xf.progress = Some(&prog);
    let t = Instant::now();
    b.upload(&src, "duzy/big.bin", 1_600_000_000, &xf).unwrap();
    let el = t.elapsed().as_secs_f64();
    assert!(el >= 1.6, "limit prędkości nie działa: {el:.2}s");
    assert_eq!(srv.cfg.puts.load(SeqCst), 1, "liczba PUT");
    assert_eq!(sent.load(SeqCst), data.len() as u64);
    assert_eq!(
        std::fs::read(srv.dir.path().join("duzy/big.bin"))
            .unwrap()
            .len(),
        data.len()
    );
    let dst = work.path().join("back.bin");
    let t = Instant::now();
    b.download("duzy/big.bin", &dst, &xf).unwrap();
    assert!(t.elapsed().as_secs_f64() >= 1.6);
    assert!(std::fs::read(&dst).unwrap() == data);
}

#[test]
fn cancel_stops_upload() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("c.bin");
    write(&src, &vec![7u8; 3 * 1024 * 1024]);
    let bw = Bandwidth {
        kbps: 512,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let mut xf = Xfer::new(&bw);
    xf.cancel = Some(&cancel);
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            cancel.store(true, SeqCst);
        });
        let t = Instant::now();
        assert!(b.upload(&src, "c.bin", 0, &xf).is_err());
        assert!(
            t.elapsed().as_secs() < 4,
            "anulowanie nie przerwało wysyłania"
        );
    });
}

#[test]
fn connection_dropped_during_upload() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("d.bin");
    write(&src, &vec![1u8; 2 * 1024 * 1024]);
    let bw = Bandwidth::default();
    // jednorazowe zerwanie: ponowienie kończy się sukcesem
    srv.cfg.drop_put.store(1, SeqCst);
    b.upload(&src, "ok.bin", 0, &x(&bw)).unwrap();
    assert_eq!(
        srv.dir.path().join("ok.bin").metadata().unwrap().len(),
        2 * 1024 * 1024
    );
    // stałe zerwanie: błąd sieci i brak pliku docelowego
    srv.cfg.drop_put.store(100, SeqCst);
    let e = b.upload(&src, "zle.bin", 0, &x(&bw)).unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
    assert!(!srv.dir.path().join("zle.bin").exists());
}

#[test]
fn connection_dropped_during_download_and_listing() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    write(&srv.dir.path().join("f.bin"), &vec![5u8; 1024 * 1024]);
    let work = tempfile::tempdir().unwrap();
    let dst = work.path().join("f.bin");
    srv.cfg.drop_get.store(true, SeqCst);
    let e = b
        .download("f.bin", &dst, &x(&Bandwidth::default()))
        .unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
    assert!(!dst.exists(), "częściowy plik powinien zostać usunięty");
    // zerwany listing = Err (silnik nie może uznać folderu za pusty)
    srv.cfg.drop_propfind.store(true, SeqCst);
    let e = b.list_files("").unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
}

#[test]
fn wrong_password_and_digest() {
    let srv = start(authed());
    let bad = remote::open("webdav", &params(&srv, "other", "/dav/", "zle"), "").unwrap();
    assert!(matches!(bad.list_files("").unwrap_err(), RError::Auth(_)));
    assert!(matches!(bad.ping().unwrap_err(), RError::Auth(_)));
    let mut p = params(&srv, "other", "/dav/", "");
    p.public.remove("user");
    let anon = remote::open("webdav", &p, "").unwrap();
    assert!(matches!(anon.list_dir("").unwrap_err(), RError::Auth(_)));
    let good = open(&srv, "other");
    assert!(good.ping().is_ok());

    let dg = start(Cfg {
        digest_only: true,
        ..Default::default()
    });
    let e = open(&dg, "other").list_files("").unwrap_err();
    match e {
        RError::Auth(m) => assert!(m.contains("Digest"), "{m}"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn bearer_token_is_sent() {
    let srv = start(Cfg {
        auth: Some("Bearer tok123".into()),
        ..Default::default()
    });
    let mut p = params(&srv, "nextcloud", "/dav/", "");
    p.public.remove("user");
    p.secret.insert("bearer".into(), "tok123".into());
    let b = remote::open("webdav", &p, "").unwrap();
    assert!(b.list_files("").unwrap().is_empty());
}

#[test]
fn empty_and_missing_folders() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    assert!(b.list_files("").unwrap().is_empty());
    assert!(b.list_files("nie ma takiego").unwrap().is_empty());
    assert!(matches!(
        b.list_dir("nie ma takiego").unwrap_err(),
        RError::NotFound(_)
    ));
    assert!(b.stat("nie/ma.txt").unwrap().is_none());
    b.delete("nie/ma.txt").unwrap();
    // korzeń nieistniejący: ping zgłasza błąd, listing pusty
    let r = remote::open(
        "webdav",
        &params(&srv, "other", "/dav/", "tajne"),
        "Brak/Folderu",
    )
    .unwrap();
    assert!(r.list_files("").unwrap().is_empty());
    assert!(matches!(r.ping().unwrap_err(), RError::NotFound(_)));
}

#[test]
fn dotdot_and_foreign_hrefs_are_ignored() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    write(&srv.dir.path().join("dobry.txt"), b"ok");
    std::fs::create_dir(srv.dir.path().join("sub")).unwrap();
    write(&srv.dir.path().join("sub/inny.txt"), b"ok2");
    srv.cfg.evil.store(true, SeqCst);
    let l = b.list_files("").unwrap();
    let mut paths: Vec<_> = l.iter().map(|e| e.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["dobry.txt", "sub/inny.txt"], "{l:?}");
    let d = b.list_dir("").unwrap();
    assert_eq!(d.len(), 2, "{d:?}");
    // lokalne wejście z `..` jest odrzucane
    assert!(b.stat("../x").is_err());
    assert!(b.delete("a/../../x").is_err());
}

#[test]
fn missing_self_entry_is_error_not_empty() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    write(&srv.dir.path().join("a.txt"), b"1");
    srv.cfg.no_self.store(true, SeqCst);
    assert!(b.list_files("").is_err());
}

#[test]
fn namespace_styles_parse_equally() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    std::fs::create_dir_all(srv.dir.path().join("k/l")).unwrap();
    write(&srv.dir.path().join("k/l/m.txt"), b"123");
    write(&srv.dir.path().join("n.txt"), b"1");
    let mut prev = None;
    for st in 0..3 {
        srv.cfg.style.store(st, SeqCst);
        let l = b.list_files("").unwrap();
        assert_eq!(l.len(), 2, "styl {st}: {l:?}");
        if let Some(p) = &prev {
            assert_eq!(&l, p);
        }
        prev = Some(l);
    }
}

#[test]
fn parallel_uploads_and_deep_listing() {
    let srv = start(authed());
    let b: Arc<dyn Backend> = Arc::from(open(&srv, "nextcloud"));
    let work = Arc::new(tempfile::tempdir().unwrap());
    let mut hs = vec![];
    for t in 0..8 {
        let (b, work) = (b.clone(), work.clone());
        hs.push(std::thread::spawn(move || {
            let bw = Bandwidth::default();
            for i in 0..5 {
                let src = work.path().join(format!("s{t}_{i}"));
                write(&src, format!("{t}-{i}").as_bytes());
                b.upload(
                    &src,
                    &format!("w{t}/g {i}/głęboko/plik {i}.txt"),
                    1_600_000_000,
                    &x(&bw),
                )
                .unwrap();
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 40);
    for t in 0..8 {
        for i in 0..5 {
            let p = format!("w{t}/g {i}/głęboko/plik {i}.txt");
            let e = l
                .iter()
                .find(|e| e.path == p)
                .unwrap_or_else(|| panic!("brak {p}"));
            assert_eq!(e.meta.size, format!("{t}-{i}").len() as u64);
        }
    }
}

#[test]
fn overwrite_rename_mkdirs_rmdir() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    let bw = Bandwidth::default();
    write(&src, b"wersja pierwsza, dluzsza");
    b.upload(&src, "p/plik.txt", 1_000, &x(&bw)).unwrap();
    write(&src, b"v2");
    b.upload(&src, "p/plik.txt", 2_000, &x(&bw)).unwrap();
    assert_eq!(
        std::fs::read(srv.dir.path().join("p/plik.txt")).unwrap(),
        b"v2"
    );
    assert_eq!(b.stat("p/plik.txt").unwrap().unwrap().meta.size, 2);

    // rename do nieistniejących folderów (tworzy rekurencyjnie)
    b.rename("p/plik.txt", "n1/n2/n3/nowy ł.txt").unwrap();
    assert!(!srv.dir.path().join("p/plik.txt").exists());
    assert_eq!(
        std::fs::read(srv.dir.path().join("n1/n2/n3/nowy ł.txt")).unwrap(),
        b"v2"
    );
    // rename nadpisuje cel
    write(&src, b"inny");
    b.upload(&src, "cel.txt", 0, &x(&bw)).unwrap();
    b.rename("n1/n2/n3/nowy ł.txt", "cel.txt").unwrap();
    assert_eq!(
        std::fs::read(srv.dir.path().join("cel.txt")).unwrap(),
        b"v2"
    );
    assert!(matches!(
        b.rename("nie/ma", "x").unwrap_err(),
        RError::NotFound(_)
    ));

    b.mkdirs("m1/m2/m3 z spacją").unwrap();
    assert!(srv.dir.path().join("m1/m2/m3 z spacją").is_dir());
    b.mkdirs("m1/m2/m3 z spacją").unwrap();

    b.rmdir_if_empty("m1/m2/m3 z spacją").unwrap();
    assert!(!srv.dir.path().join("m1/m2/m3 z spacją").exists());
    assert!(srv.dir.path().join("m1/m2").is_dir());
    // niepusty folder zostaje
    write(&srv.dir.path().join("m1/m2/zostaje.txt"), b"x");
    b.rmdir_if_empty("m1/m2").unwrap();
    assert!(srv.dir.path().join("m1/m2/zostaje.txt").exists());
    // usuwanie pliku
    b.delete("cel.txt").unwrap();
    assert!(!srv.dir.path().join("cel.txt").exists());
}

#[test]
fn root_folder_is_respected() {
    let srv = start(authed());
    let b = remote::open(
        "webdav",
        &params(&srv, "nextcloud", "/dav/", "tajne"),
        "Praca/Dokumenty ł",
    )
    .unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"1");
    b.upload(&src, "a/b.txt", 5, &x(&Bandwidth::default()))
        .unwrap();
    assert!(srv.dir.path().join("Praca/Dokumenty ł/a/b.txt").is_file());
    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].path, "a/b.txt");
}

#[test]
fn redirects_keep_method() {
    let srv = start(authed());
    srv.cfg.redirect.store(true, SeqCst);
    let b = remote::open("webdav", &params(&srv, "nextcloud", "/old/", "tajne"), "").unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"przez przekierowanie");
    b.upload(&src, "r/plik.txt", 0, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(
        std::fs::read(srv.dir.path().join("r/plik.txt")).unwrap(),
        b"przez przekierowanie"
    );
    assert_eq!(b.list_files("").unwrap().len(), 1);
    let dst = work.path().join("d");
    b.download("r/plik.txt", &dst, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"przez przekierowanie");
}

#[test]
fn status_mapping_locked_and_quota() {
    let srv = start(authed());
    let b = open(&srv, "nextcloud");
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"1");
    srv.cfg.put_status.store(423, SeqCst);
    assert!(matches!(
        b.upload(&src, "a.txt", 0, &x(&Bandwidth::default()))
            .unwrap_err(),
        RError::Busy(_)
    ));
    srv.cfg.put_status.store(507, SeqCst);
    assert!(matches!(
        b.upload(&src, "a.txt", 0, &x(&Bandwidth::default()))
            .unwrap_err(),
        RError::Quota(_)
    ));
    srv.cfg.put_status.store(403, SeqCst);
    assert!(matches!(
        b.upload(&src, "a.txt", 0, &x(&Bandwidth::default()))
            .unwrap_err(),
        RError::Auth(_)
    ));
    // 503 jest ponawiane i na końcu zgłaszane jako Busy
    srv.cfg.puts.store(0, SeqCst);
    srv.cfg.put_status.store(503, SeqCst);
    assert!(matches!(
        b.upload(&src, "a.txt", 0, &x(&Bandwidth::default()))
            .unwrap_err(),
        RError::Busy(_)
    ));
    assert_eq!(srv.cfg.puts.load(SeqCst), 4);
}

#[test]
fn bad_urls_are_rejected() {
    let mut p = Params::default();
    assert!(remote::open("webdav", &p, "").is_err());
    p.public.insert("url".into(), "ftp://x/y".into());
    assert!(remote::open("webdav", &p, "").is_err());
    p.public.insert("url".into(), "https://u:p@host/dav".into());
    assert!(remote::open("webdav", &p, "").is_err());
}
