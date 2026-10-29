//! Natywny klient WebDAV (`kind = "webdav"`): Nextcloud, ownCloud, SharePoint (WebDAV), Bitrix24 i inne serwery.
//!
//! Parametry (`Params`):
//! - `url` (wymagany) — adres folderu WebDAV konta, np. `https://cloud.example/remote.php/dav/files/jan/`;
//! - `vendor` — `nextcloud|owncloud|sharepoint|bitrix24|other` (wpływa tylko na sposób ustawiania czasu modyfikacji);
//! - `user`, `pass` (sekret) — uwierzytelnianie Basic; `bearer` (sekret) — token Bearer zamiast Basic;
//! - `no_check_certificate` — wyłącza weryfikację certyfikatu TLS (tylko świadomie, np. sieć domowa).
//!
//! Bitrix24: adres w stylu `https://firma.bitrix24.pl/company/personal/user/ID/disk/path/` (albo
//! `https://firma.bitrix24.pl/docs/path/` dla dysku firmowego), uwierzytelnianie Basic z **hasłem aplikacji**
//! (Bitrix24 → Profil → Hasła aplikacji → „Dysk”), nie z hasłem do konta.
//!
//! Ograniczenia: uwierzytelnianie Digest NIE jest obsługiwane (serwer, który oferuje wyłącznie Digest, daje czytelny błąd
//! `Auth`); SharePoint Online nie przyjmuje Basic (wymaga OAuth/ciasteczek) — działa SharePoint lokalny z Basic.
//! Wznawianie przerwanych transferów (Range) nie jest używane. Usuwanie pustego folderu sprawdza pustość osobnym
//! zapytaniem (nie ma w WebDAV warunkowego usunięcia pustego folderu, więc istnieje krótkie okno wyścigu).
use super::http::{self, ThrottledReader};
use super::*;
use std::collections::HashSet;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const LIST_THREADS: usize = 4;
const MAX_DEPTH: usize = 100;
const MAX_XML: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Vendor {
    Nextcloud,
    Owncloud,
    Sharepoint,
    Bitrix24,
    Other,
}

pub struct WebDav {
    agent: ureq::Agent,
    /// `scheme://host[:port]`
    origin: String,
    /// Zdekodowane komponenty ścieżki adresu konta.
    base: Vec<String>,
    /// Zdekodowane komponenty folderu głównego.
    root: Vec<String>,
    auth: Option<String>,
    vendor: Vendor,
    retry_ms: u64,
    /// Foldery, o których wiemy, że istnieją (ścieżki względem korzenia) — oszczędza MKCOL-e.
    known_dirs: Mutex<HashSet<String>>,
    /// Alternatywne prefiksy ścieżki, pod którymi serwer zwraca `href` po przekierowaniu (np. `/old/` → `/dav/`).
    alt_bases: Mutex<Vec<Vec<String>>>,
}

/// Błąd pojedynczego wywołania: z biblioteki HTTP albo lokalny (np. brak pliku do wysłania).
enum CErr {
    U(Box<ureq::Error>),
    Local(RError),
}

fn code_of(e: &CErr) -> Option<u16> {
    match e {
        CErr::U(b) => match &**b {
            ureq::Error::Status(c, _) => Some(*c),
            _ => None,
        },
        _ => None,
    }
}

enum Body<'a> {
    Empty,
    Text(&'a str),
    File(&'a Path, u64, &'a Xfer<'a>),
}

#[derive(Default, Debug, Clone)]
struct RawResp {
    href: String,
    is_dir: bool,
    size: Option<u64>,
    mtime: Option<i64>,
    hash: Option<Hash>,
    avail: Option<i64>,
    used: Option<i64>,
    /// Status na poziomie `response` (np. 404 dla zniknętego zasobu).
    bad_status: bool,
}

type ListResult = RResult<Option<Vec<Item>>>;

struct Item {
    rel: String,
    is_dir: bool,
    size: u64,
    mtime: i64,
    hash: Option<Hash>,
}

// ---------------------------------------------------------------------------------------------------------------------
// TLS bez weryfikacji (wspólne z FTPS)

#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Konfiguracja klienta TLS (rustls + ring); `no_check` wyłącza weryfikację certyfikatu serwera.
pub fn tls_config(no_check: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let b = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("wersje protokołu TLS");
    let cfg = if no_check {
        b.dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        b.with_root_certificates(roots).with_no_client_auth()
    };
    Arc::new(cfg)
}

// ---------------------------------------------------------------------------------------------------------------------
// Pomocnicze

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

fn comps(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(|c| c.to_string())
        .collect()
}

/// Dzieli adres na `(origin, zdekodowana ścieżka)`.
fn split_url(u: &str) -> RResult<(String, String)> {
    let u = u.trim();
    let (scheme, rest) = u.split_once("://").ok_or_else(|| {
        RError::Failed("adres WebDAV musi zaczynać się od http:// lub https://".into())
    })?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(RError::Failed(
            "adres WebDAV musi zaczynać się od http:// lub https://".into(),
        ));
    }
    let (auth, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if auth.is_empty() || auth.contains('@') {
        return Err(RError::Failed(
            "nieprawidłowy adres WebDAV (login i hasło podaj w osobnych polach)".into(),
        ));
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    Ok((
        format!("{}://{}", scheme.to_ascii_lowercase(), auth),
        http::url_decode(path),
    ))
}

fn encode_loc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b == b' ' || b >= 0x80 {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(b as char);
        }
    }
    out
}

fn origin_of(url: &str) -> &str {
    match url.find("://") {
        Some(i) => match url[i + 3..].find('/') {
            Some(j) => &url[..i + 3 + j],
            None => url,
        },
        None => url,
    }
}

fn resolve(base: &str, loc: &str) -> String {
    let loc = encode_loc(loc.trim());
    if loc.contains("://") {
        loc
    } else if loc.starts_with('/') {
        format!("{}{}", origin_of(base), loc)
    } else {
        let b = base.split(['?', '#']).next().unwrap_or(base);
        let dir = match b.rfind('/') {
            Some(i) if i > origin_of(b).len() => &b[..=i],
            _ => &format!("{}/", origin_of(b)),
        };
        format!("{dir}{loc}")
    }
}

fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc2822(s)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(s))
        .ok()
        .map(|d| d.timestamp())
}

fn rfc1123(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

fn local_name(n: &[u8]) -> String {
    let s = String::from_utf8_lossy(n).to_ascii_lowercase();
    match s.rsplit_once(':') {
        Some((_, l)) => l.to_string(),
        None => s,
    }
}

fn status_ok(s: &str) -> bool {
    match s.split_whitespace().nth(1) {
        Some(c) => c.starts_with('2'),
        None => true,
    }
}

fn pick_hash(text: &str) -> Option<Hash> {
    let mut sha1 = None;
    let mut md5 = None;
    for tok in text.split_whitespace() {
        if let Some((a, h)) = tok.split_once(':') {
            let h = h.to_ascii_lowercase();
            if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            match (a.to_ascii_lowercase().as_str(), h.len()) {
                ("sha1", 40) => sha1 = Some(h),
                ("md5", 32) => md5 = Some(h),
                _ => {}
            }
        }
    }
    sha1.map(|hex| Hash { algo: "sha1", hex })
        .or(md5.map(|hex| Hash { algo: "md5", hex }))
}

/// Odporny parser odpowiedzi `207 Multi-Status`: ignoruje prefiksy przestrzeni nazw (`D:`, `d:`, domyślna),
/// bierze tylko właściwości z `propstat` o statusie 2xx.
fn parse_multistatus(xml: &str) -> RResult<Vec<RawResp>> {
    use quick_xml_037::events::Event;
    let mut rd = quick_xml_037::Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut out = Vec::new();
    let mut cur: Option<RawResp> = None;
    let mut ps = RawResp::default();
    let mut ps_ok = true;
    loop {
        match rd.read_event() {
            Ok(Event::Start(e)) => {
                let n = local_name(e.name().as_ref());
                match n.as_str() {
                    "response" => cur = Some(RawResp::default()),
                    "propstat" => {
                        ps = RawResp::default();
                        ps_ok = true;
                    }
                    "collection" => ps.is_dir = true,
                    _ => {}
                }
                stack.push(n);
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == "collection" {
                    ps.is_dir = true;
                }
                text.clear();
            }
            Ok(Event::Text(t)) => match t.unescape() {
                Ok(s) => text.push_str(&s),
                Err(_) => text.push_str(&String::from_utf8_lossy(&t)),
            },
            Ok(Event::CData(c)) => text.push_str(&String::from_utf8_lossy(&c)),
            Ok(Event::End(_)) => {
                let n = stack.pop().unwrap_or_default();
                let parent = stack.last().map(|s| s.as_str()).unwrap_or("");
                let t = text.trim();
                match (n.as_str(), parent) {
                    ("href", "response") => {
                        if let Some(c) = cur.as_mut() {
                            c.href = t.to_string();
                        }
                    }
                    ("status", "propstat") => ps_ok = status_ok(t),
                    ("status", "response") => {
                        if let Some(c) = cur.as_mut() {
                            c.bad_status = !status_ok(t);
                        }
                    }
                    ("getcontentlength", _) => ps.size = t.parse().ok(),
                    ("getlastmodified", _) => ps.mtime = parse_time(t),
                    ("checksum", _) => ps.hash = ps.hash.take().or_else(|| pick_hash(t)),
                    ("quota-available-bytes", _) => ps.avail = t.parse().ok(),
                    ("quota-used-bytes", _) => ps.used = t.parse().ok(),
                    ("propstat", _) => {
                        if ps_ok {
                            if let Some(c) = cur.as_mut() {
                                c.is_dir |= ps.is_dir;
                                c.size = c.size.or(ps.size);
                                c.mtime = c.mtime.or(ps.mtime);
                                c.hash = c.hash.take().or(ps.hash.take());
                                c.avail = c.avail.or(ps.avail);
                                c.used = c.used.or(ps.used);
                            }
                        }
                    }
                    ("response", _) => {
                        if let Some(c) = cur.take() {
                            out.push(c);
                        }
                    }
                    _ => {}
                }
                text.clear();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(RError::Failed(format!(
                    "niepoprawna odpowiedź XML serwera: {e}"
                )))
            }
        }
    }
    Ok(out)
}

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/><d:getetag/><oc:checksums/></d:prop></d:propfind>"#;
const QUOTA_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:quota-available-bytes/><d:quota-used-bytes/></d:prop></d:propfind>"#;

impl WebDav {
    pub fn new(p: &Params, root: &str) -> RResult<WebDav> {
        let (origin, path) = split_url(p.need("url")?)?;
        let vendor = match p.get("vendor").unwrap_or("other") {
            "nextcloud" => Vendor::Nextcloud,
            "owncloud" => Vendor::Owncloud,
            "sharepoint" => Vendor::Sharepoint,
            "bitrix24" => Vendor::Bitrix24,
            _ => Vendor::Other,
        };
        let auth = if let Some(t) = p.secret("bearer") {
            Some(format!("Bearer {t}"))
        } else {
            p.get("user").map(|u| {
                format!(
                    "Basic {}",
                    b64(format!("{u}:{}", p.secret("pass").unwrap_or("")).as_bytes())
                )
            })
        };
        let mut ab = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(20))
            .timeout_read(Duration::from_secs(90))
            .timeout_write(Duration::from_secs(90))
            .redirects(0)
            .user_agent(concat!("Syncron/", env!("CARGO_PKG_VERSION")));
        if p.flag("no_check_certificate") {
            ab = ab.tls_config(tls_config(true));
        }
        Ok(WebDav {
            agent: ab.build(),
            origin,
            base: comps(&path),
            root: comps(root),
            auth,
            vendor,
            retry_ms: p.num("retry_delay_ms", 1000),
            known_dirs: Mutex::new(HashSet::new()),
            alt_bases: Mutex::new(Vec::new()),
        })
    }

    /// Adres zasobu (`rel` względem korzenia); foldery z końcowym `/`.
    fn url(&self, rel: &str, dir: bool) -> String {
        let mut parts: Vec<String> = self.base.clone();
        parts.extend(self.root.iter().cloned());
        parts.extend(comps(rel));
        let mut u = format!("{}/{}", self.origin, http::url_path(&parts.join("/")));
        if dir && !parts.is_empty() {
            u.push('/');
        }
        u
    }

    /// `href` z serwera → ścieżka względna do korzenia; `None` = poza korzeniem lub podejrzana (`..`).
    fn href_to_rel(&self, href: &str) -> Option<String> {
        let mut h = href.trim();
        if let Some(i) = h.find("://") {
            let after = &h[i + 3..];
            h = after.find('/').map(|j| &after[j..]).unwrap_or("/");
        }
        let h = h.split(['?', '#']).next().unwrap_or("");
        let dec = http::url_decode(h);
        let parts: Vec<&str> = dec
            .split(['/', '\\'])
            .filter(|c| !c.is_empty() && *c != ".")
            .collect();
        if parts
            .iter()
            .any(|c| *c == ".." || c.chars().any(|ch| (ch as u32) < 32))
        {
            return None;
        }
        let mut bases = vec![self.base.clone()];
        bases.extend(self.alt_bases.lock().unwrap().iter().cloned());
        for b in bases {
            let prefix: Vec<&String> = b.iter().chain(self.root.iter()).collect();
            if parts.len() >= prefix.len()
                && prefix.iter().zip(&parts).all(|(a, b)| a.as_str() == *b)
            {
                return Some(parts[prefix.len()..].join("/"));
            }
        }
        None
    }

    /// Po przekierowaniu zmieniającym prefiks ścieżki zapamiętuje nowy prefiks (serwer poda `href` pod nim).
    fn learn_redirect(&self, orig: &str, fin: &str) {
        let path = |u: &str| {
            let rest = &u[origin_of(u).len()..];
            comps(&http::url_decode(
                rest.split(['?', '#']).next().unwrap_or(""),
            ))
        };
        let (r, f) = (path(orig), path(fin));
        let suffix = r
            .iter()
            .rev()
            .zip(f.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_prefix = &r[..r.len() - suffix];
        if old_prefix.len() > self.base.len() || !self.base.starts_with(old_prefix) {
            return;
        }
        let mut nb: Vec<String> = f[..f.len() - suffix].to_vec();
        nb.extend(self.base[old_prefix.len()..].iter().cloned());
        let mut g = self.alt_bases.lock().unwrap();
        if nb != self.base && !g.contains(&nb) {
            g.push(nb);
        }
    }

    fn fin(&self, e: CErr) -> RError {
        match e {
            CErr::Local(r) => r,
            CErr::U(b) => match *b {
                ureq::Error::Status(401, resp) => {
                    let ch = resp
                        .header("WWW-Authenticate")
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if ch.contains("digest") && !ch.contains("basic") && !ch.contains("bearer") {
                        RError::Auth(
                        "serwer wymaga uwierzytelniania Digest, którego Syncron nie obsługuje (użyj hasła aplikacji z Basic lub tokenu)"
                            .into(),
                    )
                    } else {
                        http::map_err(ureq::Error::Status(401, resp))
                    }
                }
                e => http::map_err(e),
            },
        }
    }

    /// Jedno żądanie z obsługą przekierowań zachowujących metodę (301/302/307/308).
    fn once(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &Body,
    ) -> Result<ureq::Response, CErr> {
        let orig = url.to_string();
        let mut url = orig.clone();
        let mut method = method.to_string();
        let mut no_body = false;
        for _ in 0..6 {
            let mut req = self.agent.request(&method, &url);
            let same = url.starts_with(&self.origin)
                && matches!(url.as_bytes().get(self.origin.len()), None | Some(b'/'));
            if same {
                if let Some(a) = &self.auth {
                    req = req.set("Authorization", a);
                }
            }
            for (k, v) in headers {
                req = req.set(k, v);
            }
            let r = match (body, no_body) {
                (Body::Empty, _) | (_, true) => req.call(),
                (Body::Text(t), _) => req
                    .set("Content-Type", "application/xml; charset=utf-8")
                    .send_string(t),
                (Body::File(path, len, x), _) => {
                    let f = std::fs::File::open(path).map_err(|e| {
                        CErr::Local(RError::Failed(format!("nie można otworzyć pliku: {e}")))
                    })?;
                    req.set("Content-Length", &len.to_string())
                        .send(ThrottledReader::new(f, x))
                }
            };
            let resp = r.map_err(|e| CErr::U(Box::new(e)))?;
            match resp.status() {
                301 | 302 | 307 | 308 => match resp.header("Location") {
                    Some(l) => url = resolve(&url, l),
                    None => return Ok(resp),
                },
                303 => match resp.header("Location") {
                    Some(l) => {
                        url = resolve(&url, l);
                        method = "GET".into();
                        no_body = true;
                    }
                    None => return Ok(resp),
                },
                _ => {
                    if url != orig {
                        self.learn_redirect(&orig, &url);
                    }
                    return Ok(resp);
                }
            }
        }
        Err(CErr::Local(RError::Failed(
            "zbyt wiele przekierowań".into(),
        )))
    }

    /// Żądanie z ponawianiem (429/502/503/504 i błędy sieci), z możliwością anulowania.
    fn call(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: &Body,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<ureq::Response, CErr> {
        let mut delay = self.retry_ms;
        for attempt in 0..4 {
            match self.once(method, url, headers, body) {
                Ok(r) => return Ok(r),
                Err(CErr::U(e)) => {
                    let e = *e;
                    let cancelled = cancel.is_some_and(|c| c.load(Ordering::Relaxed));
                    let transient = matches!(
                        &e,
                        ureq::Error::Status(429 | 502 | 503 | 504, _) | ureq::Error::Transport(_)
                    );
                    if !transient || attempt == 3 || cancelled {
                        return Err(CErr::U(Box::new(e)));
                    }
                    let wait = match &e {
                        ureq::Error::Status(_, r) => r
                            .header("Retry-After")
                            .and_then(|v| v.trim().parse::<u64>().ok())
                            .map(|s| (s * 1000).min(30_000)),
                        _ => None,
                    }
                    .unwrap_or(delay);
                    std::thread::sleep(Duration::from_millis(wait));
                    delay *= 2;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    fn read_xml(resp: ureq::Response) -> RResult<String> {
        let mut buf = Vec::new();
        resp.into_reader()
            .take(MAX_XML)
            .read_to_end(&mut buf)
            .map_err(|e| RError::Network(e.to_string()))?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// PROPFIND; `Ok(None)` = 404.
    fn propfind(&self, rel: &str, depth: &str, body: &str) -> RResult<Option<Vec<RawResp>>> {
        let url = self.url(rel, true);
        match self.call(
            "PROPFIND",
            &url,
            &[("Depth", depth.to_string())],
            &Body::Text(body),
            None,
        ) {
            Ok(r) => Ok(Some(parse_multistatus(&Self::read_xml(r)?)?)),
            Err(e) if matches!(code_of(&e), Some(404 | 410)) => Ok(None),
            Err(e) => Err(self.fin(e)),
        }
    }

    /// Zawartość jednego folderu (bez niego samego); `None` = folder nie istnieje.
    fn list_one(&self, dir: &str) -> RResult<Option<Vec<Item>>> {
        let Some(resps) = self.propfind(dir, "1", PROPFIND_BODY)? else {
            return Ok(None);
        };
        let mut found_self = false;
        let mut items = Vec::new();
        for r in &resps {
            let Some(rel) = self.href_to_rel(&r.href) else {
                continue;
            };
            if rel == dir {
                found_self = true;
                continue;
            }
            // dziecko musi leżeć bezpośrednio pod `dir`
            let parent = rel.rsplit_once('/').map(|x| x.0).unwrap_or("");
            if parent != dir || r.bad_status {
                continue;
            }
            items.push(Item {
                rel,
                is_dir: r.is_dir,
                size: r.size.unwrap_or(0),
                mtime: r.mtime.unwrap_or(0),
                hash: r.hash.clone(),
            });
        }
        if found_self {
            let mut k = self.known_dirs.lock().unwrap();
            k.insert(self.dir_key(dir));
            for i in items.iter().filter(|i| i.is_dir) {
                k.insert(self.dir_key(&i.rel));
            }
        }
        if !found_self && !resps.is_empty() {
            return Err(RError::Failed(
                "odpowiedź serwera nie zawiera listowanego folderu (niezgodne ścieżki href) — przerwano dla bezpieczeństwa".into(),
            ));
        }
        Ok(Some(items))
    }

    fn par_list(&self, dirs: &[String]) -> Vec<ListResult> {
        let n = LIST_THREADS.min(dirs.len());
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
                    let r = self.list_one(&dirs[i]);
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

    fn stat_raw(&self, path: &str) -> RResult<Option<RawResp>> {
        let rel = clean_path(path)?;
        let Some(resps) = self.propfind(&rel, "0", PROPFIND_BODY)? else {
            return Ok(None);
        };
        Ok(resps
            .into_iter()
            .find(|r| self.href_to_rel(&r.href).as_deref() == Some(rel.as_str()) && !r.bad_status))
    }

    fn ensure_dirs(&self, dir: &str, force: bool) -> RResult<()> {
        // korzeń + `dir`
        let mut full: Vec<String> = Vec::new();
        let mut targets: Vec<String> = Vec::new();
        for c in self
            .root
            .iter()
            .map(|s| s.as_str())
            .chain(dir.split('/').filter(|c| !c.is_empty()))
        {
            full.push(c.to_string());
            targets.push(full.join("/"));
        }
        // ścieżki w `known_dirs` są względem adresu konta; `url(rel)` dokłada korzeń, więc tworzymy przez surowy adres
        for t in targets {
            if !force && self.known_dirs.lock().unwrap().contains(&t) {
                continue;
            }
            let mut u = format!("{}/", self.origin);
            let mut parts = self.base.clone();
            parts.extend(comps(&t));
            u.push_str(&http::url_path(&parts.join("/")));
            u.push('/');
            match self.call("MKCOL", &u, &[], &Body::Empty, None) {
                Ok(_) => {}
                Err(e) if matches!(code_of(&e), Some(405 | 301 | 302)) => {}
                Err(e) => return Err(self.fin(e)),
            }
            self.known_dirs.lock().unwrap().insert(t);
        }
        Ok(())
    }

    /// Klucz folderu w pamięci podręcznej: ścieżka od adresu konta (korzeń + `rel`).
    fn dir_key(&self, rel: &str) -> String {
        self.root
            .iter()
            .map(|s| s.as_str())
            .chain(rel.split('/').filter(|c| !c.is_empty()))
            .collect::<Vec<_>>()
            .join("/")
    }

    fn dir_known(&self, rel: &str) -> bool {
        let k = self.dir_key(rel);
        k.is_empty() || self.known_dirs.lock().unwrap().contains(&k)
    }

    fn parent_of(rel: &str) -> &str {
        rel.rsplit_once('/').map(|x| x.0).unwrap_or("")
    }
}

impl Backend for WebDav {
    fn kind(&self) -> &'static str {
        "webdav"
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
            for r in results {
                // `None` (404) = brak folderu: pusta lista (albo folder usunięty w trakcie listowania)
                for it in r?.unwrap_or_default() {
                    if it.is_dir {
                        if seen.insert(it.rel.clone()) {
                            next.push(it.rel);
                        }
                    } else {
                        files.push(REntry {
                            path: it.rel,
                            meta: RMeta {
                                size: it.size,
                                mtime: it.mtime,
                                hash: it.hash,
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
        let items = self
            .list_one(&d)?
            .ok_or_else(|| RError::NotFound(format!("folder „{d}” nie istnieje")))?;
        Ok(items
            .into_iter()
            .map(|i| DirItem {
                name: i.rel.rsplit('/').next().unwrap_or("").to_string(),
                is_dir: i.is_dir,
            })
            .collect())
    }

    fn stat(&self, path: &str) -> RResult<Option<REntry>> {
        let rel = clean_path(path)?;
        Ok(self.stat_raw(&rel)?.filter(|r| !r.is_dir).map(|r| REntry {
            path: rel,
            meta: RMeta {
                size: r.size.unwrap_or(0),
                mtime: r.mtime.unwrap_or(0),
                hash: r.hash,
            },
        }))
    }

    fn download(&self, path: &str, dest: &Path, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        let url = self.url(&rel, false);
        let resp = self
            .call("GET", &url, &[], &Body::Empty, x.cancel)
            .map_err(|e| self.fin(e))?;
        let expected: Option<u64> = resp
            .header("Content-Length")
            .and_then(|v| v.trim().parse().ok());
        let got = http::copy_to_file(resp.into_reader(), dest, x).inspect_err(|_| {
            let _ = std::fs::remove_file(dest);
        })?;
        if let Some(e) = expected {
            if e != got {
                let _ = std::fs::remove_file(dest);
                return Err(RError::Network(format!(
                    "pobieranie przerwane: odebrano {got} z {e} bajtów"
                )));
            }
        }
        Ok(())
    }

    fn upload(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka docelowa".into()));
        }
        let len = std::fs::metadata(src)
            .map_err(|e| RError::Failed(format!("nie można odczytać pliku: {e}")))?
            .len();
        let url = self.url(&rel, false);
        let mut headers: Vec<(&str, String)> = Vec::new();
        if mtime > 0
            && matches!(
                self.vendor,
                Vendor::Nextcloud | Vendor::Owncloud | Vendor::Other
            )
        {
            headers.push(("X-OC-Mtime", mtime.to_string()));
        }
        let parent = Self::parent_of(&rel);
        if !self.dir_known(parent) {
            self.ensure_dirs(parent, false)?;
        }
        let body = Body::File(src, len, x);
        let resp = match self.call("PUT", &url, &headers, &body, x.cancel) {
            Ok(r) => r,
            Err(e) if code_of(&e) == Some(409) => {
                // brak folderu nadrzędnego
                self.ensure_dirs(Self::parent_of(&rel), true)?;
                self.call("PUT", &url, &headers, &body, x.cancel)
                    .map_err(|e| self.fin(e))?
            }
            Err(e) => return Err(self.fin(e)),
        };
        let accepted = resp
            .header("X-OC-Mtime")
            .is_some_and(|v| v.eq_ignore_ascii_case("accepted"));
        if mtime > 0 && !accepted && !matches!(self.vendor, Vendor::Sharepoint | Vendor::Bitrix24) {
            // najlepsze starania: większość serwerów (poza Nextcloud/ownCloud) odrzuci zmianę tej właściwości
            let body = format!(
                r#"<?xml version="1.0" encoding="utf-8"?><d:propertyupdate xmlns:d="DAV:"><d:set><d:prop><d:getlastmodified>{}</d:getlastmodified></d:prop></d:set></d:propertyupdate>"#,
                rfc1123(mtime)
            );
            let _ = self.call("PROPPATCH", &url, &[], &Body::Text(&body), None);
        }
        // weryfikacja rozmiaru po wysłaniu
        if let Some(r) = self.stat_raw(&rel)? {
            if let Some(sz) = r.size {
                if sz != len {
                    return Err(RError::Failed(format!(
                        "rozmiar na serwerze ({sz}) różni się od wysłanego ({len})"
                    )));
                }
            }
        } else {
            return Err(RError::Failed(
                "po wysłaniu plik nie jest widoczny na serwerze".into(),
            ));
        }
        Ok(())
    }

    fn delete(&self, path: &str) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("odmowa usunięcia korzenia".into()));
        }
        match self.call("DELETE", &self.url(&rel, false), &[], &Body::Empty, None) {
            Ok(_) => Ok(()),
            Err(e) if matches!(code_of(&e), Some(404 | 410)) => Ok(()),
            Err(e) => Err(self.fin(e)),
        }
    }

    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        let (f, t) = (clean_path(from)?, clean_path(to)?);
        let hdrs = [
            ("Destination", self.url(&t, false)),
            ("Overwrite", "T".to_string()),
        ];
        let src = self.url(&f, false);
        match self.call("MOVE", &src, &hdrs, &Body::Empty, None) {
            Ok(_) => Ok(()),
            Err(e) if code_of(&e) == Some(409) => {
                self.ensure_dirs(Self::parent_of(&t), true)?;
                self.call("MOVE", &src, &hdrs, &Body::Empty, None)
                    .map(|_| ())
                    .map_err(|e| self.fin(e))
            }
            Err(e) => Err(self.fin(e)),
        }
    }

    fn mkdirs(&self, dir: &str) -> RResult<()> {
        let d = clean_path(dir)?;
        self.ensure_dirs(&d, false)
    }

    fn rmdir_if_empty(&self, dir: &str) -> RResult<()> {
        let d = clean_path(dir)?;
        if d.is_empty() {
            return Ok(());
        }
        match self.list_one(&d) {
            Ok(Some(items)) if items.is_empty() => {
                match self.call("DELETE", &self.url(&d, true), &[], &Body::Empty, None) {
                    Ok(_) => {
                        self.known_dirs.lock().unwrap().clear();
                        Ok(())
                    }
                    Err(e) if matches!(code_of(&e), Some(404 | 410)) => Ok(()),
                    Err(e) => Err(self.fin(e)),
                }
            }
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn about(&self) -> RResult<Option<Quota>> {
        let Some(resps) = self.propfind("", "0", QUOTA_BODY)? else {
            return Ok(None);
        };
        let Some(r) = resps.first() else {
            return Ok(None);
        };
        let used = r.used.filter(|v| *v >= 0).map(|v| v as u64);
        let free = r.avail.filter(|v| *v >= 0).map(|v| v as u64);
        if used.is_none() && free.is_none() {
            return Ok(None);
        }
        Ok(Some(Quota {
            total: used.zip(free).map(|(u, f)| u + f),
            used,
            free,
        }))
    }

    fn ping(&self) -> RResult<String> {
        match self.propfind("", "0", PROPFIND_BODY)? {
            Some(_) => Ok("WebDAV: OK".to_string()),
            None => Err(RError::NotFound(
                "adres lub folder główny WebDAV nie istnieje".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixes_and_props() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
<d:response><d:href>/dav/a%20b/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/dav/a%20b/z&amp;.txt</d:href><d:propstat><d:prop><d:getcontentlength>12</d:getcontentlength><d:getlastmodified>Wed, 12 Oct 2022 10:20:30 GMT</d:getlastmodified><oc:checksums><oc:checksum>SHA1:0123456789012345678901234567890123456789 MD5:00112233445566778899aabbccddeeff</oc:checksum></oc:checksums></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
<d:propstat><d:prop><x:foo xmlns:x="x"/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>
</d:multistatus>"#;
        let r = parse_multistatus(xml).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].is_dir);
        assert_eq!(r[1].size, Some(12));
        assert_eq!(r[1].mtime, Some(1665570030));
        assert_eq!(r[1].hash.as_ref().unwrap().algo, "sha1");
        let bare = r#"<multistatus xmlns="DAV:"><response><href>/x</href><propstat><prop><getcontentlength>5</getcontentlength></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        assert_eq!(parse_multistatus(bare).unwrap()[0].size, Some(5));
        assert!(parse_multistatus("<a><b></a>").is_err());
    }

    #[test]
    fn href_mapping_rejects_outside() {
        let mut p = Params::default();
        p.public
            .insert("url".into(), "https://h.example/dav/files/u/".into());
        let w = WebDav::new(&p, "Praca").unwrap();
        assert_eq!(
            w.href_to_rel("https://h.example/dav/files/u/Praca/Za%C5%BC%C3%B3%C5%82%C4%87/a.txt")
                .as_deref(),
            Some("Zażółć/a.txt")
        );
        assert_eq!(w.href_to_rel("/dav/files/u/Praca/").as_deref(), Some(""));
        assert_eq!(w.href_to_rel("/dav/files/u/Praca/../Inny/x"), None);
        assert_eq!(w.href_to_rel("/dav/files/u/Praca/%2e%2e/x"), None);
        assert_eq!(w.href_to_rel("/dav/files/u/Inny/x"), None);
        assert_eq!(
            w.url("a b/ł.txt", false),
            "https://h.example/dav/files/u/Praca/a%20b/%C5%82.txt"
        );
    }

    #[test]
    fn b64_works() {
        assert_eq!(b64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(b64(b"a"), "YQ==");
        assert_eq!(b64(b"ab"), "YWI=");
    }
}
