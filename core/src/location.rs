//! Lokalizacje: folder lokalny/UNC albo zdalny (rclone). Listowanie i operacje na plikach.
use crate::model::Bandwidth;
use crate::names;
use crate::plan::Meta;
use crate::rules::{FileInfo, RuleSet};
use crate::vss::Vss;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
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

fn is_busy(e: &io::Error) -> bool {
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
    Rclone(String),
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
                return Location::Rclone(format!("{name}:{rest}"));
            }
        }
        Location::Local(PathBuf::from(s))
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Location::Local(_))
    }

    fn full(&self, rel: &str) -> String {
        match self {
            Location::Local(p) => local_full(p, rel).to_string_lossy().into_owned(),
            Location::Rclone(b) => {
                if rel.is_empty() {
                    b.clone()
                } else if b.ends_with(':') || b.ends_with('/') {
                    format!("{b}{rel}")
                } else {
                    format!("{b}/{rel}")
                }
            }
        }
    }

    pub fn list(&self, rules: &RuleSet) -> Result<Listing, String> {
        let (raw, skipped) = match self {
            Location::Local(root) => list_local(root)?,
            Location::Rclone(base) => list_rclone(base)?,
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
        match self {
            Location::Local(p) => meta_of(&fs::metadata(local_full(p, rel)).ok()?),
            Location::Rclone(_) => {
                let out = run_rclone(&["lsjson", "--no-mimetype", &self.full(rel)]).ok()?;
                parse_lsjson(&out, true)
                    .ok()?
                    .into_iter()
                    .next()
                    .map(|(_, m)| m)
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
            _ => {
                if let Location::Local(b) = self {
                    prepare_target(b, to_rel, true)?;
                }
                if let Location::Local(_) = from {
                    check_local_rel(from_rel)?;
                }
                // lokalne źródło zablokowane przez inny program: rclone dostaje kopię z migawki VSS
                let mut source = from.full(from_rel);
                let mut temp = None;
                if let Location::Local(a) = from {
                    let src = local_full(a, from_rel);
                    if let Err(e) = open_shared(&src) {
                        if is_busy(&e) {
                            let snap = o
                                .vss
                                .snapshot_path(&src)
                                .map_err(|n| OpError::Busy(format!("{e}; {n}")))?;
                            static N: AtomicUsize = AtomicUsize::new(0);
                            let t = std::env::temp_dir().join("syncron-vss").join(format!(
                                "{}-{}",
                                std::process::id(),
                                N.fetch_add(1, Ordering::SeqCst)
                            ));
                            copy_local(&snap, &t, false, &Bandwidth::default())?;
                            source = t.to_string_lossy().into_owned();
                            temp = Some(t);
                        }
                    }
                }
                let bw = o.bw.rclone_arg();
                let mut args = vec!["copyto", source.as_str(), ""];
                let dest = self.full(to_rel);
                args[2] = dest.as_str();
                if let Some(b) = &bw {
                    args.extend(["--bwlimit", b.as_str()]);
                }
                let res = rclone_op(&args);
                if let Some(t) = temp {
                    let _ = fs::remove_file(t);
                }
                res
            }
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
            Location::Rclone(_) => rclone_op(&["moveto", &self.full(from_rel), &self.full(to_rel)]),
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
            Location::Rclone(_) => run_rclone(&["cat", &self.full(rel)]).ok(),
        }
    }

    /// Kopiuje plik do kosza synchronizacji (przed nadpisaniem). Nie nadpisuje istniejącej wersji w koszu:
    /// gdy `stamp` jest już zajęty dla tego pliku, użyty zostaje wolny wariant `stamp-N`.
    pub fn backup(&self, rel: &str, stamp: &str) -> Result<(), OpError> {
        check_stamp(stamp)?;
        let trash_rel = self.free_trash_path(stamp, rel);
        match self {
            Location::Local(p) => {
                let dst = local_full(p, &trash_rel);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(local_full(p, rel), dst)?;
                Ok(())
            }
            Location::Rclone(_) => rclone_op(&["copyto", &self.full(rel), &self.full(&trash_rel)]),
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
                Location::Rclone(_) => rclone_op(&["deletefile", &self.full(rel)]),
            };
        }
        check_stamp(stamp)?;
        let trash_rel = self.free_trash_path(stamp, rel);
        match self {
            Location::Local(p) => {
                let src = prepare_target(p, rel, false)?;
                let dst = prepare_target(p, &trash_rel, true)?;
                if fs::rename(&src, &dst).is_err() {
                    fs::copy(&src, &dst)?;
                    fs::remove_file(&src)?;
                }
                remove_empty_parents(p, rel);
                Ok(())
            }
            Location::Rclone(_) => rclone_op(&["moveto", &self.full(rel), &self.full(&trash_rel)]),
        }
    }

    /// Ścieżka w koszu (`.syncron-trash/<stamp>[-N]/<rel>`), która jeszcze nie istnieje.
    fn free_trash_path(&self, stamp: &str, rel: &str) -> String {
        let base = if is_stamp(stamp) { &stamp[..17] } else { stamp };
        let mut cand = stamp.to_string();
        for n in 1..10_000u32 {
            let path = format!("{TRASH_DIR}/{cand}/{rel}");
            let taken = match self {
                Location::Local(p) => local_full(p, &path).symlink_metadata().is_ok(),
                Location::Rclone(_) => self.stat(&path).is_some(),
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
            Location::Rclone(_) => {
                let base = self.full(TRASH_DIR);
                let raw =
                    match run_rclone(&["lsjson", "-R", "--files-only", "--no-mimetype", &base]) {
                        Ok(s) => s,
                        Err(e) if e.to_lowercase().contains("directory not found") => {
                            return Ok(out)
                        }
                        Err(e) => return Err(e),
                    };
                for (path, m) in parse_lsjson(&raw, false)? {
                    if let Some((stamp, rel)) = path.split_once('/') {
                        out.push((stamp.to_string(), rel.to_string(), m.size));
                    }
                }
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
        let target = if self.stat(rel).is_some() {
            names::conflict_name(rel, &format!("przywrócono {stamp}"))
        } else {
            rel.to_string()
        };
        match self {
            Location::Local(p) => {
                let dst = local_full(p, &target);
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(local_full(p, &from_rel), dst)?;
            }
            Location::Rclone(_) => {
                rclone_op(&["copyto", &self.full(&from_rel), &self.full(&target)])?
            }
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
            Location::Rclone(_) => {
                let r = if file {
                    rclone_op(&["deletefile", &self.full(&target)])
                } else {
                    rclone_op(&["purge", &self.full(&target)])
                };
                match r {
                    Err(OpError::Failed(e))
                        if e.to_lowercase().contains("directory not found")
                            || e.to_lowercase().contains("object not found") =>
                    {
                        Ok(())
                    }
                    other => other,
                }
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
            Location::Rclone(_) => {
                if !rel.is_empty() {
                    names::check_rel(rel)?;
                }
                let raw = run_rclone(&["lsjson", "--dirs-only", "--no-mimetype", &self.full(rel)])?;
                let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                for it in v.as_array().into_iter().flatten() {
                    if let Some(n) = it["Name"]
                        .as_str()
                        .filter(|n| {
                            !(rel.is_empty() && (*n == TRASH_DIR || *n == LEGACY_TRASH_DIR))
                        })
                        .filter(|n| names::check_rel(n).is_ok())
                    {
                        out.push(n.to_string());
                    }
                }
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
            Location::Rclone(_) => {
                let Ok(raw) = run_rclone(&[
                    "lsjson",
                    "--dirs-only",
                    "--no-mimetype",
                    &self.full(TRASH_DIR),
                ]) else {
                    return;
                };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                    return;
                };
                for it in v.as_array().into_iter().flatten() {
                    if let Some(n) = it["Name"].as_str().filter(|n| old(n)) {
                        let _ = self.delete_trash(Some(n), None);
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
fn check_local_rel(rel: &str) -> Result<(), OpError> {
    let cleaned: String = rel
        .chars()
        .map(|c| if (c as u32) < 32 { '_' } else { c })
        .collect();
    names::check_rel(&cleaned).map_err(|e| OpError::Failed(format!("niebezpieczna ścieżka ({e})")))
}

/// Zwraca pełną ścieżkę `rel` pod `root` po sprawdzeniu, że żaden komponent nie jest dowiązaniem symbolicznym
/// (zapis/odczyt nie może wyjść poza korzeń). `include_last` = sprawdź też ostatni komponent. `create` = utwórz
/// brakujące foldery nadrzędne (pojedynczo, bez podążania za dowiązaniami).
fn resolve_local(
    root: &Path,
    rel: &str,
    include_last: bool,
    create: bool,
) -> Result<PathBuf, OpError> {
    check_local_rel(rel)?;
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
                return Err(OpError::Failed(format!(
                    "odmowa zapisu: „{}” jest dowiązaniem symbolicznym (mogłoby wyprowadzić poza folder synchronizacji)",
                    parts[..=i].join("/")
                )))
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(OpError::Failed(format!(
                    "„{}” nie jest folderem",
                    parts[..=i].join("/")
                )))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                match fs::create_dir(&cur) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(cur)
}

/// Pełna ścieżka pliku `rel` pod `root`, bezpieczna do zapisu/usunięcia (patrz `resolve_local`).
fn prepare_target(root: &Path, rel: &str, create: bool) -> Result<PathBuf, OpError> {
    resolve_local(root, rel, false, create)
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

fn meta_of(m: &fs::Metadata) -> Option<Meta> {
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

fn open_shared(p: &Path) -> io::Result<File> {
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
fn create_new_tmp(dst: &Path) -> io::Result<(File, PathBuf)> {
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

fn copy_local(src: &Path, dst: &Path, verify: bool, bw: &Bandwidth) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = std::cell::RefCell::new(None::<PathBuf>);
    let mut inp = open_shared(src)?;
    let before = inp.metadata()?;
    let res = (|| {
        let (mut out, tmp) = create_new_tmp(dst)?;
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

// ---------- rclone ----------

fn rclone_bin() -> String {
    std::env::var("SYNCRON_RCLONE").unwrap_or_else(|_| "rclone".into())
}

/// Limit czasu jednego wywołania rclone. `SYNCRON_RCLONE_TIMEOUT` (sekundy) nadpisuje wartości domyślne:
/// kopiowanie/przenoszenie – 6 godzin, konfiguracja (logowanie w przeglądarce) – 15 minut, reszta – 10 minut.
fn rclone_timeout(op: &str) -> Duration {
    if let Some(n) = std::env::var("SYNCRON_RCLONE_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
    {
        return Duration::from_secs(n);
    }
    Duration::from_secs(match op {
        "copyto" | "moveto" | "copy" | "move" | "sync" => 6 * 3600,
        "config" => 900,
        _ => 600,
    })
}

/// Uruchamia rclone bez stdin, z limitem czasu. Zob. `run_rclone_with`.
pub fn run_rclone(args: &[&str]) -> Result<String, String> {
    run_rclone_with(args, &[], None)
}

/// Jak `run_rclone`, ale z dodatkowymi zmiennymi środowiskowymi procesu rclone (np. sekrety, które nie mogą
/// trafić do argumentów widocznych na liście procesów) i opcjonalnymi danymi na stdin.
pub fn run_rclone_with(
    args: &[&str],
    env: &[(String, String)],
    stdin_data: Option<&str>,
) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    let op = args.first().copied().unwrap_or("");
    let limit = rclone_timeout(op);
    let mut c = Command::new(rclone_bin());
    c.args(args)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // limity połączenia/bezczynności dla operacji sieciowych (przez środowisko: nie zmieniają listy argumentów)
    if matches!(
        op,
        "lsjson" | "lsd" | "copyto" | "moveto" | "deletefile" | "cat" | "purge" | "delete"
    ) {
        for (k, v) in [("RCLONE_CONTIMEOUT", "30s"), ("RCLONE_TIMEOUT", "120s")] {
            if std::env::var_os(k).is_none() {
                c.env(k, v);
            }
        }
    }
    for (k, v) in env {
        c.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // bez okna konsoli
    }
    let mut child = c.spawn().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            format!(
                "nie można uruchomić rclone: nie znaleziono programu „{}”; zainstaluj rclone albo wskaż go w zmiennej SYNCRON_RCLONE",
                rclone_bin()
            )
        } else {
            format!("nie można uruchomić rclone: {e}")
        }
    })?;
    if let (Some(data), Some(mut si)) = (stdin_data, child.stdin.take()) {
        let _ = si.write_all(data.as_bytes());
    }
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let mut readers = 0;
    if let Some(mut so) = child.stdout.take() {
        let tx = tx.clone();
        readers += 1;
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = so.read_to_end(&mut b);
            let _ = tx.send((true, b));
        });
    }
    if let Some(mut se) = child.stderr.take() {
        let tx = tx.clone();
        readers += 1;
        std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = se.read_to_end(&mut b);
            let _ = tx.send((false, b));
        });
    }
    drop(tx);
    let start = std::time::Instant::now();
    let mut nap = 1u64;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() >= limit {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "rclone ({op}) nie odpowiedział w ciągu {} s i został przerwany (limit czasu; zmienna SYNCRON_RCLONE_TIMEOUT)",
                        limit.as_secs()
                    ));
                }
                sleep(Duration::from_millis(nap));
                nap = (nap * 2).min(20);
            }
            Err(e) => return Err(format!("nie można sprawdzić stanu rclone: {e}")),
        }
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..readers {
        // proces potomny mógł zostawić proces trzymający rury: nie czekaj w nieskończoność
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok((true, b)) => stdout = b,
            Ok((false, b)) => stderr = b,
            Err(_) => break,
        }
    }
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&stderr).trim().to_string();
        if !err.is_empty() {
            return Err(err);
        }
        let out = String::from_utf8_lossy(&stdout);
        let out: String = out.trim().chars().take(300).collect();
        let code = match status.code() {
            Some(c) => format!("kodem {c}"),
            None => "przez sygnał".to_string(),
        };
        Err(if out.is_empty() {
            format!("rclone ({op}) zakończył się {code} bez komunikatu błędu")
        } else {
            format!("rclone ({op}) zakończył się {code}: {out}")
        })
    }
}

fn rclone_op(args: &[&str]) -> Result<(), OpError> {
    run_rclone(args).map(|_| ()).map_err(|e| {
        // tylko znane frazy (nie podciąg z nazwy pliku, np. „unlocked.txt”)
        let l = e.to_lowercase();
        if l.contains("used by another process")
            || l.contains("sharing violation")
            || l.contains("lock violation")
            || l.contains("file is locked")
            || l.contains("is locked by")
        {
            OpError::Busy(e)
        } else {
            OpError::Failed(e)
        }
    })
}

/// Nazwa konta rclone podawana z aplikacji: min. 2 znaki, zaczyna się od litery/cyfry ASCII.
pub fn valid_remote_name(n: &str) -> bool {
    n.chars().count() >= 2
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Dozwolone rodzaje kont tworzonych z aplikacji (`local` celowo nie).
pub const REMOTE_KINDS: &[&str] = &["drive", "onedrive", "webdav", "sftp", "crypt"];

/// Klucz parametru konta: tylko litery ASCII, cyfry i `_` (żadne `--opcje`).
pub fn valid_param_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_lsjson(s: &str, skip_special: bool) -> Result<Vec<(String, Meta)>, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for it in v.as_array().ok_or("zła odpowiedź rclone")? {
        if it["IsDir"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(path) = it["Path"].as_str().map(str::to_string) else {
            continue; // wpis bez ścieżki
        };
        let sz = &it["Size"];
        let size = if let Some(n) = sz.as_i64().filter(|n| *n >= 0) {
            n as u64
        } else if sz.as_u64().is_some() {
            // powyżej i64::MAX: nie ukrywaj tego jako „plik zniknął”
            return Err(format!(
                "rclone zwrócił rozmiar pliku „{path}” poza zakresem ({sz}); przerwano listowanie"
            ));
        } else if sz.is_null() || sz.as_i64().is_some() {
            // rozmiar nieznany (-1 lub brak): pomijamy tylko prawdziwe dokumenty bez rozmiaru
            // (Google Docs: application/vnd.google-apps.*, albo wpis bez rozmiaru i daty);
            // inny plik bez rozmiaru to błąd listingu, żeby nie wyglądał na usunięty
            let google_doc = it["MimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("application/vnd.google-apps"));
            if google_doc || it["ModTime"].is_null() {
                continue;
            }
            return Err(format!(
                "rclone nie podał rozmiaru pliku „{path}” (Size={sz}); przerwano listowanie, by plik nie wyglądał na usunięty"
            ));
        } else {
            continue; // wartość nieliczbowa/ułamkowa: wpis uszkodzony
        };
        let name = path.rsplit('/').next().unwrap_or("");
        if skip_special
            && (path.starts_with(&format!("{TRASH_DIR}/"))
                || path.starts_with(&format!("{LEGACY_TRASH_DIR}/"))
                || names::is_temp(name))
        {
            continue;
        }
        // brak/nieczytelny ModTime: stała wartość 0 (stabilna między przebiegami, nie „teraz”)
        let mtime = it["ModTime"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp())
            .unwrap_or(0);
        out.push((path, Meta { size, mtime }));
    }
    Ok(out)
}

fn list_rclone(base: &str) -> Result<Scan, String> {
    match run_rclone(&["lsjson", "-R", "--files-only", base]) {
        Ok(s) => {
            // ścieżki ze zdalnego listingu to dane z zewnątrz: odrzucamy `..`, bezwzględne, puste itp.
            let mut skipped = Vec::new();
            let ok = parse_lsjson(&s, true)?
                .into_iter()
                // wiodący '/' (np. "/abs/x") traktujemy jak ścieżkę względną korzenia
                .map(|(p, m)| (p.strip_prefix('/').map(str::to_string).unwrap_or(p), m))
                .filter(|(p, _)| match names::check_rel(p) {
                    Ok(()) => true,
                    Err(e) => {
                        skipped.push(format!(
                            "pominięto niebezpieczną ścieżkę ze zdalnego listingu: {e}"
                        ));
                        false
                    }
                })
                .collect();
            Ok((ok, skipped))
        }
        Err(e) if e.to_lowercase().contains("directory not found") => Ok((vec![], vec![])),
        Err(e) => Err(e),
    }
}

/// Czy konto o tej nazwie jest już w konfiguracji rclone (`listremotes`).
pub fn remote_exists(name: &str) -> Result<bool, String> {
    let out = run_rclone(&["listremotes"])?;
    Ok(out
        .lines()
        .any(|l| l.trim().strip_suffix(':').unwrap_or(l.trim()) == name))
}

/// Zaciemnia hasło poleceniem `rclone obscure -` (hasło idzie przez stdin, nie przez argumenty).
pub fn rclone_obscure(plain: &str) -> Result<String, String> {
    run_rclone_with(&["obscure", "-"], &[], Some(plain)).map(|s| s.trim().to_string())
}

/// Ścieżka pliku konfiguracji rclone: `RCLONE_CONFIG` albo wynik `rclone config file`.
pub fn rclone_config_path() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("RCLONE_CONFIG").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let out = run_rclone(&["config", "file"])?;
    out.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "nie można ustalić pliku konfiguracji rclone".to_string())
}

/// Zapisuje klucze w sekcji `[name]` pliku konfiguracji rclone (tworzy sekcję, zastępuje istniejące klucze).
/// Służy do przekazania sekretów bez umieszczania ich w argumentach procesu rclone.
pub fn set_remote_keys(conf: &Path, name: &str, kv: &[(String, String)]) -> Result<(), String> {
    for (k, v) in kv {
        if !valid_param_key(k) || v.contains(['\n', '\r']) {
            return Err(format!("niepoprawny parametr konta „{k}”"));
        }
    }
    let old = fs::read_to_string(conf).unwrap_or_default();
    let header = format!("[{name}]");
    let mut out: Vec<String> = Vec::new();
    let mut in_sec = false;
    let mut found = false;
    let mut pending: Vec<String> = Vec::new(); // puste linie na końcu sekcji
    let flush = |out: &mut Vec<String>, pending: &mut Vec<String>, in_sec: bool, found: bool| {
        if in_sec && found {
            for (k, v) in kv {
                out.push(format!("{k} = {v}"));
            }
        }
        out.append(pending);
    };
    for line in old.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_sec {
                flush(&mut out, &mut pending, in_sec, found);
                in_sec = false;
            }
            if t == header {
                in_sec = true;
                found = true;
            }
            out.push(line.to_string());
            continue;
        }
        if in_sec {
            let key = t.split('=').next().unwrap_or("").trim();
            if kv.iter().any(|(k, _)| k == key) {
                continue; // zastępowany klucz
            }
            if t.is_empty() {
                pending.push(line.to_string());
                continue;
            }
            out.append(&mut pending);
        }
        out.push(line.to_string());
    }
    if in_sec {
        flush(&mut out, &mut pending, in_sec, found);
    } else if !found {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(header);
        for (k, v) in kv {
            out.push(format!("{k} = {v}"));
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    if let Some(parent) = conf.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = conf.with_extension("syncron-tmp");
    {
        use std::io::Write;
        let mut o = OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, conf).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
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
            Location::Rclone(_)
        ));
        assert!(matches!(Location::parse("bitrix:"), Location::Rclone(_)));
    }

    #[test]
    fn lsjson_parse() {
        let j = r#"[{"Path":"a/zażółć.txt","Size":3,"ModTime":"2026-10-27T10:00:00.123Z","IsDir":false},
                    {"Path":"a","Size":-1,"IsDir":true},{"Path":"~$x.docx","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#;
        let v = parse_lsjson(j, true).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "a/zażółć.txt");
        assert_eq!(v[0].1.size, 3);
    }
}
