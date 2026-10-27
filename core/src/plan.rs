//! Planowanie synchronizacji: algorytm trójstronny (lewy, prawy, ostatnia migawka).
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub size: u64,
    pub mtime: i64, // sekundy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    LeftToRight,
    RightToLeft,
    TwoWay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Newer,
    Larger,
    LeftWins,
    RightWins,
    KeepBoth,
    Ask,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub mode: Mode,
    pub no_delete_left: bool,
    pub no_delete_right: bool,
    pub conflict: Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    CopyToRight,
    CopyToLeft,
    DeleteLeft,
    DeleteRight,
    /// Konflikt do rozstrzygnięcia przez użytkownika lub zachowania obu wersji.
    Conflict { keep_both: bool },
}

pub type Tree = BTreeMap<String, Meta>;

/// Tolerancja mtime (FAT/SMB zaokrąglają do 2 s).
const TOL: i64 = 2;

fn same(a: &Meta, b: &Meta) -> bool {
    a.size == b.size && (a.mtime - b.mtime).abs() <= TOL
}

fn changed(x: Option<&Meta>, base: Option<&Meta>) -> bool {
    match (x, base) {
        (None, None) => false,
        (Some(a), Some(b)) => !same(a, b),
        _ => true,
    }
}

/// Zmiana istniejącego pliku względem migawki (usunięcie nie liczy się).
fn lc2(x: Option<&Meta>, b: Option<&Meta>) -> bool {
    matches!((x, b), (Some(a), Some(c)) if !same(a, c))
}

fn resolve(l: &Meta, r: &Meta, c: Conflict) -> Action {
    match c {
        Conflict::Newer if l.mtime != r.mtime => pick(l.mtime > r.mtime),
        Conflict::Larger if l.size != r.size => pick(l.size > r.size),
        Conflict::LeftWins => Action::CopyToRight,
        Conflict::RightWins => Action::CopyToLeft,
        Conflict::KeepBoth => Action::Conflict { keep_both: true },
        _ => Action::Conflict { keep_both: false },
    }
}

fn pick(left: bool) -> Action {
    if left { Action::CopyToRight } else { Action::CopyToLeft }
}

/// Zwraca listę działań (tylko ścieżki wymagające pracy). Klucze powinny być znormalizowane (names::key).
pub fn plan(left: &Tree, right: &Tree, base: &Tree, o: Options) -> BTreeMap<String, Action> {
    let paths: BTreeSet<&String> = left.keys().chain(right.keys()).chain(base.keys()).collect();
    let mut out = BTreeMap::new();
    for p in paths {
        let (l, r, b) = (left.get(p), right.get(p), base.get(p));
        if let Some(a) = decide(l, r, b, &o) {
            out.insert(p.clone(), a);
        }
    }
    out
}

fn decide(l: Option<&Meta>, r: Option<&Meta>, b: Option<&Meta>, o: &Options) -> Option<Action> {
    if let (Some(x), Some(y)) = (l, r) {
        if same(x, y) {
            return None;
        }
    }
    match o.mode {
        Mode::LeftToRight => match (l, r) {
            (Some(_), _) => Some(Action::CopyToRight),
            (None, Some(_)) if !o.no_delete_right => Some(Action::DeleteRight),
            _ => None,
        },
        Mode::RightToLeft => match (l, r) {
            (_, Some(_)) => Some(Action::CopyToLeft),
            (Some(_), None) if !o.no_delete_left => Some(Action::DeleteLeft),
            _ => None,
        },
        Mode::TwoWay => {
            let (lc, rc) = (changed(l, b), changed(r, b));
            match (l, r) {
                (None, None) => None,
                (Some(_), None) => {
                    // nowy po lewej / zmieniony po lewej / usunięty po prawej
                    if b.is_none() || lc2(l, b) {
                        Some(Action::CopyToRight)
                    } else if o.no_delete_left {
                        Some(Action::CopyToRight) // usunięcie nie propaguje się, plik wraca
                    } else {
                        Some(Action::DeleteLeft)
                    }
                }
                (None, Some(_)) => {
                    if b.is_none() || lc2(r, b) {
                        Some(Action::CopyToLeft)
                    } else if o.no_delete_right {
                        Some(Action::CopyToLeft)
                    } else {
                        Some(Action::DeleteRight)
                    }
                }
                (Some(x), Some(y)) => match (lc, rc) {
                    (true, false) => Some(Action::CopyToRight),
                    (false, true) => Some(Action::CopyToLeft),
                    _ => Some(resolve(x, y, o.conflict)),
                },
            }
        }
    }
}

/// Zabezpieczenie: true, gdy plan usuwa zbyt dużo (>`max_pct`% plików) lub źródło jest nagle puste.
pub fn mass_delete_guard(actions: &BTreeMap<String, Action>, total: usize, max_pct: usize) -> bool {
    let dels = actions
        .values()
        .filter(|a| matches!(a, Action::DeleteLeft | Action::DeleteRight))
        .count();
    dels > 0 && total > 0 && dels * 100 > total * max_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(size: u64, t: i64) -> Meta {
        Meta { size, mtime: t }
    }
    fn tree(v: &[(&str, Meta)]) -> Tree {
        v.iter().map(|(k, x)| (k.to_string(), *x)).collect()
    }
    fn opts(mode: Mode) -> Options {
        Options { mode, no_delete_left: false, no_delete_right: false, conflict: Conflict::Newer }
    }

    #[test]
    fn two_way_new_files() {
        let l = tree(&[("żółw.txt", m(1, 10))]);
        let r = tree(&[("ąę.txt", m(2, 10))]);
        let a = plan(&l, &r, &Tree::new(), opts(Mode::TwoWay));
        assert_eq!(a["żółw.txt"], Action::CopyToRight);
        assert_eq!(a["ąę.txt"], Action::CopyToLeft);
    }

    #[test]
    fn two_way_delete_propagates() {
        let b = tree(&[("a", m(1, 10))]);
        let r = b.clone();
        let a = plan(&Tree::new(), &r, &b, opts(Mode::TwoWay));
        assert_eq!(a["a"], Action::DeleteRight);
    }

    #[test]
    fn no_delete_right_restores_to_left() {
        let b = tree(&[("a", m(1, 10))]);
        let mut o = opts(Mode::TwoWay);
        o.no_delete_right = true;
        let a = plan(&Tree::new(), &b.clone(), &b, o);
        assert_eq!(a["a"], Action::CopyToLeft);
    }

    #[test]
    fn conflict_newer_wins() {
        let b = tree(&[("a", m(1, 10))]);
        let l = tree(&[("a", m(2, 100))]);
        let r = tree(&[("a", m(3, 50))]);
        assert_eq!(plan(&l, &r, &b, opts(Mode::TwoWay))["a"], Action::CopyToRight);
    }

    #[test]
    fn mtime_tolerance() {
        let l = tree(&[("a", m(1, 10))]);
        let r = tree(&[("a", m(1, 11))]);
        assert!(plan(&l, &r, &l, opts(Mode::TwoWay)).is_empty());
    }

    #[test]
    fn one_way_mirror_and_no_delete() {
        let l = Tree::new();
        let r = tree(&[("x", m(1, 1))]);
        assert_eq!(plan(&l, &r, &Tree::new(), opts(Mode::LeftToRight))["x"], Action::DeleteRight);
        let mut o = opts(Mode::LeftToRight);
        o.no_delete_right = true;
        assert!(plan(&l, &r, &Tree::new(), o).is_empty());
    }

    #[test]
    fn guard() {
        let mut a = BTreeMap::new();
        for i in 0..5 {
            a.insert(i.to_string(), Action::DeleteRight);
        }
        assert!(mass_delete_guard(&a, 10, 25));
        assert!(!mass_delete_guard(&a, 100, 25));
    }
}
