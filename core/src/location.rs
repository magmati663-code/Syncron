//! Lokalizacje: folder lokalny/UNC albo zdalny (konto natywne `konto:ścieżka`). Listowanie i operacje na plikach.
use crate::model::Bandwidth;
use crate::names;
use crate::plan::Meta;
use crate::rules::{FileInfo, RuleSet};
use crate::vss::Vss;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, UNIX_EPOCH};

pub const TRASH_DIR: &str = ".syncron-trash";
/// Nazwy z czasów, gdy aplikacja nazywała się SyncPolak: nadal pomijane przy listowaniu.
pub const LEGACY_TRASH_DIR: &str = ".syncpolak-trash";
pub const LEGACY_TMP_SUFFIX: &str = ".syncpolak.tmp";

#[derive(Debug, Clone)]
pub struct Entry {
    /// Rzeczywista ścieżka względna (znaki '/'), jak na dysku.
    pub real: String,
    pub meta: Meta,
}

/// Wynik listowania: klucz (NFC + małe litery) -> wpis. `collisions` to ścieżki różniące się tylko wielkością liter/normalizacją.
#[derive(Debug, Default)]
pub struct Listing {
    pub files: BTreeMap<String, Entry>,
    pub collisions: Vec<String>,
    /// Pominięte elementy (np. nazwy spoza UTF-8, niebezpieczne ścieżki ze zdalnego listingu): opisy po polsku.
    pub skipped: Vec<String>,
}

#[derive(Debug)]
pub enum OpError {
    /// Plik zajęty/zablokowany przez inny program: ponów później.
    Busy(String),
    Failed(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::Busy(s) => write!(f, "plik zajęty: {s}"),
            OpError::Failed(s) => write!(f, "{s}"),
        }
    }
}

impl From<io::Error> for OpError {
    fn from(e: io::Error) -> Self {
        if is_busy(&e) {
            OpError::Busy(e.to_string())
        } else if e.kind() == io::ErrorKind::PermissionDenied {
            OpError::Failed(format!("brak uprawnień: {e}"))
        } else {
            OpError::Failed(e.to_string())
        }
    }
}

pub(crate) fn is_busy(e: &io::Error) -> bool {
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(32) | Some(33)) {
        return true; // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
    }
    // brak uprawnień (PermissionDenied) to zwykły błąd, nie zajętość pliku
    e.kind() == io::ErrorKind::Interrupted
}

/// Opcje kopiowania w ramach jednego przebiegu.
pub struct Opts {
    pub vss: Vss,
    /// Po zapisie porównaj sumę kontrolną BLAKE3 źródła i pliku docelowego.
    pub verify: bool,
    /// Limit prędkości przypadający na jeden transfer.
    pub bw: Bandwidth,
}

impl Opts {
    pub fn new(use_vss: bool, verify: bool, bw: &Bandwidth, threads: usize) -> Self {
        Opts {
            vss: Vss::new(use_vss),
            verify,
            bw: bw.split(threads as u32),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Location {
    Local(PathBuf),
    /// Konto zdalne (patrz `accounts`) i folder główny w jego obrębie.
    Remote {
        account: String,
        root: String,
    },
}

impl Location {
    pub fn parse(s: &str) -> Location {
        use unicode_normalization::UnicodeNormalization;
        let s = s.trim();
        if let Some(i) = s.find(':') {
            // nazwa konta w NFC (macOS podaje „ó” jako litera + znak łączący)
            let name: String = s[..i].nfc().collect();
            let rest = &s[i + 1..];
            let nchars = name.chars().count();
            // litera dysku (C:\ , C:/) to ścieżka lokalna; nazwa zdalna ma min. 2 znaki
            let drive =
                nchars == 1 && (rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/'));
            if nchars >= 2
                && !drive
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_-. ".contains(c))
            {
                return Location::Remote {
                    account: name,
                    root: rest.to_string(),
                };
            }
        }
        Location::Local(PathBuf::from(s))
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Location::Local(_))
    }

    /// Zapis „konto:ścieżka” (lokalna: ścieżka); odwrotność `parse`.
    pub fn spec(&self) -> String {
        match self {
            Location::Local(p) => p.to_string_lossy().into_owned(),
            Location::Remote { account, root } => format!("{account}:{root}"),
        }
    }

    pub fn list(&self, rules: &RuleSet) -> Result<Listing, String> {
        let (raw, skipped) = match self {
            Location::Local(root) => list_local(root)?,
            Location::Remote { account, root } => crate::remote::ops::list(account, root)?,
        };
        let mut out = Listing {
            skipped,
            ..Listing::default()
        };
        for (rel, meta) in raw {
            if !rules.allows(&FileInfo {
                path: rel.clone(),
                size: meta.size,
            }) {
                continue;
            }
            let key = names::key(&rel);
            match out.files.entry(key) {
                std::collections::btree_map::Entry::Occupied(_) => out.collisions.push(rel),
                std::collections::btree_map::Entry::Vacant(v) => {
                    v.insert(Entry { real: rel, meta });
                }
            }
        }
        Ok(out)
    }

    pub fn stat(&self, rel: &str) -> Option<Meta> {
        self.stat_checked(rel).ok().flatten()
    }

    /// Jak `stat`, ale odróżnia „nie istnieje” (`Ok(None)`) od „nie wiadomo” (`Err`: błąd sieci, logowania, dysku).
    /// Decyzje destrukcyjne (usunięcie po stronie przeciwnej) powinny opierać się na tej wersji.
    pub fn stat_checked(&self, rel: &str) -> Result<Option<Meta>, String> {
        match self {
            Location::Local(p) => match fs::metadata(local_full(p, rel)) {
                Ok(m) => Ok(meta_of(&m)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
            },
            Location::Remote { account, root } => crate::remote::ops::stat(account, root, rel),
        }
    }

    /// Odcisk treści do wykrywania przeniesień: BLAKE3 dla plików lokalnych, a dla zdalnych — skrót podany przez
    /// serwer (razem z rozmiarem). `None` = nie da się ustalić (wtedy nie zgadujemy przeniesienia).
    pub fn content_hash(&self, rel: &str) -> Option<[u8; 32]> {
        match self {
            Location::Local(p) => {
                let mut f = File::open(local_full(p, rel)).ok()?;
                let mut h = blake3::Hasher::new();
                io::copy(&mut f, &mut h).ok()?;
                Some(*h.finalize().as_bytes())
            }
            Location::Remote { account, root } => {
                crate::remote::ops::content_hash(account, root, rel)
            }
        }
    }

    /// Kopiuje plik `from_rel` z `from` do `self` jako `to_rel` (zapis atomowy, ponowienia dla plików zajętych).
    pub fn copy_from(
        &self,
        from: &Location,
        from_rel: &str,
        to_rel: &str,
        o: &Opts,
    ) -> Result<(), OpError> {
        match (from, self) {
            (Location::Local(a), Location::Local(b)) => {
                check_local_rel(from_rel)?;
                let src = local_full(a, from_rel);
                let dst = prepare_target(b, to_rel, true)?;
                let mut last = None;
                for delay in [0u64, 300, 1500, 4000] {
                    if delay > 0 {
                        sleep(Duration::from_millis(delay));
                    }
                    match copy_local(&src, &dst, o.verify, &o.bw) {
                        Ok(()) => return Ok(()),
                        Err(e) if is_busy(&e) => {
                            // plik zablokowany przez inny program: odczyt z migawki VSS (Windows, administrator)
                            if let Ok(snap) = o.vss.snapshot_path(&src) {
                                match copy_local(&snap, &dst, o.verify, &o.bw) {
                                    Ok(()) => return Ok(()),
                                    Err(e2) => last = Some(e2),
                                }
                            } else {
                                last = Some(e);
                            }
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                let msg = last.map(|e| e.to_string()).unwrap_or_default();
                Err(OpError::Busy(match o.vss.note() {
                    Some(n) => format!("{msg}; {n}"),
                    None => msg,
                }))
            }
            (Location::Local(a), Location::Remote { account, root }) => {
                check_local_rel(from_rel)?;
                crate::remote::ops::upload_local(account, root, &local_full(a, from_rel), to_rel, o)
            }
            (Location::Remote { account, root }, Location::Local(b)) => {
                let dst = prepare_target(b, to_rel, true)?;
                crate::remote::ops::download_to(account, root, from_rel, &dst, o)
            }
            (
                Location::Remote {
                    account: sa,
                    root: sr,
                },
                Location::Remote {
                    account: da,
                    root: dr,
                },
            ) => crate::remote::ops::remote_to_remote((sa, sr), from_rel, (da, dr), to_rel, o),
        }
    }

    /// Zmienia ścieżkę pliku w obrębie tej lokalizacji (przeniesienie bez ponownego kopiowania).
    pub fn rename(&self, from_rel: &str, to_rel: &str) -> Result<(), OpError> {
        match self {
            Location::Local(p) => {
                let src = prepare_target(p, from_rel, false)?;
                let dst = prepare_target(p, to_rel, true)?;
                fs::rename(src, &dst)?;
                remove_empty_parents(p, from_rel);
                Ok(())
            }
            Location::Remote { account, root } => {
                crate::remote::ops::rename(account, root, from_rel, to_rel)
            }
        }
    }

    /// Odczytuje mały plik tekstowy (np. `.syncignore`); brak pliku lub błąd = None.
    pub fn read_text(&self, rel: &str) -> Option<String> {
        match self {
            // Bajty dekodowane tolerancyjnie (UTF-8/BOM/UTF-16/reszta lossy), by `.syncignore` nie-UTF-8 nie był po cichu
            // pomijany w całości. Brak pliku = None; inny błąd odczytu = ostrzeżenie na stderr i None.
            Location::Local(p) => match fs::read(local_full(p, rel)) {
                Ok(b) => Some(crate::rules::decode_text(&b)),
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        eprintln!("Ostrzeżenie: nie można odczytać pliku {rel}: {e}");
                    }
                    None
                }
            },
            Location::Remote { account, root } => crate::remote::ops::read_text(account, root, rel),
        }
    }

    /// Kopiuje plik do kosza synchronizacji (przed nadpisaniem). Nie nadpisuje istniejącej wersji w koszu:
    /// gdy `stamp` jest już zajęty dla tego pliku, użyty zostaje wolny wariant `stamp-N`.
    pub fn backup(&self, rel: &str, stamp: &str) -> Result<(), OpError> {
        check_stamp(stamp)?;
        match self {
            Location::Local(p) => {
                let trash_rel = self.free_trash_path(stamp, rel);
                let dst = local_full(p, &trash_rel);
                if let Some(parent) = dst.parent() {
                    ensure_dir(parent)?;
                }
                fs::copy(local_full(p, rel), dst)?;
                Ok(())
            }
            Location::Remote { account, root } => {
                crate::remote::ops::backup(account, root, rel, stamp, stamp_base(stamp))
            }
        }
    }

    /// Przenosi plik do kosza synchronizacji (`stamp` = podfolder), albo usuwa gdy `stamp` jest puste.
    /// Istniejąca wersja w koszu nigdy nie jest nadpisywana (patrz `backup`).
    pub fn remove(&self, rel: &str, stamp: &str) -> Result<(), OpError> {
        if stamp.is_empty() {
            return match self {
                Location::Local(p) => {
                    fs::remove_file(prepare_target(p, rel, false)?)?;
                    remove_empty_parents(p, rel);
                    Ok(())
                }
                Location::Remote { account, root } => {
                    crate::remote::ops::remove(account, root, rel, "", "")
                }
            };
        }
        check_stamp(stamp)?;
        match self {
            Location::Local(p) => {
                let trash_rel = self.free_trash_path(stamp, rel);
                let src = prepare_target(p, rel, false)?;
                let dst = prepare_target(p, &trash_rel, true)?;
                if fs::rename(&src, &dst).is_err() {
                    fs::copy(&src, &dst)?;
                    fs::remove_file(&src)?;
                }
                remove_empty_parents(p, rel);
                Ok(())
            }
            Location::Remote { account, root } => {
                crate::remote::ops::remove(account, root, rel, stamp, stamp_base(stamp))
            }
        }
    }

    /// Ścieżka w koszu (tylko lokalna) (`.syncron-trash/<stamp>[-N]/<rel>`), która jeszcze nie istnieje.
    fn free_trash_path(&self, stamp: &str, rel: &str) -> String {
        let base = stamp_base(stamp);
        let mut cand = stamp.to_string();
        for n in 1..10_000u32 {
            let path = format!("{TRASH_DIR}/{cand}/{rel}");
            let taken = match self {
                Location::Local(p) => local_full(p, &path).symlink_metadata().is_ok(),
                Location::Remote { .. } => false, // zdalne: patrz remote::ops::free_trash_path
            };
            if !taken {
                return path;
            }
            cand = format!("{base}-{n}");
        }
        format!("{TRASH_DIR}/{cand}/{rel}")
    }

    /// Elementy kosza: (znacznik czasu przebiegu, ścieżka pliku, rozmiar).
    pub fn list_trash(&self) -> Result<Vec<(String, String, u64)>, String> {
        let mut out = Vec::new();
        match self {
            Location::Local(p) => {
                let root = local_full(p, TRASH_DIR);
                let Ok(rd) = fs::read_dir(&root) else {
                    return Ok(out);
                };
                for st in rd.flatten() {
                    // obce pliki w korzeniu kosza pomijamy
                    if !st.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    let stamp = st.file_name().to_string_lossy().into_owned();
                    let mut stack = vec![(st.path(), String::new())];
                    while let Some((dir, rel)) = stack.pop() {
                        let Ok(rd) = fs::read_dir(&dir) else { continue };
                        for e in rd.flatten() {
                            let n = e.file_name().to_string_lossy().into_owned();
                            let r = if rel.is_empty() {
                                n
                            } else {
                                format!("{rel}/{n}")
                            };
                            match e.file_type() {
                                Ok(t) if t.is_dir() => stack.push((e.path(), r)),
                                Ok(t) if t.is_file() => out.push((
                                    stamp.clone(),
                                    r,
                                    e.metadata().map(|m| m.len()).unwrap_or(0),
                                )),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Location::Remote { account, root } => {
                out = crate::remote::ops::list_trash(account, root)?;
            }
        }
        out.sort();
        Ok(out)
    }

    /// Przywraca plik z kosza na pierwotne miejsce; jeśli tam jest już plik, przywraca pod nazwą z dopiskiem.
    pub fn restore_trash(&self, stamp: &str, rel: &str) -> Result<String, OpError> {
        check_stamp(stamp)?;
        let rel = check_trash_rel(rel)?;
        let from_rel = format!("{TRASH_DIR}/{stamp}/{rel}");
        if let Location::Local(p) = self {
            if !local_full(p, &from_rel).is_file() {
                return Err(OpError::Failed(format!(
                    "Nie znaleziono wersji w koszu: {stamp}/{rel}"
                )));
            }
        }
        if let Location::Remote { account, root } = self {
            return crate::remote::ops::restore_trash(account, root, stamp, rel);
        }
        let target = if self.stat(rel).is_some() {
            names::conflict_name(rel, &format!("przywrócono {stamp}"))
        } else {
            rel.to_string()
        };
        match self {
            Location::Local(p) => {
                let dst = local_full(p, &target);
                if let Some(parent) = dst.parent() {
                    ensure_dir(parent)?;
                }
                fs::copy(local_full(p, &from_rel), dst)?;
            }
            Location::Remote { .. } => unreachable!("obsłużone wyżej"),
        }
        Ok(target)
    }

    /// Usuwa trwale z kosza: jeden plik (`stamp`+`rel`), cały przebieg (`stamp`) albo wszystko (obie wartości puste).
    /// Sam `rel` bez `stamp` jest błędem (nie wolno wtedy kasować całego kosza).
    pub fn delete_trash(&self, stamp: Option<&str>, rel: Option<&str>) -> Result<(), OpError> {
        let target = match (stamp, rel) {
            (Some(s), Some(r)) => {
                check_stamp(s)?;
                format!("{TRASH_DIR}/{s}/{}", check_trash_rel(r)?)
            }
            (Some(s), None) => {
                check_stamp(s)?;
                format!("{TRASH_DIR}/{s}")
            }
            (None, Some(_)) => {
                return Err(OpError::Failed(
                    "Nie podano znacznika czasu przebiegu: nie można usunąć pliku z kosza".into(),
                ))
            }
            (None, None) => TRASH_DIR.to_string(),
        };
        let file = stamp.is_some() && rel.is_some();
        match self {
            Location::Local(p) => {
                let t = local_full(p, &target);
                if !t.exists() {
                    return Ok(());
                }
                if file {
                    Ok(fs::remove_file(t)?)
                } else {
                    Ok(fs::remove_dir_all(t)?)
                }
            }
            Location::Remote { account, root } => {
                crate::remote::ops::delete_trash(account, root, &target, file)
            }
        }
    }

    /// Podfoldery (jeden poziom) wskazanego folderu: do wyboru podfolderów w kreatorze.
    pub fn list_dirs(&self, rel: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        match self {
            Location::Local(p) => {
                let dir = if rel.is_empty() {
                    local_full(p, "")
                } else {
                    resolve_local(p, rel, true, false).map_err(|e| e.to_string())?
                };
                for e in fs::read_dir(&dir)
                    .map_err(|e| format!("{}: {e}", dir.display()))?
                    .flatten()
                {
                    let Ok(n) = e.file_name().into_string() else {
                        continue; // nazwa spoza UTF-8: nie do wybrania w kreatorze
                    };
                    if !(rel.is_empty() && (n == TRASH_DIR || n == LEGACY_TRASH_DIR))
                        && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    {
                        out.push(n);
                    }
                }
            }
            Location::Remote { account, root } => {
                out = crate::remote::ops::list_dirs(account, root, rel)?;
            }
        }
        out.sort_by_key(|n| n.to_lowercase());
        Ok(out)
    }

    /// Usuwa z kosza foldery przebiegów starsze niż `days` dni (po nazwie `YYYY-MM-DD_HHMMSS[-N]`, nie po
    /// mtime plików) oraz nadmiarowe wersje (`max_versions` najnowszych na plik). Obce foldery zostają.
    pub fn purge_trash(&self, days: u32, max_versions: u32) {
        if max_versions > 0 {
            self.cap_versions(max_versions as usize);
        }
        if days == 0 {
            return;
        }
        let Some(limit) = cutoff_date(days) else {
            return; // limit sprzed zakresu dat: nic nie jest wystarczająco stare
        };
        let old = |n: &str| is_stamp(n) && n[..10] < *limit.as_str();
        match self {
            Location::Local(p) => {
                if let Ok(rd) = fs::read_dir(local_full(p, TRASH_DIR)) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().into_owned();
                        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && old(&n) {
                            let _ = fs::remove_dir_all(e.path());
                        }
                    }
                }
            }
            Location::Remote { account, root } => {
                for n in crate::remote::ops::trash_stamps(account, root) {
                    if old(&n) {
                        let _ = self.delete_trash(Some(&n), None);
                    }
                }
            }
        }
    }
}

/// Czy nazwa to znacznik przebiegu: `YYYY-MM-DD_HHMMSS` z opcjonalnym `-N`.
fn is_stamp(n: &str) -> bool {
    let b = n.as_bytes();
    if b.len() < 17 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let base = digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'_'
        && digits(11..17);
    base && (b.len() == 17 || (b[17] == b'-' && b.len() > 18 && digits(18..b.len())))
}

/// Część bazowa znacznika (bez sufiksu `-N`), jeśli to poprawny znacznik przebiegu.
pub(crate) fn stamp_base(stamp: &str) -> &str {
    if is_stamp(stamp) {
        &stamp[..17]
    } else {
        stamp
    }
}

/// Klucz sortowania znaczników: (data, numer kolejny `-N`).
fn stamp_key(n: &str) -> (String, u64) {
    if is_stamp(n) {
        (
            n[..17].to_string(),
            n.get(18..).and_then(|x| x.parse().ok()).unwrap_or(0),
        )
    } else {
        (n.to_string(), 0)
    }
}

/// Znacznik z interfejsu musi być pojedynczym bezpiecznym komponentem ścieżki (bez `..`, `/`).
fn check_stamp(stamp: &str) -> Result<(), OpError> {
    if stamp.contains('/') || stamp.contains('\\') {
        return Err(OpError::Failed(format!(
            "Nieprawidłowy znacznik kosza: {stamp}"
        )));
    }
    names::check_rel(stamp)
        .map_err(|e| OpError::Failed(format!("Nieprawidłowy znacznik kosza: {e}")))
}

/// Ścieżka pliku w koszu: względna (wiodące `/` są ignorowane), bez `..`.
fn check_trash_rel(rel: &str) -> Result<&str, OpError> {
    let rel = rel.trim_start_matches('/');
    names::check_rel(rel)
        .map(|_| rel)
        .map_err(|e| OpError::Failed(format!("Nieprawidłowa ścieżka w koszu: {e}")))
}

/// Data graniczna (`YYYY-MM-DD`) dla retencji `days` dni; None przy przepełnieniu zakresu dat.
fn cutoff_date(days: u32) -> Option<String> {
    let d = chrono::Duration::try_days(days as i64)?;
    let t = chrono::Local::now().checked_sub_signed(d)?;
    Some(t.format("%Y-%m-%d").to_string())
}

impl Location {
    /// Zostawia w koszu tylko `keep` najnowszych wersji każdego pliku (tylko przebiegi o poprawnym znaczniku).
    fn cap_versions(&self, keep: usize) {
        let Ok(items) = self.list_trash() else { return };
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (stamp, rel, _) in items {
            if is_stamp(&stamp) {
                by_file.entry(rel).or_default().push(stamp);
            }
        }
        for (rel, mut stamps) in by_file {
            stamps.sort_by_key(|s| stamp_key(s));
            let extra = stamps.len().saturating_sub(keep);
            for st in stamps.into_iter().take(extra) {
                let _ = self.delete_trash(Some(&st), Some(&rel));
            }
        }
        if let Location::Local(p) = self {
            // puste foldery przebiegów i ich puste podfoldery
            fn prune(dir: &Path) {
                if let Ok(rd) = fs::read_dir(dir) {
                    for e in rd.flatten() {
                        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            prune(&e.path());
                        }
                    }
                }
                let _ = fs::remove_dir(dir); // tylko gdy pusty
            }
            if let Ok(rd) = fs::read_dir(local_full(p, TRASH_DIR)) {
                for e in rd.flatten() {
                    if is_stamp(&e.file_name().to_string_lossy())
                        && e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    {
                        prune(&e.path());
                    }
                }
            }
        }
    }
}

// ---------- lokalne ----------

/// Pełna ścieżka lokalna; na Windows z prefiksem `\\?\` (ścieżki dłuższe niż 260 znaków, UNC).
/// Wynik zawsze leży pod `root`: puste komponenty, `.` i `..` są pomijane, a na Windows także `\` (jako separator)
/// i litery dysku (`C:`). Do zapisu używaj `prepare_target` (odrzuca takie ścieżki z błędem).
pub fn local_full(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    let sep: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    for part in rel
        .split(sep)
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
    {
        if cfg!(windows) && part.contains(':') {
            continue; // litera dysku / strumień NTFS: nie wychodzimy z korzenia
        }
        p.push(part);
    }
    fs_path(&p)
}

/// Walidacja ścieżki względnej dla operacji lokalnych (jak `names::check_rel`, ale znaki sterujące w nazwach
/// plików lokalnych są dozwolone).
pub(crate) fn check_local_rel(rel: &str) -> Result<(), OpError> {
    let cleaned: String = rel
        .chars()
        .map(|c| if (c as u32) < 32 { '_' } else { c })
        .collect();
    names::check_rel(&cleaned).map_err(|e| OpError::Failed(format!("niebezpieczna ścieżka ({e})")))
}

/// Zwraca pełną ścieżkę `rel` pod `root` po sprawdzeniu, że żaden komponent nie jest dowiązaniem symbolicznym
/// (zapis/odczyt nie może wyjść poza korzeń). `include_last` = sprawdź też ostatni komponent. `create` = utwórz
/// brakujące foldery nadrzędne (pojedynczo, bez podążania za dowiązaniami).
/// Błąd pojedynczej próby `resolve_local_once`: błąd domenowy (bez ponowień) albo błąd systemu plików.
enum ResolveErr {
    Op(OpError),
    Io(io::Error),
}

/// Jak `resolve_local_once`, ale ponawia próbę przy przejściowych błędach wywołanych równoległym usuwaniem
/// pustych folderów (patrz `transient_dir_error`).
fn resolve_local(
    root: &Path,
    rel: &str,
    include_last: bool,
    create: bool,
) -> Result<PathBuf, OpError> {
    let mut tries = 0;
    loop {
        match resolve_local_once(root, rel, include_last, create) {
            Ok(p) => return Ok(p),
            Err(ResolveErr::Op(e)) => return Err(e),
            Err(ResolveErr::Io(e)) if create && transient_dir_error(&e) && tries < 8 => {
                tries += 1;
                sleep(Duration::from_millis(2));
            }
            Err(ResolveErr::Io(e)) => return Err(e.into()),
        }
    }
}

fn resolve_local_once(
    root: &Path,
    rel: &str,
    include_last: bool,
    create: bool,
) -> Result<PathBuf, ResolveErr> {
    check_local_rel(rel).map_err(ResolveErr::Op)?;
    let parts: Vec<&str> = rel.split('/').collect();
    let n = if include_last {
        parts.len()
    } else {
        parts.len() - 1
    };
    let mut cur = fs_path(root);
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        if i >= n {
            continue;
        }
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(ResolveErr::Op(OpError::Failed(format!(
                    "odmowa zapisu: „{}” jest dowiązaniem symbolicznym (mogłoby wyprowadzić poza folder synchronizacji)",
                    parts[..=i].join("/")
                ))))
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(ResolveErr::Op(OpError::Failed(format!(
                    "„{}” nie jest folderem",
                    parts[..=i].join("/")
                ))))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                match fs::create_dir(&cur) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(ResolveErr::Io(e)),
                }
            }
            Err(e) => return Err(ResolveErr::Io(e)),
        }
    }
    Ok(cur)
}

/// Pełna ścieżka pliku `rel` pod `root`, bezpieczna do zapisu/usunięcia (patrz `resolve_local`).
fn prepare_target(root: &Path, rel: &str, create: bool) -> Result<PathBuf, OpError> {
    resolve_local(root, rel, false, create)
}

/// Przejściowe błędy tworzenia folderu/pliku w folderze, który właśnie jest usuwany przez inny wątek
/// (na Windows folder w stanie „delete pending” daje „odmowa dostępu”, a na macOS „nieprawidłowy argument”
/// zamiast „nie znaleziono”).
fn transient_dir_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::AlreadyExists
            | io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidInput
    )
}

/// `create_dir_all` odporne na wyścig z równoległym sprzątaniem pustych folderów (`remove_empty_parents`):
/// między utworzeniem a sprawdzeniem folder mógł zniknąć, co daje przejściowe `AlreadyExists`/`NotFound` (na Windows także `PermissionDenied`).
pub(crate) fn ensure_dir(path: &Path) -> io::Result<()> {
    let mut last = None;
    for _ in 0..8 {
        match fs::create_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if transient_dir_error(&e) => {
                last = Some(e);
                sleep(Duration::from_millis(2));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("nie można utworzyć folderu")))
}

/// Po usunięciu pliku sprząta puste foldery nadrzędne aż do korzenia (samego korzenia nigdy).
fn remove_empty_parents(root: &Path, rel: &str) {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    while !parts.is_empty() {
        let p = local_full(root, &parts.join("/"));
        if fs::remove_dir(&p).is_err() {
            break;
        }
        parts.pop();
    }
}

#[allow(unused_variables)]
pub fn fs_path(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy().replace('/', "\\");
        if s.starts_with(r"\\?\") {
            return PathBuf::from(s);
        }
        if let Some(r) = s.strip_prefix(r"\\") {
            return PathBuf::from(format!(r"\\?\UNC\{r}"));
        }
        if p.is_absolute() {
            return PathBuf::from(format!(r"\\?\{s}"));
        }
    }
    p.to_path_buf()
}

pub(crate) fn meta_of(m: &fs::Metadata) -> Option<Meta> {
    if !m.is_file() {
        return None;
    }
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(Meta {
        size: m.len(),
        mtime,
    })
}

type Scan = (Vec<(String, Meta)>, Vec<String>);

fn list_local(root: &Path) -> Result<Scan, String> {
    let base = fs_path(root);
    if !base.is_dir() {
        return Err(format!("folder niedostępny: {}", root.display()));
    }
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut stack = vec![(base, String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for ent in rd {
            let ent = ent.map_err(|e| format!("{}: {e}", dir.display()))?;
            let Ok(name) = ent.file_name().into_string() else {
                skipped.push(format!(
                    "pominięto element o nazwie spoza UTF-8: {}/{}",
                    rel,
                    ent.file_name().to_string_lossy()
                ));
                continue;
            };
            let ft = ent.file_type().map_err(|e| format!("{name}: {e}"))?;
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                // kosz synchronizacji pomijamy tylko w korzeniu
                if rel.is_empty() && (name == TRASH_DIR || name == LEGACY_TRASH_DIR) {
                    continue;
                }
                let r = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                stack.push((ent.path(), r));
                continue;
            }
            if name.ends_with(".syncron.tmp") || name.ends_with(LEGACY_TMP_SUFFIX) {
                // pozostałość po przerwanym kopiowaniu (zerwane połączenie, restart): sprzątamy, gdy jest stara
                if let Ok(age) = ent
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t.elapsed().unwrap_or_default())
                {
                    if age > Duration::from_secs(3600) {
                        let _ = fs::remove_file(ent.path());
                    }
                }
                continue;
            }
            if names::is_temp(&name) {
                continue;
            }
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_file() {
                let md = ent.metadata().map_err(|e| format!("{r}: {e}"))?;
                if let Some(m) = meta_of(&md) {
                    out.push((r, m));
                }
            }
        }
    }
    Ok((out, skipped))
}

pub(crate) fn open_shared(p: &Path) -> io::Result<File> {
    #[allow(unused_mut)]
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(7); // czytaj mimo otwarcia w innym programie
    }
    o.open(p)
}

/// Tworzy unikalny plik tymczasowy obok `dst` (`create_new`: nie podąża za istniejącym dowiązaniem/plikiem; kolizja
/// nazwy = kolejny sufiks, cudzych plików nie usuwamy). Nazwa ma ograniczoną długość niezależnie od długości nazwy docelowej.
pub(crate) fn create_new_tmp(dst: &Path) -> io::Result<(File, PathBuf)> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let fname = dst
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let h = blake3::hash(fname.as_bytes()).to_hex();
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    let mut last = None;
    for _ in 0..16 {
        let tmp = dst.with_file_name(format!(
            ".{}-{:x}-{:x}.syncron.tmp",
            &h[..12],
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        match o.open(&tmp) {
            Ok(f) => return Ok((f, tmp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("nie można utworzyć pliku tymczasowego")))
}

pub(crate) fn copy_local(src: &Path, dst: &Path, verify: bool, bw: &Bandwidth) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        ensure_dir(parent)?;
    }
    let tmp_path = std::cell::RefCell::new(None::<PathBuf>);
    let mut inp = open_shared(src)?;
    let before = inp.metadata()?;
    let res = (|| {
        // równoległe sprzątanie pustych folderów (po usunięciu pliku) może zabrać folder tuż przed utworzeniem
        // pliku tymczasowego: wtedy tworzymy folder ponownie (gdy plik tymczasowy już istnieje, folder nie jest pusty)
        let mut tries = 0;
        let (mut out, tmp) = loop {
            match create_new_tmp(dst) {
                Err(e) if transient_dir_error(&e) && tries < 8 => {
                    tries += 1;
                    sleep(Duration::from_millis(2));
                    if let Some(parent) = dst.parent() {
                        ensure_dir(parent)?;
                    }
                }
                other => break other?,
            }
        };
        *tmp_path.borrow_mut() = Some(tmp.clone());
        let mut hasher = blake3::Hasher::new();
        if verify || bw.current_kbps() > 0 || bw.hours.is_some() {
            let mut buf = vec![0u8; 256 * 1024];
            let started = std::time::Instant::now();
            let mut sent: u64 = 0;
            loop {
                let n = io::Read::read(&mut inp, &mut buf)?;
                if n == 0 {
                    break;
                }
                if verify {
                    hasher.update(&buf[..n]);
                }
                io::Write::write_all(&mut out, &buf[..n])?;
                sent += n as u64;
                let kbps = bw.current_kbps();
                if kbps > 0 {
                    // ogranicznik: czekamy tyle, ile powinno trwać wysłanie dotychczasowych bajtów
                    let due = Duration::from_secs_f64(sent as f64 / (kbps as f64 * 1024.0));
                    if let Some(wait) = due.checked_sub(started.elapsed()) {
                        sleep(wait);
                    }
                }
            }
        } else {
            io::copy(&mut inp, &mut out)?;
        }
        out.sync_all()?;
        drop(out);
        if verify {
            let mut written = blake3::Hasher::new();
            io::copy(&mut File::open(&tmp)?, &mut written)?;
            if written.finalize() != hasher.finalize() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "suma kontrolna zapisanego pliku nie zgadza się ze źródłem",
                ));
            }
        }
        let after = fs::metadata(src)?;
        if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "plik zmienił się w trakcie kopiowania",
            ));
        }
        filetime::set_file_mtime(
            &tmp,
            filetime::FileTime::from_last_modification_time(&before),
        )?;
        // pusty folder w miejscu pliku docelowego (plik↔folder) nie blokuje zapisu
        if fs::symlink_metadata(dst)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(dst);
        }
        fs::rename(&tmp, dst)
    })();
    if res.is_err() {
        if let Some(t) = tmp_path.borrow().as_ref() {
            let _ = fs::remove_file(t);
        }
    }
    res
}

/// Nazwa konta podawana z aplikacji (patrz `accounts::valid_name`).
pub fn valid_remote_name(n: &str) -> bool {
    crate::accounts::valid_name(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_locations() {
        assert!(matches!(Location::parse("C:\\Dane"), Location::Local(_)));
        assert!(matches!(Location::parse("C:"), Location::Local(_)));
        assert!(matches!(
            Location::parse("\\\\nas\\udział"),
            Location::Local(_)
        ));
        assert!(matches!(
            Location::parse("/Volumes/Dysk"),
            Location::Local(_)
        ));
        assert!(matches!(
            Location::parse("gdrive:Dokumenty"),
            Location::Remote { .. }
        ));
        assert!(matches!(
            Location::parse("bitrix:"),
            Location::Remote { .. }
        ));
    }
}
