//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` autostart entry.
//! The registry is the source of truth (5.4: "reflect actual registry state
//! in tray checkmark"), not a mirrored boolean in `config.toml`.

use std::os::windows::ffi::OsStrExt;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

const VALUE_NAME: PCWSTR = w!("Glide");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

/// `"C:\path with spaces\glide.exe"` as NUL-terminated UTF-16. Quoted so a
/// path containing spaces is not split by the shell at logon.
fn command_line_wide() -> Option<Vec<u16>> {
    let exe = std::env::current_exe().ok()?;
    let mut wide: Vec<u16> = vec![u16::from(b'"')];
    wide.extend(exe.as_os_str().encode_wide());
    wide.push(u16::from(b'"'));
    wide.push(0);
    Some(wide)
}

pub fn is_enabled() -> bool {
    // SAFETY: read-only registry query on a fixed, well-known key/value.
    unsafe {
        let mut hkey = HKEY::default();
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
    // just opened with the matching access right; the byte view of `path`
    // covers exactly its initialised UTF-16 contents and lives past the call.
    unsafe {
        let mut hkey = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, 0, KEY_SET_VALUE, &mut hkey).ok()?;
        let result = if on {
            let Some(path) = command_line_wide() else {
                let _ = RegCloseKey(hkey);
                return Ok(());
            };
            // `--disable` isn't passed: autostart should launch enabled.
            let bytes = std::slice::from_raw_parts(path.as_ptr().cast::<u8>(), path.len() * 2);
            RegSetValueExW(hkey, VALUE_NAME, 0, REG_SZ, Some(bytes))
        } else {
            let r = RegDeleteValueW(hkey, VALUE_NAME);
            // Removing an entry that isn't there is already the desired state.
            if r == ERROR_FILE_NOT_FOUND {
                let _ = RegCloseKey(hkey);
                return Ok(());
            }
            r
        };
        let _ = RegCloseKey(hkey);
        result.ok()
    }
}
