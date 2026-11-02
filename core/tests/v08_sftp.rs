//! Testy backendu SFTP przeciw PRAWDZIWEMU serwerowi SSH/SFTP uruchomionemu w procesie testu
//! (`russh` server + `russh-sftp` server obsługujące katalog tymczasowy na 127.0.0.1:0).
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, Msg, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use syncron_core::model::Bandwidth;
use syncron_core::remote::{self, Backend, Params, RError, Xfer};

const USER: &str = "tester";
const PASS: &str = "tajne-haslo";

// ----------------------------------------------------------------------------------------------------------------
// Serwer testowy

#[derive(Default)]
struct Ctl {
    root: PathBuf,
    authorized: Mutex<Option<PublicKey>>,
    /// Czy SFTP `rename` nadpisuje cel (OpenSSH v3 tego nie robi — domyślnie false).
    rename_overwrites: AtomicBool,
    /// Zabij połączenie przy najbliższym `readdir` w podkatalogu (jednorazowo).
    kill_once_on_readdir: AtomicBool,
    /// Zabijaj każde połączenie przy `readdir` w podkatalogu.
    kill_always_on_readdir: AtomicBool,
    /// Po tylu odczytach (`read`) zabij połączenie (-1 = nigdy).
    kill_after_reads: AtomicI64,
    sessions: Mutex<Vec<russh::server::Handle>>,
    rt: Mutex<Option<tokio::runtime::Handle>>,
    active: AtomicUsize,
    max_active: AtomicUsize,
    total_conns: AtomicUsize,
}

impl Ctl {
    /// Serwer zamyka wszystkie połączenia SSH (rozłączenie po stronie serwera).
    fn kill_all(&self) {
        let hs: Vec<_> = self.sessions.lock().unwrap().drain(..).collect();
        if let Some(rt) = self.rt.lock().unwrap().clone() {
            rt.spawn(async move {
                for h in hs {
                    let _ = h
                        .disconnect(
                            russh::Disconnect::ByApplication,
                            "test: zerwanie".into(),
                            "pl".into(),
                        )
                        .await;
                }
            });
        }
    }
}

struct TestServer {
    port: u16,
    ctl: Arc<Ctl>,
    rt: tokio::runtime::Runtime,
    listener_task: tokio::task::AbortHandle,
    dir: tempfile::TempDir,
    host_key: PrivateKey,
}

fn key_from_seed(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

impl TestServer {
    fn start(seed: u8) -> TestServer {
        Self::start_on(seed, 0, None, None)
    }

    fn start_on(
        seed: u8,
        port: u16,
        dir: Option<tempfile::TempDir>,
        preferred: Option<russh::Preferred>,
    ) -> TestServer {
        let dir = dir.unwrap_or_else(|| tempfile::tempdir().unwrap());
        let ctl = Arc::new(Ctl {
            root: dir.path().to_path_buf(),
            kill_after_reads: AtomicI64::new(-1),
            ..Default::default()
        });
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let host_key = key_from_seed(seed);
        let mut cfg = russh::server::Config {
            auth_rejection_time: Duration::from_millis(20),
            auth_rejection_time_initial: Some(Duration::from_millis(0)),
            keys: vec![host_key.clone()],
            ..Default::default()
        };
        if let Some(p) = preferred {
            cfg.preferred = p;
        }
        let cfg = Arc::new(cfg);
        let mut tries = 0;
        let listener = loop {
            match rt.block_on(tokio::net::TcpListener::bind(("127.0.0.1", port))) {
                Ok(l) => break l,
                Err(e) if tries < 40 => {
                    tries += 1;
                    let _ = e;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => panic!("bind: {e}"),
            }
        };
        let port = listener.local_addr().unwrap().port();
        let c2 = ctl.clone();
        let task = rt.spawn(async move {
            loop {
                let Ok((sock, peer)) = listener.accept().await else {
                    break;
                };
                let _ = sock.set_nodelay(true);
                let handler = SshSession::new(c2.clone(), peer);
                let cfg = cfg.clone();
                let c3 = c2.clone();
                tokio::spawn(async move {
                    c3.total_conns.fetch_add(1, Ordering::SeqCst);
                    let n = c3.active.fetch_add(1, Ordering::SeqCst) + 1;
                    c3.max_active.fetch_max(n, Ordering::SeqCst);
                    if let Ok(running) = russh::server::run_stream(cfg, sock, handler).await {
                        c3.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
                    }
                    c3.active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        *ctl.rt.lock().unwrap() = Some(rt.handle().clone());
        TestServer {
            port,
            ctl,
            rt,
            listener_task: task.abort_handle(),
            dir,
            host_key,
        }
    }

    fn stop(self) -> tempfile::TempDir {
        self.listener_task.abort();
        self.ctl.kill_all();
        let TestServer { rt, dir, .. } = self;
        // czeka, aż zadania (w tym gniazdo nasłuchujące) zostaną zwolnione — port da się od razu zająć ponownie
        rt.shutdown_timeout(Duration::from_secs(3));
        dir
    }

    fn fingerprint(&self) -> String {
        self.host_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string()
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn params(&self) -> Params {
        let mut p = Params::default();
        p.public.insert("host".into(), "127.0.0.1".into());
        p.public.insert("port".into(), self.port.to_string());
        p.public.insert("user".into(), USER.into());
        p.secret.insert("pass".into(), PASS.into());
        // własny plik known_hosts w katalogu tymczasowym (nie dotykamy HOME)
        p.public.insert(
            "known_hosts_store".into(),
            std::env::temp_dir()
                .join(format!("syncron-kh-{}-{}", std::process::id(), self.port))
                .to_string_lossy()
                .into_owned(),
        );
        p
    }

    fn open(&self, root: &str) -> Box<dyn Backend> {
        remote::open("sftp", &self.params(), root).unwrap()
    }
}

struct SshSession {
    ctl: Arc<Ctl>,
    channels: Mutex<HashMap<ChannelId, Channel<Msg>>>,
    _peer: SocketAddr,
}

impl SshSession {
    fn new(ctl: Arc<Ctl>, peer: SocketAddr) -> Self {
        SshSession {
            ctl,
            channels: Mutex::new(HashMap::new()),
            _peer: peer,
        }
    }
}

impl russh::server::Handler for SshSession {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASS {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        let ok = user == USER
            && self
                .ctl
                .authorized
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|k| k == key.public_key);
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().unwrap().insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name == "sftp" {
            let channel = self.channels.lock().unwrap().remove(&channel_id).unwrap();
            session.channel_success(channel_id)?;
            russh_sftp::server::run(channel.into_stream(), FsHandler::new(self.ctl.clone())).await;
        } else {
            session.channel_failure(channel_id)?;
        }
        Ok(())
    }
}

enum H {
    File(std::fs::File),
    Dir {
        entries: Vec<File>,
        pos: usize,
        sub: bool,
    },
}

struct FsHandler {
    ctl: Arc<Ctl>,
    handles: HashMap<String, H>,
    next: u64,
    reads: i64,
}

impl FsHandler {
    fn new(ctl: Arc<Ctl>) -> Self {
        FsHandler {
            ctl,
            handles: HashMap::new(),
            next: 0,
            reads: 0,
        }
    }

    /// Ścieżka klienta -> ścieżka w katalogu serwera (bez wychodzenia poza korzeń).
    fn local(&self, p: &str) -> Result<PathBuf, StatusCode> {
        let mut out = self.ctl.root.clone();
        for c in p.split('/') {
            match c {
                "" | "." => {}
                ".." => return Err(StatusCode::PermissionDenied),
                c => out.push(c),
            }
        }
        Ok(out)
    }

    fn ok(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".into(),
            language_tag: "en-US".into(),
        }
    }

    fn new_handle(&mut self, h: H) -> String {
        self.next += 1;
        let k = format!("h{}", self.next);
        self.handles.insert(k.clone(), h);
        k
    }
}

fn io_status(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attrs_of(m: &std::fs::Metadata) -> FileAttributes {
    let mut a = FileAttributes::from(m);
    a.user = None;
    a.group = None;
    a
}

impl russh_sftp::server::Handler for FsHandler {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _v: u32, _e: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _a: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = self.local(&filename)?;
        let mut o = std::fs::OpenOptions::new();
        o.read(pflags.contains(OpenFlags::READ));
        o.write(pflags.contains(OpenFlags::WRITE));
        if pflags.contains(OpenFlags::CREATE) {
            o.create(true);
        }
        if pflags.contains(OpenFlags::TRUNCATE) {
            o.truncate(true);
        }
        let f = o.open(path).map_err(io_status)?;
        let handle = self.new_handle(H::File(f));
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(Self::ok(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        self.reads += 1;
        let limit = self.ctl.kill_after_reads.load(Ordering::SeqCst);
        if limit >= 0 && self.reads > limit {
            self.ctl.kill_all();
            return Err(StatusCode::ConnectionLost);
        }
        let Some(H::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let mut buf = vec![0u8; len.min(256 * 1024) as usize];
        let n = f.read(&mut buf).map_err(io_status)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(H::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        f.write_all(&data).map_err(io_status)?;
        Ok(Self::ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = std::fs::symlink_metadata(self.local(&path)?).map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let m = std::fs::metadata(self.local(&path)?).map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(H::File(f)) = self.handles.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let m = f.metadata().map_err(io_status)?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&m),
        })
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let p = self.local(&path)?;
        if let Some(mt) = attrs.mtime {
            let t = filetime::FileTime::from_unix_time(mt as i64, 0);
            filetime::set_file_mtime(&p, t).map_err(io_status)?;
        }
        Ok(Self::ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let p = self.local(&path)?;
        let sub = p != self.ctl.root;
        let mut entries = Vec::new();
        for e in std::fs::read_dir(&p).map_err(io_status)? {
            let e = e.map_err(io_status)?;
            let m = e.metadata().map_err(io_status)?;
            // symlink_metadata: pokaż dowiązania jako dowiązania (jak prawdziwy serwer)
            let lm = std::fs::symlink_metadata(e.path()).unwrap_or(m);
            let mut a = attrs_of(&lm);
            if lm.file_type().is_symlink() {
                a.permissions = Some(0o120777);
            }
            entries.push(File::new(e.file_name().to_string_lossy().into_owned(), a));
        }
        let handle = self.new_handle(H::Dir {
            entries,
            pos: 0,
            sub,
        });
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let (kill_once, kill_always) = (
            self.ctl.kill_once_on_readdir.load(Ordering::SeqCst),
            self.ctl.kill_always_on_readdir.load(Ordering::SeqCst),
        );
        let Some(H::Dir { entries, pos, sub }) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        if *sub && kill_always
            || (*sub && kill_once && self.ctl.kill_once_on_readdir.swap(false, Ordering::SeqCst))
        {
            self.ctl.kill_all();
            return Err(StatusCode::ConnectionLost);
        }
        if *pos >= entries.len() {
            return Err(StatusCode::Eof);
        }
        let end = (*pos + 2).min(entries.len()); // małe porcje: wiele rund readdir
        let files = entries[*pos..end].to_vec();
        *pos = end;
        Ok(Name { id, files })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let p = self.local(&filename)?;
        if p.is_dir() {
            return Err(StatusCode::Failure);
        }
        std::fs::remove_file(p).map_err(io_status)?;
        Ok(Self::ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _a: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.local(&path)?).map_err(io_status)?;
        Ok(Self::ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.local(&path)?).map_err(io_status)?;
        Ok(Self::ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let rel: Vec<&str> = path
            .split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .collect();
        Ok(Name {
            id,
            files: vec![File::dummy(format!("/{}", rel.join("/")))],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let (a, b) = (self.local(&oldpath)?, self.local(&newpath)?);
        if !a.exists() {
            return Err(StatusCode::NoSuchFile);
        }
        if b.exists() && !self.ctl.rename_overwrites.load(Ordering::SeqCst) {
            return Err(StatusCode::Failure); // jak OpenSSH przy SFTP v3
        }
        std::fs::rename(a, b).map_err(io_status)?;
        Ok(Self::ok(id))
    }
}

// ----------------------------------------------------------------------------------------------------------------
// Pomocnicze

fn xfer<'a>(bw: &'a Bandwidth) -> Xfer<'a> {
    Xfer::new(bw)
}

fn write_local(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

fn mtime_of(p: &Path) -> i64 {
    filetime::FileTime::from_last_modification_time(&std::fs::metadata(p).unwrap()).unix_seconds()
}

fn tmp_leftovers(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.contains(".syncron-") {
                out.push(n.clone());
            }
            if e.path().is_dir() {
                walk(&e.path(), out);
            }
        }
    }
    walk(root, &mut out);
    out
}

// ----------------------------------------------------------------------------------------------------------------
// Testy

#[test]
fn roundtrip_polish_names_list_stat_overwrite_rename_delete() {
    let srv = TestServer::start(1);
    let b = srv.open("Kopie/Praca");
    let tmp = tempfile::tempdir().unwrap();
    let bw = Bandwidth::default();
    let x = xfer(&bw);
    let names = [
        "Zażółć gęślą jaźń.txt",
        "Dokumenty/Umowa & aneks (1).docx",
        "Dokumenty/Głębiej/Jeszcze głębiej/śćńółż.bin",
        "pusty.txt",
        "100% pewne #1.txt",
        "ZAŁĄCZNIK/ą ę.txt",
    ];
    // brak folderu głównego = pusta lista, nie błąd
    assert!(b.list_files("").unwrap().is_empty());
    for (i, n) in names.iter().enumerate() {
        let data = if *n == "pusty.txt" {
            Vec::new()
        } else {
            format!("treść {i} – zażółć").repeat(100 + i).into_bytes()
        };
        let src = write_local(tmp.path(), "src.bin", &data);
        b.upload(&src, n, 1_600_000_000 + i as i64 * 1000, &x)
            .unwrap();
    }
    assert!(
        tmp_leftovers(srv.root()).is_empty(),
        "zostały pliki tymczasowe: {:?}",
        tmp_leftovers(srv.root())
    );
    // pliki fizycznie w korzeniu + podkatalogach z polskimi nazwami
    assert!(srv
        .root()
        .join("Kopie/Praca/Dokumenty/Głębiej/Jeszcze głębiej/śćńółż.bin")
        .exists());
    let mut files = b.list_files("").unwrap();
    files.sort_by(|a, c| a.path.cmp(&c.path));
    let mut want: Vec<&str> = names.to_vec();
    want.sort();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        want
    );
    let f0 = files
        .iter()
        .find(|f| f.path == "Zażółć gęślą jaźń.txt")
        .unwrap();
    assert_eq!(f0.meta.mtime, 1_600_000_000);
    assert_eq!(b.list_files("Dokumenty").unwrap().len(), 2);
    assert!(b.list_files("Nie ma takiego").unwrap().is_empty());
    // przeglądarka folderów
    let top = b.list_dir("").unwrap();
    assert_eq!(
        top.iter()
            .filter(|d| d.is_dir)
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Dokumenty", "ZAŁĄCZNIK"]
    );
    assert!(top.iter().any(|d| !d.is_dir && d.name == "pusty.txt"));
    // stat
    let st = b.stat("Dokumenty/Umowa & aneks (1).docx").unwrap().unwrap();
    assert_eq!(st.meta.mtime, 1_600_001_000);
    assert!(st.meta.size > 100);
    assert!(b.stat("nie ma.txt").unwrap().is_none());
    assert!(b.stat("Dokumenty").unwrap().is_none());
    // pobranie
    let dest = tmp.path().join("dl.bin");
    b.download("Zażółć gęślą jaźń.txt", &dest, &x).unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "treść 0 – zażółć".repeat(100)
    );
    assert!(matches!(
        b.download("brak.txt", &dest, &x),
        Err(RError::NotFound(_))
    ));
    // nadpisanie (serwer nie nadpisuje przy rename -> ścieżka zapasowa), mtime ustawiony
    let src = write_local(tmp.path(), "nowy.bin", "nowa zawartość".as_bytes());
    b.upload(&src, "Zażółć gęślą jaźń.txt", 1_700_000_000, &x)
        .unwrap();
    b.download("Zażółć gęślą jaźń.txt", &dest, &x).unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "nowa zawartość");
    assert_eq!(
        mtime_of(&srv.root().join("Kopie/Praca/Zażółć gęślą jaźń.txt")),
        1_700_000_000
    );
    assert!(
        tmp_leftovers(srv.root()).is_empty(),
        "{:?}",
        tmp_leftovers(srv.root())
    );
    // rename: do nowego folderu, nadpisując istniejący cel
    b.rename("pusty.txt", "Nowy folder/Żółw.txt").unwrap();
    assert!(b.stat("pusty.txt").unwrap().is_none());
    assert_eq!(
        b.stat("Nowy folder/Żółw.txt").unwrap().unwrap().meta.size,
        0
    );
    b.rename("Zażółć gęślą jaźń.txt", "Nowy folder/Żółw.txt")
        .unwrap();
    assert_eq!(
        b.stat("Nowy folder/Żółw.txt").unwrap().unwrap().meta.size,
        "nowa zawartość".len() as u64
    );
    assert!(matches!(
        b.rename("nie ma", "gdzies"),
        Err(RError::NotFound(_))
    ));
    assert!(
        tmp_leftovers(srv.root()).is_empty(),
        "{:?}",
        tmp_leftovers(srv.root())
    );
    // usuwanie: brak = sukces; pusty folder znika, niepusty zostaje
    b.delete("Nowy folder/Żółw.txt").unwrap();
    b.delete("Nowy folder/Żółw.txt").unwrap();
    b.rmdir_if_empty("Nowy folder").unwrap();
    assert!(!srv.root().join("Kopie/Praca/Nowy folder").exists());
    b.rmdir_if_empty("Dokumenty").unwrap();
    assert!(srv.root().join("Kopie/Praca/Dokumenty").exists());
    b.rmdir_if_empty("Nie ma takiego").unwrap();
    // mkdirs rekurencyjne i idempotentne
    b.mkdirs("a/b/ć/d").unwrap();
    b.mkdirs("a/b/ć/d").unwrap();
    assert!(srv.root().join("Kopie/Praca/a/b/ć/d").is_dir());
    assert!(b.ping().unwrap().contains("OK"));
    srv.stop();
}

#[test]
fn rename_overwriting_server_and_base_path() {
    let srv = TestServer::start(2);
    srv.ctl.rename_overwrites.store(true, Ordering::SeqCst);
    let mut p = srv.params();
    p.public.insert("base_path".into(), "/baza".into());
    std::fs::create_dir_all(srv.root().join("baza")).unwrap();
    let b = remote::open("sftp", &p, "x").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let bw = Bandwidth::default();
    for i in 0..3 {
        let src = write_local(tmp.path(), "s", format!("wersja {i}").as_bytes());
        b.upload(&src, "plik.txt", 1_500_000_000 + i, &xfer(&bw))
            .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(srv.root().join("baza/x/plik.txt")).unwrap(),
        "wersja 2"
    );
    assert!(tmp_leftovers(srv.root()).is_empty());
    srv.stop();
}

#[test]
fn parent_dir_paths_are_rejected() {
    let srv = TestServer::start(3);
    std::fs::write(srv.root().join("sekret.txt"), "tajne").unwrap();
    let b = srv.open("podfolder");
    let tmp = tempfile::tempdir().unwrap();
    let src = write_local(tmp.path(), "a", b"x");
    let bw = Bandwidth::default();
    assert!(b.upload(&src, "../x", 0, &xfer(&bw)).is_err());
    assert!(b.upload(&src, "a/../../x", 0, &xfer(&bw)).is_err());
    assert!(b
        .download("../sekret.txt", &tmp.path().join("o"), &xfer(&bw))
        .is_err());
    assert!(b.list_files("../..").is_err());
    assert!(b.stat("a/../b").is_err());
    assert!(b.delete("..").is_err());
    assert!(b.rename("a", "../b").is_err());
    assert!(b.mkdirs("..").is_err());
    assert!(remote::open("sftp", &srv.params(), "../root").is_err());
    assert!(!srv.root().join("x").exists());
    srv.stop();
}

#[test]
fn wrong_password_user_and_missing_credentials() {
    let srv = TestServer::start(4);
    let mut p = srv.params();
    p.secret.insert("pass".into(), "zle-haslo".into());
    let b = remote::open("sftp", &p, "").unwrap();
    match b.list_files("") {
        Err(RError::Auth(m)) => assert!(!m.contains("zle-haslo")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(b.ping(), Err(RError::Auth(_))));
    let mut p = srv.params();
    p.public.insert("user".into(), "ktos-inny".into());
    assert!(matches!(
        remote::open("sftp", &p, "").unwrap().list_dir(""),
        Err(RError::Auth(_))
    ));
    let mut p = srv.params();
    p.secret.clear();
    assert!(remote::open("sftp", &p, "").is_err());
    let mut p = srv.params();
    p.public.remove("host");
    assert!(remote::open("sftp", &p, "").is_err());
    // poprawne dane nadal działają po błędach
    assert!(srv.open("").list_files("").is_ok());
    srv.stop();
}

#[test]
fn key_authentication_inline_and_file() {
    let srv = TestServer::start(5);
    let client_key = key_from_seed(77);
    *srv.ctl.authorized.lock().unwrap() = Some(client_key.public_key().clone());
    let pem = client_key.to_openssh(LineEnding::LF).unwrap().to_string();
    // klucz wklejony, bez hasła
    let mut p = srv.params();
    p.secret.clear();
    p.secret.insert("private_key".into(), pem.clone());
    let b = remote::open("sftp", &p, "").unwrap();
    assert!(b.list_files("").unwrap().is_empty());
    // klucz z pliku
    let tmp = tempfile::tempdir().unwrap();
    let kf = tmp.path().join("id_ed25519");
    std::fs::write(&kf, &pem).unwrap();
    let mut p = srv.params();
    p.secret.clear();
    p.public
        .insert("key_file".into(), kf.to_string_lossy().into_owned());
    assert!(remote::open("sftp", &p, "")
        .unwrap()
        .ping()
        .unwrap()
        .contains("OK"));
    // obcy klucz -> Auth
    let other = key_from_seed(78)
        .to_openssh(LineEnding::LF)
        .unwrap()
        .to_string();
    let mut p = srv.params();
    p.secret.clear();
    p.secret.insert("private_key".into(), other);
    assert!(matches!(
        remote::open("sftp", &p, "").unwrap().ping(),
        Err(RError::Auth(_))
    ));
    // zły klucz (śmieci) -> Auth z czytelnym komunikatem, bez paniki
    let mut p = srv.params();
    p.secret.clear();
    p.secret
        .insert("private_key".into(), "to nie jest klucz".into());
    match remote::open("sftp", &p, "") {
        Err(RError::Auth(m)) => assert!(m.contains("klucz")),
        Ok(b) => assert!(matches!(b.ping(), Err(RError::Auth(_)))),
        Err(e) => panic!("{e:?}"),
    }
    // brak pliku klucza
    let mut p = srv.params();
    p.secret.clear();
    p.public
        .insert("key_file".into(), "/nie/ma/takiego/klucza".into());
    assert!(matches!(remote::open("sftp", &p, ""), Err(RError::Auth(_))));
    // klucz odrzucony, ale hasło poprawne -> wchodzi hasłem
    let mut p = srv.params();
    p.secret.insert(
        "private_key".into(),
        key_from_seed(79)
            .to_openssh(LineEnding::LF)
            .unwrap()
            .to_string(),
    );
    assert!(remote::open("sftp", &p, "").unwrap().ping().is_ok());
    srv.stop();
}

#[test]
fn host_key_tofu_pinning_and_changed_key() {
    let srv = TestServer::start(10);
    let port = srv.port;
    let store = PathBuf::from(srv.params().get("known_hosts_store").unwrap());
    let _ = std::fs::remove_file(&store);
    // 1) pierwsze połączenie: TOFU zapisuje odcisk
    assert!(srv.open("").ping().is_ok());
    let saved = std::fs::read_to_string(&store).unwrap();
    assert!(
        saved.contains(&format!("[127.0.0.1]:{port} ssh-ed25519")),
        "{saved}"
    );
    // 2) kolejne połączenia z tym samym kluczem działają
    assert!(srv.open("").ping().is_ok());
    // 3) serwer pod tym samym adresem z NOWYM kluczem -> błąd Auth z ostrzeżeniem (to samo dane logowania)
    let dir = srv.stop();
    let srv2 = TestServer::start_on(11, port, Some(dir), None);
    match srv2.open("").ping() {
        Err(RError::Auth(m)) => {
            assert!(m.contains("ZMIENIŁ SIĘ"), "{m}");
            assert!(m.contains("SHA256:"), "{m}");
            assert!(m.contains("host_key_sha256"), "{m}");
        }
        other => panic!("oczekiwano Auth o zmianie klucza: {other:?}"),
    }
    assert!(matches!(srv2.open("").list_files(""), Err(RError::Auth(_))));
    // odcisk w pliku nie został nadpisany
    assert_eq!(std::fs::read_to_string(&store).unwrap(), saved);
    // 4) przypięty odcisk: poprawny przechodzi (nawet przy zmienionym kluczu), zły nie
    let mut p = srv2.params();
    p.public
        .insert("host_key_sha256".into(), srv2.fingerprint());
    assert!(remote::open("sftp", &p, "").unwrap().ping().is_ok());
    let mut p = srv2.params();
    p.public.insert(
        "host_key_sha256".into(),
        srv2.fingerprint().trim_start_matches("SHA256:").to_string(),
    );
    assert!(
        remote::open("sftp", &p, "").unwrap().ping().is_ok(),
        "odcisk bez prefiksu"
    );
    let mut p = srv2.params();
    p.public.insert(
        "host_key_sha256".into(),
        "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
    );
    match remote::open("sftp", &p, "").unwrap().ping() {
        Err(RError::Auth(m)) => assert!(m.contains("host_key_sha256")),
        other => panic!("{other:?}"),
    }
    // 5) known_hosts_file użytkownika: zgodny wpis OK, inny klucz -> Auth, brak wpisu -> TOFU w naszym pliku
    let tmp = tempfile::tempdir().unwrap();
    let kh = tmp.path().join("known_hosts");
    let good = srv2.host_key.public_key().to_openssh().unwrap();
    let bad = key_from_seed(99).public_key().to_openssh().unwrap();
    std::fs::write(&kh, format!("[127.0.0.1]:{port} {good}\n")).unwrap();
    let mut p = srv2.params();
    p.public
        .insert("known_hosts_file".into(), kh.to_string_lossy().into_owned());
    assert!(remote::open("sftp", &p, "").unwrap().ping().is_ok());
    std::fs::write(&kh, format!("[127.0.0.1]:{port} {bad}\n")).unwrap();
    match remote::open("sftp", &p, "").unwrap().ping() {
        Err(RError::Auth(m)) => assert!(m.contains("known_hosts"), "{m}"),
        other => panic!("{other:?}"),
    }
    srv2.stop();
    let _ = std::fs::remove_file(store);
}

#[test]
fn weak_algorithms_only_server_is_refused() {
    use russh::cipher;
    let mut pref = russh::Preferred::DEFAULT;
    pref.cipher = std::borrow::Cow::Owned(vec![cipher::AES_128_CBC, cipher::AES_256_CBC]);
    let srv = TestServer::start_on(12, 0, None, Some(pref));
    let r = srv.open("").ping();
    assert!(
        matches!(r, Err(RError::Failed(_)) | Err(RError::Network(_))),
        "{r:?}"
    );
    srv.stop();
}

#[test]
fn connection_loss_is_retried_once_and_listing_failure_is_error() {
    let srv = TestServer::start(20);
    let b = srv.open("");
    let tmp = tempfile::tempdir().unwrap();
    let bw = Bandwidth::default();
    for i in 0..5 {
        let src = write_local(tmp.path(), "s", b"x");
        b.upload(&src, &format!("d{i}/plik{i}.txt"), 1_000_000, &xfer(&bw))
            .unwrap();
    }
    // sesja bezczynna zabita przez serwer -> kolejna operacja transparentnie łączy się ponownie
    let total = srv.ctl.total_conns.load(Ordering::SeqCst);
    srv.ctl.kill_all();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(b.list_files("").unwrap().len(), 5);
    assert!(
        srv.ctl.total_conns.load(Ordering::SeqCst) > total,
        "powinno być nowe połączenie"
    );
    // zerwanie w trakcie listingu raz -> ponowienie kończy się pełną listą
    srv.ctl.kill_once_on_readdir.store(true, Ordering::SeqCst);
    assert_eq!(b.list_files("").unwrap().len(), 5);
    // zerwanie za każdym razem -> Err (silnik niczego wtedy nie usuwa), nie „pusta lista”
    srv.ctl.kill_always_on_readdir.store(true, Ordering::SeqCst);
    let r = b.list_files("");
    assert!(r.is_err(), "zerwany listing nie może być Ok: {r:?}");
    srv.ctl
        .kill_always_on_readdir
        .store(false, Ordering::SeqCst);
    assert_eq!(b.list_files("").unwrap().len(), 5);
    // zerwanie w trakcie pobierania: raz -> ponowienie; zawsze -> Err
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let src = write_local(tmp.path(), "big", &big);
    b.upload(&src, "big.bin", 1_000_000, &xfer(&bw)).unwrap();
    let dest = tmp.path().join("big.dl");
    srv.ctl.kill_after_reads.store(3, Ordering::SeqCst);
    let r = b.download("big.bin", &dest, &xfer(&bw));
    assert!(
        r.is_err(),
        "pobieranie przerwane za każdym razem powinno zwrócić błąd: {r:?}"
    );
    srv.ctl.kill_after_reads.store(-1, Ordering::SeqCst);
    b.download("big.bin", &dest, &xfer(&bw)).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), big);
    srv.stop();
}

#[test]
fn unreachable_server_is_network_error() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let srv = TestServer::start(21);
    let mut p = srv.params();
    p.public.insert("port".into(), port.to_string());
    let b = remote::open("sftp", &p, "").unwrap();
    assert!(matches!(b.list_files(""), Err(RError::Network(_))));
    srv.stop();
}

#[test]
fn parallel_transfers_respect_concurrency() {
    let srv = TestServer::start(30);
    let mut p = srv.params();
    p.public.insert("concurrency".into(), "3".into());
    let b: Arc<dyn Backend> = Arc::from(remote::open("sftp", &p, "rown").unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let mut hs = Vec::new();
    for t in 0..8 {
        let (b, dir) = (b.clone(), dir.clone());
        hs.push(std::thread::spawn(move || {
            let bw = Bandwidth::default();
            for i in 0..4 {
                let data = format!("wątek {t} plik {i} zażółć ")
                    .repeat(5000)
                    .into_bytes();
                let src = write_local(&dir, &format!("w{t}_{i}"), &data);
                let name = format!("w{t}/plik ł {i}.txt");
                b.upload(&src, &name, 1_000 + i, &Xfer::new(&bw)).unwrap();
                let dest = dir.join(format!("d{t}_{i}"));
                b.download(&name, &dest, &Xfer::new(&bw)).unwrap();
                assert_eq!(std::fs::read(&dest).unwrap(), data);
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(b.list_files("").unwrap().len(), 32);
    let max = srv.ctl.max_active.load(Ordering::SeqCst);
    assert!(max <= 3, "więcej połączeń niż concurrency: {max}");
    assert!(max >= 2, "brak równoległości: {max}");
    assert!(tmp_leftovers(srv.root()).is_empty());
    srv.stop();
}

#[test]
fn bandwidth_limit_progress_and_cancel() {
    let srv = TestServer::start(40);
    let b = srv.open("");
    let tmp = tempfile::tempdir().unwrap();
    let data = vec![0xABu8; 3 * 1024 * 1024];
    let src = write_local(tmp.path(), "big", &data);
    // 2000 KB/s => 3 MiB nie szybciej niż ok. 1,5 s
    let bw = Bandwidth {
        kbps: 2000,
        off_kbps: 0,
        hours: None,
    };
    let sent = std::sync::atomic::AtomicU64::new(0);
    let prog = |n: u64| {
        sent.fetch_add(n, Ordering::Relaxed);
    };
    let xf = Xfer {
        bw: &bw,
        cancel: None,
        progress: Some(&prog),
    };
    let t = Instant::now();
    b.upload(&src, "duzy.bin", 1_600_000_000, &xf).unwrap();
    let el = t.elapsed();
    assert!(
        el >= Duration::from_millis(1300),
        "limit wysyłania nie działa: {el:?}"
    );
    assert_eq!(sent.load(Ordering::Relaxed), data.len() as u64);
    // pobieranie z limitem
    let dest = tmp.path().join("dl");
    let t = Instant::now();
    b.download("duzy.bin", &dest, &xf).unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(1300),
        "limit pobierania nie działa"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    // anulowanie w trakcie
    let slow = Bandwidth {
        kbps: 300,
        off_kbps: 0,
        hours: None,
    };
    let cancel = AtomicBool::new(false);
    let xc = Xfer {
        bw: &slow,
        cancel: Some(&cancel),
        progress: None,
    };
    let t = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(400));
            cancel.store(true, Ordering::Relaxed);
        });
        assert!(b.upload(&src, "anulowany.bin", 1, &xc).is_err());
    });
    assert!(t.elapsed() < Duration::from_secs(5));
    assert!(!srv.root().join("anulowany.bin").exists());
    assert!(
        tmp_leftovers(srv.root()).is_empty(),
        "po anulowaniu zostały pliki tymczasowe: {:?}",
        tmp_leftovers(srv.root())
    );
    srv.stop();
}

#[cfg(unix)]
#[test]
fn symlinks_followed_for_files_skipped_for_dirs_no_loops() {
    use std::os::unix::fs::symlink;
    let srv = TestServer::start(50);
    let r = srv.root();
    std::fs::create_dir_all(r.join("a/b")).unwrap();
    std::fs::write(r.join("a/plik.txt"), "treść").unwrap();
    std::fs::write(r.join("a/b/glebiej.txt"), "x").unwrap();
    symlink(r.join("a/plik.txt"), r.join("a/link-do-pliku.txt")).unwrap();
    symlink(r.join("a"), r.join("a/b/petla")).unwrap(); // pętla: a/b/petla -> a
    symlink(r.join("a/b"), r.join("link-do-folderu")).unwrap();
    symlink(r.join("nie-ma"), r.join("zerwane")).unwrap();
    let b = srv.open("");
    let mut paths: Vec<String> = b
        .list_files("")
        .unwrap()
        .into_iter()
        .map(|e| e.path)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["a/b/glebiej.txt", "a/link-do-pliku.txt", "a/plik.txt"]
    );
    srv.stop();
}

#[test]
fn about_and_ping_do_not_fail() {
    let srv = TestServer::start(60);
    let b = srv.open("");
    // serwer testowy nie ma statvfs -> brak informacji (nie błąd)
    assert!(b.about().unwrap().is_none());
    assert_eq!(b.kind(), "sftp");
    srv.stop();
}
