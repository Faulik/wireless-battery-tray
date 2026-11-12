//! Background polling. Device reads run in a separate reader process (see `reader`), driven
//! from a thread here so the UI never waits; results are handed over through a mutex and a
//! posted window message. A reader that stops answering, or has a call stuck for a minute, is
//! killed and replaced.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::{save_snapshot, Reading};
use crate::reader::{Reply, ROUND};
use crate::updater::log;

/// How long to wait for the reader's answer. It answers within `ROUND` even with a stuck
/// provider, so missing this means the process itself is wedged.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(ROUND.as_secs() + 15);
/// A provider call running this long is treated as hung for good: the reader is restarted.
const STUCK_LIMIT: Duration = Duration::from_secs(60);

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
                let mut reader: Option<ReaderProcess> = None;
                let mut interval = Duration::from_secs(interval_secs.max(1));
                loop {
                    let started = Instant::now();
                    match poll_once(&mut reader) {
                        Ok(reply) => {
                            for (p, e) in &reply.errors {
                                eprintln!("poll: {p}: {e}");
                            }
                            let _ = save_snapshot(&reply.readings);
                            *shared.lock().unwrap() = Some(reply.readings);
                            // SAFETY: posting to a window handle is safe even if the window is gone.
                            if unsafe { PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0)) }.is_err() {
                                return;
                            }
                        }
                        // The bar keeps showing the last good readings; the next round starts a new reader.
                        Err(e) => log(format!("reader: {e:#}")),
                    }
                    let took = started.elapsed();
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

/// One round through the reader, starting it if needed. On any failure the reader is dropped
/// (killed), so the next round gets a fresh one.
fn poll_once(reader: &mut Option<ReaderProcess>) -> Result<Reply> {
    if reader.is_none() {
        *reader = Some(ReaderProcess::spawn().context("start reader")?);
    }
    let result = reader.as_mut().expect("just set").poll();
    match &result {
        Ok(reply) if reply.stuck_secs.is_some_and(|s| s >= STUCK_LIMIT.as_secs()) => {
            log(format!("reader: a device call has been stuck for {} s; restarting the reader", reply.stuck_secs.unwrap_or(0)));
            *reader = None;
        }
        Ok(_) => {}
        Err(_) => *reader = None,
    }
    result
}

struct ReaderProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl ReaderProcess {
    fn spawn() -> Result<Self> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let exe = std::env::current_exe()?;
        let mut child = std::process::Command::new(exe)
            .arg("--reader")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        end_with_this_process(&child);
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let (tx, lines) = mpsc::channel();
        // Ends on its own when the reader exits or is killed (stdout closes).
        std::thread::Builder::new().name("reader-out".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        })?;
        Ok(Self { child, stdin, lines })
    }

    fn poll(&mut self) -> Result<Reply> {
        writeln!(self.stdin, "poll").and_then(|_| self.stdin.flush()).context("reader exited")?;
        let line = match self.lines.recv_timeout(ANSWER_TIMEOUT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => return Err(anyhow!("no answer within {ANSWER_TIMEOUT:?}; restarting it")),
            Err(RecvTimeoutError::Disconnected) => return Err(anyhow!("reader exited")),
        };
        serde_json::from_str(&line).context("bad reply from reader")
    }
}

impl Drop for ReaderProcess {
    fn drop(&mut self) {
        // Not waited for: a process stuck in a driver call can take a moment to go away.
        let _ = self.child.kill();
    }
}

/// Puts the reader in a job that Windows closes, killing the reader, when the bar exits for any
/// reason (crash, update, task manager), so a stuck reader is never left behind.
fn end_with_this_process(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    static JOB: OnceLock<Option<isize>> = OnceLock::new();
    // SAFETY: plain job-object calls; the job handle is kept open for the life of the process
    // on purpose, and `info` outlives the call that reads it.
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size).ok()?;
        Some(job.0 as isize)
    });
    if let Some(job) = job {
        // SAFETY: both handles are valid: ours is kept open, the child's is owned by `child`.
        let _ = unsafe { AssignProcessToJobObject(HANDLE(*job as _), HANDLE(child.as_raw_handle())) };
    }
}
