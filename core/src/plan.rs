//! Planowanie synchronizacji: algorytm trójstronny (lewy, prawy, ostatnia migawka każdej strony).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub size: u64,
    pub mtime: i64, // sekundy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    LeftToRight,
    RightToLeft,
    TwoWay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Tylko dodawaj nowe pliki: nic nie nadpisuj i nic nie usuwaj.
    pub only_add: bool,
    pub conflict: Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    CopyToRight,
    CopyToLeft,
    DeleteLeft,
    DeleteRight,
    /// Plik przeniesiony/przemianowany po lewej: po prawej zmieniamy tylko ścieżkę starego pliku (`from` = klucz).
    RenameRight {
        from: String,
    },
    /// Odwrotnie: plik przeniesiony po prawej.
    RenameLeft {
        from: String,
    },
    /// Konflikt: `keep_both` zachowuje obie wersje, w przeciwnym razie czeka na użytkownika.
    Conflict {
        keep_both: bool,
    },
}

pub type Tree = BTreeMap<String, Meta>;

/// Tolerancja mtime (FAT/SMB zaokrąglają do 2 s).
const TOL: i64 = 2;

/// Czy dwie wersje pliku uznajemy za identyczne: ten sam rozmiar i mtime w tolerancji.
///
/// DECYZJA (T5-03): nie czytamy treści. Nowe pliki po obu stronach o tym samym rozmiarze
/// i czasie (lecz innej treści) są uznane za identyczne - tak samo jak w rsync (domyślnie
/// „quick check”). Weryfikacja hashem dotyczy kopiowania (opcja `verify`), nie porównania.
pub fn same(a: &Meta, b: &Meta) -> bool {
    a.size == b.size && a.mtime.abs_diff(b.mtime) <= TOL as u64
}

/// Zmiana względem migawki: nowy, zmieniony lub usunięty.
fn changed(x: Option<&Meta>, base: Option<&Meta>) -> bool {
    match (x, base) {
        (None, None) => false,
        (Some(a), Some(b)) => !same(a, b),
        _ => true,
    }
}

/// Zmiana istniejącego pliku względem migawki (usunięcie nie liczy się).
fn modified(x: Option<&Meta>, base: Option<&Meta>) -> bool {
    matches!((x, base), (Some(a), Some(b)) if !same(a, b))
}

fn resolve(l: &Meta, r: &Meta, c: Conflict) -> Action {
    let pick = |left: bool| {
        if left {
            Action::CopyToRight
        } else {
            Action::CopyToLeft
        }
    };
    match c {
        Conflict::Newer if l.mtime != r.mtime => pick(l.mtime > r.mtime),
        Conflict::Larger if l.size != r.size => pick(l.size > r.size),
        Conflict::LeftWins => Action::CopyToRight,
        Conflict::RightWins => Action::CopyToLeft,
        Conflict::KeepBoth => Action::Conflict { keep_both: true },
        _ => Action::Conflict { keep_both: false },
    }
}

/// Zwraca działania (tylko ścieżki wymagające pracy). Klucze powinny być znormalizowane (names::key).
pub fn plan(
    left: &Tree,
    right: &Tree,
    bl: &Tree,
    br: &Tree,
    o: Options,
) -> BTreeMap<String, Action> {
    let paths: BTreeSet<&String> = left
        .keys()
        .chain(right.keys())
        .chain(bl.keys())
        .chain(br.keys())
        .collect();
    let mut out = BTreeMap::new();
    for p in paths {
        let (l, r) = (left.get(p), right.get(p));
        if let Some(a) = decide(l, r, bl.get(p), br.get(p), &o) {
            if o.only_add {
                let ok = match a {
                    Action::CopyToRight => r.is_none(),
                    Action::CopyToLeft => l.is_none(),
                    _ => false,
                };
                if !ok {
                    continue;
                }
            }
            out.insert(p.clone(), a);
        }
    }
    out
}

fn decide(
    l: Option<&Meta>,
    r: Option<&Meta>,
    bl: Option<&Meta>,
    br: Option<&Meta>,
    o: &Options,
) -> Option<Action> {
    if let (Some(x), Some(y)) = (l, r) {
        if same(x, y) {
            return None;
        }
    }
    let (lc, rc) = (changed(l, bl), changed(r, br));
    match o.mode {
        Mode::LeftToRight => match (l, r) {
            (Some(_), Some(_)) if !lc && !rc => None,
            (Some(_), _) => Some(Action::CopyToRight),
            (None, Some(_)) if !o.no_delete_right => Some(Action::DeleteRight),
            _ => None,
        },
        Mode::RightToLeft => match (l, r) {
            (Some(_), Some(_)) if !lc && !rc => None,
            (_, Some(_)) => Some(Action::CopyToLeft),
            (Some(_), None) if !o.no_delete_left => Some(Action::DeleteLeft),
            _ => None,
        },
        Mode::TwoWay => match (l, r) {
            (None, None) => None,
            (Some(_), None) => {
                if bl.is_none() || modified(l, bl) || o.no_delete_left {
                    Some(Action::CopyToRight) // nowy, zmieniony, albo usunięcie nie propaguje się
                } else {
                    Some(Action::DeleteLeft)
                }
            }
            (None, Some(_)) => {
                if br.is_none() || modified(r, br) || o.no_delete_right {
                    Some(Action::CopyToLeft)
                } else {
                    Some(Action::DeleteRight)
                }
            }
            (Some(x), Some(y)) => match (lc, rc) {
                (false, false) => None,
                (true, false) => Some(Action::CopyToRight),
                (false, true) => Some(Action::CopyToLeft),
                (true, true) => Some(resolve(x, y, o.conflict)),
            },
        },
    }
}

/// Zabezpieczenie: true, gdy plan usuwa więcej niż `max_pct`% plików.
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
        Options {
            mode,
            no_delete_left: false,
            no_delete_right: false,
            only_add: false,
            conflict: Conflict::Newer,
        }
    }
    fn e() -> Tree {
        Tree::new()
    }

    #[test]
    fn two_way_new_files() {
        let l = tree(&[("żółw.txt", m(1, 10))]);
        let r = tree(&[("ąę.txt", m(2, 10))]);
        let a = plan(&l, &r, &e(), &e(), opts(Mode::TwoWay));
        assert_eq!(a["żółw.txt"], Action::CopyToRight);
        assert_eq!(a["ąę.txt"], Action::CopyToLeft);
    }

    #[test]
    fn two_way_delete_propagates() {
        let b = tree(&[("a", m(1, 10))]);
        let a = plan(&e(), &b, &b, &b, opts(Mode::TwoWay));
        assert_eq!(a["a"], Action::DeleteRight);
    }

    #[test]
    fn no_delete_right_restores_to_left() {
        let b = tree(&[("a", m(1, 10))]);
        let mut o = opts(Mode::TwoWay);
        o.no_delete_right = true;
        assert_eq!(plan(&e(), &b, &b, &b, o)["a"], Action::CopyToLeft);
    }

    #[test]
    fn conflict_newer_wins() {
        let b = tree(&[("a", m(1, 10))]);
        let l = tree(&[("a", m(2, 100))]);
        let r = tree(&[("a", m(3, 50))]);
        assert_eq!(
            plan(&l, &r, &b, &b, opts(Mode::TwoWay))["a"],
            Action::CopyToRight
        );
    }

    #[test]
    fn mtime_tolerance() {
        let l = tree(&[("a", m(1, 10))]);
        let r = tree(&[("a", m(1, 11))]);
        assert!(plan(&l, &r, &l, &r, opts(Mode::TwoWay)).is_empty());
    }

    #[test]
    fn remote_without_modtime_does_not_ping_pong() {
        // prawa strona nie zachowuje mtime: baza każdej strony jest osobna
        let l = tree(&[("a", m(5, 10))]);
        let r = tree(&[("a", m(5, 5000))]);
        assert!(plan(&l, &r, &l, &r, opts(Mode::TwoWay)).is_empty());
    }

    #[test]
    fn one_way_mirror_and_no_delete() {
        let r = tree(&[("x", m(1, 1))]);
        assert_eq!(
            plan(&e(), &r, &e(), &e(), opts(Mode::LeftToRight))["x"],
            Action::DeleteRight
        );
        let mut o = opts(Mode::LeftToRight);
        o.no_delete_right = true;
        assert!(plan(&e(), &r, &e(), &e(), o).is_empty());
    }

    #[test]
    fn only_add_never_overwrites() {
        let l = tree(&[("a", m(2, 100)), ("b", m(1, 1))]);
        let r = tree(&[("a", m(1, 10))]);
        let mut o = opts(Mode::LeftToRight);
        o.only_add = true;
        let a = plan(&l, &r, &e(), &e(), o);
        assert_eq!(a.len(), 1);
        assert_eq!(a["b"], Action::CopyToRight);
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
