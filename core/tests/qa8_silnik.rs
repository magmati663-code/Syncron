//! QA8 / T1: silnik 0.8 - porównywanie, tolerancje, tryb własny, invarianty (zbieżność, idempotencja, brak utraty danych).
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{run_job, run_job_opts, RunOpts};
use syncron_core::model::Job;
use syncron_core::plan::{
    plan, plan_ext, same_c, Action, Cmp, Conflict, CustomAction, CustomRules, Meta, Mode, Options,
    PlanExt, Tree,
};

struct Env {
    _t: tempfile::TempDir,
    l: PathBuf,
    r: PathBuf,
    s: PathBuf,
}

fn setup() -> Env {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    Env { _t: t, l, r, s }
}

fn job(e: &Env, mode: &str, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "qa8", "name": "qa8", "left": e.l.to_str().unwrap(), "right": e.r.to_str().unwrap(),
        "mode": mode, "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2,
        "guard_pct": 100
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

fn write(p: &Path, c: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
}

fn m(size: u64, t: i64) -> Meta {
    Meta { size, mtime: t }
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

// ---------- konkretne scenariusze ----------

/// BUG T1-01: po `compare=Content` baza zapamiętuje „zrównany” (sztuczny) czas prawej strony. Przy następnej zmianie
/// LEWEJ strony prawy plik (nietknięty) wygląda na zmieniony, więc powstaje fałszywy konflikt zamiast kopiowania.
#[test]

fn content_edycja_lewej_po_zrownaniu_nie_jest_konfliktem() {
    let e = setup();
    write(&e.l.join("a.txt"), "xx");
    write(&e.r.join("a.txt"), "xx");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_700_000_000);
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"compare": "Content", "conflict": "Ask"}),
    );
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
    // edycja tylko po lewej
    write(&e.l.join("a.txt"), "yy");
    set_mtime(&e.l.join("a.txt"), 1_950_000_000);
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.conflicts.is_empty(), "fałszywy konflikt: {rep:?}");
    assert_eq!(fs::read_to_string(e.r.join("a.txt")).unwrap(), "yy");
}

/// Wariant groźniejszy: `RightWins` + fałszywy konflikt nadpisuje świeżą edycję lewej starą treścią prawej.
#[test]

fn content_edycja_lewej_nie_jest_cofana_przy_rightwins() {
    let e = setup();
    write(&e.l.join("a.txt"), "xx");
    write(&e.r.join("a.txt"), "xx");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_700_000_000);
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"compare": "Content", "conflict": "RightWins"}),
    );
    run_job(&j, &e.s, false, false).unwrap();
    write(&e.l.join("a.txt"), "yy");
    set_mtime(&e.l.join("a.txt"), 1_950_000_000);
    run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(
        fs::read_to_string(e.l.join("a.txt")).unwrap(),
        "yy",
        "edycja lewej cofnięta"
    );
}

/// BUG T1-02: `compare=Content` nie wykrywa różnicy treści, gdy rozmiar i czas są takie same (hash liczony tylko
/// wtedy, gdy czasy się różnią) - tryb „zawartość” zachowuje się jak czas+rozmiar.
#[test]

fn content_wykrywa_inna_tresc_przy_tym_samym_czasie_i_rozmiarze() {
    let e = setup();
    write(&e.l.join("a.txt"), "AAAA");
    write(&e.r.join("a.txt"), "BBBB");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    set_mtime(&e.r.join("a.txt"), 1_900_000_000);
    let j = job(&e, "LeftToRight", serde_json::json!({"compare": "Content"}));
    let rep = run_job(&j, &e.s, true, false).unwrap();
    assert_eq!(rep.planned.len(), 1, "{rep:?}");
}

/// BUG T1-03: `same_c` przepełnia `diff + 1800` dla skrajnych czasów (panika w debug, zawinięcie w release).
#[test]

fn same_c_skrajne_czasy_bez_paniki() {
    let c = Cmp {
        tol_secs: 2,
        shift_hours: 1,
        size_only: false,
    };
    let r = std::panic::catch_unwind(|| same_c(&m(1, i64::MIN), &m(1, i64::MAX), &c));
    assert_eq!(r.ok(), Some(false));
}

#[test]
fn same_c_symetria_i_zwrotnosc() {
    let mut s = 0x1234_5678_9abc_def1u64;
    let mut rnd = |n: u64| {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s % n
    };
    for _ in 0..20_000 {
        let tol = [0u32, 1, 2, 30, 3600, 86_400][rnd(6) as usize];
        let sh = rnd(24) as u32;
        let c = Cmp {
            tol_secs: tol,
            shift_hours: sh,
            size_only: rnd(4) == 0,
        };
        let a = m(rnd(3), rnd(200_000) as i64 - 100_000);
        let b = m(rnd(3), a.mtime + rnd(200_000) as i64 - 100_000);
        assert_eq!(same_c(&a, &b, &c), same_c(&b, &a, &c), "{a:?} {b:?} {c:?}");
        assert!(same_c(&a, &a, &c));
        // monotonia: większa tolerancja nigdy nie psuje równości
        let c2 = Cmp {
            tol_secs: tol.saturating_add(5),
            ..c
        };
        if same_c(&a, &b, &c) {
            assert!(same_c(&a, &b, &c2));
        }
        // większy shift też
        let c3 = Cmp {
            shift_hours: (sh + 1).min(23),
            ..c
        };
        if same_c(&a, &b, &c) {
            assert!(same_c(&a, &b, &c3));
        }
    }
}

#[test]
fn custom_granice_kategorii_przy_tolerancji() {
    use CustomAction::*;
    let rules = CustomRules {
        left_only: Skip,
        right_only: Skip,
        left_newer: CopyToRight,
        right_newer: CopyToLeft,
        different: DeleteLeft,
    };
    let none = Tree::new();
    for tol in [0u32, 2, 10] {
        let x = PlanExt {
            cmp: Cmp {
                tol_secs: tol,
                ..Cmp::default()
            },
            custom: rules,
        };
        let t = tol as i64;
        let cases = [
            (0i64, Some(DeleteLeft)),     // ten sam czas, inny rozmiar
            (t, Some(DeleteLeft)),        // dokładnie w tolerancji
            (t + 1, Some(CopyToRight)),   // lewy nowszy
            (-(t + 1), Some(CopyToLeft)), // prawy nowszy
        ];
        for (dt, want) in cases {
            let l: Tree = [("a".to_string(), m(1, 1000 + dt))].into();
            let r: Tree = [("a".to_string(), m(2, 1000))].into();
            let got = plan_ext(&l, &r, &none, &none, opts(Mode::Custom), x)
                .get("a")
                .cloned();
            let w = want.map(|a| match a {
                DeleteLeft => Action::DeleteLeft,
                CopyToRight => Action::CopyToRight,
                CopyToLeft => Action::CopyToLeft,
                _ => unreachable!(),
            });
            assert_eq!(got, w, "tol={tol} dt={dt}");
        }
    }
}

#[test]
fn tryb_wlasny_ignoruje_baze_i_conflict() {
    // reguły Custom nie zależą od migawki ani od polityki konfliktów
    use CustomAction::*;
    let x = PlanExt {
        cmp: Cmp::default(),
        custom: CustomRules {
            left_newer: CopyToRight,
            ..Default::default()
        },
    };
    let l: Tree = [("a".to_string(), m(2, 500))].into();
    let r: Tree = [("a".to_string(), m(3, 100))].into();
    let b: Tree = [("a".to_string(), m(1, 1))].into();
    for c in [
        Conflict::Ask,
        Conflict::KeepBoth,
        Conflict::LeftWins,
        Conflict::RightWins,
        Conflict::Larger,
    ] {
        let mut o = opts(Mode::Custom);
        o.conflict = c;
        assert_eq!(plan_ext(&l, &r, &b, &b, o, x)["a"], Action::CopyToRight);
        assert_eq!(plan_ext(&l, &r, &l, &r, o, x)["a"], Action::CopyToRight);
    }
}

#[test]
fn tryb_wlasny_all_skip_nic_nie_robi_w_silniku() {
    let e = setup();
    write(&e.l.join("a"), "1");
    write(&e.r.join("b"), "2");
    write(&e.l.join("c"), "3");
    write(&e.r.join("c"), "33");
    let j = job(&e, "Custom", serde_json::json!({"custom": {}}));
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty() && rep.copied == 0 && rep.deleted == 0);
    assert!(e.l.join("a").exists() && e.r.join("b").exists());
}

#[test]
fn tryb_wlasny_bez_regul_w_job_jest_bezpieczny() {
    // Custom bez `custom` (nie przeszedł walidacji, ale run_job jej nie woła): nie wolno nic robić
    let e = setup();
    write(&e.l.join("a"), "1");
    write(&e.r.join("b"), "2");
    let j = job(&e, "Custom", serde_json::json!({}));
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn detect_moves_false_kopiuje_i_usuwa_z_koszem() {
    let e = setup();
    write(&e.l.join("stary.bin"), "treść do przeniesienia");
    set_mtime(&e.l.join("stary.bin"), 1_900_000_000);
    let j = job(
        &e,
        "TwoWay",
        serde_json::json!({"detect_moves": false, "trash_days": 5}),
    );
    run_job(&j, &e.s, false, false).unwrap();
    fs::rename(e.l.join("stary.bin"), e.l.join("nowy.bin")).unwrap();
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!(rep.moved, 0);
    assert_eq!((rep.copied, rep.deleted), (1, 1), "{rep:?}");
    assert!(e.r.join("nowy.bin").exists() && !e.r.join("stary.bin").exists());
    // usunięty stary plik jest w koszu prawej strony
    assert!(walk(&e.r)
        .iter()
        .any(|p| p.contains(".syncron-trash") && p.ends_with("stary.bin")));
    // zbieżność
    let again = run_job(&j, &e.s, false, false).unwrap();
    assert!(again.planned.is_empty(), "{again:?}");
}

#[test]
fn skip_nie_psuje_zbieznosci_po_zdjeciu_wykluczenia() {
    let e = setup();
    for i in 0..6 {
        write(&e.l.join(format!("f{i}")), &format!("c{i}"));
    }
    write(&e.r.join("stary"), "s");
    let j = job(
        &e,
        "LeftToRight",
        serde_json::json!({"guard_pct": 100, "trash_days": 3}),
    );
    let skip: HashSet<String> = ["f1", "stary"].iter().map(|s| s.to_string()).collect();
    let rep = run_job_opts(
        &j,
        &e.s,
        RunOpts {
            skip,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rep.skipped, 2);
    assert!(!e.r.join("f1").exists() && e.r.join("stary").exists());
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert_eq!((rep.copied, rep.deleted), (1, 1), "{rep:?}");
    let rep = run_job(&j, &e.s, false, false).unwrap();
    assert!(rep.planned.is_empty());
}

fn walk(dir: &Path) -> Vec<String> {
    fn go(base: &Path, d: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(base, &p, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut v = Vec::new();
    go(dir, dir, &mut v);
    v.sort();
    v
}

// ---------- fuzz planera (czysta funkcja) ----------

struct Rng(u64);
impl Rng {
    fn n(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.n(100) < pct
    }
}

fn rand_tree(r: &mut Rng, keys: &[String], present_pct: u64) -> Tree {
    let mut t = Tree::new();
    for k in keys {
        if r.chance(present_pct) {
            t.insert(
                k.clone(),
                m(r.n(3) + 1, 10_000 + r.n(6) as i64 * 7_200 + r.n(3) as i64),
            );
        }
    }
    t
}

/// Odpowiednik zapisu stanu przez silnik po wykonaniu planu (dla symulacji): zwraca nowe (l, r, bl, br).
fn apply(
    l: &mut Tree,
    r: &mut Tree,
    bl: &mut Tree,
    br: &mut Tree,
    acts: &BTreeMap<String, Action>,
    cmp: &Cmp,
) {
    for (k, a) in acts {
        match a {
            Action::CopyToRight => {
                let v = l[k];
                r.insert(k.clone(), v);
                bl.insert(k.clone(), v);
                br.insert(k.clone(), v);
            }
            Action::CopyToLeft => {
                let v = r[k];
                l.insert(k.clone(), v);
                bl.insert(k.clone(), v);
                br.insert(k.clone(), v);
            }
            Action::DeleteLeft => {
                l.remove(k);
                bl.remove(k);
                br.remove(k);
            }
            Action::DeleteRight => {
                r.remove(k);
                bl.remove(k);
                br.remove(k);
            }
            Action::Conflict { .. } => {}
            Action::RenameLeft { .. } | Action::RenameRight { .. } => unreachable!(),
        }
    }
    // odświeżenie migawki jak w engine::run_core
    let keys: BTreeSet<String> = l.keys().chain(r.keys()).chain(bl.keys()).cloned().collect();
    for k in keys {
        if acts.contains_key(&k) {
            continue;
        }
        match (l.get(&k), r.get(&k)) {
            (Some(a), Some(b)) if same_c(a, b, cmp) => {
                bl.insert(k.clone(), *a);
                br.insert(k.clone(), *b);
            }
            (None, None) => {
                bl.remove(&k);
                br.remove(&k);
            }
            _ => {}
        }
    }
}

fn keys() -> Vec<String> {
    (0..14).map(|i| format!("k{i}")).collect()
}

fn rand_cmp(r: &mut Rng) -> Cmp {
    Cmp {
        tol_secs: [0u32, 2, 5][r.n(3) as usize],
        shift_hours: [0u32, 0, 1, 2][r.n(4) as usize],
        size_only: r.chance(20),
    }
}

#[test]
fn fuzz_planer_bezpieczenstwo_i_determinizm() {
    for seed in 1..=4000u64 {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let ks = keys();
        let (l, rt) = (rand_tree(&mut r, &ks, 60), rand_tree(&mut r, &ks, 60));
        let (bl, br) = (rand_tree(&mut r, &ks, 50), rand_tree(&mut r, &ks, 50));
        let mode = [
            Mode::LeftToRight,
            Mode::RightToLeft,
            Mode::TwoWay,
            Mode::Custom,
        ][r.n(4) as usize];
        let o = Options {
            mode,
            no_delete_left: r.chance(30),
            no_delete_right: r.chance(30),
            only_add: r.chance(25),
            conflict: [
                Conflict::Newer,
                Conflict::Larger,
                Conflict::LeftWins,
                Conflict::RightWins,
                Conflict::KeepBoth,
                Conflict::Ask,
            ][r.n(6) as usize],
        };
        let pick = |r: &mut Rng, l: &[CustomAction]| l[r.n(l.len() as u64) as usize];
        use CustomAction::*;
        let custom = CustomRules {
            left_only: pick(&mut r, &[Skip, CopyToRight, DeleteLeft]),
            right_only: pick(&mut r, &[Skip, CopyToLeft, DeleteRight]),
            left_newer: pick(
                &mut r,
                &[Skip, CopyToRight, CopyToLeft, DeleteLeft, DeleteRight],
            ),
            right_newer: pick(
                &mut r,
                &[Skip, CopyToRight, CopyToLeft, DeleteLeft, DeleteRight],
            ),
            different: pick(
                &mut r,
                &[Skip, CopyToRight, CopyToLeft, DeleteLeft, DeleteRight],
            ),
        };
        let x = PlanExt {
            cmp: rand_cmp(&mut r),
            custom,
        };
        let p1 = plan_ext(&l, &rt, &bl, &br, o, x);
        let p2 = plan_ext(&l, &rt, &bl, &br, o, x);
        assert_eq!(p1, p2, "seed {seed}: niedeterministyczny");
        for (k, a) in &p1 {
            let (lm, rm) = (l.get(k), rt.get(k));
            let ctx = format!(
                "seed {seed} {k} {a:?} mode={mode:?} l={lm:?} r={rm:?} bl={:?} br={:?} {o:?} {x:?}",
                bl.get(k),
                br.get(k)
            );
            match a {
                Action::CopyToRight => assert!(lm.is_some(), "{ctx}"),
                Action::CopyToLeft => assert!(rm.is_some(), "{ctx}"),
                Action::DeleteLeft => {
                    assert!(lm.is_some() && !o.no_delete_left && !o.only_add, "{ctx}");
                    // cel nigdy nie jest kasowany, jeśli wersja drugiej strony różni się i jest nowsza (tryby synchronizacji)
                    if mode == Mode::TwoWay {
                        assert!(rm.is_none(), "{ctx}");
                        let b = bl.get(k).expect("usunięcie bez migawki");
                        assert!(
                            same_c(lm.unwrap(), b, &x.cmp),
                            "usuwa zmodyfikowany plik: {ctx}"
                        );
                    }
                }
                Action::DeleteRight => {
                    assert!(rm.is_some() && !o.no_delete_right && !o.only_add, "{ctx}");
                    if mode == Mode::TwoWay {
                        assert!(lm.is_none(), "{ctx}");
                        let b = br.get(k).expect("usunięcie bez migawki");
                        assert!(
                            same_c(rm.unwrap(), b, &x.cmp),
                            "usuwa zmodyfikowany plik: {ctx}"
                        );
                    }
                }
                Action::Conflict { .. } => {
                    assert!(
                        mode == Mode::TwoWay && lm.is_some() && rm.is_some(),
                        "{ctx}"
                    );
                    assert!(!o.only_add, "{ctx}");
                }
                _ => panic!("{ctx}"),
            }
            if o.only_add {
                match a {
                    Action::CopyToRight => assert!(rm.is_none(), "only_add nadpisuje: {ctx}"),
                    Action::CopyToLeft => assert!(lm.is_none(), "only_add nadpisuje: {ctx}"),
                    _ => panic!("only_add: {ctx}"),
                }
            }
            // identyczne pliki nigdy nie są ruszane
            if let (Some(a1), Some(b1)) = (lm, rm) {
                assert!(!same_c(a1, b1, &x.cmp), "akcja dla identycznych: {ctx}");
            }
        }
    }
}

#[test]
fn fuzz_planer_zbieznosc_tryby_synchronizacji() {
    let mut conflicts_seen = 0;
    for seed in 1..=6000u64 {
        let mut r = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
        let ks = keys();
        let (mut l, mut rt) = (rand_tree(&mut r, &ks, 60), rand_tree(&mut r, &ks, 60));
        // spójna migawka: tylko pliki, które są po obu stronach i takie same
        let cmp = rand_cmp(&mut r);
        let (mut bl, mut br) = (Tree::new(), Tree::new());
        for k in &ks {
            if let (Some(a), Some(b)) = (l.get(k), rt.get(k)) {
                if same_c(a, b, &cmp) && r.chance(80) {
                    bl.insert(k.clone(), *a);
                    br.insert(k.clone(), *b);
                }
            }
        }
        // pliki z migawki, których nie ma już po jednej stronie, mogą być po drugiej zmienione (usunięcie vs edycja)
        for k in &ks {
            if r.chance(10) && !bl.contains_key(k) {
                if let Some(v) = l.get(k).or(rt.get(k)).copied() {
                    bl.insert(k.clone(), v);
                    br.insert(k.clone(), v);
                }
            }
        }
        let mode = [Mode::LeftToRight, Mode::RightToLeft, Mode::TwoWay][r.n(3) as usize];
        let o = Options {
            mode,
            no_delete_left: false,
            no_delete_right: false,
            only_add: false,
            conflict: [
                Conflict::Newer,
                Conflict::Larger,
                Conflict::LeftWins,
                Conflict::RightWins,
            ][r.n(4) as usize],
        };
        let x = PlanExt {
            cmp,
            custom: CustomRules::default(),
        };
        let p1 = plan_ext(&l, &rt, &bl, &br, o, x);
        let c1 = p1
            .values()
            .filter(|a| matches!(a, Action::Conflict { .. }))
            .count();
        conflicts_seen += c1;
        let ctx0 = format!("seed {seed} mode={mode:?} {cmp:?} {o:?}\nl={l:?}\nr={rt:?}\nbl={bl:?}\nbr={br:?}\np1={p1:?}");
        apply(&mut l, &mut rt, &mut bl, &mut br, &p1, &cmp);
        let p2 = plan_ext(&l, &rt, &bl, &br, o, x);
        // po wykonaniu planu zostają co najwyżej konflikty, które planer sam zostawił do ręcznej decyzji
        // (Newer/Larger przy remisie) albo pary nierozstrzygnięte (obie zmienione, ale już „takie same” nie są)
        for (k, a) in &p2 {
            assert!(
                matches!(a, Action::Conflict { .. }),
                "brak zbieżności w {k}: {a:?}\n{ctx0}\nl2={l:?}\nr2={rt:?}\np2={p2:?}"
            );
        }
    }
    assert!(conflicts_seen > 0);
}

/// Jednokierunkowe tryby (lustro): po jednym przebiegu cel jest identyczny ze źródłem (przy no_delete = brak
/// nadmiarowych usunięć), drugi plan pusty.
#[test]
fn fuzz_planer_lustro_zbieznosc() {
    for seed in 1..=3000u64 {
        let mut r = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) | 1);
        let ks = keys();
        let (mut l, mut rt) = (rand_tree(&mut r, &ks, 60), rand_tree(&mut r, &ks, 60));
        let (mut bl, mut br) = (rand_tree(&mut r, &ks, 40), rand_tree(&mut r, &ks, 40));
        let cmp = rand_cmp(&mut r);
        let mode = if r.chance(50) {
            Mode::LeftToRight
        } else {
            Mode::RightToLeft
        };
        let o = opts(mode);
        let x = PlanExt {
            cmp,
            custom: CustomRules::default(),
        };
        let p1 = plan_ext(&l, &rt, &bl, &br, o, x);
        apply(&mut l, &mut rt, &mut bl, &mut br, &p1, &cmp);
        let p2 = plan_ext(&l, &rt, &bl, &br, o, x);
        // Dowolna baza (także niespójna): lustro MUSI zbiec się w jednym przebiegu.
        assert!(
            p2.is_empty(),
            "seed {seed} mode={mode:?} {cmp:?}\nl={l:?}\nr={rt:?}\nbl={bl:?}\nbr={br:?}\np1={p1:?}\np2={p2:?}"
        );
    }
}

/// Tryb własny z regułami „zbieżnymi”: po co najwyżej 3 przebiegach plan jest pusty (bez zapętleń).
#[test]
fn fuzz_planer_tryb_wlasny_zbieznosc() {
    use CustomAction::*;
    for seed in 1..=5000u64 {
        let mut r = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1);
        let ks = keys();
        let (mut l, mut rt) = (rand_tree(&mut r, &ks, 60), rand_tree(&mut r, &ks, 60));
        let (mut bl, mut br) = (Tree::new(), Tree::new());
        let pick = |r: &mut Rng, l: &[CustomAction]| l[r.n(l.len() as u64) as usize];
        let custom = CustomRules {
            left_only: pick(&mut r, &[CopyToRight, DeleteLeft]),
            right_only: pick(&mut r, &[CopyToLeft, DeleteRight]),
            left_newer: pick(&mut r, &[CopyToRight, DeleteLeft, DeleteRight]),
            right_newer: pick(&mut r, &[CopyToLeft, DeleteLeft, DeleteRight]),
            different: pick(&mut r, &[CopyToRight, CopyToLeft, DeleteLeft, DeleteRight]),
        };
        let cmp = rand_cmp(&mut r);
        let o = Options {
            mode: Mode::Custom,
            no_delete_left: false,
            no_delete_right: false,
            only_add: false,
            conflict: Conflict::Newer,
        };
        let x = PlanExt { cmp, custom };
        let mut last = BTreeMap::new();
        for pass in 0..4 {
            let p = plan_ext(&l, &rt, &bl, &br, o, x);
            if p.is_empty() {
                last = p;
                break;
            }
            apply(&mut l, &mut rt, &mut bl, &mut br, &p, &cmp);
            last = p;
            assert!(
                pass < 3,
                "seed {seed}: brak zbieżności {custom:?} {cmp:?}\nl={l:?}\nr={rt:?}\nlast={last:?}"
            );
        }
        assert!(last.is_empty());
    }
}

#[test]
fn plan_zignorowane_pole_conflict_ask_nigdy_nie_nadpisuje() {
    // Ask: konflikt nigdy nie daje Copy/Delete
    let b: Tree = [("a".to_string(), m(1, 10))].into();
    let l: Tree = [("a".to_string(), m(2, 100))].into();
    let r: Tree = [("a".to_string(), m(3, 100))].into();
    let mut o = opts(Mode::TwoWay);
    o.conflict = Conflict::Ask;
    assert_eq!(
        plan(&l, &r, &b, &b, o)["a"],
        Action::Conflict { keep_both: false }
    );
    // Newer przy remisie czasu → ręczna decyzja
    o.conflict = Conflict::Newer;
    assert_eq!(
        plan(&l, &r, &b, &b, o)["a"],
        Action::Conflict { keep_both: false }
    );
}

/// Fuzz silnika na prawdziwych plikach: dwa przebiegi (drugi pusty, poza konfliktami) i brak utraty treści
/// (każda wersja treści jest żywa po którejś stronie albo w koszu).
#[cfg(target_os = "linux")]
#[test]
fn fuzz_silnik_idempotencja_i_brak_utraty_danych() {
    for seed in 1..=40u64 {
        for (mode, cmpmode) in [
            ("TwoWay", "TimeAndSize"),
            ("TwoWay", "Content"),
            ("LeftToRight", "TimeAndSize"),
        ] {
            let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let e = setup();
            let j = job(
                &e,
                mode,
                serde_json::json!({"compare": cmpmode, "trash_days": 5, "conflict": "KeepBoth"}),
            );
            let mut contents: BTreeSet<String> = BTreeSet::new();
            let mut ctr = 0;
            let mut ops: Vec<String> = Vec::new();
            for round in 0..3 {
                let mut touched: BTreeSet<String> = BTreeSet::new();
                for side in [&e.l, &e.r] {
                    for i in 0..6 {
                        let rel = format!("d{}/f{i}.txt", i % 2);
                        if r.chance(35) && touched.insert(rel.clone()) {
                            let p = side.join(&rel);
                            if r.chance(25) {
                                ops.push(format!("r{round} rm {}", p.display()));
                                let _ = fs::remove_file(&p);
                            } else {
                                ctr += 1;
                                let c = format!("s{seed}-{round}-{ctr}");
                                write(&p, &c);
                                set_mtime(&p, 1_700_000_000 + (round * 100 + ctr) * 10);
                                ops.push(format!("r{round} {} {c}", p.display()));
                                contents.insert(c);
                            }
                        }
                    }
                }
                let rep = run_job(&j, &e.s, false, true).unwrap();
                assert!(
                    rep.errors.is_empty(),
                    "seed {seed} {mode}/{cmpmode}: {rep:?}"
                );
                let again = run_job(&j, &e.s, false, true).unwrap();
                let left: Vec<_> = again
                    .planned
                    .iter()
                    .filter(|(_, a)| !matches!(a, Action::Conflict { .. }))
                    .filter(|(k, _)| !k.contains("(konflikt"))
                    .collect();
                assert!(
                    left.is_empty(),
                    "seed {seed} {mode}/{cmpmode} round {round}: {left:?}"
                );
            }
            let mut alive = BTreeSet::new();
            for side in [&e.l, &e.r] {
                for f in walk(side) {
                    if let Ok(c) = fs::read_to_string(side.join(&f)) {
                        alive.insert(c);
                    }
                }
            }
            let lost: Vec<_> = contents.difference(&alive).collect();
            assert!(
                lost.is_empty(),
                "seed {seed} {mode}/{cmpmode}: utracone {lost:?} ops={ops:?} L={:?} R={:?}",
                walk(&e.l),
                walk(&e.r)
            );
        }
    }
}

/// Dwa przebiegi w tej samej sekundzie nie gubią wersji w koszu (unikalne ścieżki kosza).
#[cfg(target_os = "linux")]
#[test]
fn kosz_dwa_przebiegi_w_tej_samej_sekundzie_nie_gubia_wersji() {
    let e = setup();
    let j = job(&e, "LeftToRight", serde_json::json!({"trash_days": 5}));
    write(&e.l.join("a.txt"), "LEWY-1");
    write(&e.r.join("a.txt"), "prawa-wersja-A");
    set_mtime(&e.l.join("a.txt"), 1_900_000_000);
    run_job(&j, &e.s, false, false).unwrap();
    write(&e.r.join("a.txt"), "prawa-B");
    write(&e.l.join("a.txt"), "LEWY-DWA-DLUZSZY");
    set_mtime(&e.l.join("a.txt"), 1_900_000_100);
    run_job(&j, &e.s, false, false).unwrap();
    let mut alive = Vec::new();
    for f in walk(&e.r) {
        alive.push(fs::read_to_string(e.r.join(f)).unwrap());
    }
    for want in ["prawa-wersja-A", "prawa-B"] {
        assert!(
            alive.iter().any(|c| c == want),
            "utracono {want}: {alive:?}"
        );
    }
}
