//! What the strip shows: config + readings -> short labelled items. Shared by the strip and
//! the settings preview so both always agree.

use battery_core::Level;

use crate::config::{Config, DeviceConfig, Reading};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Normal,
    Low,
    /// Off, asleep or a last-known value.
    Dim,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub text: String,
    pub style: Style,
    /// One line for the tooltip.
    pub detail: String,
}

pub fn build(cfg: &Config, readings: &[Reading]) -> Vec<Item> {
    let mut out = Vec::new();
    for dev in cfg.devices.iter().filter(|d| d.show) {
        let label = label(dev);
        let parts: Vec<&Reading> = readings.iter().filter(|r| r.device_id == dev.id).collect();
        if parts.is_empty() {
            if cfg.show_offline {
                out.push(Item { text: format!("{label} –"), style: Style::Dim, detail: format!("{}: not found", dev.name) });
            }
            continue;
        }
        for r in parts {
            let label = match &r.part {
                Some(p) => format!("{label} {}", short_part(p)),
                None => label.clone(),
            };
            let stale = r.connected == Some(false);
            let charging = if r.charging == Some(true) { "⚡" } else { "" };
            let full_name = match &r.part {
                Some(p) => format!("{} ({p})", r.name),
                None => r.name.clone(),
            };
            let (text, style, state) = match &r.level {
                Level::Percent(p) => {
                    let style = if stale {
                        Style::Dim
                    } else if *p < cfg.low_threshold && r.charging != Some(true) {
                        Style::Low
                    } else {
                        Style::Normal
                    };
                    let state = if stale { format!("{p}% (last known)") } else { format!("{p}%") };
                    (format!("{label} {charging}{p}%"), style, state)
                }
                Level::Coarse(s) => (format!("{label} {s}"), Style::Normal, s.clone()),
                Level::Unavailable(why) => {
                    if !cfg.show_offline {
                        continue;
                    }
                    (format!("{label} –"), Style::Dim, why.clone())
                }
            };
            let charging_note = if r.charging == Some(true) { ", charging" } else { "" };
            out.push(Item { text, style, detail: format!("{full_name}: {state}{charging_note} [{}]", r.source) });
        }
    }
    out
}

pub fn label(dev: &DeviceConfig) -> String {
    match &dev.label {
        Some(l) if !l.trim().is_empty() => l.trim().to_string(),
        _ => auto_label(&dev.name),
    }
}

/// A short label from a product name: drop brand and generic words, keep the first model-ish word.
pub fn auto_label(name: &str) -> String {
    const SKIP: &[&str] = &[
        "wireless", "mouse", "keyboard", "headset", "headphones", "gaming", "bluetooth", "logitech", "steelseries",
        "arctis", "keychron", "soundpeats", "sony", "razer", "corsair", "hyperx",
    ];
    if name.to_ascii_lowercase().starts_with("dualshock") {
        return "DS4".into();
    }
    if name.to_ascii_lowercase().starts_with("dualsense") {
        return "DS5".into();
    }
    let word = name
        .split_whitespace()
        .find(|w| !SKIP.contains(&w.to_ascii_lowercase().as_str()))
        .unwrap_or(name);
    word.chars().take(8).collect()
}

fn short_part(part: &str) -> String {
    match part {
        "Central" => "L".into(),
        p if p.starts_with("Peripheral") => "R".into(),
        p => p.chars().take(3).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use battery_core::Kind;

    #[test]
    fn auto_labels() {
        assert_eq!(auto_label("Wireless Mouse MX Master 3"), "MX");
        assert_eq!(auto_label("SteelSeries Arctis Nova 7P"), "Nova");
        assert_eq!(auto_label("SOUNDPEATS H3"), "H3");
        assert_eq!(auto_label("Keychron K3 HE"), "K3");
        assert_eq!(auto_label("Glove80"), "Glove80");
        assert_eq!(auto_label("DualShock 4"), "DS4");
    }

    fn reading(id: &str, level: Level, connected: Option<bool>) -> Reading {
        Reading {
            device_id: id.into(),
            name: id.into(),
            part: None,
            kind: Kind::Mouse,
            level,
            charging: None,
            connected,
            source: "test".into(),
        }
    }

    #[test]
    fn styles_and_hiding() {
        let mut cfg = Config::default();
        let rs = vec![
            reading("A", Level::Percent(10), Some(true)),
            reading("B", Level::Percent(80), Some(false)),
            reading("C", Level::Unavailable("off".into()), Some(false)),
        ];
        cfg.remember(&rs);
        cfg.devices[1].label = Some("bee".into());
        let items = build(&cfg, &rs);
        assert_eq!(items.iter().map(|i| (i.text.as_str(), i.style)).collect::<Vec<_>>(), [("A 10%", Style::Low), ("bee 80%", Style::Dim)]);
        cfg.show_offline = true;
        cfg.devices[0].show = false;
        let items = build(&cfg, &rs);
        assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["bee 80%", "C –"]);
    }
}
