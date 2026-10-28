//! Zgodność wsteczna po zmianie nazwy z SyncPolak na Syncron: przeniesienie danych ze starego folderu.
use std::path::Path;

const OLD_ID: &str = "pl.syncpolak.app";

/// Jeśli nowy folder danych nie ma jeszcze zadań, a stary (`pl.syncpolak.app` obok) je ma, kopiuje konfigurację,
/// stan synchronizacji i konta rclone. Nic nie nadpisuje ani nie usuwa w starym folderze.
pub fn migrate_data_dir(new_dir: &Path) -> bool {
    if new_dir.join("jobs.json").exists() {
        return false;
    }
    let Some(old) = new_dir.parent().map(|p| p.join(OLD_ID)) else {
        return false;
    };
    if !old.join("jobs.json").exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(new_dir);
    let mut any = false;
    for f in [
        "jobs.json",
        "settings.json",
        "rclone.conf",
        "state.db",
        "state.db-wal",
        "state.db-shm",
    ] {
        if old.join(f).exists() && std::fs::copy(old.join(f), new_dir.join(f)).is_ok() {
            any = true;
        }
    }
    if any {
        log::info!("przeniesiono dane ze starego folderu {}", old.display());
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_once() {
        let t = tempfile::tempdir().unwrap();
        let (old, new) = (t.path().join(OLD_ID), t.path().join("pl.syncron.app"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("jobs.json"), "[]").unwrap();
        std::fs::write(old.join("rclone.conf"), "x").unwrap();
        assert!(migrate_data_dir(&new));
        assert_eq!(
            std::fs::read_to_string(new.join("rclone.conf")).unwrap(),
            "x"
        );
        assert!(
            old.join("jobs.json").exists(),
            "stary folder zostaje nietknięty"
        );
        assert!(!migrate_data_dir(&new), "drugi raz nic nie robi");
    }
}
