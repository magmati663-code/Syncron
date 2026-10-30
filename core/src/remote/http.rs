//! Wspólna warstwa HTTP dla backendów (WebDAV, S3, Google Drive, OneDrive, Dropbox): jeden klient (ureq + rustls),
//! jednolite mapowanie błędów na `RError`, ponawianie przy chwilowych błędach (429/502/503/504) i ograniczanie prędkości.
use super::*;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Klient z rozsądnymi limitami czasu (połączenie, odczyt, zapis).
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(90))
        .timeout_write(Duration::from_secs(90))
        .user_agent(concat!("Syncron/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// Mapuje błąd ureq na `RError` (bez sekretów w komunikacie: pokazujemy tylko status i krótki opis serwera).
pub fn map_err(e: ureq::Error) -> RError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let short: String = body.chars().filter(|c| !c.is_control()).take(200).collect();
            let msg = format!("HTTP {code} {short}").trim().to_string();
            match code {
                401 | 403 => RError::Auth(msg),
                404 | 410 => RError::NotFound(msg),
                423 | 408 | 429 | 502 | 503 | 504 => RError::Busy(msg),
                507 => RError::Quota(msg),
                _ => RError::Failed(msg),
            }
        }
        ureq::Error::Transport(t) => RError::Network(t.to_string()),
    }
}

/// Czas oczekiwania z nagłówka `Retry-After` (sekundy), gdy jest.
fn retry_after(e: &ureq::Error) -> Option<u64> {
    if let ureq::Error::Status(_, r) = e {
        return r.header("Retry-After").and_then(|v| v.trim().parse().ok());
    }
    None
}

/// Wykonuje żądanie z ponawianiem (do 4 prób) przy 429/502/503/504 i błędach sieci; `f` tworzy i wysyła żądanie.
pub fn send<F>(mut f: F) -> RResult<ureq::Response>
where
    F: FnMut() -> Result<ureq::Response, ureq::Error>,
{
    let mut delay = 1u64;
    for attempt in 0..4 {
        match f() {
            Ok(r) => return Ok(r),
            Err(e) => {
                let transient = matches!(
                    &e,
                    ureq::Error::Status(429 | 502 | 503 | 504, _) | ureq::Error::Transport(_)
                );
                if !transient || attempt == 3 {
                    return Err(map_err(e));
                }
                let wait = retry_after(&e).unwrap_or(delay).min(30);
                std::thread::sleep(Duration::from_secs(wait));
                delay *= 2;
            }
        }
    }
    unreachable!()
}

/// Ogranicza prędkość i raportuje postęp; sprawdza anulowanie.
pub struct Throttle<'a> {
    x: Xfer<'a>,
    started: Instant,
    sent: u64,
}

impl<'a> Throttle<'a> {
    pub fn new(x: &Xfer<'a>) -> Self {
        Throttle {
            x: *x,
            started: Instant::now(),
            sent: 0,
        }
    }
    /// Wywołaj po przetworzeniu `n` bajtów: czeka, gdy przekroczono limit, zgłasza postęp.
    pub fn account(&mut self, n: u64) -> RResult<()> {
        if let Some(c) = self.x.cancel {
            if c.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(RError::Failed("przerwano".into()));
            }
        }
        self.sent += n;
        if let Some(p) = self.x.progress {
            p(n);
        }
        let kbps = self.x.bw.current_kbps();
        if kbps > 0 {
            let due = Duration::from_secs_f64(self.sent as f64 / (kbps as f64 * 1024.0));
            if let Some(w) = due.checked_sub(self.started.elapsed()) {
                std::thread::sleep(w);
            }
        }
        Ok(())
    }
}

/// Czytnik z ograniczeniem prędkości (do wysyłania ciała żądania).
pub struct ThrottledReader<'a, R: Read> {
    inner: R,
    t: Throttle<'a>,
}

impl<'a, R: Read> ThrottledReader<'a, R> {
    pub fn new(inner: R, x: &Xfer<'a>) -> Self {
        ThrottledReader {
            inner,
            t: Throttle::new(x),
        }
    }
}

impl<R: Read> Read for ThrottledReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.t.account(n as u64).map_err(std::io::Error::other)?;
        }
        Ok(n)
    }
}

/// Kopiuje strumień do pliku z ograniczeniem prędkości; zwraca liczbę bajtów.
pub fn copy_to_file(mut r: impl Read, dest: &Path, x: &Xfer) -> RResult<u64> {
    let mut f = std::fs::File::create(dest).map_err(|e| RError::Failed(e.to_string()))?;
    let mut t = Throttle::new(x);
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = r
            .read(&mut buf)
            .map_err(|e| RError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])
            .map_err(|e| RError::Failed(e.to_string()))?;
        total += n as u64;
        t.account(n as u64)?;
    }
    f.flush().map_err(|e| RError::Failed(e.to_string()))?;
    Ok(total)
}

/// Koduje składnik ścieżki do URL (procent-kodowanie wszystkiego poza niezarezerwowanymi i `/`).
pub fn url_path(p: &str) -> String {
    let mut out = String::new();
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Odwrotność `url_path` (dla odpowiedzi serwera z href).
pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_roundtrip_polish() {
        let p = "Zażółć gęślą/jaźń #1 (kopia).txt";
        assert_eq!(url_decode(&url_path(p)), p);
        assert!(!url_path(p).contains(' '));
        assert_eq!(url_decode("100%"), "100%");
    }
}
