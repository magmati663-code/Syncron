//! Lokalizacje: folder lokalny/UNC albo zdalny (rclone). Listowanie i operacje na plikach.
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const TRASH_DIR: &str = ".syncpolak-trash";

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
    e.kind() == io::ErrorKind::PermissionDenied || e.kind() == io::ErrorKind::Interrupted
}

/// Opcje kopiowania w ramach jednego przebiegu.
pub struct Opts {
    pub vss: Vss,
    /// Po zapisie porównaj sumę kontrolną BLAKE3 źródła i pliku docelowego.
    pub verify: bool,
}

impl Opts {
    pub fn new(use_vss: bool, verify: bool) -> Self {
        Opts {
            vss: Vss::new(use_vss),
            verify,
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
        let s = s.trim();
        if let Some(i) = s.find(':') {
            let name = &s[..i];
            let rest = &s[i + 1..];
            // litera dysku (C:\ , C:/) to ścieżka lokalna; nazwa zdalna ma min. 2 znaki
            let drive = name.len() == 1
                && (rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/'));
            if name.len() >= 2
                && !drive
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || "_-. ".contains(c))
            {
                return Location::Rclone(s.to_string());
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
        let raw = match self {
            Location::Local(root) => list_local(root)?,
            Location::Rclone(base) => list_rclone(base)?,
        };
        let mut out = Listing::default();
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
                let (src, dst) = (local_full(a, from_rel), local_full(b, to_rel));
                let mut last = None;
                for delay in [0u64, 300, 1500, 4000] {
                    if delay > 0 {
                        sleep(Duration::from_millis(delay));
                    }
                    match copy_local(&src, &dst, o.verify) {
                        Ok(()) => return Ok(()),
                        Err(e) if is_busy(&e) => {
                            // plik zablokowany przez inny program: odczyt z migawki VSS (Windows, administrator)
                            if let Ok(snap) = o.vss.snapshot_path(&src) {
                                match copy_local(&snap, &dst, o.verify) {
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
                    if let Some(parent) = local_full(b, to_rel).parent() {
                        fs::create_dir_all(parent)?;
                    }
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
                            let t = std::env::temp_dir().join("syncpolak-vss").join(format!(
                                "{}-{}",
                                std::process::id(),
                                N.fetch_add(1, Ordering::SeqCst)
                            ));
                            copy_local(&snap, &t, false)?;
                            source = t.to_string_lossy().into_owned();
                            temp = Some(t);
                        }
                    }
                }
                let res = rclone_op(&["copyto", &source, &self.full(to_rel)]);
                if let Some(t) = temp {
                    let _ = fs::remove_file(t);
                }
                res
            }
        }
    }

    /// Kopiuje plik do kosza synchronizacji (przed nadpisaniem).
    pub fn backup(&self, rel: &str, stamp: &str) -> Result<(), OpError> {
        let trash_rel = format!("{TRASH_DIR}/{stamp}/{rel}");
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
    pub fn remove(&self, rel: &str, stamp: &str) -> Result<(), OpError> {
        if stamp.is_empty() {
            return match self {
                Location::Local(p) => Ok(fs::remove_file(local_full(p, rel))?),
                Location::Rclone(_) => rclone_op(&["deletefile", &self.full(rel)]),
            };
        }
        let trash_rel = format!("{TRASH_DIR}/{stamp}/{rel}");
        match self {
            Location::Local(p) => {
                let (src, dst) = (local_full(p, rel), local_full(p, &trash_rel));
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                if fs::rename(&src, &dst).is_err() {
                    fs::copy(&src, &dst)?;
                    fs::remove_file(&src)?;
                }
                Ok(())
            }
            Location::Rclone(_) => rclone_op(&["moveto", &self.full(rel), &self.full(&trash_rel)]),
        }
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
                    let stamp = st.file_name().to_string_lossy().into_owned();
                    let mut stack = vec![(st.path(), String::new())];
                    while let Some((dir, rel)) = stack.pop() {
                        for e in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
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
        let from_rel = format!("{TRASH_DIR}/{stamp}/{rel}");
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

    /// Usuwa trwale z kosza: jeden plik (`stamp`+`rel`), cały przebieg (`stamp`) albo wszystko.
    pub fn delete_trash(&self, stamp: Option<&str>, rel: Option<&str>) -> Result<(), OpError> {
        let target = match (stamp, rel) {
            (Some(s), Some(r)) => format!("{TRASH_DIR}/{s}/{r}"),
            (Some(s), None) => format!("{TRASH_DIR}/{s}"),
            _ => TRASH_DIR.to_string(),
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
                if file {
                    rclone_op(&["deletefile", &self.full(&target)])
                } else {
                    rclone_op(&["purge", &self.full(&target)])
                }
            }
        }
    }

    /// Podfoldery (jeden poziom) wskazanego folderu: do wyboru podfolderów w kreatorze.
    pub fn list_dirs(&self, rel: &str) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        match self {
            Location::Local(p) => {
                let dir = local_full(p, rel);
                for e in fs::read_dir(&dir)
                    .map_err(|e| format!("{}: {e}", dir.display()))?
                    .flatten()
                {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n != TRASH_DIR && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        out.push(n);
                    }
                }
            }
            Location::Rclone(_) => {
                let raw = run_rclone(&["lsjson", "--dirs-only", "--no-mimetype", &self.full(rel)])?;
                let v: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                for it in v.as_array().into_iter().flatten() {
                    if let Some(n) = it["Name"].as_str().filter(|n| *n != TRASH_DIR) {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort_by_key(|n| n.to_lowercase());
        Ok(out)
    }

    /// Usuwa z kosza lokalnego foldery starsze niż `days` dni (nazwa: `YYYY-MM-DD_HHMMSS`).
    pub fn purge_trash(&self, days: u32, max_versions: u32) {
        if let Location::Rclone(_) = self {
            if days > 0 {
                let _ = run_rclone(&[
                    "delete",
                    &self.full(TRASH_DIR),
                    "--min-age",
                    &format!("{days}d"),
                    "--rmdirs",
                ]);
            }
            return;
        }
        if let Location::Local(p) = self {
            if max_versions > 0 {
                self.cap_versions(max_versions as usize);
            }
            if days == 0 {
                return;
            }
            let dir = local_full(p, TRASH_DIR);
            let limit = chrono::Local::now() - chrono::Duration::days(days as i64);
            let limit = limit.format("%Y-%m-%d").to_string();
            if let Ok(rd) = fs::read_dir(dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.len() >= 10 && n[..10] < *limit.as_str() {
                        let _ = fs::remove_dir_all(e.path());
                    }
                }
            }
        }
    }
}

impl Location {
    /// Zostawia w koszu lokalnym tylko `keep` najnowszych wersji każdego pliku.
    fn cap_versions(&self, keep: usize) {
        let Ok(items) = self.list_trash() else { return };
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (stamp, rel, _) in items {
            by_file.entry(rel).or_default().push(stamp);
        }
        for (rel, mut stamps) in by_file {
            stamps.sort();
            let extra = stamps.len().saturating_sub(keep);
            for st in stamps.into_iter().take(extra) {
                let _ = self.delete_trash(Some(&st), Some(&rel));
            }
        }
        if let Location::Local(p) = self {
            // puste foldery przebiegów
            if let Ok(rd) = fs::read_dir(local_full(p, TRASH_DIR)) {
                for e in rd.flatten() {
                    let _ = fs::remove_dir(e.path());
                }
            }
        }
    }
}

// ---------- lokalne ----------

/// Pełna ścieżka lokalna; na Windows z prefiksem `\\?\` (ścieżki dłuższe niż 260 znaków, UNC).
pub fn local_full(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    fs_path(&p)
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

fn list_local(root: &Path) -> Result<Vec<(String, Meta)>, String> {
    let base = fs_path(root);
    if !base.is_dir() {
        return Err(format!("folder niedostępny: {}", root.display()));
    }
    let mut out = Vec::new();
    let mut stack = vec![(base, String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for ent in rd {
            let ent = ent.map_err(|e| format!("{}: {e}", dir.display()))?;
            let name = ent.file_name().to_string_lossy().into_owned();
            if name == TRASH_DIR || names::is_temp(&name) {
                continue;
            }
            let ft = ent.file_type().map_err(|e| format!("{name}: {e}"))?;
            if ft.is_symlink() {
                continue;
            }
            let r = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_dir() {
                stack.push((ent.path(), r));
            } else if ft.is_file() {
                let md = ent.metadata().map_err(|e| format!("{r}: {e}"))?;
                if let Some(m) = meta_of(&md) {
                    out.push((r, m));
                }
            }
        }
    }
    Ok(out)
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

fn copy_local(src: &Path, dst: &Path, verify: bool) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let fname = dst
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dst.with_file_name(format!("{fname}.syncpolak.tmp"));
    let mut inp = open_shared(src)?;
    let before = inp.metadata()?;
    let res = (|| {
        let mut out = File::create(&tmp)?;
        let mut hasher = blake3::Hasher::new();
        if verify {
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = io::Read::read(&mut inp, &mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                io::Write::write_all(&mut out, &buf[..n])?;
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
        fs::rename(&tmp, dst)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

// ---------- rclone ----------

fn rclone_bin() -> String {
    std::env::var("SYNCPOLAK_RCLONE").unwrap_or_else(|_| "rclone".into())
}

pub fn run_rclone(args: &[&str]) -> Result<String, String> {
    let mut c = Command::new(rclone_bin());
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // bez okna konsoli
    }
    let out = c
        .output()
        .map_err(|e| format!("nie można uruchomić rclone: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn rclone_op(args: &[&str]) -> Result<(), OpError> {
    run_rclone(args).map(|_| ()).map_err(|e| {
        let l = e.to_lowercase();
        if l.contains("used by another process")
            || l.contains("locked")
            || l.contains("sharing violation")
        {
            OpError::Busy(e)
        } else {
            OpError::Failed(e)
        }
    })
}

fn parse_lsjson(s: &str, skip_special: bool) -> Result<Vec<(String, Meta)>, String> {
    let v: serde_json::Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for it in v.as_array().ok_or("zła odpowiedź rclone")? {
        if it["IsDir"].as_bool().unwrap_or(false) {
            continue;
        }
        let size = it["Size"].as_i64().unwrap_or(-1);
        if size < 0 {
            continue; // np. dokumenty Google bez rozmiaru
        }
        let path = it["Path"].as_str().unwrap_or("").to_string();
        let name = path.rsplit('/').next().unwrap_or("");
        if skip_special && (path.starts_with(TRASH_DIR) || names::is_temp(name)) {
            continue;
        }
        let mtime = it["ModTime"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp())
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            });
        out.push((
            path,
            Meta {
                size: size as u64,
                mtime,
            },
        ));
    }
    Ok(out)
}

fn list_rclone(base: &str) -> Result<Vec<(String, Meta)>, String> {
    match run_rclone(&["lsjson", "-R", "--files-only", "--no-mimetype", base]) {
        Ok(s) => parse_lsjson(&s, true),
        Err(e) if e.to_lowercase().contains("directory not found") => Ok(vec![]),
        Err(e) => Err(e),
    }
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
