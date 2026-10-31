//! Core types shared by every battery provider and front-end.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kind {
    Keyboard,
    Mouse,
    Headset,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "value")]
pub enum Level {
    /// Exact or device-reported percentage, 0..=100.
    Percent(u8),
    /// Coarse level with no percentage ("low", "good", ...).
    Coarse(String),
    /// Device known but no reading right now ("headset off", "asleep", ...).
    Unavailable(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Battery {
    /// Stable identity of the physical device, e.g. `bt:0a1b2c3d4e5f` or `logi:c52b:1`.
    /// Entries from different providers with the same id are the same device.
    pub device_id: String,
    pub name: String,
    /// Part of a multi-battery device ("Central", "Peripheral 0"); `None` for single-battery devices.
    pub part: Option<String>,
    pub kind: Kind,
    pub level: Level,
    pub charging: Option<bool>,
    /// `Some(false)` when the device is known to be off/asleep/disconnected; a percentage is then the last known value.
    pub connected: Option<bool>,
    /// Provider that produced this reading.
    pub source: &'static str,
}

pub trait Provider {
    fn name(&self) -> &'static str;
    /// Higher wins when two providers report the same `device_id`.
    fn priority(&self) -> u8;
    fn poll(&self) -> anyhow::Result<Vec<Battery>>;
}

/// Result of polling all providers: merged readings plus per-provider errors.
pub struct Snapshot {
    pub batteries: Vec<Battery>,
    pub errors: Vec<(&'static str, String)>,
}

pub fn poll_all(providers: &[Box<dyn Provider>]) -> Snapshot {
    let mut tagged = Vec::new();
    let mut errors = Vec::new();
    for p in providers {
        let started = std::time::Instant::now();
        let result = p.poll();
        if std::env::var_os("BATTERIES_DEBUG").is_some() {
            eprintln!("{}: {:?}", p.name(), started.elapsed());
        }
        match result {
            Ok(list) => tagged.extend(list.into_iter().map(|b| (p.priority(), b))),
            Err(e) => errors.push((p.name(), format!("{e:#}"))),
        }
    }
    Snapshot { batteries: merge(tagged), errors }
}

/// Per `device_id`, keep only the entries from the highest-priority provider that reported it,
/// then drop exact duplicates of (device_id, part). Order of first appearance is preserved.
pub fn merge(tagged: Vec<(u8, Battery)>) -> Vec<Battery> {
    use std::collections::HashMap;
    let mut best: HashMap<String, u8> = HashMap::new();
    for (prio, b) in &tagged {
        let e = best.entry(b.device_id.clone()).or_insert(*prio);
        *e = (*e).max(*prio);
    }
    let mut out: Vec<Battery> = Vec::new();
    for (prio, b) in tagged {
        if best[&b.device_id] != prio {
            continue;
        }
        if out.iter().any(|o| o.device_id == b.device_id && o.part == b.part) {
            continue;
        }
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: &str, part: Option<&str>, pct: u8, source: &'static str) -> Battery {
        Battery {
            device_id: id.into(),
            name: id.into(),
            part: part.map(Into::into),
            kind: Kind::Other,
            level: Level::Percent(pct),
            charging: None,
            connected: None,
            source,
        }
    }

    #[test]
    fn higher_priority_source_replaces_lower() {
        let merged = merge(vec![
            (1, b("bt:a", None, 75, "pnp")),
            (2, b("bt:a", Some("Central"), 75, "gatt")),
            (2, b("bt:a", Some("Peripheral 0"), 60, "gatt")),
            (1, b("bt:b", None, 100, "pnp")),
        ]);
        let got: Vec<_> = merged.iter().map(|x| (x.device_id.as_str(), x.source)).collect();
        assert_eq!(got, [("bt:a", "gatt"), ("bt:a", "gatt"), ("bt:b", "pnp")]);
    }

    #[test]
    fn duplicate_parts_are_dropped() {
        let merged = merge(vec![(1, b("x", None, 1, "p")), (1, b("x", None, 2, "p"))]);
        assert_eq!(merged.len(), 1);
    }
}
