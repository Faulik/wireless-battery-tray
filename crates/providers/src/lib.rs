//! Battery providers. Each one only sends read/query requests to devices.

use std::sync::Arc;

use battery_core::Provider;

/// `eprintln!` when `BATTERIES_DEBUG` is set. Defined before the `mod`s, so it is in scope in all of them.
macro_rules! debug {
    ($($arg:tt)*) => {
        if std::env::var_os("BATTERIES_DEBUG").is_some() {
            eprintln!($($arg)*);
        }
    };
}

pub mod dualshock;
#[cfg(windows)]
pub mod gatt;
pub mod keychron;
pub mod logitech;
#[cfg(windows)]
pub mod pnp;
pub mod steelseries;

/// All providers available on this platform.
pub fn all() -> Vec<Arc<dyn Provider>> {
    let mut v: Vec<Arc<dyn Provider>> = Vec::new();
    #[cfg(windows)]
    {
        v.push(Arc::new(pnp::PnpProvider));
        v.push(Arc::new(gatt::GattProvider));
    }
    v.push(Arc::new(logitech::LogitechProvider));
    v.push(Arc::new(steelseries::SteelSeriesProvider));
    v.push(Arc::new(keychron::KeychronProvider));
    v.push(Arc::new(dualshock::DualShockProvider));
    v
}

/// `prefix:pid:serial` when the device has a USB/HID serial number, else `prefix:pid`.
pub(crate) fn stable_id(prefix: &str, pid: u16, serial: Option<&str>) -> String {
    match serial.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => format!("{prefix}:{pid:04x}:{}", s.to_ascii_lowercase()),
        None => format!("{prefix}:{pid:04x}"),
    }
}

/// Normalise a 48-bit Bluetooth address to the `bt:xxxxxxxxxxxx` device id used across providers.
pub(crate) fn bt_device_id(addr: u64) -> String {
    format!("bt:{addr:012x}")
}
