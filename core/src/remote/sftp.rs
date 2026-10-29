//! Backend SFTP na czystym Rust: `russh` (SSH, szyfrowanie przez `ring`) + `russh-sftp`. Bez OpenSSL, bez C i bez
//! wywoływania programów `ssh`/`sftp`.
//!
//! Trait `Backend` jest synchroniczny, więc backend ma własny mały runtime tokio (2 wątki) i woła go z wątku
//! wywołującego (`run`), także gdy wywołujący sam działa w innym runtime.
//!
//! Zasady:
//! - pula sesji SSH o rozmiarze `concurrency` (domyślnie 4); zerwana sesja jest porzucana, a operacja ponawiana
//!   jednokrotnie na nowym połączeniu; keepalive co 30 s;
//! - klucz serwera: przypięty odcisk `host_key_sha256`, albo `known_hosts_file` (format OpenSSH), albo własny plik
//!   `known_hosts` w katalogu danych (TOFU: pierwszy klucz zapisujemy i logujemy ostrzeżenie, zmiana = błąd `Auth`);
//! - algorytmy: tylko bezpieczne (bez SHA-1 w podpisie hosta, bez CBC/3DES/RC4, kex z SHA-2);
//! - dowiązania symboliczne: do plików są podążane (stat celu), do folderów pomijane (brak pętli);
//! - wysyłanie do pliku tymczasowego `.syncron-XXXX.tmp` w tym samym folderze + zmiana nazwy (z nadpisaniem).
use super::*;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::error::Error as SftpErr;
use russh_sftp::client::fs::Metadata;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 128 * 1024;
const MAX_DEPTH: usize = 128;

// ---------------------------------------------------------------------------------------------------------------
// Klucz serwera (TOFU / przypięcie / known_hosts)

static HOSTS_LOCK: Mutex<()> = Mutex::new(());

/// Plik `known_hosts` Syncron: `known_hosts_store` z parametrów, `SYNCRON_DATA_DIR`, albo katalog danych użytkownika.
fn known_hosts_store(p: &Params) -> PathBuf {
    if let Some(s) = p.get("known_hosts_store") {
        return PathBuf::from(s);
    }
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = env("SYNCRON_DATA_DIR").unwrap_or_else(|| {
        if cfg!(windows) {
            env("APPDATA")
                .unwrap_or_else(std::env::temp_dir)
                .join("Syncron")
        } else if cfg!(target_os = "macos") {
            env("HOME")
                .unwrap_or_else(std::env::temp_dir)
                .join("Library/Application Support/Syncron")
        } else {
            env("XDG_DATA_HOME")
                .or_else(|| env("HOME").map(|h| h.join(".local/share")))
                .unwrap_or_else(std::env::temp_dir)
                .join("syncron")
        }
    });
    dir.join("known_hosts")
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(h) = home {
            return PathBuf::from(h).join(rest);
        }
    }
    PathBuf::from(p)
}

/// Czy odcisk `fp` ("SHA256:base64") zgadza się z podanym przez użytkownika (SHA256:base64 / base64 / hex z dwukropkami).
fn fingerprint_matches(pinned: &str, fp: &str) -> bool {
    let want = fp
        .strip_prefix("SHA256:")
        .unwrap_or(fp)
        .trim_end_matches('=');
    let p = pinned.trim();
    let p = p
        .strip_prefix("SHA256:")
        .or_else(|| p.strip_prefix("sha256:"))
        .unwrap_or(p)
        .trim();
    let hexish: String = p
        .chars()
        .filter(|c| *c != ':' && !c.is_whitespace())
        .collect();
    if hexish.len() == 64 && hexish.bytes().all(|b| b.is_ascii_hexdigit()) {
        // hex -> porównaj z base64 bez paddingu przez ponowne zakodowanie
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&hexish[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect();
        return b64_nopad(&bytes) == want;
    }
    p.trim_end_matches('=') == want
}

fn b64_nopad(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16
            | (*ch.get(1).unwrap_or(&0) as u32) << 8
            | *ch.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        if ch.len() > 1 {
            out.push(T[(n >> 6) as usize & 63] as char);
        }
        if ch.len() > 2 {
            out.push(T[n as usize & 63] as char);
        }
    }
    out
}

#[derive(Debug)]
struct HErr(russh::Error);
impl From<russh::Error> for HErr {
    fn from(e: russh::Error) -> Self {
        HErr(e)
    }
}

struct Hnd {
    host: String,
    port: u16,
    pinned: Option<String>,
    known_hosts_file: Option<PathBuf>,
    store: PathBuf,
    /// Powód odrzucenia klucza serwera (czytelny komunikat dla użytkownika).
    reject: std::sync::Arc<Mutex<Option<String>>>,
}

impl Hnd {
    fn verify(&self, key: &PublicKey) -> Result<(), String> {
        let fp = key.fingerprint(HashAlg::Sha256).to_string();
        let who = format!("{}:{}", self.host, self.port);
        if let Some(pin) = &self.pinned {
            return if fingerprint_matches(pin, &fp) {
                Ok(())
            } else {
                Err(format!(
                    "Odcisk klucza serwera {who} ({fp}) nie zgadza się z przypiętym w polu „host_key_sha256”. \
                     Serwer mógł zmienić klucz albo ktoś podszywa się pod serwer (atak man-in-the-middle)."
                ))
            };
        }
        let _g = HOSTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = &self.known_hosts_file {
            match russh::keys::known_hosts::check_known_hosts_path(&self.host, self.port, key, f) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(russh::keys::Error::KeyChanged { line }) => {
                    return Err(format!(
                        "Klucz serwera {who} ZMIENIŁ SIĘ względem pliku known_hosts (linia {line}, {}). Nowy odcisk: {fp}. \
                         Możliwy atak man-in-the-middle; jeśli serwer był przeinstalowany, popraw wpis w pliku.",
                        f.display()
                    ))
                }
                Err(e) => return Err(format!("Nie można odczytać pliku known_hosts ({}): {e}", f.display())),
            }
        }
        let known =
            russh::keys::known_hosts::known_host_keys_path(&self.host, self.port, &self.store)
                .map_err(|e| format!("Nie można odczytać {}: {e}", self.store.display()))?;
        if known.is_empty() {
            russh::keys::known_hosts::learn_known_hosts_path(
                &self.host,
                self.port,
                key,
                &self.store,
            )
            .map_err(|e| {
                format!(
                    "Nie można zapisać klucza serwera do {}: {e}",
                    self.store.display()
                )
            })?;
            log::warn!(
                "SFTP: pierwsze połączenie z {who}; zapisano odcisk klucza serwera {fp} w {}",
                self.store.display()
            );
            return Ok(());
        }
        if known.iter().any(|(_, k)| k == key) {
            return Ok(());
        }
        let old: Vec<String> = known
            .iter()
            .map(|(_, k)| k.fingerprint(HashAlg::Sha256).to_string())
            .collect();
        Err(format!(
            "Klucz serwera {who} ZMIENIŁ SIĘ! Zapisany odcisk: {}; obecny: {fp}. Możliwy atak man-in-the-middle. \
             Jeśli serwer był przeinstalowany, usuń jego wpis z pliku {} albo wpisz nowy odcisk w polu „host_key_sha256”.",
            old.join(", "),
            self.store.display()
        ))
    }
}

impl client::Handler for Hnd {
    type Error = HErr;

    async fn check_server_key(&mut self, k: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = k else {
            *self.reject.lock().unwrap() = Some(
                "Serwer przedstawił certyfikat SSH zamiast klucza hosta (nieobsługiwane).".into(),
            );
            return Ok(false);
        };
        match self.verify(key) {
            Ok(()) => Ok(true),
            Err(msg) => {
                *self.reject.lock().unwrap() = Some(msg);
                Ok(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Błędy

fn map_sftp(e: SftpErr) -> RError {
    match e {
        SftpErr::Status(s) => {
            let msg = if s.error_message.is_empty() {
                s.status_code.to_string()
            } else {
                format!("{}: {}", s.status_code, s.error_message)
            };
            let low = s.error_message.to_ascii_lowercase();
            match s.status_code {
                StatusCode::NoSuchFile => RError::NotFound(msg),
                StatusCode::PermissionDenied => RError::Auth(format!("brak uprawnień ({msg})")),
                StatusCode::NoConnection | StatusCode::ConnectionLost => RError::Network(msg),
                _ if low.contains("space")
                    || low.contains("quota")
                    || low.contains("disk full") =>
                {
                    RError::Quota(msg)
                }
                _ => RError::Failed(msg),
            }
        }
        SftpErr::Timeout => RError::Network("przekroczony czas odpowiedzi serwera SFTP".into()),
        SftpErr::IO(s) => RError::Network(s),
        SftpErr::Limited(s) => RError::Failed(s),
        SftpErr::UnexpectedPacket | SftpErr::UnexpectedBehavior(_) => {
            RError::Network(e.to_string())
        }
    }
}

fn map_ssh(e: russh::Error) -> RError {
    RError::Network(format!("SSH: {e}"))
}

fn is_missing(e: &SftpErr) -> bool {
    matches!(e, SftpErr::Status(s) if s.status_code == StatusCode::NoSuchFile)
}

fn local_err(e: std::io::Error) -> RError {
    RError::Failed(format!("plik lokalny: {e}"))
}

// ---------------------------------------------------------------------------------------------------------------
// Limit prędkości (wersja async: nie blokuje wątków runtime)

struct AThrottle<'a> {
    x: Xfer<'a>,
    started: Instant,
    sent: u64,
}

impl<'a> AThrottle<'a> {
    fn new(x: &Xfer<'a>) -> Self {
        AThrottle {
            x: *x,
            started: Instant::now(),
            sent: 0,
        }
    }
    async fn account(&mut self, n: u64) -> RResult<()> {
        if self.x.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(RError::Failed("przerwano".into()));
        }
        self.sent += n;
        if let Some(p) = self.x.progress {
            p(n);
        }
        let kbps = self.x.bw.current_kbps();
        if kbps > 0 {
            let due = Duration::from_secs_f64(self.sent as f64 / (kbps as f64 * 1024.0));
            if let Some(w) = due.checked_sub(self.started.elapsed()) {
                tokio::time::sleep(w).await;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Połączenia i pula

struct Conn {
    handle: client::Handle<Hnd>,
    sftp: std::sync::Arc<SftpSession>,
}

struct Inner {
    host: String,
    port: u16,
    user: String,
    pass: Option<String>,
    key: Option<std::sync::Arc<PrivateKey>>,
    key_err: Option<String>,
    pinned: Option<String>,
    known_hosts_file: Option<PathBuf>,
    store: PathBuf,
    /// Opcjonalny bezwzględny katalog bazowy (domyślnie: katalog domowy użytkownika SFTP).
    base: Option<String>,
    root: String,
    idle: Mutex<Vec<Conn>>,
    sem: tokio::sync::Semaphore,
    tmp_seq: AtomicU64,
}

pub struct SftpBackend {
    inner: std::sync::Arc<Inner>,
    rt: Option<tokio::runtime::Runtime>,
}

impl Drop for SftpBackend {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

impl Inner {
    fn client_config(&self) -> client::Config {
        let mut pref = russh::Preferred::DEFAULT;
        // Bez podpisu hosta ssh-rsa/SHA-1; zapisany klucz hosta dostaje pierwszeństwo (blokuje „obniżenie” algorytmu).
        let mut keys: Vec<russh::keys::Algorithm> = pref
            .key
            .iter()
            .filter(|a| !matches!(a, russh::keys::Algorithm::Rsa { hash: None }))
            .cloned()
            .collect();
        if self.pinned.is_none() {
            if let Ok(known) =
                russh::keys::known_hosts::known_host_keys_path(&self.host, self.port, &self.store)
            {
                let mut first: Vec<russh::keys::Algorithm> = Vec::new();
                for (_, k) in known {
                    let a = k.algorithm();
                    if a.clone().is_rsa() {
                        for r in keys.iter().filter(|x| (*x).clone().is_rsa()) {
                            if !first.contains(r) {
                                first.push(r.clone());
                            }
                        }
                    } else if !first.contains(&a) {
                        first.push(a);
                    }
                }
                if !first.is_empty() {
                    keys.retain(|a| !first.contains(a));
                    first.extend(keys);
                    keys = first;
                }
            }
        }
        pref.key = std::borrow::Cow::Owned(keys);
        client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            nodelay: true,
            window_size: 4 * 1024 * 1024,
            maximum_packet_size: 64 * 1024,
            preferred: pref,
            ..Default::default()
        }
    }

    async fn connect(&self) -> RResult<Conn> {
        let reject = std::sync::Arc::new(Mutex::new(None));
        let h = Hnd {
            host: self.host.clone(),
            port: self.port,
            pinned: self.pinned.clone(),
            known_hosts_file: self.known_hosts_file.clone(),
            store: self.store.clone(),
            reject: reject.clone(),
        };
        let cfg = std::sync::Arc::new(self.client_config());
        let addr = (self.host.as_str(), self.port);
        let connected =
            tokio::time::timeout(Duration::from_secs(25), client::connect(cfg, addr, h)).await;
        let mut handle = match connected {
            Err(_) => {
                return Err(RError::Network(format!(
                    "przekroczony czas połączenia z {}:{}",
                    self.host, self.port
                )))
            }
            Ok(Err(e)) => {
                if let Some(msg) = reject.lock().unwrap().take() {
                    return Err(RError::Auth(msg));
                }
                return Err(match e.0 {
                    russh::Error::UnknownKey => RError::Auth("Klucz serwera odrzucony.".into()),
                    russh::Error::NoCommonAlgo { .. } => RError::Failed(format!(
                        "Serwer nie obsługuje żadnego bezpiecznego algorytmu SSH wymaganego przez Syncron ({}).",
                        e.0
                    )),
                    other => RError::Network(format!("połączenie z {}:{} nie powiodło się: {other}", self.host, self.port)),
                });
            }
            Ok(Ok(h)) => h,
        };
        tokio::time::timeout(Duration::from_secs(40), self.authenticate(&mut handle))
            .await
            .map_err(|_| RError::Network("przekroczony czas logowania".into()))??;
        let chan = handle.channel_open_session().await.map_err(map_ssh)?;
        chan.request_subsystem(true, "sftp")
            .await
            .map_err(|e| RError::Failed(format!("serwer nie udostępnia podsystemu SFTP: {e}")))?;
        let cfg = russh_sftp::client::Config {
            request_timeout_secs: 60,
            ..Default::default()
        };
        let sftp = SftpSession::new_with_config(chan.into_stream(), cfg)
            .await
            .map_err(|e| RError::Failed(format!("start SFTP: {}", map_sftp(e))))?;
        Ok(Conn {
            handle,
            sftp: std::sync::Arc::new(sftp),
        })
    }

    async fn authenticate(&self, handle: &mut client::Handle<Hnd>) -> RResult<()> {
        let denied = || {
            RError::Auth(format!(
                "serwer odrzucił logowanie użytkownika „{}” (sprawdź hasło lub klucz)",
                self.user
            ))
        };
        if let Some(e) = &self.key_err {
            if self.pass.is_none() {
                return Err(RError::Auth(e.clone()));
            }
        }
        if let Some(key) = &self.key {
            let hashes: Vec<Option<HashAlg>> = if key.algorithm().is_rsa() {
                vec![Some(HashAlg::Sha512), Some(HashAlg::Sha256)]
            } else {
                vec![None]
            };
            for h in hashes {
                let r = handle
                    .authenticate_publickey(
                        self.user.clone(),
                        PrivateKeyWithHashAlg::new(key.clone(), h),
                    )
                    .await
                    .map_err(map_ssh)?;
                if r.success() {
                    return Ok(());
                }
            }
        }
        if let Some(pass) = &self.pass {
            let r = handle
                .authenticate_password(self.user.clone(), pass.clone())
                .await
                .map_err(map_ssh)?;
            if r.success() {
                return Ok(());
            }
            // Część serwerów (NAS, Windows) wystawia hasło tylko jako keyboard-interactive.
            use russh::client::KeyboardInteractiveAuthResponse as K;
            let mut resp = handle
                .authenticate_keyboard_interactive_start(self.user.clone(), None)
                .await
                .map_err(map_ssh)?;
            for _ in 0..4 {
                match resp {
                    K::Success => return Ok(()),
                    K::Failure { .. } => break,
                    K::InfoRequest { prompts, .. } => {
                        let answers = prompts.iter().map(|_| pass.clone()).collect();
                        resp = handle
                            .authenticate_keyboard_interactive_respond(answers)
                            .await
                            .map_err(map_ssh)?;
                    }
                }
            }
        }
        if self.key.is_none() && self.pass.is_none() {
            return Err(RError::Auth(
                "brak hasła i klucza prywatnego w konfiguracji konta".into(),
            ));
        }
        if let Some(e) = &self.key_err {
            return Err(RError::Auth(format!(
                "{e}; logowanie hasłem także nie powiodło się"
            )));
        }
        Err(denied())
    }

    fn take_idle(&self) -> Option<Conn> {
        let mut g = self.idle.lock().unwrap();
        while let Some(c) = g.pop() {
            if !c.handle.is_closed() {
                return Some(c);
            }
        }
        None
    }

    /// Wykonuje operację na sesji z puli; przy błędzie sieci porzuca sesję i ponawia raz na świeżym połączeniu.
    async fn with_conn<T, F, Fut>(&self, f: F) -> RResult<T>
    where
        F: Fn(std::sync::Arc<SftpSession>) -> Fut,
        Fut: Future<Output = RResult<T>>,
    {
        let _permit = self
            .sem
            .acquire()
            .await
            .map_err(|_| RError::Failed("backend zamknięty".into()))?;
        let mut last = RError::Network("brak połączenia".into());
        for attempt in 0..2 {
            let conn = match if attempt == 0 { self.take_idle() } else { None } {
                Some(c) => c,
                None => self.connect().await?,
            };
            let r = f(conn.sftp.clone()).await;
            match r {
                Err(RError::Network(m)) => {
                    last = RError::Network(m);
                    self.idle.lock().unwrap().clear();
                    drop(conn);
                    if attempt == 0 {
                        log::debug!("SFTP: błąd sieci, ponawiam na nowym połączeniu");
                    }
                }
                other => {
                    if !conn.handle.is_closed() {
                        self.idle.lock().unwrap().push(conn);
                    }
                    return other;
                }
            }
        }
        Err(last)
    }

    /// Ścieżka zdalna dla ścieżki względnej backendu.
    fn rpath(&self, rel: &str) -> String {
        let full = join_path(&self.root, rel);
        match &self.base {
            Some(b) if full.is_empty() => b.clone(),
            Some(b) if b == "/" => format!("/{full}"),
            Some(b) => format!("{b}/{full}"),
            None if full.is_empty() => ".".to_string(),
            None => full,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Operacje (async, na jednej sesji)

fn mtime_of(m: &Metadata) -> i64 {
    m.mtime.map(|t| t as i64).unwrap_or(0)
}

fn bad_name(n: &str) -> bool {
    n.is_empty()
        || n == "."
        || n == ".."
        || n.contains('/')
        || n.contains('\\')
        || n.chars().any(|c| (c as u32) < 32)
}

async fn list_tree(s: &SftpSession, inner: &Inner, dir: &str) -> RResult<Vec<REntry>> {
    let mut out = Vec::new();
    let mut stack: Vec<(String, usize)> = vec![(dir.to_string(), 0)];
    let mut first = true;
    while let Some((rel, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(RError::Failed(format!(
                "zbyt głęboka struktura folderów (>{MAX_DEPTH}): {rel}"
            )));
        }
        let remote = inner.rpath(&rel);
        let rd = match s.read_dir(remote.as_str()).await {
            Ok(r) => r,
            Err(e) if is_missing(&e) => {
                if first {
                    return Ok(out); // brak folderu głównego = pusta lista
                }
                continue; // folder zniknął w trakcie przeglądania
            }
            Err(e) => return Err(map_sftp(e)),
        };
        first = false;
        for ent in rd {
            let name = ent.file_name();
            if bad_name(&name) {
                continue;
            }
            let child_rel = join_path(&rel, &name);
            let mut meta = ent.metadata();
            let mut ty = ent.file_type();
            if meta.permissions.is_none() || ty.is_symlink() {
                // brak typu w odpowiedzi lub dowiązanie: sprawdź cel
                match s.metadata(inner.rpath(&child_rel)).await {
                    Ok(m) => {
                        ty = m.file_type();
                        meta = m;
                        if ent.file_type().is_symlink() && ty.is_dir() {
                            continue; // dowiązania do folderów pomijamy (ochrona przed pętlami)
                        }
                    }
                    Err(e) if is_missing(&e) => continue, // zerwane dowiązanie
                    Err(e) => return Err(map_sftp(e)),
                }
            }
            if ty.is_dir() {
                stack.push((child_rel, depth + 1));
            } else if ty.is_file() {
                out.push(REntry {
                    path: child_rel,
                    meta: RMeta {
                        size: meta.size.unwrap_or(0),
                        mtime: mtime_of(&meta),
                        hash: None,
                    },
                });
            }
        }
    }
    Ok(out)
}

async fn mkdirs_abs(s: &SftpSession, path: &str) -> RResult<()> {
    // Najpierw najczęstszy przypadek: folder już jest.
    if let Ok(m) = s.metadata(path).await {
        if m.is_dir() {
            return Ok(());
        }
    }
    let absolute = path.starts_with('/');
    let mut cur = String::new();
    for comp in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
        if cur.is_empty() {
            cur = if absolute {
                format!("/{comp}")
            } else {
                comp.to_string()
            };
        } else {
            cur = format!("{cur}/{comp}");
        }
        match s.metadata(cur.as_str()).await {
            Ok(m) if m.is_dir() => continue,
            Ok(_) => {
                return Err(RError::Failed(format!(
                    "„{cur}” istnieje i nie jest folderem"
                )))
            }
            Err(e) if is_missing(&e) => {}
            Err(e) => return Err(map_sftp(e)),
        }
        if let Err(e) = s.create_dir(cur.as_str()).await {
            // wyścig z innym wątkiem: jeśli teraz jest folderem — OK
            match s.metadata(cur.as_str()).await {
                Ok(m) if m.is_dir() => {}
                _ => return Err(map_sftp(e)),
            }
        }
    }
    Ok(())
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
}

async fn download_one(
    s: &SftpSession,
    remote: &str,
    shown: &str,
    dest: &Path,
    x: &Xfer<'_>,
) -> RResult<()> {
    let mut f = s.open(remote).await.map_err(map_sftp)?;
    let size = f.metadata().await.ok().and_then(|m| m.size);
    let mut out = std::fs::File::create(dest).map_err(local_err)?;
    let mut t = AThrottle::new(x);
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| RError::Network(format!("odczyt {shown}: {e}")))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(local_err)?;
        total += n as u64;
        t.account(n as u64).await?;
    }
    out.flush().map_err(local_err)?;
    let _ = f.close().await;
    if let Some(sz) = size {
        if sz != total {
            return Err(RError::Network(format!(
                "pobieranie przerwane: {total} z {sz} bajtów"
            )));
        }
    }
    Ok(())
}

async fn replace_by_rename(s: &SftpSession, from: &str, to: &str, seq: u64) -> RResult<()> {
    match s.rename(from, to).await {
        Ok(()) => Ok(()),
        Err(first) => {
            // SFTP v3 nie nadpisuje celu: przesuń stary plik na bok, wstaw nowy, usuń kopię (z przywróceniem przy błędzie).
            if !s.try_exists(to).await.unwrap_or(false) {
                return Err(map_sftp(first));
            }
            let bak = format!("{to}.syncron-old-{seq}");
            s.rename(to, bak.as_str()).await.map_err(map_sftp)?;
            if let Err(e) = s.rename(from, to).await {
                let _ = s.rename(bak.as_str(), to).await;
                return Err(map_sftp(e));
            }
            let _ = s.remove_file(bak).await;
            Ok(())
        }
    }
}

async fn upload_one(
    s: &SftpSession,
    inner: &Inner,
    src: &Path,
    dest: &str,
    mtime: i64,
    x: &Xfer<'_>,
) -> RResult<()> {
    if let Some(parent) = parent_of(dest) {
        mkdirs_abs(s, parent).await?;
    }
    let seq = inner.tmp_seq.fetch_add(1, Ordering::Relaxed);
    let uniq = format!("{:x}{:x}", std::process::id(), seq);
    let tmp = match parent_of(dest) {
        Some(p) => format!("{p}/.syncron-{uniq}.tmp"),
        None => format!(".syncron-{uniq}.tmp"),
    };
    let r = upload_to_tmp(s, src, &tmp, dest, mtime, seq, x).await;
    if r.is_err() {
        let _ = s.remove_file(tmp.as_str()).await;
    }
    r
}

async fn upload_to_tmp(
    s: &SftpSession,
    src: &Path,
    tmp: &str,
    dest: &str,
    mtime: i64,
    seq: u64,
    x: &Xfer<'_>,
) -> RResult<()> {
    let mut input = std::fs::File::open(src).map_err(local_err)?;
    let size = input.metadata().map_err(local_err)?.len();
    let mut f = s
        .open_with_flags(
            tmp,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .map_err(map_sftp)?;
    let mut t = AThrottle::new(x);
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(local_err)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])
            .await
            .map_err(|e| RError::Network(format!("zapis: {e}")))?;
        sent += n as u64;
        t.account(n as u64).await?;
    }
    if sent != size {
        return Err(RError::Failed(
            "plik lokalny zmienił się podczas wysyłania".into(),
        ));
    }
    f.shutdown()
        .await
        .map_err(|e| RError::Network(format!("zamknięcie pliku: {e}")))?;
    if mtime > 0 && mtime <= u32::MAX as i64 {
        let attrs = FileAttributes {
            atime: Some(mtime as u32),
            mtime: Some(mtime as u32),
            ..Default::default()
        };
        // Nie każdy serwer pozwala ustawić czas (np. chroot bez uprawnień) — to nie jest błąd wysyłania.
        if let Err(e) = s.set_metadata(tmp, attrs).await {
            if matches!(e, SftpErr::Timeout | SftpErr::IO(_)) {
                return Err(map_sftp(e));
            }
            log::debug!("SFTP: nie ustawiono czasu modyfikacji: {e}");
        }
    }
    let got = s.metadata(tmp).await.map_err(map_sftp)?.size.unwrap_or(0);
    if got != size {
        return Err(RError::Network(format!(
            "serwer zapisał {got} z {size} bajtów"
        )));
    }
    replace_by_rename(s, tmp, dest, seq).await
}

// ---------------------------------------------------------------------------------------------------------------
// Backend

impl SftpBackend {
    pub fn new(p: &Params, root: &str) -> RResult<SftpBackend> {
        let host = p.need("host")?.trim().to_string();
        let port = p.num("port", 22);
        if port == 0 || port > 65535 {
            return Err(RError::Failed("niepoprawny numer portu".into()));
        }
        let user = p.need("user")?.to_string();
        let pass = p.secret("pass").map(|s| s.to_string());
        let (mut key, mut key_err) = (None, None);
        let key_pass = p.secret("key_pass").or_else(|| p.secret("key_file_pass"));
        let key_text = if let Some(k) = p.secret("private_key") {
            Some(Ok(k.to_string()))
        } else {
            p.get("key_file").map(|f| {
                std::fs::read_to_string(expand_home(f))
                    .map_err(|e| format!("Nie można odczytać pliku klucza „{f}”: {e}"))
            })
        };
        match key_text {
            Some(Ok(t)) => match russh::keys::decode_secret_key(&t, key_pass) {
                Ok(k) => key = Some(std::sync::Arc::new(k)),
                Err(e) => {
                    key_err = Some(format!(
                        "Nie można wczytać klucza prywatnego (zła fraza klucza lub nieobsługiwany format): {e}"
                    ))
                }
            },
            Some(Err(e)) => key_err = Some(e),
            None => {}
        }
        if key.is_none() && pass.is_none() {
            return Err(match key_err {
                Some(e) => RError::Auth(e),
                None => RError::Failed(
                    "brak wymaganego pola „pass” albo „key_file”/„private_key”".into(),
                ),
            });
        }
        let base = p.get("base_path").map(|b| {
            let t = b.trim_end_matches('/');
            if t.is_empty() {
                "/".to_string()
            } else {
                t.to_string()
            }
        });
        let conc = p.num("concurrency", 4).clamp(1, 16) as usize;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("syncron-sftp")
            .enable_all()
            .build()
            .map_err(|e| RError::Failed(format!("runtime: {e}")))?;
        Ok(SftpBackend {
            inner: std::sync::Arc::new(Inner {
                host,
                port: port as u16,
                user,
                pass,
                key,
                key_err,
                pinned: p.get("host_key_sha256").map(|s| s.to_string()),
                known_hosts_file: p.get("known_hosts_file").map(expand_home),
                store: known_hosts_store(p),
                base,
                root: root.to_string(),
                idle: Mutex::new(Vec::new()),
                sem: tokio::sync::Semaphore::new(conc),
                tmp_seq: AtomicU64::new(0),
            }),
            rt: Some(rt),
        })
    }

    /// Uruchamia future na runtime backendu z wątku wywołującego (bez `block_on` w jego kontekście).
    fn run<F>(&self, f: F) -> F::Output
    where
        F: Future + Send,
        F::Output: Send,
    {
        let rt = self.rt.as_ref().expect("runtime");
        std::thread::scope(|sc| match sc.spawn(|| rt.block_on(f)).join() {
            Ok(v) => v,
            Err(p) => std::panic::resume_unwind(p),
        })
    }
}

impl Backend for SftpBackend {
    fn kind(&self) -> &'static str {
        "sftp"
    }

    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>> {
        let dir = clean_path(dir)?;
        let (inner, d) = (&*self.inner, &dir);
        self.run(inner.with_conn(|s| async move { list_tree(&s, inner, d).await }))
    }

    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>> {
        let dir = clean_path(dir)?;
        let (inner, d) = (&*self.inner, &dir);
        self.run(inner.with_conn(|s| async move {
            let remote = inner.rpath(d);
            let rd = match s.read_dir(remote.as_str()).await {
                Ok(r) => r,
                Err(e) if is_missing(&e) => return Ok(Vec::new()),
                Err(e) => return Err(map_sftp(e)),
            };
            let mut items = Vec::new();
            for ent in rd {
                let name = ent.file_name();
                if bad_name(&name) {
                    continue;
                }
                let mut ty = ent.file_type();
                if ty.is_symlink() || ent.metadata().permissions.is_none() {
                    match s.metadata(inner.rpath(&join_path(d, &name))).await {
                        Ok(m) => ty = m.file_type(),
                        Err(e) if is_missing(&e) => continue,
                        Err(e) => return Err(map_sftp(e)),
                    }
                }
                if ty.is_dir() || ty.is_file() {
                    items.push(DirItem {
                        name,
                        is_dir: ty.is_dir(),
                    });
                }
            }
            items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
            Ok(items)
        }))
    }

    fn stat(&self, path: &str) -> RResult<Option<REntry>> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Ok(None);
        }
        let (inner, r) = (&*self.inner, &rel);
        self.run(inner.with_conn(|s| async move {
            match s.metadata(inner.rpath(r)).await {
                Ok(m) if m.is_dir() => Ok(None),
                Ok(m) => Ok(Some(REntry {
                    path: r.clone(),
                    meta: RMeta {
                        size: m.size.unwrap_or(0),
                        mtime: mtime_of(&m),
                        hash: None,
                    },
                })),
                Err(e) if is_missing(&e) => Ok(None),
                Err(e) => Err(map_sftp(e)),
            }
        }))
    }

    fn download(&self, path: &str, dest: &Path, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka".into()));
        }
        let (inner, r) = (&*self.inner, &rel);
        self.run(
            inner.with_conn(|s| async move { download_one(&s, &inner.rpath(r), r, dest, x).await }),
        )
    }

    fn upload(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka docelowa".into()));
        }
        let (inner, r) = (&*self.inner, &rel);
        self.run(inner.with_conn(|s| async move {
            upload_one(&s, inner, src, &inner.rpath(r), mtime, x).await
        }))
    }

    fn delete(&self, path: &str) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka".into()));
        }
        let (inner, r) = (&*self.inner, &rel);
        self.run(inner.with_conn(|s| async move {
            match s.remove_file(inner.rpath(r)).await {
                Ok(()) => Ok(()),
                Err(e) if is_missing(&e) => Ok(()),
                Err(e) => Err(map_sftp(e)),
            }
        }))
    }

    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        let (f, t) = (clean_path(from)?, clean_path(to)?);
        if f.is_empty() || t.is_empty() {
            return Err(RError::Failed("pusta ścieżka".into()));
        }
        if f == t {
            return Ok(());
        }
        let (inner, f, t) = (&*self.inner, &f, &t);
        self.run(inner.with_conn(|s| async move {
            let (a, b) = (inner.rpath(f), inner.rpath(t));
            if !s.try_exists(a.as_str()).await.map_err(map_sftp)? {
                return Err(RError::NotFound(f.clone()));
            }
            if let Some(parent) = parent_of(&b) {
                mkdirs_abs(&s, parent).await?;
            }
            let seq = inner.tmp_seq.fetch_add(1, Ordering::Relaxed);
            replace_by_rename(&s, &a, &b, seq).await
        }))
    }

    fn mkdirs(&self, dir: &str) -> RResult<()> {
        let dir = clean_path(dir)?;
        let (inner, d) = (&*self.inner, &dir);
        self.run(inner.with_conn(|s| async move { mkdirs_abs(&s, &inner.rpath(d)).await }))
    }

    fn rmdir_if_empty(&self, dir: &str) -> RResult<()> {
        let dir = clean_path(dir)?;
        if dir.is_empty() {
            return Ok(());
        }
        let (inner, d) = (&*self.inner, &dir);
        self.run(inner.with_conn(|s| async move {
            match s.remove_dir(inner.rpath(d)).await {
                Ok(()) => Ok(()),
                // niepusty / nieistniejący / brak uprawnień do usunięcia: sprzątanie jest opcjonalne
                Err(e @ (SftpErr::Timeout | SftpErr::IO(_))) => Err(map_sftp(e)),
                Err(_) => Ok(()),
            }
        }))
    }

    fn about(&self) -> RResult<Option<Quota>> {
        let inner = &*self.inner;
        self.run(inner.with_conn(|s| async move {
            match s.fs_info(inner.rpath("")).await {
                Ok(Some(v)) => {
                    let unit = if v.fragment_size > 0 {
                        v.fragment_size
                    } else {
                        v.block_size
                    };
                    let total = v.blocks.saturating_mul(unit);
                    let free = v.blocks_avail.saturating_mul(unit);
                    Ok(Some(Quota {
                        total: Some(total),
                        used: Some(total.saturating_sub(v.blocks_free.saturating_mul(unit))),
                        free: Some(free),
                    }))
                }
                Ok(None) => Ok(None),
                Err(SftpErr::Status(_)) => Ok(None),
                Err(e) => Err(map_sftp(e)),
            }
        }))
    }

    fn ping(&self) -> RResult<String> {
        let inner = &*self.inner;
        self.run(inner.with_conn(|s| async move {
            s.read_dir(inner.rpath("")).await.map_err(map_sftp)?;
            Ok(format!("OK (SFTP {}:{})", inner.host, inner.port))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_fingerprint_forms() {
        let bytes: Vec<u8> = (0u8..32).collect();
        let b64 = b64_nopad(&bytes);
        let fp = format!("SHA256:{b64}");
        assert!(fingerprint_matches(&fp, &fp));
        assert!(fingerprint_matches(&b64, &fp));
        assert!(fingerprint_matches(&format!("{b64}="), &fp));
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert!(fingerprint_matches(&hex, &fp));
        let colon: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        assert!(fingerprint_matches(&colon.join(":"), &fp));
        assert!(!fingerprint_matches("SHA256:AAAA", &fp));
        assert_eq!(b64_nopad(b"Man"), "TWFu");
        assert_eq!(b64_nopad(b"Ma"), "TWE");
        assert_eq!(b64_nopad(b"M"), "TQ");
    }
}
