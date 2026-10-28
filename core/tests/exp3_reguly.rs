//! Testy eksperta 3: glob (wydajność, Unicode), .syncignore (kodowania), nazwy.
use std::time::{Duration, Instant};
use syncron_core::names::{conflict_name, conflict_name_n, invalid_on_windows, key};
use syncron_core::rules::{decode_text, glob, parse_ignore, FileInfo, RuleSet};

fn fi(p: &str) -> FileInfo {
    FileInfo {
        path: p.into(),
        size: 1,
    }
}

#[test]
fn glob_wydajnosc() {
    let t = Instant::now();
    assert!(!glob(&format!("{}b", "*a".repeat(200)), &"a".repeat(5000)));
    assert!(glob(
        &format!("{}b", "*a".repeat(200)),
        &format!("{}b", "a".repeat(5000))
    ));
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

#[test]
fn glob_semantyka() {
    assert!(glob("*", ""));
    assert!(glob("a*/b", "a/x/y/b"));
    assert!(!glob("?", ""));
    assert!(glob("A?C", "abc"));
    assert!(!glob("abc", "abcd"));
    assert!(glob("**.txt", "x.TXT"));
}

#[test]
fn syncignore_kodowania() {
    let u16le: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(
            "*.bak\nsekret.txt\n"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes()),
        )
        .collect();
    let u16be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain("*.bak\n".encode_utf16().flat_map(|u| u.to_be_bytes()))
        .collect();
    let bom: Vec<u8> = [0xEF, 0xBB, 0xBF].into_iter().chain(*b"*.bak\n").collect();
    for b in [&u16le[..], &u16be[..], &bom[..], b"*.bak\nx_\xb3.txt\n"] {
        let rs = RuleSet {
            exclude: parse_ignore(&decode_text(b)),
            ..Default::default()
        };
        assert!(!rs.allows(&fi("a.bak")), "{b:?}");
    }
}

#[test]
fn zakotwiczone_tylko_w_korzeniu() {
    let rs = RuleSet {
        exclude: parse_ignore("/prywatne/\n/plik.txt\n"),
        ..Default::default()
    };
    assert!(!rs.allows(&fi("prywatne/a")));
    assert!(rs.allows(&fi("x/prywatne/a")));
    assert!(!rs.allows(&fi("plik.txt")));
}

#[test]
fn max_size_max_nie_przepelnia() {
    let j: syncron_core::model::Job = serde_json::from_value(serde_json::json!({
        "id": "t", "name": "t", "left": "a", "right": "b", "mode": "TwoWay", "conflict": "Newer",
        "schedule": {"type": "Manual"}, "threads": 2, "max_size": u64::MAX
    }))
    .unwrap();
    assert!(j.rules().allows(&fi("a")));
}

#[test]
fn conflict_name_dlugie() {
    for n in [200usize, 231, 232, 240, 251, 254, 255] {
        for ch in ["a", "ł", "😀"] {
            let name = format!("{}.txt", ch.repeat(n / ch.len()));
            let c = conflict_name(&name, "2026-10-28");
            assert!(c.len() <= 255, "{n} {ch}: {}", c.len());
            assert!(c.ends_with(").txt"));
            assert!(conflict_name_n(&name, "2026-10-28", 7).len() <= 255);
        }
    }
    assert_ne!(
        conflict_name_n("a.txt", "S", 2),
        conflict_name("a.txt", "S")
    );
    assert_eq!(conflict_name("d/README", "X"), "d/README (konflikt X)");
}

#[test]
fn windows_rozszerzone() {
    assert!(invalid_on_windows("LPT¹"));
    assert!(invalid_on_windows("conout$.x"));
    assert!(!invalid_on_windows("console.txt"));
    assert!(!invalid_on_windows(&"a".repeat(255)));
}

#[test]
fn key_idempotentny() {
    for s in ["ΑΣ", "ħ", "ǅ", "İ", "ﬁ", "Ǆ"] {
        let k = key(s);
        assert_eq!(key(&k), k);
    }
}
