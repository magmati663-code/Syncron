//! Windows VSS (Volume Shadow Copy): odczyt plików zablokowanych przez inne programy z migawki woluminu.
//! Tworzenie migawki wymaga uprawnień administratora. Na innych systemach moduł jest nieaktywny.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct Shadow {
    pub id: String,
    /// np. `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy5`
    pub device: String,
}

pub fn supported() -> bool {
    cfg!(windows)
}

/// `\\?\C:\Users\x\f.txt` lub `C:/Users/x/f.txt` -> (`C:\`, `Users\x\f.txt`). Ścieżki UNC nie mają migawek.
pub fn split_volume(p: &str) -> Option<(String, String)> {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let b = p.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    {
        Some((
            format!("{}:\\", p[..1].to_uppercase()),
            p[3..].replace('/', "\\"),
        ))
    } else {
        None
    }
}

fn parse_shadow(out: &str) -> Option<Shadow> {
    let line = out.lines().rev().find(|l| l.contains('|'))?;
    let (id, device) = line.trim().split_once('|')?;
    let id = id.trim();
    (is_guid(id) && device.contains("GLOBALROOT")).then(|| Shadow {
        id: id.into(),
        device: device.trim().into(),
    })
}

/// Identyfikator migawki w formacie `{8-4-4-4-12}` (cyfry szesnastkowe) — jedyny dozwolony kształt `id`.
pub fn is_guid(id: &str) -> bool {
    let Some(inner) = id.strip_prefix('{').and_then(|x| x.strip_suffix('}')) else {
        return false;
    };
    let parts: Vec<&str> = inner.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

// ---------- prywatny katalog danych (zamiast przewidywalnych nazw w %TEMP%) ----------

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Ustawia prywatny katalog danych aplikacji (rejestr migawek i kopie tymczasowe). Wywołać raz przy starcie.
pub fn set_data_dir(dir: &Path) {
    let _ = DATA_DIR.set(dir.to_path_buf());
}

/// Tworzy NOWY katalog roboczy o nieprzewidywalnej nazwie (`create_dir` zawodzi, gdy nazwa już istnieje,
/// więc nie da się podstawić cudzego katalogu). Leży w katalogu danych aplikacji, a bez niego w katalogu
/// tymczasowym; na Unixie z uprawnieniami 0700.
pub fn scratch_dir() -> std::io::Result<PathBuf> {
    use std::hash::{BuildHasher, Hasher};
    let base = match DATA_DIR.get() {
        Some(d) => d.join("tmp"),
        None => std::env::temp_dir(),
    };
    std::fs::create_dir_all(&base)?;
    for _ in 0..20 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let p = base.join(format!("syncron-vss-{:016x}", h.finish()));
        match std::fs::create_dir(&p) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700));
                }
                return Ok(p);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other(
        "nie udało się utworzyć katalogu roboczego",
    ))
}

#[cfg(windows)]
pub fn is_admin() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    unsafe { IsUserAnAdmin() != 0 }
}
#[cfg(not(windows))]
pub fn is_admin() -> bool {
    false
}

#[cfg(windows)]
fn ps(script: &str, env: &[(&str, &str)]) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    // wartości zmiennych (ścieżki, identyfikatory) trafiają do skryptu przez środowisko, nie przez sklejanie tekstu
    let out = std::process::Command::new("powershell")
        .envs(env.iter().copied())
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("PowerShell: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
#[cfg(not(windows))]
fn ps(_script: &str, _env: &[(&str, &str)]) -> Result<String, String> {
    Err("VSS jest dostępny tylko w systemie Windows".into())
}

static REGISTRY: Mutex<()> = Mutex::new(());

/// Rejestr w prywatnym katalogu danych aplikacji; bez niego (np. testy) migawki nie są zapisywane.
fn registry_path() -> Option<PathBuf> {
    DATA_DIR.get().map(|d| d.join("vss-active.txt"))
}

fn read_registry(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| is_guid(l)) // wpisy w złym formacie są ignorowane
        .collect()
}

/// Zapisuje identyfikatory aktywnych migawek, żeby po awarii aplikacji dało się je usunąć (`cleanup_stale`).
fn registry_update(id: &str, add: bool) {
    let _g = REGISTRY.lock().unwrap();
    let Some(p) = registry_path() else { return };
    let mut ids = read_registry(&p);
    ids.retain(|x| x != id);
    if add {
        ids.push(id.to_string());
    }
    if ids.is_empty() {
        let _ = std::fs::remove_file(&p);
    } else {
        let _ = std::fs::write(&p, ids.join("\n"));
    }
}

fn delete_shadow(id: &str) {
    if !is_guid(id) {
        return; // nigdy nie przekazuj nieprawidłowego id do PowerShell
    }
    let script = "Get-CimInstance Win32_ShadowCopy | Where-Object { $_.ID -eq $env:SYNCRON_SHADOW_ID } | Remove-CimInstance";
    let _ = ps(script, &[("SYNCRON_SHADOW_ID", id)]);
    registry_update(id, false);
}

/// Usuwa migawki pozostawione po nagłym zamknięciu aplikacji (wywołać przy starcie).
pub fn cleanup_stale() {
    if !supported() || !is_admin() {
        return;
    }
    if let Some(p) = registry_path() {
        for id in read_registry(&p) {
            delete_shadow(&id);
        }
    }
    // pozostałości kopii z migawek po awarii
    if let Some(d) = DATA_DIR.get() {
        let _ = std::fs::remove_dir_all(d.join("tmp"));
    }
}

fn create(volume: &str) -> Result<Shadow, String> {
    if !supported() {
        return Err("VSS jest dostępny tylko w systemie Windows".into());
    }
    if !is_admin() {
        return Err("VSS wymaga uprawnień administratora — uruchom Syncron jako administrator (Ustawienia → VSS)".into());
    }
    let script = "$r = Invoke-CimMethod -ClassName Win32_ShadowCopy -MethodName Create -Arguments @{Volume=$env:SYNCRON_VOLUME; Context='ClientAccessible'}; \
         if ($r.ReturnValue -ne 0) { throw \"VSS: kod $($r.ReturnValue)\" }; \
         $s = Get-CimInstance Win32_ShadowCopy | Where-Object { $_.ID -eq $r.ShadowID }; \
         \"$($s.ID)|$($s.DeviceObject)\"";
    let out = ps(script, &[("SYNCRON_VOLUME", volume)])
        .map_err(|e| format!("nie udało się utworzyć migawki: {e}"))?;
    let sh =
        parse_shadow(&out).ok_or_else(|| format!("nieoczekiwana odpowiedź VSS: {}", out.trim()))?;
    registry_update(&sh.id, true);
    Ok(sh)
}

/// Migawki tworzone leniwie (przy pierwszym zablokowanym pliku na danym woluminie) i usuwane po przebiegu.
pub struct Vss {
    enabled: bool,
    shadows: Mutex<HashMap<String, Result<Shadow, String>>>,
    note: Mutex<Option<String>>,
}

impl Vss {
    pub fn new(enabled: bool) -> Self {
        Vss {
            enabled: enabled && supported(),
            shadows: Mutex::new(HashMap::new()),
            note: Mutex::new(None),
        }
    }

    pub fn disabled() -> Self {
        Self::new(false)
    }

    /// Ścieżka tego samego pliku wewnątrz migawki woluminu.
    pub fn snapshot_path(&self, src: &Path) -> Result<PathBuf, String> {
        if !self.enabled {
            return Err("VSS wyłączony".into());
        }
        let s = src.to_string_lossy();
        let (vol, rest) =
            split_volume(&s).ok_or("VSS działa tylko dla dysków lokalnych (z literą dysku)")?;
        let mut map = self.shadows.lock().unwrap();
        match map.entry(vol.clone()).or_insert_with(|| create(&vol)) {
            Ok(sh) => Ok(PathBuf::from(format!(
                "{}\\{}",
                sh.device.trim_end_matches('\\'),
                rest
            ))),
            Err(e) => {
                *self.note.lock().unwrap() = Some(e.clone());
                Err(e.clone())
            }
        }
    }

    /// Powód, dla którego VSS nie zadziałał (do pokazania użytkownikowi).
    pub fn note(&self) -> Option<String> {
        self.note.lock().unwrap().clone()
    }
}

impl Drop for Vss {
    fn drop(&mut self) {
        for sh in self.shadows.lock().unwrap().values().flatten() {
            delete_shadow(&sh.id);
        }
    }
}

// ---------- uprawnienia administratora (Windows) ----------

/// Uruchamia ponownie aplikację z podniesionymi uprawnieniami (monit UAC); bieżący proces powinien się zakończyć.
pub fn restart_elevated(exe: &Path) -> Result<(), String> {
    // ścieżka idzie osobno, w zmiennej środowiskowej (dziedziczonej przez proces potomny) — bez cytowania w skrypcie
    let e = exe.to_string_lossy();
    ps(
        "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 2; Start-Process -FilePath $env:SYNCRON_EXE -Verb RunAs'",
        &[("SYNCRON_EXE", e.as_ref())],
    )
    .map(|_| ())
}

const TASK: &str = "Syncron";
const LEGACY_TASK: &str = "SyncPolak";

/// Autostart przy logowaniu z uprawnieniami administratora (zadanie harmonogramu; wymaga bieżących uprawnień admina).
pub fn set_elevated_autostart(exe: &Path, enable: bool) -> Result<(), String> {
    if !supported() {
        return Err("tylko Windows".into());
    }
    if !is_admin() {
        return Err("wymaga uruchomienia jako administrator".into());
    }
    let exe_s = exe.to_string_lossy();
    let script = if enable {
        format!(
            "$a = New-ScheduledTaskAction -Execute $env:SYNCRON_EXE -Argument '--hidden'; \
             $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
             $p = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest -LogonType Interactive; \
             $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero); \
             Register-ScheduledTask -TaskName '{TASK}' -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null"
        )
    } else {
        format!("foreach ($n in '{TASK}','{LEGACY_TASK}') {{ Unregister-ScheduledTask -TaskName $n -Confirm:$false -ErrorAction SilentlyContinue }}")
    };
    ps(&script, &[("SYNCRON_EXE", exe_s.as_ref())]).map(|_| ())
}

pub fn elevated_autostart_enabled() -> bool {
    supported()
        && ps(
            &format!("Get-ScheduledTask -TaskName '{TASK}' -ErrorAction Stop | Out-Null"),
            &[],
        )
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_split() {
        assert_eq!(
            split_volume(r"\\?\c:\Users\Żółć\f.docx"),
            Some(("C:\\".into(), r"Users\Żółć\f.docx".into()))
        );
        assert_eq!(
            split_volume("D:/a/b.txt"),
            Some(("D:\\".into(), r"a\b.txt".into()))
        );
        assert_eq!(split_volume(r"\\?\UNC\nas\share\f"), None);
        assert_eq!(split_volume("/home/x"), None);
    }

    #[test]
    fn parse_ps_output() {
        let o = "ostrzeżenie\n{0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4f10}|\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy7\n";
        let s = parse_shadow(o).unwrap();
        assert_eq!(s.id, "{0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4f10}");
        assert!(s.device.ends_with("ShadowCopy7"));
        assert!(parse_shadow("brak").is_none());
        // wstrzyknięcie w polu id jest odrzucane
        assert!(parse_shadow("x' }; calc; { '|\\\\?\\GLOBALROOT\\Device\\X").is_none());
    }

    #[test]
    fn guid_validation() {
        assert!(is_guid("{0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4f10}"));
        assert!(!is_guid("0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4f10"));
        assert!(!is_guid("{0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4f1}"));
        assert!(!is_guid("{0f3a9c1e-77b2-4d0a-9c11-5e6d8a2b4fzz}"));
        assert!(!is_guid("{x' }; Start-Process calc; { '}"));
    }

    #[test]
    fn scratch_dirs_are_unique_and_fresh() {
        let a = scratch_dir().unwrap();
        let b = scratch_dir().unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        let _ = std::fs::remove_dir(a);
        let _ = std::fs::remove_dir(b);
    }

    #[test]
    fn disabled_and_non_windows_fail_gracefully() {
        let v = Vss::new(true);
        if !supported() {
            assert!(v.snapshot_path(Path::new("C:\\a.txt")).is_err());
        }
        assert!(Vss::disabled()
            .snapshot_path(Path::new("C:\\a.txt"))
            .is_err());
    }
}
