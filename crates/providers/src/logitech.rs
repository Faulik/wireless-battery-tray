//! Logitech HID++ 2.0 through a Unifying / Bolt / Lightspeed receiver.
//!
//! Packet layouts follow Solaar (lib/logitech_receiver): short report 0x10 is 7 bytes, long
//! report 0x11 is 20 bytes: `[report, device index, feature index, fn << 4 | sw id, params...]`.
//! Feature index 0 is the root feature: fn 0 = getFeature(id), fn 1 = ping. HID++ 1.0 errors
//! come back as sub-id 0x8F, HID++ 2.0 errors as feature index 0xFF.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use battery_core::{Battery, Kind, Level, Provider};
use hidapi::{HidApi, HidDevice};

const VID: u16 = 0x046D;
/// Receivers to query. Wired/Bluetooth Logitech devices aren't covered here.
const RECEIVERS: &[u16] = &[0xC52B, 0xC532, 0xC534, 0xC539, 0xC53A, 0xC53F, 0xC547, 0xC548];

const SW_ID: u8 = 0x0A;
const TIMEOUT: Duration = Duration::from_millis(1000);

const FEAT_DEVICE_NAME: u16 = 0x0005;
const FEAT_BATTERY_STATUS: u16 = 0x1000;
const FEAT_BATTERY_VOLTAGE: u16 = 0x1001;
const FEAT_UNIFIED_BATTERY: u16 = 0x1004;

pub struct LogitechProvider;

impl Provider for LogitechProvider {
    fn name(&self) -> &'static str {
        "logitech"
    }
    fn priority(&self) -> u8 {
        1
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let api = HidApi::new()?;
        let mut out = Vec::new();
        for &pid in RECEIVERS {
            let path = |usage: u16| {
                api.device_list()
                    .find(|d| d.vendor_id() == VID && d.product_id() == pid && d.usage_page() == 0xFF00 && d.usage() == usage)
                    .map(|d| d.path().to_owned())
            };
            let (Some(short), Some(long)) = (path(1), path(2)) else { continue };
            let rx = Receiver { short: api.open_path(&short)?, long: api.open_path(&long)? };
            for index in 1..=6u8 {
                if let Some(b) = rx.poll_device(pid, index)? {
                    out.push(b);
                }
            }
        }
        Ok(out)
    }
}

struct Receiver {
    short: HidDevice,
    long: HidDevice,
}

enum Reply {
    Ok([u8; 20]),
    Err,
    Timeout,
}

impl Receiver {
    fn poll_device(&self, pid: u16, index: u8) -> Result<Option<Battery>> {
        // The receiver keeps each paired device's serial (register 0x2B5, sub 0x30+n), readable even
        // while the device sleeps, so the id survives re-pairing to another slot.
        let device_id = match self.read_long_register(0xB5, 0x30 + index - 1)? {
            Some(r) if r[5..9] != [0; 4] => format!("logi:{:02x}{:02x}{:02x}{:02x}", r[5], r[6], r[7], r[8]),
            _ => format!("logi:{pid:04x}:{index}"),
        };
        match self.request(index, 0, 1, &[0, 0, 0xAA])? {
            Reply::Ok(_) => {}
            // Paired but unreachable (asleep/off) still has a name in the receiver's pairing register.
            Reply::Err | Reply::Timeout => {
                return Ok(self.pairing(index)?.map(|(name, kind)| Battery {
                    device_id,
                    name,
                    part: None,
                    kind,
                    level: Level::Unavailable("asleep or off".into()),
                    charging: None,
                    connected: Some(false),
                    source: "logitech-hidpp",
                }));
            }
        }

        let (name, kind) = self.name_and_kind(index)?;
        let (level, charging) = self.battery(index)?;
        Ok(Some(Battery { device_id, name, part: None, kind, level, charging, connected: Some(true), source: "logitech-hidpp" }))
    }

    fn battery(&self, index: u8) -> Result<(Level, Option<bool>)> {
        if let Some(f) = self.feature(index, FEAT_UNIFIED_BATTERY)? {
            if let Reply::Ok(r) = self.request(index, f, 1, &[])? {
                // get_status: [state of charge %, level flags, charging status, external power]
                let (soc, flags, status) = (r[4], r[5], r[6]);
                let level = if soc > 0 {
                    Level::Percent(soc.min(100))
                } else {
                    Level::Coarse(coarse_from_flags(flags).into())
                };
                return Ok((level, Some(matches!(status, 1 | 2))));
            }
        }
        if let Some(f) = self.feature(index, FEAT_BATTERY_STATUS)? {
            if let Reply::Ok(r) = self.request(index, f, 0, &[])? {
                // get_battery_level_status: [level %, next level %, status]. Level 0 means
                // "unknown" (Solaar does the same); MX Master 3 reports it while charging.
                let charging = matches!(r[6], 1 | 2 | 4);
                let level = match r[4] {
                    0 if charging => Level::Coarse("charging".into()),
                    0 => Level::Unavailable("level unknown".into()),
                    pct => Level::Percent(pct.min(100)),
                };
                return Ok((level, Some(charging)));
            }
        }
        if let Some(f) = self.feature(index, FEAT_BATTERY_VOLTAGE)? {
            if let Reply::Ok(r) = self.request(index, f, 0, &[])? {
                // get_battery_voltage: [mV hi, mV lo, flags]; flags bit 7 = charging
                let mv = u16::from_be_bytes([r[4], r[5]]);
                return Ok((Level::Percent(percent_from_millivolts(mv)), Some(r[6] & 0x80 != 0)));
            }
        }
        Ok((Level::Unavailable("no battery feature".into()), None))
    }

    fn name_and_kind(&self, index: u8) -> Result<(String, Kind)> {
        let Some(f) = self.feature(index, FEAT_DEVICE_NAME)? else {
            return Ok((format!("Logitech device {index}"), Kind::Other));
        };
        let Reply::Ok(r) = self.request(index, f, 0, &[])? else {
            return Ok((format!("Logitech device {index}"), Kind::Other));
        };
        let len = r[4] as usize;
        let mut name = Vec::new();
        while name.len() < len {
            let Reply::Ok(r) = self.request(index, f, 1, &[name.len() as u8])? else { break };
            let chunk = &r[4..20];
            let take = (len - name.len()).min(chunk.len());
            name.extend_from_slice(&chunk[..take]);
        }
        let kind = match self.request(index, f, 2, &[])? {
            // getDeviceType: 0 keyboard, 3 mouse, 4 trackpad, 5 trackball
            Reply::Ok(r) => match r[4] {
                0 => Kind::Keyboard,
                3..=5 => Kind::Mouse,
                _ => Kind::Other,
            },
            _ => Kind::Other,
        };
        Ok((String::from_utf8_lossy(&name).trim_end_matches('\0').to_string(), kind))
    }

    /// Name and kind from the receiver's pairing register (HID++ 1.0 register 0x2B5).
    fn pairing(&self, index: u8) -> Result<Option<(String, Kind)>> {
        let Some(name) = self.read_long_register(0xB5, 0x40 + index - 1)? else { return Ok(None) };
        let len = (name[5] as usize).min(14);
        let name = String::from_utf8_lossy(&name[6..6 + len]).to_string();
        let kind = match self.read_long_register(0xB5, 0x20 + index - 1)? {
            Some(info) => match info[11] & 0x0F {
                1 => Kind::Keyboard,
                2 | 8 | 9 => Kind::Mouse,
                _ => Kind::Other,
            },
            None => Kind::Other,
        };
        Ok(Some((name, kind)))
    }

    fn read_long_register(&self, reg: u8, sub: u8) -> Result<Option<[u8; 20]>> {
        self.drain();
        self.short.write(&[0x10, 0xFF, 0x83, reg, sub, 0, 0]).context("write HID++ register read")?;
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(r) = self.read_any()? {
                if r[1] != 0xFF {
                    continue;
                }
                if r[2] == 0x83 && r[3] == reg && r[4] == sub {
                    return Ok(Some(r));
                }
                if r[2] == 0x8F && r[3] == 0x83 && r[4] == reg {
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    fn feature(&self, index: u8, id: u16) -> Result<Option<u8>> {
        Ok(match self.request(index, 0, 0, &id.to_be_bytes())? {
            Reply::Ok(r) if r[4] != 0 => Some(r[4]),
            _ => None,
        })
    }

    fn request(&self, index: u8, feature: u8, function: u8, params: &[u8]) -> Result<Reply> {
        self.drain();
        let mut msg = [0u8; 20];
        msg[..4].copy_from_slice(&[0x11, index, feature, (function << 4) | SW_ID]);
        msg[4..4 + params.len()].copy_from_slice(params);
        self.long.write(&msg).context("write HID++ request")?;

        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            let Some(r) = self.read_any()? else { continue };
            if r[1] != index {
                continue;
            }
            if r[2] == 0x8F && r[3] == feature {
                return Ok(Reply::Err);
            }
            if r[2] == 0xFF && r[3] == feature && r[4] & 0x0F == SW_ID {
                return Ok(Reply::Err);
            }
            if r[2] == feature && r[3] == (function << 4) | SW_ID {
                return Ok(Reply::Ok(r));
            }
        }
        Ok(Reply::Timeout)
    }

    fn read_any(&self) -> Result<Option<[u8; 20]>> {
        for dev in [&self.long, &self.short] {
            let mut buf = [0u8; 20];
            if dev.read_timeout(&mut buf, 5)? > 0 {
                return Ok(Some(buf));
            }
        }
        Ok(None)
    }

    /// Drop notifications queued before our request so they can't be mistaken for replies.
    fn drain(&self) {
        for dev in [&self.long, &self.short] {
            let mut buf = [0u8; 20];
            while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
        }
    }
}

fn coarse_from_flags(flags: u8) -> &'static str {
    match flags {
        f if f & 0x08 != 0 => "full",
        f if f & 0x04 != 0 => "good",
        f if f & 0x02 != 0 => "low",
        f if f & 0x01 != 0 => "critical",
        _ => "unknown",
    }
}

/// Li-ion discharge curve used by Solaar for feature 0x1001 (mV, %).
fn percent_from_millivolts(mv: u16) -> u8 {
    const CURVE: &[(u16, u8)] = &[
        (4186, 100), (4067, 90), (3989, 80), (3922, 70), (3859, 60), (3811, 50),
        (3778, 40), (3751, 30), (3717, 20), (3671, 10), (3646, 5), (3579, 2), (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let ((hv, hp), (lv, lp)) = (w[0], w[1]);
        if mv >= lv {
            return lp + ((mv - lv) as u32 * (hp - lp) as u32 / (hv - lv) as u32) as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_curve() {
        assert_eq!(percent_from_millivolts(4200), 100);
        assert_eq!(percent_from_millivolts(3811), 50);
        assert_eq!(percent_from_millivolts(3400), 0);
        assert_eq!(percent_from_millivolts(3835), 55);
    }
}
