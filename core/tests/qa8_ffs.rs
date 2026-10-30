//! QA8 / T4: import FreeFileSync (core/src/ffs.rs) - wrogie XML, realistyczne pliki, mapowanie, fuzz.
//! Testy `#[ignore = "BUG T4-n: ..."]` odtwarzają błędy (uruchom: `--ignored`).
use std::sync::mpsc;
use std::time::{Duration, Instant};
use syncron_core::engine::run_job;
use syncron_core::ffs::{parse_ffs_xml, read_ffs_file};
use syncron_core::model::{Compare, CustomAction};
use syncron_core::plan::{plan_ext, Action, Meta, Mode, Tree};
use syncron_core::rules::FileInfo;

fn pair(l: &str, r: &str) -> String {
    format!("<FreeFileSync><FolderPairs><Pair><Left>{l}</Left><Right>{r}</Right></Pair></FolderPairs></FreeFileSync>")
}

fn simple(sync: &str, cmp: &str, extra: &str) -> String {
    format!(
        "<FreeFileSync><Compare>{cmp}</Compare><Synchronize>{sync}</Synchronize>{extra}<FolderPairs><Pair><Left>/l</Left><Right>/r</Right></Pair></FolderPairs></FreeFileSync>"
    )
}

/// Wykonuje `f` w wątku z limitem czasu (zawieszony wątek jest porzucany).
fn timed<T: Send + 'static>(
    secs: u64,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<(T, Duration)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let t = Instant::now();
        let v = f();
        let _ = tx.send((v, t.elapsed()));
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

// ---------- wrogie XML (przechodzą) ----------

#[test]
fn billion_laughs_nie_rozwija_encji() {
    let mut dtd = String::from("<!DOCTYPE FreeFileSync [<!ENTITY a0 \"lol\">");
    for i in 1..10 {
        let p = i - 1;
        let refs = format!("&a{p};").repeat(10);
        dtd.push_str(&format!("<!ENTITY a{i} \"{refs}\">"));
    }
    dtd.push_str("]>");
    let x = format!(
        "<?xml version=\"1.0\"?>{dtd}<FreeFileSync><FolderPairs><Pair><Left>/a/&a9;</Left><Right>/b</Right></Pair></FolderPairs></FreeFileSync>"
    );
    let (r, d) = timed(10, move || parse_ffs_xml(&x)).expect("zawieszenie");
    assert!(d < Duration::from_secs(2));
    assert_eq!(r.unwrap()[0].left, "/a/&a9;");
}

#[test]
fn xxe_nie_czyta_plikow() {
    let t = tempfile::tempdir().unwrap();
    let secret = t.path().join("sekret.txt");
    std::fs::write(&secret, "TAJNE").unwrap();
    let x = format!(
        "<!DOCTYPE FreeFileSync [<!ENTITY x SYSTEM \"file://{}\">]><FreeFileSync><FolderPairs><Pair><Left>/a/&x;</Left><Right>/b</Right></Pair></FolderPairs></FreeFileSync>",
        secret.display()
    );
    let j = parse_ffs_xml(&x).unwrap();
    assert!(!j[0].left.contains("TAJNE"));
    let x = "<!DOCTYPE FreeFileSync SYSTEM \"http://127.0.0.1:1/x.dtd\"><FreeFileSync><FolderPairs><Pair><Left>/a</Left><Right>/b</Right></Pair></FolderPairs></FreeFileSync>";
    assert!(parse_ffs_xml(x).is_ok());
}

#[test]
fn glebokie_zagniezdzenie_i_szerokosc() {
    for n in [63usize, 64, 65, 100_000] {
        let deep = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        let _ = parse_ffs_xml(&deep);
    }
    let wide = format!("<FreeFileSync>{}</FreeFileSync>", "<a/>".repeat(250_000));
    assert!(parse_ffs_xml(&wide).is_err());
}

#[test]
fn za_duzy_plik() {
    let big = format!(
        "<FreeFileSync>{}</FreeFileSync>",
        " ".repeat(9 * 1024 * 1024)
    );
    assert!(parse_ffs_xml(&big).unwrap_err().contains("zbyt duży"));
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.ffs_gui");
    std::fs::write(&p, big).unwrap();
    assert!(read_ffs_file(&p).is_err());
}

#[test]
fn cdata_komentarze_pi_w_srodku() {
    let x = "\u{feff}<?xml version=\"1.0\"?><!-- k --><FreeFileSync><!-- x --><FolderPairs><Pair><Left><![CDATA[/a & <b>]]></Left><Right>/r<?pi x?><!--c-->/s</Right></Pair></FolderPairs></FreeFileSync>";
    let j = parse_ffs_xml(x).unwrap();
    assert_eq!(j[0].left, "/a & <b>");
    assert_eq!(j[0].right, "/r/s");
}

#[test]
fn niepoprawne_xml_bez_paniki() {
    for x in [
        "<",
        "<a",
        "<a ",
        "<a b",
        "<a b=",
        "<a b=\"",
        "<a b=\"x",
        "<a b='x'",
        "<a></",
        "<a></b>",
        "<a><![CDATA[",
        "<a><!--",
        "<a><?",
        "<!DOCTYPE",
        "<!DOCTYPE x [",
        "<!DOCTYPE x [ ]>",
        "<?xml",
        "&",
        "&#;",
        "<a>&#x;</a>",
        "<a>&#xFFFFFFFFF;</a>",
        "<a>&#1114112;</a>",
        "<a>&#xD800;</a>",
        "<a>\u{0}</a>",
        "<ą ć=\"ż\">ó</ą>",
        "</a>",
        "<a/><b/>",
        "<a b=\"1\" b=\"2\"/>",
        "<FreeFileSync><FolderPairs><Pair/></FolderPairs></FreeFileSync>",
        "<FreeFileSync/>",
        "<!DOCTYPE x [<!ENTITY a \"]>\">]><FreeFileSync/>",
    ] {
        let _ = parse_ffs_xml(x);
    }
}

#[test]
fn fuzz_mutacji_bez_paniki() {
    let seed = "<?xml version=\"1.0\" encoding=\"utf-8\"?><FreeFileSync XmlType=\"GUI\"><Compare><Variant>Content</Variant><Symlinks>Follow</Symlinks></Compare><Synchronize><Variant>Custom</Variant><CustomDirections><LeftOnly>Right</LeftOnly><Different>Left</Different></CustomDirections><DeletionPolicy>Versioning</DeletionPolicy><VersioningFolder Style=\"Replace\" MaxAge=\"3\">/x</VersioningFolder></Synchronize><Filter><Include><Item>*.a</Item></Include><Exclude><Item>*\\bin\\</Item></Exclude><SizeMin Unit=\"KB\">4</SizeMin><TimeSpan Type=\"LastDays\">3</TimeSpan></Filter><FolderPairs><Pair><Left>/home/ż &amp; ó</Left><Right><![CDATA[/r]]></Right><LocalFilter><Exclude><Item>x</Item></Exclude></LocalFilter></Pair></FolderPairs></FreeFileSync>";
    let mut s: u64 = 0x1234_5678_9abc_def0;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let frag = [
        "<",
        ">",
        "&",
        "&#",
        ";",
        "\"",
        "'",
        "<![CDATA[",
        "]]>",
        "<!--",
        "-->",
        "<?",
        "?>",
        "</",
        "/>",
        "ż",
        "\u{0}",
        "=",
        "[",
        "]>",
    ];
    for _ in 0..20_000 {
        let mut v: Vec<char> = seed.chars().collect();
        for _ in 0..(1 + rnd() % 6) {
            let pos = (rnd() as usize) % v.len();
            match rnd() % 4 {
                0 => {
                    v.remove(pos);
                }
                1 => {
                    for c in frag[(rnd() as usize) % frag.len()].chars().rev() {
                        v.insert(pos, c);
                    }
                }
                2 => v.truncate(pos.max(1)),
                _ => {
                    let q = (rnd() as usize) % v.len();
                    v.swap(pos, q);
                }
            }
            if v.is_empty() {
                v.push('<');
            }
        }
        let x: String = v.into_iter().collect();
        let _ = parse_ffs_xml(&x);
    }
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("f.ffs_batch");
    for _ in 0..500 {
        let n = (rnd() % 400) as usize;
        let mut b: Vec<u8> = (0..n).map(|_| rnd() as u8).collect();
        if rnd() % 2 == 0 {
            b.splice(0..0, [0xFF, 0xFE]);
        }
        std::fs::write(&p, &b).unwrap();
        let _ = read_ffs_file(&p);
    }
}

#[test]
fn utf16_bez_bom_i_nieparzysta_dlugosc_nie_panikuje() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.ffs_gui");
    let x = pair("/a", "/b");
    let mut b: Vec<u8> = x.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    std::fs::write(&p, &b).unwrap();
    assert!(read_ffs_file(&p).is_err());
    let mut with = vec![0xFF, 0xFE];
    with.append(&mut b);
    with.push(0x41);
    std::fs::write(&p, &with).unwrap();
    assert_eq!(read_ffs_file(&p).unwrap().len(), 1);
}

#[test]
fn nazwa_pliku_zawiera_polskie_i_dziwne_znaki() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("Zażółć <gęślą> jaźń.ffs_gui");
    std::fs::write(&p, pair("/a", "/b")).unwrap();
    let j = read_ffs_file(&p).unwrap();
    assert_eq!(j[0].name, "Zażółć <gęślą> jaźń");
}

// ---------- mapowanie i wyjście ----------

#[test]
fn wyjscie_wylaczone_bez_hookow_unikalne_id() {
    let x = "<FreeFileSync><PostSyncCommand Condition=\"Completion\">rm -rf /</PostSyncCommand><BatchConfig><PostSyncCommand>calc</PostSyncCommand></BatchConfig><FolderPairs>".to_string()
        + &(0..50)
            .map(|i| format!("<Pair><Left>/l{i}</Left><Right>/r{i}</Right></Pair>"))
            .collect::<String>()
        + "<Pair><Left>/l0</Left><Right>/r0</Right></Pair></FolderPairs></FreeFileSync>";
    let jobs = parse_ffs_xml(&x).unwrap();
    assert_eq!(jobs.len(), 51);
    let mut ids: Vec<_> = jobs.iter().map(|j| j.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 51);
    for j in &jobs {
        assert!(
            !j.enabled
                && j.run_before.is_none()
                && j.run_after.is_none()
                && j.webhook_url.is_none()
        );
        assert!(matches!(j.schedule, syncron_core::model::Schedule::Manual));
        j.validate().unwrap();
    }
    let a = parse_ffs_xml(&pair("/a", "/b")).unwrap();
    let b = parse_ffs_xml(&pair("/a", "/b")).unwrap();
    assert_ne!(a[0].id, b[0].id);
}

#[test]
fn custom_wszystkie_kierunki() {
    let dirs = ["Left", "Right", "None", "left", "RIGHT", "bzdura", ""];
    for lo in dirs {
        for ro in dirs {
            let sync = format!("<Variant>Custom</Variant><CustomDirections><LeftOnly>{lo}</LeftOnly><RightOnly>{ro}</RightOnly></CustomDirections>");
            let j =
                &parse_ffs_xml(&simple(&sync, "<Variant>TimeAndSize</Variant>", "")).unwrap()[0];
            let c = j.custom.unwrap();
            let want_l = match lo.to_lowercase().as_str() {
                "right" => CustomAction::CopyToRight,
                "left" => CustomAction::DeleteLeft,
                _ => CustomAction::Skip,
            };
            let want_r = match ro.to_lowercase().as_str() {
                "left" => CustomAction::CopyToLeft,
                "right" => CustomAction::DeleteRight,
                _ => CustomAction::Skip,
            };
            assert_eq!((c.left_only, c.right_only), (want_l, want_r), "{lo}/{ro}");
            j.validate().unwrap();
        }
    }
    let j = &parse_ffs_xml(&simple("<Variant>Custom</Variant>", "", "")).unwrap()[0];
    assert_eq!(j.custom.unwrap().left_only, CustomAction::Skip);
}

#[test]
fn mirror_usuwa_update_nie_usuwa() {
    let m = &parse_ffs_xml(&simple("<Variant>Mirror</Variant>", "", "")).unwrap()[0];
    assert_eq!(m.mode, Mode::LeftToRight);
    assert!(!m.no_delete_right && !m.no_delete_left);
    let u = &parse_ffs_xml(&simple("<Variant>Update</Variant>", "", "")).unwrap()[0];
    assert!(u.no_delete_right);
    let m = &parse_ffs_xml(&simple("<Variant>\n  MIRROR \n</Variant>", "", "")).unwrap()[0];
    assert_eq!(m.mode, Mode::LeftToRight);
}

#[test]
fn rozmiary_jednostki_i_przepelnienie() {
    let f = |a: &str| {
        format!(
            "<Filter><SizeMin Unit=\"{a}\">5</SizeMin><SizeMax Unit=\"{a}\">9</SizeMax></Filter>"
        )
    };
    for (u, m) in [
        ("Byte", 1u64),
        ("KB", 1024),
        ("MB", 1 << 20),
        ("None", 0),
        ("TB", 0),
        ("", 0),
    ] {
        let j = &parse_ffs_xml(&simple("", "", &f(u))).unwrap()[0];
        if m == 0 {
            assert_eq!((j.min_size, j.max_size), (None, None), "{u}");
        } else {
            assert_eq!((j.min_size, j.max_size), (Some(5 * m), Some(9 * m)), "{u}");
        }
    }
    let ov = "<Filter><SizeMin Unit=\"GB\">99999999999999999999</SizeMin><SizeMax Unit=\"MB\">18446744073709551615</SizeMax></Filter>";
    let j = &parse_ffs_xml(&simple("", "", ov)).unwrap()[0];
    assert_eq!((j.min_size, j.max_size), (None, None));
    let bad = "<Filter><SizeMin Unit=\"MB\">9</SizeMin><SizeMax Unit=\"MB\">1</SizeMax></Filter>";
    assert!(parse_ffs_xml(&simple("", "", bad)).is_err());
}

#[test]
fn timespan_i_wersjonowanie_granice() {
    for (t, want) in [
        ("LastDays\">30", Some(30)),
        ("LastDays\">0", None),
        ("LastDays\">99999999999", None),
        ("LastDays\">-5", None),
        ("Today\">0", Some(1)),
        ("Month\">1", None),
        ("None\">0", None),
    ] {
        let j = &parse_ffs_xml(&simple(
            "",
            "",
            &format!("<Filter><TimeSpan Type=\"{t}</TimeSpan></Filter>"),
        ))
        .unwrap()[0];
        assert_eq!(j.max_age_days, want, "{t}");
    }
    let v = r#"<DeletionPolicy>Versioning</DeletionPolicy><VersioningFolder Style="Replace" MaxAge="-1" MaxCount="99999999999">/x</VersioningFolder>"#;
    let j = &parse_ffs_xml(&simple(v, "", "")).unwrap()[0];
    assert_eq!(j.trash_days, 30);
    let v = r#"<DeletionPolicy>Versioning</DeletionPolicy><VersioningFolder MaxAge="36501"/>"#;
    assert_eq!(parse_ffs_xml(&simple(v, "", "")).unwrap()[0].trash_days, 30);
}

#[cfg(target_os = "linux")]
#[test]
fn mirror_z_brakujacym_lewym_nie_kasuje_prawego() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path().join("R");
    std::fs::create_dir_all(&r).unwrap();
    for i in 0..10 {
        std::fs::write(r.join(format!("f{i}.txt")), "x").unwrap();
    }
    let l = t.path().join("%UserProfile%").join("Documents");
    let x = format!(
        "<FreeFileSync><Synchronize><Variant>Mirror</Variant></Synchronize><FolderPairs><Pair><Left>{}</Left><Right>{}</Right></Pair></FolderPairs></FreeFileSync>",
        l.display(),
        r.display()
    );
    let mut j = parse_ffs_xml(&x).unwrap().remove(0);
    j.enabled = true;
    let _ = run_job(&j, &t.path().join("st"), false, true);
    assert_eq!(
        std::fs::read_dir(&r).unwrap().count(),
        10,
        "prawa strona została wyczyszczona"
    );
}

// ---------- BUGI ----------

#[test]

fn t4_1_main_config_layout() {
    let x = r#"<?xml version="1.0" encoding="utf-8"?>
<FreeFileSync XmlType="GUI" XmlFormat="17">
    <Notes/>
    <MainConfig>
        <Comparison>
            <Variant>TimeAndSize</Variant>
            <Symlinks>Exclude</Symlinks>
            <IgnoreTimeShift/>
        </Comparison>
        <SyncConfig>
            <Variant>Mirror</Variant>
            <DetectMovedFiles>false</DetectMovedFiles>
            <DeletionPolicy>Permanent</DeletionPolicy>
            <VersioningFolder Style="Replace"/>
        </SyncConfig>
        <GlobalFilter>
            <Include><Item>*</Item></Include>
            <Exclude><Item>\System Volume Information\</Item><Item>*\thumbs.db</Item></Exclude>
            <TimeSpan Type="None">0</TimeSpan>
            <SizeMin Unit="None">0</SizeMin>
            <SizeMax Unit="None">0</SizeMax>
        </GlobalFilter>
        <FolderPairs>
            <Pair><Left>C:\Dane</Left><Right>D:\Kopia</Right></Pair>
        </FolderPairs>
        <IgnoreErrors>false</IgnoreErrors>
        <PostSyncCommand Condition="Completion"/>
    </MainConfig>
    <GuiConfig><MainDialog Maximized="true"/></GuiConfig>
</FreeFileSync>"#;
    let j = parse_ffs_xml(x).expect("układ MainConfig nie jest rozpoznany");
    assert_eq!(j[0].mode, Mode::LeftToRight);
    assert_eq!(j[0].trash_days, 0);
    assert!(j[0].exclude.contains(&"*/thumbs.db".to_string()));
}

#[test]

fn t4_2_ampersandy_kwadratowo() {
    let x = format!(
        "<FreeFileSync><FolderPairs><Pair><Left>{}</Left><Right>/r</Right></Pair></FolderPairs></FreeFileSync>",
        "&".repeat(2_000_000)
    );
    let r = timed(10, move || parse_ffs_xml(&x).is_ok());
    assert!(r.is_some(), "import 2 MB '&' nie skończył się w 10 s");
}

#[test]

fn t4_3_maski_kwadratowo() {
    let items: String = (0..100_000).map(|i| format!("<Item>m{i}</Item>")).collect();
    let x = simple(
        "",
        "",
        &format!("<Filter><Exclude>{items}</Exclude></Filter>"),
    );
    assert!(
        timed(10, move || parse_ffs_xml(&x).is_ok()).is_some(),
        "100k masek > 10 s"
    );
}

#[test]

fn t4_4_update_nie_nadpisuje_nowszego_prawego() {
    let u = parse_ffs_xml(&simple("<Variant>Update</Variant>", "", ""))
        .unwrap()
        .remove(0);
    let mut l = Tree::new();
    let mut r = Tree::new();
    l.insert(
        "a".into(),
        Meta {
            size: 1,
            mtime: 1000,
        },
    );
    r.insert(
        "a".into(),
        Meta {
            size: 2,
            mtime: 5000,
        },
    );
    let e = Tree::new();
    let p = plan_ext(&l, &r, &e, &e, u.options(), u.plan_ext());
    assert_ne!(
        p.get("a"),
        Some(&Action::CopyToRight),
        "starszy lewy nadpisze nowszy prawy"
    );
}

#[test]
#[ignore = "BUG T4-5: maski zakotwiczone (\\config.ini) i maski plików z folderem (docs\\a.txt) mapowane błędnie"]
fn t4_5_maski_zakotwiczone() {
    let f = |m: &str| {
        parse_ffs_xml(&simple(
            "",
            "",
            &format!("<Filter><Exclude><Item>{m}</Item></Exclude></Filter>"),
        ))
        .unwrap()[0]
            .rules()
    };
    let fi = |p: &str| FileInfo {
        path: p.into(),
        size: 1,
    };
    let rs = f("\\config.ini");
    assert!(!rs.allows(&fi("config.ini")));
    assert!(
        rs.allows(&fi("sub/config.ini")),
        "\\config.ini dotyczy tylko korzenia, a wyklucza też podfoldery"
    );
    let rs = f("docs\\a.txt");
    assert!(
        !rs.allows(&fi("x/docs/a.txt")),
        "docs\\a.txt na głębszym poziomie nie jest wykluczone"
    );
}

#[test]

fn t4_6_lokalizacje_sftp_z_ffs() {
    for l in [
        "sftp://jan@host:22/home/jan",
        "ftp://host/dir",
        "sftp:|pass=abc|/x",
    ] {
        if let Ok(j) = parse_ffs_xml(&pair(l, "/mnt/kopia")) {
            let loc = syncron_core::location::Location::parse(&j[0].left);
            assert!(loc.is_local(), "{l} zinterpretowane jako konto zdalne");
        }
    }
}

#[test]

fn t4_7_znaki_sterujace_w_sciezkach() {
    for c in ["&#0;", "&#1;", "&#x1B;", "&#10;"] {
        if let Ok(j) = parse_ffs_xml(&pair(&format!("/a{c}b"), "/r")) {
            assert!(
                !j[0].left.chars().any(|c| c.is_control()),
                "{:?}",
                j[0].left
            );
            assert!(
                !j[0].name.chars().any(|c| c.is_control()),
                "{:?}",
                j[0].name
            );
        }
    }
}

#[test]

fn t4_8_amplifikacja_pamieci() {
    let items: String = (0..300).map(|i| format!("<Item>maska{i}</Item>")).collect();
    let pairs: String = (0..3000)
        .map(|i| format!("<Pair><Left>/l{i}</Left><Right>/r{i}</Right></Pair>"))
        .collect();
    let x = format!("<FreeFileSync><Filter><Exclude>{items}</Exclude></Filter><FolderPairs>{pairs}</FolderPairs></FreeFileSync>");
    let total: usize = parse_ffs_xml(&x)
        .map(|j| j.iter().map(|j| j.exclude.len()).sum())
        .unwrap_or(0);
    assert!(
        total <= 1_000_000,
        "{total} masek w pamięci z pliku {} KB",
        x.len() / 1024
    );
}

#[test]

fn t4_9_custom_size_only_different() {
    let sync = "<Variant>Custom</Variant><CustomDirections><Different>Right</Different><Conflict>None</Conflict></CustomDirections>";
    let j = &parse_ffs_xml(&simple(sync, "<Variant>Size</Variant>", "")).unwrap()[0];
    assert_eq!(j.compare, Compare::SizeOnly);
    assert_eq!(j.custom.unwrap().different, CustomAction::CopyToRight);
}

#[test]
#[ignore = "BUG T4-10: jeden zły wiersz pary (zagnieżdżone foldery / pusta strona) przerywa import całego pliku"]
fn t4_10_zla_para_przerywa_calosc() {
    let x = "<FreeFileSync><FolderPairs><Pair><Left>/a</Left><Right>/b</Right></Pair><Pair><Left>/c</Left><Right>/c/d</Right></Pair><Pair><Left>/e</Left><Right>/f</Right></Pair></FolderPairs></FreeFileSync>";
    let j = parse_ffs_xml(x).expect("dobre pary powinny zostać zaimportowane");
    assert_eq!(j.len(), 2);
}

#[test]

fn t4_11_kodowanie_8bit() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.ffs_gui");
    let mut b = b"<?xml version=\"1.0\" encoding=\"iso-8859-2\"?><FreeFileSync><FolderPairs><Pair><Left>/Zdj".to_vec();
    b.push(0xEA);
    b.extend(b"cia</Left><Right>/b</Right></Pair></FolderPairs></FreeFileSync>");
    std::fs::write(&p, b).unwrap();
    if let Ok(j) = read_ffs_file(&p) {
        assert!(!j[0].left.contains('\u{fffd}'), "{:?}", j[0].left);
    }
}
