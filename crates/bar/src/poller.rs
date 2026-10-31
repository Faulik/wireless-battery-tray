//! Background polling. Providers can take seconds (sleeping devices), so they never run on the
//! UI thread; results are handed over through a mutex and a posted window message.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use battery_core::Level;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::{load_snapshot, save_snapshot, Reading};

pub enum Command {
    RefreshNow,
    SetInterval(u64),
}

pub struct Poller {
    tx: Sender<Command>,
    latest: Arc<Mutex<Option<Vec<Reading>>>>,
}

impl Poller {
    /// Starts polling immediately; posts `msg` to `hwnd` after every poll.
    pub fn start(hwnd: HWND, msg: u32, interval_secs: u64) -> Self {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let shared = latest.clone();
        let hwnd = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("poller".into())
            .spawn(move || {
                let providers = providers::all();
                let mut last_level = HashMap::new();
                remember_levels(&mut last_level, &load_snapshot());
                let mut interval = Duration::from_secs(interval_secs.max(5));
                loop {
                    let snap = battery_core::poll_all(&providers);
                    for (p, e) in &snap.errors {
                        eprintln!("poll: {p}: {e}");
                    }
                    let mut readings: Vec<Reading> = snap.batteries.into_iter().map(Reading::from).collect();
                    fill_while_charging(&mut readings, &last_level);
                    remember_levels(&mut last_level, &readings);
                    let _ = save_snapshot(&readings);
                    *shared.lock().unwrap() = Some(readings);
                    // SAFETY: posting to a window handle is safe even if the window is gone.
                    if unsafe { PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0)) }.is_err() {
                        return;
                    }
                    match rx.recv_timeout(interval) {
                        Ok(Command::RefreshNow) | Err(RecvTimeoutError::Timeout) => {}
                        Ok(Command::SetInterval(s)) => interval = Duration::from_secs(s.max(5)),
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawn poller thread");
        Self { tx, latest }
    }

    pub fn take(&self) -> Option<Vec<Reading>> {
        self.latest.lock().unwrap().take()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

type LevelKey = (String, Option<String>);

/// Keep the latest percentage per device (and split part), including carried-over estimates.
fn remember_levels(last: &mut HashMap<LevelKey, u8>, readings: &[Reading]) {
    for r in readings {
        if let Level::Percent(p) = r.level {
            last.insert((r.device_id.clone(), r.part.clone()), p);
        }
    }
}

/// Some devices (the MX Master 3) report no level at all while charging. Show the last level we
/// read instead, flagged as approximate, rather than a bare "charging".
fn fill_while_charging(readings: &mut [Reading], last: &HashMap<LevelKey, u8>) {
    for r in readings.iter_mut().filter(|r| r.charging == Some(true) && !matches!(r.level, Level::Percent(_))) {
        if let Some(&p) = last.get(&(r.device_id.clone(), r.part.clone())) {
            r.level = Level::Percent(p);
            r.approx = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use battery_core::{Kind, KindSource};

    fn mouse(level: Level, charging: bool) -> Reading {
        Reading {
            device_id: "logi:1".into(),
            name: "MX".into(),
            part: None,
            kind: Kind::Mouse,
            kind_source: KindSource::Provider,
            level,
            charging: Some(charging),
            connected: Some(true),
            source: "logitech-hidpp".into(),
            approx: false,
        }
    }

    #[test]
    fn carries_last_level_into_charging() {
        let mut last = HashMap::new();
        remember_levels(&mut last, &[mouse(Level::Percent(20), false)]);
        let mut now = vec![mouse(Level::Coarse("charging".into()), true)];
        fill_while_charging(&mut now, &last);
        assert_eq!((now[0].level.clone(), now[0].approx), (Level::Percent(20), true));
        // Estimates are remembered too, so they survive across polls and restarts.
        remember_levels(&mut last, &now);
        let mut later = vec![mouse(Level::Coarse("charging".into()), true)];
        fill_while_charging(&mut later, &last);
        assert_eq!(later[0].level, Level::Percent(20));
        // A real reading always wins.
        let mut real = vec![mouse(Level::Percent(35), true)];
        fill_while_charging(&mut real, &last);
        assert!(!real[0].approx);
    }
}