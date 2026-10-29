//! Syncron 0.8 (zespół E): webhook po przebiegu — ładunek, formaty, ponowienia, limit czasu, przekierowania,
//! maskowanie adresu w logach i błędach, blokada adresów lokalnych, integracja z silnikiem.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use syncron_core::engine::{run_job, run_job_opts, RunOpts};
use syncron_core::model::{parse_import, Job, WebhookFormat};
use syncron_core::webhook::{self, Opts, Payload};

#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

/// Odpowiedź skryptu serwera dla n-tego żądania.
#[derive(Clone)]
enum Reply {
    Status(u16),
    Redirect(u16, String),
    /// Przyjmij połączenie i nie odpowiadaj (przez podany czas).
    Hang(Duration),
}

struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Req>>>,
}

impl Server {
    fn start(script: Vec<Reply>) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
        let s2 = seen.clone();
        std::thread::spawn(move || {
            for (n, conn) in l.incoming().enumerate() {
                let Ok(mut c) = conn else { break };
                let reply = script
                    .get(n)
                    .or(script.last())
                    .cloned()
                    .unwrap_or(Reply::Status(200));
                let s3 = s2.clone();
                std::thread::spawn(move || {
                    let Some(req) = read_req(&mut c) else { return };
                    s3.lock().unwrap().push(req);
                    match reply {
                        Reply::Status(code) => respond(&mut c, code, &[]),
                        Reply::Redirect(code, to) => respond(&mut c, code, &[("Location", &to)]),
                        Reply::Hang(d) => std::thread::sleep(d),
                    }
                });
            }
        });
        Server { port, seen }
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
    fn reqs(&self) -> Vec<Req> {
        std::thread::sleep(Duration::from_millis(50));
        self.seen.lock().unwrap().clone()
    }
}

fn read_req(c: &mut TcpStream) -> Option<Req> {
    c.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut b = [0u8; 1024];
    let head_end = loop {
        let n = c.read(&mut b).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&b[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = c.read(&mut b).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&b[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
    Some(Req {
        method,
        path,
        headers,
        body,
    })
}

fn respond(c: &mut TcpStream, code: u16, extra: &[(&str, &str)]) {
    let mut r = format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n");
    for (k, v) in extra {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str("\r\n");
    let _ = c.write_all(r.as_bytes());
}

fn fast() -> Opts {
    Opts {
        allow_private: true,
        timeout: Duration::from_secs(3),
        retries: 2,
        retry_delay: Duration::from_millis(20),
    }
}

fn sample() -> Payload {
    Payload {
        event: "sync".into(),
        job: "Zażółć gęślą".into(),
        job_id: "id-1".into(),
        status: "errors".into(),
        copied: 3,
        deleted: 1,
        moved: 2,
        conflicts: 4,
        errors: vec!["a.txt: brak dostępu".into(), "b.txt: zajęty".into()],
        duration_ms: 1500,
        started: 1_700_000_000,
        left: "/l".into(),
        right: "/r".into(),
    }
}

// ---- przechwytywanie logów ----
static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
struct Cap;
impl log::Log for Cap {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        // biblioteka HTTP loguje pełny adres na poziomie debug/trace; aplikacja i CLI ustawiają poziom Info
        if r.level() > log::Level::Info {
            return;
        }
        LOGS.get_or_init(Default::default)
            .lock()
            .unwrap()
            .push(format!("{}", r.args()));
    }
    fn flush(&self) {}
}
fn init_log() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = log::set_logger(&Cap);
        log::set_max_level(log::LevelFilter::Trace);
    });
}

#[test]
fn json_payload_zgodny_z_kontraktem() {
    let s = Server::start(vec![Reply::Status(200)]);
    webhook::send(&s.url("/hook"), WebhookFormat::Json, &sample(), &fast()).unwrap();
    let r = s.reqs();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].method, "POST");
    assert_eq!(r[0].path, "/hook");
    assert!(r[0]
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    let v: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
    for k in [
        "event",
        "job",
        "job_id",
        "status",
        "copied",
        "deleted",
        "moved",
        "conflicts",
        "errors",
        "duration_ms",
        "started",
        "left",
        "right",
    ] {
        assert!(v.get(k).is_some(), "brak pola {k}: {v}");
    }
    assert_eq!(v["job"], "Zażółć gęślą");
    assert_eq!(v["copied"], 3);
    assert_eq!(v["conflicts"], 4);
    assert_eq!(v["errors"].as_array().unwrap().len(), 2);
    assert_eq!(v["duration_ms"], 1500);
    assert_eq!(v["status"], "errors");
}

#[test]
fn slack_i_discord_maja_pole_tekstowe() {
    let s = Server::start(vec![Reply::Status(204)]);
    webhook::send(&s.url("/s"), WebhookFormat::Slack, &sample(), &fast()).unwrap();
    webhook::send(&s.url("/d"), WebhookFormat::Discord, &sample(), &fast()).unwrap();
    let r = s.reqs();
    let slack: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
    assert!(slack["text"].as_str().unwrap().contains("Zażółć gęślą"));
    assert!(slack["text"].as_str().unwrap().contains("Skopiowano 3"));
    assert!(slack.get("job").is_none());
    let disc: serde_json::Value = serde_json::from_str(&r[1].body).unwrap();
    assert!(disc["content"].as_str().unwrap().contains("błędy 2"));
    assert!(disc["content"].as_str().unwrap().chars().count() <= 2000);
}

#[test]
fn ntfy_czysty_tekst_i_naglowki() {
    let s = Server::start(vec![Reply::Status(200)]);
    webhook::send(&s.url("/temat"), WebhookFormat::Ntfy, &sample(), &fast()).unwrap();
    let r = &s.reqs()[0];
    assert!(r.header("content-type").unwrap().starts_with("text/plain"));
    assert!(r.body.contains("Skopiowano 3") && !r.body.starts_with('{'));
    // tytuł z polskimi znakami musi być zakodowany (RFC 2047), bo nagłówki HTTP są ASCII
    let title = r.header("title").unwrap();
    assert!(
        title.starts_with("=?UTF-8?B?") && title.is_ascii(),
        "{title}"
    );
    assert_eq!(r.header("priority"), Some("high"));
    assert_eq!(r.header("tags"), Some("x"));
    let mut ok = sample();
    ok.status = "ok".into();
    ok.errors.clear();
    webhook::send(&s.url("/t2"), WebhookFormat::Ntfy, &ok, &fast()).unwrap();
    let r2 = &s.reqs()[1];
    assert_eq!(r2.header("priority"), Some("default"));
    assert_eq!(r2.header("tags"), Some("white_check_mark"));
}

#[test]
fn ponowienia_po_bledach_serwera() {
    let s = Server::start(vec![
        Reply::Status(500),
        Reply::Status(503),
        Reply::Status(200),
    ]);
    webhook::send(&s.url("/r"), WebhookFormat::Json, &sample(), &fast()).unwrap();
    assert_eq!(s.reqs().len(), 3);

    // po wyczerpaniu ponowień (1 + 2) błąd
    let s = Server::start(vec![Reply::Status(500)]);
    let e = webhook::send(&s.url("/r"), WebhookFormat::Json, &sample(), &fast()).unwrap_err();
    assert_eq!(s.reqs().len(), 3);
    assert!(e.contains("500"), "{e}");
}

#[test]
fn blad_klienta_nie_jest_ponawiany() {
    let s = Server::start(vec![Reply::Status(404)]);
    assert!(webhook::send(&s.url("/x"), WebhookFormat::Json, &sample(), &fast()).is_err());
    assert_eq!(s.reqs().len(), 1);
}

#[test]
fn limit_czasu() {
    let s = Server::start(vec![Reply::Hang(Duration::from_secs(5))]);
    let o = Opts {
        timeout: Duration::from_millis(300),
        retries: 1,
        ..fast()
    };
    let t = Instant::now();
    let e = webhook::send(&s.url("/slow"), WebhookFormat::Json, &sample(), &o).unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert!(e.contains("nie powiódł"), "{e}");
    assert_eq!(s.reqs().len(), 2, "1 próba + 1 ponowienie");
}

#[test]
fn przekierowanie_na_inny_host_jest_blokowane() {
    let other = Server::start(vec![Reply::Status(200)]);
    // „localhost” to inny host niż „127.0.0.1”, choć wskazuje na ten sam serwer
    let s = Server::start(vec![Reply::Redirect(
        302,
        format!("http://localhost:{}/zlodziej", other.port),
    )]);
    let e = webhook::send(&s.url("/h"), WebhookFormat::Json, &sample(), &fast()).unwrap_err();
    assert!(e.contains("inny host"), "{e}");
    assert!(
        other.reqs().is_empty(),
        "żądanie nie może trafić do innego hosta"
    );
    assert_eq!(
        s.reqs().len(),
        1,
        "blokada przekierowania nie jest ponawiana"
    );
}

#[test]
fn przekierowanie_w_obrebie_hosta_jest_sledzone() {
    let s = Server::start(vec![
        Reply::Redirect(307, "/nowy".into()),
        Reply::Status(200),
    ]);
    webhook::send(&s.url("/stary"), WebhookFormat::Json, &sample(), &fast()).unwrap();
    let r = s.reqs();
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].path, "/nowy");
    assert_eq!(r[1].method, "POST");
    assert!(
        r[1].body.contains("\"copied\":3"),
        "ciało musi być ponowione"
    );
}

#[test]
fn petla_przekierowan_konczy_sie_bledem() {
    let s = Server::start(vec![Reply::Redirect(302, "/dalej".into())]);
    let e = webhook::send(&s.url("/a"), WebhookFormat::Json, &sample(), &fast()).unwrap_err();
    assert!(e.contains("przekierowań"), "{e}");
}

#[test]
fn adresy_lokalne_blokowane_domyslnie() {
    let s = Server::start(vec![Reply::Status(200)]);
    let o = Opts {
        allow_private: false,
        ..fast()
    };
    let e = webhook::send(&s.url("/h"), WebhookFormat::Json, &sample(), &o).unwrap_err();
    assert!(e.contains("nie powiódł"), "{e}");
    assert!(
        s.reqs().is_empty(),
        "serwer lokalny nie powinien dostać żądania"
    );
}

#[test]
fn adres_nie_trafia_do_logow_ani_bledow() {
    init_log();
    let token = "SEKRETNYTOKEN123456";
    let s = Server::start(vec![Reply::Status(500)]);
    let url = s.url(&format!("/hooks/{token}"));
    let e = webhook::send(&url, WebhookFormat::Json, &sample(), &fast()).unwrap_err();
    assert!(!e.contains(token) && !e.contains("/hooks"), "{e}");
    assert!(
        e.contains("http://1"),
        "powinien zostać początek adresu: {e}"
    );
    // zamknięty port: komunikaty biblioteki HTTP zwykle zawierają pełny adres
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e2 = webhook::send(
        &format!("http://127.0.0.1:{closed}/hooks/{token}"),
        WebhookFormat::Json,
        &sample(),
        &fast(),
    )
    .unwrap_err();
    assert!(!e2.contains(token), "{e2}");
    let logs = LOGS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .join("\n");
    assert!(logs.contains("webhook"), "brak logów: {logs}");
    assert!(!logs.contains(token), "token w logach: {logs}");
    assert_eq!(
        webhook::mask_url(&url).chars().take(8).collect::<String>(),
        "http://1"
    );
}

#[test]
fn walidacja_adresu() {
    assert!(webhook::validate_url("https://ntfy.sh/moj-temat").is_ok());
    for bad in [
        "ftp://x",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://u:p@h/x",
        "http://",
        "https://a b",
        "/tylko/sciezka",
    ] {
        assert!(webhook::validate_url(bad).is_err(), "{bad}");
    }
}

// ---- integracja z silnikiem ----
struct Env {
    _t: tempfile::TempDir,
    l: std::path::PathBuf,
    r: std::path::PathBuf,
    s: std::path::PathBuf,
}

fn env() -> Env {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    Env { _t: t, l, r, s }
}

fn job(e: &Env, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "wh1", "name": "Webhook test", "left": e.l.to_str().unwrap(), "right": e.r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2,
        "webhook_allow_private": true
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

#[test]
fn domyslne_pola_powiadomien() {
    let j: Job = serde_json::from_str(
        r#"{"id":"1","name":"t","left":"a","right":"b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"}}"#,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(j.notify_on).unwrap(), "Errors");
    assert_eq!(serde_json::to_value(j.webhook_on).unwrap(), "Never");
    assert_eq!(serde_json::to_value(j.webhook_format).unwrap(), "Json");
    assert!(j.webhook_url.is_none() && !j.webhook_allow_private);
}

#[test]
fn silnik_wysyla_webhook_po_przebiegu() {
    let e = env();
    std::fs::write(e.l.join("a.txt"), "dane").unwrap();
    let s = Server::start(vec![Reply::Status(200)]);
    let j = job(
        &e,
        serde_json::json!({"webhook_url": s.url("/ok"), "webhook_on": "Always"}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 1);
    assert!(rep.warnings.is_empty() && rep.errors.is_empty());
    let r = s.reqs();
    assert_eq!(r.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
    assert_eq!(v["event"], "sync");
    assert_eq!(v["job_id"], "wh1");
    assert_eq!(v["status"], "ok");
    assert_eq!(v["copied"], 1);
    assert_eq!(v["left"], e.l.to_str().unwrap());
}

#[test]
fn symulacja_nie_wysyla_webhooka() {
    let e = env();
    std::fs::write(e.l.join("a.txt"), "dane").unwrap();
    let s = Server::start(vec![Reply::Status(200)]);
    let j = job(
        &e,
        serde_json::json!({"webhook_url": s.url("/ok"), "webhook_on": "Always"}),
    );
    run_job_opts(
        &j,
        &e.s,
        RunOpts {
            dry: true,
            force: false,
            progress: None,
            skip: Default::default(),
        },
    )
    .unwrap();
    assert!(s.reqs().is_empty());
}

#[test]
fn tryby_webhook_on() {
    let e = env();
    std::fs::write(e.l.join("a.txt"), "dane").unwrap();
    // Never: nic; Errors przy udanym przebiegu: nic
    for mode in ["Never", "Errors"] {
        let s = Server::start(vec![Reply::Status(200)]);
        let j = job(
            &e,
            serde_json::json!({"webhook_url": s.url("/x"), "webhook_on": mode}),
        );
        run_job(&j, &e.s, false, false).unwrap();
        assert!(s.reqs().is_empty(), "{mode}");
    }
    // Errors przy zablokowanym przebiegu (polecenie przed kończy się błędem) — wysyła
    let s = Server::start(vec![Reply::Status(200)]);
    let cmd = "exit 3";
    let j = job(
        &e,
        serde_json::json!({"webhook_url": s.url("/blk"), "webhook_on": "Errors", "run_before": cmd}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.blocked.is_some());
    let r = s.reqs();
    assert_eq!(r.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
    assert_eq!(v["status"], "blocked");
    assert!(!v["errors"].as_array().unwrap().is_empty());
}

#[test]
fn blad_webhooka_trafia_do_ostrzezen_nie_do_bledow() {
    init_log();
    let e = env();
    std::fs::write(e.l.join("a.txt"), "dane").unwrap();
    let token = "TAJNY-TOKEN-987654";
    let s = Server::start(vec![Reply::Status(400)]);
    let j = job(
        &e,
        serde_json::json!({"webhook_url": s.url(&format!("/{token}")), "webhook_on": "Always"}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.copied, 1);
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(rep.warnings.len(), 1, "{:?}", rep.warnings);
    assert!(rep.warnings[0].contains("Webhook") && !rep.warnings[0].contains(token));
    assert!(
        !rep.needs_retry(),
        "błąd webhooka nie może wymuszać ponowień"
    );
    let logs = LOGS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .join("\n");
    assert!(!logs.contains(token));
}

#[test]
fn webhook_url_czyszczony_przy_imporcie_i_walidowany() {
    let e = env();
    let mut j = job(
        &e,
        serde_json::json!({"webhook_url": "https://hooks.example/abc"}),
    );
    j.enabled = true;
    let txt = serde_json::to_string(&vec![j.clone()]).unwrap();
    let out = parse_import(&txt).unwrap();
    assert!(out[0].webhook_url.is_none());
    assert!(!out[0].enabled);
    // zły schemat odrzuca walidacja zadania (więc i import)
    j.webhook_url = Some("ftp://x/y".into());
    assert!(j.validate().is_err());
    j.webhook_url = Some("https://ntfy.sh/t".into());
    assert!(j.validate().is_ok());
    let txt = serde_json::to_string(&vec![Job {
        webhook_url: Some("gopher://x".into()),
        ..j
    }])
    .unwrap();
    assert!(parse_import(&txt).is_err());
}
