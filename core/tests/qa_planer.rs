//! QA tester 1: planer synchronizacji (plan.rs), wykrywanie przeniesień i blokady (engine.rs).
//! Testy oznaczone `#[ignore = "BUG: ..."]` odtwarzają znalezione błędy.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use syncron_core::engine::{run_job, Report};
use syncron_core::model::Job;
use syncron_core::plan::{
    mass_delete_guard, plan, same, Action, Conflict, Meta, Mode, Options, Tree,
};

// ---------------------------------------------------------------- pomocnicze

fn m(size: u64, t: i64) -> Meta {
    Meta { size, mtime: t }
}
fn tree(v: &[(&str, Meta)]) -> Tree {
    v.iter().map(|(k, x)| (k.to_string(), *x)).collect()
}
fn e() -> Tree {
    Tree::new()
}
fn opts(mode: Mode, conflict: Conflict) -> Options {
    Options {
        mode,
        no_delete_left: false,
        no_delete_right: false,
        only_add: false,
        conflict,
    }
}
fn tw() -> Options {
    opts(Mode::TwoWay, Conflict::Newer)
}

fn jobx(l: &Path, r: &Path, mode: &str, conflict: &str, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "qa", "name": "qa", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": mode, "conflict": conflict, "schedule": {"type": "Manual"}, "threads": 2
    });
    if let Some(o) = extra.as_object() {
        for (k, x) in o {
            v[k] = x.clone();
        }
    }
    serde_json::from_value(v).unwrap()
}
fn job(l: &Path, r: &Path, mode: &str) -> Job {
    jobx(l, r, mode, "Newer", serde_json::json!({}))
}

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
fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}
fn write(p: &Path, c: &str, mtime: i64) {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(p, c).unwrap();
    set_mtime(p, mtime);
}
fn run(j: &Job, env: &Env) -> Report {
    run_job(j, &env.s, false, false).unwrap()
}

/// Zawartość drzewa (bez kosza i plików tymczasowych): ścieżka -> treść.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for ent in fs::read_dir(dir).unwrap() {
            let ent = ent.unwrap();
            let name = ent.file_name().to_string_lossy().into_owned();
            if name.starts_with(".syncron-trash") || name.ends_with(".syncron.tmp") {
                continue;
            }
            let p = ent.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(
                    rel,
                    String::from_utf8_lossy(&fs::read(&p).unwrap()).into_owned(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Wszystkie treści plików w koszach obu stron.
fn trash_contents(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for ent in fs::read_dir(dir).unwrap() {
            let p = ent.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(String::from_utf8_lossy(&fs::read(&p).unwrap()).into_owned());
            }
        }
    }
    let mut out = vec![];
    for ent in fs::read_dir(root).unwrap() {
        let p = ent.unwrap().path();
        if p.is_dir()
            && p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".syncron-trash")
        {
            walk(&p, &mut out);
        }
    }
    out
}

// ============================================================ RUNDA 1: podstawy (czysty planer)

#[test]
fn r1_two_way_matrix_of_single_sided_changes() {
    let b = tree(&[("a", m(1, 100))]);
    // zmiana po lewej -> w prawo
    let l = tree(&[("a", m(2, 200))]);
    assert_eq!(plan(&l, &b, &b, &b, tw())["a"], Action::CopyToRight);
    // zmiana po prawej -> w lewo
    assert_eq!(plan(&b, &l, &b, &b, tw())["a"], Action::CopyToLeft);
    // usunięcie po lewej -> usuń po prawej
    assert_eq!(plan(&e(), &b, &b, &b, tw())["a"], Action::DeleteRight);
    // usunięcie po prawej -> usuń po lewej
    assert_eq!(plan(&b, &e(), &b, &b, tw())["a"], Action::DeleteLeft);
    // zmiana po lewej + usunięcie po prawej -> zmiana wygrywa (brak utraty danych)
    assert_eq!(plan(&l, &e(), &b, &b, tw())["a"], Action::CopyToRight);
    assert_eq!(plan(&e(), &l, &b, &b, tw())["a"], Action::CopyToLeft);
    // usunięty po obu stronach: brak działań
    assert!(plan(&e(), &e(), &b, &b, tw()).is_empty());
}

#[test]
fn r1_no_delete_flags_two_way() {
    let b = tree(&[("a", m(1, 100))]);
    let mut o = tw();
    o.no_delete_left = true;
    // usunięcie po prawej NIE może usunąć po lewej -> plik wraca na prawo
    assert_eq!(plan(&b, &e(), &b, &b, o)["a"], Action::CopyToRight);
    // ale usunięcie po lewej nadal się propaguje
    assert_eq!(plan(&e(), &b, &b, &b, o)["a"], Action::DeleteRight);
    let mut o = tw();
    o.no_delete_right = true;
    assert_eq!(plan(&e(), &b, &b, &b, o)["a"], Action::CopyToLeft);
    assert_eq!(plan(&b, &e(), &b, &b, o)["a"], Action::DeleteLeft);
}

#[test]
fn r1_no_delete_flags_one_way() {
    let r = tree(&[("x", m(1, 1))]);
    let l = tree(&[("y", m(1, 1))]);
    let mut o = opts(Mode::LeftToRight, Conflict::Newer);
    o.no_delete_left = true; // nie ma znaczenia dla L->P
    assert_eq!(plan(&e(), &r, &e(), &e(), o)["x"], Action::DeleteRight);
    o.no_delete_right = true;
    assert!(plan(&e(), &r, &e(), &e(), o).is_empty());
    // R->L
    let mut o = opts(Mode::RightToLeft, Conflict::Newer);
    assert_eq!(plan(&l, &e(), &e(), &e(), o)["y"], Action::DeleteLeft);
    o.no_delete_left = true;
    assert!(plan(&l, &e(), &e(), &e(), o).is_empty());
}

#[test]
fn r1_one_way_never_writes_to_source() {
    let l = tree(&[("a", m(1, 1)), ("b", m(2, 5))]);
    let r = tree(&[("b", m(3, 9)), ("c", m(1, 1))]);
    let a = plan(&l, &r, &e(), &e(), opts(Mode::LeftToRight, Conflict::Newer));
    for act in a.values() {
        assert!(
            !matches!(act, Action::CopyToLeft | Action::DeleteLeft),
            "{act:?}"
        );
    }
    let a = plan(&l, &r, &e(), &e(), opts(Mode::RightToLeft, Conflict::Newer));
    for act in a.values() {
        assert!(
            !matches!(act, Action::CopyToRight | Action::DeleteRight),
            "{act:?}"
        );
    }
}

#[test]
fn r1_conflict_strategies() {
    let b = tree(&[("a", m(1, 10))]);
    let l = tree(&[("a", m(5, 100))]);
    let r = tree(&[("a", m(9, 50))]);
    let run = |c| plan(&l, &r, &b, &b, opts(Mode::TwoWay, c))["a"].clone();
    assert_eq!(run(Conflict::Newer), Action::CopyToRight);
    assert_eq!(run(Conflict::Larger), Action::CopyToLeft);
    assert_eq!(run(Conflict::LeftWins), Action::CopyToRight);
    assert_eq!(run(Conflict::RightWins), Action::CopyToLeft);
    assert_eq!(
        run(Conflict::KeepBoth),
        Action::Conflict { keep_both: true }
    );
    assert_eq!(run(Conflict::Ask), Action::Conflict { keep_both: false });
}

#[test]
fn r1_conflict_tie_falls_back_to_ask() {
    let b = tree(&[("a", m(1, 10))]);
    let l = tree(&[("a", m(5, 100))]);
    let r = tree(&[("a", m(9, 100))]);
    // ten sam mtime, różny rozmiar, strategia "nowszy": nie zgadujemy
    assert_eq!(
        plan(&l, &r, &b, &b, tw())["a"],
        Action::Conflict { keep_both: false }
    );
    // ten sam rozmiar, różny mtime (>tolerancji), strategia "większy"
    let l = tree(&[("a", m(5, 100))]);
    let r = tree(&[("a", m(5, 200))]);
    assert_eq!(
        plan(&l, &r, &b, &b, opts(Mode::TwoWay, Conflict::Larger))["a"],
        Action::Conflict { keep_both: false }
    );
}

#[test]
fn r1_mtime_tolerance_boundaries() {
    assert!(same(&m(1, 10), &m(1, 12)));
    assert!(same(&m(1, 12), &m(1, 10)));
    assert!(!same(&m(1, 10), &m(1, 13)));
    assert!(!same(&m(1, 10), &m(2, 10)));
    // zmiana o 2 s względem bazy nie jest zmianą, o 3 s jest
    let b = tree(&[("a", m(1, 100))]);
    assert!(plan(&tree(&[("a", m(1, 102))]), &b, &b, &b, tw()).is_empty());
    assert_eq!(
        plan(&tree(&[("a", m(1, 103))]), &b, &b, &b, tw())["a"],
        Action::CopyToRight
    );
}

#[test]
fn r1_only_add_semantics() {
    let mut o = tw();
    o.only_add = true;
    let b = tree(&[("del", m(1, 1)), ("mod", m(1, 1))]);
    let l = tree(&[("new", m(1, 1)), ("mod", m(2, 50))]);
    let r = tree(&[("del", m(1, 1)), ("mod", m(1, 1)), ("rnew", m(3, 3))]);
    // "del" usunięty po lewej, "mod" zmieniony po lewej, "new"/"rnew" nowe
    let a = plan(&l, &r, &b, &b, o);
    assert_eq!(a.get("new"), Some(&Action::CopyToRight));
    assert_eq!(a.get("rnew"), Some(&Action::CopyToLeft));
    assert!(!a.contains_key("del"), "only_add nie usuwa");
    assert!(!a.contains_key("mod"), "only_add nie nadpisuje");
}

#[test]
fn r1_only_add_one_way_and_conflicts() {
    let mut o = opts(Mode::TwoWay, Conflict::KeepBoth);
    o.only_add = true;
    let b = tree(&[("a", m(1, 1))]);
    let l = tree(&[("a", m(2, 10))]);
    let r = tree(&[("a", m(3, 20))]);
    assert!(
        plan(&l, &r, &b, &b, o).is_empty(),
        "konflikt pomijany przy only_add"
    );
    let mut o = opts(Mode::RightToLeft, Conflict::Newer);
    o.only_add = true;
    let a = plan(
        &tree(&[("x", m(1, 1))]),
        &tree(&[("y", m(1, 1))]),
        &e(),
        &e(),
        o,
    );
    assert_eq!(a.len(), 1);
    assert_eq!(a["y"], Action::CopyToLeft);
}

#[test]
fn r1_guard_basic() {
    let mut a = BTreeMap::new();
    for i in 0..3 {
        a.insert(i.to_string(), Action::DeleteLeft);
    }
    a.insert("c".into(), Action::CopyToRight);
    assert!(mass_delete_guard(&a, 10, 25)); // 30% > 25%
    assert!(!mass_delete_guard(&a, 12, 25)); // dokładnie 25%
    assert!(!mass_delete_guard(&a, 0, 25));
    assert!(!mass_delete_guard(&BTreeMap::new(), 10, 0));
    assert!(
        mass_delete_guard(&a, 1000, 0),
        "próg 0% blokuje każde usunięcie"
    );
}

// ============================================================ RUNDA 2: przypadki brzegowe (planer)

#[test]
fn r2_zero_size_files_and_zero_mtime() {
    let l = tree(&[("pusty", m(0, 0))]);
    let a = plan(&l, &e(), &e(), &e(), tw());
    assert_eq!(a["pusty"], Action::CopyToRight);
    // po synchronizacji identyczne metadane -> nic
    assert!(plan(&l, &l, &l, &l, tw()).is_empty());
    // usunięcie pliku o rozmiarze 0
    assert_eq!(plan(&e(), &l, &l, &l, tw())["pusty"], Action::DeleteRight);
}

#[test]
fn r2_identical_metadata_different_bases_is_noop() {
    // bazy każdej strony niezależne (np. prawa strona bez mtime)
    let l = tree(&[("a", m(5, 10))]);
    let r = tree(&[("a", m(5, 5000))]);
    assert!(plan(&l, &r, &l, &r, tw()).is_empty());
    // zmiana po lewej względem własnej bazy
    let l2 = tree(&[("a", m(6, 20))]);
    assert_eq!(plan(&l2, &r, &l, &r, tw())["a"], Action::CopyToRight);
}

#[test]
fn r2_future_negative_mtime_ordering() {
    let b = tree(&[("a", m(1, 10))]);
    let far = i64::from(i32::MAX) * 2; // rok 2106
    let l = tree(&[("a", m(2, far))]);
    let r = tree(&[("a", m(3, 50))]);
    assert_eq!(plan(&l, &r, &b, &b, tw())["a"], Action::CopyToRight);
    // ujemne mtime
    let l = tree(&[("a", m(2, -5000))]);
    let r = tree(&[("a", m(3, -4000))]);
    assert_eq!(plan(&l, &r, &b, &b, tw())["a"], Action::CopyToLeft);
}

#[test]
fn bug_extreme_mtime_does_not_panic() {
    // mtime z rclone (`ModTime` rok 9999 / 0001) lub uszkodzonego FS: różnica może przekroczyć i64
    let l = tree(&[("a", m(1, i64::MAX))]);
    let r = tree(&[("a", m(1, i64::MIN))]);
    let res = std::panic::catch_unwind(|| plan(&l, &r, &e(), &e(), tw()));
    assert!(
        res.is_ok(),
        "panika (przepełnienie a.mtime - b.mtime) w plan::same"
    );
}

#[test]
fn r2_tolerance_is_not_transitive_two_way() {
    // lewa = baza+2, prawa = baza-2: obie "niezmienione", ale różnią się o 4 s -> brak działania
    let bl = tree(&[("a", m(1, 100))]);
    let l = tree(&[("a", m(1, 102))]);
    let r = tree(&[("a", m(1, 98))]);
    assert!(plan(&l, &r, &bl, &bl, tw()).is_empty());
}

#[test]
fn r2_one_way_unchanged_but_different_noop_and_changed_overwrites() {
    let o = opts(Mode::LeftToRight, Conflict::Newer);
    let l = tree(&[("a", m(5, 10))]);
    let r = tree(&[("a", m(5, 5000))]);
    assert!(plan(&l, &r, &l, &r, o).is_empty());
    // prawa zmodyfikowana ręcznie, lewa nie -> lustro nadpisuje prawą
    let r2 = tree(&[("a", m(7, 6000))]);
    assert_eq!(plan(&l, &r2, &l, &r, o)["a"], Action::CopyToRight);
}

#[test]
fn r2_huge_tree_performance() {
    let n = 200_000;
    let mut l = Tree::new();
    let mut r = Tree::new();
    for i in 0..n {
        l.insert(
            format!("d{}/f{i}.txt", i % 100),
            m(i as u64, 1_000 + i as i64),
        );
        if i % 2 == 0 {
            r.insert(
                format!("d{}/f{i}.txt", i % 100),
                m(i as u64, 1_000 + i as i64),
            );
        }
    }
    let t = std::time::Instant::now();
    let a = plan(&l, &r, &e(), &e(), tw());
    assert_eq!(a.len(), n / 2);
    assert!(
        t.elapsed().as_secs() < 10,
        "plan zbyt wolny: {:?}",
        t.elapsed()
    );
}

#[test]
fn r2_guard_edge_cases() {
    let mut a = BTreeMap::new();
    a.insert("x".to_string(), Action::DeleteRight);
    // jedyny plik w folderze: usunięcie = 100% -> blokada
    assert!(mass_delete_guard(&a, 1, 25));
    // próg 100: nic nie blokuje
    assert!(!mass_delete_guard(&a, 1, 100));
    // realistyczne skrajne wartości (u32::MAX % z UI, milion plików) nie przepełniają
    assert!(!mass_delete_guard(&a, 1_000_000, u32::MAX as usize));
}

// ============================================================ RUNDA 2: silnik (system plików)

#[test]
fn r2_move_detection_basic_and_idempotent() {
    let env = setup();
    write(&env.l.join("a/plik.bin"), "0123456789", 1_700_000_000);
    write(&env.l.join("inny1.txt"), "xx1", 1_700_000_001);
    write(&env.l.join("inny2.txt"), "xx22", 1_700_000_002);
    write(&env.l.join("inny3.txt"), "xx333", 1_700_000_003);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::create_dir_all(env.l.join("b")).unwrap();
    fs::rename(env.l.join("a/plik.bin"), env.l.join("b/nowy.bin")).unwrap();
    let rep = run(&j, &env);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
    let rep = run(&j, &env);
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn r2_move_on_right_side_detected() {
    let env = setup();
    write(&env.l.join("a.bin"), "0123456789", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::create_dir_all(env.r.join("nowy")).unwrap();
    fs::rename(env.r.join("a.bin"), env.r.join("nowy/b.bin")).unwrap();
    let rep = run(&j, &env);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert!(env.l.join("nowy/b.bin").exists() && !env.l.join("a.bin").exists());
    assert!(run(&j, &env).planned.is_empty());
}

#[test]
fn r2_zero_size_file_move_is_copy_plus_delete() {
    let env = setup();
    write(&env.l.join("pusty1"), "", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::rename(env.l.join("pusty1"), env.l.join("pusty2")).unwrap();
    let rep = run(&j, &env);
    assert_eq!(
        rep.moved, 0,
        "puste pliki nie są dopasowywane jako przeniesienie"
    );
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
    assert!(!env.r.join("pusty1").exists() && env.r.join("pusty2").exists());
    assert!(run(&j, &env).planned.is_empty());
}

#[test]
fn r2_move_left_while_right_modifies_keeps_both() {
    let env = setup();
    write(&env.l.join("a.txt"), "stare-tresc", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::rename(env.l.join("a.txt"), env.l.join("b.txt")).unwrap();
    write(&env.r.join("a.txt"), "NOWA-tresc-prawa", 1_700_100_000);
    run(&j, &env);
    run(&j, &env);
    let (sl, sr) = (snapshot(&env.l), snapshot(&env.r));
    assert_eq!(sl, sr, "brak konwergencji");
    assert!(
        sl.values().any(|v| v == "NOWA-tresc-prawa"),
        "utracono zmianę z prawej: {sl:?}"
    );
    assert!(sl.values().any(|v| v == "stare-tresc"));
}

#[test]
fn r2_modify_on_left_delete_on_right_restores() {
    let env = setup();
    write(&env.l.join("a.txt"), "v1", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    write(&env.l.join("a.txt"), "v2-dluzsze", 1_700_100_000);
    fs::remove_file(env.r.join("a.txt")).unwrap();
    run(&j, &env);
    assert_eq!(
        fs::read_to_string(env.r.join("a.txt")).unwrap(),
        "v2-dluzsze"
    );
    assert!(run(&j, &env).planned.is_empty());
}

#[test]
fn r2_zero_and_future_and_negative_mtime_files_converge() {
    let env = setup();
    write(&env.l.join("zero.txt"), "z", 0);
    write(&env.l.join("future.txt"), "f", 4_000_000_000);
    write(&env.l.join("neg.txt"), "n", -86_400);
    write(&env.r.join("rfuture.txt"), "rf", 4_100_000_000);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
    assert_eq!(snapshot(&env.l).len(), 4);
    let rep = run(&j, &env);
    assert!(rep.planned.is_empty(), "{rep:?}");
    assert!(rep.errors.is_empty(), "{rep:?}");
}

#[test]
fn r2_delete_then_recreate_same_name_other_content() {
    let env = setup();
    write(&env.l.join("a.txt"), "pierwszy", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::remove_file(env.l.join("a.txt")).unwrap();
    write(&env.l.join("a.txt"), "drugi-dluzszy", 1_700_200_000);
    run(&j, &env);
    assert_eq!(
        fs::read_to_string(env.r.join("a.txt")).unwrap(),
        "drugi-dluzszy"
    );
}

#[test]
fn r2_file_vs_directory_clash_no_panic_no_loss() {
    let env = setup();
    write(&env.l.join("x"), "plik", 1_700_000_000);
    write(&env.r.join("x/w.txt"), "w-prawa", 1_700_000_000);
    let j = job(&env.l, &env.r, "TwoWay");
    let res = std::panic::catch_unwind(|| run_job(&j, &env.s, false, false));
    assert!(res.is_ok(), "panika przy kolizji plik/katalog");
    // żadna z treści nie może zniknąć
    let all: Vec<String> = snapshot(&env.l)
        .into_values()
        .chain(snapshot(&env.r).into_values())
        .collect();
    assert!(all.iter().any(|v| v == "plik") && all.iter().any(|v| v == "w-prawa"));
}

#[test]
fn r2_case_collision_same_side_reports_error_no_loss() {
    let env = setup();
    write(&env.l.join("A.txt"), "duze", 1_700_000_000);
    write(&env.l.join("a.txt"), "male", 1_700_000_000);
    let j = job(&env.l, &env.r, "TwoWay");
    let rep = run(&j, &env);
    assert!(
        rep.errors.iter().any(|e| e.contains("kolizja")),
        "kolizja nazw musi być zgłoszona: {rep:?}"
    );
    // oba pliki lewej nadal istnieją
    assert_eq!(snapshot(&env.l).len(), 2);
}

#[test]
fn r2_one_way_dest_edit_overwritten_with_backup() {
    let env = setup();
    write(&env.l.join("a.txt"), "zrodlo", 1_700_000_000);
    let j = job(&env.l, &env.r, "LeftToRight");
    run(&j, &env);
    write(&env.r.join("a.txt"), "reczna-edycja-celu", 1_700_100_000);
    run(&j, &env);
    assert_eq!(fs::read_to_string(env.r.join("a.txt")).unwrap(), "zrodlo");
    assert!(trash_contents(&env.r).contains(&"reczna-edycja-celu".to_string()));
    assert!(run(&j, &env).planned.is_empty());
}

#[test]
fn r2_guard_blocks_single_file_delete_with_message() {
    let env = setup();
    write(&env.l.join("jedyny.txt"), "x", 1_700_000_000);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::remove_file(env.l.join("jedyny.txt")).unwrap();
    let rep = run(&j, &env);
    // lewa pusta, choć wcześniej były pliki -> blokada; plik po prawej przetrwał
    assert!(rep.blocked.is_some(), "{rep:?}");
    assert!(env.r.join("jedyny.txt").exists());
}

#[test]
fn r2_force_overrides_guard_and_uses_trash() {
    let env = setup();
    for i in 0..4 {
        write(
            &env.l.join(format!("f{i}")),
            &format!("tresc{i}"),
            1_700_000_000 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    for i in 0..4 {
        fs::remove_file(env.l.join(format!("f{i}"))).unwrap();
    }
    let rep = run_job(&j, &env.s, false, true).unwrap();
    assert_eq!(rep.deleted, 4, "{rep:?}");
    assert_eq!(trash_contents(&env.r).len(), 4);
}

// ---- BŁĘDY ----

/// T1-01: wykrywanie przeniesienia po samym rozmiarze + mtime (+-2 s) bez weryfikacji treści:
/// usunięty a.txt i NOWY, inny plik b.txt o tym samym rozmiarze i mtime -> prawa strona dostaje
/// STARĄ treść pod nową nazwą, a baza uznaje strony za zgodne (cicha rozbieżność, treść b.txt nie trafia na prawo).
#[test]
fn bug_move_false_positive_same_size_and_mtime() {
    let env = setup();
    write(&env.l.join("a.txt"), "AAAA", 1_700_000_000);
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::remove_file(env.l.join("a.txt")).unwrap();
    write(&env.l.join("b.txt"), "BBBB", 1_700_000_000); // inna treść, ten sam rozmiar i mtime
    run(&j, &env);
    run(&j, &env);
    assert_eq!(
        fs::read_to_string(env.r.join("b.txt")).unwrap(),
        "BBBB",
        "prawa strona ma niewłaściwą treść b.txt"
    );
}

/// T1-02: dwa konflikty tego samego pliku w jednym dniu (KeepBoth) – nazwa aliasu zawiera tylko datę,
/// więc druga kopia konfliktowa nadpisuje pierwszą na lewej stronie bez kopii zapasowej.
#[test]
fn bug_keep_both_same_day_overwrites_previous_alias() {
    let env = setup();
    for i in 0..4 {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    write(&env.l.join("a.txt"), "baza", 1_700_000_000);
    let j = jobx(
        &env.l,
        &env.r,
        "TwoWay",
        "KeepBoth",
        serde_json::json!({"trash_days": 0}),
    );
    run(&j, &env);
    // konflikt 1
    write(&env.l.join("a.txt"), "LEWA-1", 1_700_100_000);
    write(&env.r.join("a.txt"), "PRAWA-1-wersja", 1_700_100_005);
    run(&j, &env);
    run(&j, &env);
    // konflikt 2 tego samego dnia
    write(&env.l.join("a.txt"), "LEWA-2-xx", 1_700_200_000);
    write(
        &env.r.join("a.txt"),
        "PRAWA-2-wersja-dluzsza",
        1_700_200_005,
    );
    run(&j, &env);
    run(&j, &env);
    let all: Vec<String> = snapshot(&env.l)
        .into_values()
        .chain(snapshot(&env.r).into_values())
        .collect();
    for want in ["PRAWA-1-wersja", "LEWA-2-xx", "PRAWA-2-wersja-dluzsza"] {
        assert!(
            all.iter().any(|v| v == want),
            "utracono wersję {want}: {all:?}"
        );
    }
}

/// T1-03: tryb jednostronny L->P: pusta prawa strona (nowy dysk kopii) blokuje całe zadanie,
/// choć plan zawiera same kopiowania (nic nie jest usuwane).
#[test]
fn bug_one_way_empty_destination_is_blocked() {
    let env = setup();
    write(&env.l.join("a.txt"), "1", 1_700_000_000);
    write(&env.l.join("b.txt"), "22", 1_700_000_001);
    let j = job(&env.l, &env.r, "LeftToRight");
    run(&j, &env);
    for f in ["a.txt", "b.txt"] {
        fs::remove_file(env.r.join(f)).unwrap();
    }
    let rep = run(&j, &env);
    assert!(
        rep.blocked.is_none(),
        "kopia zapasowa nie odtwarza się po wyczyszczeniu celu: {rep:?}"
    );
    assert!(env.r.join("a.txt").exists());
}

// ============================================================ RUNDA 3: macierz opcji, losowość

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const NAMES: [&str; 7] = [
    "a.txt",
    "b.txt",
    "d1/c.txt",
    "d1/d.txt",
    "d2/e/f.txt",
    "żółć.txt",
    "g.bin",
];

/// Losowe operacje użytkownika na jednej stronie; treści mają stały rozmiar, mtime są unikalne.
fn random_ops(rng: &mut Rng, root: &Path, clock: &mut i64, tag: &str, n: u32) {
    for _ in 0..n {
        let name = NAMES[rng.below(NAMES.len() as u64) as usize];
        let p = root.join(name);
        *clock += 10;
        match rng.below(4) {
            0 | 1 => write(&p, &format!("{tag}{:010}", *clock), *clock),
            2 => {
                let _ = fs::remove_file(&p);
            }
            _ => {
                let to = root.join(NAMES[rng.below(NAMES.len() as u64) as usize]);
                if p.is_file() && !to.exists() {
                    fs::create_dir_all(to.parent().unwrap()).unwrap();
                    fs::rename(&p, &to).unwrap();
                }
            }
        }
    }
}

fn all_contents(env: &Env) -> Vec<String> {
    let mut v: Vec<String> = snapshot(&env.l).into_values().collect();
    v.extend(snapshot(&env.r).into_values());
    v.extend(trash_contents(&env.l));
    v.extend(trash_contents(&env.r));
    v
}

fn fuzz_two_way(conflict: &str, extra: serde_json::Value, seeds: std::ops::Range<u64>) {
    for seed in seeds {
        let env = setup();
        let j = jobx(&env.l, &env.r, "TwoWay", conflict, extra.clone());
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let mut clock = 1_700_000_000i64;
        for round in 0..4 {
            let n = 1 + rng.below(5) as u32;
            random_ops(&mut rng, &env.l, &mut clock, "L", n);
            let n = 1 + rng.below(5) as u32;
            random_ops(&mut rng, &env.r, &mut clock, "R", n);
            let before: Vec<String> = snapshot(&env.l)
                .into_values()
                .chain(snapshot(&env.r).into_values())
                .collect();
            let ctx = format!("ziarno {seed}, runda {round}, konflikt {conflict}");
            let rep = run_job(&j, &env.s, false, true).unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert!(rep.errors.is_empty(), "{ctx}: {rep:?}");
            if conflict != "Ask" {
                run_job(&j, &env.s, false, true).unwrap();
                run_job(&j, &env.s, false, true).unwrap();
                let (sl, sr) = (snapshot(&env.l), snapshot(&env.r));
                assert_eq!(sl, sr, "{ctx}: brak konwergencji");
                let rep = run_job(&j, &env.s, false, true).unwrap();
                assert!(
                    rep.planned.is_empty(),
                    "{ctx}: nieidempotentne {:?}",
                    rep.planned
                );
                // brak utraty danych: każda treść sprzed synchronizacji nadal istnieje (strony lub kosz)
                let after = all_contents(&env);
                for c in &before {
                    assert!(after.contains(c), "{ctx}: utracono treść {c}");
                }
            } else {
                // Ask: konflikty mogą zostać; nie wolno tracić danych
                let after = all_contents(&env);
                for c in &before {
                    assert!(after.contains(c), "{ctx}: utracono treść {c}");
                }
                // wyczyść konflikty, aby dalsze rundy miały sens
                for p in rep.conflicts.iter() {
                    let _ = fs::copy(env.l.join(p), env.r.join(p));
                    let _ = filetime::set_file_mtime(
                        env.r.join(p),
                        filetime::FileTime::from_last_modification_time(
                            &fs::metadata(env.l.join(p)).unwrap(),
                        ),
                    );
                }
                run_job(&j, &env.s, false, true).unwrap();
            }
        }
    }
}

#[test]
fn r3_fuzz_two_way_newer() {
    fuzz_two_way("Newer", serde_json::json!({}), 0..40);
}

#[test]
fn r3_fuzz_two_way_keep_both() {
    fuzz_two_way("KeepBoth", serde_json::json!({}), 100..140);
}

#[test]
fn r3_fuzz_two_way_left_wins_and_larger() {
    fuzz_two_way("LeftWins", serde_json::json!({}), 200..225);
    fuzz_two_way("RightWins", serde_json::json!({}), 300..325);
}

#[test]
fn r3_fuzz_two_way_larger_no_data_loss() {
    // stały rozmiar treści => remis => konflikt do decyzji (jak Ask); sprawdzamy brak utraty danych
    fuzz_two_way("Ask", serde_json::json!({"conflict": "Larger"}), 400..425);
}

#[test]
fn r3_fuzz_two_way_ask_no_data_loss() {
    fuzz_two_way("Ask", serde_json::json!({}), 500..520);
}

#[test]
fn r3_fuzz_two_way_no_trash_still_converges() {
    // bez kosza sprawdzamy tylko konwergencję i idempotencję (utrata nadpisanych wersji jest oczekiwana)
    for seed in 600..630u64 {
        let env = setup();
        let j = jobx(
            &env.l,
            &env.r,
            "TwoWay",
            "Newer",
            serde_json::json!({"trash_days": 0}),
        );
        let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
        let mut clock = 1_700_000_000i64;
        for round in 0..4 {
            random_ops(&mut rng, &env.l, &mut clock, "L", 4);
            random_ops(&mut rng, &env.r, &mut clock, "R", 4);
            for _ in 0..3 {
                run_job(&j, &env.s, false, true).unwrap();
            }
            assert_eq!(
                snapshot(&env.l),
                snapshot(&env.r),
                "ziarno {seed} runda {round}"
            );
            assert!(run_job(&j, &env.s, false, true).unwrap().planned.is_empty());
        }
    }
}

#[test]
fn r3_fuzz_one_way_mirror() {
    for (mode, flip) in [("LeftToRight", false), ("RightToLeft", true)] {
        for seed in 700..725u64 {
            let env = setup();
            let (src, dst) = if flip {
                (&env.r, &env.l)
            } else {
                (&env.l, &env.r)
            };
            let j = job(&env.l, &env.r, mode);
            let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
            let mut clock = 1_700_000_000i64;
            for round in 0..4 {
                random_ops(&mut rng, src, &mut clock, "S", 5);
                random_ops(&mut rng, dst, &mut clock, "D", 2); // ręczne zmiany celu
                for _ in 0..2 {
                    run_job(&j, &env.s, false, true).unwrap();
                }
                assert_eq!(
                    snapshot(src),
                    snapshot(dst),
                    "{mode} ziarno {seed} runda {round}"
                );
                assert!(run_job(&j, &env.s, false, true).unwrap().planned.is_empty());
            }
        }
    }
}

#[test]
fn r3_one_way_never_modifies_source() {
    for seed in 800..815u64 {
        let env = setup();
        let j = job(&env.l, &env.r, "LeftToRight");
        let mut rng = Rng(seed | 1);
        let mut clock = 1_700_000_000i64;
        for _ in 0..4 {
            random_ops(&mut rng, &env.l, &mut clock, "S", 4);
            random_ops(&mut rng, &env.r, &mut clock, "D", 4);
            let before = snapshot(&env.l);
            run_job(&j, &env.s, false, true).unwrap();
            assert_eq!(
                before,
                snapshot(&env.l),
                "ziarno {seed}: tryb L->P zmienił źródło"
            );
        }
    }
}

#[test]
fn r3_only_add_never_deletes_or_overwrites() {
    for mode in ["TwoWay", "LeftToRight", "RightToLeft"] {
        for seed in 900..915u64 {
            let env = setup();
            let j = jobx(
                &env.l,
                &env.r,
                mode,
                "Newer",
                serde_json::json!({"only_add": true}),
            );
            let mut rng = Rng(seed | 1);
            let mut clock = 1_700_000_000i64;
            for _ in 0..4 {
                random_ops(&mut rng, &env.l, &mut clock, "L", 4);
                random_ops(&mut rng, &env.r, &mut clock, "R", 4);
                let before_l = snapshot(&env.l);
                let before_r = snapshot(&env.r);
                run_job(&j, &env.s, false, true).unwrap();
                for (side, before, now) in [
                    ("L", before_l, snapshot(&env.l)),
                    ("R", before_r, snapshot(&env.r)),
                ] {
                    for (k, v) in &before {
                        assert_eq!(
                            now.get(k),
                            Some(v),
                            "{mode} ziarno {seed}: only_add zmienił/usunął {side}/{k}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn r3_no_delete_flags_never_delete_on_protected_side() {
    for (nl, nr) in [(true, false), (false, true), (true, true)] {
        for seed in 1000..1015u64 {
            let env = setup();
            let j = jobx(
                &env.l,
                &env.r,
                "TwoWay",
                "Newer",
                serde_json::json!({"no_delete_left": nl, "no_delete_right": nr}),
            );
            let mut rng = Rng(seed | 1);
            let mut clock = 1_700_000_000i64;
            // początkowa zgodność
            random_ops(&mut rng, &env.l, &mut clock, "L", 6);
            run_job(&j, &env.s, false, true).unwrap();
            for _ in 0..3 {
                // użytkownik usuwa tylko pliki (bez modyfikacji), po przeciwnej stronie chronionej
                for side in [&env.l, &env.r] {
                    for n in NAMES {
                        if rng.below(3) == 0 {
                            let _ = fs::remove_file(side.join(n));
                        }
                    }
                }
                let before = (snapshot(&env.l), snapshot(&env.r));
                run_job(&j, &env.s, false, true).unwrap();
                run_job(&j, &env.s, false, true).unwrap();
                let (al, ar) = (snapshot(&env.l), snapshot(&env.r));
                if nl {
                    for k in before.0.keys() {
                        assert!(
                            al.contains_key(k),
                            "no_delete_left: usunięto L/{k} (seed {seed})"
                        );
                    }
                }
                if nr {
                    for k in before.1.keys() {
                        assert!(
                            ar.contains_key(k),
                            "no_delete_right: usunięto R/{k} (seed {seed})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn r3_matrix_planner_idempotent_pure() {
    // Czysta symulacja planu: zastosuj plan do (L,R,bazy) i sprawdź, że kolejny plan jest pusty.
    let modes = [Mode::LeftToRight, Mode::RightToLeft, Mode::TwoWay];
    let confl = [
        Conflict::Newer,
        Conflict::Larger,
        Conflict::LeftWins,
        Conflict::RightWins,
        Conflict::KeepBoth,
        Conflict::Ask,
    ];
    for mode in modes {
        for c in confl {
            for flags in 0..8u8 {
                let o = Options {
                    mode,
                    no_delete_left: flags & 1 != 0,
                    no_delete_right: flags & 2 != 0,
                    only_add: flags & 4 != 0,
                    conflict: c,
                };
                for seed in 0..60u64 {
                    let mut rng = Rng(seed * 7919 + 13);
                    let mut gen = |rng: &mut Rng| -> Option<Meta> {
                        match rng.below(4) {
                            0 => None,
                            _ => Some(m(rng.below(3), (rng.below(4) * 10) as i64)),
                        }
                    };
                    let mut l = Tree::new();
                    let mut r = Tree::new();
                    let mut b = Tree::new();
                    for k in 0..6 {
                        let key = format!("k{k}");
                        let base = gen(&mut rng);
                        // L i R wynikają z bazy lub zmian
                        let mut lv = if rng.below(2) == 0 {
                            base
                        } else {
                            gen(&mut rng)
                        };
                        let mut rv = if rng.below(2) == 0 {
                            base
                        } else {
                            gen(&mut rng)
                        };
                        if base.is_none() && lv == rv {
                            lv = None;
                            rv = None;
                        }
                        if let Some(x) = base {
                            b.insert(key.clone(), x);
                        }
                        if let Some(x) = lv {
                            l.insert(key.clone(), x);
                        }
                        if let Some(x) = rv {
                            r.insert(key.clone(), x);
                        }
                    }
                    let a = plan(&l, &r, &b, &b, o);
                    // zastosuj
                    let (mut l2, mut r2, mut b2) = (l.clone(), r.clone(), b.clone());
                    let mut unresolved = false;
                    for (k, act) in &a {
                        match act {
                            Action::CopyToRight => {
                                r2.insert(k.clone(), l[k]);
                                b2.insert(k.clone(), l[k]);
                            }
                            Action::CopyToLeft => {
                                l2.insert(k.clone(), r[k]);
                                b2.insert(k.clone(), r[k]);
                            }
                            Action::DeleteLeft => {
                                l2.remove(k);
                                b2.remove(k);
                            }
                            Action::DeleteRight => {
                                r2.remove(k);
                                b2.remove(k);
                            }
                            Action::Conflict { keep_both: false } => unresolved = true,
                            Action::Conflict { keep_both: true } => {
                                // silnik: wersja prawa -> alias po lewej, lewa nadpisuje prawą
                                r2.insert(k.clone(), l[k]);
                                b2.insert(k.clone(), l[k]);
                            }
                            _ => unreachable!(),
                        }
                    }
                    // odśwież bazę dla zgodnych par (jak silnik)
                    for k in l2.keys().chain(r2.keys()).cloned().collect::<Vec<_>>() {
                        match (l2.get(&k), r2.get(&k)) {
                            (Some(x), Some(y)) if same(x, y) => {
                                b2.insert(k.clone(), *x);
                            }
                            (None, None) => {
                                b2.remove(&k);
                            }
                            _ => {}
                        }
                    }
                    let a2 = plan(&l2, &r2, &b2, &b2, o);
                    // jedyne dopuszczalne pozostałości: nierozstrzygnięte konflikty (Ask) i
                    // działania blokowane przez flagi/only_add (plan nie jest wtedy pusty, ale nic nie robi)
                    let leftover: Vec<_> = a2
                        .iter()
                        .filter(|(_, x)| !matches!(x, Action::Conflict { keep_both: false }))
                        .collect();
                    if !unresolved && !o.only_add {
                        assert!(
                            leftover.is_empty(),
                            "nieidempotentny plan {o:?} seed {seed}: {leftover:?}\nL={l:?}\nR={r:?}\nB={b:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn r3_trash_receives_deleted_and_overwritten_two_way() {
    let env = setup();
    for i in 0..4 {
        write(
            &env.l.join(format!("f{i}")),
            &format!("tresc{i}"),
            1_700_000_000 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    fs::remove_file(env.l.join("f0")).unwrap();
    write(&env.r.join("f1"), "nowa-wersja-1", 1_700_100_000);
    run(&j, &env);
    let tr = trash_contents(&env.r);
    assert!(
        tr.contains(&"tresc0".to_string()),
        "usunięty plik powinien być w koszu prawej: {tr:?}"
    );
    let tl = trash_contents(&env.l);
    assert!(
        tl.contains(&"tresc1".to_string()),
        "nadpisany plik powinien być w koszu lewej: {tl:?}"
    );
}

#[test]
fn r3_blocked_run_changes_nothing_and_repeats() {
    let env = setup();
    for i in 0..8 {
        write(
            &env.l.join(format!("f{i}")),
            &format!("t{i}"),
            1_700_000_000 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    for i in 0..5 {
        fs::remove_file(env.l.join(format!("f{i}"))).unwrap();
    }
    let before = snapshot(&env.r);
    for _ in 0..3 {
        let rep = run(&j, &env);
        assert!(rep.blocked.is_some());
        assert_eq!(rep.deleted + rep.copied + rep.moved, 0);
        assert_eq!(snapshot(&env.r), before);
    }
}

#[test]
fn r3_dry_run_has_no_side_effects() {
    let env = setup();
    write(&env.l.join("a"), "1", 1_700_000_000);
    write(&env.r.join("b"), "2", 1_700_000_000);
    let j = job(&env.l, &env.r, "TwoWay");
    let (bl, br) = (snapshot(&env.l), snapshot(&env.r));
    let rep = run_job(&j, &env.s, true, false).unwrap();
    assert_eq!(rep.planned.len(), 2);
    assert_eq!((bl, br), (snapshot(&env.l), snapshot(&env.r)));
    // po symulacji pełny przebieg nadal robi wszystko
    let rep = run(&j, &env);
    assert_eq!(rep.copied, 2);
}

#[test]
fn r3_many_files_roundtrip() {
    let env = setup();
    for i in 0..1500 {
        write(
            &env.l.join(format!("d{}/p{i}.txt", i % 30)),
            &format!("zawartosc {i}"),
            1_700_000_000 + i,
        );
    }
    let j = jobx(
        &env.l,
        &env.r,
        "TwoWay",
        "Newer",
        serde_json::json!({"threads": 8}),
    );
    let rep = run(&j, &env);
    assert_eq!(rep.copied, 1500, "{rep:?}");
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
    assert!(run(&j, &env).planned.is_empty());
}

// ============================================================ RUNDA 3: interakcje (reguły, przeniesienia, flagi)

fn filler(env: &Env, n: usize) {
    for i in 0..n {
        write(
            &env.l.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
}

#[test]
fn r3_mirror_mode_move_is_renamed() {
    let env = setup();
    write(&env.l.join("a.bin"), "0123456789", 1_700_000_000);
    filler(&env, 5);
    let j = job(&env.l, &env.r, "LeftToRight");
    run(&j, &env);
    fs::rename(env.l.join("a.bin"), env.l.join("b.bin")).unwrap();
    let rep = run(&j, &env);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
}

#[test]
fn r3_right_to_left_move_is_renamed() {
    let env = setup();
    write(&env.r.join("a.bin"), "0123456789", 1_700_000_000);
    for i in 0..5 {
        write(
            &env.r.join(format!("c{i}")),
            &format!("tr{i}{}", "x".repeat(i)),
            1_700_000_010 + i as i64,
        );
    }
    let j = job(&env.l, &env.r, "RightToLeft");
    run(&j, &env);
    fs::rename(env.r.join("a.bin"), env.r.join("b.bin")).unwrap();
    let rep = run(&j, &env);
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
    assert_eq!(snapshot(&env.l), snapshot(&env.r));
}

#[test]
fn r3_move_with_no_delete_never_removes_old_copy() {
    for (nl, nr) in [(false, true), (true, false), (true, true)] {
        let env = setup();
        write(&env.l.join("a.bin"), "0123456789", 1_700_000_000);
        filler(&env, 5);
        let j = jobx(
            &env.l,
            &env.r,
            "TwoWay",
            "Newer",
            serde_json::json!({"no_delete_left": nl, "no_delete_right": nr}),
        );
        run(&j, &env);
        fs::rename(env.l.join("a.bin"), env.l.join("b.bin")).unwrap();
        run(&j, &env);
        run(&j, &env);
        if nr {
            assert!(
                env.r.join("a.bin").exists(),
                "no_delete_right: przeniesienie skasowało stary plik po prawej"
            );
        }
        assert!(env.r.join("b.bin").exists());
        assert!(run(&j, &env).planned.is_empty());
    }
}

#[test]
fn r3_only_add_move_keeps_old_copy() {
    let env = setup();
    write(&env.l.join("a.bin"), "0123456789", 1_700_000_000);
    filler(&env, 5);
    let j = jobx(
        &env.l,
        &env.r,
        "LeftToRight",
        "Newer",
        serde_json::json!({"only_add": true}),
    );
    run(&j, &env);
    fs::rename(env.l.join("a.bin"), env.l.join("b.bin")).unwrap();
    run(&j, &env);
    assert!(
        env.r.join("a.bin").exists(),
        "only_add nie może przenosić/usuwać"
    );
    assert!(env.r.join("b.bin").exists());
}

#[test]
fn r3_syncignore_excluded_file_not_deleted_when_ignored_later() {
    let env = setup();
    write(&env.l.join("dane.csv"), "1,2,3", 1_700_000_000);
    filler(&env, 5);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    write(&env.l.join(".syncignore"), "*.csv\n", 1_700_000_100);
    run(&j, &env);
    assert!(env.l.join("dane.csv").exists() && env.r.join("dane.csv").exists());
}

/// T1-04: plik zmienia rozmiar tak, że wypada poza reguły rozmiaru (max_size) tylko po jednej stronie:
/// lewa kopia staje się "niewidoczna" i jest traktowana jak usunięta -> prawa kopia trafia do kosza.
#[test]
fn bug_file_growing_past_max_size_is_not_deleted_on_other_side() {
    let env = setup();
    write(&env.l.join("a.txt"), "abc", 1_700_000_000);
    filler(&env, 5);
    let j = jobx(
        &env.l,
        &env.r,
        "TwoWay",
        "Newer",
        serde_json::json!({"max_size": 20, "guard_pct": 100}),
    );
    run(&j, &env);
    assert!(env.r.join("a.txt").exists());
    write(&env.l.join("a.txt"), &"x".repeat(100), 1_700_100_000);
    run(&j, &env);
    assert!(
        env.r.join("a.txt").exists(),
        "prawa kopia została skasowana, bo lewa wypadła poza reguły rozmiaru"
    );
}

#[test]
fn r3_temp_suffix_rename_is_treated_as_delete_documented() {
    let env = setup();
    write(&env.l.join("a.txt"), "abc", 1_700_000_000);
    filler(&env, 5);
    let j = jobx(
        &env.l,
        &env.r,
        "TwoWay",
        "Newer",
        serde_json::json!({"guard_pct": 100}),
    );
    run(&j, &env);
    fs::rename(env.l.join("a.txt"), env.l.join("a.txt.tmp")).unwrap();
    run(&j, &env);
    assert!(!env.r.join("a.txt").exists());
    assert!(
        trash_contents(&env.r).contains(&"abc".to_string()),
        "usunięta kopia musi być w koszu"
    );
}

#[test]
fn r3_guard_pct_100_still_blocks_when_source_emptied() {
    let env = setup();
    filler(&env, 6);
    let j = jobx(
        &env.l,
        &env.r,
        "TwoWay",
        "Newer",
        serde_json::json!({"guard_pct": 100}),
    );
    run(&j, &env);
    for e in fs::read_dir(&env.l).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    let rep = run(&j, &env);
    assert!(rep.blocked.is_some(), "puste źródło musi blokować: {rep:?}");
    assert_eq!(snapshot(&env.r).len(), 6);
}

#[test]
fn r3_unavailable_side_is_error_not_deletion() {
    let env = setup();
    filler(&env, 6);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    let moved = env.l.with_file_name("L-odpiety");
    fs::rename(&env.l, &moved).unwrap();
    assert!(run_job(&j, &env.s, false, false).is_err());
    assert_eq!(snapshot(&env.r).len(), 6);
    fs::rename(&moved, &env.l).unwrap();
    let rep = run(&j, &env);
    assert!(rep.planned.is_empty(), "{rep:?}");
}

#[test]
fn r3_reset_state_then_rerun_does_not_delete() {
    let env = setup();
    write(&env.l.join("a"), "L-only", 1_700_000_000);
    write(&env.r.join("b"), "R-only", 1_700_000_000);
    filler(&env, 3);
    let j = job(&env.l, &env.r, "TwoWay");
    run(&j, &env);
    syncron_core::engine::reset_state(&env.s, "qa").unwrap();
    fs::remove_file(env.l.join("a")).unwrap();
    let rep = run(&j, &env);
    assert_eq!(rep.deleted, 0, "{rep:?}");
    assert!(
        env.l.join("a").exists(),
        "bez bazy plik musi wrócić, nie zostać skasowany"
    );
}
