//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` autostart entry.
//! The registry is the source of truth (5.4: "reflect actual registry state
//! in tray checkmark"), not a mirrored boolean in `config.toml`.

use windows::core::w;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

const VALUE_NAME: windows::core::PCWSTR = w!("Glide");
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn exe_path_wide() -> Option<Vec<u16>> {
    let exe = std::env::current_exe().ok()?;
    let mut wide: Vec<u16> = exe.as_os_str().encode_wide_lossless();
    wide.push(0);
    Some(wide)
}

// Small local shim: `OsStr::encode_wide` requires the `std::os::windows`
// extension trait; named here so the call site above reads naturally.
trait EncodeWideLossless {
    fn encode_wide_lossless(&self) -> Vec<u16>;
}
impl EncodeWideLossless for std::ffi::OsStr {
    fn encode_wide_lossless(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().collect()
    }
}

pub fn is_enabled() -> bool {
    // SAFETY: read-only registry query on a fixed, well-known key/value.
    unsafe {
        let mut hkey = Default::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_QUERY_VALUE, &mut hkey).is_err() {
            return false;
        }
        let ok = RegQueryValueExW(hkey, VALUE_NAME, None, None, None, None).is_ok();
        let _ = RegCloseKey(hkey);
        ok
    }
}

pub fn set_enabled(on: bool) -> windows::core::Result<()> {
    // SAFETY: writes/deletes a single named value under a fixed HKCU key we
    // just opened with the matching access right.
    unsafe {
        let mut hkey = Default::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut hkey)?;
        let result = if on {
            let Some(mut path) = exe_path_wide() else {
                let _ = RegCloseKey(hkey);
                return Ok(());
            };
            // `--disable` isn't passed: autostart should launch enabled.
            let bytes = std::slice::from_raw_parts(path.as_mut_ptr().cast::<u8>(), path.len() * 2);
            RegSetValueExW(hkey, VALUE_NAME, 0, REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(hkey, VALUE_NAME)
        };
        let _ = RegCloseKey(hkey);
        result
    }
}
