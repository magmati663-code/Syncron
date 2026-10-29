//! Backend w pamięci (do testów silnika i interfejsu): współdzielony po `id`, bez sieci i dysku.
//! Obsługuje wstrzykiwanie błędów (`Faults`) do testów odporności oraz opcjonalne skróty SHA-256.
use super::*;
use sha2::Digest;
use std::sync::{Mutex, OnceLock};

/// Błędy wstrzykiwane do magazynu `id` (wszystkie domyślnie wyłączone).
#[derive(Debug, Clone, Default)]
pub struct Faults {
    /// `list_files` kończy się błędem sieci (silnik musi przerwać przebieg bez usuwania).
    pub fail_list: bool,
    /// `list_dir` kończy się błędem.
    pub fail_list_dir: bool,
    /// `stat` kończy się błędem sieci.
    pub fail_stat: bool,
    /// Po tylu udanych wysłaniach kolejne kończą się błędem (`Some(0)` = od razu).
    pub fail_upload_after: Option<usize>,
    /// `download` kończy się błędem.
    pub fail_download: bool,
    /// `delete` kończy się błędem.
    pub fail_delete: bool,
    /// `rename` kończy się błędem (brak przenoszenia po stronie serwera: sprawdza zapasowe kopiuj+usuń).
    pub fail_rename: bool,
    /// Wszystkie operacje kończą się błędem logowania.
    pub auth_fail: bool,
    /// Operacje zapisu na ścieżkach zawierających jeden z tych fragmentów kończą się `Busy`.
    pub busy_paths: Vec<String>,
    /// Wysłany plik jest zapisywany bez ostatniego bajtu (niezgodny rozmiar po wysłaniu).
    pub truncate_upload: bool,
    /// Zapisy (wysłanie, usunięcie, zmiana nazwy) na ścieżkach zawierających jeden z tych fragmentów kończą się
    /// błędem braku miejsca (nie `Busy`).
    pub fail_write_paths: Vec<String>,
    /// Numery prób wysłania (od 0, liczone łącznie z nieudanymi), które kończą się błędem sieci.
    pub fail_upload_on: Vec<usize>,
    /// Numery prób wysłania (od 0), które kończą się `Busy`.
    pub busy_upload_on: Vec<usize>,
    /// Wpisy, które `list_files` pomija, choć `stat` je widzi (niespójny listing). Dopasowanie: równość ścieżki.
    pub hide_from_list: Vec<String>,
    /// Opóźnienie każdego wysłania/pobrania w milisekundach (test równoległości).
    pub latency_ms: u64,
}

#[derive(Default)]
struct State {
    files: BTreeMap<String, (Vec<u8>, i64)>,
    faults: Faults,
    uploads: usize,
    attempts: usize,
    hash: bool,
    ops: usize,
}

type Store = Mutex<BTreeMap<String, State>>;

fn stores() -> &'static Store {
    static S: OnceLock<Store> = OnceLock::new();
    S.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub struct MemBackend {
    id: String,
    root: String,
}

impl MemBackend {
    pub fn shared(id: &str, root: &str) -> MemBackend {
        MemBackend {
            id: id.to_string(),
            root: root.to_string(),
        }
    }
    /// Czyści wszystkie dane i błędy magazynu `id` (testy).
    pub fn reset(id: &str) {
        stores().lock().unwrap().remove(id);
    }
    /// Ustawia wstrzykiwane błędy magazynu `id`.
    pub fn set_faults(id: &str, f: Faults) {
        let mut g = stores().lock().unwrap();
        let st = g.entry(id.to_string()).or_default();
        st.faults = f;
        st.uploads = 0;
        st.attempts = 0;
    }
    pub fn clear_faults(id: &str) {
        Self::set_faults(id, Faults::default());
    }
    /// Włącza zwracanie skrótów SHA-256 przez `list_files`/`stat`.
    pub fn set_hashes(id: &str, on: bool) {
        stores()
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .hash = on;
    }
    /// Zapis „surowy” pod dokładnym kluczem `key` (względem początku magazynu, bez czyszczenia ścieżki):
    /// pozwala wstawić wpisy, których prawdziwy serwer nie powinien zwracać (`../x`, `/abs`).
    pub fn put_raw(id: &str, key: &str, data: &[u8], mtime: i64) {
        let mut g = stores().lock().unwrap();
        g.entry(id.to_string())
            .or_default()
            .files
            .insert(key.to_string(), (data.to_vec(), mtime));
    }
    /// Zawartość pliku (klucz bezwzględny w magazynie, tj. razem z korzeniem konta).
    pub fn get_raw(id: &str, key: &str) -> Option<Vec<u8>> {
        let g = stores().lock().unwrap();
        g.get(id)?.files.get(key).map(|(d, _)| d.clone())
    }
    /// Wszystkie klucze (bezwzględne) magazynu `id`.
    pub fn keys(id: &str) -> Vec<String> {
        let g = stores().lock().unwrap();
        g.get(id)
            .map(|s| s.files.keys().cloned().collect())
            .unwrap_or_default()
    }
    /// Liczba wywołań operacji na magazynie (statystyka do testów).
    pub fn op_count(id: &str) -> usize {
        stores().lock().unwrap().get(id).map(|s| s.ops).unwrap_or(0)
    }
    fn full(&self, rel: &str) -> String {
        join_path(&self.root, rel)
    }
    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut g = stores().lock().unwrap();
        let st = g.entry(self.id.clone()).or_default();
        st.ops += 1;
        f(st)
    }
    fn check_auth(&self) -> RResult<()> {
        if self.with(|s| s.faults.auth_fail) {
            Err(RError::Auth("HTTP 401 (wstrzyknięty błąd)".into()))
        } else {
            Ok(())
        }
    }
    fn check_write(&self, full: &str) -> RResult<()> {
        if self
            .with(|s| s.faults.fail_write_paths.iter().any(|p| full.contains(p.as_str())))
        {
            return Err(RError::Quota(format!(
                "wstrzyknięty brak miejsca: {full}"
            )));
        }
        Ok(())
    }
    fn lag(&self) {
        let ms = self.with(|s| s.faults.latency_ms);
        if ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
    fn check_busy(&self, full: &str) -> RResult<()> {
        if self.with(|s| {
            s.faults
                .busy_paths
                .iter()
                .any(|p| full.contains(p.as_str()))
        }) {
            Err(RError::Busy(format!("plik zajęty: {full}")))
        } else {
            Ok(())
        }
    }
    fn meta(st: &State, d: &[u8], t: i64) -> RMeta {
        RMeta {
            size: d.len() as u64,
            mtime: t,
            hash: st.hash.then(|| Hash {
                algo: "sha256",
                hex: sha2::Sha256::digest(d)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            }),
        }
    }
    fn prefix_of(&self, base: &str) -> String {
        if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        }
    }
}

impl Backend for MemBackend {
    fn kind(&self) -> &'static str {
        "mem"
    }
    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>> {
        self.check_auth()?;
        if self.with(|s| s.faults.fail_list) {
            return Err(RError::Network("wstrzyknięty błąd listowania".into()));
        }
        let base = self.full(&clean_path(dir)?);
        let prefix = self.prefix_of(&base);
        let root_prefix = self.prefix_of(&self.root);
        Ok(self.with(|st| {
            st.files
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, (d, t))| REntry {
                    path: k[root_prefix.len().min(k.len())..].to_string(),
                    meta: Self::meta(st, d, *t),
                })
                .filter(|e| !st.faults.hide_from_list.contains(&e.path))
                .collect()
        }))
    }
    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>> {
        self.check_auth()?;
        if self.with(|s| s.faults.fail_list_dir) {
            return Err(RError::Network("wstrzyknięty błąd listowania".into()));
        }
        let base = self.full(&clean_path(dir)?);
        let prefix = self.prefix_of(&base);
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        self.with(|st| {
            for k in st.files.keys().filter(|k| k.starts_with(&prefix)) {
                let rest = &k[prefix.len()..];
                match rest.split_once('/') {
                    Some((d, _)) => {
                        seen.insert(d.to_string(), true);
                    }
                    None => {
                        seen.entry(rest.to_string()).or_insert(false);
                    }
                }
            }
        });
        Ok(seen
            .into_iter()
            .map(|(name, is_dir)| DirItem { name, is_dir })
            .collect())
    }
    fn stat(&self, path: &str) -> RResult<Option<REntry>> {
        self.check_auth()?;
        if self.with(|s| s.faults.fail_stat) {
            return Err(RError::Network("wstrzyknięty błąd stat".into()));
        }
        let rel = clean_path(path)?;
        let full = self.full(&rel);
        Ok(self.with(|st| {
            st.files.get(&full).map(|(d, t)| REntry {
                path: rel.clone(),
                meta: Self::meta(st, d, *t),
            })
        }))
    }
    fn download(&self, path: &str, dest: &Path, _x: &Xfer) -> RResult<()> {
        self.check_auth()?;
        if self.with(|s| s.faults.fail_download) {
            return Err(RError::Network("wstrzyknięty błąd pobierania".into()));
        }
        self.lag();
        let full = self.full(&clean_path(path)?);
        let data = self
            .with(|m| m.files.get(&full).map(|(d, _)| d.clone()))
            .ok_or_else(|| RError::NotFound(path.to_string()))?;
        std::fs::write(dest, data).map_err(|e| RError::Failed(e.to_string()))
    }
    fn upload(&self, src: &Path, path: &str, mtime: i64, _x: &Xfer) -> RResult<()> {
        self.check_auth()?;
        let full = self.full(&clean_path(path)?);
        self.check_busy(&full)?;
        self.check_write(&full)?;
        self.lag();
        let mut data = std::fs::read(src).map_err(|e| RError::Failed(e.to_string()))?;
        self.with(|st| {
            let attempt = st.attempts;
            st.attempts += 1;
            if st.faults.busy_upload_on.contains(&attempt) {
                return Err(RError::Busy("wstrzyknięta zajętość".into()));
            }
            if st.faults.fail_upload_on.contains(&attempt) {
                return Err(RError::Network("wstrzyknięty błąd wysyłania".into()));
            }
            if let Some(n) = st.faults.fail_upload_after {
                if st.uploads >= n {
                    return Err(RError::Network("wstrzyknięty błąd wysyłania".into()));
                }
            }
            st.uploads += 1;
            if st.faults.truncate_upload {
                data.pop();
            }
            st.files.insert(full, (data, mtime));
            Ok(())
        })
    }
    fn delete(&self, path: &str) -> RResult<()> {
        self.check_auth()?;
        let full = self.full(&clean_path(path)?);
        self.check_busy(&full)?;
        self.check_write(&full)?;
        if self.with(|s| s.faults.fail_delete) {
            return Err(RError::Network("wstrzyknięty błąd usuwania".into()));
        }
        self.with(|m| m.files.remove(&full));
        Ok(())
    }
    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        self.check_auth()?;
        let (f, t) = (self.full(&clean_path(from)?), self.full(&clean_path(to)?));
        self.check_busy(&f)?;
        self.check_write(&f)?;
        self.check_write(&t)?;
        if self.with(|s| s.faults.fail_rename) {
            return Err(RError::Failed("wstrzyknięty brak przenoszenia".into()));
        }
        self.with(|m| match m.files.remove(&f) {
            Some(v) => {
                m.files.insert(t, v);
                Ok(())
            }
            None => Err(RError::NotFound(from.to_string())),
        })
    }
    fn mkdirs(&self, _dir: &str) -> RResult<()> {
        self.check_auth()
    }
}
