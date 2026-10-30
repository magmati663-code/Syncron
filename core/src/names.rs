//! Normalizacja nazw: polskie znaki (NFC/NFD), wielkość liter, nazwy Windows.
use unicode_normalization::UnicodeNormalization;

/// Klucz porównawczy: NFC + małe litery (kolizje case-insensitive).
///
/// Małe litery znak po znaku (bez kontekstowej sigmy końcowej), NFC powtarzane do stałości (ograniczona liczba
/// kroków) — klucz jest idempotentny i taki sam dla NFC i NFD tej samej nazwy.
pub fn key(name: &str) -> String {
    let mut k: String = name.nfc().collect();
    for _ in 0..8 {
        let next: String = k.chars().flat_map(|c| c.to_lowercase()).nfc().collect();
        if next == k {
            break;
        }
        k = next;
    }
    k
}

/// Ta sama nazwa niezależnie od NFC/NFD.
pub fn same_name(a: &str, b: &str) -> bool {
    a.nfc().eq(b.nfc())
}

const RESERVED: [&str; 26] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "CONIN$",
];

/// Czy nazwa jest niedozwolona na Windows (znaki, zastrzeżone nazwy także ze spacją przed kropką i z indeksami
/// górnymi, `CONIN$`/`CONOUT$`, kropka/spacja na końcu, limit 255 jednostek UTF-16).
pub fn invalid_on_windows(name: &str) -> bool {
    if name.is_empty() || name.ends_with('.') || name.ends_with(' ') {
        return true;
    }
    if name.encode_utf16().count() > 255 {
        return true;
    }
    if name
        .chars()
        .any(|c| "<>:\"/\\|?*".contains(c) || (c as u32) < 32)
    {
        return true;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    stem == "CONOUT$"
        || RESERVED.contains(&stem.as_str())
        || stem
            .strip_prefix("LPT")
            .is_some_and(|d| matches!(d, "\u{b9}" | "\u{b2}" | "\u{b3}"))
}

/// Pliki tymczasowe/systemowe pomijane domyślnie.
pub fn is_temp(name: &str) -> bool {
    let l = name.to_lowercase();
    name.starts_with("~$")
        || name.starts_with(".~lock.")
        || [
            ".tmp",
            ".crdownload",
            ".part",
            ".syncron.tmp",
            ".syncpolak.tmp",
        ]
        .iter()
        .any(|e| l.ends_with(e))
        || matches!(l.as_str(), ".ds_store" | "thumbs.db" | "desktop.ini")
}

/// Waliduje ścieżkę względną (separator `/`) pochodzącą z zewnątrz (listing zdalny, interfejs, CLI): odrzuca puste
/// komponenty, `.` i `..`, ścieżki bezwzględne i z literą dysku, a na Windows także `\\`. Zwraca opis błędu po polsku.
pub fn check_rel(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("pusta ścieżka".into());
    }
    let drive = cfg!(windows)
        && rel.as_bytes().get(1) == Some(&b':')
        && rel.as_bytes()[0].is_ascii_alphabetic();
    if rel.starts_with('/') || rel.starts_with('\\') || drive {
        return Err(format!("ścieżka bezwzględna niedozwolona: {rel}"));
    }
    if cfg!(windows) && rel.contains('\\') {
        return Err(format!("odwrócony ukośnik niedozwolony: {rel}"));
    }
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("niedozwolony komponent ścieżki w: {rel}"));
        }
        if part.chars().any(|c| (c as u32) < 32) {
            return Err(format!("znak sterujący w ścieżce: {rel:?}"));
        }
    }
    Ok(())
}

/// Obcina `s` do `max` bajtów na granicy znaku.
fn cut_utf8(s: &str, max: usize) -> &str {
    let mut n = max.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// Nazwa kopii konfliktowej: `raport.docx` -> `raport (konflikt 2026-10-27).docx`.
/// Część bazowa jest skracana, by nazwa pliku mieściła się w 255 B (sufiks i rozszerzenie zostają).
pub fn conflict_name(rel: &str, stamp: &str) -> String {
    conflict_name_n(rel, stamp, 1)
}

/// Jak [`conflict_name`], z licznikiem: dla `n >= 2` sufiks ma postać ` (konflikt STAMP-N)` (unikalne nazwy w ciągu dnia).
pub fn conflict_name_n(rel: &str, stamp: &str, n: u32) -> String {
    const MAX: usize = 255;
    let (dir, file) = match rel.rfind('/') {
        Some(i) => (&rel[..=i], &rel[i + 1..]),
        None => ("", rel),
    };
    let suffix = if n >= 2 {
        format!(" (konflikt {stamp}-{n})")
    } else {
        format!(" (konflikt {stamp})")
    };
    let (base, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    let ext = cut_utf8(ext, MAX.saturating_sub(suffix.len() + 1));
    let budget = MAX.saturating_sub(suffix.len() + ext.len());
    format!("{dir}{}{suffix}{ext}", cut_utf8(base, budget))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfc_nfd_equal() {
        let nfc = "zażółć gęślą jaźń.txt";
        let nfd: String = unicode_normalization::UnicodeNormalization::nfd(nfc).collect();
        assert_ne!(nfc, nfd);
        assert!(same_name(nfc, &nfd));
        assert_eq!(key(nfc), key(&nfd));
    }

    #[test]
    fn case_collision_polish() {
        assert_eq!(key("ŁÓDŹ.txt"), key("łódź.TXT"));
    }

    #[test]
    fn windows_names() {
        assert!(invalid_on_windows("con.txt"));
        assert!(invalid_on_windows("plik?.txt"));
        assert!(invalid_on_windows("koniec."));
        assert!(!invalid_on_windows("Żółć.docx"));
    }

    #[test]
    fn check_rel_rules() {
        for ok in ["a", "a/b.txt", "zażółć/gęślą jaźń.txt", "a b/c", "..a/b.."] {
            assert!(check_rel(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/a", "a//b", "a/../b", "..", "a/.", "a/b/", "x\u{7}y"] {
            assert!(check_rel(bad).is_err(), "{bad:?}");
        }
        if cfg!(windows) {
            assert!(check_rel("C:/x").is_err() && check_rel("a\\b").is_err());
        }
    }

    #[test]
    fn conflict_names() {
        assert_eq!(
            conflict_name("a/żółć.docx", "X"),
            "a/żółć (konflikt X).docx"
        );
        assert_eq!(conflict_name("README", "X"), "README (konflikt X)");
    }

    #[test]
    fn temp_files() {
        assert!(is_temp("~$raport.docx"));
        assert!(is_temp(".DS_Store"));
        assert!(!is_temp("raport.docx"));
    }
}
