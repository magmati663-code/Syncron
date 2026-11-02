//! Syncron 0.8: import ustawień FreeFileSync.
use syncron_core::ffs::{parse_ffs_named, parse_ffs_xml, read_ffs_file};
use syncron_core::model::{Compare, CustomAction, Symlinks, Versioning};
use syncron_core::plan::Mode;

const GUI: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<FreeFileSync XmlType="GUI" XmlFormat="17">
    <Compare>
        <Variant>TimeAndSize</Variant>
        <Symlinks>Follow</Symlinks>
        <IgnoreTimeShift>1</IgnoreTimeShift>
    </Compare>
    <Synchronize>
        <Variant>TwoWay</Variant>
        <DetectMovedFiles>true</DetectMovedFiles>
        <DeletionPolicy>Versioning</DeletionPolicy>
        <VersioningFolder Style="Replace" MaxAge="14" MaxCount="3">/kopie</VersioningFolder>
    </Synchronize>
    <Filter>
        <Include>
            <Item>*</Item>
        </Include>
        <Exclude>
            <Item>*\bin\</Item>
            <Item>*.tmp</Item>
            <Item>\System Volume Information\</Item>
        </Exclude>
        <SizeMin Unit="KB">4</SizeMin>
        <SizeMax Unit="MB">10</SizeMax>
        <TimeSpan Type="LastDays">30</TimeSpan>
    </Filter>
    <FolderPairs>
        <Pair>
            <Left>/home/jan/Zdjęcia &amp; filmy</Left>
            <Right>/mnt/kopia/Zdjęcia</Right>
        </Pair>
        <Pair>
            <Left>/home/jan/Dokumenty</Left>
            <Right>/mnt/kopia/Dokumenty</Right>
            <Synchronize><Variant>Mirror</Variant></Synchronize>
            <LocalFilter>
                <Exclude><Item>*.bak</Item></Exclude>
            </LocalFilter>
        </Pair>
        <Pair><Left></Left><Right></Right></Pair>
    </FolderPairs>
</FreeFileSync>"#;

#[test]
fn import_gui_dwie_pary() {
    let jobs = parse_ffs_named(GUI, Some("Kopie żółte")).unwrap();
    assert_eq!(jobs.len(), 2);
    let (a, b) = (&jobs[0], &jobs[1]);
    assert_eq!(a.name, "Kopie żółte (1)");
    assert_eq!(b.name, "Kopie żółte (2)");
    assert_ne!(a.id, b.id);
    assert_eq!(a.left, "/home/jan/Zdjęcia & filmy");
    assert_eq!(a.right, "/mnt/kopia/Zdjęcia");
    for j in &jobs {
        assert!(!j.enabled);
        assert!(j.run_before.is_none() && j.run_after.is_none());
        assert_eq!(j.symlinks, Symlinks::Follow);
        assert_eq!(j.time_shift_hours, 1);
        assert_eq!(j.min_size, Some(4096));
        assert_eq!(j.max_size, Some(10 * 1024 * 1024));
        assert_eq!(j.max_age_days, Some(30));
        assert!(j.include.is_empty(), "{:?}", j.include);
        assert!(j.exclude.contains(&"*.tmp".to_string()));
        assert!(j.exclude.contains(&"*/bin/*".to_string()));
        assert!(j
            .exclude
            .contains(&"System Volume Information/*".to_string()));
        assert_eq!(j.versioning, Versioning::Replace);
        assert_eq!(j.trash_days, 14);
        assert_eq!(j.max_versions, 3);
        assert!(j.detect_moves);
        j.validate().unwrap();
    }
    assert_eq!(a.mode, Mode::TwoWay);
    // para 2 nadpisuje wariant i dodaje lokalny filtr
    assert_eq!(b.mode, Mode::LeftToRight);
    assert!(!b.no_delete_right);
    assert!(b.exclude.contains(&"*.bak".to_string()));
    assert!(!a.exclude.contains(&"*.bak".to_string()));
}

#[test]
fn nazwy_bez_trzonu_pliku() {
    let jobs = parse_ffs_xml(GUI).unwrap();
    assert_eq!(jobs[0].name, "Zdjęcia & filmy ↔ Zdjęcia");
}

fn simple(sync: &str, compare: &str, extra: &str) -> String {
    format!(
        r#"<FreeFileSync XmlType="BATCH"><Compare>{compare}</Compare><Synchronize>{sync}</Synchronize>{extra}
        <FolderPairs><Pair><Left>/l</Left><Right>/r</Right></Pair></FolderPairs></FreeFileSync>"#
    )
}

#[test]
fn warianty_synchronizacji() {
    let mirror = &parse_ffs_xml(&simple("<Variant>Mirror</Variant>", "", "")).unwrap()[0];
    assert_eq!(mirror.mode, Mode::LeftToRight);
    assert!(!mirror.no_delete_right);
    let update = &parse_ffs_xml(&simple("<Variant>Update</Variant>", "", "")).unwrap()[0];
    assert_eq!(update.mode, Mode::Custom);
    assert!(update.no_delete_right);
    let two = &parse_ffs_xml(&simple("<Variant>TwoWay</Variant>", "", "")).unwrap()[0];
    assert_eq!(two.mode, Mode::TwoWay);
    // brak wariantu → najbezpieczniejszy dwustronny
    assert_eq!(
        parse_ffs_xml(&simple("", "", "")).unwrap()[0].mode,
        Mode::TwoWay
    );
    // domyślnie jak w FFS: przeniesienia wyłączone, kosz 30 dni
    assert!(!two.detect_moves);
}

#[test]
fn wariant_wlasny_kierunki() {
    let sync = r#"<Variant>Custom</Variant><CustomDirections>
        <LeftOnly>Right</LeftOnly><RightOnly>Right</RightOnly>
        <LeftNewer>Right</LeftNewer><RightNewer>Left</RightNewer>
        <Different>None</Different><Conflict>Left</Conflict></CustomDirections>"#;
    let j = &parse_ffs_xml(&simple(sync, "<Variant>TimeAndSize</Variant>", "")).unwrap()[0];
    assert_eq!(j.mode, Mode::Custom);
    let c = j.custom.unwrap();
    assert_eq!(c.left_only, CustomAction::CopyToRight);
    assert_eq!(c.right_only, CustomAction::DeleteRight);
    assert_eq!(c.left_newer, CustomAction::CopyToRight);
    assert_eq!(c.right_newer, CustomAction::CopyToLeft);
    // w trybie czas+rozmiar kategorię „różne” zasila Conflict
    assert_eq!(c.different, CustomAction::CopyToLeft);
    j.validate().unwrap();

    let sync2 = r#"<Variant>Custom</Variant><CustomDirections>
        <LeftOnly>Left</LeftOnly><RightOnly>Left</RightOnly><Different>Right</Different></CustomDirections>"#;
    let j = &parse_ffs_xml(&simple(sync2, "<Variant>Content</Variant>", "")).unwrap()[0];
    assert_eq!(j.compare, Compare::Content);
    let c = j.custom.unwrap();
    assert_eq!(c.left_only, CustomAction::DeleteLeft);
    assert_eq!(c.right_only, CustomAction::CopyToLeft);
    // w trybie zawartości „Different” dotyczy też nowszy/starszy
    assert_eq!(c.left_newer, CustomAction::CopyToRight);
    assert_eq!(c.right_newer, CustomAction::CopyToRight);
    j.validate().unwrap();
}

#[test]
fn tryb_porownania_i_tolerancja() {
    for (v, expect) in [
        ("TimeAndSize", Compare::TimeAndSize),
        ("FileSize", Compare::SizeOnly),
        ("Size", Compare::SizeOnly),
        ("Content", Compare::Content),
        ("ByContent", Compare::Content),
    ] {
        let j = &parse_ffs_xml(&simple("", &format!("<Variant>{v}</Variant>"), "")).unwrap()[0];
        assert_eq!(j.compare, expect, "{v}");
    }
    let j = &parse_ffs_xml(&simple("", "", "<FileTimeTolerance>5</FileTimeTolerance>")).unwrap()[0];
    assert_eq!(j.time_tolerance_secs, 5);
    let j =
        &parse_ffs_xml(&simple("", "", "<FileTimeTolerance>-3</FileTimeTolerance>")).unwrap()[0];
    assert_eq!(j.time_tolerance_secs, 0);
    let j = &parse_ffs_xml(&simple(
        "",
        "<IgnoreTimeShift>-1, 0, 2</IgnoreTimeShift>",
        "",
    ))
    .unwrap()[0];
    assert_eq!(j.time_shift_hours, 2);
    let j = &parse_ffs_xml(&simple("", "<IgnoreTimeShift>99</IgnoreTimeShift>", "")).unwrap()[0];
    assert_eq!(j.time_shift_hours, 23);
}

#[test]
fn polityka_usuwania() {
    let perm = &parse_ffs_xml(&simple(
        "<DeletionPolicy>Permanent</DeletionPolicy>",
        "",
        "",
    ))
    .unwrap()[0];
    assert_eq!(perm.trash_days, 0);
    let rec =
        &parse_ffs_xml(&simple("<DeletionPolicy>Recycler</DeletionPolicy>", "", "")).unwrap()[0];
    assert_eq!(rec.trash_days, 30);
    let v = &parse_ffs_xml(&simple(
        r#"<DeletionPolicy>Versioning</DeletionPolicy><VersioningFolder Style="TimeStamp (Folder)">/x</VersioningFolder>"#,
        "",
        "",
    ))
    .unwrap()[0];
    assert_eq!(v.versioning, Versioning::Folders);
    assert_eq!(v.trash_days, 30);
}

#[test]
fn filtry_maski_rozdzielone() {
    let f =
        "<Filter><Include>*.docx|*.xlsx\nfaktury\\</Include><Exclude>~*;*.log</Exclude></Filter>";
    let j = &parse_ffs_xml(&simple("", "", f)).unwrap()[0];
    assert!(j.include.contains(&"*.docx".to_string()));
    assert!(j.include.contains(&"*.xlsx".to_string()));
    assert!(j.include.contains(&"faktury/*".to_string()));
    assert!(j.exclude.contains(&"~*".to_string()) && j.exclude.contains(&"*.log".to_string()));
    // maski faktycznie działają w regułach zadania
    let rs = j.rules();
    let fi = |p: &str| syncron_core::rules::FileInfo {
        path: p.into(),
        size: 1,
    };
    assert!(rs.allows(&fi("a/b.docx")));
    assert!(rs.allows(&fi("faktury/2024/x.pdf")));
    assert!(!rs.allows(&fi("a/b.pdf")));
    assert!(!rs.allows(&fi("a/b.log")));
}

#[test]
fn bledne_pliki() {
    assert!(parse_ffs_xml("").is_err());
    assert!(parse_ffs_xml("to nie xml").is_err());
    assert!(parse_ffs_xml("<a><b></a>").is_err());
    let e = parse_ffs_xml("<Inny/>").unwrap_err();
    assert!(e.contains("FreeFileSync"), "{e}");
    let e = parse_ffs_xml("<FreeFileSync><FolderPairs/></FreeFileSync>").unwrap_err();
    assert!(e.contains("pary folderów"), "{e}");
    let e = parse_ffs_xml(
        "<FreeFileSync><FolderPairs><Pair><Left>/a</Left></Pair></FolderPairs></FreeFileSync>",
    )
    .unwrap_err();
    assert!(e.contains("brakuje"), "{e}");
    // identyczne foldery: walidacja zadania
    let e = parse_ffs_xml(
        "<FreeFileSync><FolderPairs><Pair><Left>/a</Left><Right>/a</Right></Pair></FolderPairs></FreeFileSync>",
    )
    .unwrap_err();
    assert!(e.contains("Para folderów nr 1"), "{e}");
}

#[test]
fn odporny_na_niebezpieczny_xml() {
    // DOCTYPE z encjami jest ignorowany, nie rozwijany
    let x = r#"<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "AAAA">]><FreeFileSync>
        <FolderPairs><Pair><Left>/&a;</Left><Right>/r</Right></Pair></FolderPairs></FreeFileSync>"#;
    let j = &parse_ffs_xml(x).unwrap()[0];
    assert_eq!(j.left, "/&a;");
    // zbyt głębokie zagnieżdżenie
    let deep = format!("{}{}", "<a>".repeat(500), "</a>".repeat(500));
    assert!(parse_ffs_xml(&deep).is_err());
}

#[test]
fn pliki_bom_utf16_i_polskie_znaki() {
    let dir = tempfile::tempdir().unwrap();
    let xml = GUI.replace("encoding=\"utf-8\"", "encoding=\"utf-16\"");
    // UTF-8 z BOM
    let p8 = dir.path().join("Kopie żółte.ffs_gui");
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend(GUI.as_bytes());
    std::fs::write(&p8, b).unwrap();
    let jobs = read_ffs_file(&p8).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].name, "Kopie żółte (1)");
    // UTF-16 LE z BOM
    let p16 = dir.path().join("utf16.ffs_batch");
    let mut b = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        b.extend(u.to_le_bytes());
    }
    std::fs::write(&p16, b).unwrap();
    let jobs = read_ffs_file(&p16).unwrap();
    assert_eq!(jobs[0].left, "/home/jan/Zdjęcia & filmy");
    assert_eq!(jobs[0].name, "utf16 (1)");
    // UTF-16 BE z BOM
    let pbe = dir.path().join("be.ffs_gui");
    let mut b = vec![0xFE, 0xFF];
    for u in xml.encode_utf16() {
        b.extend(u.to_be_bytes());
    }
    std::fs::write(&pbe, b).unwrap();
    assert_eq!(read_ffs_file(&pbe).unwrap().len(), 2);
    // brak pliku
    assert!(read_ffs_file(&dir.path().join("nie ma.ffs_gui")).is_err());
}
