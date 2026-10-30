//! Konta zdalnych magazynów: jawne parametry w `accounts.json`, sekrety (hasła, tokeny) w magazynie sekretów.
//!
//! Zasady:
//! - w `accounts.json` nigdy nie ma haseł ani tokenów (tylko ich NAZWY w `secret_keys`);
//! - sekrety trafiają do kluczy systemu (Keyring: Windows Credential Manager, macOS Keychain, Linux Secret Service)
//!   albo, gdy to niemożliwe (np. Linux bez D-Bus), do pliku `secrets.enc` zaszyfrowanego XChaCha20-Poly1305
//!   losowym kluczem z pliku `secrets.key` (uprawnienia 0600);
//! - sekrety nigdy nie trafiają do dziennika ani do komunikatów błędów;
//! - zapis plików jest atomowy (plik tymczasowy + zmiana nazwy) i odbywa się pod blokadą (jak `jobs.json`).
use crate::remote::{self, Backend, DirItem, Params};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{AeadCore, Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};
use unicode_normalization::UnicodeNormalization;

pub const ACCOUNTS_FILE: &str = "accounts.json";
pub const SECRETS_FILE: &str = "secrets.enc";
pub const SECRETS_KEY_FILE: &str = "secrets.key";
const MAGIC: &[u8; 8] = b"SYNSEC1\0";

// ---------- model ----------

/// Konto zdalnego magazynu. `params` to wyłącznie wartości jawne (host, port, użytkownik...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    /// Nazwy (nie wartości!) sekretów zapisanych w magazynie sekretów.
    #[serde(default)]
    pub secret_keys: Vec<String>,
    /// Gdzie leżą sekrety: `keyring` albo `file` (pusty = `file`).
    #[serde(default)]
    pub secret_store: String,
    /// Konto wymaga ponownego zalogowania (np. zaimportowane z rclone: tokeny OAuth nie są przenoszone).
    #[serde(default)]
    pub needs_login: bool,
}

/// Konto z wyliczonym stanem (dla interfejsu).
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub name: String,
    pub kind: String,
    pub params: BTreeMap<String, String>,
    pub secret_keys: Vec<String>,
    /// Wymaga zalogowania: oznaczone przy imporcie albo brakuje zapisanego sekretu.
    pub needs_login: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestInfo {
    pub ms: u64,
    pub info: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirEntryInfo {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AboutInfo {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkipInfo {
    pub name: String,
    pub reason: String,
}

/// Wynik importu `rclone.conf`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    /// Utworzone konta natywne (z kompletnymi danymi).
    pub imported: Vec<String>,
    /// Utworzone konta, które trzeba zalogować ponownie (Google Drive, OneDrive, Dropbox).
    pub needs_login: Vec<String>,
    /// Pominięte sekcje z powodem.
    pub skipped: Vec<SkipInfo>,
    pub warnings: Vec<String>,
    /// Plik źródłowy został usunięty po udanym imporcie.
    pub source_removed: bool,
}

/// Nazwa konta (po NFC): 2–64 znaki, litery/cyfry (Unicode), `_`, `-`, `.`, spacja; zaczyna się od litery/cyfry,
/// nie kończy spacją. Zgodna z regułą rozpoznawania `konto:ścieżka` w `Location::parse`.
pub fn valid_name(n: &str) -> bool {
    let n: String = n.nfc().collect();
    let c = n.chars().count();
    (2..=64).contains(&c)
        && n.chars().next().is_some_and(|ch| ch.is_alphanumeric())
        && !n.ends_with(' ')
        && n.chars()
            .all(|ch| ch.is_alphanumeric() || "_-. ".contains(ch))
}

/// Klucz parametru konta: tylko litery ASCII, cyfry i `_`.
pub fn valid_param_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Nazwa kanoniczna do porównań (NFC, małe litery): „Konto” i „konto” to to samo konto.
pub fn canon_name(n: &str) -> String {
    n.trim().nfc().collect::<String>().to_lowercase()
}

fn valid_kind(k: &str) -> bool {
    !k.is_empty() && k.len() <= 32 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Czy parametr jest sekretem: według specyfikacji formularza rodzaju, a gdy jej brak — po nazwie.
pub fn is_secret_param(kind: &str, key: &str) -> bool {
    if let Some(ks) = remote::spec::remote_kinds()
        .into_iter()
        .find(|k| k.kind == kind)
    {
        if let Some(f) = ks.fields.iter().find(|f| f.key == key) {
            return f.secret;
        }
    }
    let k = key.to_lowercase();
    if k.ends_with("_file") || k.ends_with("_path") || k.ends_with("_id") {
        return false;
    }
    ["pass", "secret", "token", "private_key", "credential"]
        .iter()
        .any(|w| k.contains(w))
}

// ---------- pliki: blokada i zapis atomowy ----------

struct FileLock {
    path: PathBuf,
    _g: MutexGuard<'static, ()>,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn acquire(dir: &Path, file: &str, proc: &'static Mutex<()>) -> Result<FileLock, String> {
    let guard = proc.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Nie można utworzyć folderu danych {}: {e}", dir.display()))?;
    let lock = dir.join(file);
    let start = Instant::now();
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => {
                return Ok(FileLock {
                    path: lock,
                    _g: guard,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|d| d > Duration::from_secs(30));
                if stale {
                    let _ = std::fs::remove_file(&lock);
                } else if start.elapsed() > Duration::from_secs(15) {
                    return Err(format!(
                        "Nie można uzyskać blokady {} (inny proces zapisuje konta).",
                        lock.display()
                    ));
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Err(e) => {
                return Err(format!(
                    "Nie można utworzyć blokady {}: {e}",
                    lock.display()
                ))
            }
        }
    }
}

/// Zapis atomowy: plik tymczasowy obok + zmiana nazwy. `private` = uprawnienia 0600 (Unix).
fn write_atomic(path: &Path, data: &[u8], private: bool) -> Result<(), String> {
    use std::io::Write;
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let tmp = path.with_file_name(format!(
        "{}.tmp-{}-{}",
        path.file_name().and_then(|f| f.to_str()).unwrap_or("plik"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let res = (|| -> std::io::Result<()> {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = o.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Nie można zapisać pliku {}: {e}", path.display()));
    }
    Ok(())
}

/// Zachowuje kopię uszkodzonego pliku (`<plik>.corrupt-<data>`), by niczego nie niszczyć bez śladu.
/// Adres z `://użytkownik[:hasło]@host`: dane logowania nie mogą trafić jawnie do `accounts.json`.
fn has_url_userinfo(v: &str) -> bool {
    v.split_once("://").is_some_and(|(_, rest)| {
        rest.split(['/', '?', '#'])
            .next()
            .is_some_and(|a| a.contains('@'))
    })
}

fn backup_corrupt(path: &Path) -> Option<PathBuf> {
    let content = std::fs::read(path).ok()?;
    if content.iter().all(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    // ta sama treść była już zachowana: nie mnożymy kopii przy każdym odczycie
    let prefix = format!("{name}.corrupt-");
    if let Ok(rd) = std::fs::read_dir(path.parent()?) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix)
                && std::fs::read(e.path()).is_ok_and(|c| c == content)
            {
                return Some(e.path());
            }
        }
    }
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for n in 0..1000 {
        let p = path.with_file_name(if n == 0 {
            format!("{name}.corrupt-{ts}")
        } else {
            format!("{name}.corrupt-{ts}-{n}")
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            use std::io::Write;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
            }
            return f.write_all(&content).ok().map(|_| p);
        }
    }
    None
}

// ---------- magazyn sekretów ----------

/// Magazyn sekretów: wartości identyfikowane parą (konto, nazwa).
pub trait SecretStore: Send + Sync {
    fn label(&self) -> &'static str;
    fn get(&self, account: &str, key: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str, key: &str) -> Result<(), String>;
    fn delete_account(&self, account: &str, keys: &[String]) -> Result<(), String> {
        let mut first = None;
        for k in keys {
            if let Err(e) = self.delete(account, k) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}

/// Wykonuje `f` w osobnym wątku z limitem czasu (magazyn kluczy systemu może czekać na odblokowanie).
fn with_timeout<T: Send + 'static>(
    secs: u64,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .map_err(|_| "magazyn kluczy systemu nie odpowiedział w wyznaczonym czasie".to_string())?
}

/// Magazyn kluczy systemu (Windows Credential Manager, macOS Keychain, Linux Secret Service przez D-Bus).
/// Długie wartości (np. klucze prywatne) są dzielone na kilka wpisów.
pub struct Keyring;

impl Keyring {
    const SERVICE: &'static str = "Syncron";
    const CHUNK: usize = 700;

    fn user(account: &str, key: &str, part: usize) -> String {
        if part == 0 {
            format!("{account}|{key}")
        } else {
            format!("{account}|{key}#{part}")
        }
    }

    fn get_part(account: &str, key: &str, part: usize) -> Result<Option<String>, String> {
        let (a, k) = (account.to_string(), key.to_string());
        with_timeout(20, move || {
            let e = keyring::Entry::new(Self::SERVICE, &Self::user(&a, &k, part))
                .map_err(|e| e.to_string())?;
            match e.get_password() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    fn set_part(account: &str, key: &str, part: usize, value: &str) -> Result<(), String> {
        let (a, k, v) = (account.to_string(), key.to_string(), value.to_string());
        with_timeout(20, move || {
            keyring::Entry::new(Self::SERVICE, &Self::user(&a, &k, part))
                .and_then(|e| e.set_password(&v))
                .map_err(|e| e.to_string())
        })
    }

    fn del_part(account: &str, key: &str, part: usize) -> Result<(), String> {
        let (a, k) = (account.to_string(), key.to_string());
        with_timeout(20, move || {
            let e = keyring::Entry::new(Self::SERVICE, &Self::user(&a, &k, part))
                .map_err(|e| e.to_string())?;
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    /// Czy magazyn kluczy systemu działa (zapis, odczyt, usunięcie wpisu próbnego).
    pub fn probe() -> bool {
        let r = (|| {
            Self::set_part("__probe__", "probe", 0, "ok")?;
            let v = Self::get_part("__probe__", "probe", 0)?;
            let _ = Self::del_part("__probe__", "probe", 0);
            if v.as_deref() == Some("ok") {
                Ok(())
            } else {
                Err("odczyt próbnego wpisu nie zgadza się".to_string())
            }
        })();
        r.is_ok()
    }
}

impl SecretStore for Keyring {
    fn label(&self) -> &'static str {
        "keyring"
    }
    fn get(&self, account: &str, key: &str) -> Result<Option<String>, String> {
        let Some(first) = Self::get_part(account, key, 0)? else {
            return Ok(None);
        };
        let Some(rest) = first.strip_prefix("S1:") else {
            return Ok(Some(first));
        };
        let (n, chunk0) = rest
            .split_once(':')
            .ok_or("uszkodzony wpis w magazynie kluczy")?;
        let n: usize = n
            .parse()
            .map_err(|_| "uszkodzony wpis w magazynie kluczy")?;
        let mut out = chunk0.to_string();
        for i in 1..n {
            out +=
                &Self::get_part(account, key, i)?.ok_or("niekompletny wpis w magazynie kluczy")?;
        }
        Ok(Some(out))
    }
    fn set(&self, account: &str, key: &str, value: &str) -> Result<(), String> {
        let chars: Vec<char> = value.chars().collect();
        let chunks: Vec<String> = chars
            .chunks(Self::CHUNK)
            .map(|c| c.iter().collect())
            .collect();
        // poprzednia wartość mogła mieć więcej kawałków: sprzątamy nadmiarowe
        let old_parts = match Self::get_part(account, key, 0)? {
            Some(f) => f
                .strip_prefix("S1:")
                .and_then(|r| r.split_once(':'))
                .and_then(|(n, _)| n.parse::<usize>().ok())
                .unwrap_or(1),
            None => 0,
        };
        if chunks.len() <= 1 {
            Self::set_part(account, key, 0, value)?;
        } else {
            for (i, c) in chunks.iter().enumerate().skip(1) {
                Self::set_part(account, key, i, c)?;
            }
            Self::set_part(
                account,
                key,
                0,
                &format!("S1:{}:{}", chunks.len(), chunks[0]),
            )?;
        }
        for i in chunks.len().max(1)..old_parts {
            let _ = Self::del_part(account, key, i);
        }
        Ok(())
    }
    fn delete(&self, account: &str, key: &str) -> Result<(), String> {
        let parts = match Self::get_part(account, key, 0)? {
            Some(f) => f
                .strip_prefix("S1:")
                .and_then(|r| r.split_once(':'))
                .and_then(|(n, _)| n.parse::<usize>().ok())
                .unwrap_or(1),
            None => return Ok(()),
        };
        for i in (0..parts).rev() {
            Self::del_part(account, key, i)?;
        }
        Ok(())
    }
}

type SecretMap = BTreeMap<String, BTreeMap<String, String>>;

/// Plik `secrets.enc`: XChaCha20-Poly1305, klucz losowy w `secrets.key` (0600) obok.
/// Format: `SYNSEC1\0` + nonce (24 B) + szyfrogram JSON `{"v":1,"secrets":{konto:{klucz:wartość}}}`.
pub struct EncryptedFile {
    dir: PathBuf,
}

static SECRETS_PROC: Mutex<()> = Mutex::new(());
static ACCOUNTS_PROC: Mutex<()> = Mutex::new(());

enum ReadErr {
    /// Plik uszkodzony / klucz nie pasuje: wymaga ponownego wpisania sekretów.
    Corrupt(String),
    Io(String),
}

impl EncryptedFile {
    pub fn new(dir: &Path) -> EncryptedFile {
        EncryptedFile {
            dir: dir.to_path_buf(),
        }
    }
    fn path(&self) -> PathBuf {
        self.dir.join(SECRETS_FILE)
    }
    fn key_path(&self) -> PathBuf {
        self.dir.join(SECRETS_KEY_FILE)
    }
    fn lock(&self) -> Result<FileLock, String> {
        acquire(&self.dir, "secrets.lock", &SECRETS_PROC)
    }
    fn read_key(&self) -> Option<[u8; 32]> {
        let b = std::fs::read(self.key_path()).ok()?;
        // edytory potrafią dopisać znak nowej linii: nadmiarowe białe znaki ignorujemy
        let end = b
            .iter()
            .rposition(|c| !c.is_ascii_whitespace())
            .map_or(0, |i| i + 1);
        let b = if b.len() > 32 && end <= 32 {
            &b[..32]
        } else {
            &b[..]
        };
        <[u8; 32]>::try_from(b).ok()
    }
    fn make_key(&self) -> Result<[u8; 32], String> {
        let k: [u8; 32] = XChaCha20Poly1305::generate_key(&mut OsRng).into();
        write_atomic(&self.key_path(), &k, true)?;
        Ok(k)
    }
    fn read_all(&self) -> Result<SecretMap, ReadErr> {
        let raw = match std::fs::read(self.path()) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SecretMap::new()),
            Err(e) => return Err(ReadErr::Io(format!("secrets.enc: {e}"))),
        };
        let key = self.read_key().ok_or_else(|| {
            ReadErr::Corrupt("brak klucza szyfrowania sekretów (secrets.key)".into())
        })?;
        if raw.len() < MAGIC.len() + 24 + 16 || &raw[..MAGIC.len()] != MAGIC {
            return Err(ReadErr::Corrupt("plik secrets.enc jest uszkodzony".into()));
        }
        let nonce = XNonce::from_slice(&raw[MAGIC.len()..MAGIC.len() + 24]);
        let plain = XChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt(nonce, &raw[MAGIC.len() + 24..])
            .map_err(|_| {
                ReadErr::Corrupt(
                    "nie można odszyfrować secrets.enc (zły klucz lub uszkodzony plik)".into(),
                )
            })?;
        #[derive(Deserialize)]
        struct Doc {
            #[serde(default)]
            secrets: SecretMap,
        }
        serde_json::from_slice::<Doc>(&plain)
            .map(|d| d.secrets)
            .map_err(|_| ReadErr::Corrupt("zawartość secrets.enc jest nieczytelna".into()))
    }
    fn write_all(&self, map: &SecretMap) -> Result<(), String> {
        let key = match self.read_key() {
            Some(k) => k,
            None => self.make_key()?,
        };
        #[derive(Serialize)]
        struct Doc<'a> {
            v: u32,
            secrets: &'a SecretMap,
        }
        let plain = serde_json::to_vec(&Doc { v: 1, secrets: map }).map_err(|e| e.to_string())?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = XChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(&nonce, plain.as_slice())
            .map_err(|_| "szyfrowanie sekretów nie powiodło się".to_string())?;
        let mut out = Vec::with_capacity(MAGIC.len() + 24 + ct.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        write_atomic(&self.path(), &out, true)
    }
    /// Czy plik sekretów da się odczytać (diagnostyka). `Err` = uszkodzony.
    pub fn health(&self) -> Result<(), String> {
        match self.read_all() {
            Ok(_) => Ok(()),
            Err(ReadErr::Corrupt(m)) | Err(ReadErr::Io(m)) => Err(m),
        }
    }
    /// Odczyt-zmiana-zapis pod blokadą. Uszkodzony plik jest odkładany jako `.corrupt-*` i zaczynamy od pustego
    /// (użytkownik wpisuje hasła ponownie; konta pokazują „wymaga zalogowania”).
    fn modify<R>(&self, f: impl FnOnce(&mut SecretMap) -> R) -> Result<R, String> {
        let _l = self.lock()?;
        let mut map = match self.read_all() {
            Ok(m) => m,
            Err(ReadErr::Corrupt(why)) => {
                log::warn!("{why}; zaczynam od pustego magazynu sekretów");
                backup_corrupt(&self.path());
                backup_corrupt(&self.key_path());
                let _ = std::fs::remove_file(self.path());
                let _ = std::fs::remove_file(self.key_path());
                SecretMap::new()
            }
            Err(ReadErr::Io(m)) => return Err(m),
        };
        let r = f(&mut map);
        self.write_all(&map)?;
        Ok(r)
    }
}

impl SecretStore for EncryptedFile {
    fn label(&self) -> &'static str {
        "file"
    }
    fn get(&self, account: &str, key: &str) -> Result<Option<String>, String> {
        let _l = self.lock()?;
        match self.read_all() {
            Ok(m) => Ok(m.get(account).and_then(|a| a.get(key)).cloned()),
            Err(ReadErr::Corrupt(why)) => {
                log::warn!("{why}");
                Ok(None)
            }
            Err(ReadErr::Io(m)) => Err(m),
        }
    }
    fn set(&self, account: &str, key: &str, value: &str) -> Result<(), String> {
        self.modify(|m| {
            m.entry(account.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        })
    }
    fn delete(&self, account: &str, key: &str) -> Result<(), String> {
        self.modify(|m| {
            if let Some(a) = m.get_mut(account) {
                a.remove(key);
                if a.is_empty() {
                    m.remove(account);
                }
            }
        })
    }
    fn delete_account(&self, account: &str, _keys: &[String]) -> Result<(), String> {
        self.modify(|m| {
            m.remove(account);
        })
    }
}

/// Magazyn sekretów w pamięci (testy, konta tymczasowe). Nic nie trafia na dysk.
#[derive(Default)]
pub struct MemSecrets(Mutex<SecretMap>);

impl SecretStore for MemSecrets {
    fn label(&self) -> &'static str {
        "memory"
    }
    fn get(&self, account: &str, key: &str) -> Result<Option<String>, String> {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Ok(m.get(account).and_then(|a| a.get(key)).cloned())
    }
    fn set(&self, account: &str, key: &str, value: &str) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(account.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, account: &str, key: &str) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = m.get_mut(account) {
            a.remove(key);
        }
        Ok(())
    }
}

// ---------- magazyn kont ----------

/// Magazyn kont w katalogu danych.
pub struct Store {
    dir: PathBuf,
    /// Magazyn kluczy systemu (jeśli dostępny); wybierany leniwie przy pierwszym zapisie sekretu.
    keyring: OnceLock<Option<Arc<dyn SecretStore>>>,
    allow_keyring: bool,
    /// Nadpisanie magazynu sekretów (testy): wszystkie sekrety trafiają tutaj.
    forced: Option<Arc<dyn SecretStore>>,
}

impl Store {
    /// Standardowy magazyn: sekrety w kluczach systemu, a gdy są niedostępne — w zaszyfrowanym pliku.
    /// Zmienna `SYNCRON_SECRETS=file` wymusza plik (serwery, CI, usługa bez sesji D-Bus).
    pub fn open(dir: &Path) -> Store {
        let allow = std::env::var("SYNCRON_SECRETS")
            .map(|v| !v.trim().eq_ignore_ascii_case("file"))
            .unwrap_or(true);
        Store {
            dir: dir.to_path_buf(),
            keyring: OnceLock::new(),
            allow_keyring: allow,
            forced: None,
        }
    }
    /// Tylko zaszyfrowany plik (bez dotykania kluczy systemu).
    pub fn file_only(dir: &Path) -> Store {
        Store {
            allow_keyring: false,
            ..Store::open(dir)
        }
    }
    /// Własny magazyn sekretów (testy).
    pub fn with_secrets(dir: &Path, s: Arc<dyn SecretStore>) -> Store {
        Store {
            forced: Some(s),
            ..Store::file_only(dir)
        }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn path(&self) -> PathBuf {
        self.dir.join(ACCOUNTS_FILE)
    }

    fn keyring(&self) -> Option<Arc<dyn SecretStore>> {
        if !self.allow_keyring {
            return None;
        }
        self.keyring
            .get_or_init(|| Keyring::probe().then(|| Arc::new(Keyring) as Arc<dyn SecretStore>))
            .clone()
    }
    /// Magazyn dla NOWYCH sekretów: klucze systemu, a gdy niedostępne — plik.
    fn primary(&self) -> (String, Arc<dyn SecretStore>) {
        if let Some(f) = &self.forced {
            return (f.label().to_string(), f.clone());
        }
        match self.keyring() {
            Some(k) => ("keyring".into(), k),
            None => ("file".into(), Arc::new(EncryptedFile::new(&self.dir))),
        }
    }
    /// Magazyn, w którym leżą sekrety konta.
    fn secrets_of(&self, a: &Account) -> Result<Arc<dyn SecretStore>, String> {
        if let Some(f) = &self.forced {
            return Ok(f.clone());
        }
        if a.secret_store == "keyring" {
            return self.keyring().ok_or_else(|| {
                format!(
                    "Hasła konta „{}” są w magazynie kluczy systemu, który jest teraz niedostępny (brak sesji D-Bus/Secret Service?).",
                    a.name
                )
            });
        }
        Ok(Arc::new(EncryptedFile::new(&self.dir)))
    }

    fn load_raw(&self) -> Result<Vec<Account>, String> {
        let p = self.path();
        let raw = match std::fs::read(&p) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("Nie można odczytać {}: {e}", p.display())),
        };
        let raw = raw
            .strip_prefix(b"\xEF\xBB\xBF")
            .map_or(raw.clone(), <[u8]>::to_vec);
        if raw.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(Vec::new());
        }
        #[derive(Deserialize)]
        struct Doc {
            #[serde(default)]
            accounts: Vec<Account>,
        }
        let parsed = serde_json::from_slice::<Doc>(&raw)
            .map(|d| d.accounts)
            .or_else(|_| serde_json::from_slice::<Vec<Account>>(&raw));
        parsed.map_err(|e| {
            let kept = backup_corrupt(&p)
                .map(|b| format!(" Kopia pliku: {}.", b.display()))
                .unwrap_or_default();
            format!(
                "Plik kont {} jest uszkodzony ({e}); nic nie zostało zmienione.{kept}",
                p.display()
            )
        })
    }

    fn save(&self, accounts: &[Account]) -> Result<(), String> {
        #[derive(Serialize)]
        struct Doc<'a> {
            version: u32,
            accounts: &'a [Account],
        }
        let json = serde_json::to_vec_pretty(&Doc {
            version: 1,
            accounts,
        })
        .map_err(|e| e.to_string())?;
        write_atomic(&self.path(), &json, true)
    }

    /// Odczyt-zmiana-zapis pod blokadą. Przy uszkodzonym pliku zwraca błąd i niczego nie zapisuje.
    pub fn update_accounts<R>(
        &self,
        f: impl FnOnce(&mut Vec<Account>) -> Result<R, String>,
    ) -> Result<R, String> {
        let _l = acquire(&self.dir, "accounts.lock", &ACCOUNTS_PROC)?;
        let mut v = self.load_raw()?;
        let r = f(&mut v)?;
        self.save(&v)?;
        bump_generation();
        Ok(r)
    }

    pub fn list(&self) -> Result<Vec<Account>, String> {
        let _l = acquire(&self.dir, "accounts.lock", &ACCOUNTS_PROC)?;
        self.load_raw()
    }

    pub fn get(&self, name: &str) -> Result<Account, String> {
        let c = canon_name(name);
        self.list()?
            .into_iter()
            .find(|a| canon_name(&a.name) == c)
            .ok_or_else(|| format!("Konto „{name}” nie istnieje."))
    }

    /// Lista z wyliczonym stanem „wymaga zalogowania” (brak zapisanego sekretu).
    pub fn list_view(&self) -> Result<Vec<AccountView>, String> {
        let mut out = Vec::new();
        for a in self.list()? {
            let mut needs = a.needs_login;
            if !needs && !a.secret_keys.is_empty() {
                needs = match self.secrets_of(&a) {
                    Ok(s) => a
                        .secret_keys
                        .iter()
                        .any(|k| !matches!(s.get(&a.name, k), Ok(Some(_)))),
                    Err(_) => true,
                };
            }
            out.push(AccountView {
                name: a.name,
                kind: a.kind,
                params: a.params,
                secret_keys: a.secret_keys,
                needs_login: needs,
            });
        }
        Ok(out)
    }

    fn check_new(kind: &str, params: &BTreeMap<String, String>) -> Result<(), String> {
        if !valid_kind(kind) {
            return Err(format!("Nieobsługiwany rodzaj konta: „{kind}”."));
        }
        let kinds = remote::spec::remote_kinds();
        if !kinds.is_empty() && kind != "mem" && !kinds.iter().any(|k| k.kind == kind) {
            return Err(format!("Nieobsługiwany rodzaj konta: „{kind}”."));
        }
        for (k, v) in params {
            if !valid_param_key(k) {
                return Err(format!("Niedozwolony parametr konta: „{k}”."));
            }
            if is_secret_param(kind, k) {
                if v.len() > 64 * 1024 {
                    return Err(format!("Wartość pola „{k}” jest zbyt długa."));
                }
            } else if has_url_userinfo(v) {
                return Err(format!(
                    "Pole „{k}”: nie wpisuj użytkownika ani hasła w adresie (user:hasło@host); użyj pól Użytkownik i Hasło."
                ));
            } else if v.len() > 4096 || v.chars().any(|c| c == '\0' || c == '\n' || c == '\r') {
                return Err(format!(
                    "Wartość pola „{k}” jest zbyt długa lub zawiera znaki nowej linii."
                ));
            }
        }
        Ok(())
    }

    /// Dodaje konto. `all` zawiera wszystkie pola formularza; sekrety są rozpoznawane (`is_secret_param`)
    /// i trafiają do magazynu sekretów, reszta do `accounts.json`.
    pub fn add(
        &self,
        name: &str,
        kind: &str,
        all: &BTreeMap<String, String>,
    ) -> Result<Account, String> {
        let name: String = name.trim().nfc().collect();
        if !valid_name(&name) {
            return Err("Nazwa konta: 2–64 znaki, zaczyna się od litery lub cyfry; litery, cyfry, spacja, _ - ."
                .to_string());
        }
        Self::check_new(kind, all)?;
        let (mut public, mut secret) = (BTreeMap::new(), BTreeMap::new());
        for (k, v) in all.iter().filter(|(_, v)| !v.is_empty()) {
            if is_secret_param(kind, k) {
                secret.insert(k.clone(), v.clone());
            } else {
                public.insert(k.clone(), v.clone());
            }
        }
        self.add_split(
            Account {
                name,
                kind: kind.to_string(),
                params: public,
                secret_keys: Vec::new(),
                secret_store: String::new(),
                needs_login: false,
            },
            &secret,
        )
    }

    /// Dodaje konto z już rozdzielonymi sekretami.
    pub fn add_split(
        &self,
        mut acc: Account,
        secrets: &BTreeMap<String, String>,
    ) -> Result<Account, String> {
        acc.name = acc.name.nfc().collect();
        if !valid_name(&acc.name) {
            return Err("Nieprawidłowa nazwa konta.".into());
        }
        let c = canon_name(&acc.name);
        // wstępny test istnienia (bez zapisu sekretów, gdy nazwa zajęta)
        if self.list()?.iter().any(|a| canon_name(&a.name) == c) {
            return Err(format!(
                "Konto „{}” już istnieje. Wybierz inną nazwę albo najpierw usuń to konto.",
                acc.name
            ));
        }
        let (label, store) = self.primary();
        acc.secret_keys = secrets.keys().cloned().collect();
        acc.secret_store = if secrets.is_empty() {
            String::new()
        } else {
            label
        };
        // najpierw rejestrujemy konto: przegrany wyścig o nazwę nie dotyka sekretów zwycięzcy
        let added = acc.clone();
        self.update_accounts(|v| {
            if v.iter().any(|a| canon_name(&a.name) == c) {
                return Err(format!("Konto „{}” już istnieje.", added.name));
            }
            v.push(added);
            Ok(())
        })?;
        for (k, v) in secrets {
            if let Err(e) = store.set(&acc.name, k, v) {
                let _ = store.delete_account(&acc.name, &acc.secret_keys);
                let _ = self.update_accounts(|v| {
                    v.retain(|a| canon_name(&a.name) != c);
                    Ok(())
                });
                return Err(format!("Nie można zapisać hasła w magazynie sekretów: {e}"));
            }
        }
        Ok(acc)
    }

    /// Aktualizuje konto: jawne pola są zastępowane przesłanym zestawem; sekrety — tylko niepuste
    /// (puste/nieobecne zostają bez zmian). Zmiana sekretu zdejmuje flagę „wymaga zalogowania”.
    pub fn update(&self, name: &str, all: &BTreeMap<String, String>) -> Result<Account, String> {
        let cur = self.get(name)?;
        Self::check_new(&cur.kind, all)?;
        let (mut public, mut secret) = (BTreeMap::new(), BTreeMap::new());
        for (k, v) in all.iter().filter(|(_, v)| !v.is_empty()) {
            if is_secret_param(&cur.kind, k) {
                secret.insert(k.clone(), v.clone());
            } else {
                public.insert(k.clone(), v.clone());
            }
        }
        let (label, store) = if cur.secret_keys.is_empty() {
            self.primary()
        } else {
            (cur.secret_store.clone(), self.secrets_of(&cur)?)
        };
        for (k, v) in &secret {
            store
                .set(&cur.name, k, v)
                .map_err(|e| format!("Nie można zapisać hasła w magazynie sekretów: {e}"))?;
        }
        let c = canon_name(&cur.name);
        self.update_accounts(|v| {
            let a = v
                .iter_mut()
                .find(|a| canon_name(&a.name) == c)
                .ok_or_else(|| format!("Konto „{}” nie istnieje.", cur.name))?;
            a.params = public;
            for k in secret.keys() {
                if !a.secret_keys.contains(k) {
                    a.secret_keys.push(k.clone());
                }
            }
            a.secret_keys.sort();
            if !secret.is_empty() {
                a.needs_login = false;
                if a.secret_store.is_empty() {
                    a.secret_store = label;
                }
            }
            Ok(a.clone())
        })
    }

    /// Usuwa konto wraz z jego sekretami.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        let c = canon_name(name);
        let removed = self.update_accounts(|v| {
            let i = v
                .iter()
                .position(|a| canon_name(&a.name) == c)
                .ok_or_else(|| format!("Konto „{name}” nie istnieje."))?;
            Ok(v.remove(i))
        })?;
        if !removed.secret_keys.is_empty() {
            self.secrets_of(&removed)
                .and_then(|s| s.delete_account(&removed.name, &removed.secret_keys))
                .map_err(|e| {
                    format!("Konto usunięto, ale nie udało się wyczyścić jego sekretów: {e}")
                })?;
        }
        Ok(())
    }

    /// Zapisuje pojedynczy sekret konta (np. odświeżony token OAuth).
    pub fn set_secret(&self, name: &str, key: &str, value: &str) -> Result<(), String> {
        if !valid_param_key(key) {
            return Err("Niedozwolona nazwa sekretu.".into());
        }
        let cur = self.get(name)?;
        let (label, store) = if cur.secret_keys.is_empty() {
            self.primary()
        } else {
            (cur.secret_store.clone(), self.secrets_of(&cur)?)
        };
        store.set(&cur.name, key, value)?;
        let c = canon_name(&cur.name);
        let key = key.to_string();
        self.update_accounts(|v| {
            if let Some(a) = v.iter_mut().find(|a| canon_name(&a.name) == c) {
                if !a.secret_keys.contains(&key) {
                    a.secret_keys.push(key);
                    a.secret_keys.sort();
                }
                if a.secret_store.is_empty() {
                    a.secret_store = label;
                }
            }
            Ok(())
        })
        // zmiana kont unieważnia pamięć podręczną połączeń: kolejne użycie otworzy backend z aktualnymi sekretami
    }

    /// Parametry do `remote::open`: jawne + sekrety; `on_secret_update` zapisuje odświeżone tokeny.
    pub fn params_for(self: &Arc<Self>, name: &str) -> Result<Params, String> {
        let a = self.get(name)?;
        if a.needs_login {
            return Err(format!(
                "Konto „{}” wymaga ponownego zalogowania (Konta → Zaloguj).",
                a.name
            ));
        }
        let mut p = Params {
            public: a.params.clone(),
            ..Params::default()
        };
        if !a.secret_keys.is_empty() {
            let store = self.secrets_of(&a)?;
            for k in &a.secret_keys {
                match store.get(&a.name, k)? {
                    Some(v) => {
                        p.secret.insert(k.clone(), v);
                    }
                    None => {
                        return Err(format!(
                            "Brakuje zapisanego sekretu „{k}” konta „{}”. Zaloguj się ponownie lub wpisz dane konta od nowa.",
                            a.name
                        ))
                    }
                }
            }
        }
        let (me, acc) = (self.clone(), a.name.clone());
        p.on_secret_update = Some(Arc::new(move |key: &str, value: &str| {
            if let Err(e) = me.set_secret(&acc, key, value) {
                log::warn!("nie można zapisać odświeżonego sekretu konta {acc}: {e}");
            }
        }));
        Ok(p)
    }

    /// Otwiera backend konta dla folderu głównego `root` (z pamięcią podręczną połączeń).
    pub fn open_backend(
        self: &Arc<Self>,
        name: &str,
        root: &str,
    ) -> Result<Arc<dyn Backend>, String> {
        let a = self.get(name)?;
        let key = format!(
            "{}\u{1}{}\u{1}{}",
            self.dir.display(),
            canon_name(&a.name),
            root
        );
        if let Some(b) = cache_get(&key) {
            return Ok(b);
        }
        let p = self.params_for(name)?;
        let b: Arc<dyn Backend> = remote::open(&a.kind, &p, root)
            .map_err(|e| format!("Konto „{}”: {e}", a.name))?
            .into();
        cache_put(key, b.clone());
        Ok(b)
    }

    /// Test połączenia: czas odpowiedzi i krótki opis serwera.
    pub fn test(self: &Arc<Self>, name: &str) -> Result<TestInfo, String> {
        let b = self.open_backend(name, "")?;
        run_test(b.as_ref(), name)
    }

    pub fn browse(self: &Arc<Self>, name: &str, path: &str) -> Result<Vec<DirEntryInfo>, String> {
        let b = self.open_backend(name, "")?;
        browse_with(b.as_ref(), name, path)
    }

    pub fn about(self: &Arc<Self>, name: &str) -> Result<AboutInfo, String> {
        let b = self.open_backend(name, "")?;
        about_with(b.as_ref(), name)
    }

    // ----- migracja z rclone -----

    /// Importuje konta z pliku `rclone.conf` (webdav/ftp/sftp/s3 jako konta natywne z hasłami; drive/onedrive/dropbox
    /// jako konta „wymaga zalogowania”). Istniejących kont nie nadpisuje. Hasła „obscure” są odwracane
    /// (AES-CTR stałym kluczem rclone) i trafiają do magazynu sekretów.
    pub fn import_rclone_conf(&self, path: &Path) -> Result<ImportReport, String> {
        let text = std::fs::read(path)
            .map_err(|e| format!("Nie można odczytać {}: {e}", path.display()))?;
        let text = crate::rules::decode_text(&text);
        let mut rep = ImportReport::default();
        for (name, kv) in parse_rclone_conf(&text) {
            let get = |k: &str| kv.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty());
            let ty = get("type").unwrap_or("").to_string();
            let skip = |rep: &mut ImportReport, why: String| {
                rep.skipped.push(SkipInfo {
                    name: name.clone(),
                    reason: why,
                })
            };
            if !valid_name(&name) {
                skip(
                    &mut rep,
                    "nazwa konta zawiera niedozwolone znaki (zmień ją w pliku rclone.conf)".into(),
                );
                continue;
            }
            if self
                .list()?
                .iter()
                .any(|a| canon_name(&a.name) == canon_name(&name))
            {
                skip(&mut rep, "konto o tej nazwie już istnieje".into());
                continue;
            }
            let mut public = BTreeMap::new();
            let mut secret = BTreeMap::new();
            let needs_login = false;
            let put = |m: &mut BTreeMap<String, String>, to: &str, from: &str| {
                if let Some(v) = get(from) {
                    m.insert(to.to_string(), v.to_string());
                }
            };
            let mut reveal_err = None;
            let mut reveal = |m: &mut BTreeMap<String, String>, to: &str, from: &str| {
                if let Some(v) = get(from) {
                    match rclone_reveal(v) {
                        Ok(p) => {
                            m.insert(to.to_string(), p);
                        }
                        Err(e) => reveal_err = Some(format!("pole „{from}”: {e}")),
                    }
                }
            };
            let kind = match ty.as_str() {
                "webdav" => {
                    put(&mut public, "url", "url");
                    put(&mut public, "user", "user");
                    let vendor = match get("vendor").unwrap_or("other") {
                        v @ ("nextcloud" | "owncloud" | "sharepoint") => v,
                        _ => "other",
                    };
                    public.insert("vendor".into(), vendor.into());
                    reveal(&mut secret, "pass", "pass");
                    if get("bearer_token").is_some() {
                        rep.warnings
                            .push(format!("{name}: token bearer nie jest przenoszony"));
                    }
                    "webdav"
                }
                "ftp" => {
                    put(&mut public, "host", "host");
                    put(&mut public, "port", "port");
                    put(&mut public, "user", "user");
                    let tls = if get("tls") == Some("true") {
                        "implicit"
                    } else if get("explicit_tls") == Some("true") {
                        "explicit"
                    } else {
                        "none"
                    };
                    public.insert("tls".into(), tls.into());
                    if get("no_check_certificate") == Some("true") {
                        public.insert("no_check_certificate".into(), "true".into());
                    }
                    reveal(&mut secret, "pass", "pass");
                    if tls == "none" {
                        "ftp"
                    } else {
                        "ftps"
                    }
                }
                "sftp" => {
                    put(&mut public, "host", "host");
                    put(&mut public, "port", "port");
                    put(&mut public, "user", "user");
                    put(&mut public, "key_file", "key_file");
                    put(&mut public, "known_hosts_file", "known_hosts_file");
                    reveal(&mut secret, "pass", "pass");
                    reveal(&mut secret, "key_file_pass", "key_file_pass");
                    if get("key_pem").is_some() {
                        rep.warnings.push(format!(
                            "{name}: klucz wklejony w konfiguracji (key_pem) nie jest przenoszony — wskaż plik klucza"
                        ));
                    }
                    "sftp"
                }
                "s3" => {
                    put(&mut public, "provider", "provider");
                    put(&mut public, "region", "region");
                    put(&mut public, "endpoint", "endpoint");
                    put(&mut secret, "access_key_id", "access_key_id");
                    // w rclone klucz tajny S3 jest zapisany jawnie (nie „obscure”)
                    put(&mut secret, "secret_access_key", "secret_access_key");
                    "s3"
                }
                "drive" | "onedrive" | "dropbox" | "crypt" => {
                    skip(&mut rep, "nieobsługiwane w tej wersji".into());
                    continue;
                }
                "" => {
                    skip(&mut rep, "sekcja bez pola type".into());
                    continue;
                }
                other => {
                    skip(
                        &mut rep,
                        format!("rodzaj „{other}” nie jest obsługiwany natywnie"),
                    );
                    continue;
                }
            };
            if let Some(e) = reveal_err {
                skip(&mut rep, format!("nie można odczytać hasła ({e})"));
                continue;
            }
            let acc = Account {
                name: name.clone(),
                kind: kind.to_string(),
                params: public,
                secret_keys: Vec::new(),
                secret_store: String::new(),
                needs_login,
            };
            match self.add_split(acc, &secret) {
                Ok(_) if needs_login => rep.needs_login.push(name),
                Ok(_) => rep.imported.push(name),
                Err(e) => skip(&mut rep, e),
            }
        }
        // oznacz import jako wykonany (jeśli to plik z katalogu danych)
        if path.parent() == Some(self.dir.as_path()) {
            let _ = std::fs::write(
                self.dir.join(RCLONE_DONE),
                chrono::Local::now().to_rfc3339(),
            );
        }
        Ok(rep)
    }

    /// Usuwa plik źródłowy `rclone.conf` (zawiera hasła tylko „zaciemnione”, odwracalne) po udanym imporcie.
    pub fn remove_rclone_source(&self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| format!("Nie można usunąć {}: {e}", path.display()))
    }
}

pub const RCLONE_DONE: &str = "rclone-import.done";

/// Stary `rclone.conf` w katalogu danych, jeśli nie był jeszcze importowany ani odrzucony: aplikacja proponuje import.
pub fn pending_rclone_import(dir: &Path) -> Option<PathBuf> {
    let p = dir.join("rclone.conf");
    let non_empty = std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false);
    (non_empty && !dir.join(RCLONE_DONE).exists()).then_some(p)
}

/// Użytkownik odrzucił propozycję importu: nie pytamy ponownie.
pub fn dismiss_rclone_import(dir: &Path) {
    let _ = std::fs::write(dir.join(RCLONE_DONE), "odrzucono");
}

fn run_test(b: &dyn Backend, name: &str) -> Result<TestInfo, String> {
    let t = Instant::now();
    let info = b.ping().map_err(|e| map_account_err(name, &e))?;
    Ok(TestInfo {
        ms: t.elapsed().as_millis() as u64,
        info,
    })
}

fn browse_with(b: &dyn Backend, name: &str, path: &str) -> Result<Vec<DirEntryInfo>, String> {
    let path = remote::clean_path(path).map_err(|e| e.to_string())?;
    let mut v: Vec<DirItem> = b.list_dir(&path).map_err(|e| map_account_err(name, &e))?;
    if path.is_empty() {
        v.retain(|d| {
            d.name != crate::location::TRASH_DIR && d.name != crate::location::LEGACY_TRASH_DIR
        });
    }
    v.retain(|d| crate::names::check_rel(&d.name).is_ok());
    v.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v.truncate(5000);
    Ok(v.into_iter()
        .map(|d| DirEntryInfo {
            name: d.name,
            is_dir: d.is_dir,
        })
        .collect())
}

fn about_with(b: &dyn Backend, name: &str) -> Result<AboutInfo, String> {
    match b.about().map_err(|e| map_account_err(name, &e))? {
        Some(q) => Ok(AboutInfo {
            total: q.total,
            used: q.used,
            free: q.free,
        }),
        None => Ok(AboutInfo {
            total: None,
            used: None,
            free: None,
        }),
    }
}

/// Komunikat po polsku dla błędu backendu konta (bez sekretów).
pub fn map_account_err(account: &str, e: &remote::RError) -> String {
    match e {
        remote::RError::Auth(m) => {
            format!("Konto „{account}”: {m}. Zaloguj się ponownie (Konta → Zaloguj / Edytuj).")
        }
        other => format!("Konto „{account}”: {other}"),
    }
}

// ---------- rozwiązywanie kont: globalnie ----------

static GENERATION: AtomicUsize = AtomicUsize::new(0);

fn bump_generation() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

type CacheMap = HashMap<String, (usize, Instant, Arc<dyn Backend>)>;

fn cache() -> &'static Mutex<CacheMap> {
    static C: OnceLock<Mutex<CacheMap>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Połączenia są używane ponownie przez 10 minut i tracą ważność po każdej zmianie kont.
fn cache_get(key: &str) -> Option<Arc<dyn Backend>> {
    let g = cache().lock().unwrap_or_else(|e| e.into_inner());
    let (gen, at, b) = g.get(key)?;
    (*gen == GENERATION.load(Ordering::SeqCst) && at.elapsed() < Duration::from_secs(600))
        .then(|| b.clone())
}

fn cache_put(key: String, b: Arc<dyn Backend>) {
    let mut g = cache().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    g.insert(key, (GENERATION.load(Ordering::SeqCst), Instant::now(), b));
}

fn global_slot() -> &'static Mutex<Option<Arc<Store>>> {
    static G: OnceLock<Mutex<Option<Arc<Store>>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(None))
}

/// Ustawia katalog danych, z którego `Location` rozwiązuje konta (wywoływane przy starcie aplikacji/CLI).
pub fn set_data_dir(dir: &Path) -> Arc<Store> {
    let s = Arc::new(Store::open(dir));
    *global_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(s.clone());
    bump_generation();
    s
}

/// Jak `set_data_dir`, z gotowym magazynem (testy).
pub fn set_global_store(s: Arc<Store>) {
    *global_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    bump_generation();
}

pub fn global() -> Option<Arc<Store>> {
    global_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

struct Ephemeral {
    kind: String,
    params: Params,
}

fn ephemeral() -> &'static Mutex<HashMap<String, Ephemeral>> {
    static E: OnceLock<Mutex<HashMap<String, Ephemeral>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Rejestruje konto tymczasowe tylko w pamięci procesu (testy; nie trafia na dysk). Ma pierwszeństwo przed magazynem.
pub fn register_ephemeral(name: &str, kind: &str, public: &[(&str, &str)]) {
    let mut p = Params::default();
    for (k, v) in public {
        p.public.insert(k.to_string(), v.to_string());
    }
    ephemeral()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            canon_name(name),
            Ephemeral {
                kind: kind.to_string(),
                params: p,
            },
        );
    bump_generation();
}

pub fn unregister_ephemeral(name: &str) {
    ephemeral()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&canon_name(name));
    bump_generation();
}

/// Otwiera backend konta `name` dla folderu `root`: konta tymczasowe, potem magazyn kont z `set_data_dir`.
pub fn open_backend(name: &str, root: &str) -> Result<Arc<dyn Backend>, String> {
    {
        let g = ephemeral().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.get(&canon_name(name)) {
            return remote::open(&e.kind, &e.params, root)
                .map(Arc::from)
                .map_err(|e| map_account_err(name, &e));
        }
    }
    match global() {
        Some(s) => s.open_backend(name, root),
        None => Err(format!(
            "Konto „{name}” nie istnieje (katalog kont nie jest ustawiony)."
        )),
    }
}

/// Import z `rclone.conf` do magazynu globalnego.
pub fn import_rclone_conf(path: &Path) -> Result<ImportReport, String> {
    global()
        .ok_or("Katalog kont nie jest ustawiony.")?
        .import_rclone_conf(path)
}

// ---------- rclone.conf ----------

/// Prosty parser INI używany przez rclone: `[sekcja]`, `klucz = wartość`, komentarze `#`/`;`.
pub fn parse_rclone_conf(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut out: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some(n) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            out.push((n.trim().to_string(), BTreeMap::new()));
        } else if let (Some((k, v)), Some(last)) = (t.split_once('='), out.last_mut()) {
            last.1.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// Stały klucz rclone do „zaciemniania” haseł (`rclone obscure`): to nie jest szyfrowanie, tylko kodowanie.
const RCLONE_KEY: [u8; 32] = [
    0x9c, 0x93, 0x5b, 0x48, 0x73, 0x0a, 0x55, 0x4d, 0x6b, 0xfd, 0x7c, 0x63, 0xc8, 0x86, 0xa9, 0x2b,
    0xd3, 0x90, 0x19, 0x8e, 0xb8, 0x12, 0x8a, 0xfb, 0xf4, 0xde, 0x16, 0x2b, 0x8b, 0x95, 0xf6, 0x38,
];

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn b64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for ch in b.chunks(3) {
        let n = ch.iter().fold(0u32, |a, x| (a << 8) | *x as u32) << (8 * (3 - ch.len()));
        for i in 0..=ch.len() {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

fn rclone_ctr(iv: &[u8], data: &mut [u8]) {
    use aes::cipher::{KeyIvInit, StreamCipher};
    let mut c = ctr::Ctr128BE::<aes::Aes256>::new((&RCLONE_KEY).into(), iv.into());
    c.apply_keystream(data);
}

/// Odwraca „zaciemnienie” hasła rclone (`rclone obscure`): base64 (URL, bez dopełnienia) z IV (16 B) i AES-256-CTR.
pub fn rclone_reveal(obscured: &str) -> Result<String, String> {
    let raw = b64_decode(obscured.trim()).ok_or("hasło nie jest poprawnie zakodowane")?;
    if raw.len() < 16 {
        return Err("hasło jest zbyt krótkie, by było zaciemnione przez rclone".into());
    }
    let (iv, ct) = raw.split_at(16);
    let mut buf = ct.to_vec();
    rclone_ctr(iv, &mut buf);
    String::from_utf8(buf).map_err(|_| "odszyfrowane hasło nie jest poprawnym tekstem UTF-8".into())
}

/// Tworzy „zaciemnione” hasło w formacie rclone z podanym IV (tylko do danych testowych).
#[doc(hidden)]
pub fn rclone_obscure_with_iv(plain: &str, iv: [u8; 16]) -> String {
    let mut buf = plain.as_bytes().to_vec();
    rclone_ctr(&iv, &mut buf);
    let mut raw = iv.to_vec();
    raw.extend(buf);
    b64_encode(&raw)
}
