//! Background polling. Providers can take seconds (sleeping devices), so they never run on the
//! UI thread; results are handed over through a mutex and a posted window message.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::{save_snapshot, Reading};

/// Longest a round waits for providers. A sleeping Logitech device costs about a second per
/// request, so a healthy round stays well under this; a provider still running past it is
/// reported as an error and skipped until it returns.
const POLL_TIMEOUT: Duration = Duration::from_secs(20);

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
                let mut providers = battery_core::Providers::new(providers::all());
                let mut interval = Duration::from_secs(interval_secs.max(1));
                loop {
                    let started = Instant::now();
                    let snap = providers.poll_all(POLL_TIMEOUT);
                    let took = started.elapsed();
                    for (p, e) in &snap.errors {
                        eprintln!("poll: {p}: {e}");
                    }
                    let readings: Vec<Reading> = snap.batteries.into_iter().map(Reading::from).collect();
                    let _ = save_snapshot(&readings);
                    *shared.lock().unwrap() = Some(readings);
                    // SAFETY: posting to a window handle is safe even if the window is gone.
                    if unsafe { PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0)) }.is_err() {
                        return;
                    }
                    // Rest at least as long as the poll took, so a slow round (sleeping devices,
                    // BLE timeouts) at a short interval leaves the devices idle half the time
                    // instead of being queried back to back.
                    match rx.recv_timeout(interval.max(took)) {
                        Ok(Command::RefreshNow) | Err(RecvTimeoutError::Timeout) => {}
                        Ok(Command::SetInterval(s)) => interval = Duration::from_secs(s.max(1)),
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
