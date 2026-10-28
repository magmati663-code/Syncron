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
}

impl Job {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::FileInfo;

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
