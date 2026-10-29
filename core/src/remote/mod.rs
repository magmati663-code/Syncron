//! Własna (natywna) warstwa zdalnych magazynów: WebDAV, FTP/FTPS, SFTP, S3, Google Drive, OneDrive, Dropbox.
//! Bez rclone i innych programów zewnętrznych — każdy backend to kod w tym crate.
//!
//! Zasady wspólne dla backendów:
//! - ścieżki w API to ścieżki względne z `/` (UTF-8, NFC nie jest wymuszane), względem korzenia podanego w `open`;
//! - błąd listowania = `Err` (silnik wtedy przerywa przebieg i niczego nie usuwa);
//! - `upload` jest atomowy z punktu widzenia czytelnika (plik tymczasowy + zmiana nazwy tam, gdzie protokół pozwala,
//!   w pozostałych przypadkach zapis pod docelową nazwą zakończony weryfikacją rozmiaru);
//! - sekrety nigdy nie trafiają do logów ani komunikatów błędów.
use crate::model::Bandwidth;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub mod ftp;
pub mod http;
pub mod mem;
pub mod s3;
pub mod sftp;
pub mod ops;
pub mod spec;
pub mod webdav;

/// Skrót zawartości podawany przez serwer (jeśli go ma): pozwala porównać pliki bez pobierania.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash {
    /// `md5`, `sha1`, `sha256`, `quickxor`, `dropbox` itp.
    pub algo: &'static str,
    pub hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RMeta {
    pub size: u64,
    /// Czas modyfikacji (sekundy UNIX); 0, gdy serwer go nie podaje.
    pub mtime: i64,
    pub hash: Option<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct REntry {
    /// Ścieżka względna z `/`.
    pub path: String,
    pub meta: RMeta,
}

/// Element folderu (przeglądarka folderów w kreatorze).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirItem {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RError {
    NotFound(String),
    /// Chwilowa niedostępność (zablokowany plik, limit zapytań 429/503, zajęty serwer): ponów później.
    Busy(String),
    /// Złe dane logowania / wygasły token / brak uprawnień.
    Auth(String),
    /// Sieć: DNS, połączenie, TLS, przekroczony czas.
    Network(String),
    /// Brak miejsca / przekroczony limit konta.
    Quota(String),
    Failed(String),
}

impl std::fmt::Display for RError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RError::NotFound(s) => write!(f, "nie znaleziono: {s}"),
            RError::Busy(s) => write!(f, "zajęte/chwilowo niedostępne: {s}"),
            RError::Auth(s) => write!(f, "błąd logowania lub uprawnień: {s}"),
            RError::Network(s) => write!(f, "błąd sieci: {s}"),
            RError::Quota(s) => write!(f, "brak miejsca lub limit: {s}"),
            RError::Failed(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for RError {}

pub type RResult<T> = Result<T, RError>;

/// Kontekst transferu: limit prędkości, anulowanie i postęp.
#[derive(Clone, Copy)]
pub struct Xfer<'a> {
    pub bw: &'a Bandwidth,
    pub cancel: Option<&'a AtomicBool>,
    /// Wywoływane z liczbą bajtów przesłanych od poprzedniego wywołania.
    pub progress: Option<&'a (dyn Fn(u64) + Sync)>,
}

impl<'a> Xfer<'a> {
    pub fn new(bw: &'a Bandwidth) -> Self {
        Xfer {
            bw,
            cancel: None,
            progress: None,
        }
    }
}

/// Zdalny magazyn przypięty do konkretnego konta i folderu głównego.
pub trait Backend: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Rekurencyjna lista PLIKÓW pod `dir` ("" = korzeń). Brak folderu = pusta lista. Błąd = `Err`.
    /// Zwracane `path` są ZAWSZE względem korzenia backendu (a nie względem `dir`), np. dla `dir` = `.syncron-trash`
    /// wpis ma ścieżkę `.syncron-trash/2026-01-01_120000/a.txt`.
    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>>;
    /// Jeden poziom folderu (do przeglądarki): foldery i pliki.
    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>>;
    /// Metadane jednego pliku (`None` = nie istnieje).
    fn stat(&self, path: &str) -> RResult<Option<REntry>>;
    /// Pobiera plik do `dest` (zapis przez wywołującego jest atomowy: on podaje plik tymczasowy).
    fn download(&self, path: &str, dest: &Path, x: &Xfer) -> RResult<()>;
    /// Wysyła `src` pod `path` (tworzy foldery nadrzędne, zastępuje istniejący plik, ustawia mtime jeśli protokół pozwala).
    fn upload(&self, src: &Path, path: &str, mtime: i64, x: &Xfer) -> RResult<()>;
    /// Usuwa plik. Brak pliku to sukces.
    fn delete(&self, path: &str) -> RResult<()>;
    /// Przenosi/zmienia nazwę po stronie serwera (tworzy foldery nadrzędne, zastępuje cel).
    fn rename(&self, from: &str, to: &str) -> RResult<()>;
    /// Kopiuje plik po stronie serwera (np. kopia zapasowa do kosza przed nadpisaniem). Domyślnie: pobranie do pliku
    /// tymczasowego i wysłanie; backendy z kopiowaniem serwerowym (Drive, S3, WebDAV COPY...) nadpisują tę metodę.
    fn copy(&self, from: &str, to: &str, x: &Xfer) -> RResult<()> {
        copy_via_temp(self, from, to, x)
    }
    /// Tworzy folder wraz z nadrzędnymi.
    fn mkdirs(&self, dir: &str) -> RResult<()>;
    /// Usuwa pusty folder (sprzątanie po usunięciu plików); niepusty/nieistniejący = `Ok`.
    fn rmdir_if_empty(&self, _dir: &str) -> RResult<()> {
        Ok(())
    }
    fn about(&self) -> RResult<Option<Quota>> {
        Ok(None)
    }
    /// Test połączenia i uprawnień (lista korzenia); zwraca krótki opis (np. wersja serwera).
    fn ping(&self) -> RResult<String> {
        self.list_dir("").map(|_| "OK".to_string())
    }
}

/// Kopiowanie przez plik tymczasowy (domyślna implementacja `Backend::copy`).
pub fn copy_via_temp<B: Backend + ?Sized>(b: &B, from: &str, to: &str, x: &Xfer) -> RResult<()> {
    let meta = b
        .stat(from)?
        .ok_or_else(|| RError::NotFound(from.to_string()))?;
    let dir = crate::vss::scratch_dir().map_err(|e| RError::Failed(e.to_string()))?;
    let tmp = dir.join("kopia");
    let res = b
        .download(from, &tmp, x)
        .and_then(|_| b.upload(&tmp, to, meta.meta.mtime, x));
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir(&dir);
    res
}

/// Odbiornik odświeżonych sekretów (nazwa, wartość).
pub type SecretSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// Parametry konta przekazywane backendom: jawne (host, port, użytkownik...) i sekretne (hasła, tokeny).
#[derive(Clone, Default)]
pub struct Params {
    pub public: BTreeMap<String, String>,
    pub secret: BTreeMap<String, String>,
    /// Wywoływane, gdy backend odświeży sekret (np. token OAuth) — wywołujący zapisuje go w magazynie sekretów.
    pub on_secret_update: Option<SecretSink>,
}

impl Params {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.public
            .get(key)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }
    pub fn need(&self, key: &str) -> RResult<&str> {
        self.get(key)
            .ok_or_else(|| RError::Failed(format!("brak wymaganego pola „{key}”")))
    }
    pub fn secret(&self, key: &str) -> Option<&str> {
        self.secret
            .get(key)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
    }
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some("1" | "true" | "yes" | "tak"))
    }
    pub fn num(&self, key: &str, default: u64) -> u64 {
        self.get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

/// Czyści ścieżkę względną: bez pustych komponentów, `.` i `..` (błąd), bez wiodącego `/`.
pub fn clean_path(p: &str) -> RResult<String> {
    let mut out = Vec::new();
    for c in p.split(['/', '\\']) {
        match c {
            "" | "." => {}
            ".." => return Err(RError::Failed(format!("niedozwolona ścieżka: {p}"))),
            c if c.chars().any(|ch| (ch as u32) < 32) => {
                return Err(RError::Failed("znak sterujący w ścieżce".into()))
            }
            c => out.push(c),
        }
    }
    Ok(out.join("/"))
}

/// Łączy korzeń i ścieżkę względną (oba już „czyste”).
pub fn join_path(root: &str, rel: &str) -> String {
    match (root.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (_, true) => root.to_string(),
        _ => format!("{root}/{rel}"),
    }
}

/// Otwiera backend dla rodzaju konta. `root` = folder główny w obrębie konta (np. `Dokumenty/Praca`).
/// Zespoły backendów dopisują tu swoje gałęzie (każdy backend w osobnym pliku `remote/<nazwa>.rs`).
pub fn open(kind: &str, p: &Params, root: &str) -> RResult<Box<dyn Backend>> {
    let root = clean_path(root)?;
    match kind {
        "mem" => Ok(Box::new(mem::MemBackend::shared(
            p.get("id").unwrap_or("default"),
            &root,
        ))),
        "webdav" => Ok(Box::new(webdav::WebDav::new(p, &root)?)),
        "ftp" => Ok(Box::new(ftp::FtpBackend::new("ftp", p, &root)?)),
        "ftps" => Ok(Box::new(ftp::FtpBackend::new("ftps", p, &root)?)),
        "s3" => Ok(Box::new(s3::S3Backend::new(p, &root)?)),
        "sftp" => Ok(Box::new(sftp::SftpBackend::new(p, &root)?)),
        other => Err(RError::Failed(format!(
            "rodzaj konta „{other}” nie jest jeszcze obsługiwany"
        ))),
    }
}
