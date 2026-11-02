//! Testy natywnego klienta FTP/FTPS na prawdziwym (minimalnym) serwerze FTP uruchamianym w procesie testowym
//! (127.0.0.1, losowy port, katalog tymczasowy jako korzeń). Serwer obsługuje USER/PASS/FEAT/OPTS/TYPE/PWD/CWD/PASV/
//! EPSV/PORT/EPRT/LIST/MLSD/RETR/STOR/DELE/RNFR/RNTO/MKD/RMD/SIZE/MDTM/MFMT/NOOP/QUIT, a także AUTH TLS + PBSZ/PROT
//! (FTPS jawny) i TLS niejawny, oraz potrafi symulować awarie (zerwanie połączenia, `..` w nazwach, brak MLSD/MFMT).
use filetime::FileTime;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use syncron_core::model::Bandwidth;
use syncron_core::remote::{self, Backend, Params, RError, Xfer};

trait Stream: Read + Write + Send {
    fn tcp(&self) -> &TcpStream;
    fn close(&mut self) {}
}
impl Stream for TcpStream {
    fn tcp(&self) -> &TcpStream {
        self
    }
}
impl Stream for rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    fn tcp(&self) -> &TcpStream {
        &self.sock
    }
    fn close(&mut self) {
        self.conn.send_close_notify();
        let _ = self.conn.complete_io(&mut self.sock);
    }
}

struct Cfg {
    pass: String,
    mlsd: AtomicBool,
    mfmt: AtomicBool,
    mdtm_set: AtomicBool,
    rnto_overwrite: AtomicBool,
    evil: AtomicBool,
    drop_stor: AtomicUsize,
    drop_retr: AtomicBool,
    drop_mlsd: AtomicBool,
    /// sesje o niższym numerze generacji zamykają się przy następnym poleceniu
    kill_gen: AtomicUsize,
    cur: AtomicUsize,
    peak: AtomicUsize,
    logins: AtomicUsize,
    stor_names: Mutex<Vec<String>>,
    tls: Option<Arc<rustls::ServerConfig>>,
    implicit: bool,
}

impl Cfg {
    fn new() -> Cfg {
        Cfg {
            pass: "tajne".into(),
            mlsd: AtomicBool::new(true),
            mfmt: AtomicBool::new(true),
            mdtm_set: AtomicBool::new(true),
            rnto_overwrite: AtomicBool::new(true),
            evil: AtomicBool::new(false),
            drop_stor: AtomicUsize::new(0),
            drop_retr: AtomicBool::new(false),
            drop_mlsd: AtomicBool::new(false),
            kill_gen: AtomicUsize::new(0),
            cur: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            logins: AtomicUsize::new(0),
            stor_names: Mutex::new(vec![]),
            tls: None,
            implicit: false,
        }
    }
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
                    let n = cfg.cur.fetch_add(1, SeqCst) + 1;
                    cfg.peak.fetch_max(n, SeqCst);
                    let _ = session(s, &root, &cfg);
                    cfg.cur.fetch_sub(1, SeqCst);
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

fn ftp_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y%m%d%H%M%S")
        .to_string()
}

fn mtime_of(p: &Path) -> i64 {
    FileTime::from_last_modification_time(&p.metadata().unwrap()).unix_seconds()
}

enum Data {
    Pasv(TcpListener),
    Port(std::net::SocketAddr),
}

struct Sess {
    ctl: BufReader<Box<dyn Stream>>,
    root: PathBuf,
    cwd: Vec<String>,
    rnfr: Option<PathBuf>,
    data: Option<Data>,
    prot_p: bool,
    user_ok: bool,
    authed: bool,
}

impl Sess {
    fn say(&mut self, s: &str) -> std::io::Result<()> {
        let w = self.ctl.get_mut();
        w.write_all(format!("{s}\r\n").as_bytes())?;
        w.flush()
    }
    fn resolve(&self, arg: &str) -> PathBuf {
        let mut comps: Vec<String> = if arg.starts_with('/') {
            vec![]
        } else {
            self.cwd.clone()
        };
        for c in arg.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if c == ".." {
                comps.pop();
            } else {
                comps.push(c.to_string());
            }
        }
        let mut p = self.root.clone();
        p.extend(comps);
        p
    }
    fn open_data(&mut self, cfg: &Cfg) -> std::io::Result<Box<dyn Stream>> {
        let tcp = match self.data.take() {
            Some(Data::Pasv(l)) => {
                l.set_nonblocking(false)?;
                let (s, _) = l.accept()?;
                s
            }
            Some(Data::Port(a)) => TcpStream::connect(a)?,
            None => return Err(std::io::Error::other("brak kanału danych")),
        };
        if self.prot_p {
            let conn = rustls::ServerConnection::new(cfg.tls.clone().unwrap()).unwrap();
            Ok(Box::new(rustls::StreamOwned::new(conn, tcp)))
        } else {
            Ok(Box::new(tcp))
        }
    }
}

fn finish_data(mut d: Box<dyn Stream>) {
    let _ = d.flush();
    d.close();
    let tcp = d.tcp().try_clone().unwrap();
    drop(d);
    let _ = tcp.shutdown(Shutdown::Both);
}

fn unix_line(p: &Path, name: &str) -> String {
    let md = p.metadata().unwrap();
    let t = chrono::DateTime::from_timestamp(mtime_of(p), 0).unwrap();
    format!(
        "{}rw-r--r-- 1 ftp ftp {} {} {}\r\n",
        if md.is_dir() { "d" } else { "-" },
        if md.is_dir() { 4096 } else { md.len() },
        t.format("%b %e %H:%M"),
        name
    )
}

fn session(tcp: TcpStream, root: &Path, cfg: &Cfg) -> std::io::Result<()> {
    let gen = cfg.kill_gen.load(SeqCst);
    let first: Box<dyn Stream> = if cfg.implicit {
        let conn = rustls::ServerConnection::new(cfg.tls.clone().unwrap()).unwrap();
        Box::new(rustls::StreamOwned::new(conn, tcp))
    } else {
        Box::new(tcp)
    };
    let mut s = Sess {
        ctl: BufReader::new(first),
        root: root.to_path_buf(),
        cwd: vec![],
        rnfr: None,
        data: None,
        prot_p: false,
        user_ok: false,
        authed: false,
    };
    s.say("220 testowy serwer FTP")?;
    loop {
        let mut line = String::new();
        if s.ctl.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if cfg.kill_gen.load(SeqCst) != gen {
            return Ok(());
        }
        let line = line.trim_end().to_string();
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        let cmd = cmd.to_ascii_uppercase();
        match cmd.as_str() {
            "AUTH" => {
                if let Some(t) = &cfg.tls {
                    s.say("234 AUTH TLS ok")?;
                    // wymiana strumienia sterującego na TLS
                    let old = std::mem::replace(&mut s.ctl, BufReader::new(Box::new(Dummy)));
                    let tcp = old.into_inner().tcp().try_clone()?;
                    let conn = rustls::ServerConnection::new(t.clone()).unwrap();
                    s.ctl = BufReader::new(Box::new(rustls::StreamOwned::new(conn, tcp)));
                } else {
                    s.say("502 brak TLS")?;
                }
            }
            "PBSZ" => s.say("200 PBSZ=0")?,
            "PROT" => {
                s.prot_p = arg.eq_ignore_ascii_case("P");
                s.say("200 PROT ok")?
            }
            "USER" => {
                s.user_ok = arg == "jan";
                s.say("331 haslo?")?
            }
            "PASS" => {
                if s.user_ok && arg == cfg.pass {
                    s.authed = true;
                    cfg.logins.fetch_add(1, SeqCst);
                    s.say("230 zalogowano")?
                } else {
                    s.say("530 zle haslo")?
                }
            }
            "QUIT" => {
                s.say("221 pa")?;
                return Ok(());
            }
            _ if !s.authed => s.say("530 zaloguj sie")?,
            "SYST" => s.say("215 UNIX Type: L8")?,
            "FEAT" => {
                let mut f = String::from("211-Features:\r\n UTF8\r\n SIZE\r\n MDTM\r\n");
                if cfg.mlsd.load(SeqCst) {
                    f.push_str(" MLST type*;size*;modify*;\r\n");
                }
                if cfg.mfmt.load(SeqCst) {
                    f.push_str(" MFMT\r\n");
                }
                f.push_str("211 End");
                s.say(&f)?
            }
            "OPTS" => s.say("200 OK")?,
            "TYPE" => s.say("200 typ")?,
            "NOOP" => s.say("200 noop")?,
            "PWD" => s.say("257 \"/\"")?,
            "CWD" => {
                let p = s.resolve(arg);
                if p.is_dir() {
                    s.cwd = p
                        .strip_prefix(root)
                        .unwrap()
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_string())
                        .collect();
                    s.say("250 ok")?
                } else {
                    s.say("550 brak")?
                }
            }
            "PASV" | "EPSV" => {
                let l = TcpListener::bind("127.0.0.1:0")?;
                let port = l.local_addr()?.port();
                s.data = Some(Data::Pasv(l));
                if cmd == "EPSV" {
                    s.say(&format!("229 Entering Extended Passive Mode (|||{port}|)"))?
                } else {
                    s.say(&format!(
                        "227 Entering Passive Mode (127,0,0,1,{},{})",
                        port >> 8,
                        port & 255
                    ))?
                }
            }
            "PORT" => {
                let n: Vec<u16> = arg
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                let addr = format!("{}.{}.{}.{}:{}", n[0], n[1], n[2], n[3], n[4] * 256 + n[5]);
                s.data = Some(Data::Port(addr.parse().unwrap()));
                s.say("200 PORT ok")?
            }
            "EPRT" => {
                let f: Vec<&str> = arg.split('|').collect();
                s.data = Some(Data::Port(format!("{}:{}", f[2], f[3]).parse().unwrap()));
                s.say("200 EPRT ok")?
            }
            "MLSD" | "LIST" | "NLST" => {
                if cmd == "MLSD" && !cfg.mlsd.load(SeqCst) {
                    s.say("500 MLSD nieznane")?;
                    continue;
                }
                let arg = arg.trim_start_matches("-a ").to_string();
                let dir = s.resolve(&arg);
                if !dir.is_dir() {
                    s.say("550 brak folderu")?;
                    continue;
                }
                s.say("150 lista")?;
                let mut d = s.open_data(cfg)?;
                let mut kids: Vec<_> = std::fs::read_dir(&dir)?.flatten().collect();
                kids.sort_by_key(|k| k.file_name());
                let mut out = String::new();
                if cmd == "MLSD" {
                    out.push_str("type=cdir;modify=20200101000000; .\r\ntype=pdir;modify=20200101000000; ..\r\n");
                }
                for k in &kids {
                    let name = k.file_name().to_string_lossy().to_string();
                    let p = k.path();
                    if cmd == "MLSD" {
                        let md = p.metadata()?;
                        out.push_str(&format!(
                            "type={};size={};modify={}; {}\r\n",
                            if md.is_dir() { "dir" } else { "file" },
                            md.len(),
                            ftp_time(mtime_of(&p)),
                            name
                        ));
                    } else if cmd == "NLST" {
                        out.push_str(&format!("{name}\r\n"));
                    } else {
                        out.push_str(&unix_line(&p, &name));
                    }
                }
                if cfg.evil.load(SeqCst) && cmd == "MLSD" {
                    for n in ["../zly.txt", "..", "a/b.txt", "x\\y.txt"] {
                        out.push_str(&format!("type=file;size=1;modify=20200101000000; {n}\r\n"));
                    }
                }
                if cfg.drop_mlsd.load(SeqCst) {
                    let half = out.len() / 2;
                    d.write_all(&out.as_bytes()[..half])?;
                    d.flush()?;
                    finish_data(d);
                    return Ok(()); // zamknięcie sterującego bez 226
                }
                d.write_all(out.as_bytes())?;
                finish_data(d);
                s.say("226 koniec listy")?
            }
            "SIZE" => {
                let p = s.resolve(arg);
                if p.is_file() {
                    s.say(&format!("213 {}", p.metadata()?.len()))?
                } else {
                    s.say("550 brak pliku")?
                }
            }
            "MDTM" => {
                // "MDTM czas ścieżka" ustawia czas (wariant), "MDTM ścieżka" czyta
                let (ts, rest) = arg.split_once(' ').unwrap_or(("", ""));
                if ts.len() == 14 && ts.bytes().all(|b| b.is_ascii_digit()) && !rest.is_empty() {
                    if !cfg.mdtm_set.load(SeqCst) {
                        s.say("550 brak pliku")?;
                        continue;
                    }
                    let p = s.resolve(rest);
                    if !p.exists() {
                        s.say("550 brak pliku")?;
                        continue;
                    }
                    let t = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d%H%M%S")
                        .unwrap()
                        .and_utc()
                        .timestamp();
                    filetime::set_file_mtime(&p, FileTime::from_unix_time(t, 0))?;
                    s.say("213 ok")?;
                } else {
                    let p = s.resolve(arg);
                    if p.is_file() {
                        s.say(&format!("213 {}", ftp_time(mtime_of(&p))))?
                    } else {
                        s.say("550 brak pliku")?
                    }
                }
            }
            "MFMT" => {
                if !cfg.mfmt.load(SeqCst) {
                    s.say("500 MFMT nieznane")?;
                    continue;
                }
                let (ts, rest) = arg.split_once(' ').unwrap_or(("", ""));
                let p = s.resolve(rest);
                if !p.exists() {
                    s.say("550 brak pliku")?;
                    continue;
                }
                let t = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d%H%M%S")
                    .unwrap()
                    .and_utc()
                    .timestamp();
                filetime::set_file_mtime(&p, FileTime::from_unix_time(t, 0))?;
                s.say(&format!("213 Modify={ts}; {rest}"))?
            }
            "RETR" => {
                let p = s.resolve(arg);
                if !p.is_file() {
                    s.say("550 brak pliku")?;
                    continue;
                }
                let data = std::fs::read(&p)?;
                s.say("150 wysylam")?;
                let mut d = s.open_data(cfg)?;
                if cfg.drop_retr.load(SeqCst) {
                    d.write_all(&data[..data.len() / 2])?;
                    d.flush()?;
                    finish_data(d);
                    return Ok(());
                }
                d.write_all(&data)?;
                finish_data(d);
                s.say("226 ok")?
            }
            "STOR" => {
                let p = s.resolve(arg);
                cfg.stor_names.lock().unwrap().push(arg.to_string());
                if !p.parent().is_some_and(|x| x.is_dir()) {
                    s.say("550 brak folderu")?;
                    continue;
                }
                s.say("150 odbieram")?;
                let mut d = s.open_data(cfg)?;
                let mut buf = Vec::new();
                if cfg
                    .drop_stor
                    .fetch_update(SeqCst, SeqCst, |v| v.checked_sub(1))
                    .is_ok()
                {
                    let mut chunk = vec![0u8; 64 * 1024];
                    let n = d.read(&mut chunk).unwrap_or(0);
                    std::fs::write(&p, &chunk[..n])?; // częściowy plik
                    finish_data(d);
                    return Ok(());
                }
                let _ = d.read_to_end(&mut buf); // niezamknięte TLS = koniec
                std::fs::write(&p, &buf)?;
                finish_data(d);
                s.say("226 zapisano")?
            }
            "DELE" => {
                let p = s.resolve(arg);
                if p.is_file() {
                    std::fs::remove_file(p)?;
                    s.say("250 usunieto")?
                } else {
                    s.say("550 brak pliku")?
                }
            }
            "MKD" => {
                let p = s.resolve(arg);
                if p.exists() || !p.parent().is_some_and(|x| x.is_dir()) {
                    s.say("550 nie mozna")?
                } else {
                    std::fs::create_dir(p)?;
                    s.say("257 utworzono")?
                }
            }
            "RMD" => {
                let p = s.resolve(arg);
                if std::fs::remove_dir(&p).is_ok() {
                    s.say("250 usunieto")?
                } else {
                    s.say("550 nie mozna")?
                }
            }
            "RNFR" => {
                let p = s.resolve(arg);
                if p.exists() {
                    s.rnfr = Some(p);
                    s.say("350 dalej")?
                } else {
                    s.say("550 brak")?
                }
            }
            "RNTO" => {
                let to = s.resolve(arg);
                let from = s.rnfr.take();
                match from {
                    Some(f) if !to.parent().is_some_and(|x| x.is_dir()) => {
                        let _ = f;
                        s.say("550 brak folderu")?
                    }
                    Some(f) if to.exists() && !cfg.rnto_overwrite.load(SeqCst) => {
                        let _ = f;
                        s.say("550 cel istnieje")?
                    }
                    Some(f) => {
                        std::fs::rename(f, to)?;
                        s.say("250 zmieniono")?
                    }
                    None => s.say("503 najpierw RNFR")?,
                }
            }
            _ => s.say("502 nieznane polecenie")?,
        }
    }
}

struct Dummy;
impl Read for Dummy {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}
impl Write for Dummy {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Stream for Dummy {
    fn tcp(&self) -> &TcpStream {
        unreachable!()
    }
}
// ---------------------------------------------------------------------------------------------------------------------

fn params(srv: &Srv, pass: &str, tls: &str) -> Params {
    let mut p = Params::default();
    p.public.insert("host".into(), "127.0.0.1".into());
    p.public.insert("port".into(), srv.port.to_string());
    p.public.insert("user".into(), "jan".into());
    p.public.insert("tls".into(), tls.into());
    p.public.insert("no_check_certificate".into(), "1".into());
    p.secret.insert("pass".into(), pass.into());
    p
}

fn open(srv: &Srv) -> Box<dyn Backend> {
    remote::open("ftp", &params(srv, "tajne", "none"), "").unwrap()
}

fn write(p: &Path, data: &[u8]) {
    std::fs::write(p, data).unwrap();
}

fn x(bw: &Bandwidth) -> Xfer<'_> {
    Xfer::new(bw)
}

fn tmp_leftovers(dir: &Path) -> Vec<String> {
    let mut v = vec![];
    fn walk(d: &Path, v: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(".syncron-up-") {
                v.push(n);
            }
            if e.path().is_dir() {
                walk(&e.path(), v);
            }
        }
    }
    walk(dir, &mut v);
    v
}

#[test]
fn roundtrip_polish_names_spaces_mtime_and_atomic_upload() {
    let srv = start(Cfg::new());
    let b = open(&srv);
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
    // STOR szedł wyłącznie na pliki tymczasowe, a po wysłaniu nic nie zostaje
    let st = srv.cfg.stor_names.lock().unwrap().clone();
    assert_eq!(st.len(), 3);
    assert!(st.iter().all(|n| n.contains(".syncron-up-")), "{st:?}");
    assert!(tmp_leftovers(srv.dir.path()).is_empty());

    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 3, "{l:?}");
    for (i, n) in names.iter().enumerate() {
        let e = l
            .iter()
            .find(|e| e.path == *n)
            .unwrap_or_else(|| panic!("brak {n}: {l:?}"));
        assert_eq!(e.meta.mtime, 1_600_000_000 + i as i64 * 1000, "mtime {n}");
        let s = b.stat(n).unwrap().unwrap();
        assert_eq!((s.meta.size, s.meta.mtime), (e.meta.size, e.meta.mtime));
        let dst = work.path().join("dst");
        b.download(n, &dst, &x(&bw)).unwrap();
        assert_eq!(
            std::fs::read(&dst).unwrap(),
            format!("treść {i} ąęć").as_bytes()
        );
    }
    let sub = b.list_files("a b").unwrap();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].path, "a b/c d/e f.txt");
    let mut d: Vec<_> = b
        .list_dir("")
        .unwrap()
        .into_iter()
        .map(|i| (i.name, i.is_dir))
        .collect();
    d.sort();
    assert!(d.contains(&("a b".to_string(), true)));
    assert!(d.contains(&("płaski plik.bin".to_string(), false)));
    assert!(b.stat("a b").unwrap().is_none());
    assert!(b.ping().unwrap().contains("MLSD"));
}

#[test]
fn list_fallback_without_mlsd_and_mdtm_refinement() {
    let srv = start(Cfg::new());
    srv.cfg.mlsd.store(false, SeqCst);
    let b = open(&srv);
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"dane");
    b.upload(
        &src,
        "Zażółć gęślą/plik ze spacją.txt",
        1_600_000_123,
        &x(&Bandwidth::default()),
    )
    .unwrap();
    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 1, "{l:?}");
    assert_eq!(l[0].path, "Zażółć gęślą/plik ze spacją.txt");
    assert_eq!(l[0].meta.size, 4);
    // LIST ma tylko minuty — dokładny czas pochodzi z MDTM
    assert_eq!(l[0].meta.mtime, 1_600_000_123);
    assert!(b.ping().unwrap().contains("LIST"));
}

#[test]
fn mtime_variants() {
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"abc");
    // 1) tylko MDTM z czasem
    let srv = start(Cfg::new());
    srv.cfg.mfmt.store(false, SeqCst);
    let b = open(&srv);
    b.upload(&src, "m.txt", 1_500_000_000, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(b.stat("m.txt").unwrap().unwrap().meta.mtime, 1_500_000_000);
    // 2) brak jakiejkolwiek obsługi: wysyłanie działa, czas = czas wysłania (tolerancja)
    srv.cfg.mdtm_set.store(false, SeqCst);
    let b2 = open(&srv);
    b2.upload(&src, "n.txt", 1_500_000_000, &x(&Bandwidth::default()))
        .unwrap();
    let t = b2.stat("n.txt").unwrap().unwrap().meta.mtime;
    assert!(
        t > 1_600_000_000,
        "serwer bez obsługi czasu zachowuje czas wysłania"
    );
}

#[test]
fn big_file_with_speed_limit() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("big.bin");
    let mut data = vec![0u8; 20 * 1024 * 1024];
    let mut v = 777u32;
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
    assert!(
        t.elapsed().as_secs_f64() >= 1.6,
        "limit prędkości nie działa"
    );
    assert_eq!(sent.load(SeqCst), data.len() as u64);
    let dst = work.path().join("back.bin");
    let t = Instant::now();
    b.download("duzy/big.bin", &dst, &xf).unwrap();
    assert!(t.elapsed().as_secs_f64() >= 1.6);
    assert!(std::fs::read(&dst).unwrap() == data);
}

#[test]
fn connection_dropped_during_upload() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("d.bin");
    write(&src, &vec![1u8; 2 * 1024 * 1024]);
    let bw = Bandwidth::default();
    // jednorazowe zerwanie: operacja jest ponawiana na nowym połączeniu
    srv.cfg.drop_stor.store(1, SeqCst);
    b.upload(&src, "ok.bin", 0, &x(&bw)).unwrap();
    assert_eq!(
        srv.dir.path().join("ok.bin").metadata().unwrap().len(),
        2 * 1024 * 1024
    );
    // stałe zerwanie: Network, a docelowy plik nie powstaje (częściowy jest tylko tymczasowy, ukryty przed listingiem)
    srv.cfg.drop_stor.store(100, SeqCst);
    let e = b.upload(&src, "zle.bin", 0, &x(&bw)).unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
    assert!(!srv.dir.path().join("zle.bin").exists());
    srv.cfg.drop_stor.store(0, SeqCst);
    let l = b.list_files("").unwrap();
    assert!(l.iter().all(|e| !e.path.contains(".syncron-up-")), "{l:?}");
}

#[test]
fn connection_dropped_during_download_and_listing() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    write(&srv.dir.path().join("f.bin"), &vec![5u8; 1024 * 1024]);
    let work = tempfile::tempdir().unwrap();
    let dst = work.path().join("f.bin");
    srv.cfg.drop_retr.store(true, SeqCst);
    let e = b
        .download("f.bin", &dst, &x(&Bandwidth::default()))
        .unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
    assert!(!dst.exists());
    srv.cfg.drop_mlsd.store(true, SeqCst);
    let e = b.list_files("").unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
}

#[test]
fn reconnects_after_server_dropped_idle_connection() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    write(&srv.dir.path().join("a.txt"), b"1");
    assert_eq!(b.list_files("").unwrap().len(), 1);
    assert_eq!(srv.cfg.logins.load(SeqCst), 1);
    srv.cfg.kill_gen.fetch_add(1, SeqCst); // serwer zamknie istniejące sesje przy najbliższym poleceniu
    assert_eq!(b.list_files("").unwrap().len(), 1);
    assert_eq!(srv.cfg.logins.load(SeqCst), 2, "powinno być nowe logowanie");
}

#[test]
fn wrong_password_is_auth_error() {
    let srv = start(Cfg::new());
    let bad = remote::open("ftp", &params(&srv, "zle", "none"), "").unwrap();
    assert!(matches!(bad.list_files("").unwrap_err(), RError::Auth(_)));
    assert!(matches!(bad.ping().unwrap_err(), RError::Auth(_)));
    assert!(open(&srv).ping().is_ok());
}

#[test]
fn empty_and_missing_folders() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    assert!(b.list_files("").unwrap().is_empty());
    assert!(b.list_files("nie ma takiego").unwrap().is_empty());
    assert!(matches!(
        b.list_dir("nie ma takiego").unwrap_err(),
        RError::NotFound(_)
    ));
    assert!(b.stat("nie/ma.txt").unwrap().is_none());
    b.delete("nie/ma.txt").unwrap();
    let r = remote::open("ftp", &params(&srv, "tajne", "none"), "Brak/Folderu").unwrap();
    assert!(r.list_files("").unwrap().is_empty());
    assert!(matches!(r.ping().unwrap_err(), RError::NotFound(_)));
}

#[test]
fn dotdot_names_from_server_are_ignored() {
    let srv = start(Cfg::new());
    let b = open(&srv);
    write(&srv.dir.path().join("dobry.txt"), b"ok");
    std::fs::create_dir(srv.dir.path().join("sub")).unwrap();
    write(&srv.dir.path().join("sub/inny.txt"), b"ok2");
    srv.cfg.evil.store(true, SeqCst);
    let l = b.list_files("").unwrap();
    let mut paths: Vec<_> = l.iter().map(|e| e.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["dobry.txt", "sub/inny.txt"], "{l:?}");
    assert_eq!(b.list_dir("").unwrap().len(), 2);
    assert!(b.stat("../x").is_err());
    assert!(b.delete("a/../../x").is_err());
    assert!(b.mkdirs("../x").is_err());
}

#[test]
fn parallel_uploads_respect_concurrency() {
    let srv = start(Cfg::new());
    let mut p = params(&srv, "tajne", "none");
    p.public.insert("concurrency".into(), "3".into());
    let b: Arc<dyn Backend> = Arc::from(remote::open("ftp", &p, "").unwrap());
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
            let pth = format!("w{t}/g {i}/głęboko/plik {i}.txt");
            let e = l
                .iter()
                .find(|e| e.path == pth)
                .unwrap_or_else(|| panic!("brak {pth}"));
            assert_eq!(e.meta.size, format!("{t}-{i}").len() as u64);
        }
    }
    let peak = srv.cfg.peak.load(SeqCst);
    assert!((2..=3).contains(&peak), "szczyt połączeń = {peak}");
    assert!(tmp_leftovers(srv.dir.path()).is_empty());
}

#[test]
fn overwrite_rename_mkdirs_rmdir() {
    for overwrite_ok in [true, false] {
        let srv = start(Cfg::new());
        srv.cfg.rnto_overwrite.store(overwrite_ok, SeqCst);
        let b = open(&srv);
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
        let s = b.stat("p/plik.txt").unwrap().unwrap();
        assert_eq!((s.meta.size, s.meta.mtime), (2, 2_000));
        assert!(tmp_leftovers(srv.dir.path()).is_empty());

        b.rename("p/plik.txt", "n1/n2/n3/nowy ł.txt").unwrap();
        assert!(!srv.dir.path().join("p/plik.txt").exists());
        assert_eq!(
            std::fs::read(srv.dir.path().join("n1/n2/n3/nowy ł.txt")).unwrap(),
            b"v2"
        );
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
        write(&srv.dir.path().join("m1/m2/zostaje.txt"), b"x");
        b.rmdir_if_empty("m1/m2").unwrap();
        assert!(srv.dir.path().join("m1/m2/zostaje.txt").exists());
        b.delete("cel.txt").unwrap();
        assert!(!srv.dir.path().join("cel.txt").exists());
    }
}

#[test]
fn root_folder_and_leading_dash_names() {
    let srv = start(Cfg::new());
    let b = remote::open("ftp", &params(&srv, "tajne", "none"), "Praca/Dokumenty ł").unwrap();
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
fn active_mode_works() {
    let srv = start(Cfg::new());
    let mut p = params(&srv, "tajne", "none");
    p.public.insert("passive".into(), "0".into());
    let b = remote::open("ftp", &p, "").unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, b"aktywnie");
    b.upload(&src, "akt/p.txt", 1_600_000_000, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(b.list_files("").unwrap().len(), 1);
    let dst = work.path().join("d");
    b.download("akt/p.txt", &dst, &x(&Bandwidth::default()))
        .unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"aktywnie");
}

// ---------------------------------------------------------------------------------------------------------------------
// FTPS (jawny i niejawny) z certyfikatem samopodpisanym

fn tls_server_config() -> Arc<rustls::ServerConfig> {
    let ck =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".to_string()])
            .unwrap();
    let cert = ck.cert.der().clone();
    let key = rustls::pki_types::PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap(),
    )
}

fn ftps_roundtrip(tls: &str, kind: &str, implicit: bool) {
    let mut cfg = Cfg::new();
    cfg.tls = Some(tls_server_config());
    cfg.implicit = implicit;
    let srv = start(cfg);
    let b = remote::open(kind, &params(&srv, "tajne", tls), "").unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = work.path().join("s");
    write(&src, "szyfrowane dane ąęć".as_bytes());
    let bw = Bandwidth::default();
    b.upload(&src, "tls/Zażółć gęślą.txt", 1_600_000_000, &x(&bw))
        .unwrap();
    assert_eq!(
        std::fs::read(srv.dir.path().join("tls/Zażółć gęślą.txt")).unwrap(),
        "szyfrowane dane ąęć".as_bytes()
    );
    let l = b.list_files("").unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].meta.mtime, 1_600_000_000);
    let dst = work.path().join("d");
    b.download("tls/Zażółć gęślą.txt", &dst, &x(&bw)).unwrap();
    assert_eq!(
        std::fs::read(&dst).unwrap(),
        "szyfrowane dane ąęć".as_bytes()
    );
    assert!(b.ping().unwrap().contains("FTPS"));
}

#[test]
fn ftps_explicit_roundtrip() {
    ftps_roundtrip("explicit", "ftps", false);
}

#[test]
fn ftps_implicit_roundtrip() {
    ftps_roundtrip("implicit", "ftps", true);
}

#[test]
fn ftps_kind_defaults_to_explicit_and_verifies_certificate() {
    let mut cfg = Cfg::new();
    cfg.tls = Some(tls_server_config());
    let srv = start(cfg);
    // kind=ftps bez pola tls => jawny TLS
    let mut p = params(&srv, "tajne", "");
    p.public.remove("tls");
    let b = remote::open("ftps", &p, "").unwrap();
    assert!(b.ping().is_ok());
    // bez no_check_certificate certyfikat samopodpisany jest odrzucony
    let mut p2 = p.clone();
    p2.public.remove("no_check_certificate");
    let b2 = remote::open("ftps", &p2, "").unwrap();
    let e = b2.ping().unwrap_err();
    assert!(matches!(e, RError::Network(_)), "{e:?}");
}

#[test]
fn bad_params() {
    let mut p = Params::default();
    assert!(remote::open("ftp", &p, "").is_err());
    p.public.insert("host".into(), "127.0.0.1".into());
    p.public.insert("port".into(), "70000".into());
    assert!(remote::open("ftp", &p, "").is_err());
    // nieosiągalny serwer: błąd sieci w rozsądnym czasie
    p.public.insert("port".into(), "1".into());
    let b = remote::open("ftp", &p, "").unwrap();
    let t = Instant::now();
    assert!(matches!(b.ping().unwrap_err(), RError::Network(_)));
    assert!(t.elapsed() < Duration::from_secs(25));
}
