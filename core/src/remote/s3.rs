//! Backend S3 (AWS, Wasabi, MinIO, Cloudflare R2, Backblaze B2, DigitalOcean Spaces i inne zgodne z API S3).
//! Własny podpis AWS Signature V4 (nagłówkowy) na `ring`; HTTP przez `ureq` + rustls — bez OpenSSL i bez programów zewnętrznych.
//!
//! Zasady:
//! - „foldery” to prefiksy kluczy (`mkdirs`/`rmdir_if_empty` nic nie robią, nie tworzymy znaczników `klucz/`);
//! - czas modyfikacji: nagłówek `x-amz-meta-mtime` (ustawiany przy wysyłaniu), a gdy go brak — `LastModified`.
//!   Lista plików zwraca `LastModified` (jedno żądanie na stronę); `stat` zwraca dokładny `x-amz-meta-mtime`.
//!   Pole `precise_mtime=1` każe liście dopytać każdy obiekt (HEAD), co jest wolniejsze;
//! - hash: `ETag` jako `md5`, gdy to zwykły (nie wieloczęściowy) obiekt bez szyfrowania KMS (32 znaki hex);
//! - wysyłanie powyżej 64 MB: multipart (części po 16 MB, ponawiane), przy błędzie `AbortMultipartUpload`;
//! - rename = server-side CopyObject + DELETE (do 5 GB na obiekt — limit API CopyObject).
use super::http::{self, ThrottledReader};
use super::*;
use ring::{digest, hmac};
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Od tego rozmiaru wysyłamy multipart.
pub const MULTIPART_THRESHOLD: u64 = 64 * 1024 * 1024;
/// Rozmiar jednej części multipart (zwiększany automatycznie dla gigantycznych plików: maks. 10 000 części).
pub const PART_SIZE: u64 = 16 * 1024 * 1024;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

// ---------------------------------------------------------------------------------------------------------------
// AWS Signature V4

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, data).as_ref())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let k = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&k, data).as_ref().to_vec()
}

/// Kodowanie URI według SigV4: wszystko poza `A-Za-z0-9-_.~` jest %XX (wielkie litery); `/` zostaje tylko w ścieżce.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Dane do podpisania jednego żądania.
pub struct SigInput<'a> {
    pub method: &'a str,
    /// Już zakodowana ścieżka (dla S3: kodowana jeden raz).
    pub canonical_uri: &'a str,
    /// Parametry zapytania w postaci niezakodowanej.
    pub query: &'a [(String, String)],
    /// Nagłówki do podpisania (nazwy dowolną wielkością liter, `host` i `x-amz-date` obowiązkowe).
    pub headers: &'a [(String, String)],
    /// Skrót SHA-256 ciała (hex) albo `UNSIGNED-PAYLOAD`.
    pub payload_hash: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    /// `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
}

pub struct Signed {
    pub authorization: String,
    pub canonical_request: String,
    pub string_to_sign: String,
    pub signature: String,
    pub signed_headers: String,
}

pub fn canonical_query(query: &[(String, String)]) -> String {
    let mut q: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    q.sort();
    q.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn sign(i: &SigInput) -> Signed {
    let mut hs: Vec<(String, String)> = i
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    hs.sort();
    let ch: String = hs.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = hs
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        i.method,
        i.canonical_uri,
        canonical_query(i.query),
        ch,
        signed_headers,
        i.payload_hash
    );
    let date = &i.amz_date[..8];
    let scope = format!("{date}/{}/{}/aws4_request", i.region, i.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        i.amz_date,
        scope,
        sha256_hex(canonical_request.as_bytes())
    );
    let k_date = hmac_sha256(format!("AWS4{}", i.secret_key).as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, i.region.as_bytes());
    let k_service = hmac_sha256(&k_region, i.service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        i.access_key
    );
    Signed {
        authorization,
        canonical_request,
        string_to_sign,
        signature,
        signed_headers,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// XML (odporny parser ścieżek)

enum Ev<'a> {
    Start(&'a str),
    Leaf(&'a str, String),
}

/// Przechodzi po dokumencie XML; `on_start(ścieżka)` przy otwarciu elementu, `on_leaf(ścieżka, tekst)` przy jego zamknięciu.
/// Błąd = dokument ucięty lub niepoprawny (np. zerwane połączenie w trakcie odpowiedzi).
fn walk_xml(xml: &str, mut on: impl FnMut(Ev)) -> RResult<()> {
    use quick_xml::events::Event;
    let bad = |e: String| RError::Failed(format!("niepoprawna odpowiedź XML serwera S3: {e}"));
    let mut rd = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut saw_root = false;
    loop {
        match rd.read_event().map_err(|e| bad(e.to_string()))? {
            Event::Start(e) => {
                let name = e.local_name().into_inner().to_string();
                stack.push(name);
                texts.push(String::new());
                saw_root = true;
                on(Ev::Start(&stack.join("/")));
            }
            Event::Empty(e) => {
                let name = e.local_name().into_inner().to_string();
                stack.push(name);
                saw_root = true;
                let path = stack.join("/");
                on(Ev::Start(&path));
                on(Ev::Leaf(&path, String::new()));
                stack.pop();
            }
            Event::Text(t) => {
                if let Some(s) = texts.last_mut() {
                    s.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(s) = texts.last_mut() {
                    s.push_str(&t.into_inner());
                }
            }
            Event::GeneralRef(r) => {
                let add = match r.resolve_char_ref().map_err(|e| bad(e.to_string()))? {
                    Some(c) => c.to_string(),
                    None => match &*r {
                        "amp" => "&".into(),
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        other => format!("&{other};"),
                    },
                };
                if let Some(s) = texts.last_mut() {
                    s.push_str(&add);
                }
            }
            Event::End(_) => {
                let path = stack.join("/");
                let txt = texts.pop().unwrap_or_default();
                stack.pop();
                on(Ev::Leaf(&path, txt));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_root || !stack.is_empty() {
        return Err(bad("dokument ucięty".into()));
    }
    Ok(())
}

/// Odwrotność kodowania `encoding-type=url` S3 (spacja jako `+`, reszta %XX).
fn s3_url_decode(s: &str) -> String {
    http::url_decode(&s.replace('+', " "))
}

#[derive(Default)]
struct ListPage {
    objs: Vec<Obj>,
    prefixes: Vec<String>,
    truncated: bool,
    next: Option<String>,
}

#[derive(Default, Clone)]
struct Obj {
    key: String,
    size: u64,
    lm: String,
    etag: String,
}

fn parse_list(xml: &str) -> RResult<ListPage> {
    let mut page = ListPage::default();
    let mut encoded = false;
    let mut root_ok = false;
    walk_xml(xml, |ev| match ev {
        Ev::Start(p) => {
            if p == "ListBucketResult" {
                root_ok = true;
            }
            if p == "ListBucketResult/Contents" {
                page.objs.push(Obj::default());
            }
        }
        Ev::Leaf(p, t) => match p {
            "ListBucketResult/EncodingType" => encoded = t.trim() == "url",
            "ListBucketResult/IsTruncated" => page.truncated = t.trim() == "true",
            "ListBucketResult/NextContinuationToken" => page.next = Some(t),
            "ListBucketResult/Contents/Key" => {
                if let Some(o) = page.objs.last_mut() {
                    o.key = t;
                }
            }
            "ListBucketResult/Contents/Size" => {
                if let Some(o) = page.objs.last_mut() {
                    o.size = t.trim().parse().unwrap_or(0);
                }
            }
            "ListBucketResult/Contents/LastModified" => {
                if let Some(o) = page.objs.last_mut() {
                    o.lm = t;
                }
            }
            "ListBucketResult/Contents/ETag" => {
                if let Some(o) = page.objs.last_mut() {
                    o.etag = t;
                }
            }
            "ListBucketResult/CommonPrefixes/Prefix" => page.prefixes.push(t),
            _ => {}
        },
    })?;
    if !root_ok {
        return Err(RError::Failed(
            "odpowiedź serwera nie jest listą obiektów S3 (zły endpoint?)".into(),
        ));
    }
    if encoded {
        for o in &mut page.objs {
            o.key = s3_url_decode(&o.key);
        }
        for p in &mut page.prefixes {
            *p = s3_url_decode(p);
        }
    }
    Ok(page)
}

/// Kod i komunikat z ciała błędu S3.
fn parse_error(body: &str) -> (String, String) {
    let (mut code, mut msg) = (String::new(), String::new());
    let _ = walk_xml(body, |ev| {
        if let Ev::Leaf(p, t) = ev {
            match p {
                "Error/Code" => code = t,
                "Error/Message" => msg = t,
                _ => {}
            }
        }
    });
    (code, msg)
}

fn parse_iso(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

fn parse_http_date(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc2822(s.trim())
        .map(|d| d.timestamp())
        .unwrap_or(0)
}

fn etag_hash(etag: &str) -> Option<Hash> {
    let e = etag.trim().trim_matches('"');
    if e.len() == 32 && e.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(Hash {
            algo: "md5",
            hex: e.to_ascii_lowercase(),
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Backend

struct Creds {
    access_key: String,
    secret_key: String,
    token: Option<String>,
}

pub struct S3Backend {
    agent: ureq::Agent,
    scheme: String,
    /// Host (z portem, jeśli nietypowy) tak, jak trafi do żądania i do podpisu.
    host: String,
    /// Prefiks ścieżki przed kluczem: `""` (virtual-hosted) lub `/bucket` (path-style); może zawierać ścieżkę z endpointu.
    base_path: String,
    bucket: String,
    region: String,
    creds: Creds,
    root: String,
    precise_mtime: bool,
    /// Bazowe opóźnienie ponawiania w ms (pole ukryte `retry_base_ms`; domyślnie 1000).
    retry_base_ms: u64,
}

enum Body<'a> {
    None,
    Bytes(&'a [u8]),
    File {
        path: &'a Path,
        offset: u64,
        len: u64,
        x: &'a Xfer<'a>,
    },
}

struct Fail {
    err: RError,
    transient: bool,
    retry_after: Option<u64>,
}

fn default_port(scheme: &str) -> &'static str {
    if scheme == "http" {
        "80"
    } else {
        "443"
    }
}

/// Dzieli endpoint na (schemat, host[:port] bez domyślnego portu, ścieżka bez końcowego `/`).
fn split_endpoint(ep: &str) -> RResult<(String, String, String)> {
    let ep = ep.trim();
    let (scheme, rest) = match ep.split_once("://") {
        Some((s, r)) if s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https") => {
            (s.to_ascii_lowercase(), r)
        }
        Some(_) => {
            return Err(RError::Failed(
                "endpoint S3 musi zaczynać się od http:// lub https://".into(),
            ))
        }
        None => ("https".to_string(), ep),
    };
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
        None => (rest, ""),
    };
    if auth.is_empty() || auth.contains('@') {
        return Err(RError::Failed("niepoprawny endpoint S3".into()));
    }
    let auth = match auth.rsplit_once(':') {
        Some((h, p)) if p == default_port(&scheme) => h.to_string(),
        _ => auth.to_string(),
    };
    Ok((scheme, auth, path.to_string()))
}

#[derive(Debug)]
struct NoVerify(std::sync::Arc<rustls::crypto::CryptoProvider>);

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

fn build_agent(no_check_certificate: bool) -> RResult<ureq::Agent> {
    let mut b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(90))
        .timeout_write(Duration::from_secs(90))
        .redirects(0)
        .user_agent(concat!("Syncron/", env!("CARGO_PKG_VERSION")));
    if no_check_certificate {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| RError::Failed(e.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(NoVerify(provider)))
            .with_no_client_auth();
        b = b.tls_config(std::sync::Arc::new(cfg));
    }
    Ok(b.build())
}

impl S3Backend {
    pub fn new(p: &Params, root: &str) -> RResult<S3Backend> {
        let provider = p.get("provider").unwrap_or("AWS").to_ascii_lowercase();
        let is_aws = provider == "aws" || provider.contains("amazon");
        let bucket = p.need("bucket")?.trim().trim_matches('/').to_string();
        if bucket.is_empty() || bucket.contains('/') {
            return Err(RError::Failed("niepoprawna nazwa bucketa".into()));
        }
        let mut region = p.get("region").unwrap_or("").trim().to_string();
        let mut endpoint = p.get("endpoint").unwrap_or("").trim().to_string();
        if region.is_empty() && !endpoint.is_empty() {
            // s3.eu-central-1.amazonaws.com / s3-eu-west-1.amazonaws.com
            let h = endpoint.split("://").last().unwrap_or("");
            if let Some(r) = h
                .strip_prefix("s3.")
                .and_then(|x| x.strip_suffix(".amazonaws.com"))
            {
                region = r.to_string();
            }
        }
        if region.is_empty() {
            region = if provider.contains("r2") || provider.contains("cloudflare") {
                "auto".into()
            } else {
                "us-east-1".into()
            };
        }
        if endpoint.is_empty() {
            endpoint = if is_aws {
                format!("https://s3.{region}.amazonaws.com")
            } else if provider.contains("wasabi") {
                format!("https://s3.{region}.wasabisys.com")
            } else if provider.contains("digital") {
                format!("https://{region}.digitaloceanspaces.com")
            } else if provider.contains("backblaze") || provider == "b2" {
                format!("https://s3.{region}.backblazeb2.com")
            } else {
                return Err(RError::Failed(
                    "brak wymaganego pola „endpoint” (adres serwera S3)".into(),
                ));
            };
        }
        let (scheme, auth, ep_path) = split_endpoint(&endpoint)?;
        let aws_host = auth.ends_with(".amazonaws.com");
        let dns_ok = !bucket.contains('.') && bucket.len() >= 3;
        let path_style = match p.get("path_style").map(|s| s.to_ascii_lowercase()) {
            Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "tak") => true,
            Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "nie") => false,
            _ => !((is_aws || aws_host) && dns_ok),
        };
        let (host, base_path) = if path_style {
            (auth, format!("{ep_path}/{bucket}"))
        } else {
            (format!("{bucket}.{auth}"), ep_path)
        };
        let access_key = p
            .get("access_key_id")
            .or_else(|| p.secret("access_key_id"))
            .ok_or_else(|| RError::Failed("brak wymaganego pola „access_key_id”".into()))?
            .trim()
            .to_string();
        let secret_key = p
            .secret("secret_access_key")
            .ok_or_else(|| RError::Failed("brak wymaganego pola „secret_access_key”".into()))?
            .trim()
            .to_string();
        Ok(S3Backend {
            agent: build_agent(p.flag("no_check_certificate"))?,
            scheme,
            host,
            base_path,
            bucket,
            region,
            creds: Creds {
                access_key,
                secret_key,
                token: p.secret("session_token").map(|s| s.trim().to_string()),
            },
            root: root.trim_matches('/').to_string(),
            precise_mtime: p.flag("precise_mtime"),
            retry_base_ms: p.num("retry_base_ms", 1000).max(1),
        })
    }

    /// Adres żądania dla klucza: (url, kanoniczna ścieżka). Pusty klucz = operacja na buckecie.
    pub fn target(&self, key: &str) -> (String, String) {
        let uri = if key.is_empty() {
            format!("{}/", self.base_path)
        } else {
            format!("{}/{}", self.base_path, uri_encode(key, true))
        };
        (format!("{}://{}{}", self.scheme, self.host, uri), uri)
    }

    fn full(&self, rel: &str) -> String {
        join_path(&self.root, rel)
    }

    fn root_prefix(&self) -> String {
        if self.root.is_empty() {
            String::new()
        } else {
            format!("{}/", self.root)
        }
    }

    fn map_fail(&self, status: u16, code: &str, msg: &str) -> RError {
        let detail = if msg.is_empty() {
            format!("HTTP {status} {code}")
        } else {
            format!("HTTP {status} {code}: {msg}")
        };
        match (status, code) {
            (_, "SignatureDoesNotMatch") => RError::Auth(format!(
                "{detail}. Sprawdź Secret Access Key, region („{}”) i endpoint (zły region lub klucz powodują ten błąd)",
                self.region
            )),
            (_, "InvalidAccessKeyId") => RError::Auth(format!(
                "{detail}. Nieprawidłowy Access Key ID (albo klucz z innego dostawcy/regionu)"
            )),
            (_, "RequestTimeTooSkewed") => RError::Auth(format!(
                "{detail}. Zegar komputera różni się od serwera o ponad 15 minut"
            )),
            (_, "AuthorizationHeaderMalformed" | "PermanentRedirect" | "AuthorizationQueryParametersError") => {
                RError::Auth(format!("{detail}. Najpewniej zły region lub endpoint dla tego bucketa"))
            }
            (_, "ExpiredToken" | "InvalidToken") => RError::Auth(format!("{detail}. Token sesji wygasł lub jest błędny")),
            (301 | 307 | 308, _) => RError::Auth(format!(
                "HTTP {status}: serwer przekierował żądanie — zły region lub endpoint (region: „{}”)",
                self.region
            )),
            (401 | 403, _) => RError::Auth(format!(
                "{detail}. Sprawdź klucze, region („{}”) i uprawnienia do bucketa „{}”",
                self.region, self.bucket
            )),
            (_, "EntityTooLarge") => RError::Quota(format!("{detail}. Obiekt przekracza limit rozmiaru serwera")),
            (_, "QuotaExceeded" | "StorageLimitExceeded" | "MaxMessageLengthExceeded") | (507, _) => {
                RError::Quota(detail)
            }
            (_, "NoSuchBucket") => RError::NotFound(format!("{detail}. Bucket „{}” nie istnieje", self.bucket)),
            (404 | 410, _) | (_, "NoSuchKey" | "NoSuchUpload") => RError::NotFound(detail),
            (429 | 500 | 502 | 503 | 504, _) | (_, "SlowDown" | "RequestTimeout" | "InternalError") => {
                RError::Busy(detail)
            }
            _ => RError::Failed(detail),
        }
    }

    /// Jedno podpisane żądanie bez ponawiania.
    fn once(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, String)],
        extra: &[(&str, String)],
        payload_hash: &str,
        body: &Body,
    ) -> Result<ureq::Response, Fail> {
        let (url, uri) = self.target(key);
        let amz_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let q: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let mut headers: Vec<(String, String)> = vec![
            ("host".into(), self.host.clone()),
            ("x-amz-content-sha256".into(), payload_hash.to_string()),
            ("x-amz-date".into(), amz_date.clone()),
        ];
        if let Some(t) = &self.creds.token {
            headers.push(("x-amz-security-token".into(), t.clone()));
        }
        for (k, v) in extra {
            headers.push((k.to_ascii_lowercase(), v.clone()));
        }
        let signed = sign(&SigInput {
            method,
            canonical_uri: &uri,
            query: &q,
            headers: &headers,
            payload_hash,
            region: &self.region,
            service: "s3",
            access_key: &self.creds.access_key,
            secret_key: &self.creds.secret_key,
            amz_date: &amz_date,
        });
        let full_url = if q.is_empty() {
            url
        } else {
            format!("{url}?{}", canonical_query(&q))
        };
        let mut req = self.agent.request(method, &full_url);
        for (k, v) in &headers {
            if k != "host" {
                req = req.set(k, v);
            }
        }
        req = req.set("Authorization", &signed.authorization);
        let res = match body {
            Body::None => req.call(),
            Body::Bytes(b) => req.send_bytes(b),
            Body::File {
                path,
                offset,
                len,
                x,
            } => {
                let open = || -> std::io::Result<std::fs::File> {
                    let mut f = std::fs::File::open(path)?;
                    f.seek(SeekFrom::Start(*offset))?;
                    Ok(f)
                };
                match open() {
                    Ok(f) => req
                        .set("Content-Length", &len.to_string())
                        .send(ThrottledReader::new(f.take(*len), x)),
                    Err(e) => {
                        return Err(Fail {
                            err: RError::Failed(format!("odczyt pliku lokalnego: {e}")),
                            transient: false,
                            retry_after: None,
                        })
                    }
                }
            }
        };
        match res {
            Ok(r) if r.status() >= 300 => {
                let status = r.status();
                let b = r.into_string().unwrap_or_default();
                let (code, msg) = parse_error(&b);
                Err(Fail {
                    err: self.map_fail(status, &code, &msg),
                    transient: false,
                    retry_after: None,
                })
            }
            Ok(r) => Ok(r),
            Err(ureq::Error::Status(status, resp)) => {
                let retry_after = resp
                    .header("Retry-After")
                    .and_then(|v| v.trim().parse().ok());
                let b = resp.into_string().unwrap_or_default();
                let (code, msg) = parse_error(&b);
                let transient =
                    matches!(status, 429 | 500 | 502 | 503 | 504) || code == "RequestTimeout";
                Err(Fail {
                    err: self.map_fail(status, &code, &msg),
                    transient,
                    retry_after,
                })
            }
            Err(ureq::Error::Transport(t)) => Err(Fail {
                err: RError::Network(t.to_string()),
                transient: true,
                retry_after: None,
            }),
        }
    }

    /// Żądanie z ponawianiem (do 4 prób) przy błędach chwilowych: 429/5xx (SlowDown) i błędach sieci.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, String)],
        extra: &[(&str, String)],
        payload_hash: &str,
        body: Body,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> RResult<ureq::Response> {
        let mut delay = self.retry_base_ms;
        for attempt in 0..4 {
            match self.once(method, key, query, extra, payload_hash, &body) {
                Ok(r) => return Ok(r),
                Err(f) => {
                    if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Err(RError::Failed("przerwano".into()));
                    }
                    if !f.transient || attempt == 3 {
                        return Err(f.err);
                    }
                    let wait = f.retry_after.map(|s| s * 1000).unwrap_or(delay).min(30_000);
                    std::thread::sleep(Duration::from_millis(wait));
                    delay *= 2;
                }
            }
        }
        unreachable!()
    }

    fn simple(&self, method: &str, key: &str, query: &[(&str, String)]) -> RResult<ureq::Response> {
        self.call(method, key, query, &[], EMPTY_SHA256, Body::None, None)
    }

    fn list_page(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
        token: Option<&str>,
        max: Option<u32>,
    ) -> RResult<ListPage> {
        let mut q: Vec<(&str, String)> = vec![
            ("list-type", "2".into()),
            ("encoding-type", "url".into()),
            ("prefix", prefix.to_string()),
        ];
        if let Some(d) = delimiter {
            q.push(("delimiter", d.to_string()));
        }
        if let Some(t) = token {
            q.push(("continuation-token", t.to_string()));
        }
        if let Some(m) = max {
            q.push(("max-keys", m.to_string()));
        }
        self.retry_body(None, || {
            let r = self.simple("GET", "", &q)?;
            let body = r
                .into_string()
                .map_err(|e| RError::Network(format!("przerwana odpowiedź listy: {e}")))?;
            parse_list(&body)
        })
    }

    /// Ponawia całą operację (żądanie + odczyt ciała) przy zerwaniu połączenia w trakcie odpowiedzi.
    fn retry_body<T>(
        &self,
        cancel: Option<&std::sync::atomic::AtomicBool>,
        mut f: impl FnMut() -> RResult<T>,
    ) -> RResult<T> {
        let mut delay = self.retry_base_ms;
        for attempt in 0..4 {
            match f() {
                Err(RError::Network(_))
                    if attempt < 3 && !cancel.is_some_and(|c| c.load(Ordering::Relaxed)) =>
                {
                    std::thread::sleep(Duration::from_millis(delay.min(30_000)));
                    delay *= 2;
                }
                other => return other,
            }
        }
        unreachable!()
    }

    /// Wszystkie obiekty pod prefiksem (z paginacją); zerwanie = `Err`.
    fn list_all(&self, prefix: &str, delimiter: Option<&str>) -> RResult<(Vec<Obj>, Vec<String>)> {
        let (mut objs, mut prefixes) = (Vec::new(), Vec::new());
        let mut token: Option<String> = None;
        loop {
            let page = self.list_page(prefix, delimiter, token.as_deref(), None)?;
            objs.extend(page.objs);
            prefixes.extend(page.prefixes);
            if !page.truncated {
                break;
            }
            match page.next {
                Some(n) if token.as_deref() != Some(n.as_str()) && !n.is_empty() => token = Some(n),
                _ => {
                    return Err(RError::Failed(
                        "serwer S3 zgłosił dalszą część listy bez poprawnego tokenu".into(),
                    ))
                }
            }
        }
        Ok((objs, prefixes))
    }

    fn head_meta(&self, key: &str) -> RResult<Option<RMeta>> {
        let r = match self.simple("HEAD", key, &[]) {
            Ok(r) => r,
            Err(RError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let size = r
            .header("Content-Length")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let mtime = r
            .header("x-amz-meta-mtime")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .filter(|t| *t > 0)
            .unwrap_or_else(|| r.header("Last-Modified").map(parse_http_date).unwrap_or(0));
        Ok(Some(RMeta {
            size,
            mtime,
            hash: r.header("ETag").and_then(etag_hash),
        }))
    }

    fn part_size(size: u64) -> u64 {
        const MIB: u64 = 1024 * 1024;
        let need = size.div_ceil(9000).div_ceil(MIB) * MIB;
        need.max(PART_SIZE)
    }

    fn upload_multipart(
        &self,
        src: &Path,
        key: &str,
        size: u64,
        meta: &[(&str, String)],
        x: &Xfer,
    ) -> RResult<()> {
        let r = self.call(
            "POST",
            key,
            &[("uploads", String::new())],
            meta,
            EMPTY_SHA256,
            Body::None,
            x.cancel,
        )?;
        let body = r
            .into_string()
            .map_err(|e| RError::Network(e.to_string()))?;
        let mut upload_id = String::new();
        walk_xml(&body, |ev| {
            if let Ev::Leaf("InitiateMultipartUploadResult/UploadId", t) = ev {
                upload_id = t;
            }
        })?;
        if upload_id.is_empty() {
            return Err(RError::Failed("serwer nie zwrócił UploadId".into()));
        }
        let res = self.upload_parts(src, key, size, &upload_id, x);
        if res.is_err() {
            // Sprzątanie niedokończonego wysyłania (żeby nie naliczać miejsca za porzucone części).
            let _ = self.call(
                "DELETE",
                key,
                &[("uploadId", upload_id.clone())],
                &[],
                EMPTY_SHA256,
                Body::None,
                None,
            );
        }
        res
    }

    fn upload_parts(
        &self,
        src: &Path,
        key: &str,
        size: u64,
        upload_id: &str,
        x: &Xfer,
    ) -> RResult<()> {
        let ps = Self::part_size(size);
        let mut etags: Vec<(u32, String)> = Vec::new();
        let (mut off, mut n) = (0u64, 1u32);
        while off < size {
            let len = ps.min(size - off);
            let hash = hash_range(src, off, len)?;
            let q = [
                ("partNumber", n.to_string()),
                ("uploadId", upload_id.to_string()),
            ];
            let r = self.call(
                "PUT",
                key,
                &q,
                &[],
                &hash,
                Body::File {
                    path: src,
                    offset: off,
                    len,
                    x,
                },
                x.cancel,
            )?;
            let etag = r
                .header("ETag")
                .map(|s| s.to_string())
                .ok_or_else(|| RError::Failed(format!("serwer nie zwrócił ETag części {n}")))?;
            etags.push((n, etag));
            off += len;
            n += 1;
        }
        let mut xml = String::from("<CompleteMultipartUpload>");
        for (n, e) in &etags {
            xml.push_str(&format!(
                "<Part><PartNumber>{n}</PartNumber><ETag>{e}</ETag></Part>"
            ));
        }
        xml.push_str("</CompleteMultipartUpload>");
        let h = sha256_hex(xml.as_bytes());
        let r = self.call(
            "POST",
            key,
            &[("uploadId", upload_id.to_string())],
            &[("content-type", "application/xml".into())],
            &h,
            Body::Bytes(xml.as_bytes()),
            x.cancel,
        )?;
        self.check_body_error(r)
    }

    /// CopyObject i CompleteMultipartUpload potrafią zwrócić HTTP 200 z błędem w ciele.
    fn check_body_error(&self, r: ureq::Response) -> RResult<()> {
        let b = r
            .into_string()
            .map_err(|e| RError::Network(e.to_string()))?;
        if b.contains("<Error>") {
            let (code, msg) = parse_error(&b);
            return Err(self.map_fail(500, &code, &msg));
        }
        Ok(())
    }
}

fn hash_range(path: &Path, offset: u64, len: u64) -> RResult<String> {
    let io = |e: std::io::Error| RError::Failed(format!("odczyt pliku lokalnego: {e}"));
    let mut f = std::fs::File::open(path).map_err(io)?;
    f.seek(SeekFrom::Start(offset)).map_err(io)?;
    let mut ctx = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0u8; 256 * 1024];
    let mut left = len;
    while left > 0 {
        let want = (buf.len() as u64).min(left) as usize;
        let n = f.read(&mut buf[..want]).map_err(io)?;
        if n == 0 {
            return Err(RError::Failed(
                "plik lokalny zmienił się podczas wysyłania (jest krótszy)".into(),
            ));
        }
        ctx.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(hex(ctx.finish().as_ref()))
}

impl S3Backend {
    fn download_once(&self, key: &str, path: &str, dest: &Path, x: &Xfer) -> RResult<()> {
        let r = self.call("GET", key, &[], &[], EMPTY_SHA256, Body::None, x.cancel)?;
        let expected = r
            .header("Content-Length")
            .and_then(|v| v.trim().parse::<u64>().ok());
        let n = http::copy_to_file(r.into_reader(), dest, x)?;
        match expected {
            Some(e) if e != n => Err(RError::Network(format!(
                "pobieranie przerwane: odebrano {n} z {e} bajtów"
            ))),
            Some(_) => Ok(()),
            None => {
                // Brak Content-Length (np. obiekt z Content-Encoding: gzip rozpakowany w locie) — sprawdź rozmiar HEAD-em.
                match self.head_meta(key)? {
                    Some(m) if m.size == n => Ok(()),
                    Some(m) => Err(RError::Failed(format!(
                        "rozmiar pobranego pliku ({n}) różni się od rozmiaru obiektu ({}); obiekty z Content-Encoding nie są obsługiwane",
                        m.size
                    ))),
                    None => Err(RError::NotFound(path.to_string())),
                }
            }
        }
    }

    fn upload_impl(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka docelowa".into()));
        }
        let key = self.full(&rel);
        let size = std::fs::metadata(src)
            .map_err(|e| RError::Failed(format!("odczyt pliku lokalnego: {e}")))?
            .len();
        let mut meta: Vec<(&str, String)> = Vec::new();
        if mtime > 0 {
            meta.push(("x-amz-meta-mtime", mtime.to_string()));
        }
        if size > MULTIPART_THRESHOLD {
            return self.upload_multipart(src, &key, size, &meta, x);
        }
        let hash = hash_range(src, 0, size)?;
        self.call(
            "PUT",
            &key,
            &[],
            &meta,
            &hash,
            Body::File {
                path: src,
                offset: 0,
                len: size,
                x,
            },
            x.cancel,
        )?;
        Ok(())
    }
}

impl Backend for S3Backend {
    fn kind(&self) -> &'static str {
        "s3"
    }

    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>> {
        let base = self.full(&clean_path(dir)?);
        let prefix = if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        };
        let (objs, _) = self.list_all(&prefix, None)?;
        let rp = self.root_prefix();
        let mut out: Vec<(String, Obj)> = Vec::new();
        for o in objs {
            if o.key.ends_with('/') {
                continue; // znacznik folderu utworzony przez inne narzędzie
            }
            let Some(rel) = o.key.strip_prefix(rp.as_str()) else {
                continue;
            };
            if clean_path(rel).map(|c| c != rel).unwrap_or(true) {
                log::warn!("S3: pomijam klucz o niedozwolonej ścieżce: {rel}");
                continue;
            }
            out.push((rel.to_string(), o));
        }
        let mut entries: Vec<REntry> = out
            .iter()
            .map(|(rel, o)| REntry {
                path: rel.clone(),
                meta: RMeta {
                    size: o.size,
                    mtime: parse_iso(&o.lm),
                    hash: etag_hash(&o.etag),
                },
            })
            .collect();
        if self.precise_mtime && !entries.is_empty() {
            let next = std::sync::atomic::AtomicUsize::new(0);
            let results: std::sync::Mutex<Vec<(usize, RResult<Option<RMeta>>)>> =
                std::sync::Mutex::new(Vec::new());
            std::thread::scope(|s| {
                for _ in 0..8.min(entries.len()) {
                    s.spawn(|| loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= entries.len() {
                            break;
                        }
                        let r = self.head_meta(&self.full(&entries[i].path));
                        results.lock().unwrap().push((i, r));
                    });
                }
            });
            for (i, r) in results.into_inner().unwrap() {
                if let Some(m) = r? {
                    entries[i].meta.mtime = m.mtime;
                }
            }
        }
        Ok(entries)
    }

    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>> {
        let base = self.full(&clean_path(dir)?);
        let prefix = if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        };
        let (objs, prefixes) = self.list_all(&prefix, Some("/"))?;
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        for p in prefixes {
            let name = p
                .strip_prefix(prefix.as_str())
                .unwrap_or(&p)
                .trim_end_matches('/');
            if !name.is_empty() {
                seen.insert(name.to_string(), true);
            }
        }
        for o in objs {
            let name = o.key.strip_prefix(prefix.as_str()).unwrap_or(&o.key);
            if !name.is_empty() && !name.contains('/') {
                seen.entry(name.to_string()).or_insert(false);
            }
        }
        Ok(seen
            .into_iter()
            .map(|(name, is_dir)| DirItem { name, is_dir })
            .collect())
    }

    fn stat(&self, path: &str) -> RResult<Option<REntry>> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Ok(None);
        }
        Ok(self
            .head_meta(&self.full(&rel))?
            .map(|meta| REntry { path: rel, meta }))
    }

    fn download(&self, path: &str, dest: &Path, x: &Xfer) -> RResult<()> {
        let rel = clean_path(path)?;
        let key = self.full(&rel);
        self.retry_body(x.cancel, || self.download_once(&key, path, dest, x))
    }

    fn upload(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()> {
        self.upload_impl(src, path, mtime, x)
    }

    fn delete(&self, path: &str) -> RResult<()> {
        let rel = clean_path(path)?;
        if rel.is_empty() {
            return Err(RError::Failed("pusta ścieżka".into()));
        }
        match self.simple("DELETE", &self.full(&rel), &[]) {
            Ok(_) | Err(RError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        let (f, t) = (clean_path(from)?, clean_path(to)?);
        if f.is_empty() || t.is_empty() {
            return Err(RError::Failed("pusta ścieżka".into()));
        }
        if f == t {
            return Ok(());
        }
        let (fk, tk) = (self.full(&f), self.full(&t));
        let src = format!("/{}/{}", self.bucket, uri_encode(&fk, true));
        let r = self.call(
            "PUT",
            &tk,
            &[],
            &[("x-amz-copy-source", src)],
            EMPTY_SHA256,
            Body::None,
            None,
        )?;
        self.check_body_error(r).map_err(|e| match e {
            RError::NotFound(_) => RError::NotFound(from.to_string()),
            e => e,
        })?;
        self.delete(from)
    }

    fn mkdirs(&self, dir: &str) -> RResult<()> {
        clean_path(dir).map(|_| ())
    }

    fn ping(&self) -> RResult<String> {
        let p = self.list_page(&self.root_prefix(), Some("/"), None, Some(1))?;
        Ok(format!(
            "OK (S3, bucket {}, {} obiektów w próbce)",
            self.bucket,
            p.objs.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(extra: &[(&str, &str)]) -> Params {
        let mut p = Params::default();
        p.public.insert("bucket".into(), "moj-bucket".into());
        p.public.insert("access_key_id".into(), "AK".into());
        p.secret.insert("secret_access_key".into(), "SK".into());
        for (k, v) in extra {
            p.public.insert(k.to_string(), v.to_string());
        }
        p
    }

    #[test]
    fn aws_virtual_hosted_and_path_style() {
        let b = S3Backend::new(&params(&[("region", "eu-central-1")]), "").unwrap();
        let (url, uri) = b.target("Zażółć gęślą/a b+c.txt");
        assert_eq!(
            url,
            "https://moj-bucket.s3.eu-central-1.amazonaws.com/Za%C5%BC%C3%B3%C5%82%C4%87%20g%C4%99%C5%9Bl%C4%85/a%20b%2Bc.txt"
        );
        assert!(uri.starts_with("/Za"));
        assert_eq!(
            b.target("").0,
            "https://moj-bucket.s3.eu-central-1.amazonaws.com/"
        );
        let p = S3Backend::new(
            &params(&[("provider", "MinIO"), ("endpoint", "http://127.0.0.1:9000")]),
            "",
        )
        .unwrap();
        assert_eq!(p.target("a/b").0, "http://127.0.0.1:9000/moj-bucket/a/b");
        assert_eq!(p.target("").0, "http://127.0.0.1:9000/moj-bucket/");
        // bucket z kropką -> path-style nawet na AWS
        let mut q = params(&[("region", "us-east-1")]);
        q.public.insert("bucket".into(), "a.b.c".into());
        assert_eq!(
            S3Backend::new(&q, "").unwrap().target("k").0,
            "https://s3.us-east-1.amazonaws.com/a.b.c/k"
        );
        // domyślny port zdejmowany
        let d = S3Backend::new(
            &params(&[
                ("provider", "inne"),
                ("endpoint", "https://s3.example.com:443/"),
            ]),
            "",
        )
        .unwrap();
        assert_eq!(d.host, "s3.example.com");
    }

    #[test]
    fn missing_endpoint_for_custom_provider() {
        assert!(S3Backend::new(&params(&[("provider", "MinIO")]), "").is_err());
    }

    #[test]
    fn parses_list_with_entities_and_url_encoding() {
        let xml = r#"<?xml version="1.0"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
          <IsTruncated>true</IsTruncated><NextContinuationToken>tok&amp;1</NextContinuationToken><EncodingType>url</EncodingType>
          <Contents><Key>a+b/Za%C5%BC%C3%B3%C5%82%C4%87.txt</Key><LastModified>2024-01-02T03:04:05.000Z</LastModified>
          <ETag>&quot;d41d8cd98f00b204e9800998ecf8427e&quot;</ETag><Size>12</Size></Contents>
          <CommonPrefixes><Prefix>x%20y/</Prefix></CommonPrefixes></ListBucketResult>"#;
        let p = parse_list(xml).unwrap();
        assert!(p.truncated);
        assert_eq!(p.next.as_deref(), Some("tok&1"));
        assert_eq!(p.objs[0].key, "a b/Zażółć.txt");
        assert_eq!(p.objs[0].size, 12);
        assert_eq!(parse_iso(&p.objs[0].lm), 1704164645);
        assert_eq!(
            etag_hash(&p.objs[0].etag).unwrap().hex,
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(p.prefixes, vec!["x y/"]);
        assert!(parse_list("<ListBucketResult><Contents><Key>a</Key>").is_err());
        assert!(parse_list("<html>nope</html>").is_err());
        assert!(etag_hash("\"abc-3\"").is_none());
    }
}
