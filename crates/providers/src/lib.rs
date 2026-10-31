//! Battery providers. Each one only sends read/query requests to devices.

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
pub fn all() -> Vec<Box<dyn Provider>> {
    let mut v: Vec<Box<dyn Provider>> = Vec::new();
    #[cfg(windows)]
    {
        v.push(Box::new(pnp::PnpProvider));
        v.push(Box::new(gatt::GattProvider));
    }
    v.push(Box::new(logitech::LogitechProvider));
    v.push(Box::new(steelseries::SteelSeriesProvider));
    v.push(Box::new(keychron::KeychronProvider));
    v.push(Box::new(dualshock::DualShockProvider));
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
