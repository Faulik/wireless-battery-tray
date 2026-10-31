//! `%APPDATA%\batteries_app\config.toml` plus the last poll result (`snapshot.json`) that the
//! settings window uses for its preview.

use std::path::PathBuf;

use anyhow::Result;
use battery_core::{Battery, Kind, Level};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Monitor device name (`\\.\DISPLAY3`); `None` = primary.
    pub monitor: Option<String>,
    pub poll_secs: u64,
    /// Percent below which a reading is shown in red.
    pub low_threshold: u8,
    /// Text size in points.
    pub text_size: f32,
    /// Two rows instead of one.
    pub compact: bool,
    /// Show devices that are off/asleep with no reading as a dim dash.
    pub show_offline: bool,
    /// Every device ever seen, in display order.
    pub devices: Vec<DeviceConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Short label override; `None` = derived from the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default = "yes")]
    pub show: bool,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitor: None,
            poll_secs: 60,
            low_threshold: 20,
            text_size: 9.0,
            compact: false,
            show_offline: false,
            devices: Vec::new(),
        }
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("batteries_app")
}

impl Config {
    pub fn path() -> PathBuf {
        dir().join("config.toml")
    }

    /// Missing or unreadable config falls back to defaults rather than failing startup.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(dir())?;
        let tmp = Self::path().with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(tmp, Self::path())?;
        Ok(())
    }

    /// Add devices seen for the first time (at the end) and refresh names. Returns true if changed.
    pub fn remember(&mut self, readings: &[Reading]) -> bool {
        let mut changed = false;
        for r in readings {
            match self.devices.iter_mut().find(|d| d.id == r.device_id) {
                Some(d) if d.name != r.name || d.kind != r.kind => {
                    d.name = r.name.clone();
                    d.kind = r.kind;
                    changed = true;
                }
                Some(_) => {}
                None => {
                    self.devices.push(DeviceConfig {
                        id: r.device_id.clone(),
                        name: r.name.clone(),
                        kind: r.kind,
                        label: None,
                        show: true,
                    });
                    changed = true;
                }
            }
        }
        changed
    }
}

/// A battery reading in a form both processes can (de)serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub device_id: String,
    pub name: String,
    pub part: Option<String>,
    pub kind: Kind,
    pub level: Level,
    pub charging: Option<bool>,
    pub connected: Option<bool>,
    pub source: String,
}

impl From<Battery> for Reading {
    fn from(b: Battery) -> Self {
        Self {
            device_id: b.device_id,
            name: b.name,
            part: b.part,
            kind: b.kind,
            level: b.level,
            charging: b.charging,
            connected: b.connected,
            source: b.source.to_string(),
        }
    }
}

pub fn snapshot_path() -> PathBuf {
    dir().join("snapshot.json")
}

pub fn save_snapshot(readings: &[Reading]) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(snapshot_path(), serde_json::to_vec_pretty(readings)?)?;
    Ok(())
}

pub fn load_snapshot() -> Vec<Reading> {
    std::fs::read(snapshot_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_missing_fields() {
        let cfg: Config = toml::from_str("poll_secs = 30\n[[devices]]\nid = \"x\"\nname = \"X\"\nkind = \"Mouse\"\n").unwrap();
        assert_eq!(cfg.poll_secs, 30);
        assert_eq!(cfg.low_threshold, 20);
        assert!(cfg.devices[0].show);
        let again: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(again, cfg);
    }
}
