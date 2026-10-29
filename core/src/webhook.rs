//! Webhook po zakończeniu przebiegu: POST z podsumowaniem (JSON albo formaty Slack / Discord / ntfy).
//!
//! Zasady bezpieczeństwa: adres jest zwykle sekretem (token w ścieżce), więc nigdy nie trafia do logów ani do
//! komunikatów błędów w całości ([`mask_url`]); przekierowania wolno śledzić tylko w obrębie tego samego hosta;
//! adresy lokalne, prywatne i linkowo-lokalne są domyślnie blokowane już w momencie łączenia (resolver),
//! więc przekierowanie DNS (rebinding) też ich nie ominie. Zezwala na nie dopiero `allow_private`.
use crate::engine::Report;
use crate::model::{Job, NotifyOn, WebhookFormat};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

/// Najwięcej błędów w ładunku.
pub const MAX_ERRORS: usize = 20;
const MAX_REDIRECTS: usize = 3;

/// Ładunek webhooka (format `Json`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Payload {
    /// `sync` (przebieg) albo `test`.
    pub event: String,
    pub job: String,
    pub job_id: String,
    /// `ok` | `errors` | `conflicts` | `blocked` | `failed` | `test`.
    pub status: String,
    pub copied: usize,
    pub deleted: usize,
    pub moved: usize,
    pub conflicts: usize,
    pub errors: Vec<String>,
    pub duration_ms: u64,
    pub started: i64,
    pub left: String,
    pub right: String,
}

/// Parametry wysyłki (domyślne: 10 s, 2 ponowienia).
#[derive(Debug, Clone)]
pub struct Opts {
    pub allow_private: bool,
    pub timeout: Duration,
    pub retries: u32,
    /// Przerwa przed pierwszym ponowieniem (każde kolejne dwa razy dłuższa).
    pub retry_delay: Duration,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            allow_private: false,
            timeout: Duration::from_secs(10),
            retries: 2,
            retry_delay: Duration::from_millis(1500),
        }
    }
}

/// Adres do logów: tylko pierwsze 8 znaków (zwykle `https://`), reszta zamaskowana.
pub fn mask_url(url: &str) -> String {
    let head: String = url.chars().take(8).collect();
    if url.chars().count() <= 8 {
        head
    } else {
        format!("{head}…(zamaskowano)")
    }
}

/// Sprawdza składnię: tylko `http://` lub `https://`, z hostem, bez danych logowania w adresie
/// i bez znaków sterujących. Pusty adres = brak webhooka.
pub fn validate_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Ok(());
    }
    if u.chars().count() > 2048 || u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Adres webhooka jest zbyt długi albo zawiera niedozwolone znaki.".into());
    }
    let rest = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .ok_or("Adres webhooka musi zaczynać się od http:// albo https://.")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.starts_with(':') {
        return Err("Adres webhooka nie zawiera nazwy hosta.".into());
    }
    if authority.contains('@') {
        return Err(
            "Adres webhooka nie może zawierać nazwy użytkownika ani hasła (użyj tokenu w ścieżce)."
                .into(),
        );
    }
    Ok(())
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?.to_string()
    } else {
        authority.split(':').next()?.to_string()
    };
    Some(host.to_ascii_lowercase())
}

/// Adresy, do których domyślnie nie łączymy się z webhookiem.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || o[0] == 0
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_private_ip(&IpAddr::V4(v4));
            }
            let s = a.segments();
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // ULA
                || (s[0] & 0xffc0) == 0xfe80 // link-local
        }
    }
}

struct SafeResolver {
    allow_private: bool,
}

impl ureq::Resolver for SafeResolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let all: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
        if self.allow_private {
            return Ok(all);
        }
        let ok: Vec<SocketAddr> = all
            .iter()
            .copied()
            .filter(|a| !is_private_ip(&a.ip()))
            .collect();
        if ok.is_empty() && !all.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "adres lokalny lub prywatny jest zablokowany",
            ));
        }
        Ok(ok)
    }
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Wartość nagłówka: ASCII bez zmian, inaczej RFC 2047 (`=?UTF-8?B?…?=`, rozumie ją ntfy).
fn header_value(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.is_ascii() {
        clean
    } else {
        format!("=?UTF-8?B?{}?=", b64(clean.as_bytes()))
    }
}

fn status_pl(status: &str) -> &'static str {
    match status {
        "ok" => "zakończono pomyślnie",
        "errors" => "zakończono z błędami",
        "conflicts" => "wykryto konflikty",
        "blocked" => "zatrzymano przez zabezpieczenie",
        "failed" => "przebieg nie powiódł się",
        "test" => "wiadomość testowa",
        _ => "zakończono",
    }
}

/// Czytelne podsumowanie (Slack / Discord / ntfy).
pub fn summary_text(p: &Payload) -> String {
    let mut t = format!(
        "Syncron — {}: {}.\nSkopiowano {}, usunięto {}, przeniesiono {}, konflikty {}, błędy {} ({:.1} s).",
        p.job,
        status_pl(&p.status),
        p.copied,
        p.deleted,
        p.moved,
        p.conflicts,
        p.errors.len(),
        p.duration_ms as f64 / 1000.0
    );
    for e in p.errors.iter().take(3) {
        t.push_str("\n• ");
        t.push_str(&e.chars().take(200).collect::<String>());
    }
    t
}

type Rendered = (&'static str, Vec<u8>, Vec<(&'static str, String)>);

/// Ciało i nagłówki dla wybranego formatu: (content-type, ciało, dodatkowe nagłówki).
pub fn render(format: WebhookFormat, p: &Payload) -> Rendered {
    match format {
        WebhookFormat::Json => (
            "application/json",
            serde_json::to_vec(p).unwrap_or_default(),
            vec![],
        ),
        WebhookFormat::Slack => (
            "application/json",
            serde_json::json!({ "text": summary_text(p) })
                .to_string()
                .into_bytes(),
            vec![],
        ),
        WebhookFormat::Discord => {
            let t: String = summary_text(p).chars().take(1900).collect();
            (
                "application/json",
                serde_json::json!({ "content": t }).to_string().into_bytes(),
                vec![],
            )
        }
        WebhookFormat::Ntfy => {
            let (prio, tag) = match p.status.as_str() {
                "ok" | "test" => ("default", "white_check_mark"),
                "conflicts" => ("high", "warning"),
                _ => ("high", "x"),
            };
            (
                "text/plain; charset=utf-8",
                summary_text(p).into_bytes(),
                vec![
                    ("Title", header_value(&format!("Syncron — {}", p.job))),
                    ("Priority", prio.to_string()),
                    ("Tags", tag.to_string()),
                ],
            )
        }
    }
}

/// Opis błędu bez adresu (komunikaty `ureq` zawierają pełny URL).
fn describe(e: &ureq::Error, url: &str) -> String {
    let m = match e {
        ureq::Error::Status(c, _) => format!("serwer odpowiedział kodem {c}"),
        ureq::Error::Transport(t) => {
            let msg = t.message().unwrap_or("").to_string();
            if msg.is_empty() {
                format!("{:?}", t.kind())
            } else {
                format!("{:?} ({msg})", t.kind())
            }
        }
    };
    m.replace(url, "[adres]")
}

type Fail = (String, bool);

fn attempt(url: &str, (ct, body, headers): &Rendered, o: &Opts) -> Result<(), Fail> {
    let agent = ureq::AgentBuilder::new()
        .timeout(o.timeout)
        .redirects(0)
        .resolver(SafeResolver {
            allow_private: o.allow_private,
        })
        .user_agent("Syncron-webhook")
        .build();
    let origin = host_of(url).unwrap_or_default();
    let mut cur = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let mut req = agent.post(&cur).set("Content-Type", ct);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_bytes(body) {
            Ok(r) if (300..400).contains(&r.status()) => {
                let Some(loc) = r.header("Location").map(str::to_string) else {
                    return Err(("przekierowanie bez nagłówka Location".into(), false));
                };
                let next = if loc.starts_with("http://") || loc.starts_with("https://") {
                    loc
                } else if loc.starts_with('/') {
                    let (scheme, rest) = cur.split_once("://").unwrap_or(("https", ""));
                    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
                    format!("{scheme}://{auth}{loc}")
                } else {
                    return Err(("nieobsługiwane przekierowanie".into(), false));
                };
                if host_of(&next).as_deref() != Some(origin.as_str()) {
                    return Err((
                        "przekierowanie na inny host zostało zablokowane".into(),
                        false,
                    ));
                }
                if cur.starts_with("https://") && next.starts_with("http://") {
                    return Err((
                        "przekierowanie z https na http zostało zablokowane".into(),
                        false,
                    ));
                }
                cur = next;
            }
            Ok(_) => return Ok(()),
            Err(e) => {
                let retry = match &e {
                    ureq::Error::Status(c, _) => *c == 429 || *c >= 500,
                    ureq::Error::Transport(_) => true,
                };
                return Err((describe(&e, &cur), retry));
            }
        }
    }
    Err(("zbyt wiele przekierowań".into(), false))
}

/// Wysyła ładunek (z ponowieniami). Błąd nie zawiera adresu — tylko jego zamaskowaną postać.
pub fn send(url: &str, format: WebhookFormat, p: &Payload, o: &Opts) -> Result<(), String> {
    let url = url.trim();
    validate_url(url)?;
    if url.is_empty() {
        return Err("Adres webhooka jest pusty.".into());
    }
    let rendered = render(format, p);
    let mut delay = o.retry_delay;
    let mut last = String::new();
    for n in 0..=o.retries {
        match attempt(url, &rendered, o) {
            Ok(()) => return Ok(()),
            Err((msg, retry)) => {
                log::warn!(
                    "webhook {} (próba {}/{}): {msg}",
                    mask_url(url),
                    n + 1,
                    o.retries + 1
                );
                last = msg;
                if !retry {
                    break;
                }
            }
        }
        if n < o.retries {
            std::thread::sleep(delay);
            delay *= 2;
        }
    }
    Err(format!("Webhook {} nie powiódł się: {last}", mask_url(url)))
}

/// Ładunek testowy (przycisk „Wyślij testowy”).
pub fn test_payload() -> Payload {
    Payload {
        event: "test".into(),
        job: "Test Syncron".into(),
        job_id: "test".into(),
        status: "test".into(),
        copied: 0,
        deleted: 0,
        moved: 0,
        conflicts: 0,
        errors: vec![],
        duration_ms: 0,
        started: chrono::Local::now().timestamp(),
        left: "-".into(),
        right: "-".into(),
    }
}

/// Wysyła wiadomość testową (bez ponowień).
pub fn send_test(url: &str, format: WebhookFormat, allow_private: bool) -> Result<(), String> {
    send(
        url,
        format,
        &test_payload(),
        &Opts {
            allow_private,
            retries: 0,
            ..Opts::default()
        },
    )
}

/// Status przebiegu: `ok` | `errors` | `conflicts` | `blocked` | `failed`.
pub fn status_of(res: &anyhow::Result<Report>) -> &'static str {
    match res {
        Err(_) => "failed",
        Ok(r) if r.blocked.is_some() => "blocked",
        Ok(r) if !r.errors.is_empty() => "errors",
        Ok(r) if !r.conflicts.is_empty() => "conflicts",
        Ok(_) => "ok",
    }
}

/// Czy dla danego statusu wysyłać powiadomienie.
pub fn wanted(on: NotifyOn, status: &str) -> bool {
    match on {
        NotifyOn::Never => false,
        NotifyOn::Always => true,
        NotifyOn::Errors => status != "ok",
    }
}

/// Buduje ładunek z wyniku przebiegu.
pub fn payload_for(job: &Job, res: &anyhow::Result<Report>, duration_ms: u64) -> Payload {
    let mut p = Payload {
        event: "sync".into(),
        job: job.name.clone(),
        job_id: job.id.clone(),
        status: status_of(res).into(),
        copied: 0,
        deleted: 0,
        moved: 0,
        conflicts: 0,
        errors: vec![],
        duration_ms,
        started: chrono::Local::now().timestamp(),
        left: job.left.clone(),
        right: job.right.clone(),
    };
    match res {
        Ok(r) => {
            p.copied = r.copied;
            p.deleted = r.deleted;
            p.moved = r.moved;
            p.conflicts = r.conflicts.len();
            p.started = r.started;
            p.errors = r.errors.iter().take(MAX_ERRORS).cloned().collect();
            if let Some(b) = &r.blocked {
                p.errors.insert(0, b.clone());
                p.errors.truncate(MAX_ERRORS);
            }
        }
        Err(e) => p.errors = vec![e.to_string()],
    }
    p
}

/// Wywoływane po przebiegu (nie w symulacji). Zwraca ostrzeżenie, gdy wysyłka się nie udała.
pub fn notify_after_run(
    job: &Job,
    res: &anyhow::Result<Report>,
    duration_ms: u64,
) -> Option<String> {
    let url = job
        .webhook_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())?;
    let status = status_of(res);
    if !wanted(job.webhook_on, status) {
        return None;
    }
    let p = payload_for(job, res, duration_ms);
    let o = Opts {
        allow_private: job.webhook_allow_private,
        ..Opts::default()
    };
    match send(url, job.webhook_format, &p, &o) {
        Ok(()) => {
            log::info!("{}: webhook {} wysłany", job.name, mask_url(url));
            None
        }
        Err(e) => {
            log::warn!("{}: {e}", job.name);
            Some(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_keeps_eight_chars() {
        let m = mask_url("https://hooks.example.com/SECRET-TOKEN");
        assert!(m.starts_with("https://") && !m.contains("hooks") && !m.contains("SECRET"));
        assert_eq!(mask_url("abc"), "abc");
    }

    #[test]
    fn url_validation() {
        assert!(validate_url("https://ntfy.sh/topic").is_ok());
        assert!(validate_url("").is_ok());
        assert!(validate_url("ftp://x/y").is_err());
        assert!(validate_url("https://u:p@host/x").is_err());
        assert!(validate_url("https:///x").is_err());
        assert!(validate_url("https://a b/x").is_err());
    }

    #[test]
    fn private_ranges() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_private_ip(&s.parse().unwrap()), "{s}");
        }
        for s in ["8.8.8.8", "2606:4700::1111"] {
            assert!(!is_private_ip(&s.parse().unwrap()), "{s}");
        }
    }

    #[test]
    fn header_encoding() {
        assert_eq!(header_value("abc"), "abc");
        assert!(header_value("Zażółć").starts_with("=?UTF-8?B?"));
        assert_eq!(b64(b"ab"), "YWI=");
    }
}
