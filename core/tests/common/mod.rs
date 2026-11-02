//! Wspólne narzędzia testów zdalnych lokalizacji: konto tymczasowe na backendzie w pamięci (`mem`).
#![allow(dead_code)]
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use syncron_core::accounts;
use syncron_core::model::Job;
use syncron_core::remote::mem::{Faults, MemBackend};

/// „Chmura” w pamięci: unikalne konto + magazyn. Klucze są bezwzględne (razem z folderem głównym zadania).
pub struct Mem {
    pub id: String,
    pub name: String,
}

impl Mem {
    pub fn new(tag: &str) -> Mem {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let m = Mem {
            id: format!("{tag}-{}-{n}", std::process::id()),
            name: format!("{tag}{n}"),
        };
        MemBackend::reset(&m.id);
        accounts::register_ephemeral(&m.name, "mem", &[("id", m.id.as_str())]);
        m
    }
    /// Zapis `konto:folder` do zadania.
    pub fn spec(&self, root: &str) -> String {
        format!("{}:{root}", self.name)
    }
    pub fn put(&self, key: &str, content: &str, mtime: i64) {
        MemBackend::put_raw(&self.id, key, content.as_bytes(), mtime);
    }
    pub fn get(&self, key: &str) -> Option<String> {
        MemBackend::get_raw(&self.id, key).map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    pub fn has(&self, key: &str) -> bool {
        MemBackend::get_raw(&self.id, key).is_some()
    }
    pub fn keys(&self) -> Vec<String> {
        MemBackend::keys(&self.id)
    }
    /// Klucze bez kosza synchronizacji.
    pub fn live_keys(&self) -> Vec<String> {
        self.keys()
            .into_iter()
            .filter(|k| !k.contains(".syncron-trash"))
            .collect()
    }
    pub fn faults(&self, f: Faults) {
        MemBackend::set_faults(&self.id, f);
    }
    pub fn clear_faults(&self) {
        MemBackend::clear_faults(&self.id);
    }
    pub fn hashes(&self, on: bool) {
        MemBackend::set_hashes(&self.id, on);
    }
    pub fn wipe(&self) {
        MemBackend::reset(&self.id);
    }
}

impl Drop for Mem {
    fn drop(&mut self) {
        accounts::unregister_ephemeral(&self.name);
        MemBackend::reset(&self.id);
    }
}

pub fn job_between(left: &str, right: &str, extra: serde_json::Value) -> Job {
    let mut v = serde_json::json!({
        "id": "v08", "name": "v08", "left": left, "right": right,
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}, "threads": 2
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    serde_json::from_value(v).unwrap()
}

pub fn set_mtime(p: &Path, secs: i64) {
    filetime::set_file_mtime(p, filetime::FileTime::from_unix_time(secs, 0)).unwrap();
}

pub fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}
