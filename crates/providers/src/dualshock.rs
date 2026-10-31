//! Sony DualShock 4 (USB, Bluetooth or the Sony wireless dongle).
//!
//! Battery sits in the input report's `status[0]`, per Linux drivers/hid/hid-playstation.c
//! (`struct dualshock4_input_report_common`, `dualshock4_parse_report`): byte 30 of the USB
//! report 0x01, byte 32 of the Bluetooth report 0x11. Low nibble = capacity, bit 4 = cable.
//! Over Bluetooth the controller sends a reduced 0x01 report until the host reads the BT
//! calibration feature report 0x05; we do that read only if no full report arrives (Steam
//! usually has already switched it). Nothing is ever written to the controller.
//!
//! DS4Windows with HidHide hides the controller from other apps; it then won't show up here.

use std::time::{Duration, Instant};

use anyhow::Result;
use battery_core::{Battery, Kind, KindSource, Level, Provider};
use hidapi::{HidApi, HidDevice};

use crate::debug;

const VID: u16 = 0x054C;
/// DS4 v1, DS4 v2, Sony wireless adapter.
const PIDS: &[u16] = &[0x05C4, 0x09CC, 0x0BA0];
const DONGLE_PID: u16 = 0x0BA0;
const BT_HID_SERVICE: &str = "00001124-0000-1000-8000-00805f9b34fb";
const FEATURE_CALIBRATION_BT: u8 = 0x05;
const FEATURE_CALIBRATION_BT_SIZE: usize = 41;
const LISTEN: Duration = Duration::from_millis(500);

pub struct DualShockProvider;

impl Provider for DualShockProvider {
    fn name(&self) -> &'static str {
        "dualshock"
    }
    fn priority(&self) -> u8 {
        1
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let api = HidApi::new()?;
        let mut out = Vec::new();
        for (n, info) in
            api.device_list().filter(|d| d.vendor_id() == VID && PIDS.contains(&d.product_id()) && d.usage_page() == 0x01 && d.usage() == 0x05).enumerate()
        {
            let bluetooth = info.path().to_string_lossy().to_ascii_lowercase().contains(BT_HID_SERVICE);
            let dev = api.open_path(info.path())?;
            let mut status = read_status(&dev, bluetooth)?;
            if status.is_none() && bluetooth {
                debug!("dualshock: only reduced reports, reading BT calibration to enable full reports");
                let mut buf = [0u8; FEATURE_CALIBRATION_BT_SIZE];
                buf[0] = FEATURE_CALIBRATION_BT;
                dev.get_feature_report(&mut buf)?;
                status = read_status(&dev, bluetooth)?;
            }
            let (level, charging, connected) = match status {
                Some(s) if info.product_id() == DONGLE_PID && s[1] & 0x04 != 0 => (Level::Unavailable("controller off".into()), None, Some(false)),
                Some(s) => {
                    let (pct, charging) = parse_status0(s[0]);
                    (pct.map_or(Level::Unavailable("charge error".into()), Level::Percent), charging, Some(true))
                }
                None => (Level::Unavailable("no input report".into()), None, None),
            };
            out.push(Battery {
                // Over Bluetooth the HID serial is the controller's MAC; without one, fall back to order.
                device_id: match info.serial_number().filter(|s| !s.is_empty()) {
                    Some(_) => crate::stable_id("ds4", info.product_id(), info.serial_number()),
                    None => format!("ds4:{:04x}:{n}", info.product_id()),
                },
                name: "DualShock 4".into(),
                part: None,
                kind: Kind::Gamepad,
                kind_source: KindSource::Provider,
                level,
                charging,
                connected,
                source: "dualshock-hid",
            });
        }
        Ok(out)
    }
}

/// `[status0, status1]` from the first full input report, if one arrives.
fn read_status(dev: &HidDevice, bluetooth: bool) -> Result<Option<[u8; 2]>> {
    let deadline = Instant::now() + LISTEN;
    while Instant::now() < deadline {
        let mut buf = [0u8; 128];
        let n = dev.read_timeout(&mut buf, 50)?;
        let offset = match (buf[0], bluetooth) {
            (0x11, _) => 32,
            (0x01, false) => 30,
            _ => continue,
        };
        if n > offset + 1 {
            return Ok(Some([buf[offset], buf[offset + 1]]));
        }
    }
    Ok(None)
}

/// Percent (mid-point of each 10% step, as Linux and the PS4 do) and charging state.
/// `None` percent for the error codes 14/15 on cable.
fn parse_status0(s: u8) -> (Option<u8>, Option<bool>) {
    let cap = s & 0x0F;
    if s & 0x10 == 0 {
        return (Some(if cap < 10 { cap * 10 + 5 } else { 100 }), Some(false));
    }
    match cap {
        0..=9 => (Some(cap * 10 + 5), Some(true)),
        10 => (Some(100), Some(true)),
        11 => (Some(100), Some(false)),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_byte() {
        // Real Bluetooth report from this machine: status0 = 0x02, no cable.
        assert_eq!(parse_status0(0x02), (Some(25), Some(false)));
        assert_eq!(parse_status0(0x0A), (Some(100), Some(false)));
        assert_eq!(parse_status0(0x13), (Some(35), Some(true)));
        assert_eq!(parse_status0(0x1B), (Some(100), Some(false)));
        assert_eq!(parse_status0(0x1F), (None, None));
    }
}
