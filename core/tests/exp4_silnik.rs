//! Ekspert 4: planer i silnik (przeniesienia z weryfikacją treści, weryfikacja przed usunięciem, open_db).
use std::fs;
use std::path::Path;
use syncron_core::engine::{open_db, run_job};
use syncron_core::model::Job;
use syncron_core::plan::{same, Meta};

fn job(l: &Path, r: &Path, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "e4", "name": "e4", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2,
        "guard_pct": 100
    });
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(v).unwrap()
}

fn put(p: &Path, c: &str, t: i64) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, c).unwrap();
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(t, 0)).unwrap();
}

#[test]
fn same_does_not_overflow() {
    let a = Meta {
        size: 1,
        mtime: i64::MAX,
    };
    let b = Meta {
        size: 1,
        mtime: i64::MIN,
    };
    assert!(!same(&a, &b));
}

#[test]
fn real_move_with_same_content_is_still_a_move() {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (t.path().join("L"), t.path().join("R"), t.path().join("s"));
    put(&l.join("a.txt"), "treść", 1_700_000_000);
    put(&l.join("z.txt"), "inne", 1_700_000_500);
    fs::create_dir_all(&r).unwrap();
    let j = job(&l, &r, serde_json::json!({}));
    run_job(&j, &s, false, true).unwrap();
    fs::rename(l.join("a.txt"), l.join("b.txt")).unwrap();
    let rep = run_job(&j, &s, false, true).unwrap();
    assert_eq!((rep.moved, rep.copied, rep.deleted), (1, 0, 0), "{rep:?}");
}

#[test]
fn delete_is_skipped_when_file_still_exists_on_other_side() {
    // plik wypadł ze skanu (reguła rozmiaru), ale istnieje: nie wolno go usuwać po drugiej stronie
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (t.path().join("L"), t.path().join("R"), t.path().join("s"));
    put(&l.join("a.txt"), "abc", 1_700_000_000);
    put(&l.join("b.txt"), "def", 1_700_000_001);
    fs::create_dir_all(&r).unwrap();
    let j = job(&l, &r, serde_json::json!({"max_size": 20}));
    run_job(&j, &s, false, true).unwrap();
    put(&l.join("a.txt"), &"x".repeat(100), 1_700_100_000);
    let rep = run_job(&j, &s, false, true).unwrap();
    assert!(r.join("a.txt").exists());
    assert!(
        rep.warnings.iter().any(|e| e.contains("ostrzeżenie")),
        "{rep:?}"
    );
}

#[test]
fn open_db_parallel_fresh() {
    for i in 0..20 {
        let t = tempfile::tempdir().unwrap();
        let st = t.path().join(format!("s{i}"));
        std::thread::scope(|sc| {
            let hs: Vec<_> = (0..4)
                .map(|_| sc.spawn(|| open_db(&st).map(|_| ())))
                .collect();
            for h in hs {
                h.join().unwrap().unwrap();
            }
        });
    }
}
