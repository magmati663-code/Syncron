//! Testy backendu S3: podpis SigV4 na oficjalnych wektorach AWS oraz pełny backend przeciw mockowi serwera S3
//! działającemu w procesie testu (mock WERYFIKUJE podpis każdego żądania).
use ring::digest;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use syncron_core::model::Bandwidth;
use syncron_core::remote::s3::{self, sha256_hex, uri_encode, SigInput};
use syncron_core::remote::{self, Backend, Params, RError, Xfer};

const AK: &str = "AKIDTEST";
const SK: &str = "sekret/KEY+test";
const BUCKET: &str = "testbkt";
const REGION: &str = "eu-central-1";

// ----------------------------------------------------------------------------------------------------------------
// Wektory testowe AWS

#[allow(clippy::too_many_arguments)]
fn vec_sig(
    method: &str,
    uri: &str,
    query: &[(&str, &str)],
    headers: &[(&str, &str)],
    payload: &str,
    region: &str,
    service: &str,
    ak: &str,
    sk: &str,
    date: &str,
) -> s3::Signed {
    let q: Vec<(String, String)> = query
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let h: Vec<(String, String)> = headers
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    s3::sign(&SigInput {
        method,
        canonical_uri: uri,
        query: &q,
        headers: &h,
        payload_hash: payload,
        region,
        service,
        access_key: ak,
        secret_key: sk,
        amz_date: date,
    })
}

const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[test]
fn sigv4_aws_suite_get_vanilla() {
    let s = vec_sig(
        "GET",
        "/",
        &[],
        &[
            ("Host", "example.amazonaws.com"),
            ("X-Amz-Date", "20150830T123600Z"),
        ],
        EMPTY,
        "us-east-1",
        "service",
        "AKIDEXAMPLE",
        "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830T123600Z",
    );
    assert_eq!(
        s.canonical_request,
        format!("GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n{EMPTY}")
    );
    assert_eq!(
        s.string_to_sign,
        "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\nbb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
    );
    assert_eq!(
        s.signature,
        "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
    assert_eq!(
        s.authorization,
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
    );
}

#[test]
fn sigv4_aws_suite_post_vanilla() {
    let s = vec_sig(
        "POST",
        "/",
        &[],
        &[
            ("Host", "example.amazonaws.com"),
            ("X-Amz-Date", "20150830T123600Z"),
        ],
        EMPTY,
        "us-east-1",
        "service",
        "AKIDEXAMPLE",
        "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        "20150830T123600Z",
    );
    assert_eq!(
        s.signature,
        "5da7c1a2acd57cee7505fc6676e4e544621c30862966e37dddb68e92efbe5d6b"
    );
}

const S3_AK: &str = "AKIAIOSFODNN7EXAMPLE";
const S3_SK: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

#[test]
fn sigv4_s3_docs_examples() {
    let host = "examplebucket.s3.amazonaws.com";
    // GET Object z nagłówkiem Range
    let s = vec_sig(
        "GET",
        "/test.txt",
        &[],
        &[
            ("host", host),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY),
            ("x-amz-date", "20130524T000000Z"),
        ],
        EMPTY,
        "us-east-1",
        "s3",
        S3_AK,
        S3_SK,
        "20130524T000000Z",
    );
    assert_eq!(
        s.signature,
        "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
    );
    // PUT Object ze znakiem `$` w kluczu (kodowanie ścieżki) i dodatkowym nagłówkiem x-amz-
    let body_hash = sha256_hex(b"Welcome to Amazon S3.");
    assert_eq!(uri_encode("/test$file.text", true), "/test%24file.text");
    let s = vec_sig(
        "PUT",
        "/test%24file.text",
        &[],
        &[
            ("date", "Fri, 24 May 2013 00:00:00 GMT"),
            ("host", host),
            ("x-amz-content-sha256", &body_hash),
            ("x-amz-date", "20130524T000000Z"),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
        ],
        &body_hash,
        "us-east-1",
        "s3",
        S3_AK,
        S3_SK,
        "20130524T000000Z",
    );
    assert_eq!(
        s.signature,
        "98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
    );
    // GET Bucket Lifecycle (parametr bez wartości)
    let s = vec_sig(
        "GET",
        "/",
        &[("lifecycle", "")],
        &[
            ("host", host),
            ("x-amz-content-sha256", EMPTY),
            ("x-amz-date", "20130524T000000Z"),
        ],
        EMPTY,
        "us-east-1",
        "s3",
        S3_AK,
        S3_SK,
        "20130524T000000Z",
    );
    assert_eq!(
        s.signature,
        "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
    );
    // GET Bucket (List Objects) — sortowanie parametrów zapytania
    let s = vec_sig(
        "GET",
        "/",
        &[("prefix", "J"), ("max-keys", "2")],
        &[
            ("host", host),
            ("x-amz-content-sha256", EMPTY),
            ("x-amz-date", "20130524T000000Z"),
        ],
        EMPTY,
        "us-east-1",
        "s3",
        S3_AK,
        S3_SK,
        "20130524T000000Z",
    );
    assert_eq!(
        s.signature,
        "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
    );
}

#[test]
fn uri_encoding_rules() {
    assert_eq!(uri_encode("a b+c/ł~._-", true), "a%20b%2Bc/%C5%82~._-");
    assert_eq!(uri_encode("a/b", false), "a%2Fb");
    assert_eq!(uri_encode("100%", true), "100%25");
}

// ----------------------------------------------------------------------------------------------------------------
// Mock serwera S3

#[derive(Default, Clone)]
struct Object {
    data: Vec<u8>,
    meta: BTreeMap<String, String>,
    modified: i64,
    etag: String,
}

#[derive(Default)]
struct Upload {
    key: String,
    meta: BTreeMap<String, String>,
    parts: BTreeMap<u32, Vec<u8>>,
}

#[derive(Default)]
struct State {
    objs: BTreeMap<String, Object>,
    uploads: HashMap<String, Upload>,
    log: Vec<String>,
    page_size: usize,
    /// Następne N żądań (poza podpisem) dostaje 503 SlowDown.
    slowdown: u32,
    /// Numer części, którą mock odrzuca (400 EntityTooLarge).
    reject_part: Option<u32>,
    /// Pierwsze N odpowiedzi GET obiektu jest ucięte w połowie ciała.
    truncate_get: u32,
    /// Wszystkie odpowiedzi listy są ucięte.
    truncate_list: bool,
    next_id: u64,
    sig_checked: u64,
}

struct Mock {
    port: u16,
    st: Arc<Mutex<State>>,
}

impl Mock {
    fn start() -> Mock {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let st = Arc::new(Mutex::new(State {
            page_size: 3,
            ..Default::default()
        }));
        let s2 = st.clone();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let s3 = s2.clone();
                std::thread::spawn(move || serve(c, s3));
            }
        });
        Mock { port, st }
    }
    fn params(&self) -> Params {
        let mut p = Params::default();
        for (k, v) in [
            ("provider", "MinIO"),
            ("bucket", BUCKET),
            ("region", REGION),
            ("access_key_id", AK),
            ("path_style", "true"),
            ("retry_base_ms", "5"),
        ] {
            p.public.insert(k.into(), v.into());
        }
        p.public
            .insert("endpoint".into(), format!("http://127.0.0.1:{}", self.port));
        p.secret.insert("secret_access_key".into(), SK.into());
        p
    }
    fn open(&self, root: &str) -> Box<dyn Backend> {
        remote::open("s3", &self.params(), root).unwrap()
    }
    fn count(&self, needle: &str) -> usize {
        self.st
            .lock()
            .unwrap()
            .log
            .iter()
            .filter(|l| l.contains(needle))
            .count()
    }
}

struct Req {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn pdecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Kodowanie jak S3 przy encoding-type=url: spacja jako `+`.
fn s3enc(s: &str) -> String {
    uri_encode(s, true).replace("%20", "+")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn read_req(r: &mut BufReader<TcpStream>) -> Option<Req> {
    let mut line = String::new();
    if r.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let target = it.next()?.to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        r.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).ok()?;
    Some(Req {
        method,
        path,
        query,
        headers,
        body,
    })
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// Ucina odpowiedź po tylu bajtach ciała i zamyka połączenie.
    cut_at: Option<usize>,
}

fn resp(status: u16, body: &str) -> Resp {
    Resp {
        status,
        headers: vec![("Content-Type".into(), "application/xml".into())],
        body: body.as_bytes().to_vec(),
        cut_at: None,
    }
}

fn err(status: u16, code: &str, msg: &str) -> Resp {
    resp(
        status,
        &format!(
            "<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>{msg}</Message></Error>"
        ),
    )
}

/// Weryfikacja SigV4 po stronie mocka (niezależna normalizacja ścieżki i zapytania).
fn verify_sig(q: &Req) -> Result<(), Resp> {
    let h = |n: &str| {
        q.headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone())
    };
    let auth = h("authorization").ok_or_else(|| err(403, "AccessDenied", "brak podpisu"))?;
    let rest = auth
        .strip_prefix("AWS4-HMAC-SHA256 ")
        .ok_or_else(|| err(400, "InvalidArgument", "algorytm"))?;
    let (mut cred, mut shs, mut sig) = ("", "", "");
    for part in rest.split(", ") {
        if let Some(v) = part.strip_prefix("Credential=") {
            cred = v;
        } else if let Some(v) = part.strip_prefix("SignedHeaders=") {
            shs = v;
        } else if let Some(v) = part.strip_prefix("Signature=") {
            sig = v;
        }
    }
    let mut c = cred.split('/');
    let ak = c.next().unwrap_or("");
    let _date = c.next().unwrap_or("");
    let region = c.next().unwrap_or("");
    let service = c.next().unwrap_or("");
    if ak != AK {
        return Err(err(
            403,
            "InvalidAccessKeyId",
            "The AWS Access Key Id you provided does not exist",
        ));
    }
    if service != "s3" {
        return Err(err(403, "SignatureDoesNotMatch", "usługa"));
    }
    let amz_date = h("x-amz-date").unwrap_or_default();
    let phash = h("x-amz-content-sha256").unwrap_or_default();
    if phash != "UNSIGNED-PAYLOAD" && phash != sha256_hex(&q.body) {
        return Err(err(400, "XAmzContentSHA256Mismatch", "skrót ciała"));
    }
    let names: Vec<&str> = shs.split(';').collect();
    for must in ["host", "x-amz-date", "x-amz-content-sha256"] {
        if !names.contains(&must) {
            return Err(err(
                403,
                "SignatureDoesNotMatch",
                "brak podpisanego nagłówka",
            ));
        }
    }
    for (k, _) in &q.headers {
        if k.starts_with("x-amz-") && k != "x-amz-content-sha256" && !names.contains(&k.as_str()) {
            return Err(err(
                403,
                "SignatureDoesNotMatch",
                &format!("niepodpisany nagłówek {k}"),
            ));
        }
    }
    let headers: Vec<(String, String)> = names
        .iter()
        .map(|n| (n.to_string(), h(n).unwrap_or_default()))
        .collect();
    let query: Vec<(String, String)> = q
        .query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (pdecode(k), pdecode(v))
        })
        .collect();
    let uri = uri_encode(&pdecode(&q.path), true);
    let want = s3::sign(&SigInput {
        method: &q.method,
        canonical_uri: &uri,
        query: &query,
        headers: &headers,
        payload_hash: &phash,
        region,
        service,
        access_key: ak,
        secret_key: SK,
        amz_date: &amz_date,
    });
    if region != REGION || want.signature != sig {
        return Err(err(
            403,
            "SignatureDoesNotMatch",
            "The request signature we calculated does not match",
        ));
    }
    Ok(())
}

fn fake_etag(data: &[u8]) -> String {
    sha256_hex(data)[..32].to_string()
}

fn iso(ts: i64) -> String {
    chrono_fmt(ts, "%Y-%m-%dT%H:%M:%S.000Z")
}

fn chrono_fmt(ts: i64, f: &str) -> String {
    let secs = ts.max(0) as u64;
    // Prosty format bez zależności od chrono w testach: używamy `date` zaimplementowanego ręcznie.
    let (y, mo, d, h, mi, s) = civil(secs);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][((secs / 86400) % 7) as usize];
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(mo - 1) as usize];
    if f.starts_with("%Y-") {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.000Z")
    } else {
        format!("{wd}, {d:02} {mon} {y:04} {h:02}:{mi:02}:{s:02} GMT")
    }
}

fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn qget(q: &str, key: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (pdecode(k) == key).then(|| pdecode(v))
    })
}

fn handle(st: &Arc<Mutex<State>>, q: &Req) -> Resp {
    {
        let mut s = st.lock().unwrap();
        s.log
            .push(format!("{} {}?{}", q.method, pdecode(&q.path), q.query));
        if s.slowdown > 0 {
            s.slowdown -= 1;
            return err(503, "SlowDown", "Please reduce your request rate.");
        }
    }
    if let Err(r) = verify_sig(q) {
        return r;
    }
    st.lock().unwrap().sig_checked += 1;
    let path = pdecode(&q.path);
    let Some(rest) = path.strip_prefix(&format!("/{BUCKET}")) else {
        return err(404, "NoSuchBucket", "The specified bucket does not exist");
    };
    let key = rest.strip_prefix('/').unwrap_or(rest).to_string();
    let mut s = st.lock().unwrap();
    match (q.method.as_str(), key.is_empty()) {
        ("GET", true) => {
            let prefix = qget(&q.query, "prefix").unwrap_or_default();
            let delim = qget(&q.query, "delimiter");
            let after = qget(&q.query, "continuation-token")
                .map(|t| t.trim_start_matches("T:").to_string());
            let max = qget(&q.query, "max-keys")
                .and_then(|m| m.parse::<usize>().ok())
                .unwrap_or(1000)
                .min(s.page_size);
            let mut out = String::new();
            let (mut n, mut trunc, mut last) = (0usize, false, String::new());
            let mut seen_prefixes: Vec<String> = Vec::new();
            for (k, o) in &s.objs {
                if !k.starts_with(&prefix)
                    || after.as_ref().is_some_and(|a| k.as_str() <= a.as_str())
                {
                    continue;
                }
                let rem = &k[prefix.len()..];
                let common = delim.as_ref().and_then(|d| {
                    rem.find(d.as_str())
                        .map(|i| format!("{prefix}{}", &rem[..i + d.len()]))
                });
                if let Some(cp) = &common {
                    if seen_prefixes.contains(cp) {
                        last = k.clone();
                        continue;
                    }
                }
                if n == max {
                    trunc = true;
                    break;
                }
                n += 1;
                last = k.clone();
                match common {
                    Some(cp) => {
                        out.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", xml_escape(&s3enc(&cp))));
                        seen_prefixes.push(cp);
                    }
                    None => out.push_str(&format!(
                        "<Contents><Key>{}</Key><LastModified>{}</LastModified><ETag>&quot;{}&quot;</ETag><Size>{}</Size></Contents>",
                        xml_escape(&s3enc(k)),
                        iso(o.modified),
                        o.etag,
                        o.data.len()
                    )),
                }
            }
            let token = if trunc {
                format!(
                    "<NextContinuationToken>T:{}</NextContinuationToken>",
                    xml_escape(&last)
                )
            } else {
                String::new()
            };
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>{BUCKET}</Name><Prefix>{}</Prefix><EncodingType>url</EncodingType><KeyCount>{n}</KeyCount><IsTruncated>{trunc}</IsTruncated>{token}{out}</ListBucketResult>",
                xml_escape(&s3enc(&prefix))
            );
            let mut r = resp(200, &body);
            if s.truncate_list {
                r.cut_at = Some(body.len() / 2);
            }
            r
        }
        ("GET" | "HEAD", false) => match s.objs.get(&key).cloned() {
            None => {
                if q.method == "HEAD" {
                    Resp {
                        status: 404,
                        headers: vec![],
                        body: vec![],
                        cut_at: None,
                    }
                } else {
                    err(404, "NoSuchKey", "The specified key does not exist.")
                }
            }
            Some(o) => {
                let mut headers = vec![
                    ("ETag".to_string(), format!("\"{}\"", o.etag)),
                    ("Last-Modified".to_string(), chrono_fmt(o.modified, "%a")),
                ];
                for (k, v) in &o.meta {
                    headers.push((format!("x-amz-meta-{k}"), v.clone()));
                }
                let mut r = Resp {
                    status: 200,
                    headers,
                    body: o.data.clone(),
                    cut_at: None,
                };
                if q.method == "GET" && s.truncate_get > 0 {
                    s.truncate_get -= 1;
                    r.cut_at = Some(o.data.len() / 2);
                }
                r
            }
        },
        ("PUT", false) => {
            let meta: BTreeMap<String, String> = q
                .headers
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix("x-amz-meta-")
                        .map(|m| (m.to_string(), v.clone()))
                })
                .collect();
            if let Some(src) = q
                .headers
                .iter()
                .find(|(k, _)| k == "x-amz-copy-source")
                .map(|(_, v)| pdecode(v))
            {
                let skey = src
                    .trim_start_matches('/')
                    .strip_prefix(&format!("{BUCKET}/"))
                    .unwrap_or("")
                    .to_string();
                return match s.objs.get(&skey).cloned() {
                    None => err(404, "NoSuchKey", "źródło"),
                    Some(mut o) => {
                        o.modified = now();
                        s.objs.insert(key, o);
                        resp(200, "<CopyObjectResult><ETag>x</ETag></CopyObjectResult>")
                    }
                };
            }
            if let (Some(pn), Some(uid)) =
                (qget(&q.query, "partNumber"), qget(&q.query, "uploadId"))
            {
                let pn: u32 = pn.parse().unwrap();
                if s.reject_part == Some(pn) {
                    return err(
                        400,
                        "EntityTooLarge",
                        "Your proposed upload exceeds the maximum allowed size",
                    );
                }
                let Some(u) = s.uploads.get_mut(&uid) else {
                    return err(404, "NoSuchUpload", "brak");
                };
                u.parts.insert(pn, q.body.clone());
                let mut r = resp(200, "");
                r.headers
                    .push(("ETag".into(), format!("\"{}\"", fake_etag(&q.body))));
                return r;
            }
            let etag = fake_etag(&q.body);
            s.objs.insert(
                key,
                Object {
                    data: q.body.clone(),
                    meta,
                    modified: now(),
                    etag: etag.clone(),
                },
            );
            let mut r = resp(200, "");
            r.headers.push(("ETag".into(), format!("\"{etag}\"")));
            r
        }
        ("POST", false) if q.query.starts_with("uploads") => {
            s.next_id += 1;
            let id = format!("up{}", s.next_id);
            let meta: BTreeMap<String, String> = q
                .headers
                .iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix("x-amz-meta-")
                        .map(|m| (m.to_string(), v.clone()))
                })
                .collect();
            s.uploads.insert(
                id.clone(),
                Upload {
                    key,
                    meta,
                    parts: BTreeMap::new(),
                },
            );
            resp(
                200,
                &format!("<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"),
            )
        }
        ("POST", false) => {
            let Some(uid) = qget(&q.query, "uploadId") else {
                return err(400, "InvalidRequest", "uploadId");
            };
            let Some(u) = s.uploads.remove(&uid) else {
                return err(404, "NoSuchUpload", "brak");
            };
            let xml = String::from_utf8_lossy(&q.body).to_string();
            let mut data = Vec::new();
            let mut n = 0;
            for chunk in xml.split("<PartNumber>").skip(1) {
                let pn: u32 = chunk.split('<').next().unwrap().parse().unwrap();
                let etag = chunk
                    .split("<ETag>")
                    .nth(1)
                    .unwrap()
                    .split('<')
                    .next()
                    .unwrap()
                    .trim_matches('"')
                    .to_string();
                let part = u.parts.get(&pn).expect("brak części");
                if fake_etag(part) != etag {
                    return err(400, "InvalidPart", "zły ETag");
                }
                data.extend_from_slice(part);
                n += 1;
            }
            s.objs.insert(
                u.key,
                Object {
                    data,
                    meta: u.meta,
                    modified: now(),
                    etag: format!("{}-{n}", &sha256_hex(xml.as_bytes())[..32]),
                },
            );
            resp(
                200,
                "<CompleteMultipartUploadResult><ETag>x</ETag></CompleteMultipartUploadResult>",
            )
        }
        ("DELETE", false) => {
            if let Some(uid) = qget(&q.query, "uploadId") {
                s.uploads.remove(&uid);
            } else {
                s.objs.remove(&key);
            }
            Resp {
                status: 204,
                headers: vec![],
                body: vec![],
                cut_at: None,
            }
        }
        _ => err(400, "InvalidRequest", "nieobsługiwane"),
    }
}

fn serve(c: TcpStream, st: Arc<Mutex<State>>) {
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    while let Some(q) = read_req(&mut r) {
        let rs = handle(&st, &q);
        let reason = match rs.status {
            200 => "OK",
            204 => "No Content",
            _ => "Err",
        };
        let mut head = format!("HTTP/1.1 {} {reason}\r\n", rs.status);
        for (k, v) in &rs.headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", rs.body.len()));
        if w.write_all(head.as_bytes()).is_err() {
            return;
        }
        if q.method != "HEAD" {
            let end = rs.cut_at.unwrap_or(rs.body.len());
            if w.write_all(&rs.body[..end]).is_err() {
                return;
            }
            if rs.cut_at.is_some() {
                let _ = w.flush();
                let _ = w.shutdown(std::net::Shutdown::Both);
                return;
            }
        }
        let _ = w.flush();
    }
}

// ----------------------------------------------------------------------------------------------------------------
// Testy backendu

fn xfer<'a>(bw: &'a Bandwidth) -> Xfer<'a> {
    Xfer::new(bw)
}

fn put_local(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

#[test]
fn roundtrip_polish_names_list_stat_overwrite_rename_delete() {
    let m = Mock::start();
    let b = m.open("Kopie/Praca");
    let tmp = tempfile::tempdir().unwrap();
    let bw = Bandwidth::default();
    let x = xfer(&bw);
    let names = [
        "Zażółć gęślą jaźń.txt",
        "Dokumenty/Umowa & aneks (1).docx",
        "Dokumenty/a+b=c.txt",
        "Dokumenty/Głębiej/śćńółż.bin",
        "pusty.txt",
        "100% pewne #1.txt",
        "z'apostrof\"cudzysłów.txt",
        "ZAŁĄCZNIK/ą.txt",
    ];
    for (i, n) in names.iter().enumerate() {
        let data = format!("treść pliku {i} – zażółć").into_bytes();
        let data = if *n == "pusty.txt" { Vec::new() } else { data };
        let src = put_local(tmp.path(), "src.bin", &data);
        b.upload(&src, n, 1_600_000_000 + i as i64, &x).unwrap();
    }
    // paginacja (page_size = 3) i komplet nazw
    let mut files = b.list_files("").unwrap();
    files.sort_by(|a, c| a.path.cmp(&c.path));
    let mut want: Vec<&str> = names.to_vec();
    want.sort();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        want
    );
    assert!(m.count("continuation-token") >= 2, "powinna być paginacja");
    // klucze w S3 mają prefiks korzenia
    assert!(m
        .st
        .lock()
        .unwrap()
        .objs
        .contains_key("Kopie/Praca/Zażółć gęślą jaźń.txt"));
    // podkatalog
    let sub = b.list_files("Dokumenty").unwrap();
    assert_eq!(sub.len(), 3);
    // przeglądarka folderów
    let top = b.list_dir("").unwrap();
    let dirs: Vec<_> = top
        .iter()
        .filter(|d| d.is_dir)
        .map(|d| d.name.as_str())
        .collect();
    assert_eq!(dirs, vec!["Dokumenty", "ZAŁĄCZNIK"]);
    assert!(top
        .iter()
        .any(|d| !d.is_dir && d.name == "Zażółć gęślą jaźń.txt"));
    let d2 = b.list_dir("Dokumenty").unwrap();
    assert!(d2.iter().any(|d| d.is_dir && d.name == "Głębiej"));
    assert!(d2.iter().any(|d| !d.is_dir && d.name == "a+b=c.txt"));
    // stat: dokładny mtime z metadanych, rozmiar, brak
    let st = b.stat("Zażółć gęślą jaźń.txt").unwrap().unwrap();
    assert_eq!(st.meta.mtime, 1_600_000_000);
    assert_eq!(st.meta.size, "treść pliku 0 – zażółć".len() as u64);
    assert!(st.meta.hash.is_some());
    assert!(b.stat("nie ma.txt").unwrap().is_none());
    assert!(b.stat("Dokumenty").unwrap().is_none());
    // pobranie
    let dest = tmp.path().join("dl.bin");
    b.download("Dokumenty/Umowa & aneks (1).docx", &dest, &x)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&dest).unwrap(),
        "treść pliku 1 – zażółć"
    );
    assert!(matches!(
        b.download("brak.txt", &dest, &x),
        Err(RError::NotFound(_))
    ));
    // nadpisanie
    let src = put_local(tmp.path(), "nowy.bin", b"nowa zawartosc");
    b.upload(&src, "Dokumenty/a+b=c.txt", 1_700_000_000, &x)
        .unwrap();
    b.download("Dokumenty/a+b=c.txt", &dest, &x).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"nowa zawartosc");
    assert_eq!(
        b.stat("Dokumenty/a+b=c.txt").unwrap().unwrap().meta.mtime,
        1_700_000_000
    );
    // rename (server-side): nowy folder, mtime zachowany, stary klucz znika, istniejący cel zastąpiony
    b.rename("Dokumenty/a+b=c.txt", "Nowy folder/Żółw.txt")
        .unwrap();
    assert!(b.stat("Dokumenty/a+b=c.txt").unwrap().is_none());
    let moved = b.stat("Nowy folder/Żółw.txt").unwrap().unwrap();
    assert_eq!(moved.meta.mtime, 1_700_000_000);
    b.rename("pusty.txt", "Nowy folder/Żółw.txt").unwrap();
    assert_eq!(
        b.stat("Nowy folder/Żółw.txt").unwrap().unwrap().meta.size,
        0
    );
    assert!(matches!(
        b.rename("nie ma", "gdzies"),
        Err(RError::NotFound(_))
    ));
    // usuwanie (brak = sukces)
    b.delete("Nowy folder/Żółw.txt").unwrap();
    b.delete("Nowy folder/Żółw.txt").unwrap();
    assert!(b.stat("Nowy folder/Żółw.txt").unwrap().is_none());
    // foldery to prefiksy
    b.mkdirs("Cokolwiek/głębiej").unwrap();
    b.rmdir_if_empty("Cokolwiek").unwrap();
    assert!(!m
        .st
        .lock()
        .unwrap()
        .objs
        .keys()
        .any(|k| k.contains("Cokolwiek")));
    // każde żądanie miało poprawny podpis
    assert!(m.st.lock().unwrap().sig_checked > 20);
}

#[test]
fn parent_dir_paths_are_rejected() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let src = put_local(tmp.path(), "a", b"x");
    let bw = Bandwidth::default();
    assert!(b.upload(&src, "../x", 0, &xfer(&bw)).is_err());
    assert!(b.upload(&src, "a/../../x", 0, &xfer(&bw)).is_err());
    assert!(b.list_files("../..").is_err());
    assert!(b.stat("a/../b").is_err());
    assert!(b
        .download("../x", &tmp.path().join("o"), &xfer(&bw))
        .is_err());
    assert!(b.delete("..").is_err());
    assert!(b.rename("a", "../b").is_err());
    assert!(remote::open("s3", &m.params(), "../root").is_err());
    assert!(m.st.lock().unwrap().objs.is_empty());
}

#[test]
fn wrong_credentials_and_bucket_map_to_auth_and_notfound() {
    let m = Mock::start();
    // zły klucz tajny
    let mut p = m.params();
    p.secret
        .insert("secret_access_key".into(), "zly-klucz".into());
    let b = remote::open("s3", &p, "").unwrap();
    match b.list_files("") {
        Err(RError::Auth(msg)) => {
            assert!(msg.contains("region") || msg.contains("klucz"), "{msg}");
            assert!(!msg.contains("zly-klucz"), "sekret w komunikacie");
        }
        other => panic!("oczekiwano Auth: {other:?}"),
    }
    assert!(matches!(b.ping(), Err(RError::Auth(_))));
    // zły Access Key ID
    let mut p = m.params();
    p.public.insert("access_key_id".into(), "AKIDZLY".into());
    assert!(matches!(
        remote::open("s3", &p, "").unwrap().list_dir(""),
        Err(RError::Auth(_))
    ));
    // zły region -> podpis nie pasuje
    let mut p = m.params();
    p.public.insert("region".into(), "us-west-2".into());
    match remote::open("s3", &p, "").unwrap().stat("a") {
        Err(RError::Auth(msg)) => assert!(msg.contains("region"), "{msg}"),
        other => panic!("{other:?}"),
    }
    // nieistniejący bucket
    let mut p = m.params();
    p.public.insert("bucket".into(), "nie-ma-takiego".into());
    assert!(matches!(
        remote::open("s3", &p, "").unwrap().list_files(""),
        Err(RError::NotFound(_))
    ));
    // brak wymaganych pól
    let mut p = m.params();
    p.secret.clear();
    assert!(remote::open("s3", &p, "").is_err());
    // poprawne dane: ping
    assert!(m.open("").ping().unwrap().contains("OK"));
}

#[test]
fn slowdown_is_retried_and_eventually_busy() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let src = put_local(tmp.path(), "a", b"abc");
    let bw = Bandwidth::default();
    m.st.lock().unwrap().slowdown = 2;
    b.upload(&src, "a.txt", 5, &xfer(&bw)).unwrap();
    assert_eq!(m.count("503"), 0);
    assert_eq!(
        m.count("PUT /testbkt/a.txt"),
        3,
        "dwie próby odrzucone + sukces"
    );
    m.st.lock().unwrap().slowdown = 100;
    assert!(matches!(b.stat("a.txt"), Err(RError::Busy(_))));
    m.st.lock().unwrap().slowdown = 0;
    assert!(b.stat("a.txt").unwrap().is_some());
}

#[test]
fn connection_cut_during_download_is_retried_and_during_listing_is_error() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let src = put_local(tmp.path(), "big", &data);
    let bw = Bandwidth::default();
    b.upload(&src, "big.bin", 1, &xfer(&bw)).unwrap();
    // jedno ucięcie GET -> ponowienie kończy się sukcesem
    m.st.lock().unwrap().truncate_get = 1;
    let dest = tmp.path().join("dl");
    b.download("big.bin", &dest, &xfer(&bw)).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    // stałe ucinanie listy -> Err (silnik niczego wtedy nie usuwa)
    m.st.lock().unwrap().truncate_list = true;
    assert!(b.list_files("").is_err());
    assert!(b.list_dir("").is_err());
    m.st.lock().unwrap().truncate_list = false;
    assert_eq!(b.list_files("").unwrap().len(), 1);
}

#[test]
fn unreachable_server_is_network_error() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut p = Mock::start().params();
    p.public
        .insert("endpoint".into(), format!("http://127.0.0.1:{port}"));
    let b = remote::open("s3", &p, "").unwrap();
    assert!(matches!(b.list_files(""), Err(RError::Network(_))));
}

#[test]
fn multipart_70mb_with_bandwidth_limit() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let size = 70 * 1024 * 1024usize;
    let mut data = vec![0u8; size];
    let mut x: u32 = 12345;
    for c in data.iter_mut() {
        x = x.wrapping_mul(1664525).wrapping_add(1013904223);
        *c = (x >> 24) as u8;
    }
    let src = put_local(tmp.path(), "duzy.bin", &data);
    // 40 000 KB/s => 70 MiB nie szybciej niż ok. 1,8 s
    let bw = Bandwidth {
        kbps: 40_000,
        off_kbps: 0,
        hours: None,
    };
    let sent = std::sync::atomic::AtomicU64::new(0);
    let prog = |n: u64| {
        sent.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    };
    let xf = Xfer {
        bw: &bw,
        cancel: None,
        progress: Some(&prog),
    };
    let t = Instant::now();
    b.upload(&src, "Wielki plik ż.bin", 1_650_000_000, &xf)
        .unwrap();
    let el = t.elapsed();
    assert!(
        el >= Duration::from_millis(1500),
        "limit prędkości nie działa: {el:?}"
    );
    assert_eq!(sent.load(std::sync::atomic::Ordering::Relaxed), size as u64);
    assert_eq!(m.count("uploads="), 1);
    assert_eq!(m.count("partNumber="), 5, "70 MiB / 16 MiB = 5 części");
    let stored =
        m.st.lock()
            .unwrap()
            .objs
            .get("Wielki plik ż.bin")
            .cloned()
            .unwrap();
    assert_eq!(stored.data.len(), size);
    assert_eq!(
        digest::digest(&digest::SHA256, &stored.data).as_ref(),
        digest::digest(&digest::SHA256, &data).as_ref()
    );
    assert_eq!(
        stored.meta.get("mtime").map(|s| s.as_str()),
        Some("1650000000")
    );
    // ETag wieloczęściowy ("-N") nie jest hashem zawartości
    assert!(b
        .stat("Wielki plik ż.bin")
        .unwrap()
        .unwrap()
        .meta
        .hash
        .is_none());
    // pobranie z powrotem (bez limitu)
    let dest = tmp.path().join("back.bin");
    b.download("Wielki plik ż.bin", &dest, &xfer(&Bandwidth::default()))
        .unwrap();
    assert_eq!(std::fs::metadata(&dest).unwrap().len(), size as u64);
    assert!(std::fs::read(&dest).unwrap() == data);
}

#[test]
fn multipart_failure_aborts_upload() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("duzy2.bin");
    {
        let f = std::fs::File::create(&src).unwrap();
        f.set_len(66 * 1024 * 1024).unwrap(); // plik rzadki: same zera
    }
    m.st.lock().unwrap().reject_part = Some(3);
    let bw = Bandwidth::default();
    let r = b.upload(&src, "x/duzy2.bin", 1, &xfer(&bw));
    assert!(matches!(r, Err(RError::Quota(_))), "{r:?}");
    assert_eq!(
        m.count("DELETE /testbkt/x/duzy2.bin?uploadId="),
        1,
        "brak AbortMultipartUpload"
    );
    let s = m.st.lock().unwrap();
    assert!(
        s.uploads.is_empty(),
        "porzucone wysyłanie nie zostało sprzątnięte"
    );
    assert!(s.objs.is_empty());
}

#[test]
fn parallel_uploads_and_downloads() {
    let m = Mock::start();
    let b: Arc<dyn Backend> = Arc::from(m.open("rown"));
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let mut hs = Vec::new();
    for t in 0..8 {
        let b = b.clone();
        let dir = dir.clone();
        hs.push(std::thread::spawn(move || {
            let bw = Bandwidth::default();
            for i in 0..5 {
                let data = format!("wątek {t} plik {i}").repeat(1000).into_bytes();
                let src = put_local(&dir, &format!("w{t}_{i}"), &data);
                let name = format!("w{t}/plik ł {i}.txt");
                b.upload(&src, &name, 100 + i, &Xfer::new(&bw)).unwrap();
                let dest = dir.join(format!("d{t}_{i}"));
                b.download(&name, &dest, &Xfer::new(&bw)).unwrap();
                assert_eq!(std::fs::read(&dest).unwrap(), data);
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(b.list_files("").unwrap().len(), 40);
}

#[test]
fn precise_mtime_lists_metadata_mtime() {
    let m = Mock::start();
    let tmp = tempfile::tempdir().unwrap();
    let src = put_local(tmp.path(), "a", b"abc");
    let bw = Bandwidth::default();
    m.open("")
        .upload(&src, "a.txt", 1_234_567_890, &xfer(&bw))
        .unwrap();
    let plain = m.open("").list_files("").unwrap();
    assert_ne!(
        plain[0].meta.mtime, 1_234_567_890,
        "zwykła lista zwraca LastModified"
    );
    assert!(plain[0].meta.mtime >= now() - 100);
    let mut p = m.params();
    p.public.insert("precise_mtime".into(), "1".into());
    let precise = remote::open("s3", &p, "").unwrap().list_files("").unwrap();
    assert_eq!(precise[0].meta.mtime, 1_234_567_890);
}

#[test]
fn cancel_stops_upload() {
    let m = Mock::start();
    let b = m.open("");
    let tmp = tempfile::tempdir().unwrap();
    let src = put_local(tmp.path(), "a", &vec![7u8; 3_000_000]);
    let bw = Bandwidth {
        kbps: 500,
        off_kbps: 0,
        hours: None,
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let xf = Xfer {
        bw: &bw,
        cancel: Some(&cancel),
        progress: None,
    };
    let t = Instant::now();
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert!(b.upload(&src, "a.bin", 1, &xf).is_err());
    });
    assert!(t.elapsed() < Duration::from_secs(4));
    assert!(m.st.lock().unwrap().objs.is_empty());
}

#[test]
fn no_check_certificate_option_builds_backend() {
    let m = Mock::start();
    let mut p = m.params();
    p.public.insert("no_check_certificate".into(), "1".into());
    let b = remote::open("s3", &p, "").unwrap();
    assert!(b.ping().is_ok());
}
