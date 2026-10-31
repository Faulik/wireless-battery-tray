//! "Start with Windows": a value under HKCU\...\Run. Off unless the user turns it on.

use anyhow::{bail, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const VALUE: PCWSTR = w!("BatteriesBar");

fn open() -> Result<HKEY> {
    let mut key = HKEY::default();
    // SAFETY: valid out-pointer; key closed by callers.
    let err = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, 0, KEY_READ | KEY_WRITE, &mut key) };
    if err != ERROR_SUCCESS {
        bail!("open Run key: {err:?}");
    }
    Ok(key)
}

pub fn is_enabled() -> bool {
    let Ok(key) = open() else { return false };
    // SAFETY: existence query only.
    unsafe {
        let found = RegQueryValueExW(key, VALUE, None, None, None, None) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        found
    }
}

pub fn set(enabled: bool) -> Result<()> {
    let key = open()?;
    // SAFETY: data buffer outlives the call.
    let err = unsafe {
        let err = if enabled {
            let exe = std::env::current_exe()?;
            let cmd: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
            let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
            RegSetValueExW(key, VALUE, 0, REG_SZ, Some(bytes))
        } else {
            match RegDeleteValueW(key, VALUE) {
                e if e == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                e => e,
            }
        };
        let _ = RegCloseKey(key);
        err
    };
    if err != ERROR_SUCCESS {
        bail!("update Run key: {err:?}");
    }
    Ok(())
}
