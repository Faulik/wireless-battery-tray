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
    pub device_id: String,
    /// Icon glyph (Segoe Fluent Icons), when the device is shown with an icon.
    pub icon: Option<char>,
    /// Small name tag after the icon, e.g. "K580", when the icon alone is ambiguous.
    pub tag: Option<String>,
    pub text: String,
    pub style: Style,
    /// One line for the tooltip.
    pub detail: String,
    /// Outlined on the strip (the settings window is hovering this device).
    pub highlight: bool,
}

pub fn build(cfg: &Config, readings: &[Reading]) -> Vec<Item> {
    let tags = tags(cfg, readings);
    let mut out = Vec::new();
    for (dev, tag) in cfg.devices.iter().zip(tags).filter(|(d, _)| d.show) {
        let label = label(dev);
        let icon = (dev.display != Display::Label).then(|| dev.icon().glyph());
        let tag = tag.map(|t| t.text);
        let item = |label: &str, value: String, style, detail| Item {
            device_id: dev.id.clone(),
            icon,
            tag: tag.as_ref().map(|t| match label.strip_prefix(&*self::label(dev)) {
                // Split-keyboard halves keep their "L"/"R" suffix on the tag.
                Some(rest) if !rest.is_empty() => format!("{t}{rest}"),
                _ => t.clone(),
            }),
            text: if dev.display == Display::Label { format!("{label} {value}") } else { value },
            style,
            detail,
            highlight: false,
        };
        let parts: Vec<&Reading> = readings.iter().filter(|r| r.device_id == dev.id).collect();
        if parts.is_empty() {
            if cfg.show_offline {
                out.push(item(&label, "–".into(), Style::Dim, format!("{}: not found", dev.name)));
            }
            continue;
        }
        for r in parts {
            let label = match &r.part {
                Some(p) => format!("{label} {}", short_part(p)),
                None => label.clone(),
            };
            let stale = r.connected == Some(false);
            if stale && !cfg.show_offline {
                continue;
            }
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
                // Charging with no level reported (MX Master 3): just the bolt.
                Level::Coarse(s) if s == "charging" && r.charging == Some(true) => ("⚡".into(), Style::Normal, "charging (no level reported)".into()),
                Level::Coarse(s) => (format!("{charging}{s}"), Style::Normal, s.clone()),
                Level::Unavailable(why) => {
                    if !cfg.show_offline {
                        continue;
                    }
                    ("–".into(), Style::Dim, why.clone())
                }
            };
            let charging_note = if r.charging == Some(true) { ", charging" } else { "" };
            let detail = format!("{full_name}: {state}{charging_note} · {}", connection(&r.source, &dev.id));
            out.push(item(&label, value, style, detail));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    pub text: String,
    /// Another shown device would have had the same tag; a hint from the id was appended.
    pub clashed: bool,
}

/// Per device (in config order): the tag shown next to its icon, if any.
/// Auto mode tags a device only when another shown device uses the same icon.
pub fn tags(cfg: &Config, readings: &[Reading]) -> Vec<Option<Tag>> {
    let iconic = |d: &DeviceConfig| on_strip(cfg, d, readings) && d.display != Display::Label;
    let wanted: Vec<bool> = cfg
        .devices
        .iter()
        .map(|d| match d.display {
            Display::IconLabel => on_strip(cfg, d, readings),
            Display::Auto => iconic(d) && cfg.devices.iter().filter(|o| iconic(o) && o.icon() == d.icon()).count() > 1,
            Display::Icon | Display::Label => false,
        })
        .collect();
    let base: Vec<String> = cfg.devices.iter().map(label).collect();
    cfg.devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            if !wanted[i] {
                return None;
            }
            let clashed = (0..cfg.devices.len()).any(|j| j != i && wanted[j] && base[j].eq_ignore_ascii_case(&base[i]));
            let text = if clashed { format!("{}·{}", base[i], id_hint(&d.id)) } else { base[i].clone() };
            Some(Tag { text, clashed })
        })
        .collect()
}

/// Whether a device currently appears on the strip (disconnected devices are left out unless
/// "show disconnected" is on).
pub fn on_strip(cfg: &Config, dev: &DeviceConfig, readings: &[Reading]) -> bool {
    dev.show && (cfg.show_offline || readings.iter().any(|r| r.device_id == dev.id && is_live(r)))
}

/// A reading from a device that is connected right now.
pub fn is_live(r: &Reading) -> bool {
    r.connected != Some(false) && !matches!(r.level, Level::Unavailable(_))
}

/// Last two characters of the id (MAC/serial tail), to tell identical models apart.
fn id_hint(id: &str) -> String {
    let tail: Vec<char> = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    tail[tail.len().saturating_sub(2)..].iter().collect::<String>().to_ascii_uppercase()
}

/// How a device is connected, in words, from the reading's source and the id.
pub fn connection(source: &str, id: &str) -> &'static str {
    match source {
        "windows-pnp" | "ble-gatt" => "Bluetooth",
        "logitech-hidpp" => "Logitech receiver",
        "steelseries-hid" => "SteelSeries dongle",
        "keychron-link" => "Keychron receiver",
        "dualshock-hid" if id.starts_with("bt:") || id.len() > 12 => "Bluetooth",
        "dualshock-hid" => "USB",
        _ => "",
    }
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
            kind_source: battery_core::KindSource::Provider,
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
        assert_eq!(items.iter().map(|i| (i.text.as_str(), i.style)).collect::<Vec<_>>(), [("A 10%", Style::Low)]);
        cfg.show_offline = true;
        cfg.devices[0].show = false;
        let items = build(&cfg, &rs);
        assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["bee 80%", "C –"]);
    }

    #[test]
    fn tags_only_where_icons_clash() {
        let mut cfg = Config::default();
        let mut rs = vec![
            reading("Logitech K580", Level::Percent(50), Some(true)),
            reading("Mouse M1", Level::Percent(60), Some(true)),
            reading("Glove80", Level::Percent(70), Some(true)),
        ];
        rs[0].kind = Kind::Keyboard;
        rs[2].kind = Kind::Keyboard;
        cfg.remember(&rs);
        let t: Vec<Option<String>> = tags(&cfg, &rs).into_iter().map(|t| t.map(|t| t.text)).collect();
        assert_eq!(t, [Some("K580".into()), None, Some("Glove80".into())]);
        let items = build(&cfg, &rs);
        assert_eq!((items[0].icon, items[0].tag.as_deref(), items[0].text.as_str()), (Some('\u{E765}'), Some("K580"), "50%"));
        assert_eq!(items[1].tag, None);
        // Glove80 disconnects: it leaves the strip, so the K580 no longer needs a tag.
        rs[2].connected = Some(false);
        assert_eq!(tags(&cfg, &rs)[0], None);
        assert_eq!(build(&cfg, &rs).len(), 2);
    }

    #[test]
    fn charging_without_level_shows_just_the_bolt() {
        let cfg_rs = vec![Reading { charging: Some(true), ..reading("MX", Level::Coarse("charging".into()), Some(true)) }];
        let mut cfg = Config::default();
        cfg.remember(&cfg_rs);
        assert_eq!(build(&cfg, &cfg_rs)[0].text, "⚡");
    }

    #[test]
    fn identical_models_get_an_id_hint() {
        let mut cfg = Config::default();
        let mut rs = vec![reading("MX Master 3", Level::Percent(50), Some(true)), reading("MX Master 3", Level::Percent(60), Some(true))];
        rs[0].device_id = "logi:0badf00d".into();
        rs[1].device_id = "logi:12345678".into();
        cfg.remember(&rs);
        let t: Vec<Tag> = tags(&cfg, &rs).into_iter().flatten().collect();
        assert_eq!(t.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["MX·0D", "MX·78"]);
        assert!(t.iter().all(|t| t.clashed));
    }
}