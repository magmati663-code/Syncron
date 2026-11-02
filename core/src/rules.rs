//! Reguły filtrowania plików.
use crate::names::is_temp;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Default)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub enum Rule {
    /// Maska glob (`*`, `?`) dopasowywana do nazwy lub całej ścieżki, bez rozróżniania wielkości liter.
    Glob(String),
    MinSize(u64),
    MaxSize(u64),
}

#[derive(Debug, Clone)]
pub struct RuleSet {
    pub include: Vec<Rule>, // pusty = wszystko
    pub exclude: Vec<Rule>,
    pub skip_temp: bool,
}

impl Default for RuleSet {
    fn default() -> Self {
        Self {
            include: vec![],
            exclude: vec![],
            skip_temp: true,
        }
    }
}

/// Dekoduje zawartość pliku tekstowego (np. `.syncignore`): UTF-8 (z BOM lub bez), UTF-16 LE/BE (BOM, np. z Notatnika
/// Windows); w pozostałych przypadkach (np. Windows-1250) dekodowanie z zamianą nieprawidłowych bajtów na U+FFFD, tak
/// by pozostałe linie nadal działały (zamiast ignorowania całego pliku).
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units = (0..b.len() / 2).map(|i| {
            let pair = [b[2 * i], b[2 * i + 1]];
            if le {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        });
        char::decode_utf16(units)
            .map(|r| r.unwrap_or('\u{fffd}'))
            .collect::<String>()
    };
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(b, true);
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(b, false);
    }
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(b).into_owned()
}

/// Reguły z pliku `.syncignore` (jedna maska na linię, `#` = komentarz, `nazwa/` = cały folder na dowolnej głębokości,
/// `/nazwa/` = folder tylko w korzeniu; wiodący `/` oznacza maskę względem korzenia i jest usuwany; BOM jest pomijany).
pub fn parse_ignore(text: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let l = line.trim().trim_start_matches('\u{feff}');
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let anchored = l.starts_with('/');
        let l = l.trim_start_matches('/');
        if l.is_empty() {
            continue;
        }
        match l.strip_suffix('/') {
            Some(dir) if !dir.is_empty() => {
                out.push(Rule::Glob(format!("{dir}/*")));
                if !anchored {
                    out.push(Rule::Glob(format!("*/{dir}/*")));
                }
            }
            Some(_) => {}
            None => out.push(Rule::Glob(l.to_string())),
        }
    }
    out
}

/// NFC + małe litery znak po znaku (jeden znak -> jeden znak; gdy małe litery rozbijają znak, np. `İ`, zostaje on sam).
fn fold(s: &str) -> Vec<char> {
    s.nfc()
        .map(|c| {
            let mut it = c.to_lowercase();
            match (it.next(), it.next()) {
                (Some(l), None) => l,
                _ => c,
            }
        })
        .collect()
}

/// Dopasowanie maski (`*` = dowolny ciąg, także `/`; `?` = jeden znak) bez rozróżniania wielkości liter, po
/// normalizacji NFC obu argumentów. Iteracyjne (cofanie do ostatniej gwiazdki): czas O(n·m), bez wykładniczego ReDoS.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p = fold(pattern);
    let t = fold(text);
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pozycja po gwiazdce we wzorcu, pozycja w tekście)
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            pi += 1;
            star = Some((pi, ti));
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            star = Some((sp, st + 1));
            pi = sp;
            ti = st + 1;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

impl Rule {
    fn matches(&self, f: &FileInfo) -> bool {
        match self {
            Rule::Glob(g) => {
                let name = f.path.rsplit('/').next().unwrap_or(&f.path);
                glob(g, name) || glob(g, &f.path)
            }
            Rule::MinSize(n) => f.size >= *n,
            Rule::MaxSize(n) => f.size <= *n,
        }
    }
}

impl RuleSet {
    pub fn allows(&self, f: &FileInfo) -> bool {
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        if self.skip_temp && is_temp(name) {
            return false;
        }
        if self.exclude.iter().any(|r| r.matches(f)) {
            return false;
        }
        self.include.is_empty() || self.include.iter().any(|r| r.matches(f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn f(p: &str, s: u64) -> FileInfo {
        FileInfo {
            path: p.into(),
            size: s,
        }
    }

    #[test]
    fn syncignore_parsing() {
        let rs = RuleSet {
            exclude: parse_ignore("# komentarz\n\n*.bak\nnode_modules/\n"),
            ..Default::default()
        };
        let f = |p: &str| FileInfo {
            path: p.into(),
            size: 1,
        };
        assert!(!rs.allows(&f("a/b.bak")));
        assert!(!rs.allows(&f("node_modules/x/y.js")));
        assert!(!rs.allows(&f("projekt/node_modules/x.js")));
        assert!(rs.allows(&f("projekt/src/x.js")));
    }

    #[test]
    fn glob_basic() {
        assert!(glob("*.DOCX", "Sprawozdanie.docx"));
        assert!(glob("faktura_??.pdf", "faktura_01.pdf"));
        assert!(!glob("*.pdf", "a.txt"));
    }

    #[test]
    fn rules() {
        let rs = RuleSet {
            exclude: vec![Rule::Glob("*.log".into()), Rule::MaxSize(10)],
            ..Default::default()
        };
        assert!(!rs.allows(&f("a/b.log", 1)));
        assert!(!rs.allows(&f("a/~$x.docx", 1)));
        assert!(rs.allows(&f("a/zażółć.txt", 11)));
        assert!(!rs.allows(&f("a/zażółć.txt", 5)));
    }
}
