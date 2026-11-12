//! The reader process: `wireless-battery-tray.exe --reader`, started and owned by the bar.
//!
//! Device reads run here rather than in the bar, so a read that hangs inside Windows (Bluetooth
//! after resume from hibernate, a stale HID handle) can be ended for real: the bar kills this
//! process and starts a fresh one. A thread can't be killed safely; a process can.
//!
//! Protocol over stdin/stdout, one line each way: the bar writes `poll`, the reader answers with
//! one JSON [`Reply`]. The reader exits when stdin closes.

use std::io::{BufRead, Write};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Reading;

/// Longest one round waits for providers before answering without the slow ones.
pub const ROUND: Duration = Duration::from_secs(20);

#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub readings: Vec<Reading>,
    /// (provider, message)
    pub errors: Vec<(String, String)>,
    /// Seconds the longest still-running provider call has been running, if one is.
    pub stuck_secs: Option<u64>,
}

pub fn serve() {
    // Stuck calls are never abandoned here: the bar restarts the whole process instead.
    let mut providers = battery_core::Providers::new(providers::all()).restart_after(Duration::MAX);
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim() != "poll" {
            continue;
        }
        let snap = providers.poll_all(ROUND);
        let reply = Reply {
            readings: snap.batteries.into_iter().map(Reading::from).collect(),
            errors: snap.errors.into_iter().map(|(p, e)| (p.to_string(), e)).collect(),
            stuck_secs: providers.stuck_for().map(|d| d.as_secs()),
        };
        let sent = serde_json::to_writer(&mut out, &reply).is_ok() && writeln!(out).is_ok() && out.flush().is_ok();
        if !sent {
            break;
        }
    }
    // Ends any provider threads that are still stuck.
    std::process::exit(0);
}
