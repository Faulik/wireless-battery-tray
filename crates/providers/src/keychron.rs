//! Keychron Link 2.4 GHz receiver (3434:D030) and what is paired to it.
//!
//! Protocol taken from how Keychron Launcher (launcher.keychron.com, WebHID) talks to the Link
//! over its raw-HID interface (usage page 0xFF60, usage 0x61, 32-byte reports, report id 0):
//! - `B2` (FR_GET_STATE) answers `B2 ?? [vid lo, vid hi, pid lo, pid hi, on] x 3` for the
//!   receiver's three slots (keyboard 1, keyboard 2, mouse).
//! - `A7 31` (KC_USER_CMD_NAPE_GET_BAT_REPORT) is forwarded to the paired keyboard, which
//!   answers `A7 31 level status`. Older Keychron firmware doesn't implement it and stays silent.
//!
//! Keychron's QMK firmware (github.com/Keychron/qmk_firmware, keyboards/keychron/common) has
//! no raw-HID battery query: the keyboard pushes its level to the wireless module, which sends
//! it over BLE's Battery Service in Bluetooth mode (picked up by the `gatt` provider). A K3 HE
//! on this Link (firmware d.3.1) answered none of A0-A2, A7 31 or VIA 01 over 2.4 GHz, and the
//! Link's vendor feature reports 0x51-0x53 stall, so its battery is not readable this way yet.
//!
//! Keychron 8K mouse receivers (3434:D028) use a different report (`B3 06`, level at byte 20,
//! >100 = charging) per github.com/MrAdrianPl/keychron_battery_widget; not handled here yet.

use std::time::{Duration, Instant};

use anyhow::Result;
use battery_core::{Battery, Kind, KindSource, Level, Provider};
use hidapi::{HidApi, HidDevice};

const VID: u16 = 0x3434;
const LINK_PIDS: &[u16] = &[0xD030];
const FR_GET_STATE: u8 = 0xB2;
const KC_USER_CMD: u8 = 0xA7;
const NAPE_GET_BAT_REPORT: u8 = 0x31;
const TIMEOUT: Duration = Duration::from_millis(1000);

pub struct KeychronProvider;

impl Provider for KeychronProvider {
    fn name(&self) -> &'static str {
        "keychron"
    }
    fn priority(&self) -> u8 {
        1
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let api = HidApi::new()?;
        let mut out = Vec::new();
        for info in api.device_list() {
            if info.vendor_id() != VID || !LINK_PIDS.contains(&info.product_id()) || info.usage_page() != 0xFF60 || info.usage() != 0x61 {
                continue;
            }
            let dev = api.open_path(info.path())?;
            let Some(state) = request(&dev, &[FR_GET_STATE], |r| r[0] == FR_GET_STATE)? else {
                continue;
            };
            for (slot, sub) in parse_state(&state).into_iter().enumerate() {
                let Some((pid, on)) = sub else { continue };
                let (level, connected) = if !on {
                    (Level::Unavailable("off".into()), Some(false))
                } else {
                    match request(&dev, &[KC_USER_CMD, NAPE_GET_BAT_REPORT], |r| r[0] == KC_USER_CMD && r[1] == NAPE_GET_BAT_REPORT)? {
                        Some(r) => {
                            debug!("keychron: {pid:04x} battery report {:02x?}", &r[..4]);
                            (Level::Percent(r[2].min(100)), Some(true))
                        }
                        None => (Level::Unavailable("connected, battery not reported".into()), Some(true)),
                    }
                };
                out.push(Battery {
                    device_id: format!("{}:{slot}", crate::stable_id("keychron", pid, info.serial_number())),
                    name: model_name(pid).map_or_else(|| format!("Keychron {VID:04X}:{pid:04X}"), String::from),
                    part: None,
                    kind: if slot == 2 { Kind::Mouse } else { Kind::Keyboard },
                    kind_source: KindSource::Provider,
                    level,
                    charging: None,
                    connected,
                    source: "keychron-link",
                });
            }
        }
        Ok(out)
    }
}

/// Models confirmed on real hardware; others are shown by VID:PID.
fn model_name(pid: u16) -> Option<&'static str> {
    match pid {
        0x0E30 => Some("Keychron K3 HE"),
        _ => None,
    }
}

/// Three receiver slots: `Some((pid, on))` where a device is paired.
fn parse_state(r: &[u8; 32]) -> [Option<(u16, bool)>; 3] {
    std::array::from_fn(|i| {
        let b = &r[2 + i * 5..7 + i * 5];
        let vid = u16::from_le_bytes([b[0], b[1]]);
        let pid = u16::from_le_bytes([b[2], b[3]]);
        (vid != 0 || pid != 0).then_some((pid, b[4] != 0))
    })
}

fn request(dev: &HidDevice, cmd: &[u8], is_reply: impl Fn(&[u8; 32]) -> bool) -> Result<Option<[u8; 32]>> {
    let mut drain = [0u8; 32];
    while dev.read_timeout(&mut drain, 0)? > 0 {}
    let mut msg = [0u8; 33];
    msg[1..1 + cmd.len()].copy_from_slice(cmd);
    dev.write(&msg)?;
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let mut buf = [0u8; 32];
        if dev.read_timeout(&mut buf, 50)? > 0 && is_reply(&buf) {
            return Ok(Some(buf));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_receiver_state() {
        // Real reply from this machine's Link: one device 3434:0E30, on.
        let mut r = [0u8; 32];
        r[..7].copy_from_slice(&[0xb2, 0x01, 0x34, 0x34, 0x30, 0x0e, 0x01]);
        assert_eq!(parse_state(&r), [Some((0x0E30, true)), None, None]);
    }
}
