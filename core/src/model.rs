//! Model zadania synchronizacji i jego (de)serializacja.
use crate::plan::{Conflict, Mode, Options};
use crate::rules::{Rule, RuleSet};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Schedule {
    /// Tylko ręcznie.
    Manual,
    /// Natychmiast po zmianie (watcher) + okresowy polling dla stron bez zdarzeń (dyski sieciowe, chmury).
    Realtime {
        poll_secs: u64,
    },
    Interval {
        secs: u64,
    },
    Daily {
        hour: u32,
        minute: u32,
    },
}

fn d_true() -> bool {
    true
}
fn d_guard() -> u32 {
    25
}
fn d_trash() -> u32 {
    30
}
fn d_threads() -> usize {
    4
}

/// Warunki uruchamiania zadania.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Conditions {
    /// Tylko przy zasilaniu sieciowym (nie na baterii).
    #[serde(default)]
    pub ac_only: bool,
    /// Dozwolone nazwy sieci Wi-Fi (puste = dowolna).
    #[serde(default)]
    pub wifi: Vec<String>,
}

/// Limit prędkości transferu (KB/s; 0 = bez limitu).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bandwidth {
    /// Limit w godzinach `hours` (albo zawsze, gdy `hours` puste).
    #[serde(default)]
    pub kbps: u32,
    /// Limit poza godzinami `hours` (0 = bez limitu).
    #[serde(default)]
    pub off_kbps: u32,
    /// Godziny [od, do), w których obowiązuje `kbps`.
    #[serde(default)]
    pub hours: Option<[u32; 2]>,
}

impl Bandwidth {
    /// Limit obowiązujący teraz (KB/s, 0 = bez limitu).
    pub fn current_kbps(&self) -> u32 {
        use chrono::Timelike;
        match self.hours {
            None => self.kbps,
            Some([a, b]) => {
                let h = chrono::Local::now().hour();
                let inside = if a <= b {
                    h >= a && h < b
                } else {
                    h >= a || h < b
                };
                if inside {
                    self.kbps
                } else {
                    self.off_kbps
                }
            }
        }
    }

    /// Limit rozłożony na `n` równoległych transferów (każdy proces/wątek dostaje swoją część).
    pub fn split(&self, n: u32) -> Bandwidth {
        let n = n.max(1);
        let f = |k: u32| if k == 0 { 0 } else { (k / n).max(1) };
        Bandwidth {
            kbps: f(self.kbps),
            off_kbps: f(self.off_kbps),
            hours: self.hours,
        }
    }

    /// Wartość dla `rclone --bwlimit` (np. `512k` albo rozkład godzinowy `08:00,512k 18:00,off`).
    pub fn rclone_arg(&self) -> Option<String> {
        let k = |v: u32| {
            if v == 0 {
                "off".to_string()
            } else {
                format!("{v}k")
            }
        };
        match self.hours {
            _ if self.kbps == 0 && self.off_kbps == 0 => None,
            None => (self.kbps > 0).then(|| k(self.kbps)),
            Some([a, b]) => Some(format!(
                "{a:02}:00,{} {b:02}:00,{}",
                k(self.kbps),
                k(self.off_kbps)
            )),
        }
    }
}

/// Dodatkowe wyzwalacze uruchamiania (poza harmonogramem).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Triggers {
    /// Po podłączeniu dysku (zewnętrznego/sieciowego), na którym leży jedna z lokalizacji.
    #[serde(default)]
    pub on_drive: bool,
    /// Po wybudzeniu komputera z uśpienia.
    #[serde(default)]
    pub on_wake: bool,
    /// Po zmianie sieci (adresu IP).
    #[serde(default)]
    pub on_network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    /// Ścieżka lokalna/UNC albo `remote:ścieżka` (rclone: Google Drive, OneDrive, Bitrix24 WebDAV...).
    pub left: String,
    pub right: String,
    pub mode: Mode,
    #[serde(default)]
    pub no_delete_left: bool,
    #[serde(default)]
    pub no_delete_right: bool,
    #[serde(default)]
    pub only_add: bool,
    pub conflict: Conflict,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub min_size: Option<u64>,
    #[serde(default)]
    pub max_size: Option<u64>,
    pub schedule: Schedule,
    /// Okno czasowe [od_godziny, do_godziny); poza nim zadanie nie startuje.
    #[serde(default)]
    pub window: Option<[u32; 2]>,
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Blokada, gdy plan usuwa więcej niż tyle % plików.
    #[serde(default = "d_guard")]
    pub guard_pct: u32,
    /// Retencja kosza synchronizacji w dniach (0 = bez kosza).
    #[serde(default = "d_trash")]
    pub trash_days: u32,
    #[serde(default = "d_threads")]
    pub threads: usize,
    /// Windows: czytaj pliki zablokowane przez inne programy z migawki VSS (wymaga administratora).
    #[serde(default = "d_true")]
    pub use_vss: bool,
    /// Po kopiowaniu lokalnym porównaj sumę kontrolną (BLAKE3) źródła i zapisanego pliku.
    #[serde(default)]
    pub verify: bool,
    /// Ile poprzednich wersji pliku trzymać w koszu (0 = bez limitu, decyduje retencja w dniach).
    #[serde(default)]
    pub max_versions: u32,
    #[serde(default)]
    pub conditions: Conditions,
    #[serde(default)]
    pub bandwidth: Bandwidth,
    #[serde(default)]
    pub triggers: Triggers,
}

/// Znormalizowana lokalizacja do porównań: (nazwa konta rclone lub None, prefiks, komponenty).
type Norm = (Option<String>, String, Vec<String>);

/// Normalizacja leksykalna (bez dotykania dysku): separatory, końcowe `/`, `.`/`..`, wielkość liter (Windows/macOS).
fn normalize_location(s: &str, ci: bool) -> Norm {
    use crate::location::Location;
    let fold = |x: &str| if ci { x.to_lowercase() } else { x.to_string() };
    let s = s.trim();
    match Location::parse(s) {
        Location::Rclone(full) => {
            let (name, rest) = full.split_once(':').unwrap_or((&full, ""));
            let mut c: Vec<String> = Vec::new();
            for part in rest.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        c.pop();
                    }
                    p => c.push(p.to_string()),
                }
            }
            (Some(name.to_string()), String::new(), c)
        }
        Location::Local(_) => {
            let win_like = cfg!(windows)
                || s.starts_with("\\\\")
                || (s.len() >= 3
                    && s.as_bytes()[0].is_ascii_alphabetic()
                    && s.as_bytes()[1] == b':'
                    && s.as_bytes()[2] == b'\\');
            let mut t = s.strip_prefix("\\\\?\\").unwrap_or(s).to_string();
            if win_like {
                t = t.replace('\\', "/");
            }
            let mut prefix = String::new();
            let mut rest = t.as_str();
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                prefix = rest[..2].to_uppercase();
                rest = &rest[2..];
            }
            if let Some(r) = rest.strip_prefix("//") {
                prefix.push_str("//");
                rest = r;
            } else if let Some(r) = rest.strip_prefix('/') {
                prefix.push('/');
                rest = r;
            }
            let mut c: Vec<String> = Vec::new();
            for part in rest.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        if matches!(c.last(), Some(l) if l != "..") {
                            c.pop();
                        } else if prefix.is_empty() {
                            c.push("..".into());
                        }
                    }
                    p => c.push(fold(p)),
                }
            }
            (None, fold(&prefix), c)
        }
    }
}

fn is_prefix(a: &Norm, b: &Norm) -> bool {
    a.0 == b.0 && a.1 == b.1 && b.2.len() >= a.2.len() && b.2[..a.2.len()] == a.2[..]
}

impl Job {
    /// Sprawdza poprawność zadania (puste pola, takie same/zagnieżdżone lokalizacje, zakresy liczb).
    /// Zwraca komunikat po polsku. Wywoływane przy zapisie i imporcie zadań.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("Zadanie nie ma identyfikatora.".into());
        }
        if self.name.trim().is_empty() {
            return Err("Podaj nazwę zadania.".into());
        }
        if self.left.trim().is_empty() || self.right.trim().is_empty() {
            return Err("Podaj obie lokalizacje (lewą i prawą).".into());
        }
        let ci = cfg!(any(windows, target_os = "macos"));
        let (l, r) = (
            normalize_location(&self.left, ci),
            normalize_location(&self.right, ci),
        );
        if l == r {
            return Err("Lewa i prawa lokalizacja nie mogą być takie same.".into());
        }
        if is_prefix(&l, &r) {
            return Err("Prawa lokalizacja leży wewnątrz lewej — każde uruchomienie kopiowałoby folder w siebie. Wybierz rozłączne foldery.".into());
        }
        if is_prefix(&r, &l) {
            return Err("Lewa lokalizacja leży wewnątrz prawej — każde uruchomienie kopiowałoby folder w siebie. Wybierz rozłączne foldery.".into());
        }
        match &self.schedule {
            Schedule::Manual => {}
            Schedule::Realtime { poll_secs } if *poll_secs < 1 => {
                return Err("Interwał sprawdzania musi wynosić co najmniej 1 sekundę.".into())
            }
            Schedule::Interval { secs } if *secs < 1 => {
                return Err("Interwał harmonogramu musi wynosić co najmniej 1 sekundę.".into())
            }
            Schedule::Daily { hour, minute } if *hour >= 24 || *minute >= 60 => {
                return Err(format!(
                    "Nieprawidłowa godzina harmonogramu dziennego: {hour}:{minute:02} (godzina 0–23, minuta 0–59)."
                ))
            }
            _ => {}
        }
        if let Some([a, b]) = self.window {
            if a >= 24 || b >= 24 {
                return Err("Okno czasowe: godziny muszą mieścić się w zakresie 0–23.".into());
            }
        }
        if !(1..=100).contains(&self.guard_pct) {
            return Err("Zabezpieczenie przed masowym usuwaniem: wartość 1–100%.".into());
        }
        if !(1..=16).contains(&self.threads) {
            return Err("Liczba wątków musi mieścić się w zakresie 1–16.".into());
        }
        if self.trash_days > 36_500 {
            return Err("Retencja kosza: maksymalnie 36500 dni.".into());
        }
        let max_kbps = u32::MAX / 1024;
        if self.bandwidth.kbps > max_kbps || self.bandwidth.off_kbps > max_kbps {
            return Err(format!(
                "Limit prędkości jest zbyt duży (maksymalnie {max_kbps} KB/s)."
            ));
        }
        if let Some([a, b]) = self.bandwidth.hours {
            if a >= 24 || b >= 24 {
                return Err("Godziny limitu prędkości muszą mieścić się w zakresie 0–23.".into());
            }
        }
        if let (Some(min), Some(max)) = (self.min_size, self.max_size) {
            if min > max {
                return Err("Minimalny rozmiar pliku jest większy od maksymalnego.".into());
            }
        }
        Ok(())
    }
    pub fn options(&self) -> Options {
        Options {
            mode: self.mode,
            no_delete_left: self.no_delete_left,
            no_delete_right: self.no_delete_right,
            only_add: self.only_add,
            conflict: self.conflict,
        }
    }

    pub fn rules(&self) -> RuleSet {
        let mut rs = RuleSet {
            include: self
                .include
                .iter()
                .filter(|g| !g.trim().is_empty())
                .map(|g| Rule::Glob(g.trim().into()))
                .collect(),
            exclude: self
                .exclude
                .iter()
                .filter(|g| !g.trim().is_empty())
                .map(|g| Rule::Glob(g.trim().into()))
                .collect(),
            skip_temp: true,
        };
        if let Some(min) = self.min_size.filter(|m| *m > 0) {
            rs.exclude.push(Rule::MaxSize(min - 1));
        }
        if let Some(max) = self.max_size {
            rs.exclude.push(Rule::MinSize(max + 1));
        }
        rs
    }
}

/// Wczytuje zadania z tekstu pliku importu: usuwa BOM, waliduje każde zadanie (`Job::validate`)
/// i wyłącza je (`enabled = false`) — plik od osoby trzeciej nie może uruchomić synchronizacji bez wiedzy użytkownika.
/// Duplikaty identyfikatorów w pliku: wygrywa ostatnie wystąpienie.
pub fn parse_import(text: &str) -> Result<Vec<Job>, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut jobs: Vec<Job> = serde_json::from_str(text).map_err(|e| {
        format!(
            "To nie jest plik konfiguracji Syncron (błąd w wierszu {}, kolumnie {}).",
            e.line(),
            e.column()
        )
    })?;
    if jobs.is_empty() {
        return Err("Plik nie zawiera żadnych zadań.".into());
    }
    for (i, j) in jobs.iter().enumerate() {
        j.validate()
            .map_err(|e| format!("Zadanie nr {} („{}”): {e}", i + 1, j.name))?;
    }
    let mut out: Vec<Job> = Vec::new();
    for mut j in jobs.drain(..) {
        j.enabled = false;
        match out.iter().position(|x| x.id == j.id) {
            Some(p) => out[p] = j,
            None => out.push(j),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::FileInfo;

    #[test]
    fn bandwidth_math() {
        let b = Bandwidth {
            kbps: 800,
            off_kbps: 0,
            hours: Some([8, 18]),
        };
        assert_eq!(b.rclone_arg().unwrap(), "08:00,800k 18:00,off");
        assert_eq!(
            Bandwidth {
                kbps: 500,
                ..Default::default()
            }
            .rclone_arg()
            .unwrap(),
            "500k"
        );
        assert!(Bandwidth::default().rclone_arg().is_none());
        assert_eq!(b.split(4).kbps, 200);
        assert_eq!(
            Bandwidth {
                kbps: 3,
                ..Default::default()
            }
            .split(4)
            .kbps,
            1
        );
    }

    #[test]
    fn size_rules() {
        let mut j: Job = serde_json::from_str(
            r#"{"id":"1","name":"t","left":"a","right":"b","mode":"TwoWay","conflict":"Newer","schedule":{"type":"Manual"},"min_size":10,"max_size":100}"#,
        )
        .unwrap();
        j.exclude = vec!["*.log".into()];
        let rs = j.rules();
        let f = |p: &str, s| FileInfo {
            path: p.into(),
            size: s,
        };
        assert!(!rs.allows(&f("a.txt", 5)));
        assert!(rs.allows(&f("a.txt", 50)));
        assert!(!rs.allows(&f("a.txt", 500)));
        assert!(!rs.allows(&f("a.log", 50)));
        assert_eq!(j.guard_pct, 25);
    }
}
