//! Zgodność wsteczna po zmianie nazwy z SyncPolak na Syncron: przeniesienie danych ze starego folderu.
use std::path::Path;

const OLD_ID: &str = "pl.syncpolak.app";

/// Jeśli nowy folder danych nie ma jeszcze zadań, a stary (`pl.syncpolak.app` obok) ma zadania albo konta
/// rclone, kopiuje konfigurację, stan synchronizacji i konta rclone. Kopiuje tylko pliki, których w nowym
/// folderze jeszcze nie ma (przez plik tymczasowy + zmianę nazwy). Nic nie usuwa w starym folderze.
/// Błędy kopiowania są logowane; zwraca `true`, gdy cokolwiek przeniesiono.
pub fn migrate_data_dir(new_dir: &Path) -> bool {
    if new_dir.join("jobs.json").exists() {
        return false;
    }
    let Some(old) = new_dir.parent().map(|p| p.join(OLD_ID)) else {
        return false;
    };
    if !old.join("jobs.json").exists() && !old.join("rclone.conf").exists() {
        return false;
    }
    if let Err(e) = std::fs::create_dir_all(new_dir) {
        log::error!("migracja: nie można utworzyć {}: {e}", new_dir.display());
        return false;
    }
    // baza stanu to komplet plików (state.db + -wal + -shm): nie mieszamy ze starymi
    let db = ["state.db", "state.db-wal", "state.db-shm"];
    let db_present = db.iter().any(|f| new_dir.join(f).exists());
    let mut any = false;
    for f in ["settings.json", "rclone.conf"]
        .into_iter()
        .chain(db.into_iter().filter(|_| !db_present))
        // jobs.json na końcu: jego obecność oznacza zakończoną migrację
        .chain(["jobs.json"])
    {
        let (src, dst) = (old.join(f), new_dir.join(f));
        if !src.exists() || dst.exists() {
            continue;
        }
        let tmp = new_dir.join(format!("{f}.migracja-tmp"));
        match std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dst)) {
            Ok(()) => any = true,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                log::error!("migracja: nie można skopiować {}: {e}", src.display());
            }
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
