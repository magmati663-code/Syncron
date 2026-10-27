//! Lokalizacje: folder lokalny/UNC albo zdalny (rclone). Listowanie i operacje na plikach.
use crate::names;
use crate::plan::Meta;
use crate::rules::{FileInfo, RuleSet};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
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
            let drive = name.len() == 1 && (rest.is_empty() || rest.starts_with('\\') || rest.starts_with('/'));
            if name.len() >= 2 && !drive && name.chars().all(|c| c.is_alphanumeric() || "_-. ".contains(c)) {
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
            if !rules.allows(&FileInfo { path: rel.clone(), size: meta.size }) {
                continue;
            }
            let key = names::key(&rel);
            if out.files.contains_key(&key) {
                out.collisions.push(rel);
            } else {
                out.files.insert(key, Entry { real: rel, meta });
            }
        }
        Ok(out)
    }

    pub fn stat(&self, rel: &str) -> Option<Meta> {
        match self {
            Location::Local(p) => meta_of(&fs::metadata(local_full(p, rel)).ok()?),
            Location::Rclone(_) => {
                let out = run_rclone(&["lsjson", "--no-mimetype", &self.full(rel)]).ok()?;
                parse_lsjson(&out).ok()?.into_iter().next().map(|(_, m)| m)
            }
        }
    }

    /// Kopiuje plik `from_rel` z `from` do `self` jako `to_rel` (zapis atomowy, ponowienia dla plików zajętych).
    pub fn copy_from(&self, from: &Location, from_rel: &str, to_rel: &str) -> Result<(), OpError> {
        match (from, self) {
            (Location::Local(a), Location::Local(b)) => {
                let (src, dst) = (local_full(a, from_rel), local_full(b, to_rel));
                let mut last = None;
                for delay in [0u64, 300, 1500, 4000] {
                    if delay > 0 {
                        sleep(Duration::from_millis(delay));
                    }
                    match copy_local(&src, &dst) {
                        Ok(()) => return Ok(()),
                        Err(e) if is_busy(&e) => last = Some(e),
                        Err(e) => return Err(e.into()),
                    }
                }
                Err(OpError::Busy(last.map(|e| e.to_string()).unwrap_or_default()))
            }
            _ => {
                if let Location::Local(b) = self {
                    if let Some(parent) = local_full(b, to_rel).parent() {
                        fs::create_dir_all(parent)?;
                    }
                }
                rclone_op(&["copyto", &from.full(from_rel), &self.full(to_rel)])
            }
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

    /// Usuwa z kosza lokalnego foldery starsze niż `days` dni (nazwa: `YYYY-MM-DD_HHMMSS`).
    pub fn purge_trash(&self, days: u32) {
        if let Location::Local(p) = self {
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
    Some(Meta { size: m.len(), mtime })
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
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
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

fn copy_local(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let fname = dst.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!("{fname}.syncpolak.tmp"));
    let mut inp = open_shared(src)?;
    let before = inp.metadata()?;
    let res = (|| {
        let mut out = File::create(&tmp)?;
        io::copy(&mut inp, &mut out)?;
        out.sync_all()?;
        drop(out);
        let after = fs::metadata(src)?;
        if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "plik zmienił się w trakcie kopiowania"));
        }
        filetime::set_file_mtime(&tmp, filetime::FileTime::from_last_modification_time(&before))?;
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
    let out = c.output().map_err(|e| format!("nie można uruchomić rclone: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn rclone_op(args: &[&str]) -> Result<(), OpError> {
    run_rclone(args).map(|_| ()).map_err(|e| {
        let l = e.to_lowercase();
        if l.contains("used by another process") || l.contains("locked") || l.contains("sharing violation") {
            OpError::Busy(e)
        } else {
            OpError::Failed(e)
        }
    })
}

fn parse_lsjson(s: &str) -> Result<Vec<(String, Meta)>, String> {
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
        if path.starts_with(TRASH_DIR) || names::is_temp(name) {
            continue;
        }
        let mtime = it["ModTime"]
            .as_str()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp())
            .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0));
        out.push((path, Meta { size: size as u64, mtime }));
    }
    Ok(out)
}

fn list_rclone(base: &str) -> Result<Vec<(String, Meta)>, String> {
    match run_rclone(&["lsjson", "-R", "--files-only", "--no-mimetype", base]) {
        Ok(s) => parse_lsjson(&s),
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
        assert!(matches!(Location::parse("\\\\nas\\udział"), Location::Local(_)));
        assert!(matches!(Location::parse("/Volumes/Dysk"), Location::Local(_)));
        assert!(matches!(Location::parse("gdrive:Dokumenty"), Location::Rclone(_)));
        assert!(matches!(Location::parse("bitrix:"), Location::Rclone(_)));
    }

    #[test]
    fn lsjson_parse() {
        let j = r#"[{"Path":"a/zażółć.txt","Size":3,"ModTime":"2026-10-27T10:00:00.123Z","IsDir":false},
                    {"Path":"a","Size":-1,"IsDir":true},{"Path":"~$x.docx","Size":1,"ModTime":"2026-10-27T10:00:00Z","IsDir":false}]"#;
        let v = parse_lsjson(j).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "a/zażółć.txt");
        assert_eq!(v[0].1.size, 3);
    }
}
