//! Normalizacja nazw: polskie znaki (NFC/NFD), wielkość liter, nazwy Windows.
use unicode_normalization::UnicodeNormalization;

/// Klucz porównawczy: NFC + małe litery (kolizje case-insensitive).
pub fn key(name: &str) -> String {
    name.nfc().collect::<String>().to_lowercase()
}

/// Ta sama nazwa niezależnie od NFC/NFD.
pub fn same_name(a: &str, b: &str) -> bool {
    a.nfc().eq(b.nfc())
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Czy nazwa jest niedozwolona na Windows.
pub fn invalid_on_windows(name: &str) -> bool {
    if name.is_empty() || name.ends_with('.') || name.ends_with(' ') {
        return true;
    }
    if name.chars().any(|c| "<>:\"/\\|?*".contains(c) || (c as u32) < 32) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or("").to_uppercase();
    RESERVED.contains(&stem.as_str())
}

/// Pliki tymczasowe/systemowe pomijane domyślnie.
pub fn is_temp(name: &str) -> bool {
    let l = name.to_lowercase();
    name.starts_with("~$")
        || name.starts_with(".~lock.")
        || [".tmp", ".crdownload", ".part", ".syncpolak.tmp"].iter().any(|e| l.ends_with(e))
        || matches!(l.as_str(), ".ds_store" | "thumbs.db" | "desktop.ini")
}

/// Nazwa kopii konfliktowej: `raport.docx` -> `raport (konflikt 2026-10-27).docx`.
pub fn conflict_name(rel: &str, stamp: &str) -> String {
    let (dir, file) = match rel.rfind('/') {
        Some(i) => (&rel[..=i], &rel[i + 1..]),
        None => ("", rel),
    };
    match file.rfind('.') {
        Some(i) if i > 0 => format!("{dir}{} (konflikt {stamp}){}", &file[..i], &file[i..]),
        _ => format!("{dir}{file} (konflikt {stamp})"),
    }
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
    fn conflict_names() {
        assert_eq!(conflict_name("a/żółć.docx", "X"), "a/żółć (konflikt X).docx");
        assert_eq!(conflict_name("README", "X"), "README (konflikt X)");
    }

    #[test]
    fn temp_files() {
        assert!(is_temp("~$raport.docx"));
        assert!(is_temp(".DS_Store"));
        assert!(!is_temp("raport.docx"));
    }
}
