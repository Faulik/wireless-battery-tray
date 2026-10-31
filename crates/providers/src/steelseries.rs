//! SteelSeries Arctis Nova 7 family over the 2.4 GHz dongle.
//!
//! Protocol from HeadsetControl (lib/devices/protocols/steelseries_protocol.hpp and
//! steelseries_arctis_nova_7.hpp): interface 3, usage page 0xFFC0 usage 1; write
//! `[0x00, 0xB0]`, read 8 bytes. `data[3]`: 0 = headset offline, 1/2 = charging.
//! `data[2]`: battery, 0..=4 steps on original firmware PIDs, 0..=100 on newer ones.

use std::time::Duration;

use anyhow::Result;
use battery_core::{Battery, Kind, KindSource, Level, Provider};
use hidapi::HidApi;

use crate::stable_id;

const VID: u16 = 0x1038;

/// (pid, name, discrete 0..=4 battery)
const MODELS: &[(u16, &str, bool)] = &[
    (0x2202, "SteelSeries Arctis Nova 7", true),
    (0x22a1, "SteelSeries Arctis Nova 7", false),
    (0x227e, "SteelSeries Arctis Nova 7 Gen 2", false),
    (0x2206, "SteelSeries Arctis Nova 7X", true),
    (0x2258, "SteelSeries Arctis Nova 7X", false),
    (0x229e, "SteelSeries Arctis Nova 7X", false),
    (0x22ad, "SteelSeries Arctis Nova 7X", false),
    (0x22a4, "SteelSeries Arctis Nova 7X", true),
    (0x22a5, "SteelSeries Arctis Nova 7X", false),
    (0x223a, "SteelSeries Arctis Nova 7 Diablo IV", true),
    (0x22a9, "SteelSeries Arctis Nova 7 Diablo IV", false),
    (0x227a, "SteelSeries Arctis Nova 7 WoW Edition", true),
    (0x220a, "SteelSeries Arctis Nova 7P", true),
    (0x22a7, "SteelSeries Arctis Nova 7P", false),
    (0x2298, "SteelSeries Arctis Nova 7P", false),
];

const TIMEOUT: Duration = Duration::from_millis(1000);

pub struct SteelSeriesProvider;

impl Provider for SteelSeriesProvider {
    fn name(&self) -> &'static str {
        "steelseries"
    }
    fn priority(&self) -> u8 {
        1
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let api = HidApi::new()?;
        let mut out = Vec::new();
        for info in api.device_list() {
            if info.vendor_id() != VID || info.usage_page() != 0xFFC0 || info.usage() != 1 {
                continue;
            }
            let Some(&(pid, name, discrete)) = MODELS.iter().find(|m| m.0 == info.product_id()) else {
                continue;
            };
            let dev = api.open_path(info.path())?;
            dev.write(&[0x00, 0xB0])?;
            let mut data = [0u8; 8];
            let n = dev.read_timeout(&mut data, TIMEOUT.as_millis() as i32)?;
            let (level, charging) = if n < 4 { (Level::Unavailable("no reply from dongle".into()), None) } else { parse_status(&data, discrete) };
            out.push(Battery {
                device_id: stable_id("ss", pid, info.serial_number()),
                name: name.to_string(),
                part: None,
                kind: Kind::Headset,
                kind_source: KindSource::Provider,
                connected: Some(!matches!(level, Level::Unavailable(_))),
                level,
                charging,
                source: "steelseries-hid",
            });
        }
        Ok(out)
    }
}

fn parse_status(data: &[u8], discrete: bool) -> (Level, Option<bool>) {
    if data[3] == 0x00 {
        return (Level::Unavailable("headset off".into()), None);
    }
    let pct = if discrete { u16::from(data[2]) * 100 / 4 } else { u16::from(data[2]) };
    (Level::Percent(pct.min(100) as u8), Some(matches!(data[3], 0x01 | 0x02)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_and_levels() {
        // Real reply from a 0x220A dongle with the headset switched off.
        assert_eq!(parse_status(&[0xb0, 0x02, 0x00, 0x00, 0xff, 0xff, 0, 0], true).0, Level::Unavailable("headset off".into()));
        assert_eq!(parse_status(&[0xb0, 0, 3, 0x03, 0, 0, 0, 0], true), (Level::Percent(75), Some(false)));
        assert_eq!(parse_status(&[0xb0, 0, 64, 0x01, 0, 0, 0, 0], false), (Level::Percent(64), Some(true)));
    }
}
