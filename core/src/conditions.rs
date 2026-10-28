//! Warunki uruchamiania: zasilanie sieciowe i nazwa sieci Wi-Fi (wykrywanie najlepszym wysiłkiem).
use crate::model::Conditions;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let o = c.output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// `Some(true)` = zasilanie sieciowe, `Some(false)` = bateria, `None` = nie wiadomo.
pub fn on_ac_power() -> Option<bool> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Sps {
            ac: u8,
            flag: u8,
            pct: u8,
            sys: u8,
            life: u32,
            full: u32,
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GetSystemPowerStatus(s: *mut Sps) -> i32;
        }
        let mut s = Sps {
            ac: 255,
            flag: 0,
            pct: 0,
            sys: 0,
            life: 0,
            full: 0,
        };
        if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
            return None;
        }
        match s.ac {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let o = run("pmset", &["-g", "batt"])?;
        if o.contains("AC Power") {
            Some(true)
        } else if o.contains("Battery Power") {
            Some(false)
        } else {
            None
        }
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

pub fn parse_netsh_ssid(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let t = l.trim();
        let rest = t.strip_prefix("SSID")?;
        let v = rest.trim_start().strip_prefix(':')?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Nazwa aktualnej sieci Wi-Fi (None: brak Wi-Fi albo nie da się ustalić).
pub fn wifi_ssid() -> Option<String> {
    #[cfg(windows)]
    {
        parse_netsh_ssid(&run("netsh", &["wlan", "show", "interfaces"])?)
    }
    #[cfg(target_os = "macos")]
    {
        let o = run("networksetup", &["-getairportnetwork", "en0"])?;
        o.split_once("Network:")
            .map(|(_, n)| n.trim().to_string())
            .filter(|n| !n.is_empty())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        run("iwgetid", &["-r"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

type Cache = Option<(Instant, Option<bool>, Option<String>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

fn probe() -> (Option<bool>, Option<String>) {
    let mut c = CACHE.lock().unwrap();
    if let Some((t, ac, ssid)) = c.as_ref() {
        if t.elapsed() < Duration::from_secs(15) {
            return (*ac, ssid.clone());
        }
    }
    let (ac, ssid) = (on_ac_power(), wifi_ssid());
    *c = Some((Instant::now(), ac, ssid.clone()));
    (ac, ssid)
}

/// Ok, gdy warunki są spełnione; w przeciwnym razie powód (pokazywany użytkownikowi).
pub fn check(c: &Conditions) -> Result<(), String> {
    if !c.ac_only && c.wifi.is_empty() {
        return Ok(());
    }
    let (ac, ssid) = probe();
    if c.ac_only && ac == Some(false) {
        return Err("komputer działa na baterii (zadanie wymaga zasilania sieciowego)".into());
    }
    let allowed: Vec<&str> = c
        .wifi
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if !allowed.is_empty() {
        if let Some(s) = ssid {
            if !allowed.iter().any(|a| a.eq_ignore_ascii_case(&s)) {
                return Err(format!(
                    "połączono z siecią Wi-Fi „{s}”, która nie jest na liście dozwolonych"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netsh_parse() {
        let o = "    Name                   : Wi-Fi\n    SSID                   : Dom Żółć\n    BSSID                  : aa:bb\n";
        assert_eq!(parse_netsh_ssid(o), Some("Dom Żółć".into()));
        assert_eq!(parse_netsh_ssid("nic"), None);
    }

    #[test]
    fn empty_conditions_always_ok() {
        assert!(check(&Conditions::default()).is_ok());
    }
}
