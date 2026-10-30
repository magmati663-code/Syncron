//! Natywny klient FTP/FTPS (`kind = "ftp"` i `kind = "ftps"`) oparty na crate `suppaftp` (rustls + ring, bez OpenSSL).
//!
//! Parametry (`Params`): `host`, `port` (21; dla `tls=implicit` domyślnie 990), `user`, `pass` (sekret; brak użytkownika =
//! logowanie anonimowe), `tls` = `none|explicit|implicit` (dla `kind="ftps"` brak lub `none` oznacza `explicit`),
//! `no_check_certificate`, `passive` (domyślnie tak; `0/false/nie` = tryb aktywny), `concurrency` (1–8, domyślnie 4).
//!
//! Zachowanie:
//! - pula połączeń (do `concurrency`), każde połączenie: TLS (jeśli trzeba), logowanie, `TYPE I`, `OPTS UTF8 ON`, `FEAT`;
//! - nieużywane połączenia dłużej niż 15 s są przed ponownym użyciem sprawdzane `NOOP` (keepalive „na żądanie”);
//! - listing: `MLSD`, a gdy serwer go nie zna — `LIST` (Unix i DOS) z doprecyzowaniem czasu przez `MDTM`;
//! - wysyłanie: do pliku `.syncron-up-XXXX` w tym samym folderze, `SIZE`, ustawienie czasu (`MFMT`, a gdy brak —
//!   `MDTM <czas> <plik>`; jeśli serwer żadnego nie zna, czas pozostaje czasem wysłania — liczy się tolerancja czasu),
//!   potem `RNFR/RNTO` (gdy cel istnieje i serwer odmawia nadpisania: `DELE` + `RNTO`);
//! - zerwane połączenie = `RError::Network`, operacja jest ponawiana raz na nowym połączeniu; zerwany listing = `Err`;
//! - nazwy z `..` lub `/` zwrócone przez serwer są pomijane.
//!
//! Ograniczenia: dla `tls=implicit` biblioteka nie pozwala ustawić limitu czasu łączenia (działa systemowy); FTP nie ma
//! sum kontrolnych, więc `Hash` jest zawsze `None`; limit czasu danych 90 s.
use super::http::ThrottledReader;
use super::*;
use std::collections::HashSet;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use suppaftp::types::FileType;
use suppaftp::{FtpError, RustlsConnector, RustlsFtpStream, Status};

type Client = RustlsFtpStream;

const TMP_PREFIX: &str = ".syncron-up-";
const IO_TIMEOUT: Duration = Duration::from_secs(90);
const IDLE_CHECK: Duration = Duration::from_secs(15);
const MAX_DEPTH: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tls {
    None,
    Explicit,
    Implicit,
}

#[derive(Debug, Clone)]
struct FEntry {
    name: String,
    is_dir: bool,
    size: u64,
    mtime: i64,
}

type ListResult = RResult<Option<Vec<FEntry>>>;

struct Pooled {
    c: Client,
    at: Instant,
}

#[derive(Default)]
struct Pool {
    idle: Vec<Pooled>,
    open: usize,
}

pub struct FtpBackend {
    kind: &'static str,
    host: String,
    port: u16,
    user: String,
    pass: String,
    tls: Tls,
    no_check: bool,
    passive: bool,
    conc: usize,
    root: Vec<String>,
    pool: Mutex<Pool>,
    cv: Condvar,
    /// 0 = nieznane, 1 = tak, 2 = nie
    mlsd: AtomicU8,
    /// 0 = nieznane, 1 = MFMT, 2 = MDTM z czasem, 3 = brak
    setmtime: AtomicU8,
    /// 0 = nieznane, 1 = tak, 2 = nie
    mdtm: AtomicU8,
    known_dirs: Mutex<HashSet<String>>,
    tmp_seq: AtomicUsize,
}

fn short(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

fn ferr(e: FtpError) -> RError {
    match e {
        FtpError::ConnectionError(e) => RError::Network(e.to_string()),
        FtpError::SecureError(s) => RError::Network(format!("TLS: {s}")),
        FtpError::UnexpectedResponse(r) => {
            let code = r.status.code();
            let msg = format!("FTP {code} {}", short(&r.body));
            match code {
                530 | 430 | 332 | 331 => RError::Auth(msg),
                421 | 450 | 425 => RError::Busy(msg),
                426 => RError::Network(msg),
                452 | 552 => RError::Quota(msg),
                550 => RError::NotFound(msg),
                _ => RError::Failed(msg),
            }
        }
        other => RError::Failed(other.to_string()),
    }
}

/// Błąd listowania: nieczytelny/przerwany strumień danych (`BadResponse`) to w praktyce zerwane połączenie.
fn lerr(e: FtpError) -> RError {
    match e {
        FtpError::BadResponse => {
            RError::Network("przerwana lub nieczytelna lista z serwera".into())
        }
        other => ferr(other),
    }
}

fn code_of(e: &FtpError) -> Option<u32> {
    match e {
        FtpError::UnexpectedResponse(r) => Some(r.status.code()),
        _ => None,
    }
}

fn comps(p: &str) -> Vec<String> {
    p.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(|c| c.to_string())
        .collect()
}

fn ftp_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%d%H%M%S")
        .to_string()
}

fn parse_mdtm_fact(m: &str) -> i64 {
    let m = m.trim();
    if m.len() < 14 || !m.is_char_boundary(14) {
        return 0;
    }
    chrono::NaiveDateTime::parse_from_str(&m[..14], "%Y%m%d%H%M%S")
        .map(|d| d.and_utc().timestamp())
        .unwrap_or(0)
}

fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n != "."
        && n != ".."
        && !n.contains('/')
        && !n.contains('\\')
        && !n.chars().any(|c| (c as u32) < 32)
}

fn parse_mlsd(lines: &[String]) -> Vec<FEntry> {
    let mut out = Vec::new();
    for l in lines {
        let l = l.trim_end_matches(['\r', '\n']);
        let Some((facts, name)) = l.split_once(' ') else {
            continue;
        };
        if !safe_name(name) {
            continue;
        }
        let (mut ty, mut size, mut mtime) = (String::new(), 0u64, 0i64);
        for f in facts.split(';').filter(|f| !f.is_empty()) {
            let Some((k, v)) = f.split_once('=') else {
                continue;
            };
            match k.to_ascii_lowercase().as_str() {
                "type" => ty = v.to_ascii_lowercase(),
                "size" => size = v.parse().unwrap_or(0),
                "modify" => mtime = parse_mdtm_fact(v),
                _ => {}
            }
        }
        let is_dir = match ty.as_str() {
            "file" => false,
            "dir" => true,
            // cdir, pdir, dowiązania (OS.unix=slink...), urządzenia: pomijamy
            _ => continue,
        };
        out.push(FEntry {
            name: name.to_string(),
            is_dir,
            size,
            mtime,
        });
    }
    out
}

fn tok(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

fn month(m: &str) -> Option<u32> {
    const M: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    M.iter()
        .position(|x| m.to_ascii_lowercase().starts_with(x))
        .map(|i| i as u32 + 1)
}

/// Parsuje wiersz `LIST` w formacie Unix lub DOS/IIS.
fn parse_list_line(line: &str) -> Option<FEntry> {
    use chrono::{Datelike, NaiveDate, NaiveTime};
    let line = line.trim_end_matches(['\r', '\n']);
    let first = line.chars().next()?;
    if first.is_ascii_digit() {
        // DOS: 01-05-24  12:34PM       <DIR>          nazwa
        let (d, r) = tok(line);
        let (t, r) = tok(r);
        let (sz, r) = tok(r);
        let name = r.trim_start();
        let dp: Vec<u32> = d.split('-').filter_map(|x| x.parse().ok()).collect();
        if dp.len() != 3 {
            return None;
        }
        let year = if dp[2] < 100 {
            if dp[2] < 70 {
                2000 + dp[2]
            } else {
                1900 + dp[2]
            }
        } else {
            dp[2]
        };
        let tu = t.to_ascii_uppercase();
        let pm = tu.ends_with("PM");
        let tt = tu.trim_end_matches("AM").trim_end_matches("PM");
        let (h, m) = tt.split_once(':')?;
        let mut h: u32 = h.parse().ok()?;
        if pm && h < 12 {
            h += 12;
        }
        if !pm && tu.ends_with("AM") && h == 12 {
            h = 0;
        }
        let dt = NaiveDate::from_ymd_opt(year as i32, dp[0], dp[1])?
            .and_time(NaiveTime::from_hms_opt(h, m.parse().ok()?, 0)?);
        let is_dir = sz.eq_ignore_ascii_case("<DIR>");
        return Some(FEntry {
            name: name.to_string(),
            is_dir,
            size: if is_dir { 0 } else { sz.parse().ok()? },
            mtime: dt.and_utc().timestamp(),
        });
    }
    if !matches!(first, '-' | 'd') {
        return None; // l (dowiązanie), "total", urządzenia itd.
    }
    let (_perm, r) = tok(line);
    let (_links, r) = tok(r);
    let (_owner, r) = tok(r);
    let (_group, r) = tok(r);
    let (size, r) = tok(r);
    let (mon, r) = tok(r);
    let (day, r) = tok(r);
    let (ty, r) = tok(r);
    let name = r.get(1..)?;
    let size: u64 = size.parse().ok()?;
    let mon = month(mon)?;
    let day: u32 = day.parse().ok()?;
    let now = chrono::Utc::now();
    let mtime = if let Some((h, m)) = ty.split_once(':') {
        let mut y = now.year();
        let mut dt = NaiveDate::from_ymd_opt(y, mon, day)?.and_time(NaiveTime::from_hms_opt(
            h.parse().ok()?,
            m.parse().ok()?,
            0,
        )?);
        if dt.and_utc().timestamp() > now.timestamp() + 86400 {
            y -= 1;
            dt = NaiveDate::from_ymd_opt(y, mon, day)?.and_time(NaiveTime::from_hms_opt(
                h.parse().ok()?,
                m.parse().ok()?,
                0,
            )?);
        }
        dt.and_utc().timestamp()
    } else {
        NaiveDate::from_ymd_opt(ty.parse().ok()?, mon, day)?
            .and_time(NaiveTime::MIN)
            .and_utc()
            .timestamp()
    };
    Some(FEntry {
        name: name.to_string(),
        is_dir: first == 'd',
        size,
        mtime,
    })
}

fn parse_list(lines: &[String]) -> Vec<FEntry> {
    lines
        .iter()
        .filter_map(|l| parse_list_line(l))
        .filter(|e| safe_name(&e.name))
        .collect()
}

impl FtpBackend {
    pub fn new(kind: &'static str, p: &Params, root: &str) -> RResult<FtpBackend> {
        let host = p.need("host")?.trim().to_string();
        let tls = match (p.get("tls").unwrap_or(""), kind) {
            ("implicit", _) => Tls::Implicit,
            ("explicit", _) => Tls::Explicit,
            ("none", "ftps") | ("", "ftps") => Tls::Explicit,
            _ => Tls::None,
        };
        let default_port = if tls == Tls::Implicit { 990 } else { 21 };
        let port = p.num("port", default_port);
        if port == 0 || port > 65535 {
            return Err(RError::Failed("nieprawidłowy port".into()));
        }
        Ok(FtpBackend {
            kind: if kind == "ftps" { "ftps" } else { "ftp" },
            host,
            port: port as u16,
            user: p.get("user").unwrap_or("").to_string(),
            pass: p.secret("pass").unwrap_or("").to_string(),
            tls,
            no_check: p.flag("no_check_certificate"),
            passive: !matches!(p.get("passive"), Some("0" | "false" | "no" | "nie")),
            conc: p.num("concurrency", 4).clamp(1, 8) as usize,
            root: comps(root),
            pool: Mutex::new(Pool::default()),
            cv: Condvar::new(),
            mlsd: AtomicU8::new(0),
            setmtime: AtomicU8::new(0),
            mdtm: AtomicU8::new(0),
            known_dirs: Mutex::new(HashSet::new()),
            tmp_seq: AtomicUsize::new(0),
        })
    }

    // ------------------------------------------------------------------------------------------------ połączenia

    fn connect(&self) -> RResult<Client> {
        let addrs: Vec<_> = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| RError::Network(format!("nie można rozwiązać adresu {}: {e}", self.host)))?
            .collect();
        let cfg = || RustlsConnector::from(super::webdav::tls_config(self.no_check));
        let mut c = if self.tls == Tls::Implicit {
            Client::connect_secure_implicit((self.host.as_str(), self.port), cfg(), &self.host)
                .map_err(ferr)?
        } else {
            let mut last = RError::Network("brak adresu serwera".into());
            let mut tcp = None;
            for a in addrs {
                match TcpStream::connect_timeout(&a, Duration::from_secs(20)) {
                    Ok(t) => {
                        tcp = Some(t);
                        break;
                    }
                    Err(e) => last = RError::Network(format!("połączenie z {a}: {e}")),
                }
            }
            let tcp = tcp.ok_or(last)?;
            let _ = tcp.set_read_timeout(Some(IO_TIMEOUT));
            let _ = tcp.set_write_timeout(Some(IO_TIMEOUT));
            let _ = tcp.set_nodelay(true);
            let c = Client::connect_with_stream(tcp).map_err(ferr)?;
            if self.tls == Tls::Explicit {
                c.into_secure(cfg(), &self.host).map_err(ferr)?
            } else {
                c
            }
        };
        {
            let s = c.get_ref();
            let _ = s.set_read_timeout(Some(IO_TIMEOUT));
            let _ = s.set_write_timeout(Some(IO_TIMEOUT));
        }
        if self.passive {
            c.set_passive_nat_workaround(true);
            c = c.passive_stream_builder(|addr| {
                let t = TcpStream::connect_timeout(&addr, Duration::from_secs(20))
                    .map_err(FtpError::ConnectionError)?;
                let _ = t.set_read_timeout(Some(IO_TIMEOUT));
                let _ = t.set_write_timeout(Some(IO_TIMEOUT));
                let _ = t.set_nodelay(true);
                Ok(t)
            });
        } else {
            c = c.active_mode(Duration::from_secs(30));
        }
        let (u, pw) = if self.user.is_empty() {
            ("anonymous", "anonymous@")
        } else {
            (self.user.as_str(), self.pass.as_str())
        };
        c.login(u, pw).map_err(|e| match ferr(e) {
            RError::NotFound(m) | RError::Failed(m) => RError::Auth(m),
            other => other,
        })?;
        if self.tls == Tls::Implicit {
            // przy TLS niejawnym biblioteka nie negocjuje ochrony kanału danych — robimy to sami
            c.custom_command("PBSZ 0", &[Status::CommandOk])
                .map_err(ferr)?;
            c.custom_command("PROT P", &[Status::CommandOk])
                .map_err(ferr)?;
        }
        c.transfer_type(FileType::Binary).map_err(ferr)?;
        let _ = c.custom_command("OPTS UTF8 ON", &[Status::CommandOk]);
        if self.mlsd.load(Ordering::Relaxed) == 0 {
            if let Ok(f) = c.feat() {
                let has = |n: &str| f.keys().any(|k| k.trim().eq_ignore_ascii_case(n));
                self.mlsd
                    .store(if has("MLST") { 1 } else { 2 }, Ordering::Relaxed);
                if has("MFMT") {
                    let _ =
                        self.setmtime
                            .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed);
                }
            }
        }
        Ok(c)
    }

    fn acquire(&self, fresh: bool) -> RResult<Pooled> {
        let mut g = self.pool.lock().unwrap();
        if fresh {
            let n = g.idle.len();
            g.idle.clear();
            g.open -= n;
        }
        loop {
            if let Some(mut p) = g.idle.pop() {
                if p.at.elapsed() > IDLE_CHECK {
                    drop(g);
                    if p.c.noop().is_ok() {
                        return Ok(p);
                    }
                    g = self.pool.lock().unwrap();
                    g.open -= 1;
                    self.cv.notify_one();
                    continue;
                }
                return Ok(p);
            }
            if g.open < self.conc {
                g.open += 1;
                drop(g);
                return match self.connect() {
                    Ok(c) => Ok(Pooled {
                        c,
                        at: Instant::now(),
                    }),
                    Err(e) => {
                        self.pool.lock().unwrap().open -= 1;
                        self.cv.notify_one();
                        Err(e)
                    }
                };
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    fn release(&self, mut p: Pooled, keep: bool) {
        let mut g = self.pool.lock().unwrap();
        if keep {
            p.at = Instant::now();
            g.idle.push(p);
        } else {
            g.open -= 1;
        }
        self.cv.notify_one();
    }

    /// Wykonuje `op` na połączeniu z puli; przy błędzie sieci ponawia raz na nowym połączeniu.
    fn with_conn<T>(&self, mut op: impl FnMut(&mut Client) -> RResult<T>) -> RResult<T> {
        let mut fresh = false;
        for attempt in 0..2 {
            let mut p = self.acquire(fresh)?;
            match op(&mut p.c) {
                Ok(v) => {
                    self.release(p, true);
                    return Ok(v);
                }
                Err(RError::Network(m)) => {
                    self.release(p, false);
                    if attempt == 1 {
                        return Err(RError::Network(m));
                    }
                    fresh = true;
                }
                Err(e) => {
                    let keep = matches!(
                        e,
                        RError::NotFound(_) | RError::Failed(_) | RError::Quota(_)
                    );
                    self.release(p, keep);
                    return Err(e);
                }
            }
        }
        unreachable!()
    }

    // ------------------------------------------------------------------------------------------------ ścieżki

    fn full(&self, rel: &str) -> String {
        let mut v: Vec<&str> = self.root.iter().map(|s| s.as_str()).collect();
        v.extend(rel.split('/').filter(|c| !c.is_empty()));
        v.join("/")
    }

    fn arg(full: &str) -> String {
        if full.starts_with('-') {
            format!("./{full}")
        } else {
            full.to_string()
        }
    }

    // ------------------------------------------------------------------------------------------------ operacje

    /// Zawartość folderu; `None` = folder nie istnieje.
    fn list_raw(&self, c: &mut Client, full: &str) -> RResult<Option<Vec<FEntry>>> {
        let arg = if full.is_empty() {
            None
        } else {
            Some(Self::arg(full))
        };
        if self.mlsd.load(Ordering::Relaxed) != 2 {
            match c.mlsd(arg.as_deref()) {
                Ok(l) => {
                    self.mlsd.store(1, Ordering::Relaxed);
                    return Ok(Some(parse_mlsd(&l)));
                }
                Err(e) => match code_of(&e) {
                    Some(550) => return Ok(None),
                    Some(500 | 502 | 504 | 501) => {
                        if code_of(&e) != Some(501) {
                            self.mlsd.store(2, Ordering::Relaxed);
                        }
                    }
                    _ => return Err(lerr(e)),
                },
            }
        }
        match c.list(arg.as_deref()) {
            Ok(l) => {
                let mut v = parse_list(&l);
                self.refine_mtimes(c, full, &mut v)?;
                Ok(Some(v))
            }
            Err(e) if code_of(&e) == Some(550) => Ok(None),
            Err(e) => Err(lerr(e)),
        }
    }

    /// `LIST` daje czas z dokładnością do minuty/dnia — doprecyzowanie przez `MDTM` (jeśli serwer go zna).
    fn refine_mtimes(&self, c: &mut Client, dir: &str, v: &mut [FEntry]) -> RResult<()> {
        if self.mdtm.load(Ordering::Relaxed) == 2 {
            return Ok(());
        }
        for e in v.iter_mut().filter(|e| !e.is_dir) {
            let path = if dir.is_empty() {
                e.name.clone()
            } else {
                format!("{dir}/{}", e.name)
            };
            match c.mdtm(Self::arg(&path)) {
                Ok(t) => {
                    self.mdtm.store(1, Ordering::Relaxed);
                    e.mtime = t.and_utc().timestamp();
                }
                Err(FtpError::ConnectionError(x)) => return Err(RError::Network(x.to_string())),
                Err(err) => {
                    if self.mdtm.load(Ordering::Relaxed) == 0
                        && matches!(code_of(&err), Some(500 | 502 | 504))
                    {
                        self.mdtm.store(2, Ordering::Relaxed);
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    fn par_list(&self, dirs: &[String]) -> Vec<ListResult> {
        let n = self.conc.min(dirs.len());
        let next = AtomicUsize::new(0);
        let results: Mutex<Vec<Option<ListResult>>> =
            Mutex::new((0..dirs.len()).map(|_| None).collect());
        std::thread::scope(|s| {
            for _ in 0..n {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= dirs.len() {
                        break;
                    }
                    let full = self.full(&dirs[i]);
                    let r = self.with_conn(|c| self.list_raw(c, &full));
                    results.lock().unwrap()[i] = Some(r);
                });
            }
        });
        results
            .into_inner()
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(RError::Failed("przerwano listowanie".into()))))
            .collect()
    }

    fn ensure_dirs(&self, c: &mut Client, rel_dir: &str, force: bool) -> RResult<()> {
        let mut acc: Vec<String> = Vec::new();
        let all: Vec<String> = self
            .root
            .iter()
            .cloned()
            .chain(
                rel_dir
                    .split('/')
                    .filter(|c| !c.is_empty())
                    .map(|s| s.to_string()),
            )
            .collect();
        for comp in all {
            acc.push(comp);
            let p = acc.join("/");
            if !force && self.known_dirs.lock().unwrap().contains(&p) {
                continue;
            }
            match c.mkdir(Self::arg(&p)) {
                Ok(()) => {}
                Err(e) => match code_of(&e) {
                    // 550/521/553: zwykle „już istnieje” — sprawdzamy to na końcu przez listing nadrzędnego
                    Some(550 | 521 | 553 | 501) => {}
                    _ => return Err(ferr(e)),
                },
            }
            self.known_dirs.lock().unwrap().insert(p);
        }
        // weryfikacja: ostatni folder musi istnieć
        if !acc.is_empty() {
            let full = acc.join("/");
            if self.list_raw(c, &full)?.is_none() {
                self.known_dirs.lock().unwrap().remove(&full);
                return Err(RError::Failed(format!(
                    "nie można utworzyć folderu „{full}”"
                )));
            }
        }
        Ok(())
    }

    fn set_mtime(&self, c: &mut Client, path: &str, t: i64) {
        if t <= 0 {
            return;
        }
        let ts = ftp_time(t);
        let st = self.setmtime.load(Ordering::Relaxed);
        let mut unsupported = 0;
        if st == 0 || st == 1 {
            match c.custom_command(format!("MFMT {ts} {path}"), &[Status::File]) {
                Ok(_) => {
                    self.setmtime.store(1, Ordering::Relaxed);
                    return;
                }
                Err(e) => {
                    if matches!(code_of(&e), Some(500 | 501 | 502 | 504)) {
                        unsupported += 1;
                    }
                }
            }
        }
        if st == 0 || st == 2 || st == 1 {
            match c.custom_command(format!("MDTM {ts} {path}"), &[Status::File]) {
                Ok(_) => {
                    self.setmtime.store(2, Ordering::Relaxed);
                    return;
                }
                Err(e) => {
                    if matches!(code_of(&e), Some(500 | 501 | 502 | 504)) {
                        unsupported += 1;
                    }
                }
            }
        }
        if unsupported >= 2 || (st == 2 && unsupported >= 1) {
            self.setmtime.store(3, Ordering::Relaxed);
        }
    }

    fn tmp_name(&self, dir_full: &str) -> String {
        let n = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = format!(
            "{:08x}",
            (t as u64 ^ ((n as u64) << 40) ^ (std::process::id() as u64) << 20) & 0xffff_ffff
        );
        if dir_full.is_empty() {
            format!("{TMP_PREFIX}{id}")
        } else {
            format!("{dir_full}/{TMP_PREFIX}{id}")
        }
    }

    fn stat_conn(&self, c: &mut Client, full: &str) -> RResult<Option<FEntry>> {
        let name = full.rsplit('/').next().unwrap_or(full).to_string();
        match c.size(Self::arg(full)) {
            Ok(sz) => {
                let mtime = c
                    .mdtm(Self::arg(full))
                    .map(|t| t.and_utc().timestamp())
                    .unwrap_or(0);
                Ok(Some(FEntry {
                    name,
                    is_dir: false,
                    size: sz as u64,
                    mtime,
                }))
            }
            Err(e) => match code_of(&e) {
                Some(550) => Ok(None),
                Some(500 | 501 | 502 | 504) => {
                    let parent = full.rsplit_once('/').map(|x| x.0).unwrap_or("");
                    Ok(self
                        .list_raw(c, parent)?
                        .and_then(|l| l.into_iter().find(|x| x.name == name && !x.is_dir)))
                }
                _ => Err(ferr(e)),
            },
        }
    }
}

impl Drop for FtpBackend {
    fn drop(&mut self) {
        if let Ok(mut g) = self.pool.lock() {
            for mut p in g.idle.drain(..) {
                let _ = p.c.quit();
            }
        }
    }
}

impl Backend for FtpBackend {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>> {
        let start = clean_path(dir)?;
        let mut files = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(start.clone());
        let mut level = vec![start];
        let mut depth = 0;
        while !level.is_empty() {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(RError::Failed(
                    "zbyt głębokie drzewo folderów (pętla na serwerze?)".into(),
                ));
            }
            let results = self.par_list(&level);
            let mut next = Vec::new();
            for (d, r) in level.iter().zip(results) {
                for e in r?.unwrap_or_default() {
                    if e.name.starts_with(TMP_PREFIX) {
                        continue;
                    }
                    let rel = if d.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{d}/{}", e.name)
                    };
                    if e.is_dir {
                        if seen.insert(rel.clone()) {
                            next.push(rel);
                        }
                    } else {
                        files.push(REntry {
                            path: rel,
                            meta: RMeta {
                                size: e.size,
                                mtime: e.mtime,
                                hash: None,
                            },
                        });
                    }
                }
            }
            level = next;
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>> {
        let d = clean_path(dir)?;
        let full = self.full(&d);
        let l = self
            .with_conn(|c| self.list_raw(c, &full))?
            .ok_or_else(|| RError::NotFound(format!("folder „{d}” nie istnieje")))?;
        Ok(l.into_iter()
            .filter(|e| !e.name.starts_with(TMP_PREFIX))
            .map(|e| DirItem {
                name: e.name,
                is_dir: e.is_dir,
            })
            .collect())
    }

    fn stat(&self, path: &str) -> RResult<Option<REntry>> {
        let rel = clean_path(path)?;
        let full = self.full(&rel);
        if rel.is_empty() {
            return Ok(None);
        }
        Ok(self
            .with_conn(|c| self.stat_conn(c, &full))?
            .map(|e| REntry {
                path: rel,
                meta: RMeta {
                    size: e.size,
                    mtime: e.mtime,
                    hash: None,
                },
            }))
    }

    fn download(&self, path: &str, dest: &Path, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        let full = self.full(&rel);
        let r = self.with_conn(|c| {
            let expected = c.size(Self::arg(&full)).ok().map(|s| s as u64);
            let mut s = c.retr_as_stream(Self::arg(&full)).map_err(ferr)?;
            let got = super::http::copy_to_file(&mut s, dest, x)?;
            s.finish().map_err(ferr)?;
            if let Some(e) = expected {
                if e != got {
                    return Err(RError::Network(format!(
                        "pobieranie przerwane: odebrano {got} z {e} bajtów"
                    )));
                }
            }
            Ok(())
        });
        if r.is_err() {
            let _ = std::fs::remove_file(dest);
        }
        r
    }

    fn upload(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka docelowa".into()));
        }
        let len = std::fs::metadata(src)
            .map_err(|e| RError::Failed(format!("nie można odczytać pliku: {e}")))?
            .len();
        let full = self.full(&rel);
        let parent = full.rsplit_once('/').map(|p| p.0).unwrap_or("").to_string();
        let rel_parent = rel.rsplit_once('/').map(|p| p.0).unwrap_or("").to_string();
        self.with_conn(|c| {
            // foldery nadrzędne (z pamięcią podręczną, żeby nie wysyłać MKD przy każdym pliku)
            let known = self.known_dirs.lock().unwrap().contains(&parent);
            if !known {
                self.ensure_dirs(c, &rel_parent, false)?;
            }
            let tmp = self.tmp_name(&parent);
            let file = std::fs::File::open(src)
                .map_err(|e| RError::Failed(format!("nie można otworzyć pliku: {e}")))?;
            let res = (|| -> RResult<()> {
                let mut s = match c.put_with_stream(Self::arg(&tmp)) {
                    Ok(s) => s,
                    Err(e) if code_of(&e) == Some(550) || code_of(&e) == Some(553) => {
                        // folder mógł zniknąć: utwórz od nowa i spróbuj raz jeszcze
                        self.ensure_dirs(c, &rel_parent, true)?;
                        c.put_with_stream(Self::arg(&tmp)).map_err(ferr)?
                    }
                    Err(e) => return Err(ferr(e)),
                };
                let mut rd = ThrottledReader::new(file, x);
                if let Err(e) = std::io::copy(&mut rd, &mut s) {
                    if x.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Err(RError::Failed("przerwano".into()));
                    }
                    return Err(RError::Network(e.to_string()));
                }
                s.finish().map_err(ferr)?;
                if let Ok(sz) = c.size(Self::arg(&tmp)) {
                    if sz as u64 != len {
                        return Err(RError::Failed(format!(
                            "rozmiar na serwerze ({sz}) różni się od wysłanego ({len})"
                        )));
                    }
                }
                self.set_mtime(c, &tmp, mtime);
                match c.rename(Self::arg(&tmp), Self::arg(&full)) {
                    Ok(()) => Ok(()),
                    Err(_) => {
                        // serwer nie nadpisuje istniejącego pliku przy RNTO: usuń cel i ponów
                        let _ = c.rm(Self::arg(&full));
                        c.rename(Self::arg(&tmp), Self::arg(&full)).map_err(ferr)
                    }
                }
            })();
            if res.is_err() {
                let _ = c.rm(Self::arg(&tmp));
            }
            res
        })
    }

    fn delete(&self, path: &str) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("odmowa usunięcia korzenia".into()));
        }
        let full = self.full(&rel);
        self.with_conn(|c| match c.rm(Self::arg(&full)) {
            Ok(()) => Ok(()),
            Err(e) if code_of(&e) == Some(550) => {
                // 550 = brak pliku albo brak uprawnień — rozróżniamy przez SIZE
                match self.stat_conn(c, &full)? {
                    None => Ok(()),
                    Some(_) => Err(RError::Failed(format!("FTP nie pozwolił usunąć „{rel}”"))),
                }
            }
            Err(e) => Err(ferr(e)),
        })
    }

    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        let (f, t) = (clean_path(from)?, clean_path(to)?);
        let (ff, tf) = (self.full(&f), self.full(&t));
        let tparent = t.rsplit_once('/').map(|p| p.0).unwrap_or("").to_string();
        self.with_conn(|c| {
            self.ensure_dirs(c, &tparent, false)?;
            match c.rename(Self::arg(&ff), Self::arg(&tf)) {
                Ok(()) => Ok(()),
                Err(e) => {
                    if self.stat_conn(c, &ff)?.is_none() {
                        return Err(RError::NotFound(format!("nie znaleziono: {f}")));
                    }
                    if matches!(e, FtpError::ConnectionError(_)) {
                        return Err(ferr(e));
                    }
                    let _ = c.rm(Self::arg(&tf));
                    c.rename(Self::arg(&ff), Self::arg(&tf)).map_err(ferr)
                }
            }
        })
    }

    fn mkdirs(&self, dir: &str) -> RResult<()> {
        let d = clean_path(dir)?;
        self.with_conn(|c| self.ensure_dirs(c, &d, false))
    }

    fn rmdir_if_empty(&self, dir: &str) -> RResult<()> {
        let d = clean_path(dir)?;
        if d.is_empty() {
            return Ok(());
        }
        let full = self.full(&d);
        self.with_conn(|c| match c.rmdir(Self::arg(&full)) {
            Ok(()) => {
                self.known_dirs.lock().unwrap().clear();
                Ok(())
            }
            // niepusty / nieistniejący / brak uprawnień: nic nie robimy
            Err(e) if matches!(code_of(&e), Some(550 | 450 | 553)) => Ok(()),
            Err(e) => Err(ferr(e)),
        })
    }

    fn ping(&self) -> RResult<String> {
        let n = self.list_dir("")?.len();
        let proto = match self.tls {
            Tls::None => "FTP",
            Tls::Explicit => "FTPS (explicit)",
            Tls::Implicit => "FTPS (implicit)",
        };
        let l = if self.mlsd.load(Ordering::Relaxed) == 2 {
            "LIST"
        } else {
            "MLSD"
        };
        Ok(format!("{proto}: OK, {n} elementów w korzeniu ({l})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mlsd_lines() {
        let v = parse_mlsd(&[
            "type=cdir;modify=20240101000000; .".into(),
            "type=file;size=12;modify=20240102030405.123;UNIX.mode=0644; Zażółć gęślą.txt".into(),
            "type=dir;modify=20240102030405; pod folder".into(),
            "type=file;size=1; ../evil".into(),
            "type=OS.unix=slink:/x;size=1; link".into(),
        ]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Zażółć gęślą.txt");
        assert_eq!(v[0].size, 12);
        assert_eq!(v[0].mtime, 1704164645);
        assert!(v[1].is_dir);
    }

    #[test]
    fn list_unix_dos() {
        let u = parse_list_line("-rw-r--r--   1 ftp ftp      1234 Jan  5  2023 plik ze spacją.txt")
            .unwrap();
        assert_eq!(
            (u.name.as_str(), u.size, u.is_dir),
            ("plik ze spacją.txt", 1234, false)
        );
        assert_eq!(u.mtime, 1672876800);
        let d = parse_list_line("drwxr-xr-x   2 ftp ftp      4096 Mar 10 12:30 folder").unwrap();
        assert!(d.is_dir);
        assert!(parse_list_line("lrwxrwxrwx 1 a b 3 Jan 1 2020 l -> x").is_none());
        assert!(parse_list_line("total 12").is_none());
        let w = parse_list_line("01-05-24  12:34PM       <DIR>          My Dir").unwrap();
        assert!(w.is_dir);
        assert_eq!(w.name, "My Dir");
        let f = parse_list_line("01-05-2024  09:05AM              2048 a b.bin").unwrap();
        assert_eq!((f.size, f.name.as_str()), (2048, "a b.bin"));
        assert_eq!(f.mtime, 1704445500);
    }
}
