//! Reguły filtrowania plików.
use crate::names::is_temp;

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
        Self { include: vec![], exclude: vec![], skip_temp: true }
    }
}

pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
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
        FileInfo { path: p.into(), size: s }
    }

    #[test]
    fn glob_basic() {
        assert!(glob("*.DOCX", "Sprawozdanie.docx"));
        assert!(glob("faktura_??.pdf", "faktura_01.pdf"));
        assert!(!glob("*.pdf", "a.txt"));
    }

    #[test]
    fn rules() {
        let rs = RuleSet { exclude: vec![Rule::Glob("*.log".into()), Rule::MaxSize(10)], ..Default::default() };
        assert!(!rs.allows(&f("a/b.log", 1)));
        assert!(!rs.allows(&f("a/~$x.docx", 1)));
        assert!(rs.allows(&f("a/zażółć.txt", 11)));
        assert!(!rs.allows(&f("a/zażółć.txt", 5)));
    }
}
