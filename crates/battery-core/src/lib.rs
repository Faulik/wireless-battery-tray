//! Core types shared by every battery provider and front-end.

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Keyboard,
    Mouse,
    /// Over-ear with a microphone.
    Headset,
    /// Over-ear, no microphone.
    Headphones,
    /// In-ear.
    Earbuds,
    Neckband,
    Gamepad,
    Other,
}

mod classify;
pub use classify::{refine_kind, KindSource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Which detection layer decided `kind`.
    pub kind_source: KindSource,
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

/// Runs each provider on its own thread with a deadline, so one that hangs (a WinRT call that
/// never completes after the Bluetooth stack resets on resume from sleep, say) can't hold up the
/// others or the caller. A provider still stuck from an earlier round is skipped until it returns.
pub struct Providers {
    entries: Vec<Entry>,
}

struct Entry {
    provider: Arc<dyn Provider + Send + Sync>,
    /// Set while a poll of this provider is running; holds the channel its result arrives on.
    pending: Option<mpsc::Receiver<anyhow::Result<Vec<Battery>>>>,
}

impl Providers {
    pub fn new(providers: Vec<Arc<dyn Provider + Send + Sync>>) -> Self {
        Self { entries: providers.into_iter().map(|provider| Entry { provider, pending: None }).collect() }
    }

    /// Polls every provider in parallel and waits at most `timeout` for all of them.
    pub fn poll_all(&mut self, timeout: Duration) -> Snapshot {
        let deadline = Instant::now() + timeout;
        let mut errors = Vec::new();
        for e in &mut self.entries {
            if let Some(rx) = &e.pending {
                match rx.try_recv() {
                    // Finished late: that result is stale, poll again below.
                    Ok(_) | Err(TryRecvError::Disconnected) => e.pending = None,
                    Err(TryRecvError::Empty) => {
                        errors.push((e.provider.name(), "still busy from an earlier poll; skipped".to_string()));
                        continue;
                    }
                }
            }
            let (tx, rx) = mpsc::channel();
            let p = e.provider.clone();
            let spawned = std::thread::Builder::new().name(format!("poll-{}", p.name())).spawn(move || {
                let started = Instant::now();
                let result = p.poll();
                if std::env::var_os("BATTERIES_DEBUG").is_some() {
                    eprintln!("{}: {:?}", p.name(), started.elapsed());
                }
                let _ = tx.send(result);
            });
            match spawned {
                Ok(_) => e.pending = Some(rx),
                Err(err) => errors.push((e.provider.name(), format!("spawn poll thread: {err}"))),
            }
        }

        let mut tagged = Vec::new();
        for e in &mut self.entries {
            let Some(rx) = &e.pending else { continue };
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(result) => {
                    e.pending = None;
                    match result {
                        Ok(list) => tagged.extend(list.into_iter().map(|mut b| {
                            (b.kind, b.kind_source) = refine_kind(&b.name, b.kind, b.kind_source);
                            (e.provider.priority(), b)
                        })),
                        Err(err) => errors.push((e.provider.name(), format!("{err:#}"))),
                    }
                }
                Err(RecvTimeoutError::Timeout) => errors.push((e.provider.name(), format!("no answer within {timeout:?}"))),
                Err(RecvTimeoutError::Disconnected) => {
                    e.pending = None;
                    errors.push((e.provider.name(), "poll thread panicked".to_string()));
                }
            }
        }
        // Collected in provider order, whichever thread finished first.
        Snapshot { batteries: merge(tagged), errors }
    }
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
            kind_source: KindSource::Provider,
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

    struct Fixed(&'static str, Duration);

    impl Provider for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn priority(&self) -> u8 {
            1
        }
        fn poll(&self) -> anyhow::Result<Vec<Battery>> {
            std::thread::sleep(self.1);
            Ok(vec![b(self.0, None, 50, "test")])
        }
    }

    #[test]
    fn a_hung_provider_does_not_block_the_others() {
        let mut ps = Providers::new(vec![Arc::new(Fixed("slow", Duration::from_secs(3))), Arc::new(Fixed("fast", Duration::ZERO))]);
        let started = Instant::now();
        let snap = ps.poll_all(Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(snap.batteries.iter().map(|b| b.device_id.as_str()).collect::<Vec<_>>(), ["fast"]);
        assert_eq!(snap.errors.len(), 1);
        assert_eq!(snap.errors[0].0, "slow");

        // Still running: skipped rather than started a second time.
        let snap = ps.poll_all(Duration::from_millis(300));
        assert_eq!(snap.batteries.len(), 1);
        assert!(snap.errors[0].1.contains("still busy"));
    }

    #[test]
    fn results_keep_provider_order() {
        let mut ps = Providers::new(vec![Arc::new(Fixed("a", Duration::from_millis(100))), Arc::new(Fixed("b", Duration::ZERO))]);
        let snap = ps.poll_all(Duration::from_secs(5));
        assert_eq!(snap.batteries.iter().map(|b| b.device_id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(snap.errors.is_empty());
    }
}
