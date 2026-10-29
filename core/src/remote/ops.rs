//! Operacje `Location::Remote` na backendach `remote::Backend`: listowanie, kopiowanie (lokalne↔zdalne i
//! zdalne↔zdalne z weryfikacją), kosz synchronizacji (`.syncron-trash/<stamp>/...`), przenoszenie, odczyt tekstu.
//! Wszystkie ścieżki z serwera są traktowane jako dane z zewnątrz (`names::check_rel`).
use super::{Backend, Hash, RError, RResult, Xfer};
use crate::accounts;
use crate::location::{
    copy_local, create_new_tmp, ensure_dir, is_busy, meta_of, open_shared, OpError, Opts,
    LEGACY_TMP_SUFFIX, LEGACY_TRASH_DIR, TRASH_DIR,
};
use crate::model::Bandwidth;
use crate::names;
use crate::plan::Meta;
use sha2::Digest;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

type Hashes = std::collections::BTreeMap<String, Vec<(String, String)>>;
type Scan = (Vec<(String, Meta)>, Vec<String>, Hashes);

/// Błąd backendu -> błąd operacji: `Busy` zostaje `Busy` (ponowienie później), reszta to opis po polsku.
pub fn op_err(account: &str, e: RError) -> OpError {
    match e {
        RError::Busy(m) => OpError::Busy(m),
        other => OpError::Failed(accounts::map_account_err(account, &other)),
    }
}

fn be(account: &str, root: &str) -> Result<Arc<dyn Backend>, OpError> {
    accounts::open_backend(account, root).map_err(OpError::Failed)
}

fn rel_ok(rel: &str) -> Result<(), OpError> {
    names::check_rel(rel).map_err(|e| OpError::Failed(format!("niebezpieczna ścieżka ({e})")))
}

/// Ponawia operację przy `Busy` (chwilowa niedostępność): 0, 0,3 s, 1,5 s.
fn retry<T>(mut f: impl FnMut() -> RResult<T>) -> RResult<T> {
    let mut last = None;
    for delay in [0u64, 300, 1500] {
        if delay > 0 {
            sleep(Duration::from_millis(delay));
        }
        match f() {
            Err(RError::Busy(m)) => last = Some(RError::Busy(m)),
            other => return other,
        }
    }
    Err(last.unwrap_or_else(|| RError::Busy("zajęte".into())))
}

fn is_special(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or("");
    path.starts_with(&format!("{TRASH_DIR}/"))
        || path.starts_with(&format!("{LEGACY_TRASH_DIR}/"))
        || names::is_temp(name)
        || name.ends_with(".syncron.tmp")
        || name.ends_with(LEGACY_TMP_SUFFIX)
}

/// Lista plików konta pod korzeniem. Błąd listowania = `Err` (silnik przerywa przebieg bez usuwania).
pub fn list(account: &str, root: &str, want_hashes: bool) -> Result<Scan, String> {
    let b = accounts::open_backend(account, root)?;
    let entries = b
        .list_files("")
        .map_err(|e| accounts::map_account_err(account, &e))?;
    let mut out = Vec::with_capacity(entries.len());
    let mut skipped = Vec::new();
    let mut hashes = Hashes::new();
    for e in entries {
        // ścieżki z serwera to dane z zewnątrz: `..`, bezwzględne (wiodący `/`), puste komponenty i znaki sterujące odrzucamy
        let p = e.path;
        if let Err(why) = names::check_rel(&p) {
            skipped.push(format!(
                "pominięto niebezpieczną ścieżkę ze zdalnego listingu: {why}"
            ));
            continue;
        }
        if is_special(&p) {
            continue;
        }
        if want_hashes {
            if let Some(h) = &e.meta.hash {
                hashes.insert(
                    p.clone(),
                    vec![(h.algo.to_lowercase(), h.hex.to_lowercase())],
                );
            }
        }
        out.push((
            p,
            Meta {
                size: e.meta.size,
                mtime: e.meta.mtime,
            },
        ));
    }
    Ok((out, skipped, hashes))
}

/// Metadane pliku: `Ok(None)` = nie istnieje, `Err` = nie wiadomo (błąd sieci/logowania).
pub fn stat(account: &str, root: &str, rel: &str) -> Result<Option<Meta>, String> {
    let b = accounts::open_backend(account, root)?;
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || names::check_rel(rel).is_err() {
        return Ok(None);
    }
    b.stat(rel)
        .map(|o| {
            o.map(|e| Meta {
                size: e.meta.size,
                mtime: e.meta.mtime,
            })
        })
        .map_err(|e| accounts::map_account_err(account, &e))
}

/// Odcisk treści pliku zdalnego (z hasha podanego przez serwer): `None`, gdy serwer go nie podaje.
pub fn content_hash(account: &str, root: &str, rel: &str) -> Option<[u8; 32]> {
    let b = accounts::open_backend(account, root).ok()?;
    names::check_rel(rel).ok()?;
    let e = b.stat(rel).ok()??;
    let h = e.meta.hash?;
    let mut x = blake3::Hasher::new();
    x.update(format!("remote:{}:{}:{}", h.algo, h.hex.to_lowercase(), e.meta.size).as_bytes());
    Some(*x.finalize().as_bytes())
}

// ---------- skróty ----------

fn digest_file<D: Digest>(p: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut d = D::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        d.update(&buf[..n]);
    }
    Ok(d.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Skrót pliku lokalnego w algorytmie serwera; `None` dla algorytmów, których nie liczymy lokalnie.
pub(crate) fn local_digest(p: &Path, algo: &str) -> std::io::Result<Option<String>> {
    Ok(match algo.to_ascii_lowercase().as_str() {
        "md5" => Some(digest_file::<md5::Md5>(p)?),
        "sha1" | "sha-1" => Some(digest_file::<sha1::Sha1>(p)?),
        "sha256" | "sha-256" => Some(digest_file::<sha2::Sha256>(p)?),
        _ => None,
    })
}

/// Porównuje skrót serwera ze skrótem pliku lokalnego (gdy algorytm jest obsługiwany).
fn verify_hash(remote: &Option<Hash>, local: &Path) -> Result<(), OpError> {
    let Some(h) = remote else { return Ok(()) };
    match local_digest(local, h.algo)? {
        Some(l) if !l.eq_ignore_ascii_case(&h.hex) => Err(OpError::Failed(format!(
            "suma kontrolna ({}) pliku na serwerze nie zgadza się z lokalną",
            h.algo
        ))),
        _ => Ok(()),
    }
}

// ---------- kopiowanie ----------

/// Lokalny -> zdalny. Źródło zablokowane przez inny program jest czytane z migawki VSS (Windows).
pub fn upload_local(
    account: &str,
    root: &str,
    src: &Path,
    to_rel: &str,
    o: &Opts,
) -> Result<(), OpError> {
    rel_ok(to_rel)?;
    let b = be(account, root)?;
    let mut source = src.to_path_buf();
    let mut temp: Option<PathBuf> = None;
    match open_shared(src) {
        Ok(f) => drop(f),
        Err(e) if is_busy(&e) => {
            let snap = o
                .vss
                .snapshot_path(src)
                .map_err(|n| OpError::Busy(format!("{e}; {n}")))?;
            let t = crate::vss::scratch_dir()?.join("kopia");
            copy_local(&snap, &t, false, &Bandwidth::default(), None)?;
            source = t.clone();
            temp = Some(t);
        }
        Err(e) => return Err(e.into()),
    }
    let res = upload_checked(account, b.as_ref(), &source, src, to_rel, o);
    if let Some(t) = temp {
        let _ = fs::remove_file(&t);
        if let Some(d) = t.parent() {
            let _ = fs::remove_dir(d);
        }
    }
    res
}

fn upload_checked(
    account: &str,
    b: &dyn Backend,
    source: &Path,
    original: &Path,
    to_rel: &str,
    o: &Opts,
) -> Result<(), OpError> {
    let before = fs::metadata(source)?;
    let mtime = meta_of(&before).map(|m| m.mtime).unwrap_or(0);
    let x = Xfer::new(&o.bw);
    retry(|| b.upload(source, to_rel, mtime, &x)).map_err(|e| op_err(account, e))?;
    let bad = |why: String| {
        // wysłana kopia jest niepoprawna: usuwamy ją, by następny przebieg nie wziął jej za zmianę użytkownika
        let _ = b.delete(to_rel);
        Err(OpError::Failed(why))
    };
    match b.stat(to_rel).map_err(|e| op_err(account, e))? {
        None => {
            return Err(OpError::Failed(
                "po wysłaniu plik nie istnieje na serwerze".into(),
            ))
        }
        Some(e) if e.meta.size != before.len() => {
            return bad(format!(
                "rozmiar po wysłaniu nie zgadza się (wysłano {} B, na serwerze {} B)",
                before.len(),
                e.meta.size
            ))
        }
        Some(e) if o.verify => {
            if let Err(OpError::Failed(m)) = verify_hash(&e.meta.hash, source) {
                return bad(m);
            }
        }
        Some(_) => {}
    }
    if source == original {
        let after = fs::metadata(original)?;
        if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            let _ = b.delete(to_rel);
            return Err(OpError::Busy(
                "plik zmienił się w trakcie kopiowania".into(),
            ));
        }
    }
    Ok(())
}

/// Zdalny -> lokalny: pobranie do pliku tymczasowego obok celu, kontrola rozmiaru (i skrótu), zmiana nazwy.
pub fn download_to(
    account: &str,
    root: &str,
    from_rel: &str,
    dst: &Path,
    o: &Opts,
) -> Result<(), OpError> {
    rel_ok(from_rel)?;
    let b = be(account, root)?;
    let meta = b
        .stat(from_rel)
        .map_err(|e| op_err(account, e))?
        .ok_or_else(|| OpError::Failed(format!("nie znaleziono pliku na serwerze: {from_rel}")))?
        .meta;
    if let Some(parent) = dst.parent() {
        ensure_dir(parent)?;
    }
    let (f, tmp) = create_new_tmp(dst)?;
    drop(f);
    let res = (|| {
        let x = Xfer::new(&o.bw);
        retry(|| b.download(from_rel, &tmp, &x)).map_err(|e| op_err(account, e))?;
        let got = fs::metadata(&tmp)?.len();
        if got != meta.size {
            return Err(OpError::Failed(format!(
                "pobrano {got} B zamiast {} B (przerwane połączenie?)",
                meta.size
            )));
        }
        if o.verify {
            verify_hash(&meta.hash, &tmp)?;
        }
        if meta.mtime > 0 {
            filetime::set_file_mtime(&tmp, filetime::FileTime::from_unix_time(meta.mtime, 0))?;
        }
        if fs::symlink_metadata(dst)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(dst);
        }
        fs::rename(&tmp, dst)?;
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Zdalny -> zdalny: przez plik tymczasowy w prywatnym katalogu roboczym.
pub fn remote_to_remote(
    (sa, sr): (&str, &str),
    from_rel: &str,
    (da, dr): (&str, &str),
    to_rel: &str,
    o: &Opts,
) -> Result<(), OpError> {
    rel_ok(from_rel)?;
    rel_ok(to_rel)?;
    let (sb, db) = (be(sa, sr)?, be(da, dr)?);
    let sm = sb
        .stat(from_rel)
        .map_err(|e| op_err(sa, e))?
        .ok_or_else(|| OpError::Failed(format!("nie znaleziono pliku na serwerze: {from_rel}")))?
        .meta;
    let dir = crate::vss::scratch_dir()?;
    let tmp = dir.join("kopia");
    let res = (|| {
        let x = Xfer::new(&o.bw);
        retry(|| sb.download(from_rel, &tmp, &x)).map_err(|e| op_err(sa, e))?;
        let got = fs::metadata(&tmp)?.len();
        if got != sm.size {
            return Err(OpError::Failed(format!(
                "pobrano {got} B zamiast {} B (przerwane połączenie?)",
                sm.size
            )));
        }
        if o.verify {
            verify_hash(&sm.hash, &tmp)?;
        }
        retry(|| db.upload(&tmp, to_rel, sm.mtime, &x)).map_err(|e| op_err(da, e))?;
        match db.stat(to_rel).map_err(|e| op_err(da, e))? {
            Some(e) if e.meta.size == sm.size => {
                if o.verify {
                    if let Err(OpError::Failed(m)) = verify_hash(&e.meta.hash, &tmp) {
                        let _ = db.delete(to_rel);
                        return Err(OpError::Failed(m));
                    }
                }
                Ok(())
            }
            Some(e) => {
                let _ = db.delete(to_rel);
                Err(OpError::Failed(format!(
                    "rozmiar po wysłaniu nie zgadza się (wysłano {} B, na serwerze {} B)",
                    sm.size, e.meta.size
                )))
            }
            None => Err(OpError::Failed(
                "po wysłaniu plik nie istnieje na serwerze".into(),
            )),
        }
    })();
    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir(&dir);
    res
}

// ---------- przenoszenie, usuwanie, kosz ----------

/// Usuwa puste foldery nadrzędne `rel` (od najgłębszego): tylko po sprawdzeniu, że folder jest pusty.
fn prune_parents(b: &dyn Backend, rel: &str) {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    while !parts.is_empty() {
        let dir = parts.join("/");
        match b.list_dir(&dir) {
            Ok(v) if v.is_empty() => {
                if b.rmdir_if_empty(&dir).is_err() {
                    break;
                }
            }
            _ => break,
        }
        parts.pop();
    }
}

pub fn rename(account: &str, root: &str, from: &str, to: &str) -> Result<(), OpError> {
    rel_ok(from)?;
    rel_ok(to)?;
    let b = be(account, root)?;
    retry(|| b.rename(from, to)).map_err(|e| op_err(account, e))?;
    prune_parents(b.as_ref(), from);
    Ok(())
}

pub fn read_text(account: &str, root: &str, rel: &str) -> Option<String> {
    names::check_rel(rel).ok()?;
    let b = accounts::open_backend(account, root).ok()?;
    let meta = match b.stat(rel) {
        Ok(Some(e)) => e.meta,
        Ok(None) => return None,
        Err(e) => {
            eprintln!("Ostrzeżenie: nie można odczytać pliku {rel}: {e}");
            return None;
        }
    };
    if meta.size > 4 * 1024 * 1024 {
        eprintln!(
            "Ostrzeżenie: plik {rel} jest za duży ({} B), pominięto",
            meta.size
        );
        return None;
    }
    let dir = crate::vss::scratch_dir().ok()?;
    let tmp = dir.join("tekst");
    let bw = Bandwidth::default();
    let r = retry(|| b.download(rel, &tmp, &Xfer::new(&bw)));
    let out = match r {
        Ok(()) => fs::read(&tmp).ok().map(|d| crate::rules::decode_text(&d)),
        Err(e) => {
            eprintln!("Ostrzeżenie: nie można odczytać pliku {rel}: {e}");
            None
        }
    };
    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir(&dir);
    out
}

/// Pierwsza wolna ścieżka w koszu: `.syncron-trash/<stamp>[-N]/<rel>`. Błąd sprawdzania = `Err`
/// (nigdy nie zakładamy, że miejsce jest wolne, gdy nie wiemy).
pub fn free_trash_path(
    b: &dyn Backend,
    account: &str,
    stamp: &str,
    rel: &str,
    base: &str,
) -> Result<String, OpError> {
    let mut cand = stamp.to_string();
    for n in 1..10_000u32 {
        let path = format!("{TRASH_DIR}/{cand}/{rel}");
        if b.stat(&path).map_err(|e| op_err(account, e))?.is_none() {
            return Ok(path);
        }
        cand = format!("{base}-{n}");
    }
    Ok(format!("{TRASH_DIR}/{cand}/{rel}"))
}

pub fn backup(
    account: &str,
    root: &str,
    rel: &str,
    stamp: &str,
    base: &str,
) -> Result<String, OpError> {
    rel_ok(rel)?;
    let b = be(account, root)?;
    let to = free_trash_path(b.as_ref(), account, stamp, rel, base)?;
    let bw = Bandwidth::default();
    retry(|| b.copy(rel, &to, &Xfer::new(&bw))).map_err(|e| op_err(account, e))?;
    Ok(to)
}

/// Usuwa plik (`stamp` pusty) albo przenosi go do kosza: przeniesienie po stronie serwera, a gdy się nie da —
/// kopia do kosza i dopiero potem usunięcie.
pub fn remove(
    account: &str,
    root: &str,
    rel: &str,
    stamp: &str,
    base: &str,
) -> Result<String, OpError> {
    rel_ok(rel)?;
    let b = be(account, root)?;
    if stamp.is_empty() {
        retry(|| b.delete(rel)).map_err(|e| op_err(account, e))?;
        prune_parents(b.as_ref(), rel);
        return Ok(String::new());
    }
    let to = free_trash_path(b.as_ref(), account, stamp, rel, base)?;
    match retry(|| b.rename(rel, &to)) {
        Ok(()) => {}
        Err(RError::Busy(m)) => return Err(OpError::Busy(m)),
        Err(_) => {
            // brak przenoszenia po stronie serwera: kopia do kosza, potem usunięcie oryginału
            let bw = Bandwidth::default();
            retry(|| b.copy(rel, &to, &Xfer::new(&bw))).map_err(|e| op_err(account, e))?;
            retry(|| b.delete(rel)).map_err(|e| op_err(account, e))?;
        }
    }
    prune_parents(b.as_ref(), rel);
    Ok(to)
}

/// Elementy kosza: (znacznik, ścieżka, rozmiar).
pub fn list_trash(account: &str, root: &str) -> Result<Vec<(String, String, u64)>, String> {
    let b = accounts::open_backend(account, root)?;
    let entries = b
        .list_files(TRASH_DIR)
        .map_err(|e| accounts::map_account_err(account, &e))?;
    let prefix = format!("{TRASH_DIR}/");
    let mut out = Vec::new();
    for e in entries {
        let Some(rest) = e.path.strip_prefix(&prefix) else {
            continue;
        };
        if let Some((stamp, rel)) = rest.split_once('/') {
            if names::check_rel(stamp).is_ok() && names::check_rel(rel).is_ok() {
                out.push((stamp.to_string(), rel.to_string(), e.meta.size));
            }
        }
    }
    Ok(out)
}

/// Przywraca plik z kosza; zwraca ścieżkę docelową.
pub fn restore_trash(account: &str, root: &str, stamp: &str, rel: &str) -> Result<String, OpError> {
    let b = be(account, root)?;
    let from = format!("{TRASH_DIR}/{stamp}/{rel}");
    if b.stat(&from).map_err(|e| op_err(account, e))?.is_none() {
        return Err(OpError::Failed(format!(
            "Nie znaleziono wersji w koszu: {stamp}/{rel}"
        )));
    }
    let target = if b.stat(rel).map_err(|e| op_err(account, e))?.is_some() {
        names::conflict_name(rel, &format!("przywrócono {stamp}"))
    } else {
        rel.to_string()
    };
    let bw = Bandwidth::default();
    retry(|| b.copy(&from, &target, &Xfer::new(&bw))).map_err(|e| op_err(account, e))?;
    Ok(target)
}

/// Usuwa trwale: jeden plik (`file`) albo całe drzewo pod `target` (folder przebiegu lub cały kosz).
pub fn delete_trash(account: &str, root: &str, target: &str, file: bool) -> Result<(), OpError> {
    let b = be(account, root)?;
    if file {
        retry(|| b.delete(target)).map_err(|e| op_err(account, e))?;
        prune_parents(b.as_ref(), target);
        return Ok(());
    }
    let entries = b.list_files(target).map_err(|e| op_err(account, e))?;
    let prefix = format!("{target}/");
    let mut dirs = std::collections::BTreeSet::new();
    let mut first_err = None;
    for e in entries.iter().filter(|e| e.path.starts_with(&prefix)) {
        match retry(|| b.delete(&e.path)) {
            Ok(()) => {
                let mut p = e.path.as_str();
                while let Some((d, _)) = p.rsplit_once('/') {
                    if d.len() < target.len() {
                        break;
                    }
                    dirs.insert(d.to_string());
                    p = d;
                }
            }
            Err(err) => {
                first_err.get_or_insert(op_err(account, err));
            }
        }
    }
    dirs.insert(target.to_string());
    // foldery od najgłębszego, tylko puste
    let mut dirs: Vec<String> = dirs.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));
    for d in dirs {
        if matches!(b.list_dir(&d), Ok(v) if v.is_empty()) {
            let _ = b.rmdir_if_empty(&d);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Podfoldery (jeden poziom).
pub fn list_dirs(account: &str, root: &str, rel: &str) -> Result<Vec<String>, String> {
    if !rel.is_empty() {
        names::check_rel(rel)?;
    }
    let b = accounts::open_backend(account, root)?;
    let items = b
        .list_dir(rel)
        .map_err(|e| accounts::map_account_err(account, &e))?;
    Ok(items
        .into_iter()
        .filter(|d| d.is_dir)
        .filter(|d| !(rel.is_empty() && (d.name == TRASH_DIR || d.name == LEGACY_TRASH_DIR)))
        .filter(|d| names::check_rel(&d.name).is_ok())
        .map(|d| d.name)
        .collect())
}

/// Nazwy folderów przebiegów w koszu (jeden poziom).
pub fn trash_stamps(account: &str, root: &str) -> Vec<String> {
    let Ok(b) = accounts::open_backend(account, root) else {
        return Vec::new();
    };
    b.list_dir(TRASH_DIR)
        .map(|v| v.into_iter().filter(|d| d.is_dir).map(|d| d.name).collect())
        .unwrap_or_default()
}
