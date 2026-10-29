//! Backend w pamięci (do testów silnika i interfejsu): współdzielony po `id`, bez sieci i dysku.
use super::*;
use std::sync::{Mutex, OnceLock};

type Store = Mutex<BTreeMap<String, BTreeMap<String, (Vec<u8>, i64)>>>;

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
    /// Czyści wszystkie dane magazynu `id` (testy).
    pub fn reset(id: &str) {
        stores().lock().unwrap().remove(id);
    }
    fn full(&self, rel: &str) -> String {
        join_path(&self.root, rel)
    }
    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<String, (Vec<u8>, i64)>) -> R) -> R {
        let mut g = stores().lock().unwrap();
        f(g.entry(self.id.clone()).or_default())
    }
}

impl Backend for MemBackend {
    fn kind(&self) -> &'static str {
        "mem"
    }
    fn list_files(&self, dir: &str) -> RResult<Vec<REntry>> {
        let base = self.full(&clean_path(dir)?);
        let prefix = if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        };
        let root_prefix = if self.root.is_empty() {
            String::new()
        } else {
            format!("{}/", self.root)
        };
        Ok(self.with(|m| {
            m.iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, (d, t))| REntry {
                    path: k[root_prefix.len()..].to_string(),
                    meta: RMeta {
                        size: d.len() as u64,
                        mtime: *t,
                        hash: None,
                    },
                })
                .collect()
        }))
    }
    fn list_dir(&self, dir: &str) -> RResult<Vec<DirItem>> {
        let base = self.full(&clean_path(dir)?);
        let prefix = if base.is_empty() {
            String::new()
        } else {
            format!("{base}/")
        };
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        self.with(|m| {
            for k in m.keys().filter(|k| k.starts_with(&prefix)) {
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
        let rel = clean_path(path)?;
        let full = self.full(&rel);
        Ok(self.with(|m| {
            m.get(&full).map(|(d, t)| REntry {
                path: rel.clone(),
                meta: RMeta {
                    size: d.len() as u64,
                    mtime: *t,
                    hash: None,
                },
            })
        }))
    }
    fn download(&self, path: &str, dest: &Path, _x: &Xfer) -> RResult<()> {
        let full = self.full(&clean_path(path)?);
        let data = self
            .with(|m| m.get(&full).map(|(d, _)| d.clone()))
            .ok_or_else(|| RError::NotFound(path.to_string()))?;
        std::fs::write(dest, data).map_err(|e| RError::Failed(e.to_string()))
    }
    fn upload(&self, src: &Path, path: &str, mtime: i64, _x: &Xfer) -> RResult<()> {
        let full = self.full(&clean_path(path)?);
        let data = std::fs::read(src).map_err(|e| RError::Failed(e.to_string()))?;
        self.with(|m| m.insert(full, (data, mtime)));
        Ok(())
    }
    fn delete(&self, path: &str) -> RResult<()> {
        let full = self.full(&clean_path(path)?);
        self.with(|m| m.remove(&full));
        Ok(())
    }
    fn rename(&self, from: &str, to: &str) -> RResult<()> {
        let (f, t) = (self.full(&clean_path(from)?), self.full(&clean_path(to)?));
        self.with(|m| match m.remove(&f) {
            Some(v) => {
                m.insert(t, v);
                Ok(())
            }
            None => Err(RError::NotFound(from.to_string())),
        })
    }
    fn mkdirs(&self, _dir: &str) -> RResult<()> {
        Ok(())
    }
}
