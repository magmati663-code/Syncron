//! Syncron 0.8 (zespół E): raporty z historii — filtry, podsumowanie, CSV/JSON/HTML, rotacja historii.
use std::collections::HashMap;
use std::path::Path;
use syncron_core::engine::open_db;
use syncron_core::report::{self, Filter, Format};

const DAY: i64 = 86_400;

fn add(dir: &Path, ts: i64, job: &str, kind: &str, path: &str, detail: &str) {
    let c = open_db(dir).unwrap();
    c.execute(
        "INSERT INTO history(ts,job,kind,path,detail) VALUES(?1,?2,?3,?4,?5)",
        rusqlite::params![ts, job, kind, path, detail],
    )
    .unwrap();
}

fn names() -> HashMap<String, String> {
    [("j1", "Dokumenty"), ("j2", "Zdjęcia")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

fn seed(dir: &Path) {
    let n = now();
    add(
        dir,
        n - 10 * DAY,
        "j1",
        "kopiowanie →",
        "stary.txt",
        "100 B",
    );
    add(dir, n - 2 * DAY, "j1", "kopiowanie →", "a.txt", "2048 B");
    add(dir, n - 2 * DAY, "j1", "kopiowanie ←", "b.txt", "1000 B");
    add(dir, n - 2 * DAY, "j1", "usunięcie", "c.txt", "");
    add(dir, n - 2 * DAY, "j1", "błąd", "d.txt", "brak dostępu");
    add(
        dir,
        n - 2 * DAY,
        "j1",
        "polecenie po",
        "",
        "Polecenie po zakończone",
    );
    add(dir, n - DAY, "j2", "kopiowanie →", "x.jpg", "");
    add(dir, n - DAY, "j2", "błąd", "y.jpg", "brak dostępu");
    add(dir, n - DAY, "j2", "błąd", "z.jpg", "dysk pełny");
    add(dir, n - DAY, "j2", "konflikt", "k.doc", "wymaga decyzji");
    add(
        dir,
        n - DAY,
        "j2",
        "konflikt rozstrzygnięty",
        "k2.doc",
        "lewy",
    );
    add(dir, n - DAY, "j2", "przeniesienie", "m.txt", "z: n.txt");
}

#[test]
fn filtr_po_zadaniu_rodzaju_i_czasie() {
    let t = tempfile::tempdir().unwrap();
    seed(t.path());
    let all = report::query(t.path(), &Filter::default(), 1000).unwrap();
    assert_eq!(all.len(), 12);

    let f = Filter {
        job: Some("j2".into()),
        ..Default::default()
    };
    assert_eq!(report::query(t.path(), &f, 1000).unwrap().len(), 6);

    let f = Filter {
        kind: Some("błąd".into()),
        ..Default::default()
    };
    assert_eq!(report::query(t.path(), &f, 1000).unwrap().len(), 3);
    // alias angielski i wielkość liter
    let f = Filter {
        kind: Some("ERROR".into()),
        ..Default::default()
    };
    assert_eq!(report::query(t.path(), &f, 1000).unwrap().len(), 3);

    let f = Filter {
        since: Some(report::parse_since("3d", now()).unwrap()),
        ..Default::default()
    };
    assert_eq!(report::query(t.path(), &f, 1000).unwrap().len(), 11);
    let f = Filter {
        since: Some(now() - 5 * DAY),
        until: Some(now() - DAY - 100),
        job: Some("j1".into()),
        ..Default::default()
    };
    assert_eq!(report::query(t.path(), &f, 1000).unwrap().len(), 5);

    // znaki specjalne LIKE nie działają jak wzorzec
    let f = Filter {
        kind: Some("%".into()),
        ..Default::default()
    };
    assert!(report::query(t.path(), &f, 1000).unwrap().is_empty());
    assert_eq!(
        report::query(t.path(), &Filter::default(), 3)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn podsumowanie() {
    let t = tempfile::tempdir().unwrap();
    seed(t.path());
    let s = report::summary(t.path(), &Filter::default(), &names()).unwrap();
    assert_eq!(s.total, 12);
    assert_eq!(s.copied, 4);
    assert_eq!(s.deleted, 1);
    assert_eq!(s.moved, 1);
    assert_eq!(s.conflicts, 1);
    assert_eq!(s.resolved, 1);
    assert_eq!(s.errors, 3);
    // polecenie po nie liczy się jako przebieg: (j1,-10d), (j1,-2d), (j2,-1d)
    assert_eq!(s.runs, 3);
    assert_eq!(s.bytes_copied, 100 + 2048 + 1000);
    assert_eq!(
        s.bytes_known_files, 3,
        "wpis bez rozmiaru nie wchodzi do sumy"
    );
    assert_eq!(s.top_errors[0].message, "brak dostępu");
    assert_eq!(s.top_errors[0].count, 2);
    assert_eq!(s.top_errors.len(), 2);
    assert_eq!(s.by_job.len(), 2);
    assert_eq!(s.by_job[0].name, "Dokumenty");
    assert_eq!(s.by_job[0].copied, 3);
    assert_eq!(s.by_job[1].errors, 2);
    assert!(s.first_ts.unwrap() < s.last_ts.unwrap());
}

#[test]
fn csv_bom_separator_i_ochrona_przed_formulami() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    add(
        t.path(),
        n,
        "j1",
        "błąd",
        "=HYPERLINK(\"http://zly\";\"x\")",
        "+cmd|' /C calc'!A0",
    );
    add(t.path(), n, "j1", "błąd", "@SUM(1;2)", "-2+3");
    add(
        t.path(),
        n,
        "j1",
        "kopiowanie →",
        "z;średnikiem \"i cudzysłowem\".txt",
        "10 B",
    );
    add(
        t.path(),
        n,
        "j1",
        "kopiowanie →",
        "wiersz\nnowy.txt",
        "10 B",
    );
    let rows = report::query(t.path(), &Filter::default(), 100).unwrap();
    let csv = report::to_csv(&rows, &names());
    assert!(csv.starts_with("\u{feff}czas;zadanie;operacja;plik;szczegóły\r\n"));
    assert!(csv.contains(";Dokumenty;"), "nazwa zadania zamiast id");
    for field in ["'=HYPERLINK", "'+cmd", "'@SUM", "'-2+3"] {
        assert!(csv.contains(field), "{field} w: {csv}");
    }
    assert!(
        !csv.contains(";=") && !csv.contains(";+") && !csv.contains(";@") && !csv.contains(";-2")
    );
    assert!(csv.contains("\"z;średnikiem \"\"i cudzysłowem\"\".txt\""));
    assert!(csv.contains("\"wiersz\nnowy.txt\""));
}

#[test]
fn json_jest_poprawny_i_zawiera_podsumowanie() {
    let t = tempfile::tempdir().unwrap();
    seed(t.path());
    let f = Filter {
        job: Some("j2".into()),
        ..Default::default()
    };
    let rows = report::query(t.path(), &f, 1000).unwrap();
    let txt = report::render(Format::Json, &rows, &f, &names());
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["rows"].as_array().unwrap().len(), 6);
    assert_eq!(v["rows"][0]["job_name"], "Zdjęcia");
    assert_eq!(v["summary"]["errors"], 2);
    assert_eq!(v["filter"]["job"], "j2");
}

#[test]
fn html_jest_samodzielny_i_escapowany() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>";
    add(
        t.path(),
        n,
        "j<1>",
        "błąd",
        evil,
        "\"><svg onload=alert(3)>",
    );
    add(t.path(), n, "j<1>", "kopiowanie →", "ok.txt", "5 B");
    let nm: HashMap<String, String> =
        [("j<1>".to_string(), "<b>Zadanie</b> & co".to_string())].into();
    let rows = report::query(t.path(), &Filter::default(), 100).unwrap();
    let html = report::render(Format::Html, &rows, &Filter::default(), &nm);
    assert!(html.starts_with("<!doctype html>"));
    assert!(!html.contains("<script") && !html.contains("<img") && !html.contains("<svg"));
    assert!(!html.contains("<b>Zadanie"));
    assert!(
        html.contains("&lt;script&gt;") && html.contains("&lt;b&gt;Zadanie&lt;/b&gt; &amp; co")
    );
    // brak zewnętrznych zasobów
    for bad in ["<link", "<a ", "url(", "@import", "http://", "https://"] {
        assert!(!html.contains(bad), "nieoczekiwane {bad}");
    }
    assert!(html.contains("Content-Security-Policy"));
    assert!(html.contains("Najczęstsze błędy"));
}

#[test]
fn eksport_do_pliku_wszystkie_formaty() {
    let t = tempfile::tempdir().unwrap();
    seed(t.path());
    for (fmt, ext) in [
        (Format::Csv, "csv"),
        (Format::Json, "json"),
        (Format::Html, "html"),
    ] {
        let out = t.path().join(format!("raport.{ext}"));
        let n = report::export(t.path(), fmt, &Filter::default(), &names(), &out).unwrap();
        assert_eq!(n, 12);
        assert!(std::fs::metadata(&out).unwrap().len() > 100);
        assert!(!out.with_extension("syncron-tmp").exists());
    }
    assert!(report::export(
        t.path(),
        Format::Csv,
        &Filter::default(),
        &names(),
        t.path()
    )
    .is_err());
    assert!(Format::parse("xml").is_err());
    assert_eq!(Format::parse("HTML").unwrap(), Format::Html);
}

#[test]
fn rotacja_historii_wedlug_ustawienia() {
    let t = tempfile::tempdir().unwrap();
    let n = now();
    add(
        t.path(),
        n - 400 * DAY,
        "j1",
        "kopiowanie →",
        "bardzo-stary",
        "",
    );
    add(t.path(), n - 100 * DAY, "j1", "kopiowanie →", "stary", "");
    add(t.path(), n - DAY, "j1", "kopiowanie →", "nowy", "");

    assert_eq!(
        report::history_limit_days(t.path()),
        365,
        "domyślnie 365 dni"
    );
    assert_eq!(report::prune_now(t.path()).unwrap(), 1);
    let left = report::query(t.path(), &Filter::default(), 100).unwrap();
    assert_eq!(left.len(), 2);

    std::fs::write(
        t.path().join("settings.json"),
        r#"{"history_limit_days":30}"#,
    )
    .unwrap();
    assert_eq!(report::history_limit_days(t.path()), 30);
    assert_eq!(report::prune_now(t.path()).unwrap(), 1);
    let left = report::query(t.path(), &Filter::default(), 100).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].path, "nowy");

    // 0 = bez rotacji; uszkodzony plik = domyślnie
    add(t.path(), n - 900 * DAY, "j1", "kopiowanie →", "relikt", "");
    std::fs::write(
        t.path().join("settings.json"),
        r#"{"history_limit_days":0}"#,
    )
    .unwrap();
    assert_eq!(report::prune_now(t.path()).unwrap(), 0);
    std::fs::write(t.path().join("settings.json"), "{zepsute").unwrap();
    assert_eq!(report::history_limit_days(t.path()), 365);
}

#[cfg(target_os = "linux")]
#[test]
fn przebieg_zapisuje_rozmiar_i_stosuje_limit_historii() {
    use syncron_core::engine::run_job;
    use syncron_core::model::Job;
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    std::fs::create_dir_all(&s).unwrap();
    std::fs::write(l.join("a.bin"), vec![7u8; 1234]).unwrap();
    std::fs::write(s.join("settings.json"), r#"{"history_limit_days":30}"#).unwrap();
    add(&s, now() - 60 * DAY, "inne", "kopiowanie →", "stary", "");
    add(&s, now() - 5 * DAY, "inne", "kopiowanie →", "swiezy", "");
    let j: Job = serde_json::from_value(serde_json::json!({
        "id": "rep1", "name": "Raport", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}
    }))
    .unwrap();
    run_job(&j, &s, false, false).unwrap();
    let sum = report::summary(&s, &Filter::default(), &HashMap::new()).unwrap();
    assert_eq!(sum.bytes_copied, 1234);
    assert_eq!(sum.copied, 2, "nowy wpis + świeży; sprzed 60 dni usunięty");
}
