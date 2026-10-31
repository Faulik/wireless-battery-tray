//! What the strip shows: config + readings -> short labelled items. Shared by the strip and
//! the settings preview so both always agree.

use battery_core::Level;

use crate::config::{Config, DeviceConfig, Display, Reading};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Normal,
    Low,
    /// Off, asleep or a last-known value.
    Dim,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Icon glyph (Segoe Fluent Icons), when the device is shown with an icon.
    pub icon: Option<char>,
    /// Tells apart devices that show the same icon: 1, 2, 3 in settings order.
    pub badge: Option<u32>,
    pub text: String,
    pub style: Style,
    /// One line for the tooltip.
    pub detail: String,
}

pub fn build(cfg: &Config, readings: &[Reading]) -> Vec<Item> {
    let badges = badges(cfg);
    let mut out = Vec::new();
    for (dev, badge) in cfg.devices.iter().zip(badges).filter(|(d, _)| d.show) {
        let label = label(dev);
        let icon = (dev.display != Display::Label).then(|| dev.icon().glyph());
        let show_label = dev.display != Display::Icon;
        let item = |label: &str, value: String, style, detail| Item {
            icon,
            badge,
            text: if show_label { format!("{label} {value}") } else { value },
            style,
            detail,
        };
        let parts: Vec<&Reading> = readings.iter().filter(|r| r.device_id == dev.id).collect();
        if parts.is_empty() {
            if cfg.show_offline {
                out.push(item(&label, "–".into(), Style::Dim, format!("{}: not found [{}]", dev.name, dev.id)));
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
            let (value, style, state) = match &r.level {
                Level::Percent(p) => {
                    let style = if stale {
                        Style::Dim
                    } else if *p < cfg.low_threshold && r.charging != Some(true) {
                        Style::Low
                    } else {
                        Style::Normal
                    };
                    let state = if stale { format!("{p}% (last known)") } else { format!("{p}%") };
                    (format!("{charging}{p}%"), style, state)
                }
                Level::Coarse(s) => (format!("{charging}{s}"), Style::Normal, s.clone()),
                Level::Unavailable(why) => {
                    if !cfg.show_offline {
                        continue;
                    }
                    ("–".into(), Style::Dim, why.clone())
                }
            };
            let charging_note = if r.charging == Some(true) { ", charging" } else { "" };
            let badge_note = badge.map(|b| format!(" #{b}")).unwrap_or_default();
            let detail = format!("{full_name}{badge_note}: {state}{charging_note} [{}, {}]", r.source, dev.id);
            out.push(item(&label, value, style, detail));
        }
    }
    out
}

/// Per device (in config order): its number among shown icon-only devices with the same icon,
/// or `None` if that icon is unique. Numbering ignores whether a device is online right now, so
/// a device's number doesn't change when another one goes to sleep.
pub fn badges(cfg: &Config) -> Vec<Option<u32>> {
    let icon_only = |d: &DeviceConfig| d.show && d.display == Display::Icon;
    let mut seen = std::collections::HashMap::new();
    cfg.devices
        .iter()
        .map(|d| {
            if !icon_only(d) {
                return None;
            }
            let total = cfg.devices.iter().filter(|o| icon_only(o) && o.icon() == d.icon()).count();
            let n = seen.entry(d.icon()).or_insert(0u32);
            *n += 1;
            (total > 1).then_some(*n)
        })
        .collect()
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
        for d in &mut cfg.devices {
            d.display = Display::Label;
        }
        cfg.devices[1].label = Some("bee".into());
        let items = build(&cfg, &rs);
        assert_eq!(items.iter().map(|i| (i.text.as_str(), i.style)).collect::<Vec<_>>(), [("A 10%", Style::Low), ("bee 80%", Style::Dim)]);
        cfg.show_offline = true;
        cfg.devices[0].show = false;
        let items = build(&cfg, &rs);
        assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["bee 80%", "C –"]);
    }

    #[test]
    fn same_icon_gets_numbered() {
        let mut cfg = Config::default();
        let mut rs = vec![
            reading("kb1", Level::Percent(50), Some(true)),
            reading("mouse", Level::Percent(60), Some(true)),
            reading("kb2", Level::Unavailable("asleep".into()), Some(false)),
        ];
        rs[0].kind = Kind::Keyboard;
        rs[2].kind = Kind::Keyboard;
        cfg.remember(&rs);
        assert_eq!(badges(&cfg), [Some(1), None, Some(2)]);
        let items = build(&cfg, &rs);
        assert_eq!(items[0].text, "50%");
        assert_eq!(items[0].icon, Some('\u{E765}'));
        // kb2 is asleep and hidden, but kb1 keeps badge 1 and the mouse has none.
        assert_eq!(items.iter().map(|i| i.badge).collect::<Vec<_>>(), [Some(1), None]);
        // A label on one of them takes it out of the numbering.
        cfg.devices[2].display = Display::IconLabel;
        assert_eq!(badges(&cfg), [None, None, None]);
    }
}
