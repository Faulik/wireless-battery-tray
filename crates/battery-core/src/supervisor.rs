//! Runs every provider on a worker thread with a deadline, so one provider stuck inside a device or
//! OS call (a Bluetooth request that never completes after resume from sleep, say) cannot freeze
//! the whole poll. Threads cannot be killed: a stuck call is left to finish on its own, and the
//! provider is skipped until it does. If it stays stuck for `retry_after`, it is abandoned and a
//! fresh call is started, at most `MAX_ABANDONED` times per provider at once.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{merge, refine_kind, Battery, Provider, Snapshot};

/// One poll of a provider may take this long before it is reported as timed out.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
/// A provider stuck this long gets a fresh call next time instead of being skipped.
pub const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);
/// Stuck threads left behind per provider before it is only skipped, never retried.
const MAX_ABANDONED: u32 = 3;

#[derive(Default)]
struct State {
    /// Call in flight: its generation and when it started.
    running: Option<(u64, Instant)>,
    next_gen: u64,
    /// Calls given up on that have not returned yet.
    abandoned: u32,
}

struct Slot {
    provider: Arc<dyn Provider>,
    state: Arc<Mutex<State>>,
}

pub struct Supervisor {
    slots: Vec<Slot>,
    timeout: Duration,
    retry_after: Duration,
}

/// Marks the call finished when the worker returns or panics.
struct Finished {
    state: Arc<Mutex<State>>,
    gen: u64,
}

impl Drop for Finished {
    fn drop(&mut self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match s.running {
            Some((g, _)) if g == self.gen => s.running = None,
            _ => s.abandoned = s.abandoned.saturating_sub(1),
        }
    }
}

enum Outcome {
    Done(anyhow::Result<Vec<Battery>>),
    Failed(String),
}

impl Supervisor {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self::with_limits(providers, DEFAULT_TIMEOUT, DEFAULT_RETRY_AFTER)
    }

    pub fn with_limits(providers: Vec<Arc<dyn Provider>>, timeout: Duration, retry_after: Duration) -> Self {
        let slots = providers.into_iter().map(|provider| Slot { provider, state: Arc::default() }).collect();
        Self { slots, timeout, retry_after }
    }

    /// Polls every provider in turn, each for at most `timeout`.
    pub fn poll(&self) -> Snapshot {
        let mut tagged = Vec::new();
        let mut errors = Vec::new();
        for slot in &self.slots {
            let p = &slot.provider;
            let started = Instant::now();
            let outcome = self.run(slot);
            if std::env::var_os("BATTERIES_DEBUG").is_some() {
                eprintln!("{}: {:?}", p.name(), started.elapsed());
            }
            match outcome {
                Outcome::Done(Ok(list)) => tagged.extend(list.into_iter().map(|mut b| {
                    (b.kind, b.kind_source) = refine_kind(&b.name, b.kind, b.kind_source);
                    (p.priority(), b)
                })),
                Outcome::Done(Err(e)) => errors.push((p.name(), format!("{e:#}"))),
                Outcome::Failed(e) => errors.push((p.name(), e)),
            }
        }
        Snapshot { batteries: merge(tagged), errors }
    }

    fn run(&self, slot: &Slot) -> Outcome {
        let gen = {
            let mut s = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, since)) = s.running {
                let stuck = since.elapsed();
                if stuck < self.retry_after || s.abandoned >= MAX_ABANDONED {
                    return Outcome::Failed(format!("still stuck in the previous poll ({}s), skipped", stuck.as_secs()));
                }
                s.abandoned += 1;
            }
            let gen = s.next_gen;
            s.next_gen += 1;
            s.running = Some((gen, Instant::now()));
            gen
        };
        let (tx, rx) = mpsc::channel();
        let provider = slot.provider.clone();
        let finished = Finished { state: slot.state.clone(), gen };
        let spawned = std::thread::Builder::new().name(format!("poll-{}", provider.name())).spawn(move || {
            let _finished = finished;
            let _ = tx.send(provider.poll());
        });
        if let Err(e) = spawned {
            return Outcome::Failed(format!("could not start a poll thread: {e}"));
        }
        match rx.recv_timeout(self.timeout) {
            Ok(result) => Outcome::Done(result),
            Err(RecvTimeoutError::Timeout) => Outcome::Failed(format!("timed out after {}s", self.timeout.as_secs())),
            Err(RecvTimeoutError::Disconnected) => Outcome::Failed("panicked".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, KindSource, Level};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn battery(id: &str) -> Battery {
        Battery {
            device_id: id.into(),
            name: id.into(),
            part: None,
            kind: Kind::Other,
            kind_source: KindSource::Provider,
            level: Level::Percent(50),
            charging: None,
            connected: None,
            source: "test",
        }
    }

    struct Quick;
    impl Provider for Quick {
        fn name(&self) -> &'static str {
            "quick"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn poll(&self) -> anyhow::Result<Vec<Battery>> {
            Ok(vec![battery("quick:1")])
        }
    }

    /// Hangs on its first `hangs` calls until the test lets go, then answers at once.
    struct Hangs {
        calls: AtomicU32,
        hangs: u32,
        release: Arc<Mutex<()>>,
    }
    impl Provider for Hangs {
        fn name(&self) -> &'static str {
            "hangs"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn poll(&self) -> anyhow::Result<Vec<Battery>> {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.hangs {
                drop(self.release.lock().unwrap());
            }
            Ok(vec![battery("hangs:1")])
        }
    }

    struct Panics;
    impl Provider for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn priority(&self) -> u8 {
            1
        }
        fn poll(&self) -> anyhow::Result<Vec<Battery>> {
            panic!("boom")
        }
    }

    fn ids(s: &Snapshot) -> Vec<&str> {
        s.batteries.iter().map(|b| b.device_id.as_str()).collect()
    }

    #[test]
    fn stuck_provider_does_not_block_the_others() {
        let release = Arc::new(Mutex::new(()));
        let guard = release.lock().unwrap();
        let hangs = Arc::new(Hangs { calls: AtomicU32::new(0), hangs: 1, release: release.clone() });
        let sup = Supervisor::with_limits(vec![hangs.clone(), Arc::new(Quick)], Duration::from_millis(50), Duration::from_secs(3600));

        let first = sup.poll();
        assert_eq!(ids(&first), ["quick:1"]);
        assert_eq!(first.errors, [("hangs", "timed out after 0s".to_string())]);

        // Still stuck: skipped without starting another call.
        let second = sup.poll();
        assert_eq!(ids(&second), ["quick:1"]);
        assert!(second.errors[0].1.starts_with("still stuck"), "{:?}", second.errors);
        assert_eq!(hangs.calls.load(Ordering::SeqCst), 1);

        // Once the stuck call returns, the provider is polled again.
        drop(guard);
        while sup.slots[0].state.lock().unwrap().running.is_some() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let third = sup.poll();
        assert_eq!(ids(&third), ["hangs:1", "quick:1"]);
        assert!(third.errors.is_empty());
    }

    #[test]
    fn provider_stuck_past_retry_after_gets_a_fresh_call() {
        let release = Arc::new(Mutex::new(()));
        let _guard = release.lock().unwrap();
        let hangs = Arc::new(Hangs { calls: AtomicU32::new(0), hangs: 1, release: release.clone() });
        let sup = Supervisor::with_limits(vec![hangs.clone()], Duration::from_millis(50), Duration::ZERO);

        assert!(sup.poll().batteries.is_empty());
        // The first call is still stuck, but the retry gets an answer.
        let retried = sup.poll();
        assert_eq!(ids(&retried), ["hangs:1"]);
        assert_eq!(hangs.calls.load(Ordering::SeqCst), 2);
        assert_eq!(sup.slots[0].state.lock().unwrap().abandoned, 1);
    }

    #[test]
    fn retries_stop_at_the_abandoned_limit() {
        let release = Arc::new(Mutex::new(()));
        let _guard = release.lock().unwrap();
        let hangs = Arc::new(Hangs { calls: AtomicU32::new(0), hangs: u32::MAX, release: release.clone() });
        let sup = Supervisor::with_limits(vec![hangs.clone()], Duration::from_millis(20), Duration::ZERO);

        for _ in 0..10 {
            sup.poll();
        }
        assert_eq!(hangs.calls.load(Ordering::SeqCst), 1 + MAX_ABANDONED);
    }

    #[test]
    fn panicking_provider_is_an_error_and_polled_again() {
        let sup = Supervisor::new(vec![Arc::new(Panics), Arc::new(Quick)]);
        for _ in 0..2 {
            let s = sup.poll();
            assert_eq!(ids(&s), ["quick:1"]);
            assert_eq!(s.errors, [("panics", "panicked".to_string())]);
        }
    }
}
