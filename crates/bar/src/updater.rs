//! Update checks against this repo's GitHub releases. Checks run on a background thread (30 s
//! after start, then daily, or on demand); installing only ever happens when the user asks.
//! The result of the last check is also written to `update.json` for the settings window.

use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::dir;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST_URL: &str = "https://api.github.com/repos/Faulik/batteries_app/releases/latest";
const EXE_ASSET: &str = "batteries-bar.exe";
const SUMS_ASSET: &str = "SHA256SUMS.txt";
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub page: String,
    exe_url: String,
    sums_url: String,
}

/// Last check, as shown in settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// Unix seconds.
    pub checked: Option<u64>,
    /// A newer release than this build, if there is one.
    pub newer: Option<Release>,
    pub error: Option<String>,
}

pub enum Command {
    CheckNow,
    SetEnabled(bool),
    Install,
}

/// What the UI thread is told after a check or an install attempt.
#[derive(Debug, Default)]
pub struct Outcome {
    pub newer: Option<Release>,
    /// `Some(Ok)` once the new exe is in place and started: the caller should quit.
    pub installed: Option<Result<(), String>>,
}

pub struct Updater {
    tx: Sender<Command>,
    outcome: Arc<Mutex<Outcome>>,
}

impl Updater {
    /// Posts `msg` to `hwnd` after every check and install attempt.
    pub fn start(hwnd: HWND, msg: u32, enabled: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let outcome = Arc::new(Mutex::new(Outcome::default()));
        let shared = outcome.clone();
        let hwnd = hwnd.0 as isize;
        // SAFETY: posting to a window handle is safe even if the window is gone.
        let notify = move || unsafe { PostMessageW(HWND(hwnd as _), msg, WPARAM(0), LPARAM(0)) }.is_ok();
        std::thread::Builder::new()
            .name("updater".into())
            .spawn(move || {
                let mut enabled = enabled;
                let mut next = Instant::now() + FIRST_CHECK;
                loop {
                    let forced = match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                        Ok(Command::CheckNow) => true,
                        Ok(Command::SetEnabled(e)) => {
                            enabled = e;
                            continue;
                        }
                        Ok(Command::Install) => {
                            let newer = shared.lock().unwrap().newer.clone();
                            let result = match &newer {
                                Some(r) => install(r).map_err(|e| format!("{e:#}")),
                                None => Err("no update available".into()),
                            };
                            if let Err(e) = &result {
                                let _ = save_status(&Status { checked: Some(now()), newer, error: Some(format!("Update failed: {e}")) });
                            }
                            shared.lock().unwrap().installed = Some(result);
                            if !notify() {
                                return;
                            }
                            continue;
                        }
                        Err(RecvTimeoutError::Timeout) => false,
                        Err(RecvTimeoutError::Disconnected) => return,
                    };
                    next = Instant::now() + INTERVAL;
                    if !forced && !enabled {
                        continue;
                    }
                    // Offline or rate limited: stay quiet and try again next time.
                    let status = match check() {
                        Ok(newer) => Status { checked: Some(now()), newer, error: None },
                        Err(e) => Status { checked: Some(now()), newer: None, error: Some(format!("Couldn't check: {e:#}")) },
                    };
                    let _ = save_status(&status);
                    shared.lock().unwrap().newer = status.newer;
                    if !notify() {
                        return;
                    }
                }
            })
            .expect("spawn updater thread");
        Self { tx, outcome }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn available(&self) -> Option<Release> {
        self.outcome.lock().unwrap().newer.clone()
    }

    pub fn take_installed(&self) -> Option<Result<(), String>> {
        self.outcome.lock().unwrap().installed.take()
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).user_agent(concat!("batteries-bar/", env!("CARGO_PKG_VERSION"))).build()
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// The latest published release, if it is newer than this build and has the files we need.
fn check() -> Result<Option<Release>> {
    let r: GhRelease = match agent().get(LATEST_URL).set("Accept", "application/vnd.github+json").call() {
        Ok(resp) => resp.into_json()?,
        // No releases yet.
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(ureq::Error::Status(403 | 429, _)) => bail!("GitHub rate limit reached, try again later"),
        Err(e) => return Err(e.into()),
    };
    if r.draft || r.prerelease || !is_newer(&r.tag_name, CURRENT) {
        return Ok(None);
    }
    let asset = |name: &str| r.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
    let (Some(exe_url), Some(sums_url)) = (asset(EXE_ASSET), asset(SUMS_ASSET)) else {
        return Ok(None);
    };
    Ok(Some(Release { version: r.tag_name.trim_start_matches('v').to_string(), page: r.html_url, exe_url, sums_url }))
}

/// `x.y.z` (optionally `v`-prefixed); pre-release versions (`1.2.0-rc1`) are never offered.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    if v.contains('-') {
        return None;
    }
    let mut parts = v.split('+').next()?.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().unwrap_or(Ok(0)).ok()?;
    let patch = parts.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    agent().get(url).call()?.into_reader().take(limit).read_to_end(&mut body)?;
    Ok(body)
}

/// The hash listed for `name` in a `sha256sum`-style file.
fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

/// Download and verify the new exe, swap it in for the running one and start it.
fn install(r: &Release) -> Result<()> {
    let sums = String::from_utf8(download(&r.sums_url, 64 * 1024)?).context("SHA256SUMS.txt")?;
    let want = expected_hash(&sums, EXE_ASSET).ok_or_else(|| anyhow!("{EXE_ASSET} is not listed in {SUMS_ASSET}"))?;
    let exe = download(&r.exe_url, 200 * 1024 * 1024)?;
    let got = Sha256::digest(&exe).iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    if got != want {
        bail!("downloaded file does not match its checksum");
    }

    let current = std::env::current_exe()?;
    // Same folder as the running exe, so the swap is a pair of renames.
    let new = current.with_extension("new");
    std::fs::write(&new, &exe).with_context(|| format!("can't write to {}", parent(&current).display()))?;
    let old = old_path(&current);
    let _ = std::fs::remove_file(&old);
    // Windows lets a running exe be renamed, not overwritten.
    std::fs::rename(&current, &old)?;
    if let Err(e) = std::fs::rename(&new, &current) {
        let _ = std::fs::rename(&old, &current);
        return Err(e.into());
    }
    std::process::Command::new(&current).arg("--updated").spawn().context("starting the new version")?;
    Ok(())
}

fn parent(p: &Path) -> &Path {
    p.parent().unwrap_or(p)
}

/// Where the previous exe is parked during an update; removed on the next start.
pub fn old_path(exe: &Path) -> PathBuf {
    exe.with_extension("old")
}

pub fn status_path() -> PathBuf {
    dir().join("update.json")
}

fn save_status(s: &Status) -> Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(status_path(), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}

pub fn load_status() -> Status {
    std::fs::read(status_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("v0.2.0-rc1", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn finds_hash_in_sums_file() {
        let sums = "AB12  batteries_app-0.1.0-windows-x64.zip\nCD34 *batteries-bar.exe\n";
        assert_eq!(expected_hash(sums, "batteries-bar.exe").as_deref(), Some("cd34"));
        assert_eq!(expected_hash(sums, "missing.exe"), None);
    }
}
