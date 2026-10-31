//! Battery providers. Each one only sends read/query requests to devices.

use battery_core::Provider;

/// `eprintln!` when `BATTERIES_DEBUG` is set.
macro_rules! debug {
    ($($arg:tt)*) => {
        if std::env::var_os("BATTERIES_DEBUG").is_some() {
            eprintln!($($arg)*);
        }
    };
}
pub(crate) use debug;

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

/// Normalise a 48-bit Bluetooth address to the `bt:xxxxxxxxxxxx` device id used across providers.
pub(crate) fn bt_device_id(addr: u64) -> String {
    format!("bt:{addr:012x}")
}
