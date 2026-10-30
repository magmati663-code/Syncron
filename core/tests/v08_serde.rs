//! Syncron 0.8: zgodność JSON z interfejsem (kontrakt) i rozmiary w planie.
use std::fs;
use syncron_core::engine::run_job;
use syncron_core::model::Job;

#[test]
fn planned_sizes_dla_podgladu() {
    let t = tempfile::tempdir().unwrap();
    let (l, r, s) = (
        t.path().join("L"),
        t.path().join("R"),
        t.path().join("state"),
    );
    fs::create_dir_all(&l).unwrap();
    fs::create_dir_all(&r).unwrap();
    fs::write(l.join("Zażółć.txt"), "12345").unwrap();
    let j: Job = serde_json::from_value(serde_json::json!({
        "id": "s", "name": "s", "left": l.to_str().unwrap(), "right": r.to_str().unwrap(),
        "mode": "LeftToRight", "conflict": "Newer", "schedule": {"type": "Manual"}
    }))
    .unwrap();
    let rep = run_job(&j, &s, true, false).unwrap();
    assert_eq!(rep.planned.len(), 1);
    let (k, _) = &rep.planned[0];
    assert_eq!(rep.planned_sizes[k], 5);
}

fn full() -> serde_json::Value {
    serde_json::json!({
        "id": "u", "name": "UI", "left": "/a", "right": "/b", "mode": "TwoWay", "conflict": "Newer",
        "schedule": {"type": "Manual"}, "compare": "TimeAndSize", "time_tolerance_secs": 2,
        "time_shift_hours": 0, "symlinks": "Skip", "ignore_hidden": false, "min_age_days": 0,
        "max_age_days": 36500, "detect_moves": true, "preserve_permissions": false,
        "versioning": "Folders", "custom": null, "run_before": null, "run_after": null,
        "run_timeout_secs": 300
    })
}

#[test]
fn json_zapisywany_przez_ui() {
    // wszystkie nowe pola, custom = null, zakres wieku 0..36500 (równe min i max też dozwolone)
    let j: Job = serde_json::from_value(full()).unwrap();
    j.validate().unwrap();
    let mut v = full();
    v["min_age_days"] = 36500.into();
    serde_json::from_value::<Job>(v)
        .unwrap()
        .validate()
        .unwrap();
    // tryb Custom z 5 kluczami
    let mut v = full();
    v["mode"] = "Custom".into();
    v["custom"] = serde_json::json!({"left_only":"Skip","right_only":"Skip","left_newer":"Skip","right_newer":"Skip","different":"Skip"});
    serde_json::from_value::<Job>(v)
        .unwrap()
        .validate()
        .unwrap();
    // zapis i odczyt zachowują kształt
    let j: Job = serde_json::from_value(full()).unwrap();
    let back = serde_json::to_value(&j).unwrap();
    for k in [
        "compare",
        "time_tolerance_secs",
        "time_shift_hours",
        "symlinks",
        "ignore_hidden",
        "min_age_days",
        "max_age_days",
        "detect_moves",
        "preserve_permissions",
        "versioning",
        "custom",
        "run_before",
        "run_after",
        "run_timeout_secs",
    ] {
        assert!(back.get(k).is_some(), "brak pola {k} w serializacji");
    }
}
