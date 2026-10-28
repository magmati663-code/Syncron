//! Windows VSS (Volume Shadow Copy): odczyt plików zablokowanych przez inne programy z migawki woluminu.
//! Tworzenie migawki wymaga uprawnień administratora. Na innych systemach moduł jest nieaktywny.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    (!id.is_empty() && device.contains("GLOBALROOT")).then(|| Shadow {
        id: id.into(),
        device: device.trim().into(),
    })
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
fn ps(script: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell")
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
fn ps(_script: &str) -> Result<String, String> {
    Err("VSS jest dostępny tylko w systemie Windows".into())
}

static REGISTRY: Mutex<()> = Mutex::new(());

fn registry_path() -> PathBuf {
    std::env::temp_dir().join("syncpolak-vss-active.txt")
}

/// Zapisuje identyfikatory aktywnych migawek, żeby po awarii aplikacji dało się je usunąć (`cleanup_stale`).
fn registry_update(id: &str, add: bool) {
    let _g = REGISTRY.lock().unwrap();
    let p = registry_path();
    let mut ids: Vec<String> = std::fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .filter(|l| !l.is_empty())
        .collect();
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
    let script = format!("Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.ID -eq '{id}' }} | Remove-CimInstance");
    let _ = ps(&script);
    registry_update(id, false);
}

/// Usuwa migawki pozostawione po nagłym zamknięciu aplikacji (wywołać przy starcie).
pub fn cleanup_stale() {
    if !supported() || !is_admin() {
        return;
    }
    let ids: Vec<String> = std::fs::read_to_string(registry_path())
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect();
    for id in ids.iter().filter(|i| !i.is_empty()) {
        delete_shadow(id);
    }
}

fn create(volume: &str) -> Result<Shadow, String> {
    if !supported() {
        return Err("VSS jest dostępny tylko w systemie Windows".into());
    }
    if !is_admin() {
        return Err("VSS wymaga uprawnień administratora — uruchom SyncPolak jako administrator (Ustawienia → VSS)".into());
    }
    let script = format!(
        "$r = Invoke-CimMethod -ClassName Win32_ShadowCopy -MethodName Create -Arguments @{{Volume='{volume}'; Context='ClientAccessible'}}; \
         if ($r.ReturnValue -ne 0) {{ throw \"VSS: kod $($r.ReturnValue)\" }}; \
         $s = Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.ID -eq $r.ShadowID }}; \
         \"$($s.ID)|$($s.DeviceObject)\""
    );
    let out = ps(&script).map_err(|e| format!("nie udało się utworzyć migawki: {e}"))?;
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
    let e = exe.to_string_lossy().replace('\'', "''");
    ps(&format!(
        "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 2; Start-Process -FilePath ''{e}'' -Verb RunAs'"
    ))
    .map(|_| ())
}

const TASK: &str = "SyncPolak";

/// Autostart przy logowaniu z uprawnieniami administratora (zadanie harmonogramu; wymaga bieżących uprawnień admina).
pub fn set_elevated_autostart(exe: &Path, enable: bool) -> Result<(), String> {
    if !supported() {
        return Err("tylko Windows".into());
    }
    if !is_admin() {
        return Err("wymaga uruchomienia jako administrator".into());
    }
    let script = if enable {
        let e = exe.to_string_lossy().replace('\'', "''");
        format!(
            "$a = New-ScheduledTaskAction -Execute '{e}' -Argument '--hidden'; \
             $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
             $p = New-ScheduledTaskPrincipal -UserId $env:USERNAME -RunLevel Highest -LogonType Interactive; \
             $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero); \
             Register-ScheduledTask -TaskName '{TASK}' -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null"
        )
    } else {
        format!("Unregister-ScheduledTask -TaskName '{TASK}' -Confirm:$false -ErrorAction SilentlyContinue")
    };
    ps(&script).map(|_| ())
}

pub fn elevated_autostart_enabled() -> bool {
    supported()
        && ps(&format!(
            "Get-ScheduledTask -TaskName '{TASK}' -ErrorAction Stop | Out-Null"
        ))
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
        let o = "ostrzeżenie\n{ABC-123}|\\\\?\\GLOBALROOT\\Device\\HarddiskVolumeShadowCopy7\n";
        let s = parse_shadow(o).unwrap();
        assert_eq!(s.id, "{ABC-123}");
        assert!(s.device.ends_with("ShadowCopy7"));
        assert!(parse_shadow("brak").is_none());
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
