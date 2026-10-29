//! Raporty z historii synchronizacji: filtr, podsumowanie, eksport do CSV / JSON / HTML oraz rotacja historii.
//!
//! Dane pochodzą z tabeli `history` w `state.db`. Rozmiar skopiowanych danych jest znany tylko dla wpisów
//! zapisanych przez nowsze wersje (szczegóły `"<N> B"` przy kopiowaniu); starsze wpisy nie wchodzą do sumy bajtów.
use crate::engine::{open_db, HistoryRow};
use rusqlite::params_from_iter;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// Domyślna liczba dni przechowywania historii.
pub const DEFAULT_HISTORY_DAYS: u32 = 365;
/// Maksymalna wartość `history_limit_days`.
pub const MAX_HISTORY_DAYS: u32 = 36_500;
/// Twardy limit wierszy w jednym raporcie.
pub const MAX_ROWS: usize = 1_000_000;
/// Limit wierszy tabeli w raporcie HTML (reszta jest tylko w podsumowaniu).
pub const HTML_ROWS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Html,
}

impl Format {
    pub fn parse(s: &str) -> Result<Format, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Format::Csv),
            "json" => Ok(Format::Json),
            "html" | "htm" => Ok(Format::Html),
            other => Err(format!(
                "Nieznany format raportu „{other}” (dostępne: csv, json, html)."
            )),
        }
    }
}

/// Filtr historii: zadanie (identyfikator), rodzaj operacji (fragment nazwy lub alias), zakres czasu (sekundy UNIX).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Filter {
    pub job: Option<String>,
    pub kind: Option<String>,
    /// Od (włącznie).
    pub since: Option<i64>,
    /// Do (włącznie).
    pub until: Option<i64>,
}

/// „7d”, „12h”, „30m”, „2w” albo data `RRRR-MM-DD` → sekundy UNIX (okres liczony wstecz od `now`).
pub fn parse_since(s: &str, now: i64) -> Result<i64, String> {
    let t = s.trim();
    let bad =
        || format!("Nieprawidłowy zakres czasu „{t}” (użyj np. 7d, 12h, 2w albo RRRR-MM-DD).");
    if let Ok(d) = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        use chrono::TimeZone;
        let dt = d.and_hms_opt(0, 0, 0).ok_or_else(bad)?;
        return chrono::Local
            .from_local_datetime(&dt)
            .earliest()
            .map(|x| x.timestamp())
            .ok_or_else(bad);
    }
    let (num, unit) = t.split_at(t.len().saturating_sub(1));
    let n: i64 = num.parse().map_err(|_| bad())?;
    let mult = match unit {
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(bad()),
    };
    if !(0..=100_000).contains(&n) {
        return Err(bad());
    }
    Ok(now - n * mult)
}

fn kind_pattern(k: &str) -> String {
    match k.trim().to_lowercase().as_str() {
        "copy" | "copied" => "kopiowanie".into(),
        "delete" | "deleted" | "remove" => "usunięcie".into(),
        "move" | "moved" | "rename" => "przeniesienie".into(),
        "conflict" | "conflicts" => "konflikt".into(),
        "error" | "errors" | "err" => "błąd".into(),
        "warning" | "warn" | "warnings" => "ostrzeżenie".into(),
        "busy" => "zajęty".into(),
        "hook" | "command" => "polecenie".into(),
        other => other.to_string(),
    }
}

/// Wpisy historii pasujące do filtra, od najnowszych.
pub fn query(state_dir: &Path, f: &Filter, limit: usize) -> anyhow::Result<Vec<HistoryRow>> {
    let c = open_db(state_dir)?;
    let mut sql = String::from("SELECT ts,job,kind,path,detail FROM history WHERE 1=1");
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(j) = f.job.as_deref().filter(|j| !j.is_empty()) {
        sql.push_str(" AND job=?");
        args.push(j.to_string().into());
    }
    if let Some(k) = f.kind.as_deref().filter(|k| !k.trim().is_empty()) {
        // fragment nazwy, bez rozróżniania wielkości liter; znaki LIKE traktujemy dosłownie
        sql.push_str(" AND instr(lower(kind), ?) > 0");
        args.push(kind_pattern(k).to_lowercase().into());
    }
    if let Some(t) = f.since {
        sql.push_str(" AND ts>=?");
        args.push(t.into());
    }
    if let Some(t) = f.until {
        sql.push_str(" AND ts<=?");
        args.push(t.into());
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    args.push((limit.min(MAX_ROWS) as i64).into());
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.iter()), |r| {
        Ok(HistoryRow {
            ts: r.get(0)?,
            job: r.get(1)?,
            kind: r.get(2)?,
            path: r.get(3)?,
            detail: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TopError {
    pub message: String,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JobSummary {
    pub job: String,
    pub name: String,
    pub copied: usize,
    pub deleted: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub total: usize,
    /// Przebiegi, które zostawiły ślad w historii (zmiany, konflikty albo błędy): różne pary (zadanie, czas).
    pub runs: usize,
    pub copied: usize,
    pub deleted: usize,
    pub moved: usize,
    pub conflicts: usize,
    pub resolved: usize,
    pub errors: usize,
    pub warnings: usize,
    pub busy: usize,
    /// Suma rozmiarów skopiowanych plików, dla których rozmiar jest znany.
    pub bytes_copied: u64,
    /// Ile skopiowanych plików miało znany rozmiar (reszta: wpisy sprzed 0.8).
    pub bytes_known_files: usize,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub top_errors: Vec<TopError>,
    pub by_job: Vec<JobSummary>,
}

fn parse_bytes(detail: &str) -> Option<u64> {
    detail.strip_suffix(" B")?.parse().ok()
}

fn is_op(kind: &str, what: &str) -> bool {
    kind == what || kind.starts_with(&format!("{what}:")) || kind.starts_with(&format!("{what} "))
}

/// Podsumowanie wierszy; `names` zamienia identyfikatory zadań na nazwy.
pub fn summarize(rows: &[HistoryRow], names: &HashMap<String, String>) -> Summary {
    let mut s = Summary {
        total: rows.len(),
        ..Default::default()
    };
    let mut runs: BTreeSet<(&str, i64)> = BTreeSet::new();
    let mut errs: HashMap<String, usize> = HashMap::new();
    let mut jobs: BTreeMap<&str, JobSummary> = BTreeMap::new();
    for r in rows {
        s.first_ts = Some(s.first_ts.map_or(r.ts, |x| x.min(r.ts)));
        s.last_ts = Some(s.last_ts.map_or(r.ts, |x| x.max(r.ts)));
        let k = r.kind.as_str();
        let js = jobs.entry(r.job.as_str()).or_insert_with(|| JobSummary {
            job: r.job.clone(),
            name: names.get(&r.job).cloned().unwrap_or_else(|| r.job.clone()),
            ..Default::default()
        });
        if !k.starts_with("polecenie") {
            runs.insert((r.job.as_str(), r.ts));
        }
        if k.starts_with("kopiowanie") {
            s.copied += 1;
            js.copied += 1;
            if let Some(b) = parse_bytes(&r.detail) {
                s.bytes_copied = s.bytes_copied.saturating_add(b);
                s.bytes_known_files += 1;
            }
        } else if k == "usunięcie" {
            s.deleted += 1;
            js.deleted += 1;
        } else if k == "przeniesienie" {
            s.moved += 1;
        } else if k == "konflikt rozstrzygnięty" {
            s.resolved += 1;
        } else if is_op(k, "konflikt") {
            s.conflicts += 1;
        } else if k == "błąd" {
            s.errors += 1;
            js.errors += 1;
            let m: String = r.detail.chars().take(200).collect();
            *errs.entry(m).or_default() += 1;
        } else if k == "ostrzeżenie" {
            s.warnings += 1;
        } else if k == "zajęty" {
            s.busy += 1;
        }
    }
    s.runs = runs.len();
    let mut top: Vec<TopError> = errs
        .into_iter()
        .map(|(message, count)| TopError { message, count })
        .collect();
    top.sort_by(|a, b| b.count.cmp(&a.count).then(a.message.cmp(&b.message)));
    top.truncate(5);
    s.top_errors = top;
    let mut by: Vec<JobSummary> = jobs.into_values().collect();
    by.sort_by(|a, b| a.name.cmp(&b.name));
    s.by_job = by;
    s
}

pub fn fmt_ts(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Rozmiar w czytelnej postaci (KiB/MiB/…).
pub fn fmt_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Pole CSV: separator `;`, cudzysłowy podwajane; wartości zaczynające się od `= + - @` (lub tabulatora/CR)
/// dostają apostrof na początku, żeby Excel/LibreOffice nie wykonał ich jako formuły.
pub fn csv_field(v: &str) -> String {
    let mut s = v.to_string();
    if matches!(s.chars().next(), Some('=' | '+' | '-' | '@' | '\t' | '\r')) {
        s.insert(0, '\'');
    }
    if s.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

fn job_name<'a>(names: &'a HashMap<String, String>, id: &'a str) -> &'a str {
    names.get(id).map(String::as_str).unwrap_or(id)
}

pub fn to_csv(rows: &[HistoryRow], names: &HashMap<String, String>) -> String {
    let mut out = String::from("\u{feff}czas;zadanie;operacja;plik;szczegóły\r\n");
    for r in rows {
        let line = [
            fmt_ts(r.ts),
            job_name(names, &r.job).to_string(),
            r.kind.clone(),
            r.path.clone(),
            r.detail.clone(),
        ]
        .iter()
        .map(|v| csv_field(v))
        .collect::<Vec<_>>()
        .join(";");
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out
}

pub fn to_json(
    rows: &[HistoryRow],
    summary: &Summary,
    filter: &Filter,
    names: &HashMap<String, String>,
) -> String {
    let rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "ts": r.ts,
                "time": fmt_ts(r.ts),
                "job": r.job,
                "job_name": job_name(names, &r.job),
                "kind": r.kind,
                "path": r.path,
                "detail": r.detail,
            })
        })
        .collect();
    let v = serde_json::json!({
        "generated": chrono::Local::now().timestamp(),
        "filter": filter,
        "summary": summary,
        "rows": rows,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Ucieczka HTML (tekst i atrybuty).
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c if c.is_control() && c != '\n' && c != '\t' => o.push('?'),
            c => o.push(c),
        }
    }
    o
}

const CSS: &str = ":root{--bg:#fff;--fg:#1c2330;--mut:#5d6778;--card:#f3f5f9;--line:#d9dee7;--bad:#b3261e;--ok:#1b7a3e;--warn:#9a6700}\
@media (prefers-color-scheme:dark){:root{--bg:#14181f;--fg:#e6e9ef;--mut:#9aa4b5;--card:#1d232d;--line:#313a48;--bad:#ff8a80;--ok:#6ddc93;--warn:#f2c14e}}\
body{margin:0;padding:24px 16px;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,-apple-system,Segoe UI,Roboto,sans-serif}\
main{max-width:1100px;margin:0 auto}h1{font-size:22px;margin:0 0 4px}h2{font-size:16px;margin:28px 0 8px}\
.mut{color:var(--mut)}.cards{display:grid;grid-template-columns:repeat(auto-fit,minmax(130px,1fr));gap:10px;margin:16px 0}\
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:10px 12px}.card b{display:block;font-size:22px}\
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:5px 8px;border-bottom:1px solid var(--line);vertical-align:top;word-break:break-word}\
th{color:var(--mut);font-weight:600}.bad{color:var(--bad)}.ok{color:var(--ok)}.warn{color:var(--warn)}.wrap{overflow-x:auto}";

fn kind_class(k: &str) -> &'static str {
    if k.contains("błąd") {
        "bad"
    } else if k.contains("zajęty") || k.contains("konflikt") || k.contains("ostrzeżenie") {
        "warn"
    } else if k.contains("kopiowanie") {
        "ok"
    } else {
        ""
    }
}

pub fn to_html(
    rows: &[HistoryRow],
    summary: &Summary,
    filter: &Filter,
    names: &HashMap<String, String>,
) -> String {
    let mut h = String::new();
    h.push_str("<!doctype html>\n<html lang=\"pl\"><head><meta charset=\"utf-8\">");
    h.push_str("<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; img-src data:\">");
    h.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    h.push_str("<title>Raport Syncron</title><style>");
    h.push_str(CSS);
    h.push_str("</style></head><body><main>");
    h.push_str("<h1>Raport Syncron</h1>");
    let range = match (summary.first_ts, summary.last_ts) {
        (Some(a), Some(b)) => format!("{} – {}", fmt_ts(a), fmt_ts(b)),
        _ => "brak wpisów".to_string(),
    };
    let mut scope = Vec::new();
    if let Some(j) = &filter.job {
        scope.push(format!("zadanie: {}", job_name(names, j)));
    }
    if let Some(k) = &filter.kind {
        scope.push(format!("rodzaj: {k}"));
    }
    if let Some(t) = filter.since {
        scope.push(format!("od: {}", fmt_ts(t)));
    }
    if let Some(t) = filter.until {
        scope.push(format!("do: {}", fmt_ts(t)));
    }
    h.push_str(&format!(
        "<p class=\"mut\">Wygenerowano {} · zakres wpisów: {} · filtr: {}</p>",
        esc(&fmt_ts(chrono::Local::now().timestamp())),
        esc(&range),
        esc(&if scope.is_empty() {
            "brak".to_string()
        } else {
            scope.join(", ")
        })
    ));
    let size = if summary.bytes_known_files > 0 {
        fmt_bytes(summary.bytes_copied)
    } else {
        "—".to_string()
    };
    h.push_str("<div class=\"cards\">");
    for (label, val, cls) in [
        ("Przebiegi", summary.runs.to_string(), ""),
        ("Skopiowane", summary.copied.to_string(), "ok"),
        ("Rozmiar skopiowanych", size, ""),
        ("Usunięte", summary.deleted.to_string(), ""),
        ("Przeniesione", summary.moved.to_string(), ""),
        ("Konflikty", summary.conflicts.to_string(), "warn"),
        ("Błędy", summary.errors.to_string(), "bad"),
    ] {
        h.push_str(&format!(
            "<div class=\"card\"><span class=\"mut\">{}</span><b class=\"{cls}\">{}</b></div>",
            esc(label),
            esc(&val)
        ));
    }
    h.push_str("</div>");
    if !summary.top_errors.is_empty() {
        h.push_str("<h2>Najczęstsze błędy</h2><div class=\"wrap\"><table><tr><th>Komunikat</th><th>Liczba</th></tr>");
        for e in &summary.top_errors {
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                esc(&e.message),
                e.count
            ));
        }
        h.push_str("</table></div>");
    }
    if summary.by_job.len() > 1 {
        h.push_str("<h2>Według zadań</h2><div class=\"wrap\"><table><tr><th>Zadanie</th><th>Skopiowane</th><th>Usunięte</th><th>Błędy</th></tr>");
        for j in &summary.by_job {
            h.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&j.name),
                j.copied,
                j.deleted,
                j.errors
            ));
        }
        h.push_str("</table></div>");
    }
    h.push_str("<h2>Wpisy</h2><div class=\"wrap\"><table><tr><th>Czas</th><th>Zadanie</th><th>Operacja</th><th>Plik</th><th>Szczegóły</th></tr>");
    for r in rows.iter().take(HTML_ROWS) {
        h.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td class=\"mut\">{}</td></tr>",
            esc(&fmt_ts(r.ts)),
            esc(job_name(names, &r.job)),
            kind_class(&r.kind),
            esc(&r.kind),
            esc(&r.path),
            esc(&r.detail)
        ));
    }
    h.push_str("</table></div>");
    if rows.len() > HTML_ROWS {
        h.push_str(&format!(
            "<p class=\"mut\">Pokazano {HTML_ROWS} z {} wpisów (reszta jest w podsumowaniu; pełne dane: eksport CSV lub JSON).</p>",
            rows.len()
        ));
    }
    h.push_str("</main></body></html>\n");
    h
}

/// Renderuje raport w wybranym formacie.
pub fn render(
    format: Format,
    rows: &[HistoryRow],
    filter: &Filter,
    names: &HashMap<String, String>,
) -> String {
    match format {
        Format::Csv => to_csv(rows, names),
        Format::Json => to_json(rows, &summarize(rows, names), filter, names),
        Format::Html => to_html(rows, &summarize(rows, names), filter, names),
    }
}

/// Podsumowanie dla filtra (polecenie `report_summary`).
pub fn summary(
    state_dir: &Path,
    filter: &Filter,
    names: &HashMap<String, String>,
) -> anyhow::Result<Summary> {
    Ok(summarize(&query(state_dir, filter, MAX_ROWS)?, names))
}

/// Zapisuje raport do pliku; zwraca liczbę wpisów. Plik powstaje najpierw pod tymczasową nazwą (bez wpisów
/// częściowych po awarii), a rozszerzenie nie musi pasować do formatu.
pub fn export(
    state_dir: &Path,
    format: Format,
    filter: &Filter,
    names: &HashMap<String, String>,
    out: &Path,
) -> anyhow::Result<usize> {
    if out.as_os_str().is_empty() || out.is_dir() {
        anyhow::bail!("Podaj ścieżkę pliku docelowego raportu.");
    }
    let rows = query(state_dir, filter, MAX_ROWS)?;
    let text = render(format, &rows, filter, names);
    let tmp = out.with_extension("syncron-tmp");
    std::fs::write(&tmp, text.as_bytes())
        .map_err(|e| anyhow::anyhow!("Nie można zapisać raportu: {e}"))?;
    if let Err(e) = std::fs::rename(&tmp, out) {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("Nie można zapisać raportu: {e}");
    }
    Ok(rows.len())
}

/// `history_limit_days` z `settings.json` w folderze stanu (brak pliku/pola = 365; 0 = bez rotacji).
pub fn history_limit_days(state_dir: &Path) -> u32 {
    std::fs::read_to_string(state_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("history_limit_days").and_then(|d| d.as_u64()))
        .map(|d| d.min(u64::from(MAX_HISTORY_DAYS)) as u32)
        .unwrap_or(DEFAULT_HISTORY_DAYS)
}

/// Usuwa wpisy starsze niż `days` dni względem `now` (0 = nic nie usuwa). Zwraca liczbę usuniętych.
pub fn prune_history(c: &rusqlite::Connection, now: i64, days: u32) -> rusqlite::Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    c.execute(
        "DELETE FROM history WHERE ts < ?1",
        [now - i64::from(days) * 86_400],
    )
}

/// Rotacja historii na żądanie (np. przy starcie aplikacji): stosuje `history_limit_days`.
pub fn prune_now(state_dir: &Path) -> anyhow::Result<usize> {
    let c = open_db(state_dir)?;
    Ok(prune_history(
        &c,
        chrono::Local::now().timestamp(),
        history_limit_days(state_dir),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_protects_formulas() {
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("@x"), "'@x");
        assert_eq!(csv_field("a;b"), "\"a;b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("zwykły"), "zwykły");
    }

    #[test]
    fn since_parsing() {
        assert_eq!(
            parse_since("7d", 1_000_000).unwrap(),
            1_000_000 - 7 * 86_400
        );
        assert_eq!(parse_since("2h", 100_000).unwrap(), 100_000 - 7200);
        assert!(parse_since("abc", 0).is_err());
        assert!(parse_since("", 0).is_err());
        assert!(parse_since("2025-01-02", 0).is_ok());
    }

    #[test]
    fn html_escapes() {
        assert_eq!(esc("<b>&\"'"), "&lt;b&gt;&amp;&quot;&#39;");
    }
}
